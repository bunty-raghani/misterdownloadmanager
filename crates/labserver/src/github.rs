//! A fake GitHub (#1337): its API refusing every request for its rate limit, as it refuses a
//! shared internet address whose 60 requests an hour others used up, and its release pages
//! answering as github.com's do. Its releases are files in `--files DIR`:
//! `gh/<owner>/<repo>/LATEST` holds the latest release's tag, `gh/<owner>/<repo>/<tag>/` its
//! files.
//!
//! - `GET /gh/api/*` — 403 with `x-ratelimit-remaining: 0` and GitHub's message
//! - `GET /gh/web/<owner>/<repo>/releases/latest` — 302 to `…/releases/tag/<tag>` (to
//!   `…/releases` when the repository has no release)
//! - `GET /gh/web/<owner>/<repo>/releases/tag/<tag>` — the release's page (a stub)
//! - `GET|HEAD /gh/web/<owner>/<repo>/releases/download/<tag>/<file>` — 302 to where the file is
//!   kept (`/gh/assets/…`), 404 when the release has no such file
//! - `GET /gh/assets/<owner>/<repo>/<tag>/<file>` — the file (Range too), with its
//!   `Last-Modified`
//! - `GET /gh/raw/<owner>/<repo>/<tag>/<path>` — a file of the repository at that tag
//!   (raw.githubusercontent.com's addresses)

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::server::{Shared, serve_file};

/// GitHub's answer once the hour's requests are used up.
const RATE_LIMITED: &str = r#"{"message":"API rate limit exceeded for 127.0.0.1. (Authenticated requests get a higher rate limit.)","documentation_url":"https://docs.github.com/rest/using-the-rest-api/rate-limits-for-the-rest-api"}"#;

/// `GET /gh/api/*`: refused for the rate limit, whatever is asked.
pub(crate) async fn api(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let query = uri.query().map(str::to_owned);
    let entry = shared.begin(peer, &method, uri.path().to_owned(), query, &headers, &[]);
    shared.record(entry, 403, "rate-limited");
    (
        StatusCode::FORBIDDEN,
        [
            (
                header::CONTENT_TYPE.as_str(),
                "application/json; charset=utf-8",
            ),
            ("x-ratelimit-limit", "60"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-used", "60"),
            ("x-ratelimit-resource", "core"),
        ],
        RATE_LIMITED,
    )
        .into_response()
}

/// This server's own address, as the request named it (GitHub redirects with whole addresses).
fn origin(headers: &HeaderMap) -> String {
    headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| format!("http://{h}"))
        .unwrap_or_default()
}

/// `s` as one segment of a path (RFC 3986's unreserved characters as they are).
fn segment(s: &str) -> String {
    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(s, SEGMENT).to_string()
}

/// `GET /gh/web/<owner>/<repo>/releases`: the list (a stub).
pub(crate) async fn releases(Path((owner, repo)): Path<(String, String)>) -> Response {
    Html(format!("<h1>{owner}/{repo}: releases</h1>")).into_response()
}

/// `GET|HEAD /gh/web/<owner>/<repo>/releases/<rest>`: `latest`, `tag/<tag>`,
/// `download/<tag>/<file>`.
pub(crate) async fn release(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path((owner, repo, rest)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let base = format!("{}/gh/web/{owner}/{repo}/releases", origin(&headers));
    let logged = format!("/gh/web/{owner}/{repo}/releases/{rest}");
    let entry = shared.begin(peer, &method, logged, None, &headers, &[]);
    let found = |status: u16, outcome: &str| {
        shared.record(entry, status, outcome);
    };
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        ["latest"] => {
            let latest = shared.file_path(&format!("gh/{owner}/{repo}/LATEST"));
            let tag = match latest {
                Some(p) => tokio::fs::read_to_string(p).await.ok(),
                None => None,
            };
            let to = match tag.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(tag) => format!("{base}/tag/{}", segment(tag)),
                None => base,
            };
            found(302, "latest");
            (StatusCode::FOUND, [(header::LOCATION, to)]).into_response()
        }
        ["tag", tag] => {
            found(200, "page");
            Html(format!("<h1>{owner}/{repo} {tag}</h1>")).into_response()
        }
        ["download", tag, file] => {
            let name = format!("gh/{owner}/{repo}/{tag}/{file}");
            if shared.file_path(&name).is_some_and(|p| p.is_file()) {
                found(302, "download");
                let to = format!(
                    "{}/gh/assets/{owner}/{repo}/{}/{}",
                    origin(&headers),
                    segment(tag),
                    segment(file)
                );
                (StatusCode::FOUND, [(header::LOCATION, to)]).into_response()
            } else {
                found(404, "error");
                (StatusCode::NOT_FOUND, "Not Found").into_response()
            }
        }
        _ => {
            found(404, "error");
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}

/// `GET /gh/assets/<path>`: a release's file, as GitHub's file storage sends it.
pub(crate) async fn asset(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = shared.file_path(&format!("gh/{name}"));
    let logged = format!("/gh/assets/{name}");
    serve_file(&shared, peer, &method, &headers, logged, path, true).await
}

/// `GET /gh/raw/<owner>/<repo>/<tag>/<path>`: a file of the repository at a tag.
pub(crate) async fn raw(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = shared.file_path(&format!("gh/{name}"));
    let logged = format!("/gh/raw/{name}");
    serve_file(&shared, peer, &method, &headers, logged, path, false).await
}
