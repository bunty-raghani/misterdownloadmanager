//! A fake LuluStream for the engine's tests. Its host names are told apart by the `Host` header
//! (the tests resolve them to the lab server, `HOSTS`), and each answers as LuluStream does for a
//! free video: a watch page (`/<code>`), a download page (`/d/<code>`) linking a page per quality
//! on another of its domains (`/d/<code>_<mode>`), whose form, posted back with the hash the page
//! gave, answers with a link on a file server that gives the file for a while. The pages are
//! written here, not copied from LuluStream.
//!
//! A code says what it is ([`code`]): the file's size, how long its link lasts, and how the form
//! behaves (`lab`: as the site's; `fla`: it takes the post on the third try only, as the real
//! site's "Security error" now and then; `exp`: its first post's link lasts the code's time, its
//! next ones' ten minutes, for a link that expires during a download; `cap`: it wants a captcha; `bad`: it never
//! takes it). Posts are counted per code in the server's own state, so each `LabServer` starts
//! again.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::HttpBody;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

use crate::server::{LogEntry, Shared, serve, unix_ms};

/// The domain its links are given on, the mirror its download page links its qualities on, and
/// the file server its form's answer leads to.
pub const SITE: &str = "lplaygo.com";
pub const MIRROR: &str = "lulustream.com";
pub const FILES: &str = "s1.lulu-files.test";
/// The host names the fake LuluStream answers on: resolve them to the lab server.
pub const HOSTS: [&str; 3] = [SITE, MIRROR, FILES];
/// The name its files are served under.
pub const FILE_NAME: &str = "Lab video.mp4";

/// How a code's form behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// As the site's: posted back with its hash, it answers with the file's link.
    Plain,
    /// It answers its first two posts with the form again, and takes the third.
    Flaky,
    /// Its first post's link lasts the code's time only, the next ones' ten minutes.
    Expiring,
    /// It wants a captcha (`data-sitekey`).
    Captcha,
    /// It never takes the post: its answer is the form again.
    Refusing,
}

/// A video's code: 12 lowercase letters and digits, `<kind><size>t<link's life, s>` padded with
/// `z` (`lab2mt600zzz`): a file of `size` (`64k`, `2m`) whose links last `ttl_s` seconds.
pub fn code(form: Form, size: &str, ttl_s: u64) -> String {
    let kind = match form {
        Form::Plain => "lab",
        Form::Flaky => "fla",
        Form::Expiring => "exp",
        Form::Captcha => "cap",
        Form::Refusing => "bad",
    };
    let mut code = format!("{kind}{}t{ttl_s}", size.to_ascii_lowercase());
    assert!(code.len() <= 12, "{code}");
    while code.len() < 12 {
        code.push('z');
    }
    code
}

/// What a code says: its form, size and its links' life.
fn parse(code: &str) -> Option<(Form, String, u64)> {
    let form = match code.get(..3)? {
        "lab" => Form::Plain,
        "fla" => Form::Flaky,
        "exp" => Form::Expiring,
        "cap" => Form::Captcha,
        "bad" => Form::Refusing,
        _ => return None,
    };
    let (size, ttl) = code[3..].trim_end_matches('z').split_once('t')?;
    let size = size.to_ascii_uppercase();
    crate::content::parse_size(&size)?;
    Some((form, size, ttl.parse().ok()?))
}

/// The content seed of the video `code` (`content::fill`).
pub fn seed(code: &str) -> u64 {
    // FNV-1a.
    code.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The hash a quality's page gives its form: when it stops being taken (unix ms), and a check.
fn hash(code: &str, until: u64) -> String {
    format!("{until}-{:x}", seed(code) ^ until)
}

/// The hash is the one a page gave for `code`, and still good.
fn hash_ok(code: &str, hash: &str) -> bool {
    hash.split_once('-')
        .and_then(|(until, _)| until.parse::<u64>().ok())
        .is_some_and(|until| until > unix_ms() && self::hash(code, until) == hash)
}

/// How many times `code`'s form has been posted to this server, this one counted.
fn nth_post(shared: &Shared, code: &str) -> u32 {
    let mut posts = shared.lulu_posts.lock().unwrap();
    let n = posts.entry(code.to_owned()).or_insert(0);
    *n += 1;
    *n
}

/// Every request on LuluStream's host names (`server::by_host`), `body` the request's.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle(
    shared: Arc<Shared>,
    peer: SocketAddr,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: &[u8],
    host: &str,
    port: &str,
) -> Response {
    let path = uri.path().to_owned();
    let query = uri.query().map(str::to_owned);
    let entry = shared.begin(peer, &method, path.clone(), query.clone(), &headers, body);
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let pages = matches!(host, SITE | MIRROR);
    match (pages, host, segs.as_slice()) {
        // The watch page and the player: a player, no link to the file.
        (true, _, [code] | ["e", code]) if parse(code).is_some() => {
            page(StatusCode::OK, &watch_page(code), shared, entry)
        }
        // The download page: its qualities, on the mirror (high and normal; no original).
        (true, _, ["d", code]) if parse(code).is_some() => {
            let on = |mode: char| format!("http://{MIRROR}:{port}/d/{code}_{mode}");
            page(
                StatusCode::OK,
                &download_page(&[on('n'), on('h')]),
                shared,
                entry,
            )
        }
        // A quality's page: its form, and its answer when posted.
        (true, _, ["d", last]) => {
            let Some((code, mode)) = last.rsplit_once('_') else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            let Some((form, _, ttl)) = parse(code) else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            if method == Method::POST {
                let nth = nth_post(&shared, code);
                let fields: Vec<(String, String)> =
                    serde_urlencoded::from_bytes(body).unwrap_or_default();
                let field = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
                let form_typed = headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|t| t.starts_with("application/x-www-form-urlencoded"));
                let taken = matches!(form, Form::Plain | Form::Flaky | Form::Expiring)
                    && form_typed
                    && field("op") == Some("download_orig")
                    && field("id") == Some(code)
                    && field("mode") == Some(mode)
                    && field("hash").is_some_and(|h| hash_ok(code, h))
                    && (form != Form::Flaky || nth > 2);
                if taken {
                    let life = match form {
                        Form::Expiring if nth > 1 => 600,
                        _ => ttl,
                    };
                    let until = unix_ms() + life * 1000;
                    let link = format!(
                        "http://{FILES}:{port}/v/01/{code}_{mode}/Lab%20video.mp4?t={until}"
                    );
                    return page(StatusCode::OK, &answer_page(&link), shared, entry);
                }
            }
            let hash = hash(code, unix_ms() + 600_000);
            let page_html = quality_page(code, mode, &hash, form == Form::Captcha);
            page(StatusCode::OK, &page_html, shared, entry)
        }
        // The file, while its link lasts; refused after, as the real one is.
        (false, FILES, ["v", _, folder, _name]) => {
            let until = query
                .as_deref()
                .and_then(|q| serde_urlencoded::from_str::<Vec<(String, String)>>(q).ok())
                .and_then(|q| q.into_iter().find(|(k, _)| k == "t"))
                .and_then(|(_, v)| v.parse::<u64>().ok());
            let spec = folder.rsplit_once('_').and_then(|(code, _)| parse(code));
            let (Some(until), Some((_, size, _))) = (until, spec) else {
                return page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry);
            };
            if unix_ms() >= until {
                return page(StatusCode::FORBIDDEN, FORBIDDEN, shared, entry);
            }
            let code = folder.rsplit_once('_').map_or("", |(c, _)| c);
            let q = format!("seed={}", seed(code));
            serve(shared, entry, &method, &size, Some(FILE_NAME), &q).await
        }
        _ => page(StatusCode::NOT_FOUND, NOT_FOUND, shared, entry),
    }
}

const NOT_FOUND: &str = "<!doctype html><title>LuluStream</title><p>File was deleted.</p>";
const FORBIDDEN: &str = "<html><body><h1>403 Forbidden</h1></body></html>";

/// The watch page: a player, the site's links.
fn watch_page(code: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>Lab video</title></head><body>\n\
         <a href=\"/premium\">Premium</a>\n<iframe src=\"/e/{code}\"></iframe>\n</body></html>"
    )
}

/// The download page: the site's links, a link per quality, the report form.
fn download_page(qualities: &[String]) -> String {
    let links: String = qualities
        .iter()
        .map(|q| format!("<a href=\"{q}\" class=\"btn\">Download</a>\n"))
        .collect();
    format!(
        "<!DOCTYPE html><html><body>\n<a href=\"/premium\">Premium</a>\n{links}\
         <form method=\"POST\"><input type=\"hidden\" name=\"op\" value=\"report_file\"></form>\n\
         </body></html>"
    )
}

/// A quality's page: its form with the hash for this visit; a captcha's when it wants one.
fn quality_page(code: &str, mode: &str, hash: &str, captcha: bool) -> String {
    let captcha = if captcha {
        "<div class=\"g-recaptcha\" data-sitekey=\"lab-key\"></div>\n"
    } else {
        ""
    };
    format!(
        "<!DOCTYPE html><html><body>\n<a href=\"/d/{code}_{mode}#\">here</a>\n\
         <form method=\"POST\" id=\"F1\" action=\"\">\n\
         <input type=\"hidden\" name=\"op\" value=\"download_orig\">\n\
         <input type=\"hidden\" name=\"id\" value=\"{code}\">\n\
         <input type=\"hidden\" name=\"mode\" value=\"{mode}\">\n\
         <input type=\"hidden\" name=\"hash\" value=\"{hash}\">\n\
         {captcha}<button class=\"g-recaptcha btn\">{FILE_NAME}</button></form>\n</body></html>"
    )
}

/// The form's answer: the file's link, among the site's.
fn answer_page(link: &str) -> String {
    let link = link.replace('&', "&amp;");
    format!(
        "<!DOCTYPE html><html><body>\n<a href=\"/premium\">Premium</a>\n\
         <table><tr><td><a href=\"{link}\" class=\"btn\">Direct link</a></td></tr></table>\n\
         </body></html>"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_say_form_size_and_link_life() {
        let c = code(Form::Plain, "2M", 600);
        assert_eq!(c, "lab2mt600zzz");
        assert!(
            c.len() == 12
                && c.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        );
        assert_eq!(parse(&c), Some((Form::Plain, "2M".to_owned(), 600)));
        assert_eq!(
            parse(&code(Form::Captcha, "64K", 1)),
            Some((Form::Captcha, "64K".to_owned(), 1))
        );
        assert_eq!(parse("xmgvk2758mtu"), None);
    }

    #[test]
    fn a_hash_is_taken_for_its_code_while_it_lasts() {
        let c = code(Form::Plain, "1M", 600);
        assert!(hash_ok(&c, &hash(&c, unix_ms() + 60_000)));
        assert!(!hash_ok(&c, &hash(&c, 1)));
        assert!(!hash_ok(
            &code(Form::Plain, "2M", 600),
            &hash(&c, unix_ms() + 60_000)
        ));
        assert!(!hash_ok(&c, "nonsense"));
    }
}
