//! A fake Rapidgator for the engine's tests. Its host names are told apart by the `Host` header
//! (the tests resolve them to the lab server, `HOSTS`), and each answers as Rapidgator does for a
//! signed-in premium account: a file's page (`/file/<id>/<name>.html`) holds a download link on a
//! file host (`/download/<token>`), which gives the file for a while only (from its first request:
//! `lasting_link`) and then answers 404 Not Found, as the real one did at 90 % of a 3 GB file.
//! Without the account's session cookie ([`COOKIE`]) the page has no link (a free account's or a
//! signed-out visitor's page). The pages are written here, not copied from Rapidgator.
//!
//! An id says what it is ([`id`]): the file's size, how long a link its page hands out lasts,
//! how many requests a link answers, and whether the page redirects to its link (a premium
//! account's "direct downloads") instead of showing it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::HttpBody;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

use crate::server::{LogEntry, Shared, serve, unix_ms};

/// The pages' host, and the file host its links lead to.
pub const SITE: &str = "rapidgator.net";
pub const FILES: &str = "s118.rapidgator.net";
/// Another of its file servers, with its own copy of each file (its own ETag and date): a fresh
/// link may lead there (`Spec::mirrors`).
pub const FILES2: &str = "s113.rapidgator.net";
/// The host names the fake Rapidgator answers on: resolve them to the lab server.
pub const HOSTS: [&str; 3] = [SITE, FILES, FILES2];
/// The name its files are served under.
pub const FILE_NAME: &str = "Lab.Archive.part1.rar";
/// The premium account's session, as the browser sends it (a lab name: Rapidgator's own cookies
/// aren't known here, nor needed).
pub const COOKIE: &str = "lab_session=premium";

/// What an id says (`id`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spec {
    size: String,
    /// How long a link the page hands out lasts, from its first request.
    ttl_ms: u64,
    /// How many requests a link answers before 404 (None: as many as come while it lasts).
    uses: Option<u64>,
    /// The page redirects to its link.
    direct: bool,
    /// Its page's fresh links lead to another file server (`FILES2`) than the first link's
    /// (`FILES`), each with its own copy.
    mirrors: bool,
}

/// An id: `lab<SIZE>t<TTL ms>[u<USES>][m][d]` (`lab4Mt1500`, `lab1Mt600000u1d`): a file of
/// `size` (`64K`, `4M`) whose page hands out links lasting `ttl_ms`, each answering `uses`
/// requests (None: any number), the page redirecting to it when `direct`.
pub fn id(size: &str, ttl_ms: u64, uses: Option<u64>, direct: bool) -> String {
    let mut id = format!("lab{size}t{ttl_ms}");
    if let Some(n) = uses {
        id.push_str(&format!("u{n}"));
    }
    if direct {
        id.push('d');
    }
    id
}

/// [`id`] whose page's fresh links lead to another file server than the first link's, each with
/// its own copy of the file (its own ETag and Last-Modified), as Rapidgator's may.
pub fn mirrored_id(size: &str, ttl_ms: u64, uses: Option<u64>, direct: bool) -> String {
    let mut id = format!("lab{size}t{ttl_ms}");
    if let Some(n) = uses {
        id.push_str(&format!("u{n}"));
    }
    id.push('m');
    if direct {
        id.push('d');
    }
    id
}

fn parse(id: &str) -> Option<Spec> {
    let rest = id.strip_prefix("lab")?;
    let (rest, direct) = match rest.strip_suffix('d') {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let (rest, mirrors) = match rest.strip_suffix('m') {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let (size, rest) = rest.split_once('t')?;
    crate::content::parse_size(size)?;
    let (ttl, uses) = match rest.split_once('u') {
        Some((ttl, uses)) => (ttl, Some(uses.parse().ok()?)),
        None => (rest, None),
    };
    Some(Spec {
        size: size.to_owned(),
        ttl_ms: ttl.parse().ok()?,
        uses,
        direct,
        mirrors,
    })
}

/// The content seed of the file `id` (`content::fill`).
pub fn seed(id: &str) -> u64 {
    // FNV-1a.
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The file `id`'s page, at the lab's `port`: where the browser's tab was.
pub fn file_page(port: u16, id: &str) -> String {
    format!("http://{SITE}:{port}/file/{id}/{FILE_NAME}.html")
}

/// Links handed out so far, for their tokens (each its own).
static LINKS: AtomicU64 = AtomicU64::new(0);
/// Requests each link with a number of uses answered.
static USED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// First requests of the links that last from it ([`lasting_link`]), by token (unix ms).
static FIRST: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// A new link to the file `id` on the file host, at the lab's `port`, good until `expires` (unix
/// ms), however late it is first asked: one the browser handed over a while before.
pub fn file_link(port: u16, id: &str, expires: u64) -> String {
    let n = LINKS.fetch_add(1, Ordering::SeqCst);
    format!("http://{FILES}:{port}/download/{expires}-{n}-{id}")
}

/// A new link to the file `id` on the file host, at the lab's `port`, good for `ttl_ms` from the
/// first request made of it: what a file's page gives, and what the browser hands over just
/// after. Its time runs from the download's first request, not from when it was made: a
/// download's first request may come late on a busy computer (a test's link of 700 ms, made
/// before the download started, had expired before it asked on a busy CI runner).
pub fn lasting_link(port: u16, id: &str, ttl_ms: u64) -> String {
    let n = LINKS.fetch_add(1, Ordering::SeqCst);
    format!("http://{FILES}:{port}/download/f{ttl_ms}-{n}-{id}")
}

/// When the link `token` expires: its `expires` part (unix ms), or `f<ttl>` after its first
/// request ([`lasting_link`]), this one when it is the first.
fn expiry(token: &str, expires: &str) -> Option<u64> {
    match expires.strip_prefix('f') {
        Some(ttl) => {
            let ttl: u64 = ttl.parse().ok()?;
            let mut first = FIRST.lock().unwrap();
            let at = *first
                .get_or_insert_with(HashMap::new)
                .entry(token.to_owned())
                .or_insert_with(unix_ms);
            Some(at + ttl)
        }
        None => expires.parse().ok(),
    }
}

/// Every request on Rapidgator's host names (`server::by_host`).
pub(crate) async fn handle(
    shared: Arc<Shared>,
    peer: SocketAddr,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    host: &str,
    port: &str,
) -> Response {
    let path = uri.path().to_owned();
    let query = uri.query().map(str::to_owned);
    let entry = shared.begin(peer, &method, path.clone(), query.clone(), &headers, &[]);
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let premium = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|c| c.trim() == COOKIE);
    match (host, segs.as_slice()) {
        // A file's page: a premium account's has a fresh link (or redirects to it); anyone
        // else's only the slow way, which the browser takes.
        (SITE, ["file", id, ..]) => match parse(id) {
            Some(spec) if premium => {
                let mut link = lasting_link(port.parse().unwrap_or(80), id, spec.ttl_ms);
                // A mirrored file's fresh links lead to its other server.
                if spec.mirrors {
                    link = link.replacen(FILES, FILES2, 1);
                }
                if spec.direct {
                    redirect(&link, shared, entry)
                } else {
                    page(StatusCode::OK, &premium_page(&link), shared, entry)
                }
            }
            Some(_) => page(StatusCode::OK, FREE_PAGE, shared, entry),
            None => page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry),
        },
        // The file, while its link lasts (and has uses left); 404 after.
        (FILES | FILES2, ["download", token]) => {
            let mut parts = token.splitn(3, '-');
            let (expires, id) = (parts.next().and_then(|e| expiry(token, e)), parts.nth(1));
            let (Some(expires), Some(id)) = (expires, id) else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            let Some(spec) = parse(id) else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            let spent = spec.uses.is_some_and(|uses| {
                let mut used = USED.lock().unwrap();
                let n = used
                    .get_or_insert_with(HashMap::new)
                    .entry((*token).to_owned())
                    .or_insert(0);
                *n += 1;
                *n > uses
            });
            if unix_ms() >= expires || spent {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            }
            // Its faults (`rate=`…) as the lab's other files take them, after its own.
            let mut q = format!("seed={}", seed(id));
            if host == FILES2 {
                q.push_str("&node=1");
            }
            if let Some(more) = query.filter(|q| !q.is_empty()) {
                q.push('&');
                q.push_str(&more);
            }
            serve(shared, entry, &method, &spec.size, Some(FILE_NAME), &q).await
        }
        _ => page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry),
    }
}

const NOT_FOUND: &str = "<!doctype html><title>404 Not Found</title><h1>404 Not Found</h1>";

/// A free account's (or a signed-out visitor's) file page: the slow way only, started by a
/// script on the site's own host, and a sign-in link.
const FREE_PAGE: &str = "<!DOCTYPE html><html><head><title>Download file - Rapidgator</title></head><body>\n\
<a href=\"/auth/login\">Log in</a>\n<a href=\"/article/premium\">Premium</a>\n\
<a class=\"btn\" href=\"/download/AjaxStartTimer?fid=1\">Slow download</a>\n\
<script>var getLink = '/download/AjaxGetDownloadLink';</script>\n</body></html>";

/// A premium account's file page: the site's links first, then the download button, its
/// attributes on lines of their own, and a long page after it.
fn premium_page(link: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>Download file {FILE_NAME} - Rapidgator</title></head><body>\n\
         <a href=\"/article/premium\">Premium</a>\n<a href=\"/profile/index\">My account</a>\n\
         <div class=\"btm\"><a class=\"btn btn-download\"\n    href=\"{link}\"\n    >Download</a></div>\n\
         <!-- {} -->\n</body></html>",
        "x".repeat(60 << 10)
    )
}

fn page(status: StatusCode, html: &str, shared: Arc<Shared>, mut entry: LogEntry) -> Response {
    let resp = (status, Html(html.to_owned())).into_response();
    let len = resp.body().size_hint().exact().unwrap_or(0);
    entry.body_len = len;
    entry.bytes_sent = len;
    shared.record(entry, status.as_u16(), "page");
    resp
}

fn redirect(to: &str, shared: Arc<Shared>, entry: LogEntry) -> Response {
    shared.record(entry, 302, "redirect");
    (StatusCode::FOUND, [(header::LOCATION, to.to_owned())]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_say_size_link_lifetime_uses_and_redirect() {
        let i = id("4M", 1500, None, false);
        assert_eq!(i, "lab4Mt1500");
        assert!(i.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(
            parse(&i),
            Some(Spec {
                size: "4M".into(),
                ttl_ms: 1500,
                uses: None,
                direct: false,
                mirrors: false,
            })
        );
        assert_eq!(
            parse(&id("64K", 600_000, Some(1), true)),
            Some(Spec {
                size: "64K".into(),
                ttl_ms: 600_000,
                uses: Some(1),
                direct: true,
                mirrors: false,
            })
        );
        assert_eq!(parse("6e2d0c8a3a7b4f1e9d5c2b1a0f9e8d7c"), None);
        assert_eq!(parse("labXXt1"), None);
    }
}
