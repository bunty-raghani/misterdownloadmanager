//! End-to-end: downloads a browser form starts (`Options::post`), against a small server of
//! their own (the lab server answers GET only), and the hook that sees the part file created.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_engine::{Destination, Download, FileHook, Options, Request};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request as the server saw it.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: Vec<u8>,
    content_type: Option<String>,
    range: Option<String>,
}

/// What the server answers: status, headers, body.
type Answer = (u16, Vec<(String, String)>, Vec<u8>);
type Handler = Arc<dyn Fn(&Seen) -> Answer + Send + Sync>;

struct Server {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// The most requests whose answers were being sent at the same time.
    peak: Arc<AtomicUsize>,
}

impl Server {
    async fn start(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let peak = Arc::new(AtomicUsize::new(0));
        let now = Arc::new(AtomicUsize::new(0));
        let handler: Handler = Arc::new(handler);
        let (s, p) = (seen.clone(), peak.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (s, p, now, handler) = (s.clone(), p.clone(), now.clone(), handler.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, &s, &p, &now, &handler).await;
                });
            }
        });
        Server { base, seen, peak }
    }

    fn url(&self, path: &str) -> url::Url {
        format!("{}{path}", self.base).parse().unwrap()
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// One request per connection: read it, answer it slowly enough (64 KiB every 5 ms) that a
/// download would open its other connections meanwhile.
async fn serve(
    mut stream: TcpStream,
    seen: &Mutex<Vec<Seen>>,
    peak: &AtomicUsize,
    now: &AtomicUsize,
    handler: &Handler,
) -> std::io::Result<()> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path) = (
        first.next().unwrap_or("").to_owned(),
        first.next().unwrap_or("").to_owned(),
    );
    let header = |name: &str| {
        head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = raw[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    let req = Seen {
        method,
        path,
        body,
        content_type: header("content-type"),
        range: header("range"),
    };
    seen.lock().unwrap().push(req.clone());
    let (status, headers, data) = handler(&req);
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        data.len()
    );
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes()).await?;
    peak.fetch_max(now.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
    let mut sent = Ok(());
    for chunk in data.chunks(64 * 1024) {
        sent = stream.write_all(chunk).await;
        if sent.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    now.fetch_sub(1, Ordering::SeqCst);
    sent
}

/// The file's bytes.
fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// `data` as a full answer, or the part a `Range: bytes=a-b` asks (when `ranges`).
fn file(seen: &Seen, all: &[u8], ranges: bool) -> Answer {
    let mut headers = vec![("Content-Type".into(), "application/octet-stream".into())];
    let asked = seen.range.as_deref().and_then(|r| {
        let (a, b) = r.strip_prefix("bytes=")?.split_once('-')?;
        let a: usize = a.parse().ok()?;
        let b: usize = b
            .parse::<usize>()
            .map_or(all.len() - 1, |b| b.min(all.len() - 1));
        Some((a, b))
    });
    match asked.filter(|_| ranges) {
        Some((a, b)) => {
            headers.push((
                "Content-Range".into(),
                format!("bytes {a}-{b}/{}", all.len()),
            ));
            (206, headers, all[a..=b].to_vec())
        }
        None => {
            if ranges {
                headers.push(("Accept-Ranges".into(), "bytes".into()));
            }
            (200, headers, all.to_vec())
        }
    }
}

const FORM: &[u8] = b"report=2026&format=zip";

fn form_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    h
}

fn post_opts() -> Options {
    Options {
        post: Some(bytes::Bytes::from_static(FORM)),
        retry_delay: Duration::from_millis(50),
        ..Options::default()
    }
}

/// Download `path` from `s` as the form's POST into a temp dir; checks the file is `want` and
/// returns its name.
async fn fetch(s: &Server, path: &str, opts: Options, want: &[u8]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url(path),
        headers: form_headers(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .expect("download succeeds");
    let got = std::fs::read(&out.path).unwrap();
    assert!(
        got == want,
        "content mismatch ({} of {} bytes)",
        got.len(),
        want.len()
    );
    out.path.file_name().unwrap().to_string_lossy().into_owned()
}

/// #376: Cloudflare's challenge, as it marks it, is reported as the site's bot check; another
/// 403 stays a 403. It is asked once more the way a browser connects (#668), never more: a
/// challenge that one meets too stays the bot check's refusal.
#[tokio::test]
async fn a_bot_check_is_named() {
    let s = Server::start(|seen| {
        let headers = if seen.path.starts_with("/challenge") {
            vec![("Cf-Mitigated".into(), "challenge".into())]
        } else {
            Vec::new()
        };
        (403, headers, b"forbidden".to_vec())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let attempt = |path: &str| {
        Download::start(
            Request {
                url: s.url(path),
                headers: HeaderMap::new(),
                dest: Destination::Dir(dir.path().to_owned()),
            },
            Options::default(),
        )
    };
    let challenge = attempt("/challenge/f.dat").wait().await.unwrap_err();
    assert!(
        matches!(challenge, mdm_engine::Error::BotCheck),
        "{challenge:?}"
    );
    let asked = if cfg!(feature = "browser-connection") {
        2
    } else {
        1
    };
    assert_eq!(
        s.seen.lock().unwrap().len(),
        asked,
        "a bot check is asked once more at most"
    );
    let plain = attempt("/plain/f.dat").wait().await.unwrap_err();
    assert!(matches!(plain, mdm_engine::Error::Status(_)), "{plain:?}");
}

fn is_form_post(r: &Seen) -> bool {
    r.method == "POST"
        && r.body == FORM
        && r.content_type.as_deref() == Some("application/x-www-form-urlencoded")
}

#[tokio::test]
async fn the_form_is_posted_and_the_answer_saved() {
    let all = data(300 * 1024);
    let file_data = all.clone();
    let s = Server::start(move |r| {
        if is_form_post(r) {
            let (st, mut h, b) = file(r, &file_data, false);
            h.push((
                "Content-Disposition".into(),
                "attachment; filename=\"report.zip\"; filename*=UTF-8''r%C3%A9sum%C3%A9.zip".into(),
            ));
            (st, h, b)
        } else {
            (400, vec![], b"not the form".to_vec())
        }
    })
    .await;
    let name = fetch(&s, "/export", post_opts(), &all).await;
    assert_eq!(name, "résumé.zip", "filename* wins over its ASCII fallback");
    let seen = s.seen();
    assert!(seen.iter().all(is_form_post), "{seen:?}");
    // No ranges shown: the form is posted once, its answer is the file.
    assert_eq!(seen.len(), 1, "{seen:?}");
}

#[tokio::test]
async fn the_probe_posts_the_form_too() {
    let all = data(1000);
    let file_data = all.clone();
    let s = Server::start(move |r| {
        if is_form_post(r) {
            file(r, &file_data, false)
        } else {
            (405, vec![], vec![])
        }
    })
    .await;
    let info = mdm_engine::probe(s.url("/export"), form_headers(), &post_opts())
        .await
        .expect("probe succeeds");
    assert_eq!(info.size, Some(1000));
    assert!(s.seen().iter().all(is_form_post));
}

#[tokio::test]
async fn a_303_after_the_post_is_followed_with_get() {
    let all = data(2 << 20);
    let file_data = all.clone();
    let s = Server::start(move |r| match r.path.as_str() {
        "/submit" if is_form_post(r) => {
            (303, vec![("Location".into(), "/result.bin".into())], vec![])
        }
        // The result takes ranges: its connections are GETs too.
        "/result.bin" if r.method == "GET" => file(r, &file_data, true),
        _ => (405, vec![], vec![]),
    })
    .await;
    fetch(&s, "/submit", post_opts(), &all).await;
    let seen = s.seen();
    let submits: Vec<_> = seen.iter().filter(|r| r.path == "/submit").collect();
    assert_eq!(submits.len(), 1, "{seen:?}");
    assert!(is_form_post(submits[0]));
    let gets: Vec<_> = seen.iter().filter(|r| r.path == "/result.bin").collect();
    assert!(gets.len() > 1, "segmented once ranges showed: {seen:?}");
    for r in gets {
        assert!(
            r.method == "GET" && r.body.is_empty() && r.content_type.is_none(),
            "the form's body and type stay behind: {r:?}"
        );
    }
}

#[tokio::test]
async fn a_307_after_the_post_keeps_the_post() {
    let all = data(200 * 1024);
    let file_data = all.clone();
    let s = Server::start(move |r| match r.path.as_str() {
        "/submit" if is_form_post(r) => (307, vec![("Location".into(), "/again".into())], vec![]),
        "/again" if is_form_post(r) => file(r, &file_data, false),
        _ => (405, vec![], vec![]),
    })
    .await;
    fetch(&s, "/submit", post_opts(), &all).await;
    let seen = s.seen();
    assert!(seen.iter().all(is_form_post), "{seen:?}");
    assert_eq!(seen.len(), 2, "{seen:?}");
}

#[tokio::test]
async fn a_post_that_ignores_ranges_comes_on_one_connection() {
    let all = data(3 << 20);
    let file_data = all.clone();
    let s = Server::start(move |r| {
        if is_form_post(r) {
            file(r, &file_data, false)
        } else {
            (405, vec![], vec![])
        }
    })
    .await;
    fetch(&s, "/export", post_opts(), &all).await;
    let seen = s.seen();
    assert_eq!(seen.len(), 1, "never a second POST: {seen:?}");
    assert!(seen[0].range.is_none());
    assert_eq!(s.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_post_that_claims_ranges_but_ignores_them_still_completes() {
    let all = data(3 << 20);
    let file_data = all.clone();
    let s = Server::start(move |r| {
        if is_form_post(r) {
            // Says "Accept-Ranges: bytes", then answers every range with the whole file.
            let (_, mut h, b) = file(
                &Seen {
                    range: None,
                    ..r.clone()
                },
                &file_data,
                true,
            );
            h.retain(|(k, _)| k != "Content-Range");
            (200, h, b)
        } else {
            (405, vec![], vec![])
        }
    })
    .await;
    fetch(&s, "/export", post_opts(), &all).await;
    assert!(s.seen().iter().all(is_form_post));
}

/// `Accept-Ranges` on the form's answer is no proof: a form may do something each time it is
/// posted, so only a range actually answered (206) lets more connections post it again.
#[tokio::test]
async fn a_post_that_only_announces_ranges_is_posted_once() {
    let all = data(4 << 20);
    let file_data = all.clone();
    let s = Server::start(move |r| {
        if is_form_post(r) {
            file(r, &file_data, true)
        } else {
            (405, vec![], vec![])
        }
    })
    .await;
    fetch(&s, "/export", post_opts(), &all).await;
    let seen = s.seen();
    assert!(seen.iter().all(is_form_post), "the form, every time");
    assert_eq!(seen.len(), 1, "posted once: {seen:?}");
    assert_eq!(s.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_hook_sees_the_part_file_before_any_data() {
    let all = data(500 * 1024);
    let file_data = all.clone();
    let s = Server::start(move |r| file(r, &file_data, true)).await;
    let created = Arc::new(Mutex::new(Vec::new()));
    let c = created.clone();
    let opts = Options {
        on_file_created: Some(FileHook::new(move |p| {
            c.lock().unwrap().push(p.to_owned());
            // What the app does on Windows: Mark of the Web in an alternate data stream.
            #[cfg(windows)]
            {
                let mut ads = p.as_os_str().to_owned();
                ads.push(":Zone.Identifier");
                std::fs::write(ads, "[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
            }
        })),
        ..Options::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/file.bin"),
        headers: HeaderMap::new(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts).wait().await.unwrap();
    let created = created.lock().unwrap().clone();
    assert_eq!(
        created,
        vec![mdm_engine::naming::part_path(&out.path)],
        "once, the part file"
    );
    assert!(std::fs::read(&out.path).unwrap() == all);
    // The stream moved with the part file to the final name.
    #[cfg(windows)]
    {
        let mut ads = out.path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        assert_eq!(
            std::fs::read_to_string(ads).unwrap(),
            "[ZoneTransfer]\r\nZoneId=3\r\n"
        );
    }
}
