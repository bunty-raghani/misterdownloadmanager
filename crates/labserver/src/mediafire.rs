//! A fake MediaFire for the engine's tests (#952). MediaFire's host names are told apart by the
//! `Host` header (the tests resolve them to the lab server, `HOSTS`), and each answers as MediaFire
//! does: a file's page holds its download button, whose address on a file host gives the file
//! until its ticket expires and then redirects to the "repair" page, which links the file's page
//! again. The pages are written here, not copied from MediaFire.
//!
//! A key says what it is ([`key`]): the file's size and how long a ticket its page hands out
//! lasts. A file host's address is `/t<expiry, unix ms>/<key>/<name>` ([`file_address`]).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::HttpBody;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

use crate::server::{LogEntry, Shared, serve, unix_ms};

/// The pages' host, and the file host its buttons lead to.
pub const WWW: &str = "www.mediafire.com";
pub const FILES: &str = "download1589.mediafire.com";
/// The host names the fake MediaFire answers on: resolve them to the lab server.
pub const HOSTS: [&str; 2] = [WWW, FILES];
/// The name its files are served under.
pub const FILE_NAME: &str = "Lab file.zip";

/// A key: `lab<SIZE>t<TTL ms>`, letters and digits padded with `z` to 15 (`lab4Mt60000zzzz`): a
/// file of `size` (`64K`, `4M`) whose page hands out tickets lasting `ttl_ms`.
pub fn key(size: &str, ttl_ms: u64) -> String {
    let mut key = format!("lab{size}t{ttl_ms}");
    while key.len() < 15 {
        key.push('z');
    }
    key
}

/// The size and ticket lifetime a key says.
fn parse(key: &str) -> Option<(String, u64)> {
    let rest = key.strip_prefix("lab")?.trim_end_matches('z');
    let (size, ttl) = rest.split_once('t')?;
    crate::content::parse_size(size)?;
    Some((size.to_owned(), ttl.parse().ok()?))
}

/// The content seed of the file `key` (`content::fill`).
pub fn seed(key: &str) -> u64 {
    // FNV-1a.
    key.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The file `key`'s address on the file host, at the lab's `port`, good until `expires` (unix
/// ms): what a file's page links, and what the browser hands over.
pub fn file_address(port: u16, key: &str, expires: u64) -> String {
    format!("http://{FILES}:{port}/t{expires}/{key}/Lab%20file.zip")
}

/// Every request on MediaFire's host names (`server::by_host`).
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
    let at = |host: &str, path: &str| format!("http://{host}:{port}{path}");
    match (host, segs.as_slice()) {
        // A file's page, by any of its addresses: the button with a fresh ticket.
        (WWW, ["file" | "file_premium" | "download", key, ..]) => match parse(key) {
            Some((_, ttl)) => {
                let button = file_address(port.parse().unwrap_or(80), key, unix_ms() + ttl);
                page(StatusCode::OK, &file_page(&button), shared, entry)
            }
            None => page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry),
        },
        // An expired ticket's page: back to the file's page.
        (WWW, ["download_repair.php"]) => {
            let key = uri
                .query()
                .and_then(|q| serde_urlencoded::from_str::<Vec<(String, String)>>(q).ok())
                .and_then(|q| q.into_iter().find(|(k, _)| k == "qkey"))
                .map(|(_, v)| v)
                .unwrap_or_default();
            let link = format!("/file/{key}/Lab%20file.zip/file");
            page(StatusCode::OK, &repair_page(&link), shared, entry)
        }
        // The file, while its ticket lasts; the repair page after.
        (FILES, [ticket, key, _name]) => {
            let expires = ticket.strip_prefix('t').and_then(|t| t.parse::<u64>().ok());
            let (Some(expires), Some((size, _))) = (expires, parse(key)) else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            if unix_ms() >= expires {
                let to = at(
                    WWW,
                    &format!("/download_repair.php?flag=1&dkey={ticket}&qkey={key}&ref=3"),
                );
                return redirect(&to, shared, entry);
            }
            // Its faults (`rate=`…) as the lab's other files take them, after its own.
            let mut q = format!("seed={}", seed(key));
            if let Some(more) = query.filter(|q| !q.is_empty()) {
                q.push('&');
                q.push_str(&more);
            }
            serve(shared, entry, &method, &size, Some(FILE_NAME), &q).await
        }
        _ => page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry),
    }
}

const NOT_FOUND: &str =
    "<!doctype html><title>File not found - MediaFire</title><p>Invalid or deleted file.</p>";

/// A file's page: other links first, then the button, its attributes on lines of their own.
fn file_page(button: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>{FILE_NAME} - MediaFire</title></head><body>\n\
         <a href=\"/login/\">Log in</a>\n<a href=\"/upgrade/\">Upgrade</a>\n\
         <div class=\"download_link\"><a class=\"input popsok\"\n    aria-label=\"Download file\"\n    \
         href=\"{button}\"    id=\"downloadButton\"\n    rel=\"nofollow\">Download</a></div>\n\
         <!-- {} -->\n</body></html>",
        "x".repeat(100 << 10)
    )
}

/// The repair page: what happened, and the way back to the file's page.
fn repair_page(link: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>MediaFire</title></head><body>\n\
         <p>This download link has expired.</p>\n<a href=\"/help/\">Help</a>\n\
         <a href=\"{link}\">Back to the file</a>\n</body></html>"
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
    fn keys_say_size_and_ticket_lifetime() {
        let k = key("4M", 1500);
        assert_eq!(k, "lab4Mt1500zzzzz");
        assert!(k.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(parse(&k), Some(("4M".to_owned(), 1500)));
        assert_eq!(
            parse(&key("64K", 600_000)),
            Some(("64K".to_owned(), 600_000))
        );
        assert_eq!(parse("k4makp2pk726bmb"), None);
        assert_eq!(parse("labXXt1zzzzzzzz"), None);
    }
}
