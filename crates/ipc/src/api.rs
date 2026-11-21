//! The automation API's answers, shared by the app (which writes them) and the programs that
//! read them (the command line, the MCP server), so both sides keep one shape.
//! `docs/automation-api.schema.json` is made from these types and [`crate::Command`]
//! (`MDM_WRITE_SCHEMA=1 cargo test -p mdm-ipc schema`); a test fails while it is out of date.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `{"query": "list"}`: every download, and the total speed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListAnswer {
    pub downloads: Vec<DownloadInfo>,
    /// The total download speed, bytes per second.
    pub speed: u64,
    /// The queues, in the Scheduler's order (the first is the main queue).
    #[serde(default)]
    pub queues: Vec<QueueInfo>,
    /// The categories a new download can go to (General first).
    #[serde(default)]
    pub categories: Vec<CategoryInfo>,
    /// Free space where downloads go (the General category's folder), bytes; null when unknown.
    #[serde(default)]
    pub free: Option<u64>,
}

/// One queue of the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct QueueInfo {
    /// Its name: what `queue` takes (a built-in queue's in English, as shipped).
    pub name: String,
    /// Is it running?
    pub running: bool,
    /// Its downloads' ids, in its order.
    pub ids: Vec<u64>,
}

/// One category of the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CategoryInfo {
    /// What `category` takes and a download's `category` says (a built-in category's English
    /// name, as shipped).
    pub key: String,
    /// Its name as the user sees it: a built-in one's English name (translated where it is
    /// shown) unless the user renamed it.
    pub name: String,
}

/// Criteria to filter downloads in a [`ListAnswer`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListFilter<'a> {
    pub id: Option<u64>,
    pub status: Option<&'a str>,
    pub search: Option<&'a str>,
    pub category: Option<&'a str>,
    pub queue: Option<&'a str>,
    pub limit: Option<usize>,
}

/// A filter could not be applied to a [`ListAnswer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterError {
    UnknownStatus {
        status: String,
        valid: &'static [&'static str],
    },
    UnknownQueue {
        queue: String,
        available: Vec<String>,
    },
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStatus { status, valid } => {
                write!(f, "Unknown status {status:?}: use one of {valid:?}.")
            }
            Self::UnknownQueue { queue, available } => {
                write!(
                    f,
                    "Unknown queue {queue:?}: the app has queues {available:?}."
                )
            }
        }
    }
}

impl std::error::Error for FilterError {}

impl ListAnswer {
    /// Return a copy of this answer with its downloads filtered by the given criteria,
    /// sorted newest-first (descending id).
    pub fn filtered(&self, filter: &ListFilter) -> Result<ListAnswer, FilterError> {
        let status = match filter.status.map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(
                STATUSES
                    .into_iter()
                    .find(|k| k.eq_ignore_ascii_case(s))
                    .ok_or_else(|| FilterError::UnknownStatus {
                        status: s.to_string(),
                        valid: &STATUSES,
                    })?,
            ),
        };

        let search = filter
            .search
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        let category = filter.category.map(str::trim).filter(|s| !s.is_empty());
        let queue = filter.queue.map(str::trim).filter(|s| !s.is_empty());

        let queue_ids: Option<Vec<u64>> = match queue {
            Some(q) => {
                let found = self
                    .queues
                    .iter()
                    .find(|qi| qi.name.eq_ignore_ascii_case(q));
                match found {
                    Some(qi) => Some(qi.ids.clone()),
                    None => {
                        return Err(FilterError::UnknownQueue {
                            queue: q.to_string(),
                            available: self.queues.iter().map(|qi| qi.name.clone()).collect(),
                        });
                    }
                }
            }
            None => None,
        };

        let mut downloads: Vec<DownloadInfo> = self
            .downloads
            .iter()
            .filter(|d| filter.id.is_none_or(|id| d.id == id))
            .filter(|d| status.is_none_or(|s| d.status.eq_ignore_ascii_case(s)))
            .filter(|d| {
                search.as_ref().is_none_or(|q| {
                    d.file.to_lowercase().contains(q) || d.url.to_lowercase().contains(q)
                })
            })
            .filter(|d| {
                category.as_ref().is_none_or(|c| {
                    d.category.eq_ignore_ascii_case(c)
                        || self.categories.iter().any(|ci| {
                            ci.key.eq_ignore_ascii_case(&d.category)
                                && ci.name.eq_ignore_ascii_case(c)
                        })
                })
            })
            .filter(|d| queue_ids.as_ref().is_none_or(|ids| ids.contains(&d.id)))
            .cloned()
            .collect();

        downloads.sort_by_key(|d| std::cmp::Reverse(d.id));
        if let Some(limit) = filter.limit {
            downloads.truncate(limit);
        }

        Ok(ListAnswer {
            downloads,
            speed: self.speed,
            queues: self.queues.clone(),
            categories: self.categories.clone(),
            free: self.free,
        })
    }
}

/// An address added (`url`): the download's id, or `pending` while a torrent's files are looked
/// up (it joins the list once they are known).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default)]
    pub pending: bool,
}

/// One download of the list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadInfo {
    /// What the actions take (`ids`); never reused.
    pub id: u64,
    /// The file's name.
    pub file: String,
    /// Its address (a video's page, a torrent's magnet link).
    pub url: String,
    /// Where it is saved: the file, or a torrent's folder.
    pub path: String,
    /// `Queued` (added, never started), `Downloading`, `Stopped`, `Complete` or `Error`.
    pub status: String,
    /// Its size in bytes; null while unknown.
    pub size: Option<u64>,
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Bytes per second; 0 when it isn't running.
    pub speed: u64,
    /// When it was added, as the list shows it; null for downloads from before that column.
    pub added: Option<String>,
    /// Its category's `key` (see [`CategoryInfo`]).
    #[serde(default)]
    pub category: String,
    /// Why it failed, for a download in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Seconds left at the speed it goes now, while it downloads with a known size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u64>,
    /// `Complete`: whether its file (a torrent's folder) is still at `path`: false once it was
    /// moved or deleted. Null while not looked at yet, and for the other statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    /// `Complete`: when it finished, in seconds since 1970 (null when the list doesn't know).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<i64>,
    /// What downloads it when it isn't a plain file: `video` (from a page, by yt-dlp),
    /// `torrent`, `gallery` (by gallery-dl). Null: a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// It can be downloaded again from the start (Redownload): a file that isn't running.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redownload: bool,
    /// The browser popup's list (#1496): its file type icon, a key of [`RecentAnswer::icons`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The statuses [`DownloadInfo::status`] takes.
pub const STATUSES: [&str; 5] = ["Queued", "Downloading", "Stopped", "Complete", "Error"];

/// The browser popup's list of downloads (#1496), [`crate::Command::recent`]'s answer: the
/// newest that match its search and filter, how many match each filter, and the file type icons
/// it lacked. Within [`RECENT_BUDGET`] bytes, so the native host's message to the browser stays
/// under its 1 MB limit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecentAnswer {
    /// Newest first, at most the `limit` asked for.
    pub downloads: Vec<DownloadInfo>,
    /// How many match the search under each filter (all of them, not only those answered).
    pub counts: RecentCounts,
    /// Downloads running now, whatever the search (the toolbar button's count).
    pub running: usize,
    /// The total download speed, bytes per second.
    pub speed: u64,
    /// The icons the popup lacked, by key: `data:` addresses of PNG pictures (an SVG one for a
    /// Linux desktop's scalable icon theme), the system's own for each type.
    pub icons: std::collections::BTreeMap<String, String>,
    /// MDM can open this system's share sheet for a file (Windows, macOS: Linux has none).
    pub share: bool,
}

/// How many downloads match the popup's search under each of its filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecentCounts {
    pub all: usize,
    pub running: usize,
    pub done: usize,
    pub failed: usize,
}

/// The most bytes a [`RecentAnswer`] takes as JSON: half the browser's limit for a native
/// message, the host's own fields and escaping left room.
pub const RECENT_BUDGET: usize = 512 << 10;

/// The popup's filter (`crate::RECENT_FILTERS`) a download of `status` is listed under, besides
/// `all`.
pub fn recent_filter(status: &str) -> Option<&'static str> {
    match status {
        "Downloading" => Some("running"),
        "Complete" => Some("done"),
        "Error" => Some("failed"),
        _ => None,
    }
}

impl RecentAnswer {
    /// The popup's view of `downloads` for `ask`: those whose name or address has every word
    /// of its search, under its filter (an unknown one is `all`), newest first (the highest
    /// id), at most its `limit` ([`crate::RECENT_MAX`]); the counts of each filter. Icons are
    /// the app's to add.
    pub fn of(downloads: Vec<DownloadInfo>, ask: &crate::RecentAsk, speed: u64) -> RecentAnswer {
        let words: Vec<String> = ask
            .search
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let found = |d: &DownloadInfo| {
            let (file, url) = (d.file.to_lowercase(), d.url.to_lowercase());
            words.iter().all(|w| file.contains(w) || url.contains(w))
        };
        let running = downloads
            .iter()
            .filter(|d| recent_filter(&d.status) == Some("running"))
            .count();
        let mut matching: Vec<DownloadInfo> = downloads.into_iter().filter(found).collect();
        let mut counts = RecentCounts {
            all: matching.len(),
            ..Default::default()
        };
        for d in &matching {
            match recent_filter(&d.status) {
                Some("running") => counts.running += 1,
                Some("done") => counts.done += 1,
                Some("failed") => counts.failed += 1,
                _ => {}
            }
        }
        let filter = crate::RECENT_FILTERS
            .into_iter()
            .find(|f| *f == ask.filter && *f != "all");
        if let Some(f) = filter {
            matching.retain(|d| recent_filter(&d.status) == Some(f));
        }
        matching.sort_by_key(|d| std::cmp::Reverse(d.id));
        matching.truncate(ask.limit.min(crate::RECENT_MAX));
        RecentAnswer {
            downloads: matching,
            counts,
            running,
            speed,
            ..Default::default()
        }
    }

    /// Within `max` bytes as JSON: the oldest downloads left out until it is (their icons
    /// with them, when no other download has the type). A download's address can be 8 KB.
    pub fn fit(&mut self, max: usize) {
        let mut size = line(self).len();
        while size > max {
            let Some(row) = self.downloads.pop() else {
                break;
            };
            size = size.saturating_sub(line(&row).len() + 1);
            if let Some(key) = row.icon
                && !self.downloads.iter().any(|d| d.icon.as_ref() == Some(&key))
                && let Some(icon) = self.icons.remove(&key)
            {
                size = size.saturating_sub(key.len() + icon.len() + 6);
            }
        }
        // What the estimate missed (a comma, a key's quotes): measured.
        while line(self).len() > max && self.downloads.pop().is_some() {}
    }
}

/// [`crate::Command::recent_act`]'s answer: done, or why not.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecentActAnswer {
    pub done: bool,
    /// Why not: one of [`recent_why`]'s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl RecentActAnswer {
    pub fn done() -> RecentActAnswer {
        RecentActAnswer {
            done: true,
            why: None,
        }
    }

    pub fn not(why: &str) -> RecentActAnswer {
        RecentActAnswer {
            done: false,
            why: Some(why.into()),
        }
    }
}

/// [`RecentActAnswer::why`]'s values.
pub mod recent_why {
    /// No download has that id (taken off the list since).
    pub const GONE: &str = "gone";
    /// Its file isn't where it was saved (moved or deleted).
    pub const MISSING: &str = "missing";
    /// It is running: stop it first (remove, delete, redownload).
    pub const RUNNING: &str = "running";
    /// Not for this download now (resume a complete one, pause one that isn't running, a
    /// torrent busy checking, redownload a video).
    pub const CANT: &str = "cant";
    /// This system can't (share on Linux; copying a file without an X11 display).
    pub const UNSUPPORTED: &str = "unsupported";
    /// The system refused (the clipboard held by another program, the share sheet).
    pub const FAILED: &str = "failed";
    /// Not one of [`crate::RECENT_ACTS`].
    pub const UNKNOWN: &str = "unknown";
}

/// `{"action": …, "ids": […]}`: what was done, and what was skipped and why.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ActAnswer {
    pub done: Vec<u64>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Skipped {
    pub id: u64,
    /// "no such download", "not running", "already running", "complete" or "running: pause it
    /// first".
    pub why: String,
}

/// `{"pair": true}` (#157): a new pairing code for the remote control, while it is on.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PairAnswer {
    /// The 8-digit code; it works once, for `minutes`.
    pub code: String,
    pub minutes: u32,
    /// The page's addresses, the one a phone most likely reaches first.
    pub urls: Vec<String>,
    /// A QR code of the first address with the code, drawn with text blocks for a terminal.
    pub qr: String,
}

/// `{"api": true}` / `{"api": false}`: the API's new state.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApiAnswer {
    pub api: bool,
}

/// An unknown question or action, or a question while the API is off.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ErrorAnswer {
    /// For people; its wording may change.
    pub error: String,
    /// For programs: `"api-off"` while the API is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// [`ErrorAnswer::code`] while the API is off.
pub const API_OFF: &str = "api-off";

/// The browser's quality menu (`video_lookup`, `video_poll`, `video_pick`): where a look-up is,
/// or what became of a choice. Answered whatever the automation API's switch (the browser's
/// host asks, not a program).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VideoLookupAnswer {
    /// One of [`lookup`]'s states.
    pub state: String,
    /// `pending` and `ready`: the look-up's token, to ask about it and to pick from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `ready`: the video's title, the file's name: the page's when yt-dlp knows the video only
    /// by its address, as the Download video window names it ([`crate::Command::video_title`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `ready`: the qualities to choose from, labelled in MDM's language; `video_pick` takes a
    /// place in this list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<VideoChoice>,
    /// `failed`: [`lookup::NO_YTDLP`], [`lookup::NO_CHOICES`] or [`lookup::ERROR`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `failed` with [`lookup::ERROR`]: why, in MDM's words and language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `ready` (and `failed` with [`lookup::NO_CHOICES`]): what the look-up lacked and how to
    /// mend it, shown with the choices (#852: YouTube's qualities need a JavaScript runtime).
    /// None from an app older than the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<VideoNotice>,
}

/// A word in the browser's quality menu on what the look-up lacked (#852), in MDM's language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VideoNotice {
    /// What is wrong and what to do ("YouTube needs a JavaScript runtime…").
    pub text: String,
    /// The link's words ("How to install one").
    pub link: String,
    /// The help page it opens, on MDM's website.
    pub url: String,
}

/// One quality the browser's quality menu offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VideoChoice {
    /// What the menu shows ("720p mp4"), in MDM's language.
    pub label: String,
    /// The file's type (`mp4`, `mp3`).
    pub ext: String,
    /// Bytes, exact or estimated (`approx`); null when unknown.
    pub size: Option<u64>,
    /// Only the sound (an audio file, a conversion to MP3, Opus or FLAC): the menu lists these
    /// apart from the videos. False from an app older than the field.
    pub audio: bool,
    /// The picture's height in pixels (the button's chip: 1080p, 4K); none for the sound, or a
    /// file known only by its type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Frames a second, rounded (an app older than #755 said it only above 31).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// The sound's bitrate in kbit/s, for the sound as the site has it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kbps: Option<u32>,
    /// The sound as the site sends it (not converted): the button's list says "original".
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub original: bool,
    /// The picture's codec as people name it: `AV1`, `VP9`, `H.264`, `H.265`… (#755). None
    /// from an app older than the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// The picture's dynamic range when worth saying: `HDR`, `Dolby Vision`, or `SDR` beside an
    /// HDR choice of the same height (#755).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// `size` is an estimate (a stream's bitrate times its length, a conversion's), shown as
    /// about ("~532 MB"); otherwise it is the bytes of exactly the streams this downloads (#801).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub approx: bool,
    /// The most compatible choice (#1439): an H.264 picture with AAC sound in an MP4, which
    /// every video editor, TV and phone plays; the menu says so beside its height. False from
    /// an app older than the field.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub compatible: bool,
}

/// [`VideoLookupAnswer::state`]'s values, and [`VideoLookupAnswer::reason`]'s.
pub mod lookup {
    /// Still looking (`token` to ask again).
    pub const PENDING: &str = "pending";
    /// Found: `title` and `choices`.
    pub const READY: &str = "ready";
    /// Not found: `reason`, with `message` for an error.
    pub const FAILED: &str = "failed";
    /// The page is a list of videos (a playlist, a channel): chosen in MDM's window.
    pub const LIST: &str = "list";
    /// Two look-ups are running already.
    pub const BUSY: &str = "busy";
    /// The video button is off on this site (Options' rules).
    pub const OFF: &str = "off";
    /// No such token (never given, or forgotten after 10 minutes).
    pub const GONE: &str = "gone";
    /// The Download video window does it instead (MDM not running, an older MDM, a Google Drive
    /// file, MDM without a window, a choice that doesn't fit its look-up).
    pub const USE_WINDOW: &str = "use-window";
    /// The choice is downloading.
    pub const STARTED: &str = "started";
    /// The choice was added to the queue (Options: "only add to the queue").
    pub const QUEUED: &str = "queued";
    /// MDM took the choice, and it isn't a new download: it is in the list already in that
    /// quality (Options' "If a duplicate download link is added": that one was opened or
    /// resumed, or MDM asks what to do in its own window). From the browser's host: MDM didn't
    /// say in time what became of it (`timeout`); the command is in its queue and will run.
    /// Either way the Download video window mustn't do it a second time; the extension takes it
    /// for `started`.
    pub const SENT: &str = "sent";
    /// `failed`: yt-dlp isn't installed.
    pub const NO_YTDLP: &str = "no-ytdlp";
    /// `failed`: the page has nothing this can download.
    pub const NO_CHOICES: &str = "no-choices";
    /// `failed`: yt-dlp couldn't look it up (`message` says why).
    pub const ERROR: &str = "error";
}

impl VideoLookupAnswer {
    /// An answer that is only a state.
    pub fn of(state: &str) -> VideoLookupAnswer {
        VideoLookupAnswer {
            state: state.into(),
            ..Default::default()
        }
    }

    /// Still looking up `token`.
    pub fn pending(token: &str) -> VideoLookupAnswer {
        VideoLookupAnswer {
            token: Some(token.into()),
            ..VideoLookupAnswer::of(lookup::PENDING)
        }
    }

    /// Not found, for `reason` (with `message` for an error).
    pub fn failed(reason: &str, message: Option<String>) -> VideoLookupAnswer {
        VideoLookupAnswer {
            reason: Some(reason.into()),
            message,
            ..VideoLookupAnswer::of(lookup::FAILED)
        }
    }
}

/// An answer as its line.
pub fn line<T: Serialize>(answer: &T) -> String {
    serde_json::to_string(answer).unwrap_or_default()
}

/// An answer read back: an [`ErrorAnswer`] or the answer asked for.
pub fn parse<T: serde::de::DeserializeOwned>(
    answer: &str,
) -> Result<Result<T, ErrorAnswer>, String> {
    if let Ok(e) = serde_json::from_str::<ErrorAnswer>(answer) {
        return Ok(Err(e));
    }
    serde_json::from_str(answer)
        .map(Ok)
        .map_err(|e| format!("unexpected answer ({e}): {answer}"))
}

/// The JSON Schema of the channel: a request line and every answer.
pub fn schema() -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let request = generator.subschema_for::<crate::Envelope>();
    let _ = generator.subschema_for::<ListAnswer>();
    let _ = generator.subschema_for::<ActAnswer>();
    let _ = generator.subschema_for::<ApiAnswer>();
    let _ = generator.subschema_for::<AddAnswer>();
    let _ = generator.subschema_for::<PairAnswer>();
    let _ = generator.subschema_for::<ErrorAnswer>();
    let _ = generator.subschema_for::<VideoLookupAnswer>();
    let _ = generator.subschema_for::<RecentAnswer>();
    let _ = generator.subschema_for::<RecentActAnswer>();
    let defs = generator.take_definitions(true);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://misterdownloadmanager.com/automation-api.schema.json",
        "title": "Mister Download Manager automation API",
        "description": "One JSON line to the port in ipc.json (on 127.0.0.1), one line back. \
            A request is `Request`. A command (an address to add, start_queue) is answered \
            `ok`; a question is answered with the JSON object for it (`ListAnswer`, \
            `ActAnswer`, `ApiAnswer`) or an `ErrorAnswer`. A wrong token is answered `denied`, \
            an app that doesn't answer within 2 seconds `timeout`. The browser's quality menu \
            (`video_lookup`, `video_poll`, `video_pick`, sent by its native host) is answered \
            with a `VideoLookupAnswer`; the browser popup's list (`recent`, `recent_act`, \
            sent by its native host too, #1496) with a `RecentAnswer` or a `RecentActAnswer`. \
            Guide: https://misterdownloadmanager.com/help/command-line.html#the-automation-api",
        "$ref": request.get("$ref").cloned().unwrap_or(serde_json::Value::Null),
        "$defs": defs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_file_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/automation-api.schema.json");
        let made = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("MDM_WRITE_SCHEMA").is_some() {
            std::fs::write(&path, &made).unwrap();
        }
        let file = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            file == made,
            "docs/automation-api.schema.json is out of date: MDM_WRITE_SCHEMA=1 cargo test -p mdm-ipc schema"
        );
    }

    #[test]
    fn errors_and_answers_read_back() {
        let off = r#"{"error":"the automation API is off","code":"api-off"}"#;
        let e = parse::<ListAnswer>(off).unwrap().unwrap_err();
        assert_eq!(e.code.as_deref(), Some(API_OFF));
        let list = parse::<ListAnswer>(r#"{"downloads":[],"speed":5}"#)
            .unwrap()
            .unwrap();
        assert_eq!(list.speed, 5);
        assert!(parse::<ListAnswer>("ok").is_err());
    }

    /// The quality menu's answers as the browser's host reads them: only what each state has,
    /// a size unknown said as null, and back the same.
    #[test]
    fn video_lookup_answers_round_trip() {
        assert_eq!(
            line(&VideoLookupAnswer::pending("abc")),
            r#"{"state":"pending","token":"abc"}"#
        );
        assert_eq!(
            line(&VideoLookupAnswer::of(lookup::USE_WINDOW)),
            r#"{"state":"use-window"}"#
        );
        assert_eq!(
            line(&VideoLookupAnswer::failed(lookup::NO_YTDLP, None)),
            r#"{"state":"failed","reason":"no-ytdlp"}"#
        );
        let ready = VideoLookupAnswer {
            token: Some("abc".into()),
            title: Some("A talk".into()),
            choices: vec![
                VideoChoice {
                    label: "720p mp4".into(),
                    ext: "mp4".into(),
                    size: Some(187_500_000),
                    audio: false,
                    ..Default::default()
                },
                VideoChoice {
                    label: "Audio only mp3".into(),
                    ext: "mp3".into(),
                    size: None,
                    audio: true,
                    ..Default::default()
                },
            ],
            ..VideoLookupAnswer::of(lookup::READY)
        };
        let json = line(&ready);
        assert_eq!(
            json,
            r#"{"state":"ready","token":"abc","title":"A talk","choices":[{"label":"720p mp4","ext":"mp4","size":187500000,"audio":false},{"label":"Audio only mp3","ext":"mp3","size":null,"audio":true}]}"#
        );
        assert_eq!(
            serde_json::from_str::<VideoLookupAnswer>(&json).unwrap(),
            ready
        );
        // An older app's choices, without `audio`: none is sound alone.
        let old: VideoLookupAnswer = serde_json::from_str(
            r#"{"state":"ready","choices":[{"label":"Audio only mp3","ext":"mp3","size":null}]}"#,
        )
        .unwrap();
        assert!(!old.choices[0].audio);
        let failed = VideoLookupAnswer::failed(lookup::ERROR, Some("Video unavailable".into()));
        assert_eq!(
            serde_json::from_str::<VideoLookupAnswer>(&line(&failed)).unwrap(),
            failed
        );
        // Not one of these (an error answer, an older app's `ok`): no state to read.
        let other: VideoLookupAnswer =
            serde_json::from_str(r#"{"error":"the automation API is off"}"#).unwrap();
        assert!(other.state.is_empty());
        assert!(serde_json::from_str::<VideoLookupAnswer>("ok").is_err());
    }

    fn row(id: u64, file: &str, status: &str) -> DownloadInfo {
        DownloadInfo {
            id,
            file: file.into(),
            url: format!("https://files.example.com/{file}"),
            status: status.into(),
            ..Default::default()
        }
    }

    /// The popup's list (#1496): every word of the search in the name or the address, the
    /// filter, newest first, the limit; counts of each filter for the search; the running ones
    /// whatever the search.
    #[test]
    fn recent_view_filters_counts_and_limits() {
        let all = vec![
            row(1, "Report final.pdf", "Complete"),
            row(2, "report draft.docx", "Error"),
            row(3, "holiday.mp4", "Downloading"),
            row(4, "report-2.zip", "Downloading"),
            row(5, "notes.txt", "Stopped"),
            row(6, "report 3.pdf", "Complete"),
        ];
        let ask = |filter: &str, search: &str, limit: usize| crate::RecentAsk {
            filter: filter.into(),
            search: search.into(),
            limit,
            ..Default::default()
        };
        let ids = |a: &RecentAnswer| a.downloads.iter().map(|d| d.id).collect::<Vec<_>>();
        let every = RecentAnswer::of(all.clone(), &ask("all", "", 50), 9);
        assert_eq!(ids(&every), [6, 5, 4, 3, 2, 1]);
        assert_eq!(
            every.counts,
            RecentCounts {
                all: 6,
                running: 2,
                done: 2,
                failed: 1
            }
        );
        assert_eq!((every.running, every.speed), (2, 9));
        let reports = RecentAnswer::of(all.clone(), &ask("done", "REPORT", 50), 0);
        assert_eq!(ids(&reports), [6, 1]);
        assert_eq!(
            reports.counts,
            RecentCounts {
                all: 4,
                running: 1,
                done: 2,
                failed: 1
            }
        );
        assert_eq!(reports.running, 2, "the toolbar's count ignores the search");
        // Every word, in the name or the address.
        let two = RecentAnswer::of(all.clone(), &ask("all", "report files.example", 50), 0);
        assert_eq!(ids(&two), [6, 4, 2, 1]);
        assert!(
            ids(&RecentAnswer::of(
                all.clone(),
                &ask("all", "report mp4", 50),
                0
            ))
            .is_empty()
        );
        // Running, failed; an unknown filter is all; the limit, at most RECENT_MAX.
        assert_eq!(
            ids(&RecentAnswer::of(all.clone(), &ask("running", "", 50), 0)),
            [4, 3]
        );
        assert_eq!(
            ids(&RecentAnswer::of(all.clone(), &ask("failed", "", 50), 0)),
            [2]
        );
        assert_eq!(
            ids(&RecentAnswer::of(all.clone(), &ask("paused", "", 2), 0)),
            [6, 5]
        );
        let none = RecentAnswer::of(all, &ask("all", "", 0), 0);
        assert!(none.downloads.is_empty());
        assert_eq!((none.counts.all, none.running), (6, 2));
        let many: Vec<DownloadInfo> = (1..=500).map(|i| row(i, "a.bin", "Complete")).collect();
        let capped = RecentAnswer::of(many, &ask("all", "", 10_000), 0);
        assert_eq!(capped.downloads.len(), crate::RECENT_MAX);
        assert_eq!(capped.downloads[0].id, 500);
    }

    /// Within the budget whatever the addresses: the oldest rows go, and their icons with them
    /// when no other row has the type; the newest stay.
    #[test]
    fn recent_answer_fits_the_native_message() {
        let long = "x".repeat(8000);
        let mut rows = Vec::new();
        for id in 1..=200u64 {
            let old = id <= 10;
            let mut d = row(id, if old { "old.iso" } else { "new.zip" }, "Complete");
            d.url = format!("https://files.example.com/{long}/{id}");
            d.icon = Some(if old { "iso" } else { "zip" }.into());
            rows.push(d);
        }
        let ask = crate::RecentAsk {
            limit: 200,
            ..Default::default()
        };
        let mut answer = RecentAnswer::of(rows, &ask, 0);
        let png = |c: &str| format!("data:image/png;base64,{}", c.repeat(3000));
        answer.icons.insert("iso".into(), png("A"));
        answer.icons.insert("zip".into(), png("B"));
        assert!(line(&answer).len() > RECENT_BUDGET);
        answer.fit(RECENT_BUDGET);
        let size = line(&answer).len();
        assert!(size <= RECENT_BUDGET, "{size}");
        assert!(
            size > RECENT_BUDGET - 9000,
            "only what had to go went: {size}"
        );
        assert_eq!(answer.downloads[0].id, 200);
        assert!(answer.downloads.iter().all(|d| d.id > 10));
        assert!(answer.icons.contains_key("zip") && !answer.icons.contains_key("iso"));
        // What fits stays whole.
        let mut small = RecentAnswer::of(vec![row(1, "a.zip", "Complete")], &ask, 0);
        let before = small.clone();
        small.fit(RECENT_BUDGET);
        assert_eq!(small, before);
    }

    /// The new fields of a download are left out when they say nothing, so the automation API's
    /// lines stay as they were for a download that has none.
    #[test]
    fn download_info_leaves_empty_fields_out() {
        let json = line(&row(1, "a.zip", "Queued"));
        for field in ["left", "exists", "finished", "kind", "redownload", "icon"] {
            assert!(!json.contains(&format!("\"{field}\"")), "{json}");
        }
        let full = DownloadInfo {
            left: Some(12),
            exists: Some(false),
            finished: Some(1_800_000_000),
            kind: Some("torrent".into()),
            redownload: true,
            icon: Some("zip".into()),
            ..row(1, "a.zip", "Complete")
        };
        let back: DownloadInfo = serde_json::from_str(&line(&full)).unwrap();
        assert_eq!(back, full);
        assert_eq!(
            line(&RecentActAnswer::not(recent_why::MISSING)),
            r#"{"done":false,"why":"missing"}"#
        );
        assert_eq!(line(&RecentActAnswer::done()), r#"{"done":true}"#);
    }

    #[test]
    fn list_filtering_works() {
        let list = ListAnswer {
            downloads: vec![
                DownloadInfo {
                    id: 1,
                    file: "test1.zip".into(),
                    url: "https://example.com/test1.zip".into(),
                    status: "Downloading".into(),
                    category: "Compressed".into(),
                    ..Default::default()
                },
                DownloadInfo {
                    id: 2,
                    file: "song.mp3".into(),
                    url: "https://example.com/song.mp3".into(),
                    status: "Complete".into(),
                    category: "Music".into(),
                    ..Default::default()
                },
                DownloadInfo {
                    id: 3,
                    file: "video.mp4".into(),
                    url: "https://example.com/video.mp4".into(),
                    status: "Downloading".into(),
                    category: "Video".into(),
                    ..Default::default()
                },
            ],
            queues: vec![QueueInfo {
                name: "Main download queue".into(),
                running: true,
                ids: vec![1, 3],
            }],
            categories: vec![
                CategoryInfo {
                    key: "Compressed".into(),
                    name: "Compressed".into(),
                },
                CategoryInfo {
                    key: "Music".into(),
                    name: "Music".into(),
                },
            ],
            speed: 1234,
            free: Some(50000000000),
        };

        // Filter by id
        let by_id = list
            .filtered(&ListFilter {
                id: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_id.downloads.len(), 1);
        assert_eq!(by_id.downloads[0].id, 2);

        // Filter by status (case-insensitive)
        let by_status = list
            .filtered(&ListFilter {
                status: Some("downloading"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_status.downloads.len(), 2);
        assert_eq!(by_status.downloads[0].id, 3);
        assert_eq!(by_status.downloads[1].id, 1);

        // Filter by search
        let by_search = list
            .filtered(&ListFilter {
                search: Some("SONG"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_search.downloads.len(), 1);
        assert_eq!(by_search.downloads[0].id, 2);

        // Filter by category key or name
        let by_cat = list
            .filtered(&ListFilter {
                category: Some("music"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_cat.downloads.len(), 1);
        assert_eq!(by_cat.downloads[0].id, 2);

        // Filter by queue
        let by_queue = list
            .filtered(&ListFilter {
                queue: Some("main download queue"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_queue.downloads.len(), 2);
        assert_eq!(by_queue.downloads[0].id, 3);
        assert_eq!(by_queue.downloads[1].id, 1);

        // Limit
        let limited = list
            .filtered(&ListFilter {
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.downloads.len(), 1);
        assert_eq!(limited.downloads[0].id, 3);

        // Errors
        assert!(matches!(
            list.filtered(&ListFilter {
                queue: Some("missing"),
                ..Default::default()
            }),
            Err(FilterError::UnknownQueue { .. })
        ));
        assert!(matches!(
            list.filtered(&ListFilter {
                status: Some("paused"),
                ..Default::default()
            }),
            Err(FilterError::UnknownStatus { .. })
        ));
    }
}
