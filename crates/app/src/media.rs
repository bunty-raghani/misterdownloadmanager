//! A local, token-checked upload channel for pages only they can fetch (Telegram's protected
//! media: its viewer streams through the page's own service worker/session, while an outside
//! request for the same address answers 302). The page sends the media here; MDM writes it as
//! one of its downloads, with its own progress window, never handing the page-only URL to the
//! engine.
//!
//! Bound to 127.0.0.1 only, with a token in `<data>/media.json` (the native host gives it to the
//! extension). The page posts metadata once, then the media in chunks; the app's UI picks the
//! uploads up (`App::media_tick`), creates the row and progress window, and moves the finished
//! file to its category folder.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

/// One chunk is a few megabytes; the limit leaves room for a large first response body.
const MAX_CHUNK: usize = 32 * 1024 * 1024;

pub struct Server {
    pub port: u16,
    pub token: String,
    /// The uploads, by id. Finished ones stay (the UI ignores an item already complete).
    pub uploads: Arc<Mutex<BTreeMap<u64, Arc<Upload>>>>,
    /// Wakes the app's frame loop when a chunk arrives, even while its window is idle.
    ctx: egui::Context,
    next: AtomicU64,
}

pub struct Upload {
    pub id: u64,
    pub name: String,
    pub url: String,
    /// Bytes, when the page knew (0 until its first range answer says).
    pub size: AtomicU64,
    pub size_known: AtomicBool,
    pub mime: Mutex<String>,
    /// The file being written, under the app's data folder.
    pub temp: PathBuf,
    pub downloaded: AtomicU64,
    pub cancel: AtomicBool,
    pub state: Mutex<UploadState>,
    /// The rate window: the page sends 4 MB chunks in bursts, not a steady engine stream, and
    /// two concurrent Telegram downloads can take turns. Keep the last 6 s of (time, bytes)
    /// samples and report their average, so the speed and time left do not blink to 0 between
    /// chunks; it still reaches 0 after 6 s with no data.
    rate_window: Mutex<RateWindow>,
}

struct RateWindow {
    samples: VecDeque<(std::time::Instant, u64)>,
    rate: f64,
}

#[derive(Clone, Debug)]
pub enum UploadState {
    Downloading,
    /// Paused by the person: the page stops sending, but the bytes written so far stay on disk
    /// so Resume can carry on from them (`Server::resume`).
    Paused,
    Done,
    Failed(String),
    Cancelled,
}

impl Upload {
    /// Bytes/s over the last 6 s, sampled at most every 250 ms; 0 when nothing arrived in that
    /// window. The average hides the gaps between the page's chunk uploads.
    pub fn rate(&self) -> f64 {
        const SAMPLE: std::time::Duration = std::time::Duration::from_millis(250);
        const WINDOW: std::time::Duration = std::time::Duration::from_secs(6);
        let now = std::time::Instant::now();
        let downloaded = self.downloaded.load(Ordering::Relaxed);
        let mut w = self.rate_window.lock().unwrap();
        if w.samples
            .back()
            .is_none_or(|(at, _)| now.duration_since(*at) >= SAMPLE)
        {
            w.samples.push_back((now, downloaded));
        }
        while w
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > WINDOW)
        {
            w.samples.pop_front();
        }
        if let (Some(&(first_at, first)), Some(&(last_at, last))) =
            (w.samples.front(), w.samples.back())
        {
            let elapsed = last_at.duration_since(first_at).as_secs_f64();
            if elapsed >= SAMPLE.as_secs_f64() {
                w.rate = last.saturating_sub(first) as f64 / elapsed;
            }
        }
        w.rate
    }
}

impl Server {
    /// Start the local server (a port of the system's choosing) and write `media.json` for the
    /// native host. None when no port can be opened (the feature then just doesn't offer).
    pub fn start(rt: &tokio::runtime::Handle, ctx: &egui::Context) -> Option<Arc<Server>> {
        // Unit tests build App objects; none may take a port or overwrite a running app's
        // media.json.
        if cfg!(test) {
            return None;
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        listener.set_nonblocking(true).ok()?;
        let port = listener.local_addr().ok()?.port();
        let mut random = [0u8; 24];
        aws_lc_rs::rand::fill(&mut random).ok()?;
        use base64::Engine as _;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        let server = Arc::new(Server {
            port,
            token: token.clone(),
            uploads: Default::default(),
            ctx: ctx.clone(),
            next: AtomicU64::new(1),
        });
        let state = server.clone();
        rt.spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                return;
            };
            let app = Router::new()
                .route("/telegram/start", post(start))
                .route("/telegram/chunk", post(chunk))
                .route("/telegram/end", post(end))
                .route("/telegram/cancel", post(cancel))
                .route("/telegram/state", post(upload_state))
                .route("/telegram/fail", post(fail))
                .layer(DefaultBodyLimit::max(MAX_CHUNK))
                .layer(axum::middleware::from_fn(cors))
                .with_state(state);
            let _ = axum::serve(listener, app).await;
        });
        if let Some(dir) = mdm_ipc::data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                dir.join("media.json"),
                serde_json::json!({"port": server.port, "token": token}).to_string(),
            );
        }
        Some(server)
    }

    /// One upload, for the app's Cancel decision.
    pub fn upload(&self, id: u64) -> Option<Arc<Upload>> {
        self.uploads.lock().unwrap().get(&id).cloned()
    }

    pub fn cancel(&self, id: u64) {
        if let Some(u) = self.uploads.lock().unwrap().get(&id) {
            u.cancel.store(true, Ordering::Relaxed);
            *u.state.lock().unwrap() = UploadState::Cancelled;
        }
        self.ctx.request_repaint();
    }

    /// Pause: stop the page sending, but keep the bytes written so Resume can go on from them.
    /// The download stays in the list as Stopped with its progress and its temporary file.
    pub fn pause(&self, id: u64) {
        if let Some(u) = self.uploads.lock().unwrap().get(&id) {
            u.cancel.store(true, Ordering::Relaxed);
            *u.state.lock().unwrap() = UploadState::Paused;
        }
        self.ctx.request_repaint();
    }

    /// Resume a paused upload: let the page send again, starting a fresh rate window. The page
    /// picks up from the bytes already written (it reads them from `/telegram/state`), so none
    /// are fetched or written twice.
    pub fn resume(&self, id: u64) {
        if let Some(u) = self.uploads.lock().unwrap().get(&id) {
            u.cancel.store(false, Ordering::Relaxed);
            *u.state.lock().unwrap() = UploadState::Downloading;
            *u.rate_window.lock().unwrap() = RateWindow {
                samples: VecDeque::from([(
                    std::time::Instant::now(),
                    u.downloaded.load(Ordering::Relaxed),
                )]),
                rate: 0.0,
            };
        }
        self.ctx.request_repaint();
    }

    /// Whether this upload is still bringing bytes in: for the app's Pause and Pause All, which
    /// must see a page upload as a running download (it has no engine job).
    pub fn is_downloading(&self, id: u64) -> bool {
        self.upload(id)
            .is_some_and(|u| matches!(*u.state.lock().unwrap(), UploadState::Downloading))
    }

    /// The item is gone from the list: stop its upload and remove the unfinished file.
    pub fn forget(&self, id: u64) {
        if let Some(u) = self.uploads.lock().unwrap().remove(&id) {
            let _ = std::fs::remove_file(&u.temp);
        }
    }

    pub fn snapshot(&self) -> Vec<Arc<Upload>> {
        self.uploads.lock().unwrap().values().cloned().collect()
    }
}

#[derive(Deserialize)]
struct Start {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mime: String,
}

#[derive(Deserialize)]
struct Id {
    id: u64,
}

/// What the uploader asks while paused: whether MDM resumed (or stopped) it, and how many
/// bytes are on disk so the page can fetch the rest from there.
#[derive(Serialize)]
struct UploadStateReply {
    state: &'static str,
    downloaded: u64,
}

#[derive(Serialize)]
struct Started {
    id: u64,
}

#[derive(Serialize)]
struct ChunkReply {
    #[serde(rename = "continue")]
    more: bool,
    downloaded: u64,
    /// True when MDM paused the upload (as opposed to cancelling it): the page holds its place
    /// and waits for Resume instead of giving up.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    paused: bool,
}

fn allowed(headers: &HeaderMap, server: &Server) -> bool {
    headers
        .get("x-mdm-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == server.token)
}

async fn start(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(req): Json<Start>,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(dir) = mdm_ipc::data_dir().map(|d| d.join("media")) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let id = server.next.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!("{id}.part"));
    if std::fs::File::create(&temp).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let size = req.size.filter(|s| *s > 0);
    let upload = Arc::new(Upload {
        id,
        name: req.name.chars().take(300).collect(),
        url: req.url.chars().take(2000).collect(),
        size: AtomicU64::new(size.unwrap_or(0)),
        size_known: AtomicBool::new(size.is_some()),
        mime: Mutex::new(req.mime.chars().take(100).collect()),
        temp,
        downloaded: AtomicU64::new(0),
        cancel: AtomicBool::new(false),
        state: Mutex::new(UploadState::Downloading),
        rate_window: Mutex::new(RateWindow {
            samples: VecDeque::from([(std::time::Instant::now(), 0)]),
            rate: 0.0,
        }),
    });
    server.uploads.lock().unwrap().insert(id, upload);
    server.ctx.request_repaint();
    Json(Started { id }).into_response()
}

async fn chunk(
    State(server): State<Arc<Server>>,
    Query(q): Query<Id>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(upload) = server.uploads.lock().unwrap().get(&q.id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let state = upload.state.lock().unwrap().clone();
    if upload.cancel.load(Ordering::Relaxed) || !matches!(state, UploadState::Downloading) {
        return Json(ChunkReply {
            more: false,
            downloaded: upload.downloaded.load(Ordering::Relaxed),
            paused: matches!(state, UploadState::Paused),
        })
        .into_response();
    }
    // The page's first chunk carries the total (and type) it learned from the media answer.
    if let Some(total) = headers
        .get("x-mdm-total")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
    {
        upload.size.store(total, Ordering::Relaxed);
        upload.size_known.store(true, Ordering::Relaxed);
    }
    let mut file = match std::fs::OpenOptions::new().append(true).open(&upload.temp) {
        Ok(f) => f,
        Err(e) => {
            *upload.state.lock().unwrap() = UploadState::Failed(e.to_string());
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if let Err(e) = file.write_all(&body) {
        *upload.state.lock().unwrap() = UploadState::Failed(e.to_string());
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let downloaded = upload
        .downloaded
        .fetch_add(body.len() as u64, Ordering::Relaxed)
        + body.len() as u64;
    server.ctx.request_repaint();
    Json(ChunkReply {
        more: !upload.cancel.load(Ordering::Relaxed),
        downloaded,
        paused: false,
    })
    .into_response()
}

async fn end(
    State(server): State<Arc<Server>>,
    Query(q): Query<Id>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(upload) = server.uploads.lock().unwrap().get(&q.id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    *upload.state.lock().unwrap() = UploadState::Done;
    server.ctx.request_repaint();
    StatusCode::NO_CONTENT.into_response()
}

async fn cancel(
    State(server): State<Arc<Server>>,
    Query(q): Query<Id>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(upload) = server.uploads.lock().unwrap().get(&q.id).cloned() {
        upload.cancel.store(true, Ordering::Relaxed);
        *upload.state.lock().unwrap() = UploadState::Cancelled;
    }
    server.ctx.request_repaint();
    StatusCode::NO_CONTENT.into_response()
}

/// The uploader polls this while paused: on Resume MDM lets it send again and it continues
/// from `downloaded`; on Cancel (or if the app went away) it stops for good.
async fn upload_state(
    State(server): State<Arc<Server>>,
    Query(q): Query<Id>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(upload) = server.uploads.lock().unwrap().get(&q.id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let state = match &*upload.state.lock().unwrap() {
        UploadState::Downloading => "Downloading",
        UploadState::Paused => "Paused",
        UploadState::Done => "Done",
        UploadState::Failed(_) => "Failed",
        UploadState::Cancelled => "Cancelled",
    };
    Json(UploadStateReply {
        state,
        downloaded: upload.downloaded.load(Ordering::Relaxed),
    })
    .into_response()
}

async fn fail(
    State(server): State<Arc<Server>>,
    Query(q): Query<Id>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !allowed(&headers, &server) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(upload) = server.uploads.lock().unwrap().get(&q.id).cloned() {
        let why: String = String::from_utf8_lossy(&body).chars().take(300).collect();
        *upload.state.lock().unwrap() = UploadState::Failed(if why.trim().is_empty() {
            "the page's download stopped".into()
        } else {
            why
        });
    }
    server.ctx.request_repaint();
    StatusCode::NO_CONTENT.into_response()
}

/// CORS and Chrome's Private Network Access: the page (HTTPS) posts to 127.0.0.1, a private
/// address. The token is the real check; these headers let the browser make the request.
async fn cors(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let preflight = req.method() == Method::OPTIONS;
    let mut res = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-mdm-token, x-mdm-total"),
    );
    h.insert(
        HeaderName::from_static("access-control-allow-private-network"),
        HeaderValue::from_static("true"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(id: u64, dir: &std::path::Path) -> Arc<Upload> {
        let temp = dir.join(format!("{id}.part"));
        std::fs::write(&temp, b"").unwrap();
        Arc::new(Upload {
            id,
            name: "clip.mp4".into(),
            url: "https://web.telegram.org/k/".into(),
            size: AtomicU64::new(1000),
            size_known: AtomicBool::new(true),
            mime: Mutex::new("video/mp4".into()),
            temp,
            downloaded: AtomicU64::new(200),
            cancel: AtomicBool::new(false),
            state: Mutex::new(UploadState::Downloading),
            rate_window: Mutex::new(RateWindow {
                samples: VecDeque::new(),
                rate: 0.0,
            }),
        })
    }

    /// Pausing keeps the partial file and the byte count; Resume starts the same upload again
    /// from those bytes, so the page fetches only the rest. Cancel still stops it for good.
    #[test]
    fn pause_keeps_the_bytes_and_resume_carries_on() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server {
            port: 0,
            token: "token".into(),
            uploads: Default::default(),
            ctx: egui::Context::default(),
            next: AtomicU64::new(1),
        };
        let u = upload(1, dir.path());
        server.uploads.lock().unwrap().insert(1, u.clone());

        assert!(server.is_downloading(1));
        server.pause(1);
        assert!(!server.is_downloading(1));
        assert!(u.cancel.load(Ordering::Relaxed));
        assert!(matches!(*u.state.lock().unwrap(), UploadState::Paused));
        assert_eq!(
            u.downloaded.load(Ordering::Relaxed),
            200,
            "bytes stay for Resume"
        );
        assert!(u.temp.exists(), "the partial file stays for Resume");

        server.resume(1);
        assert!(server.is_downloading(1));
        assert!(!u.cancel.load(Ordering::Relaxed));
        assert!(matches!(*u.state.lock().unwrap(), UploadState::Downloading));
        assert_eq!(
            u.downloaded.load(Ordering::Relaxed),
            200,
            "no bytes lost on Resume"
        );

        server.cancel(1);
        assert!(!server.is_downloading(1));
        assert!(matches!(*u.state.lock().unwrap(), UploadState::Cancelled));
    }
}
