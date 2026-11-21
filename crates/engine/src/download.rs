//! One download: probe, parallel connections driven by the planner, positional writes, progress.
//!
//! The first connection is also the probe: a plain `GET` (no Range, as IDM does — D-007) that
//! keeps streaming from byte 0 while further connections split the file (notes/ENGINE.md §1).
//! Range support is learned from the first range answer: 206 confirms it; 200 means the server
//! ignores ranges, so the file goes back to the probe connection alone.
//! One HTTP/1.1 client per download, shared by its connections: one pool (keep-alive sockets
//! reused between segments, TLS sessions resumed) with at most `connections()` requests at a
//! time, since each worker has one request in flight.

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::disk;
use crate::dns;
use crate::html;
use crate::limiter::Limiter;
use crate::naming;
use crate::net;
use crate::planner::{Claim, Planner, WorkerId};
use crate::sites::{self, PageSays, Refusal};
use crate::sparse;
use crate::state::{ResumeState, Validators, state_path};

/// Bytes buffered per connection before claiming and writing them. Each batch is one buffer of
/// exactly this capacity, filled and never grown (`Writer::push`): what a connection holds in
/// memory is what it counts (`Writer::in_memory`).
const BATCH: usize = 256 * 1024;
/// Written batches a connection keeps for the next ones rather than freeing them and asking
/// the allocator again (`Writer::spare`): one write's worth.
const SPARE_MAX: usize = WRITE_MAX / BATCH;
/// Received bytes are written at least this often, so a slow connection shows progress and a
/// pause keeps what arrived.
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// While the disk takes a connection's write, what it receives meanwhile waits in memory, at
/// most this much for the whole download (`Writer`), and only then does it stand still for the
/// disk: a flush on NTFS holds every write to the file (measured: 3-10 s on a slow disk, #361),
/// and a disk slower than the network holds writes to its pace once the system's cache is full
/// (#782). About 2 s of a 1 Gbit/s link: room for the first flush, which can only be guessed
/// (`Synced::fits` keeps the others shorter).
const HOLD_MAX: u64 = 256 << 20;
/// The most one write of a connection carries (`Writer::send`): what waited in memory goes to
/// a disk that sets the pace in steps this small, so the waits stay short and even.
const WRITE_MAX: usize = 4 << 20;
/// A connection that stood still this long for the disk tells the others the disk sets the
/// pace (`too_slow`).
const DISK_HELD: Duration = Duration::from_millis(50);
/// A response's speed says what one connection to this server can do once it has run this long
/// (and brought 256 KiB, `yardstick`): long enough that what was already on its way when it
/// began doesn't count for much.
const BEST_AFTER: Duration = Duration::from_millis(100);
/// What the connections of a download bring together is measured over windows this long
/// (`note_link`): the best one says what the link to this server carries, and the share of it
/// each receiving connection gets is what one connection can do while they share it (#896).
const LINK_WINDOW: Duration = Duration::from_secs(1);

/// Shedding a slow connection (`Options::shed_slow`): after `SHED_GRACE` on a response, a
/// connection under 1/`SHED_RATIO` of what one connection can do (`shed_bar`), with more than
/// `SHED_LEFT` of work left at its own pace, reconnects. A slow TCP path (or a slow server
/// slot) rarely survives a new request, and otherwise every free connection can end up stuck
/// on one (measured: 14 s instead of ~2 s for 32 MiB when every third request was slow).
const SHED_GRACE: Duration = Duration::from_secs(2);
const SHED_RATIO: u64 = 4;
const SHED_LEFT: Duration = Duration::from_secs(2);
/// A download starts with one connection per this many bytes (at most `max_connections`).
const INITIAL_PIECE: u64 = 64 * 1024;
/// A pending connection is taken over after this many first-byte times of the probe (and never
/// sooner than PREEMPT_MIN).
const PREEMPT_TTFB: u32 = 2;
const PREEMPT_MIN: Duration = Duration::from_millis(20);
/// The most connections a download offers to use (IDM's ceiling too).
pub const MAX_CONNECTIONS: u32 = 32;
/// The most connections once the user allows more than `MAX_CONNECTIONS` (issue #12); the
/// engine never opens more, whatever it is given.
pub const MAX_CONNECTIONS_UNLOCKED: u32 = 128;

#[derive(Clone, Debug)]
pub struct Options {
    /// Max simultaneous connections. IDM's default is 8 (notes/ENGINE.md §4). Read through
    /// [`Options::connections`], which keeps it in `1..=MAX_CONNECTIONS_UNLOCKED`.
    pub max_connections: u32,
    /// Smallest half a split may leave. IDM split pieces down to ≤ 12.6 KB (lab S4, D-007).
    pub min_split: u64,
    pub connect_timeout: Duration,
    /// Max silence on a connection before it is considered dead.
    pub read_timeout: Duration,
    /// Consecutive failures after which a connection gives up (it retires if others are
    /// still working, otherwise the download fails).
    pub max_retries: u32,
    /// The first retry is immediate (as IDM, lab S5); then this delay, doubling, capped at 30 s.
    pub retry_delay: Duration,
    /// The download's User-Agent: by default the automatic one (`agent::automatic`, the default
    /// browser's), else the user's or the one a browser handed over.
    pub user_agent: String,
    /// `user_agent` is MDM's automatic one, not one the user set or a browser handed over: a
    /// site that wants an agent of its own (Google Drive's hosts, `sites::user_agent`) gets it.
    /// [`Options::speak_as`] sets another agent and clears this.
    pub automatic_agent: bool,
    /// Asked with instead of `user_agent` when the server refuses the first request with 403
    /// (#372): a browser's User-Agent on a connection that isn't a browser's trips some sites'
    /// bot checks (Cloudflare, seen on a public PDF), while MDM's own passes. Set when
    /// `user_agent` is the browser's. A later 403 is left to the download.
    pub agent_fallback: Option<String>,
    /// The download's last try at a site whose bot check refused it (`refused_as_a_program`)
    /// and the browser-like connection too (#668): its first request once more, on MDM's own
    /// connection, with this User-Agent instead (#1422). MDM's own name
    /// (`agent::OWN_USER_AGENT`): SourceForge's check lets programs that say what they are
    /// through, and refuses whatever claims to be a browser without connecting like one. The
    /// site is then remembered for the session (`agent::own_for`): its next downloads ask this
    /// way at once. None: no such try (the download's User-Agent is one the user chose, or a
    /// stream's that must stay its resolver's); a User-Agent in the request's own headers is
    /// the caller's choice too, never replaced.
    pub own_agent: Option<String>,
    /// From the browser: the address its own download of this file ended at, its redirects
    /// followed. When the address asked for is refused before any byte (`link_refused`: a
    /// one-time link the browser already spent, a token bound to its session, a site that
    /// turns away whatever isn't that browser, or answers with its page, #701), the download
    /// asks there once instead (#630),
    /// the address's cookies and logins going only if it is the same site (`scoped_headers`).
    /// Never for a form's POST (its body goes where it was given) or through a service account.
    pub browser_final: Option<Url>,
    /// How often resume state is saved (IDM: "several times per minute").
    pub save_interval: Duration,
    /// Speed limit for this download, bytes/s (IDM's limiter is per file). None = unlimited.
    pub speed_limit: Option<u64>,
    /// Before continuing a segment that already has data on disk, re-fetch up to this many of
    /// its last written bytes and compare them (IDM's resume overlap check; catches changed
    /// files on servers without validators). 0 turns it off.
    pub verify_overlap: u64,
    /// Fixed addresses for host names, like hosts-file entries (tests).
    pub hosts: Vec<(String, Vec<std::net::IpAddr>)>,
    /// Options → Connection → DNS: who answers the names (#36).
    pub dns: crate::dns::DnsChoice,
    /// Options → Proxy / Socks → Network adapter (#35): the adapter every connection goes
    /// through, or none at all.
    pub route: crate::net::Route,
    /// A service account that unlocks this download's link first (#44, `crate::unlock`): asked
    /// before the first request and when the direct link dies. Not in the resume state (like
    /// the headers): whoever resumes gives it again.
    pub unlocker: Option<Arc<dyn crate::unlock::Unlocker>>,
    /// Options → Proxy / Socks.
    pub proxy: Proxy,
    /// A limit shared by every download using the same limiter (a global speed cap), on top
    /// of the download's own.
    pub global: Option<Arc<Limiter>>,
    /// The finished file must have this hash (else the server's announced one, if any).
    pub checksum: Option<crate::verify::Hash>,
    /// Reconnect a connection far slower than the others (see `SHED_*`). Beyond IDM's rule.
    pub shed_slow: bool,
    /// Connection caps learned per host, shared by the downloads that use the same table.
    pub host_limits: Option<Arc<crate::hosts::HostLimits>>,
    /// Resume across a changed modification time (IDM: "Ignore file modification time changes
    /// when resuming a download"): servers that stamp every answer with a new date would
    /// otherwise make each resume start over. Only an ETag then tells a changed file.
    pub ignore_mtime: bool,
    /// Give the finished file the server's date (IDM: "Set file creation date as provided by
    /// the server"): created and modified times from `Last-Modified`.
    pub server_time: bool,
    /// The user chose to replace the file already at the destination (#1088: a system Save
    /// dialog's "Replace?" answered Yes): a file there doesn't count as taken, and it is replaced
    /// only as the download completes, so a download that fails or is cancelled leaves it as it
    /// was. One that can't be replaced then (open in another program, read-only) is kept, and
    /// the download takes the next free numbered name. Off: a file there is never replaced.
    /// [`Destination::Dir`] ignores it (the server names that file).
    pub replace_existing: bool,
    /// FTP data connections: passive (the client connects; IDM's "Use passive mode for FTP",
    /// on by default), else active (the server connects back).
    pub ftp_passive: bool,
    /// Certificates (DER) trusted besides the system's for FTPS and HTTPS (tests' own CA).
    pub extra_roots: Vec<Vec<u8>>,
    /// Send every request as POST with this body, the Content-Type coming from the headers (a
    /// browser form's download). Redirects go as in browsers: 301, 302 and 303 turn it into a
    /// GET without the body (and without its Content-Type), 307 and 308 keep both. A POST may
    /// have side effects, so further connections repeat it only once an answer has shown that
    /// the server takes ranges (206 or `Accept-Ranges: bytes`); until then the first
    /// connection takes the whole file alone. Not stored in the resume state (like the
    /// headers): whoever resumes gives it again. HTTP only.
    pub post: Option<Bytes>,
    /// Called with the part file's path right after the engine creates it, before any data
    /// is written: once per new download (`Download::start`, `fetch`), never by a resume, which
    /// writes into the part file it finds. The part file is renamed (never copied) to the final
    /// name, so whatever the hook attaches to it — Windows' Mark of the Web stream, macOS'
    /// quarantine attribute — stays with the finished file.
    pub on_file_created: Option<FileHook>,
    /// From the browser: the MIME type its own download had. A web page where it had another
    /// type is the server refusing MDM (a firewall's "The requested URL was rejected"), an error
    /// and never saved as the file (#533, `expects_other_file`).
    pub expected_type: Option<String>,
    /// From the browser: the size its own download announced. A file larger than any waiting
    /// page: a web page in its place is the site's (a countdown, a captcha), never the file
    /// (#701, `expects_other_file`).
    pub expected_size: Option<u64>,
    /// From the browser: the title of the page a media file was played on (#800). A download
    /// the engine names ([`Destination::Dir`]) whose address names nothing (a script, a bare id,
    /// `videoplayback`) is named by it ([`naming::answer_name`]).
    pub page_title: Option<String>,
    /// From the browser (#607): the web page the download was started from, whole (its tab's
    /// address), where the Referer is often the site's address alone. A site whose file
    /// addresses expire and are given on that page (Rapidgator's file page, `sites::origin`) is
    /// asked there again for a fresh one when the address stops working. Not in the resume state
    /// (like the headers): whoever resumes gives it again.
    pub page: Option<Url>,
    /// The most one request asks for (bytes): every request, the first one too, is a range of
    /// at most this many, the next one following on the same connection. For servers that slow
    /// a long answer down (YouTube's media servers serve 10 MB ranges at full speed and a whole
    /// file at a fraction of it: yt-dlp's `http_chunk_size`, #769). None: each connection asks
    /// for its whole part at once. HTTP only, never with a POST.
    pub request_cap: Option<u64>,
    /// Tests and measurements: a disk slower than the network, behind a write cache, whose
    /// flush holds every write to the file (as NTFS's does, #361, #782). None: the real disk.
    #[doc(hidden)]
    pub slow_disk: Option<crate::slowdisk::SlowDisk>,
}

/// A callback given the path of a file the engine created (`Options::on_file_created`).
#[derive(Clone)]
pub struct FileHook(pub Arc<dyn Fn(&Path) + Send + Sync>);

impl FileHook {
    pub fn new(f: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        FileHook(Arc::new(f))
    }
}

impl std::fmt::Debug for FileHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileHook")
    }
}

/// Which proxy downloads go through (Options → Proxy / Socks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Proxy {
    /// "No proxy/socks" (IDM's default): direct connections.
    #[default]
    None,
    /// "Use system settings": Windows' proxy (and the environment's).
    System,
    /// "Manual proxy/socks configuration": `http://host:port` (or `socks5://…`), with an
    /// optional login, for the listed schemes.
    Manual {
        url: String,
        login: Option<(String, String)>,
        http: bool,
        https: bool,
    },
    /// "Use automatic configuration script": the script decides per address (`pac`); one
    /// that doesn't run leaves it to the system's settings.
    Pac { script: Arc<str> },
}

/// Whether the engine can download `url`: http, https, ftp, ftps (implicit TLS) and ftpes
/// (explicit TLS). Magnet links are the torrent engine's.
pub fn supported(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") || crate::ftp::is_ftp(url)
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_connections: 8,
            min_split: 12 * 1024,
            connect_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            max_retries: 5,
            retry_delay: Duration::from_secs(1),
            user_agent: crate::agent::automatic(),
            automatic_agent: true,
            agent_fallback: None,
            own_agent: None,
            browser_final: None,
            save_interval: Duration::from_secs(2),
            speed_limit: None,
            verify_overlap: 64,
            hosts: Vec::new(),
            dns: Default::default(),
            route: Default::default(),
            unlocker: None,
            proxy: Proxy::None,
            global: None,
            checksum: None,
            shed_slow: true,
            host_limits: None,
            ignore_mtime: false,
            server_time: false,
            replace_existing: false,
            ftp_passive: true,
            extra_roots: Vec::new(),
            post: None,
            on_file_created: None,
            expected_type: None,
            expected_size: None,
            page_title: None,
            page: None,
            request_cap: None,
            slow_disk: None,
        }
    }
}

impl Options {
    /// Speak as `agent` (the user's, a browser's) instead of the automatic User-Agent.
    pub fn speak_as(&mut self, agent: String) {
        self.user_agent = agent;
        self.automatic_agent = false;
    }

    /// A site's own User-Agent may replace this download's (`request_agent`): the agent is the
    /// automatic one, or MDM's former one (#372's fallback, D-007), as before #668.
    fn site_agent(&self) -> bool {
        self.automatic_agent || self.user_agent == crate::agent::IE11_USER_AGENT
    }

    /// The connections this download may open: `max_connections` in
    /// `1..=MAX_CONNECTIONS_UNLOCKED` (a hand-edited setting can hold anything).
    pub fn connections(&self) -> u32 {
        self.max_connections.clamp(1, MAX_CONNECTIONS_UNLOCKED)
    }

    /// `request_cap`, when it applies: some bytes, without a form's POST.
    fn cap(&self) -> Option<u64> {
        self.request_cap.filter(|c| *c > 0 && self.post.is_none())
    }
}

#[derive(Clone, Debug)]
pub enum Destination {
    /// Save into this folder under the name the server/URL suggests (numbered if taken).
    Dir(PathBuf),
    /// Save exactly here.
    File(PathBuf),
    /// Save here, or under the next free numbered name in its folder (`naming::unique_path`)
    /// when this one is taken as the download starts: a name given before its folder was looked
    /// at (#743: a window that waits for a sleeping drive freezes), decided here, beside it.
    Free(PathBuf),
}

impl Destination {
    /// The folder the download is saved in.
    pub fn folder(&self) -> &Path {
        match self {
            Destination::Dir(dir) => dir,
            Destination::File(p) | Destination::Free(p) => p.parent().unwrap_or(Path::new("")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    /// Extra headers sent on every request (Referer, Cookie, Authorization …).
    pub headers: HeaderMap,
    pub dest: Destination,
}

/// Cloneable (the network and disk errors it wraps are shared), so a snapshot can carry the
/// failure itself for an interface to word in its own language (`Snapshot::failure`).
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("{}", describe_http(.0))]
    Http(Arc<reqwest::Error>),
    #[error("server answered {0}")]
    Status(StatusCode),
    /// The site's bot check refused the request: it lets browsers through, not programs (#376).
    /// Cloudflare says so in its answer (`cf-mitigated: challenge`).
    #[error("the site's bot check refused the download")]
    BotCheck,
    /// 429 / 503: the server is busy; retry after its `Retry-After` when it gives one.
    #[error("server busy ({0})")]
    Throttled(StatusCode, Option<Duration>),
    #[error("server sent an unexpected range: {0}")]
    BadRange(String),
    #[error("connection closed before the segment was complete")]
    ShortBody,
    #[error("the file changed on the server")]
    FileChanged,
    /// The server ignores ranges and the probe connection can't take the file back alone.
    #[error("the server does not support ranges")]
    NoRanges,
    #[error("cannot resume: {0}")]
    BadState(String),
    #[error("disk: {0}")]
    Io(Arc<io::Error>),
    /// Checked before starting: the file and a safety margin don't fit on the drive.
    #[error("not enough disk space: {} needed, {} free", disk::human(*needed), disk::human(*free))]
    NoSpace { needed: u64, free: u64 },
    /// The drive filled up while downloading (another program took the space).
    #[error("the disk is full")]
    DiskFull,
    #[error("{} is too big for this {file_system} drive (files up to 4 GB)", disk::human(*size))]
    TooBigForDrive { size: u64, file_system: String },
    #[error("cancelled")]
    Cancelled,
    /// The chosen network adapter is down, or can't reach this server (#35).
    #[error(transparent)]
    Adapter(#[from] crate::net::NetError),
    /// Another download in this process is writing to the same file.
    #[error("already being downloaded: {0}")]
    InUse(PathBuf),
    /// The save folder can't be reached or created (a removed drive, an offline share).
    #[error("the folder {} is not available (drive removed or network share offline?)", .0.display())]
    DestinationUnavailable(PathBuf),
    /// The drive or network share went away while downloading.
    #[error("the drive or network share is no longer available")]
    DriveGone,
    /// Longer than Windows programs can open (MAX_PATH).
    #[error("the path is too long for Windows: {}", .0.display())]
    PathTooLong(PathBuf),
    /// A web page came instead of the file (a file host's countdown or captcha, a sign-in wall,
    /// an expired link's notice, #533, #701): the browser gets past it, MDM doesn't.
    #[error(
        "the site sent a web page instead of the file (a waiting page, a captcha or a sign-in): open the link in your browser, finish there, and the download comes back to MDM"
    )]
    GotWebPage,
    /// The server's answer is no larger than a web page (`sent` bytes) where the browser's own
    /// download of the link was a file of `size` bytes, larger than any page (#1460): an error,
    /// a notice or an empty answer in the file's place (a link that works once, or only in the
    /// browser), never saved as the file.
    #[error(
        "the server sent only {} where the browser's file is {} (a link that works once, or only in the browser): Alt+click the link to let the browser download it",
        disk::human(*sent),
        disk::human(*size)
    )]
    NotTheFile { size: u64, sent: u64 },
    /// A full answer to a range request for the same, unchanged file: the server (or one CDN
    /// edge) didn't serve the range this time. Retried; the file did not change.
    #[error("the server did not send the requested part")]
    RangesUnavailable,
    /// The redirected address stopped working and the original address no longer leads to this
    /// file: the user has to give a fresh address (Refresh download address).
    #[error("the download link has expired (refresh the download address)")]
    LinkExpired,
    /// This connection was far slower than the others: it reconnects for the rest of its
    /// segment (not a failure).
    #[error("much slower than the other connections; reconnecting")]
    SlowConnection,
    /// The finished file doesn't have the expected hash; it stays as the part file.
    #[error("the file's {0} checksum doesn't match (the data is kept; redownload it)")]
    ChecksumMismatch(&'static str),
    /// An FTP server's refusal: its reply code and text.
    #[error("the FTP server answered {code} {text}")]
    FtpReply { code: u32, text: String },
    /// A network failure outside HTTP (FTP's sessions): `cause` as the system words it.
    #[error("{}", describe_net(*.kind, .cause))]
    Net { kind: NetKind, cause: String },
    /// A site's own refusal (Google Drive's quota page, its sign-in wall, a folder link, #34):
    /// retrying now won't help, the user has to do something else.
    #[error("{0}")]
    Refused(Refusal),
    /// A secure connection (HTTPS, FTPS) on a system without trusted certificates to check
    /// the server with (a minimal Linux without `ca-certificates`, #264).
    #[error("{}", crate::tls::NO_ROOTS)]
    NoCertificates,
    /// The server's certificate was refused (expired, not trusted, for another name): what a
    /// browser warns about. Retrying won't help, and there is no way around it (#762).
    #[error("{0}")]
    Certificate(crate::tls::CertProblem),
}

/// What failed on the network (`Error::Net`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Connect,
    /// The server went quiet.
    Timeout,
    /// The connection broke while receiving.
    Lost,
    /// The secure connection failed (an untrusted certificate, a handshake the server refused).
    Tls,
}

impl Error {
    /// The download stopped because its address no longer gives the file: it expired, or the
    /// server answers it 401, 403, 404 or 410 (a signed link's end), or with a web page (a file
    /// host's, #701). A fresh address for the same file (Refresh download address, the browser's
    /// next download of it) is what helps; the interface waits for one.
    pub fn wants_new_address(&self) -> bool {
        matches!(self, Error::Status(s) if matches!(s.as_u16(), 401 | 403 | 404 | 410))
            || matches!(
                self,
                Error::LinkExpired | Error::GotWebPage | Error::NotTheFile { .. }
            )
    }

    /// The address no longer serves this file (or refuses us): retrying won't help.
    fn link_dead(&self) -> bool {
        matches!(self, Error::Status(s) if matches!(s.as_u16(), 401 | 403 | 404 | 410 | 416))
            || matches!(
                self,
                Error::GotWebPage | Error::NotTheFile { .. } | Error::LinkExpired | Error::BotCheck
            )
            // A site's refusal; a service's, unless waiting helps (#44).
            || matches!(self, Error::Refused(r) if !matches!(r, Refusal::Service { fault, .. } if fault.retry_wait().is_some()))
            // Login refused; no such file (or no permission); a name the server won't take.
            || matches!(self, Error::FtpReply { code: 530 | 532 | 550 | 553, .. })
    }

    /// Errors that stop the whole download instead of being retried by a connection.
    fn is_fatal(&self) -> bool {
        matches!(
            self,
            Error::FileChanged
                | Error::NoRanges
                | Error::BadState(_)
                | Error::Io(_)
                | Error::NoSpace { .. }
                | Error::DiskFull
                | Error::TooBigForDrive { .. }
                | Error::DestinationUnavailable(_)
                | Error::DriveGone
                | Error::PathTooLong(_)
                | Error::Adapter(_)
                | Error::ChecksumMismatch(_)
                | Error::Cancelled
                | Error::InUse(_)
                | Error::NoCertificates
                | Error::Certificate(_)
        )
    }

    /// A failure of the first request that a later try may not have: a busy server (429, 503),
    /// a server error (500, 502, 504), a network that failed to connect or went quiet.
    fn transient(&self) -> bool {
        match self {
            Error::Throttled(..) => true,
            Error::Status(s) => matches!(s.as_u16(), 500 | 502 | 504),
            Error::Http(h) => h.is_connect() || h.is_timeout() || h.is_request() || h.is_body(),
            Error::ShortBody => true,
            // Too many sessions; no data connection; transfer aborted; file busy; local error.
            Error::FtpReply { code, .. } => matches!(code, 421 | 425 | 426 | 450 | 451),
            Error::Net { kind, .. } => *kind != NetKind::Tls,
            Error::Refused(Refusal::Service { fault, .. }) => fault.retry_wait().is_some(),
            _ => false,
        }
    }

    /// A server turning this connection away while others work: its connection limit. FTP
    /// servers say it at login, with 421 or (vsftpd, ProFTPD) 530.
    fn refused(&self) -> bool {
        match self {
            Error::Throttled(..) => true,
            Error::Status(s) => s.as_u16() == 403,
            Error::Http(h) => h.is_connect(),
            Error::FtpReply { code, .. } => matches!(code, 421 | 530),
            Error::Net { kind, .. } => *kind == NetKind::Connect,
            _ => false,
        }
    }
}

/// How long to wait before try number `failures + 1` after `e`: the first retry at once (IDM
/// reconnects within ~12 ms, lab S5), then `retry_delay` doubling up to 30 s; a busy server's
/// Retry-After wins (capped at 5 minutes). One policy for the first request and every connection.
fn retry_wait(opts: &Options, failures: u32, e: &Error) -> Option<Duration> {
    match e {
        Error::Throttled(_, Some(after)) => Some((*after).min(Duration::from_secs(300))),
        _ if failures > 1 => {
            Some((opts.retry_delay * 2u32.pow((failures - 2).min(10))).min(Duration::from_secs(30)))
        }
        _ => None,
    }
}

/// A network error in words a user can act on, with its deepest cause ("connection refused",
/// "dns error: …") but not the address, which the dialog already shows.
fn describe_http(e: &reqwest::Error) -> String {
    let what = if e.is_timeout() {
        "the server did not answer in time"
    } else if e.is_connect() {
        "cannot connect to the server"
    } else if e.is_body() || e.is_decode() {
        "the connection was lost while receiving data"
    } else if e.is_redirect() {
        "too many redirects"
    } else {
        "network error"
    };
    let mut cause = None;
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        cause = Some(s.to_string());
        source = s.source();
    }
    match cause.map(|c| strip_os_error_code(&c)) {
        Some(c) if !c.is_empty() && !c.contains("://") => format!("{what} ({c})"),
        _ => what.to_owned(),
    }
}

/// A network failure outside HTTP in words a user can act on, with its cause.
fn describe_net(kind: NetKind, cause: &str) -> String {
    let what = match kind {
        NetKind::Connect => "cannot connect to the server",
        NetKind::Timeout => "the server did not answer in time",
        NetKind::Lost => "the connection was lost while receiving data",
        NetKind::Tls => "the secure connection failed",
    };
    if cause.is_empty() {
        what.to_owned()
    } else {
        format!("{what} ({cause})")
    }
}

/// "… refused it. (os error 10061)" → "… refused it".
pub(crate) fn strip_os_error_code(s: &str) -> String {
    let s = match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => &s[..i],
        _ => s,
    };
    s.trim().trim_end_matches('.').to_owned()
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        // NOT_READY, BAD_NETPATH, DEV_NOT_EXIST, NETNAME_DELETED, BAD_NET_NAME,
        // DEVICE_NOT_CONNECTED: the drive or share is gone, not a disk fault.
        #[cfg(windows)]
        if matches!(e.raw_os_error(), Some(21 | 53 | 55 | 64 | 67 | 1167)) {
            return Error::DriveGone;
        }
        // ENXIO, ENODEV (both systems); ENOMEDIUM (Linux): the device is gone.
        #[cfg(unix)]
        if matches!(e.raw_os_error(), Some(6 | 19))
            || (cfg!(target_os = "linux") && e.raw_os_error() == Some(123))
        {
            return Error::DriveGone;
        }
        match e.kind() {
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Error::DiskFull,
            _ => Error::Io(Arc::new(e)),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if crate::tls::is_no_roots(&e) {
            return Error::NoCertificates;
        }
        if let Some(problem) = crate::tls::certificate_problem(&e) {
            return Error::Certificate(problem);
        }
        Error::Http(Arc::new(e))
    }
}

impl From<disk::Refusal> for Error {
    fn from(r: disk::Refusal) -> Self {
        match r {
            disk::Refusal::NoSpace { needed, free } => Error::NoSpace { needed, free },
            disk::Refusal::TooBigForFileSystem { size, file_system } => {
                Error::TooBigForDrive { size, file_system }
            }
        }
    }
}

/// The request headers to send to `to` when they were given for `from`: credentials
/// (cookies, authorization) never follow a redirect to another host or from https down to
/// http — as reqwest does while it follows the redirect itself — except between one site's own
/// hosts (`sites::same_party`: a Drive link's cookies go to Drive's file host).
pub(crate) fn scoped_headers(headers: &HeaderMap, from: &Url, to: &Url) -> HeaderMap {
    let mut h = headers.clone();
    let same_host = (from.host_str() == to.host_str()
        && from.port_or_known_default() == to.port_or_known_default())
        || sites::same_party(from, to);
    let downgrade = from.scheme() == "https" && to.scheme() != "https";
    if !same_host || downgrade {
        for name in [
            header::COOKIE,
            header::AUTHORIZATION,
            header::PROXY_AUTHORIZATION,
        ] {
            h.remove(name);
        }
    }
    h
}

/// The addresses a POST download's redirects turned into GETs (301, 302, 303, as browsers
/// do), recorded by its client's redirect policy: requests to them afterwards are GETs too.
#[derive(Clone, Default)]
struct AsGet(Arc<Mutex<HashSet<Url>>>);

impl AsGet {
    fn contains(&self, url: &Url) -> bool {
        self.0.lock().unwrap().contains(url)
    }

    /// Following up to `MAX_REDIRECTS`, noting the method each hop leaves with.
    fn policy(&self) -> reqwest::redirect::Policy {
        let seen = self.0.clone();
        reqwest::redirect::Policy::custom(move |attempt| {
            let mut seen = seen.lock().unwrap();
            // `previous` ends with the address that redirected; the first is the one asked.
            let came_as_get = attempt.previous().len() > 1
                && attempt.previous().last().is_some_and(|u| seen.contains(u));
            if came_as_get || matches!(attempt.status().as_u16(), 301..=303) {
                seen.insert(attempt.url().clone());
            } else {
                seen.remove(attempt.url());
            }
            drop(seen);
            reqwest::redirect::Policy::limited(MAX_REDIRECTS).redirect(attempt)
        })
    }
}

/// Most HTTP redirects (301, 302, 303, 307, 308) a request follows, as browsers do (20; reqwest
/// stops at 10): download links often go through a tracker, a mirror picker and a CDN.
const MAX_REDIRECTS: usize = 20;

/// A GET's redirects, followed up to `MAX_REDIRECTS`, except one between two of a site's own
/// hosts (`sites::same_party`: Drive's `open?id=` to Docs): reqwest would drop the user's cookies
/// there, so it is left to `get`, which follows it with them.
fn get_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let party_hop = attempt.previous().last().is_some_and(|from| {
            from.host_str() != attempt.url().host_str() && sites::same_party(from, attempt.url())
        });
        if party_hop {
            attempt.stop()
        } else {
            reqwest::redirect::Policy::limited(MAX_REDIRECTS).redirect(attempt)
        }
    })
}

/// The body a POST download sends to an address reached with or without a redirect to a GET
/// (`as_get`); for a GET, the body's headers go too, as browsers drop them. None: a GET.
fn post_for(opts: &Options, as_get: bool, headers: &mut HeaderMap) -> Option<Bytes> {
    let body = opts.post.clone()?;
    if !as_get {
        return Some(body);
    }
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_ENCODING,
    ] {
        headers.remove(name);
    }
    None
}

/// A request for `url` with `headers`: a POST with `post` as its body, else a GET. The client
/// sends the download's User-Agent (`Options::user_agent`); a site that wants a browser's gets
/// it instead when `site_agent` (`Options::site_agent`: the agent is MDM's automatic one).
fn build(
    client: &Client,
    url: Url,
    post: Option<&Bytes>,
    headers: &HeaderMap,
    site_agent: bool,
) -> RequestBuilder {
    let mut headers = headers.clone();
    if let Some(site) = request_agent(site_agent, &url)
        && !headers.contains_key(header::USER_AGENT)
    {
        headers.insert(header::USER_AGENT, HeaderValue::from_static(site));
    }
    let req = match post {
        Some(body) => client.post(url).body(body.clone()),
        None => client.get(url),
    };
    req.headers(headers)
}

/// The User-Agent a request to `url` carries instead of the download's own: a site's browser
/// agent (`sites::user_agent`: Google Drive's hosts), only when `site_agent` (the download's is
/// MDM's automatic one, `Options::site_agent`). A browser's agent handed over with the download,
/// or one the user set, always wins.
fn request_agent(site_agent: bool, url: &Url) -> Option<&'static str> {
    site_agent.then(|| sites::user_agent(url)).flatten()
}

/// A header's text: UTF-8, else ISO-8859-1 (servers send file names either way, while
/// `HeaderValue::to_str` takes ASCII only).
fn header_text(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    let raw = h.get(name)?.as_bytes();
    Some(match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    })
}

/// What the server's range answers have shown so far.
const RANGES_UNKNOWN: u8 = 0;
const RANGES_YES: u8 = 1;
const RANGES_NO: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Probing,
    Downloading,
    /// Checking the finished file against its checksum.
    Verifying,
    /// A video's streams joined into its file (ffmpeg, run by yt-dlp; #736): the app's view of a
    /// video download, never an engine download's.
    Merging,
    Completing,
    /// Stopped (paused) by [`Download::cancel`] while downloading: its connections are closed,
    /// and what they received and the resume state are still being written (#1328). A disk
    /// slower than the network takes as long as it takes; the download is stopped for whoever
    /// shows it, and a new run at its place waits for it (`naming::Settling`). `Cancelled`
    /// follows once everything is saved.
    Stopping,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub enum ConnStatus {
    Connecting,
    Receiving,
    /// Waiting before a retry, with the last error.
    Retrying(Error),
    Finished,
    Failed(Error),
    /// Closed for good: `host` allows `cap` connections and the others fill them (learned).
    Retired {
        host: String,
        cap: u32,
    },
}

#[derive(Clone, Debug)]
pub struct ConnView {
    pub id: WorkerId,
    pub downloaded: u64,
    pub status: ConnStatus,
    /// Bytes/s over the current response (0 while not receiving).
    pub speed: u64,
}

impl ConnView {
    /// Done: nothing left for it, or every byte of the download is in (#736). One still
    /// connecting, receiving or about to retry is finished; a failed or retired one keeps saying
    /// why. None receives any more.
    pub fn finish(&mut self) {
        if matches!(
            self.status,
            ConnStatus::Connecting | ConnStatus::Receiving | ConnStatus::Retrying(_)
        ) {
            self.status = ConnStatus::Finished;
        }
        self.speed = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentView {
    pub start: u64,
    pub cursor: u64,
    pub end: u64,
}

/// A point-in-time view for progress displays.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub url: Url,
    pub file: Option<PathBuf>,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub resumable: Option<bool>,
    pub segments: Vec<SegmentView>,
    pub connections: Vec<ConnView>,
    /// Why the download failed (state `Failed`), in English (the command line, logs).
    pub error: Option<String>,
    /// The same failure as data, for an interface that words it in its own language.
    pub failure: Option<Error>,
    /// What happened, oldest first (the last `EVENTS`): the download's event log. Never
    /// cookies or credentials.
    pub events: Vec<Event>,
    /// While a service account prepares the file's link (#44): the service, and how far it is
    /// (0 to 1) when it says. The list shows it instead of "Connecting...".
    pub at_service: Option<(String, Option<f32>)>,
    /// Where its server is, as its connections saw it (#140).
    pub server: Option<crate::Server>,
    /// How far `Verifying` (the bytes checked) or `Merging` has come, 0 to 1, when that is
    /// known (#736).
    pub finishing: Option<f64>,
}

/// One entry of a download's event log; `seq` counts up within one `Download`.
#[derive(Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub at: std::time::SystemTime,
    pub text: String,
}

/// Events kept per download.
pub const EVENTS: usize = 50;

/// What a quick look at a URL tells before downloading (for "Download File Info").
#[derive(Clone, Debug)]
pub struct ProbeInfo {
    pub final_url: Url,
    pub size: Option<u64>,
    /// Suggested local name (Content-Disposition, else URL).
    pub file_name: String,
    /// Where the server is, as the look saw it (#140).
    pub server: Option<crate::Server>,
    /// The answer was a web page (HTML the server didn't name as a file of its own): Link
    /// grabber reads an address given on its own for its links ([`page_links`], #705).
    pub web_page: bool,
}

/// One plain `GET` (or `Options::post`'s POST) to learn the final URL, size and name, through
/// the redirects `get` follows (a waiting page's look reads up to `PAGE_PEEK` of it and keeps its
/// wait); the file's own body is not read.
pub async fn probe(url: Url, headers: HeaderMap, opts: &Options) -> Result<ProbeInfo, Error> {
    probe_kept(url, headers, opts).await.map(|(info, _)| info)
}

/// The open answer of a look (`probe_kept`), for the download that follows it to read (#28): a
/// one-time link, or a host that allows one download at a time, refuses the download asking
/// again. Dropping it closes the connection.
pub struct KeptAnswer {
    /// The address asked: only a download of the same address takes it.
    url: Url,
    client: Client,
    as_get: AsGet,
    head: Head,
    body: Body,
    /// The browser-like connection it came through, when only that one got past the site's
    /// bot check (#668): the download goes on with it.
    browser: Option<Arc<crate::browserlike::Browser>>,
    /// The User-Agent it got through with, when only MDM's own name got past the site's bot
    /// check (#1422): the download goes on under it.
    own_agent: Option<String>,
    headers: HeaderMap,
    took: Duration,
    at: std::time::Instant,
    /// The service's direct link it was asked at (#44), with its connection cap and name.
    unlocked: Option<crate::unlock::Unlocked>,
    /// What its client learnt of the server (#140): the download goes on with it.
    seen: Arc<crate::server::ServerSeen>,
}

impl std::fmt::Debug for KeptAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptAnswer")
            .field("url", &self.url.as_str())
            .field("status", &self.head.status)
            .field("browser_like", &self.browser.is_some())
            .finish_non_exhaustive()
    }
}

/// How long a kept answer is worth taking: a server ends an idle connection after a while,
/// and the download then asks again (as when it isn't kept).
const KEEP_ANSWER_FOR: Duration = Duration::from_secs(15 * 60);

/// `probe`, keeping its answer open when its body is the file from byte 0 (200, or 206 from
/// 0), for `Download::start_with`. Not for FTP, whose look is a separate command.
pub async fn probe_kept(
    url: Url,
    headers: HeaderMap,
    opts: &Options,
) -> Result<(ProbeInfo, Option<KeptAnswer>), Error> {
    if crate::ftp::is_ftp(&url) {
        let site = crate::ftp::Site::new(&url, &headers, opts)?;
        let facts = site.check().await?;
        return Ok((
            ProbeInfo {
                file_name: naming::file_name(None, &url),
                final_url: url,
                size: facts.size,
                server: None,
                web_page: false,
            },
            None,
        ));
    }
    let as_get = AsGet::default();
    // Through a service account (#44): its direct link is looked at instead, without the host's
    // cookies or logins.
    let unlocked = unlock_origin(opts, &url, None, &CancellationToken::new()).await?;
    let (target, headers) = match &unlocked {
        Some(u) => (u.url.clone(), HeaderMap::new()),
        None => (url.clone(), headers),
    };
    let seen = Arc::new(crate::server::ServerSeen::default());
    // #1422: MDM's own name, when the site's bot check let only it through earlier this
    // session: asked so at once; otherwise the last try, after the browser-like connection.
    let own = own_agent(opts, &headers).filter(|_| unlocked.is_none());
    let remembered = own.clone().filter(|_| crate::agent::own_for(&target));
    let as_own = remembered.clone().map(|own| speaking_own(opts, own));
    let first_opts = as_own.as_ref().unwrap_or(opts);
    let client = client(first_opts, &target, &as_get, &seen)?;
    let expects = expects_other_file(
        opts.expected_type.as_deref(),
        opts.expected_size,
        None,
        &url,
    );
    // Where the look asked, and with what: asked again there if the site refuses (#668).
    let (asked_at, asked_with) = (target.clone(), headers.clone());
    // A browser's User-Agent refused (#372): asked once more as MDM.
    let (client, asked) = match get(
        &client,
        target.clone(),
        headers.clone(),
        opts.post.as_ref(),
        first_opts.site_agent(),
        expects,
        &CancellationToken::new(),
    )
    .await
    {
        Err(e) if agent_refused(&e) && first_opts.agent_fallback.is_some() => {
            let mut plain = opts.clone();
            plain.user_agent = opts.agent_fallback.clone().unwrap_or_default();
            let client = self::client(&plain, &target, &as_get, &seen)?;
            let asked = get(
                &client,
                target,
                headers,
                opts.post.as_ref(),
                plain.site_agent(),
                expects,
                &CancellationToken::new(),
            )
            .await;
            (client, asked)
        }
        asked => (client, asked),
    };
    let (mut client, mut own_agent) = (client, remembered);
    let (head, body, browser, headers, took) = match asked {
        Ok((resp, headers, took)) => (Head::of(&resp), Body::Http(resp), None, headers, took),
        // #668: the site refused MDM's connection as a program's (`refused_as_a_program`):
        // looked at once more the way a browser connects, its answer kept for the download
        // (which then asks no more).
        Err(e) if refused_as_a_program(&e) && unlocked.is_none() && opts.post.is_none() => {
            if own_agent.take().is_some() {
                // Remembered, but no longer let through: learnt again next time.
                crate::agent::forget_own(&asked_at);
            }
            let cancel = CancellationToken::new();
            match browser_ask(opts, &asked_at, &asked_with, expects, &cancel).await {
                Ok((b, (head, body, headers, took))) => (head, body, Some(b), headers, took),
                Err(other @ (Error::Cancelled | Error::Adapter(_))) => return Err(other),
                // #1422: refused that way too; once more under MDM's own name, the last try.
                Err(_) => match own.filter(|_| as_own.is_none()) {
                    Some(own) => {
                        let o = speaking_own(opts, own.clone());
                        client = self::client(&o, &asked_at, &as_get, &seen)?;
                        let (resp, headers, took) = get(
                            &client,
                            asked_at.clone(),
                            asked_with,
                            None,
                            o.site_agent(),
                            expects,
                            &cancel,
                        )
                        .await
                        .map_err(|other| match other {
                            Error::Cancelled | Error::Adapter(_) => other,
                            _ => e,
                        })?;
                        crate::agent::remember_own(&asked_at);
                        own_agent = Some(own);
                        (Head::of(&resp), Body::Http(resp), None, headers, took)
                    }
                    None => return Err(e),
                },
            }
        }
        Err(e) => return Err(e),
    };
    // Whichever connection it came through, the browser-like one too (#954).
    seen.answered(&head);
    let final_url = head.url.clone();
    let (size, from_zero) = match head.status {
        StatusCode::PARTIAL_CONTENT => {
            let (start, _, total) = parse_content_range(&head.headers)?;
            (total, start == 0)
        }
        StatusCode::OK => (head.len, true),
        _ => (head.len, false),
    };
    // No larger than a page where the browser's file is larger than any (#1460): said here, before
    // anything is saved.
    if let Some(sent) = size {
        not_the_file(opts.expected_size, sent)?;
    }
    // A page where a file that never is one was expected, whatever type it came as (#701).
    let body = if from_zero
        && never_a_page(
            opts.expected_type.as_deref(),
            &name_for(&head, None, unlocked.as_ref(), opts),
        ) {
        not_a_page(body, &CancellationToken::new()).await?
    } else {
        body
    };
    // A name its address and the server leave untyped: the bytes' type, when they are the
    // file's first (#800).
    let (body, sniffed) = if from_zero && names_by_bytes(&head, unlocked.as_ref()) {
        sniff_body(body, &CancellationToken::new()).await?
    } else {
        (body, None)
    };
    let file_name = name_for(&head, sniffed, unlocked.as_ref(), opts);
    let info = ProbeInfo {
        final_url,
        size: size.or(unlocked.as_ref().and_then(|u| u.size)),
        file_name,
        server: seen.get(),
        web_page: is_web_page(&head.headers) && !named(&head.headers),
    };
    let kept = from_zero.then(|| KeptAnswer {
        url,
        client,
        seen,
        as_get,
        head,
        body,
        browser,
        own_agent,
        headers,
        took,
        at: std::time::Instant::now(),
        unlocked,
    });
    Ok((info, kept))
}

/// A client for requests to `url`'s server that aren't downloads (a debrid service's API, #44):
/// the same proxy, DNS and network adapter as `opts`' downloads.
pub fn http_client(opts: &Options, url: &Url) -> Result<Client, Error> {
    client(opts, url, &AsGet::default(), &Default::default())
}

/// A client for one request to `url`'s server, as [`http_client`]'s, that follows no redirect
/// and adds no `Referer` of its own: its caller follows each hop itself, with a client for that
/// hop's server (its proxy and adapter checks are that server's), and decides which of its
/// headers go there (the browser's quality menu reading a stream's manifest, mdm-tools'
/// `manifest`: the browser's cookie never reaches another server).
pub fn hop_client(opts: &Options, url: &Url) -> Result<Client, Error> {
    client_with(opts, url, &AsGet::default(), &Default::default(), false)
}

/// The file's name: the server's (Content-Disposition), else the service's (#44), else the
/// address's, typed by the server's `Content-Type` or the bytes' own type (`sniffed`), and named
/// by the page's title when the address names nothing (#695, #800: [`naming::answer_name`]).
fn name_for(
    head: &Head,
    sniffed: Option<&str>,
    unlocked: Option<&crate::unlock::Unlocked>,
    opts: &Options,
) -> String {
    let cd = header_text(&head.headers, header::CONTENT_DISPOSITION);
    match (&cd, unlocked.and_then(|u| u.file_name.as_deref())) {
        (None, Some(name)) if !name.trim().is_empty() => naming::sanitize(name),
        _ => naming::answer_name(
            &head.url,
            &naming::Answer {
                disposition: cd.as_deref(),
                content_type: header_str(&head.headers, header::CONTENT_TYPE),
                sniffed,
                title: opts.page_title.as_deref(),
            },
        ),
    }
}

/// Whether the answer's first bytes are needed for its name (`name_for`): it is named by its
/// address, whose name doesn't say the file's type, and neither does the server (#800).
fn names_by_bytes(head: &Head, unlocked: Option<&crate::unlock::Unlocked>) -> bool {
    let cd = header_text(&head.headers, header::CONTENT_DISPOSITION);
    let service = unlocked
        .and_then(|u| u.file_name.as_deref())
        .is_some_and(|n| !n.trim().is_empty());
    !(cd.is_none() && service)
        && naming::needs_sniff(
            &head.url,
            cd.as_deref(),
            header_str(&head.headers, header::CONTENT_TYPE),
        )
}

/// The type `body`'s first bytes show ([`naming::sniff`]), and the body that still gives them:
/// read up to [`naming::SNIFF_BYTES`] (less at its end).
async fn sniff_body(
    body: Body,
    cancel: &CancellationToken,
) -> Result<(Body, Option<&'static str>), Error> {
    let (body, seen) = peek(body, naming::SNIFF_BYTES, cancel).await?;
    Ok((body, naming::sniff(&seen)))
}

/// `opts.unlocker`'s direct link for `url` (#44), noted in the event log (never the link
/// itself): None without an unlocker, for a form's POST, or when the service declines the host.
async fn unlock_origin(
    opts: &Options,
    url: &Url,
    shared: Option<&Shared>,
    cancel: &CancellationToken,
) -> Result<Option<crate::unlock::Unlocked>, Error> {
    let Some(u) = &opts.unlocker else {
        return Ok(None);
    };
    if opts.post.is_some() || !matches!(url.scheme(), "http" | "https") {
        return Ok(None);
    }
    unlock_noted(u, url, shared, cancel).await.map_err(|fault| {
        Error::Refused(Refusal::Service {
            service: u.name().to_owned(),
            fault,
        })
    })
}

/// `u`'s link for `url`, its waits told in the event log and the list's status ("40% at
/// TorBox") meanwhile, and the link it gave noted (never the link itself).
async fn unlock_noted(
    u: &Arc<dyn crate::unlock::Unlocker>,
    url: &Url,
    shared: Option<&Shared>,
    cancel: &CancellationToken,
) -> Result<Option<crate::unlock::Unlocked>, crate::unlock::ServiceFault> {
    let note = |s: String, progress: Option<f32>| {
        if let Some(sh) = shared {
            sh.event(s);
            sh.view.lock().unwrap().at_service = Some((u.name().to_owned(), progress));
        }
    };
    let out = u.unlock(url, &note, cancel).await;
    if let Some(sh) = shared {
        sh.view.lock().unwrap().at_service = None;
        if let Ok(Some(x)) = &out {
            sh.event(u.noted(x));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    pub size: u64,
}

/// A running download.
pub struct Download {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<Outcome, Error>>,
}

impl Download {
    /// Start downloading in the background. Must be called inside a Tokio runtime.
    pub fn start(req: Request, opts: Options) -> Download {
        Download::start_with(req, opts, None)
    }

    /// `start`, connection 1 reading `kept` (a look's open answer, `probe_kept`) instead of
    /// asking again, when it is the same address's and recent (`KEEP_ANSWER_FOR`).
    pub fn start_with(req: Request, opts: Options, kept: Option<KeptAnswer>) -> Download {
        Download::spawn(req.url.clone(), move |shared, cancel| {
            let sh = shared.clone();
            let body = async move {
                after_clearing(req.dest.folder(), &cancel).await?;
                after_settling(&req.dest, &cancel).await?;
                if crate::ftp::is_ftp(&req.url) {
                    run_fresh_ftp(req, opts, sh, cancel).await
                } else {
                    run_fresh(req, opts, sh, cancel, kept).await
                }
            };
            run(body, shared)
        })
    }

    /// Continue a paused, cancelled or crashed download from its state file
    /// (`state::state_path(dest)`). `headers` are sent on every request (cookies etc. are not
    /// stored in the state file).
    pub fn resume(
        state_file: PathBuf,
        headers: HeaderMap,
        opts: Options,
    ) -> Result<Download, Error> {
        // A removed drive or offline share: say so (the data is still there for later).
        if let Some(dir) = state_file.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.is_dir()
        {
            return Err(Error::DestinationUnavailable(dir.to_owned()));
        }
        let saved = ResumeState::load(&state_file).map_err(Error::BadState)?;
        Ok(Download::spawn(saved.url.clone(), move |shared, cancel| {
            run(
                run_resume(saved, state_file, headers, opts, shared.clone(), cancel),
                shared,
            )
        }))
    }

    /// [`Download::resume`] when there is a resume state at `state_file`, else `req` from the
    /// start ([`Download::start_with`], `kept` its look's answer), as the download's own task
    /// finds on the disk: the caller never waits for it. A folder on a network share that doesn't
    /// answer, or a drive waking up, holds this download only, not the caller (the app's window:
    /// found with #1170). A state that is there but can't be resumed fails it, as
    /// [`Download::resume`] would, and the data stays for later: it is never started over.
    /// Until the disk has answered, it shows `shown` bytes (what the caller shows of it); a start
    /// from scratch then shows 0, unless its folder isn't there (a drive unplugged: its data may
    /// still be on it).
    pub fn resume_or_start(
        state_file: PathBuf,
        req: Request,
        opts: Options,
        kept: Option<KeptAnswer>,
        shown: u64,
    ) -> Download {
        Download::spawn_showing(req.url.clone(), shown, move |shared, cancel| {
            let sh = shared.clone();
            let body = async move {
                after_clearing(req.dest.folder(), &cancel).await?;
                after_settling(&req.dest, &cancel).await?;
                let look = {
                    let state_file = state_file.clone();
                    tokio::task::spawn_blocking(move || Found::look(&state_file))
                };
                let found = tokio::select! {
                    _ = cancel.cancelled() => return Err(Error::Cancelled),
                    found = look => found.unwrap_or_else(|e| {
                        Found::State(Err(format!("{}: {e}", state_file.display())))
                    }),
                };
                match found {
                    Found::State(Ok(saved)) => {
                        run_resume(*saved, state_file, req.headers, opts, sh, cancel).await
                    }
                    Found::State(Err(e)) => Err(Error::BadState(e)),
                    Found::Unavailable(dir) => Err(Error::DestinationUnavailable(dir)),
                    Found::Nothing { folder } => {
                        if folder {
                            sh.view.lock().unwrap().downloaded = 0;
                        }
                        if crate::ftp::is_ftp(&req.url) {
                            run_fresh_ftp(req, opts, sh, cancel).await
                        } else {
                            run_fresh(req, opts, sh, cancel, kept).await
                        }
                    }
                }
            };
            run(body, shared)
        })
    }

    fn spawn<F, Fut>(url: Url, f: F) -> Download
    where
        F: FnOnce(Arc<Shared>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<Outcome, Error>> + Send + 'static,
    {
        Download::spawn_showing(url, 0, f)
    }

    /// [`Download::spawn`], its snapshot showing `downloaded` bytes until the run says.
    fn spawn_showing<F, Fut>(url: Url, downloaded: u64, f: F) -> Download
    where
        F: FnOnce(Arc<Shared>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<Outcome, Error>> + Send + 'static,
    {
        let cancel = CancellationToken::new();
        let shared = Arc::new(Shared {
            view: Mutex::new(Snapshot {
                state: State::Probing,
                url,
                file: None,
                size: None,
                downloaded,
                resumable: None,
                segments: Vec::new(),
                connections: Vec::new(),
                error: None,
                failure: None,
                events: Vec::new(),
                server: None,
                at_service: None,
                finishing: None,
            }),
            seen: Default::default(),
            planner: Mutex::new(None),
            limiter: Limiter::new(None),
            best_speed: Default::default(),
            received: Default::default(),
            link: Default::default(),
            disk_held: Default::default(),
            held: Default::default(),
            checked: Default::default(),
            settle: Default::default(),
        });
        let task = tokio::spawn(f(shared.clone(), cancel.clone()));
        Download {
            shared,
            cancel,
            task,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Change this download's speed limit (bytes/s, None = unlimited) while it runs.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        self.shared.limiter.set(rate);
    }

    pub fn speed_limit(&self) -> Option<u64> {
        self.shared.limiter.get()
    }

    pub async fn wait(self) -> Result<Outcome, Error> {
        match self.task.await {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => Err(Error::Cancelled),
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }
}

/// What [`Download::resume_or_start`] finds on the disk.
enum Found {
    /// A resume state, or why the one there doesn't read.
    State(Result<Box<ResumeState>, String>),
    /// A state whose folder isn't there (a removed drive or an offline share).
    Unavailable(PathBuf),
    /// No state: started from scratch. `folder`: its folder is there.
    Nothing { folder: bool },
}

impl Found {
    /// The look [`Download::resume`] and its caller made, on the thread that may wait.
    fn look(state_file: &Path) -> Found {
        let dir = state_file.parent().filter(|d| !d.as_os_str().is_empty());
        if !state_file.exists() {
            return Found::Nothing {
                folder: dir.is_none_or(Path::is_dir),
            };
        }
        if let Some(dir) = dir
            && !dir.is_dir()
        {
            return Found::Unavailable(dir.to_owned());
        }
        Found::State(ResumeState::load(state_file).map(Box::new))
    }
}

/// Download `url` to `path` and wait for it: MDM's own downloads that aren't in the list (its
/// helpers, its updates, a crash report's symbols) use the engine like every download. An
/// earlier attempt cut off at `path` resumes; `progress(done, total)` while it runs.
pub async fn fetch(
    url: Url,
    path: &Path,
    opts: Options,
    progress: impl Fn(u64, Option<u64>),
) -> Result<Outcome, Error> {
    let state = crate::state::state_path(path);
    let fresh = || {
        Download::start(
            Request {
                url: url.clone(),
                headers: HeaderMap::new(),
                dest: Destination::File(path.to_owned()),
            },
            opts.clone(),
        )
    };
    // A saved state for another address (an earlier release's file) is not resumed.
    let dl = match crate::state::ResumeState::load(&state) {
        Ok(saved) if saved.url == url => {
            Download::resume(state, HeaderMap::new(), opts.clone()).unwrap_or_else(|_| fresh())
        }
        _ => fresh(),
    };
    loop {
        let s = dl.snapshot();
        progress(s.downloaded, s.size);
        if matches!(s.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    dl.wait().await
}

struct Shared {
    view: Mutex<Snapshot>,
    /// What the download's client learnt of its server (#140); the look's when it went on
    /// from the look's answer.
    seen: std::sync::OnceLock<Arc<crate::server::ServerSeen>>,
    planner: Mutex<Option<Planner>>,
    limiter: Limiter,
    /// The best speed one connection of this download sustained (`yardstick`), bytes/s: what a good
    /// connection to this server does, the yardstick for shedding (with `link`, `shed_bar`).
    best_speed: std::sync::atomic::AtomicU64,
    /// Bytes every connection of this download received (claimed), for `link`.
    received: std::sync::atomic::AtomicU64,
    /// What the connections bring together (`note_link`).
    link: Mutex<LinkMeter>,
    /// When a connection last stood still for the disk for `DISK_HELD` or longer (`Pace`).
    disk_held: Mutex<Option<std::time::Instant>>,
    /// Bytes received and claimed but not yet written, in the connections' buffers: shown as
    /// downloaded (they are), but never saved as such.
    held: std::sync::atomic::AtomicU64,
    /// While `Verifying`: the bytes of the finished file checked so far (#736).
    checked: Arc<std::sync::atomic::AtomicU64>,
    /// Stopped and still writing (`State::Stopping`): its place waits for it until `run` has
    /// let go of everything (#1328).
    settle: Mutex<Option<naming::Settling>>,
}

impl Shared {
    /// Stopped while downloading to `dest` (#1328): the connections are closing, and the download
    /// says so at once; what they received and the resume state are written after, and the
    /// place is held for that long (`naming::settling`).
    fn stopping(&self, dest: &Path) {
        let mut v = self.view.lock().unwrap();
        if v.state != State::Downloading {
            return;
        }
        v.state = State::Stopping;
        drop(v);
        *self.settle.lock().unwrap() = Some(naming::settling(dest));
        self.event("Stopped");
    }

    fn snapshot(&self) -> Snapshot {
        let mut s = self.view.lock().unwrap().clone();
        s.server = self.seen.get().and_then(|seen| seen.get());
        if let Some(p) = self.planner.lock().unwrap().as_ref() {
            s.downloaded = p.downloaded() + self.held.load(Ordering::Relaxed);
            s.segments = p
                .segments()
                .iter()
                .map(|g| SegmentView {
                    start: g.start,
                    cursor: g.cursor,
                    end: g.end,
                })
                .collect();
        }
        if s.state == State::Verifying
            && let Some(size) = s.size.filter(|n| *n > 0)
        {
            let checked = self.checked.load(Ordering::Relaxed);
            s.finishing = Some((checked as f64 / size as f64).min(1.0));
        }
        s
    }

    fn set_state(&self, state: State) {
        self.view.lock().unwrap().state = state;
    }

    /// Every byte is in (#736): while the file is checked and named, no connection shows one
    /// on its way (one stopped with the others still said "Receiving data...").
    fn every_byte_in(&self) {
        for c in &mut self.view.lock().unwrap().connections {
            c.finish();
        }
    }

    /// Add to the event log.
    fn event(&self, text: impl Into<String>) {
        let mut v = self.view.lock().unwrap();
        let seq = v.events.last().map_or(1, |e| e.seq + 1);
        if v.events.len() >= EVENTS {
            v.events.remove(0);
        }
        v.events.push(Event {
            seq,
            at: std::time::SystemTime::now(),
            text: text.into(),
        });
    }

    fn conn(&self, id: WorkerId, f: impl FnOnce(&mut ConnView)) {
        let mut v = self.view.lock().unwrap();
        let i = match v.connections.iter().position(|c| c.id == id) {
            Some(i) => i,
            None => {
                v.connections.push(ConnView {
                    id,
                    downloaded: 0,
                    status: ConnStatus::Connecting,
                    speed: 0,
                });
                v.connections.len() - 1
            }
        };
        f(&mut v.connections[i]);
    }

    fn with_planner<T>(&self, f: impl FnOnce(&mut Planner) -> T) -> T {
        f(self
            .planner
            .lock()
            .unwrap()
            .as_mut()
            .expect("planner exists while workers run"))
    }
}

/// The address ranges are fetched from, with the headers for it. `generation` counts switches.
#[derive(Clone)]
struct Target {
    url: Url,
    headers: HeaderMap,
    /// A POST download's body for this address (None: GET; see `post_for`).
    post: Option<Bytes>,
    generation: u64,
}

/// After asking the original address didn't help, how long until it may be asked again.
const REFRESH_EVERY: Duration = Duration::from_secs(120);

/// The refreshes of a download's target so far (`Ctx::refresh_target`).
#[derive(Default)]
struct Refreshes {
    /// When asking the origin last failed to help.
    failed_at: Option<std::time::Instant>,
    /// When it last gave a new target.
    switched_at: Option<std::time::Instant>,
}

/// What every connection needs to fetch a segment.
struct Ctx {
    /// The download's client: every connection's requests go through its pool.
    client: Client,
    /// The file's version; each range request's `If-Range` is chosen from them.
    validators: Mutex<Validators>,
    /// Where ranges come from now (after redirects); switched when it expires.
    target: Mutex<Target>,
    /// The address the user gave and its headers, when it redirected, or the page its site gives
    /// the target on (`origin_of`): asked again when the target expires.
    origin: Option<(Url, HeaderMap)>,
    /// When asking the origin last failed to help (one refresh at a time; after a failure,
    /// none for `REFRESH_EVERY`), and when it last gave a new target.
    refreshed: tokio::sync::Mutex<Refreshes>,
    /// The newest target generation a range request was answered on, plus one (0: none yet):
    /// a target a refresh gave that was refused before answering once isn't refreshed again
    /// at once (`refresh_target`).
    answered: std::sync::atomic::AtomicU64,
    size: u64,
    file: Arc<File>,
    opts: Options,
    shared: Arc<Shared>,
    /// Stops the connections: a stop asked for (`stop`), every byte in, or a fatal error.
    cancel: CancellationToken,
    /// The download asked to stop ([`Download::cancel`]): `cancel`'s parent.
    stop: CancellationToken,
    /// `RANGES_*`: learned from the first range answer.
    ranges: AtomicU8,
    /// The first answer advertised ranges (`Accept-Ranges: bytes`, or a 206): a full answer to
    /// a range request with the same ETag is then an edge that missed the range this time, not
    /// a server without ranges (#136).
    advertised: bool,
    /// Further connections wait for a range answer (`range_answered`): a POST whose server
    /// hasn't shown yet that it takes ranges (`Options::post`).
    wait_for_ranges: bool,
    /// Signalled by every 206 answer.
    range_answered: tokio::sync::Notify,
    /// Where a POST download's redirects turned into GETs (refreshing the target).
    as_get: AsGet,
    /// The file itself is a web page; otherwise an HTML answer is a sign-in wall or an error.
    web_page_ok: bool,
    /// The file is a kind whose bytes never start as a web page's ([`never_a_page`], #701): a
    /// whole answer that does is the site's page, whatever type it came as.
    never_page: bool,
    /// An FTP download's server and its idle sessions (else HTTP through `client`).
    ftp: Option<Arc<crate::ftp::Site>>,
    /// The browser-like connection every request goes through instead of `client`, when only it
    /// got past the site's bot check (#668).
    browser: Option<Arc<crate::browserlike::Browser>>,
}

/// An answer's body coming in: an HTTP response, or an FTP transfer.
pub(crate) enum Body {
    Http(Response),
    Ftp(crate::ftp::Transfer),
    /// An answer to the browser-like connection (#668, `crate::browserlike`).
    #[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
    Browser(crate::browserlike::Stream),
    /// A body whose first bytes were read already (to know the file's type, #800): they come
    /// first, then the rest.
    Prefixed(Option<bytes::Bytes>, Box<Body>),
}

impl Body {
    /// The next bytes; None at the end.
    async fn chunk(&mut self) -> Result<Option<bytes::Bytes>, Error> {
        match self {
            Body::Http(r) => Ok(r.chunk().await?),
            Body::Ftp(t) => t.chunk().await,
            Body::Browser(b) => b.chunk().await,
            Body::Prefixed(first, rest) => match first.take().filter(|b| !b.is_empty()) {
                Some(b) => Ok(Some(b)),
                None => Box::pin(rest.chunk()).await,
            },
        }
    }

    /// The bytes it will carry, if known.
    fn len(&self) -> Option<u64> {
        match self {
            Body::Http(r) => r.content_length(),
            Body::Ftp(t) => t.expected(),
            Body::Browser(b) => b.len(),
            Body::Prefixed(_, rest) => rest.len(),
        }
    }

    /// Done with it, at its end or before: its connection may serve the next request.
    async fn done(self, at_end: bool) {
        match self {
            // Read the end of the body so the connection goes back to the pool.
            Body::Http(mut r) => {
                if !at_end {
                    let _ = r.chunk().await;
                }
            }
            Body::Ftp(t) => t.done(at_end).await,
            // As an HTTP answer's: its connection goes back to be asked again (`browserlike`).
            Body::Browser(mut b) => {
                if !at_end {
                    let _ = b.chunk().await;
                }
            }
            Body::Prefixed(_, rest) => Box::pin(rest.done(at_end)).await,
        }
    }
}

/// What the engine reads of an HTTP answer's head, from either client (reqwest's, or the
/// browser-like one's, #668).
#[derive(Clone, Debug)]
pub(crate) struct Head {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    /// Where the answer came from, its redirects followed.
    pub(crate) url: Url,
    pub(crate) len: Option<u64>,
    /// The address that sent it: the server's, or the proxy's when one carried the request
    /// (`crate::server`, #954).
    pub(crate) from: Option<std::net::SocketAddr>,
}

impl Head {
    fn of(resp: &Response) -> Head {
        Head {
            status: resp.status(),
            headers: resp.headers().clone(),
            url: resp.url().clone(),
            len: resp.content_length(),
            from: resp.remote_addr(),
        }
    }
}

impl Ctx {
    fn target(&self) -> Target {
        self.target.lock().unwrap().clone()
    }

    /// After `generation`'s address answered as a dead link: ask the original address for
    /// today's target (once for all connections, at most every `REFRESH_EVERY`) and switch to
    /// it if it is the same file. True when there is a newer target to retry with.
    async fn refresh_target(&self, generation: u64) -> bool {
        let Some((origin, headers)) = &self.origin else {
            return false;
        };
        let mut refreshes = self.refreshed.lock().await;
        if self.target.lock().unwrap().generation != generation {
            return true; // another connection already switched
        }
        // Signed links may live minutes while the download takes hours: refresh as often as
        // that helps, but after an unhelpful refresh wait before asking again.
        if refreshes
            .failed_at
            .is_some_and(|t| t.elapsed() < REFRESH_EVERY)
        {
            return false;
        }
        // The target the last refresh gave, refused before it answered one range: the site
        // gives links it then refuses (an account's limit, a link bound elsewhere). Asking its
        // page again at once would only repeat that, connection after connection: an unhelpful
        // refresh, waited out as one.
        if generation > 0
            && self.answered.load(Ordering::SeqCst) <= generation
            && refreshes
                .switched_at
                .is_some_and(|t| t.elapsed() < REFRESH_EVERY)
        {
            self.shared.event(
                "The fresh address was refused before it gave any of the file; not asking again for now",
            );
            refreshes.failed_at = Some(std::time::Instant::now());
            return false;
        }
        let Some((url, mut headers)) = refresh_final_url(
            &self.client,
            origin,
            headers,
            self.opts.post.as_ref(),
            self.opts.site_agent(),
            self.size,
            &self.cancel,
            self.opts.unlocker.as_ref(),
            &self.shared,
        )
        .await
        .filter(|(u, _)| *u != self.target.lock().unwrap().url) else {
            refreshes.failed_at = Some(std::time::Instant::now());
            return false;
        };
        refreshes.failed_at = None;
        refreshes.switched_at = Some(std::time::Instant::now());
        self.shared.event(format!(
            "The address stopped working; the original address now leads to {}",
            url.host_str().unwrap_or("?")
        ));
        let post = post_for(&self.opts, self.as_get.contains(&url), &mut headers);
        let mut t = self.target.lock().unwrap();
        t.headers = headers;
        t.post = post;
        t.url = url;
        t.generation += 1;
        true
    }

    /// Whether ranges work, learned from the first answer that tells; true when this one did.
    fn learn_ranges(&self, yes: bool) -> bool {
        let v = if yes { RANGES_YES } else { RANGES_NO };
        let learned = self
            .ranges
            .compare_exchange(RANGES_UNKNOWN, v, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if learned {
            self.shared.view.lock().unwrap().resumable = Some(yes);
        }
        learned
    }
}

/// What a download asks again for today's target when `target` stops working (`Ctx::origin`),
/// with its headers: `given`, the address the user gave, when it led elsewhere; else the page a
/// site gives `target` on when its file addresses expire (`sites::origin`: MediaFire's, #952;
/// Rapidgator's, one of `pages`), credentials kept to their host. None for a browser-like
/// download: asked again through MDM's own connection, which the site's bot check refuses, it
/// keeps the address it has (#668).
fn origin_of(
    given: &Url,
    headers: &HeaderMap,
    target: &Url,
    browser_like: bool,
    pages: &[Url],
) -> Option<(Url, HeaderMap)> {
    if browser_like {
        None
    } else if given != target {
        Some((given.clone(), headers.clone()))
    } else {
        sites::origin(target, pages).map(|page| {
            let headers = scoped_headers(headers, given, &page);
            (page, headers)
        })
    }
}

/// The web pages a download's file may be given on again (`sites::origin`): the browser's tab
/// it was started from (`Options::page`), then its Referer.
fn pages_of(opts: &Options, headers: &HeaderMap) -> Vec<Url> {
    let referer = header_str(headers, header::REFERER).and_then(|r| Url::parse(r).ok());
    opts.page.iter().cloned().chain(referer).collect()
}

/// How long an idle keep-alive socket is kept: shorter than most servers' keep-alive, so a
/// paused download doesn't come back to sockets the server already closed.
const POOL_IDLE: Duration = Duration::from_secs(15);

/// The client for a download of `url`; a POST download's redirects are noted in `as_get`.
fn client(
    opts: &Options,
    url: &Url,
    as_get: &AsGet,
    seen: &Arc<crate::server::ServerSeen>,
) -> Result<Client, Error> {
    client_with(opts, url, as_get, seen, true)
}

/// [`client`], following redirects (`follow`) or none at all ([`hop_client`]).
fn client_with(
    opts: &Options,
    url: &Url,
    as_get: &AsGet,
    seen: &Arc<crate::server::ServerSeen>,
    follow: bool,
) -> Result<Client, Error> {
    let port = url.port_or_known_default().unwrap_or(443);
    let route = &opts.route;
    // An IP address in the URL never reaches the resolver, which drops what the chosen adapter
    // can't reach (#35): check it here.
    net::check_url(route, url)?;
    let mut builder = Client::builder();
    if !opts.extra_roots.is_empty() {
        builder = builder.tls_certs_merge(
            opts.extra_roots
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der).ok()),
        );
    }
    // With an adapter chosen, the system's proxy is looked up now, so that it can be checked
    // (and a proxy on this computer recognised) like the others.
    let system = match (&opts.proxy, route.is_any()) {
        (Proxy::System, false) => Some(net::system_proxy(url)),
        _ => None,
    };
    // The proxy that carries every request of this client, when it's on this computer: such a
    // connection isn't bound; where the proxy sends the traffic is up to it.
    let mut through_local_proxy = false;
    let mut check_proxy = |proxy: &str, covers_all: bool| -> Result<(), Error> {
        let host = Url::parse(proxy)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        if net::is_this_computer(&host) {
            through_local_proxy |= covers_all;
        } else if let Ok(ip) = host.trim_matches(['[', ']']).parse() {
            route.check(ip)?;
        }
        Ok(())
    };
    // Whether a proxy carries this download (#140: the address that answers is then the
    // proxy's): known here for each kind of setting.
    let mut proxied = false;
    let builder = match (&opts.proxy, system) {
        (Proxy::None, _) => builder.no_proxy(),
        (Proxy::System, Some(Some(proxy))) => {
            check_proxy(&proxy, true)?;
            proxied = true;
            builder.no_proxy().proxy(reqwest::Proxy::all(&proxy)?)
        }
        (Proxy::System, Some(None)) => builder.no_proxy(),
        // reqwest reads the system's proxy by default.
        (Proxy::System, None) => {
            proxied = net::system_proxy(url).is_some();
            builder
        }
        (
            Proxy::Manual {
                url: proxy,
                login,
                http,
                https,
            },
            _,
        ) => {
            let with_login = |p: reqwest::Proxy| match login {
                Some((user, pass)) => p.basic_auth(user, pass),
                None => p,
            };
            if *http || *https {
                check_proxy(proxy, *http && *https)?;
            }
            proxied = if url.scheme() == "https" {
                *https
            } else {
                *http
            };
            let mut b = builder.no_proxy();
            if *http {
                b = b.proxy(with_login(reqwest::Proxy::http(proxy.as_str())?));
            }
            if *https {
                b = b.proxy(with_login(reqwest::Proxy::https(proxy.as_str())?));
            }
            b
        }
        // The script's first route MDM can use: a proxy, or straight to the server.
        (Proxy::Pac { script }, _) => {
            let first = crate::pac::routes(script, url).ok().and_then(|routes| {
                routes.into_iter().find_map(|r| match r {
                    crate::pac::Route::Direct => Some(None),
                    crate::pac::Route::Proxy(p) => reqwest::Proxy::all(&p).ok().map(|_| Some(p)),
                })
            });
            // No usable answer: the system's proxy, as reqwest reads it (with an adapter
            // chosen, looked up now to be checked).
            let first = match first {
                None if !route.is_any() => Some(net::system_proxy(url)),
                other => other,
            };
            match first {
                Some(Some(p)) => {
                    check_proxy(&p, true)?;
                    proxied = true;
                    builder.no_proxy().proxy(reqwest::Proxy::all(&p)?)
                }
                Some(None) => builder.no_proxy(),
                None => {
                    proxied = net::system_proxy(url).is_some();
                    builder
                }
            }
        }
    };
    // A connection to this computer isn't bound (#35); its redirects may then only lead back
    // to this computer, or they would leave unbound.
    let to_this_computer = through_local_proxy
        || (net::is_this_computer(url.host_str().unwrap_or_default())
            && !matches!(opts.proxy, Proxy::Manual { .. } | Proxy::Pac { .. }));
    let builder = net::bind_client(builder, route, to_this_computer)?;
    let inner = match opts.post {
        Some(_) => as_get.policy(),
        None => get_policy(),
    };
    let builder = if !follow {
        builder
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
    } else if route.is_any() {
        builder.redirect(inner)
    } else {
        let route = route.clone();
        let stay_here = to_this_computer && !through_local_proxy;
        builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let refused = net::check_url(&route, attempt.url()).err().or_else(|| {
                (stay_here && !net::is_this_computer(attempt.url().host_str().unwrap_or(""))).then(
                    || net::NetError::Unbound {
                        label: route.label().unwrap_or_default().to_owned(),
                    },
                )
            });
            match refused {
                Some(e) => attempt.error(e),
                None => inner.redirect(attempt),
            }
        }))
    };
    seen.set_proxied(proxied);
    Ok(builder
        // Each connection's opening timed (#140): the download's own, no extra probe.
        .connector_layer(crate::server::Timed(seen.clone()))
        .dns_resolver(Arc::new(dns::Resolver::new(
            &opts.hosts,
            port,
            &opts.dns,
            &opts.extra_roots,
            route,
            Some(opts.connect_timeout),
        )))
        .http1_only()
        .user_agent(&opts.user_agent)
        .connect_timeout(opts.connect_timeout)
        .read_timeout(opts.read_timeout)
        .pool_max_idle_per_host(opts.connections() as usize)
        .pool_idle_timeout(POOL_IDLE)
        .tcp_nodelay(true)
        .build()?)
}

/// Runs a download body and records its final state.
async fn run(
    body: impl Future<Output = Result<Outcome, Error>>,
    shared: Arc<Shared>,
) -> Result<Outcome, Error> {
    let result = body.await;
    // Said as it stopped (`Shared::stopping`) when it was downloading.
    let said = shared.view.lock().unwrap().state == State::Stopping;
    match &result {
        Ok(o) => shared.event(format!("Complete: {}", o.path.display())),
        Err(Error::Cancelled) if said => {}
        Err(Error::Cancelled) => shared.event("Stopped"),
        Err(e) => shared.event(format!("Failed: {e}")),
    }
    // The body has let go of its place and its part file: whatever waits for them goes on.
    drop(shared.settle.lock().unwrap().take());
    let mut v = shared.view.lock().unwrap();
    v.state = match &result {
        Ok(o) => {
            // Where it really is: `finish` numbers the name when another file took it meanwhile,
            // and whoever reads the view (the app's row) goes by this.
            v.file = Some(o.path.clone());
            State::Done
        }
        Err(Error::Cancelled) => State::Cancelled,
        Err(e) => {
            v.error = Some(e.to_string());
            v.failure = Some(e.clone());
            State::Failed
        }
    };
    drop(v);
    result
}

/// Before a download starts at its place (`Destination::File`): the run stopped there just
/// before has let go of it, its last bytes and its resume state written (#1328: it said it was
/// stopped as its connections closed, and a slow disk may still be writing). Waits on a thread
/// that may wait, unless the download is cancelled meanwhile; at once when nothing settles there.
/// Whether it waited.
async fn after_settling(dest: &Destination, cancel: &CancellationToken) -> Result<bool, Error> {
    let Destination::File(place) = dest else {
        return Ok(false);
    };
    if !naming::is_settling(place) {
        return Ok(false);
    }
    let place = place.clone();
    let wait = tokio::task::spawn_blocking(move || naming::wait_settled(&place));
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = wait => Ok(true),
    }
}

/// Before a download looks at its folder: the deletes of old files there that it must come
/// after ([`naming::clear`]: a Redownload's, a duplicate overwritten, a restart under the same
/// name) have finished, on a thread that may wait (the disk can take seconds), unless the
/// download is cancelled meanwhile. Nothing being deleted there: at once.
async fn after_clearing(dir: &Path, cancel: &CancellationToken) -> Result<(), Error> {
    if !naming::being_cleared(dir) {
        return Ok(());
    }
    let dir = dir.to_owned();
    let wait = tokio::task::spawn_blocking(move || naming::wait_cleared(&dir));
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = wait => Ok(()),
    }
}

async fn run_fresh(
    req: Request,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
    kept: Option<KeptAnswer>,
) -> Result<Outcome, Error> {
    let mut opts = opts;
    // The first request had to give up the browser's User-Agent (#372).
    let mut asked_as_mdm = false;
    // It went under MDM's own name, the only one the site's bot check let through (#1422).
    let mut asked_own = false;
    shared.limiter.set(opts.speed_limit);
    let (shared, cancel) = (&shared, &cancel);
    shared.conn(0, |_| {});
    // Probe on connection 0: a plain GET (or the form's POST) whose body becomes the segment
    // starting at 0; or the look's answer kept for it (#28), not asked for again.
    // A web page where the download expects another kind of file is an error (#533): the look's
    // answer, asked before the file's name was settled, is asked for again then (`get` says so).
    let expects = expects_other_file(
        opts.expected_type.as_deref(),
        opts.expected_size,
        dest_name(&req.dest),
        &req.url,
    );
    let kept = kept.filter(|k| {
        k.url == req.url
            && k.at.elapsed() < KEEP_ANSWER_FOR
            && !(expects && is_web_page(&k.head.headers) && !named(&k.head.headers))
    });
    let mut unlocked = kept.as_ref().and_then(|k| k.unlocked.clone());
    // Where the first request went: a service's (or a helper's, #769) direct link when one gave
    // it, which isn't a redirect.
    let mut first_at = req.url.clone();
    // The browser-like connection the download goes on with, when only it got through (#668).
    let mut browser: Option<Arc<crate::browserlike::Browser>> = None;
    let (as_get, client0, (head, body), headers, ttfb) = match kept {
        Some(k) => {
            shared.event(
                "Connection 1: continuing from the answer the file's information window got",
            );
            let _ = shared.seen.set(k.seen);
            browser = k.browser;
            // The look got through under MDM's own name (#1422): so does the rest.
            if let Some(own) = k.own_agent {
                opts = speaking_own(&opts, own);
                asked_own = true;
            }
            (k.as_get, k.client, (k.head, k.body), k.headers, k.took)
        }
        None => {
            // Through a service account (#44): its direct link, without the host's cookies
            // or logins. The host's link stays the download's origin, asked again (through the
            // service) when the direct link dies.
            unlocked = unlock_origin(&opts, &req.url, Some(shared), cancel).await?;
            let (target, mut asked) = match &unlocked {
                Some(u) => (u.url.clone(), u.headers.clone()),
                None => (req.url.clone(), req.headers.clone()),
            };
            // A server that slows long answers down (`request_cap`): the first request is a
            // range of that size too, its total the file's size.
            if let Some(cap) = opts.cap() {
                asked.insert(header::RANGE, range_value(0, cap));
            }
            first_at = target.clone();
            let as_get = AsGet::default();
            let seen = shared.seen.get_or_init(Default::default).clone();
            // The download's own User-Agent, which the browser-like connection carries (#668),
            // even when MDM's connection had to give it up (#372, #1422).
            let given_agent = (opts.user_agent.clone(), opts.automatic_agent);
            // #1422: a site whose bot check let only MDM's own name through earlier this
            // session is asked so at once, not refused twice more first.
            let own = own_agent(&opts, &asked).filter(|_| unlocked.is_none());
            if let Some(own) = own.clone().filter(|_| crate::agent::own_for(&target)) {
                shared.event(format!(
                    "{}: its bot check let only Mister Download Manager's own name through \
                     earlier; asking so",
                    target.host_str().unwrap_or("?"),
                ));
                opts = speaking_own(&opts, own);
                asked_own = true;
            }
            // Its own name tried already: not once more after the browser-like connection.
            let own_tried = asked_own;
            let as_given = opts.clone();
            let ask = FirstAsk {
                as_get: &as_get,
                seen: &seen,
                expects,
                shared,
                cancel,
            };
            let (mut client0, mut first, as_mdm) = ask.at(&mut opts, &target, &asked).await?;
            asked_as_mdm = as_mdm;
            // #630: the link refused, where the browser's own download of it ended is asked
            // once, as the link was (the browser's User-Agent first). The link's refusal is
            // the one told if that fails too.
            if let Err(e) = &first
                && link_refused(e)
                && unlocked.is_none()
                && opts.post.is_none()
                && let Some(other) = opts.browser_final.clone().filter(|o| *o != target)
            {
                shared.event(format!(
                    "{}: {e}; asking where the browser's download ended ({})",
                    target.host_str().unwrap_or("?"),
                    other.host_str().unwrap_or("?"),
                ));
                let mut again = as_given;
                let headers = scoped_headers(&asked, &target, &other);
                match ask.at(&mut again, &other, &headers).await {
                    Ok((client1, Ok(answer), as_mdm)) => {
                        (opts, client0, first, asked_as_mdm) = (again, client1, Ok(answer), as_mdm);
                    }
                    Ok((_, Err(Error::Cancelled), _)) | Err(Error::Cancelled) => {
                        return Err(Error::Cancelled);
                    }
                    Ok((_, Err(e), _)) | Err(e) => {
                        shared.event(format!("{}: {e}", other.host_str().unwrap_or("?")));
                    }
                }
            }
            // #668: the site refused MDM's connection as a program's (its bot check, or a 412):
            // asked once more, the way a browser connects, at the same address with the same
            // headers. Its refusal stays the one told if that fails too.
            match first {
                Ok(((resp, headers), ttfb)) => {
                    let head = Head::of(&resp);
                    (as_get, client0, (head, Body::Http(resp)), headers, ttfb)
                }
                Err(e) if refused_as_a_program(&e) && unlocked.is_none() && opts.post.is_none() => {
                    if asked_as_mdm || asked_own {
                        (opts.user_agent, opts.automatic_agent) = given_agent;
                        (asked_as_mdm, asked_own) = (false, false);
                    }
                    if own_tried {
                        // Remembered, but no longer let through: learnt again next time.
                        crate::agent::forget_own(&target);
                    }
                    match browser_first(&opts, &target, &asked, e, expects, shared, cancel).await {
                        Ok((b, head, body, headers, ttfb)) => {
                            browser = Some(b);
                            (as_get, client0, (head, body), headers, ttfb)
                        }
                        // #1422: refused the way a browser connects too; once more under MDM's
                        // own name, the last try.
                        Err(refused) if refused_as_a_program(&refused) && !own_tried => {
                            let Some(own) = own else {
                                return Err(refused);
                            };
                            let (o, client1, ((resp, headers), ttfb)) =
                                ask.as_own(&opts, own, &target, &asked, refused).await?;
                            (opts, asked_own) = (o, true);
                            let head = Head::of(&resp);
                            (as_get, client1, (head, Body::Http(resp)), headers, ttfb)
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
    };
    // Whichever connection it came through, the browser-like one too (#954); the look's answer,
    // kept, was told as it came.
    shared.seen.get_or_init(Default::default).answered(&head);
    // The browser-like connection (#668) downloads on as many connections as any download: the
    // first part asked of it tells whether the site takes ranges on it (`wait_for_ranges`,
    // `request`), else it goes on with this one connection, as a browser downloads.
    // The service's own cap on connections to its link (Real-Debrid's "chunks").
    if let Some(cap) = unlocked
        .as_ref()
        .and_then(|u| u.max_connections)
        .filter(|c| *c >= 1)
    {
        opts.max_connections = opts.max_connections.min(cap);
    }
    let preempt_after = (ttfb * PREEMPT_TTFB).max(PREEMPT_MIN);
    let size = match head.status {
        StatusCode::OK => head.len,
        // Some servers answer partially even without Range; accept it if it starts at 0.
        StatusCode::PARTIAL_CONTENT => match parse_content_range(&head.headers)? {
            (0, _, total) => total,
            (start, ..) => return Err(Error::BadRange(format!("probe started at {start}"))),
        },
        s => return Err(Error::Status(s)),
    };
    let ranges = if head.status == StatusCode::PARTIAL_CONTENT {
        RANGES_YES
    } else {
        RANGES_UNKNOWN
    };
    // The first answer says the server takes ranges (`Accept-Ranges: bytes`).
    let advertised = ranges == RANGES_YES
        || header_str(&head.headers, header::ACCEPT_RANGES)
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("bytes")));
    let final_url = head.url.clone();
    shared.event(format!(
        "Connected to {}{}: {}, size {}",
        final_url.host_str().unwrap_or("?"),
        if final_url != first_at {
            " (redirected)"
        } else {
            ""
        },
        head.status,
        size.map_or("unknown".to_owned(), disk::human),
    ));
    // #1460: no larger than a page where the browser's file is larger than any, however it was
    // asked (the look's kept answer, the browser-like connection, MDM's own name).
    if let Some(sent) = size {
        not_the_file(opts.expected_size, sent)?;
    }
    // Credentials stay with the host they were given for, with the cookies pages set on the way
    // (`get`); the first request's own range (`request_cap`) isn't theirs.
    let mut headers = headers;
    headers.remove(header::RANGE);
    // A POST stays one if its redirects kept it (307, 308); it is sent again only once the
    // server has answered a range (206): `Accept-Ranges` alone isn't proof, and a form may do
    // something each time it is posted (an export, an order).
    let post = post_for(&opts, as_get.contains(&final_url), &mut headers);
    // So is a download on the browser-like connection (#668): one connection asks for a part
    // first, and the others follow its 206; a site that takes the whole file on it but refuses
    // its ranges (or ignores them) leaves it on one connection (`request`).
    let wait_for_ranges = (post.is_some() || browser.is_some()) && ranges != RANGES_YES;
    let h = &head.headers;
    // The expected hash: the user's, else what a full (200) answer announces for its content.
    let checksum = opts.checksum.clone().or_else(|| {
        (head.status == StatusCode::OK)
            .then(|| crate::verify::Hash::from_headers(h))
            .flatten()
    });
    let validators = Validators::from_response(
        header_str(h, header::ETAG),
        header_str(h, header::LAST_MODIFIED),
        header_str(h, header::DATE),
        final_url.host_str(),
    );
    // The server's date for the finished file (Options::server_time): its Last-Modified as
    // sent, whether or not it is strong enough to resume by.
    let stamp = opts
        .server_time
        .then(|| header_str(h, header::LAST_MODIFIED))
        .flatten()
        .and_then(|lm| httpdate::parse_http_date(lm).ok());

    let content_type = head
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // #701: a page where a kind of file that never is one was expected, whatever type it came
    // as (application/octet-stream, none, or text/html under a file's name of the server's): its
    // first bytes say so, before anything is saved. The name is the one it would be saved as.
    let never_page = never_a_page(
        opts.expected_type.as_deref(),
        &dest_name(&req.dest).map_or_else(
            || name_for(&head, None, unlocked.as_ref(), &opts),
            str::to_owned,
        ),
    );
    let body = if never_page {
        not_a_page(body, cancel).await?
    } else {
        body
    };
    // #1460: an answer that doesn't say its size, read as far as a page goes first where the
    // browser's file is larger: one that ends there isn't the file.
    let body = match size {
        None => not_short(body, opts.expected_size, cancel).await?,
        Some(_) => body,
    };
    // A file the engine names whose address and server leave its type unsaid: its first bytes
    // say it (#800), and still come first to the file.
    let (body, sniffed) =
        if matches!(req.dest, Destination::Dir(_)) && names_by_bytes(&head, unlocked.as_ref()) {
            sniff_body(body, cancel).await?
        } else {
            (body, None)
        };
    let (dest, _reservation, file) = open_destination(&req.dest, size, &opts, || {
        name_for(&head, sniffed, unlocked.as_ref(), &opts)
    })
    .await?;
    downloading(shared, &dest, size, ranges);
    let file = Arc::new(file);

    let size = if let Some(size) = size {
        let mut planner = Planner::new(size, opts.min_split);
        planner.set_preempt_after(preempt_after);
        *shared.planner.lock().unwrap() = Some(planner);
        let mut saved = ResumeState::new(
            req.url.clone(),
            final_url.clone(),
            dest.clone(),
            size,
            validators.clone(),
        );
        saved.content_type = content_type.clone();
        saved.checksum = checksum.clone();
        saved.final_get = opts.post.is_some() && post.is_none();
        saved.asked_as_mdm = asked_as_mdm;
        saved.asked_as_own = asked_own;
        saved.browser_like = browser.is_some();
        let ctx = Arc::new(Ctx {
            client: client0.clone(),
            validators: Mutex::new(validators),
            target: Mutex::new(Target {
                url: final_url.clone(),
                headers: headers.clone(),
                post: post.clone(),
                generation: 0,
            }),
            origin: origin_of(
                &req.url,
                &req.headers,
                &final_url,
                browser.is_some(),
                &pages_of(&opts, &req.headers),
            ),
            refreshed: Default::default(),
            answered: Default::default(),
            size,
            file: file.clone(),
            opts: opts.clone(),
            shared: shared.clone(),
            // A child token: the supervisor stops its own connections without cancelling the
            // download (which may still fall back to one connection).
            cancel: cancel.child_token(),
            stop: cancel.clone(),
            ranges: AtomicU8::new(ranges),
            advertised,
            wait_for_ranges,
            range_answered: Default::default(),
            as_get,
            web_page_ok: expects_web_page(&dest, content_type.as_deref()),
            never_page,
            ftp: None,
            browser: browser.clone(),
        });
        match segmented(ctx, saved, Some(body)).await {
            Ok(()) => size,
            // The server ignores ranges and the probe's data can't be kept: start over on
            // one connection.
            Err(Error::NoRanges) => {
                *shared.planner.lock().unwrap() = None;
                shared.view.lock().unwrap().resumable = Some(false);
                let body = match &browser {
                    Some(b) => {
                        let (head, body, _) = b.get(final_url, headers, false, cancel).await?;
                        crate::browserlike::check_first(&head, expects)?;
                        body
                    }
                    None => {
                        let agent = opts.site_agent();
                        let (resp, ..) = get(
                            &client0,
                            final_url,
                            headers,
                            post.as_ref(),
                            agent,
                            expects,
                            cancel,
                        )
                        .await?;
                        Body::Http(resp)
                    }
                };
                let body = if never_page {
                    not_a_page(body, cancel).await?
                } else {
                    body
                };
                single_stream(body, &file, shared, opts.global.as_deref(), cancel).await?
            }
            Err(e) => return Err(e),
        }
    } else {
        single_stream(body, &file, shared, opts.global.as_deref(), cancel).await?
    };
    verify_part(shared, &dest, checksum.as_ref()).await?;
    finish(shared, file, &dest, size, stamp, replaces(&req.dest, &opts))
}

/// Prepare the destination of a new download of `size` bytes (if known), named `name()` in a
/// chosen folder: the folder exists, the file fits on the drive, the name is reserved in this
/// process, and the part file is created (exclusively, for a chosen folder: another program
/// may have taken the name meanwhile) at its full size, and shown to `opts.on_file_created`.
async fn open_destination(
    dest: &Destination,
    size: Option<u64>,
    opts: &Options,
    name: impl FnOnce() -> String,
) -> Result<(PathBuf, naming::Reservation, File), Error> {
    // The folder must exist: we create it, but a removed drive or offline share can't be.
    let folder = dest.folder().to_owned();
    std::fs::create_dir_all(&folder).map_err(|_| Error::DestinationUnavailable(folder.clone()))?;
    if let Destination::File(p) | Destination::Free(p) = dest
        && !naming::path_fits(p)
    {
        return Err(Error::PathTooLong(p.clone()));
    }
    // Will it fit? Refuse now rather than fail half-way (a drive we can't identify is skipped).
    if let Some(size) = size {
        // Off the async threads, and bounded: a stale network drive can hang the volume probe.
        if let Some(volume) = disk::probe(folder).await {
            disk::check(&volume, size)?;
        }
    }
    let (dest, reservation, file) = match dest {
        Destination::File(p) => {
            let r = naming::reserve(p).ok_or_else(|| Error::InUse(p.clone()))?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(naming::part_path(p))?;
            (p.clone(), r, file)
        }
        Destination::Dir(dir) => {
            let name = name();
            // Shortened to fit Windows' path limit; a folder too deep for any name is an error.
            let name =
                naming::fit_name(dir, &name).ok_or_else(|| Error::PathTooLong(dir.join(&name)))?;
            claim_free(dir, &name, false)?
        }
        Destination::Free(p) => {
            let dir = p.parent().unwrap_or(Path::new(""));
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            claim_free(dir, &name, opts.replace_existing)?
        }
    };
    // Before any data: a mark the hook sets (Mark of the Web, quarantine) is on the file from
    // its first byte, and the final rename keeps it.
    if let Some(hook) = &opts.on_file_created {
        (hook.0)(&naming::part_path(&dest));
    }
    if let Some(size) = size {
        // Sparse first: writes far into a merely extended file make NTFS zero-fill up to them.
        if size >= sparse::MIN_SIZE {
            sparse::set(&file, true);
        }
        file.set_len(size)?;
    }
    Ok((dest, reservation, file))
}

/// `name` in `dir`, or the next free numbered name (`naming::unique_path`; with `replace`, a file
/// there to be replaced doesn't count: `naming::replacing_path`): reserved in this process, its
/// part file created exclusively (another program may take the name meanwhile).
fn claim_free(
    dir: &Path,
    name: &str,
    replace: bool,
) -> Result<(PathBuf, naming::Reservation, File), Error> {
    let mut attempt = 0;
    loop {
        let dest = if replace {
            naming::replacing_path(dir, name)
        } else {
            naming::unique_path(dir, name)
        };
        if let Some(r) = naming::reserve(&dest) {
            // Read too: reconnecting connections re-read their last bytes (overlap check).
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(naming::part_path(&dest))
            {
                Ok(file) => return Ok((dest, r, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {}
                Err(e) => return Err(e.into()),
            }
        }
        attempt += 1;
    }
}

/// The download has its file and is receiving it.
fn downloading(shared: &Shared, dest: &Path, size: Option<u64>, ranges: u8) {
    let mut v = shared.view.lock().unwrap();
    v.state = State::Downloading;
    v.file = Some(dest.to_owned());
    v.size = size;
    v.resumable = (ranges == RANGES_YES)
        .then_some(true)
        .or(size.is_none().then_some(false));
}

/// A new FTP download: as `run_fresh`, with the server's `SIZE` and `MDTM` for the answer's
/// headers and `REST` for ranges.
async fn run_fresh_ftp(
    req: Request,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<Outcome, Error> {
    shared.limiter.set(opts.speed_limit);
    let (shared, cancel) = (&shared, &cancel);
    let site = Arc::new(crate::ftp::Site::new(&req.url, &req.headers, &opts)?);
    // Connection 0 asks about the file and receives it from byte 0 (the probe).
    shared.conn(0, |_| {});
    let ((facts, first), ttfb) = first_try(&opts, shared, cancel, || site.start()).await?;
    let preempt_after = (ttfb * PREEMPT_TTFB).max(PREEMPT_MIN);
    let size = facts.size;
    shared.event(format!(
        "Connected to {} ({}), size {}",
        req.url.host_str().unwrap_or("?"),
        match req.url.scheme() {
            "ftp" => "FTP",
            _ => "FTPS",
        },
        size.map_or("unknown".to_owned(), disk::human),
    ));
    let ranges = if facts.rest {
        RANGES_YES
    } else {
        RANGES_UNKNOWN
    };
    // The modification time stands in for Last-Modified: resume checks it.
    let validators = Validators {
        last_modified: facts.modified.map(httpdate::fmt_http_date),
        ..Validators::default()
    };
    let stamp = opts.server_time.then_some(facts.modified).flatten();
    let (dest, _reservation, file) =
        open_destination(&req.dest, size, &opts, || naming::file_name(None, &req.url)).await?;
    downloading(shared, &dest, size, ranges);
    let file = Arc::new(file);
    let first = Body::Ftp(first);
    let size = if let Some(size) = size {
        let mut planner = Planner::new(size, opts.min_split);
        planner.set_preempt_after(preempt_after);
        *shared.planner.lock().unwrap() = Some(planner);
        // No password on disk: whoever resumes gives the login again.
        let url = crate::ftp::without_password(&req.url);
        let mut saved = ResumeState::new(
            url.clone(),
            url.clone(),
            dest.clone(),
            size,
            validators.clone(),
        );
        saved.checksum = opts.checksum.clone();
        let ctx = Arc::new(Ctx {
            // FTP's connections are its own (`ftp`): this client's are none of the server's.
            client: client(&opts, &req.url, &AsGet::default(), &Default::default())?,
            validators: Mutex::new(validators),
            target: Mutex::new(Target {
                url,
                headers: HeaderMap::new(),
                post: None,
                generation: 0,
            }),
            origin: None,
            refreshed: Default::default(),
            answered: Default::default(),
            size,
            file: file.clone(),
            opts: opts.clone(),
            shared: shared.clone(),
            cancel: cancel.child_token(),
            stop: cancel.clone(),
            ranges: AtomicU8::new(ranges),
            // FTP's REST is asked for, not advertised.
            advertised: true,
            wait_for_ranges: false,
            range_answered: Default::default(),
            as_get: AsGet::default(),
            web_page_ok: true,
            never_page: false,
            ftp: Some(site.clone()),
            browser: None,
        });
        match segmented(ctx, saved, Some(first)).await {
            Ok(()) => size,
            // REST refused: the probe's data can't be kept; the whole file on one connection.
            Err(Error::NoRanges) => {
                *shared.planner.lock().unwrap() = None;
                shared.view.lock().unwrap().resumable = Some(false);
                let t = cancellable(cancel, site.open_at(0, Some(size))).await?;
                single_stream(Body::Ftp(t), &file, shared, opts.global.as_deref(), cancel).await?
            }
            Err(e) => return Err(e),
        }
    } else {
        single_stream(first, &file, shared, opts.global.as_deref(), cancel).await?
    };
    verify_part(shared, &dest, opts.checksum.as_ref()).await?;
    finish(shared, file, &dest, size, stamp, replaces(&req.dest, &opts))
}

async fn run_resume(
    saved: ResumeState,
    state_file: PathBuf,
    mut headers: HeaderMap,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<Outcome, Error> {
    let mut opts = opts;
    if saved.asked_as_mdm
        && let Some(agent) = opts.agent_fallback.take()
    {
        opts.user_agent = agent;
    }
    // Only MDM's own name got past the site's bot check (#1422), for this download or for its
    // site earlier this session: it goes on under it.
    if !saved.browser_like
        && (saved.asked_as_own || crate::agent::own_for(&saved.url))
        && let Some(own) = own_agent(&opts, &headers)
    {
        shared.event(
            "Going on under Mister Download Manager's own name, as the site's bot check asked",
        );
        opts = speaking_own(&opts, own);
    }
    shared.limiter.set(opts.speed_limit);
    let bad = |what: &str| Error::BadState(format!("{}: {what}", state_file.display()));
    if state_file != state_path(&saved.dest) {
        return Err(bad("does not belong to the file it names"));
    }
    // Stopped a moment ago and still saving (#1328): it goes on from the state saved last.
    let saved = match after_settling(&Destination::File(saved.dest.clone()), &cancel).await? {
        true => ResumeState::load(&state_file).map_err(Error::BadState)?,
        false => saved,
    };
    if state_file != state_path(&saved.dest) {
        return Err(bad("does not belong to the file it names"));
    }
    let _reservation =
        naming::reserve(&saved.dest).ok_or_else(|| Error::InUse(saved.dest.clone()))?;
    let part = naming::part_path(&saved.dest);
    // A crash between the final rename and removing the state file: the download is complete.
    if !part.exists() && std::fs::metadata(&saved.dest).is_ok_and(|m| m.len() == saved.size) {
        let _ = std::fs::remove_file(&state_file);
        shared.set_state(State::Completing);
        return Ok(Outcome {
            path: saved.dest.clone(),
            size: saved.size,
        });
    }
    // Signed links from a redirect expire: ask the original address again for today's target
    // (it must still be the same file), else keep the stored one.
    let as_get = AsGet::default();
    let seen = shared.seen.get_or_init(Default::default).clone();
    let client = client(&opts, &saved.url, &as_get, &seen)?;
    // Only the browser-like connection got past the site's bot check (#668): it goes on with
    // it, at the address it had (MDM's own connection would be refused); its first connection
    // asks for its part before the others follow (`wait_for_ranges`).
    let browser = match saved.browser_like && crate::browserlike::AVAILABLE {
        true => Some(Arc::new(crate::browserlike::Browser::new(
            &opts,
            &saved.final_url,
            &headers,
        )?)),
        false => None,
    };
    if browser.is_some() {
        shared.event("Going on the way a browser connects, as the site's bot check asked");
    }
    // Through a service account (#44), the service is asked for today's link even when the
    // state holds none of its links (the download went directly until now).
    let refreshed =
        if browser.is_none() && (saved.url != saved.final_url || opts.unlocker.is_some()) {
            let post = opts.post.as_ref();
            let agent = opts.site_agent();
            refresh_final_url(
                &client,
                &saved.url,
                &headers,
                post,
                agent,
                saved.size,
                &cancel,
                opts.unlocker.as_ref(),
                &shared,
            )
            .await
        } else {
            None
        };
    // A POST download's target is asked with GET when its redirects turned it into one.
    let (url, final_get, refreshed_headers) = match refreshed {
        Some((url, h)) => (url.clone(), as_get.contains(&url), Some(h)),
        None => (saved.final_url.clone(), saved.final_get, None),
    };
    let origin = origin_of(
        &saved.url,
        &headers,
        &url,
        browser.is_some(),
        &pages_of(&opts, &headers),
    );
    // Credentials stay with the host they were given for (the refresh's own way there scoped
    // them as it went).
    headers = refreshed_headers.unwrap_or_else(|| scoped_headers(&headers, &saved.url, &url));
    let post = post_for(&opts, final_get, &mut headers);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&part)
        .map_err(|e| bad(&format!("part file: {e}")))?;
    if file.metadata()?.len() != saved.size {
        return Err(bad("part file has the wrong size"));
    }
    // FTP: the server's size and modification time must still be those of the data on disk.
    let ftp = if crate::ftp::is_ftp(&url) {
        let site = Arc::new(crate::ftp::Site::new(&url, &headers, &opts)?);
        match cancellable(&cancel, site.check()).await {
            Ok(facts) => {
                let modified = facts.modified.map(httpdate::fmt_http_date);
                let time_changed = !opts.ignore_mtime
                    && modified.is_some()
                    && saved.validators.last_modified.is_some()
                    && modified != saved.validators.last_modified;
                if time_changed || facts.size.is_some_and(|n| n != saved.size) {
                    return Err(Error::FileChanged);
                }
            }
            // Not answering now: the connections retry, and each checks its overlap.
            Err(e) if e.transient() => {}
            Err(e) => return Err(e),
        }
        Some(site)
    } else {
        None
    };
    shared.event(format!(
        "Resuming: {} of {} on disk, from {}",
        disk::human(saved.downloaded()),
        disk::human(saved.size),
        url.host_str().unwrap_or("?"),
    ));
    let mut planner = Planner::resume(saved.size, opts.min_split, &saved.segments)
        .map_err(|_| bad("segments do not cover the file"))?;
    planner.set_preempt_after(PREEMPT_MIN);
    *shared.planner.lock().unwrap() = Some(planner);
    {
        let mut v = shared.view.lock().unwrap();
        v.state = State::Downloading;
        v.file = Some(saved.dest.clone());
        v.size = Some(saved.size);
        v.resumable = Some(true);
    }
    let file = Arc::new(file);
    let (dest, size) = (saved.dest.clone(), saved.size);
    let checksum = opts.checksum.clone().or_else(|| saved.checksum.clone());
    // Given again by whoever resumes, as the headers are: not in the resume state.
    let replace = opts.replace_existing;
    let web_page_ok = expects_web_page(&dest, saved.content_type.as_deref());
    // The server's type for it then stands for the browser's (#701).
    let never_page = never_a_page(
        saved.content_type.as_deref(),
        &dest.file_name().unwrap_or_default().to_string_lossy(),
    );
    let ctx = Arc::new(Ctx {
        client,
        validators: Mutex::new({
            let mut v = saved.validators.clone();
            if opts.ignore_mtime {
                v.last_modified = None;
            }
            v
        }),
        // A POST's ranges may never have been tried (it came on one connection): one
        // connection asks first, the others follow its 206. So does the browser-like
        // connection's (#668), which may have come on one connection too.
        wait_for_ranges: (post.is_some() || browser.is_some()) && ftp.is_none(),
        target: Mutex::new(Target {
            url,
            headers,
            post,
            generation: 0,
        }),
        origin,
        refreshed: Default::default(),
        answered: Default::default(),
        size,
        file: file.clone(),
        opts,
        shared: shared.clone(),
        cancel: cancel.child_token(),
        stop: cancel.clone(),
        // Ranges worked before: a 200 now is a changed file or a range-less edge.
        ranges: AtomicU8::new(RANGES_YES),
        advertised: true,
        range_answered: Default::default(),
        as_get,
        web_page_ok,
        never_page,
        ftp,
        browser,
    });
    let stamp = ctx
        .opts
        .server_time
        .then(|| ctx.validators.lock().unwrap().last_modified.clone())
        .flatten()
        .and_then(|lm| httpdate::parse_http_date(&lm).ok());
    segmented(ctx, saved, None).await?;
    verify_part(&shared, &dest, checksum.as_ref()).await?;
    finish(&shared, file, &dest, size, stamp, replace)
}

/// Follow the original address again (with its own headers, and a POST download's body) as the
/// first request did (`get`: redirects, waiting pages, a site's own steps) and return where it
/// leads now, if that is still the same file: a ranged answer with the saved size; with the
/// headers to go on with there. None keeps the stored address (no redirect any more, a different
/// file, or the site is unreachable).
#[allow(clippy::too_many_arguments)]
async fn refresh_final_url(
    client: &Client,
    origin: &Url,
    headers: &HeaderMap,
    post: Option<&Bytes>,
    site_agent: bool,
    size: u64,
    cancel: &CancellationToken,
    unlocker: Option<&Arc<dyn crate::unlock::Unlocker>>,
    shared: &Shared,
) -> Option<(Url, HeaderMap)> {
    // Through a service account (#44): the service gives today's direct link (the host's link
    // itself leads to its web page); the host's cookies and logins stay with the host.
    let unlocked = match unlocker.filter(|_| post.is_none()) {
        Some(u) => unlock_noted(u, origin, Some(shared), cancel)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let (target, mut asked) = match &unlocked {
        Some(u) => (u.url.clone(), u.headers.clone()),
        None => (origin.clone(), headers.clone()),
    };
    asked.insert(header::RANGE, HeaderValue::from_static("bytes=0-0"));
    let (resp, mut headers, _) = get(client, target, asked, post, site_agent, false, cancel)
        .await
        .ok()?;
    headers.remove(header::RANGE);
    if resp.status() != StatusCode::PARTIAL_CONTENT {
        return None;
    }
    let (_, _, total) = parse_content_range(resp.headers()).ok()?;
    let url = resp.url().clone();
    if total != Some(size) {
        return None;
    }
    // Read to its end (one byte), the connection goes back to the pool, where a connection that
    // asks for its part after it has gone back takes it rather than opening one more (two round
    // trips to a far server, #1328).
    let mut resp = resp;
    let mut read = 0;
    while read < PAGE_SNIFF_BYTES
        && let Ok(Some(chunk)) = cancellable(cancel, async { Ok(resp.chunk().await?) }).await
    {
        read += chunk.len();
    }
    Some((url, headers))
}

/// Check the finished part file against `hash` (at most two files at once: it reads them
/// whole). No hash, nothing to do.
async fn verify_part(
    shared: &Shared,
    dest: &Path,
    hash: Option<&crate::verify::Hash>,
) -> Result<(), Error> {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    shared.every_byte_in();
    let Some(hash) = hash.cloned() else {
        return Ok(());
    };
    shared.set_state(State::Verifying);
    let _slot = SLOTS.acquire().await.map_err(|_| Error::Cancelled)?;
    let part = naming::part_path(dest);
    let algo = hash.algo;
    // Counted as it reads (#736): the progress window and the list show how far it is.
    let checked = shared.checked.clone();
    checked.store(0, Ordering::Relaxed);
    let ok =
        tokio::task::spawn_blocking(move || crate::verify::matches_counted(&part, &hash, &checked))
            .await
            .expect("verifier panicked")?;
    if ok {
        shared.event(format!("Checksum verified ({})", algo.name()));
        Ok(())
    } else {
        Err(Error::ChecksumMismatch(algo.name()))
    }
}

/// Flush and rename the part file to its final name. With `replace` (`Options::replace_existing`)
/// a file at `dest` is replaced now, as the download completes.
fn finish(
    shared: &Shared,
    file: Arc<File>,
    dest: &Path,
    size: u64,
    stamp: Option<std::time::SystemTime>,
    replace: bool,
) -> Result<Outcome, Error> {
    shared.every_byte_in();
    shared.set_state(State::Completing);
    let file = Arc::try_unwrap(file).map_err(|_| io::Error::other("part file still in use"))?;
    file.set_len(size)?;
    // Every byte is written: a normal file again (nothing left to allocate).
    if size >= sparse::MIN_SIZE {
        sparse::set(&file, false);
    }
    file.sync_all()?;
    // The server's date (Options::server_time), set before the rename moves the file.
    if let Some(t) = stamp {
        let times = std::fs::FileTimes::new().set_modified(t).set_accessed(t);
        #[cfg(windows)]
        let times = {
            use std::os::windows::fs::FileTimesExt;
            times.set_created(t)
        };
        let _ = file.set_times(times);
    }
    drop(file);
    let part = naming::part_path(dest);
    let original = dest;
    let numbered = || {
        let dir = original.parent().unwrap_or(Path::new("."));
        let name = original
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        naming::unique_path(dir, name)
    };
    // Antivirus scanners and indexers briefly lock new files on Windows: retry the rename.
    let rename = |to: &Path| {
        craft_atomic::retry_rename(craft_atomic::RenameRetry::platform(), || {
            std::fs::rename(&part, to)
        })
    };
    let dest = match final_place(dest, replace) {
        Final::Free => {
            rename(dest)?;
            dest.to_owned()
        }
        // The user chose to replace it (`Options::replace_existing`): renamed over it (on every
        // system `rename` replaces a file). One that can't be (open in another program,
        // read-only) is kept: the download is saved beside it, numbered.
        Final::Replace => match rename(dest) {
            Ok(()) => {
                shared.event(format!("Replaced the file that was at {}", dest.display()));
                dest.to_owned()
            }
            Err(e) => {
                let other = numbered();
                shared.event(format!(
                    "Could not replace {} ({e}): saved as {}",
                    dest.display(),
                    other.display()
                ));
                rename(&other)?;
                other
            }
        },
        // Never replace a file that appeared at the destination meanwhile (rename replaces on
        // Windows): take the next free numbered name instead.
        Final::Numbered => {
            let other = numbered();
            rename(&other)?;
            other
        }
    };
    // Only now is the resume state obsolete.
    let _ = std::fs::remove_file(state_path(original));
    Ok(Outcome { path: dest, size })
}

/// Whether a new download to `dest` may replace the file at its place
/// ([`Options::replace_existing`]): never one the server names in a folder.
fn replaces(dest: &Destination, opts: &Options) -> bool {
    opts.replace_existing && !matches!(dest, Destination::Dir(_))
}

/// What a finished download's part file becomes ([`final_place`]).
#[derive(Debug, PartialEq, Eq)]
enum Final {
    /// Nothing is at its name: it takes it.
    Free,
    /// A file is there that the user chose to replace (`Options::replace_existing`).
    Replace,
    /// Something is there that it may not replace: the next free numbered name.
    Numbered,
}

/// Where a download finishing at `dest` goes, `replace` being `Options::replace_existing`. A
/// folder at that name is never replaced.
fn final_place(dest: &Path, replace: bool) -> Final {
    if !dest.exists() {
        Final::Free
    } else if replace && !dest.is_dir() {
        Final::Replace
    } else {
        Final::Numbered
    }
}

/// Run the connections with the resume state saved every `save_interval`, and once more when
/// they stop. The state file is removed when the download completes.
async fn segmented(
    ctx: Arc<Ctx>,
    mut saved: ResumeState,
    first: Option<Body>,
) -> Result<(), Error> {
    let path = state_path(&saved.dest);
    let dest = saved.dest.clone();
    let mut synced = Synced {
        at: std::time::Instant::now(),
        bytes: 0,
        took: Duration::ZERO,
        covered: 0,
    };
    let interval = ctx.opts.save_interval;
    // Asked to stop while the connections ran (`Shared::stopping`).
    let mut told = false;
    let result = {
        // Saves until told to stop or a save fails (resolving to that error). It stops only
        // between saves: a save's blocking write can't be cancelled, so dropping the saver
        // mid-save would leave that write running after the download has returned, writing a
        // stale state file next to a finished or already resumed download (#106).
        let stop = CancellationToken::new();
        let saver = async {
            // The state as the connections start, beside them: a resume's first request no
            // longer waits for a write to the disk (#1328).
            if let Err(e) = save_state(&ctx, &mut saved, &path, &mut synced, true).await {
                return Some(e);
            }
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {}
                    () = stop.cancelled() => return None,
                }
                if let Err(e) = save_state(&ctx, &mut saved, &path, &mut synced, false).await {
                    return Some(e);
                }
            }
        };
        let supervisor = supervise(ctx.clone(), first);
        tokio::pin!(saver, supervisor);
        loop {
            tokio::select! {
                r = &mut supervisor => {
                    // The connections stopped: let a save under way finish. Its error, if any,
                    // is the final save's to report (it writes the same file, forced, below).
                    stop.cancel();
                    let _ = saver.as_mut().await;
                    break r;
                }
                e = &mut saver => {
                    // The connections are stopped and waited for, as every stop does (#201).
                    ctx.cancel.cancel();
                    let _ = supervisor.as_mut().await;
                    break Err(e.expect("the saver only stops when told to"));
                }
                // Paused or stopped (#1328): the download says so as its connections close, not
                // once a disk slower than the network has taken what they received and the
                // state (a flush under way, then the last one: over a minute, measured).
                () = ctx.stop.cancelled(), if !told => {
                    told = true;
                    ctx.shared.stopping(&dest);
                }
            }
        }
    };
    match &result {
        // Not resumable on one connection: the state is useless.
        Err(Error::NoRanges) => {
            let _ = std::fs::remove_file(&path);
        }
        // Complete: the final state stays until `finish` has renamed the part file, so a
        // crash in between still resumes (to an immediate rename).
        Ok(()) | Err(_) => save_state(&ctx, &mut saved, &path, &mut synced, true).await?,
    }
    // Said stopped, it stays stopped, even when its last bytes came in meanwhile: the next start
    // finds every byte in, checks and names the file (#1328).
    if told && result.is_ok() {
        return Err(Error::Cancelled);
    }
    result
}

/// While downloading, the part file is flushed (and the state rewritten) only after 1/16 of
/// the file (1-64 MiB) has arrived or this long, at most once per save tick, and always when
/// the connections stop. IDM saves "several times per minute" [IDM-doc].
///
/// A flush holds every write to the file for as long as it runs (NTFS, measured, #361); the
/// connections keep receiving into memory meanwhile (`HOLD_MAX`), so a flush the memory covers
/// costs the download nothing. What a flush has to write is what still waits in the system's
/// cache: Windows writes it out lazily, so that is about all that came since the last flush,
/// and on a disk slower than the network, all that piled up. Spacing the flushes out by how
/// long the last one took (0.5.1 to 0.5.2) therefore made each one longer than the last: past
/// the first gigabytes a flush took over 10 s and the download froze (#782). So a flush is only
/// started when it should end well before the memory runs out (`Synced::fits`): flushes come
/// as often as the file grows while they stay short, and on a disk that can't keep up they
/// stop, and the system holds the writes to the disk's pace instead, evenly. They start again
/// if the download slows down enough for them to fit.
///
/// What it gives up: after a power cut or a crash the bytes since the last flush are downloaded
/// again: about 2 s of them on a disk that keeps up, everything since it fell behind on one
/// that doesn't. Pausing, stopping and finishing still flush everything, and the saved
/// positions never claim bytes that weren't flushed.
const SYNC_BYTES_MAX: u64 = 64 << 20;
const SYNC_BYTES_MIN: u64 = 1 << 20;
const SYNC_EVERY: Duration = Duration::from_secs(30);

fn sync_bytes(size: u64) -> u64 {
    (size / 16).clamp(SYNC_BYTES_MIN, SYNC_BYTES_MAX)
}

/// When the part file was last flushed, how many bytes were on disk then, how long it took,
/// and the bytes it caught up with (those written since the flush before it).
struct Synced {
    at: std::time::Instant,
    bytes: u64,
    took: Duration,
    covered: u64,
}

impl Synced {
    /// Is a flush due with `bytes` on disk now, `elapsed` since the last one ended, `room`
    /// left in memory for what arrives meanwhile?
    fn due(&self, bytes: u64, size: u64, elapsed: Duration, room: u64) -> bool {
        let new = bytes.saturating_sub(self.bytes);
        (new >= sync_bytes(size) || elapsed >= SYNC_EVERY) && self.fits(new, elapsed, room)
    }

    /// Whether a flush now, `new` bytes and `elapsed` after the last one, should take at most
    /// three quarters of the time `room` lasts at the pace those bytes came: the connections
    /// then keep receiving through it. With the memory full (a disk that sets the pace), none
    /// does: it would stop them for as long as it takes.
    fn fits(&self, new: u64, elapsed: Duration, room: u64) -> bool {
        let lasts = room as f64 * elapsed.as_secs_f64() / new.max(1) as f64;
        self.expected(new).as_secs_f64() <= lasts * 0.75
    }

    /// How long a flush of `new` bytes (written since the last one began) should take: as long
    /// as that one took for each byte it caught up with. What waits in the cache is what came
    /// since (above), by bytes, not by time: what the connections kept in memory while the last
    /// flush held their writes reaches the cache all at once when it ends. Counted by the
    /// seconds since, a flush right after a long one was expected to take a seventh of what it
    /// did, and stopped the transfer for the rest (on GitHub's macOS runners: 3 s, 1.4 s of it
    /// with the memory full).
    fn expected(&self, new: u64) -> Duration {
        if self.covered == 0 {
            return self.took;
        }
        self.took.mul_f64(new as f64 / self.covered as f64)
    }
}

/// Save the resume state if it is due (`force`: always). `saved` is the state last written; it
/// never claims bytes that weren't flushed (snapshot -> flush -> write).
async fn save_state(
    ctx: &Ctx,
    saved: &mut ResumeState,
    path: &Path,
    synced: &mut Synced,
    force: bool,
) -> Result<(), Error> {
    let segments = ctx.shared.with_planner(|p| p.save());
    let validators = ctx.validators.lock().unwrap().clone();
    let target = ctx.target();
    let (final_url, final_get) = (target.url, ctx.opts.post.is_some() && target.post.is_none());
    let meta_changed = validators != saved.validators
        || final_url != saved.final_url
        || final_get != saved.final_get;
    let bytes: u64 = segments.iter().map(|s| s.cursor - s.start).sum();
    // Room left in memory for what arrives while the flush holds the writes.
    let room = HOLD_MAX.saturating_sub(ctx.shared.held.load(Ordering::Relaxed));
    let due = force || synced.due(bytes, ctx.size, synced.at.elapsed(), room);
    let file = ctx.file.clone();
    let path = path.to_owned();
    if !due {
        // Between flushes only a new validator or address is written (with segments on disk).
        if meta_changed {
            saved.validators = validators;
            saved.final_url = final_url;
            saved.final_get = final_get;
            let s = saved.clone();
            tokio::task::spawn_blocking(move || s.write(&path))
                .await
                .expect("state writer panicked")?;
        }
        return Ok(());
    }
    if segments == saved.segments && !meta_changed && path.exists() {
        // Nothing new since the last write.
        *synced = Synced {
            at: std::time::Instant::now(),
            bytes,
            took: Duration::ZERO,
            covered: 0,
        };
        return Ok(());
    }
    let mut s = saved.clone();
    s.validators = validators;
    s.final_url = final_url;
    s.final_get = final_get;
    // A save that claims no byte the last one didn't has nothing to flush: the first state of a
    // new download, a resume's new address (a signed link asked again) or validators. A flush
    // waits for all the disk still has to write, and a resume waited for it (#1328).
    if bytes <= saved.downloaded() {
        s.segments = segments;
        *saved = tokio::task::spawn_blocking(move || s.write(&path).map(|()| s))
            .await
            .expect("state writer panicked")?;
        return Ok(());
    }
    // Tests' slow disk (its extra flush time not for the forced saves at the start and the end).
    let slow = ctx.opts.slow_disk.clone();
    let began = std::time::Instant::now();
    let done = tokio::task::spawn_blocking(move || match slow {
        Some(disk) => disk.flush(!force, || s.save(&path, segments, &file).map(|()| s)),
        None => s.save(&path, segments, &file).map(|()| s),
    })
    .await
    .expect("state writer panicked");
    *saved = done?;
    *synced = Synced {
        at: std::time::Instant::now(),
        bytes,
        took: forced_gap(force, began.elapsed()),
        covered: bytes.saturating_sub(synced.bytes),
    };
    Ok(())
}

/// The flush time the next flush is judged by. A forced save says nothing of the next one: the
/// last runs once the connections have stopped, and the first as they start, before they have
/// written much (it usually claims nothing and flushes nothing); the first one, which also paid
/// for creating the part file, once delayed the first save of progress past the end of a short
/// download (#518).
fn forced_gap(force: bool, took: Duration) -> Duration {
    if force { Duration::ZERO } else { took }
}

/// A refusal of the address asked for that the browser's own download got past (#630): 401
/// and 403 (a token bound to the browser's session), 404 and 410 (a one-time link the browser
/// already spent, which some servers then forget, others call gone), 412 (FileHippo's answer
/// to whatever isn't a browser). A site's bot check stays the site's call (#376).
fn link_refused(e: &Error) -> bool {
    matches!(e, Error::Status(s) if matches!(s.as_u16(), 401 | 403 | 404 | 410 | 412))
        // The site's page where the browser got the file (#701): its link spent, or bound to
        // the browser's visit. So is an answer far smaller than the browser's file (#1460).
        || matches!(e, Error::GotWebPage | Error::NotTheFile { .. })
}

/// What a download's first request needs besides its address, headers and options.
struct FirstAsk<'a> {
    as_get: &'a AsGet,
    seen: &'a Arc<crate::server::ServerSeen>,
    expects: bool,
    shared: &'a Shared,
    cancel: &'a CancellationToken,
}

/// A first request's client, its answer (or why not) and whether it went as MDM (#372).
type FirstAnswer = (
    Client,
    Result<((Response, HeaderMap), Duration), Error>,
    bool,
);

impl FirstAsk<'_> {
    /// `first_get` at `url` with its client; asked once more as MDM when the browser's
    /// User-Agent is refused (#372, `opts` then goes on as MDM: the third value says so).
    async fn at(
        &self,
        opts: &mut Options,
        url: &Url,
        headers: &HeaderMap,
    ) -> Result<FirstAnswer, Error> {
        let (shared, cancel, expects) = (self.shared, self.cancel, self.expects);
        let client0 = client(opts, url, self.as_get, self.seen)?;
        match first_get(&client0, url, headers, opts, expects, shared, cancel).await {
            Err(e) if agent_refused(&e) && opts.agent_fallback.is_some() => {
                // #372: the rest of the download goes as MDM too.
                opts.user_agent = opts.agent_fallback.take().unwrap_or_default();
                shared.event(
                    "The server refused the browser's User-Agent; asking as Mister Download Manager",
                );
                let client1 = client(opts, url, self.as_get, self.seen)?;
                let first = first_get(&client1, url, headers, opts, expects, shared, cancel).await;
                Ok((client1, first, true))
            }
            first => Ok((client0, first, false)),
        }
    }

    /// #1422: the first request once more, after the site's bot check refused the download's
    /// User-Agent and the browser-like connection too (`refused`, the error told if this fails
    /// as well): on MDM's own connection under `own` (`Options::own_agent`), at the same address
    /// with the same headers (cookies, Referer, range), once. With the answer, the options and
    /// client the download goes on with; the site is remembered for the session.
    async fn as_own(
        &self,
        opts: &Options,
        own: String,
        url: &Url,
        headers: &HeaderMap,
        refused: Error,
    ) -> Result<(Options, Client, ((Response, HeaderMap), Duration)), Error> {
        let (shared, cancel) = (self.shared, self.cancel);
        let host = url.host_str().unwrap_or("?");
        shared.event(format!(
            "{host}: refused the way a browser connects too; asking once more as Mister \
             Download Manager itself ({own})"
        ));
        let opts = speaking_own(opts, own);
        let client = client(&opts, url, self.as_get, self.seen)?;
        match first_get(&client, url, headers, &opts, self.expects, shared, cancel).await {
            Ok(answer) => {
                crate::agent::remember_own(url);
                shared.event(format!(
                    "{host}: let Mister Download Manager through under its own name; \
                     downloading so (remembered for this session)"
                ));
                Ok((opts, client, answer))
            }
            Err(e @ (Error::Cancelled | Error::Adapter(_))) => Err(e),
            Err(e) => {
                shared.event(format!("{host}, asked as Mister Download Manager: {e}"));
                Err(refused)
            }
        }
    }
}

/// A plain 403, which a browser's User-Agent may have brought on (#372). A site's bot check
/// isn't asked again as MDM: it gets one try the way a browser connects (`browser_first`).
fn agent_refused(e: &Error) -> bool {
    matches!(e, Error::Status(s) if s.as_u16() == 403)
}

/// A first answer that refuses MDM's connection for not being a browser's (#668): the site's bot
/// check (`Error::BotCheck`, Cloudflare's), or 412 Precondition Failed, which MDM's first
/// request, sent without any precondition (no `If-…` header), can only get as such a refusal
/// (FileHippo's, #630).
fn refused_as_a_program(e: &Error) -> bool {
    matches!(e, Error::BotCheck) || matches!(e, Error::Status(s) if s.as_u16() == 412)
}

/// The User-Agent a download may name itself with at a site whose bot check refused it
/// (#1422, `Options::own_agent`): never in place of one the request's own headers carry (the
/// caller's choice), nor when it is the download's already.
fn own_agent(opts: &Options, headers: &HeaderMap) -> Option<String> {
    opts.own_agent
        .clone()
        .filter(|own| *own != opts.user_agent && !headers.contains_key(header::USER_AGENT))
}

/// `opts` speaking under MDM's own name `own` (#1422): no other User-Agent is tried after it.
fn speaking_own(opts: &Options, own: String) -> Options {
    let mut opts = opts.clone();
    opts.speak_as(own);
    opts.agent_fallback = None;
    opts
}

/// #668: the first request once more, after the site refused MDM's own connection as a
/// program's (`refused_as_a_program`, its error `refused`): through the browser-like connection
/// (`crate::browserlike`), at the same address with the same headers, once, its answer checked
/// as the first one is. When that fails too, `refused` is what is told (the error users know,
/// with its way out: the browser), the reason in the event log. With the answer, the connection
/// the download goes on with.
async fn browser_first(
    opts: &Options,
    url: &Url,
    headers: &HeaderMap,
    refused: Error,
    expects: bool,
    shared: &Shared,
    cancel: &CancellationToken,
) -> Result<
    (
        Arc<crate::browserlike::Browser>,
        Head,
        Body,
        HeaderMap,
        Duration,
    ),
    Error,
> {
    if !crate::browserlike::AVAILABLE {
        return Err(refused);
    }
    let host = url.host_str().unwrap_or("?");
    shared.event(format!(
        "{host}: {refused}; asking once more the way a browser connects"
    ));
    let (b, asked) = match browser_ask(opts, url, headers, expects, cancel).await {
        Ok(got) => got,
        Err(e @ (Error::Cancelled | Error::Adapter(_))) => return Err(e),
        Err(e) => {
            shared.event(format!("{host}, asked the way a browser connects: {e}"));
            return Err(refused);
        }
    };
    let (head, body, headers, took) = asked;
    shared.event(format!(
        "{host}: the browser-like connection got through; downloading on it"
    ));
    Ok((b, head, body, headers, took))
}

/// The browser-like connection for `url`, and its answer to the first request (`browser_first`,
/// `probe_kept`): the file's, or why not.
async fn browser_ask(
    opts: &Options,
    url: &Url,
    headers: &HeaderMap,
    expects: bool,
    cancel: &CancellationToken,
) -> Result<
    (
        Arc<crate::browserlike::Browser>,
        (Head, Body, HeaderMap, Duration),
    ),
    Error,
> {
    let b = Arc::new(crate::browserlike::Browser::new(opts, url, headers)?);
    let asked = std::time::Instant::now();
    let (head, body, headers) = b.get(url.clone(), headers.clone(), false, cancel).await?;
    let took = asked.elapsed();
    crate::browserlike::check_first(&head, expects)?;
    Ok((b, (head, body, headers, took)))
}

/// A plain `GET` (no Range), or a POST of `post`, that must answer 200 or 206. A GET follows
/// the HTTP redirects (the client's policy) and then the redirects a page makes itself (#28):
/// a download site's "your download starts in a moment" page, sent with a `Refresh` header or a
/// `<meta http-equiv="refresh">`, leads on to the file as it does in a browser (`refresh_target`,
/// `meta_refresh`). Its wait, up to `PAGE_WAIT`, is kept, and the cookies it sets go on to its
/// own host. A page that leads on only with a script isn't followed: it stays the answer.
///
/// A site whose links aren't the file's address (`sites`, #34: Google Drive) is asked its own
/// way: its link goes to the file's address first, its pages say where to go next or why it
/// refuses (`Error::Refused`), and a page of its where the file was expected is an error, never
/// the file. So is any web page when the download `expects` another kind of file (#533,
/// `expects_other_file`): a server's refusal, a sign-in wall. A small answer without a type is
/// looked into then, and asked for again if it isn't a page.
///
/// With the answer, `headers` as the download goes on with them at the address it ended at:
/// credentials scoped to it (`scoped_headers`), the pages' cookies added; and how long the last
/// request took to answer (a page's wait and its own request not counted: the time to first byte).
#[allow(clippy::too_many_arguments)]
async fn get(
    client: &Client,
    url: Url,
    headers: HeaderMap,
    post: Option<&Bytes>,
    site_agent: bool,
    expects: bool,
    cancel: &CancellationToken,
) -> Result<(Response, HeaderMap, Duration), Error> {
    let (mut url, mut headers) = (url, headers);
    // A site's link is asked at its file's address (a Drive link: Drive's file host), the same
    // party, so the user's cookies go too. A form's POST goes where it was given.
    if post.is_none() {
        if let Some(r) = sites::not_a_file(&url) {
            return Err(Error::Refused(r));
        }
        if let Some(next) = sites::rewrite(&url) {
            headers = scoped_headers(&headers, &url, &next);
            url = next;
        }
    }
    // After a page's redirect, a page read but leading nowhere is asked for again, as is.
    let mut hops = 0;
    // A site's refusal while it is looked at once more (`sites::second_look`), with the address
    // that refused; and whether that look was taken.
    let mut held: Option<(Refusal, Url)> = None;
    let mut looked = false;
    // Redirects between a site's own hosts followed here (`get_policy`).
    let mut party_hops = 0;
    // A site's form to post with the next request only (`PageSays::Form`: LuluStream's), and how
    // many times one was: a site that answers with its form again didn't take it this time
    // (LuluStream's "Security error", about a post in three), and it is posted once more, up to
    // `MAX_FORM_POSTS`; those posts don't count as a page's redirects.
    let mut form: Option<Bytes> = None;
    let mut posts = 0;
    loop {
        let req = match form.take() {
            Some(body) => {
                let mut with_type = headers.clone();
                with_type.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                build(client, url.clone(), Some(&body), &with_type, site_agent)
            }
            None => build(client, url.clone(), post, &headers, site_agent),
        };
        let asked = std::time::Instant::now();
        let resp = cancellable(cancel, async { Ok(req.send().await?) }).await?;
        let took = asked.elapsed();
        let from = resp.url().clone();
        // A redirect between a site's own hosts, which the client leaves (`get_policy`): followed
        // here, the user's credentials and the cookies it sets going on.
        if post.is_none()
            && resp.status().is_redirection()
            && party_hops < MAX_REDIRECTS
            && let Some(next) = header_str(resp.headers(), header::LOCATION)
                .and_then(|l| from.join(l).ok())
                .filter(|next| sites::same_party(&from, next))
        {
            headers = scoped_headers(&scoped_headers(&headers, &url, &from), &from, &next);
            if !(from.scheme() == "https" && next.scheme() != "https") {
                add_cookies(&mut headers, &set_cookies(resp.headers()));
            }
            url = next;
            party_hops += 1;
            continue;
        }
        // The second look stands only if it leads to the file another way (a Google Doc's
        // editor, whose export follows below); otherwise the refusal was the answer.
        if let Some((refusal, refused_at)) = held.take() {
            let other_way = resp.status() == StatusCode::OK
                && is_web_page(resp.headers())
                && sites::rewrite(&from).is_some_and(|n| n != refused_at);
            if !other_way {
                return Err(Error::Refused(refusal));
            }
        }
        if let Some(r) = sites::refused(&url, &from, resp.status().as_u16()) {
            return Err(Error::Refused(r));
        }
        match resp.status() {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
            s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::SERVICE_UNAVAILABLE => {
                return Err(Error::Throttled(s, retry_after(resp.headers())));
            }
            s if is_bot_check(s, resp.headers()) => return Err(Error::BotCheck),
            s => return Err(Error::Status(s)),
        }
        // Where the HTTP redirects ended: the headers' credentials follow as reqwest's did.
        headers = scoped_headers(&headers, &url, &from);
        // A site's page, without a file name, where its file was expected: the site talking
        // (a page it changed, a wall), never the file.
        let site_page = post.is_none()
            && resp.status() == StatusCode::OK
            && is_web_page(resp.headers())
            && !named(resp.headers())
            && (sites::expects_file(&from) || expects);
        // No type said where another kind of file is expected (#533): small, it is looked into.
        let untyped = post.is_none()
            && expects
            && hops < MAX_PAGE_REDIRECTS
            && resp.status() == StatusCode::OK
            && !resp.headers().contains_key(header::CONTENT_TYPE)
            && !named(resp.headers())
            && resp.content_length().is_none_or(|n| n <= PAGE_PEEK as u64);
        // A form is never posted again, and a page's own redirects end somewhere.
        if post.is_some() || resp.status() != StatusCode::OK || hops >= MAX_PAGE_REDIRECTS {
            if site_page {
                return Err(Error::GotWebPage);
            }
            return Ok((resp, headers, took));
        }
        let cookies = set_cookies(resp.headers());
        let refresh =
            header_text(resp.headers(), header::REFRESH).and_then(|v| refresh_target(&v, &from));
        // A site's link reached through redirects (a short link to a Drive file; a Google Doc's
        // editor after Drive's `open?id=`): its file's address.
        let site_link = is_web_page(resp.headers())
            .then(|| sites::rewrite(&from))
            .flatten();
        let (wait, next) = if let Some(next) = refresh {
            next
        } else if let Some(next) = site_link {
            (Duration::ZERO, next)
        } else if (is_web_page(resp.headers()) || untyped)
            // A web page small enough to look into (larger ones aren't waiting pages).
            && resp.content_length().is_none_or(|n| n <= PAGE_PEEK as u64)
        {
            let page = read_up_to(resp, PAGE_PEEK, cancel).await?;
            // Without a type and not a page: the file, asked for again as is.
            let site_page = site_page || (untyped && sniffs_as_html(&page));
            if untyped && !site_page {
                add_cookies(&mut headers, &cookies);
                url = from;
                hops = MAX_PAGE_REDIRECTS;
                continue;
            }
            match sites::read_page(&from, &page) {
                PageSays::Next(next) => (Duration::ZERO, next),
                // Posted, its answer read as a page (the file's link, a refusal, the form again).
                PageSays::Form { action, body } if posts < MAX_FORM_POSTS => {
                    if posts > 0 {
                        // The form again: posted once more, not a page's redirect.
                        hops -= 1;
                    }
                    posts += 1;
                    form = Some(Bytes::from(body));
                    (Duration::ZERO, action)
                }
                // Still the form after its last post: the site didn't take it this time, said in
                // its name where it has one (`sites::form_refused`).
                PageSays::Form { .. } => {
                    return Err(
                        sites::form_refused(&from).map_or(Error::GotWebPage, Error::Refused)
                    );
                }
                PageSays::Refused(r) => match sites::second_look(&from).filter(|_| !looked) {
                    Some(look) => {
                        looked = true;
                        held = Some((r, from.clone()));
                        (Duration::ZERO, look)
                    }
                    None => return Err(Error::Refused(r)),
                },
                PageSays::Nothing => match meta_refresh(&page, &from) {
                    Some(next) => next,
                    None if site_page => return Err(Error::GotWebPage),
                    // Just a page: asked for again, as its body went into looking.
                    None => {
                        add_cookies(&mut headers, &cookies);
                        url = from;
                        hops = MAX_PAGE_REDIRECTS;
                        continue;
                    }
                },
            }
        } else if site_page {
            return Err(Error::GotWebPage);
        } else {
            return Ok((resp, headers, took));
        };
        if !wait.is_zero() {
            cancellable(cancel, async {
                tokio::time::sleep(wait.min(PAGE_WAIT)).await;
                Ok(())
            })
            .await?;
        }
        headers = scoped_headers(&headers, &from, &next);
        // Its own host's (or its site's, `sites::same_party`), and never from https down to http
        // (as the user's own cookies).
        if (next.host_str() == from.host_str() || sites::same_party(&from, &next))
            && !(from.scheme() == "https" && next.scheme() != "https")
        {
            add_cookies(&mut headers, &cookies);
        }
        url = next;
        hops += 1;
    }
}

/// Most redirects in a row that pages make themselves (`get`).
const MAX_PAGE_REDIRECTS: usize = 5;
/// Most times a site's form is posted in one look (`get`, `PageSays::Form`): a site that answers
/// with the form again now and then (LuluStream's, about a post in three) gives the file within
/// a few; one that never takes it is a web page after these.
const MAX_FORM_POSTS: usize = 5;
/// How much of a web page is read to find its redirect.
const PAGE_PEEK: usize = 256 << 10;
/// The longest wait a page asks for before it leads on that is kept.
const PAGE_WAIT: Duration = Duration::from_secs(10);

/// The first `max` bytes of `resp`'s body (all of it when shorter).
async fn read_up_to(
    mut resp: Response,
    max: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    while body.len() < max {
        let chunk = cancellable(cancel, async { Ok(resp.chunk().await?) }).await?;
        let Some(chunk) = chunk else { break };
        body.extend_from_slice(&chunk);
    }
    body.truncate(max);
    Ok(body)
}

/// The most of a web page [`page_links`] reads.
const PAGE_LINKS_MAX: usize = 8 << 20;
/// The most of a web page [`page_videos`] reads: what it declares is in its head, or near it.
const PAGE_VIDEOS_MAX: usize = 2 << 20;

/// The web page at `url`, asked as a download's look asks (`get`: the user's proxy and agent,
/// the redirects and waiting pages it follows), read up to `max` bytes, with the address it came
/// from. None for an answer that isn't a web page (its body isn't read).
async fn web_page(
    url: Url,
    headers: HeaderMap,
    opts: &Options,
    max: usize,
) -> Result<(Url, Option<String>), Error> {
    let cancel = CancellationToken::new();
    let client = http_client(opts, &url)?;
    let (resp, _, _) = get(
        &client,
        url,
        headers,
        None,
        opts.site_agent(),
        false,
        &cancel,
    )
    .await?;
    let at = resp.url().clone();
    if !is_web_page(resp.headers()) {
        return Ok((at, None));
    }
    let page = read_up_to(resp, max, &cancel).await?;
    Ok((at, Some(String::from_utf8_lossy(&page).into_owned())))
}

/// The links on the web page at `url`, for Link grabber (#705): an address given on its own
/// (pasted, copied, in a list) that is a page lists the page's links, as the browser's
/// "Download all links" does. Read as [`web_page`] reads it, up to `PAGE_LINKS_MAX`, and its
/// links found by [`html::links`] against the address the page came from, which comes back
/// too. An answer that isn't a web page has no links.
pub async fn page_links(
    url: Url,
    headers: HeaderMap,
    opts: &Options,
) -> Result<(Url, Vec<Url>), Error> {
    let (at, page) = web_page(url, headers, opts, PAGE_LINKS_MAX).await?;
    let links = page.map(|p| html::links(&p, &at)).unwrap_or_default();
    Ok((at, links))
}

/// The videos the web page at `url` declares its own (#1076: its JSON-LD, Open Graph and
/// Twitter card, [`html::declared_videos`]), read as [`web_page`] reads it, up to
/// `PAGE_VIDEOS_MAX`, against the address the page came from, which comes back too. An answer
/// that isn't a web page declares none.
pub async fn page_videos(
    url: Url,
    headers: HeaderMap,
    opts: &Options,
) -> Result<(Url, html::DeclaredVideos), Error> {
    let (at, page) = web_page(url, headers, opts, PAGE_VIDEOS_MAX).await?;
    let videos = page
        .map(|p| html::declared_videos(&p, &at))
        .unwrap_or_default();
    Ok((at, videos))
}

/// Where a page leads on to and after how long, from its `Refresh` value or a meta refresh's
/// `content` (`5; url=next.html`, `0;URL='https://…'`, `3, https://…`), relative to `base`.
/// None unless it leads to another http(s) address: a page that only reloads itself is the
/// answer.
fn refresh_target(value: &str, base: &Url) -> Option<(Duration, Url)> {
    let (delay, rest) = value.split_once([';', ','])?;
    let delay = delay
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d >= 0.0)?;
    let rest = rest.trim();
    // `url=` is optional; the address may be quoted.
    let target = match rest.get(..3) {
        Some(u) if u.eq_ignore_ascii_case("url") && rest[3..].trim_start().starts_with('=') => {
            rest[3..].trim_start()[1..].trim()
        }
        _ => rest,
    };
    let target = target
        .strip_prefix(['"', '\''])
        .map_or(target, |t| t.trim_end_matches(['"', '\'']));
    // A meta tag's value is HTML: `&amp;` and the like.
    let target = html::decode(target);
    let next = base.join(target.trim()).ok()?;
    let mut same = next.clone();
    same.set_fragment(base.fragment());
    (matches!(next.scheme(), "http" | "https") && same != *base).then(|| {
        (
            Duration::from_secs_f64(delay.min(PAGE_WAIT.as_secs_f64())),
            next,
        )
    })
}

/// The redirect a page's `<meta http-equiv="refresh" content="…">` makes (`refresh_target`).
fn meta_refresh(page: &[u8], base: &Url) -> Option<(Duration, Url)> {
    let page = String::from_utf8_lossy(page);
    html::tags(&page, "meta").find_map(|(_, tag)| {
        html::attribute(tag, "http-equiv")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("refresh"))
            .then(|| html::attribute(tag, "content"))
            .flatten()
            .and_then(|c| refresh_target(&c, base))
    })
}

/// The `name=value` pairs a response's `Set-Cookie` headers set.
pub(crate) fn set_cookies(h: &HeaderMap) -> Vec<String> {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .map(str::trim)
        .filter(|c| c.contains('='))
        .map(str::to_owned)
        .collect()
}

/// `cookies` added to the request's `Cookie` header, each replacing one of the same name.
pub(crate) fn add_cookies(headers: &mut HeaderMap, cookies: &[String]) {
    if cookies.is_empty() {
        return;
    }
    let name = |c: &str| c.split('=').next().unwrap_or_default().trim().to_owned();
    let new: HashSet<String> = cookies.iter().map(|c| name(c)).collect();
    let mut all: Vec<String> = header_str(headers, header::COOKIE)
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty() && !new.contains(&name(c)))
        .map(str::to_owned)
        .collect();
    all.extend(cookies.iter().cloned());
    if let Ok(v) = HeaderValue::from_str(&all.join("; ")) {
        headers.insert(header::COOKIE, v);
    }
}

/// The first request of a download, retried as a connection is (`retry_wait`) when it fails
/// in a way a later try may not: a server busy at the moment the download starts no longer
/// fails the whole download. With the answer (and the headers to go on with, `get`), how long its
/// last request waited for it: a waiting page's delay isn't the server's time to first byte.
async fn first_get(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    opts: &Options,
    expects: bool,
    shared: &Shared,
    cancel: &CancellationToken,
) -> Result<((Response, HeaderMap), Duration), Error> {
    let ((resp, headers, took), _) = first_try(opts, shared, cancel, || {
        get(
            client,
            url.clone(),
            headers.clone(),
            opts.post.as_ref(),
            opts.site_agent(),
            expects,
            cancel,
        )
    })
    .await?;
    // #1460: an answer no larger than a page where the browser's file is larger than any, told
    // here so the address the browser's download ended at is asked instead (#630).
    if let Some(sent) = answer_size(resp.status(), resp.headers(), resp.content_length()) {
        not_the_file(opts.expected_size, sent)?;
    }
    Ok(((resp, headers), took))
}

/// The whole file's size an answer gives: a 200's length, a 206's total; None when it doesn't
/// say (a chunked answer, a 206 of unknown total).
fn answer_size(status: StatusCode, headers: &HeaderMap, len: Option<u64>) -> Option<u64> {
    match status {
        StatusCode::OK => len,
        StatusCode::PARTIAL_CONTENT => parse_content_range(headers).ok().and_then(|(.., t)| t),
        _ => None,
    }
}

/// `Error::NotTheFile` when the server's answer, `sent` bytes in all, is no larger than a web
/// page (`PAGE_PEEK`) where the browser's own download of the link announced a file of `size`
/// bytes, larger than any page (#1460, `Options::expected_size`): an error, a notice or an empty
/// answer in the file's place, which a type alone doesn't tell (JSON, plain text, none). Sizes
/// that differ less are left alone: a file the server changed, a browser's estimate.
fn not_the_file(size: Option<u64>, sent: u64) -> Result<(), Error> {
    match size {
        Some(size) if size > PAGE_PEEK as u64 && sent <= PAGE_PEEK as u64 => {
            Err(Error::NotTheFile { size, sent })
        }
        _ => Ok(()),
    }
}

/// [`not_the_file`] for an answer that doesn't say its size (chunked): its first bytes, up to
/// one more than a page has, are read (still first in the body returned); an answer that ends
/// before is no larger than a page. Nothing is read when the browser announced no larger file.
async fn not_short(
    body: Body,
    size: Option<u64>,
    cancel: &CancellationToken,
) -> Result<Body, Error> {
    if size.is_none_or(|n| n <= PAGE_PEEK as u64) {
        return Ok(body);
    }
    let (body, first) = peek(body, PAGE_PEEK + 1, cancel).await?;
    not_the_file(size, first.len() as u64)?;
    Ok(body)
}

/// `attempt` retried as `first_get` does; with its result, how long the try that got it took.
async fn first_try<T, F, Fut>(
    opts: &Options,
    shared: &Shared,
    cancel: &CancellationToken,
    mut attempt: F,
) -> Result<(T, Duration), Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let mut failures = 0;
    loop {
        let asked = std::time::Instant::now();
        match cancellable(cancel, attempt()).await {
            Ok(got) => return Ok((got, asked.elapsed())),
            Err(e) if e.transient() && failures < opts.max_retries => {
                failures += 1;
                shared.event(format!("Connection 1: {e}; retrying"));
                shared.conn(0, |c| c.status = ConnStatus::Retrying(e.clone()));
                if let Some(delay) = retry_wait(opts, failures, &e) {
                    cancellable(cancel, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Run connections until the planner is complete. Respawns a connection when work is left
/// and none is running (e.g. the others retired).
async fn supervise(ctx: Arc<Ctx>, first: Option<Body>) -> Result<(), Error> {
    let mut set = JoinSet::new();
    let mut next_id: WorkerId = 0;
    let probing = first.is_some();
    if let Some(body) = first {
        // The probe's body is byte 0 onward: reserve that segment for connection 0 before any
        // other connection can split or take it.
        ctx.shared.with_planner(|p| p.assign(0, None));
        set.spawn(worker(ctx.clone(), 0, Some(body)));
        next_id = 1;
    }
    // No more connections than the host is known to allow.
    let host = ctx.target().url.host_str().unwrap_or("").to_owned();
    let learned = ctx.opts.host_limits.as_ref().and_then(|h| h.cap(&host));
    let wanted = ctx.opts.connections();
    if let Some(cap) = learned.filter(|c| *c < wanted) {
        ctx.shared
            .event(format!("{host} allows {cap} connection(s) (learned)"));
    }
    let max = learned.map_or(wanted, |c| c.min(wanted));
    // A small file starts with fewer connections (one per INITIAL_PIECE): opened all at once
    // they would cut it into slivers and fetch some twice (benchmark 2026-10-30: 256 KiB took
    // 13-19 requests; IDM, which ramps up in waves, took 4). Later splits still go down to
    // `min_split` as connections free up.
    let size = ctx.shared.with_planner(|p| p.size());
    let by_size = u32::try_from(size.div_ceil(INITIAL_PIECE)).unwrap_or(u32::MAX);
    let max = max.min(by_size.max(1)).max(1);
    // A POST the server hasn't shown to take ranges: one connection, until a 206 says the
    // others may follow (a second POST may do the form's work twice). The browser-like
    // connection's (#668): while the first answer streams the whole file, a second connection
    // asks for a part of it, and the others follow its 206 (a site that refuses it leaves the
    // download on the first: `request`).
    let mut held = ctx.wait_for_ranges;
    let scouts = if probing && ctx.browser.is_some() {
        2
    } else {
        1
    };
    while next_id < if held { scouts.min(max) } else { max } {
        set.spawn(worker(ctx.clone(), next_id, None));
        next_id += 1;
    }
    let mut last_err = None;
    loop {
        let joined = tokio::select! {
            joined = set.join_next() => match joined {
                Some(joined) => joined,
                None => break,
            },
            () = ctx.range_answered.notified(), if held => {
                held = false;
                if ctx.browser.is_some() && next_id < max {
                    ctx.shared.event(format!(
                        "{host} takes ranges on the browser-like connection: {max} connections"
                    ));
                }
                while next_id < max {
                    set.spawn(worker(ctx.clone(), next_id, None));
                    next_id += 1;
                }
                continue;
            }
        };
        match joined.expect("worker panicked") {
            Err(e) if e.is_fatal() => {
                stop_connections(&ctx, &mut set).await;
                return Err(e);
            }
            Err(e) => last_err = Some(e),
            Ok(()) => {}
        }
        let complete = ctx.shared.with_planner(|p| p.is_complete());
        // Every byte is in: the connections still asking (a retry under way, an answer late on
        // a busy machine) have nothing left to fetch. Stopped now, a late answer can't start
        // the download over (#136: a server without ranges sent the whole file twice).
        if complete {
            stop_connections(&ctx, &mut set).await;
            break;
        }
        if set.is_empty() {
            if let Some(e) = last_err.take() {
                return Err(e); // the last connection exhausted its retries
            }
            set.spawn(worker(ctx.clone(), next_id, None));
            next_id += 1;
        }
    }
    Ok(())
}

/// Stop the connections still running, and wait until each has. They are told to stop, not
/// aborted: a connection aborted in a blocking read or write of the part file (the check of
/// its overlap, a write) leaves that call running, with the file open, after the download has
/// ended. A finished download then failed with "part file still in use", the rename's handle
/// still taken (#201: a connection taken over in its overlap check, on a busy machine), and a
/// stopped one went on writing after it had returned. Every wait of a connection ends on its
/// token, and a blocking call only takes as long as one read or write.
async fn stop_connections(ctx: &Ctx, set: &mut JoinSet<Result<(), Error>>) {
    ctx.cancel.cancel();
    while set.join_next().await.is_some() {}
}

/// One connection: take an assignment, fetch it, repeat until there is nothing left to do.
async fn worker(ctx: Arc<Ctx>, id: WorkerId, mut first: Option<Body>) -> Result<(), Error> {
    let mut after = None;
    let mut failures = 0u32;
    // Bytes this connection had received at its last failure: progress since resets the budget.
    let mut at_last_failure = 0u64;
    loop {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let Some(a) = ctx.shared.with_planner(|p| p.assign(id, after.take())) else {
            ctx.shared.conn(id, ConnView::finish);
            return Ok(());
        };
        ctx.shared.conn(id, |c| {
            c.status = ConnStatus::Connecting;
            c.speed = 0;
        });
        let target = ctx.target();

        // The server ignores ranges: only the probe's plain GET can bring data. While it
        // still streams it takes this segment over as it gets there; once it has stopped,
        // the download starts over on one connection.
        if first.is_none() && ctx.ranges.load(Ordering::SeqCst) == RANGES_NO {
            ctx.shared.with_planner(|p| p.release(id));
            return if ctx.shared.with_planner(|p| p.holds(0)) {
                ctx.shared.conn(id, ConnView::finish);
                Ok(())
            } else {
                Err(Error::NoRanges)
            };
        }
        let whole = first.is_some();
        // Re-fetch the last few bytes already on disk and compare them before going on.
        let overlap = if first.is_some() {
            0
        } else {
            a.written.min(ctx.opts.verify_overlap)
        };
        let result = match first.take() {
            Some(body) if a.from == 0 => {
                let to = request_end(&ctx, id, 0).unwrap_or(a.to);
                receive_all(&ctx, id, &target, Some(body), 0, to, 0, whole).await
            }
            Some(_) => Err(Error::BadRange(
                "probe segment no longer starts at 0".into(),
            )),
            None => {
                let to = request_end(&ctx, id, a.from).unwrap_or(a.to);
                receive_all(&ctx, id, &target, None, a.from, to, overlap, false).await
            }
        };
        match result {
            Ok(Some(end)) => {
                failures = 0;
                after = Some(end);
            }
            Ok(None) => {
                // Preempted (another connection took the segment over): ask again.
                ctx.shared.with_planner(|p| p.release(id));
            }
            Err(Error::NoRanges) => {
                // The server ignores ranges: connection 0 (the probe) keeps the whole file if it
                // can, every other connection stops. If it can't have it all at once (it was
                // split), it still takes each segment over as it reaches it, while it streams.
                ctx.shared.with_planner(|p| p.release(id));
                return if ctx.shared.with_planner(|p| p.make_single(0) || p.holds(0)) {
                    ctx.shared.conn(id, ConnView::finish);
                    Ok(())
                } else {
                    Err(Error::NoRanges)
                };
            }
            Err(e) => {
                ctx.shared.with_planner(|p| p.release(id));
                if e.is_fatal() {
                    return Err(e);
                }
                // Shed: reconnect at once for the rest of the segment (not a failure).
                if matches!(e, Error::SlowConnection) {
                    ctx.shared.event(format!("Connection {}: {e}", id + 1));
                    continue;
                }
                let (working, receiving) = ctx
                    .shared
                    .with_planner(|p| (p.active_workers(), p.receiving_workers()));
                let others = working > 0;
                // An extra connection refused while others work: the host's connection limit.
                // Once learned (twice seen), this connection retires instead of retrying — if the
                // others fill the cap. Below it the server most likely still counts a connection
                // of ours that just closed (a shed or a split reconnects at once): retry as usual
                // (benchmark 2026-10-29: retiring here left 50 MiB on one connection for 12 s).
                if e.refused()
                    && others
                    && let Some(limits) = &ctx.opts.host_limits
                {
                    let host = ctx.target().url.host_str().unwrap_or("").to_owned();
                    if let Some(cap) = limits.refused(&host, receiving as u32)
                        && working as u32 >= cap
                    {
                        ctx.shared.event(format!(
                            "Connection {}: {host} allows {cap} connection(s) (learned)",
                            id + 1
                        ));
                        ctx.shared
                            .conn(id, |c| c.status = ConnStatus::Retired { host, cap });
                        return Ok(());
                    }
                }
                // A dead or forbidden link is not retried. While other connections work it
                // means this extra one is refused (a per-client connection limit): retire it.
                if e.link_dead() {
                    // A redirected address that expired (signed CDN links): go where the
                    // original address leads now, if that is still this file.
                    if !matches!(e, Error::LinkExpired)
                        && ctx.refresh_target(target.generation).await
                    {
                        ctx.shared.conn(id, |c| c.status = ConnStatus::Connecting);
                        continue;
                    }
                    // A site's signed link (`sites::expiring`) is expired by its dead answer too,
                    // when no page is known to ask for a fresh one: a fresh address helps.
                    let e = match e {
                        Error::Status(_)
                            if ctx.origin.is_some() || sites::expiring(&target.url) =>
                        {
                            Error::LinkExpired
                        }
                        e => e,
                    };
                    ctx.shared
                        .conn(id, |c| c.status = ConnStatus::Failed(e.clone()));
                    return if others { Ok(()) } else { Err(e) };
                }
                let mut received = 0;
                ctx.shared.conn(id, |c| received = c.downloaded);
                if received > at_last_failure {
                    failures = 0;
                }
                at_last_failure = received;
                failures += 1;
                if failures > ctx.opts.max_retries {
                    let others = ctx.shared.with_planner(|p| p.active_workers()) > 0;
                    ctx.shared
                        .conn(id, |c| c.status = ConnStatus::Failed(e.clone()));
                    return if others { Ok(()) } else { Err(e) };
                }
                ctx.shared
                    .event(format!("Connection {}: {e}; retrying", id + 1));
                ctx.shared
                    .conn(id, |c| c.status = ConnStatus::Retrying(e.clone()));
                if let Some(delay) = retry_wait(&ctx.opts, failures, &e) {
                    cancellable(&ctx.cancel, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
            }
        }
    }
}

/// `Range: bytes=<from>-<from + len - 1>`.
fn range_value(from: u64, len: u64) -> HeaderValue {
    HeaderValue::from_str(&format!("bytes={from}-{}", from + len - 1)).expect("ascii")
}

/// Where connection `id`'s next request starting at `from` ends: its segment's end, or sooner
/// under `request_cap` (HTTP). None: the segment is no longer its own.
fn request_end(ctx: &Ctx, id: WorkerId, from: u64) -> Option<u64> {
    let end = from + ctx.shared.with_planner(|p| p.room(id, from))?;
    Some(match ctx.opts.cap().filter(|_| ctx.ftp.is_none()) {
        Some(cap) => end.min(from.saturating_add(cap)),
        None => end,
    })
}

/// Connection `id`'s part from `from`: `body` (the first answer) or a request for `[from, to)`;
/// then, under `request_cap`, one request after another on the same connection until its
/// segment is complete. As [`receive`]: the segment's end, or None when it was taken over.
#[allow(clippy::too_many_arguments)]
async fn receive_all(
    ctx: &Ctx,
    id: WorkerId,
    target: &Target,
    body: Option<Body>,
    from: u64,
    to: u64,
    overlap: u64,
    whole: bool,
) -> Result<Option<u64>, Error> {
    let (mut from, mut to, mut overlap, mut whole) = (from, to, overlap, whole);
    let mut body = body;
    loop {
        let b = match body.take() {
            Some(b) => b,
            None => {
                // The bytes re-read to check the resume (`overlap`) count in the cap too.
                if let Some(cap) = ctx.opts.cap().filter(|c| overlap < *c && ctx.ftp.is_none()) {
                    to = to.min(from - overlap + cap);
                }
                request(ctx, target, from - overlap, to).await?
            }
        };
        match receive(ctx, id, b, from, to, overlap, whole).await? {
            Received::Complete(end) => return Ok(Some(end)),
            Received::Lost => return Ok(None),
            // The capped request came whole, its segment goes on: the next piece of it.
            Received::More(pos) => {
                let Some(end) = request_end(ctx, id, pos) else {
                    return Ok(None);
                };
                (from, to, overlap, whole) = (pos, end, 0, false);
            }
        }
    }
}

/// How one answer's body ended in [`receive`].
enum Received {
    /// The segment is complete, at this end.
    Complete(u64),
    /// Another connection took the segment over.
    Lost,
    /// All of a capped request (`request_cap`) came, up to here; the segment goes on.
    More(u64),
}

/// `GET` bytes `[from, to)`; checks the server returned exactly that range of the same file.
/// FTP: `REST from` and `RETR`.
async fn request(ctx: &Ctx, target: &Target, from: u64, to: u64) -> Result<Body, Error> {
    if let Some(site) = &ctx.ftp {
        return match cancellable(&ctx.cancel, site.open_at(from, Some(ctx.size))).await {
            Ok(t) => {
                if from > 0 {
                    ctx.learn_ranges(true);
                }
                Ok(Body::Ftp(t))
            }
            // REST refused ("not implemented", "syntax error") where it wasn't known to work.
            Err(Error::FtpReply {
                code: 500 | 501 | 502 | 504,
                ..
            }) if from > 0 && ctx.ranges.load(Ordering::SeqCst) != RANGES_YES => {
                ctx.learn_ranges(false);
                Err(Error::NoRanges)
            }
            Err(e) => Err(e),
        };
    }
    let if_range = ctx
        .validators
        .lock()
        .unwrap()
        .if_range(target.url.host_str())
        .and_then(|v| HeaderValue::from_str(v).ok());
    let range = format!("bytes={from}-{}", to - 1);
    let (head, body) = match &ctx.browser {
        // The browser-like connection's own (#668): its redirects followed as `get` does.
        Some(b) => {
            let mut headers = target.headers.clone();
            headers.insert(header::RANGE, HeaderValue::from_str(&range).expect("ascii"));
            if let Some(v) = &if_range {
                headers.insert(header::IF_RANGE, v.clone());
            }
            let (head, body, _) = b
                .get(target.url.clone(), headers, true, &ctx.cancel)
                .await?;
            (head, body)
        }
        None => {
            let mut req = build(
                &ctx.client,
                target.url.clone(),
                target.post.as_ref(),
                &target.headers,
                ctx.opts.site_agent(),
            )
            .header(header::RANGE, range);
            if let Some(v) = &if_range {
                req = req.header(header::IF_RANGE, v);
            }
            let resp = cancellable(&ctx.cancel, async { Ok(req.send().await?) }).await?;
            (Head::of(&resp), Body::Http(resp))
        }
    };
    // A resumed download asks nothing before its connections: the first one's answer is the
    // first it gets (#954).
    if let Some(seen) = ctx.shared.seen.get() {
        seen.connection_answered(&head);
    }
    // #668: the browser-like connection got the file, but the first part asked of it is refused
    // (403, the bot check's among them, or 412) or comes whole (its Range ignored, below): the
    // download goes on with the connection it has, as a browser downloads, the others unopened.
    if ctx.browser.is_some()
        && ctx.ranges.load(Ordering::SeqCst) == RANGES_UNKNOWN
        && matches!(head.status.as_u16(), 403 | 412)
    {
        browser_ranges_refused(ctx, &head, &format!("refused ({})", head.status));
        return Err(Error::NoRanges);
    }
    if head.status.is_success() && !ctx.web_page_ok && is_web_page(&head.headers) {
        return Err(Error::GotWebPage);
    }
    match head.status {
        StatusCode::PARTIAL_CONTENT => {
            ctx.learn_ranges(true);
            ctx.range_answered.notify_one();
        }
        // The same file under another ETag (each CDN node has its own): go on with the date.
        StatusCode::OK if per_node_etag(ctx, if_range.as_ref(), &head) => {
            return Err(Error::RangesUnavailable);
        }
        // Another server's copy, after the address moved there (a file host's fresh link):
        // its own ETag and date, the same length. Its validators are taken; the overlap check
        // still compares the bytes on disk as each connection goes on.
        StatusCode::OK if other_server_copy(ctx, &head) => {
            return Err(Error::RangesUnavailable);
        }
        // A full body for a ranged request once ranges are known to work: If-Range failed (the
        // file changed), unless it is still the same file and this answer just lacks the range.
        StatusCode::OK if ctx.ranges.load(Ordering::SeqCst) == RANGES_YES => {
            if same_file(ctx, &head) {
                return Err(Error::RangesUnavailable);
            }
            // Not the file any more: the site's page where it was, whatever its type says
            // (#701), else another file.
            if ctx.never_page && !ctx.web_page_ok {
                not_a_page(body, &ctx.cancel).await?;
            }
            return Err(Error::FileChanged);
        }
        StatusCode::OK if ctx.browser.is_some() => {
            browser_ranges_refused(ctx, &head, "answered with the whole file");
            return Err(Error::NoRanges);
        }
        StatusCode::OK => {
            ctx.learn_ranges(false);
            return Err(Error::NoRanges);
        }
        s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::SERVICE_UNAVAILABLE => {
            return Err(Error::Throttled(s, retry_after(&head.headers)));
        }
        s => return Err(Error::Status(s)),
    }
    let (start, _, total) = parse_content_range(&head.headers)?;
    if start != from || total.is_some_and(|t| t != ctx.size) {
        return Err(Error::BadRange(format!(
            "asked {from}-{}, got {start}..",
            to - 1
        )));
    }
    ctx.answered
        .fetch_max(target.generation + 1, Ordering::SeqCst);
    Ok(body)
}

/// The site takes no ranges on the browser-like connection (#668, `request`): learned once, and
/// told why in the event log.
fn browser_ranges_refused(ctx: &Ctx, head: &Head, why: &str) {
    if ctx.learn_ranges(false) {
        ctx.shared.event(format!(
            "{}: a part of the file asked on the browser-like connection was {why}; going on with one connection",
            head.url.host_str().unwrap_or("?")
        ));
    }
}

/// A full answer that still describes the file being downloaded: the same length and, when the
/// download has validators, the same one it would send.
fn same_file(ctx: &Ctx, resp: &Head) -> bool {
    let host = resp.url.host_str();
    let v = ctx.validators.lock().unwrap();
    let h = &resp.headers;
    let theirs = match v.if_range(host) {
        None => return resp.len == Some(ctx.size),
        Some(ours) if Some(ours) == v.etag.as_deref() => header_str(h, header::ETAG),
        Some(_) => header_str(h, header::LAST_MODIFIED),
    };
    resp.len == Some(ctx.size) && theirs.is_some() && theirs == v.if_range(host)
}

/// A full answer to an ETag `If-Range` that still matches by length and Last-Modified: the
/// ETag differs per node (CDNs, mirrors), not the file. The ETag is dropped for the rest of the
/// download, so the next request validates with the date (and the overlap check).
fn per_node_etag(ctx: &Ctx, sent: Option<&HeaderValue>, resp: &Head) -> bool {
    let mut v = ctx.validators.lock().unwrap();
    // Sent with an ETag (dates never start with a quote) — possibly one another connection has
    // just dropped, so don't compare it with the current one.
    let sent_etag = sent.is_some_and(|s| s.as_bytes().starts_with(b"\""));
    // Another node's ETag differs from the one sent. The same ETag is that too only from a
    // server that advertised ranges: one that didn't, answering in full, has no ranges (#136),
    // and is told so at once rather than after a round of retries.
    let theirs = header_str(&resp.headers, header::ETAG);
    let other_node = ctx.advertised
        || (theirs.is_some() && theirs.map(str::as_bytes) != sent.map(HeaderValue::as_bytes));
    let same = sent_etag
        && other_node
        && resp.len == Some(ctx.size)
        && v.last_modified.is_some()
        && header_str(&resp.headers, header::LAST_MODIFIED) == v.last_modified.as_deref();
    if same {
        v.etag = None;
        v.etag_host = None;
    }
    same
}

/// A full answer from a server other than the one the download's validators came from (its
/// address moved: Rapidgator's fresh link may be on another of its servers, whose copy of the
/// file has its own ETag and date), with the file's length and validators of its own: that
/// server's are taken for the rest of the download, so its ranges are asked with them.
fn other_server_copy(ctx: &Ctx, resp: &Head) -> bool {
    let mut v = ctx.validators.lock().unwrap();
    let host = resp.url.host_str();
    let theirs = Validators::from_response(
        header_str(&resp.headers, header::ETAG),
        header_str(&resp.headers, header::LAST_MODIFIED),
        header_str(&resp.headers, header::DATE),
        host,
    );
    let same = is_other_server_copy(&v, &theirs, &resp.url, resp.len == Some(ctx.size));
    if same {
        ctx.shared.event(format!(
            "{} has its own copy of the file (same size): going on with its version",
            host.unwrap_or("?")
        ));
        *v = theirs;
    }
    same
}

/// The decision of `other_server_copy`: `theirs` (the answer's validators, from `at`) is another
/// server's copy of the file when the download's validators came from another host, the length
/// is the file's, the answer has an ETag of its own, and both hosts are one file host's own
/// (`sites::expiring` for the answer, `sites::same_party` between the two). Any other site's
/// same-length answer under other validators is a file that changed on the server.
fn is_other_server_copy(ours: &Validators, theirs: &Validators, at: &Url, same_len: bool) -> bool {
    let host = at.host_str();
    let Some(old) = ours.etag_host.as_deref().filter(|old| Some(*old) != host) else {
        return false;
    };
    let mut was = at.clone();
    let party =
        sites::expiring(at) && was.set_host(Some(old)).is_ok() && sites::same_party(&was, at);
    party && same_len && theirs.etag.is_some() && theirs != ours
}

fn header_str(h: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// A site's bot check turning the request away: Cloudflare's, which marks it
/// (`cf-mitigated: challenge`, #376), or Akamai's, whose edge answers a connection it takes for a
/// program's with its own 403 page ("Access Denied", served by `AkamaiGHost`, #771), where a
/// browser gets the file. The site's own 403 behind Akamai comes from its servers, not the edge's
/// page, and stays a plain refusal.
pub(crate) fn is_bot_check(status: StatusCode, h: &HeaderMap) -> bool {
    status == StatusCode::FORBIDDEN
        && (h
            .get("cf-mitigated")
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"challenge"))
            || (header_str(h, header::SERVER)
                .is_some_and(|s| s.trim().eq_ignore_ascii_case("AkamaiGHost"))
                && is_web_page(h)))
}

#[cfg(test)]
mod bot_check_tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, header::HeaderValue::from_static(v));
        }
        h
    }

    /// Cloudflare's challenge and Akamai's edge refusal (#771) are bot checks; a 403 from the site
    /// itself, or Akamai's edge sending something other than its page, is not.
    #[test]
    fn bot_checks_are_cloudflares_challenge_and_akamais_edge_page() {
        let forbidden = StatusCode::FORBIDDEN;
        assert!(is_bot_check(
            forbidden,
            &headers(&[("cf-mitigated", "challenge")])
        ));
        let akamai = headers(&[("server", "AkamaiGHost"), ("content-type", "text/html")]);
        assert!(is_bot_check(forbidden, &akamai));
        assert!(!is_bot_check(StatusCode::NOT_FOUND, &akamai));
        assert!(!is_bot_check(
            forbidden,
            &headers(&[
                ("server", "AkamaiGHost"),
                ("content-type", "application/pdf")
            ])
        ));
        assert!(!is_bot_check(
            forbidden,
            &headers(&[("server", "nginx"), ("content-type", "text/html")])
        ));
        assert!(!is_bot_check(forbidden, &HeaderMap::new()));
    }
}

pub(crate) fn is_web_page(h: &HeaderMap) -> bool {
    h.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(is_web_page_type)
}

fn is_web_page_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
}

/// The server names what it sends (`Content-Disposition`): its own file, page or not.
pub(crate) fn named(h: &HeaderMap) -> bool {
    h.contains_key(header::CONTENT_DISPOSITION)
}

/// Where a download is saved under a name of its own (the browser's, the user's), that name.
fn dest_name(dest: &Destination) -> Option<&str> {
    match dest {
        Destination::File(p) | Destination::Free(p) => p.file_name().and_then(|n| n.to_str()),
        Destination::Dir(_) => None,
    }
}

/// Types of file a web page never is, by extension: documents, archives, programs, disk images,
/// audio, video and pictures. A name without one of these (a page's `.php`, no extension at all)
/// says nothing.
const FILE_EXTENSIONS: &[&str] = &[
    "doc",
    "docx",
    "docm",
    "dot",
    "dotx",
    "xls",
    "xlsx",
    "xlsm",
    "xlsb",
    "ppt",
    "pptx",
    "pps",
    "ppsx",
    "odt",
    "ods",
    "odp",
    "rtf",
    "pdf",
    "epub",
    "mobi",
    "djvu",
    "zip",
    "rar",
    "7z",
    "gz",
    "tgz",
    "bz2",
    "xz",
    "zst",
    "tar",
    "cab",
    "arj",
    "lz",
    "lzh",
    "exe",
    "msi",
    "msix",
    "msixbundle",
    "appx",
    "apk",
    "deb",
    "rpm",
    "appimage",
    "dmg",
    "pkg",
    "jar",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "mp3",
    "m4a",
    "aac",
    "flac",
    "wav",
    "wma",
    "ogg",
    "opus",
    "mp4",
    "m4v",
    "mkv",
    "avi",
    "mov",
    "wmv",
    "flv",
    "webm",
    "mpg",
    "mpeg",
    "jpg",
    "jpeg",
    "png",
    "gif",
    "bmp",
    "webp",
    "tif",
    "tiff",
    "psd",
    "torrent",
];

/// Of [`FILE_EXTENSIONS`], the ones programs also save web pages under (a report "exported to
/// Excel" is an HTML table named `.xls`; Word opens a page named `.doc`): their bytes may be a
/// page's and still be the file.
const PAGE_LIKE_EXTENSIONS: &[&str] = &["doc", "dot", "xls", "ppt", "pps"];

/// A web page's own names.
const PAGE_EXTENSIONS: &[&str] = &["htm", "html", "xhtml", "shtml"];

fn extension_in(name: &str, list: &[&str]) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The download expects a kind of file a web page isn't (#533): the browser's own download had
/// another type (`expected_type`), or its name (`name`, else the address's) is a file's, or the
/// browser announced a size no waiting page has (`expected_size`, #701). A web page in its
/// place, without a name of the server's, is an error (`Error::GotWebPage`). A page the browser
/// saved as one (`text/html`), a `.html` link, or a server's own type for the file
/// (`application/octet-stream`) are downloads like any other.
fn expects_other_file(
    expected_type: Option<&str>,
    expected_size: Option<u64>,
    name: Option<&str>,
    url: &Url,
) -> bool {
    if let Some(t) = expected_type.map(str::trim).filter(|t| !t.is_empty()) {
        return !is_web_page_type(t);
    }
    let name = name.map_or_else(|| naming::file_name(None, url), str::to_owned);
    extension_in(&name, FILE_EXTENSIONS)
        || (expected_size.is_some_and(|n| n > PAGE_PEEK as u64)
            && !extension_in(&name, PAGE_EXTENSIONS))
}

/// Types the browser gives files whose bytes never start as a web page's (besides every
/// picture but SVG, audio and video, and the Office and OpenDocument formats, which are zips).
const FILE_TYPES: &[&str] = &[
    "application/zip",
    "application/x-zip-compressed",
    "application/x-rar-compressed",
    "application/x-rar",
    "application/vnd.rar",
    "application/x-7z-compressed",
    "application/gzip",
    "application/x-gzip",
    "application/x-tar",
    "application/x-bzip2",
    "application/x-xz",
    "application/zstd",
    "application/x-zstd",
    "application/pdf",
    "application/epub+zip",
    "application/x-msdownload",
    "application/x-msdos-program",
    "application/x-dosexec",
    "application/vnd.microsoft.portable-executable",
    "application/x-msi",
    "application/x-ms-installer",
    "application/vnd.android.package-archive",
    "application/x-iso9660-image",
    "application/x-cd-image",
    "application/x-apple-diskimage",
    "application/vnd.debian.binary-package",
    "application/x-debian-package",
    "application/x-rpm",
    "application/x-redhat-package-manager",
    "application/java-archive",
    "application/x-bittorrent",
];

/// Whether the download is a kind of file whose bytes never start as a web page's (#701): an
/// archive, a program, a disk image, a PDF, a picture, audio or video, by its name (`name`) or,
/// when that names no such kind, the browser's own type for it (`expected_type`). Its first
/// bytes reading as HTML are then the site's page in its place (a countdown, a captcha), whatever
/// type the server sent them as. Never a download the browser had as a page, nor the old Office
/// formats ([`PAGE_LIKE_EXTENSIONS`]).
fn never_a_page(expected_type: Option<&str>, name: &str) -> bool {
    let expected = expected_type.map(str::trim).filter(|t| !t.is_empty());
    if expected.is_some_and(is_web_page_type) {
        return false;
    }
    if extension_in(name, FILE_EXTENSIONS) {
        return !extension_in(name, PAGE_LIKE_EXTENSIONS);
    }
    !extension_in(name, PAGE_EXTENSIONS)
        && expected.is_some_and(|t| {
            let mime = t
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            match mime.split_once('/') {
                Some(("image", sub)) => sub != "svg+xml",
                Some(("audio" | "video", _)) => true,
                _ => {
                    FILE_TYPES.contains(&mime.as_str())
                        || mime.starts_with("application/vnd.openxmlformats-officedocument.")
                        || mime.starts_with("application/vnd.oasis.opendocument.")
                }
            }
        })
}

/// How many first bytes [`not_a_page`] reads: enough for a page's blank lines before its tag.
const PAGE_SNIFF_BYTES: usize = 1024;

/// The body, its first bytes (up to `want`, fewer at its end) read already and still first.
async fn peek(
    mut body: Body,
    want: usize,
    cancel: &CancellationToken,
) -> Result<(Body, Bytes), Error> {
    let mut seen: Vec<u8> = Vec::new();
    while seen.len() < want {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            n = body.chunk() => n?,
        };
        match next {
            Some(c) => seen.extend_from_slice(&c),
            None => break,
        }
    }
    let seen = Bytes::from(seen);
    Ok((Body::Prefixed(Some(seen.clone()), Box::new(body)), seen))
}

/// `Error::GotWebPage` when the body's first bytes are a web page's (#701), else the body, those
/// bytes still first. For a file [`never_a_page`] says isn't one, from its start.
async fn not_a_page(body: Body, cancel: &CancellationToken) -> Result<Body, Error> {
    let (body, first) = peek(body, PAGE_SNIFF_BYTES, cancel).await?;
    if sniffs_as_html(&first) {
        return Err(Error::GotWebPage);
    }
    Ok(body)
}

/// Bytes that start as an HTML document (after a byte order mark and white space).
fn sniffs_as_html(body: &[u8]) -> bool {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    let start = body
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(body.len());
    let head: Vec<u8> = body[start..]
        .iter()
        .take(16)
        .map(u8::to_ascii_lowercase)
        .collect();
    [
        "<!doctype html",
        "<html",
        "<head",
        "<body",
        "<title",
        "<meta",
        "<script",
        "<!--",
    ]
    .iter()
    .any(|t| head.starts_with(t.as_bytes()))
}

/// Whether the download is itself a web page (it was served as one, or is named like one).
fn expects_web_page(dest: &Path, content_type: Option<&str>) -> bool {
    content_type.is_some_and(is_web_page_type)
        || dest
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| extension_in(n, PAGE_EXTENSIONS))
}

#[cfg(test)]
mod web_page_tests {
    use super::*;

    fn url(u: &str) -> Url {
        u.parse().unwrap()
    }

    /// #533, #701: what says a file is expected where a web page came.
    #[test]
    fn a_file_is_expected_by_type_name_or_announced_size() {
        let link = url("https://drop.example/dl/AbC123");
        // The browser's own type for it, whichever its name.
        assert!(expects_other_file(
            Some("application/zip"),
            None,
            None,
            &link
        ));
        assert!(!expects_other_file(
            Some("text/html; charset=utf-8"),
            Some(300 << 20),
            Some("KtK.zip"),
            &link
        ));
        // Its name (the browser's, else the address's).
        assert!(expects_other_file(None, None, Some("KtK.zip"), &link));
        assert!(expects_other_file(
            None,
            None,
            None,
            &url("https://host.example/cedbo/hwinfo_8.60.zip")
        ));
        assert!(!expects_other_file(None, None, None, &link));
        // A size the browser announced, larger than any waiting page; never for a page's name.
        assert!(expects_other_file(None, Some(320 << 20), None, &link));
        assert!(!expects_other_file(None, Some(40 << 10), None, &link));
        assert!(!expects_other_file(
            None,
            Some(320 << 20),
            Some("big.html"),
            &link
        ));
    }

    /// #701: the kinds of file whose bytes never start as a page's, by name, else by type.
    #[test]
    fn archives_programs_and_media_are_never_pages() {
        for name in [
            "KtK.zip",
            "a.RAR",
            "x.7z",
            "setup.exe",
            "disk.iso",
            "v.mp4",
            "b.pdf",
        ] {
            assert!(never_a_page(None, name), "{name}");
        }
        // Programs write pages as old Office files; a page is a page; a name saying nothing.
        for name in [
            "report.xls",
            "letter.doc",
            "deck.ppt",
            "page.html",
            "AbC123",
            "x.bin",
        ] {
            assert!(!never_a_page(None, name), "{name}");
        }
        // The browser's type when the name says nothing.
        for t in [
            "application/zip",
            "application/x-rar-compressed",
            "video/mp4",
            "image/png",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ] {
            assert!(never_a_page(Some(t), "AbC123"), "{t}");
        }
        for t in [
            "application/octet-stream",
            "image/svg+xml",
            "text/plain",
            "",
        ] {
            assert!(!never_a_page(Some(t), "AbC123"), "{t}");
        }
        // The browser had it as a page: a page it is.
        assert!(!never_a_page(Some("text/html"), "KtK.zip"));
        // An old Office name, even with a file's type, may hold a page.
        assert!(!never_a_page(
            Some("application/vnd.ms-excel"),
            "report.xls"
        ));
    }

    #[test]
    fn a_pages_first_bytes_are_told_from_a_files() {
        for page in [
            &b"<!DOCTYPE html><html>"[..],
            b"\r\n\r\n   <html lang=ru>",
            b"\xEF\xBB\xBF<!-- generated --><!doctype html>",
            b"<HEAD><meta charset=utf-8>",
            b"<script>location='x'</script>",
        ] {
            assert!(sniffs_as_html(page), "{page:?}");
        }
        for file in [
            &b"PK\x03\x04\x14\0"[..],
            b"Rar!\x1a\x07\x01\0",
            b"MZ\x90\0",
            b"%PDF-1.7",
            b"",
            b"   ",
            b"<?xml version=\"1.0\"?><svg",
        ] {
            assert!(!sniffs_as_html(file), "{file:?}");
        }
    }

    /// A body as a server's answer sends it, in `chunks`.
    fn answer(chunks: &[&'static [u8]]) -> Body {
        let stream = futures::stream::iter(
            chunks
                .iter()
                .map(|c| Ok::<_, std::io::Error>(Bytes::from_static(c)))
                .collect::<Vec<_>>(),
        );
        let resp = http::Response::new(reqwest::Body::wrap_stream(stream));
        Body::Http(Response::from(resp))
    }

    async fn all(mut body: Body) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(c) = body.chunk().await.unwrap() {
            out.extend_from_slice(&c);
        }
        out
    }

    /// A body whose first bytes are a page's is refused, even after blank lines in chunks of
    /// their own; a file's comes whole, its first bytes still first.
    #[tokio::test]
    async fn not_a_page_keeps_a_files_bytes() {
        let cancel = CancellationToken::new();
        let page = answer(&[
            b"\r\n",
            b"\r\n  ",
            b"<!doctype html><title>Wait 30 s</title>",
        ]);
        assert!(matches!(
            not_a_page(page, &cancel).await,
            Err(Error::GotWebPage)
        ));
        let mut file: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        file[..4].copy_from_slice(b"PK\x03\x04");
        let leaked: &'static [u8] = Box::leak(file.clone().into_boxed_slice());
        let body = answer(&[&leaked[..4], &leaked[4..700], &leaked[700..]]);
        let kept = not_a_page(body, &cancel).await.unwrap();
        assert_eq!(all(kept).await, file);
        // A file shorter than what is looked at.
        let short = not_a_page(answer(&[b"Rar!"]), &cancel).await.unwrap();
        assert_eq!(all(short).await, b"Rar!");
    }

    /// #1460: an answer no larger than a page where the browser's file is larger than any is
    /// not the file; sizes that differ less, or no size from the browser, are left alone.
    #[test]
    fn an_answer_far_smaller_than_the_browsers_file_is_not_it() {
        let page = PAGE_PEEK as u64;
        // The report's: 117.65 MB announced, 4.65 KB sent.
        let report = 123_365_000;
        assert!(matches!(
            not_the_file(Some(report), 4_762),
            Err(Error::NotTheFile {
                size: 123_365_000,
                sent: 4_762
            })
        ));
        assert!(not_the_file(Some(report), 0).is_err(), "an empty answer");
        assert!(not_the_file(Some(page + 1), page).is_err());
        // The file, a file a little different (changed, re-encoded), or a larger one.
        assert!(not_the_file(Some(report), report).is_ok());
        assert!(not_the_file(Some(report), report - (5 << 20)).is_ok());
        assert!(not_the_file(Some(page + 1), page + 1).is_ok());
        // A browser's file no larger than a page: a small file is a small file.
        assert!(not_the_file(Some(page), 10).is_ok());
        assert!(not_the_file(Some(40 << 10), 4_762).is_ok());
        // Nothing announced (a link added by hand, Link grabber).
        assert!(not_the_file(None, 0).is_ok());
    }

    /// #1460: the size an answer gives, by its status.
    #[test]
    fn an_answers_size_by_its_status() {
        let mut h = HeaderMap::new();
        assert_eq!(answer_size(StatusCode::OK, &h, Some(4_762)), Some(4_762));
        assert_eq!(answer_size(StatusCode::OK, &h, None), None);
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-1023/123365000"),
        );
        assert_eq!(
            answer_size(StatusCode::PARTIAL_CONTENT, &h, Some(1024)),
            Some(123_365_000)
        );
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-1023/*"),
        );
        assert_eq!(
            answer_size(StatusCode::PARTIAL_CONTENT, &h, Some(1024)),
            None
        );
        assert_eq!(answer_size(StatusCode::NO_CONTENT, &h, Some(0)), None);
    }

    /// #1460: a chunked answer that ends before a page's size, where the browser's file is
    /// larger, is refused; a longer one comes whole, its first bytes still first; nothing is read
    /// without a larger file announced.
    #[tokio::test]
    async fn a_chunked_answer_is_read_as_far_as_a_page() {
        let cancel = CancellationToken::new();
        let big = Some(123_365_000);
        let json: &'static [u8] = b"{\"zt\":0,\"info\":\"link expired\"}";
        assert!(matches!(
            not_short(answer(&[&json[..5], &json[5..]]), big, &cancel).await,
            Err(Error::NotTheFile { sent: 30, .. })
        ));
        assert!(matches!(
            not_short(answer(&[]), big, &cancel).await,
            Err(Error::NotTheFile { sent: 0, .. })
        ));
        let file: Vec<u8> = (0..(PAGE_PEEK as u32 + 5000))
            .map(|i| (i % 251) as u8)
            .collect();
        let leaked: &'static [u8] = Box::leak(file.clone().into_boxed_slice());
        let body = answer(&[&leaked[..100], &leaked[100..70_000], &leaked[70_000..]]);
        let kept = not_short(body, big, &cancel).await.unwrap();
        assert_eq!(all(kept).await, file);
        let small = not_short(answer(&[json]), Some(40 << 10), &cancel)
            .await
            .unwrap();
        assert_eq!(all(small).await, json);
        let none = not_short(answer(&[json]), None, &cancel).await.unwrap();
        assert_eq!(all(none).await, json);
    }
}

/// Stream a response into the file at `from`. `Some(end)` when the segment is complete,
/// `None` when the segment was taken over by another connection.
///
/// Every chunk is claimed from the planner as soon as it arrives (not when the buffer is
/// written), so a split can never fall inside data this connection has already received.
///
/// Writing runs beside receiving (`Writer`): while the disk takes one write, what arrives waits
/// in memory, up to the connection's share of `HOLD_MAX`, and only then does the connection
/// stand still for the disk (#361, #782).
async fn receive(
    ctx: &Ctx,
    id: WorkerId,
    mut body: Body,
    from: u64,
    to: u64,
    mut verify: u64,
    whole: bool,
) -> Result<Received, Error> {
    ctx.shared.conn(id, |c| c.status = ConnStatus::Receiving);
    let mut out = Writer::new(ctx, id, from);
    let mut pace = Pace::new();
    let mut checked = std::time::Instant::now();
    loop {
        let next = tokio::select! {
            _ = ctx.cancel.cancelled() => None,
            n = body.chunk() => Some(n),
        };
        let Some(next) = next else {
            // Paused or stopped: the connection closes now, whatever the disk still has to do
            // (#1328), and what arrived is kept (it is claimed, so it is ours to write).
            drop(body);
            out.finish(&mut pace).await?;
            return Err(Error::Cancelled);
        };
        // `complete`: the segment ended where this connection had written (a split shortened it).
        let (ended, err, complete) = match next {
            Ok(Some(mut chunk)) => {
                // The response starts `verify` bytes before `from`: they must equal the disk.
                if verify > 0 {
                    let k = verify.min(chunk.len() as u64);
                    let (file, at) = (ctx.file.clone(), from - verify);
                    let disk = tokio::task::spawn_blocking(move || read_at(&file, at, k as usize))
                        .await
                        .expect("reader panicked")?;
                    if disk[..] != chunk[..k as usize] {
                        out.finish(&mut pace).await?;
                        return Err(Error::FileChanged);
                    }
                    verify -= k;
                    chunk = chunk.slice(k as usize..);
                    if chunk.is_empty() {
                        continue;
                    }
                }
                let want = chunk.len() as u64;
                // Pay first: the wait is when other connections may split this segment, and
                // the claim below then grants only what still belongs to it. Stop ends the
                // wait (under a low limit it is seconds long; Stop used to wait it out).
                let paid = async {
                    ctx.shared.limiter.acquire(want).await;
                    if let Some(g) = &ctx.opts.global {
                        g.acquire(want).await;
                    }
                };
                let stopped = tokio::select! {
                    _ = ctx.cancel.cancelled() => true,
                    _ = paid => false,
                };
                if stopped {
                    // Not claimed yet: this chunk isn't ours to keep; what is, is written, the
                    // connection closed first.
                    drop(body);
                    out.finish(&mut pace).await?;
                    return Err(Error::Cancelled);
                }
                let at = out.next();
                // An answer that runs to the end of the file (`whole`: the probe's plain GET),
                // while ranges aren't known to work: at its segment's end it takes over the
                // split-off segments nothing has arrived for yet, rather than dropping bytes it
                // is already receiving for a range request the server may ignore.
                let keep_reading = whole && ctx.ranges.load(Ordering::SeqCst) != RANGES_YES;
                let claimed = ctx.shared.with_planner(|p| {
                    let Claim::Granted(mut n) = p.claim(id, at, want) else {
                        return None;
                    };
                    while keep_reading && n < want && p.absorb_next(id) {
                        match p.claim(id, at + n, want - n) {
                            Claim::Granted(m) => n += m,
                            Claim::Lost => break,
                        }
                    }
                    // Ending exactly at the boundary: take the next segment over too.
                    let full =
                        p.room(id, at + n) == Some(0) && !(keep_reading && p.absorb_next(id));
                    Some((n, full))
                });
                // Claimed bytes can't be taken over, so a lost segment has nothing waiting for
                // the disk (a write still running is waited for all the same, #201).
                let refund = |n: u64| {
                    ctx.shared.limiter.refund(n);
                    if let Some(g) = &ctx.opts.global {
                        g.refund(n);
                    }
                };
                let Some((n, full)) = claimed else {
                    refund(want);
                    out.finish(&mut pace).await?;
                    return Ok(Received::Lost);
                };
                if n < want {
                    refund(want - n);
                }
                if pace.received == 0
                    && n > 0
                    && let Some(limits) = &ctx.opts.host_limits
                {
                    // This connection's first data: tell the host's learned cap how many are
                    // receiving now.
                    let now = ctx.shared.with_planner(|p| p.receiving_workers()) as u32;
                    limits.working(ctx.target().url.host_str().unwrap_or(""), now);
                }
                out.push(&chunk[..n as usize]);
                pace.received += n;
                ctx.shared.received.fetch_add(n, Ordering::Relaxed);
                if checked.elapsed() >= Duration::from_millis(500) {
                    checked = std::time::Instant::now();
                    ctx.shared.conn(id, |c| c.speed = pace.speed());
                    note_best(ctx, &pace);
                    if should_shed(ctx, id, &pace, out.next()) {
                        out.finish(&mut pace).await?;
                        return Err(Error::SlowConnection);
                    }
                }
                if full {
                    (false, None, None)
                } else {
                    match out.step(&mut pace).await? {
                        Some(end) => (false, None, Some(end)),
                        None => continue,
                    }
                }
            }
            Err(e) => (true, Some(e), None),
            Ok(None) => (true, None, None),
        };
        let complete = match complete {
            Some(end) => Some(end),
            None => out.finish(&mut pace).await?,
        };
        match complete {
            Some(end) => {
                note_best(ctx, &pace);
                // A segment another connection shortened leaves an HTTP body far from its end
                // (dropped); an FTP session is kept either way.
                if err.is_none() && (out.pos >= to || matches!(body, Body::Ftp(_))) {
                    // Not past a stop (`stop_connections` waits for this connection).
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => {}
                        () = body.done(ended) => {}
                    }
                }
                return Ok(Received::Complete(end));
            }
            // A capped request (`request_cap`) answered in full: the next one goes on.
            None if ended && err.is_none() && out.pos >= to && ctx.opts.cap().is_some() => {
                return Ok(Received::More(out.pos));
            }
            None if ended => return Err(err.unwrap_or(Error::ShortBody)),
            None => {}
        }
    }
}

/// A connection's received bytes on their way to the disk (`receive`): written and committed
/// up to `pos`, then the write the disk is taking now (one at a time, in order), then the
/// batches waiting in memory (`queue`), then `buf`, the batch still filling. Every byte claimed
/// and not yet written counts in `Shared::held`.
///
/// Every batch is a buffer of `BATCH` capacity that is filled, never grown, and handed to the
/// disk as it is: the memory the connection holds is its batches, counted by capacity
/// (`in_memory`), and `hold_cap` bounds that. It counted the bytes once, not the memory: a
/// batch grew past `BATCH` when a chunk overran it (Vec doubling), and a write joined its
/// batches into one buffer grown in steps up to `WRITE_MAX`, copying them; 0.5.2 kept one
/// buffer per connection, grown by doubling up to its share of the cap. A 4 GB download from a
/// fast server committed 967 MB in 0.5.2's window, 575 MB before this, 523 MB after (235 MB of
/// it the window's own); `tests/memory.rs` counts every allocation.
struct Writer<'a> {
    ctx: &'a Ctx,
    id: WorkerId,
    pos: u64,
    writing: Option<Writing>,
    queue: std::collections::VecDeque<Vec<u8>>,
    queued: u64,
    /// The batch filling: no buffer (capacity 0) until bytes come.
    buf: Vec<u8>,
    /// Written batches, emptied, for the next ones (at most `SPARE_MAX`).
    spare: Vec<Vec<u8>>,
    /// When `buf` was last closed: received bytes go to the disk at least `FLUSH_EVERY`.
    closed_at: std::time::Instant,
}

/// The write the disk is taking (`Writer::send`): its task, which hands the batches back once
/// written, and how many bytes and batches it carries.
struct Writing {
    task: tokio::task::JoinHandle<io::Result<Vec<Vec<u8>>>>,
    bytes: u64,
    batches: usize,
}

impl<'a> Writer<'a> {
    fn new(ctx: &'a Ctx, id: WorkerId, from: u64) -> Self {
        Writer {
            ctx,
            id,
            pos: from,
            writing: None,
            queue: Default::default(),
            queued: 0,
            buf: Vec::new(),
            spare: Vec::new(),
            closed_at: std::time::Instant::now(),
        }
    }

    /// Bytes claimed and not written yet.
    fn pending(&self) -> u64 {
        self.writing.as_ref().map_or(0, |w| w.bytes) + self.queued + self.buf.len() as u64
    }

    /// The memory the bytes not written yet take: their batches' capacity.
    fn in_memory(&self) -> u64 {
        in_memory(
            self.writing.as_ref().map_or(0, |w| w.batches),
            self.queue.len(),
            self.buf.capacity(),
        )
    }

    /// The next byte to claim.
    fn next(&self) -> u64 {
        self.pos + self.pending()
    }

    /// `data` into batches: `buf` filled up to `BATCH` (a spare one, or a new one), then closed.
    fn push(&mut self, mut data: &[u8]) {
        self.ctx
            .shared
            .held
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        while !data.is_empty() {
            if self.buf.capacity() == 0 {
                self.buf = self
                    .spare
                    .pop()
                    .unwrap_or_else(|| Vec::with_capacity(BATCH));
            }
            let n = (BATCH - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.buf.len() == BATCH {
                self.close();
            }
        }
    }

    /// After a chunk: once `buf` is a batch (`push` closes it) or `FLUSH_EVERY` old it goes to
    /// the disk, or waits in memory while the disk still takes an earlier write. Past `hold_cap`
    /// in memory, the connection waits for the disk. `Some(end)`: the segment is complete.
    async fn step(&mut self, pace: &mut Pace) -> Result<Option<u64>, Error> {
        if self.writing.as_ref().is_some_and(|w| w.task.is_finished())
            && let Some(end) = self.wait().await?
        {
            return Ok(Some(end));
        }
        if self.closed_at.elapsed() >= FLUSH_EVERY {
            self.close();
        }
        if self.writing.is_some() && self.in_memory() >= hold_cap(self.ctx) {
            let began = std::time::Instant::now();
            let done = self.wait().await;
            pace.waited(self.ctx, began.elapsed());
            if let Some(end) = done? {
                return Ok(Some(end));
            }
        }
        if self.writing.is_none() {
            self.send();
        }
        Ok(None)
    }

    /// Everything to the disk, waiting for it (counted as a wait for the disk in `pace`).
    /// `Some(end)`: the segment is complete.
    async fn finish(&mut self, pace: &mut Pace) -> Result<Option<u64>, Error> {
        let began = std::time::Instant::now();
        self.close();
        let mut end = None;
        let done = loop {
            match self.wait().await {
                Ok(e) => end = e.or(end),
                Err(e) => break Err(e),
            }
            if self.queue.is_empty() {
                break Ok(end);
            }
            self.send();
        };
        pace.waited(self.ctx, began.elapsed());
        done
    }

    /// `buf` joins the queue.
    fn close(&mut self) {
        self.closed_at = std::time::Instant::now();
        if !self.buf.is_empty() {
            let batch = std::mem::take(&mut self.buf);
            debug_assert_eq!(batch.capacity(), BATCH, "a batch never grows");
            self.queued += batch.len() as u64;
            self.queue.push_back(batch);
        }
    }

    /// Start writing the queue's first batches, up to `WRITE_MAX` (nothing else is being
    /// written). Small writes keep the waits for a disk that sets the pace short and even. The
    /// batches go to the disk as they are, one after the other (no copy joins them).
    fn send(&mut self) {
        debug_assert!(self.writing.is_none());
        let mut batches = Vec::new();
        let mut n = 0;
        while let Some(next) = self.queue.front()
            && (batches.is_empty() || n + next.len() <= WRITE_MAX)
        {
            n += next.len();
            batches.extend(self.queue.pop_front());
        }
        if batches.is_empty() {
            return;
        }
        let n = n as u64;
        self.queued -= n;
        let count = batches.len();
        let (file, at) = (self.ctx.file.clone(), self.pos);
        let slow = self.ctx.opts.slow_disk.clone();
        let task = tokio::task::spawn_blocking(move || {
            let write = || write_batches(&file, at, &batches);
            match slow {
                Some(disk) => disk.write(n, write),
                None => write(),
            }
            .map(|()| batches)
        });
        self.writing = Some(Writing {
            task,
            bytes: n,
            batches: count,
        });
    }

    /// Wait for the write under way and commit it. `Some(end)`: the segment is complete.
    async fn wait(&mut self) -> Result<Option<u64>, Error> {
        let Some(Writing { task, bytes: n, .. }) = self.writing.take() else {
            return Ok(None);
        };
        let written = task.await.expect("writer panicked");
        self.ctx.shared.held.fetch_sub(n, Ordering::Relaxed);
        // The written batches, emptied, take the next bytes (a few; the rest are freed).
        for mut batch in written? {
            if self.spare.len() < SPARE_MAX {
                batch.clear();
                self.spare.push(batch);
            }
        }
        self.pos += n;
        self.ctx.shared.conn(self.id, |c| c.downloaded += n);
        Ok(self
            .ctx
            .shared
            .with_planner(|p| p.commit(self.id, self.pos)))
    }
}

impl Drop for Writer<'_> {
    /// Bytes that will never be written (an error ended the connection) are no longer held.
    fn drop(&mut self) {
        let left = self.pending();
        self.ctx.shared.held.fetch_sub(left, Ordering::Relaxed);
    }
}

/// How much memory a connection may fill while the disk takes its writes (`HOLD_MAX`, shared
/// by the connections this download may open), counted as `Writer::in_memory` counts it.
fn hold_cap(ctx: &Ctx) -> u64 {
    HOLD_MAX / u64::from(ctx.opts.connections().max(1))
}

/// The memory a connection's bytes not yet written take: `writing` batches on their way to the
/// disk, `queued` ones waiting, and `filling`, the capacity of the one filling.
fn in_memory(writing: usize, queued: usize, filling: usize) -> u64 {
    ((writing + queued) * BATCH + filling) as u64
}

/// One response's pace (`receive`): where a connection's speed comes from, for the list and for
/// the slow check alike.
struct Pace {
    started: std::time::Instant,
    received: u64,
    /// The time the connection stood still for the disk (`Writer`): it wasn't reading the
    /// network then.
    on_disk: Duration,
}

impl Pace {
    fn new() -> Self {
        Pace {
            started: std::time::Instant::now(),
            received: 0,
            on_disk: Duration::ZERO,
        }
    }

    /// Bytes/s since the response began, every second counted: what the connection brought.
    /// The bytes the system kept for it while it stood still for the disk count over all the
    /// time they took to come, so reading them in a burst afterwards is not speed (#782).
    fn speed(&self) -> u64 {
        rate(self.received, self.started.elapsed())
    }

    /// The time spent reading the network.
    fn on_net(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.on_disk)
    }

    /// Bytes/s while reading the network: for the slow check, so a wait for the disk never
    /// counts against a connection (#361). The burst after a wait makes it read high, which
    /// can keep a connection, never shed one.
    fn net_speed(&self) -> u64 {
        rate(self.received, self.on_net())
    }

    /// The connection stood still `d` for the disk; a long wait tells the others (`disk_held`).
    fn waited(&mut self, ctx: &Ctx, d: Duration) {
        self.on_disk += d;
        if d >= DISK_HELD {
            *ctx.shared.disk_held.lock().unwrap() = Some(std::time::Instant::now());
        }
    }
}

/// `bytes` over `d`, bytes/s (`u64::MAX` for no time at all: nothing is slower than that).
fn rate(bytes: u64, d: Duration) -> u64 {
    let secs = d.as_secs_f64();
    if secs > 0.0 {
        (bytes as f64 / secs) as u64
    } else {
        u64::MAX
    }
}

/// Remember the speed of a response that ran long enough (`BEST_AFTER`, 256 KiB) to say what
/// this server can do over one connection, and what the connections bring together.
fn note_best(ctx: &Ctx, pace: &Pace) {
    if let Some(speed) = yardstick(pace.received, pace.started.elapsed()) {
        ctx.shared.best_speed.fetch_max(speed, Ordering::Relaxed);
    }
    note_link(&ctx.shared);
}

/// What a download's connections receive together, window by window (`LINK_WINDOW`).
#[derive(Debug, Default)]
struct LinkMeter {
    /// Where the window under way began: when, and the bytes received by then.
    since: Option<(std::time::Instant, u64)>,
    /// The most the connections brought together in one window, bytes/s.
    best: u64,
}

/// Close the link's window once it is `LINK_WINDOW` old, and keep its speed if it is the best.
/// Not a window in which a connection stood still for the disk (`disk_held`), nor the one after:
/// what the system kept for the connections meanwhile came in a burst once the disk let them
/// read again, faster than the link (#782).
fn note_link(shared: &Shared) {
    let now = std::time::Instant::now();
    let total = shared.received.load(Ordering::Relaxed);
    let held = *shared.disk_held.lock().unwrap();
    let mut m = shared.link.lock().unwrap();
    let Some((since, then)) = m.since else {
        m.since = Some((now, total));
        return;
    };
    let took = now.saturating_duration_since(since);
    if took < LINK_WINDOW {
        return;
    }
    let disk = held.is_some_and(|t| now.saturating_duration_since(t) < took + LINK_WINDOW);
    if !disk {
        m.best = m.best.max(rate(total.saturating_sub(then), took));
    }
    m.since = Some((now, total));
}

/// What one connection of this download can do, the bar `too_slow` measures connections
/// against: the best speed one connection sustained (`one`), but never more than its share of
/// the best the connections brought together (`link`) among the `receiving` ones. A first
/// connection alone on the line takes all of it, and once the others join each gets a part:
/// measured against that first speed every one of them read as slow, and they were reconnected
/// together every few seconds, a dip in the speed each time (#896, found by mhmdeve). Where each
/// connection has a speed of its own (a server that caps each one), the share is above it and
/// the bar stays that speed.
///
/// The link carried at least what one connection brought (`one`), so it is never measured at
/// less: the windows in which a connection stood still for the disk don't count (`note_link`),
/// and when the disk set the pace through every window of a download's fast part, the first
/// window that counted was its end's, where only the crawling connections were left. The link
/// then read a few KB/s, the bar their own pace, and a crawling connection held a last piece
/// too small to split for a minute instead of reconnecting (seen on busy test machines).
fn shed_bar(one: u64, link: u64, receiving: usize) -> u64 {
    if link == 0 {
        return one;
    }
    one.min(link.max(one) / receiving.max(1) as u64)
}

/// The speed a response of `received` bytes in `elapsed` (all of its time, the disk's waits
/// too) sets as what one connection can do, if it says so yet. Over all its time, never its
/// time on the network only: a connection that stood still for the disk reads what the system
/// kept for it meanwhile in a few milliseconds, and that burst once raised the bar so high
/// that every connection fell under it and was shed, all at once (#782).
fn yardstick(received: u64, elapsed: Duration) -> Option<u64> {
    (received >= 256 * 1024 && elapsed >= BEST_AFTER).then(|| rate(received, elapsed))
}

/// Whether connection `id` should reconnect (`too_slow`), its next byte `at`. Never while a
/// speed limit makes connections slow on purpose, nor under `request_cap`: each request ends
/// soon by itself, and the server's pace varies from one to the next (YouTube's, #769:
/// reconnecting cost more than it won).
fn should_shed(ctx: &Ctx, id: WorkerId, pace: &Pace, at: u64) -> bool {
    let limited = ctx.shared.limiter.get().is_some()
        || ctx.opts.global.as_ref().is_some_and(|g| g.get().is_some());
    if !ctx.opts.shed_slow || limited || ctx.opts.cap().is_some() {
        return false;
    }
    let held = ctx.shared.disk_held.lock().unwrap().map(|t| t.elapsed());
    let one = ctx.shared.best_speed.load(Ordering::Relaxed);
    let link = ctx.shared.link.lock().unwrap().best;
    let (left, receiving) = ctx
        .shared
        .with_planner(|p| (p.room(id, at).unwrap_or(0), p.receiving_workers()));
    too_slow(
        pace.net_speed(),
        pace.on_net(),
        shed_bar(one, link, receiving),
        left,
        held,
    )
}

/// A connection at `speed` over `on_net` of reading the network, with `left` bytes of its
/// segment ahead, is slow enough to reconnect: far slower than `best`, what one connection of
/// this download can do (`shed_bar`; comparing with the others now fails once they are all
/// stuck on slow paths),
/// with a lot left at its pace. Not while the disk sets the pace (`held`: a connection last
/// stood still for it that long ago, #782): the connections' speeds then say what the disk
/// lets through, a new connection would wait for the same disk, and the ones that waited
/// restart slowly (their sender saw a closed window).
fn too_slow(speed: u64, on_net: Duration, best: u64, left: u64, held: Option<Duration>) -> bool {
    if on_net < SHED_GRACE || held.is_some_and(|ago| ago < SHED_GRACE) {
        return false;
    }
    speed.saturating_mul(SHED_RATIO) < best
        && left as f64 / speed.max(1) as f64 > SHED_LEFT.as_secs_f64()
}

/// Servers without range support: one connection, written in order. Returns the size.
async fn single_stream(
    mut body: Body,
    file: &Arc<File>,
    shared: &Arc<Shared>,
    global: Option<&Limiter>,
    cancel: &CancellationToken,
) -> Result<u64, Error> {
    shared.conn(0, |c| c.status = ConnStatus::Receiving);
    let expected = body.len();
    let mut pos = 0u64;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            n = body.chunk() => n,
        };
        let Some(chunk) = next? else { break };
        // Stop ends a limiter's wait too (under a low limit it is seconds long).
        let paid = async {
            shared.limiter.acquire(chunk.len() as u64).await;
            if let Some(g) = global {
                g.acquire(chunk.len() as u64).await;
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = paid => {}
        }
        let (f, at, n) = (file.clone(), pos, chunk.len() as u64);
        tokio::task::spawn_blocking(move || write_all_at(&f, at, &chunk))
            .await
            .expect("writer panicked")?;
        pos += n;
        let mut v = shared.view.lock().unwrap();
        v.downloaded = pos;
        if let Some(c) = v.connections.first_mut() {
            c.downloaded = pos;
        }
    }
    if expected.is_some_and(|e| e != pos) {
        return Err(Error::ShortBody);
    }
    shared.conn(0, ConnView::finish);
    Ok(pos)
}

fn read_at(file: &File, mut at: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0;
    while got < len {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[got..], at)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[got..], at)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        got += n;
        at += n as u64;
    }
    Ok(buf)
}

/// `batches` one after the other into `file` from `at`.
fn write_batches(file: &File, mut at: u64, batches: &[Vec<u8>]) -> io::Result<()> {
    for b in batches {
        write_all_at(file, at, b)?;
        at += b.len() as u64;
    }
    Ok(())
}

fn write_all_at(file: &File, mut at: u64, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(file, data, at)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(file, data, at)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        at += n as u64;
        data = &data[n..];
    }
    Ok(())
}

/// `Content-Range: bytes a-b/total` → `(a, b, total)`; `total` is `None` for `*`.
fn parse_content_range(h: &HeaderMap) -> Result<(u64, u64, Option<u64>), Error> {
    let raw = h
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let bad = || Error::BadRange(raw.to_owned());
    let rest = raw.strip_prefix("bytes ").ok_or_else(bad)?;
    let (range, total) = rest.split_once('/').ok_or_else(bad)?;
    let (a, b) = range.split_once('-').ok_or_else(bad)?;
    let a = a.trim().parse().map_err(|_| bad())?;
    let b = b.trim().parse().map_err(|_| bad())?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().map_err(|_| bad())?),
    };
    Ok((a, b, total))
}

/// `Retry-After`: delay in seconds, or an HTTP date.
pub(crate) fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let v = h.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = httpdate::parse_http_date(v).ok()?;
    Some(
        when.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

pub(crate) async fn cancellable<T>(
    cancel: &CancellationToken,
    fut: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        r = fut => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_lead_on_as_browsers_read_them() {
        let base = Url::parse("https://site.test/download/abc").unwrap();
        let at = |v: &str| refresh_target(v, &base).map(|(d, u)| (d.as_secs(), u.to_string()));
        assert_eq!(
            at("0; url=https://cdn.test/f.zip"),
            Some((0, "https://cdn.test/f.zip".into()))
        );
        assert_eq!(
            at("5;URL='/get/f.zip'"),
            Some((5, "https://site.test/get/f.zip".into()))
        );
        assert_eq!(
            at("3, next?a=1&amp;b=2"),
            Some((3, "https://site.test/download/next?a=1&b=2".into()))
        );
        // The wait is capped; a page that reloads itself, or leads off the web, is the answer.
        assert_eq!(at("600; url=/x").map(|a| a.0), Some(PAGE_WAIT.as_secs()));
        assert_eq!(at("30"), None);
        assert_eq!(at("0; url=abc#top"), None);
        assert_eq!(at("0; url=javascript:alert(1)"), None);
        assert_eq!(at("soon; url=/x"), None);

        let page = |html: &str| meta_refresh(html.as_bytes(), &base).map(|(_, u)| u.to_string());
        assert_eq!(
            page(r#"<html><META HTTP-EQUIV="Refresh" CONTENT="0;URL=/file.zip"></html>"#),
            Some("https://site.test/file.zip".into())
        );
        assert_eq!(
            page("<meta content='2; url=https://cdn.test/a.bin' http-equiv=refresh>"),
            Some("https://cdn.test/a.bin".into())
        );
        // Other meta tags, and attribute names inside others, don't count.
        assert_eq!(
            page(
                r#"<meta name="description" content="0; url=/x"><meta data-http-equiv="refresh" content="0; url=/y">"#
            ),
            None
        );
        assert_eq!(page("<p>window.location = '/x'</p>"), None);
    }

    #[test]
    fn a_pages_cookies_go_on_with_it() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; session=old"));
        let mut set = HeaderMap::new();
        set.append(
            header::SET_COOKIE,
            HeaderValue::from_static("session=new; Path=/; HttpOnly"),
        );
        set.append(
            header::SET_COOKIE,
            HeaderValue::from_static("xsrf=t; Secure"),
        );
        add_cookies(&mut h, &set_cookies(&set));
        assert_eq!(
            header_str(&h, header::COOKIE),
            Some("a=1; session=new; xsrf=t")
        );
    }

    /// #782: a flush is started only when it should be over well before the connections run
    /// out of memory: on a disk that keeps up, as often as the file grows; on one that falls
    /// behind (each flush writes all that came since the last), not while it can't keep up.
    #[test]
    fn a_flush_starts_only_when_it_should_be_short() {
        let s = Duration::from_secs;
        let at = |took, covered| Synced {
            at: std::time::Instant::now(),
            bytes: 0,
            took,
            covered,
        };
        let (gb, mb) = (1u64 << 30, 1u64 << 20);
        // A disk that keeps up: 0.1 s to write 2 s of 115 MB/s. Every tick once 64 MiB came.
        let fast = at(Duration::from_millis(100), 230 * mb);
        assert!(fast.due(230 * mb, 45 * gb, s(2), HOLD_MAX));
        assert!(!fast.due(32 * mb, 45 * gb, s(2), HOLD_MAX));
        // Few bytes: at the latest after SYNC_EVERY.
        assert!(fast.due(32 * mb, 45 * gb, s(30), HOLD_MAX));
        // A hard disk behind the network: 2 s to write what 2 s of 115 MB/s left in the cache.
        // The next would take as long again, more than 256 MiB at that pace (2.3 s) covers with
        // room to spare, and every second waited makes it worse.
        let behind = at(s(2), 230 * mb);
        assert!(!behind.due(230 * mb, 45 * gb, s(2), HOLD_MAX));
        assert!(!behind.due(gb, 45 * gb, s(9), HOLD_MAX));
        assert!(!behind.due(7 * gb, 45 * gb, s(60), HOLD_MAX));
        // The same disk once the download has slowed to 10 MB/s: a flush fits again.
        assert!(behind.due(70 * mb, 45 * gb, s(7), HOLD_MAX));
        // 0.5.2's spacing (nine times the last flush) let them grow: 2 s, then 18 s later 18 s.
        assert_eq!(behind.expected(9 * 230 * mb), s(18));
        // The memory full of what waits for the disk (one that sets the pace): not even on
        // the fast disk.
        assert!(!fast.due(230 * mb, 45 * gb, s(2), 0));
        // A flush that takes 1.5 s whatever it writes (#361's test disk), here after 100 ms of
        // 115 MB/s: not again.
        assert!(!at(Duration::from_millis(1500), 11 * mb).due(gb, gb, s(5), HOLD_MAX));
        // Right after a flush of 0.8 s for 48 MiB, the 80 MiB that came meanwhile (the memory
        // keeps them while it holds the writes) reach the cache at once: the next flush has them
        // to write, 1.3 s at that pace, too long for the 210 MiB of memory left. Counted by
        // the 0.16 s since, it was expected to take 0.3 s; it took 2.1 s, and on GitHub's macOS
        // runners 3 s, the transfer stopped for 1.4 s of it.
        let after_one = at(Duration::from_millis(800), 48 * mb);
        assert!(!after_one.due(80 * mb, 768 * mb, Duration::from_millis(155), 210 * mb));
        // Once the download has slowed enough for it to fit, it is due again.
        assert!(after_one.due(80 * mb, 768 * mb, s(4), 210 * mb));
    }

    /// #518: when the forced save at the start (no connection writing yet) takes 0.5 s on a busy
    /// disk, the first save of progress still comes at the first tick.
    #[test]
    fn a_forced_save_does_not_push_the_next_back() {
        let at = |took| Synced {
            at: std::time::Instant::now(),
            bytes: 0,
            took,
            // 1 ms of 24 MB/s.
            covered: 24 << 10,
        };
        let (slow, tick) = (Duration::from_millis(500), Duration::from_secs(2));
        assert!(at(forced_gap(true, slow)).due(15 << 20, 24 << 20, tick, HOLD_MAX));
        // A save during the transfer does count: 0.5 s for 1 ms of data is a disk far behind.
        assert!(!at(forced_gap(false, slow)).due(15 << 20, 24 << 20, tick, HOLD_MAX));
    }

    /// #782: what the slow check makes of connections that wait for the disk. A connection
    /// stood still 8 s for the disk, 2 MiB into its response, then read the 30 MiB the system
    /// kept for it in 50 ms. Over its time on the network that is 128 MiB/s, which made every
    /// connection of a 1 Gbit/s link (14 MiB/s each) four times slower than the best: all of
    /// them were shed, at once. Over all its time it is what the link gave it.
    #[test]
    fn waiting_for_the_disk_never_makes_a_connection_slow() {
        let mb = 1u64 << 20;
        let (on_net, waited) = (Duration::from_millis(250), Duration::from_secs(8));
        let burst = yardstick(32 * mb, on_net + waited).unwrap();
        assert!(burst < 14 * mb, "{burst}");
        let link = 14 * mb;
        let left = 1 << 30;
        let three = Duration::from_secs(3);
        // Against the burst's speed over its network time alone, a connection doing what the
        // link gives would be shed (0.5.2); against the honest yardstick it isn't.
        assert!(too_slow(link, three, rate(32 * mb, on_net), left, None));
        assert!(!too_slow(link, three, burst.max(link), left, None));
        // A connection that itself waited for the disk: its speed leaves the waits out (8 s
        // waited of 10 s, 28 MiB: 14 MiB/s over the 2 s it read).
        assert_eq!(rate(28 * mb, Duration::from_secs(2)), link);
        // A connection really slow on its path (1 MiB/s) is still shed, the disk keeping up...
        assert!(too_slow(mb, three, link, left, None));
        assert!(too_slow(
            mb,
            three,
            link,
            left,
            Some(Duration::from_secs(5))
        ));
        // ...but not while the disk sets the pace: a connection stood still for it 1 s ago.
        assert!(!too_slow(
            mb,
            three,
            link,
            left,
            Some(Duration::from_secs(1))
        ));
        // Nor before it read the network for SHED_GRACE, nor with little left.
        assert!(!too_slow(mb, Duration::from_secs(1), link, left, None));
        assert!(!too_slow(mb, three, link, mb, None));
        // A response says what a connection can do only once it ran BEST_AFTER (0.1 s).
        assert_eq!(yardstick(32 * mb, Duration::from_millis(50)), None);
        assert_eq!(yardstick(100 << 10, Duration::from_secs(2)), None);
    }

    /// #896: a line of 8 MiB/s that the first connection had to itself before the others
    /// opened. Shared by 8, each gets 1 MiB/s: against the first one's speed every one of them
    /// was slow; against its share of the line none is, and one that crawls still is.
    #[test]
    fn connections_sharing_a_line_are_measured_against_their_share() {
        let mb = 1u64 << 20;
        let (left, three) = (1 << 30, Duration::from_secs(3));
        let (first, line) = (8 * mb, 8 * mb);
        assert!(too_slow(mb, three, first, left, None), "the old bar");
        let bar = shed_bar(first, line, 8);
        assert_eq!(bar, mb);
        assert!(!too_slow(mb, three, bar, left, None));
        assert!(too_slow(64 << 10, three, bar, left, None), "a crawling one");
        // A server that caps each connection (2 MiB/s each, 16 MiB/s for 8): the share is
        // above what one connection does, so the bar stays that, with 8 receiving or 2.
        assert_eq!(shed_bar(2 * mb, 16 * mb, 8), 2 * mb);
        assert_eq!(shed_bar(2 * mb, 16 * mb, 2), 2 * mb);
        // Nothing measured together yet: the one connection's best.
        assert_eq!(shed_bar(first, 0, 8), first);
        // Only the end measured together, three connections crawling at 1 KiB/s once the others
        // were done (the disk set the pace before): the line still carried what the first one
        // brought, and the crawling ones are still slow against it.
        let crawl = 1 << 10;
        assert_eq!(shed_bar(first, 3 * crawl, 1), first);
        assert_eq!(shed_bar(first, 3 * crawl, 3), first / 3);
        let end = shed_bar(first, 3 * crawl, 1);
        assert!(too_slow(crawl, three, end, 62 << 10, None), "a last piece");
    }

    #[test]
    fn flush_step_scales_with_the_file() {
        assert_eq!(sync_bytes(24 << 20), 3 << 19);
        assert_eq!(sync_bytes(100 << 10), 1 << 20);
        assert_eq!(sync_bytes(10 << 30), 64 << 20);
    }

    #[test]
    fn connections_stay_in_range() {
        let with = |n| Options {
            max_connections: n,
            ..Options::default()
        };
        assert_eq!(with(0).connections(), 1);
        assert_eq!(with(100).connections(), 100);
        assert_eq!(with(5000).connections(), MAX_CONNECTIONS_UNLOCKED);
    }

    #[tokio::test]
    async fn network_errors_read_plainly() {
        // Nothing listens on port 1.
        let e = reqwest::get("http://127.0.0.1:1/").await.unwrap_err();
        let msg = Error::from(e).to_string();
        assert!(msg.starts_with("cannot connect to the server"), "{msg}");
        assert!(
            !msg.contains("127.0.0.1") && !msg.contains("os error"),
            "{msg}"
        );
    }

    #[test]
    fn retry_after_seconds_or_date() {
        let mut h = HeaderMap::new();
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        let later = std::time::SystemTime::now() + Duration::from_secs(90);
        h.insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&httpdate::fmt_http_date(later)).unwrap(),
        );
        let d = retry_after(&h).unwrap();
        assert!(
            d > Duration::from_secs(80) && d <= Duration::from_secs(90),
            "{d:?}"
        );
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("soon"));
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn dead_links_are_not_retried() {
        for s in [401, 403, 404, 410, 416] {
            assert!(
                Error::Status(StatusCode::from_u16(s).unwrap()).link_dead(),
                "{s}"
            );
        }
        for s in [500, 502] {
            assert!(
                !Error::Status(StatusCode::from_u16(s).unwrap()).link_dead(),
                "{s}"
            );
        }
        assert!(!Error::Throttled(StatusCode::SERVICE_UNAVAILABLE, None).link_dead());
    }

    #[test]
    fn credentials_stay_with_their_host() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("sid=1"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic x"));
        h.insert(header::REFERER, HeaderValue::from_static("https://site/"));
        let u = |s: &str| Url::parse(s).unwrap();
        let same = scoped_headers(&h, &u("https://site/a"), &u("https://site/b"));
        assert!(same.contains_key(header::COOKIE) && same.contains_key(header::AUTHORIZATION));
        let cdn = scoped_headers(&h, &u("https://site/a"), &u("https://cdn.example/a"));
        assert!(!cdn.contains_key(header::COOKIE) && !cdn.contains_key(header::AUTHORIZATION));
        assert!(
            cdn.contains_key(header::REFERER),
            "non-credential headers stay"
        );
        let down = scoped_headers(&h, &u("https://site/a"), &u("http://site/a"));
        assert!(
            !down.contains_key(header::COOKIE),
            "no credentials over a https -> http downgrade"
        );
        let port = scoped_headers(&h, &u("http://site/a"), &u("http://site:8080/a"));
        assert!(
            !port.contains_key(header::COOKIE),
            "another port is another origin"
        );
    }

    #[test]
    fn a_drive_links_cookies_go_to_drives_own_hosts_only() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("SID=1"));
        let u = |s: &str| Url::parse(s).unwrap();
        let link = u("https://drive.google.com/file/d/x/view");
        for to in [
            "https://drive.usercontent.google.com/download?id=x",
            "https://docs.google.com/document/d/x/export",
        ] {
            assert!(
                scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
        for to in [
            "https://accounts.google.com/ServiceLogin",
            "https://doc-0.docs.googleusercontent.com/export/x",
            "http://drive.usercontent.google.com/download?id=x",
        ] {
            assert!(
                !scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
    }

    #[test]
    fn an_archive_org_links_cookies_go_to_archive_storage_nodes() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("logged-in-user=user; logged-in-sig=sig"),
        );
        let u = |s: &str| Url::parse(s).unwrap();
        let link = u("https://archive.org/download/sample-item/sample.pdf");
        for to in [
            "https://ia600100.us.archive.org/24/items/sample-item/sample.pdf",
            "https://ia800102.us.archive.org/items/sample-item/sample.pdf",
            "https://dn720303.ca.archive.org/0/items/sample-item/sample.pdf",
        ] {
            assert!(
                scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
        for to in [
            "https://attacker.example/sample.pdf",
            "https://notarchive.org/download",
            "http://ia600100.us.archive.org/sample.pdf",
        ] {
            assert!(
                !scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
    }

    #[test]
    fn drive_gets_a_browsers_agent_and_only_drive() {
        let u = |s: &str| Url::parse(s).unwrap();
        let browser = Some(sites::gdrive::USER_AGENT);
        // MDM's automatic agent (the default browser's, #668), by default.
        let auto = Options::default();
        assert!(auto.automatic_agent && auto.site_agent());
        for drive in [
            "https://drive.google.com/file/d/x/view",
            "https://drive.usercontent.google.com/download?id=x",
            "http://docs.google.com:8080/document/d/x/export",
        ] {
            assert_eq!(
                request_agent(auto.site_agent(), &u(drive)),
                browser,
                "{drive}"
            );
        }
        for other in [
            "https://example.com/a.zip",
            "https://accounts.google.com/ServiceLogin",
            "https://doc-0.docs.googleusercontent.com/export/x",
            "https://drive.google.com.evil.test/x",
        ] {
            assert_eq!(request_agent(auto.site_agent(), &u(other)), None, "{other}");
        }
        // The browser's own agent (handed over with the download) or the user's wins, even
        // when it is the same text as the automatic one.
        for own in [
            "Mozilla/5.0 Firefox/140.0".to_owned(),
            crate::agent::automatic(),
        ] {
            let mut opts = Options::default();
            opts.speak_as(own);
            assert!(!opts.site_agent());
            assert_eq!(
                request_agent(opts.site_agent(), &u("https://drive.google.com/x")),
                None
            );
        }
        // MDM's former agent, asked with when a browser's is refused (#372), still gets it.
        let mut fallback = Options::default();
        fallback.speak_as(crate::agent::IE11_USER_AGENT.to_owned());
        assert!(fallback.site_agent());
    }

    #[test]
    fn content_range() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-99/1000"),
        );
        assert_eq!(parse_content_range(&h).unwrap(), (0, 99, Some(1000)));
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 5-9/*"),
        );
        assert_eq!(parse_content_range(&h).unwrap(), (5, 9, None));
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("items 0-1/2"),
        );
        assert!(parse_content_range(&h).is_err());
    }
    fn copy_validators(etag: &str, host: &str, modified: &str) -> Validators {
        Validators::from_response(
            Some(etag),
            Some(modified),
            Some("Wed, 21 Oct 2015 08:00:00 GMT"),
            Some(host),
        )
    }

    #[test]
    fn other_server_copy_accepts_rapidgator_node_rotation() {
        let ours = copy_validators(
            "\"a\"",
            "s1.rapidgator.net",
            "Wed, 21 Oct 2015 07:28:00 GMT",
        );
        let theirs = copy_validators(
            "\"b\"",
            "s2.rapidgator.net",
            "Wed, 21 Oct 2015 07:28:12 GMT",
        );
        let at = Url::parse("https://s2.rapidgator.net/download/token").unwrap();
        assert_ne!(ours.last_modified, theirs.last_modified);
        assert!(is_other_server_copy(&ours, &theirs, &at, true));
        assert!(!is_other_server_copy(&ours, &theirs, &at, false));
        assert!(!is_other_server_copy(&theirs, &theirs, &at, true));
        assert!(!is_other_server_copy(
            &Validators::default(),
            &theirs,
            &at,
            true
        ));
        for etag in [None, Some("W/\"b\""), Some("malformed")] {
            let theirs = Validators::from_response(etag, None, None, at.host_str());
            assert!(!is_other_server_copy(&ours, &theirs, &at, true));
        }
    }

    #[test]
    fn other_server_copy_rejects_generic_mirrors_and_unrelated_hosts() {
        for (old, target) in [
            ("dl1.example.com", "https://dl2.example.com/firmware.bin"),
            (
                "dl1.example.com",
                "https://s2.rapidgator.net/download/token",
            ),
            ("rg.to", "https://s2.rapidgator.net/download/token"),
            (
                "rapidgator.net.example.com",
                "https://s2.rapidgator.net/download/token",
            ),
            (
                "s1.rapidgator.net",
                "https://s2.rapidgator.net.example.com/download/token",
            ),
            (
                "s1.rapidgator.net",
                "https://s2.rapidgator.net/firmware.bin",
            ),
        ] {
            let ours = copy_validators("\"a\"", old, "Wed, 21 Oct 2015 07:28:00 GMT");
            let at = Url::parse(target).unwrap();
            let theirs = copy_validators(
                "\"b\"",
                at.host_str().unwrap(),
                "Wed, 21 Oct 2015 07:28:12 GMT",
            );
            assert!(
                !is_other_server_copy(&ours, &theirs, &at, true),
                "{old} -> {target}"
            );
        }
    }
}
