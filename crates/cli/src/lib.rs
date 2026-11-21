//! `misterdownloadmanager-cli`'s commands. The app's program carries them (#25: one download
//! engine shipped, not two): `misterdownloadmanager-cli` (main.rs) is a small launcher that runs
//! `misterdownloadmanager --cli <arguments>`, and the app's `main` hands those to [`main`].

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, Subcommand};
use mdm_engine::download::{ConnStatus, MAX_CONNECTIONS_UNLOCKED, State};
use mdm_engine::{Destination, Download, Options, Proxy, Request};
use reqwest::header::{COOKIE, HeaderMap, HeaderName, HeaderValue, REFERER, USER_AGENT};
use serde_json::json;

#[derive(Parser)]
#[command(
    version,
    about = "Mister Download Manager command line",
    long_about = "Mister Download Manager command line: downloads, videos and torrents without the \
                  window, and the running app's automation API. Guide: \
                  https://misterdownloadmanager.com/help/command-line.html"
)]
struct Cli {
    /// Print JSON for programs: one object per line (`progress` events, then `done`, `paused`
    /// or `stopped`, for a download; one object for a question), `{"error": …}` on failure.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
// Parsed once per run: `get`'s options (a DNS and a proxy, #1556) outweigh the others' and
// nothing is gained by boxing them.
#[allow(clippy::large_enum_variant)]
enum Cmd {
    /// Download a URL. Ctrl+C pauses; continue later with `resume`.
    Get {
        /// The address to download (http or https).
        url: url::Url,
        /// Folder to save into (name from the server), or a file path ending in a name.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Maximum connections (1 to 128).
        #[arg(short, long, default_value_t = Options::default().max_connections,
              value_parser = clap::value_parser!(u32).range(1..=MAX_CONNECTIONS_UNLOCKED as i64))]
        connections: u32,
        /// Extra request header, e.g. "Referer: https://example.com/" (repeatable).
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// The page's address (Referer header), for links that need one or an expired link's renewal.
        #[arg(short = 'r', long = "referer", alias = "referrer")]
        referer: Option<String>,
        /// The User-Agent header to send with download requests.
        #[arg(long = "user-agent", alias = "ua")]
        user_agent: Option<String>,
        /// The Cookie header to send with download requests. A cookie on the command line ends
        /// up in the shell's history; "-" reads it from standard input instead.
        #[arg(long = "cookie", alias = "cookies")]
        cookie: Option<String>,
        /// The finished file must have this hash: hex MD5 / SHA-1 / SHA-256 / SHA-512 / CRC32,
        /// optionally named ("sha256:…").
        #[arg(long, value_parser = parse_checksum)]
        checksum: Option<mdm_engine::verify::Hash>,
        /// The DNS that looks the names up, instead of the app's Options → Connection → DNS:
        /// "system"; a service and how to ask it, "cloudflare-https", "quad9-tls",
        /// "google-plain", "adguard-https", "nextdns-tls"; DNS servers' addresses,
        /// "9.9.9.9,149.112.112.112"; or a DNS-over-HTTPS address,
        /// "https://dns.example/dns-query". No falling back to the system's DNS.
        #[arg(long, value_parser = parse_dns)]
        dns: Option<mdm_engine::DnsChoice>,
        /// The network adapter every connection goes through (#35), instead of the app's Options
        /// → Proxy / Socks → Network adapter: its name ("Ethernet", "wg0") or one of its
        /// addresses ("any": none). When it isn't there or has no address, nothing is sent and
        /// the download fails.
        #[arg(long)]
        adapter: Option<String>,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
        /// Download the address itself, never through a debrid account. Without it, a link
        /// from a file host that an account in the app's Options → Accounts takes goes through
        /// that account, as in the app.
        #[arg(long)]
        direct: bool,
    },
    /// The running app's downloads, as JSON (automation API; turn it on with `api on`).
    #[command(alias = "list")]
    Status {
        /// Only the download with this id.
        #[arg(long)]
        id: Option<u64>,
        /// Only downloads with this status: Queued, Downloading, Stopped, Complete or Error.
        #[arg(short = 's', long)]
        status: Option<String>,
        /// Only downloads whose file name or address contains this text (any case).
        #[arg(long)]
        search: Option<String>,
        /// A category for the download (e.g. "Compressed", "Documents", "Music", "Programs", "Video").
        #[arg(short = 'c', long)]
        category: Option<String>,
        /// The queue to list (by name; e.g. "Main download queue").
        #[arg(short = 'q', long)]
        queue: Option<String>,
        /// At most this many, newest first.
        #[arg(short = 'l', long)]
        limit: Option<usize>,
    },
    /// Install (or update) yt-dlp for video downloads: its checksum list is verified against
    /// yt-dlp's signing key, the program against that list.
    InstallYtdlp {
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Install (or update) ffmpeg, for the video qualities that merge video and audio: BtbN's
    /// LGPL build, checked against its release's SHA-256 list (x64 and ARM64 Windows; elsewhere
    /// the system's, and on 32-bit Windows one chosen in Helpers).
    InstallFfmpeg {
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Install (or update) gallery-dl for image galleries: its checksum list is verified against
    /// its author's signing key, the program against that list (Windows and Linux x86-64).
    InstallGalleryDl {
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Install (or update) MDM's own JavaScript runtime, which yt-dlp needs for YouTube's
    /// qualities: Node.js's newest LTS official build, checked against its release's checksums
    /// signed by Node.js's releasers (Windows, Linux and macOS). A runtime already on the
    /// computer (Deno, Node.js 22 or newer, QuickJS, Bun) or one chosen in Helpers is still used
    /// first. The Deno earlier versions of MDM installed is removed once Node.js is in place.
    /// `install-deno` is its earlier name.
    #[command(alias = "install-deno")]
    InstallJsRuntime {
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Check MDM's own helpers (yt-dlp, ffmpeg, gallery-dl, Node.js) for newer versions now and
    /// install them, each checked as its first install was, and print their versions. A helper a
    /// download is using is switched to its new version once that download ends. Programs you
    /// chose in Helpers, and the system's, are yours to update.
    UpdateHelpers {
        /// The proxy for this run, instead of the app's Options → Proxy / Socks (as for the
        /// install commands).
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Download an image gallery's files with gallery-dl (needs `install-gallery-dl`).
    GalleryGet {
        /// The gallery's address (a Wikimedia Commons category, a Flickr album, ...).
        url: String,
        /// Folder to save into: a folder named after the gallery is made there.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// At most this many files (the first the site lists).
        #[arg(long)]
        limit: Option<u64>,
        /// The network adapter gallery-dl goes through (#35), as `get --adapter`.
        #[arg(long)]
        adapter: Option<String>,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// What video a page holds, and the choices to download (needs `install-ytdlp`).
    VideoInfo {
        /// The page's address (YouTube, Vimeo, ...).
        url: String,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Download a video with yt-dlp (needs `install-ytdlp`).
    VideoGet {
        /// The page's address.
        url: String,
        /// Which quality: a choice's selector from `video-info` (e.g. "b[height=360]"); "b" is
        /// the best single file; "ba" the sound alone as the site has it (not converted);
        /// "mp3:320", "mp3:256", "mp3:192" or "mp3:128" the sound as an MP3 at that bitrate,
        /// "mp3:0" at its best variable one; "opus:" as Opus; "flac:" as FLAC (lossless, but it
        /// can't add quality a lossy source doesn't have). Conversions need ffmpeg. Without it
        /// (and `--quality`), the best, as the Download video window starts on: the best picture
        /// with the best sound joined to it (with ffmpeg), else the best file with both.
        #[arg(short, long, conflicts_with = "quality")]
        format: Option<String>,
        /// The video's height ("1080", "720p"): the best of that height, else of the nearest
        /// below it the page offers (else the nearest above), as `video-info` lists them.
        /// "compatible": the most compatible, as the Download video window's "Most compatible
        /// (H.264 + AAC)": the best H.264 picture with AAC sound, in an MP4 (never converted).
        #[arg(short, long, value_parser = parse_quality)]
        quality: Option<Quality>,
        /// Folder to save into.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Subtitles to save beside the video: language codes from `video-info`, separated by
        /// commas ("en,fr"), "all" for every one the uploader gave, or "none".
        #[arg(long, default_value = "none")]
        subs: String,
        /// Take the subtitles from the site's automatic captions instead of the uploader's.
        #[arg(long)]
        auto_subs: bool,
        /// Put the subtitles into the video file instead of beside it (needs ffmpeg; MP4/MKV).
        #[arg(long)]
        embed_subs: bool,
        /// Connections per stream (1 to 128), as `get`'s: MDM's engine downloads the streams
        /// yt-dlp finds (#769), one in pieces (HLS, DASH) this many pieces at once.
        #[arg(short, long, default_value_t = Options::default().max_connections,
              value_parser = clap::value_parser!(u32).range(1..=MAX_CONNECTIONS_UNLOCKED as i64))]
        connections: u32,
        /// The network adapter the downloads go through (#35), as `get --adapter`: the
        /// engine's connections, and yt-dlp's (and the ffmpeg it starts).
        #[arg(long)]
        adapter: Option<String>,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// The videos of a playlist or a channel, listed as they come (needs `install-ytdlp`).
    PlaylistInfo {
        /// The playlist's or channel's address.
        url: String,
        /// The first entry (from 1).
        #[arg(long, default_value_t = 1)]
        from: u64,
        /// How many entries at most (none: to the end of the list).
        #[arg(long)]
        count: Option<u64>,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Download a playlist's or channel's videos one after another (needs `install-ytdlp`).
    PlaylistGet {
        /// The playlist's or channel's address.
        url: String,
        /// One quality for all: a selector such as "bv*[height<=720]+ba/b[height<=720]", "ba",
        /// "mp3:0" (as for `video-get`); by default each video's best.
        #[arg(short, long, default_value = "bv*+ba/b")]
        format: String,
        /// Folder to save into: a folder named after the list is made there.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Connections per stream (1 to 128), as for `video-get`.
        #[arg(short, long, default_value_t = Options::default().max_connections,
              value_parser = clap::value_parser!(u32).range(1..=MAX_CONNECTIONS_UNLOCKED as i64))]
        connections: u32,
        /// The first entry (from 1).
        #[arg(long, default_value_t = 1)]
        from: u64,
        /// How many entries at most (none: to the end of the list).
        #[arg(long)]
        count: Option<u64>,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
    /// Make a .torrent of a file or folder (no trackers; peers find it through the DHT).
    TorrentCreate {
        /// The file or folder to share.
        path: PathBuf,
        /// The .torrent file to write.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// What a torrent holds: its name, files (numbered) and size.
    TorrentInfo {
        /// A magnet link or a .torrent file.
        source: String,
    },
    /// Download a torrent.
    TorrentGet {
        /// A magnet link or a .torrent file.
        source: String,
        /// Folder to save into.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Only these files, by their numbers in `torrent-info` (e.g. 0,2); all by default.
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<usize>>,
        /// Stop after this many seconds (a test run).
        #[arg(long)]
        seconds: Option<u64>,
        /// Use the torrent's files already in the folder: what matches the torrent is kept, the
        /// rest downloaded into them. Without it, such files stop the download.
        #[arg(long)]
        use_existing: bool,
    },
    /// Pause downloads in the running app, by their ids from `status` (automation API).
    Pause {
        /// Download ids.
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    /// Start (or resume) downloads in the running app, by id (automation API).
    Start {
        /// Download ids.
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    /// Take downloads off the running app's list, keeping their files (automation API).
    Remove {
        /// Download ids.
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    /// Take downloads off the running app's list and delete their files (automation API).
    Delete {
        /// Download ids.
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    /// Add a download to the running app.
    Add {
        /// The address to download (http, https, ftp, ftps, or a magnet link).
        url: String,
        /// Folder to save into.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// File name to save as.
        #[arg(short = 'f', long = "name")]
        name: Option<String>,
        /// A category for the download (e.g. "Compressed", "Documents", "Music", "Programs", "Video").
        #[arg(short = 'c', long = "category")]
        category: Option<String>,
        /// The queue to add to (by name; adds to the main queue with `--later` if omitted).
        #[arg(short = 'q', long = "queue")]
        queue: Option<String>,
        /// The finished file must have this hash: hex MD5 / SHA-1 / SHA-256 / SHA-512 / CRC32,
        /// optionally named ("sha256:…").
        #[arg(long)]
        checksum: Option<String>,
        /// The page's address (Referer header), for links that need one or an expired link's renewal.
        #[arg(short = 'r', long = "referer", alias = "referrer")]
        referer: Option<String>,
        /// The User-Agent header to send with download requests.
        #[arg(long = "user-agent", alias = "ua")]
        user_agent: Option<String>,
        /// The Cookie header to send with download requests. A cookie on the command line ends
        /// up in the shell's history; "-" reads it from standard input instead.
        #[arg(long = "cookie", alias = "cookies")]
        cookie: Option<String>,
        /// Add to the queue without starting it now.
        #[arg(short = 'l', long = "later")]
        later: bool,
        /// Silent: do not show the Download File Info dialog in the app.
        #[arg(short = 's', long = "silent")]
        silent: bool,
        /// Private: do not save the download to the app's history or saved list.
        #[arg(short = 'p', long = "private")]
        private: bool,
    },
    /// Start a download queue in the running app (automation API). Starts the main queue by default.
    StartQueue {
        /// The queue to start (by name); starts the main queue if omitted.
        #[arg(short, long)]
        queue: Option<String>,
        /// The queue to start (by name), if not given with `--queue`.
        #[arg(value_name = "QUEUE", conflicts_with = "queue")]
        name: Option<String>,
    },
    /// Stop a download queue in the running app (automation API). Stops the main queue by default.
    StopQueue {
        /// The queue to stop (by name); stops the main queue if omitted.
        #[arg(short, long)]
        queue: Option<String>,
        /// The queue to stop (by name), if not given with `--queue`.
        #[arg(value_name = "QUEUE", conflicts_with = "queue")]
        name: Option<String>,
    },
    /// Show a new code that pairs a phone or a browser with the running app's remote control
    /// (#141, #157): its address, the code and a QR code to scan.
    Pair,
    /// Turn the running app's automation API on or off (off by default).
    Api {
        /// on or off.
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Continue a paused or interrupted download from its `.mdm.json` state file.
    Resume {
        /// The `<file>.mdm.json` next to the unfinished download.
        state: PathBuf,
        /// Maximum connections (1 to 128).
        #[arg(short, long, default_value_t = Options::default().max_connections,
              value_parser = clap::value_parser!(u32).range(1..=MAX_CONNECTIONS_UNLOCKED as i64))]
        connections: u32,
        /// Extra request header, e.g. "Cookie: id=1" (repeatable).
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// The page's address (Referer header), for links that need one or an expired link's renewal.
        #[arg(short = 'r', long = "referer", alias = "referrer")]
        referer: Option<String>,
        /// The User-Agent header to send with download requests.
        #[arg(long = "user-agent", alias = "ua")]
        user_agent: Option<String>,
        /// The Cookie header to send with download requests. A cookie on the command line ends
        /// up in the shell's history; "-" reads it from standard input instead.
        #[arg(long = "cookie", alias = "cookies")]
        cookie: Option<String>,
        /// The finished file must have this hash: hex MD5 / SHA-1 / SHA-256 / SHA-512 / CRC32,
        /// optionally named ("sha256:…").
        #[arg(long, value_parser = parse_checksum)]
        checksum: Option<mdm_engine::verify::Hash>,
        /// The DNS that looks the names up, instead of the app's Options → Connection → DNS:
        /// "system"; a service and how to ask it, "cloudflare-https", "quad9-tls",
        /// "google-plain", "adguard-https", "nextdns-tls"; DNS servers' addresses,
        /// "9.9.9.9,149.112.112.112"; or a DNS-over-HTTPS address,
        /// "https://dns.example/dns-query". No falling back to the system's DNS.
        #[arg(long, value_parser = parse_dns)]
        dns: Option<mdm_engine::DnsChoice>,
        /// The network adapter every connection goes through (#35): its name ("Ethernet",
        /// "wg0") or one of its addresses. When it isn't there or has no address, nothing is
        /// sent and the download fails.
        #[arg(long)]
        adapter: Option<String>,
        /// Continue from the address itself, never through a debrid account (see `get`).
        #[arg(long)]
        direct: bool,
        /// The proxy for this run, instead of the app's Options → Proxy / Socks: "none", "system"
        /// (the HTTPS_PROXY / HTTP_PROXY / ALL_PROXY variables, else the system's settings), or
        /// an address: http://host:port, https://, socks5://, socks5h:// (the proxy looks the
        /// names up), socks4://, socks4a://. A login: the user name in the address
        /// (socks5://ann@host:1080) and the password in MDM_PROXY_PASSWORD, never on the command
        /// line.
        #[arg(long, value_parser = parse_proxy)]
        proxy: Option<Proxy>,
    },
}

/// What [`Accounts::unlocker`] gives: the unlocker, or why the services couldn't say.
pub type Unlocking<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<Option<std::sync::Arc<dyn mdm_engine::Unlocker>>, String>,
            > + Send
            + 'a,
    >,
>;

/// The debrid accounts the command line downloads through (#44): the ones in the app's Options
/// → Accounts. The app keeps them in the system's keychain, which this library doesn't read: the
/// app's program hands them over ([`main_with`]).
pub trait Accounts: Send + Sync {
    /// The unlocker for `url`: the first account, in the user's order, whose service takes its
    /// host, the services asked through `net` (the download's own DNS, proxy and adapter).
    /// `Ok(None)`: no account takes it, or there is none. `Err`: none of the services that
    /// answered takes it, and one didn't say which hosts it takes (in English).
    fn unlocker<'a>(&'a self, url: &'a url::Url, net: &'a Options) -> Unlocking<'a>;
}

/// `opts` with the unlocker for `url` (#44), unless `direct` or there are no accounts; says
/// which service the download goes through.
async fn through_account(
    accounts: Option<&dyn Accounts>,
    url: &url::Url,
    direct: bool,
    mut opts: Options,
    json: bool,
) -> anyhow::Result<Options> {
    let Some(accounts) = accounts.filter(|_| !direct) else {
        return Ok(opts);
    };
    // As the app: a service that can't say which hosts it takes isn't asked; the address is
    // downloaded itself (said on stderr, not in the JSON output).
    let unlocker = accounts.unlocker(url, &opts).await.unwrap_or_else(|e| {
        eprintln!("warning: {e}; downloading the address itself");
        None
    });
    if let Some(u) = &unlocker {
        if json {
            emit(json!({ "event": "through", "service": u.name() }));
        } else {
            println!("through {}", u.name());
        }
    }
    opts.unlocker = unlocker;
    Ok(opts)
}

/// One JSON line on stdout (`--json`).
/// `update-helpers`: each of MDM's own helpers checked now and updated, one line (or JSON object)
/// each. Fails when a check failed (the others are still checked).
async fn update_helpers(json: bool) -> anyhow::Result<()> {
    use mdm_tools::autoupdate::{Checked, Tool, Why};
    let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
    let net = own_net();
    let mut failed = 0;
    for tool in Tool::ALL {
        let name = tool.name();
        let got = mdm_tools::autoupdate::check(
            &dir,
            tool,
            &net,
            Why::Asked,
            mdm_tools::autoupdate::unix_now(),
        )
        .await;
        let (status, version, line) = match &got {
            Ok(Checked::NotOurs) => (
                "not-ours",
                None,
                format!(
                    "{name}: not MDM's own copy (yours or the system's to update, or not installed)"
                ),
            ),
            Ok(Checked::UpToDate { now, .. }) => (
                "up-to-date",
                Some(now.version.clone()),
                format!("{name} {}: up to date", now.version),
            ),
            Ok(Checked::Updated { from, now }) => (
                "updated",
                Some(now.version.clone()),
                format!("{name} {from} -> {}: verified and installed", now.version),
            ),
            Ok(Checked::Ready { now, ready }) => (
                "ready",
                Some(ready.version.clone()),
                format!(
                    "{name} {} -> {}: verified, used once the downloads using {name} end",
                    now.version, ready.version
                ),
            ),
            Ok(Checked::Skipped { now, latest }) => (
                "skipped",
                Some(now.version.clone()),
                format!(
                    "{name} {}: {latest} left out (you went back from it)",
                    now.version
                ),
            ),
            Ok(Checked::Busy | Checked::NotDue) => {
                ("busy", None, format!("{name}: being updated already"))
            }
            Err(e) => {
                failed += 1;
                ("failed", None, format!("{name}: couldn't check: {e}"))
            }
        };
        if json {
            let mut v = json!({ "tool": name, "status": status, "version": version });
            if let Err(e) = &got {
                v["error"] = json!(e.to_string());
            }
            emit(v);
        } else {
            println!("{line}");
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} helper check(s) failed");
    }
    Ok(())
}

fn emit(v: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// A failure already printed (the app's own `{"error"}` answer): exit 1, print nothing more.
#[derive(Debug)]
struct Reported;

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("reported")
    }
}

impl std::error::Error for Reported {}

/// Ask the running app (automation API) and print its answer, which is JSON already; an
/// `{"error"}` answer fails the command.
fn ask(cmd: mdm_ipc::Command) -> anyhow::Result<()> {
    let answer = mdm_ipc::request(&cmd)
        .map_err(|e| anyhow::anyhow!("Mister Download Manager isn't running ({e})"))?;
    println!("{answer}");
    if serde_json::from_str::<mdm_ipc::api::ErrorAnswer>(&answer).is_ok() {
        return Err(Reported.into());
    }
    Ok(())
}

fn status_command(
    id: Option<u64>,
    status: Option<String>,
    search: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let answer = mdm_ipc::request(&mdm_ipc::Command {
        query: Some("list".into()),
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("Mister Download Manager isn't running ({e})"))?;

    if serde_json::from_str::<mdm_ipc::api::ErrorAnswer>(&answer).is_ok() {
        println!("{answer}");
        return Err(Reported.into());
    }

    if id.is_none()
        && status.is_none()
        && search.is_none()
        && category.is_none()
        && queue.is_none()
        && limit.is_none()
    {
        println!("{answer}");
        return Ok(());
    }

    let list: mdm_ipc::api::ListAnswer =
        serde_json::from_str(&answer).map_err(|e| anyhow::anyhow!("unreadable answer: {e}"))?;

    let filter = mdm_ipc::api::ListFilter {
        id,
        status: status.as_deref(),
        search: search.as_deref(),
        category: category.as_deref(),
        queue: queue.as_deref(),
        limit,
    };

    let filtered = list.filtered(&filter).map_err(|e| anyhow::anyhow!("{e}"))?;

    let output = serde_json::to_string(&filtered)?;
    println!("{output}");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_command(
    url: String,
    output: Option<PathBuf>,
    name: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    checksum: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
    later: bool,
    silent: bool,
    private: bool,
) -> anyhow::Result<mdm_ipc::Command> {
    add_command_with(
        url,
        output,
        name,
        category,
        queue,
        checksum,
        referer,
        user_agent,
        cookie,
        later,
        silent,
        private,
        std::io::stdin(),
    )
}

fn read_cookie(cookie: Option<String>, mut stdin: impl Read) -> anyhow::Result<Option<String>> {
    match cookie.as_deref() {
        Some("-") => {
            let mut buf = String::new();
            stdin
                .read_to_string(&mut buf)
                .context("reading cookie from standard input")?;
            let trimmed = buf.trim().to_string();
            Ok((!trimmed.is_empty()).then_some(trimmed))
        }
        _ => Ok(cookie),
    }
}

#[allow(clippy::too_many_arguments)]
fn add_command_with(
    url: String,
    output: Option<PathBuf>,
    name: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    checksum: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
    later: bool,
    silent: bool,
    private: bool,
    stdin: impl Read,
) -> anyhow::Result<mdm_ipc::Command> {
    let dir = output.map(|p| {
        std::path::absolute(&p)
            .unwrap_or(p)
            .to_string_lossy()
            .to_string()
    });
    let cookies = read_cookie(cookie, stdin)?;
    Ok(mdm_ipc::Command {
        url: Some(url),
        dir,
        name,
        category,
        queue,
        checksum,
        referer,
        user_agent,
        cookies,
        later,
        silent,
        private,
        ..Default::default()
    })
}

/// `pair`: a new pairing code from the running app, printed for a person (or raw JSON with `--json`).
fn pair(json: bool) -> anyhow::Result<()> {
    if json {
        return ask(mdm_ipc::Command {
            pair: true,
            ..Default::default()
        });
    }
    let answer = mdm_ipc::request(&mdm_ipc::Command {
        pair: true,
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("Mister Download Manager isn't running ({e})"))?;
    match mdm_ipc::api::parse::<mdm_ipc::api::PairAnswer>(&answer).map_err(anyhow::Error::msg)? {
        Ok(a) => {
            println!("{}", a.qr);
            println!(
                "Open {} on the phone (or scan the code above),",
                a.urls.join("  or  ")
            );
            println!(
                "then enter the code {} {} (it works once, for {} minutes).",
                a.code.get(..4).unwrap_or_default(),
                a.code.get(4..).unwrap_or_default(),
                a.minutes
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("{}", e.error);
            Err(Reported.into())
        }
    }
}

/// An automation API action on the running app's downloads; prints its answer.
fn act(action: &str, ids: Vec<u64>) -> anyhow::Result<()> {
    ask(mdm_ipc::Command {
        action: Some(action.into()),
        ids,
        ..Default::default()
    })
}

/// The first argument the launcher starts the app's program with: run these commands, not the
/// window. On Windows the launcher adds its process id (`--cli=1234`), see [`Launch`].
pub const FLAG: &str = "--cli";

/// #852: what a YouTube look-up yt-dlp made without a JavaScript runtime says (its list is short,
/// on some days only the sound), as the Download video window says it, with the help page's
/// section on installing one.
fn js_runtime_note() -> String {
    "YouTube needs a JavaScript runtime to list its video qualities: install Node.js or Deno, \
     then look it up again (https://misterdownloadmanager.com/help/video.html#javascript-runtime)"
        .into()
}

/// The launcher's name, which the commands' help and messages show.
pub const NAME: &str = "misterdownloadmanager-cli";

/// The app's program, started for the command line.
pub struct Launch {
    /// Windows: the launcher's process id, `--cli=<id>`. The program ends if the launcher is
    /// killed, as one process would: a script that kills it (a time limit) doesn't leave a
    /// download going on unseen.
    pub launcher: Option<u32>,
    /// The command line's arguments, without the program's name.
    pub args: Vec<OsString>,
}

/// Was the app's program started for the command line? `args` are its own (`std::env::args_os`):
/// [`FLAG`] first, or on Linux and macOS a program named [`NAME`] (a link to the app's program
/// works as the launcher there; on Windows the app is a windowed program, which a console
/// doesn't wait for, so it needs the launcher).
pub fn launched(args: Vec<OsString>) -> Option<Launch> {
    let mut args = args.into_iter();
    let program = args.next()?;
    let rest: Vec<OsString> = args.collect();
    if let Some(first) = rest.first().and_then(|a| a.to_str()) {
        let launcher = if first == FLAG {
            Some(None)
        } else {
            first
                .strip_prefix(FLAG)
                .and_then(|r| r.strip_prefix('='))
                .and_then(|id| id.parse().ok())
                .map(Some)
        };
        if let Some(launcher) = launcher {
            return Some(Launch {
                launcher,
                args: rest[1..].to_vec(),
            });
        }
    }
    let by_name = std::path::Path::new(&program)
        .file_name()
        .is_some_and(|n| n == NAME);
    (cfg!(unix) && by_name).then_some(Launch {
        launcher: None,
        args: rest,
    })
}

/// Run the command line `args` (without the program's name): what `misterdownloadmanager-cli`
/// does, its output, and its exit code (1 when a command fails).
pub fn main(args: Vec<OsString>) -> ExitCode {
    main_with(args, None, Net::default())
}

/// How the command line connects (#1556): the app's Options → Proxy / Socks, Connection → DNS
/// and network adapter, and Torrent settings' proxy, as the app's program reads them from its
/// settings ([`main_with`]). A command's `--proxy`, `--dns` and `--adapter` take their place for
/// that run. The default, without the app's settings: straight to the servers, the system's DNS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    pub proxy: Proxy,
    pub dns: mdm_engine::DnsChoice,
    /// The network adapter by name ("" any), looked for when the command starts.
    pub adapter: String,
    pub torrent_proxy: TorrentProxy,
}

/// Torrent settings' "Use the proxy" (#40), for `torrent-info` and `torrent-get`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TorrentProxy {
    /// Off: torrents connect directly.
    #[default]
    Off,
    /// Through this SOCKS5 proxy (`socks5://…`, with its login).
    Through(String),
    /// On, but Options → Proxy / Socks has no SOCKS5 proxy: torrents don't start, as in the app.
    Missing,
}

/// How this command connects: [`Net`] with the command's own `--proxy`, `--dns`, `--adapter`.
#[derive(Clone, Debug, Default)]
struct Conn {
    proxy: Proxy,
    dns: mdm_engine::DnsChoice,
    route: mdm_engine::Route,
    torrent_proxy: TorrentProxy,
}

static CONN: std::sync::RwLock<Option<Conn>> = std::sync::RwLock::new(None);

/// This command's connection ([`Conn`]); before it is set (the unit tests), direct.
fn conn() -> Conn {
    CONN.read().unwrap().clone().unwrap_or_default()
}

impl Conn {
    /// `net` with `cmd`'s own choices.
    fn of(cmd: &Cmd, net: &Net) -> Conn {
        let (proxy, dns, adapter) = match cmd {
            Cmd::Get {
                proxy,
                dns,
                adapter,
                ..
            }
            | Cmd::Resume {
                proxy,
                dns,
                adapter,
                ..
            } => (proxy, dns.as_ref(), adapter.as_deref()),
            Cmd::GalleryGet { proxy, adapter, .. } | Cmd::VideoGet { proxy, adapter, .. } => {
                (proxy, None, adapter.as_deref())
            }
            Cmd::VideoInfo { proxy, .. }
            | Cmd::PlaylistInfo { proxy, .. }
            | Cmd::PlaylistGet { proxy, .. }
            | Cmd::InstallYtdlp { proxy }
            | Cmd::InstallFfmpeg { proxy }
            | Cmd::InstallGalleryDl { proxy }
            | Cmd::InstallJsRuntime { proxy }
            | Cmd::UpdateHelpers { proxy } => (proxy, None, None),
            _ => (&None, None, None),
        };
        let adapter = adapter.unwrap_or(net.adapter.as_str()).trim();
        // Worked out once, as the app does each second (a missing adapter: Down).
        let route = if adapter.is_empty() || adapter.eq_ignore_ascii_case("any") {
            mdm_engine::Route::Any
        } else {
            mdm_engine::net::route_for(adapter, &mdm_engine::net::adapters())
        };
        Conn {
            proxy: proxy.clone().unwrap_or_else(|| net.proxy.clone()),
            dns: dns.cloned().unwrap_or_else(|| net.dns.clone()),
            route,
            torrent_proxy: net.torrent_proxy.clone(),
        }
    }

    /// This connection for every download, look-up and helper of the command: yt-dlp and
    /// gallery-dl take the proxy, DNS and adapter from `mdm_tools::proc`.
    fn set(self) {
        mdm_tools::proc::set_proxy(self.proxy.clone());
        mdm_tools::proc::set_dns(self.dns.clone());
        mdm_tools::proc::set_route(self.route.clone());
        *CONN.write().unwrap() = Some(self);
    }
}

/// MDM's own downloads in this command (the helpers' installs): its engine, as itself, through
/// this command's connection.
fn own_net() -> Options {
    let c = conn();
    Options {
        dns: c.dns,
        route: c.route,
        ..mdm_tools::net(c.proxy)
    }
}

/// A look-up's options (a page read for its video, #1076): this command's connection.
fn net_opts() -> Options {
    let c = conn();
    Options {
        proxy: c.proxy,
        dns: c.dns,
        route: c.route,
        ..Options::default()
    }
}

/// `--proxy`: see [`Cmd::Get`]. A password in the address is refused: other programs can read a
/// command line.
fn parse_proxy(s: &str) -> Result<Proxy, String> {
    proxy_arg(s, std::env::var("MDM_PROXY_PASSWORD").ok())
}

fn proxy_arg(s: &str, password: Option<String>) -> Result<Proxy, String> {
    let s = s.trim();
    match s.to_ascii_lowercase().as_str() {
        "none" | "direct" => return Ok(Proxy::None),
        "system" => return Ok(Proxy::System),
        _ => {}
    }
    let typed = if s.contains("://") {
        s.to_owned()
    } else {
        format!("http://{s}")
    };
    let mut u = url::Url::parse(&typed).map_err(|e| format!("{s:?}: {e}"))?;
    let scheme = u.scheme().to_owned();
    if !matches!(
        scheme.as_str(),
        "http" | "https" | "socks5" | "socks5h" | "socks4" | "socks4a"
    ) {
        return Err(format!(
            "{scheme}:// is not a proxy's: none, system, or http://, https://, socks5://, socks5h://, socks4://, socks4a://"
        ));
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err(format!("{s:?}: no proxy server in it"));
    }
    if u.password().is_some() {
        return Err(
            "a password on the command line can be read by other programs: give the user name in the address (socks5://ann@host:1080) and the password in the MDM_PROXY_PASSWORD environment variable"
                .into(),
        );
    }
    // The address's user name as typed (`%40` for an `@` in it).
    let decode = |s: &str| {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match (b[i], b.get(i + 1..i + 3)) {
                (b'%', Some(hex))
                    if std::str::from_utf8(hex)
                        .is_ok_and(|h| u8::from_str_radix(h, 16).is_ok()) =>
                {
                    out.push(
                        u8::from_str_radix(std::str::from_utf8(hex).unwrap_or("0"), 16)
                            .unwrap_or(0),
                    );
                    i += 3;
                }
                (c, _) => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    let login =
        (!u.username().is_empty()).then(|| (decode(u.username()), password.unwrap_or_default()));
    let _ = u.set_username("");
    Ok(Proxy::Manual {
        url: u.as_str().trim_end_matches('/').to_owned(),
        login,
        http: true,
        https: true,
    })
}

/// Does the command line `args` (as [`main_with`] takes them) download a link with MDM's own
/// User-Agent (`get`, `resume`)? The app waits for the default browser's (#668) only for these.
pub fn downloads(args: &[OsString]) -> bool {
    Cli::try_parse_from(std::iter::once(OsString::from(NAME)).chain(args.iter().cloned()))
        .is_ok_and(|cli| matches!(cli.cmd, Cmd::Get { .. } | Cmd::Resume { .. }))
}

/// [`main`], downloading through the app's debrid `accounts` (#44) and its network settings
/// (`net`, #1556).
pub fn main_with(args: Vec<OsString>, accounts: Option<&dyn Accounts>, net: Net) -> ExitCode {
    let cli = Cli::parse_from(std::iter::once(OsString::from(NAME)).chain(args));
    Conn::of(&cli.cmd, &net).set();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let json = cli.json;
    #[cfg(unix)]
    if runs_helpers(&cli.cmd) {
        end_helpers_when_stopped(&rt, json);
    }
    match rt.block_on(run(cli.cmd, json, accounts)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is::<Reported>() => ExitCode::FAILURE,
        Err(e) => {
            if json {
                match e.downcast_ref::<NoFormat>() {
                    Some(n) => emit(n.json()),
                    None => emit(json!({ "error": format!("{e:#}") })),
                }
            } else {
                eprintln!("Error: {e:?}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Whether `cmd` runs yt-dlp or gallery-dl (a look-up, a download, an install's version check).
/// On Linux and macOS each runs in a process group of its own (#1301), which the terminal's
/// Ctrl+C doesn't reach: such a command ends them itself when stopped
/// ([`end_helpers_when_stopped`]). Windows: their job objects end them with this process.
#[cfg(unix)]
fn runs_helpers(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::InstallYtdlp { .. }
            | Cmd::UpdateHelpers { .. }
            | Cmd::InstallFfmpeg { .. }
            | Cmd::InstallGalleryDl { .. }
            | Cmd::InstallJsRuntime { .. }
            | Cmd::GalleryGet { .. }
            | Cmd::VideoInfo { .. }
            | Cmd::PlaylistInfo { .. }
            | Cmd::PlaylistGet { .. }
            | Cmd::VideoGet { .. }
    )
}

/// Linux, macOS: once the command is asked to stop (Ctrl+C, SIGTERM, its terminal closed), the
/// helpers it runs end with all they started, what they downloaded left as a pause leaves it,
/// and the command ends (128 + the signal's number). Heard from the start, on the runtime's
/// other threads: a look-up runs on the command's own thread until yt-dlp answers.
#[cfg(unix)]
fn end_helpers_when_stopped(rt: &tokio::runtime::Runtime, json: bool) {
    use tokio::signal::unix::{SignalKind, signal};
    let _in = rt.enter();
    let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
    ) else {
        return;
    };
    rt.spawn(async move {
        let number: i32 = tokio::select! {
            _ = int.recv() => 2,
            _ = term.recv() => 15,
            _ = hup.recv() => 1,
        };
        mdm_tools::proc::end_all();
        if json {
            emit(json!({ "event": "stopped" }));
        } else {
            eprintln!("\nstopped");
        }
        std::process::exit(128 + number);
    });
}

async fn run(cmd: Cmd, json: bool, accounts: Option<&dyn Accounts>) -> anyhow::Result<()> {
    match cmd {
        Cmd::Get {
            url,
            output,
            connections,
            headers,
            referer,
            user_agent,
            cookie,
            checksum,
            direct,
            ..
        } => {
            let dest = if output.is_dir() {
                Destination::Dir(output)
            } else {
                Destination::File(output)
            };
            let headers = build_headers(&headers, referer, user_agent, cookie)?;
            let opts = Options {
                checksum,
                ..opts(connections)
            };
            let opts = through_account(accounts, &url, direct, opts, json).await?;
            let req = Request { url, headers, dest };
            watch(Download::start(req, opts), json).await
        }
        Cmd::InstallYtdlp { .. } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let i = mdm_tools::install_latest(&dir, &own_net(), None).await?;
            if json {
                emit(json!({ "tool": "yt-dlp", "version": i.version, "path": i.exe }));
            } else {
                println!(
                    "yt-dlp {} verified and installed: {}",
                    i.version,
                    i.exe.display()
                );
            }
            Ok(())
        }
        Cmd::InstallFfmpeg { .. } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let i = mdm_tools::ffmpeg::install_latest(&dir, &own_net(), None).await?;
            if json {
                emit(json!({ "tool": "ffmpeg", "version": i.version, "path": i.exe }));
            } else {
                println!(
                    "ffmpeg {} verified and installed: {}",
                    i.version,
                    i.exe.display()
                );
            }
            Ok(())
        }
        Cmd::InstallJsRuntime { .. } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let i = mdm_tools::jsruntime::install_latest(&dir, &own_net(), None).await?;
            // What yt-dlp uses: the one just installed, unless one was chosen or found first.
            let used = mdm_tools::jsruntime::current(Some(dir.as_path()), None);
            if json {
                emit(json!({
                    "tool": "node",
                    "version": i.version,
                    "path": i.exe,
                    "used": used.as_ref().map(|r| json!({
                        "runtime": r.kind.ytdlp_name(),
                        "version": r.version,
                        "path": r.exe,
                    })),
                }));
            } else {
                println!(
                    "Node.js {} verified and installed: {}",
                    i.version,
                    i.exe.display()
                );
                if let Some(r) = used.filter(|r| r.exe != i.exe) {
                    println!(
                        "yt-dlp uses {} {}, found first: {}",
                        r.kind.label(),
                        r.version.as_deref().unwrap_or(""),
                        r.exe.display()
                    );
                }
            }
            Ok(())
        }
        Cmd::UpdateHelpers { .. } => update_helpers(json).await,
        Cmd::InstallGalleryDl { .. } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let i = mdm_tools::gallery::install_latest(&dir, &own_net(), None).await?;
            if json {
                emit(json!({ "tool": "gallery-dl", "version": i.version, "path": i.exe }));
            } else {
                println!(
                    "gallery-dl {} verified and installed: {}",
                    i.version,
                    i.exe.display()
                );
            }
            Ok(())
        }
        Cmd::GalleryGet {
            url, output, limit, ..
        } => {
            use mdm_tools::video::State;
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            // A gallery-dl of the person's too old for MDM is passed over (#236), said plainly.
            let old = mdm_tools::gallery::chosen_too_old(&dir).map(|old| {
                format!(
                    "your gallery-dl {} ({}) is too old: MDM needs {} or newer",
                    old.version,
                    old.exe.display(),
                    mdm_tools::gallery::OLDEST
                )
            });
            let tool = mdm_tools::gallery::installed(&dir).ok_or_else(|| match &old {
                Some(why) => anyhow::anyhow!("{why}; run install-gallery-dl for MDM's own copy"),
                None => anyhow::anyhow!("gallery-dl is not installed: run install-gallery-dl"),
            })?;
            if let Some(why) = &old {
                if tool.is_system() {
                    eprintln!("{why}: using the system's, {}", tool.exe.display());
                } else {
                    eprintln!("{why}: using MDM's own copy, {}", tool.version);
                }
            }
            if !mdm_tools::gallery::handles(&tool, &url)? {
                anyhow::bail!("gallery-dl can't read this address");
            }
            let name = mdm_tools::gallery::name_of(&url::Url::parse(&url)?);
            let folder = output.join(name);
            let ask = mdm_tools::gallery::Ask { limit, rate: None };
            let job = mdm_tools::gallery::GalleryJob::start(
                &tool,
                &url,
                &folder,
                &ask,
                &Default::default(),
                Some(video_engine(8)),
                None,
            )?;
            let p = loop {
                let p = job.progress();
                if p.state != State::Running {
                    break p;
                }
                if !json {
                    eprint!("\r{} bytes so far", p.downloaded);
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            };
            if !json {
                eprintln!();
            }
            for (_, e) in &p.events {
                eprintln!("{e}");
            }
            match p.state {
                State::Done(f) => {
                    if json {
                        emit(json!({ "folder": f, "bytes": p.downloaded }));
                    } else {
                        println!("saved into {}", f.display());
                    }
                    Ok(())
                }
                State::Failed(e) => anyhow::bail!(e),
                _ => anyhow::bail!("stopped"),
            }
        }
        Cmd::VideoInfo { url, .. } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let tool = mdm_tools::installed(&dir)
                .ok_or_else(|| anyhow::anyhow!("yt-dlp is not installed (install-ytdlp)"))?;
            // The page's own declared video when yt-dlp read it generically (#1076).
            let (looked, declared) = look_up_page(&tool.exe, &url, net_opts())?;
            let v = match looked {
                mdm_tools::resolve::Looked::Video(v) => v,
                mdm_tools::resolve::Looked::List(l) => {
                    let size = l.total.map_or(String::new(), |n| format!(", {n} videos"));
                    if json {
                        emit(json!({ "list": l.title, "total": l.total }));
                    } else {
                        println!("{}: a list of videos{size}; see playlist-info", l.title);
                    }
                    return Ok(());
                }
            };
            let merge = mdm_tools::ffmpeg::installed(&dir).is_some();
            let choices = mdm_tools::resolve::choices(&v, merge);
            // The subtitles a person would be offered (the automatic ones: the spoken
            // language's and English).
            let subs = mdm_tools::resolve::sub_choices(&v, "en");
            if json {
                let choices: Vec<_> = choices.iter().map(choice_json).collect();
                let subs: Vec<_> = subs
                    .iter()
                    .map(|s| json!({ "code": s.code, "name": s.name, "automatic": s.automatic }))
                    .collect();
                emit(json!({
                    "title": v.title,
                    "choices": choices,
                    "subtitles": subs,
                    "needs_js_runtime": v.needs_js_runtime,
                    "declared": declared.as_ref().map(|d| &d.address),
                }));
                return Ok(());
            }
            if v.needs_js_runtime {
                eprintln!("{}", js_runtime_note());
            }
            if let Some(d) = &declared {
                eprintln!("{}", declared_note(d));
            }
            println!("{}", v.title);
            for c in &choices {
                println!("{}", choice_line(c));
            }
            if !subs.is_empty() {
                println!("subtitles:");
                for s in subs {
                    let auto = if s.automatic { "  --auto-subs" } else { "" };
                    println!("  {}    --subs {}{auto}", s.name, s.code);
                }
            }
            Ok(())
        }
        Cmd::PlaylistInfo {
            url, from, count, ..
        } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let tool = mdm_tools::installed(&dir)
                .ok_or_else(|| anyhow::anyhow!("yt-dlp is not installed (install-ytdlp)"))?;
            let to = count.map(|c| from + c.max(1) - 1);
            let listing = mdm_tools::playlist::Listing::start(
                &tool.exe,
                &url,
                &Default::default(),
                from,
                to,
            )?;
            let mut seen = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let p = listing.progress();
                for e in listing.entries_from(seen) {
                    seen += 1;
                    if json {
                        emit(json!({
                            "index": e.index, "title": e.title, "url": e.url,
                            "duration": e.duration, "unavailable": e.unavailable,
                        }));
                    } else {
                        let len = e
                            .duration
                            .map_or("--:--".into(), mdm_tools::playlist::clock);
                        let gone = if e.unavailable { "  (unavailable)" } else { "" };
                        println!("{:>6}  {len:>8}  {}  {}{gone}", e.index, e.title, e.url);
                    }
                }
                if p.done {
                    if let Some(e) = p.error {
                        anyhow::bail!(e);
                    }
                    let total = p.total.map_or(String::new(), |t| format!(" of {t}"));
                    let capped = if p.capped { " (the most kept)" } else { "" };
                    if json {
                        emit(
                            json!({ "title": p.title, "listed": p.listed, "total": p.total, "capped": p.capped }),
                        );
                    } else {
                        eprintln!(
                            "{}: {} listed{total}{capped}",
                            p.title.as_deref().unwrap_or("the list"),
                            p.listed
                        );
                    }
                    return Ok(());
                }
            }
        }
        Cmd::PlaylistGet {
            url,
            format,
            output,
            connections,
            from,
            count,
            ..
        } => {
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let tool = mdm_tools::installed(&dir)
                .ok_or_else(|| anyhow::anyhow!("yt-dlp is not installed (install-ytdlp)"))?;
            let ffmpeg = mdm_tools::ffmpeg::installed(&dir).map(|f| f.exe);
            let to = count.map(|c| from + c.max(1) - 1);
            let listing = mdm_tools::playlist::Listing::start(
                &tool.exe,
                &url,
                &Default::default(),
                from,
                to,
            )?;
            while !listing.progress().done {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let p = listing.progress();
            if let Some(e) = p.error {
                anyhow::bail!(e);
            }
            let entries: Vec<_> = listing
                .entries_from(0)
                .into_iter()
                .filter(|e| !e.unavailable)
                .collect();
            // A folder named after the list, as the window makes it.
            let folder = output.join(mdm_engine::naming::sanitize(
                p.title.as_deref().unwrap_or("playlist"),
            ));
            std::fs::create_dir_all(&folder)?;
            let ext = mdm_tools::resolve::mp3_quality(&format).map_or("mp4", |_| "mp3");
            let n = entries.len();
            let mut failed = 0;
            for (i, e) in entries.iter().enumerate() {
                let name = format!("{}.{ext}", mdm_engine::naming::sanitize(&e.title));
                let free = mdm_engine::naming::stem_destination(&folder, &name, |_| false);
                let stem = free
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "video".into());
                let job = mdm_tools::video::VideoJob::start(
                    &tool.exe,
                    &e.url,
                    &format,
                    &folder,
                    &stem,
                    None,
                    ffmpeg.as_deref(),
                    None,
                    &Default::default(),
                    &Default::default(),
                    Some(video_engine(connections)),
                )?;
                let state = loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let s = job.progress().state;
                    if s != mdm_tools::video::State::Running {
                        break s;
                    }
                };
                match state {
                    mdm_tools::video::State::Done(f) => {
                        if json {
                            emit(json!({ "event": "done", "index": e.index, "path": f }));
                        } else {
                            println!("[{}/{n}] saved {}", i + 1, f.display());
                        }
                    }
                    other => {
                        failed += 1;
                        if json {
                            emit(
                                json!({ "event": "failed", "index": e.index, "error": format!("{other:?}") }),
                            );
                        } else {
                            println!("[{}/{n}] failed: {} ({other:?})", i + 1, e.title);
                        }
                    }
                }
            }
            if failed > 0 {
                anyhow::bail!("{failed} of {n} videos failed");
            }
            Ok(())
        }
        Cmd::TorrentCreate { path, output } => {
            let spawner = mdm_torrent::librqbit::spawn_utils::BlockingSpawner::new(1);
            let made = mdm_torrent::librqbit::create_torrent(
                &path,
                mdm_torrent::librqbit::CreateTorrentOptions {
                    name: None,
                    trackers: Vec::new(),
                    piece_length: None,
                },
                &spawner,
            )
            .await?;
            std::fs::write(&output, made.as_bytes()?)?;
            if json {
                emit(json!({ "info_hash": made.info_hash().as_string(), "path": output }));
            } else {
                println!("{}  {}", made.info_hash().as_string(), output.display());
            }
            Ok(())
        }
        Cmd::TorrentInfo { source } => {
            let (t, _dir) = torrents().await?;
            let m = t
                .metadata(&torrent_source(&source)?, Duration::from_secs(180), None)
                .await?;
            let files = m.files.iter().enumerate().filter(|(_, f)| !f.padding);
            if json {
                let files: Vec<_> = files
                    .map(|(i, f)| json!({ "index": i, "size": f.size, "path": f.path }))
                    .collect();
                emit(json!({
                    "name": m.name,
                    "info_hash": m.info_hash,
                    "size": m.total(),
                    "files": files,
                }));
            } else {
                println!("{}  {}  {} bytes", m.name, m.info_hash, m.total());
                for (i, f) in files {
                    println!("{i:>4}  {:>14}  {}", f.size, f.path.display());
                }
            }
            t.stop().await;
            Ok(())
        }
        Cmd::TorrentGet {
            source,
            output,
            files,
            seconds,
            use_existing,
        } => {
            let (t, _dir) = torrents().await?;
            let m = t
                .metadata(&torrent_source(&source)?, Duration::from_secs(180), None)
                .await?;
            // The app asks first; here, the flag says yes (#40).
            let there = m.existing_in(&output);
            if let (false, Some(first)) = (use_existing, there.first()) {
                t.stop().await;
                anyhow::bail!(
                    "files of the torrent are already there ({}, first: {}); add --use-existing \
                     to use them, or choose another folder with -o",
                    there.len(),
                    first.display()
                );
            }
            let folder = m.folder_in(&output);
            let hash = t
                .add(&m.torrent, &folder, files, m.peers.clone(), false)
                .await?;
            let started = std::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                let s = t.stats(&hash)?;
                if json {
                    emit(json!({
                        "event": "progress",
                        "phase": format!("{:?}", s.phase),
                        "downloaded": s.done,
                        "size": s.total,
                        "speed": s.down_speed as u64,
                        "peers": s.peers_live,
                        "peers_seen": s.peers_seen,
                    }));
                } else {
                    eprint!(
                        "\r{:?} {} / {}  {:.0} KB/s  peers {}/{}      ",
                        s.phase,
                        s.done,
                        s.total,
                        s.down_speed / 1024.0,
                        s.peers_live,
                        s.peers_seen
                    );
                }
                if s.phase == mdm_torrent::Phase::Finished {
                    if json {
                        emit(json!({ "event": "done", "path": folder, "size": s.total }));
                    } else {
                        eprintln!();
                        println!("saved in {}", folder.display());
                    }
                    break;
                }
                if let Some(e) = s.error {
                    anyhow::bail!(e);
                }
                if seconds.is_some_and(|n| started.elapsed().as_secs() >= n) {
                    t.pause(&hash).await?;
                    let secs = started.elapsed().as_secs();
                    if json {
                        emit(json!({ "event": "stopped", "downloaded": s.done, "seconds": secs }));
                    } else {
                        eprintln!();
                        println!("stopped after {secs} s; {} bytes there", s.done);
                    }
                    break;
                }
            }
            t.stop().await;
            Ok(())
        }
        Cmd::VideoGet {
            url,
            format,
            quality,
            output,
            subs,
            auto_subs,
            embed_subs,
            connections,
            ..
        } => {
            let subs = subs_arg(&subs, auto_subs, embed_subs);
            let dir = mdm_tools::tools_dir().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
            let tool = mdm_tools::installed(&dir)
                .ok_or_else(|| anyhow::anyhow!("yt-dlp is not installed (install-ytdlp)"))?;
            // The page's own declared video when yt-dlp read it generically (#1076): the one
            // downloaded, asked with the page as its Referer.
            let (looked, declared) = look_up_page(&tool.exe, &url, net_opts())?;
            let v = match looked {
                mdm_tools::resolve::Looked::Video(v) => v,
                mdm_tools::resolve::Looked::List(l) => {
                    anyhow::bail!(
                        "yt-dlp failed: the address is a list of videos ({})",
                        l.title
                    )
                }
            };
            if v.needs_js_runtime {
                eprintln!("{}", js_runtime_note());
            }
            if let Some(d) = &declared {
                eprintln!("{}", declared_note(d));
            }
            let (address, browser) = match &declared {
                Some(d) => (d.address.clone(), d.browser(&Default::default())),
                None => (url.clone(), Default::default()),
            };
            let ffmpeg = mdm_tools::ffmpeg::installed(&dir).map(|f| f.exe);
            // The qualities Download video lists for it (#1240), and the one it starts on, or
            // the height asked's nearest.
            let choices = mdm_tools::resolve::choices(&v, ffmpeg.is_some());
            let format = match format {
                Some(f) => f,
                None => pick_quality(&v, &choices, quality, ffmpeg.is_some(), json)?,
            };
            // A stem no file in the folder has, whatever the extension: yt-dlp takes any
            // `<stem>.*` there as its own (#32). The extension only sizes the name.
            let ext = mdm_tools::resolve::audio_conversion(&format).map_or("mp4", |a| a.format);
            let name = format!("{}.{ext}", mdm_engine::naming::sanitize(&v.title));
            let free = mdm_engine::naming::stem_destination(&output, &name, |_| false);
            let stem = free
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "video".into());
            let job = mdm_tools::video::VideoJob::start(
                &tool.exe,
                &address,
                &format,
                &output,
                &stem,
                None,
                ffmpeg.as_deref(),
                // No size shown here: the progress steadies itself (video::Meter).
                None,
                &browser,
                &subs,
                Some(video_engine(connections)),
            )?;
            // The download's log as it comes (a quality not served falling back to the
            // nearest, #94; a server given up), as the window's log has it.
            let mut said = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let p = job.progress();
                for (_, text) in p.events.iter().skip(said) {
                    if json {
                        emit(json!({ "event": "note", "text": text }));
                    } else {
                        eprintln!("\n{text}");
                    }
                }
                said = p.events.len();
                if json {
                    emit(json!({
                        "event": "progress",
                        "downloaded": p.downloaded,
                        "size": p.total,
                        "speed": p.speed as u64,
                    }));
                } else {
                    eprint!(
                        "\r{} / {}  {:.0} KB/s      ",
                        p.downloaded,
                        p.total.map_or("?".into(), |t| t.to_string()),
                        p.speed / 1024.0
                    );
                }
                // The error below the progress line, not at its end.
                if !json
                    && matches!(
                        p.state,
                        mdm_tools::video::State::Failed(_) | mdm_tools::video::State::Stopped
                    )
                {
                    eprintln!();
                }
                match p.state {
                    mdm_tools::video::State::Running => {}
                    mdm_tools::video::State::Done(f) => {
                        // Subtitles the site refused: the video is saved all the same (#68).
                        let problems: Vec<(String, String, String, bool)> = p
                            .sub_problems
                            .iter()
                            .map(|s| {
                                let why = match &s.reason {
                                    mdm_tools::video::SubReason::TooManyRequests => {
                                        "the site refused (too many requests)".to_owned()
                                    }
                                    mdm_tools::video::SubReason::Other(w) => w.clone(),
                                };
                                (s.code.clone(), s.name.clone(), why, s.beside)
                            })
                            .collect();
                        if json {
                            let problems: Vec<_> = problems
                                .iter()
                                .map(|(code, name, why, beside)| {
                                    json!({ "code": code, "name": name, "reason": why, "beside": beside })
                                })
                                .collect();
                            emit(json!({
                                "event": "done",
                                "path": f,
                                "subtitles": p.side_files,
                                "subtitle_problems": problems,
                            }));
                        } else {
                            eprintln!();
                            println!("saved {}", f.display());
                            for s in &p.side_files {
                                println!("saved {}", s.display());
                            }
                            for (code, name, why, beside) in problems {
                                if beside {
                                    println!(
                                        "subtitles in {name} ({code}) came on a second try: beside the video, not in it"
                                    );
                                } else {
                                    println!(
                                        "subtitles in {name} ({code}) couldn't be downloaded: {why}"
                                    );
                                }
                            }
                        }
                        return Ok(());
                    }
                    // A path yt-dlp couldn't make, too long for the system (#1102): said as such.
                    mdm_tools::video::State::Failed(e)
                        if let Some(path) = mdm_tools::video::too_long(&e) =>
                    {
                        anyhow::bail!(mdm_tools::proc::ProcError::PathTooLong(path))
                    }
                    // No format the page offers matched (#1240): with the ones it does.
                    mdm_tools::video::State::Failed(e)
                        if mdm_tools::servers::format_unavailable(&e) =>
                    {
                        anyhow::bail!(NoFormat::new(without_list_advice(&e), &v, &choices))
                    }
                    // yt-dlp's words, not `Failed("…")`.
                    mdm_tools::video::State::Failed(e) => anyhow::bail!(e),
                    other => anyhow::bail!("{other:?}"),
                }
            }
        }
        Cmd::Status {
            id,
            status,
            search,
            category,
            queue,
            limit,
        } => status_command(id, status, search, category, queue, limit),
        Cmd::Pause { ids } => act("pause", ids),
        Cmd::Start { ids } => act("start", ids),
        Cmd::Remove { ids } => act("remove", ids),
        Cmd::Delete { ids } => act("delete", ids),
        Cmd::Add {
            url,
            output,
            name,
            category,
            queue,
            checksum,
            referer,
            user_agent,
            cookie,
            later,
            silent,
            private,
        } => ask(add_command(
            url, output, name, category, queue, checksum, referer, user_agent, cookie, later,
            silent, private,
        )?),
        Cmd::StartQueue { queue, name } => ask(mdm_ipc::Command {
            action: Some("start_queue".into()),
            queue: queue.or(name),
            ..Default::default()
        }),
        Cmd::StopQueue { queue, name } => ask(mdm_ipc::Command {
            action: Some("stop_queue".into()),
            queue: queue.or(name),
            ..Default::default()
        }),
        Cmd::Pair => pair(json),
        Cmd::Api { state } => ask(mdm_ipc::Command {
            api: Some(state == "on"),
            ..Default::default()
        }),
        Cmd::Resume {
            state,
            connections,
            headers,
            referer,
            user_agent,
            cookie,
            checksum,
            direct,
            ..
        } => {
            let base_opts = Options {
                checksum,
                ..opts(connections)
            };
            // Through the account its address goes through (#44): the link a service gave
            // expires, and the service gives a new one.
            let opts = match mdm_engine::state::ResumeState::load(&state) {
                Ok(saved) => through_account(accounts, &saved.url, direct, base_opts, json).await?,
                // Download::resume says what is wrong with it.
                Err(_) => base_opts,
            };
            let headers = build_headers(&headers, referer, user_agent, cookie)?;
            let dl = Download::resume(state, headers, opts)?;
            watch(dl, json).await
        }
    }
}

/// `video-get --subs` (#68): "none" (or nothing), "all", or language codes separated by commas.
fn subs_arg(langs: &str, automatic: bool, embed: bool) -> mdm_tools::video::Subs {
    let langs: Vec<String> = langs
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("none"))
        .map(|l| {
            if l.eq_ignore_ascii_case(mdm_tools::video::ALL_UPLOADERS) {
                mdm_tools::video::ALL_UPLOADERS.to_owned()
            } else {
                l.to_owned()
            }
        })
        .collect();
    mdm_tools::video::Subs {
        langs,
        automatic,
        embed,
    }
}

/// MDM's engine for a video's streams (#769): this many connections each, through this
/// command's connection.
fn video_engine(connections: u32) -> mdm_tools::streams::Engine {
    mdm_tools::streams::Engine {
        handle: tokio::runtime::Handle::current(),
        opts: opts(connections),
    }
}

/// yt-dlp's look-up of the page `url` as the app's Download video window makes it (#1076): the
/// video the page declares its own when yt-dlp read it generically, the page read through
/// `opts` (`mdm_tools::declared::look_up`). On this runtime's thread, which it holds meanwhile.
fn look_up_page(
    exe: &std::path::Path,
    url: &str,
    opts: Options,
) -> Result<
    (
        mdm_tools::resolve::Looked,
        Option<mdm_tools::declared::Declared>,
    ),
    mdm_tools::proc::ProcError,
> {
    let reader = mdm_tools::streams::Engine {
        handle: tokio::runtime::Handle::current(),
        opts,
    };
    tokio::task::block_in_place(|| {
        mdm_tools::declared::look_up(exe, url, &Default::default(), &reader)
            .map(|(looked, _, declared)| (looked, declared))
    })
}

/// What `video-info` and `video-get` say when the page's declared video is the one looked up.
fn declared_note(d: &mdm_tools::declared::Declared) -> String {
    format!("the page declares its video: {}", d.address)
}

/// `video-get --quality`: a height, or the most compatible quality (#1439).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quality {
    Height(u32),
    /// The window's "Most compatible (H.264 + AAC)".
    Compatible,
}

/// `video-get --quality`: a height, as "1080" or "1080p", or "compatible" (#1439).
fn parse_quality(s: &str) -> Result<Quality, String> {
    if s.trim().eq_ignore_ascii_case("compatible") {
        return Ok(Quality::Compatible);
    }
    parse_height(s).map(Quality::Height).map_err(|_| {
        format!(
            "{:?} is not a height (such as 1080 or 720p) or \"compatible\"",
            s.trim()
        )
    })
}

/// A height, as "1080" or "1080p".
fn parse_height(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let digits = s.strip_suffix(['p', 'P']).unwrap_or(s);
    match digits.parse::<u32>() {
        Ok(h) if h > 0 => Ok(h),
        _ => Err(format!("{s:?} is not a height (such as 1080 or 720p)")),
    }
}

/// A quality as `video-info --json` lists it, and `video-get --json`'s error (#1240).
fn choice_json(c: &mdm_tools::resolve::Choice) -> serde_json::Value {
    json!({
        "label": c.label,
        "format": c.selector,
        "size": c.size,
        "approx": c.approx,
        "height": c.height,
        "fps": c.fps,
        "codec": c.codec,
        "range": c.range,
    })
}

/// A quality's line as `video-info` prints it, and `video-get`'s error (#1240): its label, size
/// (the streams' own bytes, or "~" before an estimate, #801) and what `-f` takes for it.
fn choice_line(c: &mdm_tools::resolve::Choice) -> String {
    let about = if c.approx { "~" } else { "" };
    let size = c
        .size
        .map_or(String::new(), |s| format!("  {about}{} MB", s / 1_000_000));
    format!("  {}{size}    -f \"{}\"", c.label, c.selector)
}

/// `video-get` without `-f` (#1240): the quality Download video starts on, or the one of the
/// height asked, else its nearest ([`mdm_tools::resolve::pick`]), or the most compatible one
/// (#1439), said on stderr (a `quality` event with `--json`). Its selector; or, with nothing to
/// download, what the page offers.
fn pick_quality(
    v: &mdm_tools::resolve::Video,
    choices: &[mdm_tools::resolve::Choice],
    quality: Option<Quality>,
    merge: bool,
    json: bool,
) -> anyhow::Result<String> {
    use mdm_tools::resolve::{compatible_choice, is_audio, needs_merging, pick};
    // Its own line in the list, or the best when that is the very download (#1439).
    if quality == Some(Quality::Compatible) {
        let Some(c) = compatible_choice(v, merge) else {
            let why = if needs_merging(v) && !merge {
                "no H.264 video with AAC sound to give without ffmpeg: its pictures come apart \
                 from their sound, and joining them needs ffmpeg (install-ffmpeg)"
            } else {
                "no H.264 video with AAC sound on that page, the most compatible quality \
                 (nothing is converted)"
            };
            anyhow::bail!(NoFormat::new(why.to_owned(), v, choices));
        };
        if json {
            emit(json!({
                "event": "quality",
                "label": c.label,
                "format": c.selector,
                "height": c.height,
                "asked": "compatible",
                "nearest": false,
            }));
        } else {
            eprintln!("downloading {}", c.label);
        }
        return Ok(c.selector);
    }
    let height = match quality {
        Some(Quality::Height(h)) => Some(h),
        _ => None,
    };
    let apart = needs_merging(v) && !merge;
    let picked = pick(choices, height)
        // The sound alone for a video only when the page has no picture at all.
        .filter(|p| !(height.is_none() && apart && is_audio(p.choice)));
    let Some(p) = picked else {
        let why = match height {
            _ if apart => "its pictures come apart from their sound, and joining them needs \
                           ffmpeg (install-ffmpeg); -f ba downloads the sound alone"
                .to_owned(),
            Some(h) => format!("no video of a known height on that page to take {h}p from"),
            None => "no video this can download on that page".to_owned(),
        };
        anyhow::bail!(NoFormat::new(why, v, choices));
    };
    let c = p.choice;
    if json {
        emit(json!({
            "event": "quality",
            "label": c.label,
            "format": c.selector,
            "height": c.height,
            "asked": height,
            "nearest": p.instead_of.is_some(),
        }));
    } else if let Some(asked) = p.instead_of {
        eprintln!(
            "{asked}p isn't offered: downloading the nearest, {}",
            c.label
        );
    } else {
        eprintln!("downloading {}", c.label);
    }
    Ok(c.selector.clone())
}

/// yt-dlp's message without its advice for its own command line ("Use --list-formats for a list
/// of available formats"): `video-get` lists them itself (#1240).
fn without_list_advice(message: &str) -> String {
    match message.find(" Use --list-formats") {
        Some(i) => message[..i].to_owned(),
        None => message.to_owned(),
    }
}

/// `video-get` found nothing to download (#1240): why, with what the page offers instead, as
/// `video-info` lists it (its conversions left out), else yt-dlp's own formats. `--json` gives
/// them as `choices` and `formats` beside the `error`.
#[derive(Debug)]
struct NoFormat {
    why: String,
    choices: Vec<mdm_tools::resolve::Choice>,
    formats: Vec<mdm_tools::resolve::Format>,
}

impl NoFormat {
    fn new(
        why: String,
        v: &mdm_tools::resolve::Video,
        choices: &[mdm_tools::resolve::Choice],
    ) -> Self {
        let choices = choices
            .iter()
            .filter(|c| mdm_tools::resolve::audio_conversion(&c.selector).is_none())
            .cloned()
            .collect();
        NoFormat {
            why,
            choices,
            formats: v.formats.clone(),
        }
    }

    /// yt-dlp's format, briefly: its ID, type, height and what it carries.
    fn format_line(f: &mdm_tools::resolve::Format) -> String {
        let height = f.height.map_or(String::new(), |h| format!(" {h}p"));
        let has = match (f.video, f.audio) {
            (true, true) => "picture and sound",
            (true, false) => "picture only",
            (false, true) => "sound only",
            (false, false) => "neither",
        };
        format!("  {} {}{height}, {has}", f.id, f.ext)
    }

    fn json(&self) -> serde_json::Value {
        let formats: Vec<_> = self
            .formats
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "ext": f.ext,
                    "height": f.height,
                    "video": f.video,
                    "audio": f.audio,
                })
            })
            .collect();
        json!({
            "error": self.why,
            "choices": self.choices.iter().map(choice_json).collect::<Vec<_>>(),
            "formats": formats,
        })
    }
}

impl std::fmt::Display for NoFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.why)?;
        if !self.choices.is_empty() {
            f.write_str("\nthe page offers (-f, or --quality with a height or compatible):")?;
            for c in &self.choices {
                write!(f, "\n{}", choice_line(c))?;
            }
        } else if !self.formats.is_empty() {
            f.write_str("\nthe page's formats (yt-dlp's IDs, for -f):")?;
            for x in &self.formats {
                write!(f, "\n{}", Self::format_line(x))?;
            }
        } else {
            f.write_str("\nthe page lists no format")?;
        }
        Ok(())
    }
}

impl std::error::Error for NoFormat {}

/// A download's options: the app's defaults (the default browser's User-Agent, #668, MDM's
/// former one once if a site refuses it, #372, and MDM's own name once if a site's bot check
/// refuses the browser's and the browser-like connection too, #1422; a `--user-agent` or a
/// `User-Agent` header given is the only one sent), through this command's connection (#1556).
fn opts(connections: u32) -> Options {
    Options {
        max_connections: connections,
        agent_fallback: Some(mdm_engine::agent::IE11_USER_AGENT.to_owned()),
        own_agent: Some(mdm_engine::agent::OWN_USER_AGENT.to_owned()),
        ..net_opts()
    }
}

/// `--dns`: see [`Cmd::Get`].
fn parse_dns(s: &str) -> Result<mdm_engine::DnsChoice, String> {
    use mdm_engine::DnsChoice;
    use mdm_engine::dns::{Dns, Mode, Preset, parse_servers};
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("system") {
        return Ok(DnsChoice::default());
    }
    if s.starts_with("https://") {
        return Ok(DnsChoice {
            dns: Dns::Https {
                url: s.to_owned(),
                bootstrap: Vec::new(),
            },
            fallback: false,
            label: s.to_owned(),
        });
    }
    if let Some((id, how)) = s.rsplit_once('-')
        && let Some(p) = Preset::by_id(&id.to_ascii_lowercase())
    {
        let mode = match how.to_ascii_lowercase().as_str() {
            "plain" => Mode::Plain,
            "tls" => Mode::Tls,
            "https" => Mode::Https,
            _ => return Err(format!("`{how}`: plain, tls or https")),
        };
        if !p.offers(mode) {
            return Err(format!("{} isn't offered as {how}", p.name));
        }
        return Ok(p.choice(mode, "", false, format!("{} ({how})", p.name)));
    }
    let servers = parse_servers(s, 53).map_err(|bad| {
        format!("`{bad}`: not a DNS server's address, a service or an https:// address")
    })?;
    Ok(DnsChoice {
        dns: Dns::Plain(servers),
        fallback: false,
        label: s.to_owned(),
    })
}

fn parse_checksum(s: &str) -> Result<mdm_engine::verify::Hash, String> {
    mdm_engine::verify::Hash::parse(s)
        .ok_or_else(|| "not a checksum (hex MD5, SHA-1, SHA-256, SHA-512 or CRC32)".into())
}

fn parse_headers(raw: &[String]) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for h in raw {
        let (k, v) = h
            .split_once(':')
            .with_context(|| format!("header `{h}` needs `Name: value`"))?;
        headers.insert(
            HeaderName::try_from(k.trim())?,
            HeaderValue::try_from(v.trim())?,
        );
    }
    Ok(headers)
}

fn build_headers(
    headers: &[String],
    referer: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
) -> anyhow::Result<HeaderMap> {
    build_headers_with(headers, referer, user_agent, cookie, std::io::stdin())
}

fn build_headers_with(
    headers: &[String],
    referer: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
    stdin: impl Read,
) -> anyhow::Result<HeaderMap> {
    let mut map = parse_headers(headers)?;
    if let Some(r) = referer.filter(|s| !s.trim().is_empty()) {
        map.insert(REFERER, HeaderValue::try_from(r.trim())?);
    }
    if let Some(ua) = user_agent.filter(|s| !s.trim().is_empty()) {
        map.insert(USER_AGENT, HeaderValue::try_from(ua.trim())?);
    }
    if let Some(c) = read_cookie(cookie, stdin)? {
        map.insert(COOKIE, HeaderValue::try_from(c.trim())?);
    }
    Ok(map)
}

/// Print progress until the download ends; Ctrl+C pauses it (state is saved). `json`: one
/// `progress` line per tick, then `done` or `paused`.
async fn watch(dl: Download, json: bool) -> anyhow::Result<()> {
    let started = Instant::now();
    let mut last = (Instant::now(), 0u64);
    let mut speed = 0.0;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = tokio::signal::ctrl_c() => {
                dl.cancel();
                if !json {
                    println!("\npausing…");
                }
            }
        }
        let s = dl.snapshot();
        let dt = last.0.elapsed().as_secs_f64().max(1e-3);
        let inst = s.downloaded.saturating_sub(last.1) as f64 / dt;
        speed = if speed == 0.0 {
            inst
        } else {
            0.7 * speed + 0.3 * inst
        };
        last = (Instant::now(), s.downloaded);
        let active = s
            .connections
            .iter()
            .filter(|c| matches!(c.status, ConnStatus::Receiving))
            .count();
        if json {
            emit(json!({
                "event": "progress",
                "downloaded": s.downloaded,
                "size": s.size,
                "speed": speed as u64,
                "connections": active,
                "segments": s.segments.len(),
            }));
        } else {
            let pct = s.size.map_or(String::new(), |t| {
                format!("{:5.1}% ", 100.0 * s.downloaded as f64 / t.max(1) as f64)
            });
            print!(
                "\r{pct}{} / {}  {}/s  conns {active}  segs {}   ",
                mb(s.downloaded),
                s.size.map_or("?".into(), mb),
                mb(speed as u64),
                s.segments.len()
            );
            std::io::stdout().flush().ok();
        }
        if !matches!(
            s.state,
            State::Probing | State::Downloading | State::Completing
        ) {
            break;
        }
    }
    if !json {
        println!();
    }
    let file = dl.snapshot().file;
    match dl.wait().await {
        Ok(out) => {
            let secs = started.elapsed().as_secs_f64();
            if json {
                emit(
                    json!({ "event": "done", "path": out.path, "size": out.size, "seconds": secs }),
                );
            } else {
                println!(
                    "saved {} ({}) in {secs:.1}s",
                    out.path.display(),
                    mb(out.size)
                );
            }
            Ok(())
        }
        Err(mdm_engine::Error::Cancelled) => {
            let state = file.map(|f| mdm_engine::state::state_path(&f));
            if json {
                emit(json!({ "event": "paused", "state": state }));
            } else if let Some(state) = state {
                println!(
                    "paused. continue with:\n  {NAME} resume \"{}\"",
                    state.display()
                );
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn mb(b: u64) -> String {
    format!("{:.2} MB", b as f64 / (1 << 20) as f64)
}

/// A throwaway torrent session for the command line (its own state folder, nothing kept),
/// through Torrent settings' SOCKS5 proxy when they say to use it (#1556), as the app's.
async fn torrents() -> anyhow::Result<(mdm_torrent::Torrents, tempfile::TempDir)> {
    let proxy = match conn().torrent_proxy {
        TorrentProxy::Off => None,
        TorrentProxy::Through(p) => Some(p),
        TorrentProxy::Missing => anyhow::bail!(
            "Torrent settings say to use the proxy, and Options → Proxy / Socks has no SOCKS5 proxy: set one, or turn Use the proxy off"
        ),
    };
    let dir = tempfile::tempdir()?;
    let t = mdm_torrent::Torrents::start(mdm_torrent::Config {
        // Its own folder, nothing of the person's is there: the engine may open what it makes.
        require_authorization: false,
        proxy,
        ..mdm_torrent::Config::new(dir.path().to_owned())
    })
    .await?;
    Ok((t, dir))
}

fn torrent_source(s: &str) -> anyhow::Result<mdm_torrent::Source> {
    match mdm_torrent::Source::magnet(s) {
        Some(m) => Ok(m),
        None => Ok(mdm_torrent::Source::File(mdm_torrent::read_file(
            std::path::Path::new(s),
        )?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1556: `--proxy` takes none, system or a proxy's address (http, https, socks5, socks5h,
    /// socks4, socks4a; http when none is written); a user name in the address with the password
    /// from the environment, never a password in it.
    #[test]
    fn the_proxy_switch() {
        let manual = |url: &str, login: Option<(&str, &str)>| Proxy::Manual {
            url: url.into(),
            login: login.map(|(u, p)| (u.into(), p.into())),
            http: true,
            https: true,
        };
        assert_eq!(proxy_arg("none", None), Ok(Proxy::None));
        assert_eq!(proxy_arg(" System ", None), Ok(Proxy::System));
        assert_eq!(
            proxy_arg("127.0.0.1:7890", None),
            Ok(manual("http://127.0.0.1:7890", None))
        );
        assert_eq!(
            proxy_arg("socks5h://127.0.0.1:1080/", None),
            Ok(manual("socks5h://127.0.0.1:1080", None))
        );
        assert_eq!(
            proxy_arg("socks5://ann%40home@10.0.0.1:1080", Some("pw".into())),
            Ok(manual("socks5://10.0.0.1:1080", Some(("ann@home", "pw"))))
        );
        assert_eq!(
            proxy_arg("https://ann@proxy.corp:443", None),
            Ok(manual("https://proxy.corp", Some(("ann", ""))))
        );
        assert!(
            proxy_arg("http://ann:secret@10.0.0.1:8080", None)
                .unwrap_err()
                .contains("MDM_PROXY_PASSWORD")
        );
        assert!(proxy_arg("ftp://10.0.0.1", None).is_err());
        assert!(proxy_arg("socks5://", None).is_err());
    }

    /// #1556: a command goes through the app's network settings, its own switches over them.
    #[test]
    fn a_command_takes_the_apps_network_and_its_own_switches() {
        let net = Net {
            proxy: Proxy::System,
            dns: parse_dns("quad9-tls").unwrap(),
            adapter: String::new(),
            torrent_proxy: TorrentProxy::Through("socks5://127.0.0.1:1080".into()),
        };
        let cmd = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once(NAME).chain(args.iter().copied()))
                .unwrap()
                .cmd
        };
        let c = Conn::of(&cmd(&["get", "http://a.test/f"]), &net);
        assert_eq!(
            (c.proxy, c.dns, c.route),
            (net.proxy.clone(), net.dns.clone(), mdm_engine::Route::Any)
        );
        let c = Conn::of(
            &cmd(&[
                "get",
                "http://a.test/f",
                "--proxy",
                "none",
                "--dns",
                "system",
            ]),
            &net,
        );
        assert_eq!(
            (c.proxy, c.dns),
            (Proxy::None, mdm_engine::DnsChoice::default())
        );
        let c = Conn::of(&cmd(&["install-ytdlp", "--proxy", "socks5h://h:1"]), &net);
        assert!(matches!(c.proxy, Proxy::Manual { ref url, .. } if url == "socks5h://h:1"));
        let c = Conn::of(&cmd(&["video-info", "https://v.test/w"]), &net);
        assert_eq!(c.proxy, Proxy::System);
        assert_eq!(
            Conn::of(&cmd(&["torrent-info", "x.torrent"]), &net).torrent_proxy,
            net.torrent_proxy
        );
        // An adapter that isn't there: nothing goes out.
        let c = Conn::of(
            &cmd(&[
                "get",
                "http://a.test/f",
                "--adapter",
                "no-such-adapter-1556",
            ]),
            &net,
        );
        assert!(
            matches!(c.route, mdm_engine::Route::Down { .. }),
            "{:?}",
            c.route
        );
        let c = Conn::of(&cmd(&["resume", "file.mdm.json"]), &net);
        assert_eq!(
            (c.proxy, c.dns, c.route),
            (net.proxy.clone(), net.dns.clone(), mdm_engine::Route::Any)
        );
        let c = Conn::of(
            &cmd(&[
                "resume",
                "file.mdm.json",
                "--proxy",
                "none",
                "--dns",
                "system",
            ]),
            &net,
        );
        assert_eq!(
            (c.proxy, c.dns),
            (Proxy::None, mdm_engine::DnsChoice::default())
        );
        let c = Conn::of(
            &cmd(&[
                "resume",
                "file.mdm.json",
                "--adapter",
                "no-such-adapter-1556",
            ]),
            &net,
        );
        assert!(
            matches!(c.route, mdm_engine::Route::Down { .. }),
            "{:?}",
            c.route
        );
    }

    /// #1240: `video-get --quality` takes a height as people write it.
    #[test]
    fn a_quality_is_a_height() {
        assert_eq!(parse_height("1080"), Ok(1080));
        assert_eq!(parse_height("720p"), Ok(720));
        assert_eq!(parse_height(" 4320P "), Ok(4320));
        assert!(parse_height("0").is_err());
        assert!(parse_height("best").is_err());
        assert!(parse_height("-1").is_err());
        let cli = Cli::try_parse_from([NAME, "video-get", "https://v.example/1", "-q", "720p"]);
        assert!(matches!(
            cli.map(|c| c.cmd),
            Ok(Cmd::VideoGet {
                quality: Some(Quality::Height(720)),
                format: None,
                ..
            })
        ));
        // #1439: the most compatible, by its name.
        assert_eq!(parse_quality("compatible"), Ok(Quality::Compatible));
        assert_eq!(parse_quality(" Compatible "), Ok(Quality::Compatible));
        assert_eq!(parse_quality("1080p"), Ok(Quality::Height(1080)));
        assert!(parse_quality("best").unwrap_err().contains("compatible"));
        // One or the other.
        assert!(Cli::try_parse_from([NAME, "video-get", "u", "-q", "720", "-f", "b"]).is_err());
    }

    /// #1240: a page like Bilibili's (pictures and sounds apart): with ffmpeg, the best picture
    /// joined to the best sound; without, no video to give, said with what the page offers.
    #[test]
    fn video_get_picks_as_the_window_does() {
        let v = mdm_tools::resolve::parse(
            br#"{"title": "A video", "formats": [
            {"format_id": "30216", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "filesize": 1200000},
            {"format_id": "100022", "ext": "mp4", "height": 288, "vcodec": "av01.0.00M.08", "acodec": "none", "filesize": 4890000},
            {"format_id": "30016", "ext": "mp4", "height": 288, "vcodec": "avc1.64001E", "acodec": "none", "filesize": 5380000}]}"#,
        )
        .unwrap();
        let merged = mdm_tools::resolve::choices(&v, true);
        assert_eq!(
            pick_quality(&v, &merged, None, true, true).unwrap(),
            "100022+30216/bv*[height=288][vcodec^=av01]+ba/bv*[height=288]+ba/b[height=288]"
        );
        assert_eq!(
            pick_quality(&v, &merged, Some(Quality::Height(4320)), true, true).unwrap(),
            "100022+30216/bv*[height=288][vcodec^=av01]+ba/bv*[height=288]+ba/b[height=288]"
        );
        let apart = mdm_tools::resolve::choices(&v, false);
        let e = pick_quality(&v, &apart, None, false, true).unwrap_err();
        let n = e.downcast_ref::<NoFormat>().expect("the formats said");
        assert!(n.why.contains("needs ffmpeg"), "{}", n.why);
        let text = n.to_string();
        assert!(text.contains("-f \"30216/ba\""), "{text}");
        let json = n.json();
        assert_eq!(json["choices"][0]["format"], "30216/ba");
        assert_eq!(json["formats"].as_array().unwrap().len(), 3);
        assert_eq!(json["formats"][1]["id"], "100022");
        assert_eq!(json["formats"][1]["audio"], false);
        // Nothing at all on the page: yt-dlp's own formats, or that there are none.
        let empty = mdm_tools::resolve::parse(br#"{"title": "T", "formats": []}"#).unwrap();
        let e = pick_quality(&empty, &[], None, true, true).unwrap_err();
        assert!(e.to_string().ends_with("the page lists no format"), "{e}");
        // #1439: the most compatible: its H.264 picture (30016) with the AAC sound, not the AV1
        // one the best is; a page without one says so, with what it offers.
        let compatible = Some(Quality::Compatible);
        assert_eq!(
            pick_quality(&v, &merged, compatible, true, true).unwrap(),
            "30016+30216/bv[vcodec^=avc][height<=288]+ba[acodec^=mp4a]\
             /b[vcodec^=avc][acodec^=mp4a][height<=288]"
        );
        let e = pick_quality(&v, &apart, compatible, false, true).unwrap_err();
        assert!(e.to_string().contains("needs ffmpeg"), "{e}");
        let av1 = mdm_tools::resolve::parse(
            br#"{"title": "A video", "formats": [
            {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "filesize": 1200000},
            {"format_id": "399", "ext": "mp4", "height": 1080, "vcodec": "av01.0.08M.08", "acodec": "none", "filesize": 4890000}]}"#,
        )
        .unwrap();
        let offered = mdm_tools::resolve::choices(&av1, true);
        let e = pick_quality(&av1, &offered, compatible, true, true).unwrap_err();
        let n = e.downcast_ref::<NoFormat>().expect("the formats said");
        assert!(n.why.contains("no H.264 video with AAC sound"), "{}", n.why);
        assert!(n.to_string().contains("1080p MP4 · AV1"), "{n}");
    }

    #[test]
    fn yt_dlps_advice_is_left_out() {
        assert_eq!(
            without_list_advice(
                "yt-dlp failed: [BiliBili] BV1_p1: Requested format is not available. Use --list-formats for a list of available formats"
            ),
            "yt-dlp failed: [BiliBili] BV1_p1: Requested format is not available."
        );
        assert_eq!(without_list_advice("other"), "other");
    }

    /// #668: only the commands that download a link wait for the default browser's User-Agent.
    #[test]
    fn only_downloads_wait_for_the_default_browser() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(downloads(&args(&[
            "get",
            "https://example.test/a.zip",
            "-o",
            "."
        ])));
        assert!(downloads(&args(&[
            "--json",
            "get",
            "https://example.test/a.zip"
        ])));
        assert!(downloads(&args(&["resume", "a.zip.mdm.json"])));
        assert!(!downloads(&args(&["start-queue"])));
        assert!(!downloads(&args(&["stop-queue"])));
        assert!(!downloads(&args(&["add", "https://example.test/a.zip"])));
        assert!(!downloads(&args(&["api", "on"])));
        assert!(!downloads(&args(&["status"])));
        assert!(!downloads(&args(&["--help"])));
        assert!(!downloads(&args(&[])));
    }

    #[test]
    fn queue_commands_parse() {
        let cli = Cli::try_parse_from([NAME, "start-queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StartQueue {
                queue: None,
                name: None
            }
        ));

        let cli = Cli::try_parse_from([NAME, "start-queue", "Night queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StartQueue {
                queue: None,
                name: Some(ref n),
            } if n == "Night queue"
        ));

        let cli = Cli::try_parse_from([NAME, "start-queue", "-q", "Night queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StartQueue {
                queue: Some(ref q),
                name: None,
            } if q == "Night queue"
        ));

        let cli = Cli::try_parse_from([NAME, "stop-queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StopQueue {
                queue: None,
                name: None
            }
        ));

        let cli = Cli::try_parse_from([NAME, "stop-queue", "Night queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StopQueue {
                queue: None,
                name: Some(ref n),
            } if n == "Night queue"
        ));

        let cli = Cli::try_parse_from([NAME, "stop-queue", "--queue", "Night queue"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::StopQueue {
                queue: Some(ref q),
                name: None,
            } if q == "Night queue"
        ));

        // Both positional and option conflict.
        assert!(Cli::try_parse_from([NAME, "start-queue", "-q", "A", "B"]).is_err());
        assert!(Cli::try_parse_from([NAME, "stop-queue", "--queue", "A", "B"]).is_err());
    }

    #[test]
    fn add_command_parses() {
        let cli = Cli::try_parse_from([NAME, "add", "https://example.test/a.zip"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                output: None,
                name: None,
                category: None,
                queue: None,
                checksum: None,
                referer: None,
                user_agent: None,
                cookie: None,
                later: false,
                silent: false,
                private: false,
            } if url == "https://example.test/a.zip"
        ));

        let sha256 = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "magnet:?xt=urn:btih:abc",
            "-o",
            "D:\\Downloads",
            "-f",
            "my_torrent",
            "-c",
            "Torrents",
            "-q",
            "Night",
            "--checksum",
            sha256,
            "-l",
            "-s",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                output: Some(ref o),
                name: Some(ref n),
                category: Some(ref c),
                queue: Some(ref q),
                checksum: Some(ref h),
                referer: None,
                user_agent: None,
                cookie: None,
                later: true,
                silent: true,
                private: false,
            } if url == "magnet:?xt=urn:btih:abc"
                && o == &PathBuf::from("D:\\Downloads")
                && n == "my_torrent"
                && c == "Torrents"
                && q == "Night"
                && h == sha256
        ));

        let cmd = match cli.cmd {
            Cmd::Add {
                url,
                output,
                name,
                category,
                queue,
                checksum,
                referer,
                user_agent,
                cookie,
                later,
                silent,
                private,
            } => add_command(
                url, output, name, category, queue, checksum, referer, user_agent, cookie, later,
                silent, private,
            )
            .unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(cmd.checksum.as_deref(), Some(sha256));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "-r",
            "https://example.test/page.html",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                referer: Some(ref r),
                ..
            } if url == "https://example.test/file.zip" && r == "https://example.test/page.html"
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "--referrer",
            "https://example.test/page.html",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                referer: Some(ref r),
                ..
            } if url == "https://example.test/file.zip" && r == "https://example.test/page.html"
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "--user-agent",
            "CustomAgent/1.0",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                user_agent: Some(ref u),
                ..
            } if url == "https://example.test/file.zip" && u == "CustomAgent/1.0"
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "--ua",
            "CustomAgent/1.0",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                user_agent: Some(ref u),
                ..
            } if url == "https://example.test/file.zip" && u == "CustomAgent/1.0"
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "--cookie",
            "session=abc123",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                cookie: Some(ref c),
                ..
            } if url == "https://example.test/file.zip" && c == "session=abc123"
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "add",
            "https://example.test/file.zip",
            "--cookies",
            "session=abc123",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Add {
                ref url,
                cookie: Some(ref c),
                ..
            } if url == "https://example.test/file.zip" && c == "session=abc123"
        ));
    }

    #[test]
    fn add_command_makes_relative_output_folder_absolute() {
        let rel = PathBuf::from("downloads");
        let expected = std::path::absolute(&rel)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let cmd = add_command(
            "https://example.test/a.zip".into(),
            Some(rel),
            None,
            None,
            None,
            None,
            Some("https://example.test/source".into()),
            None,
            None,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(cmd.dir.as_deref(), Some(expected.as_str()));
        assert_eq!(cmd.referer.as_deref(), Some("https://example.test/source"));
        assert_eq!(cmd.user_agent, None);
        assert_eq!(cmd.cookies, None);
        assert!(!cmd.private);
    }

    #[test]
    fn add_command_reads_cookie_from_stdin() {
        let cmd = add_command_with(
            "https://example.test/a.zip".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("-".into()),
            false,
            false,
            false,
            std::io::Cursor::new("session=secret_token_123\r\n"),
        )
        .unwrap();
        assert_eq!(cmd.cookies.as_deref(), Some("session=secret_token_123"));
        // A script's add with cookies is not the browser's own hand-off.
        assert!(!cmd.browser_handoff);
        assert!(!cmd.private);

        let empty = add_command_with(
            "https://example.test/a.zip".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("-".into()),
            false,
            false,
            false,
            std::io::Cursor::new("   \r\n"),
        )
        .unwrap();
        assert_eq!(empty.cookies, None);
        assert!(!empty.private);
    }

    #[test]
    fn add_command_parses_private() {
        for flag in ["--private", "-p"] {
            let cli =
                Cli::try_parse_from([NAME, "add", "https://example.test/file.zip", flag]).unwrap();
            assert!(matches!(
                cli.cmd,
                Cmd::Add {
                    ref url,
                    private: true,
                    ..
                } if url == "https://example.test/file.zip"
            ));
        }

        let cmd = add_command(
            "https://example.test/file.zip".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            true,
        )
        .unwrap();
        assert!(cmd.private);
    }

    #[test]
    fn get_and_resume_commands_parse_headers_options() {
        let cli = Cli::try_parse_from([
            NAME,
            "get",
            "https://example.test/a.zip",
            "--referer",
            "https://example.test/page.html",
            "--user-agent",
            "CustomAgent/1.0",
            "--cookie",
            "-",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Get {
                ref referer,
                ref user_agent,
                ref cookie,
                ..
            } if referer.as_deref() == Some("https://example.test/page.html")
                && user_agent.as_deref() == Some("CustomAgent/1.0")
                && cookie.as_deref() == Some("-")
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "get",
            "https://example.test/a.zip",
            "-r",
            "https://example.test/page.html",
            "--ua",
            "CustomAgent/1.0",
            "--cookies",
            "session=abc",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Get {
                ref referer,
                ref user_agent,
                ref cookie,
                ..
            } if referer.as_deref() == Some("https://example.test/page.html")
                && user_agent.as_deref() == Some("CustomAgent/1.0")
                && cookie.as_deref() == Some("session=abc")
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "resume",
            "test.mdm.json",
            "-r",
            "https://example.test/source",
            "--user-agent",
            "MyAgent/2.0",
            "--cookie",
            "-",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Resume {
                ref referer,
                ref user_agent,
                ref cookie,
                ..
            } if referer.as_deref() == Some("https://example.test/source")
                && user_agent.as_deref() == Some("MyAgent/2.0")
                && cookie.as_deref() == Some("-")
        ));
    }

    #[test]
    fn resume_command_parses_checksum_dns_and_adapter() {
        let sha256_str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let expected_checksum = parse_checksum(sha256_str).unwrap();
        let expected_dns = parse_dns("cloudflare-https").unwrap();
        let cli = Cli::try_parse_from([
            NAME,
            "resume",
            "test.mdm.json",
            "--checksum",
            sha256_str,
            "--dns",
            "cloudflare-https",
            "--adapter",
            "Ethernet",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Resume {
                checksum: Some(checksum),
                dns: Some(dns),
                adapter: Some(adapter),
                ..
            } => {
                assert_eq!(adapter, "Ethernet");
                assert_eq!(dns, expected_dns);
                assert_eq!(checksum, expected_checksum);
            }
            _ => panic!("expected Cmd::Resume with checksum, dns, and adapter"),
        }
    }

    #[test]
    fn build_headers_combines_options_and_stdin_cookie() {
        let raw = vec!["X-Custom: custom-value".to_string()];
        let headers = build_headers_with(
            &raw,
            Some("https://example.test/from".into()),
            Some("Agent/1.0".into()),
            Some("-".into()),
            std::io::Cursor::new("session=abc123xyz\n"),
        )
        .unwrap();

        assert_eq!(headers.get("x-custom").unwrap(), "custom-value");
        assert_eq!(headers.get(REFERER).unwrap(), "https://example.test/from");
        assert_eq!(headers.get(USER_AGENT).unwrap(), "Agent/1.0");
        assert_eq!(headers.get(COOKIE).unwrap(), "session=abc123xyz");

        let empty_stdin_headers = build_headers_with(
            &[],
            None,
            None,
            Some("-".into()),
            std::io::Cursor::new("   \r\n"),
        )
        .unwrap();
        assert_eq!(empty_stdin_headers.get(COOKIE), None);
    }

    #[test]
    fn status_command_parses() {
        let cli = Cli::try_parse_from([NAME, "status"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Status {
                id: None,
                status: None,
                search: None,
                category: None,
                queue: None,
                limit: None,
            }
        ));

        let cli = Cli::try_parse_from([NAME, "list"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Status {
                id: None,
                status: None,
                search: None,
                category: None,
                queue: None,
                limit: None,
            }
        ));

        let cli = Cli::try_parse_from([
            NAME,
            "status",
            "--id",
            "42",
            "-s",
            "Downloading",
            "-c",
            "Music",
            "-q",
            "Main download queue",
            "--search",
            "audio",
            "-l",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Status {
                id: Some(42),
                status: Some(ref s),
                category: Some(ref c),
                queue: Some(ref q),
                search: Some(ref search),
                limit: Some(5),
            } if s == "Downloading" && c == "Music" && q == "Main download queue" && search == "audio"
        ));
    }

    /// A service that takes `hoster.test` only, or (`broken`) can't say which hosts it takes.
    #[derive(Debug)]
    struct Service;

    impl mdm_engine::Unlocker for Service {
        fn name(&self) -> &str {
            "Fake-Debrid"
        }

        fn unlock<'a>(
            &'a self,
            _: &'a url::Url,
            _: &'a (dyn Fn(String, Option<f32>) + Send + Sync),
            _: &'a tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<mdm_engine::Unlocked>, mdm_engine::ServiceFault>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
    }

    struct Fake {
        broken: bool,
    }

    impl Accounts for Fake {
        fn unlocker<'a>(&'a self, url: &'a url::Url, _: &'a Options) -> Unlocking<'a> {
            Box::pin(async move {
                if self.broken {
                    return Err("Fake-Debrid didn't say".into());
                }
                Ok((url.host_str() == Some("hoster.test")).then(|| {
                    std::sync::Arc::new(Service) as std::sync::Arc<dyn mdm_engine::Unlocker>
                }))
            })
        }
    }

    #[tokio::test]
    async fn downloads_go_through_the_apps_accounts_unless_direct() {
        let host: url::Url = "http://hoster.test/file/1".parse().unwrap();
        let other: url::Url = "http://example.test/a.zip".parse().unwrap();
        let fake = Fake { broken: false };
        let via = |u, direct, a: Option<&'static Fake>| {
            let u: &url::Url = u;
            let u = u.clone();
            async move {
                through_account(a.map(|a| a as &dyn Accounts), &u, direct, opts(1), true)
                    .await
                    .unwrap()
                    .unlocker
                    .map(|u| u.name().to_owned())
            }
        };
        let fake: &'static Fake = Box::leak(Box::new(fake));
        assert_eq!(
            via(&host, false, Some(fake)).await.as_deref(),
            Some("Fake-Debrid")
        );
        // --direct, another host, no accounts (the plain library), a service that can't say.
        assert_eq!(via(&host, true, Some(fake)).await, None);
        assert_eq!(via(&other, false, Some(fake)).await, None);
        assert_eq!(via(&host, false, None).await, None);
        let broken: &'static Fake = Box::leak(Box::new(Fake { broken: true }));
        assert_eq!(via(&host, false, Some(broken)).await, None);
    }

    #[test]
    fn pair_command_parses() {
        let cli = Cli::try_parse_from(["misterdownloadmanager-cli", "pair"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Pair));
        assert!(!cli.json);

        let cli_json =
            Cli::try_parse_from(["misterdownloadmanager-cli", "--json", "pair"]).unwrap();
        assert!(matches!(cli_json.cmd, Cmd::Pair));
        assert!(cli_json.json);
    }

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_launcher_flag_starts_the_command_line() {
        let l = launched(args(&["mdm", "--cli", "get", "x"])).unwrap();
        assert_eq!((l.launcher, l.args), (None, args(&["get", "x"])));
        let l = launched(args(&["mdm", "--cli=42", "status"])).unwrap();
        assert_eq!((l.launcher, l.args), (Some(42), args(&["status"])));
        // The app's own command line, and look-alikes, start the app.
        assert!(launched(args(&["mdm"])).is_none());
        assert!(launched(args(&["mdm", "/d", "--cli"])).is_none());
        assert!(launched(args(&["mdm", "--cli=x"])).is_none());
        assert!(launched(args(&["mdm", "--client"])).is_none());
        // A link named as the launcher (Linux and macOS).
        let named = launched(args(&["/usr/bin/misterdownloadmanager-cli", "status"]));
        assert_eq!(named.is_some(), cfg!(unix));
    }
}
