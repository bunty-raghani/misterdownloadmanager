//! Download list items, IDM's default categories, and IDM-style formatting.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mdm_engine::Download;
use mdm_engine::download::State;
use serde::{Deserialize, Serialize};
use url::Url;

/// IDM's default file categories (notes/research/idm-ui-and-integration.md, notes/UI.md), our
/// Torrents (D-021) and the user's own (D-021: Options → Save to → New, the Categories
/// right-click menu). Built-in ones are saved by name, so lists from before load unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    General,
    Compressed,
    Documents,
    Music,
    Programs,
    Video,
    /// Every torrent download (not by file type): its own folder, `Downloads\Torrents`.
    Torrents,
    /// The user's category with this id (`Settings::custom_categories`).
    Custom(u32),
}

/// A category the user made: its name, file types (`MP4 R0*`…, as Options lists them) and
/// folder (None: `Downloads\<name>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCategory {
    pub id: u32,
    pub name: String,
    pub types: String,
    pub folder: Option<String>,
}

/// The user's categories as the settings hold them, for code that only has a `Category` (its
/// label, its file types). Set at start and whenever they change (`set_custom`).
static CUSTOM: std::sync::RwLock<Vec<CustomCategory>> = std::sync::RwLock::new(Vec::new());

pub fn set_custom(list: &[CustomCategory]) {
    // The rules' lock first, as `rules` takes it: nobody matches a name against half a change.
    let mut rules = RULES.write().unwrap();
    let mut custom = CUSTOM.write().unwrap();
    if *custom != list {
        *custom = list.to_vec();
        *rules = None;
        CHANGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn custom(id: u32) -> Option<CustomCategory> {
    CUSTOM.read().unwrap().iter().find(|c| c.id == id).cloned()
}

/// A built-in category the user changed (#93): another name, other file types, or deleted.
/// What it doesn't say stays as shipped (its folder lives in `Settings::category_dirs`, under
/// the built-in's `key`, as before).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinEdit {
    pub category: Category,
    /// The user's name for it, kept as typed (None: the shipped name, translated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its file types, as Options lists them (`MP3 FLAC`; None: the shipped ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<String>,
    /// Deleted: not in the Categories pane, Options or Download File Info, and no file goes to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

/// The built-in categories' edits as the settings hold them; set at start and whenever they
/// change (`set_builtin`), like `CUSTOM`.
static BUILTIN: std::sync::RwLock<Vec<BuiltinEdit>> = std::sync::RwLock::new(Vec::new());

pub fn set_builtin(list: &[BuiltinEdit]) {
    let mut rules = RULES.write().unwrap();
    let mut builtin = BUILTIN.write().unwrap();
    if *builtin != list {
        *builtin = list.to_vec();
        *rules = None;
        CHANGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The file types each category takes, in the order [`Category::of`] tries them: the user's
/// categories, then the built-in ones not deleted. Worked out from `CUSTOM` and `BUILTIN` once,
/// and again after they change (`None` until then): `of` runs for each link of a list of
/// thousands, and listing every category's types again for each name held the window (#1063).
type Rules = Vec<(Category, Vec<String>)>;
static RULES: std::sync::RwLock<Option<Rules>> = std::sync::RwLock::new(None);

/// How many times the categories changed (`set_custom`, `set_builtin`): what was worked out
/// from them (a link's category) is worked out again when this moves.
static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`CHANGES`].
pub fn categories_changed() -> u64 {
    CHANGES.load(std::sync::atomic::Ordering::Relaxed)
}

/// `f` with the rules, worked out first if the categories changed since.
fn rules<R>(f: impl FnOnce(&Rules) -> R) -> R {
    if let Some(rules) = RULES.read().unwrap().as_ref() {
        return f(rules);
    }
    let mut rules = RULES.write().unwrap();
    f(rules.get_or_insert_with(|| {
        let custom: Vec<Category> = CUSTOM
            .read()
            .unwrap()
            .iter()
            .map(|c| Category::Custom(c.id))
            .collect();
        // A deleted built-in takes no file: its types go to another category, or to General.
        custom
            .into_iter()
            .chain(Category::BUILT_IN.into_iter().filter(|c| !c.deleted()))
            .map(|c| (c, c.extensions()))
            .collect()
    }))
}

fn builtin(cat: Category) -> Option<BuiltinEdit> {
    BUILTIN
        .read()
        .unwrap()
        .iter()
        .find(|e| e.category == cat)
        .cloned()
}

/// `types` (`MP3 *.flac, R0*`) as a list of lower-case patterns.
fn parse_types(types: &str) -> Vec<String> {
    types
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|t| t.trim_start_matches("*.").trim_start_matches('.'))
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Does `ext` match a file-type pattern (`mp4`, `r0*`), in any case?
fn type_matches(pattern: &str, ext: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => ext
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix)),
        None => ext.eq_ignore_ascii_case(pattern),
    }
}

impl Category {
    /// IDM's named categories, then Torrents.
    pub const BUILT_IN: [Category; 6] = [
        Category::Compressed,
        Category::Documents,
        Category::Music,
        Category::Programs,
        Category::Video,
        Category::Torrents,
    ];

    /// The named categories: the built-in ones not deleted, then the user's (as in the
    /// Categories tree).
    pub fn named() -> Vec<Category> {
        let custom = CUSTOM.read().unwrap();
        Category::BUILT_IN
            .into_iter()
            .filter(|c| !c.deleted())
            .chain(custom.iter().map(|c| Category::Custom(c.id)))
            .collect()
    }

    /// A built-in category the user deleted (#93).
    pub fn deleted(self) -> bool {
        !self.is_custom() && builtin(self).is_some_and(|e| e.deleted)
    }

    /// Can its name and file types be changed, and can it be deleted? The user's categories and
    /// the built-in ones by file type; not General (where everything else goes) nor Torrents
    /// (by source, not by file type).
    pub fn editable(self) -> bool {
        !matches!(self, Category::General | Category::Torrents)
    }

    /// The name settings and folders know it by, whatever the user renamed it to: a built-in
    /// one's English name (`Downloads\Music`, Options' stored folder), the user's for theirs.
    pub fn key(self) -> std::borrow::Cow<'static, str> {
        match self {
            Category::Custom(_) => self.label(),
            c => std::borrow::Cow::Borrowed(c.shipped_name()),
        }
    }

    /// A built-in category's name as shipped (English; shown translated).
    fn shipped_name(self) -> &'static str {
        match self {
            // i18n: category name: files of no particular kind
            Category::General => crate::i18n::mark("General"),
            // i18n: category name: compressed archives (zip, rar, 7z)
            Category::Compressed => crate::i18n::mark("Compressed"),
            // i18n: category name: documents (pdf, doc…)
            Category::Documents => crate::i18n::mark("Documents"),
            // i18n: category name: audio files
            Category::Music => crate::i18n::mark("Music"),
            // i18n: category name: programs and installers (exe, msi)
            Category::Programs => crate::i18n::mark("Programs"),
            // i18n: category name: video files
            Category::Video => crate::i18n::mark("Video"),
            // i18n: category name: torrent downloads (BitTorrent)
            Category::Torrents => crate::i18n::mark("Torrents"),
            Category::Custom(_) => "",
        }
    }

    /// The name as shown, in the interface language (the user's names as typed).
    pub fn shown(self) -> String {
        match self {
            Category::Custom(_) => self.label().into_owned(),
            c => match builtin(c).and_then(|e| e.name) {
                Some(name) => name,
                None => c.shipped_shown(),
            },
        }
    }

    /// A built-in category's shipped name in the interface language.
    pub fn shipped_shown(self) -> String {
        crate::i18n::t(self.shipped_name()).to_owned()
    }

    /// Every category: General first.
    pub fn all() -> Vec<Category> {
        std::iter::once(Category::General)
            .chain(Category::named())
            .collect()
    }

    /// The name: English for built-in ones as shipped (shown translated, `t`), the user's for
    /// theirs and for a built-in one they renamed. Settings and folders use [`Self::key`].
    pub fn label(self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow::{Borrowed, Owned};
        match self {
            Category::Custom(id) => Owned(custom(id).map_or_else(|| "?".to_owned(), |c| c.name)),
            c => match builtin(c).and_then(|e| e.name) {
                Some(name) => Owned(name),
                None => Borrowed(c.shipped_name()),
            },
        }
    }

    /// The user's own category (editable, deletable)?
    pub fn is_custom(self) -> bool {
        matches!(self, Category::Custom(_))
    }

    fn extensions(self) -> Vec<String> {
        if let Some(types) = builtin(self).and_then(|e| e.types) {
            return parse_types(&types);
        }
        self.shipped_extensions()
    }

    /// A built-in category's file types as shipped (the user's: theirs).
    fn shipped_extensions(self) -> Vec<String> {
        let fixed: &[&str] = match self {
            Category::General | Category::Torrents => &[],
            Category::Compressed => &[
                "zip", "rar", "r0*", "r1*", "arj", "gz", "sit", "sitx", "sea", "ace", "bz2", "7z",
            ],
            Category::Documents => &["doc", "pdf", "ppt", "pps", "docx", "pptx"],
            Category::Music => &[
                "mp3", "wav", "wma", "mpa", "ram", "ra", "aac", "aif", "m4a", "tsa",
            ],
            Category::Programs => &["exe", "msi"],
            Category::Video => &[
                "avi", "mpg", "mpe", "mpeg", "asf", "wmv", "mov", "qt", "rm", "mp4", "flv", "m4v",
                "webm", "ogv", "ogg", "mkv", "ts", "tsv",
            ],
            Category::Custom(id) => {
                return custom(id)
                    .map(|c| parse_types(&c.types))
                    .unwrap_or_default();
            }
        };
        fixed.iter().map(|e| (*e).to_owned()).collect()
    }

    /// A built-in category's file types as shipped, as Options writes them (`ZIP RAR R0*`).
    pub fn shipped_types_text(self) -> String {
        upper_list(&self.shipped_extensions())
    }

    /// Options → Save to: "Automatically put in this category the following file types".
    pub fn types_text(self) -> String {
        match self {
            Category::General => {
                crate::i18n::t("The file types that are not listed in any other category").into()
            }
            Category::Torrents => {
                crate::i18n::t("Torrent downloads (magnet links and .torrent files)").into()
            }
            c => upper_list(&c.extensions()),
        }
    }

    /// The category a file name falls into: the user's categories first (that is why they made
    /// one), then IDM's; Torrents only by source, never by name.
    /// Nothing is listed or copied for a name: the categories' types are worked out once
    /// ([`RULES`], #1063).
    pub fn of(file_name: &str) -> Category {
        let Some((_, ext)) = file_name.rsplit_once('.') else {
            return Category::General;
        };
        rules(|rules| {
            rules
                .iter()
                .find(|(_, types)| types.iter().any(|p| type_matches(p, ext)))
                .map_or(Category::General, |(c, _)| *c)
        })
    }

    /// [`Self::of`] as it was before #1063 (every category's types listed again for each
    /// name), for the tests that show both give the same answers.
    #[cfg(test)]
    pub fn of_as_before(file_name: &str) -> Category {
        let Some((_, ext)) = file_name.rsplit_once('.') else {
            return Category::General;
        };
        let ext = ext.to_ascii_lowercase();
        let type_matches = |pattern: &str, ext: &str| {
            let pattern = pattern.to_ascii_lowercase();
            match pattern.strip_suffix('*') {
                Some(prefix) => ext.starts_with(prefix),
                None => ext == pattern,
            }
        };
        let custom: Vec<Category> = CUSTOM
            .read()
            .unwrap()
            .iter()
            .map(|c| Category::Custom(c.id))
            .collect();
        custom
            .into_iter()
            .chain(Category::BUILT_IN.into_iter().filter(|c| !c.deleted()))
            .find(|c| c.extensions().iter().any(|p| type_matches(p, &ext)))
            .unwrap_or(Category::General)
    }

    /// Default folder: `Downloads\` for General, `Downloads\<Category>\` otherwise (a user's
    /// category: its own folder when it has one).
    pub fn folder(self, downloads: &Path) -> PathBuf {
        match self {
            Category::Custom(id) => match custom(id) {
                Some(CustomCategory {
                    folder: Some(f), ..
                }) => PathBuf::from(f),
                _ => self.default_folder(downloads),
            },
            c => c.default_folder(downloads),
        }
    }

    /// The folder it has when none was chosen: `Downloads\<name>` (General: `Downloads\`).
    pub fn default_folder(self, downloads: &Path) -> PathBuf {
        match self {
            Category::General => downloads.to_owned(),
            Category::Custom(id) => match custom(id) {
                Some(c) => downloads.join(mdm_engine::naming::sanitize(&c.name)),
                None => downloads.to_owned(),
            },
            // The shipped name, whatever it is called now: renaming doesn't move a folder.
            c => downloads.join(c.key().as_ref()),
        }
    }
}

/// `["zip", "r0*"]` as Options writes it: `ZIP R0*`.
fn upper_list(exts: &[String]) -> String {
    exts.iter()
        .map(|e| e.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The base folder that categories without their own folder live under (#556): General's
/// folder when one was chosen in Options → Save to, else `downloads`.
pub fn default_downloads<'a>(
    downloads: &'a Path,
    dirs: &'a [(String, String)],
) -> std::borrow::Cow<'a, Path> {
    let general = Category::General.key();
    dirs.iter()
        .find(|(c, _)| *c == *general)
        .map(|(_, d)| std::borrow::Cow::Owned(crate::dialogs::resolve_path(d)))
        .unwrap_or(std::borrow::Cow::Borrowed(downloads))
}

/// The name a download from `context` has before its server answers, when nobody named it: the
/// address's ([`mdm_engine::naming::answer_name`]), typed by the browser's own type for it and,
/// for a page's media whose address names nothing (a script, a bare id), its page's title
/// (#695, #800). The engine names it again from the server's answer as it starts.
pub fn browser_guess(url: &url::Url, context: &mdm_ipc::Command) -> String {
    mdm_engine::naming::answer_name(
        url,
        &mdm_engine::naming::Answer {
            content_type: context.mime.as_deref(),
            title: context.video_title.as_deref(),
            ..Default::default()
        },
    )
}

/// The category's folder: the one chosen in Options → Save to (or remembered from Download
/// File Info), else the default under [`default_downloads`] (#556).
pub fn category_folder(cat: Category, downloads: &Path, dirs: &[(String, String)]) -> PathBuf {
    let base = default_downloads(downloads, dirs);
    if cat.is_custom() {
        return cat.folder(&base);
    }
    let key = cat.key();
    dirs.iter()
        .find(|(c, _)| *c == *key)
        // A folder may be saved as shown, relative to the user's home.
        .map(|(_, d)| crate::dialogs::resolve_path(d))
        .unwrap_or_else(|| cat.folder(&base))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Added with "Download Later", never started.
    Queued,
    Downloading,
    /// Stopped by the user; resumable from its state file.
    Stopped,
    Complete,
    Error,
}

/// One row of the download list. The persisted part is everything but `live`.
#[derive(Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub url: Url,
    pub file_name: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub status: Status,
    pub category: Category,
    pub description: String,
    pub last_try: Option<String>,
    pub error: Option<String>,
    pub resumable: Option<bool>,
    /// The hash the finished file must have (Verify checksum…; beyond IDM, D-015).
    #[serde(default)]
    pub checksum: Option<mdm_engine::verify::Hash>,
    /// When the download was added ("Added" column), as `Last Try Date` is written. Downloads
    /// added before the column existed have none.
    #[serde(default)]
    pub added: Option<String>,
    /// Its own speed limit, bytes/s (progress window → Speed Limiter, "Remember Speed Limiter
    /// settings for this file on download stop/resume"): used again whenever it starts.
    #[serde(default)]
    pub own_limit: Option<u64>,
    /// A video from a page, downloaded by yt-dlp (its `url` is the page).
    #[serde(default)]
    pub video: Option<VideoSpec>,
    /// A torrent or magnet (M15): its `url` is a magnet link, `dest` its folder or one file.
    #[serde(default)]
    pub torrent: Option<crate::torrents::TorrentSpec>,
    /// An image gallery downloaded by gallery-dl (#58): its `url` is the gallery's page, `dest`
    /// the folder its files go into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gallery: Option<GallerySpec>,
    /// The event log: (unix seconds, what happened), oldest first, the last `LOG` entries.
    #[serde(default)]
    pub log: Vec<(i64, String)>,
    /// `file_name` is only the address's guess (nobody named it: no browser, no answer, no
    /// user): the download starts into its folder, under the name the server gives (its
    /// Content-Disposition), as a browser does (#28). Cleared once the file is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub name_from_url: bool,
    /// Its name was given without waiting for its folder to be looked at (Download File Info:
    /// a sleeping drive or a network folder can take seconds to answer, #743): nothing of its
    /// own is on the disk yet, and it starts under that name or the next free numbered one, as
    /// the engine finds as it starts (`Destination::Free`). Cleared once the file is made. A
    /// video's (Download video's name, given before its folder was read) is numbered on past the
    /// files there beside the window, before it starts (videoplace.rs); cleared then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub number_at_start: bool,
    /// The file the user chose to replace with this download (#1088: Download File Info's Save
    /// dialog asked "Replace?" and was answered Yes). While the download's place is that file
    /// ([`Item::replaces`]), the engine replaces it as the download completes, never before
    /// (`Options::replace_existing`): one that fails or is cancelled leaves it as it was. Cleared
    /// once complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_file: Option<PathBuf>,
    /// The Link grabber package it came in (#37): its archives are extracted together once
    /// every volume is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageTag>,
    /// The downloads it was added with (a playlist, a batch, Link grabber's links, #766): the
    /// first one's id. They show one summary once all are done, not a Download complete window
    /// each; cleared once it was summed up. None for a download added alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u64>,
    /// Where its server was, as its last run saw it (#140): Properties shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<crate::whereis::Seen>,
    /// Downloaded directly, never through a debrid account (#44): the user's choice (Download
    /// File Info, File Properties), or the browser had already started it (the host was sending
    /// the file). Kept, so a download goes the same way after a restart.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub direct: bool,
    /// A torrent through a debrid account (#44): its `url` is the magnet link, `dest` its folder
    /// (or its one file, once known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteSpec>,
    /// Safe deferred main name chosen in the progress window: applied in place once the
    /// download reaches `Status::Complete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_on_complete: Option<String>,
    /// True when a folder download's destination already held files before MDM added it, so
    /// folder-level deferred rename refuses to sweep untracked files along.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preexisting_folder: bool,
    /// Its file was dragged out of Download complete and moved to a place MDM couldn't learn
    /// (#869): Open and Open folder say so while it isn't at `dest`. Cleared when it is placed
    /// again (a move MDM follows, Move/Rename) or downloaded again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved_away: bool,
    /// The page it was found on, as the browser sent it (its Referer): kept with the list, so
    /// File Properties shows it and a start after a restart sends it again (#795). The request's
    /// own copy is `context.referer`; this one follows it (`set_referer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    /// The web page it was started from in the browser, whole (#607), where its Referer is
    /// often the site's address only. Refresh download address opens it; kept with the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// It waits for a new address, as IDM's downloads do after Refresh download address: its last
    /// run stopped because its address no longer gives the file (a signed link expired, 403, 404,
    /// 410: the engine's `Error::wants_new_address`), or the user chose Refresh download address.
    /// The browser's next download of the same file (`crate::freshlink`) gives it its address and
    /// it goes on, instead of a new row starting over. Kept with the list (a link that expired
    /// overnight is renewed the next day); cleared as it starts again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waits_for_address: bool,
    /// It was made from a private (incognito) window's download. Saved (with Options → General →
    /// "Keep private-window downloads in the list"), unlike `context.private`, which is memory
    /// only: read back, the row stays apart from normal windows' downloads (it neither takes
    /// nor gives a fresh address, `crate::freshlink`), never starts waiting by itself, and a
    /// video never falls back to the normal browser's cookie store.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private_window: bool,
    /// A video's (its `url` is the page): the address its file came from, the first stream of
    /// its last run as yt-dlp gave it to MDM's engine (#795). The windows' "Address".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_url: Option<Url>,
    #[serde(skip)]
    pub live: Option<Live>,
    /// Bytes/s for a download the app itself receives (Telegram media streamed by its page
    /// into `media.rs`): there is no engine job to ask. Memory only.
    #[serde(skip)]
    pub stream_rate: f64,
    /// The session's temporary view while a torrent is being checked; never saved.
    #[serde(skip)]
    pub torrent_check: Option<(u64, u64)>,
    /// The engine's last view of this download (segments, connections).
    #[serde(skip)]
    pub snap: Option<mdm_engine::Snapshot>,
    /// "Use authorization" login; kept in memory only, never written to disk.
    #[serde(skip)]
    pub login: Option<(String, String)>,
    /// Request context from the browser (Referer, Cookie, User-Agent); memory only.
    #[serde(skip)]
    pub context: mdm_ipc::Command,
    /// What the Download complete window says besides (a video's subtitles the site refused,
    /// #68), in the interface language; memory only.
    #[serde(skip)]
    pub notices: Vec<String>,
    /// A video chosen in the browser's quality menu: the menu's look-up, for its first run only
    /// (taken then); memory only, never written (its links are signed).
    #[serde(skip)]
    pub preloaded: Option<mdm_tools::video::Preloaded>,
    /// A finished video's subtitles saved beside it (#68): a rename on completion takes them
    /// along (#655). Memory only.
    #[serde(skip)]
    pub(crate) side_files: Vec<PathBuf>,
    /// Files known to belong to a finished gallery or debrid folder download, kept across
    /// restarts so a requeued or resumed folder rename still checks for untracked files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) written_files: Option<Vec<PathBuf>>,
    /// Whether its resume state is at its place, as read beside the window at start
    /// (resumeread.rs): the place read and the answer. Until that read comes it is taken as there
    /// (the safe side: the row keeps its place, and no other row takes it), and it is forgotten as
    /// the row starts (its run makes or removes the state). Memory only.
    #[serde(skip)]
    pub(crate) state_here: Option<(PathBuf, bool)>,
    /// A copy ([`Item::place_copy`]) of a row that was running: it holds its place as the row
    /// does ([`Item::holds_place`]). Memory only.
    #[serde(skip)]
    pub(crate) live_copy: bool,
}

/// Adds `more` to a download's `written_files`, each path once, in order. A gallery lists up to
/// 100 000 files: each told from those listed at once, not searched for among them (time
/// squared, on the window's thread).
fn add_written(list: &mut Vec<PathBuf>, more: Vec<PathBuf>) {
    let mut listed: std::collections::HashSet<PathBuf> = list.iter().cloned().collect();
    for p in more {
        if !listed.contains(&p) {
            listed.insert(p.clone());
            list.push(p);
        }
    }
}

/// A Link grabber package, as each of its downloads keeps it (#37).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTag {
    /// The same for every download of the package.
    pub id: u64,
    pub name: String,
    /// Extract its archives when they are all downloaded.
    pub extract: bool,
    /// Then move them to the Recycle Bin (the Trash).
    pub delete_after: bool,
    /// The archives' password, typed in Link grabber: memory only, never written to disk.
    #[serde(skip)]
    pub password: Option<String>,
}

/// What runs a download: the engine (a file), yt-dlp (a video from a page), gallery-dl (an
/// image gallery, #58), or librqbit (a torrent).
pub enum Job {
    File(Download),
    Video {
        job: mdm_tools::video::VideoJob,
        url: Url,
    },
    Gallery {
        job: mdm_tools::gallery::GalleryJob,
        url: Url,
    },
    Torrent(crate::torrents::Run),
    /// A torrent through a debrid account, then its files over HTTP (#44).
    Remote(mdm_debrid::torrents::RemoteTorrent),
}

/// A torrent sent to a debrid account (#44).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSpec {
    /// The account it went to (Options → Accounts).
    pub account: u64,
    /// The service's id for it, once sent: a restart continues there, nothing is sent twice.
    #[serde(default)]
    pub transfer: Option<String>,
    /// The folder its files go into (a torrent of several files gets its own folder there).
    pub folder: PathBuf,
    /// Custom file or folder name chosen in Add torrent (#517), when renamed from the torrent's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed: Option<String>,
}

/// A gallery download's choice (#58).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GallerySpec {
    /// At most this many files (the first the site lists, on most the newest); none: all.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Why a video failed when joining its picture and sound did (ffmpeg's `error`, in English),
/// and what became of what was downloaded: `kept` beside its file, for Resume to join again
/// without downloading it, until the download is deleted with its files.
fn merge_failure(error: &str, kept: &[PathBuf]) -> String {
    let error = crate::i18n::ltr(error);
    if kept.is_empty() {
        return crate::i18n::fmt(
            // i18n: why a video download failed: ffmpeg couldn't join its picture and sound into one file; {error} = ffmpeg's reason, in English
            crate::i18n::t("its picture and sound couldn't be joined into one file ({error})"),
            &[("error", &error)],
        );
    }
    let files = kept
        .iter()
        .filter_map(|f| f.file_name())
        .map(|n| crate::i18n::isolate(&n.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(", ");
    crate::i18n::fmt(
        // i18n: why a video download failed: ffmpeg couldn't join its picture and sound into one file; {error} = ffmpeg's reason, in English; {files} = the downloaded parts' file names, kept in the download folder; "Also delete the file from disk" is the Delete box's choice
        crate::i18n::t(
            "its picture and sound couldn't be joined into one file ({error}). What was downloaded stays in the folder for Resume to join again ({files}); deleting the download with \"Also delete the file from disk\" removes it",
        ),
        &[("error", &error), ("files", &files)],
    )
}

/// `text` with every web address in it cut before its query and fragment (`?…`, `#…`): what a
/// log may keep of a download's failure (#1262), since a log is attached to reports and an
/// address's query often carries a token.
fn without_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = ["https://", "http://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        out.push_str(&rest[..at]);
        let addr = &rest[at..];
        let end = addr
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ']'))
            .unwrap_or(addr.len());
        let (url, after) = addr.split_at(end);
        match url.find(['?', '#']) {
            Some(q) => {
                out.push_str(&url[..q]);
                out.push_str("?…");
            }
            None => out.push_str(url),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// A helper's progress (yt-dlp's, gallery-dl's) as the engine's view: one segment when its size
/// is known, one connection (theirs: neither opens more), its notes as events. A video whose
/// streams MDM's engine downloads (#769) shows the engine's own: the streams' segments one after
/// another, the connections of the one under way, its server.
fn tool_snapshot(p: mdm_tools::video::Progress, url: &Url) -> mdm_engine::Snapshot {
    use mdm_engine::download::{ConnStatus, ConnView};
    use mdm_tools::video::State as V;
    let (state, file, error) = match p.state {
        // Every stream in, their file being joined (#736).
        V::Running if p.merging.is_some() => (State::Merging, None, None),
        V::Running => (State::Downloading, None, None),
        V::Done(f) => (State::Done, Some(f), None),
        V::Failed(e) => (
            State::Failed,
            None,
            Some(match &p.merge_failed {
                Some(kept) => merge_failure(&e, kept),
                // A path yt-dlp couldn't make, too long for the system (#1102): said as such,
                // not as Python's "No such file or directory".
                None => match mdm_tools::video::too_long(&e) {
                    Some(path) => crate::errors::describe(&mdm_engine::Error::PathTooLong(path)),
                    None => e,
                },
            }),
        ),
        V::Stopped => (State::Cancelled, None, None),
    };
    let size = p.total;
    // A live stream yt-dlp records says no progress: what it has recorded is what's downloaded,
    // so Downloaded and the rate (from its growth) follow the recording.
    let downloaded = p.downloaded.max(p.recorded);
    // Downloaded by the helper itself (a live stream, RTMP; looking the page up): its one
    // connection, not an empty list (#793); receiving once a live stream's recorder has opened
    // its file (it is connected: `recording`, its file still 0 bytes until its first block) or
    // its recording grows, though yt-dlp gives it no speed.
    let own = (p.engine.is_none() && state == State::Downloading).then(|| ConnView {
        id: 0,
        downloaded,
        status: if p.speed > 0.0 || p.recorded > 0 || p.recording {
            ConnStatus::Receiving
        } else {
            ConnStatus::Connecting
        },
        speed: p.speed as u64,
    });
    let engine = p.engine.unwrap_or_default();
    let segments = if engine.segments.is_empty() {
        size.map(|t| {
            vec![mdm_engine::download::SegmentView {
                start: 0,
                cursor: downloaded.min(t),
                end: t,
            }]
        })
        .unwrap_or_default()
    } else {
        engine.segments
    };
    mdm_engine::Snapshot {
        state,
        // The stream under way's own address once the engine downloads it (#795), else the
        // page's.
        url: engine.address.clone().unwrap_or_else(|| url.clone()),
        file,
        size,
        downloaded,
        resumable: Some(true),
        segments,
        connections: own.into_iter().chain(engine.connections).collect(),
        error,
        failure: None,
        events: p
            .events
            .iter()
            .enumerate()
            .map(|(i, (at, text))| mdm_engine::download::Event {
                seq: i as u64 + 1,
                at: *at,
                text: text.clone(),
            })
            .collect(),
        // yt-dlp's and gallery-dl's connections are theirs: not seen here (#140); the engine's
        // are (#769).
        server: engine.server,
        at_service: None,
        finishing: p.merging.flatten(),
    }
}

impl Job {
    /// The engine's view, or one made from yt-dlp's progress (one segment, no connections).
    pub fn snapshot(&self) -> mdm_engine::Snapshot {
        match self {
            Job::File(d) => d.snapshot(),
            Job::Torrent(t) => t.snapshot(),
            Job::Remote(r) => r.snapshot(),
            Job::Video { job, url } => tool_snapshot(job.progress(), url),
            Job::Gallery { job, url } => tool_snapshot(job.progress(), url),
        }
    }

    pub fn cancel(&self) {
        match self {
            Job::File(d) => d.cancel(),
            Job::Video { job, .. } => job.stop(),
            Job::Gallery { job, .. } => job.stop(),
            Job::Torrent(t) => t.cancel(),
            Job::Remote(r) => r.cancel(),
        }
    }

    /// Stop and save: a live stream yt-dlp records is stopped and what it recorded kept
    /// as a video, the download then done; anything else is cancelled as [`Job::cancel`] does.
    pub fn save(&self) {
        match self {
            Job::Video { job, .. } => job.save(),
            _ => self.cancel(),
        }
    }

    /// A live stream's download: recording, being saved, or neither (`None`: every other job).
    pub fn live_state(&self) -> Option<mdm_tools::video::LiveState> {
        match self {
            Job::Video { job, .. } => job.live_state(),
            _ => None,
        }
    }

    /// A helper program downloads it, and takes a speed limit only as it starts, so a new limit
    /// starts it again (#708, `App::reapply_helper_limits`): gallery-dl, and yt-dlp for a video
    /// MDM's engine doesn't download (#769: a live stream, RTMP…).
    pub fn is_helper(&self) -> bool {
        match self {
            Job::Video { job, .. } => job.helper().is_some(),
            Job::Gallery { job, .. } => job.on_helper(),
            _ => false,
        }
    }

    /// The speed limit can change while a file downloads, and while the engine downloads a
    /// video's streams (#769); gallery-dl, and yt-dlp downloading by itself, take it at the start
    /// (the app starts them again, `is_helper`); a torrent's applies to the whole torrent session.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        match self {
            Job::File(d) => d.set_speed_limit(rate),
            Job::Torrent(t) => t.set_speed_limit(rate),
            Job::Remote(r) => r.set_speed_limit(rate),
            Job::Video { job, .. } => job.set_speed_limit(rate),
            Job::Gallery { job, .. } => job.set_speed_limit(rate),
        }
    }

    pub fn speed_limit(&self) -> Option<u64> {
        match self {
            Job::File(d) => d.speed_limit(),
            Job::Torrent(t) => t.speed_limit(),
            Job::Video { job, .. } => job.speed_limit(),
            Job::Gallery { job, .. } => job.speed_limit(),
            Job::Remote(r) => r.speed_limit(),
        }
    }
}

/// A subtitle language a video came without, in the interface language (#68).
pub fn sub_notice(p: &mdm_tools::video::SubProblem, site: &str) -> String {
    use mdm_tools::video::SubReason;
    let language = p.name.as_str();
    if p.beside {
        return crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name; its subtitles were to go inside the video, but came on a second try, so they're a separate file beside it
            crate::i18n::t(
                "Subtitles in {language} came on a second try: they're beside the video, not in it.",
            ),
            &[("language", language)],
        );
    }
    match &p.reason {
        SubReason::TooManyRequests => crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name, {site} = a web site's name (youtube.com); the site refused because it was asked too often
            crate::i18n::t(
                "Subtitles in {language} couldn't be downloaded: {site} refused (too many requests).",
            ),
            &[("language", language), ("site", site)],
        ),
        SubReason::Other(why) => crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name, {reason} = the helper program's own (English) words
            crate::i18n::t("Subtitles in {language} couldn't be downloaded: {reason}"),
            &[("language", language), ("reason", why)],
        ),
    }
}

/// A video download's choice (from "Download video from a page"): the page and yt-dlp's
/// format selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSpec {
    pub selector: String,
    /// The subtitles saved with it (#68; none in a list saved before them).
    #[serde(default)]
    pub subs: mdm_tools::video::Subs,
    /// What its last run kept beside its file when joining its streams failed, for Resume
    /// (`mdm_tools::video::Progress::merge_failed`): deleting it with its files removes them,
    /// whatever yt-dlp named them (`Talk.faud-English.mp4`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PathBuf>,
    /// The user typed its file name (#1102): kept as typed, never shortened to fit the path
    /// limits; one too long for its folder says so when it starts. A name MDM gave (the page's
    /// title) is shortened instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub named: bool,
    /// The video's length (whole seconds) as its look-up said: the same video picked again on
    /// its page is known by it when its stream's address is signed in its path for each load
    /// (`same_video`). None in a list saved before it, for a live stream or a length not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
}

/// A running download and its speed measurement.
pub struct Live {
    pub dl: Job,
    pub speed: Speed,
    /// The engine's last event already copied into the item's log.
    seen: u64,
    /// A video's first stream address of this run is in `Item::file_url` (#795).
    addressed: bool,
}

impl Live {
    pub fn new(dl: Job, downloaded: u64) -> Self {
        Live {
            dl,
            seen: 0,
            addressed: false,
            speed: Speed::new(Instant::now(), downloaded),
        }
    }
}

/// How long the speed shown takes to settle on a new pace (seconds): its average follows the
/// bytes with this time constant (#1176), about two thirds of the way in 3 s.
pub const SPEED_TAU: f64 = 3.0;
/// The same for the global limiter's split between files and torrents (app.rs).
pub const SPLIT_TAU: f64 = 2.0;
/// How often the speed shown changes (seconds), with its time left: every download's on the
/// same clock, so the list's rows and the total beside them change together (#1176).
pub const SHOWN_EVERY: f64 = 0.5;
/// No byte for this long (seconds): the speed is 0, not what is left of the average.
const STILL: f64 = 5.0;

/// `avg` (bytes a second) after `bytes` came in `dt` seconds: an exponential average whose
/// weight is the time that passed (1 − e^(−dt/τ), `tau` seconds), so it is the same whatever
/// the frame rate that reads the bytes.
pub fn average(avg: f64, bytes: u64, dt: f64, tau: f64) -> f64 {
    if dt <= 0.0 {
        return avg;
    }
    let k = 1.0 - (-dt / tau).exp();
    avg + (bytes as f64 / dt - avg) * k
}

/// The clock the shown speeds change on ([`SHOWN_EVERY`]).
fn shown_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// The tick of the shown speeds' clock that `now` is in.
fn shown_tick(now: Instant) -> u64 {
    (now.saturating_duration_since(shown_epoch()).as_secs_f64() / SHOWN_EVERY) as u64
}

/// How long from `now` to the next change of the shown speeds, a few milliseconds past it so the
/// frame lands in the new tick.
pub fn until_shown(now: Instant) -> std::time::Duration {
    let t = now.saturating_duration_since(shown_epoch()).as_secs_f64();
    let next = ((t / SHOWN_EVERY).floor() + 1.0) * SHOWN_EVERY;
    std::time::Duration::from_secs_f64(next - t + 0.005)
}

/// A download's speed (#1176): an average of the bytes over time ([`average`]), whatever the
/// frames that read them, and the figure shown from it, which changes on a steady clock
/// ([`SHOWN_EVERY`]) so its digits can be read. The time left goes with the figure shown.
#[derive(Clone, Debug)]
pub struct Speed {
    /// The average (bytes a second) and how much of it is measured (0 to 1): until the
    /// average has had a few seconds, it is the bytes over the time measured, not pulled
    /// towards the 0 it began at.
    avg: f64,
    weight: f64,
    /// The bytes last read and when.
    last: (Instant, u64),
    /// When a byte last came.
    moved: Instant,
    /// The figures shown: the speed, the bytes done when it was read, and its tick.
    shown: f64,
    shown_done: u64,
    tick: Option<u64>,
}

impl Speed {
    pub fn new(now: Instant, done: u64) -> Self {
        Speed {
            avg: 0.0,
            weight: 0.0,
            last: (now, done),
            moved: now,
            shown: 0.0,
            shown_done: done,
            tick: None,
        }
    }

    /// The average now (bytes a second).
    pub fn average(&self) -> f64 {
        if self.weight > 0.0 {
            self.avg / self.weight
        } else {
            0.0
        }
    }

    /// The bytes done read at `now`.
    pub fn read(&mut self, now: Instant, done: u64) {
        let (at, before) = self.last;
        let dt = now.saturating_duration_since(at).as_secs_f64();
        if done < before {
            // Fewer bytes than read before: bytes a dropped connection had counted, a video's
            // next stream, a file started over. The average goes on from here (it says how
            // fast the bytes come); a fresh one over a moment would jump.
            self.last = (now, done);
        } else if self.weight == 0.0 && done == before {
            // Nothing yet: the wait for the first byte isn't averaged in.
            self.last = (now, done);
            self.moved = now;
        } else if dt > 0.0 {
            let k = 1.0 - (-dt / SPEED_TAU).exp();
            self.avg = average(self.avg, done - before, dt, SPEED_TAU);
            self.weight += (1.0 - self.weight) * k;
            self.last = (now, done);
            if done > before {
                self.moved = now;
            } else if now.saturating_duration_since(self.moved).as_secs_f64() >= STILL {
                self.avg = 0.0;
            }
        }
        let tick = shown_tick(now);
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.shown = self.average();
            self.shown_done = self.last.1;
        }
    }

    /// Every byte is in (#736): no speed any more, at once; measured afresh from `done`.
    pub fn clear(&mut self, now: Instant, done: u64) {
        *self = Speed {
            tick: self.tick,
            ..Speed::new(now, done)
        };
    }

    /// The speed shown (bytes a second).
    pub fn shown(&self) -> f64 {
        self.shown
    }

    /// The time left (seconds) to `size` at the speed shown, from the bytes done when it was
    /// read; None without a speed.
    pub fn seconds_left(&self, size: u64) -> Option<f64> {
        (self.shown > 1.0).then(|| size.saturating_sub(self.shown_done) as f64 / self.shown)
    }
}

/// Entries kept in an item's event log.
pub const LOG: usize = 50;

fn push_log(log: &mut Vec<(i64, String)>, at: i64, text: String) {
    if log.len() >= LOG {
        log.remove(0);
    }
    log.push((at, text));
}

/// The size a row shows after a snapshot: the server's when it said one, else what was known. A
/// download that ended without one is as long as what came down, not what the browser announced
/// for the file it was going to fetch (#374).
fn size_after(
    state: State,
    server: Option<u64>,
    downloaded: u64,
    known: Option<u64>,
) -> Option<u64> {
    match server {
        None if state == State::Done => Some(downloaded),
        server => server.or(known),
    }
}

impl Item {
    /// Made from a private window's download, in this session or one before it.
    pub fn is_private_window(&self) -> bool {
        self.private_window || self.context.private
    }

    /// Add an app-side event (added, started, stopped …) to the log.
    pub fn note(&mut self, text: impl Into<String>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        push_log(&mut self.log, now, text.into());
    }

    /// The last `n` log entries as tooltip lines ("14:02:11  Connected to …").
    pub fn log_lines(&self, n: usize) -> String {
        self.log
            .iter()
            .rev()
            .take(n)
            .rev()
            .map(|(at, text)| {
                // In the interface's way (English month names in every language before).
                // With the offset of now (crate::clock, #536).
                let t = crate::clock::local_at(*at)
                    .map(|t| when_text(t.naive_local()))
                    .unwrap_or_default();
                format!("{t}  {text}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The file at its place is the one the user chose to replace (`replace_file`): the engine
    /// replaces it as the download completes. Its place moved since (renamed, numbered): no.
    pub fn replaces(&self) -> bool {
        self.replace_file.as_ref() == Some(&self.dest)
    }

    /// What tells where this row's files are (its place, its kind, the files it wrote), for a
    /// look at the disk on a thread other than the window's (#743): no live download, no log.
    pub fn place_copy(&self) -> Item {
        let mut c = Item::new(self.id, self.url.clone(), self.dest.clone(), self.category);
        c.file_name.clone_from(&self.file_name);
        c.status = self.status;
        c.downloaded = self.downloaded;
        c.video.clone_from(&self.video);
        c.torrent.clone_from(&self.torrent);
        c.gallery.clone_from(&self.gallery);
        c.remote.clone_from(&self.remote);
        c.preexisting_folder = self.preexisting_folder;
        c.written_files.clone_from(&self.written_files);
        c.number_at_start = self.number_at_start;
        c.state_here.clone_from(&self.state_here);
        c.live_copy = self.live.is_some() || self.live_copy;
        c
    }

    /// A new, not yet started download (queued state, nothing on disk).
    pub fn new(id: u64, url: Url, dest: PathBuf, category: Category) -> Self {
        Item {
            id,
            url,
            file_name: dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            dest,
            size: None,
            downloaded: 0,
            status: Status::Queued,
            category,
            description: String::new(),
            last_try: None,
            error: None,
            resumable: None,
            checksum: None,
            added: Some(now_stamp()),
            own_limit: None,
            video: None,
            torrent: None,
            gallery: None,
            log: Vec::new(),
            name_from_url: false,
            number_at_start: false,
            replace_file: None,
            package: None,
            group: None,
            server: None,
            direct: false,
            remote: None,
            rename_on_complete: None,
            preexisting_folder: false,
            moved_away: false,
            referer: None,
            page: None,
            waits_for_address: false,
            private_window: false,
            file_url: None,
            live: None,
            stream_rate: 0.0,
            torrent_check: None,
            snap: None,
            login: None,
            context: Default::default(),
            notices: Vec::new(),
            preloaded: None,
            side_files: Vec::new(),
            written_files: None,
            state_here: None,
            live_copy: false,
        }
    }
    /// This row's resume state is at its place. `downloaded` can't say: after a crash the list
    /// holds what it held at its last save, often nothing. As read beside the window at start
    /// while that holds (`state_here`), else from the disk.
    pub fn owns_resume_state(&self) -> bool {
        if let Some((at, here)) = &self.state_here
            && *at == self.dest
        {
            return *here;
        }
        let state = mdm_engine::state::state_path(&self.dest);
        mdm_engine::state::ResumeState::load(&state).is_ok_and(|s| s.url == self.url)
    }

    /// Whether this row has its place on the disk: running, part-way (its pieces and resume
    /// state are there), or finished with the file still there. A row not started yet holds
    /// nothing: several may wait under one name, and the first to start keeps it.
    pub fn holds_place(&self) -> bool {
        self.live.is_some()
            || self.live_copy
            || if self.status == Status::Complete {
                self.dest.exists()
            } else {
                self.downloaded > 0 || self.owns_resume_state()
            }
    }

    /// Pull the engine's snapshot into the row. Returns true when the download just ended.
    pub fn poll(&mut self) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        // Before the snapshot, not after: a live stream recording or being saved as this is read
        // can only show, in the snapshot taken next, more recorded, 0 (its run over, being
        // saved) or its file's size (saved). Read after, a save begun or ended in between would
        // show 0 for one look, unheld, and its file's size counted again by the download limit.
        let live_now = live.dl.live_state();
        let s = live.dl.snapshot();
        // The service's id for a torrent sent to it (#44), kept to continue after a restart.
        if let (Job::Remote(r), Some(spec)) = (&live.dl, self.remote.as_mut())
            && spec.transfer.is_none()
        {
            spec.transfer = r.transfer();
        }
        if let Some(server) = &s.server {
            self.server = Some(crate::whereis::Seen::from(server));
        }
        // A video's file address (#795): its first stream's, once the engine downloads it.
        if self.video.is_some() && !live.addressed && s.url != self.url {
            live.addressed = true;
            self.file_url = Some(s.url.clone());
        }
        self.downloaded = shown_downloaded(live_now, s.downloaded, self.downloaded);
        self.size = size_after(s.state, s.size, s.downloaded, self.size);
        self.resumable = s.resumable.or(self.resumable);
        if let Some(f) = &s.file {
            // Named now: a later start from scratch keeps this name, and its file is its own.
            let guessed = std::mem::take(&mut self.name_from_url);
            self.number_at_start = false;
            if *f != self.dest {
                // The file as saved (a video's real extension, a numbered duplicate, the name
                // the server gave).
                self.dest = f.clone();
                if let Some(name) = f.file_name() {
                    self.file_name = name.to_string_lossy().into_owned();
                }
                // The guess's category was the address's: the real name's now (the file stays
                // in the folder it was started in).
                if guessed {
                    self.category = Category::of(&self.file_name);
                }
            }
        }
        if self.torrent.is_none() && every_byte_in(&s) {
            // Every byte in (#736): nothing comes in while the last of it is written out, the
            // file checked, joined or named; the rate the download ended on isn't one any more.
            live.speed.clear(Instant::now(), self.downloaded);
        } else {
            live.speed.read(Instant::now(), self.downloaded);
        }
        let seen = live.seen;
        for e in s.events.iter().filter(|e| e.seq > seen) {
            let at =
                e.at.duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
            live.seen = e.seq;
            push_log(&mut self.log, at, e.text.clone());
        }
        self.snap = Some(s.clone());
        let ended = match s.state {
            State::Probing
            | State::Downloading
            | State::Verifying
            | State::Merging
            | State::Completing => None,
            State::Done => Some(Status::Complete),
            // Stopped, its last bytes and state still being written (#1328): stopped for the
            // list now; a start at its place waits for them in the engine.
            State::Stopping | State::Cancelled => Some(Status::Stopped),
            State::Failed => Some(Status::Error),
        };
        match ended {
            Some(status) => {
                let failed_now = status == Status::Error && self.status != Status::Error;
                // The reason in English, for the log: the engine's failure, else the helper's.
                let reason = failed_now.then(|| {
                    s.failure
                        .as_ref()
                        .map(ToString::to_string)
                        .or_else(|| s.error.clone())
                        .map_or_else(|| "no reason given".to_owned(), |r| without_queries(&r))
                });
                self.status = status;
                // A file's address that no longer gives it (expired, refused): it waits for a
                // fresh one, which the browser's next download of the file gives it.
                if failed_now
                    && self.video.is_none()
                    && !self.is_private_window()
                    && crate::freshlink::expiring_site(&self.url)
                    && s.failure
                        .as_ref()
                        .is_some_and(mdm_engine::Error::wants_new_address)
                {
                    self.waits_for_address = true;
                }
                // In the interface language; the engine's English when it gave no failure.
                self.error = s.failure.as_ref().map(crate::errors::describe).or(s.error);
                if self.video.is_some()
                    && self.is_private_window()
                    && let Some(error) = self
                        .error
                        .as_ref()
                        .and_then(|e| crate::videodlg::private_auth_error(e))
                {
                    self.error = Some(error);
                }
                // Why it failed, in the log too, once (#1262: 0.5.2's log said nothing of a
                // video yt-dlp couldn't download, only the progress window did). The site's
                // name, not its address: a log is attached to reports, and addresses carry
                // tokens.
                if let Some(reason) = reason {
                    tracing::warn!(
                        "download {} failed ({}{}): {reason}",
                        self.id,
                        if self.video.is_some() {
                            "a video, "
                        } else {
                            ""
                        },
                        self.url.host_str().unwrap_or("no host"),
                    );
                }
                // A video's streams kept after its merge failed: its own, for Delete.
                if let Some(Live {
                    dl: Job::Video { job, .. },
                    ..
                }) = &self.live
                    && let Some(v) = self.video.as_mut()
                {
                    v.parts = job.progress().merge_failed.unwrap_or_default();
                }
                if status == Status::Complete {
                    self.downloaded = self.size.unwrap_or(self.downloaded);
                    // Replaced (or kept, when it couldn't be): the file there now is its own.
                    self.replace_file = None;
                }
                // A video and its subtitles (#68), already marked as downloaded from the web by
                // its job, before it said it was done (#1103).
                let mut notices = Vec::new();
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Video { job, url },
                        ..
                    }) = &self.live
                {
                    let done = job.progress();
                    // The subtitles the site wouldn't give: said in the log and the Download
                    // complete window (the video is saved all the same).
                    if matches!(url.scheme(), "http" | "https") {
                        let site = url.host_str().unwrap_or("").trim_start_matches("www.");
                        notices = done
                            .sub_problems
                            .iter()
                            .map(|p| sub_notice(p, site))
                            .collect();
                    }
                    self.side_files = done.side_files;
                }
                // A gallery's files (#58), already marked by its job as the video's are.
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Gallery { job, .. },
                        ..
                    }) = &self.live
                {
                    let known = job.known_files();
                    add_written(self.written_files.get_or_insert_with(Vec::new), known);
                }
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Remote(r), ..
                    }) = &self.live
                {
                    if r.preexisting_folder() {
                        self.preexisting_folder = true;
                    }
                    let saved = r.saved_files();
                    add_written(self.written_files.get_or_insert_with(Vec::new), saved);
                }
                for n in &notices {
                    self.note(n.clone());
                }
                self.notices = notices;
                self.live = None;
                // The session's last check numbers belong to the run just ended (a finished
                // torrent leaves the session): not left saying "Checking" until the next tick.
                self.torrent_check = None;
                true
            }
            None => false,
        }
    }

    /// It failed because the site sent a web page where its file was expected (a file host's
    /// countdown or captcha, #701): the browser gets past it, so the progress window offers to
    /// open the link there. As its last run ended (the engine's failure, this session only).
    pub fn got_web_page(&self) -> bool {
        self.status == Status::Error
            && self.live.is_none()
            && self.snap.as_ref().is_some_and(|s| {
                s.state == State::Failed && matches!(s.failure, Some(mdm_engine::Error::GotWebPage))
            })
    }

    /// It failed because the server's answer was no larger than a web page where the browser's
    /// file was larger than any (#1460): the progress window explains it in full under "Error",
    /// as a site's page. As its last run ended (this session only).
    pub fn got_other_answer(&self) -> bool {
        self.status == Status::Error
            && self.live.is_none()
            && self.snap.as_ref().is_some_and(|s| {
                s.state == State::Failed
                    && matches!(s.failure, Some(mdm_engine::Error::NotTheFile { .. }))
            })
    }

    /// Its last run failed joining a video's streams and kept them for Resume
    /// ([`VideoSpec::parts`]): the progress window explains it in full under "Error", as a
    /// site's page (#701).
    pub fn merge_failed(&self) -> bool {
        self.status == Status::Error && self.video.as_ref().is_some_and(|v| !v.parts.is_empty())
    }

    /// Where its file comes from, the windows' "Address" (#795): a running video's stream under
    /// way (its page until the engine has one), a stopped or finished one's first stream of its
    /// last run; a file's (and a video's never downloaded) own address.
    pub fn file_address(&self) -> &Url {
        if self.video.is_none() {
            return &self.url;
        }
        match (&self.live, &self.snap) {
            (Some(_), Some(s)) => &s.url,
            _ => self.file_url.as_ref().unwrap_or(&self.url),
        }
    }

    /// The web page it was obtained from (#795): a video's or a gallery's is its own address
    /// (the page yt-dlp, gallery-dl look up); a file's the page the browser started it from
    /// (#607), else the one it sent as its referer (an older extension's, Import's, File
    /// Properties'). Refresh download address opens it. A site's address alone (a Referer a
    /// referrer policy cut, `https://host/`) only when none of them names a page on it.
    pub fn web_page(&self) -> Option<&str> {
        if self.video.is_some() || self.gallery.is_some() {
            return Some(self.url.as_str());
        }
        let known = [
            self.page.as_deref(),
            self.context.referer.as_deref(),
            self.referer.as_deref(),
        ];
        let mut known = known.into_iter().flatten().filter(|r| !r.is_empty());
        let first = known.clone().next()?;
        Some(known.find(|r| !site_only(r)).unwrap_or(first))
    }

    /// The referer sent with it from now on (the browser's, File Properties'): the request's
    /// copy and the list's together (#795).
    pub fn set_referer(&mut self, referer: Option<String>) {
        self.referer = referer.clone().filter(|r| !r.is_empty());
        self.context.referer = referer;
    }

    /// Why yt-dlp downloads this running video itself (#769), once it has found out: MDM's engine
    /// downloads every other one.
    pub fn video_helper(&self) -> Option<mdm_tools::streams::Helper> {
        match &self.live {
            Some(Live {
                dl: Job::Video { job, .. },
                ..
            }) => job.helper(),
            _ => None,
        }
    }

    /// A running live stream's download: recording, or its recording being saved
    /// ([`Job::live_state`]); `None` when nothing runs.
    pub fn live_state(&self) -> Option<mdm_tools::video::LiveState> {
        self.live.as_ref().and_then(|l| l.dl.live_state())
    }

    /// Resume / Download Now may start it: not running, and not finished (a gallery finished
    /// may start again: it fetches the files not there yet, #58).
    pub fn can_start(&self) -> bool {
        self.live.is_none()
            && (matches!(
                self.status,
                Status::Queued | Status::Stopped | Status::Error
            ) || (self.status == Status::Complete && self.gallery.is_some()))
    }

    /// The speed shown (bytes a second): the list's Transfer rate, the progress window's, the
    /// total in the status strip and the tray's tip all show this one ([`Speed`]).
    pub fn rate(&self) -> f64 {
        self.live
            .as_ref()
            .map_or(self.stream_rate, |l| l.speed.shown())
    }

    /// The time left (seconds) at the speed shown, while it downloads with a known size.
    pub fn seconds_left(&self) -> Option<f64> {
        if self.status != Status::Downloading || self.past_last_byte() {
            return None;
        }
        let size = self.size?;
        if let Some(live) = &self.live {
            return live.speed.seconds_left(size);
        }
        // A Telegram media item has no engine job (`live`): its bytes and speed come from the
        // local upload server, which keeps a 6-second rate in `stream_rate`. Use the bytes
        // written so far (the ones the row shows) to work out what is left at that speed.
        let rate = self.stream_rate;
        (rate > 1.0).then(|| size.saturating_sub(self.downloaded) as f64 / rate)
    }

    /// How much has come, 0 to 1, or None without a size to take a share of (a live stream,
    /// some HLS, a server that doesn't say): no percentage rather than 0 %. A video reads at
    /// most 99.9 % until the file is finished: another stream or the merge may still follow
    /// (#344), and the sizes beside it stay the real ones.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.size.filter(|t| *t > 0)?;
        let f = (self.downloaded as f64 / total as f64).clamp(0.0, 1.0);
        Some(if self.video.is_some() && self.status != Status::Complete {
            f.min(mdm_tools::video::MAX_WHILE_RUNNING)
        } else {
            f
        })
    }

    /// The Status column: `13.38%`, `Complete`, `Error`, or blank.
    /// A torrent at its debrid service (#44), before its files come, or a file-host link whose file
    /// the service fetches first (TorBox): "45% at Real-Debrid" (the percentage first: the list's
    /// Status column is narrow).
    pub fn at_service(&self) -> Option<String> {
        let (service, progress) = match &self.live {
            Some(Live {
                dl: Job::Remote(r), ..
            }) => r.at_service().map(|(s, p)| (s.to_owned(), p))?,
            Some(_) => self.snap.as_ref()?.at_service.clone()?,
            None => return None,
        };
        let service = service.as_str();
        Some(crate::i18n::fmt(
            // i18n: a torrent's status while the user's debrid service downloads it, before MDM gets its files; {service} the service ("Real-Debrid"), {percent} how far it is ("45%")
            crate::i18n::t("{percent} at {service}"),
            &[
                ("service", service),
                (
                    "percent",
                    &progress.map_or_else(|| "...".to_owned(), |p| percent(p as f64)),
                ),
            ],
        ))
    }

    /// "Checking 42%" while a torrent's files are read and checked (added over files already
    /// there, Recheck): a running one from its live numbers (librqbit counts the bytes checked),
    /// one not running from the session's (`torrent_check`, every 2 s). Whole percents: the
    /// list's Status column is narrow.
    pub fn torrent_checking(&self) -> Option<String> {
        self.torrent.as_ref()?;
        let (checked, total) = match (&self.live, &self.snap) {
            (Some(_), Some(s)) if s.state == State::Verifying => {
                (s.downloaded, s.size.unwrap_or(0))
            }
            (Some(_), _) => return None,
            (None, _) => self.torrent_check?,
        };
        let ratio = if total > 0 {
            checked as f64 / total as f64
        } else {
            0.0
        };
        Some(checking(&crate::i18n::percent(ratio.min(1.0), 0)))
    }

    /// After the last byte, while the file is checked against its checksum or a video's
    /// streams are joined (#736): "Verifying... (45%)", "Merging...", with how far it is when
    /// that's known. `short`: the list's Status column, one short word and how far, "Checking
    /// 45%" (as a torrent's check) and "Merging 45%" ("Merging ..." before that is known); the
    /// column is wide enough for them in the interface language and font ([`crate::list::status_width`]). The
    /// cell's tip and a screen reader say it whole ([`crate::list::Row::status_whole`]).
    pub fn finishing(&self, short: bool) -> Option<String> {
        let s = self.snap.as_ref()?;
        if !matches!(s.state, State::Verifying | State::Merging) {
            return None;
        }
        let percent = s
            .finishing
            .map(|f| crate::i18n::percent(f.clamp(0.0, 1.0), 0));
        let verifying = s.state == State::Verifying;
        Some(match (short, percent) {
            // As "... at Real-Debrid" before its service says how far.
            (true, percent) => {
                let percent = percent.as_deref().unwrap_or("...");
                if verifying {
                    checking(percent)
                } else {
                    merging(percent)
                }
            }
            (false, Some(percent)) => crate::i18n::fmt(
                if verifying {
                    // i18n: a download's status while the finished file is checked against its checksum; {percent} how much is checked, e.g. "45%"
                    crate::i18n::t("Verifying... ({percent})")
                } else {
                    // i18n: a video's status while its picture and sound are joined into one file (ffmpeg); {percent} how far, e.g. "45%"
                    crate::i18n::t("Merging... ({percent})")
                },
                &[("percent", &percent)],
            ),
            (false, None) if verifying => {
                // i18n: a download's status while the finished file is checked against its checksum
                crate::i18n::t("Verifying...").to_owned()
            }
            (false, None) => {
                // i18n: a video's status while its picture and sound are joined into one file (ffmpeg)
                crate::i18n::t("Merging...").to_owned()
            }
        })
    }

    /// Every byte is in (#736): no rate and no time left, as once it is complete.
    pub fn past_last_byte(&self) -> bool {
        self.torrent.is_none() && self.snap.as_ref().is_some_and(every_byte_in)
    }

    pub fn status_text(&self) -> String {
        if let Some(at) = self.at_service() {
            return at;
        }
        if let Some(checking) = self.torrent_checking() {
            return checking;
        }
        if let Some(finishing) = self.finishing(true) {
            return finishing;
        }
        match self.status {
            // i18n: a download's status (state it is in, not a command): finished
            Status::Complete => crate::i18n::t("Complete").into(),
            // i18n: a download's status (state it is in, not a command): it failed
            Status::Error => crate::i18n::t("Error").into(),
            // Where IDM leaves the cell blank (nothing yet, or no size to take a share of), it
            // says what the download is doing.
            // i18n: a download's status (state it is in, not a command): waiting in a queue for its turn
            Status::Queued if self.downloaded == 0 => crate::i18n::t("Waiting").into(),
            _ => match (self.fraction(), self.status) {
                (Some(f), _) => percent(f),
                // i18n: a download's status (state it is in, not a command): stopped by the user
                (_, Status::Stopped) => crate::i18n::t("Stopped").into(),
                (_, Status::Queued) => crate::i18n::t("Waiting").into(),
                // No size to take a share of: how much has come (it looked idle).
                (_, Status::Downloading) if self.downloaded > 0 => size_list(self.downloaded),
                _ => String::new(),
            },
        }
    }

    pub fn time_left(&self) -> String {
        self.seconds_left().map_or(String::new(), time)
    }
}

/// A web address that is a site's only (`https://host/`, no path, query or fragment): what a
/// referrer policy leaves of the page a download was started from ([`Item::web_page`]).
fn site_only(address: &str) -> bool {
    Url::parse(address)
        .is_ok_and(|u| u.path() == "/" && u.query().is_none() && u.fragment().is_none())
}

/// What a running download shows as downloaded: `now`, its job's; a live stream recording or
/// being saved (`live`, read before `now`) never less than `shown`. Its recording being saved, its
/// run is over and what it recorded no longer counts in its progress (0 until its file is
/// there): what it got stays, for the window and for the download limit, which counts only what
/// is more than before (`Quota::record`) and would count the recording again as it came back.
fn shown_downloaded(live: Option<mdm_tools::video::LiveState>, now: u64, shown: u64) -> u64 {
    if live.is_some() { now.max(shown) } else { now }
}

/// Every byte of a download is in (#736): the last of it is written out (its whole size
/// received and no connection receiving or connecting, as the progress window's "Finishing..."),
/// the file checked, a video's streams joined, or the file named. Nothing comes in any more.
fn every_byte_in(s: &mdm_engine::Snapshot) -> bool {
    use mdm_engine::download::ConnStatus;
    matches!(
        s.state,
        State::Verifying | State::Merging | State::Completing
    ) || (s.state == State::Downloading
        && s.size.is_some_and(|t| t > 0 && s.downloaded >= t)
        && !s
            .connections
            .iter()
            .any(|c| matches!(c.status, ConnStatus::Receiving | ConnStatus::Connecting)))
}

/// The list's Status column while a torrent's files or a finished file are checked (against
/// the torrent's hashes, against a checksum): one short word and how far (#736).
pub fn checking(percent: &str) -> String {
    crate::i18n::fmt(
        // i18n: the list's Status column while a torrent's files or a finished file are checked (against the torrent's hashes or a checksum); {percent} how much is checked, e.g. "42%". One short word: the column is narrow
        crate::i18n::t("Checking {percent}"),
        &[("percent", percent)],
    )
}

/// The list's Status column while a video's picture and sound are joined: one short word and
/// how far (#736).
pub fn merging(percent: &str) -> String {
    crate::i18n::fmt(
        // i18n: the list's Status column while a video's picture and sound are joined into one file (ffmpeg); {percent} how far, e.g. "45%". One short word: the column is narrow
        crate::i18n::t("Merging {percent}"),
        &[("percent", percent)],
    )
}

/// The "Last Try Date" and "Added" columns as stored, e.g. `Oct 28 21:44:12 2026` (the list's
/// format; read back by `stamp_time`).
pub fn now_stamp() -> String {
    crate::clock::now().format(STAMP).to_string()
}

const STAMP: &str = "%b %d %H:%M:%S %Y";

/// A stored stamp as a time (local), for sorting and for `date_text`.
pub fn stamp_time(stored: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(stored.trim(), STAMP).ok()
}

/// A stored stamp as the list shows it: in the formats chosen in Options → Regional; else IDM's
/// own text in English (on the system's 12- or 24-hour clock), and the user's Windows region
/// format in other languages (its digits and order; month names would be in the region's
/// language).
pub fn date_text(stored: &str) -> String {
    let Some(t) = stamp_time(stored) else {
        return stored.to_owned();
    };
    when_text(t)
}

/// A local time as the interface writes dates and times (Options → Regional; IDM's text in
/// English, on the system's clock; the Windows region's elsewhere).
pub fn when_text(t: chrono::NaiveDateTime) -> String {
    let formats = crate::regional::current();
    if formats.custom {
        return crate::i18n::ltr(&crate::regional::short_date_time(t));
    }
    if english() {
        return stamp_text(t, formats.clock24);
    }
    #[cfg(windows)]
    if let Some(s) = crate::win32::local_date_time(t) {
        return crate::i18n::ltr(&s);
    }
    crate::i18n::ltr(&crate::regional::short_date_time(t))
}

/// IDM's text for a time, as stored (`Oct 28 21:44:12 2026`) on a 24-hour clock, and with the
/// system's 12-hour clock (#392) `Oct 28 9:44:12 PM 2026`.
fn stamp_text(t: chrono::NaiveDateTime, clock24: bool) -> String {
    use chrono::Timelike;
    format!(
        "{} {} {}",
        t.format("%b %d"),
        crate::regional::time_text(t.hour(), t.minute(), t.second(), clock24),
        t.format("%Y")
    )
}

/// Whether numbers are written IDM's way (English): `7.05 MB`, `13.38%`, `1 min 22 sec`.
/// Every other language writes them as its region does (digits, decimal mark, unit names; D-019).
fn english() -> bool {
    crate::i18n::code() == "en"
}

/// `13.38%` (IDM), or the interface language's percentage (`13,38 %`).
pub fn percent(ratio: f64) -> String {
    crate::i18n::percent(ratio, 2)
}

/// `{n} MB`-style unit text in the interface language (catalog context `unit`).
fn unit(n: f64, decimals: usize, template: &str) -> String {
    crate::i18n::i18n().measure(crate::i18n::lang(), n, decimals, template)
}

/// `7.05 MB`, `412.20 KB`, `1.97 GB` — IDM shows two decimals.
pub fn size(b: u64) -> String {
    if !english() {
        let v = b as f64;
        let k = 1024.0;
        return if v >= k * k * k {
            unit(v / (k * k * k), 2, "{n} GB")
        } else if v >= k * k {
            unit(v / (k * k), 2, "{n} MB")
        } else if v >= k {
            unit(v / k, 2, "{n} KB")
        } else {
            crate::i18n::tn(b, "{n} byte", "{n} bytes")
        };
    }
    let b = b as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0)) // not translated: English path
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / (1024.0 * 1024.0)) // not translated: English path
    } else if b >= 1024.0 {
        format!("{:.2} KB", b / 1024.0) // not translated: English path
    } else {
        format!("{b} Bytes") // not translated: English path
    }
}

/// `200.000 MB` — the progress dialog uses three decimals (and KB below a kilobyte).
pub fn size3(b: u64) -> String {
    let v = b as f64;
    let k = 1024.0;
    if !english() {
        return if v >= k * k * k {
            unit(v / (k * k * k), 3, "{n} GB")
        } else if v >= k * k {
            unit(v / (k * k), 3, "{n} MB")
        } else {
            unit(v / k, 3, "{n} KB")
        };
    }
    if v >= k * k * k {
        format!("{:.3} GB", v / (k * k * k)) // not translated: English path
    } else if v >= k * k {
        format!("{:.3} MB", v / (k * k)) // not translated: English path
    } else {
        format!("{:.3} KB", v / k) // not translated: English path
    }
}

/// `11.623 MB/sec`, `512.000 KB/sec` (IDM's progress dialog uses three decimals).
pub fn rate(bps: f64) -> String {
    if !english() {
        return if bps >= 1024.0 * 1024.0 {
            unit(bps / (1024.0 * 1024.0), 3, "{n} MB/sec")
        } else {
            unit(bps / 1024.0, 3, "{n} KB/sec")
        };
    }
    if bps >= 1024.0 * 1024.0 {
        format!("{:.3} MB/sec", bps / (1024.0 * 1024.0)) // not translated: English path
    } else {
        format!("{:.3} KB/sec", bps / 1024.0) // not translated: English path
    }
}

/// A speed limit as the user typed it, in whole KBytes/sec: `512 KB/sec`, `2048 KB/sec` (the
/// Speed Limiter's boxes take KBytes/sec).
pub fn limit_rate(bps: u64) -> String {
    let kb = (bps / 1024).max(1);
    if english() {
        format!("{kb} KB/sec") // not translated: English path
    } else {
        unit(kb as f64, 0, "{n} KB/sec")
    }
}

/// The speed limit that holds a running download back (#1191): what its Transfer rate says
/// beside the speed, so a limiter left on is not forgotten.
///
/// It names the limit, not that the download is "limited": the speed measured goes a little
/// above and below a limit it runs at, and a download slower than its limit (a slow server) is
/// under that limit all the same, so comparing the two would flicker or claim too much.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// At most this many bytes a second for this download: its own limit (its progress
    /// window's Speed Limiter tab, or remembered for the file), else the Speed Limiter's for one
    /// file.
    One(u64),
    /// This many bytes a second for all downloads together (Speed Limiter settings, beyond
    /// IDM), the tighter one for this download.
    All(u64),
}

impl Limit {
    /// The limit that binds a download whose own limit is `one`, while the limit for all
    /// downloads together is `all` (both bytes/s, None: off) and `running` downloads share it
    /// (this one among them). Its own binds unless that many downloads, each at it, would go
    /// over the shared one: then each gets less than its own, and the shared one binds.
    pub fn binding(one: Option<u64>, all: Option<u64>, running: usize) -> Option<Limit> {
        match (one, all) {
            (None, None) => None,
            (Some(one), None) => Some(Limit::One(one)),
            (None, Some(all)) => Some(Limit::All(all)),
            (Some(one), Some(all)) => {
                let together = one.saturating_mul(running.max(1) as u64);
                Some(if together <= all {
                    Limit::One(one)
                } else {
                    Limit::All(all)
                })
            }
        }
    }

    /// Bytes a second.
    pub fn rate(self) -> u64 {
        match self {
            Limit::One(r) | Limit::All(r) => r,
        }
    }

    /// The progress window's Transfer rate with the limit after the speed `shown` (a
    /// [`rate`]): `512.000 KB/sec (limit: 512 KB/sec)`.
    pub fn after(self, shown: &str) -> String {
        let limit = limit_rate(self.rate());
        let template = match self {
            // i18n: the progress window's Transfer rate row while a speed limit applies: {rate} is the download's speed now ("512.000 KB/sec"), {limit} the most this download may take ("512 KB/sec"). The row is narrow: keep it short
            Limit::One(_) => crate::i18n::t("{rate} (limit: {limit})"),
            // i18n: the progress window's Transfer rate row while the limit for all downloads together applies: {rate} is this download's speed now ("512.000 KB/sec"), {limit} the most all downloads together may take ("1024 KB/sec"). The row is narrow: keep it short
            Limit::All(_) => crate::i18n::t("{rate} (limit for all downloads: {limit})"),
        };
        crate::i18n::fmt(template, &[("rate", shown), ("limit", &limit)])
    }

    /// The tip of a download's Transfer rate cell in the list (the column is too narrow for
    /// the limit beside the speed).
    pub fn tip(self) -> String {
        let limit = limit_rate(self.rate());
        let template = match self {
            // i18n: tip of a download's Transfer rate in the list: {limit} is the most this download may take ("512 KB/sec"), set with the Speed Limiter
            Limit::One(_) => crate::i18n::t("Speed Limiter: at most {limit} for this download"),
            // i18n: tip of a download's Transfer rate in the list: {limit} is the most all downloads together may take ("1024 KB/sec"), set in the Speed Limiter's settings
            Limit::All(_) => crate::i18n::t("Speed Limiter: {limit} for all downloads together"),
        };
        crate::i18n::fmt(template, &[("limit", &limit)])
    }
}

/// The status strip's and the tray tip's word that the Speed Limiter is on (#1191), whatever
/// runs: the limit for one download (`one`, Downloads → Speed Limiter → Turn on) and the limit
/// for all downloads together (`all`, while it applies), bytes/s. None while both are off.
pub fn limiter_hint(one: Option<u64>, all: Option<u64>) -> Option<String> {
    let t = crate::i18n::t;
    Some(match (one, all) {
        (None, None) => return None,
        (Some(one), None) => crate::i18n::fmt(
            // i18n: the status bar and the tray icon's tip while Downloads → Speed Limiter is turned on: {limit} is the most each download may take ("512 KB/sec")
            t("Speed Limiter: {limit} per download"),
            &[("limit", &limit_rate(one))],
        ),
        (None, Some(all)) => crate::i18n::fmt(
            // i18n: the status bar and the tray icon's tip while the Speed Limiter's limit for all downloads together is on: {limit} is the most they may take together ("1024 KB/sec")
            t("Speed Limiter: {limit} for all downloads"),
            &[("limit", &limit_rate(all))],
        ),
        (Some(one), Some(all)) => crate::i18n::fmt(
            // i18n: the status bar and the tray icon's tip while both Speed Limiter limits are on: {one} is the most each download may take ("512 KB/sec"), {all} the most all of them together may take ("1024 KB/sec")
            t("Speed Limiter: {one} per download, {all} for all"),
            &[("one", &limit_rate(one)), ("all", &limit_rate(all))],
        ),
    })
}

/// The list's Size column: IDM puts two spaces before the unit (`200.00  MB`).
pub fn size_list(b: u64) -> String {
    if english() {
        size(b).replacen(' ', "  ", 1)
    } else {
        size(b)
    }
}

/// `14 sec`, `1 min 22 sec`, `2 hour(s) 5 min`.
pub fn time(secs: f64) -> String {
    let s = secs.round() as u64;
    if !english() {
        let n = crate::i18n::integer;
        let (h, m, sec) = (n(s / 3600), n((s % 3600) / 60), n(s % 60));
        let t = crate::i18n::t;
        return match s {
            0..60 => crate::i18n::fmt(t("{s} sec"), &[("s", &sec)]),
            60..3600 => crate::i18n::fmt(t("{m} min {s} sec"), &[("m", &m), ("s", &sec)]),
            _ => crate::i18n::fmt(t("{h} hour(s) {m} min"), &[("h", &h), ("m", &m)]),
        };
    }
    match s {
        0..60 => format!("{s} sec"), // not translated: English path
        60..3600 => format!("{} min {} sec", s / 60, s % 60), // not translated: English path
        _ => format!("{} hour(s) {} min", s / 3600, (s % 3600) / 60), // not translated: English path
    }
}

#[cfg(test)]
mod file_address_tests {
    use super::*;

    const PAGE: &str = "https://www.example.com/watch?v=abc";
    const STREAM: &str = "https://media.example.net/videoplayback?itag=399";

    fn video() -> Item {
        let mut i = Item::new(
            1,
            PAGE.parse().unwrap(),
            PathBuf::from("Clip.mp4"),
            Category::Video,
        );
        i.video = Some(VideoSpec {
            selector: "best".into(),
            subs: Default::default(),
            parts: Vec::new(),
            named: false,
            duration: None,
        });
        i
    }

    /// The engine's view of a video's stream carries its address: the snapshot's (#795).
    #[test]
    fn a_video_snapshot_has_its_streams_address() {
        let page: Url = PAGE.parse().unwrap();
        let mut p = mdm_tools::video::Progress {
            state: mdm_tools::video::State::Running,
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
        // Looking the page up: no stream yet.
        assert_eq!(tool_snapshot(p.clone(), &page).url, page);
        p.engine = Some(mdm_tools::streams::View {
            address: Some(STREAM.parse().unwrap()),
            ..Default::default()
        });
        assert_eq!(tool_snapshot(p, &page).url.as_str(), STREAM);
    }

    /// Address: a video's stream, its page the web page it was obtained from; a file's own
    /// address, its referer the page (#795).
    #[test]
    fn the_windows_show_the_file_address_and_the_page() {
        let mut v = video();
        assert_eq!(v.file_address().as_str(), PAGE);
        v.file_url = Some(STREAM.parse().unwrap());
        assert_eq!(v.file_address().as_str(), STREAM);
        assert_eq!(v.web_page(), Some(PAGE));

        let mut f = Item::new(
            2,
            "https://files.example.net/a.zip".parse().unwrap(),
            PathBuf::from("a.zip"),
            Category::Compressed,
        );
        assert_eq!(f.file_address().as_str(), "https://files.example.net/a.zip");
        assert_eq!(f.web_page(), None);
        f.set_referer(Some("https://www.example.com/get".into()));
        assert_eq!(f.web_page(), Some("https://www.example.com/get"));
        assert_eq!(
            f.context.referer.as_deref(),
            Some("https://www.example.com/get")
        );
        // The page the browser started it from, whole, before its Referer (#607): a referrer
        // policy often cuts that to the site's address.
        f.set_referer(Some("https://www.example.com/".into()));
        f.page = Some("https://www.example.com/files/a.html".into());
        assert_eq!(f.web_page(), Some("https://www.example.com/files/a.html"));
        // A site's address alone is the page only when nothing names one on it: a page the
        // browser gave as only the site's (a tab gone to the home page) gives way to a Referer
        // that is a whole page; a Referer cut to the site, with no page, is still shown.
        f.page = Some("https://www.example.com/".into());
        f.set_referer(Some("https://www.example.com/files/a.html".into()));
        assert_eq!(f.web_page(), Some("https://www.example.com/files/a.html"));
        f.page = None;
        f.set_referer(Some("https://www.example.com/".into()));
        assert_eq!(f.web_page(), Some("https://www.example.com/"));
        f.page = Some(String::new());
        assert_eq!(f.web_page(), Some("https://www.example.com/"));
    }

    /// The referer and a video's file address are kept with the list (#795); the browser's
    /// other context is not.
    #[test]
    fn the_referer_and_file_address_are_saved() {
        let mut v = video();
        v.file_url = Some(STREAM.parse().unwrap());
        v.set_referer(Some("https://www.example.com/".into()));
        v.context.cookies = Some("sid=1".into());
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("sid=1"));
        let back: Item = serde_json::from_str(&json).unwrap();
        assert_eq!(back.referer.as_deref(), Some("https://www.example.com/"));
        assert_eq!(back.file_url.as_ref().map(Url::as_str), Some(STREAM));
        // A list saved before them reads as before.
        let old = json
            .replace(r#","referer":"https://www.example.com/""#, "")
            .replace(&format!(",\"file_url\":\"{STREAM}\""), "");
        let back: Item = serde_json::from_str(&old).unwrap();
        assert_eq!((back.referer, back.file_url), (None, None));
    }

    /// The page a download was started from is kept with the list (#607), and none is written
    /// for a download without one.
    #[test]
    fn the_page_it_was_started_from_is_saved() {
        let mut f = Item::new(
            3,
            "https://cdn.example.net/a.zip".parse().unwrap(),
            PathBuf::from("a.zip"),
            Category::Compressed,
        );
        assert!(!serde_json::to_string(&f).unwrap().contains("\"page\""));
        f.page = Some("https://www.example.com/files/a.html".into());
        let back: Item = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(
            back.web_page(),
            Some("https://www.example.com/files/a.html")
        );
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn a_finished_download_is_as_long_as_what_came_down() {
        // The browser announced 178 MB; the server sent a 15 KB page and no size.
        assert_eq!(
            size_after(State::Done, None, 14473, Some(186662440)),
            Some(14473)
        );
        assert_eq!(
            size_after(State::Downloading, None, 14473, Some(186662440)),
            Some(186662440)
        );
        assert_eq!(size_after(State::Done, Some(100), 100, None), Some(100));
    }
}

#[cfg(test)]
mod tests {
    /// #1262: a failure's reason goes to the log without the queries of the addresses in it.
    #[test]
    fn a_failure_is_logged_without_tokens() {
        assert_eq!(
            super::without_queries(
                "error sending request for url (https://cdn.example/v/1.m3u8?vid=9&dt=abc): 414"
            ),
            "error sending request for url (https://cdn.example/v/1.m3u8?…): 414"
        );
        assert_eq!(
            super::without_queries("http://a.example/x#frag and https://b.example/y"),
            "http://a.example/x?… and https://b.example/y"
        );
        assert_eq!(
            super::without_queries("HTTP Error 414: URI Too Long"),
            "HTTP Error 414: URI Too Long"
        );
    }

    use super::*;

    /// #1088: a download replaces the file chosen in the Save dialog only while its place is that
    /// file; the choice is kept with the list (a download queued with Later, MDM restarted).
    #[test]
    fn a_download_replaces_only_the_file_chosen_while_it_is_its_place() {
        let url = Url::parse("http://127.0.0.1:9/a.7z").unwrap();
        let taken = PathBuf::from("dl").join("taken.7z");
        let mut it = Item::new(1, url, taken.clone(), Category::Compressed);
        assert!(!it.replaces());
        it.replace_file = Some(taken.clone());
        assert!(it.replaces());
        let back: Item = serde_json::from_str(&serde_json::to_string(&it).unwrap()).unwrap();
        assert_eq!(back.replace_file.as_deref(), Some(taken.as_path()));
        assert!(back.replaces());
        // Its place moved (another row's, a rename): nothing replaced there.
        it.dest = PathBuf::from("dl").join("taken (1).7z");
        assert!(!it.replaces());
        // Never written for a download without one.
        let none = Item::new(
            2,
            Url::parse("http://127.0.0.1:9/b.7z").unwrap(),
            PathBuf::from("b.7z"),
            Category::Compressed,
        );
        assert!(
            !serde_json::to_string(&none)
                .unwrap()
                .contains("replace_file")
        );
    }

    /// A finished gallery's files join its written files in time linear in them, each once.
    #[test]
    fn written_files_join_in_linear_time() {
        let paths = |from: usize, n: usize| -> Vec<PathBuf> {
            (from..from + n)
                .map(|i| PathBuf::from(format!("/gallery/image-{i:09}.jpg")))
                .collect()
        };
        mdm_labserver::growth::assert_linear(
            2_000,
            |n| (paths(0, n / 2), paths(n / 4, n)),
            |(mut list, more)| {
                add_written(&mut list, more);
                list.len()
            },
        );
        let mut list = paths(0, 3);
        add_written(&mut list, paths(2, 3));
        assert_eq!(list, paths(0, 5));
    }

    fn video_item(size: Option<u64>, downloaded: u64, status: Status) -> Item {
        let url = Url::parse("https://example.org/a.mp4").unwrap();
        let mut it = Item::new(1, url, PathBuf::from("a.mp4"), Category::Video);
        it.size = size;
        it.downloaded = downloaded;
        it.status = status;
        it.video = Some(VideoSpec {
            selector: "18".into(),
            subs: Default::default(),
            parts: Vec::new(),
            named: false,
            duration: None,
        });
        it
    }

    /// A Telegram media item has no engine job, so its time left comes from the upload
    /// server's rate and the bytes written: without that fallback the progress window and
    /// the browser popup show no time left at all.
    #[test]
    fn a_media_item_has_time_left_from_its_stream_rate() {
        let url = Url::parse("https://web.telegram.org/k/").unwrap();
        let mut it = Item::new(1, url, PathBuf::from("clip.mp4"), Category::Video);
        it.status = Status::Downloading;
        it.size = Some(1000);
        it.downloaded = 200;
        it.stream_rate = 100.0;
        assert!(it.live.is_none(), "a page upload has no engine job");
        assert_eq!(it.seconds_left(), Some(8.0));
        // No speed yet (the 6-second window has not sampled a chunk): no figure.
        it.stream_rate = 0.0;
        assert_eq!(it.seconds_left(), None);
        // Not downloading: no time left, whatever the rate.
        it.stream_rate = 100.0;
        it.status = Status::Stopped;
        assert_eq!(it.seconds_left(), None);
    }

    /// Joining a video's streams failed: why, and that what was downloaded stays for Resume,
    /// by name; without anything kept, only why.
    #[test]
    fn a_failed_merge_says_what_stays() {
        let url = Url::parse("https://example.org/a.mp4").unwrap();
        let mut p = mdm_tools::video::Progress {
            state: mdm_tools::video::State::Failed("Postprocessing: Conversion failed!".into()),
            merge_failed: Some(vec![
                PathBuf::from("/v/Talk.f640.mp4"),
                PathBuf::from("/v/Talk.faud-English.mp4"),
            ]),
            ..Default::default()
        };
        let e = super::tool_snapshot(p.clone(), &url).error.unwrap();
        assert!(e.contains("Conversion failed!"), "{e}");
        assert!(e.contains("Resume"), "{e}");
        assert!(
            e.contains("Talk.f640.mp4") && e.contains("Talk.faud-English.mp4"),
            "{e}"
        );
        p.merge_failed = Some(Vec::new());
        let e = super::tool_snapshot(p.clone(), &url).error.unwrap();
        assert!(
            e.contains("Conversion failed!") && !e.contains("Resume"),
            "{e}"
        );
        // Any other failure: yt-dlp's words as they are.
        p.merge_failed = None;
        let e = super::tool_snapshot(p, &url).error.unwrap();
        assert_eq!(e, "Postprocessing: Conversion failed!");
    }

    /// #736: after the last byte the Status column says what is happening and how far.
    #[test]
    fn verifying_and_merging_say_how_far() {
        let m = 1 << 20;
        let mut it = video_item(Some(100 * m), 100 * m, Status::Downloading);
        let url = Url::parse("https://example.org/a.mp4").unwrap();
        let mut p = mdm_tools::video::Progress {
            total: Some(100 * m),
            downloaded: 100 * m,
            merging: Some(Some(0.45)),
            ..Default::default()
        };
        it.snap = Some(super::tool_snapshot(p.clone(), &url));
        // A short word and how far in the list; the progress window says it whole.
        assert_eq!(it.status_text(), "Merging 45%");
        assert_eq!(it.finishing(false).as_deref(), Some("Merging... (45%)"));
        assert!(it.past_last_byte());
        p.merging = Some(None);
        it.snap = Some(super::tool_snapshot(p.clone(), &url));
        assert_eq!(it.status_text(), "Merging ...");
        assert_eq!(it.finishing(false).as_deref(), Some("Merging..."));
        let mut s = super::tool_snapshot(p.clone(), &url);
        s.state = State::Verifying;
        s.finishing = Some(0.5);
        it.snap = Some(s);
        assert_eq!(it.status_text(), "Checking 50%");
        assert_eq!(it.finishing(false).as_deref(), Some("Verifying... (50%)"));
        assert!(it.past_last_byte());
        // Still coming in (yt-dlp's own connection receiving): a rate and a time left as before.
        p.merging = None;
        p.speed = 1000.0;
        it.snap = Some(super::tool_snapshot(p.clone(), &url));
        assert!(!it.past_last_byte());
        assert_eq!(it.finishing(true), None);
        // Every byte in, the engine's connections done, before the merge: nothing comes in.
        p.speed = 0.0;
        p.engine = Some(Default::default());
        it.snap = Some(super::tool_snapshot(p.clone(), &url));
        assert!(it.past_last_byte());
        // Short of the whole size: still coming.
        p.downloaded = 99 * m;
        it.snap = Some(super::tool_snapshot(p, &url));
        assert!(!it.past_last_byte());
    }

    /// Without a size there's no share to show: no 0 % (from #384).
    #[test]
    fn no_size_no_percentage() {
        let it = video_item(None, 5_000_000, Status::Downloading);
        assert_eq!(it.fraction(), None);
        assert_eq!(video_item(Some(0), 5, Status::Downloading).fraction(), None);
    }

    /// A video with all its bytes in may still be merging: 99.9 % until the file is finished,
    /// with the real sizes beside it.
    #[test]
    fn a_video_reads_100_only_when_finished() {
        let m = 1 << 20;
        let running = video_item(Some(100 * m), 100 * m, Status::Downloading);
        assert_eq!(running.fraction(), Some(0.999));
        let done = video_item(Some(100 * m), 100 * m, Status::Complete);
        assert_eq!(done.fraction(), Some(1.0));
        let mut file = video_item(Some(100), 100, Status::Downloading);
        file.video = None;
        assert_eq!(file.fraction(), Some(1.0));
    }

    #[test]
    fn a_folder_saved_as_shown_is_under_home() {
        let home = dirs::home_dir().unwrap();
        let dirs = vec![("Video".to_owned(), "Downloads/Video".to_owned())];
        let got = category_folder(Category::Video, Path::new("x"), &dirs);
        assert_eq!(got, home.join("Downloads/Video"));
        assert!(got.is_absolute());
        let whole = home.join("Elsewhere").display().to_string();
        let dirs = vec![("Video".to_owned(), whole.clone())];
        assert_eq!(
            category_folder(Category::Video, Path::new("x"), &dirs),
            PathBuf::from(whole)
        );
    }

    #[test]
    fn categories() {
        assert_eq!(Category::of("a.ZIP"), Category::Compressed);
        assert_eq!(Category::of("movie.part.r01"), Category::Compressed);
        assert_eq!(Category::of("setup.exe"), Category::Programs);
        assert_eq!(Category::of("song.mp3"), Category::Music);
        assert_eq!(Category::of("README"), Category::General);
        assert_eq!(Category::of("x.unknown"), Category::General);
    }

    #[test]
    fn formatting() {
        assert_eq!(size(7_392_461), "7.05 MB");
        assert_eq!(size(500), "500 Bytes");
        assert_eq!(time(14.2), "14 sec");
        assert_eq!(time(82.0), "1 min 22 sec");
        assert_eq!(time(7500.0), "2 hour(s) 5 min");
        assert_eq!(rate(12_187_628.0), "11.623 MB/sec");
    }

    /// The folder shown under the profile (`Downloads/Video`) is under the user's home, not
    /// under the process's working directory (a desktop launch made it `/Downloads/Video`, on
    /// the read-only system volume).
    #[test]
    fn a_category_folder_shown_under_the_profile_is_under_the_home() {
        let home = dirs::home_dir().unwrap();
        let downloads = home.join("Downloads");
        let dirs = vec![("Video".to_string(), "Downloads/Video".to_string())];
        let found = category_folder(Category::Video, &downloads, &dirs);
        assert_eq!(found, home.join("Downloads/Video"));
        assert!(found.is_absolute());
        // An absolute folder is kept as it is.
        let absolute = home.join("Media/Video");
        let dirs = vec![("Video".to_string(), absolute.display().to_string())];
        assert_eq!(
            category_folder(Category::Video, &downloads, &dirs),
            absolute
        );
    }
}

#[cfg(test)]
mod stamp_tests {
    #[test]
    fn stamps_read_back_and_sort_by_time() {
        let apr = super::stamp_time("Apr 02 08:00:00 2027").unwrap();
        let oct = super::stamp_time("Oct 28 21:44:12 2026").unwrap();
        assert!(oct < apr, "October 2026 comes before April 2027");
        assert!(super::stamp_time("not a date").is_none());
    }

    #[test]
    fn english_stamps_follow_the_clock() {
        let at = |s| super::stamp_time(s).unwrap();
        // 24-hour: exactly as stored.
        for s in ["Oct 28 21:44:12 2026", "Jan 01 00:05:09 2027"] {
            assert_eq!(super::stamp_text(at(s), true), s);
        }
        // 12-hour (#392): midnight and noon are 12.
        assert_eq!(
            super::stamp_text(at("Oct 28 21:44:12 2026"), false),
            "Oct 28 9:44:12 PM 2026"
        );
        assert_eq!(
            super::stamp_text(at("Jan 01 00:05:09 2027"), false),
            "Jan 01 12:05:09 AM 2027"
        );
        assert_eq!(
            super::stamp_text(at("Jun 15 12:00:00 2027"), false),
            "Jun 15 12:00:00 PM 2027"
        );
    }
}

/// An address as typed: `www.example.com/a.zip` (no scheme, as browsers take it) is read as
/// https; anything else as written.
pub fn parse_address(typed: &str) -> Result<url::Url, url::ParseError> {
    let typed = typed.trim();
    match url::Url::parse(typed) {
        Err(url::ParseError::RelativeUrlWithoutBase)
            if !typed.contains(char::is_whitespace)
                && typed
                    .split('/')
                    .next()
                    .is_some_and(|host| host.contains('.')) =>
        {
            url::Url::parse(&format!("https://{typed}"))
        }
        other => other,
    }
}

#[cfg(test)]
mod address_tests {
    #[test]
    fn an_address_without_its_scheme_is_https() {
        let u = super::parse_address(" www.example.com/a.zip ").unwrap();
        assert_eq!(u.as_str(), "https://www.example.com/a.zip");
        assert_eq!(super::parse_address("ftp://h/f").unwrap().scheme(), "ftp");
        assert!(super::parse_address("not an address").is_err());
    }
}

/// A finished download whose file was moved or deleted (#342). Not when its folder is gone too:
/// that is more likely a drive that is unplugged, and the file may come back.
pub fn finished_file_gone(status: Status, dest: &std::path::Path) -> bool {
    status == Status::Complete && !dest.exists() && dest.parent().is_some_and(|d| d.is_dir())
}

#[cfg(test)]
mod finished_file_gone_tests {
    use super::*;

    #[test]
    fn only_a_finished_file_gone_from_a_folder_that_is_there() {
        let dir = std::env::temp_dir().join(format!("mdm-342-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let there = dir.join("there.bin");
        std::fs::write(&there, b"x").unwrap();
        assert!(!finished_file_gone(Status::Complete, &there));
        assert!(finished_file_gone(Status::Complete, &dir.join("gone.bin")));
        // Not finished: its file isn't expected yet.
        assert!(!finished_file_gone(Status::Queued, &dir.join("gone.bin")));
        // Its folder is gone too: maybe a drive that is unplugged.
        assert!(!finished_file_gone(
            Status::Complete,
            &dir.join("no").join("gone.bin")
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod helper_connection_tests {
    use super::*;
    use mdm_engine::download::{ConnStatus, ConnView};

    fn progress() -> mdm_tools::video::Progress {
        mdm_tools::video::Progress {
            state: mdm_tools::video::State::Running,
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
        }
    }

    /// A live stream yt-dlp records says no progress: what it has recorded is what's downloaded
    /// (Downloaded, and the rate from its growth); a progress of its own counts when larger.
    #[test]
    fn a_recording_counts_as_downloaded() {
        let page: Url = "https://www.example.com/live".parse().unwrap();
        let mut p = progress();
        p.helper = Some(mdm_tools::streams::Helper::Live);
        p.recorded = 3 << 20;
        let s = tool_snapshot(p.clone(), &page);
        assert_eq!(s.downloaded, 3 << 20);
        assert_eq!(s.connections[0].downloaded, 3 << 20);
        assert!(matches!(s.connections[0].status, ConnStatus::Receiving));
        p.downloaded = 4 << 20;
        assert_eq!(tool_snapshot(p.clone(), &page).downloaded, 4 << 20);
        p.recorded = 0;
        p.downloaded = 0;
        assert_eq!(tool_snapshot(p, &page).downloaded, 0);
    }

    /// A live stream whose recorder has opened its file is receiving, though nothing is written
    /// yet (the file stays 0 bytes until its first block): not "Connecting". Before that, it is
    /// connecting.
    #[test]
    fn a_recorder_that_opened_its_file_is_receiving() {
        let page: Url = "https://www.example.com/live".parse().unwrap();
        let mut p = progress();
        p.helper = Some(mdm_tools::streams::Helper::Live);
        let s = tool_snapshot(p.clone(), &page);
        assert!(matches!(s.connections[0].status, ConnStatus::Connecting));
        p.recording = true;
        let s = tool_snapshot(p, &page);
        assert_eq!((s.downloaded, s.connections[0].downloaded), (0, 0));
        assert!(matches!(s.connections[0].status, ConnStatus::Receiving));
    }

    /// A live stream recorded, stopped and saved: what it shows as downloaded never drops while
    /// it is saved (its run over, its progress 0), and the download limit counts the recording
    /// once, not again as its file comes. A live stream started anew shows its new recording.
    #[test]
    fn a_saved_recording_is_counted_once() {
        use mdm_tools::video::LiveState::{Recording, Saving};
        const M: u64 = 1 << 20;
        // (live_state read before the snapshot, the snapshot's downloaded)
        let looks = [
            // Started: nothing recorded yet.
            (None, 0),
            (Some(Recording), 4 * M),
            (Some(Recording), 10 * M),
            // Stop and save: read as recording, its run over in the snapshot.
            (Some(Recording), 0),
            (Some(Saving), 0),
            // Saved as the snapshot was taken: its file's size.
            (Some(Saving), 10 * M - 1000),
            (None, 10 * M - 1000),
        ];
        let start = std::time::Instant::now();
        let mut quota = crate::quota::Quota::default();
        let mut shown = 0;
        for (n, (live, now)) in looks.into_iter().enumerate() {
            let before = shown;
            shown = shown_downloaded(live, now, shown);
            if live.is_some() {
                assert!(shown >= before, "look {n}: {shown} < {before}");
            }
            quota.record(start, std::iter::once((7, shown)));
        }
        assert_eq!(shown, 10 * M - 1000);
        assert_eq!(
            quota.used(start, std::time::Duration::from_secs(3600)),
            10 * M
        );
        // Not a live stream, or one not recording (yet, again): its job's own figure.
        assert_eq!(shown_downloaded(None, 0, 10 * M), 0);
    }

    /// #793: a video yt-dlp downloads itself shows its one connection, not an empty list; one
    /// MDM's engine downloads shows the engine's connections, every one of them.
    #[test]
    fn the_connections_list_is_never_empty_while_a_video_downloads() {
        let page: Url = "https://www.example.com/watch?v=abc".parse().unwrap();
        let mut p = progress();
        let s = tool_snapshot(p.clone(), &page);
        assert_eq!(s.connections.len(), 1);
        assert!(matches!(s.connections[0].status, ConnStatus::Connecting));
        p.downloaded = 5_000;
        p.speed = 1_000.0;
        p.helper = Some(mdm_tools::streams::Helper::Live);
        let s = tool_snapshot(p.clone(), &page);
        assert_eq!(
            (s.connections[0].id, s.connections[0].downloaded),
            (0, 5_000)
        );
        assert!(matches!(s.connections[0].status, ConnStatus::Receiving));

        let conn = |id| ConnView {
            id,
            downloaded: 100,
            status: ConnStatus::Receiving,
            speed: 10,
        };
        p.helper = None;
        p.engine = Some(mdm_tools::streams::View {
            connections: (0..8).map(conn).collect(),
            ..Default::default()
        });
        let s = tool_snapshot(p.clone(), &page);
        assert_eq!(
            s.connections.iter().map(|c| c.id).collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );

        // Done: no connection left.
        p.engine = None;
        p.state = mdm_tools::video::State::Done("Clip.mp4".into());
        assert!(tool_snapshot(p, &page).connections.is_empty());
    }
}

/// The speed's average and the figure shown (#1176).
#[cfg(test)]
mod speed_tests {
    use super::{SHOWN_EVERY, STILL, Speed, shown_epoch, shown_tick, until_shown};
    use std::time::{Duration, Instant};

    const MB: f64 = 1_048_576.0;

    fn at(t: f64) -> Instant {
        shown_epoch() + Duration::from_secs_f64(t)
    }

    /// Reads `bytes(t)` at each moment of `times`: the average after each, and the figure shown.
    fn run(times: &[f64], bytes: impl Fn(f64) -> u64) -> (Speed, Vec<(f64, f64, f64)>) {
        let mut s = Speed::new(at(0.0), 0);
        let mut seen = Vec::new();
        for &t in times {
            s.read(at(t), bytes(t));
            seen.push((t, s.average(), s.shown()));
        }
        (s, seen)
    }

    /// 1 MB/s for 5 s, then 3 MB/s.
    fn step(t: f64) -> u64 {
        if t < 5.0 {
            (t * MB) as u64
        } else {
            (5.0 * MB + (t - 5.0) * 3.0 * MB) as u64
        }
    }

    fn every(gap: f64, until: f64) -> Vec<f64> {
        (1..)
            .map(|n| n as f64 * gap)
            .take_while(|t| *t <= until + 1e-9)
            .collect()
    }

    /// Frames 25 a second, four a second, twice a second, or uneven: the same bytes make the
    /// same average (the old fixed weight settled in 0.5 s at one and in 6 s at another).
    #[test]
    fn the_same_bytes_make_the_same_average_whatever_the_frames() {
        let uneven: Vec<f64> = (1..)
            .scan(0.0, |t, n: u32| {
                *t += [0.25, 0.31, 0.19, 0.27, 0.42, 0.04][n as usize % 6];
                Some(*t)
            })
            .take_while(|t| *t <= 10.0)
            .chain([10.0])
            .collect();
        let gaps = [
            every(0.04, 10.0),
            every(0.25, 10.0),
            every(0.5, 10.0),
            uneven,
        ];
        for check in [6.5, 10.0] {
            let averages: Vec<f64> = gaps
                .iter()
                .map(|times| {
                    let upto: Vec<f64> = times.iter().copied().filter(|t| *t <= check).collect();
                    run(&upto, step).0.average()
                })
                .collect();
            let (lo, hi) = averages
                .iter()
                .fold((f64::MAX, 0.0f64), |(lo, hi), a| (lo.min(*a), hi.max(*a)));
            assert!(hi / lo < 1.04, "at {check} s: {averages:?}");
        }
    }

    /// It settles on a new pace: about two thirds of the way in three seconds, all of it in 15.
    #[test]
    fn it_settles_on_a_new_pace() {
        let (s, seen) = run(&every(0.25, 20.0), step);
        assert!(
            (s.average() / (3.0 * MB) - 1.0).abs() < 0.01,
            "{}",
            s.average()
        );
        let after_3 = seen.iter().find(|(t, ..)| *t >= 8.0).unwrap().1 / MB;
        assert!((2.1..2.5).contains(&after_3), "{after_3}");
    }

    /// From the first figure on, the average is the speed, not a climb from 0; the wait for the
    /// first byte isn't counted in it.
    #[test]
    fn it_starts_at_the_speed() {
        let (s, _) = run(&every(0.25, 0.5), |t| (t * MB) as u64);
        assert!((s.average() / MB - 1.0).abs() < 1e-6, "{}", s.average());
        let (s, _) = run(&every(0.25, 2.5), |t| ((t - 2.0).max(0.0) * MB) as u64);
        assert!((s.average() / MB - 1.0).abs() < 0.15, "{}", s.average());
    }

    /// A steady download whose bytes come in bursts (64 KB at a time, read at uneven frames)
    /// shows a steady figure: within 3 % of its speed once it has had three seconds.
    #[test]
    fn steady_bytes_show_a_steady_figure() {
        let chunk = 64 * 1024;
        let bursts = |t: f64| ((t * 2.0 * MB) as u64 / chunk) * chunk;
        let times: Vec<f64> = (1..)
            .scan(0.0, |t, n: u32| {
                *t += [0.25, 0.21, 0.3, 0.04, 0.33][n as usize % 5];
                Some(*t)
            })
            .take_while(|t| *t <= 20.0)
            .collect();
        let (_, seen) = run(&times, bursts);
        for (t, _, shown) in seen.iter().filter(|(t, ..)| *t >= 3.0) {
            assert!((shown / (2.0 * MB) - 1.0).abs() < 0.03, "{shown} at {t}");
        }
    }

    /// The figure shown changes on the clock's ticks, twice a second, however often it is read;
    /// the average moves under it meanwhile.
    #[test]
    fn the_figure_shown_changes_twice_a_second() {
        // A speed that wobbles about 1 MB/s.
        let wobble = |t: f64| ((t + 0.5 * (1.0 - t.cos())) * MB) as u64;
        let (_, seen) = run(&every(0.04, 5.0), wobble);
        let changes: Vec<_> = seen.windows(2).filter(|w| w[0].2 != w[1].2).collect();
        // One each tick after the first.
        let ticks = (5.0 / SHOWN_EVERY) as usize;
        assert!((ticks - 1..=ticks).contains(&changes.len()), "{changes:?}");
        for w in &changes {
            assert_ne!(shown_tick(at(w[0].0)), shown_tick(at(w[1].0)), "{w:?}");
        }
        // Between ticks the figure stays as it is.
        let within: Vec<_> = seen
            .iter()
            .filter(|(t, ..)| (2.1..2.45).contains(t))
            .collect();
        assert!(within.iter().all(|x| x.2 == within[0].2));
        assert!(within.first().unwrap().1 != within.last().unwrap().1);
        // The next frame is asked for as the clock ticks.
        let wait = until_shown(at(2.3)).as_secs_f64();
        assert!((0.2..0.21).contains(&wait), "{wait}");
        assert_eq!(
            shown_tick(at(2.3) + until_shown(at(2.3))),
            shown_tick(at(2.6))
        );
    }

    /// The time left goes with the figure shown: from the bytes done when it was read.
    #[test]
    fn the_time_left_goes_with_the_figure_shown() {
        let (s, _) = run(&every(0.04, 4.02), |t| (t * MB) as u64);
        let left = s.seconds_left((10.0 * MB) as u64).unwrap();
        assert!((left - 6.0).abs() < 0.05, "{left}");
        assert_eq!(Speed::new(at(0.0), 0).seconds_left(100), None);
    }

    /// Nothing for a while: 0, not what is left of the average. Every byte in (#736): 0 at once.
    /// Fewer bytes than before (a connection's dropped, a file started over): the average goes
    /// on, no jump.
    #[test]
    fn a_stall_an_end_and_a_start_over() {
        let mut times = every(0.25, 3.0);
        times.extend(every(0.25, STILL + 1.0).into_iter().map(|t| t + 3.0));
        let (s, _) = run(&times, |t| (t.min(3.0) * MB) as u64);
        assert_eq!((s.average(), s.shown()), (0.0, 0.0));

        let (mut s, _) = run(&every(0.25, 3.0), |t| (t * MB) as u64);
        s.clear(at(3.1), (3.1 * MB) as u64);
        assert_eq!((s.average(), s.shown()), (0.0, 0.0));

        let (mut s, _) = run(&every(0.25, 3.0), |t| (t * MB) as u64);
        s.read(at(3.25), 1000);
        assert!((s.average() / MB - 1.0).abs() < 1e-6, "{}", s.average());
        s.read(at(3.5), 1000 + MB as u64 / 4);
        assert!((s.average() / MB - 1.0).abs() < 1e-6, "{}", s.average());
    }
}

/// Which speed limit a download's Transfer rate names, and how (#1191).
#[cfg(test)]
mod limit_tests {
    use super::{Limit, limit_rate, limiter_hint};

    const KB: u64 = 1024;

    #[test]
    fn its_own_limit_binds_until_the_shared_one_is_tighter() {
        assert_eq!(Limit::binding(None, None, 1), None);
        assert_eq!(
            Limit::binding(Some(10 * KB), None, 3),
            Some(Limit::One(10 * KB))
        );
        assert_eq!(
            Limit::binding(None, Some(500 * KB), 3),
            Some(Limit::All(500 * KB))
        );
        // One download at 10 KB/s under 500 KB/s for all: its own limit holds it.
        assert_eq!(
            Limit::binding(Some(10 * KB), Some(500 * KB), 1),
            Some(Limit::One(10 * KB))
        );
        // 50 of them at 10 KB/s fill 500 KB/s exactly: still their own.
        assert_eq!(
            Limit::binding(Some(10 * KB), Some(500 * KB), 50),
            Some(Limit::One(10 * KB))
        );
        // 51 would take more than 500 KB/s together: the shared limit holds them.
        assert_eq!(
            Limit::binding(Some(10 * KB), Some(500 * KB), 51),
            Some(Limit::All(500 * KB))
        );
        // Its own above the shared one: the shared one, even alone.
        assert_eq!(
            Limit::binding(Some(800 * KB), Some(500 * KB), 1),
            Some(Limit::All(500 * KB))
        );
        // No count (asked before it runs) counts it alone; no overflow at the top.
        assert_eq!(
            Limit::binding(Some(10 * KB), Some(500 * KB), 0),
            Some(Limit::One(10 * KB))
        );
        assert_eq!(
            Limit::binding(Some(u64::MAX), Some(500 * KB), 2),
            Some(Limit::All(500 * KB))
        );
    }

    #[test]
    fn limits_read_as_typed() {
        assert_eq!(limit_rate(512 * KB), "512 KB/sec");
        assert_eq!(limit_rate(2048 * KB), "2048 KB/sec");
        // Below a kilobyte (a share of the limit for all): never "0 KB/sec".
        assert_eq!(limit_rate(100), "1 KB/sec");
    }

    #[test]
    fn transfer_rate_tip_and_hint() {
        assert_eq!(
            Limit::One(512 * KB).after("511.998 KB/sec"),
            "511.998 KB/sec (limit: 512 KB/sec)"
        );
        assert_eq!(
            Limit::All(1024 * KB).after("340.112 KB/sec"),
            "340.112 KB/sec (limit for all downloads: 1024 KB/sec)"
        );
        assert_eq!(
            Limit::One(10 * KB).tip(),
            "Speed Limiter: at most 10 KB/sec for this download"
        );
        assert_eq!(
            Limit::All(500 * KB).tip(),
            "Speed Limiter: 500 KB/sec for all downloads together"
        );
        assert_eq!(limiter_hint(None, None), None);
        assert_eq!(
            limiter_hint(Some(10 * KB), None).as_deref(),
            Some("Speed Limiter: 10 KB/sec per download")
        );
        assert_eq!(
            limiter_hint(None, Some(500 * KB)).as_deref(),
            Some("Speed Limiter: 500 KB/sec for all downloads")
        );
        assert_eq!(
            limiter_hint(Some(10 * KB), Some(500 * KB)).as_deref(),
            Some("Speed Limiter: 10 KB/sec per download, 500 KB/sec for all")
        );
    }
}
