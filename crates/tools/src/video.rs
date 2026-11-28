//! A video download run by yt-dlp under supervision: our file name, progress read from a fixed
//! `--progress-template`, the final path from `--print after_move`. Stopping kills it; starting
//! again continues (`--continue`). Each run takes the page as looked up (`-J`), points links at
//! servers that answer ([`crate::servers`]) and downloads from that list (`--load-info-json`),
//! so a run reads the page once. A run looks the page up afresh, except the first run of a
//! download chosen in the browser's quality menu, which takes the look-up the menu was made from
//! ([`Preloaded`]): the page is then read once for the menu and the download together.
//!
//! The bytes come down through MDM's engine (#769, [`crate::streams`]): yt-dlp chooses the
//! formats, the engine downloads each stream into the file yt-dlp looks for, then yt-dlp merges
//! them, saves the subtitles and the metadata as after its own download. What the engine can't
//! download (a live stream, RTMP…) yt-dlp downloads itself, as before.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::proc::{self, ProcError};
use crate::servers;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Done(PathBuf),
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub state: State,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes/s.
    pub speed: f64,
    /// Notes for the download's log, oldest first (a server swapped for another).
    pub events: Vec<(std::time::SystemTime, String)>,
    /// Saved beside the video once it is done: its subtitle files (#68).
    pub side_files: Vec<PathBuf>,
    /// Subtitles that didn't come with the video, once it is done (#68): the video is saved all
    /// the same.
    pub sub_problems: Vec<SubProblem>,
    /// While MDM's engine downloads the streams (#769): its segments, connections and server.
    pub engine: Option<crate::streams::View>,
    /// Why yt-dlp downloads this one itself (#769), once a run has found out.
    pub helper: Option<crate::streams::Helper>,
    /// Once every stream is in: their file being joined into one (ffmpeg, run by yt-dlp; #736),
    /// with how far that is (0 to 1) when its file's size tells.
    pub merging: Option<Option<f64>>,
    /// It failed joining its streams: what of it is kept beside its file, the whole streams
    /// among them, for Resume to join again without downloading them (yt-dlp takes them as they
    /// are; the engine's `fetch` keeps a whole one). Deleting the download with its files
    /// removes them.
    pub merge_failed: Option<Vec<PathBuf>>,
    /// A live stream yt-dlp records: the size of its recording so far (its `.part`, read every
    /// second; yt-dlp says no progress while it records). 0 otherwise, and again once a run ends.
    pub recorded: u64,
    /// A live stream yt-dlp records: the recorder has opened its file (ffmpeg connected and
    /// created the `.part`, which stays 0 bytes until its first block, 256 KiB, is written), so
    /// the recording has started whatever `recorded` says. False otherwise, and again once a run
    /// ends.
    pub recording: bool,
}

/// What a live stream's download does now ([`VideoJob::live_state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    /// yt-dlp records it: the recorder has opened its file ([`Progress::recording`]), though
    /// what it has written ([`Progress::recorded`]) may still be 0. [`VideoJob::save`] keeps
    /// what there is.
    Recording,
    /// Stopped and being saved: its recording made into the file kept.
    Saving,
}

/// Whether a live stream's recorder runs and has opened its file, or has written some of it:
/// what [`LiveState::Recording`] and [`VideoJob::save`] both go by.
fn is_recording(p: &Progress) -> bool {
    p.helper == Some(crate::streams::Helper::Live) && (p.recorded > 0 || p.recording)
}

/// A subtitle language the site wouldn't give (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubProblem {
    /// yt-dlp's code (`en`) and the site's name for it (`English`; the code when none).
    pub code: String,
    pub name: String,
    pub reason: SubReason,
    /// They came on the second try, but beside the video, not in it (they were to be embedded).
    pub beside: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubReason {
    /// HTTP 429: the site limits how often subtitles may be fetched (YouTube does, most of all
    /// for its machine-translated captions).
    TooManyRequests,
    /// yt-dlp's own words.
    Other(String),
}

/// How long to wait before asking once more for the subtitles a site refused. A site that
/// answers "too many requests" wants a pause, not a quick retry. `MDM_SUBS_RETRY_SECS`
/// shortens it for the tests (tests/fake_ytdlp.rs).
fn subs_retry_delay() -> std::time::Duration {
    let secs = std::env::var("MDM_SUBS_RETRY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    std::time::Duration::from_secs(secs)
}

/// The subtitle languages yt-dlp said it couldn't download (with `--ignore-errors` it goes on
/// with the video and only warns: `Unable to download video subtitles for 'en': HTTP Error 429:
/// Too Many Requests`), and why.
fn subtitle_failures(warnings: &[String]) -> Vec<(String, SubReason)> {
    const SAID: &str = "Unable to download video subtitles for '";
    let mut out: Vec<(String, SubReason)> = Vec::new();
    for w in warnings {
        let Some(rest) = w.split_once(SAID).map(|(_, r)| r) else {
            continue;
        };
        let Some((code, why)) = rest.split_once('\'') else {
            continue;
        };
        let why = why.trim_start_matches(':').trim();
        let reason =
            if why.contains("429") || why.to_ascii_lowercase().contains("too many requests") {
                SubReason::TooManyRequests
            } else {
                SubReason::Other(why.to_owned())
            };
        if !out.iter().any(|(c, _)| c == code) {
            out.push((code.to_owned(), reason));
        }
    }
    out
}

/// The site's name for a subtitle language, from the looked-up page (`code` when none).
fn sub_name(info: &serde_json::Value, code: &str) -> String {
    ["subtitles", "automatic_captions"]
        .iter()
        .find_map(|k| info[k][code][0]["name"].as_str())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| code.to_owned())
}

/// What a second, subtitles-only run needs (#68): what the first asked for, and its own
/// arguments (the output name, the browser's context, where ffmpeg is).
#[derive(Clone, Debug)]
struct SubsRetry {
    automatic: bool,
    embed: bool,
    ffmpeg: bool,
    base: Vec<String>,
}

impl SubsRetry {
    /// yt-dlp's arguments to fetch `codes` again, beside the video (a second run can't put them
    /// into a video already saved).
    fn args(&self, codes: &[String]) -> Vec<String> {
        let again = Subs {
            langs: codes.to_vec(),
            automatic: self.automatic,
            embed: false,
        };
        let mut out = self.base.clone();
        out.push("--skip-download".into());
        out.extend(subtitle_args(&again, self.ffmpeg));
        out
    }
}

/// Which subtitles to save with a video (#68). `langs` empty: none.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Subs {
    /// yt-dlp's language codes, or [`ALL_UPLOADERS`] alone for every one the uploader gave.
    pub langs: Vec<String>,
    /// The site's automatic captions rather than the uploader's.
    pub automatic: bool,
    /// Put into the video file instead of beside it (needs ffmpeg; otherwise beside it).
    pub embed: bool,
}

/// `Subs::langs` for all of the uploader's subtitles.
pub const ALL_UPLOADERS: &str = "all";

/// The files subtitles are saved as.
const SUB_EXTS: [&str; 4] = ["srt", "vtt", "ass", "ttml"];

/// yt-dlp's arguments for `subs`: written beside the video as `<stem>.<lang>.srt` (converted to
/// SRT by ffmpeg, which every player reads; without ffmpeg SRT when the site has it, else its
/// own format, usually WebVTT), or embedded into the video with ffmpeg (yt-dlp removes the
/// separate files then).
fn subtitle_args(subs: &Subs, ffmpeg: bool) -> Vec<String> {
    if subs.langs.is_empty() {
        return Vec::new();
    }
    let langs = if subs.langs.len() == 1 && subs.langs[0] == ALL_UPLOADERS {
        // Not the chat replay of a live stream, which yt-dlp lists with them.
        "all,-live_chat".to_owned()
    } else {
        subs.langs.join(",")
    };
    let embed = subs.embed && ffmpeg;
    let mut out: Vec<String> = Vec::new();
    // `--embed-subs` fetches the uploader's itself, and keeps no files behind when it is the
    // only one asked (`--write-subs` too would keep them).
    match (subs.automatic, embed) {
        (true, _) => out.push("--write-auto-subs".into()),
        (false, false) => out.push("--write-subs".into()),
        (false, true) => {}
    }
    out.extend(["--sub-langs".into(), langs]);
    if ffmpeg {
        out.extend([
            "--sub-format".into(),
            "srt/best".into(),
            "--convert-subs".into(),
            "srt".into(),
        ]);
    } else {
        out.extend(["--sub-format".into(), "srt/vtt/best".into()]);
    }
    if embed {
        out.push("--embed-subs".into());
    }
    // A subtitle the site refuses must not cost the video: yt-dlp stops on it unless told to
    // go on, and then only warns (`subtitle_failures`). Whether the video was saved is checked
    // on its own (`attempt`).
    out.push("--ignore-errors".into());
    out
}

/// Download video's Embed chapters (#827): on unless the app says otherwise ([`set_chapters`]).
static CHAPTERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether the videos started from now on get their chapters (the app's setting, shared as it
/// changes; the command line keeps the default, on).
pub fn set_chapters(on: bool) {
    CHAPTERS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The chapters a video's page lists, put into the file (#827): `--embed-chapters` on yt-dlp's
/// last run, the one that merges and fixes the file up (after MDM's engine too). Only with ffmpeg,
/// which writes them, and for a video or the sound as the site has it, not a conversion to MP3,
/// Opus or FLAC. A video without chapters is left as it is.
fn chapter_args(chapters: bool, ffmpeg: bool, converted: bool) -> Vec<String> {
    if chapters && ffmpeg && !converted {
        vec!["--embed-chapters".into()]
    } else {
        Vec::new()
    }
}

/// The subtitle files saved beside `video` (`<stem>.<lang>.<srt|vtt…>`, as yt-dlp names them).
fn side_files(video: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem()) else {
        return Vec::new();
    };
    let prefix = format!("{}.", stem.to_string_lossy());
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p != video && p.is_file())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            name.is_some_and(|n| n.starts_with(&prefix))
                && ext.is_some_and(|e| SUB_EXTS.contains(&e.as_str()))
        })
        .collect();
    out.sort();
    out
}

/// How long the browser's quality menu's look-up may take: a stream's playlists
/// (`crate::manifest`, 5 s), yt-dlp's look-up and its second one without the cookies
/// (`proc::look_up`, 90 s each), and a little more. The download chosen there waits for it that
/// long at most, and a second click shares a look-up still running that long (the app's
/// videolookup.rs).
pub const LOOKUP_WAIT: Duration = Duration::from_secs(
    crate::manifest::BUDGET.as_secs() + 2 * crate::resolve::LOOK_UP_TIMEOUT.as_secs() + 5,
);

/// The largest look-up kept for the download (a page's is a few hundred KB, YouTube's a few MB):
/// a bigger one isn't kept, and the download looks the page up itself.
pub const INFO_MAX: usize = 8 * 1024 * 1024;

/// All the look-ups kept at once, together: past it, the oldest are let go (their downloads look
/// the page up themselves). Twenty look-ups are kept for ten minutes (videolookup.rs), and
/// [`INFO_MAX`] each would be 160 MB.
pub const INFO_TOTAL: usize = 32 * 1024 * 1024;

/// The look-ups' answers kept, oldest first, with their sizes: what [`INFO_TOTAL`] counts. A
/// look-up no longer kept anywhere (its slot dropped) counts no more.
struct Kept {
    limit: usize,
    answers: Mutex<std::collections::VecDeque<(std::sync::Weak<SlotInner>, usize)>>,
}

impl Kept {
    fn new(limit: usize) -> Arc<Kept> {
        Arc::new(Kept {
            limit,
            answers: Mutex::default(),
        })
    }

    /// Every look-up's: the process's own pool.
    fn shared() -> Arc<Kept> {
        static POOL: std::sync::LazyLock<Arc<Kept>> =
            std::sync::LazyLock::new(|| Kept::new(INFO_TOTAL));
        POOL.clone()
    }

    /// `slot` keeps an answer of `len` bytes now: the oldest others are let go while all of
    /// them together are more than the limit.
    fn add(&self, slot: &Arc<SlotInner>, len: usize) {
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.retain(|(s, _)| s.strong_count() > 0);
        answers.push_back((Arc::downgrade(slot), len));
        let mut total: usize = answers.iter().map(|(_, n)| n).sum();
        while total > self.limit && answers.len() > 1 {
            let Some((oldest, n)) = answers.pop_front() else {
                break;
            };
            total -= n;
            if let Some(s) = oldest.upgrade() {
                s.let_go();
            }
        }
    }
}

/// yt-dlp's look-up (`-J`) of a page for the browser's quality menu, kept for the download
/// chosen from it: being looked up, done (the answer as yt-dlp gave it, and whether it took
/// looking the page up without cookies), or failed. In memory only, never written or logged
/// (its links are signed): its `Debug` shows nothing of it.
#[derive(Clone)]
pub struct InfoSlot(Arc<SlotInner>);

struct SlotInner {
    info: Mutex<Info>,
    changed: Condvar,
    born: Instant,
    /// Where its answer counts ([`INFO_TOTAL`]).
    pool: Arc<Kept>,
}

impl SlotInner {
    /// Its answer dropped to make room: as if the look-up had failed.
    fn let_go(&self) {
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*info, Info::Ready(..)) {
            *info = Info::Failed;
            self.changed.notify_all();
        }
    }
}

enum Info {
    Pending,
    Ready(Arc<Vec<u8>>, bool),
    Failed,
}

impl std::fmt::Debug for InfoSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InfoSlot")
    }
}

impl InfoSlot {
    /// A look-up begun at `born`.
    pub fn new(born: Instant) -> Self {
        Self::counted_in(born, Kept::shared())
    }

    fn counted_in(born: Instant, pool: Arc<Kept>) -> Self {
        InfoSlot(Arc::new(SlotInner {
            info: Mutex::new(Info::Pending),
            changed: Condvar::new(),
            born,
            pool,
        }))
    }

    pub fn born(&self) -> Instant {
        self.0.born
    }

    fn info(&self) -> std::sync::MutexGuard<'_, Info> {
        self.0.info.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// yt-dlp answered with `json` (`without_cookies`: as `proc::LookedUp` says). Bigger than
    /// [`INFO_MAX`]: as if it had failed; the oldest others are let go past [`INFO_TOTAL`]. Only
    /// a look-up still pending takes it.
    pub fn fill(&self, json: Vec<u8>, without_cookies: bool) {
        let len = json.len();
        {
            let mut info = self.info();
            if !matches!(*info, Info::Pending) {
                return;
            }
            *info = if len > INFO_MAX {
                Info::Failed
            } else {
                Info::Ready(Arc::new(json), without_cookies)
            };
            self.0.changed.notify_all();
        }
        // Not under its own lock: letting another go locks that one.
        if len <= INFO_MAX {
            self.0.pool.add(&self.0, len);
        }
    }

    /// The look-up failed (or ended without an answer): the download looks the page up itself.
    pub fn fail(&self) {
        let mut info = self.info();
        if matches!(*info, Info::Pending) {
            *info = Info::Failed;
            self.0.changed.notify_all();
        }
    }

    pub fn pending(&self) -> bool {
        matches!(*self.info(), Info::Pending)
    }

    /// How many hold this look-up (the menu's, the download chosen from it): a check that a
    /// choice let go of it.
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    /// The answer once there, waiting until `until` at most, or until `stopped` says so.
    fn wait(&self, until: Instant, stopped: impl Fn() -> bool) -> Option<(Arc<Vec<u8>>, bool)> {
        let mut info = self.info();
        loop {
            match &*info {
                Info::Ready(json, without) => return Some((json.clone(), *without)),
                Info::Failed => return None,
                Info::Pending => {}
            }
            let now = Instant::now();
            if now >= until || stopped() {
                return None;
            }
            // A tenth of a second at a time: a Stop meanwhile is seen at once.
            let step = (until - now).min(Duration::from_millis(100));
            info = self
                .0
                .changed
                .wait_timeout(info, step)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// The browser's quality menu's look-up for the download chosen there (its first run only):
/// the look-up, and the choice as the menu offered it (MDM's selector, its label, untranslated,
/// and whether the menu's list had merged qualities). The page yt-dlp read must offer that very
/// choice, or the run looks the page up itself.
pub struct Preloaded {
    pub slot: InfoSlot,
    pub selector: String,
    pub label: String,
    pub merge: bool,
}

/// How a run tells whether a media server can be reached and serves a stream, in place of
/// `servers::reachable` and `servers::Prober` (which ask the servers): set by tests only
/// (tests/fake_ytdlp.rs), whose servers must never be asked.
type ServerChecks = (fn(&str) -> bool, fn(&str) -> bool);

static SERVER_CHECKS: std::sync::RwLock<Option<ServerChecks>> = std::sync::RwLock::new(None);

/// Tests only: media servers `alive` says are reachable, serving every stream `serves` says.
#[doc(hidden)]
pub fn check_servers_with(alive: fn(&str) -> bool, serves: fn(&str) -> bool) {
    *SERVER_CHECKS.write().unwrap_or_else(|e| e.into_inner()) = Some((alive, serves));
}

/// yt-dlp's look-up `json` as a run's page, when it offers `selector` under `label` among its
/// choices (`resolve::choices` with `merge`): the menu offered that very choice from it.
fn offered(json: &[u8], selector: &str, label: &str, merge: bool) -> Option<serde_json::Value> {
    let video = crate::resolve::parse(json)?;
    crate::resolve::choices(&video, merge)
        .iter()
        .any(|c| c.selector == selector && c.label == label)
        .then(|| serde_json::from_slice(json).ok())
        .flatten()
}

/// The containers and sounds yt-dlp saves (`%(ext)s`), merged, converted or as the site has them.
const MEDIA: [&str; 19] = [
    "mp4", "m4a", "m4v", "webm", "weba", "mkv", "mka", "mov", "flv", "3gp", "ts", "mp3", "opus",
    "ogg", "oga", "flac", "wav", "aac", "avi",
];
/// The subtitle files yt-dlp saves beside a video (#68).
const SUBTITLES: [&str; 10] = [
    "srt", "vtt", "ass", "ssa", "ttml", "srv1", "srv2", "srv3", "json3", "lrc",
];

/// Whether `rest`, a file name after `<stem>.` (lower case), is one yt-dlp writes for a download
/// saved as `<stem>.%(ext)s`: an unfinished run's (`<ext>.part`, `<ext>.ytdl`,
/// `<ext>.part-Frag<N>[.part]`, each also for one stream of a merged choice as `f<id>.<ext>…`,
/// that stream once whole, `f<id>.<ext>`, and a merge's `temp.<ext>`; MDM's engine's part file
/// and resume state for such a stream, `f<id>.<ext>.mdm.part` and `.mdm.json`, #769); with
/// `outputs`, also the finished file (`<ext>`) and its subtitles (`<language>.<srt|vtt…>`).
/// Nothing else: another download's files (MDM's own `.mdm.part`), a file merely named alike.
fn yt_dlp_leftover(rest: &str, outputs: bool) -> bool {
    let media = |s: &str| MEDIA.contains(&s);
    let word = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    // A format id as yt-dlp names a stream: `137`, `hls-720p`, `dash-1`, `http-480`.
    let format = |s: &str| {
        s.strip_prefix('f').is_some_and(|id| {
            word(id)
                && (id.starts_with(|c: char| c.is_ascii_digit())
                    || ["hls-", "dash-", "http-"].iter().any(|p| {
                        id.strip_prefix(p)
                            .is_some_and(|r| r.chars().any(|c| c.is_ascii_digit()))
                    }))
        })
    };
    let fragment = |s: &str| {
        s.strip_prefix("part-frag")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let parts: Vec<&str> = rest.split('.').collect();
    // An unfinished file: `<ext>` then `part`, `ytdl`, `part-frag<N>` or `part-frag<N>.part`.
    let unfinished = |p: &[&str]| match p {
        [ext, "part" | "ytdl"] => media(ext),
        [ext, frag] => media(ext) && fragment(frag),
        [ext, frag, "part"] => media(ext) && fragment(frag),
        _ => false,
    };
    let working = match parts.as_slice() {
        ["temp", ext] => media(ext),
        [f, ext] if format(f) => media(ext),
        [f, ext, "mdm", "part" | "json"] if format(f) => media(ext),
        [f, tail @ ..] if format(f) => unfinished(tail),
        p => unfinished(p),
    };
    let finished = outputs
        && match parts.as_slice() {
            [ext] => media(ext),
            [language, sub] => word(language) && SUBTITLES.contains(sub),
            _ => false,
        };
    working || finished
}

/// What yt-dlp left in `dir` for a download saved as `<stem>.%(ext)s` (our template), as
/// [`yt_dlp_leftover`] tells them, letter case aside: what an unfinished run left, and with
/// `outputs` the finished file and its subtitles. A file not of that shape is left alone.
pub fn own_files(dir: &Path, stem: &str, outputs: bool) -> Vec<PathBuf> {
    let prefix = format!("{}.", stem.to_lowercase());
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.strip_prefix(&prefix)
                .is_some_and(|rest| yt_dlp_leftover(rest, outputs))
        })
        .map(|e| e.path())
        .collect()
}

pub struct VideoJob {
    /// yt-dlp once the page is looked up (`None` before).
    child: Arc<Mutex<Option<proc::Spawned>>>,
    progress: Arc<Mutex<Progress>>,
    /// The engine's download of a stream, while one is under way (#769).
    current: Arc<crate::streams::Current>,
    recorder: Arc<Recorder>,
}

/// A live stream's recording, stopped and saved ([`VideoJob::save`]): yt-dlp is ended as a Stop
/// ends it (it records with ffmpeg, which leaves the recording an MPEG-TS `.part`), then MDM's
/// ffmpeg makes that `.part` into the MP4 kept.
struct Recorder {
    /// Set once by `save`, under the child's lock and the progress's: from then on no yt-dlp
    /// starts again, and a Stop changes nothing. Read and set with the progress locked, so it
    /// and the state change together.
    saving: std::sync::atomic::AtomicBool,
    /// MDM's ffmpeg, which makes the recording an MP4; without it the recording is kept as the
    /// MPEG-TS it is.
    ffmpeg: Option<PathBuf>,
}

impl Recorder {
    fn saving(&self) -> bool {
        self.saving.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// How long a rename or a removal of a recording's files is tried again: the processes just
/// ended may hold them a moment longer (Windows: a file open in ffmpeg can't be renamed).
const RETRY_FILES: Duration = Duration::from_secs(10);

/// `f` until it works, for [`RETRY_FILES`] at most; its last error then.
fn retrying<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let end = Instant::now() + RETRY_FILES;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if Instant::now() >= end => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// `path` removed, or already gone.
fn removed(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What yt-dlp records a live stream into, in `dir` for `<stem>.%(ext)s`: its unfinished files
/// (`<stem>.<ext>.part`, a stream's `<stem>.f<id>.<ext>.part`), not the fragments of a stream
/// in pieces nor MDM's engine's own. In name order.
fn recordings(dir: &Path, stem: &str) -> Vec<PathBuf> {
    let mut parts: Vec<PathBuf> = own_files(dir, stem, false)
        .into_iter()
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            name.ends_with(".part") && !name.ends_with(".mdm.part") && !name.contains(".part-frag")
        })
        .collect();
    parts.sort();
    parts
}

/// [`recordings`] with their sizes (their own: a directory's listing may show a file another
/// process writes as smaller).
fn parts_of(dir: &Path, stem: &str) -> Vec<(PathBuf, u64)> {
    recordings(dir, stem)
        .into_iter()
        .filter_map(|p| std::fs::metadata(&p).ok().map(|m| (p, m.len())))
        .collect()
}

/// Whether `path` is an MPEG transport stream ([`content_ext`]), what yt-dlp's ffmpeg records a
/// live stream as.
fn is_ts(path: &Path) -> bool {
    use std::io::Read;
    let mut head = Vec::with_capacity(8192);
    std::fs::File::open(path)
        .and_then(|f| f.take(8192).read_to_end(&mut head))
        .is_ok_and(|_| content_ext(&head) == Some("ts"))
}

/// A name for a recording kept in `dir`: `<stem>.<ext>` (when `plain`) if free, else
/// `<stem> (recording N).<ext>`, the first free. Never MDM's own numbering, `<stem> (N)`, which
/// another download of the same name takes (and its files with it).
fn recording_name(dir: &Path, stem: &str, ext: &str, plain: bool) -> std::io::Result<PathBuf> {
    let plain = plain.then(|| dir.join(format!("{stem}.{ext}")));
    plain
        .into_iter()
        .chain((1..1000).map(|n| dir.join(format!("{stem} (recording {n}).{ext}"))))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .ok_or_else(|| std::io::Error::other("no free name"))
}

/// `from` renamed to a free [`recording_name`], tried again for a while ([`retrying`]).
fn keep_as(
    from: &Path,
    dir: &Path,
    stem: &str,
    ext: &str,
    plain: bool,
) -> std::io::Result<PathBuf> {
    retrying(|| {
        let to = recording_name(dir, stem, ext, plain)?;
        std::fs::rename(from, &to).map(|()| to)
    })
}

/// A file's name, for the log.
fn name_of(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Earlier recordings `parts` (MPEG-TS, checked) kept aside, each as its own
/// `<stem> (recording N).ts`, the log saying so; the first that can't be renamed stops it
/// (`unfinished` says why), and it stays as it is.
fn keep_aside(
    parts: &[PathBuf],
    dir: &Path,
    stem: &str,
    progress: &Mutex<Progress>,
    unfinished: impl Fn(&Path, String) -> String,
) -> Result<(), String> {
    for part in parts {
        match keep_as(part, dir, stem, "ts", false) {
            Ok(to) => note(
                progress,
                format!("The earlier recording was kept as {}", name_of(&to)),
            ),
            Err(e) => return Err(unfinished(part, format!("couldn't be kept aside ({e})"))),
        }
    }
    Ok(())
}

/// An earlier run's recording parts of `<stem>` that are exactly 0 bytes (see [`recordings`]; one
/// with yt-dlp's resume state beside it, its `.ytdl` or `.part-Frag` pieces, is a download to go
/// on with, and stays) removed, the log saying so: the recorder creates its file empty once it has
/// connected and writes its first block much later, so a stop before that leaves one, which holds
/// nothing and would otherwise pass for the new recording's. The length is read again right
/// before each removal, so nothing that holds anything is removed; one that can't be removed
/// stays, and nothing stops for it.
fn remove_empty_parts(dir: &Path, stem: &str, progress: &Mutex<Progress>) {
    let names: Vec<String> = own_files(dir, stem, false)
        .iter()
        .map(|p| name_of(p).to_lowercase())
        .collect();
    for (part, len) in parts_of(dir, stem) {
        let name = name_of(&part).to_lowercase();
        let base = name.strip_suffix(".part").unwrap_or(&name);
        let resumable = names
            .iter()
            .any(|n| *n == format!("{base}.ytdl") || n.starts_with(&format!("{name}-frag")));
        if len == 0
            && !resumable
            && std::fs::metadata(&part).is_ok_and(|m| m.len() == 0)
            && std::fs::remove_file(&part).is_ok()
        {
            note(
                progress,
                format!(
                    "Removed the empty unfinished file {} left by an earlier run",
                    name_of(&part)
                ),
            );
        }
    }
}

/// Before yt-dlp records a live stream again (Resume, another try): it would write its `.part`
/// files over (ffmpeg `-y`), so every earlier recording left there (a stream's own part too) is
/// kept aside first, each as `<stem> (recording N).ts`, and the log says so. One that isn't
/// MPEG-TS, or can't be renamed: yt-dlp isn't started, and the error names it (none that isn't
/// MPEG-TS is renamed; it stays as it is). An earlier run's file that is empty (a stop in its
/// first seconds, before the recorder's first block) holds nothing to keep: it is removed
/// ([`remove_empty_parts`]), so the recording that starts now is told from it by its file
/// being created.
fn set_aside_recordings(ours: &[String], progress: &Mutex<Progress>) -> Result<(), String> {
    let Some((dir, stem)) = template_of(ours) else {
        return Ok(());
    };
    let unfinished = |part: &Path, why: String| {
        format!(
            "An earlier recording of this live stream wasn't finished: {} {why}; move or rename it, then resume",
            part.display()
        )
    };
    remove_empty_parts(&dir, &stem, progress);
    // Nothing recorded in one: nothing to lose.
    let parts: Vec<PathBuf> = parts_of(&dir, &stem)
        .into_iter()
        .filter(|(_, len)| *len > 0)
        .map(|(p, _)| p)
        .collect();
    if let Some(odd) = parts.iter().find(|p| !is_ts(p)) {
        return Err(unfinished(
            odd,
            "isn't MPEG-TS, so it can't be kept aside".into(),
        ));
    }
    keep_aside(&parts, &dir, &stem, progress, unfinished)
}

/// Before any run of a video (live or not, whoever downloads it): a recording an earlier run left
/// (a live stream stopped, Resumed once it ended and became a video yt-dlp downloads on to that
/// very file; a recording of the picture and the sound apart that was cancelled, or couldn't be
/// joined, and was kept as it was) is kept aside, each part as its own `<stem> (recording N).ts`:
/// a `<stem>.<ext>.part` or a stream's `<stem>.f<id>.<ext>.part` that is MPEG-TS with no resume
/// state of yt-dlp's beside it (its `.ytdl`, its `.part-Frag` pieces). Any other `.part` (a DASH
/// stream's MP4 or WebM) is a download to go on with, and is left alone. One that can't be
/// renamed: nothing starts.
fn set_aside_earlier(ours: &[String], progress: &Mutex<Progress>) -> Result<(), String> {
    let Some((dir, stem)) = template_of(ours) else {
        return Ok(());
    };
    let names: Vec<String> = own_files(&dir, &stem, false)
        .iter()
        .map(|p| name_of(p).to_lowercase())
        .collect();
    let parts: Vec<PathBuf> = parts_of(&dir, &stem)
        .into_iter()
        .filter(|(p, len)| {
            let name = name_of(p).to_lowercase();
            let base = name.strip_suffix(".part").unwrap_or(&name);
            let resumable = names
                .iter()
                .any(|n| *n == format!("{base}.ytdl") || n.starts_with(&format!("{name}-frag")));
            *len > 0 && !resumable && is_ts(p)
        })
        .map(|(p, _)| p)
        .collect();
    keep_aside(&parts, &dir, &stem, progress, |part, why| {
        format!(
            "An earlier recording of this video wasn't finished: {} {why}; move or rename it, then resume",
            part.display()
        )
    })
}

/// How a saved recording ended.
#[derive(Debug, PartialEq)]
enum Saved {
    Done(PathBuf),
    /// Nothing was recorded.
    Stopped,
    Failed(String),
}

/// A live stream's recording stopped and saved ([`VideoJob::save`]), yt-dlp ended: its `.part`
/// files (MPEG-TS; a stream's each, when yt-dlp recorded the picture and the sound apart) made
/// into one `<stem>.mp4` by MDM's `ffmpeg` ([`remuxed`]), named `<stem> (recording N).mp4` when
/// the name is taken. A recording is removed only when the MP4 is known to hold all of it
/// ([`whole`]); otherwise it is kept beside the MP4, a TS one as `<stem> (recording N).ts` (never
/// `<stem>.ts`, which would be taken for the download's own file), and the log says so: a part
/// that can't be renamed stays as it is, said too, and is never removed.
///
/// No usable MP4 (ffmpeg failed, made too little, or there is none): a recording of one part is
/// kept as the MPEG-TS it is, `<stem>.ts`; never a file that isn't one: a `.part` that isn't
/// MPEG-TS stays as it is, and the download fails saying where it is. Several parts are kept as
/// they are, and the download fails naming them. `said`: the file yt-dlp said it saved (`MDMF`).
fn finish_recording(
    ours: &[String],
    said: Option<PathBuf>,
    ffmpeg: Option<&Path>,
    progress: &Mutex<Progress>,
) -> Saved {
    let Some((dir, stem)) = template_of(ours) else {
        return Saved::Stopped;
    };
    let all = parts_of(&dir, &stem);
    // yt-dlp finished the file itself (it said so, or it is there by our name).
    if all.is_empty() {
        return match said.filter(|f| f.is_file()).or_else(|| saved_by_name(ours)) {
            Some(f) => Saved::Done(f),
            None => Saved::Stopped,
        };
    }
    // Nothing recorded in a part: nothing of it to keep.
    let parts: Vec<(PathBuf, u64)> = all.into_iter().filter(|(_, len)| *len > 0).collect();
    if parts.is_empty() {
        return Saved::Stopped;
    }
    let listed = || {
        parts
            .iter()
            .map(|(p, _)| name_of(p))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if parts.len() > 1 {
        note(
            progress,
            format!("The recording came in {} parts: {}", parts.len(), listed()),
        );
    }
    let made = match ffmpeg {
        Some(ffmpeg) => remuxed(ffmpeg, &parts, &dir, &stem),
        None => Err("there's no ffmpeg".to_owned()),
    };
    match made {
        Ok(mp4) => {
            if whole(ffmpeg.and_then(ffprobe_of).as_deref(), &mp4, &parts) {
                for (part, _) in &parts {
                    if let Err(e) = retrying(|| removed(part)) {
                        note(
                            progress,
                            format!(
                                "The recording is saved, but its unfinished file {} couldn't be removed ({e})",
                                part.display()
                            ),
                        );
                    }
                }
            } else {
                // The MP4 may lack some of it: every part stays, a TS one named as a video beside
                // it, out of the download's own names.
                let mut kept = Vec::new();
                for (part, _) in &parts {
                    if !is_ts(part) {
                        kept.push(format!("{} (as it is)", name_of(part)));
                        continue;
                    }
                    match keep_as(part, &dir, &stem, "ts", false) {
                        Ok(to) => kept.push(name_of(&to)),
                        Err(e) => note(
                            progress,
                            format!(
                                "The MP4 doesn't hold all of the recording (it plays shorter, or one of its streams starts later or ends sooner), and the recording {} couldn't be renamed ({e}); it stays as {}",
                                part.display(),
                                name_of(part)
                            ),
                        ),
                    }
                }
                if !kept.is_empty() {
                    note(
                        progress,
                        format!(
                            "The MP4 doesn't hold all of the recording (it plays shorter, or one of its streams starts later or ends sooner), so the recording is kept beside it too: {}",
                            kept.join(", ")
                        ),
                    );
                }
            }
            return Saved::Done(mp4);
        }
        Err(why) if parts.len() > 1 => {
            return Saved::Failed(format!(
                "The recording's {} parts couldn't be joined into an MP4 ({why}); they are kept as they are: {}",
                parts.len(),
                parts
                    .iter()
                    .map(|(p, _)| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Err(why) if ffmpeg.is_some() => note(
            progress,
            format!("The recording couldn't be made an MP4 ({why}); keeping it as MPEG-TS"),
        ),
        Err(_) => {}
    }
    let part = &parts[0].0;
    let failed = |why: String| {
        Saved::Failed(format!(
            "The recording couldn't be finished: {} {why}; it is kept as it is",
            part.display()
        ))
    };
    if !is_ts(part) {
        return failed("isn't MPEG-TS".into());
    }
    match keep_as(part, &dir, &stem, "ts", true) {
        Ok(file) => Saved::Done(file),
        Err(e) => failed(format!("couldn't be renamed ({e})")),
    }
}

/// `p` for ffmpeg and ffprobe as yt-dlp names files to them: `file:<p>`, never read as a
/// protocol or an option.
fn ff_file(p: &Path) -> std::ffi::OsString {
    let mut s = std::ffi::OsString::from("file:");
    s.push(p);
    s
}

/// The recording `parts` (with their sizes) made into one MP4 by `ffmpeg`: every stream of every
/// part (`-map` each input) copied as it is into one file, data streams left out and a stream
/// the MP4 can't hold dropped, as yt-dlp's `FixupM3u8` does with the one file of a live stream it
/// recorded; a picture and a sound recorded apart are joined that way, side by side. Into
/// `<stem>.temp.mp4`, then renamed to its [`recording_name`]. Usable only when ffmpeg ended well and its file is at least half the
/// parts' size (a copy of the same streams). Whatever fails, the temporary file goes and the
/// parts stay.
fn remuxed(
    ffmpeg: &Path,
    parts: &[(PathBuf, u64)],
    dir: &Path,
    stem: &str,
) -> Result<PathBuf, String> {
    let temp = dir.join(format!("{stem}.temp.mp4"));
    let len: u64 = parts.iter().map(|(_, n)| n).sum();
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-nostdin", "-y"]);
    for (part, _) in parts {
        cmd.arg("-i").arg(ff_file(part));
    }
    for i in 0..parts.len() {
        cmd.args(["-map".to_owned(), i.to_string()]);
    }
    cmd.args(["-dn", "-ignore_unknown", "-c", "copy", "-f", "mp4"])
        .arg(ff_file(&temp));
    // A copy runs at the disk's speed: ten minutes, and a second for every 2 MB.
    let timeout = Duration::from_secs(600 + len / (2 << 20));
    let made = proc::output(cmd, "ffmpeg", timeout)
        .map_err(|e| e.to_string())
        .and_then(|_| {
            std::fs::metadata(&temp)
                .map(|m| m.len())
                .map_err(|_| "ffmpeg made no file".to_owned())
        })
        .and_then(|n| {
            if n.saturating_mul(2) >= len {
                Ok(())
            } else {
                Err(format!("ffmpeg's file is {n} bytes, the recording {len}"))
            }
        })
        .and_then(|()| {
            keep_as(&temp, dir, stem, "mp4", true)
                .map_err(|e| format!("{} couldn't be renamed ({e})", temp.display()))
        });
    if made.is_err() {
        let _ = retrying(|| removed(&temp));
    }
    made
}

/// ffprobe beside `ffmpeg` (MDM's ffmpeg comes with it): its name with `ffprobe` for `ffmpeg`,
/// else `ffprobe` there. None when it isn't there.
fn ffprobe_of(ffmpeg: &Path) -> Option<PathBuf> {
    let name = name_of(ffmpeg);
    let probe = match name.rfind("ffmpeg") {
        Some(i) => format!("{}ffprobe{}", &name[..i], &name[i + "ffmpeg".len()..]),
        None => format!("ffprobe{}", std::env::consts::EXE_SUFFIX),
    };
    Some(ffmpeg.with_file_name(probe)).filter(|p| p.is_file())
}

/// What ffprobe reads of a file: how long it plays (its container's duration), and how long each
/// of its audio and video streams does (None: not said).
struct Probed {
    plays: f64,
    streams: Vec<Option<f64>>,
}

/// [`Probed`] of `file`, in seconds. None when ffprobe can't tell, or only estimated it from the
/// bitrate (a file whose timestamps it couldn't read: that length is a guess).
fn probed(ffprobe: &Path, file: &Path) -> Option<Probed> {
    let mut cmd = std::process::Command::new(ffprobe);
    cmd.args([
        "-v",
        "warning",
        "-show_entries",
        "format=duration:stream=codec_type,duration",
        "-of",
        "json",
    ])
    .arg(ff_file(file));
    let (out, said) = proc::output_said(cmd, "ffprobe", Duration::from_secs(60)).ok()?;
    if said.contains("Estimating duration from bitrate") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out).ok()?;
    let seconds = |d: &serde_json::Value| {
        d.as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d >= 0.0)
    };
    let streams = v["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| matches!(s["codec_type"].as_str(), Some("audio" | "video")))
        .map(|s| seconds(&s["duration"]))
        .collect();
    Some(Probed {
        plays: seconds(&v["format"]["duration"])?,
        streams,
    })
}

/// How much shorter than the recording an MP4 made of it may play and still hold all of it:
/// timestamps' rounding, a last frame. Not a share of it, which grows with a long recording.
const SLACK: f64 = 1.0;

/// Whether the MP4 made of a recording's `parts` holds all of it, so the parts may go, by
/// ffprobe's durations: it plays as long as the longest part, and every one of its audio and
/// video streams as long as the shortest part (a sound cut short in an MP4 that plays to the
/// end is not all of it), within [`SLACK`]. A length ffprobe can't read, or only estimated
/// (or no ffprobe): the MP4 is nine tenths of the parts' size at least. A copy that stopped
/// early (a damaged stream ffmpeg ended well on) is neither, and the parts stay.
fn whole(ffprobe: Option<&Path>, mp4: &Path, parts: &[(PathBuf, u64)]) -> bool {
    let durations = ffprobe.and_then(|probe| {
        let made = probed(probe, mp4)?;
        let tracks: Vec<f64> = made.streams.iter().copied().collect::<Option<_>>()?;
        let lengths: Vec<f64> = parts
            .iter()
            .map(|(p, _)| probed(probe, p).map(|p| p.plays))
            .collect::<Option<_>>()?;
        let track = tracks.iter().copied().reduce(f64::min)?;
        let longest = lengths.iter().copied().reduce(f64::max)?;
        let shortest = lengths.iter().copied().reduce(f64::min)?;
        Some((made.plays, track, longest, shortest))
    });
    if let Some((plays, track, longest, shortest)) = durations {
        let whole = plays >= longest - SLACK && track >= shortest - SLACK;
        tracing::info!(
            "A saved recording's MP4 plays {plays:.1} s (its shortest stream {track:.1} s) of its {longest:.1} s: {}",
            if whole { "whole" } else { "short" }
        );
        return whole;
    }
    let len: u64 = parts.iter().map(|(_, n)| n).sum();
    let made = std::fs::metadata(mp4).map_or(0, |m| m.len());
    let whole = made.saturating_mul(10) >= len.saturating_mul(9);
    tracing::info!(
        "A saved recording's MP4 is {made} bytes of its {len}, its length unknown: {}",
        if whole { "whole" } else { "short" }
    );
    whole
}

/// A private file for the looked-up page (its links are signed; removed when the run ends), in
/// the helpers' temporary folder ([`crate::workdir::temp_dir`]).
fn info_file() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    crate::workdir::temp_dir().join(format!(
        "mdm-video-{}-{}.json",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The page's video information from yt-dlp (one video, not a playlist), asked as the browser
/// asked for the page (`browser`: its Referer, User-Agent and cookies, `FromBrowser::args`).
/// Without them (#229) a page that wants a sign-in, YouTube's "confirm you're not a bot", was
/// refused here although the video window's look-up, which had them, had listed its qualities.
fn look_up(exe: &Path, url: &str, browser: &[String]) -> Result<(serde_json::Value, bool), String> {
    let mut args: Vec<String> = ["-J", "--no-playlist", "--no-warnings"]
        .map(String::from)
        .to_vec();
    args.extend(browser.iter().cloned());
    let out = proc::look_up(exe, &args, url, std::time::Duration::from_secs(90))
        .map_err(|e| e.to_string())?;
    let info = serde_json::from_slice(&out.json)
        .map_err(|_| "yt-dlp gave no video information".to_owned())?;
    Ok((info, out.without_cookies))
}

type Child = Arc<Mutex<Option<proc::Spawned>>>;

/// The browser's cookies for one video's download (#686). Signed in, YouTube may refuse what
/// yt-dlp asks as that viewer (its look-up, or the streams it gave) where a private window gets
/// the video: once its look-up worked only without them, or its streams were refused with them,
/// every later run of this video (a new address, another try, the subtitles) goes without them.
/// Another video still gets them first, and other sites' runs keep them.
#[derive(Default)]
struct Cookies {
    /// This video goes without them from now on.
    without: std::cell::Cell<bool>,
    /// The last run looked the YouTube page up with them, and anyone may watch the video: its
    /// streams refused may be the signed-in viewer's refusal.
    droppable: std::cell::Cell<bool>,
}

impl Cookies {
    /// `args` (yt-dlp's for a run of this video) as it goes now: without the cookies once it
    /// goes without them.
    fn args(&self, args: &[String]) -> Vec<String> {
        if self.without.get() {
            proc::without_cookies(args)
        } else {
            args.to_vec()
        }
    }
}

/// How one run of yt-dlp ended.
enum Attempt {
    /// The video saved, the subtitles that didn't come with it, and the quality yt-dlp chose
    /// (`137+251 1920x1080`).
    Done(PathBuf, Vec<SubProblem>, Option<String>),
    Stopped,
    /// `rerouted`: links were moved to other servers for this run.
    Failed {
        error: String,
        rerouted: bool,
    },
    /// Every stream came, and joining them failed: no other server or look-up mends that. With
    /// the streams' format ids (`MDMQ`'s `1280+aud-English`).
    MergeFailed(String, Vec<String>),
    /// A live stream's recording stopped to be saved ([`VideoJob::save`]): never another run,
    /// whatever ended this one. With the file yt-dlp said it saved (`MDMF`), if it did.
    Save(Option<PathBuf>),
    /// yt-dlp wasn't started: it would have recorded over an earlier recording that couldn't be
    /// kept aside ([`set_aside_recordings`]). No other try mends that.
    Blocked(String),
}

/// End yt-dlp and everything it started (#79): whenever the download is given up, nothing may
/// go on downloading unseen (its job on Windows, its process group on Linux and macOS, #1301:
/// the launcher's child and ffmpeg with it).
fn end(child: &Child) {
    let mut slot = child.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut s) = slot.take() {
        s.kill();
    }
}

/// The format selector in yt-dlp's arguments (`-f <selector>`).
fn selector_of(ours: &[String]) -> Option<&str> {
    ours.iter()
        .position(|a| a == "-f")
        .and_then(|i| ours.get(i + 1))
        .map(String::as_str)
}

/// `ours` with another format selector.
fn with_selector(ours: &[String], selector: &str) -> Vec<String> {
    let mut out = ours.to_vec();
    if let Some(i) = out.iter().position(|a| a == "-f")
        && let Some(s) = out.get_mut(i + 1)
    {
        *s = selector.to_owned();
    }
    out
}

/// The nearest quality to a choice from the window (`bv*[height=1080]+ba/b[height=1080]`) when
/// that one isn't offered (#94: the servers this PC reaches don't serve it): the best at or below
/// that height, else the best video there is, always a video (`b` alone would take a sound-only
/// format when no video is left; live 2026-11-05). None when the choice names no height.
fn relaxed(selector: &str) -> Option<String> {
    selector
        .contains("[height=")
        .then(|| format!("{}/bv*+ba/bv*", selector.replace("[height=", "[height<=")))
}

/// Runs the download, and again when a run fails in a way a new look-up can mend:
/// - a media server yt-dlp couldn't reach (it resolved when looked up, then didn't: #94) is
///   treated as dead, so its streams move to an alternative;
/// - a run whose links had to be moved to other servers gets one fresh look-up (new links, often
///   other servers: YouTube's alternatives refuse some streams some of the time);
/// - the chosen quality not offered at all (left out by the move, #94) falls back to the nearest
///   one below it, said in the log; never the whole download's failure while another quality
///   is there;
/// - no video left at all after the move: once more on the page's own servers, unmoved;
/// - a YouTube video looked up with the browser's cookies whose streams YouTube refused (#686,
///   [`crate::youtube::refused_signed_in`]): once more without the cookies, as a private window
///   gets it, unless only a signed-in viewer may watch it. That video goes without them from then
///   on ([`Cookies`]).
///
/// Whenever it is given up, yt-dlp's whole job is ended (#79).
#[allow(clippy::too_many_arguments)]
fn supervise(
    exe: &Path,
    url: &str,
    ours: &[String],
    browser: &[String],
    progress: &Mutex<Progress>,
    child: &Child,
    expected: Option<u64>,
    retry: Option<&SubsRetry>,
    preloaded: Option<Preloaded>,
    engine: Option<&ByEngine>,
    saved: Option<&mdm_engine::FileHook>,
    recorder: &Recorder,
    part: Option<&PartRun>,
) {
    // The first run only: a run after it looks the page up afresh (links expire).
    let mut preloaded = preloaded;
    let mut ours = ours.to_vec();
    let mut dead: std::collections::BTreeSet<String> = Default::default();
    let mut looked_again = false;
    let mut relaxed_to: Option<String> = None;
    let mut reroute = true;
    let mut last = String::from("yt-dlp stopped");
    let cookies = Cookies::default();
    // An earlier run's recording is never downloaded on to, whatever this run turns out to be.
    if let Err(error) = set_aside_earlier(&ours, progress) {
        let mut p = progress.lock().unwrap();
        if p.state != State::Stopped {
            p.state = State::Failed(error);
        }
        return;
    }
    for _ in 0..5 {
        let page = preloaded.take();
        // Being saved: no run after it, another look-up neither.
        let ended = if recorder.saving() {
            Attempt::Save(None)
        } else {
            attempt(
                exe,
                url,
                &ours,
                browser,
                progress,
                child,
                expected,
                retry,
                &dead,
                reroute,
                page.as_ref(),
                engine,
                &cookies,
                recorder,
                part,
            )
        };
        match ended {
            Attempt::Stopped => return,
            Attempt::Done(f, problems, quality) => {
                end(child);
                // Which way it worked (#686): the browser's cookies are the usual way, unsaid.
                if cookies.without.get() && proc::has_cookies(browser) {
                    note(progress, "Downloaded without the browser's cookies".into());
                    tracing::info!("A video was downloaded without the browser's cookies (#686)");
                }
                if relaxed_to.is_some()
                    && let Some(q) = quality
                {
                    note(progress, format!("Downloaded {q} instead"));
                }
                done(progress, saved, f, problems);
                return;
            }
            Attempt::Save(said) => {
                // Every process of yt-dlp's ended first: until then its ffmpeg holds the file.
                end(child);
                match finish_recording(&ours, said, recorder.ffmpeg.as_deref(), progress) {
                    Saved::Done(f) => done(progress, saved, f, Vec::new()),
                    Saved::Stopped => {
                        let mut p = progress.lock().unwrap();
                        p.speed = 0.0;
                        p.state = State::Stopped;
                    }
                    Saved::Failed(error) => {
                        let mut p = progress.lock().unwrap();
                        p.speed = 0.0;
                        p.state = State::Failed(error);
                    }
                }
                return;
            }
            Attempt::Blocked(error) => {
                end(child);
                let mut p = progress.lock().unwrap();
                if p.state != State::Stopped {
                    p.speed = 0.0;
                    p.state = State::Failed(error);
                }
                return;
            }
            Attempt::MergeFailed(error, ids) => {
                end(child);
                // The streams stay for Resume (the merge's own unfinished file is gone).
                let kept = template_of(&ours)
                    .map(|(dir, stem)| kept_parts(&dir, &stem, &ids))
                    .unwrap_or_default();
                let mut p = progress.lock().unwrap();
                if p.state != State::Stopped {
                    p.speed = 0.0;
                    p.merging = None;
                    p.merge_failed = Some(kept);
                    p.state = State::Failed(error);
                }
                return;
            }
            Attempt::Failed { error, rerouted } => {
                end(child);
                let gone: Vec<String> = servers::unreachable_in(&error)
                    .into_iter()
                    .filter(|h| !dead.contains(h))
                    .collect();
                // On the page's own servers (no moving any more), an unreachable one is the end:
                // another look-up would go to it again (live 2026-11-05).
                if !gone.is_empty() && reroute {
                    for h in &gone {
                        note(
                            progress,
                            format!(
                                "The server {h} couldn't be reached; looking the page up again without it"
                            ),
                        );
                    }
                    dead.extend(gone);
                } else if cookies.droppable.get() && crate::youtube::refused_signed_in(&error) {
                    // Refused as a signed-in viewer (#686): the private window's way, once.
                    cookies.without.set(true);
                    note(
                        progress,
                        format!(
                            "{error}; YouTube refused the video with the browser's cookies: looking it up again without them, as a private window would"
                        ),
                    );
                    tracing::info!(
                        "YouTube refused a video's streams with the browser's cookies ({error}); trying without them"
                    );
                } else if servers::format_unavailable(&error) && rerouted && !looked_again {
                    looked_again = true;
                    note(progress, format!("{error}; looking the page up again"));
                } else if servers::format_unavailable(&error)
                    && relaxed_to.is_none()
                    && let Some(sel) = selector_of(&ours).and_then(relaxed)
                {
                    note(
                        progress,
                        "The chosen quality isn't offered by the servers this computer can reach; downloading the nearest quality below it".into(),
                    );
                    ours = with_selector(&ours, &sel);
                    relaxed_to = Some(sel);
                } else if servers::format_unavailable(&error) && rerouted && reroute {
                    note(
                        progress,
                        "No video the servers this computer can reach serve is left; trying the page's own servers".into(),
                    );
                    reroute = false;
                } else if rerouted && !looked_again {
                    looked_again = true;
                    note(progress, format!("{error}; looking the page up again"));
                } else {
                    last = error;
                    break;
                }
                last = error;
            }
        }
    }
    end(child);
    let mut p = progress.lock().unwrap();
    if p.state != State::Stopped {
        p.speed = 0.0;
        p.state = State::Failed(last);
    }
}

/// The download done, saved as `f` (`problems`: the subtitles that didn't come with it): named
/// for what it holds, its size and the subtitles beside it said, shown to `saved` first (#1103).
fn done(
    progress: &Mutex<Progress>,
    saved: Option<&mdm_engine::FileHook>,
    f: PathBuf,
    problems: Vec<SubProblem>,
) {
    // Named for what it holds (#569): HLS pieces joined without ffmpeg.
    let f = named_for_content(f);
    // The saved file's own size: a merged video or a converted MP3 isn't the size of what came
    // down.
    let size = std::fs::metadata(&f).ok().map(|m| m.len());
    let side = side_files(&f);
    // Before Done, with the progress unlocked: a disk slow to take the mark keeps this thread
    // waiting, never a window reading the progress (#1103).
    if let Some(hook) = saved {
        for file in std::iter::once(&f).chain(&side).filter(|p| p.is_file()) {
            (hook.0)(file);
        }
    }
    let mut p = progress.lock().unwrap();
    if p.state != State::Stopped {
        p.speed = 0.0;
        p.side_files = side;
        p.sub_problems = problems;
        if let Some(len) = size {
            p.total = Some(len);
            p.downloaded = len;
        }
        p.state = State::Done(f);
    }
}

/// The most the bar shows while yt-dlp still works: all the bytes in is not the end, another
/// stream or the merge may follow (#344). The window's percentage takes it from here; the sizes
/// stay what they are.
pub const MAX_WHILE_RUNNING: f64 = 0.999;

/// yt-dlp's progress lines (`MDMP <downloaded> <total> <estimate> <speed> <format id>`) made
/// into one steady figure. Each stream of a merged download (video, then audio) reports from 0
/// with its own total, and a fragmented stream's total (HLS, DASH) is an estimate that moves with
/// every fragment: the streams are added up, and the fraction done goes back only when a stream's
/// end had looked like the whole (no `expected`, the next stream starts).
///
/// The size the video window showed (`expected`, every stream) holds the total up until the last
/// stream: a stream's end before it never looks like the whole download (#344). That size is
/// often an estimate (an HLS stream's bitrate, a peak, times its length) well above what comes:
/// in the last stream, once yt-dlp knows that stream's own total, the total is what came before
/// and that stream's, so the bar reaches the end. Which streams there are, yt-dlp says before it
/// downloads ([`Meter::quality`]), and each line names the one it is for ([`Meter::named`]).
///
/// A line named otherwise (no `MDMQ`, an id it didn't name, `NA`) is read as before: back near 0
/// is the next stream, never counted as one of `MDMQ`'s, and the window's size holds the total.
/// Subtitle files come before the video with lines of their own, `NA` (#68): what they brought
/// leaves the bar when the first stream's line comes.
struct Meter {
    expected: Option<u64>,
    /// `MDMQ`'s format id and resolution (`137+140 1920x1080`), once yt-dlp has said.
    quality: Option<String>,
    /// The stream the lines are for now (its format id), how many of `MDMQ`'s streams have
    /// started, and whether a line was for all of them at once (`137+140`).
    current: Option<String>,
    started: usize,
    all_at_once: bool,
    /// Every line so far said `NA` for its format id (subtitle files).
    only_na: bool,
    /// The streams already finished, and the current one's last total and bytes.
    finished: u64,
    last_total: u64,
    last_got: u64,
    /// The highest fraction shown so far.
    frac: f64,
}

impl Meter {
    fn new(expected: Option<u64>) -> Self {
        Meter {
            expected: expected.filter(|e| *e > 0),
            quality: None,
            current: None,
            started: 0,
            all_at_once: false,
            only_na: true,
            finished: 0,
            last_total: 0,
            last_got: 0,
            frac: 0.0,
        }
    }

    /// yt-dlp's `MDMQ <format id> <resolution>` ([`QUALITY`]): a merged choice's id names each
    /// stream (`137+140`).
    fn quality(&mut self, rest: &str) {
        let rest = rest.trim();
        self.quality = (!rest.is_empty()).then(|| rest.to_owned());
    }

    /// How many streams `MDMQ` named.
    fn streams(&self) -> Option<usize> {
        self.quality.as_ref().map(|q| q.split('+').count())
    }

    /// Whether a line's format `id` is one of the streams `MDMQ` named (`137` of `137+140`;
    /// false), or all of them at once (`137+140`, as ffmpeg downloads and merges them; true).
    /// None for any other: none named, `NA`, an id `MDMQ` didn't name, no `MDMQ`.
    fn named<'a>(&self, id: Option<&'a str>) -> Option<(&'a str, bool)> {
        let (q, id) = (self.quality.as_deref()?, id.filter(|id| *id != "NA")?);
        // The id, then the resolution after the last one (one word, or two: `audio only`).
        let is = |s: &str| {
            s.strip_prefix(id)
                .is_some_and(|r| r.is_empty() || r.starts_with(' '))
        };
        if q.split('+').any(is) {
            Some((id, false))
        } else {
            is(q).then_some((id, true))
        }
    }

    /// The next stream started: the one before counts in full.
    fn next_stream(&mut self) {
        self.finished += self.last_total.max(self.last_got);
        self.last_total = 0;
        self.last_got = 0;
        // Without the whole size from the start, the first stream's end looked like the
        // whole download: the next stream's total pulls the bar back (#344).
        if self.expected.is_none() {
            self.frac = 0.0;
        }
    }

    fn line(&mut self, rest: &str, p: &mut Progress) {
        let fields: Vec<&str> = rest.trim().splitn(5, ' ').collect();
        let f: Vec<Option<f64>> = fields.iter().take(4).map(|v| v.parse().ok()).collect();
        let mut got = f.first().copied().flatten().unwrap_or(0.0).max(0.0) as u64;
        let total = f
            .get(1)
            .copied()
            .flatten()
            .or(f.get(2).copied().flatten())
            .map(|t| t.max(0.0) as u64);
        // Only an estimate, and no more than what came: yt-dlp's first line for a stream in
        // pieces, before it knows how many there are (1 KB of 1 KB), or its last one. It says
        // nothing of how much is left, and the bar must not hold on to its end (#622).
        let blind = f.get(1).copied().flatten().is_none() && total.is_some_and(|t| t <= got);
        let id = fields
            .get(4)
            .map(|id| id.trim())
            .filter(|id| !id.is_empty());
        let named = self.named(id);
        match named {
            // Each stream by its own id: a count from 0 again within one (a retry from the
            // start) is not the next stream, and the bar waits there until it is back where it
            // was.
            Some((id, _)) if self.current.as_deref() == Some(id) => {
                got = got.max(self.last_got);
            }
            Some((id, all)) => {
                if self.only_na {
                    // Subtitle files came first (#68): theirs are not the video's bytes (the bar
                    // gives back their share of the whole, a sliver beside a video's).
                    self.finished = 0;
                    self.last_total = 0;
                    self.last_got = 0;
                    self.frac = 0.0;
                } else if self.current.is_some() || self.last_got > 0 {
                    self.next_stream();
                }
                if all {
                    self.all_at_once = true;
                } else {
                    self.started += 1;
                }
                self.current = Some(id.to_owned());
                self.only_na = false;
            }
            // As before: back near 0 is the next stream.
            None => {
                let back = got < self.last_got;
                if back && self.last_got > 0 {
                    self.next_stream();
                }
                self.only_na &= id == Some("NA");
            }
        }
        self.last_got = got;
        if let Some(t) = total {
            self.last_total = t;
        }
        let downloaded = self.finished + got;
        p.downloaded = downloaded;
        p.speed = f.get(3).copied().flatten().unwrap_or(0.0);
        // Neither yt-dlp nor the video window knows how much is coming (a live stream, some
        // HLS): no percentage rather than what came so far as the whole, 100 % (#344).
        if self.last_total == 0 && self.expected.is_none() {
            p.total = None;
            return;
        }
        // Such a line leaves the bar where it was (none yet: no percentage). The stream's bytes
        // still count in full when the next one starts.
        if blind {
            p.total = (self.frac > 0.0)
                .then(|| ((downloaded as f64 / self.frac).round() as u64).max(downloaded + 1));
            return;
        }
        let mut whole = self.finished + self.last_total.max(got);
        // The window's size stands for the streams still to come (less what is known so far);
        // in the last stream, once its own total is known, nothing is left to come. When that
        // size was an estimate above what came, the bar lurches forward there (a merged
        // download's audio, after its video: from about the video's share of the estimate to
        // its share of what came); never back, and it glides there in the progress window.
        let last = named.is_some()
            && (self.all_at_once || self.streams().is_some_and(|n| self.started >= n));
        if let Some(e) = self.expected
            && !(last && self.last_total > 0)
        {
            whole = whole.max(e);
        }
        if whole == 0 {
            p.total = None;
            return;
        }
        let frac = (downloaded as f64 / whole as f64).min(MAX_WHILE_RUNNING);
        // When the estimate grew, the bar stays where it was rather than move back. Otherwise
        // the total is the whole, as big as it is: at the end it isn't inflated to read 99.9 %.
        p.total = Some(if frac >= self.frac {
            self.frac = frac;
            whole
        } else {
            ((downloaded as f64 / self.frac)
                .round()
                .max(whole.min(downloaded + 1) as f64)) as u64
        });
    }
}

/// How one yt-dlp run on the looked-up page ended.
struct Ran {
    status: std::io::Result<std::process::ExitStatus>,
    /// Where it said it saved the file (`MDMF`).
    saved: Option<PathBuf>,
    /// The quality it chose (`MDMQ`).
    quality: Option<String>,
    warnings: Vec<String>,
    /// What it said when it failed.
    failure: String,
    /// The merge's file when yt-dlp said it merges next and never that it was done.
    merging: Option<PathBuf>,
}

/// Run yt-dlp with `ours` on the page looked up into `file`, following its progress with
/// `meter` (a live stream's, by its recording's size). None: stopped before or while it ran, or
/// `recorder` was saving before it started.
#[allow(clippy::too_many_arguments)]
fn run(
    exe: &Path,
    url: &str,
    ours: &[String],
    file: &Path,
    progress: &Mutex<Progress>,
    child: &Child,
    mut meter: Option<&mut Meter>,
    recorder: &Recorder,
    cut: Option<&Cut>,
) -> Result<Option<Ran>, String> {
    // A live stream yt-dlp records: what it says nothing of, its recording's size tells. What is
    // in the folder before yt-dlp starts is not the recorder's (taken before the spawn: the
    // recorder can't have made its file yet, and the watcher's thread may start late; no test
    // checks "before the spawn" itself, the fake can't be made to race it). An earlier run's
    // empty file is removed before this (`set_aside_recordings`), so what is left here is a file
    // that couldn't be removed, or one that holds something.
    let recording = (progress.lock().unwrap().helper == Some(crate::streams::Helper::Live))
        .then(|| template_of(ours))
        .flatten()
        .map(|(dir, stem)| {
            let before = parts_of(&dir, &stem);
            (dir, stem, before)
        });
    let stdout = {
        // Under the child's lock: stop() and save() either came first (nothing starts) or find
        // the child to kill.
        let mut slot = child.lock().unwrap();
        if progress.lock().unwrap().state == State::Stopped || recorder.saving() {
            return Ok(None);
        }
        let spawned = proc::ytdlp_info(exe, &[], ours, file, url).and_then(|mut cmd| {
            // A part read through MDM's server (#1662): ffmpeg goes to 127.0.0.1 straight, never
            // through a proxy yt-dlp hands it (the network adapter's relay, Options' proxy).
            if cut.is_some_and(|c| c.server.is_some()) {
                cmd.env("no_proxy", "127.0.0.1,localhost");
            }
            proc::spawn(cmd, "yt-dlp")
        });
        match spawned {
            Ok(mut s) => {
                let out = s.child.stdout.take().expect("piped");
                *slot = Some(s);
                out
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let mut saved = None;
    let mut quality = None;
    progress.lock().unwrap().merging = None;
    // The merge's file, once yt-dlp says it's next; watched until the reading ends.
    let merged: Mutex<Option<PathBuf>> = Mutex::new(None);
    let read_all = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| watch_merge(&merged, &read_all, progress));
        if let Some((dir, stem, before)) = &recording {
            scope.spawn(|| watch_recording(dir, stem, before, &read_all, progress));
        }
        if let Some(c) = cut {
            scope.spawn(|| watch_cut(c, ours, &read_all, progress));
        }
        // Every line to the end, read as bytes (#79): a line that isn't UTF-8 (a title in a
        // legacy code page) used to end the reading there, while yt-dlp went on; it is read
        // leniently now, and the saved file is also found by its name (`attempt`). The end is
        // yt-dlp's (#1521): what it started may hold its output open long after it ended, and
        // the download waited for that, shown running with its last progress.
        let mut output = proc::Output::read(stdout, MAX_LINE);
        while let Some(buf) = output.next_line(|| proc::ended(child)) {
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end_matches(['\r', '\n']);
            if let Some(rest) = line.strip_prefix("MDMF ") {
                saved = Some(PathBuf::from(rest.trim()));
                // Merged and moved: done joining.
                *merged.lock().unwrap() = None;
                progress.lock().unwrap().merging = None;
            } else if let Some(rest) = line.strip_prefix("MDMQ ") {
                if let Some(m) = meter.as_deref_mut() {
                    m.quality(rest);
                }
                quality = Some(rest.trim().to_owned());
            } else if let Some(rest) = line.strip_prefix("MDMP ")
                && let Some(m) = meter.as_deref_mut()
            {
                m.line(rest, &mut progress.lock().unwrap());
            } else if let Some(rest) = line.strip_prefix("MDMM ")
                && quality.as_deref().is_some_and(several_streams)
            {
                // Several streams: ffmpeg joins them now (#736). Told by this run's `MDMQ`, not
                // by the meter: when MDM's engine downloaded the streams (#769) there is none,
                // and that is how most videos come down.
                *merged.lock().unwrap() = Some(merge_file(Path::new(rest.trim())));
                progress.lock().unwrap().merging = Some(None);
            }
        }
        if output.held_open() {
            proc::note_held_open("yt-dlp");
            note(
                progress,
                "yt-dlp ended, but a program it started kept its output open: not waited for"
                    .into(),
            );
        }
        read_all.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let merging = merged.into_inner().unwrap_or_else(|e| e.into_inner());
    // Poll (the lock is free in between, so stop() can still kill it).
    let status = loop {
        let mut slot = child.lock().unwrap();
        let Some(s) = slot.as_mut() else {
            break Err(std::io::Error::other("yt-dlp is gone"));
        };
        match s.child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        drop(slot);
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut p = progress.lock().unwrap();
        // The run is over: nothing records now, so save() is a Stop from here.
        p.recorded = 0;
        p.recording = false;
        if p.state == State::Stopped {
            return Ok(None);
        }
    }
    let slot = child.lock().unwrap();
    let (warnings, failure) = slot.as_ref().map_or_else(
        || (Vec::new(), "yt-dlp stopped".to_owned()),
        |s| (s.warnings(), s.failure().to_string()),
    );
    Ok(Some(Ran {
        status,
        saved,
        quality,
        warnings,
        failure,
        merging,
    }))
}

/// The file yt-dlp's merge writes for `path` before it renames it there: `<stem>.temp.<ext>`
/// (yt-dlp's `prepend_extension`).
fn merge_file(path: &Path) -> PathBuf {
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => {
            let mut name = stem.to_os_string();
            name.push(".temp.");
            name.push(ext);
            path.with_file_name(name)
        }
        _ => {
            let mut name = path.as_os_str().to_os_string();
            name.push(".temp");
            PathBuf::from(name)
        }
    }
}

/// How far a merge is (#736): the joined file's size against the streams' (ffmpeg copies them
/// as they are, so it ends about as big), read every quarter second while it is `merged`; at
/// most 99 %, the rename is still to come, and never back ([`merge_step`]). Ends when the
/// reading does.
fn watch_merge(
    merged: &Mutex<Option<PathBuf>>,
    read_all: &std::sync::atomic::AtomicBool,
    progress: &Mutex<Progress>,
) {
    while !read_all.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let Some(file) = merged.lock().unwrap().clone() else {
            continue;
        };
        let Ok(len) = std::fs::metadata(&file).map(|m| m.len()) else {
            continue;
        };
        let mut p = progress.lock().unwrap();
        if let Some(was) = p.merging
            && p.downloaded > 0
        {
            p.merging = Some(Some(merge_step(was, merge_fraction(len, p.downloaded))));
        }
    }
}

/// A live stream's recording, yt-dlp's `.part` files in `dir` for `<stem>` ([`parts_of`]; the
/// picture's and the sound's, when it records them apart): their size together into `recorded`
/// every second, and `recording` once the recorder has opened its file: a part that wasn't there
/// before the run (`before`; ffmpeg creates it, 0 bytes, once it has connected, and writes its
/// first block much later) or one that holds something. A 0-byte part that was already there
/// (one an earlier run left and [`remove_empty_parts`] couldn't remove) doesn't count until it
/// grows. Ends when the reading does.
fn watch_recording(
    dir: &Path,
    stem: &str,
    before: &[(PathBuf, u64)],
    read_all: &std::sync::atomic::AtomicBool,
    progress: &Mutex<Progress>,
) {
    let mut next = Instant::now();
    while !read_all.load(std::sync::atomic::Ordering::Relaxed) {
        if Instant::now() >= next {
            next = Instant::now() + Duration::from_secs(1);
            let parts = parts_of(dir, stem);
            if !parts.is_empty() {
                let opened = parts
                    .iter()
                    .any(|(p, len)| *len > 0 || !before.iter().any(|(b, _)| b == p));
                let mut p = progress.lock().unwrap();
                p.recorded = parts.iter().map(|(_, len)| len).sum();
                p.recording |= opened;
            }
        }
        // A tenth of a second at a time: the run's end isn't kept waiting.
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether `MDMQ`'s quality (`137+140 1920x1080`) is a choice of several streams, which
/// yt-dlp's ffmpeg merges.
fn several_streams(quality: &str) -> bool {
    quality
        .split_whitespace()
        .next()
        .is_some_and(|ids| ids.contains('+'))
}

/// A merge's share done: `written` of about `streams` bytes, held short of the end.
fn merge_fraction(written: u64, streams: u64) -> f64 {
    (written as f64 / streams.max(1) as f64).min(0.99)
}

/// The merge's share after a reading of `now`, from `was` (none yet): never back. The file can
/// read smaller than before (empty a moment while it is written again, made again by a second
/// pass, read between ffmpeg's truncating and writing it): the bar holds where it was.
fn merge_step(was: Option<f64>, now: f64) -> f64 {
    was.map_or(now, |was| was.max(now))
}

/// The media file yt-dlp saved under our output name (`<dir>/<stem>.%(ext)s`), when it didn't say
/// where, or said it in a form that names no file (#79). Its subtitles (`<stem>.<lang>.srt`) and
/// unfinished parts don't count; the newest one wins.
fn saved_by_name(ours: &[String]) -> Option<PathBuf> {
    const MEDIA: [&str; 15] = [
        "mp4", "mkv", "webm", "mov", "m4v", "m4a", "mp3", "opus", "ogg", "oga", "flac", "wav",
        "aac", "flv", "3gp",
    ];
    let (dir, stem) = template_of(ours)?;
    MEDIA
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .filter(|p| p.is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// What a failed merge left in `dir` for `<stem>.%(ext)s`: what [`own_files`] tells (unfinished
/// files, streams named by a numbered or `hls-` id), and each stream of format `ids` by its name,
/// `<stem>.f<id>.<ext>`, however yt-dlp named it (`Talk.faud-English.mp4`, an HLS sound's).
fn kept_parts(dir: &Path, stem: &str, ids: &[String]) -> Vec<PathBuf> {
    let mut kept = own_files(dir, stem, false);
    let named: Vec<String> = ids
        .iter()
        .map(|id| format!("{}.f{}.", stem, id).to_lowercase())
        .collect();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let whole = named.iter().any(|n| {
                name.strip_prefix(n.as_str())
                    .is_some_and(|ext| MEDIA.contains(&ext))
            });
            if whole && e.file_type().is_ok_and(|t| t.is_file()) && !kept.contains(&e.path()) {
                kept.push(e.path());
            }
        }
    }
    kept.sort();
    kept
}

/// The folder and the file stem of our output template (`-o <dir>/<stem>.%(ext)s`).
fn template_of(ours: &[String]) -> Option<(PathBuf, String)> {
    let template = ours
        .iter()
        .position(|a| a == "-o")
        .and_then(|i| ours.get(i + 1))?;
    let t = Path::new(template);
    let stem = t
        .file_name()?
        .to_string_lossy()
        .strip_suffix(".%(ext)s")?
        .replace("%%", "%");
    Some((t.parent()?.to_path_buf(), stem))
}

/// What a saved file holds when its name says otherwise: an HLS stream's pieces joined as they
/// came, which only ffmpeg makes into the MP4 they're named for (#569). YouTube's HLS sound is
/// raw AAC (ADTS frames, each piece led by an ID3 tag with its time): `aac`, which players open.
/// A video's pieces are an MPEG transport stream: `ts`. A real MP4 (ffmpeg was there, or the
/// format came as a file) is none of these.
fn content_ext(head: &[u8]) -> Option<&'static str> {
    // ID3v2 tags first: a 10-byte header, then the tag's size in 7-bit bytes.
    let mut at = 0usize;
    while head.len() >= at + 10 && &head[at..at + 3] == b"ID3" {
        let size = head[at + 6..at + 10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(b & 0x7f));
        let footer = if head[at + 5] & 0x10 != 0 { 10 } else { 0 };
        at += 10 + size + footer;
    }
    let rest = head.get(at..)?;
    // ADTS: a 12-bit sync word, then MPEG layer 0.
    if rest.len() >= 7 && rest[0] == 0xff && rest[1] & 0xf6 == 0xf0 {
        return Some("aac");
    }
    // A transport stream: a sync byte every 188 bytes.
    if at == 0 && head.len() > 376 && [0, 188, 376].iter().all(|&i| head[i] == 0x47) {
        return Some("ts");
    }
    None
}

/// `f`, renamed for what it holds when its MP4 name is wrong ([`content_ext`]); as it was
/// otherwise, or when the rename fails (the file is kept).
fn named_for_content(f: PathBuf) -> PathBuf {
    let named = f
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if !matches!(named.as_deref(), Some("mp4" | "m4a")) {
        return f;
    }
    let mut head = Vec::with_capacity(8192);
    let read = std::fs::File::open(&f).and_then(|file| {
        use std::io::Read;
        file.take(8192).read_to_end(&mut head)
    });
    let Some(ext) = read.ok().and_then(|_| content_ext(&head)) else {
        return f;
    };
    let stem = f
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let free = (1..100)
        .map(|n| match n {
            1 => f.with_file_name(format!("{stem}.{ext}")),
            n => f.with_file_name(format!("{stem} ({n}).{ext}")),
        })
        .find(|p| !p.exists());
    match free {
        Some(to) if std::fs::rename(&f, &to).is_ok() => to,
        _ => f,
    }
}

/// Wait `d`, or less if the download is stopped meanwhile (false then).
fn pause(d: std::time::Duration, progress: &Mutex<Progress>) -> bool {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        if progress.lock().unwrap().state == State::Stopped {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    progress.lock().unwrap().state != State::Stopped
}

/// The page as the quality menu's look-up read it (`preloaded`), once yt-dlp has answered there
/// (waiting for it as long as that look-up may take, [`LOOKUP_WAIT`]) and if it offers the choice
/// the menu showed; else none, and the run looks the page up itself.
fn preloaded_page(
    preloaded: &Preloaded,
    progress: &Mutex<Progress>,
) -> Option<(serde_json::Value, bool)> {
    let until = preloaded.slot.born() + LOOKUP_WAIT;
    let stopped = || progress.lock().unwrap().state == State::Stopped;
    let (json, without_cookies) = preloaded.slot.wait(until, stopped)?;
    let info = offered(
        &json,
        &preloaded.selector,
        &preloaded.label,
        preloaded.merge,
    )?;
    Some((info, without_cookies))
}

/// One run: look the page up (or take the quality menu's look-up, `preloaded`), move links off
/// unreachable servers (and off `dead`, the ones an earlier run couldn't reach), download from
/// that list. Subtitles the site refused are asked for once more after a pause (`retry`), beside
/// the video; the ones still refused are reported with it, never failing it.
#[allow(clippy::too_many_arguments)]
fn attempt(
    exe: &Path,
    url: &str,
    ours: &[String],
    browser: &[String],
    progress: &Mutex<Progress>,
    child: &Child,
    expected: Option<u64>,
    retry: Option<&SubsRetry>,
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
    preloaded: Option<&Preloaded>,
    engine: Option<&ByEngine>,
    cookies: &Cookies,
    recorder: &Recorder,
    part: Option<&PartRun>,
) -> Attempt {
    let failed = |error: String, rerouted: bool| Attempt::Failed { error, rerouted };
    cookies.droppable.set(false);
    let page = match preloaded.and_then(|p| preloaded_page(p, progress)) {
        Some(page) => Ok(page),
        // Stopped while waiting for it: nothing more to do.
        None if progress.lock().unwrap().state == State::Stopped => return Attempt::Stopped,
        None => look_up(exe, url, &cookies.args(browser)),
    };
    let mut info = match page {
        Ok((v, without_cookies)) => {
            // The look-up worked only without them (#352, #686). On YouTube, so does the rest of
            // this video's download: the streams it gave are a visitor's.
            if without_cookies && !cookies.without.get() {
                cookies
                    .without
                    .set(crate::youtube::is_youtube(url) && proc::has_cookies(browser));
                note(
                    progress,
                    "The site refused the look-up with the browser's cookies, or offered no format with them; looked the page up without them".into(),
                );
            }
            v
        }
        Err(e) => return failed(e, false),
    };
    // Looked up with the cookies, a YouTube video anyone may watch: refused streams may be the
    // signed-in viewer's (#686). Every run from here takes the arguments as decided.
    cookies.droppable.set(
        !cookies.without.get()
            && proc::has_cookies(browser)
            && crate::youtube::is_youtube(url)
            && !crate::youtube::signed_in_only(&info),
    );
    let (ours, browser) = (&cookies.args(ours)[..], &cookies.args(browser)[..]);
    let changed = moved(&mut info, dead, reroute);
    for (gone, alt) in &changed.swaps {
        note(
            progress,
            format!("The server {gone} can't be reached; downloading from {alt}"),
        );
    }
    if !changed.left_out.is_empty() {
        note(
            progress,
            format!(
                "Not served by the servers this computer can reach, so left out: {}",
                changed.left_out.join(", ")
            ),
        );
    }
    let rerouted = !changed.is_empty();
    let file = info_file();
    if let Err(e) = std::fs::write(&file, info.to_string()) {
        return failed(format!("Cannot write the video information: {e}"), rerouted);
    }
    let _cleanup = Remove(file.clone());
    // A part (#1662): yt-dlp's ffmpeg cuts it, reading through MDM's connections when the
    // engine runs (`partserve`).
    let mut cut = None;
    if let Some(pr) = part {
        match prepare_part(
            exe, url, browser, ours, &mut info, &file, progress, engine, cookies, pr, expected,
            dead, reroute,
        ) {
            Ok(c) => cut = Some(c),
            Err(None) => return Attempt::Stopped,
            Err(Some(e)) => return failed(e, rerouted),
        }
    }
    let with_cut: Vec<String>;
    let ours = match &cut {
        Some(c) => {
            with_cut = [ours, &c.args[..]].concat();
            &with_cut[..]
        }
        None => ours,
    };
    // MDM's engine downloads the streams (#769); yt-dlp then finds them in place.
    let mut by_engine = false;
    if let Some(e) = engine.filter(|_| part.is_none()) {
        // With the run's selector: the nearest quality once the chosen one wasn't offered
        // (#94), or the engine would ask for that one again (#1240).
        let selector = selector_of(ours);
        match e.fetch(
            exe, url, browser, selector, &file, progress, dead, reroute, cookies,
        ) {
            Engined::Ready => by_engine = true,
            Engined::Helper(why) => {
                note(
                    progress,
                    format!("yt-dlp downloads this video itself: {}", why.english()),
                );
                progress.lock().unwrap().helper = Some(why);
            }
            Engined::Stopped => return Attempt::Stopped,
            Engined::Failed(error) => return failed(error, rerouted),
        }
    }
    let mut meter = Meter::new(expected);
    // The streams are in place: what yt-dlp says of them isn't the download's progress. Nor is
    // it a part's: its ffmpeg says nothing until the end, what it reads is the progress.
    let meter = (!by_engine && cut.is_none()).then_some(&mut meter);
    // A live stream yt-dlp records again (Resume, another try) would be recorded over the last
    // recording: kept aside first, or yt-dlp isn't started.
    if progress.lock().unwrap().helper == Some(crate::streams::Helper::Live)
        && !recorder.saving()
        && let Err(e) = set_aside_recordings(ours, progress)
    {
        return Attempt::Blocked(e);
    }
    let ran = match run(
        exe,
        url,
        ours,
        &file,
        progress,
        child,
        meter,
        recorder,
        cut.as_ref(),
    ) {
        Ok(Some(r)) => r,
        Ok(None) if recorder.saving() => return Attempt::Save(None),
        Ok(None) => return Attempt::Stopped,
        Err(e) => return failed(e, rerouted),
    };
    // What MDM's connections met, in its words rather than ffmpeg's (an I/O error).
    let mut ran = ran;
    if let Some(why) = cut.as_ref().and_then(|c| c.server.as_ref()?.failure())
        && !ran.status.as_ref().is_ok_and(|s| s.success())
    {
        ran.failure = format!("The part couldn't be downloaded: {why}");
    }
    // Stopped to be saved: whatever ended the run, its recording is saved as it is, and nothing
    // runs again (no other try, no look-up). Unless yt-dlp finished first, and said so.
    if recorder.saving()
        && !(ran.status.as_ref().is_ok_and(|s| s.success())
            && ran.saved.as_ref().is_some_and(|f| f.is_file()))
    {
        return Attempt::Save(ran.saved);
    }
    let quality = ran.quality.clone();
    let video = match (&ran.status, ran.saved) {
        // `--ignore-errors` (subtitles asked) lets yt-dlp end well with no file: the file
        // itself is the proof.
        (Ok(s), Some(f)) if s.success() && f.is_file() => f,
        // It said where, in a form that names no file (#79), or didn't say: our own name for it.
        (Ok(s), _) if s.success() && saved_by_name(ours).is_some() => {
            saved_by_name(ours).expect("just found")
        }
        (Ok(s), Some(_)) if s.success() => {
            return failed(
                "yt-dlp said it saved the file, but it isn't there".into(),
                rerouted,
            );
        }
        (Ok(s), None) if s.success() => {
            return failed("yt-dlp didn't say where it saved the file".into(), rerouted);
        }
        // Every stream in, and joining them failed (yt-dlp said it merges next, never that it
        // was done): what ffmpeg left half written goes.
        _ if let Some(temp) = &ran.merging => {
            let _ = std::fs::remove_file(temp);
            let ids = ran
                .quality
                .as_deref()
                .and_then(|q| q.split_whitespace().next())
                .map(|ids| ids.split('+').map(str::to_owned).collect())
                .unwrap_or_default();
            return Attempt::MergeFailed(ran.failure, ids);
        }
        _ => return failed(ran.failure, rerouted),
    };
    let refused = subtitle_failures(&ran.warnings);
    if refused.is_empty() {
        return Attempt::Done(video, Vec::new(), quality);
    }
    for (code, why) in &refused {
        note(
            progress,
            format!(
                "Subtitles in {code} couldn't be downloaded: {}",
                english(why)
            ),
        );
    }
    // Once more after a pause, for the ones refused (a site limiting requests often gives them
    // a little later).
    let mut still = refused.clone();
    if let Some(r) = retry {
        let delay = subs_retry_delay();
        note(
            progress,
            format!("Asking for the subtitles again in {} s", delay.as_secs()),
        );
        if !pause(delay, progress) {
            return Attempt::Done(video, Vec::new(), quality);
        }
        let codes: Vec<String> = refused.iter().map(|(c, _)| c.clone()).collect();
        match run(
            exe,
            url,
            &cookies.args(&r.args(&codes)),
            &file,
            progress,
            child,
            None,
            recorder,
            None,
        ) {
            Ok(Some(again)) if again.status.as_ref().is_ok_and(|s| s.success()) => {
                still = subtitle_failures(&again.warnings);
            }
            Ok(None) => return Attempt::Done(video, Vec::new(), quality.clone()),
            _ => {}
        }
    }
    let mut problems = Vec::new();
    for (code, why) in &refused {
        let gone = still.iter().find(|(c, _)| c == code);
        match gone {
            Some((_, why)) => problems.push(SubProblem {
                code: code.clone(),
                name: sub_name(&info, code),
                reason: why.clone(),
                beside: false,
            }),
            None => {
                note(
                    progress,
                    format!("Subtitles in {code} came on the second try"),
                );
                // Asked into the video, they're beside it now.
                if retry.is_some_and(|r| r.embed) {
                    problems.push(SubProblem {
                        code: code.clone(),
                        name: sub_name(&info, code),
                        reason: why.clone(),
                        beside: true,
                    });
                }
            }
        }
    }
    Attempt::Done(video, problems, quality)
}

/// Move `info`'s links off servers that can't be reached (and off `dead`, the ones an earlier run
/// couldn't reach), when `reroute`.
fn moved(
    info: &mut serde_json::Value,
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
) -> servers::Rerouted {
    let checks = *SERVER_CHECKS.read().unwrap_or_else(|e| e.into_inner());
    if let (true, Some((alive, serves))) = (reroute, checks) {
        servers::reroute(info, |h| !dead.contains(h) && alive(h), |l, _| serves(l))
    } else if reroute {
        let prober = servers::Prober::new();
        servers::reroute(
            info,
            |h| !dead.contains(h) && servers::reachable(h),
            |l, h| prober.serves(l, h),
        )
    } else {
        servers::Rerouted::default()
    }
}

/// yt-dlp's choice (`-O "%()j"` after its format selection) on the page looked up into `file`,
/// with `args` (the choice's `-f`, `-o`, merge and conversion, the browser's context).
fn choose(
    exe: &Path,
    url: &str,
    args: &[String],
    file: &Path,
) -> Result<serde_json::Value, String> {
    let mut ours = args.to_vec();
    ours.extend(["-O".into(), CHOICE.into()]);
    let cmd = proc::ytdlp_info(exe, &[], &ours, file, url).map_err(|e| e.to_string())?;
    let out = proc::output(cmd, "yt-dlp", Duration::from_secs(90)).map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out)
        .lines()
        .find_map(|l| l.strip_prefix("MDMJ "))
        .and_then(|j| serde_json::from_str(j).ok())
        .ok_or_else(|| "yt-dlp gave no choice of formats".to_owned())
}

/// yt-dlp's choice as a line of JSON: `MDMJ {…}` (`filename` and the chosen formats in it).
const CHOICE: &str = "MDMJ %()j";

/// A part's download (#1662): the part, how yt-dlp chooses the formats (the download's `-f`,
/// `-o`, merge and conversion, without downloading), and the ffmpeg its exact cut encodes with.
pub(crate) struct PartRun {
    part: crate::part::Part,
    choose: Vec<String>,
    ffmpeg: Option<PathBuf>,
}

/// A run of a part as prepared: what it adds to yt-dlp's arguments (an exact cut's encoders),
/// MDM's server its ffmpeg reads through (with MDM's engine), and its size as far as told.
struct Cut {
    args: Vec<String>,
    server: Option<crate::partserve::PartServer>,
    total: Option<u64>,
}

/// A run of a part on the page looked up into `file` (`info`, its servers as this run moved
/// them): yt-dlp's choice of formats, the exact cut's encoders for what it makes, the part's
/// size (`expected`, the window's estimate, else its share of the chosen formats'), and, with
/// MDM's `engine`, the server each chosen stream is read through (a file over HTTP, an HLS
/// playlist; another kind ffmpeg reads itself), its address in the page in their place. `Err(None)`:
/// stopped meanwhile.
#[allow(clippy::too_many_arguments)]
fn prepare_part(
    exe: &Path,
    url: &str,
    browser: &[String],
    ours: &[String],
    info: &mut serde_json::Value,
    file: &Path,
    progress: &Mutex<Progress>,
    engine: Option<&ByEngine>,
    cookies: &Cookies,
    pr: &PartRun,
    expected: Option<u64>,
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
) -> Result<Cut, Option<String>> {
    // With the run's selector: the nearest quality once the chosen one wasn't offered (#94).
    let args = match selector_of(ours) {
        Some(s) => with_selector(&pr.choose, s),
        None => pr.choose.clone(),
    };
    let choose_args = cookies.args(&args);
    let chosen = choose(exe, url, &choose_args, file).map_err(Some)?;
    if progress.lock().unwrap().state == State::Stopped {
        return Err(None);
    }
    let part = &pr.part;
    let mut args = Vec::new();
    if part.exact
        && let Some(ff) = &pr.ffmpeg
    {
        let made = crate::part::Made::of(&chosen);
        let encode = crate::part::exact_args(&made, &crate::part::encoders(ff));
        if !encode.is_empty() {
            args.extend([
                "--downloader-args".into(),
                crate::part::downloader_args("ffmpeg_o", &encode),
            ]);
        }
    }
    let formats: Vec<&serde_json::Value> = chosen
        .get("requested_formats")
        .and_then(serde_json::Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().collect())
        .unwrap_or_else(|| vec![&chosen]);
    let duration = chosen
        .get("duration")
        .or_else(|| info.get("duration"))
        .and_then(serde_json::Value::as_f64);
    let size_of = |f: &serde_json::Value| {
        let n = |k: &str| {
            f.get(k)
                .and_then(serde_json::Value::as_f64)
                .filter(|n| *n > 0.0)
        };
        n("filesize")
            .or(n("filesize_approx"))
            .or_else(|| Some(n("tbr")? * duration? * 125.0))
            .map(|n| n as u64)
    };
    let whole: Option<u64> = formats.iter().map(|f| size_of(f)).sum();
    let total = expected.or_else(|| part.estimate(whole, duration));
    let server = match engine {
        Some(e) => serve_part(
            e,
            &formats,
            info,
            file,
            (part, duration),
            (exe, url, browser),
            &choose_args,
            dead,
            reroute,
        )
        .map_err(Some)?,
        None => None,
    };
    let hours = crate::part::length(duration).is_some_and(|l| l >= 3600);
    note(
        progress,
        format!(
            "Only the part from {} to {}: {}",
            crate::part::clock(part.from, hours),
            crate::part::clock(part.to, hours),
            if part.exact {
                "an exact cut, encoded again"
            } else {
                "a quick cut, at the keyframes"
            }
        ),
    );
    if server.is_some() {
        note(
            progress,
            "ffmpeg reads the part through MDM's connections".into(),
        );
    }
    let mut p = progress.lock().unwrap();
    p.helper = None;
    p.engine = None;
    p.total = total;
    p.downloaded = 0;
    p.speed = 0.0;
    Ok(Cut {
        args,
        server,
        total,
    })
}

/// MDM's server for a part's chosen `formats` (#1662, [`crate::partserve`]): each one over HTTP
/// (a file, an HLS playlist) is read through it, its address in the page looked up into `file`
/// (`info`) made the server's. None when none can be. `page`: yt-dlp, the page's address and the
/// browser's context, for an address that dies, asked again as the engine's streams are.
#[allow(clippy::too_many_arguments)]
fn serve_part(
    e: &ByEngine,
    formats: &[&serde_json::Value],
    info: &mut serde_json::Value,
    file: &Path,
    (part, duration): (&crate::part::Part, Option<f64>),
    page: (&Path, &str, &[String]),
    choose_args: &[String],
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
) -> Result<Option<crate::partserve::PartServer>, String> {
    let mut sources = Vec::new();
    for f in formats {
        let text = |k: &str| f.get(k).and_then(serde_json::Value::as_str);
        let playlist = matches!(text("protocol"), Some("m3u8" | "m3u8_native"));
        let plain = matches!(text("protocol"), Some("http" | "https"));
        let (Some(id), true) = (text("format_id"), playlist || plain) else {
            continue;
        };
        let Some(s) = crate::streams::stream_of(f) else {
            continue;
        };
        sources.push(crate::partserve::Source {
            url: s.url,
            headers: s.headers,
            size: s.size.filter(|_| s.exact),
            playlist,
            format_id: id.to_owned(),
            ends_near: ends_near(s.size, part, duration),
        });
    }
    if sources.is_empty() {
        return Ok(None);
    }
    let relink: crate::partserve::Relink = {
        let (exe, url, browser) = (page.0.to_owned(), page.1.to_owned(), page.2.to_vec());
        let (choose, dead) = (choose_args.to_vec(), dead.clone());
        Arc::new(move |id: &str| {
            let f = relink(&exe, &url, &browser, &choose, &dead, reroute, id)?;
            let s = crate::streams::stream_of(&f).ok_or("no address for the stream")?;
            Ok((s.url, s.headers))
        })
    };
    let server = crate::partserve::PartServer::start(
        &e.engine.handle,
        e.engine.opts.clone(),
        e.current.limiter(),
        sources.clone(),
        Some(relink),
    )
    .map_err(|err| format!("Cannot start the part's server: {err}"))?;
    for (i, s) in sources.iter().enumerate() {
        let local = serde_json::Value::String(server.url(i));
        let ours = |f: &serde_json::Value| {
            f.get("format_id").and_then(serde_json::Value::as_str) == Some(s.format_id.as_str())
        };
        if let Some(all) = info
            .get_mut("formats")
            .and_then(serde_json::Value::as_array_mut)
        {
            all.iter_mut()
                .filter(|f| ours(f))
                .for_each(|f| f["url"] = local.clone());
        } else if ours(info) {
            info["url"] = local;
        }
    }
    std::fs::write(file, info.to_string())
        .map_err(|err| format!("Cannot write the video information: {err}"))?;
    Ok(Some(server))
}

/// About where `part` ends in a stream of `size` bytes `duration` long (#1662): its share of the
/// stream, and a tenth of the part's size and a MB more, for a bitrate that varies.
fn ends_near(size: Option<u64>, part: &crate::part::Part, duration: Option<f64>) -> Option<u64> {
    let d = duration.filter(|d| *d > 0.0)?;
    let size = size? as f64;
    let end = size * f64::from(part.to) / d;
    let len = size * f64::from(part.seconds()) / d;
    Some((end + len / 10.0) as u64 + (1 << 20))
}

/// A part's progress while its ffmpeg runs (#1662): what MDM's server brought in, or (ffmpeg
/// reading by itself) the size of the file it writes, against the part's size, every quarter
/// second; held short of the whole while it runs. Ends when the reading does.
fn watch_cut(
    c: &Cut,
    ours: &[String],
    read_all: &std::sync::atomic::AtomicBool,
    progress: &Mutex<Progress>,
) {
    let place = template_of(ours);
    let mut last: Option<(Instant, u64)> = None;
    let mut speed = 0.0;
    while !read_all.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        let got = match (&c.server, &place) {
            (Some(s), _) => {
                speed = s.speed();
                s.given()
            }
            (None, Some((dir, stem))) => {
                let got: u64 = parts_of(dir, stem).iter().map(|(_, n)| n).sum();
                let now = Instant::now();
                if let Some((t, was)) = last {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt >= 1.0 {
                        speed = got.saturating_sub(was) as f64 / dt;
                        last = Some((now, got));
                    }
                } else {
                    last = Some((now, got));
                }
                got
            }
            (None, None) => continue,
        };
        let mut p = progress.lock().unwrap();
        p.downloaded = got;
        p.speed = speed;
        p.total = Some(match c.total {
            Some(t) if (got as f64) < t as f64 * MAX_WHILE_RUNNING => t,
            _ => (got as f64 / MAX_WHILE_RUNNING) as u64 + 1,
        });
    }
}

/// MDM's engine for a video's streams (#769): the engine itself, the arguments yt-dlp chooses
/// the formats with (`choose`), and the download's engine part under way.
pub(crate) struct ByEngine {
    engine: crate::streams::Engine,
    choose: Vec<String>,
    current: Arc<crate::streams::Current>,
}

/// How the engine's part of a run ended.
enum Engined {
    /// Every stream is in place for yt-dlp.
    Ready,
    /// yt-dlp downloads this choice itself.
    Helper(crate::streams::Helper),
    Stopped,
    Failed(String),
}

/// The engine's progress into the download's (`Progress`).
struct Reporter<'a>(&'a Mutex<Progress>);

impl crate::streams::Report for Reporter<'_> {
    fn progress(
        &self,
        downloaded: u64,
        total: Option<u64>,
        speed: f64,
        view: crate::streams::View,
    ) {
        let mut p = self.0.lock().unwrap();
        p.downloaded = downloaded;
        p.total = total;
        p.speed = speed;
        p.engine = Some(view);
    }

    fn note(&self, text: String) {
        note(self.0, text);
    }

    fn stopped(&self) -> bool {
        self.0.lock().unwrap().state == State::Stopped
    }
}

impl ByEngine {
    /// Choose the formats on the page looked up into `file` and download their streams with the
    /// engine; a stream's address that dies is asked for again with a new look-up (`url`, as
    /// the browser asked, `browser`; servers moved as this run moved them; with the browser's
    /// cookies or without them, as `cookies` decided). `selector`: the run's own format
    /// selector, when the download moved to another (the nearest quality, #94).
    #[allow(clippy::too_many_arguments)]
    fn fetch(
        &self,
        exe: &Path,
        url: &str,
        browser: &[String],
        selector: Option<&str>,
        file: &Path,
        progress: &Mutex<Progress>,
        dead: &std::collections::BTreeSet<String>,
        reroute: bool,
        cookies: &Cookies,
    ) -> Engined {
        let args = match selector {
            Some(s) => with_selector(&self.choose, s),
            None => self.choose.clone(),
        };
        let choose_args = cookies.args(&args);
        let chosen = match choose(exe, url, &choose_args, file) {
            Ok(c) => c,
            Err(e) => return Engined::Failed(e),
        };
        if progress.lock().unwrap().state == State::Stopped {
            return Engined::Stopped;
        }
        let plan = match crate::streams::plan(&chosen) {
            Ok(p) => p,
            Err(why) => return Engined::Helper(why),
        };
        let Ok(page) = url::Url::parse(url) else {
            return Engined::Failed("not a web address".into());
        };
        {
            let mut p = progress.lock().unwrap();
            p.helper = None;
            p.total = plan.total().or(p.total);
        }
        let relink: crate::streams::Relink = {
            let (exe, url, browser) = (exe.to_owned(), url.to_owned(), browser.to_vec());
            let (choose, dead) = (choose_args, dead.clone());
            Arc::new(move |id: &str| relink(&exe, &url, &browser, &choose, &dead, reroute, id))
        };
        let out = crate::streams::fetch(
            &plan,
            &self.engine,
            &page,
            &relink,
            &self.current,
            &Reporter(progress),
        );
        // The engine's part is over: no connection receives while yt-dlp joins the streams
        // (#736); the last view taken of a stream in pieces still had them under way.
        let mut p = progress.lock().unwrap();
        p.speed = 0.0;
        if let (crate::streams::Fetched::Done, Some(view)) = (&out, &mut p.engine) {
            view.connections
                .iter_mut()
                .for_each(mdm_engine::download::ConnView::finish);
        }
        drop(p);
        match out {
            crate::streams::Fetched::Done => Engined::Ready,
            crate::streams::Fetched::Stopped => Engined::Stopped,
            crate::streams::Fetched::Failed(e) => Engined::Failed(e),
            crate::streams::Fetched::Helper(why) => Engined::Helper(why),
        }
    }
}

/// Format `id` on the page as yt-dlp gives it today (its address, headers, pieces): looked up
/// again (as the browser asked, `browser`), its servers moved as the run moved them, and chosen
/// with `-f <id>`.
fn relink(
    exe: &Path,
    url: &str,
    browser: &[String],
    choose_args: &[String],
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
    id: &str,
) -> Result<serde_json::Value, String> {
    let (mut info, _) = look_up(exe, url, browser)?;
    moved(&mut info, dead, reroute);
    let file = info_file();
    std::fs::write(&file, info.to_string())
        .map_err(|e| format!("Cannot write the video information: {e}"))?;
    let _cleanup = Remove(file.clone());
    let chosen = choose(exe, url, &with_selector(choose_args, id), &file)?;
    crate::streams::format_of(&chosen, id)
        .cloned()
        .ok_or_else(|| format!("the page no longer offers format {id}"))
}

/// A line for the download's log (English, as yt-dlp's own).
fn note(progress: &Mutex<Progress>, text: String) {
    progress
        .lock()
        .unwrap()
        .events
        .push((std::time::SystemTime::now(), text));
}

/// A subtitle refusal in the log's words.
fn english(r: &SubReason) -> String {
    match r {
        SubReason::TooManyRequests => "the site refused (too many requests)".into(),
        SubReason::Other(why) => why.clone(),
    }
}

/// Removes a file when dropped.
struct Remove(PathBuf);

impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The path a failure of yt-dlp's says it couldn't make because it is too long for the system
/// (#1102), from Python's own words: `[WinError 206] The filename or extension is too long`,
/// `[Errno 36] File name too long` (Linux; 63 on macOS), and `[Errno 2] No such file or
/// directory` or `[Errno 22] Invalid argument` for a path past the limits: Windows' `MAX_PATH`
/// while long paths are off makes a path too long look like a missing folder, and a name past
/// 255 like a wrong one. None for anything else (a folder really missing).
pub fn too_long(error: &str) -> Option<PathBuf> {
    const SAID: [(&str, bool); 5] = [
        ("[WinError 206] ", true),
        ("[Errno 36] ", true),
        ("[Errno 63] ", true),
        ("[Errno 2] ", false),
        // Windows: a name past 255 characters.
        ("[Errno 22] ", false),
    ];
    SAID.iter().find_map(|(said, always)| {
        let (_, rest) = error.split_once(said)?;
        let path = python_quoted(rest.split_once(": ")?.1)?;
        (*always || !mdm_engine::naming::fits(&path)).then_some(path)
    })
}

/// The string Python's `repr` quoted at the start of `s` (`'C:\\a'`, or `"He'd"` when it holds
/// a `'`), unescaped.
fn python_quoted(s: &str) -> Option<PathBuf> {
    let mut chars = s.chars();
    let quote = chars.next().filter(|q| matches!(q, '\'' | '"'))?;
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                e @ ('\\' | '\'' | '"') => out.push(e),
                e => out.extend(['\\', e]),
            },
            c if c == quote => return Some(PathBuf::from(out)),
            c => out.push(c),
        }
    }
    None
}

/// `%` would be read as a template field by yt-dlp.
fn template_text(s: &str) -> String {
    s.replace('%', "%%")
}

/// yt-dlp's speed when it downloads by itself (#708; MDM's engine downloads every other video,
/// #769): at most `rate` bytes/s.
pub fn limit_args(rate: Option<u64>) -> Vec<String> {
    rate.map(|r| vec!["--limit-rate".into(), r.to_string()])
        .unwrap_or_default()
}

/// Our progress line: `MDMP <downloaded> <total> <estimate> <speed> <format id>` (`NA` when
/// unknown; the format id is the stream's, `NA` for a subtitle file).
const PROGRESS: &str = "download:MDMP %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(info.format_id)s";
/// The final file: `MDMF <path>`.
const FINAL: &str = "after_move:MDMF %(filepath)s";
/// Every stream is in, the file they make is next (`MDMM <path>`): printed before yt-dlp's
/// post-processing, its merge first (#736).
const MERGE: &str = "post_process:MDMM %(filepath)s";
/// The quality yt-dlp chose, before it downloads: `MDMQ <format id> <resolution>` (the log names
/// it when it isn't the one asked for, #94).
const QUALITY: &str = "video:MDMQ %(format_id)s %(resolution)s";

/// The longest line of yt-dlp's read as one ([`proc::Output`]); its lines are a path at most.
const MAX_LINE: usize = 1024 * 1024;

impl VideoJob {
    /// Download `url` in format `selector` to `dir/<stem>.<ext>`; when yt-dlp downloads it by
    /// itself, at most `rate` bytes/s ([`limit_args`]). `ffmpeg` (its exe) merges separate video and audio into an MP4. `expected`: the size the
    /// choice was shown with (every stream), so the progress starts from the whole. `browser`:
    /// what the browser's request for the page carried (proc::FromBrowser). `subs`: the
    /// subtitles to save with it (#68; none for a sound alone, which has no picture to put them
    /// on). `engine`: MDM's engine, which downloads the streams yt-dlp finds (#769; its options'
    /// speed limit is the download's); without it, or for what it can't download, yt-dlp does.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        exe: &Path,
        url: &str,
        selector: &str,
        dir: &Path,
        stem: &str,
        rate: Option<u64>,
        ffmpeg: Option<&Path>,
        expected: Option<u64>,
        browser: &proc::FromBrowser,
        subs: &Subs,
        engine: Option<crate::streams::Engine>,
    ) -> Result<VideoJob, ProcError> {
        Self::start_with(
            exe, url, selector, dir, stem, rate, ffmpeg, expected, browser, subs, None, engine,
            None, None,
        )
    }

    /// [`VideoJob::start`], the first run taking the quality menu's look-up (`preloaded`) when
    /// it is for this very choice. `saved` is shown the video and the files saved beside it
    /// (subtitles) on the job's own thread before the job says it is done (#1103): the app's
    /// mark that they came from the internet, written before the file is announced, opened or
    /// renamed (a rename keeps the mark), and never on the window's thread. `part`: only that
    /// part of it (#1662, [`crate::part`]), without subtitles or chapters (they'd be the whole
    /// video's, out of step with the part); it needs `ffmpeg`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_with(
        exe: &Path,
        url: &str,
        selector: &str,
        dir: &Path,
        stem: &str,
        rate: Option<u64>,
        ffmpeg: Option<&Path>,
        expected: Option<u64>,
        browser: &proc::FromBrowser,
        subs: &Subs,
        preloaded: Option<Preloaded>,
        engine: Option<crate::streams::Engine>,
        saved: Option<mdm_engine::FileHook>,
        part: Option<crate::part::Part>,
    ) -> Result<VideoJob, ProcError> {
        proc::safe(url)?;
        let preloaded = preloaded.filter(|p| p.selector == selector);
        // A full path: yt-dlp runs in a folder of its own (#834).
        let folder = crate::workdir::absolute(dir);
        // Every file of the video within the path limits (#1102), or the download says so now:
        // yt-dlp would only say `[Errno 2] No such file or directory` once it has the page.
        if !mdm_engine::naming::stem_fits(&folder, stem) {
            return Err(ProcError::PathTooLong(folder.join(stem)));
        }
        let output = folder.join(format!("{}.%(ext)s", template_text(stem)));
        // An MP3, Opus or FLAC choice (#32, #70): the best sound, converted by ffmpeg.
        let convert = crate::resolve::audio_conversion(selector);
        let format = if convert.is_some() {
            "ba/b".to_owned()
        } else if ffmpeg.is_none() {
            // A format that comes as a file first (#569): without ffmpeg an HLS stream stays
            // its raw pieces.
            crate::resolve::without_remuxing(selector)
        } else {
            selector.to_owned()
        };
        // What comes down is the sound as the site has it, not the file the choice's size
        // estimated: the progress follows yt-dlp's own total.
        let expected = if convert.is_some() { None } else { expected };
        // Subtitles go with a picture only (none for a sound), and with the whole video only: a
        // part's would be the whole's, their times out of step with it (#1662).
        let with_subs = convert.is_none() && !subs.langs.is_empty() && part.is_none();
        // How yt-dlp chooses the formats for the engine (#769): the same choice, merge and
        // conversion as the download's run, without downloading.
        let mut choose: Vec<String> = vec![
            "-f".into(),
            format.clone(),
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
        ];
        let mut ours: Vec<String> = vec![
            "-f".into(),
            format,
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
            "--newline".into(),
            // `--print` makes yt-dlp quiet; this keeps the progress lines.
            "--progress".into(),
            "--continue".into(),
            "--no-mtime".into(),
            "--progress-template".into(),
            PROGRESS.into(),
            "--print".into(),
            FINAL.into(),
            "--print".into(),
            QUALITY.into(),
            "--print".into(),
            MERGE.into(),
        ];
        ours.extend(limit_args(rate));
        // A subtitles-only second run, for the ones a site refuses (#68): the same output name.
        let mut retry_base: Vec<String> = vec![
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
        ];
        let converted = convert.is_some();
        match convert {
            Some(a) => {
                let x = ["-x".into(), "--audio-format".into(), a.format.into()];
                ours.extend(x.clone());
                choose.extend(x);
                if let Some(q) = a.quality {
                    ours.extend(["--audio-quality".into(), q]);
                }
            }
            None if part.is_none() => ours.extend(subtitle_args(subs, ffmpeg.is_some())),
            None => {}
        }
        // A part's chapters would be the whole video's, at the whole's times (yt-dlp doesn't
        // move them): none.
        ours.extend(chapter_args(
            CHAPTERS.load(std::sync::atomic::Ordering::Relaxed) && part.is_none(),
            ffmpeg.is_some(),
            converted,
        ));
        // Only the part (#1662): yt-dlp's ffmpeg seeks to it in each stream and reads only it;
        // an exact cut has it encode the part again (its encoders are chosen with the formats,
        // `prepare_part`).
        if let Some(p) = &part {
            ours.extend(["--download-sections".into(), p.sections()]);
            if p.exact {
                ours.push("--force-keyframes-at-cuts".into());
            }
        }
        let (theirs, cookies) = browser.args(url)?;
        retry_base.extend(theirs.iter().cloned());
        ours.extend(theirs.iter().cloned());
        choose.extend(theirs.iter().cloned());
        if let Some(dir) = ffmpeg.and_then(Path::parent) {
            let merge = [
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
                "--merge-output-format".into(),
                "mp4".into(),
            ];
            ours.extend(merge.clone());
            choose.extend(merge);
            retry_base.extend([
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
            ]);
        }
        // With the engine, every HLS stream is yt-dlp's hlsnative's (`--hls-prefer-native`): the
        // engine joins its pieces as hlsnative does, and yt-dlp's fixups after it then follow
        // (a live one hlsnative hands to ffmpeg, as before).
        if engine.is_some() {
            ours.push("--hls-prefer-native".into());
            choose.push("--hls-prefer-native".into());
        }
        let retry = with_subs.then(|| SubsRetry {
            automatic: subs.automatic,
            embed: subs.embed && ffmpeg.is_some(),
            ffmpeg: ffmpeg.is_some(),
            base: retry_base,
        });
        for a in retry.iter().flat_map(|r| r.base.iter()) {
            proc::safe(a)?;
        }
        for a in ours.iter().chain(&choose) {
            proc::safe(a)?;
        }
        // The engine's streams are there before yt-dlp's run: it fixes them up all the same
        // (`--fixup force`; by default only what it downloaded itself), as after its own
        // download: a DASH M4A sound, HLS pieces' MPEG-TS in an MP4. A part is yt-dlp's own
        // download (its ffmpeg's), fixed up as yt-dlp fixes its own.
        if engine.is_some() && part.is_none() {
            ours.extend(["--fixup".into(), "force".into()]);
        }
        // Whose cookies yt-dlp has, for the log (#229): the browser's name only, never a
        // cookie or the profile (a folder may name the person).
        let events = match proc::browser_cookies() {
            Some(b) if !browser.private && cookies.is_none() => vec![(
                std::time::SystemTime::now(),
                format!(
                    "Cookies read from {} on this computer (Options → Sites Logins)",
                    b.browser
                ),
            )],
            _ => Vec::new(),
        };
        let progress = Arc::new(Mutex::new(Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events,
            side_files: Vec::new(),
            sub_problems: Vec::new(),
            engine: None,
            helper: None,
            merging: None,
            merge_failed: None,
            recorded: 0,
            recording: false,
        }));
        let recorder = Arc::new(Recorder {
            saving: std::sync::atomic::AtomicBool::new(false),
            ffmpeg: ffmpeg.map(Path::to_owned),
        });
        let child: Child = Arc::new(Mutex::new(None));
        let current = Arc::new(crate::streams::Current::new(
            engine.as_ref().and_then(|e| e.opts.speed_limit),
        ));
        let part = part.map(|part| PartRun {
            part,
            choose: choose.clone(),
            ffmpeg: ffmpeg.map(Path::to_owned),
        });
        let by_engine = engine.map(|engine| ByEngine {
            engine,
            choose,
            current: current.clone(),
        });
        // MDM's own yt-dlp, ffmpeg and JavaScript runtime, held for the whole download (#1544): none
        // is switched to a new version (nor its old Deno removed, #1570) between its runs.
        let runtime = crate::tools_dir()
            .as_deref()
            .and_then(crate::jsruntime::own)
            .map(|r| r.exe);
        let hold = crate::autoupdate::Hold::of(
            [Some(exe), ffmpeg, runtime.as_deref()]
                .into_iter()
                .flatten(),
        );
        {
            let (progress, child, recorder) = (progress.clone(), child.clone(), recorder.clone());
            let (exe, url) = (exe.to_owned(), url.to_owned());
            std::thread::spawn(move || {
                // The cookie file lives as long as the run, and so do the helpers held.
                let _cookies = cookies;
                let _hold = hold;
                // A panic in here must not leave the download "running" forever.
                let run = std::panic::AssertUnwindSafe(|| {
                    supervise(
                        &exe,
                        &url,
                        &ours,
                        &theirs,
                        &progress,
                        &child,
                        expected,
                        retry.as_ref(),
                        preloaded,
                        by_engine.as_ref(),
                        saved.as_ref(),
                        &recorder,
                        part.as_ref(),
                    )
                });
                if std::panic::catch_unwind(run).is_err() {
                    // Given up: nothing may go on downloading unseen (#79).
                    end(&child);
                    if let Some(b) = &by_engine {
                        b.current.cancel();
                    }
                    let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                    if p.state == State::Running {
                        p.speed = 0.0;
                        p.state =
                            State::Failed("the video download stopped on an internal error".into());
                    }
                }
            });
        }
        Ok(VideoJob {
            child,
            progress,
            current,
            recorder,
        })
    }

    /// Its progress. Stopped only once the engine's download has saved where it stopped: a
    /// start again before then would find the stream still held.
    pub fn progress(&self) -> Progress {
        let mut p = self.progress.lock().unwrap().clone();
        if p.state == State::Stopped && self.current.busy() {
            p.state = State::Running;
            p.speed = 0.0;
        }
        p
    }

    /// Stop (the partial file stays; starting again continues it). Nothing while a live
    /// stream's recording is being saved ([`VideoJob::save`]): it ends saved.
    pub fn stop(&self) {
        {
            let mut p = self.progress.lock().unwrap();
            if self.recorder.saving() {
                return;
            }
            p.state = State::Stopped;
        }
        self.current.cancel();
        if let Some(s) = self.child.lock().unwrap().as_mut() {
            s.kill();
        }
    }

    /// A new speed limit (bytes/s): the engine's download takes it at once (#769). yt-dlp
    /// downloading by itself took its limit as it started.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        self.current.set_limit(rate);
    }

    pub fn speed_limit(&self) -> Option<u64> {
        self.current.limit()
    }

    /// Why yt-dlp downloads this one itself (#769: a live stream, RTMP…), as far as known: a new
    /// speed limit then takes starting it again.
    pub fn helper(&self) -> Option<crate::streams::Helper> {
        self.progress.lock().unwrap().helper.clone()
    }

    /// Stop and save: a live stream yt-dlp records (its recorder has opened its file) is stopped
    /// and its recording kept as a video, the download then done ([`finish_recording`]); yt-dlp
    /// never starts again for it. Before the recorder's first block is written there is nothing
    /// to keep: it ends stopped, with the empty file left as it is, to resume. Anything else:
    /// [`VideoJob::stop`]. Again while saving: nothing more.
    pub fn save(&self) {
        {
            // Under the child's lock, as a run starts: it is recording, or it is a Stop.
            let mut slot = self.child.lock().unwrap();
            let p = self.progress.lock().unwrap();
            if self.recorder.saving() {
                return;
            }
            let recording = slot.is_some() && p.state == State::Running && is_recording(&p);
            if recording {
                self.recorder
                    .saving
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                drop(p);
                // Ended as a Stop ends it (yt-dlp, its ffmpeg): the state stays Running until
                // the recording is saved.
                if let Some(s) = slot.as_mut() {
                    s.kill();
                }
                return;
            }
        }
        self.stop();
    }

    /// A live stream's download: recording (the recorder has opened its file), being saved, or
    /// neither (`None`: not a live stream, not started recording, or over).
    pub fn live_state(&self) -> Option<LiveState> {
        let p = self.progress.lock().unwrap();
        if p.state != State::Running {
            None
        } else if self.recorder.saving() {
            Some(LiveState::Saving)
        } else if is_recording(&p) {
            Some(LiveState::Recording)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A video download's files that "overwrite" (a duplicate added again) removes: exactly what
    /// yt-dlp leaves for its stem, the finished file and subtitles only with `outputs`; never a
    /// file merely named alike, another stem's, or MDM's own part files.
    #[test]
    fn a_videos_own_files_are_exactly_yt_dlps() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let working = [
            "Clip.mp4.part",
            "Clip.mp4.ytdl",
            "Clip.mp4.part-Frag4",
            "Clip.mp4.part-Frag4.part",
            "Clip.f137.mp4",
            "Clip.f137.mp4.part",
            "Clip.f137.mp4.ytdl",
            "Clip.f137.mp4.part-Frag12",
            "Clip.f137.mp4.part-Frag12.part",
            "Clip.f251.webm.part",
            "Clip.fhls-720p.mp4",
            "Clip.fdash-1.m4a.part",
            "Clip.fhttp-480p.mp4",
            "clip.temp.mp4",
            "Clip.f137.mp4.mdm.part",
            "Clip.f137.mp4.mdm.json",
            "Clip.f251.webm.mdm.part",
        ];
        let finished = [
            "Clip.mp4",
            "Clip.en.srt",
            "Clip.pt-BR.vtt",
            "Clip.fr.srt",
            "Clip.fi.vtt",
        ];
        let others = [
            "Clip.final.mp4",
            "Clip.full.mkv",
            "Clip.f1.jpg",
            "Clip.fhls-video.mp4",
            "Clip.v2.mp4.part",
            "Clip.pdf.mdm.part",
            "Clip.v2.zip.mdm.part",
            "Clip.mp4.mdm.part",
            "Clip.f137.mp4.mdm.zip",
            "Clip.f1.jpg.mdm.part",
            "Clip.zip",
            "Clip.notes.txt",
            "Clip.final cut.mp4",
            "Clip.mp4.part-Fragment",
            "Clip (1).f137.mp4",
            "Clip (1).f137.mp4.part",
            "Clip (1).en.srt",
            "Clipper.mp4.part",
            "Other.en.srt",
        ];
        for f in working.iter().chain(&finished).chain(&others) {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let names = |outputs| {
            let mut n: Vec<String> = own_files(d, "Clip", outputs)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            n.sort();
            n
        };
        let mut want: Vec<String> = working.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(
            names(false),
            want,
            "subtitles (fr.srt, fi.vtt) are never working files"
        );
        want.extend(finished.iter().map(|s| s.to_string()));
        want.sort();
        assert_eq!(names(true), want);
        assert!(own_files(&d.join("missing"), "Clip", true).is_empty());
    }
    /// #736: the merge's own file, which yt-dlp renames when it's done, and its share done.
    #[test]
    fn a_merge_is_followed_by_its_file() {
        assert_eq!(
            merge_file(Path::new("/v/Big Buck Bunny.mp4")),
            PathBuf::from("/v/Big Buck Bunny.temp.mp4")
        );
        assert_eq!(
            merge_file(Path::new("/v/clip")),
            PathBuf::from("/v/clip.temp")
        );
        assert_eq!(merge_fraction(45, 100), 0.45);
        // About as big as the streams, never "done" before the rename.
        assert_eq!(merge_fraction(120, 100), 0.99);
        // A smaller file read later (truncated and written again): the share holds, then goes on.
        assert_eq!(merge_step(None, 0.4), 0.4);
        assert_eq!(merge_step(Some(0.4), 0.0), 0.4);
        assert_eq!(merge_step(Some(0.4), 0.45), 0.45);
        // A merge is a choice of several streams, as `MDMQ` names them.
        assert!(several_streams("401+140 3840x2160"));
        assert!(!several_streams("18 640x360"));
        assert!(!several_streams("140 audio only"));
    }

    /// #569: what a file joined from HLS pieces holds, from its first bytes. YouTube's sound:
    /// pieces each led by an ID3 tag (Apple's timestamp), then ADTS frames.
    #[test]
    fn joined_hls_pieces_are_named_for_what_they_hold() {
        let mut aac = b"ID3     ?".to_vec();
        aac.extend(std::iter::repeat_n(0u8, 0x3f));
        aac.extend([0xff, 0xf1, 0x50, 0x80, 0x2e, 0x7f, 0xfc]);
        assert_eq!(content_ext(&aac), Some("aac"));
        // ADTS with no tag, and MPEG-1 layer 3 (an MP3: not ours to rename).
        assert_eq!(content_ext(&[0xff, 0xf9, 0x50, 0x80, 0, 0, 0]), Some("aac"));
        assert_eq!(content_ext(&[0xff, 0xfb, 0x90, 0x64, 0, 0, 0]), None);
        let mut ts = vec![0u8; 400];
        for i in [0, 188, 376] {
            ts[i] = 0x47;
        }
        assert_eq!(content_ext(&ts), Some("ts"));
        // A real MP4 (ffmpeg was there): as it is.
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend(b"ftypisom");
        mp4.resize(400, 0);
        assert_eq!(content_ext(&mp4), None);

        let dir = tempfile::tempdir().unwrap();
        let joined = dir.path().join("Talk.mp4");
        std::fs::write(&joined, &aac).unwrap();
        assert_eq!(named_for_content(joined), dir.path().join("Talk.aac"));
        assert!(dir.path().join("Talk.aac").is_file());
        // Another Talk.aac there already: not overwritten.
        std::fs::write(dir.path().join("Talk.mp4"), &aac).unwrap();
        let second = named_for_content(dir.path().join("Talk.mp4"));
        assert_eq!(second, dir.path().join("Talk (2).aac"));
        // A real MP4 keeps its name; so does a sound already named .aac.
        let real = dir.path().join("Real.mp4");
        std::fs::write(&real, &mp4).unwrap();
        assert_eq!(named_for_content(real.clone()), real);
        let named = dir.path().join("Talk.aac");
        assert_eq!(named_for_content(named.clone()), named);
    }

    #[test]
    fn a_limit_for_yt_dlp_downloading_by_itself() {
        let s = |v: Vec<String>| v.join(" ");
        assert_eq!(s(limit_args(None)), "");
        assert_eq!(s(limit_args(Some(524_288))), "--limit-rate 524288");
    }

    /// Live (network, yt-dlp): `MDM_LIVE_YTDLP=<yt-dlp.exe> MDM_LIVE_PAGE=<video page>
    /// cargo test -p mdm-tools live_download -- --ignored --nocapture`. Downloads the smallest
    /// audio and prints the log (a swapped server shows there).
    #[test]
    #[ignore]
    fn live_download() {
        let exe = PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let page = std::env::var("MDM_LIVE_PAGE").expect("MDM_LIVE_PAGE");
        let dir = tempfile::tempdir().unwrap();
        let format = std::env::var("MDM_LIVE_FORMAT").unwrap_or_else(|_| "worstaudio".into());
        let ffmpeg = std::env::var("MDM_LIVE_FFMPEG").ok().map(PathBuf::from);
        // MDM's engine downloads the streams (#769), unless MDM_LIVE_YTDLP_ALONE is set.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine =
            std::env::var_os("MDM_LIVE_YTDLP_ALONE")
                .is_none()
                .then(|| crate::streams::Engine {
                    handle: rt.handle().clone(),
                    opts: mdm_engine::Options::default(),
                });
        let job = VideoJob::start(
            &exe,
            &page,
            &format,
            dir.path(),
            "a",
            None,
            ffmpeg.as_deref(),
            None,
            &Default::default(),
            &Default::default(),
            engine,
        )
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(180);
        // When the bytes started and stopped coming: the transfer's own speed.
        let (mut first, mut last, mut most) = (None, None, 0u64);
        let p = loop {
            let p = job.progress();
            if p.downloaded > most {
                most = p.downloaded;
                first.get_or_insert_with(std::time::Instant::now);
                last = Some(std::time::Instant::now());
            }
            if p.state != State::Running || std::time::Instant::now() > end {
                break p;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        for (_, e) in &p.events {
            println!("event: {e}");
        }
        println!("state: {:?}, {} bytes", p.state, p.downloaded);
        if let (Some(a), Some(b)) = (first, last) {
            let secs = (b - a).as_secs_f64().max(0.001);
            println!(
                "transfer: {most} bytes in {secs:.1} s, {:.2} MB/s",
                most as f64 / secs / 1e6
            );
        }
        assert!(
            matches!(p.state, State::Done(ref f) if f.exists()),
            "{:?}",
            p.state
        );
    }

    /// Live (#61): `MDM_LIVE_YTDLP=<yt-dlp.exe> MDM_LIVE_SLOW=<a slow file's address>
    /// cargo test -p mdm-tools live_stop -- --ignored --nocapture` (the lab server:
    /// `/gen/20M/clip.mp4?rate=50K`). Stopping a video download ends every yt-dlp process it
    /// started (Windows: the launcher and its child), and the part file stops growing.
    #[test]
    #[ignore]
    fn live_stop_ends_every_process() {
        let exe = PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let url = std::env::var("MDM_LIVE_SLOW").expect("MDM_LIVE_SLOW");
        let dir = tempfile::tempdir().unwrap();
        let job = VideoJob::start(
            &exe,
            &url,
            "b",
            dir.path(),
            "clip",
            None,
            None,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap();
        let size = || {
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum::<u64>()
        };
        std::thread::sleep(std::time::Duration::from_secs(8));
        println!("before stop: {} bytes, {:?}", size(), job.progress().state);
        job.stop();
        std::thread::sleep(std::time::Duration::from_secs(3));
        let stopped = size();
        std::thread::sleep(std::time::Duration::from_secs(4));
        println!("after stop: {stopped} then {} bytes", size());
        assert_eq!(size(), stopped, "nothing downloads after Stop");
        // Started again: it continues the same part file (nothing else holds it now).
        let again = VideoJob::start(
            &exe,
            &url,
            "b",
            dir.path(),
            "clip",
            None,
            None,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(8));
        // yt-dlp's own count (a file open in another process shows a stale size on Windows).
        let p = again.progress();
        println!("resumed: yt-dlp at {} bytes, {:?}", p.downloaded, p.state);
        assert!(p.downloaded > stopped, "it went on from where it stopped");
        assert!(
            !matches!(p.state, State::Failed(_)),
            "resumed without an error: {:?}",
            p.state
        );
        again.stop();
    }

    fn run(expected: Option<u64>, lines: &[&str]) -> Vec<(u64, Option<u64>)> {
        let mut m = Meter::new(expected);
        let mut p = Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events: Vec::new(),
            side_files: Vec::new(),
            sub_problems: Vec::new(),
            engine: None,
            helper: None,
            merging: None,
            merge_failed: None,
            recorded: 0,
            recording: false,
        };
        // `MDMQ …` as yt-dlp prints it before it downloads; every other line is an `MDMP` one's.
        lines
            .iter()
            .filter_map(|l| match l.strip_prefix("MDMQ ") {
                Some(q) => {
                    m.quality(q);
                    None
                }
                None => {
                    m.line(l, &mut p);
                    Some((p.downloaded, p.total))
                }
            })
            .collect()
    }

    /// The bar as the progress window draws it: held short of the end while yt-dlp works
    /// (`Item::fraction` takes [`MAX_WHILE_RUNNING`]; the total itself is the real size).
    fn fractions(v: &[(u64, Option<u64>)]) -> Vec<f64> {
        v.iter()
            .map(|(d, t)| (*d as f64 / t.unwrap() as f64).min(MAX_WHILE_RUNNING))
            .collect()
    }

    /// A fragmented 1080p stream whose estimate grows, then the separate audio from 0: the bar
    /// only moves forward, and ends at the whole.
    #[test]
    fn video_then_audio_never_moves_back() {
        let lines = [
            "100 NA 1000 5",
            "300 NA 1300 5",
            "600 NA 1500 5",
            "1400 NA 1600 5",
            "1600 1600 NA 5",
            "50 200 NA 5",
            "150 200 NA 5",
            "200 200 NA 5",
        ];
        let f = fractions(&run(Some(1800), &lines));
        for w in f.windows(2) {
            assert!(w[1] >= w[0] - 1e-9, "went back: {f:?}");
        }
        let last = run(Some(1800), &lines).pop().unwrap();
        assert_eq!(last, (1800, Some(1800)));
        assert!(f[f.len() - 1] > 0.99, "{f:?}");
    }

    /// The file size stays the file's size at the end: holding the bar short of 100 % is the
    /// window's (`Item::fraction`), not a total made 0.1 % too big (100.100 MB for 100 MB).
    #[test]
    fn the_size_is_not_inflated_at_the_end() {
        let m = 1 << 20;
        let line = format!("{} {} NA 900000", 100 * m, 100 * m);
        let v = run(Some(100 * m), &[line.as_str()]);
        assert_eq!(v, vec![(100 * m, Some(100 * m))]);
    }

    /// #344: with no size known from the start, the second stream brings the bar back from
    /// where the first one's end had taken it, and it never says 100 % before the end.
    #[test]
    fn a_stream_ending_is_not_the_whole_download() {
        let lines = [
            "100 1000 NA 5",
            "1000 1000 NA 5",
            "50 500 NA 5",
            "400 500 NA 5",
        ];
        let f = fractions(&run(None, &lines));
        assert!(f[2] < 0.75, "the first stream's 100 % stayed: {f:?}");
        assert!(f[3] > f[2], "{f:?}");
    }

    /// #344: no total and no estimate for a stream, nothing said by the video window: no
    /// percentage at all, and certainly not 100 %.
    #[test]
    fn nothing_known_is_no_percentage() {
        let v = run(None, &["100 NA NA 5", "9000 NA NA 5"]);
        assert_eq!(v, vec![(100, None), (9000, None)]);
    }

    /// The size the window showed counts both streams from the start: the first stream alone
    /// never looks like the whole download, whether or not yt-dlp has named the streams.
    #[test]
    fn expected_size_holds_the_bar_back() {
        let v = run(Some(1000), &["400 400 NA 0"]);
        assert_eq!(v[0], (400, Some(1000)));
        let v = run(Some(1000), &["MDMQ 137+140 1920x1080", "400 400 NA 0 137"]);
        assert_eq!(v[0], (400, Some(1000)));
    }

    /// A line with no figure at all: no percentage.
    #[test]
    fn a_line_of_nothing_is_no_percentage() {
        let v = run(None, &["NA NA NA NA"]);
        assert_eq!(v[0], (0, None));
    }

    /// Never back, but for the total's rounding to a whole byte (these sizes are tiny).
    fn assert_forward(f: &[f64]) {
        for w in f.windows(2) {
            assert!(w[1] >= w[0] - 1e-3, "went back: {f:?}");
        }
    }

    /// The bar at its end (`v`, the meter's lines). Drawn, it is held short of the end
    /// ([`fractions`]); the meter's own figures reach the whole at the last line only, and never
    /// go past it: 100 % is never said before the end.
    fn assert_ends(v: &[(u64, Option<u64>)]) {
        let f = fractions(v);
        assert!(f[f.len() - 1] >= MAX_WHILE_RUNNING - 1e-9, "{f:?}");
        let raw: Vec<f64> = v
            .iter()
            .map(|(d, t)| *d as f64 / t.unwrap() as f64)
            .collect();
        assert!(raw.iter().all(|x| *x <= 1.0), "past 100 %: {raw:?}");
        assert!(
            raw[..raw.len() - 1].iter().all(|x| *x < 1.0),
            "100 % before the end: {raw:?}"
        );
    }

    /// An HLS stream's lines, as yt-dlp prints them for format `id` (none: lines without one,
    /// as before it was asked for): from nothing, then its estimate after each of `frags`
    /// fragments of `frag` bytes, then its own total at its end.
    fn hls_stream(id: &str, frags: u64, frag: u64) -> Vec<String> {
        let whole = frags * frag;
        let mut out = vec!["0 NA NA 0".to_owned()];
        out.extend((1..frags).map(|k| format!("{} NA {whole} 5", k * frag)));
        out.push(format!("{whole} {whole} NA 5"));
        out.into_iter()
            .map(|l| format!("{l} {id}").trim_end().to_owned())
            .collect()
    }

    fn lines<'a>(quality: Option<&'a str>, streams: &'a [Vec<String>]) -> Vec<&'a str> {
        quality
            .into_iter()
            .chain(streams.iter().flatten().map(String::as_str))
            .collect()
    }

    /// The figures as before yt-dlp named its streams: the same lines without `MDMQ` and without
    /// their format ids.
    fn as_before(expected: Option<u64>, streams: &[Vec<String>]) -> Vec<(u64, Option<u64>)> {
        let bare: Vec<Vec<String>> = streams
            .iter()
            .map(|s| {
                s.iter()
                    .map(|l| l.splitn(5, ' ').take(4).collect::<Vec<_>>().join(" "))
                    .collect()
            })
            .collect();
        run(expected, &lines(None, &bare))
    }

    /// The video window's size an estimate twice what comes (an HLS stream's BANDWIDTH is its
    /// peak): with yt-dlp's word that the choice is one stream, the bar ends at the end, not
    /// halfway; never back. Without that word, as before: the window's size holds it.
    #[test]
    fn an_estimate_above_the_size_still_ends_at_the_end() {
        let stream = [hls_stream("hls-6400", 10, 100)];
        let v = run(Some(2000), &lines(Some("MDMQ hls-6400 1280x720"), &stream));
        assert_forward(&fractions(&v));
        assert_ends(&v);
        let f = fractions(&as_before(Some(2000), &stream));
        assert!((f[f.len() - 1] - 0.5).abs() < 1e-9, "{f:?}");
    }

    /// The same for a merged choice (video, then audio): the bar ends at the end, and the
    /// video's end is still not the whole download, nor does the bar go back (#344).
    #[test]
    fn a_merged_estimate_above_the_size_still_ends_at_the_end() {
        let streams = [
            hls_stream("hls-7040", 16, 100),
            hls_stream("hls-aud-English", 4, 100),
        ];
        let v = run(
            Some(4000),
            &lines(Some("MDMQ hls-7040+hls-aud-English 1280x720"), &streams),
        );
        let f = fractions(&v);
        assert_forward(&f);
        let video_end = streams[0].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// One line for all the streams at once (`137+140`: yt-dlp's ffmpeg downloader fetches and
    /// merges them together, and says so once, at the end): that is the whole download, and its
    /// own total its end.
    #[test]
    fn a_line_for_every_stream_at_once_is_the_whole_download() {
        let q = "MDMQ 7040+aud-English 1280x720";
        let all = [hls_stream("7040+aud-English", 20, 100)];
        let v = run(Some(4000), &lines(Some(q), &all));
        assert_forward(&fractions(&v));
        assert_ends(&v);
        let v = run(Some(4000), &[q, "2000 2000 NA 2129866.9 7040+aud-English"]);
        assert_ends(&v);
    }

    /// Lines whose format id `MDMQ` didn't name, or `NA` on every line (a yt-dlp that names no
    /// stream): read exactly as before, never left out (the bar never stands still at 0).
    #[test]
    fn lines_not_named_by_mdmq_are_read_as_before() {
        for (expected, ids) in [
            (Some(4000), ["999", "998"]),
            (Some(4000), ["NA", "NA"]),
            (Some(2000), ["NA", "NA"]),
            (None, ["NA", "NA"]),
            (None, ["999", "998"]),
        ] {
            let streams = [hls_stream(ids[0], 16, 100), hls_stream(ids[1], 4, 100)];
            let v = run(expected, &lines(Some("MDMQ 137+140 1920x1080"), &streams));
            assert_eq!(v, as_before(expected, &streams), "{expected:?} {ids:?}");
            assert_eq!(v[v.len() - 1].0, 2000);
            if expected != Some(4000) {
                assert_ends(&v[v.len() - 1..]);
            }
        }
    }

    /// Subtitles asked for (#68): yt-dlp downloads their file first, with lines of its own that
    /// name no format, `NA` (as yt-dlp 2026.10.26 prints them for an HLS subtitle rendition), each
    /// file counting from 0 again. They are no stream: the video is not taken for the last one,
    /// its end is still not the whole download (#344), and their bytes leave the bar when the
    /// video's first line comes.
    #[test]
    fn subtitles_before_the_video_are_no_stream() {
        let streams = [
            hls_stream("NA", 3, 10),
            hls_stream("NA", 3, 10),
            hls_stream("137", 16, 100),
            hls_stream("140", 4, 100),
        ];
        let v = run(Some(4000), &lines(Some("MDMQ 137+140 1920x1080"), &streams));
        let subs = streams[0].len() + streams[1].len();
        assert!(fractions(&v[..subs]).iter().all(|x| *x < 0.02), "{v:?}");
        assert_eq!(v[subs - 1].0, 60);
        assert_eq!(v[subs].0, 0);
        let f = fractions(&v[subs..]);
        assert_forward(&f);
        let video_end = streams[2].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v[subs..]);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// A stream's count from 0 again (a retry from the start) is the same stream, not the next
    /// one: the video is not taken for the last stream, its bytes count once, and the bar waits
    /// where it was until the retry is back there.
    #[test]
    fn a_retry_from_the_start_is_not_the_next_stream() {
        let mut video = hls_stream("137", 16, 100);
        video.splice(
            9..9,
            ["300 NA 1600 5 137".to_owned(), "0 NA NA 0 137".to_owned()],
        );
        let streams = [video, hls_stream("140", 4, 100)];
        let v = run(Some(4000), &lines(Some("MDMQ 137+140 1920x1080"), &streams));
        assert_eq!([v[8].0, v[9].0, v[10].0, v[11].0], [800, 800, 800, 900]);
        let f = fractions(&v);
        assert_forward(&f);
        let video_end = streams[0].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v);
        assert_eq!(v[video_end].0, 1600);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// A variable bitrate's estimates: low at first, then up and down around the size as the
    /// fragments come. The bar never goes back and ends at the end.
    #[test]
    fn noisy_estimates_never_pull_the_bar_back() {
        let mut l = vec![
            "MDMQ hls-1 1280x720".to_owned(),
            "0 NA NA 0 hls-1".to_owned(),
        ];
        for (got, estimate) in [
            (100, 400),
            (200, 900),
            (300, 1300),
            (400, 1100),
            (500, 1000),
            (600, 1200),
            (700, 1000),
            (800, 1050),
            (900, 1000),
        ] {
            l.push(format!("{got} NA {estimate} 5 hls-1"));
        }
        l.push("1000 1000 NA 5 hls-1".to_owned());
        let l: Vec<&str> = l.iter().map(String::as_str).collect();
        let v = run(Some(2000), &l);
        assert_forward(&fractions(&v));
        assert_ends(&v);
    }

    /// The video window's size below what comes: the same figures as before yt-dlp named its
    /// streams (a stream's end never pulls the bar back).
    #[test]
    fn an_estimate_below_the_size_is_as_before() {
        for (expected, streams, quality) in [
            (1000, vec![hls_stream("22", 15, 100)], "MDMQ 22 1280x720"),
            (
                1200,
                vec![hls_stream("137", 16, 100), hls_stream("140", 4, 100)],
                "MDMQ 137+140 1920x1080",
            ),
            (
                1800,
                vec![hls_stream("137", 16, 100), hls_stream("140", 4, 100)],
                "MDMQ 137+140 1920x1080",
            ),
        ] {
            let with = run(Some(expected), &lines(Some(quality), &streams));
            assert_eq!(with, as_before(Some(expected), &streams));
            assert_forward(&fractions(&with));
        }
    }

    /// The streams `MDMQ` names, and which format ids are theirs: the resolution after the last
    /// one (two words for a sound) is not part of its id; the merged id is all of them at once;
    /// nothing is named before yt-dlp has said, nor `NA`.
    #[test]
    fn the_streams_are_named_by_their_format_ids() {
        let mut m = Meter::new(None);
        assert_eq!(m.named(Some("137")), None);
        let cases: [(&str, &[&str], &[&str]); 4] = [
            (
                "137+140 1920x1080",
                &["137", "140"],
                &["13", "14", "251", "NA"],
            ),
            ("18 640x360", &["18"], &["1", "640x360"]),
            (
                "hls-720p+hls-audio-English 1280x720",
                &["hls-720p", "hls-audio-English"],
                &["hls-audio", "hls-720p+hls-audio"],
            ),
            (
                "aud-English (US) audio only",
                &["aud-English (US)"],
                &["aud"],
            ),
        ];
        for (q, ids, others) in cases {
            m.quality(q);
            assert_eq!(m.streams(), Some(ids.len()), "{q}");
            for id in ids {
                assert_eq!(m.named(Some(*id)), Some((*id, false)), "{q}");
            }
            for id in others {
                assert_eq!(m.named(Some(*id)), None, "{q}: {id}");
            }
        }
        m.quality("137+140 1920x1080");
        assert_eq!(m.named(Some("137+140")), Some(("137+140", true)));
        assert_eq!(m.named(None), None);
        m.quality("");
        assert_eq!(m.streams(), None);
    }

    /// The subtitle arguments (#68): none asked, none passed; beside the video as SRT with
    /// ffmpeg, the site's own format without; embedded only with ffmpeg; every uploader's
    /// language but a live stream's chat.
    #[test]
    fn subtitle_arguments() {
        let subs = |langs: &[&str], automatic: bool, embed: bool| Subs {
            langs: langs.iter().map(|l| l.to_string()).collect(),
            automatic,
            embed,
        };
        let s = |v: Vec<String>| v.join(" ");
        assert!(subtitle_args(&Subs::default(), true).is_empty());
        assert_eq!(
            s(subtitle_args(&subs(&["fr"], false, false), true)),
            "--write-subs --sub-langs fr --sub-format srt/best --convert-subs srt --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&["fr"], false, false), false)),
            "--write-subs --sub-langs fr --sub-format srt/vtt/best --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&["en-orig"], true, true), true)),
            "--write-auto-subs --sub-langs en-orig --sub-format srt/best --convert-subs srt --embed-subs --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&[ALL_UPLOADERS], false, true), true)),
            "--sub-langs all,-live_chat --sub-format srt/best --convert-subs srt --embed-subs --ignore-errors"
        );
        // Embedding needs ffmpeg: without it they're saved beside the video.
        assert_eq!(
            s(subtitle_args(&subs(&["de", "fr"], false, true), false)),
            "--write-subs --sub-langs de,fr --sub-format srt/vtt/best --ignore-errors"
        );
    }

    /// Embed chapters (#827): `--embed-chapters` for a video or the sound as the site has it,
    /// with ffmpeg; nothing when the option is off, without ffmpeg (which writes them), or for a
    /// conversion to MP3, Opus or FLAC.
    #[test]
    fn chapter_arguments() {
        assert_eq!(chapter_args(true, true, false), ["--embed-chapters"]);
        assert!(chapter_args(false, true, false).is_empty());
        assert!(chapter_args(true, false, false).is_empty());
        assert!(chapter_args(true, true, true).is_empty());
    }

    /// yt-dlp's warnings for subtitles it couldn't get (#68): each language once, a site's
    /// "too many requests" told apart from other reasons, other warnings ignored.
    #[test]
    fn refused_subtitles_from_the_warnings() {
        let w: Vec<String> = [
            "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests",
            "[youtube] abc: Some formats are missing",
            "Unable to download video subtitles for 'de': HTTP Error 404: Not Found",
            "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            subtitle_failures(&w),
            [
                ("en".to_string(), SubReason::TooManyRequests),
                (
                    "de".to_string(),
                    SubReason::Other("HTTP Error 404: Not Found".into())
                ),
            ]
        );
        assert!(subtitle_failures(&[]).is_empty());
        // The retry fetches only those, beside the video, never into it.
        let r = SubsRetry {
            automatic: true,
            embed: true,
            ffmpeg: true,
            base: vec!["-o".into(), "x.%(ext)s".into()],
        };
        assert_eq!(
            r.args(&["en".into()]).join(" "),
            "-o x.%(ext)s --skip-download --write-auto-subs --sub-langs en --sub-format srt/best --convert-subs srt --ignore-errors"
        );
        let info = serde_json::json!({"automatic_captions": {"en": [{"name": "English"}]}});
        assert_eq!(sub_name(&info, "en"), "English");
        assert_eq!(sub_name(&info, "ru"), "ru");
    }

    /// The files found beside a finished video are its own subtitles only.
    #[test]
    fn side_files_are_the_videos_subtitles() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "Talk.mp4",
            "Talk.fr.srt",
            "Talk.en-orig.vtt",
            "Talk.part",
            "Talk 2.en.srt",
            "Other.fr.srt",
            "Talk.jpg",
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let names: Vec<String> = side_files(&dir.path().join("Talk.mp4"))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Talk.en-orig.vtt", "Talk.fr.srt"]);
    }

    /// #94: a window's choice falls back to the nearest quality below it, then to the best;
    /// choices without a height (audio, "b") have no fallback; the selector is swapped in place.
    #[test]
    fn a_missing_quality_falls_back_to_the_nearest_below() {
        assert_eq!(
            relaxed("bv*[height=1080]+ba/b[height=1080]").as_deref(),
            Some("bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/bv*")
        );
        assert_eq!(relaxed("ba/b"), None);
        assert_eq!(relaxed("b"), None);
        let ours: Vec<String> = ["-f", "bv*[height=720]+ba/b[height=720]", "-o", "x"]
            .map(String::from)
            .to_vec();
        assert_eq!(selector_of(&ours), Some("bv*[height=720]+ba/b[height=720]"));
        assert_eq!(
            with_selector(&ours, "b"),
            ["-f", "b", "-o", "x"].map(String::from)
        );
    }

    /// #79: the saved file found by our own output name: the media file, not its subtitles,
    /// its unfinished part or another video's file; `%` in a title as it was written.
    #[test]
    fn the_saved_file_is_found_by_its_name() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "Şirin 100% ğüş.mp4",
            "Şirin 100% ğüş.ru.srt",
            "Şirin 100% ğüş.f137.mp4.part",
            "Other.mp4",
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let template = dir
            .path()
            .join(format!("{}.%(ext)s", template_text("Şirin 100% ğüş")));
        let ours: Vec<String> = vec!["-o".into(), template.to_string_lossy().into_owned()];
        assert_eq!(
            saved_by_name(&ours)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "Şirin 100% ğüş.mp4"
        );
        let none: Vec<String> = vec![
            "-o".into(),
            dir.path()
                .join("Missing.%(ext)s")
                .to_string_lossy()
                .into_owned(),
        ];
        assert_eq!(saved_by_name(&none), None);
    }

    /// A2: the quality menu's look-up is taken for the very choice it offered only (its selector
    /// under the same label, merged or not as the menu had it), and waited for while it's being
    /// made, never past its time or a Stop. Too big to keep: none. Nothing of it in `Debug`.
    #[test]
    fn the_menus_look_up_serves_its_own_choice_only() {
        const PAGE: &str = r#"{"title": "T", "duration": 10, "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "tbr": 500}]}"#;
        let page = PAGE.as_bytes();
        let label = "360p MP4 · H.264";
        assert!(
            offered(
                page,
                "18/b[height=360][vcodec^=avc1]/b[height=360]",
                label,
                false
            )
            .is_some()
        );
        assert!(offered(page, "18/b[height=720]", "720p MP4 · H.264", false).is_none());
        assert!(offered(page, "18/b[height=360]", "360p WEBM · H.264", false).is_none());
        assert!(offered(page, "18/b[height=360]", label, true).is_none());
        assert!(
            offered(
                page,
                "18/bv*[height=360][vcodec^=avc1]+ba/bv*[height=360]+ba/b[height=360]",
                label,
                true
            )
            .is_some()
        );
        assert!(offered(b"not json", "b", "b", false).is_none());

        let slot = InfoSlot::new(Instant::now());
        let filler = slot.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            filler.fill(page.to_vec(), true);
        });
        let (json, without) = slot
            .wait(Instant::now() + Duration::from_secs(10), || false)
            .unwrap();
        assert_eq!((json.as_slice(), without), (page, true));
        slot.fail();
        assert!(slot.wait(Instant::now(), || false).is_some(), "kept");
        assert_eq!(format!("{slot:?}"), "InfoSlot");

        let pending = InfoSlot::new(Instant::now());
        let at = Instant::now();
        assert!(
            pending
                .wait(at + Duration::from_millis(200), || false)
                .is_none()
        );
        assert!(at.elapsed() >= Duration::from_millis(200));
        assert!(
            pending
                .wait(at + Duration::from_secs(60), || true)
                .is_none(),
            "stopped"
        );
        assert!(pending.pending());
        pending.fail();
        assert!(!pending.pending());

        let big = InfoSlot::new(Instant::now());
        big.fill(vec![b' '; INFO_MAX + 1], false);
        assert!(!big.pending());
        assert!(
            big.wait(Instant::now() + Duration::from_secs(1), || false)
                .is_none()
        );
    }

    /// All the look-ups kept together stay within their budget: the oldest answers are let go
    /// (their downloads look the page up afresh); one no longer kept anywhere counts no more.
    #[test]
    fn look_ups_kept_have_a_budget_together() {
        let pool = Kept::new(100);
        let now = Instant::now();
        let kept = |n: usize| {
            let slot = InfoSlot::counted_in(now, pool.clone());
            slot.fill(vec![b' '; n], false);
            slot
        };
        let has = |s: &InfoSlot| s.wait(Instant::now(), || false).is_some();
        let (a, b) = (kept(40), kept(40));
        assert!(has(&a) && has(&b));
        let c = kept(40);
        assert!(!has(&a), "the oldest let go");
        assert!(has(&b) && has(&c));
        assert!(!a.pending(), "not waited for either");
        // Dropped: its 40 bytes are free.
        drop(b);
        let d = kept(40);
        assert!(has(&c) && has(&d));
        // One bigger than the rest together: the others go, it stays.
        let e = kept(90);
        assert!(!has(&c) && !has(&d) && has(&e));
        // The process's own pool: INFO_TOTAL.
        assert_eq!(Kept::shared().limit, INFO_TOTAL);
    }

    #[test]
    fn percent_signs_in_titles_stay_text() {
        assert_eq!(template_text("100% fun"), "100%% fun");
    }

    /// #622: a Dropbox video (an HLS stream whose size nobody knows beforehand) read 99.9 % from
    /// its first second to its end, the size growing with what came and 0 seconds left. yt-dlp's
    /// first line for a stream gives an estimate as big as what came (1 KB of 1 KB: it knows
    /// nothing yet), and the bar held on to that 99.9 % ever after. These are the lines yt-dlp
    /// printed for a Dropbox video, its 1080p stream then its sound.
    #[test]
    fn a_first_estimate_as_big_as_what_came_is_no_end() {
        let lines = [
            "MDMQ 1080p+aud192k-Track_1 1920x1080",
            "1024 NA 1024 491.2 1080p",
            "3072 NA 20480.0 491.2 1080p",
            "7168 NA 61440.0 491.2 1080p",
            "1047552 NA 10465280.0 452673.0 1080p",
            "1206648 NA 12046000.0 502170.4 1080p",
            "9775640 NA 37005408.0 1553343.6 1080p",
            "22221428 NA 39449345.4 1594996.9 1080p",
            "36687072 NA 42130818.8 1439490.0 1080p",
            "39940760 NA 39940760 1343764.3 1080p",
            "64512 NA 253952.0 19016.1 aud192k-Track_1",
            "1122012 NA 2178488.0 258367.6 aud192k-Track_1",
            "2185904 NA 2460726.8 369230.2 aud192k-Track_1",
        ];
        let got = run(None, &lines);
        // Nothing known at the first line: no percentage, rather than the end.
        assert_eq!(got[0], (1024, None));
        let f: Vec<f64> = got[1..]
            .iter()
            .map(|(d, t)| (*d as f64 / t.unwrap() as f64).min(MAX_WHILE_RUNNING))
            .collect();
        // A sliver at first, then what the estimate says: about a quarter at 9.8 MB of ~37 MB.
        assert!(f[0] < 0.2, "{f:?}");
        assert!(f[4] > 0.2 && f[4] < 0.3, "{f:?}");
        assert!(f[6] > 0.85 && f[6] < 0.9, "{f:?}");
        // The time left counts down from a size that is not just what came.
        assert!(got[5].1.unwrap() > got[5].0 * 3, "{got:?}");
    }

    /// #1102: yt-dlp's `[Errno 2]` for a path past the limits is the path too long, named; one for
    /// a folder that really isn't there, or another error, is not.
    #[test]
    fn a_path_too_long_is_named() {
        let stem = "x".repeat(240);
        let far = format!(r"D:\Downloads\Video\{stem}.faudio-128000-Audio.mp4.ytdl");
        let repr = far.replace('\\', r"\\");
        // As the report has it: `He'd` in the name, so Python quotes it with `"`.
        let said = format!(
            "yt-dlp failed: Unable to download video: [Errno 2] No such file or directory: \"{}He'd.mp4.part\"",
            repr.trim_end_matches(".faudio-128000-Audio.mp4.ytdl")
        );
        assert_eq!(
            too_long(&said),
            Some(PathBuf::from(format!(
                r"D:\Downloads\Video\{stem}He'd.mp4.part"
            )))
        );
        let said = format!(
            "ERROR: Unable to download video: [Errno 2] No such file or directory: '{repr}'"
        );
        assert_eq!(too_long(&said), Some(PathBuf::from(&far)));
        let missing =
            r"yt-dlp failed: [Errno 2] No such file or directory: 'D:\\Gone\\Talk.mp4.part'";
        assert_eq!(too_long(missing), None);
        let linux = "ERROR: unable to open for writing: [Errno 36] File name too long: '/home/a/Talk.f1.mp4.part'";
        assert_eq!(
            too_long(linux),
            Some(PathBuf::from("/home/a/Talk.f1.mp4.part"))
        );
        let windows =
            r"ERROR: [WinError 206] The filename or extension is too long: 'C:\\v\\Talk.mp4'";
        assert_eq!(too_long(windows), Some(PathBuf::from(r"C:\v\Talk.mp4")));
        assert_eq!(too_long("yt-dlp failed: Video unavailable"), None);
    }

    /// #1102: a video whose files wouldn't fit its folder is refused as it starts, the path
    /// named, before yt-dlp is asked anything.
    #[test]
    fn a_stem_too_long_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let stem = "s".repeat(240);
        let refused = VideoJob::start(
            Path::new("yt-dlp-is-not-run"),
            "https://video.example/talk",
            "b",
            dir.path(),
            &stem,
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            None,
        );
        match refused {
            Err(ProcError::PathTooLong(p)) => {
                assert_eq!(p, crate::workdir::absolute(dir.path()).join(&stem))
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
    }

    /// A live stream's download as yt-dlp's output name says it: `<dir>/Talk.%(ext)s`.
    fn template(dir: &Path) -> Vec<String> {
        template_named(dir, "Talk")
    }

    fn template_named(dir: &Path, stem: &str) -> Vec<String> {
        vec![
            "-o".into(),
            dir.join(format!("{stem}.%(ext)s"))
                .to_string_lossy()
                .into_owned(),
        ]
    }

    /// MPEG-TS, as yt-dlp's ffmpeg records a live stream: `len` bytes of 188-byte packets.
    fn ts(len: usize) -> Vec<u8> {
        let mut packet = [0u8; 188];
        packet[0] = 0x47;
        let mut out = packet.repeat(len.div_ceil(188));
        out.truncate(len);
        out
    }

    fn listed(dir: &Path) -> Vec<String> {
        let mut n: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    }

    fn log(progress: &Mutex<Progress>) -> Vec<String> {
        let p = progress.lock().unwrap();
        p.events.iter().map(|(_, t)| t.clone()).collect()
    }

    /// A recording saved without MDM's ffmpeg is kept as the MPEG-TS it is, named out of MDM's
    /// numbering when its name is taken; one that isn't MPEG-TS stays where it is; nothing
    /// recorded is a Stop, and a file yt-dlp finished itself is the download's.
    #[test]
    fn a_saved_recording_is_kept_as_it_is() {
        let progress = Mutex::new(Progress::default());
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ours = template(d);
        let part = d.join("Talk.mp4.part");
        let finish = |said: Option<PathBuf>| finish_recording(&ours, said, None, &progress);
        // Nothing there, or nothing in it.
        assert_eq!(finish(None), Saved::Stopped);
        std::fs::write(&part, b"").unwrap();
        assert_eq!(finish(None), Saved::Stopped);
        assert!(part.is_file(), "an empty one is left as it is");
        std::fs::remove_file(&part).unwrap();
        // yt-dlp finished it (it said where, or by our name).
        let said = d.join("Talk.mkv");
        std::fs::write(&said, b"video").unwrap();
        assert_eq!(finish(Some(said.clone())), Saved::Done(said.clone()));
        std::fs::remove_file(&said).unwrap();
        std::fs::write(d.join("Talk.mp4"), b"video").unwrap();
        assert_eq!(finish(None), Saved::Done(d.join("Talk.mp4")));
        assert_eq!(finish(Some(said)), Saved::Done(d.join("Talk.mp4")));
        std::fs::remove_file(d.join("Talk.mp4")).unwrap();
        // MPEG-TS: `Talk.ts`, then `Talk (recording 1).ts`, never written over.
        std::fs::write(&part, ts(5000)).unwrap();
        assert_eq!(finish(None), Saved::Done(d.join("Talk.ts")));
        std::fs::write(&part, ts(6000)).unwrap();
        assert_eq!(finish(None), Saved::Done(d.join("Talk (recording 1).ts")));
        assert_eq!(std::fs::read(d.join("Talk.ts")).unwrap(), ts(5000));
        assert_eq!(listed(d), ["Talk (recording 1).ts", "Talk.ts"]);
        // Not MPEG-TS: it stays, and the error says where.
        std::fs::write(&part, vec![1u8; 5000]).unwrap();
        let Saved::Failed(e) = finish(None) else {
            panic!("not failed")
        };
        assert!(e.contains(&part.display().to_string()), "{e}");
        assert_eq!(std::fs::read(&part).unwrap(), vec![1u8; 5000]);
        assert!(log(&progress).is_empty(), "{:?}", log(&progress));
        // A stream's own part is one of the recording's; its fragments are not.
        std::fs::remove_file(&part).unwrap();
        std::fs::write(d.join("Talk.mp4.part-Frag3.part"), ts(9000)).unwrap();
        let f96 = d.join("Talk.f96.mp4.part");
        std::fs::write(&f96, ts(400)).unwrap();
        std::fs::write(&part, ts(2000)).unwrap();
        assert_eq!(
            parts_of(d, "Talk"),
            [(f96.clone(), 400), (part.clone(), 2000)]
        );
        // Several parts and no MP4 made of them: every one kept as it is, named.
        let Saved::Failed(e) = finish(None) else {
            panic!("not failed")
        };
        assert!(
            e.starts_with(
                "The recording's 2 parts couldn't be joined into an MP4 (there's no ffmpeg)"
            ) && e.contains(&f96.display().to_string())
                && e.contains(&part.display().to_string()),
            "{e}"
        );
        assert_eq!(std::fs::read(&f96).unwrap(), ts(400));
        assert_eq!(std::fs::read(&part).unwrap(), ts(2000));
        assert_eq!(
            log(&progress),
            ["The recording came in 2 parts: Talk.f96.mp4.part, Talk.mp4.part"]
        );
    }

    /// Before a live stream is recorded again, an earlier recording is kept aside as
    /// `<stem> (recording N).ts` (the log says so); one that isn't MPEG-TS stops yt-dlp from
    /// starting. Neither this download's files nor another's of a name MDM numbered.
    #[test]
    fn an_earlier_recording_is_kept_aside() {
        let progress = Mutex::new(Progress::default());
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ours = template(d);
        let part = d.join("Talk.mp4.part");
        std::fs::write(&part, ts(1000)).unwrap();
        assert_eq!(set_aside_recordings(&ours, &progress), Ok(()));
        std::fs::write(&part, ts(2000)).unwrap();
        assert_eq!(set_aside_recordings(&ours, &progress), Ok(()));
        assert_eq!(
            std::fs::read(d.join("Talk (recording 1).ts")).unwrap(),
            ts(1000)
        );
        assert_eq!(
            std::fs::read(d.join("Talk (recording 2).ts")).unwrap(),
            ts(2000)
        );
        assert_eq!(
            log(&progress),
            [
                "The earlier recording was kept as Talk (recording 1).ts",
                "The earlier recording was kept as Talk (recording 2).ts"
            ]
        );
        // Empty: nothing to keep, removed (the recorder creates its own).
        std::fs::write(&part, b"").unwrap();
        assert_eq!(set_aside_recordings(&ours, &progress), Ok(()));
        assert!(!part.exists());
        assert_eq!(
            log(&progress).last().unwrap(),
            "Removed the empty unfinished file Talk.mp4.part left by an earlier run"
        );
        // Not MPEG-TS: refused, named, left as it is.
        std::fs::write(&part, vec![1u8; 1000]).unwrap();
        let e = set_aside_recordings(&ours, &progress).unwrap_err();
        assert!(e.contains(&part.display().to_string()), "{e}");
        assert_eq!(std::fs::read(&part).unwrap(), vec![1u8; 1000]);
        // Kept aside, they are no download's files: not this one's, nor `Talk (2)`'s.
        std::fs::write(d.join("Talk (2).mp4"), b"another").unwrap();
        assert_eq!(own_files(d, "Talk (2)", true), [d.join("Talk (2).mp4")]);
        assert_eq!(own_files(d, "Talk", true), std::slice::from_ref(&part));
        assert_eq!(
            recording_name(d, "Talk", "mp4", true).unwrap(),
            d.join("Talk.mp4")
        );
        assert_eq!(
            recording_name(d, "Talk", "ts", false).unwrap(),
            d.join("Talk (recording 3).ts")
        );
        // The picture and the sound recorded apart: each kept aside on its own; one of them not
        // MPEG-TS, and none is.
        std::fs::remove_file(&part).unwrap();
        assert!(own_files(d, "Talk", false).is_empty());
        let (video, audio) = (d.join("Talk.f1.mp4.part"), d.join("Talk.f2.m4a.part"));
        std::fs::write(&video, ts(3000)).unwrap();
        std::fs::write(&audio, vec![1u8; 500]).unwrap();
        let mut own = own_files(d, "Talk", false);
        own.sort();
        assert_eq!(own, [video.clone(), audio.clone()]);
        assert!(set_aside_recordings(&ours, &progress).is_err());
        assert!(video.is_file() && audio.is_file());
        std::fs::write(&audio, ts(500)).unwrap();
        assert_eq!(set_aside_recordings(&ours, &progress), Ok(()));
        assert_eq!(
            std::fs::read(d.join("Talk (recording 3).ts")).unwrap(),
            ts(3000)
        );
        assert_eq!(
            std::fs::read(d.join("Talk (recording 4).ts")).unwrap(),
            ts(500)
        );
        assert!(!video.exists() && !audio.exists());
    }

    /// A live stream's earlier run left empty files (stopped before the recorder's first block):
    /// removed before it records again, none that holds anything, none with yt-dlp's resume state
    /// beside it, none of MDM's own (`.mdm.part`) nor of another download's name.
    #[test]
    fn an_earlier_runs_empty_recording_is_removed_and_nothing_else() {
        let progress = Mutex::new(Progress::default());
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ours = template(d);
        let write = |name: &str, bytes: &[u8]| std::fs::write(d.join(name), bytes).unwrap();
        write("Talk.mp4.part", b"");
        write("Talk.f1.mp4.part", b"");
        write("Talk.f1.mp4.ytdl", b"{}");
        write("Talk.f2.m4a.part", b"");
        write("Talk.f2.m4a.part-Frag0", b"x");
        write("Talk.f3.m4a.part", &ts(500));
        write("Talk.mp4.mdm.part", b"");
        write("Other.mp4.part", b"");
        assert_eq!(set_aside_recordings(&ours, &progress), Ok(()));
        // Only the empty `Talk.mp4.part` went; the one holding something was kept aside (the
        // rest of the folder is as it was).
        assert_eq!(
            listed(d),
            [
                "Other.mp4.part",
                "Talk (recording 1).ts",
                "Talk.f1.mp4.part",
                "Talk.f1.mp4.ytdl",
                "Talk.f2.m4a.part",
                "Talk.f2.m4a.part-Frag0",
                "Talk.mp4.mdm.part"
            ]
        );
        assert_eq!(
            std::fs::read(d.join("Talk (recording 1).ts")).unwrap(),
            ts(500)
        );
        assert_eq!(
            log(&progress),
            [
                "Removed the empty unfinished file Talk.mp4.part left by an earlier run",
                "The earlier recording was kept as Talk (recording 1).ts"
            ]
        );
    }

    /// Before any run of a video: a plain `.part` an earlier run left that is MPEG-TS, with no
    /// resume state of yt-dlp's beside it, is a recording, kept aside; anything else is a
    /// download to go on with, left alone.
    #[test]
    fn an_earlier_runs_recording_is_kept_aside_before_any_run() {
        let progress = Mutex::new(Progress::default());
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let ours = template(d);
        let part = d.join("Talk.mp4.part");
        std::fs::write(&part, ts(1000)).unwrap();
        assert_eq!(set_aside_earlier(&ours, &progress), Ok(()));
        assert_eq!(listed(d), ["Talk (recording 1).ts"]);
        assert_eq!(
            log(&progress),
            ["The earlier recording was kept as Talk (recording 1).ts"]
        );
        // yt-dlp's resume state beside it (its own download in pieces, a stream's too), a DASH
        // stream's part (MP4), MDM's engine's part, one not MPEG-TS, an empty one: all left
        // alone.
        std::fs::write(&part, ts(1000)).unwrap();
        std::fs::write(d.join("Talk.mp4.ytdl"), b"{}").unwrap();
        std::fs::write(d.join("Clip.webm.part"), ts(1000)).unwrap();
        std::fs::write(d.join("Clip.webm.part-Frag2.part"), ts(10)).unwrap();
        std::fs::write(d.join("Talk.f137.mp4.part"), vec![1u8; 1000]).unwrap();
        std::fs::write(d.join("Talk.f1.mp4.part"), ts(1000)).unwrap();
        std::fs::write(d.join("Talk.f1.mp4.ytdl"), b"{}").unwrap();
        std::fs::write(d.join("Talk.f140.m4a.mdm.part"), ts(1000)).unwrap();
        std::fs::write(d.join("Talk.mkv.part"), vec![1u8; 1000]).unwrap();
        std::fs::write(d.join("Talk.webm.part"), b"").unwrap();
        let before = listed(d);
        assert_eq!(set_aside_earlier(&ours, &progress), Ok(()));
        assert_eq!(
            set_aside_earlier(&template_named(d, "Clip"), &progress),
            Ok(())
        );
        assert_eq!(listed(d), before);
        // Without their resume state: recordings, a stream's own part too, each on its own.
        std::fs::remove_file(d.join("Talk.mp4.ytdl")).unwrap();
        std::fs::remove_file(d.join("Talk.f1.mp4.ytdl")).unwrap();
        assert_eq!(set_aside_earlier(&ours, &progress), Ok(()));
        assert!(!part.exists() && !d.join("Talk.f1.mp4.part").exists());
        assert_eq!(
            std::fs::read(d.join("Talk (recording 2).ts")).unwrap(),
            ts(1000)
        );
        assert_eq!(
            std::fs::read(d.join("Talk (recording 3).ts")).unwrap(),
            ts(1000)
        );
        assert!(
            d.join("Talk.f137.mp4.part").is_file(),
            "a DASH stream's goes on"
        );
    }

    /// Whether the MP4 made of a recording holds all of it, by its size when ffprobe can't
    /// tell: nine tenths of the parts' at least. ffprobe is looked for beside ffmpeg.
    #[test]
    fn an_mp4_is_whole_by_its_size_without_ffprobe() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let parts = [(d.join("a.part"), 600), (d.join("b.part"), 400)];
        let mp4 = d.join("Talk.mp4");
        std::fs::write(&mp4, vec![0u8; 900]).unwrap();
        assert!(whole(None, &mp4, &parts));
        std::fs::write(&mp4, vec![0u8; 899]).unwrap();
        assert!(!whole(None, &mp4, &parts));
        // An ffprobe that can't be run: the same.
        assert!(!whole(Some(&d.join("missing-ffprobe")), &mp4, &parts));
        // Beside ffmpeg, named alike.
        let exe = std::env::consts::EXE_SUFFIX;
        assert_eq!(ffprobe_of(&d.join(format!("ffmpeg{exe}"))), None);
        std::fs::write(d.join(format!("ffprobe{exe}")), b"").unwrap();
        std::fs::write(d.join(format!("fake-ffprobe{exe}")), b"").unwrap();
        assert_eq!(
            ffprobe_of(&d.join(format!("ffmpeg{exe}"))),
            Some(d.join(format!("ffprobe{exe}")))
        );
        assert_eq!(
            ffprobe_of(&d.join(format!("fake-ffmpeg{exe}"))),
            Some(d.join(format!("fake-ffprobe{exe}")))
        );
        assert_eq!(
            ffprobe_of(&d.join("transcoder")),
            Some(d.join(format!("ffprobe{exe}")))
        );
    }

    /// Real ffmpeg: `MDM_LIVE_FFMPEG=<ffmpeg> cargo test -p mdm-tools real_ -- --ignored
    /// --nocapture` (ffprobe beside it, as MDM's ffmpeg has it). `args` for ffmpeg, which must end
    /// well.
    fn ffmpeg_made(ffmpeg: &Path, args: &[&str], out: &Path) {
        let made = std::process::Command::new(ffmpeg)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .arg(out)
            .status()
            .unwrap();
        assert!(made.success(), "{args:?}");
    }

    /// A minute of a picture (small, few frames) and a sound, as lavfi makes them, MPEG-TS.
    const PICTURE: [&str; 6] = [
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=60:size=160x120:rate=10",
        "-c:v",
        "mpeg2video",
    ];
    const SOUND: [&str; 6] = [
        "-f",
        "lavfi",
        "-i",
        "sine=duration=60:sample_rate=22050",
        "-c:a",
        "mp2",
    ];

    fn real_ffmpeg() -> (PathBuf, PathBuf) {
        let ffmpeg = PathBuf::from(std::env::var("MDM_LIVE_FFMPEG").expect("MDM_LIVE_FFMPEG"));
        let probe = ffprobe_of(&ffmpeg).expect("ffprobe beside ffmpeg");
        (ffmpeg, probe)
    }

    /// Real ffmpeg (see [`ffmpeg_made`]): a live stream's recording, a minute of MPEG-TS (video and
    /// sound), cut in the middle of a packet as a Stop leaves it, saved: an MP4 ffmpeg reads to the
    /// end, known whole by ffprobe's durations, the `.part` and the temporary file gone.
    #[test]
    #[ignore]
    fn real_a_recording_cut_short_becomes_an_mp4() {
        let (ffmpeg, probe) = real_ffmpeg();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let full = d.join("whole.ts");
        let mut args = PICTURE.to_vec();
        args.extend(SOUND);
        args.extend(["-f", "mpegts"]);
        ffmpeg_made(&ffmpeg, &args, &full);
        let mut cut = std::fs::read(&full).unwrap();
        // Two thirds in, a little into a packet.
        let at = cut.len() / 3 * 2 / 188 * 188 + 77;
        cut.truncate(at);
        let part = d.join("Talk.mp4.part");
        std::fs::write(&part, &cut).unwrap();
        let copy = d.join("cut.ts");
        std::fs::write(&copy, &cut).unwrap();
        let recorded = probed(&probe, &copy).expect("the recording's length").plays;
        let progress = Mutex::new(Progress::default());
        let saved = finish_recording(&template(d), None, Some(&ffmpeg), &progress);
        println!("{saved:?} {:?}", log(&progress));
        let mp4 = d.join("Talk.mp4");
        assert_eq!(saved, Saved::Done(mp4.clone()));
        assert!(!part.exists() && !d.join("Talk.temp.mp4").exists());
        // Whole by ffprobe's durations, not by a guess: none of them unknown or estimated.
        let made = probed(&probe, &mp4).expect("the MP4's length");
        println!(
            "whole by duration: the MP4 plays {:.3} s, its streams {:?}, the recording {recorded:.3} s",
            made.plays, made.streams
        );
        assert!(made.streams.len() == 2 && made.streams.iter().all(Option::is_some));
        let bytes = std::fs::read(&mp4).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        let read = std::process::Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&mp4)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        // Read to the end; the picture cut in the middle of its packet (the last one) may be
        // said damaged by the decoder, as it would be in the recording itself.
        let said = String::from_utf8_lossy(&read.stderr);
        let complaints = said.lines().filter(|l| !l.trim().is_empty()).count();
        assert!(read.status.success(), "{said}");
        assert!(
            said.lines()
                .all(|l| l.trim().is_empty() || l.contains("[mpeg2video @")),
            "{said}"
        );
        println!(
            "a TS of {} bytes cut at {at}: an MP4 of {} bytes, read to the end ({complaints} decoder lines about the cut picture)",
            std::fs::metadata(&full).unwrap().len(),
            bytes.len()
        );
    }

    /// Real ffmpeg (see [`ffmpeg_made`]): what [`whole`] says of real files. A minute of MPEG-TS
    /// against its MP4 (whole) and against an MP4 of its first half (not); a picture and a sound
    /// recorded apart, joined (whole), and joined with the sound cut to half (not: the MP4 plays
    /// to the end, its sound doesn't).
    #[test]
    #[ignore]
    fn real_whole_by_ffprobes_durations() {
        let (ffmpeg, probe) = real_ffmpeg();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let size = |p: &Path| std::fs::metadata(p).unwrap().len();
        let parts = |ps: &[&Path]| -> Vec<(PathBuf, u64)> {
            ps.iter().map(|p| (p.to_path_buf(), size(p))).collect()
        };
        let report = |what: &str, mp4: &Path| {
            let m = probed(&probe, mp4).unwrap();
            println!(
                "{what}: the MP4 plays {:.3} s, its streams {:?}",
                m.plays, m.streams
            );
        };
        let ts = d.join("both.ts");
        let mut args = PICTURE.to_vec();
        args.extend(SOUND);
        args.extend(["-f", "mpegts"]);
        ffmpeg_made(&ffmpeg, &args, &ts);
        let copy = ["-map", "0", "-c", "copy", "-f", "mp4"];
        // (a) The whole of it.
        let all = d.join("all.mp4");
        let mut a = vec!["-i", ts.to_str().unwrap()];
        a.extend(copy);
        ffmpeg_made(&ffmpeg, &a, &all);
        report("(a) whole", &all);
        assert!(whole(Some(&probe), &all, &parts(&[&ts])));
        // (b) Its first half.
        let half = d.join("half.mp4");
        let mut b = vec!["-i", ts.to_str().unwrap(), "-t", "30"];
        b.extend(copy);
        ffmpeg_made(&ffmpeg, &b, &half);
        report("(b) first half", &half);
        assert!(!whole(Some(&probe), &half, &parts(&[&ts])));
        // (c) A picture and a sound recorded apart, joined as a recording's parts are.
        let (video, audio) = (d.join("Talk.f1.mp4.part"), d.join("Talk.f2.mp4.part"));
        let mut v = PICTURE.to_vec();
        v.extend(["-f", "mpegts"]);
        ffmpeg_made(&ffmpeg, &v, &video);
        let mut s = SOUND.to_vec();
        s.extend(["-f", "mpegts"]);
        ffmpeg_made(&ffmpeg, &s, &audio);
        let split = parts(&[&video, &audio]);
        let joined = remuxed(&ffmpeg, &split, d, "Talk").unwrap();
        report("(c) joined", &joined);
        assert!(whole(Some(&probe), &joined, &split));
        // (d) The same, its sound cut to half: the MP4 plays to the end, its sound doesn't.
        let cut = d.join("cut.mp4");
        let mut c = vec!["-i", video.to_str().unwrap(), "-t", "30", "-i"];
        c.push(audio.to_str().unwrap());
        c.extend(["-map", "0", "-map", "1", "-c", "copy", "-f", "mp4"]);
        ffmpeg_made(&ffmpeg, &c, &cut);
        report("(d) sound cut", &cut);
        assert!(!whole(Some(&probe), &cut, &split));
        println!("whole by ffprobe's durations: (a) yes, (b) no, (c) yes, (d) no");
    }
}
