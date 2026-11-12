//! A stream in pieces downloaded by MDM's engine (#769): HLS (yt-dlp's `m3u8_native`) and DASH
//! (`http_dash_segments`), brought in as yt-dlp's own downloaders for them (hlsnative,
//! dashsegments) do, by MDM: the pieces listed (an HLS media playlist read as hlsnative reads it,
//! DASH's from yt-dlp's `fragments`), downloaded several at once (as many as Options →
//! Connection gives a file) over the engine's connections (proxy, network adapter, DNS), under
//! the speed limits, each kept on disk once whole so that a stop goes on from the pieces there,
//! AES-128 pieces decrypted with the playlist's key, and joined in order into the file yt-dlp
//! looks for: what hlsnative would have written. yt-dlp then fixes it up and merges it with ffmpeg
//! as after its own download.
//!
//! A piece refused (401, 403, 404, 410: its signed address expired) has the list asked of yt-dlp
//! again (`relink`), and the pieces go on from the new one.
//!
//! The same connections bring a gallery's files in (gallery-dl's list, #769): each piece is then a
//! file of its own ([`Target::Files`]), saved where gallery-dl would save it, and one that can't
//! be had is left for gallery-dl rather than failing the others.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_engine::download::{ConnStatus, ConnView, SegmentView};
use mdm_engine::limiter::Limiter;
use reqwest::header::{HeaderMap, HeaderValue, RANGE};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

/// One piece: its address, a part of it (`start`, length) when the playlist names one, and the
/// key it is encrypted with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub url: Url,
    pub range: Option<(u64, u64)>,
    pub key: Option<Key>,
}

/// An AES-128 key's address and the piece's IV.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub uri: Url,
    pub iv: [u8; 16],
}

/// Why a playlist's pieces can't be MDM's to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unlisted {
    /// No end yet: a live stream still being recorded.
    Live,
    /// Encrypted otherwise than with AES-128 (SAMPLE-AES, a DRM's): its method.
    Protected(String),
    /// Not a media playlist with pieces (a master playlist, an empty one).
    NoPieces,
}

/// An attribute list (`METHOD=AES-128,URI="k.bin",IV=0x01`): names and values, quotes taken off.
fn attributes(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some((name, after)) = rest.split_once('=') else {
            break;
        };
        let (value, next) = if let Some(q) = after.strip_prefix('"') {
            match q.split_once('"') {
                Some((v, n)) => (v, n.trim_start_matches(',').trim_start()),
                None => (q, ""),
            }
        } else {
            match after.split_once(',') {
                Some((v, n)) => (v, n.trim_start()),
                None => (after, ""),
            }
        };
        out.insert(name.trim().to_ascii_uppercase(), value.trim().to_owned());
        rest = next;
    }
    out
}

/// An IV as the playlist writes it (`0x` and up to 32 hex digits), right-aligned in 16 bytes.
fn iv_of(s: &str) -> Option<[u8; 16]> {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    if hex.is_empty() || hex.len() > 32 {
        return None;
    }
    let padded = format!("{hex:0>32}");
    let mut iv = [0u8; 16];
    for (i, b) in iv.iter_mut().enumerate() {
        *b = u8::from_str_radix(&padded[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(iv)
}

/// The IV of the piece numbered `sequence` when the playlist gives none: 8 zero bytes, then the
/// number, big-endian (hlsnative's `struct.pack('>8xq', media_sequence)`).
fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

/// `url` with yt-dlp's `extra_param_to_segment_url` (or `_key_url`) query added.
fn with_query(mut url: Url, extra: Option<&str>) -> Url {
    if let Some(extra) = extra.filter(|e| !e.is_empty()) {
        let query = match url.query() {
            Some(q) if !q.is_empty() => format!("{q}&{extra}"),
            _ => extra.to_owned(),
        };
        url.set_query(Some(&query));
    }
    url
}

/// An HLS media playlist's pieces, read as yt-dlp's hlsnative reads them: `#EXT-X-MAP`'s init
/// section first where it stands, each piece's key (`#EXT-X-KEY`, AES-128 or none) with its IV
/// (the playlist's, else the piece's sequence number), byte ranges (`#EXT-X-BYTERANGE`, an
/// offset left out following on from the range before), addresses taken from `base`, with
/// yt-dlp's extra queries for pieces and keys.
pub fn hls(
    text: &str,
    base: &Url,
    extra_query: Option<&str>,
    extra_key_query: Option<&str>,
) -> Result<Vec<Piece>, Unlisted> {
    let mut pieces = Vec::new();
    let mut sequence = 0u64;
    let mut key: Option<(Url, Option<[u8; 16]>)> = None;
    let mut range: Option<(u64, u64)> = None;
    let mut range_end = 0u64;
    let mut ended = false;
    let join = |uri: &str| base.join(uri).ok();
    let key_for = |key: &Option<(Url, Option<[u8; 16]>)>, sequence: u64| {
        key.as_ref().map(|(uri, iv)| Key {
            uri: uri.clone(),
            iv: iv.unwrap_or_else(|| sequence_iv(sequence)),
        })
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !line.starts_with('#') {
            let url = join(line).ok_or(Unlisted::NoPieces)?;
            pieces.push(Piece {
                url: with_query(url, extra_query),
                range: range.take(),
                key: key_for(&key, sequence),
            });
            sequence += 1;
        } else if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            sequence = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("#EXT-X-KEY:") {
            let a = attributes(v);
            match a.get("METHOD").map(String::as_str) {
                Some("NONE") => key = None,
                Some("AES-128") => {
                    let uri = a
                        .get("URI")
                        .and_then(|u| join(u))
                        .ok_or(Unlisted::NoPieces)?;
                    let iv = a.get("IV").and_then(|iv| iv_of(iv));
                    key = Some((with_query(uri, extra_key_query), iv));
                }
                other => return Err(Unlisted::Protected(other.unwrap_or("?").to_owned())),
            }
        } else if let Some(v) = line.strip_prefix("#EXT-X-MAP:") {
            let a = attributes(v);
            let url = a
                .get("URI")
                .and_then(|u| join(u))
                .ok_or(Unlisted::NoPieces)?;
            let part = a.get("BYTERANGE").and_then(|r| byte_range(r, 0));
            pieces.push(Piece {
                url: with_query(url, extra_query),
                range: part,
                key: key_for(&key, sequence),
            });
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            if let Some(r) = byte_range(v, range_end) {
                range_end = r.0 + r.1;
                range = Some(r);
            }
        } else if line.starts_with("#EXT-X-ENDLIST") {
            ended = true;
        } else if line.starts_with("#EXT-X-STREAM-INF") {
            return Err(Unlisted::NoPieces);
        }
    }
    if !ended {
        return Err(Unlisted::Live);
    }
    if pieces.is_empty() {
        return Err(Unlisted::NoPieces);
    }
    Ok(pieces)
}

/// `<length>[@<offset>]`: (offset, length), the offset `after` when left out.
fn byte_range(s: &str, after: u64) -> Option<(u64, u64)> {
    let (len, at) = match s.trim().split_once('@') {
        Some((l, o)) => (l.trim().parse().ok()?, o.trim().parse().ok()?),
        None => (s.trim().parse().ok()?, after),
    };
    Some((at, len))
}

/// A DASH format's pieces from yt-dlp (`fragments`, each its `url`, or a `path` from
/// `fragment_base_url`, with an optional `byte_range` {start, end} or `range` "a-b").
pub fn dash(format: &Value) -> Option<Vec<Piece>> {
    let base = format
        .get("fragment_base_url")
        .and_then(Value::as_str)
        .and_then(|b| Url::parse(b).ok());
    let fragments = format.get("fragments")?.as_array()?;
    let pieces: Option<Vec<Piece>> = fragments
        .iter()
        .map(|f| {
            let url = match f.get("url").and_then(Value::as_str) {
                Some(u) => Url::parse(u).ok()?,
                None => base.as_ref()?.join(f.get("path")?.as_str()?).ok()?,
            };
            let range = match (f.get("byte_range"), f.get("range").and_then(Value::as_str)) {
                (Some(r), _) => {
                    let start = r.get("start")?.as_u64()?;
                    let end = r.get("end")?.as_u64()?;
                    Some((start, end.checked_sub(start)?))
                }
                (None, Some(r)) => {
                    let (a, b) = r.split_once('-')?;
                    let (a, b): (u64, u64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                    Some((a, b.checked_sub(a)? + 1))
                }
                (None, None) => None,
            };
            Some(Piece {
                url,
                range,
                key: None,
            })
        })
        .collect();
    pieces.filter(|p| !p.is_empty())
}

/// AES-128-CBC with `key` and `iv` over `data`, then its PKCS#7 padding taken off as yt-dlp's
/// `unpad_pkcs7` does (by the last byte). None: not whole blocks, or no such key.
pub fn decrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Option<Vec<u8>> {
    use aws_lc_rs::cipher::{AES_128, DecryptingKey, DecryptionContext, UnboundCipherKey};
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return None;
    }
    let k = DecryptingKey::cbc(UnboundCipherKey::new(&AES_128, key).ok()?).ok()?;
    let mut buf = data.to_vec();
    let n = k
        .decrypt(&mut buf, DecryptionContext::Iv128((*iv).into()))
        .ok()?
        .len();
    buf.truncate(n);
    let pad = *buf.last()? as usize;
    if (1..=16).contains(&pad) && pad <= buf.len() {
        buf.truncate(buf.len() - pad);
    }
    Some(buf)
}

/// The most files of a gallery downloaded at once (#769): Wikimedia's servers answer 429 Too
/// Many Requests to more (measured: 8 at once, 2 of them refused, again and again).
pub const GALLERY_AT_ONCE: u32 = 4;

/// Where the pieces go: joined into one file (a stream's), or each a file of its own (a gallery's).
#[derive(Clone, Debug)]
pub enum Target {
    Join(PathBuf),
    Files(Vec<PathBuf>),
}

/// Where a stream's pieces wait until they are joined: `<file>.mdm.pieces`.
pub fn folder(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".mdm.pieces");
    PathBuf::from(s)
}

/// A piece's file in the folder: `<index>` once whole.
fn piece_file(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("{i:06}"))
}

impl Run {
    /// "the piece", or "the file" for a gallery's.
    fn one(&self) -> &'static str {
        if self.files.is_some() {
            "the file"
        } else {
            "the piece"
        }
    }

    /// Where piece `i` is saved once whole.
    fn file(&self, i: usize) -> PathBuf {
        match &self.files {
            Some(f) => f[i].clone(),
            None => piece_file(&self.dir, i),
        }
    }
}

/// What the pieces show while they come: bytes in, the pieces whole, each connection.
#[derive(Default)]
pub struct Shared {
    downloaded: AtomicU64,
    /// Bytes of the pieces whole, and how many.
    whole_bytes: AtomicU64,
    whole: AtomicUsize,
    /// Each piece's progress: (bytes in, its length when the server said it).
    each: Mutex<Vec<(u64, Option<u64>)>>,
    conns: Mutex<Vec<ConnView>>,
    notes: Mutex<Vec<String>>,
}

impl Shared {
    fn note(&self, s: String) {
        self.notes.lock().unwrap().push(s);
    }

    /// The log lines since the last call.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.lock().unwrap())
    }

    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }

    /// The stream's size as far as it can be told: the pieces whole tell their average; before
    /// any, `estimate` (yt-dlp's). Never under what came.
    pub fn total(&self, pieces: usize, estimate: Option<u64>) -> Option<u64> {
        let whole = self.whole.load(Ordering::Relaxed);
        let bytes = self.whole_bytes.load(Ordering::Relaxed);
        let t = if whole == pieces && whole > 0 {
            Some(bytes)
        } else if whole >= 3 || (whole > 0 && estimate.is_none()) {
            Some(bytes / whole as u64 * pieces as u64)
        } else {
            estimate
        };
        t.map(|t| t.max(self.downloaded()))
    }

    /// The pieces as segments of a stream `total` long, each an equal share of it.
    pub fn segments(&self, total: u64) -> Vec<SegmentView> {
        let each = self.each.lock().unwrap();
        let n = each.len().max(1) as u64;
        each.iter()
            .enumerate()
            .map(|(i, (got, len))| {
                let start = total * i as u64 / n;
                let end = total * (i as u64 + 1) / n;
                let frac = match len {
                    Some(l) if *l > 0 => (*got as f64 / *l as f64).min(1.0),
                    _ if *got > 0 => 0.5,
                    _ => 0.0,
                };
                SegmentView {
                    start,
                    cursor: start + ((end - start) as f64 * frac) as u64,
                    end,
                }
            })
            .collect()
    }

    pub fn connections(&self) -> Vec<ConnView> {
        self.conns.lock().unwrap().clone()
    }
}

/// How the pieces of a stream ended.
#[derive(Debug)]
pub enum Ended {
    /// Joined into the stream's file.
    Done(u64),
    Stopped,
    Failed(String),
}

/// Asks yt-dlp again for the stream's pieces (a refused piece: their addresses expired).
pub type Relist = Arc<dyn Fn() -> Result<Vec<Piece>, String> + Send + Sync>;

/// A refused piece: its address is dead (401, 403, 404, 410), as against one to try again.
enum PieceError {
    Dead(String),
    Again(String),
    /// 429 or 503: the server wants fewer requests (for this long, when it says).
    Busy(String, Option<Duration>),
    Stopped,
}

/// What every connection shares.
struct Run {
    client: reqwest::Client,
    headers: HeaderMap,
    pieces: Mutex<Vec<Piece>>,
    /// Bumped when the list is asked again: a piece of an older list asks again with the new.
    generation: AtomicU64,
    next: AtomicUsize,
    /// Pieces a connection gave back (it closed for the server's sake): taken before `next`.
    given_back: Mutex<Vec<usize>>,
    /// Connections still going.
    going: AtomicUsize,
    keys: tokio::sync::Mutex<HashMap<Url, [u8; 16]>>,
    dir: PathBuf,
    /// Each piece a file of its own (`Target::Files`): one that fails is left out, not fatal.
    files: Option<Vec<PathBuf>>,
    limiter: Arc<Limiter>,
    global: Option<Arc<Limiter>>,
    cancel: CancellationToken,
    shared: Arc<Shared>,
    max_retries: u32,
    retry_delay: Duration,
    relist: Relist,
    relisted: tokio::sync::Mutex<(u32, Option<Instant>)>,
}

/// Download `pieces` (a piece already whole is kept), `connections` at once, with `headers`,
/// under `limiter` (the download's own limit) and the engine's `opts` (its client, the limit for
/// all downloads, retries); then join them in order into the `target` file, or leave each in its
/// own (a gallery's: a file already there isn't counted as downloaded now).
#[allow(clippy::too_many_arguments)]
pub async fn download(
    pieces: Vec<Piece>,
    target: Target,
    headers: HeaderMap,
    opts: mdm_engine::Options,
    limiter: Arc<Limiter>,
    cancel: CancellationToken,
    shared: Arc<Shared>,
    relist: Relist,
) -> Ended {
    let (path, files) = match target {
        Target::Join(p) => (Some(p), None),
        Target::Files(f) => (None, Some(f)),
    };
    let dir = match (&path, &files) {
        (Some(p), _) => folder(p),
        (None, Some(f)) => f
            .first()
            .and_then(|f| f.parent())
            .map(Path::to_owned)
            .unwrap_or_default(),
        (None, None) => unreachable!(),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Ended::Failed(format!("Cannot make the pieces' folder: {e}"));
    }
    if files.as_ref().is_some_and(|f| f.len() != pieces.len()) {
        return Ended::Failed("a file for each piece".into());
    }
    let Some(first) = pieces.first() else {
        return Ended::Failed("no pieces".into());
    };
    let client = match mdm_engine::http_client(&opts, &first.url) {
        Ok(c) => c,
        Err(e) => return Ended::Failed(e.to_string()),
    };
    let n = pieces.len();
    // The pieces already whole (an earlier run's).
    {
        let mut each = shared.each.lock().unwrap();
        *each = vec![(0, None); n];
        for (i, slot) in each.iter_mut().enumerate() {
            // A gallery's file already there is gallery-dl's to tell ("already there").
            if files.is_some() {
                break;
            }
            if let Ok(m) = std::fs::metadata(piece_file(&dir, i)) {
                *slot = (m.len(), Some(m.len()));
                shared.downloaded.fetch_add(m.len(), Ordering::Relaxed);
                shared.whole_bytes.fetch_add(m.len(), Ordering::Relaxed);
                shared.whole.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let had = shared.whole.load(Ordering::Relaxed);
    if had > 0 {
        shared.note(format!("Resuming: {had} of {n} pieces on disk"));
    }
    // A gallery's files: at most `GALLERY_AT_ONCE` at once, gentle with the sites (Wikimedia's
    // answer 429 to more).
    let most = if files.is_some() {
        GALLERY_AT_ONCE
    } else {
        u32::MAX
    };
    let connections = opts.connections().min(most).min(n as u32).max(1) as usize;
    let noun = if files.is_some() { "files" } else { "pieces" };
    shared.note(format!(
        "{n} {noun}, {connections} at once, from {}",
        first.url.host_str().unwrap_or("?")
    ));
    *shared.conns.lock().unwrap() = (0..connections)
        .map(|i| ConnView {
            id: i as u32,
            downloaded: 0,
            status: ConnStatus::Connecting,
            speed: 0,
        })
        .collect();
    let run = Arc::new(Run {
        client,
        headers,
        pieces: Mutex::new(pieces),
        generation: AtomicU64::new(0),
        next: AtomicUsize::new(0),
        given_back: Default::default(),
        going: AtomicUsize::new(connections),
        keys: Default::default(),
        dir: dir.clone(),
        files,
        limiter,
        global: opts.global.clone(),
        cancel,
        shared: shared.clone(),
        max_retries: opts.max_retries,
        retry_delay: opts.retry_delay,
        relist,
        relisted: Default::default(),
    });
    let mut set = tokio::task::JoinSet::new();
    for c in 0..connections {
        set.spawn(connection(run.clone(), c));
    }
    let mut failure = None;
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if failure.is_none() {
                    failure = Some(e);
                }
                run.cancel.cancel();
            }
            Err(_) => failure = Some(PieceError::Again("a connection stopped".into())),
        }
    }
    match failure {
        Some(PieceError::Stopped) => return Ended::Stopped,
        Some(PieceError::Dead(e) | PieceError::Again(e) | PieceError::Busy(e, _)) => {
            return Ended::Failed(e);
        }
        None if run.cancel.is_cancelled() => return Ended::Stopped,
        None => {}
    }
    let Some(path) = path else {
        return Ended::Done(shared.downloaded());
    };
    let joined = tokio::task::spawn_blocking(move || join(&dir, n, &path)).await;
    match joined {
        Ok(Ok(size)) => Ended::Done(size),
        Ok(Err(e)) => Ended::Failed(format!("Cannot join the pieces: {e}")),
        Err(_) => Ended::Failed("Cannot join the pieces".into()),
    }
}

/// The pieces in `dir` (`n` of them) one after another into `path` (through its part file), then
/// the folder removed. The file's size.
fn join(dir: &Path, n: usize, path: &Path) -> std::io::Result<u64> {
    use std::io::Write;
    let part = mdm_engine::naming::part_path(path);
    let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
    let mut size = 0;
    for i in 0..n {
        let mut f = std::fs::File::open(piece_file(dir, i))?;
        size += std::io::copy(&mut f, &mut out)?;
    }
    out.flush()?;
    out.into_inner()
        .map_err(std::io::IntoInnerError::into_error)?
        .sync_all()?;
    std::fs::rename(&part, path)?;
    let _ = std::fs::remove_dir_all(dir);
    Ok(size)
}

/// One connection: the next piece not taken, until none is left.
async fn connection(run: Arc<Run>, c: usize) -> Result<(), PieceError> {
    loop {
        let given_back = run.given_back.lock().unwrap().pop();
        let i = given_back.unwrap_or_else(|| run.next.fetch_add(1, Ordering::SeqCst));
        let n = run.pieces.lock().unwrap().len();
        if i >= n {
            set_conn(&run, c, |v| {
                v.status = ConnStatus::Finished;
                v.speed = 0;
            });
            run.going.fetch_sub(1, Ordering::SeqCst);
            return Ok(());
        }
        if run.file(i).exists() {
            continue;
        }
        let mut failures = 0u32;
        loop {
            if run.cancel.is_cancelled() {
                return Err(PieceError::Stopped);
            }
            let generation = run.generation.load(Ordering::SeqCst);
            let piece = run.pieces.lock().unwrap()[i].clone();
            set_conn(&run, c, |v| v.status = ConnStatus::Connecting);
            match fetch_piece(&run, c, i, &piece).await {
                Ok(()) => break,
                Err(PieceError::Stopped) => return Err(PieceError::Stopped),
                // A gallery's file that can't be had is left for gallery-dl.
                Err(PieceError::Dead(e)) if run.files.is_some() => {
                    run.shared
                        .note(format!("File {}: {e}; left for gallery-dl", i + 1));
                    break;
                }
                // The server wants fewer requests (Wikimedia's 429): while others go on, this
                // connection closes and gives its piece back; the last one waits as it asks.
                Err(PieceError::Busy(e, _))
                    if run
                        .going
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |g| {
                            (g > 1).then(|| g - 1)
                        })
                        .is_ok() =>
                {
                    run.given_back.lock().unwrap().push(i);
                    run.shared.note(format!(
                        "Connection {}: {e}; closed, the others go on",
                        c + 1
                    ));
                    set_conn(&run, c, |v| {
                        v.status = ConnStatus::Finished;
                        v.speed = 0;
                    });
                    return Ok(());
                }
                Err(PieceError::Busy(e, wait)) => {
                    failures += 1;
                    if failures > run.max_retries {
                        if run.files.is_some() {
                            run.shared
                                .note(format!("File {}: {e}; left for gallery-dl", i + 1));
                            break;
                        }
                        return Err(PieceError::Again(format!("piece {}: {e}", i + 1)));
                    }
                    let backoff = run.retry_delay * 2u32.saturating_pow(failures - 1);
                    let wait = wait
                        .unwrap_or(backoff)
                        .max(backoff)
                        .min(Duration::from_secs(60));
                    run.shared.note(format!(
                        "Connection {}: {e}; again in {} s",
                        c + 1,
                        wait.as_secs().max(1)
                    ));
                    tokio::select! {
                        _ = run.cancel.cancelled() => return Err(PieceError::Stopped),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(PieceError::Again(e)) if run.files.is_some() && failures >= run.max_retries => {
                    run.shared
                        .note(format!("File {}: {e}; left for gallery-dl", i + 1));
                    break;
                }
                Err(PieceError::Dead(e)) => {
                    run.shared.note(format!("Piece {}: {e}", i + 1));
                    if !relist(&run, generation).await {
                        return Err(PieceError::Dead(e));
                    }
                }
                Err(PieceError::Again(e)) => {
                    failures += 1;
                    if failures > run.max_retries {
                        return Err(PieceError::Again(format!("piece {}: {e}", i + 1)));
                    }
                    run.shared
                        .note(format!("Connection {}: {e}; retrying", c + 1));
                    let wait = run.retry_delay * 2u32.saturating_pow(failures - 1);
                    tokio::select! {
                        _ = run.cancel.cancelled() => return Err(PieceError::Stopped),
                        _ = tokio::time::sleep(wait.min(Duration::from_secs(30))) => {}
                    }
                }
            }
        }
    }
}

/// The list asked of yt-dlp again after a refused piece of list `generation` (once for all
/// connections; at most 5 times, and not within 10 s of one that didn't help). True when there
/// is a newer list to try.
async fn relist(run: &Run, generation: u64) -> bool {
    let mut asked = run.relisted.lock().await;
    if run.generation.load(Ordering::SeqCst) != generation {
        return true;
    }
    if asked.0 >= 5
        || asked
            .1
            .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
    {
        return false;
    }
    asked.0 += 1;
    run.shared
        .note("Asking yt-dlp for the pieces' new addresses".into());
    let relist = run.relist.clone();
    let got = tokio::task::spawn_blocking(move || relist()).await;
    let n = run.pieces.lock().unwrap().len();
    match got {
        Ok(Ok(new)) if new.len() == n => {
            *run.pieces.lock().unwrap() = new;
            run.generation.fetch_add(1, Ordering::SeqCst);
            asked.1 = None;
            true
        }
        Ok(Ok(new)) => {
            asked.1 = Some(Instant::now());
            run.shared.note(format!(
                "The new list has {} pieces instead of {n}",
                new.len()
            ));
            false
        }
        Ok(Err(e)) => {
            asked.1 = Some(Instant::now());
            run.shared.note(e);
            false
        }
        Err(_) => {
            asked.1 = Some(Instant::now());
            false
        }
    }
}

fn set_conn(run: &Run, c: usize, f: impl FnOnce(&mut ConnView)) {
    if let Some(v) = run.shared.conns.lock().unwrap().get_mut(c) {
        f(v);
    }
}

/// The key at `uri` (16 bytes), fetched once.
async fn key_of(run: &Run, uri: &Url) -> Result<[u8; 16], PieceError> {
    let mut keys = run.keys.lock().await;
    if let Some(k) = keys.get(uri) {
        return Ok(*k);
    }
    let resp = run
        .client
        .get(uri.clone())
        .headers(run.headers.clone())
        .send()
        .await
        .map_err(|e| PieceError::Again(format!("the key: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(dead_or_again(
            status.as_u16(),
            "the key",
            retry_after(&resp),
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| PieceError::Again(format!("the key: {e}")))?;
    let key: [u8; 16] = body
        .as_ref()
        .try_into()
        .map_err(|_| PieceError::Dead(format!("the key is {} bytes, not 16", body.len())))?;
    keys.insert(uri.clone(), key);
    Ok(key)
}

fn dead_or_again(status: u16, what: &str, retry_after: Option<Duration>) -> PieceError {
    let e = format!("{what}: the server answered {status}");
    match status {
        401 | 403 | 404 | 410 => PieceError::Dead(e),
        429 | 503 => PieceError::Busy(e, retry_after),
        _ => PieceError::Again(e),
    }
}

/// `Retry-After` in seconds (a date isn't read: the backoff stands then).
fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Piece `i` on connection `c`: downloaded under the limits, decrypted, saved whole.
async fn fetch_piece(run: &Run, c: usize, i: usize, piece: &Piece) -> Result<(), PieceError> {
    let mut req = run
        .client
        .get(piece.url.clone())
        .headers(run.headers.clone());
    if let Some((start, len)) = piece.range {
        let r = format!("bytes={start}-{}", start + len.max(1) - 1);
        req = req.header(RANGE, HeaderValue::from_str(&r).expect("ascii"));
    }
    let sent = tokio::select! {
        _ = run.cancel.cancelled() => return Err(PieceError::Stopped),
        r = req.send() => r,
    };
    let mut resp = sent.map_err(|e| PieceError::Again(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(dead_or_again(status, run.one(), retry_after(&resp)));
    }
    let len = resp.content_length();
    let mut body = Vec::with_capacity(len.unwrap_or(0) as usize);
    run.shared.each.lock().unwrap()[i] = (0, len);
    set_conn(run, c, |v| v.status = ConnStatus::Receiving);
    let started = Instant::now();
    let mut got = 0u64;
    let undo = |got: u64| {
        run.shared.downloaded.fetch_sub(got, Ordering::Relaxed);
        run.shared.each.lock().unwrap()[i] = (0, None);
    };
    loop {
        let chunk = tokio::select! {
            _ = run.cancel.cancelled() => {
                undo(got);
                return Err(PieceError::Stopped);
            }
            c = resp.chunk() => c,
        };
        let chunk = match chunk {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                undo(got);
                return Err(PieceError::Again(e.to_string()));
            }
        };
        let n = chunk.len() as u64;
        let paid = async {
            run.limiter.acquire(n).await;
            if let Some(g) = &run.global {
                g.acquire(n).await;
            }
        };
        tokio::select! {
            _ = run.cancel.cancelled() => {
                undo(got);
                return Err(PieceError::Stopped);
            }
            _ = paid => {}
        }
        body.extend_from_slice(&chunk);
        got += n;
        run.shared.downloaded.fetch_add(n, Ordering::Relaxed);
        run.shared.each.lock().unwrap()[i].0 = got;
        let secs = started.elapsed().as_secs_f64().max(0.001);
        set_conn(run, c, |v| {
            v.downloaded += n;
            v.speed = (got as f64 / secs) as u64;
        });
    }
    if len.is_some_and(|l| l != got) {
        undo(got);
        return Err(PieceError::Again(format!("{} came short", run.one())));
    }
    set_conn(run, c, |v| v.speed = 0);
    let data = match &piece.key {
        Some(k) => {
            let key = key_of(run, &k.uri).await?;
            decrypt(&body, &key, &k.iv)
                .ok_or_else(|| PieceError::Dead("the piece can't be decrypted".into()))?
        }
        None => body,
    };
    let file = run.file(i);
    let part = {
        let mut s = file.as_os_str().to_owned();
        s.push(".mdm.part");
        PathBuf::from(s)
    };
    let size = data.len() as u64;
    let written = tokio::task::spawn_blocking(move || {
        std::fs::write(&part, &data).and_then(|_| std::fs::rename(&part, &file))
    })
    .await;
    if !matches!(written, Ok(Ok(()))) {
        undo(got);
        return Err(PieceError::Dead(format!("{} can't be saved", run.one())));
    }
    run.shared.whole_bytes.fetch_add(size, Ordering::Relaxed);
    run.shared.whole.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cdn.example/v/720p/index.m3u8?sig=1").unwrap()
    }

    #[test]
    fn a_playlist_is_read_as_hlsnative_reads_it() {
        let text = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-MEDIA-SEQUENCE:7\n\
            #EXT-X-MAP:URI=\"init.mp4\"\n\
            #EXTINF:4.0,\nseg7.m4s\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.example/k?id=2\",IV=0x0000000000000000000000000000000A\n\
            #EXTINF:4.0,\nseg8.m4s\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n\
            #EXTINF:4.0,\n/abs/seg9.m4s\n\
            #EXT-X-KEY:METHOD=NONE\n\
            #EXTINF:4.0,\nhttps://other.example/seg10.m4s\n\
            #EXT-X-ENDLIST\n";
        let p = hls(text, &base(), Some("tok=a"), Some("kt=b")).unwrap();
        let urls: Vec<&str> = p.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://cdn.example/v/720p/init.mp4?tok=a",
                "https://cdn.example/v/720p/seg7.m4s?tok=a",
                "https://cdn.example/v/720p/seg8.m4s?tok=a",
                "https://cdn.example/abs/seg9.m4s?tok=a",
                "https://other.example/seg10.m4s?tok=a",
            ]
        );
        assert_eq!(p[0].key, None);
        assert_eq!(p[1].key, None);
        let k = p[2].key.as_ref().unwrap();
        assert_eq!(k.uri.as_str(), "https://keys.example/k?id=2&kt=b");
        assert_eq!(k.iv[15], 10);
        // No IV given: the piece's sequence number (the third piece is number 9).
        let k = p[3].key.as_ref().unwrap();
        assert_eq!(k.uri.as_str(), "https://cdn.example/v/720p/key.bin?kt=b");
        assert_eq!(k.iv, sequence_iv(9));
        assert_eq!(&k.iv[..8], &[0; 8]);
        assert_eq!(p[4].key, None);
    }

    #[test]
    fn byte_ranges_follow_on() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"all.mp4\",BYTERANGE=\"720@0\"\n\
            #EXT-X-BYTERANGE:1000@720\n#EXTINF:4,\nall.mp4\n\
            #EXT-X-BYTERANGE:500\n#EXTINF:4,\nall.mp4\n#EXTINF:4,\nother.ts\n#EXT-X-ENDLIST";
        let p = hls(text, &base(), None, None).unwrap();
        let ranges: Vec<_> = p.iter().map(|p| p.range).collect();
        assert_eq!(
            ranges,
            [Some((0, 720)), Some((720, 1000)), Some((1720, 500)), None]
        );
    }

    #[test]
    fn what_isnt_listed() {
        assert_eq!(
            hls("#EXTM3U\n#EXTINF:4,\na.ts\n", &base(), None, None),
            Err(Unlisted::Live)
        );
        assert_eq!(
            hls(
                "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:4,\na.ts\n#EXT-X-ENDLIST",
                &base(),
                None,
                None
            ),
            Err(Unlisted::Protected("SAMPLE-AES".into()))
        );
        assert_eq!(
            hls(
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n#EXT-X-ENDLIST",
                &base(),
                None,
                None
            ),
            Err(Unlisted::NoPieces)
        );
        assert_eq!(
            hls("#EXTM3U\n#EXT-X-ENDLIST", &base(), None, None),
            Err(Unlisted::NoPieces)
        );
    }

    #[test]
    fn dash_pieces_from_yt_dlp() {
        let f = serde_json::json!({
            "fragment_base_url": "https://cdn.example/dash/v1/",
            "fragments": [
                {"path": "init.mp4"},
                {"url": "https://cdn.example/x/seg1.m4s", "duration": 4},
                {"path": "seg2.m4s", "byte_range": {"start": 10, "end": 20}},
                {"path": "seg3.m4s", "range": "20-29"}
            ]
        });
        let p = dash(&f).unwrap();
        assert_eq!(p[0].url.as_str(), "https://cdn.example/dash/v1/init.mp4");
        assert_eq!(p[1].url.as_str(), "https://cdn.example/x/seg1.m4s");
        assert_eq!(p[2].range, Some((10, 10)));
        assert_eq!(p[3].range, Some((20, 10)));
        assert!(dash(&serde_json::json!({"fragments": []})).is_none());
        assert!(dash(&serde_json::json!({"fragments": [{"path": "a"}]})).is_none());
    }

    #[test]
    fn aes_128_pieces_decrypt_as_yt_dlp_decrypts_them() {
        use aws_lc_rs::cipher::{
            AES_128, EncryptionContext, PaddedBlockEncryptingKey, UnboundCipherKey,
        };
        let key = *b"0123456789abcdef";
        let iv = sequence_iv(42);
        let plain: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let k = PaddedBlockEncryptingKey::cbc_pkcs7(UnboundCipherKey::new(&AES_128, &key).unwrap())
            .unwrap();
        let mut sealed = plain.clone();
        k.less_safe_encrypt(&mut sealed, EncryptionContext::Iv128(iv.into()))
            .unwrap();
        assert_ne!(sealed[..16], plain[..16]);
        assert_eq!(decrypt(&sealed, &key, &iv).unwrap(), plain);
        // The wrong IV spoils the first block only (CBC); not whole blocks: none.
        let wrong = decrypt(&sealed, &key, &sequence_iv(41)).unwrap();
        assert_ne!(wrong[..16], plain[..16]);
        assert_eq!(wrong[16..], plain[16..]);
        assert_eq!(decrypt(&sealed[..15], &key, &iv), None);
        assert_eq!(iv_of("0x1"), Some(sequence_iv(1)));
        assert_eq!(iv_of("12"), None);
    }

    /// A gallery's files (#769), each its own: a server that takes one connection at a time
    /// (503 to the others) has the extra connections close and give their files back, and every
    /// file comes whole, at its own path.
    #[test]
    fn a_busy_server_gets_fewer_connections() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let lab = rt
            .block_on(mdm_labserver::LabServer::start(Default::default()))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let n = 6u64;
        let pieces: Vec<Piece> = (0..n)
            .map(|i| Piece {
                url: Url::parse(&lab.url(&format!(
                    "/gen/128K/g{i}.jpg?seed={}&max_conns=1&rate=1M",
                    40 + i
                )))
                .unwrap(),
                range: None,
                key: None,
            })
            .collect();
        let paths: Vec<PathBuf> = (0..n)
            .map(|i| dir.path().join(format!("{i}.jpg")))
            .collect();
        let shared = Arc::new(Shared::default());
        let opts = mdm_engine::Options {
            max_connections: 4,
            retry_delay: Duration::from_millis(50),
            ..Default::default()
        };
        let ended = rt.block_on(download(
            pieces,
            Target::Files(paths.clone()),
            HeaderMap::new(),
            opts,
            Arc::new(Limiter::new(None)),
            CancellationToken::new(),
            shared.clone(),
            Arc::new(|| Err("listed once".into())),
        ));
        assert!(matches!(ended, Ended::Done(_)), "{ended:?}");
        for (i, p) in paths.iter().enumerate() {
            let mut want = vec![0; 128 * 1024];
            mdm_labserver::content::fill(40 + i as u64, 0, &mut want);
            assert!(std::fs::read(p).unwrap() == want, "file {i}");
        }
        let notes = shared.take_notes();
        assert!(
            notes.iter().any(|n| n.contains("closed, the others go on")),
            "{notes:?}"
        );
    }
}
