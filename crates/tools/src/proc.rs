//! Supervised helper processes (yt-dlp, gallery-dl): every child is in a Windows Job Object of
//! its own that kills it when we exit (even on a crash), or on Linux and macOS in a process group
//! of its own that MDM's end kills ([`end_all`]), runs without a console window, gets its
//! arguments in fixed layers (the user's extras, ours, then the fixed ones and `--` before the
//! URL) with CR / LF / NUL refused, keeps only a short stderr tail for messages, and goes through
//! the chosen network adapter (#35, `crate::relay`).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lines of stderr kept for error messages.
const TAIL: usize = 20;
/// How many `WARNING:` lines a run keeps (`Spawned::warnings`).
const WARNINGS_KEPT: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    #[error("an argument contains a line break or NUL")]
    BadArgument,
    #[error("could not start {0}: {1}")]
    Start(String, std::io::Error),
    #[error("{0} took too long and was stopped")]
    Timeout(String),
    #[error("{tool} failed: {message}")]
    Failed { tool: String, message: String },
    /// The chosen network adapter is down, or yt-dlp can't be held to it here (#35).
    #[error(transparent)]
    Adapter(#[from] mdm_engine::NetError),
    /// A video's files would pass the system's path limits in its folder (#1102): its name
    /// (`<folder>/<stem>`) is too long there, or the folder is too deep for any.
    #[error("the path is too long for this system with the files saved beside it: {}", .0.display())]
    PathTooLong(std::path::PathBuf),
}

/// Options → Proxy / Socks → Network adapter (#35) for the helpers and the media-server checks;
/// the app shares it as it changes ([`set_route`]).
static ROUTE: std::sync::RwLock<mdm_engine::Route> = std::sync::RwLock::new(mdm_engine::Route::Any);
/// Options → Connection → DNS (#36): what the relay looks the helpers' servers up with.
static DNS: std::sync::RwLock<Option<mdm_engine::DnsChoice>> = std::sync::RwLock::new(None);
/// The relay the helpers go through while an adapter is chosen, started when first needed.
static RELAY: Mutex<Option<Arc<crate::relay::Relay>>> = Mutex::new(None);

/// The network adapter the helpers and the media-server checks go through (the app's setting).
/// A change closes what the relay has open (it was held to the old route).
pub fn set_route(route: mdm_engine::Route) {
    if *ROUTE.read().unwrap() != route {
        *ROUTE.write().unwrap() = route;
        if let Some(relay) = RELAY.lock().unwrap().as_ref() {
            relay.route_changed();
        }
    }
}

pub fn route() -> mdm_engine::Route {
    ROUTE.read().unwrap().clone()
}

/// The DNS the helpers' servers are looked up with while an adapter is chosen (the app's
/// setting; the system's until set).
pub fn set_dns(dns: mdm_engine::DnsChoice) {
    if DNS.read().unwrap().as_ref() != Some(&dns) {
        *DNS.write().unwrap() = Some(dns);
    }
}

fn dns() -> mdm_engine::DnsChoice {
    DNS.read().unwrap().clone().unwrap_or_default()
}

/// The browsers yt-dlp reads cookies from (`--cookies-from-browser`; yt-dlp's
/// `cookies.SUPPORTED_BROWSERS`), in the order Options lists them. Safari only on macOS.
pub const COOKIE_BROWSERS: [&str; 9] = [
    "firefox", "chrome", "edge", "brave", "opera", "vivaldi", "chromium", "whale", "safari",
];

/// The browsers this system offers (Safari is macOS's only).
pub fn cookie_browsers() -> Vec<&'static str> {
    COOKIE_BROWSERS
        .iter()
        .copied()
        .filter(|b| *b != "safari" || cfg!(target_os = "macos"))
        .collect()
}

/// Options → Sites Logins → Cookies for video sites (#229): the browser whose cookies yt-dlp
/// reads on this computer, and its profile (a name, or a profile folder: a Chromium fork's
/// `chromium:<its profile folder>`), for the runs the browser's extension passed no cookies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserCookies {
    pub browser: String,
    pub profile: Option<String>,
}

impl BrowserCookies {
    /// `browser` (one of [`COOKIE_BROWSERS`]) and its profile; None for any other name.
    pub fn new(browser: &str, profile: &str) -> Option<Self> {
        let browser = browser.trim().to_ascii_lowercase();
        (!browser.is_empty() && COOKIE_BROWSERS.contains(&browser.as_str())).then(|| {
            BrowserCookies {
                browser,
                profile: Some(profile.trim().to_owned()).filter(|p| !p.is_empty()),
            }
        })
    }

    /// yt-dlp's `BROWSER[:PROFILE]`.
    pub fn spec(&self) -> String {
        match &self.profile {
            Some(p) => format!("{}:{p}", self.browser),
            None => self.browser.clone(),
        }
    }
}

/// The browser yt-dlp reads cookies from, as the app shares it ([`set_browser_cookies`]).
static BROWSER_COOKIES: std::sync::RwLock<Option<BrowserCookies>> = std::sync::RwLock::new(None);

/// Options → Sites Logins → Cookies for video sites (the app's setting; None: off).
pub fn set_browser_cookies(chosen: Option<BrowserCookies>) {
    if *BROWSER_COOKIES.read().unwrap() != chosen {
        *BROWSER_COOKIES.write().unwrap() = chosen;
    }
}

pub fn browser_cookies() -> Option<BrowserCookies> {
    BROWSER_COOKIES.read().unwrap().clone()
}

/// The cookies a yt-dlp run with arguments `args` gets from the chosen browser: none when the
/// run has cookies already, else `--cookies-from-browser BROWSER[:PROFILE]`.
///
/// The browser's extension passes the cookies of the very page (or stream) the video was on,
/// from the window and profile it was in (a private window, a Firefox container), as the
/// browser sent them a moment ago (`FromBrowser`, written to the file `--cookies` names). Those
/// win: they are that session's, and reading the whole cookie store of a browser may give
/// another profile's, an older session's, or nothing at all (Chrome on Windows). The browser's
/// store is read only for non-private requests when nothing came: a pasted address, Add URL, a
/// playlist listed later. yt-dlp would merge the two if given both; it never is.
fn browser_cookie_args(args: &[String], chosen: Option<&BrowserCookies>) -> Vec<String> {
    let settled = args
        .iter()
        .any(|a| a == "--cookies" || a == "--cookies-from-browser" || a == NO_COOKIES);
    match chosen {
        Some(c) if !settled => vec!["--cookies-from-browser".into(), c.spec()],
        _ => Vec::new(),
    }
}

/// yt-dlp's "no cookies from any browser": a run told this gets none of the chosen browser's.
const NO_COOKIES: &str = "--no-cookies-from-browser";

/// `ours` for a run without cookies (#352, #686): the cookie file taken out, the browser's store
/// refused.
pub fn without_cookies(ours: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(ours.len() + 1);
    let mut args = ours.iter();
    while let Some(a) = args.next() {
        if a == "--cookies" || a == "--cookies-from-browser" {
            args.next();
        } else if a != NO_COOKIES {
            out.push(a.clone());
        }
    }
    out.push(NO_COOKIES.to_owned());
    out
}

/// Does a run with arguments `ours` have cookies (its own, or the chosen browser's)? One told
/// [`NO_COOKIES`] has none: the chosen browser's store isn't read for it.
fn with_cookies(ours: &[String], chosen: Option<&BrowserCookies>) -> bool {
    !ours.iter().any(|a| a == NO_COOKIES)
        && (chosen.is_some()
            || ours
                .iter()
                .any(|a| a == "--cookies" || a == "--cookies-from-browser"))
}

/// Does a run of yt-dlp with arguments `ours` send cookies: the browser's extension's (a cookie
/// file), or those of the browser Options → Sites Logins names?
pub fn has_cookies(ours: &[String]) -> bool {
    with_cookies(ours, browser_cookies().as_ref())
}

/// Is a look-up of `url` that failed with cookies (`message`) to be made once more without
/// them? Any site's that left no format to choose (#352), and YouTube's refusals of a signed-in
/// viewer, which a private window gets past (#686, [`crate::youtube::refused_signed_in`]): a
/// private or a members' video, which needs them, fails as it did.
fn refused_with_cookies(url: &str, message: &str) -> bool {
    crate::servers::format_unavailable(message)
        || (crate::youtube::is_youtube(url) && crate::youtube::refused_signed_in(message))
}

/// A page's `-J` output, whether it took looking the page up without cookies, and yt-dlp's
/// `WARNING:` lines on the way (#852: the one saying it has no JavaScript runtime).
pub struct LookedUp {
    pub json: Vec<u8>,
    pub without_cookies: bool,
    pub warnings: Vec<String>,
}

/// Look a page up (`yt-dlp -J`, `ours` holding its arguments). yt-dlp's `-J` chooses a format too,
/// and fails when none is left. Signed-in cookies make that likely on YouTube (#352): yt-dlp then
/// asks only the players that need a JavaScript runtime to decode their links, and without one
/// (it looks for Deno alone) every video is dropped: "Requested format is not available". The
/// same page without the cookies lists its qualities, so a look-up that fails this way and had
/// cookies is made once more without them, as is a YouTube look-up refused for its signed-in
/// viewer (#686: the bot check, a 403, a PO token; [`refused_with_cookies`]); if that fails too,
/// the first error is the one told.
/// A look-up whose secure connection failed for a certificate yt-dlp doesn't trust is made once
/// more with the system's certificates ([`with_system_certs`], #955).
///
/// Each try waits [`QUICK_SOCKET`] on a server first, then yt-dlp's own time ([`quick_first`]).
pub fn look_up(
    exe: &Path,
    ours: &[String],
    url: &str,
    timeout: Duration,
) -> Result<LookedUp, ProcError> {
    // A plain file a server's program sends gets its real type (#695).
    let run = |args: &[String]| {
        quick_first(args, |args| {
            output_warned(ytdlp(exe, &[], args, url)?, "yt-dlp", timeout)
                .map(|(json, warnings)| (crate::resolve::typed_scripts(json), warnings))
        })
    };
    match with_system_certs(&SYSTEM_CERTS, || run(ours)) {
        Ok((json, warnings)) => Ok(LookedUp {
            json,
            without_cookies: false,
            warnings,
        }),
        Err(first)
            if with_cookies(ours, browser_cookies().as_ref())
                && refused_with_cookies(url, &first.to_string()) =>
        {
            match run(&without_cookies(ours)) {
                Ok((json, warnings)) => {
                    tracing::info!(
                        "yt-dlp's look-up failed with the browser's cookies ({first}); without them, it worked"
                    );
                    Ok(LookedUp {
                        json,
                        without_cookies: true,
                        warnings,
                    })
                }
                Err(e) => {
                    tracing::info!(
                        "yt-dlp's look-up failed with the browser's cookies ({first}), and without them ({e})"
                    );
                    Err(first)
                }
            }
        }
        Err(e) => Err(e),
    }
}

/// yt-dlp checks the sites' certificates with the system's (`--compat-options no-certifi`)
/// rather than its own list, Mozilla's (#955). Set once a look-up needed it: every later run of
/// yt-dlp (the download, a fresh address, a playlist) needs it too.
static SYSTEM_CERTS: AtomicBool = AtomicBool::new(false);

/// yt-dlp's arguments for the certificates it checks sites with: its own list (Mozilla's, from
/// Python's `certifi`), or the system's (`system`). The system's is what MDM's own connections
/// and the browsers trust (rustls-platform-verifier, #668): there an antivirus's web filter or a
/// proxy that reads HTTPS adds the certificate it signs sites with, and yt-dlp's own list,
/// without it, said `CERTIFICATE_VERIFY_FAILED` for every page the browser opened (#955). Not
/// the default: Windows fetches a root certificate the first time a connection needs it, so its
/// store can lack one Mozilla's list has.
fn cert_args(system: bool) -> &'static [&'static str] {
    if system {
        &["--compat-options", "no-certifi"]
    } else {
        &[]
    }
}

/// Did yt-dlp fail for a certificate it doesn't trust (Python's `ssl` words, in yt-dlp's
/// message)? An expired or another site's certificate says so in other words.
pub fn untrusted_certificate(message: &str) -> bool {
    message.contains("CERTIFICATE_VERIFY_FAILED")
        && (message.contains("unable to get local issuer certificate")
            || message.contains("self-signed certificate in certificate chain")
            || message.contains("self signed certificate in certificate chain"))
}

/// `run` a yt-dlp run; when it failed for a certificate its own list doesn't trust, once more
/// with the system's (`system`, [`cert_args`]), kept for the runs after when that worked. A
/// second refusal of the certificate goes back to yt-dlp's own list and tells the first error;
/// another failure is the new run's (the certificate was fine).
fn with_system_certs<T>(
    system: &AtomicBool,
    mut run: impl FnMut() -> Result<T, ProcError>,
) -> Result<T, ProcError> {
    let first = match run() {
        Err(e) if !system.load(Ordering::Relaxed) && untrusted_certificate(&e.to_string()) => e,
        done => return done,
    };
    tracing::info!(
        "yt-dlp didn't trust the site's certificate: asking with the system's ({first})"
    );
    system.store(true, Ordering::Relaxed);
    match run() {
        Err(e) if untrusted_certificate(&e.to_string()) => {
            system.store(false, Ordering::Relaxed);
            Err(first)
        }
        done => done,
    }
}

/// How long a look-up's first try lets a server keep it waiting, in seconds (`--socket-timeout`;
/// yt-dlp's own is 20). yt-dlp tries a host's addresses one after the other, each for that long,
/// and a host the network drops without a word (#1053: YouTube's HLS manifest server, where a
/// network blocks it; a dead first address, as broken IPv6 gives) cost each look-up 20 s before
/// yt-dlp went on without it. A page's servers answer within a second when they answer.
pub const QUICK_SOCKET: &str = "5";

/// `run` with `args`, [`QUICK_SOCKET`] and no retries of yt-dlp's own, then, only when that try
/// failed on the network ([`network_failure`]: a server too slow for it among them), once more
/// with `args` alone: yt-dlp's own wait and retries, so a slow server still gets its time. The
/// first try retries nothing itself: yt-dlp retries a request that timed out up to 3 times, and a
/// server answering in 8 s cost 41 s before it gave up, against 11 s with them off (#1053). A
/// caller's own `--socket-timeout` or `--extractor-retries` is kept as it is, tried once.
pub(crate) fn quick_first<T>(
    args: &[String],
    mut run: impl FnMut(&[String]) -> Result<T, ProcError>,
) -> Result<T, ProcError> {
    if args
        .iter()
        .any(|a| a == "--socket-timeout" || a == "--extractor-retries")
    {
        return run(args);
    }
    let mut quick = args.to_vec();
    quick.extend(["--socket-timeout", QUICK_SOCKET, "--extractor-retries", "0"].map(str::to_owned));
    match run(&quick) {
        Err(ProcError::Failed { message, .. }) if network_failure(&message) => run(args),
        other => other,
    }
}

/// Does yt-dlp's error say the network failed it, as its own retries would have tried again: a
/// server that didn't answer in time ("Read timed out. (read timeout=5.0)", "Connection to …
/// timed out. (connect timeout=5.0)", "The read operation timed out"), a connection that broke,
/// or a server's error (HTTP 5xx)?
pub(crate) fn network_failure(message: &str) -> bool {
    // A certificate yt-dlp doesn't trust is no network trouble: `with_system_certs` asks again.
    if message.contains("CERTIFICATE_VERIFY_FAILED") {
        return false;
    }
    let m = message.to_ascii_lowercase();
    [
        "timed out",
        "read timeout",
        "connect timeout",
        "transporterror",
        "connection reset",
        "connection aborted",
        "remote end closed",
        "http error 5",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Did yt-dlp's look-up say YouTube's qualities went missing for want of a JavaScript runtime
/// (#852)? yt-dlp needs Deno, Node.js, QuickJS or Bun to solve YouTube's challenges; without one it asks
/// only the players that need none, and their list is short (on some days only the sound and a
/// low quality). Its own words, `warnings` as [`Spawned::warnings`] gives them:
/// - "No supported JavaScript runtime could be found. [...] some formats may be missing" (when
///   it looks a YouTube video up without one);
/// - "n challenge solving failed" / "Signature solving failed: Some formats may be missing"
///   (signed in, it asks the players that need one all the same): a runtime's want only when
///   this computer has none (`runtime_here`); otherwise its solver script is what failed.
pub fn missing_js_runtime(warnings: &[String], runtime_here: bool) -> bool {
    warnings.iter().any(|w| {
        let w = w.to_ascii_lowercase();
        w.contains("no supported javascript runtime")
            || (!runtime_here
                && (w.contains("challenge solving failed")
                    || w.contains("signature solving failed")))
    })
}

/// Has this computer a JavaScript runtime yt-dlp can use (Deno, Node.js, QuickJS or Bun), where
/// the helpers' runs look for one ([`js_runtime_found`])?
pub fn js_runtime_here(exe: &Path) -> bool {
    js_runtime_found(exe).is_some()
}

/// Does yt-dlp's `WARNING:` (or `ERROR:`) line say it couldn't read the browser's cookies?
/// Kept on the failure's message (`Spawned::failure`), where `lookup_error` names it.
pub fn cookie_trouble(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "cookie database",
        "cookies database",
        "decrypt",
        "unknown cookie version",
        "keyring",
        "secretstorage",
        "kwallet",
        "local state",
        "failed to load cookies",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Options -> Proxy / Socks for the helpers (#363): yt-dlp and gallery-dl go through the same proxy
/// as MDM's downloads. The app shares it as it changes ([`set_proxy`]).
static PROXY: std::sync::RwLock<mdm_engine::Proxy> =
    std::sync::RwLock::new(mdm_engine::Proxy::None);

/// The proxy every helper run goes through (the app's setting).
pub fn set_proxy(proxy: mdm_engine::Proxy) {
    if *PROXY.read().unwrap() != proxy {
        *PROXY.write().unwrap() = proxy;
    }
}

/// How the proxy for a helper's run is given to it. A login is never put on the command line,
/// where other programs can read it (the cookies neither): only in the environment.
#[derive(Debug, PartialEq, Eq)]
enum ProxyGiven {
    /// Nothing: yt-dlp keeps doing what it always did (the system's and the environment's).
    Nothing,
    /// `--proxy URL`.
    Argument(String),
    /// `HTTP_PROXY` and the like: the address with its login.
    Environment(String),
}

/// The proxy for `url` under `proxy`, as a helper takes it. `None` proxy and an address the
/// setting doesn't cover (Manual with its scheme unticked; a script that says DIRECT): nothing,
/// so the helper does what it did before. A script that gives no usable answer leaves it to the
/// system's settings, as for a download.
fn proxy_given(proxy: &mdm_engine::Proxy, url: Option<&url::Url>) -> ProxyGiven {
    use mdm_engine::{Proxy, pac};
    let system = |url: Option<&url::Url>| {
        // A helper's run with no address known yet (a page's looked-up info) asks for any.
        let any = url::Url::parse("https://example.com/").expect("a valid address");
        mdm_engine::net::system_proxy(url.unwrap_or(&any))
    };
    let plain = |p: Option<String>| p.map_or(ProxyGiven::Nothing, ProxyGiven::Argument);
    match proxy {
        Proxy::None => ProxyGiven::Nothing,
        Proxy::System => plain(system(url)),
        Proxy::Pac { script } => {
            let first = url.and_then(|u| {
                pac::routes(script, u)
                    .ok()?
                    .into_iter()
                    .next()
                    .map(|r| match r {
                        pac::Route::Direct => None,
                        pac::Route::Proxy(p) => Some(p),
                    })
            });
            match first {
                Some(Some(p)) => ProxyGiven::Argument(p),
                Some(None) => ProxyGiven::Nothing,
                None => plain(system(url)),
            }
        }
        Proxy::Manual {
            url: address,
            login,
            http,
            https,
        } => {
            let https_page = url.is_none_or(|u| u.scheme() == "https");
            if !(if https_page { *https } else { *http }) {
                return ProxyGiven::Nothing;
            }
            match login {
                None => ProxyGiven::Argument(address.clone()),
                Some((user, pass)) => {
                    let Ok(mut u) = url::Url::parse(address) else {
                        return ProxyGiven::Nothing;
                    };
                    let _ = u.set_username(user);
                    let _ = u.set_password(Some(pass));
                    ProxyGiven::Environment(u.to_string())
                }
            }
        }
    }
}

/// The scheme (`http`, `https`) of `url` that Options' manual proxy, set with an address, isn't
/// ticked for: a helper's run for it goes straight to the site.
fn manual_skips(proxy: &mdm_engine::Proxy, url: Option<&url::Url>) -> Option<&'static str> {
    let mdm_engine::Proxy::Manual {
        url: address,
        http,
        https,
        ..
    } = proxy
    else {
        return None;
    };
    let https_page = url.is_none_or(|u| u.scheme() == "https");
    let ticked = if https_page { *https } else { *http };
    (!ticked && !address.trim().is_empty()).then_some(if https_page { "https" } else { "http" })
}

/// Give `helper` the proxy for `url`: returns the arguments to pass (before `--`).
fn through_proxy(cmd: &mut Command, url: Option<&str>) -> Vec<String> {
    let page = url.and_then(|u| url::Url::parse(u).ok());
    let proxy = PROXY.read().unwrap().clone();
    // Said in MDM's log once for each setting (#1448: it went straight to YouTube unsaid).
    if let Some(scheme) = manual_skips(&proxy, page.as_ref()) {
        static SAID: Mutex<Option<mdm_engine::Proxy>> = Mutex::new(None);
        let mut said = SAID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if said.as_ref() != Some(&proxy) {
            tracing::info!(
                "Options → Proxy / Socks: the manual proxy isn't ticked for {scheme}, so yt-dlp and gallery-dl go straight to {scheme} addresses"
            );
            *said = Some(proxy.clone());
        }
    }
    match proxy_given(&proxy, page.as_ref()) {
        ProxyGiven::Nothing => Vec::new(),
        ProxyGiven::Argument(p) => vec!["--proxy".into(), p],
        ProxyGiven::Environment(p) => {
            for var in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                cmd.env(var, &p);
            }
            cmd.env_remove("NO_PROXY").env_remove("no_proxy");
            Vec::new()
        }
    }
}

/// The helpers' relay, started on first use.
fn relay() -> Result<Arc<crate::relay::Relay>, ProcError> {
    let mut slot = RELAY.lock().unwrap();
    if let Some(r) = slot.as_ref() {
        return Ok(r.clone());
    }
    let relay = crate::relay::Relay::start(Arc::new(|| (route(), dns())))
        .map(Arc::new)
        .map_err(|e| ProcError::Start("the network adapter's relay".into(), e))?;
    *slot = Some(relay.clone());
    Ok(relay)
}

/// Which helper [`through_adapter`] sets up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Helper {
    YtDlp,
    GalleryDl,
}

/// A helper and the chosen adapter (#35): nothing to do when there's none; refused while it is
/// down; else every connection the helper and what it starts make goes through the relay
/// (`crate::relay`), which holds them to the adapter. The arguments returned go before `--`.
///
/// - yt-dlp: `--proxy` for its own requests (and the external downloaders it passes it to), and
///   `--geo-verification-proxy`. The ffmpeg it starts gets the proxy from it (`http_proxy`), and
///   may only open what goes through it: HTTP(S) and the layers under them, local files, `data:`
///   and `crypto:` (`-protocol_whitelist`). RTMP and FTP formats go to that ffmpeg, which refuses
///   them, instead of rtmpdump or yt-dlp's own FTP, which would ignore the proxy; RTSP and MMS
///   formats (mpv or mplayer, in older yt-dlp) stop the download (`--break-match-filters`; `?`
///   lets an entry not yet chosen pass, a playlist's).
/// - gallery-dl: `--proxy`.
/// - Both, and anything they start: `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` (curl, aria2c,
///   gallery-dl's Python), with `NO_PROXY` taken away so no host is let around it.
fn through_adapter(
    cmd: &mut Command,
    helper: Helper,
    url: Option<&str>,
) -> Result<Vec<String>, ProcError> {
    match route() {
        // No adapter: Options -> Proxy / Socks (#363).
        mdm_engine::Route::Any => Ok(through_proxy(cmd, url)),
        mdm_engine::Route::Down { label } => Err(mdm_engine::NetError::Down { label }.into()),
        mdm_engine::Route::Bound(_) => {
            let url = relay()?.url();
            for var in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                cmd.env(var, &url);
            }
            cmd.env_remove("NO_PROXY").env_remove("no_proxy");
            Ok(match helper {
                Helper::YtDlp => vec![
                    "--proxy".into(),
                    url.clone(),
                    "--geo-verification-proxy".into(),
                    url,
                    // yt-dlp's keys are http, ftp, m3u8, dash and rtmp only: anything else
                    // ("rtsp:ffmpeg") would make ffmpeg the downloader for everything.
                    "--downloader".into(),
                    "rtmp:ffmpeg".into(),
                    "--downloader".into(),
                    "ftp:ffmpeg".into(),
                    "--downloader-args".into(),
                    format!("ffmpeg_i:-protocol_whitelist {FFMPEG_PROTOCOLS}"),
                    "--break-match-filters".into(),
                    "protocol!*=?rtsp & protocol!*=?mms".into(),
                ],
                Helper::GalleryDl => vec!["--proxy".into(), url],
            })
        }
    }
}

/// What the ffmpeg yt-dlp starts may open while an adapter is chosen: HTTP(S) through the relay
/// (`httpproxy`, `tls` and `tcp` are the layers under them; HLS and DASH open nothing else) and
/// what stays on this computer.
const FFMPEG_PROTOCOLS: &str = "file,crypto,data,http,https,tls,tcp,httpproxy";

/// `gallery-dl` (#58) with arguments in the same layers as [`ytdlp`]: `ours`, then the fixed
/// ones (no configuration file of the person's, never a question on the console), then `--` and
/// the address. Its output is read as UTF-8, as yt-dlp's is (#79). With an adapter chosen it goes
/// through the relay (#35, [`through_adapter`]). Only options `tool`'s version accepts
/// (`gallery::Accepts`, #236: a distribution's may be older than MDM's own copy).
pub fn gallery_dl(
    tool: &crate::Installed,
    ours: &[String],
    url: &str,
) -> Result<Command, ProcError> {
    let mut cmd = helper_command(&tool.exe);
    let route = through_adapter(&mut cmd, Helper::GalleryDl, Some(url))?;
    for a in ours {
        cmd.arg(safe(a)?);
    }
    cmd.arg("--ignore-config");
    if crate::gallery::Accepts::of(&tool.version).no_input {
        cmd.arg("--no-input");
    }
    cmd.args(route);
    cmd.arg("--");
    cmd.arg(safe(url)?);
    Ok(cmd)
}

/// Text that may go into an argument list: no line breaks or NUL (they would let a page title
/// or URL smuggle in options or split a line-based protocol).
pub fn safe(s: &str) -> Result<&str, ProcError> {
    if s.contains(['\r', '\n', '\0']) {
        Err(ProcError::BadArgument)
    } else {
        Ok(s)
    }
}

/// What the browser's request for a page carried, so yt-dlp asks as the browser did: the page
/// that embeds a player (Referer), the browser's User-Agent, and the page's cookies, which sign a
/// member's or a site's own videos in: each with its domain and flags (`jar`), or, from an older
/// extension, a Cookie header's `name=value; …` (`cookies`). For a stream the page's player
/// asked for, the `Origin` it sent too (#900): a site's video server may serve its own pages'
/// players only.
#[derive(Clone, Debug, Default)]
pub struct FromBrowser {
    /// A private window's request, including a kept row after restart: never read the normal
    /// browser's cookie store. Cookies explicitly passed with this request remain usable.
    pub private: bool,
    pub referer: Option<String>,
    pub origin: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub jar: Vec<mdm_ipc::BrowserCookie>,
}

/// A private cookie file in the Netscape format yt-dlp's `--cookies` reads, removed when
/// dropped (yt-dlp may write it back at its end: keep it until then).
pub struct CookieFile(PathBuf);

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl FromBrowser {
    /// yt-dlp's arguments for `url`'s page, and the cookie file they name. Cookies never go on
    /// the command line, where other programs can read them.
    pub fn args(&self, url: &str) -> Result<(Vec<String>, Option<CookieFile>), ProcError> {
        fn given(s: &Option<String>) -> Option<&str> {
            s.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        let mut args = Vec::new();
        if let Some(r) = given(&self.referer) {
            args.extend(["--referer".to_owned(), safe(r)?.to_owned()]);
        }
        if let Some(ua) = given(&self.user_agent) {
            args.extend(["--user-agent".to_owned(), safe(ua)?.to_owned()]);
        }
        // yt-dlp sends it with the address and keeps it in each format's headers, which MDM's
        // engine sends with the stream's playlists and each of its pieces (#769).
        if let Some(origin) = given(&self.origin) {
            args.extend([
                "--add-header".to_owned(),
                format!("Origin:{}", safe(origin)?),
            ]);
        }
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        let text = match (given(&self.cookies), host) {
            _ if !self.jar.is_empty() => Some(jar_cookies(&self.jar)),
            (Some(cookies), Some(host)) => Some(header_cookies(&host, cookies)),
            _ => None,
        };
        let file = match text {
            Some(text) => {
                let file = cookie_file(&text).map_err(|e| ProcError::Start("yt-dlp".into(), e))?;
                args.extend([
                    "--cookies".to_owned(),
                    file.0.to_string_lossy().into_owned(),
                ]);
                Some(file)
            }
            _ => {
                if self.private {
                    args.push(NO_COOKIES.to_owned());
                }
                None
            }
        };
        Ok((args, file))
    }
}

/// The first line of a cookie file in the Netscape format (yt-dlp's `--cookies`). Each cookie is
/// a line: domain, sent to its subdomains too (TRUE, FALSE), path, HTTPS only (TRUE, FALSE),
/// expiry (seconds since 1970; 0: a session cookie), name, value, separated by tabs.
const NETSCAPE: &str = "# Netscape HTTP Cookie File\n";

/// Can `s` go in a cookie file's field (no tab or line break to shift the others)?
fn field(s: &str) -> bool {
    !s.contains(['\t', '\r', '\n', '\0'])
}

/// The browser's cookies (`cookies.getAll`'s), each for its own domain, path and flags: yt-dlp
/// sends a cookie of `.youtube.com` to www.youtube.com when the page was music.youtube.com, and
/// one of the page's host alone to that host only.
fn jar_cookies(jar: &[mdm_ipc::BrowserCookie]) -> String {
    let flag = |b: bool| if b { "TRUE" } else { "FALSE" };
    let mut text = String::from(NETSCAPE);
    for c in jar {
        let host = c.domain.trim_start_matches('.');
        let usable = !c.name.is_empty()
            && !host.is_empty()
            && !host.contains([' ', '/'])
            && [&c.name, &c.value, &c.domain, &c.path]
                .iter()
                .all(|s| field(s));
        if !usable {
            continue;
        }
        let domain = if c.host_only {
            host.to_owned()
        } else {
            format!(".{host}")
        };
        let path = if c.path.starts_with('/') {
            c.path.as_str()
        } else {
            "/"
        };
        let expires = c
            .expires
            .filter(|e| e.is_finite() && *e >= 1.0)
            .map_or(0, |e| e as u64);
        text.push_str(&format!(
            "{domain}\t{}\t{path}\t{}\t{expires}\t{}\t{}\n",
            flag(!c.host_only),
            flag(c.secure),
            c.name,
            c.value
        ));
    }
    text
}

/// An older extension's Cookie header (`a=1; b=2`, the page's cookies without their domains),
/// for the site's domain and its subdomains ([`header_domain`]), as session cookies.
fn header_cookies(host: &str, cookies: &str) -> String {
    let domain = header_domain(host);
    let mut text = String::from(NETSCAPE);
    for pair in cookies.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        if ![domain, name, value].iter().all(|s| field(s)) {
            continue;
        }
        text.push_str(&format!(".{domain}\tTRUE\t/\tFALSE\t0\t{name}\t{value}\n"));
    }
    text
}

/// Where a page's cookies without their domains go: its host, or the site's own domain where
/// yt-dlp asks another of its hosts. Every YouTube host (music., m., www.: yt-dlp looks videos
/// up on www.youtube.com, and the sign-in cookies are `.youtube.com`'s), and a site's mobile
/// pages (m., mobile.), whose sign-in is the site's.
fn header_domain(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    let youtube = "youtube.com";
    let lower = host.to_ascii_lowercase();
    if lower == youtube || lower.ends_with(".youtube.com") {
        return &host[host.len() - youtube.len()..];
    }
    for mobile in ["m.", "mobile."] {
        if let Some(site) = host.strip_prefix(mobile)
            && site.contains('.')
        {
            return site;
        }
    }
    host
}

/// A cookie file's `text` ([`jar_cookies`], [`header_cookies`]) in a file only this user can
/// read, in the helpers' temporary folder ([`crate::workdir::temp_dir`]).
fn cookie_file(text: &str) -> std::io::Result<CookieFile> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let path = crate::workdir::temp_dir().join(format!(
        "mdm-cookies-{}-{}.txt",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    // The temporary folder is the user's own on Windows; elsewhere it may be shared.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let file = CookieFile(path);
    open.open(&file.0)?.write_all(text.as_bytes())?;
    Ok(file)
}

/// What yt-dlp prints is read as UTF-8 (#79): on a Windows with a legacy code page (Turkish,
/// Russian…) Python writes to a pipe in that code page, and a title's letters made the
/// final-path line unreadable. `--encoding` sets yt-dlp's own output, `PYTHONIOENCODING`
/// Python's.
const UTF8: [&str; 2] = ["--encoding", "utf-8"];

/// yt-dlp's or gallery-dl's program, built as it will run: output in UTF-8 ([`UTF8`]), and in
/// the helpers' folder with it as TEMP, TMP and TMPDIR ([`crate::workdir`]), never MDM's own
/// working folder, which is whatever started MDM gave it: the install folder from the Start
/// menu or the browser (#834: Chrome starts its host in the host's folder), and
/// `C:\Windows\System32` when Windows starts it with the session (#1216). On Windows yt-dlp
/// makes the small file it tries a format with (a YouTube video put online in the last two
/// days, a stream marked as protected) in its working folder: its `get_output_path('temp')` is
/// `.` there when MDM gives no `-P temp:`. [`spawn`] gives the same to any other program.
fn helper_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env("PYTHONIOENCODING", "utf-8");
    crate::workdir::apply(&mut cmd, crate::workdir::folders());
    cmd
}

/// The user's extras and ours, then the chosen browser's cookies when the run has none
/// ([`browser_cookie_args`]). Reading them, yt-dlp's warnings stay on (a look-up's
/// `--no-warnings` is dropped): the one saying the browser's cookies couldn't be decrypted is
/// what explains the site's "sign in" that follows (`Spawned::failure`).
fn layers(cmd: &mut Command, user: &[String], ours: &[String]) -> Result<(), ProcError> {
    layers_with(cmd, user, ours, browser_cookies().as_ref())
}

fn layers_with(
    cmd: &mut Command,
    user: &[String],
    ours: &[String],
    chosen: Option<&BrowserCookies>,
) -> Result<(), ProcError> {
    let all: Vec<String> = user.iter().chain(ours).cloned().collect();
    let from_store = browser_cookie_args(&all, chosen);
    for a in &all {
        if from_store.is_empty() || a != "--no-warnings" {
            cmd.arg(safe(a)?);
        }
    }
    for a in &from_store {
        cmd.arg(safe(a)?);
    }
    Ok(())
}

/// The file named `name` (`name.exe` on Windows) in one of `dirs`.
fn program_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    file_in(dirs, &format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// `file` in one of `dirs`, the absolute ones only: a relative PATH entry (`.`, `bin`) is a
/// folder under wherever MDM was started, so a program found there is one anybody who could
/// put a file in that folder chose.
fn file_in(dirs: &[PathBuf], file: &str) -> Option<PathBuf> {
    dirs.iter()
        .filter(|d| d.is_absolute())
        .map(|d| d.join(file))
        .find(|p| p.is_file())
}

/// Node.js from nvm, every version, the newest first: a program started from the desktop doesn't
/// have nvm's folders on its PATH (#360). Unix nvm keeps them in `~/.nvm/versions/node/<v>/bin`;
/// nvm-windows in `%APPDATA%\nvm\<v>`.
fn nvm_nodes(home: &Path) -> Vec<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .map(|entries| entries.flatten().map(|e| e.path().join("bin")).collect())
        .unwrap_or_default();
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from)
        && let Ok(entries) = std::fs::read_dir(appdata.join("nvm"))
    {
        versions.extend(entries.flatten().map(|e| e.path()));
    }
    // By their numbers (`v9` before `v22`), from the folder named after the version.
    let numbers = |p: &PathBuf| -> Vec<u64> {
        let folder = if p.ends_with("bin") {
            p.parent()
        } else {
            Some(p.as_path())
        };
        folder
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().trim_start_matches('v').to_owned())
            .unwrap_or_default()
            .split('.')
            .filter_map(|n| n.parse().ok())
            .collect()
    };
    versions.sort_by_key(|p| std::cmp::Reverse(numbers(p)));
    versions
        .iter()
        .filter_map(|v| program_in(std::slice::from_ref(v), "node"))
        .collect()
}

/// The JavaScript runtimes on this computer yt-dlp could use (#360, #352, #351, #852, #1570), in
/// yt-dlp's order of priority (its README: deno, node, quickjs, bun), each kind where it is
/// looked for in turn: Deno on the PATH (`dirs`), where its own installer puts it
/// (`~/.deno/bin`: no PATH of a program started from the desktop has it), or in `more` (folders a
/// runtime installed after MDM started is in, [`runtime_dirs`], which on Windows also carries
/// `~/.deno/bin` and `~/.bun/bin`); Node.js on the PATH, under nvm (newest first), or in `more`;
/// QuickJS (`qjs`, the executable must be named so); Bun, which yt-dlp deprecated, last.
/// Whether each is a version yt-dlp can use is `jsruntime`'s to ask.
fn js_runtimes_in(
    dirs: &[PathBuf],
    more: &[PathBuf],
    home: Option<&Path>,
) -> Vec<(crate::jsruntime::Kind, PathBuf)> {
    use crate::jsruntime::Kind;
    let own_deno = home.map(|h| h.join(".deno").join("bin"));
    let mut found: Vec<(Kind, PathBuf)> = Vec::new();
    let mut add = |kind: Kind, path: Option<PathBuf>| {
        if let Some(path) = path
            && !found.iter().any(|(_, p)| *p == path)
        {
            found.push((kind, path));
        }
    };
    add(Kind::Deno, program_in(dirs, "deno"));
    add(Kind::Deno, program_in(own_deno.as_slice(), "deno"));
    add(Kind::Deno, program_in(more, "deno"));
    add(Kind::Node, program_in(dirs, "node"));
    for node in home.map(nvm_nodes).unwrap_or_default() {
        add(Kind::Node, Some(node));
    }
    add(Kind::Node, program_in(more, "node"));
    add(Kind::QuickJs, program_in(dirs, "qjs"));
    add(Kind::QuickJs, program_in(more, "qjs"));
    add(Kind::Bun, program_in(dirs, "bun"));
    add(Kind::Bun, program_in(more, "bun"));
    found
}

/// Where installers put a JavaScript runtime (#852): winget's links (Deno, Node.js) and Node.js's
/// own installer. They add these to the PATH of programs started after them, not to MDM's own,
/// so a runtime installed while MDM runs is found here when the video is looked up again.
/// None elsewhere: a package manager's folder is on the PATH already ([`usable_dirs`]).
fn runtime_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut dirs = crate::system::extra_dirs();
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            dirs.push(Path::new(&programs).join("nodejs"));
        }
        dirs
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// The folders of `path` (a PATH value) a helper can walk, in their order (#508): yt-dlp looks
/// through the PATH folders, and one Windows refused to open ("[WinError 448] The path cannot be
/// traversed because it contains an untrusted mount point": a junction Redirection Guard
/// doesn't trust) ended the look-up. A folder MDM can't open (gone, refused, not a folder) is
/// left out; one reached through a junction or link is given as the folder it leads to, so the
/// helper crosses no mount point on its way, even when Windows holds the helper to stricter
/// rules than MDM. Every other folder stays as it was written.
///
/// On Linux and macOS the package managers' folders follow, when the PATH lacks them (#659): a
/// helper found there ([`crate::system`]) looks there for its own tools too (yt-dlp for Deno
/// and ffmpeg), even when MDM was opened from Finder, whose PATH has no Homebrew.
fn usable_dirs(path: &std::ffi::OsStr) -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path)
        .filter_map(|dir| usable_dir(&dir))
        .collect();
    #[cfg(unix)]
    for extra in crate::system::PACKAGE_DIRS.map(PathBuf::from) {
        if !dirs.contains(&extra)
            && let Some(dir) = usable_dir(&extra)
        {
            dirs.push(dir);
        }
    }
    dirs
}

fn usable_dir(dir: &Path) -> Option<PathBuf> {
    if !std::fs::metadata(dir).ok()?.is_dir() {
        return None;
    }
    // Only Windows refuses mount points (Redirection Guard); elsewhere a link is just followed.
    #[cfg(windows)]
    if dir.is_absolute()
        && dir.ancestors().any(reparse_point)
        && let Ok(real) = std::fs::canonicalize(dir)
    {
        return Some(without_verbatim(real));
    }
    Some(dir.to_owned())
}

/// A junction, a link or a mounted volume's folder.
#[cfg(windows)]
fn reparse_point(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(p)
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// `\\?\C:\x` as `C:\x` and `\\?\UNC\server\share` as `\\server\share`, the way PATH writes
/// them (`canonicalize` gives the long forms).
#[cfg(windows)]
fn without_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.clone(),
    }
}

/// The PATH a helper gets ([`usable_dirs`] of `path`); `None` when it can't be written back
/// (then it's left as it was).
fn child_path(path: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    std::env::join_paths(usable_dirs(path)).ok()
}

/// Give `cmd` the PATH of [`child_path`]: of the one it was given, else of MDM's own. Every
/// helper starts here ([`spawn`]), and ffmpeg, started by yt-dlp, inherits it.
fn with_usable_path(cmd: &mut Command) {
    let is_path = |k: &std::ffi::OsStr| {
        if cfg!(windows) {
            k.eq_ignore_ascii_case("PATH")
        } else {
            k == "PATH"
        }
    };
    let path = match cmd.get_envs().find(|(k, _)| is_path(k)) {
        Some((_, Some(given))) => given.to_owned(),
        Some((_, None)) => return,
        None => match std::env::var_os("PATH") {
            Some(own) => own,
            None => return,
        },
    };
    if let Some(usable) = child_path(&path) {
        cmd.env("PATH", usable);
    }
}

/// The folders of MDM's PATH a helper gets ([`usable_dirs`]: a node.exe behind a junction is
/// named by its real folder).
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| usable_dirs(&p))
        .unwrap_or_default()
}

/// The person's home folder (`HOME`, else Windows' `USERPROFILE`).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The JavaScript runtimes on this computer for the yt-dlp at `ytdlp`, MDM's own aside
/// ([`js_runtimes_in`]): in the PATH folders a helper gets ([`path_dirs`]), and beside MDM's own
/// yt-dlp (per the EJS guide, a runtime beside `yt-dlp.exe` counts too; only MDM's, never a
/// folder a chosen program sits in, where a node.exe nobody meant could land and be run).
/// Looked for afresh, so one installed meanwhile counts at the next look-up (#852).
pub fn system_js_runtimes(ytdlp: Option<&Path>) -> Vec<(crate::jsruntime::Kind, PathBuf)> {
    let mut dirs = path_dirs();
    if let (Some(tools), Some(exe)) = (crate::tools_dir(), ytdlp)
        && exe.starts_with(&tools)
        && let Some(beside) = exe.parent()
    {
        dirs.push(beside.to_path_buf());
    }
    js_runtimes_in(&dirs, &runtime_dirs(), home_dir().as_deref())
}

/// The arguments that tell the yt-dlp at `exe` of its JavaScript runtime (`jsruntime::current`:
/// the person's choice, the computer's own, MDM's own), when there is one. Read at each run, so
/// the one Helpers just installed or chose is used from the next look-up on.
fn js_runtime_found(exe: &Path) -> Option<Vec<std::ffi::OsString>> {
    let dir = crate::tools_dir();
    // This crate's tests build yt-dlp's commands by the hundred, in parallel: they don't run
    // the test computer's own Node.js or Deno for it (what they'd find depends on that
    // computer; `jsruntime`'s own tests check the choice), only what is known without asking.
    #[cfg(test)]
    let found = crate::jsruntime::current_now(dir.as_deref(), Some(exe)).flatten();
    #[cfg(not(test))]
    let found = crate::jsruntime::current(dir.as_deref(), Some(exe));
    found.map(|r| r.args())
}

/// The first yt-dlp that has `--js-runtimes` (2025.11.12, when YouTube came to need a
/// JavaScript runtime): an older one stops at an option it doesn't know, on every site.
const JS_RUNTIMES_SINCE: &str = "2025.11.12";

/// Whether yt-dlp `version` has `--js-runtimes` ([`JS_RUNTIMES_SINCE`]); a version that isn't a
/// date (a build of the person's own) is taken to have it, as before.
fn knows_js_runtimes(version: &str) -> bool {
    let dated = version
        .split('.')
        .filter(|p| p.parse::<u64>().is_ok())
        .count()
        >= 3;
    !dated || !crate::newer(JS_RUNTIMES_SINCE, version)
}

/// Whether the yt-dlp at `exe` has `--js-runtimes`: MDM's own and a chosen one by the version
/// they were installed or chosen with; the system's is asked (`--version`), once a session.
/// Not known (it didn't answer): taken to have it, as before.
fn takes_js_runtimes(exe: &Path) -> bool {
    static KNOWN: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());
    let known = KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(p, _)| p == exe)
        .map(|(_, t)| *t);
    if let Some(takes) = known {
        return takes;
    }
    let version = crate::tools_dir()
        .as_deref()
        .and_then(crate::installed)
        .filter(|i| i.exe == exe && !i.is_system())
        .map(|i| i.version)
        .or_else(|| {
            crate::chosen::ask(crate::chosen::Helper::YtDlp, exe)
                .ok()
                .map(|i| i.version)
        });
    let takes = version.as_deref().is_none_or(knows_js_runtimes);
    if !takes {
        tracing::warn!(
            "yt-dlp {} is older than {JS_RUNTIMES_SINCE}: not told of a JavaScript runtime",
            version.unwrap_or_default()
        );
    }
    KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((exe.to_owned(), takes));
    takes
}

/// `--js-runtimes` for this computer's runtime, when yt-dlp must be told of it and knows how to
/// hear it ([`takes_js_runtimes`]). Said in MDM's log each time it changes.
fn js_runtime_args(exe: &Path) -> Vec<std::ffi::OsString> {
    let args = js_runtime_found(exe).unwrap_or_default();
    if args.is_empty() || !takes_js_runtimes(exe) {
        return Vec::new();
    }
    static SAID: Mutex<Vec<std::ffi::OsString>> = Mutex::new(Vec::new());
    let mut said = SAID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *said != args {
        let shown: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        tracing::info!("yt-dlp's JavaScript runtime: {}", shown.join(" "));
        said.clone_from(&args);
    }
    args
}

/// Options → Sites Logins → Use my yt-dlp config file: off, the default.
static USER_CONFIG: AtomicBool = AtomicBool::new(false);

/// Let every later yt-dlp run read the person's own config file (or not, the default).
pub fn set_user_config(on: bool) {
    USER_CONFIG.store(on, Ordering::Relaxed);
}

/// With the person's own config file read (yt-dlp reads it before every argument), what MDM
/// can't leave to it, placed before MDM's own arguments so those still win: yt-dlp's own
/// downloader (an external one, aria2c, prints no progress lines, and one named for HLS skips
/// the fix-up of what the engine joined; live HLS still goes to ffmpeg, live DASH is left out
/// as `native` would keep it from ffmpeg), no download archive (it skips a look-up of a video
/// once downloaded), no forced overwrites (it implies `--no-continue`, deleting the engine's
/// streams), the default fix-ups, and no marking as watched (`--mark-watched` would mark every
/// look-up, the quality menu's included, not only what is downloaded), and part files (yt-dlp
/// reuses the streams the engine downloaded only with `--continue` and `--part`, so `--no-part`
/// would fetch every one again into a doubled file). Nothing when the config isn't read
/// (`--ignore-config`).
fn config_args(on: bool) -> &'static [&'static str] {
    if on {
        &[
            "--downloader",
            "http,ftp,m3u8:native",
            "--no-download-archive",
            "--no-force-overwrites",
            "--fixup",
            "detect_or_warn",
            "--no-mark-watched",
            "--part",
        ]
    } else {
        &[]
    }
}

/// `yt-dlp` with arguments in layers: `user` extras, then `ours` (then the chosen browser's
/// cookies when neither brought any, [`layers`]), then the fixed ones last (so nothing can
/// switch them off), then `--` and the URL (so a URL starting with `-` is never read as an
/// option).
pub fn ytdlp(
    exe: &Path,
    user: &[String],
    ours: &[String],
    url: &str,
) -> Result<Command, ProcError> {
    let mut cmd = helper_command(exe);
    let route = through_adapter(&mut cmd, Helper::YtDlp, Some(url))?;
    let own_config = USER_CONFIG.load(Ordering::Relaxed);
    cmd.args(config_args(own_config));
    layers(&mut cmd, user, ours)?;
    if !own_config {
        cmd.arg("--ignore-config");
    }
    cmd.args(["--no-exec", "--no-update"]);
    cmd.args(js_runtime_args(exe));
    cmd.args(cert_args(SYSTEM_CERTS.load(Ordering::Relaxed)));
    cmd.args(UTF8);
    cmd.args(route);
    cmd.arg("--");
    cmd.arg(safe(url)?);
    Ok(cmd)
}

/// `yt-dlp` working from a looked-up page (`-J` output, maybe edited) instead of an address:
/// the same layers, then `--load-info-json` and the file. `url` is the page's address, which
/// the proxy is chosen for as for its look-up ([`through_proxy`]; #1448: Options' automatic
/// configuration script was asked about no address, and yt-dlp's download went around it).
pub fn ytdlp_info(
    exe: &Path,
    user: &[String],
    ours: &[String],
    info: &Path,
    url: &str,
) -> Result<Command, ProcError> {
    let mut cmd = helper_command(exe);
    let route = through_adapter(&mut cmd, Helper::YtDlp, Some(url))?;
    let own_config = USER_CONFIG.load(Ordering::Relaxed);
    cmd.args(config_args(own_config));
    layers(&mut cmd, user, ours)?;
    if !own_config {
        cmd.arg("--ignore-config");
    }
    cmd.args(["--no-exec", "--no-update"]);
    cmd.args(js_runtime_args(exe));
    cmd.args(cert_args(SYSTEM_CERTS.load(Ordering::Relaxed)));
    cmd.args(UTF8);
    cmd.args(route);
    cmd.arg("--load-info-json");
    cmd.arg(info);
    Ok(cmd)
}

/// A job of its own for one helper and every process it starts (#61): on Windows `yt-dlp.exe`
/// is a launcher whose child does the work, and ffmpeg runs under that child. Closing the job
/// (`Spawned::kill`, or MDM ending in any way) ends them all; ending the launcher alone left the
/// download running, holding its file and the bandwidth.
#[cfg(windows)]
fn new_job() -> Option<win32job::Job> {
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    win32job::Job::create_with_limit_info(&info).ok()
}

/// The helpers that run (#1301), and whether MDM is ending ([`end_all`]): after that, no helper
/// starts.
struct Running {
    ended: bool,
    /// Linux, macOS: each helper's process group (its first process's id, [`spawn`]) with its
    /// `stderr_done`; dropped with the helper.
    #[cfg(unix)]
    groups: Vec<(u32, Arc<AtomicBool>)>,
}

static RUNNING: Mutex<Running> = Mutex::new(Running {
    ended: false,
    #[cfg(unix)]
    groups: Vec::new(),
});

/// MDM ends (an Exit, the session's end, a command line's Ctrl+C or SIGTERM): every helper that
/// runs ends with all it started, and none starts after. Windows' job objects end them with MDM
/// in any case; on Linux and macOS each helper's process group is killed here, as nothing ends it
/// when MDM's process goes (#1301).
pub fn end_all() {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    running.ended = true;
    #[cfg(unix)]
    for (group, stderr_done) in running.groups.drain(..) {
        // Something still holds the helper's stderr: the group is there, its id not reused.
        if !stderr_done.load(Ordering::Acquire) {
            kill_group(group);
        }
    }
}

/// SIGKILL to the process group `group` (#1301): yt-dlp's one-file build on Linux
/// (`yt_dlp_linux`), and gallery-dl's, are a launcher whose child runs Python and downloads, its
/// ffmpeg under it; killing the launcher alone left them downloading. A kill at once, as Windows'
/// job does: what the stopped run left (`.part` files) is continued by the next as it is.
#[cfg(unix)]
fn kill_group(group: u32) {
    if let Some(pid) = i32::try_from(group)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        // Never "every process MDM may signal" (`kill(-1)`).
        .filter(|pid| *pid != rustix::process::Pid::INIT)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// A running helper with its stderr tail.
pub struct Spawned {
    pub child: Child,
    tail: Arc<Mutex<VecDeque<String>>>,
    /// Every `WARNING:` line (a few dozen at most): a subtitle yt-dlp couldn't get is said early,
    /// and the tail has moved past it by the end (#68).
    warnings: Arc<Mutex<Vec<String>>>,
    /// The stderr reader has read to the end (the helper and all it started closed it).
    stderr_done: Arc<std::sync::atomic::AtomicBool>,
    name: String,
    #[cfg(windows)]
    job: Option<win32job::Job>,
    /// MDM's own helpers it runs (its program, the ffmpeg and Deno it is given), not switched to
    /// a new version while it runs (#1544). Last: let go once it has ended.
    _hold: crate::autoupdate::Hold,
}

impl Spawned {
    /// End the helper and everything it started: on Windows its job, on Linux and macOS its
    /// process group (#1301: `yt_dlp_linux` is a launcher whose child downloads, as `yt-dlp.exe`
    /// is, #61). A helper dropped ends the same way, as its job closes on Windows.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        self.kill_group();
        let _ = self.child.kill();
        #[cfg(windows)]
        drop(self.job.take());
    }

    /// The helper's process group killed, unless all of it is known to have ended: its first
    /// process reaped and its stderr closed by every process that held it. Until then the
    /// group's id (that first process's) can't be another's.
    #[cfg(unix)]
    fn kill_group(&mut self) {
        let closed = self.stderr_done.load(std::sync::atomic::Ordering::Acquire);
        if !(closed && matches!(self.child.try_wait(), Ok(Some(_)))) {
            kill_group(self.child.id());
        }
    }

    /// The last stderr lines, for an error message.
    pub fn tail(&self) -> String {
        self.tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tool's `WARNING:` lines so far, oldest first, without the word.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }

    /// Once it has ended: wait for its last stderr lines (the reader is another thread, and on
    /// a busy machine it comes after the exit was seen), up to two seconds, as something it
    /// started may keep stderr open.
    fn settle(&self) {
        let end = Instant::now() + Duration::from_secs(2);
        while !self.stderr_done.load(std::sync::atomic::Ordering::Acquire) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A failure with what the tool said (its last `ERROR:` line, else its last line). Called
    /// once it has ended: its last lines are waited for (the stderr reader is another thread, and
    /// on a busy machine it came after the exit was seen: "stopped without saying why"), up to
    /// two seconds, as something it started may keep stderr open.
    pub fn failure(&self) -> ProcError {
        self.settle();
        let tail = self.tail.lock().unwrap();
        let message = tail
            .iter()
            .rev()
            .find(|l| l.starts_with("ERROR:"))
            .or(tail.back())
            .map(|l| l.trim_start_matches("ERROR:").trim().to_owned())
            .unwrap_or_else(|| "it stopped without saying why".into());
        let warnings = self.warnings.lock().unwrap();
        ProcError::Failed {
            tool: self.name.clone(),
            message: with_cookie_trouble(message, &warnings, &tail),
        }
    }
}

impl Drop for Spawned {
    /// Linux, macOS: what still runs of the helper ends, as Windows' job ends it when closed
    /// (`job`), and [`end_all`] no longer has it to end.
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.kill_group();
            let _ = self.child.kill();
            let _ = self.child.try_wait();
            let id = self.child.id();
            RUNNING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .groups
                .retain(|(group, _)| *group != id);
        }
    }
}

/// A failure's `message` with what yt-dlp said earlier about the browser's cookies, when that
/// isn't the message itself (#229): Chrome's cookies it couldn't decrypt are only a warning,
/// and the site's "sign in" that follows is the error; "failed to load cookies" comes after the
/// line saying why.
fn with_cookie_trouble(message: String, warnings: &[String], tail: &VecDeque<String>) -> String {
    let earlier = warnings
        .iter()
        .map(String::as_str)
        .chain(tail.iter().map(|l| {
            l.trim_start_matches("ERROR:")
                .trim_start_matches("WARNING:")
                .trim()
        }))
        .find(|l| cookie_trouble(l) && *l != message && !message.contains(*l));
    match earlier {
        Some(why) => format!("{message} ({why})"),
        None => message,
    }
}

/// Start `cmd` supervised: no window, in the job, stdout piped for the caller, stderr kept as a
/// tail, with a PATH it can walk ([`with_usable_path`], #508), in a folder it can write that is
/// also its TEMP ([`crate::workdir`], #834: never MDM's install folder).
pub fn spawn(mut cmd: Command, name: &str) -> Result<Spawned, ProcError> {
    with_usable_path(&mut cmd);
    crate::workdir::apply(&mut cmd, crate::workdir::folders());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // A process group of its own, which `kill` and MDM's end kill whole (#1301). Not MDM's: a
    // Ctrl+C in MDM's terminal no longer reaches it, MDM ends it instead (`end_all`).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // The helpers it runs, held before it starts (#1544).
    let hold = crate::autoupdate::Hold::of_command(&cmd);
    // Under the lock: `end_all` either came first (nothing starts) or finds it to end.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.ended {
        return Err(ProcError::Start(
            name.to_owned(),
            std::io::Error::other("Mister Download Manager is closing"),
        ));
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ProcError::Start(name.to_owned(), e))?;
    #[cfg(windows)]
    let job = new_job().filter(|job| {
        use std::os::windows::io::AsRawHandle;
        job.assign_process(child.as_raw_handle() as isize).is_ok()
    });
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let stderr_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(err) = child.stderr.take() {
        let (tail, warnings, done) = (tail.clone(), warnings.clone(), stderr_done.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(w) = line.strip_prefix("WARNING:") {
                    let mut ws = warnings.lock().unwrap();
                    if ws.len() < WARNINGS_KEPT {
                        ws.push(w.trim().to_owned());
                    }
                }
                let mut t = tail.lock().unwrap();
                if t.len() >= TAIL {
                    t.pop_front();
                }
                t.push_back(line);
            }
            done.store(true, std::sync::atomic::Ordering::Release);
        });
    } else {
        stderr_done.store(true, std::sync::atomic::Ordering::Release);
    }
    #[cfg(unix)]
    running.groups.push((child.id(), stderr_done.clone()));
    drop(running);
    Ok(Spawned {
        child,
        tail,
        warnings,
        stderr_done,
        name: name.to_owned(),
        #[cfg(windows)]
        job,
        _hold: hold,
    })
}

/// Run `cmd` to the end within `timeout` and return its stdout (killed on timeout).
pub fn output(cmd: Command, name: &str, timeout: Duration) -> Result<Vec<u8>, ProcError> {
    output_warned(cmd, name, timeout).map(|(out, _)| out)
}

/// [`output`], with the `WARNING:` lines it wrote ([`Spawned::warnings`]).
pub fn output_warned(
    cmd: Command,
    name: &str,
    timeout: Duration,
) -> Result<(Vec<u8>, Vec<String>), ProcError> {
    to_the_end(cmd, name, timeout).map(|(out, s)| (out, s.warnings()))
}

/// [`output`], with its last lines on stderr ([`Spawned::tail`]): what a tool says of its answer
/// besides it (ffprobe: a length it only estimated).
pub fn output_said(
    cmd: Command,
    name: &str,
    timeout: Duration,
) -> Result<(Vec<u8>, String), ProcError> {
    to_the_end(cmd, name, timeout).map(|(out, s)| (out, s.tail()))
}

/// How long a helper's output is still read once the helper itself has ended (#1521). What it
/// started (yt-dlp's ffmpeg, a one-file build's own process) can hold that output open after it:
/// a reading that waited for the output's end waited for as long as that process ran, the
/// download over but shown running. Its last lines come well within this, even on a busy
/// computer.
pub const AFTER_END: Duration = Duration::from_secs(2);

/// How often a reading asks whether the helper has ended ([`Output::next_line`]).
const ASK_EVERY: Duration = Duration::from_millis(100);

/// A helper's stdout, read a line at a time on a thread of its own (#1521): the reading ends
/// when every process holding the output has closed it, or [`AFTER_END`] after the helper
/// itself ended, whichever comes first. A line is given as it came, bytes (#79: not always
/// UTF-8), its `\n` kept, at most `max_line` bytes at a time (the rest of a longer one comes
/// next).
///
/// The reading thread ends with the output: once the helper's job (Windows) or process group
/// (Linux, macOS) is ended ([`Spawned::kill`]), with whatever held it.
pub struct Output {
    lines: std::sync::mpsc::Receiver<Vec<u8>>,
    /// When the helper was first seen ended, and when it was last asked.
    ended: Option<Instant>,
    asked: Option<Instant>,
    held_open: bool,
}

impl Output {
    /// Start reading `out`.
    pub fn read(out: impl Read + Send + 'static, max_line: usize) -> Output {
        // A few lines ahead at most: a helper that writes faster than it is read waits, as it
        // did on the pipe.
        let (tx, lines) = std::sync::mpsc::sync_channel(64);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(out);
            loop {
                let mut line = Vec::new();
                match (&mut reader)
                    .take(max_line as u64)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // Nobody reads any more.
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Output {
            lines,
            ended: None,
            asked: None,
            held_open: false,
        }
    }

    /// The next line, `None` at the end. `ended` says whether the helper has ended (asked every
    /// [`ASK_EVERY`] at most, until it says so): from then on lines are taken for [`AFTER_END`]
    /// more, then the reading ends even if the output is still open ([`Output::held_open`]).
    pub fn next_line(&mut self, mut ended: impl FnMut() -> bool) -> Option<Vec<u8>> {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            let now = Instant::now();
            if self.ended.is_none() && self.asked.is_none_or(|at| now >= at + ASK_EVERY) {
                self.asked = Some(now);
                if ended() {
                    self.ended = Some(now);
                }
            }
            let wait = match self.ended {
                None => ASK_EVERY,
                Some(at) => match (at + AFTER_END).checked_duration_since(now) {
                    Some(left) if !left.is_zero() => left,
                    _ => {
                        self.held_open = true;
                        return None;
                    }
                },
            };
            match self.lines.recv_timeout(wait) {
                Ok(line) => return Some(line),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// The reading ended with the output still open [`AFTER_END`] after the helper ended:
    /// something it started holds it.
    pub fn held_open(&self) -> bool {
        self.held_open
    }
}

/// Whether the helper in `slot` has ended, or isn't there any more (stopped). For
/// [`Output::next_line`]; the lock is held only to look.
pub fn ended(slot: &Mutex<Option<Spawned>>) -> bool {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .is_none_or(|s| !matches!(s.child.try_wait(), Ok(None)))
}

/// MDM's log, when a helper's output was still open [`AFTER_END`] after it ended (#1521).
pub fn note_held_open(name: &str) {
    tracing::warn!(
        "{name} ended, but something it started kept its output open; its output was read for {} s more (#1521)",
        AFTER_END.as_secs()
    );
}

/// How a helper run to the end ([`to_the_end`]) ended.
enum Ended {
    Exited(std::process::ExitStatus),
    /// Past its time.
    Late,
    Unknown(std::io::Error),
}

/// Whether `child` has ended, or run past `deadline`.
fn ended_by(child: &mut Child, deadline: Instant) -> Option<Ended> {
    match child.try_wait() {
        Ok(Some(status)) => Some(Ended::Exited(status)),
        Ok(None) if Instant::now() >= deadline => Some(Ended::Late),
        Ok(None) => None,
        Err(e) => Some(Ended::Unknown(e)),
    }
}

/// Run `cmd` to the end within `timeout` (killed on timeout): its stdout, and the helper, its
/// stderr read to the end, when it ended well. Its stdout is read to its end, or for
/// [`AFTER_END`] after the helper ended (#1521: what it started may hold it open).
fn to_the_end(
    cmd: Command,
    name: &str,
    timeout: Duration,
) -> Result<(Vec<u8>, Spawned), ProcError> {
    let mut s = spawn(cmd, name)?;
    let mut output = Output::read(s.child.stdout.take().expect("piped"), 64 * 1024);
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    let mut end = None;
    while let Some(chunk) = output.next_line(|| {
        end = end.take().or_else(|| ended_by(&mut s.child, deadline));
        end.is_some()
    }) {
        stdout.extend(chunk);
    }
    if output.held_open() && matches!(end, Some(Ended::Exited(_))) {
        note_held_open(name);
    }
    // Its output closed before it ended: waited for, within its time.
    let end = loop {
        if let Some(end) = end.take().or_else(|| ended_by(&mut s.child, deadline)) {
            break end;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    match end {
        Ended::Exited(status) if status.success() => {
            s.settle();
            Ok((stdout, s))
        }
        // `failure` waits for the stderr reader's last lines.
        Ended::Exited(_) => Err(s.failure()),
        Ended::Late => {
            s.kill();
            let _ = s.child.wait();
            Err(ProcError::Timeout(name.to_owned()))
        }
        Ended::Unknown(e) => Err(ProcError::Start(name.to_owned(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1053: a look-up waits 5 s on a server first; only a try that failed for a server that
    /// didn't answer in time is made again, once, with yt-dlp's own wait.
    #[test]
    fn a_look_up_waits_briefly_then_once_as_long_as_ytdlp_would() {
        let args = vec!["-J".to_owned()];
        let quick: Vec<String> = [
            "-J",
            "--socket-timeout",
            QUICK_SOCKET,
            "--extractor-retries",
            "0",
        ]
        .map(str::to_owned)
        .to_vec();
        let failed = |message: &str| ProcError::Failed {
            tool: "yt-dlp".into(),
            message: message.into(),
        };
        // Answered: one try, with the short wait.
        let mut tried = Vec::new();
        let r = quick_first(&args, |a| {
            tried.push(a.to_vec());
            Ok::<_, ProcError>("json")
        });
        assert_eq!(r.unwrap(), "json");
        assert_eq!(tried, std::slice::from_ref(&quick));
        // A server too slow for it: again with yt-dlp's own wait, which answers.
        let mut tried = Vec::new();
        let r = quick_first(&args, |a| {
            tried.push(a.to_vec());
            if tried.len() == 1 {
                Err(failed(
                    "ERROR: [generic] Unable to download webpage: HTTPSConnectionPool(host='slow.test', port=443): Read timed out. (read timeout=5.0)",
                ))
            } else {
                Ok("json")
            }
        });
        assert_eq!(r.unwrap(), "json");
        assert_eq!(tried, [quick.clone(), args.clone()]);
        // Timed out both times: twice only, the second error told.
        let mut tried = 0;
        let r: Result<(), _> = quick_first(&args, |_| {
            tried += 1;
            Err(failed(&format!(
                "try {tried}: The read operation timed out"
            )))
        });
        assert_eq!(tried, 2);
        assert!(r.unwrap_err().to_string().contains("try 2"));
        // Any other failure, or MDM's own limit on the whole run: not made again.
        for e in [
            failed("ERROR: [youtube] x: Video unavailable"),
            ProcError::Timeout("yt-dlp".into()),
        ] {
            let mut tried = 0;
            let mut e = Some(e);
            let r: Result<(), _> = quick_first(&args, |_| {
                tried += 1;
                Err(e.take().unwrap())
            });
            assert!(r.is_err());
            assert_eq!(tried, 1);
        }
        // A caller's own wait or retries stay, tried once.
        for own in [["--socket-timeout", "60"], ["--extractor-retries", "9"]] {
            let own = own.map(str::to_owned).to_vec();
            let mut tried = Vec::new();
            let _ = quick_first(&own, |a| {
                tried.push(a.to_vec());
                Err::<(), _>(failed("Read timed out."))
            });
            assert_eq!(tried, std::slice::from_ref(&own));
        }
        // yt-dlp's words for the network failing it, which its own retries would try again.
        for m in [
            "Read timed out. (read timeout=5.0)",
            "HTTPSConnectionPool(host='x', port=443): Max retries exceeded (Caused by ConnectTimeoutError(<…>, 'Connection to x timed out. (connect timeout=5.0)'))",
            "<urlopen error The read operation timed out>",
            "Unable to download API page: ('Connection aborted.', RemoteDisconnected('Remote end closed connection without response')) (caused by TransportError(...))",
            "Unable to download webpage: HTTP Error 503: Service Unavailable",
        ] {
            assert!(network_failure(m), "{m}");
        }
        for m in [
            "HTTP Error 403: Forbidden",
            "Video unavailable",
            "Requested format is not available",
            "Unable to download webpage: <urlopen error [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1032)> (caused by TransportError(...))",
        ] {
            assert!(!network_failure(m), "{m}");
        }
    }

    /// A program is looked for in absolute folders only: `.` on the PATH would be wherever MDM
    /// was started from.
    #[test]
    fn a_relative_folder_is_never_searched() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(file_in(&[PathBuf::from("src")], "lib.rs"), None);
        assert_eq!(file_in(&[PathBuf::from(".")], "Cargo.toml"), None);
        assert_eq!(
            file_in(&[PathBuf::from("."), here.join("src")], "lib.rs"),
            Some(here.join("src").join("lib.rs"))
        );
    }

    #[test]
    fn the_browsers_context_for_ytdlp() {
        let b = FromBrowser {
            referer: Some("https://course.test/lesson".into()),
            user_agent: Some("UA/1".into()),
            cookies: Some("sid=abc; theme=dark".into()),
            ..Default::default()
        };
        let (args, file) = b.args("https://player.test/video/1").unwrap();
        assert_eq!(
            &args[..4],
            [
                "--referer",
                "https://course.test/lesson",
                "--user-agent",
                "UA/1"
            ]
        );
        assert_eq!(args[4], "--cookies");
        let file = file.unwrap();
        let text = std::fs::read_to_string(&file.0).unwrap();
        assert!(text.starts_with("# Netscape HTTP Cookie File"));
        assert!(text.contains(".player.test\tTRUE\t/\tFALSE\t0\tsid\tabc\n"));
        assert!(
            !args.iter().any(|a| a.contains("abc")),
            "no cookie on the command line"
        );
        let path = file.0.clone();
        drop(file);
        assert!(!path.exists(), "removed once done");
        let none = FromBrowser::default().args("https://x.test/").unwrap();
        assert!(none.0.is_empty() && none.1.is_none());
    }

    /// #900: the Origin a page's player sent with a stream goes to yt-dlp as a header of its
    /// own, which yt-dlp keeps in each format's headers (the engine's for every piece); one that
    /// would break the command line is refused, as a Referer is.
    #[test]
    fn the_origin_the_player_sent_goes_to_ytdlp() {
        let b = FromBrowser {
            referer: Some("https://player.test/".into()),
            origin: Some("https://player.test".into()),
            ..Default::default()
        };
        let (args, _) = b.args("https://cdn.test/x.m3u8").unwrap();
        assert_eq!(
            args,
            [
                "--referer",
                "https://player.test/",
                "--add-header",
                "Origin:https://player.test"
            ]
        );
        let bad = FromBrowser {
            origin: Some("https://x.test\r\nCookie: a=1".into()),
            ..Default::default()
        };
        assert!(matches!(
            bad.args("https://x.test/"),
            Err(ProcError::BadArgument)
        ));
    }

    /// #229: the button over a YouTube video sends the watch page (its player plays a blob:, and
    /// a video's own requests aren't a manifest) with the cookies the browser has for that page:
    /// youtube.com's sign-in, `__Secure-` ones included. An older extension sends them as a
    /// header, without their domains: they are filed for youtube.com and its subdomains, so they
    /// reach www.youtube.com, the host yt-dlp's YouTube extractor asks (and checks the sign-in
    /// on), from a YouTube Music or mobile page too; as session cookies (expiry 0, which
    /// yt-dlp's loader keeps) sent over HTTP and HTTPS.
    #[test]
    fn a_youtube_pages_cookies_reach_the_host_ytdlp_asks() {
        let b = FromBrowser {
            cookies: Some(
                "VISITOR_INFO1_LIVE=v; SAPISID=s/1; __Secure-3PAPISID=s3; __Secure-3PSID=p3; \
                 LOGIN_INFO=AFmmF2s:QUQ3; __Host-GAPS=1:g"
                    .into(),
            ),
            ..Default::default()
        };
        let (args, file) = b
            .args("https://www.youtube.com/watch?v=A1WG3x-Ebpk")
            .unwrap();
        assert_eq!(args[0], "--cookies");
        let file = file.unwrap();
        let text = std::fs::read_to_string(&file.0).unwrap();
        for (name, value) in [
            ("VISITOR_INFO1_LIVE", "v"),
            ("SAPISID", "s/1"),
            ("__Secure-3PAPISID", "s3"),
            ("__Secure-3PSID", "p3"),
            ("LOGIN_INFO", "AFmmF2s:QUQ3"),
            ("__Host-GAPS", "1:g"),
        ] {
            let line = format!(".youtube.com\tTRUE\t/\tFALSE\t0\t{name}\t{value}\n");
            assert!(text.contains(&line), "{line:?} in {text}");
        }
        assert_eq!(text.lines().count(), 7, "the header and six cookies");
        for page in [
            "https://music.youtube.com/watch?v=A1WG3x-Ebpk",
            "https://m.youtube.com/watch?v=A1WG3x-Ebpk",
            "https://youtube.com/shorts/A1WG3x-Ebpk",
        ] {
            let (_, file) = b.args(page).unwrap();
            let text = std::fs::read_to_string(&file.unwrap().0).unwrap();
            assert!(
                text.contains(".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\ts/1\n"),
                "{page}: {text}"
            );
        }
    }

    /// The domain an older extension's header is filed under: the page's host, or its site's
    /// where yt-dlp asks another of the site's hosts.
    #[test]
    fn a_header_goes_to_the_sites_own_domain() {
        for (host, domain) in [
            ("www.youtube.com", "youtube.com"),
            ("music.youtube.com", "youtube.com"),
            ("m.youtube.com", "youtube.com"),
            ("youtube.com", "youtube.com"),
            ("Music.YouTube.com.", "YouTube.com"),
            ("m.facebook.com", "facebook.com"),
            ("mobile.twitter.com", "twitter.com"),
            ("m.vk.com", "vk.com"),
            ("player.test", "player.test"),
            ("www.example.org", "www.example.org"),
            ("notyoutube.com", "notyoutube.com"),
            ("youtube.com.evil.test", "youtube.com.evil.test"),
            ("m.org", "m.org"),
            ("127.0.0.1", "127.0.0.1"),
        ] {
            assert_eq!(header_domain(host), domain, "{host}");
        }
    }

    fn cookie(name: &str, domain: &str, host_only: bool) -> mdm_ipc::BrowserCookie {
        mdm_ipc::BrowserCookie {
            name: name.into(),
            value: format!("{name}-v"),
            domain: domain.into(),
            host_only,
            path: "/".into(),
            ..Default::default()
        }
    }

    /// A newer extension sends each cookie with the domain, path and flags the browser keeps: the
    /// file has each as it is (a host's own cookie for that host alone, a site's for its
    /// subdomains too), and the header, sent for older apps, is left out.
    #[test]
    fn each_cookie_keeps_its_domain_and_flags() {
        let b = FromBrowser {
            cookies: Some("SID=SID-v; pref=pref-v".into()),
            jar: vec![
                mdm_ipc::BrowserCookie {
                    secure: true,
                    expires: Some(1893456000.75),
                    path: "/".into(),
                    ..cookie("SID", ".youtube.com", false)
                },
                cookie("pref", "music.youtube.com", true),
                // Without a leading dot, as Firefox may name a domain cookie.
                mdm_ipc::BrowserCookie {
                    path: "/account".into(),
                    ..cookie("acct", "youtube.com", false)
                },
                // A path that isn't one: the whole site.
                mdm_ipc::BrowserCookie {
                    path: "account".into(),
                    ..cookie("odd", "youtube.com", true)
                },
                // Never a field that would shift the others, an empty name or domain.
                cookie("tab\tname", ".youtube.com", false),
                mdm_ipc::BrowserCookie {
                    value: "a\nb".into(),
                    ..cookie("line", ".youtube.com", false)
                },
                cookie("", ".youtube.com", false),
                cookie("nodomain", "", false),
                cookie("dots", "...", false),
                cookie("slash", "youtube.com/x", false),
            ],
            ..Default::default()
        };
        let (args, file) = b.args("https://music.youtube.com/watch?v=x").unwrap();
        assert_eq!(args[0], "--cookies");
        let text = std::fs::read_to_string(&file.unwrap().0).unwrap();
        assert_eq!(
            text,
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSID\tSID-v\n\
             music.youtube.com\tFALSE\t/\tFALSE\t0\tpref\tpref-v\n\
             .youtube.com\tTRUE\t/account\tFALSE\t0\tacct\tacct-v\n\
             youtube.com\tFALSE\t/\tFALSE\t0\todd\todd-v\n"
        );
        // A page with no address of its own (none parsed) still gets the jar's.
        assert!(b.args("not a url").unwrap().1.is_some());
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn proxied(cmd: &Command) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| k.to_string_lossy() == "HTTPS_PROXY")
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    /// #363: Options -> Proxy / Socks reaches yt-dlp: the address on the command line, a login
    /// only in the environment, never as an argument; a manual proxy only for its ticked
    /// schemes; "no proxy" and a script's DIRECT: nothing added, so yt-dlp's own default stays.
    #[test]
    fn a_helper_is_given_the_proxy_of_the_settings() {
        use mdm_engine::Proxy;
        let manual = |login: Option<(&str, &str)>, http, https| Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: login.map(|(u, p)| (u.to_owned(), p.to_owned())),
            http,
            https,
        };
        let youtube = url::Url::parse("https://www.youtube.com/watch?v=x").unwrap();
        assert_eq!(
            proxy_given(&Proxy::None, Some(&youtube)),
            ProxyGiven::Nothing
        );
        assert_eq!(
            proxy_given(&manual(None, true, true), Some(&youtube)),
            ProxyGiven::Argument("http://127.0.0.1:10808".into())
        );
        assert_eq!(
            proxy_given(&manual(None, true, false), Some(&youtube)),
            ProxyGiven::Nothing,
            "https unticked: this https page isn't covered"
        );
        // #1448: and that is said (MDM's log), for the scheme left out only.
        assert_eq!(
            manual_skips(&manual(None, true, false), Some(&youtube)),
            Some("https")
        );
        assert_eq!(
            manual_skips(&manual(None, true, true), Some(&youtube)),
            None
        );
        let plain = url::Url::parse("http://videos.test/v").unwrap();
        assert_eq!(
            manual_skips(&manual(None, false, true), Some(&plain)),
            Some("http")
        );
        assert_eq!(manual_skips(&Proxy::None, Some(&youtube)), None);
        // A login: in the environment, encoded, never an argument.
        assert_eq!(
            proxy_given(&manual(Some(("u@me", "p:ss")), true, true), Some(&youtube)),
            ProxyGiven::Environment("http://u%40me:p%3Ass@127.0.0.1:10808/".into())
        );
        let pac = |script: &str| Proxy::Pac {
            script: std::sync::Arc::from(script),
        };
        assert_eq!(
            proxy_given(
                &pac("function FindProxyForURL(u,h){return 'PROXY 10.0.0.1:3128';}"),
                Some(&youtube)
            ),
            ProxyGiven::Argument("http://10.0.0.1:3128".into())
        );
        assert_eq!(
            proxy_given(
                &pac("function FindProxyForURL(u,h){return 'DIRECT';}"),
                Some(&youtube)
            ),
            ProxyGiven::Nothing
        );
    }

    /// Held by the tests that set the settings' proxy, or build commands assuming there is none:
    /// it is one setting for the whole program, and the tests run at the same time.
    static GLOBAL_PROXY: Mutex<()> = Mutex::new(());

    /// #363, the wiring: the setting reaches a real command line.
    #[test]
    fn yt_dlp_runs_through_the_settings_proxy() {
        // The settings' proxy is global (`set_proxy`): one test sets it, others assume none.
        let _proxy = GLOBAL_PROXY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use mdm_engine::Proxy;
        let url = "https://www.youtube.com/watch?v=x";
        let exe = std::path::Path::new("yt-dlp");
        set_proxy(Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: None,
            http: true,
            https: true,
        });
        let plain = args_of(&ytdlp(exe, &[], &["-J".into()], url).unwrap());
        let at = plain.iter().position(|a| a == "--proxy").expect("--proxy");
        assert_eq!(plain[at + 1], "http://127.0.0.1:10808");
        assert!(at < plain.iter().position(|a| a == "--").unwrap());
        set_proxy(Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: Some(("me".into(), "secret".into())),
            http: true,
            https: true,
        });
        let cmd = ytdlp(exe, &[], &["-J".into()], url).unwrap();
        assert!(!args_of(&cmd).iter().any(|a| a.contains("secret")));
        assert_eq!(
            proxied(&cmd).as_deref(),
            Some("http://me:secret@127.0.0.1:10808/")
        );
        // #1448: a run from the looked-up page (the download, the choice of formats) asks the
        // automatic configuration script about the page, as its look-up did: it went straight
        // to the site when the script was asked about no address.
        set_proxy(Proxy::Pac {
            script: std::sync::Arc::from(
                "function FindProxyForURL(u,h){return h=='www.youtube.com'?'PROXY 10.0.0.1:3128':'DIRECT';}",
            ),
        });
        let info = std::path::Path::new("/info.json");
        for args in [
            args_of(&ytdlp(exe, &[], &["-J".into()], url).unwrap()),
            args_of(&ytdlp_info(exe, &[], &[], info, url).unwrap()),
        ] {
            let at = args.iter().position(|a| a == "--proxy").expect("--proxy");
            assert_eq!(args[at + 1], "http://10.0.0.1:3128");
        }
        let elsewhere = args_of(&ytdlp_info(exe, &[], &[], info, "https://other.test/v").unwrap());
        assert!(!elsewhere.iter().any(|a| a == "--proxy"), "DIRECT for it");
        set_proxy(Proxy::None);
        let none = args_of(&ytdlp(exe, &[], &["-J".into()], url).unwrap());
        assert!(!none.iter().any(|a| a == "--proxy"));
    }

    fn yt(chosen: Option<&BrowserCookies>, ours: &[&str]) -> Vec<String> {
        let mut cmd = Command::new("yt-dlp");
        let ours: Vec<String> = ours.iter().map(|s| s.to_string()).collect();
        layers_with(&mut cmd, &[], &ours, chosen).unwrap();
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Private requests never inherit Options' normal browser store, but may use their own
    /// cookie header or jar. Check the final command layers with the store explicitly on.
    #[test]
    fn private_video_requests_keep_their_own_cookies_only() {
        let firefox = BrowserCookies::new("firefox", "normal-profile").unwrap();
        for (private, header, jar) in [
            (true, None, vec![]),
            (true, Some("  "), vec![]),
            (true, Some("sid=private"), vec![]),
            (true, None, vec![cookie("sid", "videos.test", true)]),
            (false, None, vec![]),
        ] {
            let own = header.is_some_and(|s| !s.trim().is_empty()) || !jar.is_empty();
            let browser = FromBrowser {
                private,
                cookies: header.map(str::to_owned),
                jar,
                ..Default::default()
            };
            let (args, file) = browser.args("https://videos.test/watch").unwrap();
            let mut cmd = Command::new("yt-dlp");
            layers_with(&mut cmd, &[], &args, Some(&firefox)).unwrap();
            let final_args = args_of(&cmd);
            assert_eq!(
                final_args.iter().any(|a| a == "--cookies-from-browser"),
                !private
            );
            assert_eq!(final_args.iter().any(|a| a == "--cookies"), own);
            assert_eq!(final_args.iter().any(|a| a == NO_COOKIES), private && !own);
            assert_eq!(with_cookies(&args, Some(&firefox)), own || !private);
            if private && !own {
                assert!(!has_cookies(&args), "no pointless cookie-free retry");
            }
            assert_eq!(file.is_some(), own);
            if let Some(file) = file {
                assert!(std::fs::read_to_string(&file.0).unwrap().contains("sid"));
            }
            let retry = without_cookies(&args);
            let refs: Vec<&str> = retry.iter().map(String::as_str).collect();
            assert!(
                !yt(Some(&firefox), &refs)
                    .iter()
                    .any(|a| a == "--cookies-from-browser")
            );
        }
    }

    /// #229: Options → Cookies for video sites. Off: nothing added. On: every run without
    /// cookies of its own reads the browser's (a pasted address, a look-up), with yt-dlp's
    /// warnings kept on; a run with the extension's cookies keeps those alone.
    #[test]
    fn the_browsers_cookies_only_when_the_extension_sent_none() {
        assert_eq!(yt(None, &["-J", "--no-warnings"]), ["-J", "--no-warnings"]);
        let firefox = BrowserCookies::new("Firefox", "").unwrap();
        assert_eq!(firefox.spec(), "firefox");
        assert_eq!(
            yt(Some(&firefox), &["-J", "--no-warnings"]),
            ["-J", "--cookies-from-browser", "firefox"]
        );
        assert_eq!(
            yt(Some(&firefox), &["-J", "--cookies", "/tmp/c.txt"]),
            ["-J", "--cookies", "/tmp/c.txt"],
            "the extension's cookies win"
        );
        // A profile by name, or a Chromium fork's profile folder (Helium, Thorium...).
        let named = BrowserCookies::new("chrome", " Profile 1 ").unwrap();
        assert_eq!(named.spec(), "chrome:Profile 1");
        let fork = BrowserCookies::new(
            "chromium",
            r"C:\Users\a\AppData\Local\Helium\User Data\Default",
        )
        .unwrap();
        assert_eq!(
            yt(Some(&fork), &["-f", "b"]),
            [
                "-f",
                "b",
                "--cookies-from-browser",
                r"chromium:C:\Users\a\AppData\Local\Helium\User Data\Default"
            ]
        );
        // Only the browsers yt-dlp reads; Off is no browser.
        assert!(BrowserCookies::new("helium", "").is_none());
        assert!(BrowserCookies::new("", "x").is_none());
        // A profile with a line break never reaches yt-dlp.
        let bad = BrowserCookies::new("firefox", "a\nb").unwrap();
        let mut cmd = Command::new("yt-dlp");
        assert!(layers_with(&mut cmd, &[], &[], Some(&bad)).is_err());
        assert_eq!(
            cookie_browsers().contains(&"safari"),
            cfg!(target_os = "macos")
        );
    }

    /// #360, #852, #1570: the computer's runtimes in yt-dlp's order (Deno, Node.js, QuickJS,
    /// Bun), each from the PATH, its installer's folder, nvm (newest first) or the folders a
    /// runtime installed later is in; none twice.
    #[test]
    fn the_computers_javascript_runtimes_in_yt_dlps_order() {
        use crate::jsruntime::Kind;
        let touch = |dir: &Path, name: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
                b"",
            )
            .unwrap();
        };
        let exe =
            |dir: &Path, name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let kinds = |found: &[(Kind, PathBuf)]| found.iter().map(|(k, _)| *k).collect::<Vec<_>>();
        let root = tempfile::tempdir().unwrap();
        let (path, home) = (root.path().join("bin"), root.path().join("home"));
        let dirs = vec![path.clone()];
        std::fs::create_dir_all(&path).unwrap();
        assert!(js_runtimes_in(&dirs, &[], Some(&home)).is_empty());
        // nvm: by their numbers, v22 before v9 (an old v20 is there too).
        for v in ["v9.11.2", "v20.1.0", "v22.3.0"] {
            touch(&home.join(".nvm/versions/node").join(v).join("bin"), "node");
        }
        let found = js_runtimes_in(&dirs, &[], Some(&home));
        assert_eq!(kinds(&found), [Kind::Node; 3]);
        assert!(found[0].1.to_string_lossy().contains("v22.3.0"));
        assert!(found[1].1.to_string_lossy().contains("v20.1.0"));
        assert!(found[2].1.to_string_lossy().contains("v9.11.2"));
        touch(&path, "bun");
        assert_eq!(kinds(&js_runtimes_in(&dirs, &[], None)), [Kind::Bun]);
        // QuickJS (`qjs`) before Bun, which yt-dlp deprecated.
        let qjs_bin = root.path().join("qjs-bin");
        touch(&qjs_bin, "qjs");
        touch(&qjs_bin, "bun");
        assert_eq!(
            kinds(&js_runtimes_in(std::slice::from_ref(&qjs_bin), &[], None)),
            [Kind::QuickJs, Kind::Bun]
        );
        touch(&path, "node");
        let found = js_runtimes_in(&dirs, &[], None);
        assert_eq!(found[0], (Kind::Node, exe(&path, "node")));
        // Deno from its own installer (`~/.deno/bin`), or one in a folder a runtime installed
        // after MDM started is in (Windows: winget's links), before Node.js.
        let more = root.path().join("winget-links");
        touch(&more, "node");
        assert_eq!(
            js_runtimes_in(&[], std::slice::from_ref(&more), None),
            [(Kind::Node, exe(&more, "node"))]
        );
        touch(&more, "deno");
        assert_eq!(
            js_runtimes_in(&dirs, std::slice::from_ref(&more), None)[0],
            (Kind::Deno, exe(&more, "deno"))
        );
        touch(&home.join(".deno/bin"), "deno");
        assert_eq!(
            js_runtimes_in(&dirs, &[], Some(&home))[0],
            (Kind::Deno, exe(&home.join(".deno/bin"), "deno"))
        );
        // Deno on the PATH first of all; the same program found twice is listed once.
        touch(&path, "deno");
        let found = js_runtimes_in(&[path.clone(), path.clone()], &[], Some(&home));
        assert_eq!(found[0], (Kind::Deno, exe(&path, "deno")));
        assert_eq!(
            kinds(&found),
            [
                Kind::Deno,
                Kind::Deno,
                Kind::Node,
                Kind::Node,
                Kind::Node,
                Kind::Node,
                Kind::Bun
            ]
        );
    }

    /// yt-dlp before 2025.11.12 has no `--js-runtimes`, and stops at it: it isn't told of a
    /// runtime. A version that isn't a date (a build of the person's own) is, as before.
    #[test]
    fn only_a_yt_dlp_that_has_the_option_is_told_of_a_runtime() {
        for old in ["2025.10.22", "2025.11.11", "2024.12.23", "2023.03.04.1"] {
            assert!(!knows_js_runtimes(old), "{old}");
        }
        for new in [
            "2025.11.12",
            "2025.11.12.232521",
            "2025.12.8",
            "2026.10.26",
            "2026.10.26.1",
        ] {
            assert!(knows_js_runtimes(new), "{new}");
        }
        for unknown in ["", "system", "master", "1.0"] {
            assert!(knows_js_runtimes(unknown), "{unknown}");
        }
    }

    /// #852: what yt-dlp 2026.10.26 wrote looking a YouTube video up (`-J`, its stderr as is):
    /// with no runtime on the PATH; asking a player that needs one with none there (as when
    /// signed in); and with Node.js.
    #[test]
    fn a_youtube_look_up_without_a_javascript_runtime_is_told_apart() {
        let warnings = |stderr: &str| -> Vec<String> {
            stderr
                .lines()
                .filter_map(|l| l.strip_prefix("WARNING:"))
                .map(|w| w.trim().to_owned())
                .collect()
        };
        let none = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/no-runtime.stderr"
        ));
        let unsolved = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/challenge-failed.stderr"
        ));
        let node = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/with-node.stderr"
        ));
        assert_eq!((none.len(), unsolved.len(), node.len()), (1, 3, 1));
        // Its own words say it, whatever MDM found.
        assert!(missing_js_runtime(&none, false));
        assert!(missing_js_runtime(&none, true));
        // A challenge left unsolved is a runtime's want only when this computer has none: with
        // one, the solver script is what failed, and installing Node.js wouldn't help.
        assert!(missing_js_runtime(&unsolved, false));
        assert!(!missing_js_runtime(&unsolved, true));
        // A look-up that had one says nothing of the kind.
        assert!(!missing_js_runtime(&node, false));
        assert!(!missing_js_runtime(&[], false));
    }

    /// #508: a helper's PATH keeps its folders in their order and leaves out those that can't be
    /// opened (missing, a file); a plain PATH comes through unchanged.
    #[test]
    fn a_helpers_path_leaves_out_the_folders_it_cant_open() {
        let root = tempfile::tempdir().unwrap();
        let (a, b, gone, file) = (
            root.path().join("a"),
            root.path().join("b"),
            root.path().join("gone"),
            root.path().join("file"),
        );
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(&file, b"").unwrap();
        let join = |dirs: &[&PathBuf]| std::env::join_paths(dirs).unwrap();
        // What the helper gets: those, then (on Linux and macOS) the package folders (#659).
        let given = |dirs: &[&PathBuf]| {
            #[allow(unused_mut)]
            let mut all: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
            #[cfg(unix)]
            all.extend(
                crate::system::PACKAGE_DIRS
                    .map(PathBuf::from)
                    .into_iter()
                    .filter(|d| d.is_dir()),
            );
            std::env::join_paths(all).unwrap()
        };

        let plain = join(&[&b, &a]);
        assert_eq!(child_path(&plain).unwrap(), given(&[&b, &a]));
        assert_eq!(
            child_path(&join(&[&gone, &b, &file, &a])).unwrap(),
            given(&[&b, &a])
        );

        let mut cmd = Command::new("x");
        cmd.env("PATH", join(&[&a, &gone]));
        with_usable_path(&mut cmd);
        let got: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == "PATH").collect();
        let want = given(&[&a]);
        assert_eq!(
            got,
            [(std::ffi::OsStr::new("PATH"), Some(want.as_os_str()))]
        );
    }

    /// #659: MDM opened from Finder has no Homebrew on its PATH, yet finds yt-dlp there; the
    /// helper gets the package folders after its own, once each, so yt-dlp finds Deno and
    /// ffmpeg where Homebrew put them.
    #[cfg(unix)]
    #[test]
    fn a_helper_gets_the_package_folders_after_its_path() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own");
        std::fs::create_dir_all(&own).unwrap();
        let usr_bin = PathBuf::from("/usr/bin");

        let dirs = usable_dirs(&std::env::join_paths([&own]).unwrap());
        assert_eq!(dirs[0], own, "the PATH's own folders first");
        assert!(dirs.contains(&usr_bin), "{dirs:?}");
        for d in crate::system::PACKAGE_DIRS.map(PathBuf::from) {
            assert_eq!(dirs.contains(&d), d.is_dir(), "{d:?} in {dirs:?}");
        }

        // Already on the PATH: kept where it was, not added again.
        let dirs = usable_dirs(&std::env::join_paths([&usr_bin, &own]).unwrap());
        assert_eq!(dirs[..2], [usr_bin.clone(), own]);
        assert_eq!(dirs.iter().filter(|d| **d == usr_bin).count(), 1);
    }

    /// #508 as it happened: Windows refuses a junction to a process held to Redirection Guard
    /// ("[WinError 448] ... untrusted mount point"). A stand-in for yt-dlp (PowerShell turning
    /// the guard on for itself, then opening each PATH folder) fails on a PATH through a
    /// junction made here, and passes once started as MDM starts helpers: given the junction's
    /// real folder, in the same place.
    #[cfg(windows)]
    #[test]
    fn a_helper_held_to_redirection_guard_walks_its_path() {
        let root = tempfile::tempdir().unwrap();
        let (real, junction) = (root.path().join("real"), root.path().join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("tool.txt"), b"").unwrap();
        let made = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&real)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let path = std::env::join_paths([&system, &junction]).unwrap();
        let usable = child_path(&path).unwrap();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&usable).collect();
        assert_eq!(dirs.len(), 2);
        let found = dirs.pop().unwrap();
        assert!(
            std::fs::canonicalize(&found).unwrap() == std::fs::canonicalize(&real).unwrap()
                && !found.to_string_lossy().starts_with(r"\\?\")
                && !found.ancestors().any(reparse_point),
            "{found:?}"
        );
        assert_eq!(dirs, std::slice::from_ref(&system));

        let script = root.path().join("standin.ps1");
        std::fs::write(
            &script,
            r#"
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices;
public static class Guard { [DllImport("kernel32.dll", SetLastError=true)]
public static extern bool SetProcessMitigationPolicy(int policy, ref uint flags, UIntPtr size); }'
[uint32]$enforce = 1
if (-not [Guard]::SetProcessMitigationPolicy(16, [ref]$enforce, [UIntPtr]::new([uint32]4))) {
  'no guard here'; exit 0
}
foreach ($d in $env:PATH.Split(';')) {
  if ($d) {
    try { [void][IO.Directory]::GetFiles($d, 'tool.txt') }
    catch { [Console]::Error.WriteLine('ERROR: WinError ' + ($_.Exception.InnerException.HResult -band 0xFFFF) + ' ' + $d); exit 1 }
  }
}
'walked'
"#,
        )
        .unwrap();
        let standin = || {
            let mut cmd = Command::new(system.join(r"WindowsPowerShell\v1.0\powershell.exe"));
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .env("PATH", &path);
            cmd
        };
        let raw = standin().output().unwrap();
        let said = String::from_utf8_lossy(&raw.stdout).trim().to_owned();
        if said == "no guard here" {
            eprintln!("this Windows has no Redirection Guard: the stand-in can't be held to it");
            return;
        }
        if raw.status.success() && said == "walked" {
            // A junction an administrator made is trusted (CI runs as one): nothing to refuse.
            eprintln!(
                "this junction is trusted here (made as an administrator): nothing to refuse"
            );
            return;
        }
        let refused = String::from_utf8_lossy(&raw.stderr);
        assert!(
            !raw.status.success() && refused.contains("WinError 448"),
            "the junction is refused as it was on #508: {said} {refused}"
        );
        let out = output(standin(), "stand-in", Duration::from_secs(60)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "walked");
    }

    /// #352: the second look-up has no cookies of any kind, and everything else the first had.
    #[test]
    fn a_look_up_without_cookies_keeps_the_rest() {
        let ours: Vec<String> = [
            "-J",
            "--cookies",
            "/tmp/c.txt",
            "--referer",
            "https://p.test/",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            without_cookies(&ours),
            ["-J", "--referer", "https://p.test/", NO_COOKIES]
        );
        let firefox = BrowserCookies::new("firefox", "").unwrap();
        assert!(with_cookies(&ours, None));
        assert!(with_cookies(&["-J".to_owned()], Some(&firefox)));
        assert!(!with_cookies(&["-J".to_owned()], None));
        // Told so, the chosen browser's cookies aren't read either.
        assert_eq!(yt(Some(&firefox), &["-J", NO_COOKIES]), ["-J", NO_COOKIES]);
        // #686: a run made without them has none, the chosen browser's neither, and taking them
        // out twice is taking them out once.
        let plain = without_cookies(&ours);
        assert!(!with_cookies(&plain, None));
        assert!(!with_cookies(&plain, Some(&firefox)));
        assert_eq!(without_cookies(&plain), plain);
    }

    /// #686: which look-ups that failed with cookies are made once more without them: any
    /// site's with no format left (#352); on YouTube only, its refusals of a signed-in viewer,
    /// in yt-dlp's real words (`tests/fixtures/ytdlp-refusals`), never a video that needs the
    /// sign-in.
    #[test]
    fn a_look_up_refused_with_cookies_is_made_without_them_on_youtube_only() {
        let error = |stderr: &str| {
            stderr
                .lines()
                .find_map(|l| l.strip_prefix("ERROR:"))
                .unwrap()
                .trim()
                .to_owned()
        };
        let yt = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
        let other = "https://video.example/watch?v=1";
        let bot = error(include_str!(
            "../tests/fixtures/ytdlp-refusals/bot-check.stderr"
        ));
        let api = error(include_str!(
            "../tests/fixtures/ytdlp-refusals/api-403.stderr"
        ));
        for e in [&bot, &api] {
            assert!(refused_with_cookies(yt, e), "{e}");
            assert!(!refused_with_cookies(other, e), "another site: {e}");
        }
        for stderr in [
            include_str!("../tests/fixtures/ytdlp-refusals/private.stderr"),
            include_str!("../tests/fixtures/ytdlp-refusals/age.stderr"),
            include_str!("../tests/fixtures/ytdlp-refusals/members.stderr"),
        ] {
            let e = error(stderr);
            assert!(!refused_with_cookies(yt, &e), "{e}");
        }
        let none = error(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/challenge-failed.stderr"
        ));
        assert!(refused_with_cookies(yt, &none));
        assert!(refused_with_cookies(other, &none), "#352, every site");
    }

    /// #229: why the browser's cookies didn't help stays on the failure's message.
    #[test]
    fn the_cookie_trouble_is_kept_on_the_failure() {
        let tail: VecDeque<String> = [
            "[youtube] Extracting URL: https://www.youtube.com/watch?v=x",
            "Extracted 3 cookies from chrome (41 could not be decrypted)",
            "ERROR: [youtube] x: Sign in to confirm you're not a bot.",
        ]
        .map(String::from)
        .into();
        let warnings = vec!["unknown cookie version: \"b'v20'\"".to_owned()];
        assert_eq!(
            with_cookie_trouble(
                "[youtube] x: Sign in to confirm you're not a bot.".into(),
                &warnings,
                &tail
            ),
            "[youtube] x: Sign in to confirm you're not a bot. (unknown cookie version: \"b'v20'\")"
        );
        let copy: VecDeque<String> = [
            "ERROR: Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info",
            "ERROR: failed to load cookies",
        ]
        .map(String::from)
        .into();
        assert!(
            with_cookie_trouble("failed to load cookies".into(), &[], &copy)
                .contains("Could not copy Chrome cookie database")
        );
        // Nothing about cookies: the message as it was.
        assert_eq!(
            with_cookie_trouble(
                "Video unavailable".into(),
                &["a warning".into()],
                &VecDeque::new()
            ),
            "Video unavailable"
        );
    }

    /// #955: yt-dlp's words for a certificate its own list doesn't trust (an antivirus or a
    /// proxy that reads HTTPS), as the reporter's yt-dlp 2026.10.26 said them on Windows.
    const UNTRUSTED: &str = "[youtube] X6ek6j1gTQA: Unable to download API page: [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1085) (caused by CertificateVerifyError('[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1085)')); please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U";

    fn failed(message: &str) -> ProcError {
        ProcError::Failed {
            tool: "yt-dlp".into(),
            message: message.into(),
        }
    }

    #[test]
    fn an_untrusted_certificate_is_told_apart() {
        assert!(untrusted_certificate(UNTRUSTED));
        assert!(untrusted_certificate(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate in certificate chain (_ssl.c:1006)"
        ));
        // The system's certificates wouldn't help: an expired one, another site's.
        assert!(!untrusted_certificate(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: certificate has expired (_ssl.c:1006)"
        ));
        assert!(!untrusted_certificate(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: Hostname mismatch, certificate is not valid for 'x'. (_ssl.c:1006)"
        ));
        assert!(!untrusted_certificate(
            "Sign in to confirm you're not a bot."
        ));
        assert_eq!(cert_args(false), &[] as &[&str]);
        assert_eq!(cert_args(true), ["--compat-options", "no-certifi"]);
    }

    /// The reporter's computer: an antivirus or a proxy signs the sites, and only the system has
    /// its certificate. yt-dlp's own list refuses; once more with the system's works, and the
    /// runs after (the download, a fresh address) keep it.
    #[test]
    fn a_certificate_yt_dlp_refuses_is_asked_with_the_systems() {
        let system = AtomicBool::new(false);
        let mut runs = Vec::new();
        let mut run = || {
            let with_system = system.load(Ordering::Relaxed);
            runs.push(with_system);
            if with_system {
                Ok("json")
            } else {
                Err(failed(UNTRUSTED))
            }
        };
        assert_eq!(with_system_certs(&system, &mut run).unwrap(), "json");
        assert_eq!(with_system_certs(&system, &mut run).unwrap(), "json");
        assert_eq!(runs, [false, true, true]);
        assert!(system.load(Ordering::Relaxed));
    }

    /// The system doesn't trust it either: back to yt-dlp's own list, and the first error is
    /// told. Another failure isn't a certificate's: no second run.
    #[test]
    fn a_certificate_nobody_trusts_goes_back_to_yt_dlps_list() {
        let system = AtomicBool::new(false);
        let mut runs = 0;
        let e = with_system_certs(&system, || -> Result<(), _> {
            runs += 1;
            Err(failed(&format!("{UNTRUSTED} ({runs})")))
        })
        .unwrap_err();
        assert_eq!(runs, 2);
        assert!(e.to_string().ends_with("(1)"), "{e}");
        assert!(!system.load(Ordering::Relaxed));
        let mut runs = 0;
        let e = with_system_certs(&system, || -> Result<(), _> {
            runs += 1;
            Err(failed("Video unavailable"))
        })
        .unwrap_err();
        assert_eq!(runs, 1);
        assert!(e.to_string().contains("Video unavailable"), "{e}");
        // With the system's already: a refusal isn't asked again.
        system.store(true, Ordering::Relaxed);
        let mut runs = 0;
        let _ = with_system_certs(&system, || -> Result<(), _> {
            runs += 1;
            Err(failed(UNTRUSTED))
        });
        assert_eq!(runs, 1);
    }

    #[test]
    fn hostile_text_never_reaches_the_argument_list() {
        let exe = Path::new("yt-dlp");
        assert!(ytdlp(exe, &[], &[], "https://x/\n--exec calc").is_err());
        assert!(ytdlp(exe, &["--format\0best".into()], &[], "https://x/").is_err());
    }

    #[test]
    fn the_persons_config_only_when_asked() {
        assert!(config_args(false).is_empty());
        assert_eq!(
            config_args(true),
            [
                "--downloader",
                "http,ftp,m3u8:native",
                "--no-download-archive",
                "--no-force-overwrites",
                "--fixup",
                "detect_or_warn",
                "--no-mark-watched",
                "--part",
            ]
        );
    }

    #[test]
    fn fixed_arguments_come_last_and_the_url_after_the_separator() {
        // The settings' proxy is global (`set_proxy`): one test sets it, others assume none.
        let _proxy = GLOBAL_PROXY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cmd = ytdlp(
            Path::new("yt-dlp"),
            &["--user".into()],
            &["-J".into()],
            "-https://x/",
        )
        .unwrap();
        let mut args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // This computer's JavaScript runtime, if it has one (#360, #1570); tested in jsruntime.
        if let Some(i) = args.iter().position(|a| a == "--no-js-runtimes") {
            assert_eq!(args[i + 1], "--js-runtimes");
            args.drain(i..=i + 2);
        }
        assert_eq!(
            args,
            [
                "--user",
                "-J",
                "--ignore-config",
                "--no-exec",
                "--no-update",
                "--encoding",
                "utf-8",
                "--",
                "-https://x/"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn output_and_failures_of_a_real_process() {
        let mut ok = Command::new("cmd");
        ok.args(["/C", "echo hello"]);
        let out = output(ok, "cmd", Duration::from_secs(10)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hello");
        let mut bad = Command::new("cmd");
        bad.args(["/C", "echo ERROR: no such video 1>&2 & exit 1"]);
        let err = output(bad, "cmd", Duration::from_secs(10)).unwrap_err();
        assert!(err.to_string().contains("no such video"), "{err}");
        let mut slow = Command::new("cmd");
        slow.args(["/C", "ping -n 30 127.0.0.1 >nul"]);
        let err = output(slow, "cmd", Duration::from_millis(500)).unwrap_err();
        assert!(matches!(err, ProcError::Timeout(_)), "{err}");
    }

    /// #834: a helper (here a stand-in that prints where it runs and its temporary folders)
    /// starts in the helpers' folder, not in MDM's own working folder, and has it as TEMP, TMP
    /// and TMPDIR.
    #[test]
    fn a_helper_runs_in_the_helpers_folder() {
        let dir = crate::workdir::folders()
            .dir
            .clone()
            .expect("a temporary folder that can be written");
        #[cfg(windows)]
        let cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", "cd& echo %TEMP%& echo %TMP%& echo %TMPDIR%"]);
            c
        };
        #[cfg(unix)]
        let cmd = {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", r#"pwd -P; echo "$TEMP"; echo "$TMP"; echo "$TMPDIR""#]);
            c
        };
        let out = output(cmd, "stand-in", Duration::from_secs(10)).unwrap();
        let out = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = out.lines().map(str::trim).collect();
        assert_eq!(lines.len(), 4, "{out}");
        let real = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(real(Path::new(lines[0])), real(&dir), "{out}");
        assert_ne!(
            real(Path::new(lines[0])),
            real(&std::env::current_dir().unwrap())
        );
        for temp in &lines[1..] {
            assert_eq!(Path::new(temp), dir, "{out}");
        }
    }

    /// #1216: yt-dlp's and gallery-dl's commands are built to start in the helpers' folder, with
    /// it as TEMP, TMP and TMPDIR, whatever folder MDM itself was started in
    /// (`C:\Windows\System32` when Windows starts it with the session): yt-dlp made the file
    /// it tries a format with there, and Windows refused it.
    #[test]
    fn the_helpers_commands_start_in_the_helpers_folder() {
        // The settings' proxy is global (`set_proxy`): one test sets it, others assume none.
        let _proxy = GLOBAL_PROXY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = crate::workdir::folders()
            .dir
            .clone()
            .expect("a temporary folder that can be written");
        let exe = Path::new("yt-dlp");
        let tool = crate::Installed {
            version: "1.32.0".into(),
            exe: "gallery-dl".into(),
        };
        let built = [
            ytdlp(exe, &[], &["-J".into()], "https://x.test/v").unwrap(),
            ytdlp_info(exe, &[], &[], Path::new("/info.json"), "https://x.test/v").unwrap(),
            gallery_dl(&tool, &[], "https://x.test/g").unwrap(),
        ];
        for cmd in &built {
            let program = cmd.get_program().to_string_lossy();
            assert_eq!(cmd.get_current_dir(), Some(dir.as_path()), "{program}");
            for var in crate::workdir::TEMP_VARS {
                let value = cmd.get_envs().find(|(k, _)| *k == var).and_then(|(_, v)| v);
                assert_eq!(value, Some(dir.as_os_str()), "{program} {var}");
            }
            // And nothing it inherits is taken away: the rest of MDM's environment goes too.
            assert!(cmd.get_envs().all(|(_, v)| v.is_some()), "{program}");
        }
    }

    /// #61: killing a helper ends what it started too (yt-dlp.exe is a launcher whose child
    /// downloads). Here a script starts a child that writes a line every second for a minute,
    /// holding the helper's stderr as yt-dlp's child does: once killed, the stderr closes (every
    /// process that held it has ended), long before the child's minute, and the file stops
    /// growing.
    #[cfg(windows)]
    #[test]
    fn killing_a_helper_ends_its_children() {
        use std::sync::atomic::Ordering;
        /// Long enough for anything the test waits for to come on the slowest runner, and half
        /// the child's minute: a child left running holds the stderr past it.
        const STUCK: Duration = Duration::from_secs(30);
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.txt");
        let child = dir.path().join("child.cmd");
        let parent = dir.path().join("parent.cmd");
        std::fs::write(
            &child,
            format!(
                "@for /l %%i in (1,1,60) do @(echo x>>\"{}\" & ping -n 2 127.0.0.1 >nul)\r\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::write(
            &parent,
            format!(
                "@start \"\" /b cmd /c \"{}\"\r\n@ping -n 60 127.0.0.1 >nul\r\n",
                child.display()
            ),
        )
        .unwrap();
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(&parent);
        let mut s = spawn(cmd, "cmd").unwrap();
        let size = || std::fs::metadata(&marker).map_or(0, |m| m.len());
        let start = Instant::now();
        while size() == 0 {
            assert!(start.elapsed() < STUCK, "the child never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !s.stderr_done.load(Ordering::Acquire),
            "the stderr closed early"
        );
        s.kill();
        let start = Instant::now();
        while !s.stderr_done.load(Ordering::Acquire) {
            assert!(start.elapsed() < STUCK, "the child outlived the helper");
            std::thread::sleep(Duration::from_millis(20));
        }
        let after_kill = size();
        // Nothing left to write it: the same a child's second later.
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(size(), after_kill, "the child ended with the helper");
    }

    /// #1301: the same on Linux and macOS, where `yt_dlp_linux` is a launcher whose child
    /// #1521: a helper's output is read to its end while the helper runs, and once the helper
    /// has ended, for [`AFTER_END`] more at most: an output something else still holds open
    /// ends the reading then, said; one closed ends it at once.
    #[test]
    fn an_output_held_open_is_read_only_a_while_after_the_end() {
        use std::io::Write;
        // Held open: its writer kept.
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"one\ntwo\n").unwrap();
        let mut output = Output::read(reader, 1024);
        let running = || false;
        assert_eq!(output.next_line(running).as_deref(), Some(&b"one\n"[..]));
        assert_eq!(output.next_line(running).as_deref(), Some(&b"two\n"[..]));
        // Lines written after the end, within the time, still come.
        writer.write_all(b"last\n").unwrap();
        let start = Instant::now();
        assert_eq!(output.next_line(|| true).as_deref(), Some(&b"last\n"[..]));
        assert_eq!(output.next_line(|| true), None);
        let waited = start.elapsed();
        assert!(output.held_open());
        assert!(
            waited >= AFTER_END && waited < AFTER_END * 5,
            "read for {waited:?} after the end"
        );
        drop(writer);
        // Closed: the end, at once and not said, whether the helper is seen ended or not.
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"only").unwrap();
        drop(writer);
        let mut output = Output::read(reader, 1024);
        let start = Instant::now();
        assert_eq!(output.next_line(running).as_deref(), Some(&b"only"[..]));
        assert_eq!(output.next_line(running), None);
        assert!(!output.held_open());
        assert!(start.elapsed() < AFTER_END, "{:?}", start.elapsed());
        // A line longer than the most read at once comes in pieces.
        let mut output = Output::read(std::io::Cursor::new(b"abcdefgh\n".to_vec()), 4);
        let mut pieces = Vec::new();
        while let Some(piece) = output.next_line(running) {
            pieces.push(piece);
        }
        assert_eq!(pieces, [&b"abcd"[..], b"efgh", b"\n"]);
    }

    /// #1301: killing a helper (Stop, MDM's end) ends what it started too, as Windows' job
    /// downloads: a shell script starts a child that writes a line every second for a minute,
    /// holding the helper's stderr. Killed, the stderr closes long before the child's minute
    /// (its process group ended with the script), and the file stops growing.
    #[cfg(unix)]
    #[test]
    fn killing_a_helper_ends_its_children() {
        use std::sync::atomic::Ordering;
        const STUCK: Duration = Duration::from_secs(30);
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.txt");
        let child = dir.path().join("child.sh");
        let parent = dir.path().join("parent.sh");
        std::fs::write(
            &child,
            "i=0\nwhile [ $i -lt 60 ]; do echo x >> \"$1\"; sleep 1; i=$((i+1)); done\n",
        )
        .unwrap();
        // The child in the background (no `exec`): the script stays, as the launcher does.
        std::fs::write(&parent, "/bin/sh \"$1\" \"$2\" &\nsleep 60\n").unwrap();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(&parent).arg(&child).arg(&marker);
        let mut s = spawn(cmd, "sh").unwrap();
        let size = || std::fs::metadata(&marker).map_or(0, |m| m.len());
        let start = Instant::now();
        while size() == 0 {
            assert!(start.elapsed() < STUCK, "the child never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !s.stderr_done.load(Ordering::Acquire),
            "the stderr closed early"
        );
        s.kill();
        let _ = s.child.wait();
        let start = Instant::now();
        while !s.stderr_done.load(Ordering::Acquire) {
            assert!(start.elapsed() < STUCK, "the child outlived the helper");
            std::thread::sleep(Duration::from_millis(20));
        }
        let after_kill = size();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(size(), after_kill, "the child ended with the helper");
    }
}
