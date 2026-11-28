//! What yt-dlp's ffmpeg reads of a part (#1662), brought in over MDM's connections.
//!
//! A part is cut by ffmpeg as yt-dlp runs it (`--download-sections`): it seeks in each stream by
//! the stream's own index and reads only what the part needs. Left to itself it reads a whole
//! file's stream over one open-ended request, which YouTube slows to about the video's own pace
//! (a 160 s part of a 720p video took 96 s). So each chosen format's address in the run's page is
//! one of this server's, on 127.0.0.1: ffmpeg asks it as it would ask the site (a `Range` from
//! where it seeks), and the server brings those bytes in as MDM's engine brings a file in: in
//! requests of a few MB at most, several at once ahead of ffmpeg, over the connections Options
//! gives (proxy, network adapter, DNS), under the download's speed limit and the one for all
//! downloads, a new limit taking effect at once. What came is the download's progress.
//!
//! An HLS playlist is served with its addresses (pieces, keys, the init piece, a master
//! playlist's variants) made this server's, so its pieces come the same way.
//!
//! - It listens on 127.0.0.1 only, on a port the system picks, and answers only paths under a
//!   random token of its own, for the addresses it was given and the ones their playlists list
//!   (`http` and `https` only); nothing else is fetched.
//! - A signed address that dies (401, 403, 404, 410) is asked of yt-dlp again once (`relink`),
//!   as the engine's streams are; a part takes minutes, so it rarely comes to that.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use mdm_engine::limiter::Limiter;
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RANGE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use url::Url;

/// A request's head may be at most this long.
const HEAD_MAX: usize = 16 * 1024;
/// ffmpeg has this long to send its request once connected.
const HEAD_TIME: Duration = Duration::from_secs(30);
/// The first request of a read: small, as ffmpeg often reads a header and seeks on.
const FIRST_CHUNK: u64 = 256 * 1024;
/// Requests grow to this (YouTube serves a range of a few MB at most at full speed, a longer one at
/// the video's pace), at most [AHEAD] at once: what ffmpeg doesn't take when it has the part is
/// at most that much.
const MAX_CHUNK: u64 = 1024 * 1024;
/// Requests ahead of what ffmpeg reads, at most (fewer when Options gives fewer connections).
const AHEAD: u32 = 6;
/// What the server holds for ffmpeg in the system's buffer, at most: its send buffer.
const SEND_BUFFER: usize = 128 * 1024;
/// A playlist may be at most this long.
const PLAYLIST_MAX: usize = 8 * 1024 * 1024;
/// How many addresses a run's playlists may list, in all.
const ENTRIES_MAX: usize = 200_000;

/// A stream ffmpeg is to read: the address yt-dlp gave, its headers (`Cookie` among them), its
/// size when yt-dlp knows it, whether it is an HLS playlist, and its format id (for `relink`).
#[derive(Clone, Debug)]
pub struct Source {
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub size: Option<u64>,
    pub playlist: bool,
    pub format_id: String,
    /// About where the part ends in the file (its share of it, and a margin): requests go
    /// ahead of ffmpeg up to there, one small one at a time past it, so little comes that the
    /// part doesn't need.
    pub ends_near: Option<u64>,
}

/// What an entry of the server is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Playlist,
}

struct Entry {
    url: Url,
    headers: HeaderMap,
    kind: Kind,
    /// Its size, once known.
    size: Option<u64>,
    /// The source it is (or whose playlist listed it), for `relink`.
    source: usize,
    /// Asked of yt-dlp again already (once at most).
    relinked: bool,
    /// [`Source::ends_near`].
    ends_near: Option<u64>,
}

/// yt-dlp's format `id` looked up again (blocking: yt-dlp runs): its address and headers today.
pub type Relink = Arc<dyn Fn(&str) -> Result<(Url, Vec<(String, String)>), String> + Send + Sync>;

struct Inner {
    token: String,
    addr: SocketAddr,
    client: Mutex<HashMap<String, reqwest::Client>>,
    opts: mdm_engine::Options,
    limiter: Arc<Limiter>,
    entries: Mutex<Vec<Entry>>,
    /// An address a playlist listed → its entry.
    listed: Mutex<HashMap<String, usize>>,
    sources: Vec<String>,
    relink: Option<Relink>,
    downloaded: AtomicU64,
    /// Bytes ffmpeg was given.
    given: AtomicU64,
    /// Bytes in the last moments, for the speed: (when, total then).
    marks: Mutex<Vec<(Instant, u64)>>,
    failure: Mutex<Option<String>>,
    cancel: CancellationToken,
}

/// The server of one run of a part; it stops when dropped.
pub struct PartServer {
    inner: Arc<Inner>,
}

impl Drop for PartServer {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
    }
}

/// A plain HTTP header map of `headers` (the ones that can't be one are left out).
fn header_map(headers: &[(String, String)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            map.insert(k, v);
        }
    }
    map
}

impl PartServer {
    /// A server for `sources`, on `handle`'s runtime, fetching with `opts` (its connections,
    /// proxy, adapter, DNS, retries, `global` limit) under `limiter` (the download's own).
    pub fn start(
        handle: &tokio::runtime::Handle,
        opts: mdm_engine::Options,
        limiter: Arc<Limiter>,
        sources: Vec<Source>,
        relink: Option<Relink>,
    ) -> io::Result<PartServer> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let mut bytes = [0u8; 16];
        aws_lc_rs::rand::fill(&mut bytes).map_err(|_| io::Error::other("no random numbers"))?;
        let token = hex::encode(bytes);
        let listener = {
            let _in = handle.enter();
            TcpListener::from_std(listener)?
        };
        let entries = sources
            .iter()
            .enumerate()
            .map(|(i, s)| Entry {
                url: s.url.clone(),
                headers: header_map(&s.headers),
                kind: if s.playlist {
                    Kind::Playlist
                } else {
                    Kind::File
                },
                size: s.size,
                source: i,
                relinked: false,
                ends_near: s.ends_near,
            })
            .collect();
        let inner = Arc::new(Inner {
            token,
            addr,
            client: Mutex::new(HashMap::new()),
            opts,
            limiter,
            entries: Mutex::new(entries),
            listed: Mutex::new(HashMap::new()),
            sources: sources.iter().map(|s| s.format_id.clone()).collect(),
            relink,
            downloaded: AtomicU64::new(0),
            given: AtomicU64::new(0),
            marks: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
            cancel: CancellationToken::new(),
        });
        handle.spawn(accept(listener, inner.clone()));
        Ok(PartServer { inner })
    }

    /// The address ffmpeg reads source `i` at.
    pub fn url(&self, i: usize) -> String {
        let entries = self.inner.entries.lock().unwrap();
        local_url(&self.inner, i, &entries[i].url)
    }

    /// Bytes brought in so far.
    pub fn downloaded(&self) -> u64 {
        self.inner.downloaded.load(Ordering::Relaxed)
    }

    /// Bytes ffmpeg was given so far: the part's progress (what came in ahead of it and it
    /// didn't need isn't).
    pub fn given(&self) -> u64 {
        self.inner.given.load(Ordering::Relaxed)
    }

    /// Bytes a second over the last two seconds.
    pub fn speed(&self) -> f64 {
        let now = Instant::now();
        let total = self.downloaded();
        let mut marks = self.inner.marks.lock().unwrap();
        marks.push((now, total));
        marks.retain(|(t, _)| now.duration_since(*t) <= Duration::from_secs(2));
        match marks.first() {
            Some((t, n)) if now.duration_since(*t) >= Duration::from_millis(400) => {
                (total - n) as f64 / now.duration_since(*t).as_secs_f64()
            }
            _ => 0.0,
        }
    }

    /// Why a stream couldn't be brought in, when one couldn't (ffmpeg only says it couldn't read).
    pub fn failure(&self) -> Option<String> {
        self.inner.failure.lock().unwrap().clone()
    }
}

/// `http://127.0.0.1:<port>/<token>/<i>/<name>`: the remote address's last path segment kept,
/// so ffmpeg sees the same file type it would.
fn local_url(inner: &Inner, i: usize, remote: &Url) -> String {
    let name = remote
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|n| !n.is_empty() && n.len() <= 120)
        .unwrap_or("media");
    format!("http://{}/{}/{i}/{name}", inner.addr, inner.token)
}

async fn accept(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        let accepted = tokio::select! {
            _ = inner.cancel.cancelled() => return,
            a = listener.accept() => a,
        };
        match accepted {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let cancel = inner.cancel.clone();
                    tokio::select! {
                        _ = cancel.cancelled() => {}
                        _ = serve(stream, &inner) => {}
                    }
                });
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// A request as ffmpeg sends it: its method, path and the range it asks for.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    head_only: bool,
    path: String,
    /// `bytes=<start>-[<end>]` (the end included).
    range: Option<(u64, Option<u64>)>,
}

fn parse_request(head: &str) -> Option<Asked> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?;
    let path = first.next()?.to_owned();
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return None,
    };
    let mut range = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("range") {
            let spec = value.trim().strip_prefix("bytes=")?;
            let (a, b) = spec.split_once('-')?;
            let start = a.trim().parse().ok()?;
            let end = match b.trim() {
                "" => None,
                b => Some(b.parse().ok()?),
            };
            if end.is_some_and(|e| e < start) {
                return None;
            }
            range = Some((start, end));
        }
    }
    Some(Asked {
        head_only,
        path,
        range,
    })
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1024];
    loop {
        let n = stream.read(&mut byte).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&byte[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            buf.truncate(end);
            return String::from_utf8(buf).ok();
        }
        if buf.len() > HEAD_MAX {
            return None;
        }
    }
}

async fn reply(stream: &mut TcpStream, status: &str) {
    let _ = stream
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await;
}

/// One request of ffmpeg's: a file's range or a playlist, from the entry its path names.
async fn serve(mut stream: TcpStream, inner: &Arc<Inner>) {
    let _ = stream.set_nodelay(true);
    // Little held for ffmpeg in the system's buffer: what it leaves unread is little.
    let _ = socket2::SockRef::from(&stream).set_send_buffer_size(SEND_BUFFER);
    let Ok(Some(head)) = tokio::time::timeout(HEAD_TIME, read_head(&mut stream)).await else {
        return;
    };
    let Some(asked) = parse_request(&head) else {
        return reply(&mut stream, "400 Bad Request").await;
    };
    let mut parts = asked.path.trim_start_matches('/').splitn(3, '/');
    let token_ok = parts.next() == Some(inner.token.as_str());
    let id = parts.next().and_then(|i| i.parse::<usize>().ok());
    let kind = id.and_then(|i| inner.entries.lock().unwrap().get(i).map(|e| e.kind));
    let (true, Some(id), Some(kind)) = (token_ok, id, kind) else {
        return reply(&mut stream, "404 Not Found").await;
    };
    match kind {
        Kind::Playlist => serve_playlist(&mut stream, inner, id, asked.head_only).await,
        Kind::File => serve_file(&mut stream, inner, id, &asked).await,
    }
}

/// The client for `url`'s host (made once per host, as the engine's are per server).
fn client(inner: &Inner, url: &Url) -> Result<reqwest::Client, String> {
    let host = url.host_str().unwrap_or("").to_owned();
    let mut clients = inner.client.lock().unwrap();
    if let Some(c) = clients.get(&host) {
        return Ok(c.clone());
    }
    let c = mdm_engine::http_client(&inner.opts, url).map_err(|e| e.to_string())?;
    clients.insert(host, c.clone());
    Ok(c)
}

/// How one request to the site ended.
enum Fetch {
    /// The bytes, the file's whole size (`Content-Range`'s, or the length of a whole answer), and
    /// its type.
    Got(Bytes, Option<u64>, Option<String>),
    /// The server sends the whole file for a range from its start (it takes no ranges): its
    /// answer, to hand on as it comes.
    Whole(Box<reqwest::Response>),
    /// The address died (401, 403, 404, 410): asked again it may live.
    Dead(u16),
    /// The range is past the end (416).
    Past,
    Failed(String),
}

/// Bytes `start..=end` of entry `id` (all of it from `start` without `end`), under the limits,
/// tried again as the engine's options say on a failure that may pass.
async fn fetch(inner: &Inner, id: usize, start: u64, end: Option<u64>) -> Fetch {
    let mut tries = 0;
    loop {
        let (url, headers) = {
            let e = inner.entries.lock().unwrap();
            (e[id].url.clone(), e[id].headers.clone())
        };
        let got = fetch_once(inner, &url, headers, start, end).await;
        match got {
            Fetch::Dead(status) => {
                if !relink(inner, id).await {
                    return Fetch::Dead(status);
                }
            }
            Fetch::Failed(_) if tries < inner.opts.max_retries.min(10) => {
                tries += 1;
                tokio::select! {
                    _ = inner.cancel.cancelled() => return Fetch::Failed("stopped".into()),
                    _ = tokio::time::sleep(inner.opts.retry_delay.min(Duration::from_secs(5))) => {}
                }
            }
            other => return other,
        }
    }
}

async fn fetch_once(
    inner: &Inner,
    url: &Url,
    mut headers: HeaderMap,
    start: u64,
    end: Option<u64>,
) -> Fetch {
    let client = match client(inner, url) {
        Ok(c) => c,
        Err(e) => return Fetch::Failed(e),
    };
    let range = match end {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    };
    headers.insert(RANGE, HeaderValue::from_str(&range).expect("ascii"));
    let resp = match client.get(url.clone()).headers(headers).send().await {
        Ok(r) => r,
        Err(e) => return Fetch::Failed(e.to_string()),
    };
    let status = resp.status().as_u16();
    match status {
        401 | 403 | 404 | 410 => return Fetch::Dead(status),
        416 => return Fetch::Past,
        200 | 206 => {}
        s => return Fetch::Failed(format!("the server answered {s}")),
    }
    // A server that sends the whole file for a range: only from its start is that the range, and
    // then it comes as one answer.
    if status == 200 && start > 0 {
        return Fetch::Failed("the server doesn't send parts of the file".into());
    }
    if status == 200 {
        return Fetch::Whole(Box::new(resp));
    }
    let total = if status == 206 {
        resp.headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit_once('/'))
            .and_then(|(_, t)| t.trim().parse::<u64>().ok())
    } else {
        resp.content_length()
    };
    let kind = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let want = end.map(|e| e - start + 1);
    let mut body = Vec::with_capacity(want.unwrap_or(FIRST_CHUNK).min(MAX_CHUNK) as usize);
    let mut resp = resp;
    loop {
        let chunk = tokio::select! {
            _ = inner.cancel.cancelled() => return Fetch::Failed("stopped".into()),
            c = resp.chunk() => c,
        };
        match chunk {
            Ok(Some(c)) => {
                let n = c.len() as u64;
                inner.limiter.acquire(n).await;
                if let Some(g) = &inner.opts.global {
                    g.acquire(n).await;
                }
                inner.downloaded.fetch_add(n, Ordering::Relaxed);
                body.extend_from_slice(&c);
                if want.is_some_and(|w| body.len() as u64 >= w) {
                    body.truncate(want.unwrap_or(0) as usize);
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => return Fetch::Failed(e.to_string()),
        }
    }
    if want.is_some_and(|w| (body.len() as u64) < w)
        && total.is_none_or(|t| start + (body.len() as u64) < t)
    {
        return Fetch::Failed("the server's answer came short".into());
    }
    Fetch::Got(Bytes::from(body), total, kind)
}

/// Entry `id`'s address asked of yt-dlp again, once: true when there is a new one to try.
async fn relink(inner: &Inner, id: usize) -> bool {
    let Some(relink) = inner.relink.clone() else {
        return false;
    };
    let (source, done) = {
        let e = inner.entries.lock().unwrap();
        (e[id].source, e[id].relinked)
    };
    // Only a source's own address: a playlist's pieces come anew with its playlist.
    if done || id != source {
        return false;
    }
    inner.entries.lock().unwrap()[id].relinked = true;
    let format = inner.sources[source].clone();
    let asked = tokio::task::spawn_blocking(move || relink(&format)).await;
    match asked {
        Ok(Ok((url, headers))) => {
            let mut e = inner.entries.lock().unwrap();
            e[id].url = url;
            e[id].headers = header_map(&headers);
            true
        }
        _ => false,
    }
}

fn fail(inner: &Inner, why: String) {
    if !inner.cancel.is_cancelled() {
        tracing::warn!("a part's server: {why}");
    }
    let mut f = inner.failure.lock().unwrap();
    if f.is_none() {
        *f = Some(why);
    }
}

fn english(f: &Fetch) -> String {
    match f {
        Fetch::Dead(s) => format!("the server refused the stream ({s})"),
        Fetch::Past => "the server has nothing past the stream's end".into(),
        Fetch::Failed(why) => why.clone(),
        Fetch::Got(..) | Fetch::Whole(_) => String::new(),
    }
}

/// A file's range: its first request alone (it tells the size), then the rest in growing
/// requests, several at once, written to ffmpeg in order as they come, until the range's end or
/// until ffmpeg closes (it seeks elsewhere, or has what the part needs).
async fn serve_file(stream: &mut TcpStream, inner: &Arc<Inner>, id: usize, asked: &Asked) {
    let (start, end) = asked.range.unwrap_or((0, None));
    let (known, ends_near) = {
        let e = inner.entries.lock().unwrap();
        (e[id].size, e[id].ends_near)
    };
    if known.is_some_and(|s| start >= s) {
        return reply(stream, "416 Range Not Satisfiable").await;
    }
    let first_end = start + FIRST_CHUNK - 1;
    let first_end = end.map_or(first_end, |e| e.min(first_end));
    let first_end = known.map_or(first_end, |s| first_end.min(s - 1));
    let first = fetch(inner, id, start, Some(first_end)).await;
    let (bytes, total, kind) = match first {
        Fetch::Got(b, t, k) => (b, t.or(known), k),
        Fetch::Whole(resp) => return pipe_whole(stream, inner, *resp, asked.head_only).await,
        Fetch::Past => return reply(stream, "416 Range Not Satisfiable").await,
        other => {
            fail(inner, english(&other));
            return reply(stream, "502 Bad Gateway").await;
        }
    };
    if let Some(t) = total {
        inner.entries.lock().unwrap()[id].size = Some(t);
    }
    // The range's last byte: as asked, else the file's last.
    let last = match (end, total) {
        (Some(e), Some(t)) => e.min(t.saturating_sub(1)),
        (Some(e), None) => e,
        (None, Some(t)) => t.saturating_sub(1),
        // Neither said: what the first request brought is all there is to tell.
        (None, None) => start + bytes.len() as u64 - 1,
    };
    let len = (last + 1).saturating_sub(start);
    let kind = kind.unwrap_or_else(|| "application/octet-stream".into());
    let head = match (asked.range.is_some(), total) {
        (true, Some(t)) => format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {kind}\r\nAccept-Ranges: bytes\r\n\
             Content-Range: bytes {start}-{last}/{t}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        ),
        _ => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nAccept-Ranges: bytes\r\n\
             Content-Length: {len}\r\nConnection: close\r\n\r\n"
        ),
    };
    if stream.write_all(head.as_bytes()).await.is_err() || asked.head_only {
        return;
    }
    if stream.write_all(&bytes).await.is_err() {
        return;
    }
    inner.given.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let mut next = start + bytes.len() as u64;
    if bytes.is_empty() || next > last {
        return;
    }
    let ahead = inner.opts.connections().clamp(1, AHEAD) as usize;
    let mut grow = (FIRST_CHUNK * 2).min(MAX_CHUNK);
    // ffmpeg often reads a little of a file and seeks on (a header, an index): the requests
    // ahead of it grow with what it took, one more for each one it took, so a read it leaves
    // early costs little.
    let mut taken = 0usize;
    // Each request its own task: they go on while ffmpeg reads another stream (a picture and its
    // sound are read side by side), and end with this read (`Ahead`'s drop), which ends as soon
    // as ffmpeg closes its connection.
    let mut pending = Ahead(std::collections::VecDeque::new());
    let (mut rd, mut wr) = stream.split();
    loop {
        // Past where the part ends, about: one small request at a time.
        let past = ends_near.is_some_and(|e| next > e);
        let most = if past { 1 } else { ahead.min(taken + 1) };
        while pending.0.len() < most && next <= last {
            let size = if past { FIRST_CHUNK } else { grow };
            let to = (next + size - 1).min(last);
            let inner = inner.clone();
            let from = next;
            pending.0.push_back(tokio::spawn(async move {
                let got = fetch(&inner, id, from, Some(to)).await;
                (from, to, got)
            }));
            next = to + 1;
            if !past {
                grow = (grow * 2).min(MAX_CHUNK);
            }
        }
        let Some(first) = pending.0.pop_front() else {
            return;
        };
        let got = tokio::select! {
            got = first => got,
            () = closed(&mut rd) => return,
        };
        let Ok((from, to, got)) = got else {
            return;
        };
        match got {
            Fetch::Got(b, _, _) if b.len() as u64 == to - from + 1 => {
                if wr.write_all(&b).await.is_err() {
                    // ffmpeg closed: it seeks elsewhere, or has what it needs.
                    return;
                }
                inner.given.fetch_add(b.len() as u64, Ordering::Relaxed);
                taken += 1;
            }
            Fetch::Got(..) | Fetch::Whole(_) => {
                fail(inner, "the server's answer came short".into());
                return;
            }
            other => {
                fail(inner, english(&other));
                return;
            }
        }
    }
}

/// A whole file from a server that takes no ranges (an HLS piece from a plain web server), handed
/// on to ffmpeg as it comes, under the limits; it can't be sought in, as from the site itself.
async fn pipe_whole(
    stream: &mut TcpStream,
    inner: &Arc<Inner>,
    mut resp: reqwest::Response,
    head_only: bool,
) {
    let kind = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let length = resp
        .content_length()
        .map(|n| format!("Content-Length: {n}\r\n"))
        .unwrap_or_default();
    let head =
        format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{length}Connection: close\r\n\r\n");
    if stream.write_all(head.as_bytes()).await.is_err() || head_only {
        return;
    }
    loop {
        let chunk = tokio::select! {
            _ = inner.cancel.cancelled() => return,
            c = resp.chunk() => c,
        };
        match chunk {
            Ok(Some(c)) => {
                let n = c.len() as u64;
                inner.limiter.acquire(n).await;
                if let Some(g) = &inner.opts.global {
                    g.acquire(n).await;
                }
                inner.downloaded.fetch_add(n, Ordering::Relaxed);
                if stream.write_all(&c).await.is_err() {
                    return;
                }
                inner.given.fetch_add(n, Ordering::Relaxed);
            }
            Ok(None) => return,
            Err(e) => {
                fail(inner, e.to_string());
                return;
            }
        }
    }
}

/// Until ffmpeg closes its side of a read (it sends nothing after its request; what it may send
/// is read and dropped).
async fn closed(rd: &mut tokio::net::tcp::ReadHalf<'_>) {
    let mut buf = [0u8; 512];
    loop {
        match rd.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// The requests of a read still under way: ended with it.
struct Ahead(std::collections::VecDeque<tokio::task::JoinHandle<(u64, u64, Fetch)>>);

impl Drop for Ahead {
    fn drop(&mut self) {
        for h in &self.0 {
            h.abort();
        }
    }
}

/// A playlist, its addresses made this server's: each piece, key and init piece a file of its,
/// each variant of a master playlist a playlist.
async fn serve_playlist(stream: &mut TcpStream, inner: &Arc<Inner>, id: usize, head_only: bool) {
    let (url, headers) = {
        let e = inner.entries.lock().unwrap();
        (e[id].url.clone(), e[id].headers.clone())
    };
    let text = match playlist_text(inner, &url, headers.clone()).await {
        Ok(t) => t,
        Err(why) => {
            // A dead signed address: asked of yt-dlp again, once.
            if why.starts_with("dead") && relink(inner, id).await {
                let (url, headers) = {
                    let e = inner.entries.lock().unwrap();
                    (e[id].url.clone(), e[id].headers.clone())
                };
                match playlist_text(inner, &url, headers).await {
                    Ok(t) => t,
                    Err(why) => {
                        fail(inner, why);
                        return reply(stream, "502 Bad Gateway").await;
                    }
                }
            } else {
                fail(inner, why);
                return reply(stream, "502 Bad Gateway").await;
            }
        }
    };
    let (url, headers) = {
        let e = inner.entries.lock().unwrap();
        (e[id].url.clone(), e[id].headers.clone())
    };
    let source = inner.entries.lock().unwrap()[id].source;
    let rewritten = rewrite(&text, &url, &mut |remote, playlist| {
        listed(inner, remote, playlist, &headers, source)
    });
    let body = rewritten.into_bytes();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).await.is_ok() && !head_only {
        let _ = stream.write_all(&body).await;
    }
}

/// The entry for an address a playlist listed (made the first time it is seen), its local
/// address; none for an address that isn't `http`/`https`, or past [`ENTRIES_MAX`].
fn listed(
    inner: &Inner,
    remote: &Url,
    playlist: bool,
    headers: &HeaderMap,
    source: usize,
) -> Option<String> {
    if !matches!(remote.scheme(), "http" | "https") {
        return None;
    }
    let mut listed = inner.listed.lock().unwrap();
    if let Some(&i) = listed.get(remote.as_str()) {
        return Some(local_url(inner, i, remote));
    }
    let mut entries = inner.entries.lock().unwrap();
    if entries.len() >= ENTRIES_MAX {
        return None;
    }
    let i = entries.len();
    entries.push(Entry {
        url: remote.clone(),
        // The playlist's headers, as ffmpeg sends them to every address of it.
        headers: headers.clone(),
        kind: if playlist { Kind::Playlist } else { Kind::File },
        size: None,
        source,
        relinked: false,
        ends_near: None,
    });
    listed.insert(remote.as_str().to_owned(), i);
    Some(local_url(inner, i, remote))
}

async fn playlist_text(inner: &Inner, url: &Url, mut headers: HeaderMap) -> Result<String, String> {
    let client = client(inner, url)?;
    headers.remove(RANGE);
    let mut tries = 0;
    loop {
        let sent = tokio::select! {
            _ = inner.cancel.cancelled() => return Err("stopped".into()),
            r = client.get(url.clone()).headers(headers.clone()).send() => r,
        };
        let failed = match sent {
            Ok(r) if r.status().is_success() => {
                let mut r = r;
                let mut body = Vec::new();
                loop {
                    match r.chunk().await {
                        Ok(Some(c)) => {
                            inner
                                .downloaded
                                .fetch_add(c.len() as u64, Ordering::Relaxed);
                            body.extend_from_slice(&c);
                            if body.len() > PLAYLIST_MAX {
                                return Err("the playlist is too long".into());
                            }
                        }
                        Ok(None) => return Ok(String::from_utf8_lossy(&body).into_owned()),
                        Err(e) => break e.to_string(),
                    }
                }
            }
            Ok(r) if matches!(r.status().as_u16(), 401 | 403 | 404 | 410) => {
                return Err(format!(
                    "dead: the server refused the playlist ({})",
                    r.status().as_u16()
                ));
            }
            Ok(r) => format!(
                "the server answered {} for the playlist",
                r.status().as_u16()
            ),
            Err(e) => e.to_string(),
        };
        if tries >= inner.opts.max_retries.min(10) {
            return Err(failed);
        }
        tries += 1;
        tokio::time::sleep(inner.opts.retry_delay.min(Duration::from_secs(5))).await;
    }
}

/// An HLS playlist (at `base`) with every address in it made another (`local`: the remote
/// address, and whether it is a playlist; none leaves the line out, a `URI` attribute as it was):
/// each URI line (a piece, or a variant after `#EXT-X-STREAM-INF`) and the `URI` attributes of
/// `#EXT-X-KEY`, `#EXT-X-SESSION-KEY`, `#EXT-X-MAP`, `#EXT-X-MEDIA` (a rendition's playlist) and
/// `#EXT-X-I-FRAME-STREAM-INF` (a playlist).
fn rewrite(text: &str, base: &Url, local: &mut dyn FnMut(&Url, bool) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut variant_next = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if let Some(tag) = t.strip_prefix('#') {
            let playlist_uri =
                tag.starts_with("EXT-X-MEDIA:") || tag.starts_with("EXT-X-I-FRAME-STREAM-INF:");
            let has_uri = playlist_uri
                || tag.starts_with("EXT-X-KEY:")
                || tag.starts_with("EXT-X-SESSION-KEY:")
                || tag.starts_with("EXT-X-MAP:");
            if tag.starts_with("EXT-X-STREAM-INF:") {
                variant_next = true;
            }
            if has_uri && let Some(new) = with_uri(t, base, playlist_uri, local) {
                out.push_str(&new);
            } else {
                out.push_str(t);
            }
            out.push('\n');
            continue;
        }
        let playlist = std::mem::take(&mut variant_next);
        if let Some(new) = base.join(t).ok().and_then(|u| local(&u, playlist)) {
            out.push_str(&new);
            out.push('\n');
        }
    }
    out
}

/// A tag's `URI="…"` attribute made `local`'s address (none when it has none, or `local` gives
/// none).
fn with_uri(
    tag: &str,
    base: &Url,
    playlist: bool,
    local: &mut dyn FnMut(&Url, bool) -> Option<String>,
) -> Option<String> {
    let at = tag.find("URI=\"")? + 5;
    let len = tag[at..].find('"')?;
    let uri = &tag[at..at + len];
    let new = local(&base.join(uri).ok()?, playlist)?;
    Some(format!("{}{}{}", &tag[..at], new, &tag[at + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpegs_requests_are_read() {
        let r =
            parse_request("GET /t/0/v.mp4 HTTP/1.1\r\nUser-Agent: Lavf\r\nRange: bytes=100-\r\n")
                .unwrap();
        assert_eq!(
            r,
            Asked {
                head_only: false,
                path: "/t/0/v.mp4".into(),
                range: Some((100, None))
            }
        );
        let r = parse_request("GET /t/1/a HTTP/1.1\r\nrange: bytes=0-99\r\n").unwrap();
        assert_eq!(r.range, Some((0, Some(99))));
        assert_eq!(parse_request("GET /x HTTP/1.1\r\n").unwrap().range, None);
        assert!(parse_request("POST /x HTTP/1.1\r\n").is_none());
        assert!(parse_request("GET /x HTTP/1.1\r\nRange: bytes=9-1\r\n").is_none());
        assert!(parse_request("GET /x HTTP/1.1\r\nRange: items=1-2\r\n").is_none());
    }

    #[test]
    fn a_playlists_addresses_become_the_servers() {
        let text = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-MAP:URI=\"init.mp4\"\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.example/k?id=2\",IV=0x01\n\
            #EXTINF:4.0,\nseg7.m4s?s=1\n#EXTINF:4.0,\n/abs/seg8.m4s\n\
            #EXTINF:4.0,\nftp://other.example/seg9.ts\n#EXT-X-ENDLIST\n";
        let base = Url::parse("https://cdn.example/v/720p/index.m3u8?sig=1").unwrap();
        let mut seen = Vec::new();
        let out = rewrite(text, &base, &mut |u, p| {
            seen.push((u.to_string(), p));
            (u.scheme() != "ftp").then(|| format!("L{}", seen.len()))
        });
        assert_eq!(
            seen.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(),
            [
                "https://cdn.example/v/720p/init.mp4",
                "https://keys.example/k?id=2",
                "https://cdn.example/v/720p/seg7.m4s?s=1",
                "https://cdn.example/abs/seg8.m4s",
                "ftp://other.example/seg9.ts",
            ]
        );
        assert_eq!(
            out,
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-MAP:URI=\"L1\"\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"L2\",IV=0x01\n#EXTINF:4.0,\nL3\n#EXTINF:4.0,\nL4\n\
             #EXTINF:4.0,\n#EXT-X-ENDLIST\n"
        );
    }

    #[test]
    fn a_master_playlists_variants_are_playlists() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"audio/en.m3u8\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000\nv/720.m3u8\nseg.ts\n";
        let base = Url::parse("https://cdn.example/m.m3u8").unwrap();
        let mut seen = Vec::new();
        rewrite(text, &base, &mut |u, p| {
            seen.push((u.path().to_owned(), p));
            Some(String::new())
        });
        assert_eq!(
            seen,
            [
                ("/audio/en.m3u8".to_owned(), true),
                ("/v/720.m3u8".to_owned(), true),
                ("/seg.ts".to_owned(), false),
            ]
        );
    }
}
