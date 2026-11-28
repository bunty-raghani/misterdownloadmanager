//! Torrents in the app (M15, D-014). One librqbit session for the whole app, started the first
//! time a torrent needs it (no listening port or DHT for people who never use torrents). Each
//! torrent download is a [`Run`]: it starts the session if needed, adds or resumes its torrent,
//! and reads its progress for the list and the progress window, as the engine's downloads do.
//!
//! The download list is the truth: on start, torrents the session restored but the list no
//! longer has are forgotten (their files stay), and the rest wait paused until started.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

use mdm_engine::Snapshot;
use mdm_engine::download::State;
use mdm_torrent::{Config, Phase, Torrents};
use serde::{Deserialize, Serialize};

/// What a torrent download keeps in the list (downloads.json). `Eq` is not derived: its live
/// `up_speed` is an `f64`, and nothing compares a whole spec for equality.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TorrentSpec {
    /// The info hash, 40 hex digits; the .torrent itself is in [`torrent_file`].
    pub hash: String,
    /// The chosen files (indexes into the torrent's list); None: all of them.
    pub files: Option<Vec<usize>>,
    /// Where its files go (a multi-file torrent's own folder included).
    pub folder: PathBuf,
    /// Whether Add torrent adds its own outer folder. Old downloads retain that policy.
    /// `folder` is already resolved and is used directly when resuming, either way.
    #[serde(default = "subfolder_by_default")]
    pub create_subfolder: bool,
    /// Authority to delete objects created by this download. Missing on older rows.
    #[serde(default)]
    pub ownership: Option<crate::torrent_ownership::Ownership>,
    /// Bytes sent to peers, in all its runs (librqbit counts only the current one: from 0
    /// again at each resume and each start of the app). Kept by [`Accountant`].
    #[serde(default)]
    pub uploaded: u64,
    /// Seconds it has spent sharing once finished, in all its runs.
    #[serde(default)]
    pub seeded_secs: u64,
    /// Peers connected to it now (#357), for the list's "Peers" column; every connected peer,
    /// seeders and leechers alike. Live only: never saved, and zero whenever there is no torrent
    /// session or the session has no numbers for it.
    #[serde(skip)]
    pub peers: usize,
    /// What it sends to peers right now, bytes/s (#357), for the list's "Upload speed" column.
    /// Live only, like [`peers`](Self::peers): never saved, zero without a session or stats.
    #[serde(skip)]
    pub up_speed: f64,
    /// Its own upload limit, bytes/s (None: only Torrent settings' limit for all torrents).
    #[serde(default)]
    pub upload_limit: Option<u64>,
    /// Downloaded in order (#39): its files' start first, to watch a video while it
    /// downloads. Off: the rarest pieces first, as BitTorrent clients usually do (better for
    /// the swarm, and no slower). Torrents kept from before have it off.
    #[serde(default)]
    pub in_order: bool,
    /// In order: each file's first and last pieces first (a video's index is often at its
    /// end, and a player reads it before anything plays).
    #[serde(default = "first_and_last_first")]
    pub ends_first: bool,
}

fn subfolder_by_default() -> bool {
    true
}

fn first_and_last_first() -> bool {
    true
}

impl TorrentSpec {
    /// A multi-file download whose row points to a shared destination, not an owned folder.
    pub fn shares_folder(&self, dest: &Path) -> bool {
        !self.create_subfolder && self.folder == dest
    }

    /// The order its pieces are downloaded in (#39).
    pub fn order(&self) -> mdm_torrent::PieceOrder {
        if self.in_order {
            mdm_torrent::PieceOrder::InOrder {
                first_and_last_first: self.ends_first,
            }
        } else {
            mdm_torrent::PieceOrder::RarestFirst
        }
    }
}

/// Torrent settings' seeding limits (`Settings::sharing_limits`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SharingLimits {
    /// Stop at this share ratio (sent ÷ what is there).
    pub ratio: Option<f64>,
    /// Stop after this many seconds of sharing once finished.
    pub secs: Option<u64>,
}

/// Which seeding limit a finished torrent reached.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reached {
    /// The share ratio it reached.
    Ratio(f64),
    /// The seconds it shared.
    Time(u64),
}

/// Whether `spec`, with `done` bytes there, has reached a seeding limit (the ratio first).
pub fn sharing_limit_reached(
    spec: &TorrentSpec,
    done: u64,
    limits: SharingLimits,
) -> Option<Reached> {
    let ratio = share_ratio(spec.uploaded, done);
    if let Some(r) = limits.ratio
        && done > 0
        && ratio >= r
    {
        return Some(Reached::Ratio(ratio));
    }
    if let Some(s) = limits.secs
        && spec.seeded_secs >= s
    {
        return Some(Reached::Time(spec.seeded_secs));
    }
    None
}

/// No session is running: nothing is connected and nothing is moving, whatever the list last
/// showed (#357).
pub fn clear_live<'a>(specs: impl Iterator<Item = &'a mut TorrentSpec>) {
    specs.for_each(|t| {
        t.peers = 0;
        t.up_speed = 0.0;
    });
}

/// A torrent's share ratio: what it sent to peers against what it has. Zero while it has nothing.
pub fn share_ratio(sent: u64, done: u64) -> f64 {
    if done == 0 {
        0.0
    } else {
        sent as f64 / done as f64
    }
}

/// [`share_ratio`] as the interface writes it, in the list and the progress window's General tab:
/// two decimals, `0.00` while it has nothing.
pub fn share_ratio_text(sent: u64, done: u64) -> String {
    crate::i18n::decimal(share_ratio(sent, done), 2)
}

/// Adds up what each torrent sent, run after run: librqbit's count starts at 0 again each time
/// a torrent resumes (and is 0 while it is paused), so a smaller count than the last one seen
/// is all new.
#[derive(Default)]
pub struct Accountant {
    seen: std::collections::HashMap<String, u64>,
}

impl Accountant {
    /// The bytes `hash` sent since it was last seen; `live` is librqbit's count now.
    pub fn sent(&mut self, hash: &str, live: u64) -> u64 {
        let last = self.seen.insert(hash.to_owned(), live);
        match last {
            Some(last) if live >= last => live - last,
            // First seen in this run of the app, or a new count since: all of it is new.
            _ => live,
        }
    }

    /// Forget torrents no longer in the list.
    pub fn keep(&mut self, hashes: &HashSet<String>) {
        self.seen.retain(|h, _| hashes.contains(h));
    }
}

/// Our torrent data: librqbit's resume data and DHT nodes, and a copy of each .torrent.
pub fn state_dir() -> Option<PathBuf> {
    mdm_ipc::data_dir().map(|d| d.join("torrents"))
}

pub fn torrent_file(hash: &str) -> Option<PathBuf> {
    state_dir().map(|d| d.join("files").join(format!("{hash}.torrent")))
}

pub fn save_torrent(hash: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = torrent_file(hash)
        // i18n: an error: the app has no folder for its own data
        .ok_or_else(|| std::io::Error::other(crate::i18n::t("no data folder").to_owned()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Through a temporary file: Add trackers rewrites it, and a half-written copy would lose
    // the torrent.
    let tmp = path.with_extension("torrent.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)
}

/// A change of a torrent's kept files, beside the window.
type Change = Box<dyn FnOnce() + Send>;

/// MDM's own files for the torrents of the list, in its data folder: the .torrent kept for each
/// and the record of a single file's new name (#517). A torrent's Delete changes them beside the
/// window, on a thread of their own, one change after the other (an antivirus or a busy disk
/// could hold the window on them); a torrent added while changes are under way has its files
/// written after them, so a Delete never takes away the files of the same torrent added again,
/// and what reads them for a torrent ([`read_kept`]) waits for that torrent's changes first.
pub struct KeptFiles {
    /// None when no thread could be made: then each change is made on the caller's.
    tx: Option<mpsc::Sender<(String, Change)>>,
    /// The changes handed over and not made yet, by torrent (lowercase info hash).
    pending: Arc<(Mutex<HashMap<String, usize>>, Condvar)>,
}

impl KeptFiles {
    fn new() -> KeptFiles {
        let pending: Arc<(Mutex<HashMap<String, usize>>, Condvar)> = Arc::default();
        let (tx, rx) = mpsc::channel::<(String, Change)>();
        let spawned = std::thread::Builder::new()
            .name("torrent files".into())
            .spawn({
                let pending = pending.clone();
                move || {
                    for (hash, change) in rx {
                        let began = std::time::Instant::now();
                        change();
                        if began.elapsed() >= std::time::Duration::from_secs(1) {
                            tracing::info!(
                                "torrents: a kept file's change took {} ms, beside the window",
                                began.elapsed().as_millis()
                            );
                        }
                        KeptFiles::done(&pending, &hash);
                    }
                }
            });
        let tx = match spawned {
            Ok(_) => Some(tx),
            Err(e) => {
                tracing::warn!("torrents: no thread for the kept files ({e}); changed at once");
                None
            }
        };
        KeptFiles { tx, pending }
    }

    fn done(pending: &(Mutex<HashMap<String, usize>>, Condvar), hash: &str) {
        let (map, cv) = pending;
        let mut map = map.lock().unwrap();
        if let Some(n) = map.get_mut(hash) {
            *n -= 1;
            if *n == 0 {
                map.remove(hash);
            }
        }
        cv.notify_all();
    }

    /// Make `change` to torrent `hash`'s kept files after the changes before. Returns at once.
    pub fn change(&self, hash: &str, change: impl FnOnce() + Send + 'static) {
        let hash = hash.to_ascii_lowercase();
        *self
            .pending
            .0
            .lock()
            .unwrap()
            .entry(hash.clone())
            .or_default() += 1;
        let job = (hash, Box::new(change) as Change);
        let (hash, change) = match &self.tx {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        change();
        KeptFiles::done(&self.pending, &hash);
    }

    /// Whether any change is under way (a torrent added now has its files written after them).
    pub fn changing(&self) -> bool {
        !self.pending.0.lock().unwrap().is_empty()
    }

    /// Whether torrent `hash`'s files are being changed. Never waits.
    pub fn changing_for(&self, hash: &str) -> bool {
        self.pending
            .0
            .lock()
            .unwrap()
            .contains_key(&hash.to_ascii_lowercase())
    }

    /// Wait until torrent `hash`'s changes are made (on a thread that may wait).
    pub fn settle(&self, hash: &str) {
        let hash = hash.to_ascii_lowercase();
        let (map, cv) = &*self.pending;
        let map = map.lock().unwrap();
        let _unused = cv.wait_while(map, |m| m.contains_key(&hash)).unwrap();
    }
}

/// The kept files' changes, for the whole app.
pub fn kept_files() -> &'static KeptFiles {
    static KEPT: OnceLock<KeptFiles> = OnceLock::new();
    KEPT.get_or_init(KeptFiles::new)
}

/// The .torrent kept for `hash`, read once its changes under way are made ([`KeptFiles`]), on
/// the torrents' own tasks.
pub async fn read_kept(hash: &str) -> Result<Vec<u8>, String> {
    if kept_files().changing_for(hash) {
        let h = hash.to_owned();
        let _ = tokio::task::spawn_blocking(move || kept_files().settle(&h)).await;
    }
    let file = torrent_file(hash).ok_or_else(|| crate::i18n::t("no data folder").to_owned())?;
    std::fs::read(&file).map_err(|_| {
        // i18n: an error: the copy of the .torrent file the app keeps is missing
        crate::i18n::t("the .torrent file kept for it is gone").to_owned()
    })
}

/// A magnet link for a torrent in the list, with its trackers, from the .torrent kept for it.
pub fn magnet_of(hash: &str) -> Option<String> {
    let bytes = std::fs::read(torrent_file(hash)?).ok()?;
    mdm_torrent::magnet_link(&bytes)
}

/// Whether a web address is a .torrent file's (Add URL, the browser): its path ends in
/// `.torrent`, or the browser named the file so or gave its type.
pub fn is_torrent_url(url: &url::Url, name: Option<&str>, mime: Option<&str>) -> bool {
    let ends = |s: &str| s.to_ascii_lowercase().ends_with(".torrent");
    matches!(url.scheme(), "http" | "https")
        && (ends(url.path())
            || name.is_some_and(ends)
            || mime.is_some_and(|m| {
                m.split(';')
                    .next()
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/x-bittorrent"))
            }))
}

/// A .torrent on the web: its address and the request it goes with (`App::request_parts`: the
/// browser's cookies and Referer, a site login, Proxy / Socks).
#[derive(Clone)]
pub struct Remote {
    pub url: url::Url,
    pub parts: crate::app::RequestParts,
}

/// Fetch a .torrent from the web with MDM's own downloader, as `remote` says; at most
/// `mdm_torrent::MAX_TORRENT_FILE` (a bigger file isn't a torrent, and is stopped there).
pub async fn fetch_torrent(remote: Remote) -> Result<Vec<u8>, String> {
    let not = || crate::errors::torrent(&mdm_torrent::TorrentError::NotATorrent);
    fetch_small(remote, mdm_torrent::MAX_TORRENT_FILE)
        .await?
        .ok_or_else(not)
}

/// Fetch a small file from the web (a .torrent, an RSS feed) with MDM's own downloader, as
/// `remote` says, into memory: None when it is bigger than `max` bytes (stopped there). Its
/// temporary file, resume state and part file are removed.
pub async fn fetch_small(remote: Remote, max: u64) -> Result<Option<Vec<u8>>, String> {
    use mdm_engine::download::{Destination, Download, Request};
    let dir = state_dir().ok_or(crate::i18n::t("no data folder"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "fetched-{}.part-of-mdm",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let dl = Download::start(
        Request {
            url: remote.url,
            headers: remote.parts.headers,
            dest: Destination::File(path.clone()),
        },
        remote.parts.opts,
    );
    let too_big = loop {
        let s = dl.snapshot();
        if s.size.is_some_and(|z| z > max) || s.downloaded > max {
            dl.cancel();
            break true;
        }
        if matches!(s.state, State::Done | State::Failed | State::Cancelled) {
            break false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let res = dl.wait().await;
    let bytes = if too_big {
        Ok(None)
    } else {
        match res {
            Ok(_) => std::fs::read(&path)
                .map(|b| (b.len() as u64 <= max).then_some(b))
                .map_err(|e| e.to_string()),
            Err(e) => Err(crate::errors::describe(&e)),
        }
    };
    // The file, its resume state, and the part file a stopped one leaves.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(mdm_engine::state::state_path(&path));
    let _ = std::fs::remove_file(mdm_engine::naming::part_path(&path));
    bytes
}

/// The blocklist as MDM converted it (mdm_torrent::blocklist), in the torrent data folder.
pub const BLOCKLIST_FILE: &str = "blocklist.p2p";
/// The address it was fetched from (another address: fetched again).
const BLOCKLIST_SOURCE: &str = "blocklist.source";
/// How often a blocklist is fetched again.
pub const BLOCKLIST_DAYS: u64 = 7;

/// When the blocklist at `url` was last converted (None: never, or from another address).
pub fn blocklist_updated(url: &str) -> Option<std::time::SystemTime> {
    let dir = state_dir()?;
    let source = std::fs::read_to_string(dir.join(BLOCKLIST_SOURCE)).ok()?;
    (source.trim() == url.trim())
        .then(|| {
            std::fs::metadata(dir.join(BLOCKLIST_FILE))
                .ok()?
                .modified()
                .ok()
        })
        .flatten()
}

/// Fetch and convert the blocklist at `url` (#40) when it was never converted, comes from
/// another address now, or is older than `BLOCKLIST_DAYS`; in the background, one at a time
/// (`busy`), with MDM's own downloader (Proxy / Socks applies). A list that isn't one keeps the
/// last good one. The torrent session reads it when it starts.
pub fn refresh_blocklist(
    rt: &tokio::runtime::Handle,
    url: &str,
    busy: &Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let url = url.trim().to_owned();
    let Some(dir) = state_dir() else { return };
    let Ok(parsed) = url::Url::parse(&url) else {
        return;
    };
    let fresh = blocklist_updated(&url)
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(BLOCKLIST_DAYS * 86_400));
    if url.is_empty() || fresh || busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let busy = busy.clone();
    rt.spawn(async move {
        let _ = std::fs::create_dir_all(&dir);
        let download = dir.join("blocklist.download");
        let fetched =
            mdm_engine::fetch(parsed, &download, crate::settings::own_net(), |_, _| {}).await;
        match fetched {
            Ok(_) => match std::fs::read(&download)
                .ok()
                .as_deref()
                .and_then(mdm_torrent::blocklist::convert)
            {
                Some(list) => {
                    let tmp = dir.join(format!("{BLOCKLIST_FILE}.tmp"));
                    if std::fs::write(&tmp, &list.text).is_ok()
                        && std::fs::rename(&tmp, dir.join(BLOCKLIST_FILE)).is_ok()
                    {
                        let _ = std::fs::write(dir.join(BLOCKLIST_SOURCE), &url);
                        tracing::info!(
                            "blocklist: {} ranges ({} lines skipped)",
                            list.ranges,
                            list.skipped
                        );
                    }
                }
                None => tracing::warn!("blocklist: {url} is not a list MDM can read"),
            },
            Err(e) => tracing::warn!("blocklist: {e}"),
        }
        let _ = std::fs::remove_file(&download);
        let _ = std::fs::remove_file(mdm_engine::state::state_path(&download));
        busy.store(false, Ordering::SeqCst);
    });
}

/// How the torrent session is held to the network adapter as it is (#35, #143): None without
/// one chosen; else why torrents can't start (it isn't connected).
pub fn route_bind(route: &mdm_engine::Route) -> Result<Option<mdm_torrent::Bind>, String> {
    let down = |label: &str| {
        crate::i18n::fmt(
            // i18n: an error: the network adapter chosen in Options (a VPN's, usually) is off or gone; {adapter} its name
            crate::i18n::t("the network adapter {adapter} is not connected"),
            &[("adapter", label)],
        )
    };
    match route {
        mdm_engine::Route::Any => Ok(None),
        mdm_engine::Route::Down { label } => Err(down(label)),
        // Windows: its address (strong host model, see mdm_engine::net); IPv4 when it has one.
        #[cfg(windows)]
        mdm_engine::Route::Bound(b) => {
            b.v4.map(std::net::IpAddr::V4)
                .or(b.v6.map(std::net::IpAddr::V6))
                .map(|ip| Some(mdm_torrent::Bind::Address(ip)))
                .ok_or_else(|| down(&b.label))
        }
        // Linux and macOS: its device.
        #[cfg(not(windows))]
        mdm_engine::Route::Bound(b) => Ok(Some(mdm_torrent::Bind::Device {
            name: b.name.clone(),
            v4_only: b.v6.is_none(),
        })),
    }
}

/// The app's one session, started on first use, as Torrent settings say (`set_config`); it
/// stops when no torrent needs it (`stop_if_idle`), and a new one takes the settings of then.
#[derive(Clone)]
pub struct Host {
    cell: Arc<tokio::sync::Mutex<Option<Arc<Torrents>>>>,
    /// The settings the next session starts with (None: they can't be met, see
    /// Settings::torrent_config) and those of the one running.
    wanted: Arc<Mutex<Option<Config>>>,
    running: Arc<Mutex<Option<Config>>>,
    /// The global cap (D-012), bytes/s: every torrent together stays under it (`set_cap`).
    cap: Arc<Mutex<Option<u64>>>,
    /// Torrent settings' upload limit for all torrents together, bytes/s (`set_upload`).
    upload: Arc<Mutex<Option<u64>>>,
    /// The torrents an action is busy with (`act`): taken out of the session and added again,
    /// or paused a moment. Their `Run` shows them checking meanwhile.
    busy: Arc<Mutex<HashSet<String>>>,
    /// What each finished action says, by info hash, for its event log (`take_outcomes`).
    outcomes: Arc<Mutex<Vec<Outcome>>>,
    deleting: Arc<Mutex<HashMap<String, Vec<PathBuf>>>>,
    initializing: Arc<Mutex<HashMap<String, OwnershipReady>>>,
}

type OwnershipReady = tokio::sync::watch::Receiver<Option<Result<(), String>>>;

struct DeleteRequest {
    spec: TorrentSpec,
    dest: PathBuf,
    files: bool,
    protected: Vec<PathBuf>,
    name: String,
    file: Option<PathBuf>,
    data: Option<PathBuf>,
}

/// A torrent action (#39): the list's right-click menu (Torrent), or the Files tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Read and check every piece again.
    Recheck,
    /// Ask the trackers for peers again now.
    Reannounce,
    /// Trackers to add, by address (kept in its .torrent, `torrent_file`).
    AddTrackers(Vec<String>),
    /// The files to download from now on (indexes into the torrent's list); `pause`: it isn't
    /// running in the list (stopped, or finished and shared), so it is held paused, not left to
    /// fetch the files out of sight.
    Choose { files: Vec<usize>, pause: bool },
}

/// A finished action: on which torrent, what was asked, and what it did or why it couldn't.
pub struct Outcome {
    pub hash: String,
    pub act: Act,
    pub res: Result<Done, String>,
}

/// What a finished action did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// The check started (the app notes what it found once it ends).
    Rechecking,
    Reannounced,
    /// How many trackers were new.
    TrackersAdded(usize),
    Chosen,
}

impl Host {
    pub fn new() -> Self {
        Host {
            cell: Arc::new(tokio::sync::Mutex::new(None)),
            wanted: Arc::new(Mutex::new(state_dir().map(|dir| {
                let mut cfg = Config::new(dir);
                cfg.require_authorization = true;
                cfg
            }))),
            running: Arc::new(Mutex::new(None)),
            cap: Arc::new(Mutex::new(None)),
            upload: Arc::new(Mutex::new(None)),
            busy: Arc::new(Mutex::new(HashSet::new())),
            outcomes: Arc::new(Mutex::new(Vec::new())),
            deleting: Arc::new(Mutex::new(HashMap::new())),
            initializing: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Do `act` on `spec`'s torrent, in the background; its outcome comes back through
    /// `take_outcomes`. `known`: the list's torrents (for a session it starts).
    pub fn act(
        &self,
        rt: &tokio::runtime::Handle,
        known: HashSet<String>,
        spec: &TorrentSpec,
        dest: PathBuf,
        protected: Vec<PathBuf>,
        act: Act,
    ) {
        let (host, spec) = (self.clone(), spec.clone());
        self.busy.lock().unwrap().insert(spec.hash.clone());
        rt.spawn(async move {
            let res = host
                .act_now(known, &spec, &dest, protected, act.clone())
                .await;
            host.busy.lock().unwrap().remove(&spec.hash);
            host.outcomes.lock().unwrap().push(Outcome {
                hash: spec.hash,
                act,
                res,
            });
        });
    }

    async fn act_now(
        &self,
        known: HashSet<String>,
        spec: &TorrentSpec,
        dest: &Path,
        protected: Vec<PathBuf>,
        act: Act,
    ) -> Result<Done, String> {
        let err = |e: mdm_torrent::TorrentError| crate::errors::torrent(&e);
        let hash = spec.hash.as_str();
        match act {
            Act::AddTrackers(more) => {
                let bytes = read_kept(hash).await?;
                let with = mdm_torrent::trackers::add(&bytes, &more)
                    .ok_or_else(|| err(mdm_torrent::TorrentError::NotATorrent))?;
                let added = mdm_torrent::trackers::listed(&with).len()
                    - mdm_torrent::trackers::listed(&bytes).len();
                if with != bytes {
                    save_torrent(hash, &with).map_err(|e| e.to_string())?;
                    // The running session's copy too; a session isn't started for it (the
                    // next one adds it from the file).
                    if let Some(t) = self.get().filter(|t| t.has(hash)) {
                        t.readd(hash, &with, false).await.map_err(err)?;
                    }
                }
                Ok(Done::TrackersAdded(added))
            }
            Act::Recheck => {
                let t = self.session(known).await?;
                let bytes = read_kept(hash).await?;
                if t.has(hash) {
                    t.readd(hash, &bytes, true).await.map_err(err)?;
                } else {
                    // Not in the session: added paused, which checks what is there. The engine
                    // opens (and sizes) every file at its paths when it checks, so they are
                    // checked and handed over first, as at Start.
                    prepare_storage(self, spec, dest, protected).await?;
                    t.add(&bytes, &spec.folder, spec.files.clone(), Vec::new(), true)
                        .await
                        .map_err(|e| {
                            // Not added: nothing will take the handles it was given.
                            revoke_grant(hash);
                            err(e)
                        })?;
                }
                Ok(Done::Rechecking)
            }
            Act::Reannounce => {
                if let Some(t) = self.get().filter(|t| t.has(hash)) {
                    t.reannounce(hash).await.map_err(err)?;
                }
                Ok(Done::Reannounced)
            }
            Act::Choose { files, pause } => {
                // A file ticked now whose bytes are in a parts file (or kept in one by the
                // running storage) has no file yet: it must be made and get them, even on a
                // finished torrent whose session stopped while idle, so the session is started
                // for it. Otherwise a stopped session only keeps the choice (Resume uses it).
                let reopen = chosen_with_parts(
                    state_dir().map(|d| d.join("session")).as_deref(),
                    hash,
                    &files,
                );
                let running = match self.get() {
                    Some(t) => Some(t),
                    None if reopen => Some(self.session(known).await?),
                    None => None,
                };
                if let Some(t) = running {
                    if !t.has(hash) {
                        let bytes = read_kept(hash).await?;
                        // Its ticked files are made (or checked) and handed over as at Start,
                        // before it is added; a ticked file's bytes in a parts file go into it
                        // as the engine opens it.
                        prepare_storage(self, spec, dest, protected).await?;
                        t.add(&bytes, &spec.folder, Some(files.clone()), Vec::new(), pause)
                            .await
                            .map_err(|e| {
                                revoke_grant(hash);
                                err(e)
                            })?;
                        return Ok(Done::Chosen);
                    }
                    // In the session: librqbit doesn't open its files again when the choice
                    // changes, so the file is made (checked as at Start) and the torrent opened
                    // again on it, its pieces kept; what the parts file held goes into it.
                    if reopen {
                        prepare_storage(self, spec, dest, protected).await?;
                    }
                    t.choose(hash, &files.iter().copied().collect())
                        .await
                        .map_err(err)?;
                    if reopen {
                        let bytes = read_kept(hash).await?;
                        t.readd(hash, &bytes, false).await.map_err(|e| {
                            revoke_grant(hash);
                            err(e)
                        })?;
                    }
                    if pause {
                        if t.stats(hash).is_ok_and(|s| s.phase == Phase::Finished) {
                            t.remove(hash, false).await.map_err(err)?;
                        } else {
                            t.pause(hash).await.map_err(err)?;
                        }
                    }
                }
                Ok(Done::Chosen)
            }
        }
    }

    /// Whether an action is busy with this torrent.
    pub fn busy(&self, hash: &str) -> bool {
        self.busy.lock().unwrap().contains(hash) || self.deleting.lock().unwrap().contains_key(hash)
    }

    pub fn deletion_claims(&self) -> Vec<PathBuf> {
        self.deleting
            .lock()
            .unwrap()
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    pub fn initialize_ownership(
        &self,
        rt: &tokio::runtime::Handle,
        spec: &TorrentSpec,
        dest: &Path,
        existing: Vec<crate::torrent_ownership::Consent>,
    ) {
        let Some(owner) = spec.ownership.clone() else {
            return;
        };
        let (tx, rx) = tokio::sync::watch::channel(None);
        self.initializing
            .lock()
            .unwrap()
            .insert(owner.token.clone(), rx);
        let (folder, dest) = (spec.folder.clone(), dest.to_owned());
        let (initializing, token) = (self.initializing.clone(), owner.token.clone());
        rt.spawn_blocking(move || {
            let result = state_dir()
                .ok_or_else(|| crate::i18n::t("no data folder").to_owned())
                .and_then(|data| {
                    owner
                        .initialize(&data, &folder, &dest, &existing)
                        .map_err(|e| e.to_string())
                });
            tx.send_replace(Some(result));
            // Whoever waits has the answer; a failure doesn't stay until MDM is restarted.
            initializing.lock().unwrap().remove(&token);
        });
    }

    async fn ownership_ready(
        &self,
        owner: &crate::torrent_ownership::Ownership,
    ) -> Result<(), String> {
        let receiver = self.initializing.lock().unwrap().get(&owner.token).cloned();
        if let Some(mut receiver) = receiver {
            loop {
                if let Some(result) = receiver.borrow().clone() {
                    return result;
                }
                receiver.changed().await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Rename a finished torrent's file or folder on disk through the host's busy guard.
    ///
    /// Returns `Err(hold)` when another host action is currently busy with `spec.hash` so the
    /// caller can retry on a later tick. When `keep_sharing` is on:
    /// - a renamed multi-file folder torrent is re-attached at the new folder with its saved
    ///   piece bitmap, rate limits and piece order;
    /// - if the rename fails after detaching, the torrent is re-attached at `spec.folder` so
    ///   sharing does not stop.
    #[allow(clippy::too_many_arguments)]
    pub fn rename_finished(
        &self,
        rt: &tokio::runtime::Handle,
        known: HashSet<String>,
        spec: &TorrentSpec,
        from: PathBuf,
        initial_to: PathBuf,
        hold: Option<mdm_engine::naming::Reservation>,
        is_dir: bool,
        keep_sharing: bool,
        tx: std::sync::mpsc::Sender<(Result<PathBuf, String>, Option<String>)>,
    ) -> Result<(), Option<mdm_engine::naming::Reservation>> {
        {
            let mut busy = self.busy.lock().unwrap();
            if busy.contains(&spec.hash) {
                return Err(hold);
            }
            busy.insert(spec.hash.clone());
        }
        let (host, spec) = (self.clone(), spec.clone());
        rt.spawn(async move {
            let mut sharing_error: Option<String> = None;
            let session = if keep_sharing && is_dir {
                match host.session(known).await {
                    Ok(t) => Some(t),
                    Err(e) => {
                        sharing_error = Some(e);
                        None
                    }
                }
            } else {
                host.get()
            };
            let detached = match &session {
                Some(t) => t.detach(&spec.hash).await.ok().flatten(),
                None => None,
            };
            let mut res = crate::deferred_rename::rename_entry_io(&from, &initial_to);
            for _ in 1..6 {
                match &res {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => break,
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        res = crate::deferred_rename::rename_entry_io(&from, &initial_to);
                    }
                }
            }
            let final_res = match res {
                Ok(()) => {
                    drop(hold);
                    Ok(initial_to)
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    crate::deferred_rename::rename_reserved_or_number(
                        &from, initial_to, hold, is_dir,
                    )
                    .map_err(|e| e.to_string())
                }
                Err(e) => {
                    drop(hold);
                    Err(e.to_string())
                }
            };
            let reattach_at = match &final_res {
                Ok(actual_to) if is_dir => Some(actual_to.as_path()),
                Err(_) if detached.is_some() => Some(spec.folder.as_path()),
                _ => None,
            };
            if keep_sharing
                && let Some(t) = &session
                && let Some(folder) = reattach_at
            {
                match read_kept(&spec.hash).await {
                    Ok(bytes) => {
                        // The engine opens the files again: at the new folder it may only
                        // open what MDM checks there first (the old grant is for the old one).
                        let mut at = spec.clone();
                        at.folder = folder.to_owned();
                        let single_at = match &final_res {
                            Ok(to) => to.clone(),
                            Err(_) => from.clone(),
                        };
                        revoke_grant(&spec.hash);
                        let attached = match prepare_storage(&host, &at, &single_at, Vec::new())
                            .await
                        {
                            Ok(_) => t
                                .attach(&spec.hash, &bytes, folder, spec.files.clone(), detached)
                                .await
                                .map_err(|e| crate::errors::torrent(&e)),
                            Err(e) => Err(e),
                        };
                        if let Err(e) = attached {
                            sharing_error = Some(e);
                        }
                    }
                    Err(why) => sharing_error = Some(why),
                }
            }
            host.busy.lock().unwrap().remove(&spec.hash);
            let _ = tx.send((final_res, sharing_error));
        });
        Ok(())
    }

    /// The finished actions' outcomes since last asked, by info hash.
    pub fn take_outcomes(&self) -> Vec<Outcome> {
        std::mem::take(&mut *self.outcomes.lock().unwrap())
    }

    /// Torrent settings' upload limit for all torrents (None: unlimited), for the session.
    /// Cheap: called each frame, as `set_cap`; a session that starts takes it at once.
    pub fn set_upload(&self, rate: Option<u64>) {
        *self.upload.lock().unwrap() = rate;
        if let Some(t) = self.get() {
            t.set_upload_limit(rate);
        }
    }

    /// The torrents' share of the global cap now (None: off), for the session's download limit:
    /// torrents together stay under it (`App::apply_global_limit` splits the cap with file
    /// downloads). Cheap: called each frame; the session's limit changes only when it
    /// differs (`Torrents::set_download_limit`), and a session that starts takes it at once.
    pub fn set_cap(&self, rate: Option<u64>) {
        *self.cap.lock().unwrap() = rate;
        if let Some(t) = self.get() {
            t.set_download_limit(rate);
        }
    }

    /// The settings for the next session (Torrent settings changed, or at start).
    pub fn set_config(&self, mut cfg: Option<Config>) {
        if let Some(cfg) = &mut cfg {
            cfg.require_authorization = true;
        }
        *self.wanted.lock().unwrap() = cfg;
    }

    /// The session if it already runs.
    pub fn get(&self) -> Option<Arc<Torrents>> {
        self.cell.try_lock().ok().and_then(|s| s.clone())
    }

    /// Whether the running session was started with other settings than those wanted now.
    pub fn settings_changed(&self) -> bool {
        self.get().is_some() && *self.running.lock().unwrap() != *self.wanted.lock().unwrap()
    }

    /// The session, started if needed. `known`: the torrents the list has (the rest of what
    /// the session restores is forgotten).
    pub async fn session(&self, known: HashSet<String>) -> Result<Arc<Torrents>, String> {
        let mut cell = self.cell.lock().await;
        if let Some(t) = cell.as_ref() {
            return Ok(t.clone());
        }
        // i18n: an error: the app has no folder for its torrent data
        state_dir().ok_or(crate::i18n::t("no data folder for torrents"))?;
        // Held to the adapter as it is now, not as it was when the settings were last read.
        let bind = route_bind(&crate::settings::own_route())?;
        let mut cfg = self.wanted.lock().unwrap().clone().ok_or_else(|| {
            // i18n: an error: Torrent settings ask for the proxy, and Options → Proxy / Socks has no SOCKS5 one
            crate::i18n::t(
                "Torrent settings say to use the proxy, but Proxy / Socks has no SOCKS5 proxy",
            )
            .to_owned()
        })?;
        cfg.bind = bind;
        let t = Torrents::start(cfg.clone()).await.map_err(|e| {
            crate::i18n::fmt(
                // i18n: an error, with the reason
                crate::i18n::t("torrents could not start: {error}"),
                &[("error", &e.to_string())],
            )
        })?;
        for hash in t.hashes() {
            if known.contains(&hash) {
                let _ = t.pause(&hash).await;
            } else {
                let _ = t.remove(&hash, false).await;
            }
        }
        t.set_download_limit(*self.cap.lock().unwrap());
        t.set_upload_limit(*self.upload.lock().unwrap());
        let t = Arc::new(t);
        *cell = Some(t.clone());
        *self.running.lock().unwrap() = Some(cfg);
        Ok(t)
    }

    /// Stop the session now, whatever it is doing (the network adapter changed, #35: its sockets
    /// were bound to the one before, or to none). The next one starts as things are then.
    pub fn stop_now(&self, rt: &tokio::runtime::Handle) {
        let Some(t) = self.cell.try_lock().ok().and_then(|mut s| s.take()) else {
            return;
        };
        *self.running.lock().unwrap() = None;
        rt.spawn(async move { t.stop().await });
    }

    /// Stop the session when no torrent needs it (`idle`) and Torrent settings say so, or its
    /// settings changed: it stops listening, and the next one starts with the new ones.
    pub fn stop_if_idle(&self, rt: &tokio::runtime::Handle, idle: bool, stop_idle: bool) {
        if !idle || !(stop_idle || self.settings_changed()) {
            return;
        }
        let Some(t) = self.cell.try_lock().ok().and_then(|mut s| s.take()) else {
            return;
        };
        *self.running.lock().unwrap() = None;
        rt.spawn(async move { t.stop().await });
    }

    /// Keep sharing (true) or stop sharing (false) finished torrents: `jobs` are the list's
    /// complete ones. Sharing starts the session if needed. A torrent the session did not
    /// restore (the engine restores only what MDM has checked, so none at start) is checked and
    /// added as at Start, asking nobody: one that needs a question is left for the person.
    pub fn set_sharing(
        &self,
        rt: &tokio::runtime::Handle,
        known: HashSet<String>,
        jobs: Vec<ShareJob>,
        on: bool,
    ) {
        if jobs.is_empty() || (!on && self.get().is_none()) {
            return;
        }
        let host = self.clone();
        rt.spawn(async move {
            let Ok(t) = host.session(known).await else {
                return;
            };
            // The other downloads' files are looked at once for the whole batch.
            let cache = Arc::new(crate::torrent_ownership::ClaimCache::default());
            for job in jobs {
                let h = job.spec.hash.clone();
                if !on {
                    let _ = t.remove(&h, false).await;
                } else if t.has(&h) {
                    let _ = t.resume(&h).await;
                } else if host.hold(&h) {
                    // Held like any action: a delete in the first seconds waits for it, instead
                    // of finding the files made again under it.
                    let shared = host.share(&t, &job, &cache).await;
                    host.busy.lock().unwrap().remove(&h);
                    if let Err(e) = shared {
                        tracing::info!("torrents: {h} is not shared again: {e}");
                    }
                }
            }
        });
    }

    /// Take the busy guard for `hash`, unless an action or a deletion already has it.
    fn hold(&self, hash: &str) -> bool {
        !self.deleting.lock().unwrap().contains_key(hash)
            && self.busy.lock().unwrap().insert(hash.to_owned())
    }

    async fn share(
        &self,
        t: &Arc<Torrents>,
        job: &ShareJob,
        cache: &Arc<crate::torrent_ownership::ClaimCache>,
    ) -> Result<(), String> {
        let mut protected = self.deletion_claims();
        for (id, paths) in job.claims.iter() {
            if *id != job.id {
                protected.extend(paths.iter().cloned());
            }
        }
        prepare_storage_as(
            self,
            &job.spec,
            &job.dest,
            protected,
            false,
            Some(cache.clone()),
        )
        .await?;
        let bytes = read_kept(&job.spec.hash).await?;
        t.add(
            &bytes,
            &job.spec.folder,
            job.spec.files.clone(),
            Vec::new(),
            false,
        )
        .await
        .map_err(|e| {
            revoke_grant(&job.spec.hash);
            crate::errors::torrent(&e)
        })?;
        Ok(())
    }

    /// Forget a torrent, using the saved metadata if completion or an idle session
    /// already released its engine handle. Deletion never needs to start the network.
    pub fn forget(
        &self,
        rt: &tokio::runtime::Handle,
        spec: &TorrentSpec,
        dest: &Path,
        files: bool,
        protected: Vec<PathBuf>,
        name: String,
    ) {
        crate::torrent_rename::ensure_loaded();
        self.start_delete(
            rt,
            DeleteRequest {
                spec: spec.clone(),
                dest: dest.to_owned(),
                files,
                protected,
                name,
                file: torrent_file(&spec.hash),
                data: state_dir(),
            },
        );
    }

    fn start_delete(&self, rt: &tokio::runtime::Handle, request: DeleteRequest) {
        let DeleteRequest {
            spec,
            dest,
            files,
            protected,
            name,
            file,
            data,
        } = request;
        let claims = spec
            .ownership
            .as_ref()
            .map_or_else(|| vec![dest.to_owned()], |o| o.paths(&spec.folder, &dest));
        self.deleting
            .lock()
            .unwrap()
            .insert(spec.hash.clone(), claims);
        let host = self.clone();
        let rename_dir = data.as_ref().map(|p| p.join("session"));
        rt.spawn(async move {
            let result = async {
                if let Some(owner) = &spec.ownership {
                    host.ownership_ready(owner).await?;
                }
                let session = host.cell.lock().await.clone();
                let data = data.ok_or_else(|| crate::i18n::t("no data folder").to_owned())?;
                forget_torrent(session, file, spec.clone(), dest, files, protected, data).await
            }
            .await;
            match result {
                Ok(kept) => {
                    if let Some(dir) = rename_dir {
                        mdm_torrent::rename_plugin::remove_renamed_file(&dir, &spec.hash);
                    }
                    if !kept.is_empty() {
                        crate::dialogs::tell(
                            crate::i18n::mark("Delete"),
                            crate::i18n::fmt(
                                // i18n: said after Delete with files when some files stayed on the disk; {file} is the download's name; the files' paths follow it
                                crate::i18n::t("{file}: these files stayed on the disk, because MDM didn't create them or another download uses them:"),
                                &[("file", &crate::i18n::isolate(&name))],
                            ) + "\n\n"
                                + &file_list(kept.iter().map(|p| p.as_path())),
                        );
                    }
                }
                Err(error) => crate::dialogs::tell(
                    crate::i18n::mark("Delete"),
                    crate::i18n::fmt(
                        // i18n: error dialog when a torrent's files could not be deleted; {file} is the download's name, {error} why
                        crate::i18n::t("{file} could not be deleted from the disk: {error}"),
                        &[("file", &crate::i18n::isolate(&name)), ("error", &error)],
                    ),
                ),
            }
            if let Some(owner) = &spec.ownership {
                host.initializing.lock().unwrap().remove(&owner.token);
            }
            // Keep both the hash and its paths reserved until every old side effect is done.
            host.deleting.lock().unwrap().remove(&spec.hash);
        });
    }
}

async fn forget_torrent(
    session: Option<Arc<Torrents>>,
    file: Option<PathBuf>,
    spec: TorrentSpec,
    dest: PathBuf,
    files: bool,
    protected: Vec<PathBuf>,
    data: PathBuf,
) -> Result<Vec<PathBuf>, String> {
    if let Some(t) = session
        && t.has(&spec.hash)
    {
        // Release Windows file handles before touching the files ourselves.
        if let Err(e) = t.remove(&spec.hash, false).await
            && t.has(&spec.hash)
        {
            return Err(e.to_string());
        }
    }
    // Nothing of this torrent may be opened by the engine on an old grant from now on.
    revoke_grant(&spec.hash);
    tokio::task::spawn_blocking(move || {
        let mut kept = Vec::new();
        if files {
            let file = file.as_ref().ok_or_else(|| {
                crate::i18n::t("the .torrent file kept for it is gone").to_owned()
            })?;
            let bytes = mdm_torrent::read_file(file).map_err(|e| e.to_string())?;
            let meta =
                mdm_torrent::librqbit::torrent_from_bytes(&bytes).map_err(|e| e.to_string())?;
            if !meta.info_hash.as_string().eq_ignore_ascii_case(&spec.hash) {
                return Err(
                    // i18n: error when deleting a torrent: the .torrent file MDM kept is another torrent's
                    crate::i18n::t("The saved .torrent file does not match this download")
                        .to_owned(),
                );
            }
            let listed = mdm_torrent::files_from_bytes(&bytes).map_err(|e| e.to_string())?;
            let single = listed.iter().filter(|f| !f.padding).count() == 1;
            match spec.ownership.as_ref() {
                Some(owner) => {
                    owner
                        .validate(&bytes, &spec.hash)
                        .map_err(|e| e.to_string())?;
                    kept = owner
                        .delete(
                            &data,
                            &spec.folder,
                            &dest,
                            !spec.create_subfolder,
                            &protected,
                        )
                        .map_err(|e| e.to_string())?;
                }
                // Added before ownership records, one file: it goes as it always did, by
                // name, when it is a plain file inside the folder it was saved to and no
                // other download claims it.
                None if single => {
                    crate::torrent_ownership::delete_single_file(&spec.folder, &dest, &protected)
                        .map_err(|e| e.to_string())?;
                }
                // Added before ownership records, with the outer folder MDM made for it: its
                // files go as they always did, except what another download claims.
                None if spec.create_subfolder && spec.folder == dest => {
                    crate::torrent_ownership::delete_outer_folder(&bytes, &spec.folder, &protected)
                        .map_err(|e| e.to_string())?;
                }
                None => {
                    return Err(crate::i18n::t(
                        // i18n: error when deleting a torrent added by an older MDM that saved no record of its files: they stay on the disk
                        "The files were kept because this older download has no ownership record",
                    )
                    .to_owned());
                }
            }
        }
        // The parts of the pieces its unticked files shared: MDM's own, gone with it.
        if let Err(e) =
            mdm_torrent::authorized_storage::remove_parts(&data.join("session"), &spec.hash)
        {
            tracing::warn!("torrent {}: its parts files stayed: {e}", spec.hash);
        }
        // Keep the metadata and rename mapping if deletion failed, for recovery.
        if let Some(file) = file {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            }
        }
        if let Some(owner) = &spec.ownership {
            owner.forget(&data).map_err(|e| e.to_string())?;
        }
        Ok(kept)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A torrent added before ownership records whose files MDM's own engine wrote: it has its
/// outer folder (the one MDM made for it) or is one file. What is at its paths is taken as
/// agreed to; a torrent saved straight in Save to (new in this version) never qualifies.
fn owns_what_is_there(
    spec: &TorrentSpec,
    owner: &crate::torrent_ownership::Ownership,
    dest: &Path,
) -> bool {
    spec.ownership.is_none()
        && (owner.files.len() == 1 || (spec.create_subfolder && spec.folder == dest))
}

/// Up to eight paths, one to a line, then how many more: for a question or a notice.
fn file_list<'a>(paths: impl ExactSizeIterator<Item = &'a Path>) -> String {
    let total = paths.len();
    let mut text = String::new();
    for path in paths.take(8) {
        text.push_str(&crate::i18n::isolate(&path.display().to_string()));
        text.push('\n');
    }
    if total > 8 {
        text.push_str(&crate::i18n::fmt(
            // i18n: number of additional files in a list of files
            &crate::i18n::tn(
                (total - 8) as u64,
                "And {n} other file.",
                "And {n} other files.",
            ),
            &[("n", &crate::i18n::integer((total - 8) as u64))],
        ));
    }
    text
}

/// A finished torrent to share again: its row, and what every row of the list claims (by row),
/// for the check before it is added.
pub struct ShareJob {
    pub id: u64,
    pub spec: TorrentSpec,
    pub dest: PathBuf,
    pub claims: Arc<Vec<(u64, Vec<PathBuf>)>>,
}

/// Whether one of the files ticked now (`files`) has its bytes in a parts file under `session`
/// (the torrents' session folder), or is kept in one by the running storage: it has no file
/// yet, and ticking it must make it.
fn chosen_with_parts(session: Option<&Path>, hash: &str, files: &[usize]) -> bool {
    session.is_some_and(|s| {
        files
            .iter()
            .any(|i| mdm_torrent::authorized_storage::has_parts(s, hash, *i))
    })
}

/// The engine may not open a torrent's files on the grant it was given for another folder or
/// another time: used when MDM itself moves, renames or deletes them.
pub fn revoke_grant(hash: &str) {
    held().lock().unwrap().remove(hash);
    if let Some(data) = state_dir() {
        mdm_torrent::authorized_storage::forget(&data.join("session"), hash);
    }
}

/// The objects each torrent's files were last handed to the engine as (by file index): a
/// paused torrent keeps writing to those, so a resume can tell that a file was swapped.
type Held = Mutex<HashMap<String, Vec<(usize, crate::torrent_ownership::Identity)>>>;
fn held() -> &'static Held {
    static HELD: OnceLock<Held> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

/// Check what is at the torrent's paths and hand the engine its files. `Ok(true)` when a
/// torrent already in the session still holds a different object than the one now at a path.
async fn prepare_storage(
    host: &Host,
    spec: &TorrentSpec,
    dest: &Path,
    protected: Vec<PathBuf>,
) -> Result<bool, String> {
    prepare_storage_as(host, spec, dest, protected, true, None).await
}

/// [`prepare_storage`]; `ask` false (nobody is there to answer: sharing at start) stops at the
/// first file that needs a question instead of asking it.
async fn prepare_storage_as(
    host: &Host,
    spec: &TorrentSpec,
    dest: &Path,
    protected: Vec<PathBuf>,
    ask: bool,
    cache: Option<Arc<crate::torrent_ownership::ClaimCache>>,
) -> Result<bool, String> {
    if let Some(owner) = &spec.ownership {
        host.ownership_ready(owner).await?;
    }
    let data = state_dir().ok_or_else(|| crate::i18n::t("no data folder").to_owned())?;
    let path =
        torrent_file(&spec.hash).ok_or_else(|| crate::i18n::t("no data folder").to_owned())?;
    let bytes = tokio::task::spawn_blocking(move || mdm_torrent::read_file(&path)).await
        .map_err(|e| e.to_string())?.map_err(|_| crate::i18n::t(
            // i18n: starting an old torrent whose cached metadata was lost; re-adding can restore it without deleting data
            "The saved .torrent file is missing or unreadable. Add the torrent again to recover its metadata; keep its existing files.",
        ).to_owned())?;
    let owner = match &spec.ownership {
        Some(owner) => owner.clone(),
        None => crate::torrent_ownership::Ownership::recover(&bytes, &spec.hash)
            .map_err(|e| e.to_string())?,
    };
    owner
        .validate(&bytes, &spec.hash)
        .map_err(|e| e.to_string())?;
    let (scan_owner, scan_folder, scan_dest) =
        (owner.clone(), spec.folder.clone(), dest.to_owned());
    let collision = tokio::task::spawn_blocking(move || {
        crate::torrent_ownership::conflicts_cached(
            &scan_owner.paths(&scan_folder, &scan_dest),
            &protected,
            true,
            cache.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    if !collision.is_empty() {
        return Err(crate::i18n::t(
            // i18n: starting a torrent whose destination another listed download claims
            "Another download already claims this torrent's destination",
        )
        .to_owned());
    }
    // Added before ownership records, with the folder MDM made for it (or one file): what is
    // there, MDM's own engine wrote, so resuming or sharing it after an update asks nothing.
    if owns_what_is_there(spec, &owner, dest) {
        let (owner3, data3, folder3, dest3) = (
            owner.clone(),
            data.clone(),
            spec.folder.clone(),
            dest.to_owned(),
        );
        tokio::task::spawn_blocking(move || owner3.adopt_existing(&data3, &folder3, &dest3))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    }
    let mut fresh = Vec::new();
    for _ in 0..3 {
        let (owner2, data2, folder2, dest2) = (
            owner.clone(),
            data.clone(),
            spec.folder.clone(),
            dest.to_owned(),
        );
        let wanted = spec.files.clone();
        let result = tokio::task::spawn_blocking(move || {
            owner2.prepare_for(&data2, &folder2, &dest2, fresh, wanted.as_deref())
        })
        .await
        .map_err(|e| e.to_string())?;
        match result {
            Ok(prepared) => {
                let (owner, folder, dest, ledger, wanted) = (
                    owner.clone(),
                    spec.folder.clone(),
                    dest.to_owned(),
                    data.clone(),
                    spec.files.clone(),
                );
                let now = crate::torrent_ownership::identities(&prepared.files);
                let swapped = held()
                    .lock()
                    .unwrap()
                    .insert(spec.hash.clone(), now.clone())
                    .is_some_and(|was| crate::torrent_ownership::swapped(&was, &now));
                let hash = spec.hash.clone();
                mdm_torrent::authorized_storage::authorize_parted(
                    &data.join("session"),
                    &spec.hash,
                    &spec.folder,
                    prepared,
                    Arc::new(move || {
                        // A later init (re-added, restarted after an error) takes new handles:
                        // they are what the engine holds from then on. Files ticked since are
                        // prepared again before (`Act::Choose`), with a grant of their own.
                        let prepared = owner.prepare_for(
                            &ledger,
                            &folder,
                            &dest,
                            Vec::new(),
                            wanted.as_deref(),
                        )?;
                        held().lock().unwrap().insert(
                            hash.clone(),
                            crate::torrent_ownership::identities(&prepared.files),
                        );
                        Ok(prepared)
                    }),
                );
                return Ok(swapped);
            }
            Err(error) => {
                let files =
                    crate::torrent_ownership::take_consent(error).map_err(|e| e.to_string())?;
                if !ask {
                    return Err(crate::i18n::t(
                        // i18n: the user refused write permission for existing files when starting a torrent
                        "The torrent was not started because permission to use existing files was declined.",
                    ).to_owned());
                }
                let mut text = crate::i18n::t(
                    // i18n: question before starting/resuming a torrent that would modify files MDM did not create; Yes authorizes writes, No leaves files unchanged
                    "MDM didn't create these files. Starting the torrent can resize them and replace their contents, including unticked files that share pieces. Use them for this torrent?",
                ).to_owned();
                text.push_str("\n\n");
                text.push_str(&file_list(files.iter().map(|f| f.path.as_path())));
                if !crate::dialogs::confirm(crate::i18n::mark("Use existing torrent files"), text)
                    .await
                {
                    return Err(crate::i18n::t(
                        // i18n: the user refused write permission for existing files when starting a torrent
                        "The torrent was not started because permission to use existing files was declined.",
                    ).to_owned());
                }
                fresh = files;
            }
        }
    }
    Err(crate::i18n::t(
        // i18n: files kept appearing while the user was confirming a torrent start; stop rather than repeat questions indefinitely
        "The destination keeps changing. Stop changes to its files and try starting the torrent again.",
    ).to_owned())
}

enum RunState {
    Starting,
    Ready(Arc<Torrents>),
    Failed(String),
}

/// Off-screen test runs only (MDM_OFFSCREEN): peers every torrent added tries first, from
/// MDM_LAB_PEERS (`127.0.0.1:6881,…`), the local test swarm (`crates/torrent/examples/swarm.rs`):
/// with no DHT, no local discovery and no incoming connections, a test copy finds it so.
fn lab_peers() -> Vec<std::net::SocketAddr> {
    if !crate::offscreen() {
        return Vec::new();
    }
    std::env::var("MDM_LAB_PEERS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|a| a.trim().parse().ok())
        .collect()
}

/// A running torrent download.
pub struct Run {
    state: Arc<Mutex<RunState>>,
    rt: tokio::runtime::Handle,
    hash: String,
    url: url::Url,
    stopped: Arc<Mutex<bool>>,
    /// Keep sharing once finished (D-018; off by default).
    keep_sharing: bool,
    /// Where its files are (`TorrentSpec::folder`).
    folder: PathBuf,
    /// Its files' Mark-of-the-Web (#40): 0 not yet, 1 once librqbit created them, 2 finished.
    marked: Arc<std::sync::atomic::AtomicU8>,
    /// Its own limits, bytes/s, (download, upload): the download one from the Speed Limiter's
    /// "for one file" or the progress window, the upload one from the progress window
    /// (`TorrentSpec::upload_limit`). Set on the torrent once it is in the session, and again
    /// on each change.
    limit: Arc<Mutex<(Option<u64>, Option<u64>)>>,
    /// The order its pieces are downloaded in (`TorrentSpec::order`): set once it is in the
    /// session, and again on each change.
    order: Arc<Mutex<mdm_torrent::PieceOrder>>,
    /// The torrents an action is busy with (`Host::act`): while this one is, it shows as
    /// checking, with the bytes last seen (`last`: done, total), not as gone or stopped.
    busy: Arc<Mutex<HashSet<String>>>,
    last: Mutex<(u64, Option<u64>)>,
}

/// The origin a torrent's files are marked with: not one website, and never the magnet link
/// (its trackers), as Windows' guidance for an unknown source says (and qBittorrent does).
const TORRENT_ORIGIN: &str = "about:internet";

/// Mark `files` (padding left out) under `folder` as downloaded from the internet, as web
/// downloads are (Windows' zone, so SmartScreen and Office's Protected View check them; macOS'
/// quarantine; Linux's origin attribute, which nothing enforces). Every file of the torrent,
/// ticked or not: librqbit creates them all.
fn mark_files(folder: &Path, files: &[mdm_torrent::FileEntry]) {
    for f in files.iter().filter(|f| !f.padding) {
        let path = folder.join(&f.path);
        if !path.is_file() {
            continue;
        }
        crate::webmark::mark(&path, TORRENT_ORIGIN, None);
    }
}

/// What the progress window's torrent tabs show.
pub struct Details {
    /// Each file (padding left out): its index in the torrent's list, its entry, chosen?,
    /// bytes there.
    pub files: Vec<(usize, mdm_torrent::FileEntry, bool, u64)>,
    pub piece_length: u32,
    pub peers: Vec<mdm_torrent::Peer>,
    pub trackers: Vec<String>,
    pub pieces: Vec<bool>,
    pub stats: Option<mdm_torrent::Stats>,
}

impl Run {
    /// Start (or resume) `spec`'s torrent, at most `limit` bytes/s down for this torrent (and
    /// its own upload limit, `spec.upload_limit`).
    pub fn start(
        host: &Host,
        rt: &tokio::runtime::Handle,
        item: &crate::model::Item,
        known: HashSet<String>,
        limit: Option<u64>,
        keep_sharing: bool,
        protected: Vec<PathBuf>,
    ) -> Run {
        let spec = item.torrent.as_ref().expect("torrent run has a spec");
        let (dest, url) = (item.dest.clone(), item.url.clone());
        let state = Arc::new(Mutex::new(RunState::Starting));
        let own = Arc::new(Mutex::new((limit, spec.upload_limit)));
        let order = Arc::new(Mutex::new(spec.order()));
        let busy = host.busy.clone();
        let (out, host, spec2, own2) = (state.clone(), host.clone(), spec.clone(), own.clone());
        let order2 = order.clone();
        rt.spawn(async move {
            let res = async {
                let swapped = prepare_storage(&host, &spec2, &dest, protected).await?;
                let t = host.session(known).await?;
                if swapped && t.has(&spec2.hash) {
                    // A file was replaced while it was stopped: the engine still writes to
                    // the old one. Take it out of the session, and add it again below on the
                    // handles just checked.
                    t.remove(&spec2.hash, false)
                        .await
                        .map_err(|e| crate::errors::torrent(&e))?;
                }
                if t.has(&spec2.hash) {
                    // Its order before it asks any peer for a piece.
                    let _ = t.set_order(&spec2.hash, *order2.lock().unwrap());
                    t.resume(&spec2.hash)
                        .await
                        .map_err(|e| crate::errors::torrent(&e))?;
                } else {
                    let bytes = read_kept(&spec2.hash).await?;
                    t.add(
                        &bytes,
                        &spec2.folder,
                        spec2.files.clone(),
                        lab_peers(),
                        false,
                    )
                    .await
                    .map_err(|e| {
                        // Not added: nothing will take the handles it was given.
                        revoke_grant(&spec2.hash);
                        crate::errors::torrent(&e)
                    })?;
                }
                Ok::<_, String>(t)
            }
            .await;
            match res {
                Ok(t) => {
                    // Under the limit's lock, so a change made meanwhile (set_speed_limit) is
                    // either seen here or applied there once it is Ready. librqbit keeps a
                    // torrent's limits only while the session runs: set them each start.
                    let own = own2.lock().unwrap();
                    let order = order2.lock().unwrap();
                    *out.lock().unwrap() = RunState::Ready(t.clone());
                    let _ = t.set_torrent_limits(&spec2.hash, own.0, own.1);
                    // Added just now: still checking its files, no piece asked for yet.
                    let _ = t.set_order(&spec2.hash, *order);
                }
                Err(e) => *out.lock().unwrap() = RunState::Failed(e),
            }
        });
        Run {
            state,
            rt: rt.clone(),
            hash: spec.hash.clone(),
            url,
            stopped: Arc::new(Mutex::new(false)),
            keep_sharing,
            folder: spec.folder.clone(),
            marked: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            limit: own,
            order,
            busy,
            last: Mutex::new((0, None)),
        }
    }

    /// Mark its files once librqbit has created them (it leaves Checking) and again once it
    /// finishes, off the window's thread (reading which files it has waits on librqbit too).
    fn mark_when(&self, t: &Arc<Torrents>, phase: Phase) {
        use std::sync::atomic::Ordering;
        // Paused: perhaps never started, its files not created yet.
        let step = match phase {
            Phase::Downloading => 1,
            Phase::Finished => 2,
            Phase::Checking | Phase::Paused | Phase::Error => return,
        };
        if self.marked.fetch_max(step, Ordering::SeqCst) >= step {
            return;
        }
        let (t, hash, folder) = (t.clone(), self.hash.clone(), self.folder.clone());
        self.rt.spawn_blocking(move || {
            if let Some((files, _, _)) = t.files(&hash) {
                mark_files(&folder, &files);
            }
        });
    }

    fn session(&self) -> Option<Arc<Torrents>> {
        match &*self.state.lock().unwrap() {
            RunState::Ready(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// What the list shows. On the window's thread: the session's numbers as last read
    /// (`Torrents::seen`), never waiting on librqbit (#693).
    pub fn snapshot(&self) -> Snapshot {
        let mut s = Snapshot {
            state: State::Probing,
            url: self.url.clone(),
            file: None,
            size: None,
            downloaded: 0,
            resumable: Some(true),
            segments: Vec::new(),
            connections: Vec::new(),
            error: None,
            failure: None,
            events: Vec::new(),
            // A torrent has peers, not one server (#140).
            server: None,
            at_service: None,
            finishing: None,
        };
        if *self.stopped.lock().unwrap() {
            s.state = State::Cancelled;
            if let Some(st) = self.session().and_then(|t| t.seen(&self.hash)) {
                s.downloaded = st.done;
                s.size = Some(st.total);
            }
            return s;
        }
        if self.busy.lock().unwrap().contains(&self.hash) {
            s.state = State::Verifying;
            (s.downloaded, s.size) = *self.last.lock().unwrap();
            return s;
        }
        let t = match &*self.state.lock().unwrap() {
            RunState::Starting => return s,
            RunState::Failed(e) => {
                s.state = State::Failed;
                s.error = Some(e.clone());
                return s;
            }
            RunState::Ready(t) => t.clone(),
        };
        let Some(st) = t.seen(&self.hash) else {
            let (done, size) = *self.last.lock().unwrap();
            if size.is_some_and(|total| total > 0 && done >= total) {
                s.state = State::Done;
                s.downloaded = done;
                s.size = size;
                return s;
            }
            s.state = State::Failed;
            // i18n: an error: the torrent is gone from the torrent session
            s.error = Some(crate::i18n::t("the torrent is no longer there").into());
            return s;
        };
        s.downloaded = st.done;
        s.size = Some(st.total);
        *self.last.lock().unwrap() = (st.done, s.size);
        s.state = match st.phase {
            Phase::Checking => State::Verifying,
            Phase::Downloading => State::Downloading,
            Phase::Finished => State::Done,
            Phase::Paused => State::Cancelled,
            Phase::Error => State::Failed,
        };
        s.error = st.error;
        self.mark_when(&t, st.phase);
        // Done: it stops sharing (a download manager finishes downloads; D-018), unless the
        // person chose to keep sharing.
        if s.state == State::Done && !self.keep_sharing {
            self.remove_in_background(&t);
        }
        s
    }

    fn pause_in_background(&self, t: &Arc<Torrents>) {
        let (t, hash) = (t.clone(), self.hash.clone());
        self.rt.spawn(async move {
            let _ = t.pause(&hash).await;
        });
    }

    fn remove_in_background(&self, t: &Arc<Torrents>) {
        let (t, hash) = (t.clone(), self.hash.clone());
        self.rt.spawn(async move {
            let _ = t.remove(&hash, false).await;
        });
    }

    /// Stop: a completed run leaves the session; an incomplete one pauses to resume later.
    pub fn cancel(&self) {
        *self.stopped.lock().unwrap() = true;
        if let Some(t) = self.session() {
            if t.seen(&self.hash)
                .is_some_and(|s| s.phase == Phase::Finished)
            {
                self.remove_in_background(&t);
            } else {
                self.pause_in_background(&t);
            }
        }
    }

    /// This torrent's own download limit (#39: not every torrent's): now if it is in the
    /// session, else once it is.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        let mut own = self.limit.lock().unwrap();
        own.0 = rate;
        if let Some(t) = self.session() {
            let _ = t.set_torrent_limits(&self.hash, own.0, own.1);
        }
    }

    pub fn speed_limit(&self) -> Option<u64> {
        self.limit.lock().unwrap().0
    }

    /// The order its pieces are downloaded in (#39): now if it is in the session, else once it
    /// is.
    pub fn set_order(&self, order: mdm_torrent::PieceOrder) {
        let mut own = self.order.lock().unwrap();
        *own = order;
        if let Some(t) = self.session() {
            let _ = t.set_order(&self.hash, order);
        }
    }

    /// This torrent's own upload limit: now if it is in the session, else once it is.
    pub fn set_upload_limit(&self, rate: Option<u64>) {
        let mut own = self.limit.lock().unwrap();
        own.1 = rate;
        if let Some(t) = self.session() {
            let _ = t.set_torrent_limits(&self.hash, own.0, own.1);
        }
    }

    /// Bytes received from peers since it last started (for the global cap, which HTTP
    /// downloads share with torrents); None until it is in the session.
    pub fn fetched(&self) -> Option<u64> {
        let t = self.session()?;
        t.seen(&self.hash).map(|s| s.fetched)
    }

    pub fn details(&self) -> Details {
        self.session()
            .and_then(|t| details_of(&t, &self.hash))
            .unwrap_or(Details {
                files: Vec::new(),
                piece_length: 0,
                peers: Vec::new(),
                trackers: Vec::new(),
                pieces: Vec::new(),
                stats: None,
            })
    }
}

/// What the progress window's torrent tabs show for `hash`, from the session: a `Run`'s
/// torrent, or a finished one still shared without a `Run` (None: not in the session). As last
/// read: the window's thread never waits on librqbit (#693).
pub fn details_of(t: &Torrents, hash: &str) -> Option<Details> {
    let stats = t.seen(hash)?;
    let done = stats.file_done.clone();
    let (entries, chosen, piece_length) = t.seen_files(hash).unwrap_or_default();
    let swarm = t.seen_swarm(hash);
    Some(Details {
        files: entries
            .into_iter()
            .enumerate()
            .filter(|(_, f)| !f.padding)
            .map(|(i, f)| {
                let on = chosen.as_ref().is_none_or(|c| c.contains(&i));
                (i, f, on, done.get(i).copied().unwrap_or(0))
            })
            .collect(),
        piece_length,
        peers: swarm.peers,
        trackers: swarm.trackers,
        pieces: swarm.pieces,
        stats: Some(stats),
    })
}

/// A magnet link for a torrent (what the list shows as its address).
pub fn magnet_url(hash: &str, name: &str) -> url::Url {
    let mut u = url::Url::parse(&format!("magnet:?xt=urn:btih:{hash}")).expect("valid magnet");
    u.query_pairs_mut().append_pair("dn", name);
    u
}

/// The query key that marks a row saved without an outer folder. An older MDM keeps a row's
/// address as it is but drops the fields it does not know (`create_subfolder`, `ownership`),
/// so this is what lets this one see, when it reads the row back, that its folder is the
/// shared Save to folder and not a folder of the torrent's own.
const DIRECT_MARK: &str = "x.mdm";

/// A magnet link for a torrent saved straight in Save to (see [`DIRECT_MARK`]).
pub fn direct_magnet_url(hash: &str, name: &str) -> url::Url {
    let mut u = magnet_url(hash, name);
    mark_direct(&mut u);
    u
}

pub fn mark_direct(url: &mut url::Url) {
    if url.scheme() == "magnet" && !url_marks_direct(url) {
        url.query_pairs_mut().append_pair(DIRECT_MARK, "direct");
    }
}

/// Whether a row's address says it was saved straight in Save to.
pub fn url_marks_direct(url: &url::Url) -> bool {
    url.scheme() == "magnet"
        && url
            .query_pairs()
            .any(|(k, v)| k == DIRECT_MARK && v == "direct")
}

/// The file the list row stands for: the torrent's folder, or its one file.
pub fn dest_for(
    meta: &mdm_torrent::Meta,
    dir: &Path,
    files: Option<&[usize]>,
    create_subfolder: bool,
) -> PathBuf {
    let folder = meta.folder_in_with_subfolder(dir, create_subfolder);
    let chosen: Vec<&mdm_torrent::FileEntry> = meta
        .files
        .iter()
        .enumerate()
        .filter(|(i, f)| !f.padding && files.is_none_or(|c| c.contains(i)))
        .map(|(_, f)| f)
        .collect();
    match chosen.as_slice() {
        [one] if meta.files.iter().filter(|f| !f.padding).count() == 1 => folder.join(&one.path),
        _ => folder,
    }
}

/// A torrent through a debrid account (#44): the files ticked in Add torrent, `files` (indexes
/// into `meta.files`), as the service is given them: their paths in the torrent, `/`-separated,
/// padding files left out (None: all of them); and their total size.
pub fn remote_choice(
    meta: &mdm_torrent::Meta,
    files: Option<&[usize]>,
) -> (Option<Vec<String>>, u64) {
    let ticked: Vec<&mdm_torrent::FileEntry> = meta
        .files
        .iter()
        .enumerate()
        .filter(|(i, f)| !f.padding && files.is_none_or(|c| c.contains(i)))
        .map(|(_, f)| f)
        .collect();
    let size = ticked.iter().map(|f| f.size).sum();
    if ticked.len() == meta.files.iter().filter(|f| !f.padding).count() {
        return (None, size);
    }
    let paths = ticked
        .iter()
        .map(|f| {
            f.path
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    (Some(paths), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debrid_torrent_is_given_the_ticked_files_paths_and_size() {
        let file = |p: &str, size, padding| mdm_torrent::FileEntry {
            path: p.split('/').collect(),
            size,
            padding,
        };
        let meta = mdm_torrent::Meta {
            name: "Lab Show".into(),
            info_hash: "h".into(),
            files: vec![
                file("video.mkv", 2 << 20, false),
                file(".pad/100", 100, true),
                file("extras/notes.txt", 64 << 10, false),
            ],
            piece_length: 1 << 18,
            torrent: Vec::new(),
            peers: Vec::new(),
        };
        // All ticked (or the dialog's None): all, and no padding in the size.
        assert_eq!(remote_choice(&meta, None), (None, (2 << 20) + (64 << 10)));
        assert_eq!(
            remote_choice(&meta, Some(&[0, 2])),
            (None, (2 << 20) + (64 << 10))
        );
        assert_eq!(
            remote_choice(&meta, Some(&[2])),
            (Some(vec!["extras/notes.txt".to_owned()]), 64 << 10)
        );
        // A padding file's index never counts.
        assert_eq!(
            remote_choice(&meta, Some(&[0, 1])),
            (Some(vec!["video.mkv".to_owned()]), 2 << 20)
        );
    }

    fn a_spec() -> TorrentSpec {
        TorrentSpec {
            ownership: None,
            hash: "h".into(),
            files: None,
            folder: "f".into(),
            create_subfolder: true,
            uploaded: 0,
            seeded_secs: 0,
            peers: 0,
            up_speed: 0.0,
            upload_limit: None,
            in_order: false,
            ends_first: true,
        }
    }

    #[test]
    fn direct_only_an_older_torrent_with_its_own_folder_or_one_file_takes_what_is_there() {
        use crate::torrent_ownership::{Claim, Ownership};
        let owner = |n: usize| Ownership {
            token: "0".repeat(64),
            files: (0..n)
                .map(|index| Claim {
                    index,
                    path: format!("f{index}").into(),
                })
                .collect(),
        };
        let dest = PathBuf::from("f");
        // Added before records, with the outer folder MDM made: its own engine wrote the files.
        assert!(owns_what_is_there(&a_spec(), &owner(3), &dest));
        // Added before records, one file.
        let mut other = a_spec();
        other.folder = "elsewhere".into();
        assert!(owns_what_is_there(&other, &owner(1), &dest));
        // Saved straight in Save to: never a torrent of an older MDM.
        let mut direct = a_spec();
        direct.create_subfolder = false;
        assert!(!owns_what_is_there(&direct, &owner(3), &dest));
        // A folder that is not the torrent's destination is not its own.
        assert!(!owns_what_is_there(&other, &owner(3), &dest));
        // A torrent with a record is asked about what is not its own, as always.
        let mut recorded = a_spec();
        recorded.ownership = Some(owner(3));
        assert!(!owns_what_is_there(&recorded, &owner(3), &dest));
    }

    #[tokio::test]
    async fn direct_delete_reserves_hash_and_paths_until_old_metadata_cleanup_finishes() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Host::new();
        let mut spec = a_spec();
        spec.folder = tmp.path().join("output");
        spec.ownership = Some(crate::torrent_ownership::Ownership {
            token: mdm_ipc::random_token(),
            files: vec![crate::torrent_ownership::Claim {
                index: 0,
                path: "README.txt".into(),
            }],
        });
        let (ready, receiver) = tokio::sync::watch::channel(None);
        host.initializing
            .lock()
            .unwrap()
            .insert(spec.ownership.as_ref().unwrap().token.clone(), receiver);
        let file = tmp.path().join("same-hash.torrent");
        std::fs::write(&file, b"old metadata").unwrap();
        let dest = spec.folder.join("README.txt");
        host.start_delete(
            &tokio::runtime::Handle::current(),
            DeleteRequest {
                spec: spec.clone(),
                dest: dest.clone(),
                files: false,
                protected: Vec::new(),
                name: "old row".into(),
                file: Some(file.clone()),
                data: Some(tmp.path().to_owned()),
            },
        );
        assert!(host.busy(&spec.hash), "a quick re-add must be rejected");
        assert_eq!(host.deletion_claims(), vec![dest]);
        tokio::task::yield_now().await;
        assert!(file.exists(), "old delete is held before touching metadata");
        ready.send_replace(Some(Ok(())));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while host.busy(&spec.hash) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!file.exists());
        assert!(host.deletion_claims().is_empty());
        std::fs::write(&file, b"new add's metadata").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(std::fs::read(file).unwrap(), b"new add's metadata");
    }

    #[tokio::test]
    async fn direct_delete_failure_is_shown_and_keeps_metadata_for_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("keep.torrent");
        std::fs::write(&file, b"keep metadata").unwrap();
        let host = Host::new();
        let spec = a_spec();
        let name = format!("delete-error-{}", mdm_ipc::random_token());
        host.start_delete(
            &tokio::runtime::Handle::current(),
            DeleteRequest {
                spec: spec.clone(),
                dest: tmp.path().join("unrelated"),
                files: true,
                protected: Vec::new(),
                name: name.clone(),
                file: Some(file.clone()),
                data: None,
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while host.busy(&spec.hash) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            crate::dialogs::told()
                .iter()
                .any(|(title, text)| *title == "Delete" && text.contains(&name))
        );
        assert_eq!(std::fs::read(file).unwrap(), b"keep metadata");
    }

    #[test]
    fn direct_rows_say_so_in_their_address_where_an_older_build_keeps_it() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let plain = magnet_url(hash, "Set");
        let direct = direct_magnet_url(hash, "Set");
        assert!(!url_marks_direct(&plain));
        assert!(url_marks_direct(&direct));
        // The mark is a magnet's: another address is left as it is, not marked at every load.
        let mut web = url::Url::parse("https://example.org/a.torrent").unwrap();
        mark_direct(&mut web);
        assert_eq!(web.as_str(), "https://example.org/a.torrent");
        // What an older build saves of a row: the address, but not the fields it does not know.
        let older: TorrentSpec = serde_json::from_str(&format!(
            r#"{{"hash":"{hash}","files":null,"folder":"Downloads"}}"#
        ))
        .unwrap();
        assert!(
            older.create_subfolder,
            "the default an older row is read with"
        );
        assert_eq!(
            direct.query_pairs().find(|(k, _)| k == "xt").unwrap().1,
            "urn:btih:".to_owned() + hash,
            "the link is still the torrent's"
        );
    }

    #[test]
    fn direct_layout_survives_restart_and_old_downloads_keep_their_folder() {
        let old: TorrentSpec =
            serde_json::from_str(r#"{"hash":"h","files":null,"folder":"Downloads/Set"}"#).unwrap();
        assert!(old.create_subfolder);
        let mut direct = old;
        direct.create_subfolder = false;
        direct.folder = "Downloads".into();
        let saved = serde_json::to_string(&direct).unwrap();
        let restored: TorrentSpec = serde_json::from_str(&saved).unwrap();
        assert!(!restored.create_subfolder);
        assert_eq!(restored.folder, Path::new("Downloads"));
        assert!(restored.shares_folder(Path::new("Downloads")));
        assert!(
            !restored.shares_folder(Path::new("Downloads/one.bin")),
            "a single file does not represent the shared directory"
        );
    }

    #[tokio::test]
    async fn direct_deletion_survives_detached_and_stopped_sessions() {
        use mdm_torrent::librqbit;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.bin"), b"abc").unwrap();
        std::fs::write(source.join("nested/b.bin"), b"defg").unwrap();
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
        let made = librqbit::create_torrent(
            &source,
            librqbit::CreateTorrentOptions {
                name: Some("Set"),
                trackers: Vec::new(),
                piece_length: Some(16384),
            },
            &spawner,
        )
        .await
        .unwrap();
        let bytes = made.as_bytes().unwrap().to_vec();
        let hash = librqbit::torrent_from_bytes(&bytes)
            .unwrap()
            .info_hash
            .as_string();
        let session = Arc::new(
            Torrents::start(Config {
                offline: true,
                dht: false,
                require_authorization: false,
                ..Config::new(tmp.path().join("session"))
            })
            .await
            .unwrap(),
        );
        for lifecycle in 0..3 {
            for sentinel in [false, true] {
                let output = tmp.path().join(format!("case-{lifecycle}-{sentinel}"));
                std::fs::create_dir_all(&output).unwrap();
                let data = tmp.path().join("ownership-state");
                let owner = crate::torrent_ownership::Ownership {
                    token: mdm_ipc::random_token(),
                    files: mdm_torrent::files_from_bytes(&bytes)
                        .unwrap()
                        .into_iter()
                        .enumerate()
                        .map(|(index, f)| crate::torrent_ownership::Claim {
                            index,
                            path: f.path,
                        })
                        .collect(),
                };
                owner.initialize(&data, &output, &output, &[]).unwrap();
                owner.prepare(&data, &output, &output).unwrap();
                std::fs::write(output.join("a.bin"), b"abc").unwrap();
                std::fs::write(output.join("nested/b.bin"), b"defg").unwrap();
                if sentinel {
                    std::fs::write(output.join("keep.txt"), b"unrelated").unwrap();
                }
                let file = tmp.path().join("saved.torrent");
                std::fs::write(&file, &bytes).unwrap();
                let mut spec = a_spec();
                spec.hash = hash.clone();
                spec.folder = output.clone();
                spec.create_subfolder = false;
                spec.ownership = Some(owner);
                if lifecycle < 2 {
                    session
                        .add(&bytes, &output, None, Vec::new(), true)
                        .await
                        .unwrap();
                    if lifecycle == 1 {
                        session.remove(&hash, false).await.unwrap();
                    }
                }
                let live = (lifecycle < 2).then(|| session.clone());
                forget_torrent(
                    live,
                    Some(file.clone()),
                    spec,
                    output.clone(),
                    true,
                    Vec::new(),
                    data,
                )
                .await
                .unwrap();
                assert!(!file.exists());
                assert!(!output.join("a.bin").exists());
                assert!(!output.join("nested").exists());
                assert!(
                    output.is_dir(),
                    "the shared destination survives even empty"
                );
                if sentinel {
                    assert_eq!(
                        std::fs::read(output.join("keep.txt")).unwrap(),
                        b"unrelated"
                    );
                }
            }
        }
        session.stop().await;
    }

    /// A file ticked on a torrent out of the session (finished, its session stopped while
    /// idle, or after MDM starts again) whose bytes are in a parts file needs the session: its
    /// parts file on the disk says so.
    #[test]
    fn a_ticked_file_with_bytes_in_parts_needs_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session = tmp.path().join("session");
        let hash = "ab".repeat(20);
        let parts = session.join(format!("{hash}.parts"));
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("3"), b"shared piece").unwrap();
        assert!(chosen_with_parts(Some(&session), &hash, &[0, 3]));
        assert!(!chosen_with_parts(Some(&session), &hash, &[0, 1]));
        assert!(!chosen_with_parts(Some(&session), &"cd".repeat(20), &[3]));
        assert!(!chosen_with_parts(None, &hash, &[3]));
    }

    /// #357: the ratio the list and the General tab share, and the seeding limit's.
    #[test]
    fn a_torrents_share_ratio() {
        assert_eq!(share_ratio(0, 0), 0.0);
        assert_eq!(share_ratio(500, 0), 0.0, "nothing taken: no ratio");
        assert_eq!(share_ratio(500, 1000), 0.5);
        assert_eq!(
            share_ratio_text(0, 0),
            "0.00",
            "as the General tab writes it"
        );
        assert_eq!(share_ratio_text(500, 1000), "0.50");
        assert_eq!(share_ratio_text(2000, 1000), "2.00");
        assert_eq!(share_ratio_text(1, 3), "0.33");
    }

    /// #357: a torrent's live numbers aren't kept across a restart: they aren't saved and an old
    /// value in the file is ignored, so a stopped torrent can't show a stale count or rate.
    #[test]
    fn a_torrents_live_numbers_are_not_saved() {
        let mut t = a_spec();
        t.peers = 7;
        t.up_speed = 4096.0;
        let saved = serde_json::to_value(&t).unwrap();
        assert!(saved.get("peers").is_none(), "the count isn't written");
        assert!(saved.get("up_speed").is_none(), "the rate isn't written");
        let mut with_old = saved.clone();
        with_old["peers"] = 9.into();
        with_old["seeders"] = 9.into();
        with_old["up_speed"] = 999.0.into();
        let loaded: TorrentSpec = serde_json::from_value(with_old).unwrap();
        assert_eq!(
            loaded.peers, 0,
            "an old value in the file doesn't come back"
        );
        assert_eq!(loaded.up_speed, 0.0, "nor does an old rate");
    }

    /// #357: with no session running, the columns say nobody is connected and nothing is moving.
    #[test]
    fn no_session_means_no_live_numbers() {
        let mut specs = [a_spec(), a_spec()];
        specs[0].peers = 3;
        specs[0].up_speed = 4096.0;
        specs[1].peers = 12;
        specs[1].up_speed = 1024.0;
        clear_live(specs.iter_mut());
        assert_eq!([specs[0].peers, specs[1].peers], [0, 0]);
        assert_eq!([specs[0].up_speed, specs[1].up_speed], [0.0, 0.0]);
    }

    /// #39: what a torrent sent adds up across its runs: librqbit's count is 0 while paused
    /// and starts again at 0 on each resume.
    #[test]
    fn sent_bytes_add_up_across_resumes() {
        let mut a = Accountant::default();
        let mut total = 0;
        for live in [100, 250, 250, 0, 0, 40, 90] {
            total += a.sent("h", live);
        }
        assert_eq!(total, 250 + 90, "first run 250, then 90 after the pause");
        // Another torrent counts on its own; a forgotten one starts afresh.
        assert_eq!(a.sent("other", 30), 30);
        a.keep(&HashSet::from(["other".to_owned()]));
        assert_eq!(a.sent("h", 90), 90);
    }

    /// #39: a torrent kept from before downloads the rarest pieces first; asked, in order, with
    /// each file's first and last pieces first unless that is turned off.
    #[test]
    fn the_piece_order_kept_and_chosen() {
        let old: TorrentSpec =
            serde_json::from_str(r#"{"hash":"h","files":null,"folder":"f"}"#).unwrap();
        assert_eq!((old.in_order, old.ends_first), (false, true));
        assert_eq!(old.order(), mdm_torrent::PieceOrder::RarestFirst);
        let mut spec = old.clone();
        spec.in_order = true;
        assert_eq!(
            spec.order(),
            mdm_torrent::PieceOrder::InOrder {
                first_and_last_first: true
            }
        );
        spec.ends_first = false;
        assert_eq!(
            spec.order(),
            mdm_torrent::PieceOrder::InOrder {
                first_and_last_first: false
            }
        );
        let back: TorrentSpec =
            serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    /// #39: a finished torrent stops being shared at the share ratio or after the time set,
    /// whichever comes first; with both off, never.
    #[test]
    fn seeding_limits_are_reached() {
        let spec = |uploaded, seeded_secs| TorrentSpec {
            ownership: None,
            hash: "h".into(),
            files: None,
            folder: "f".into(),
            create_subfolder: true,
            uploaded,
            seeded_secs,
            peers: 0,
            up_speed: 0.0,
            upload_limit: None,
            in_order: false,
            ends_first: true,
        };
        let off = SharingLimits::default();
        let ratio2 = SharingLimits {
            ratio: Some(2.0),
            secs: None,
        };
        let hour = SharingLimits {
            ratio: None,
            secs: Some(3600),
        };
        let both = SharingLimits {
            ratio: Some(2.0),
            secs: Some(3600),
        };
        assert_eq!(sharing_limit_reached(&spec(10_000, 99_999), 100, off), None);
        assert_eq!(sharing_limit_reached(&spec(199, 0), 100, ratio2), None);
        assert_eq!(
            sharing_limit_reached(&spec(200, 0), 100, ratio2),
            Some(Reached::Ratio(2.0))
        );
        assert_eq!(sharing_limit_reached(&spec(0, 3599), 100, hour), None);
        assert_eq!(
            sharing_limit_reached(&spec(0, 3600), 100, hour),
            Some(Reached::Time(3600))
        );
        assert_eq!(
            sharing_limit_reached(&spec(250, 4000), 100, both),
            Some(Reached::Ratio(2.5))
        );
        assert_eq!(
            sharing_limit_reached(&spec(500, 0), 0, ratio2),
            None,
            "nothing there yet"
        );
    }

    /// #39: a .torrent's web address, by its path, the name the browser gave or its type.
    #[test]
    fn torrent_addresses_are_recognised() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert!(is_torrent_url(
            &u("https://x.example/a/Set.TORRENT?k=1"),
            None,
            None
        ));
        assert!(is_torrent_url(
            &u("http://x.example/dl?id=7"),
            Some("set.torrent"),
            None
        ));
        assert!(is_torrent_url(
            &u("http://x.example/dl?id=7"),
            None,
            Some("application/x-bittorrent; charset=binary")
        ));
        assert!(!is_torrent_url(
            &u("http://x.example/set.zip"),
            Some("set.zip"),
            Some("application/zip")
        ));
        assert!(!is_torrent_url(
            &u("ftp://x.example/set.torrent"),
            None,
            None
        ));
        assert!(!is_torrent_url(
            &u("http://x.example/set.torrent.html"),
            None,
            None
        ));
    }

    #[test]
    fn magnet_links_carry_the_name() {
        let u = magnet_url("0123456789abcdef0123456789abcdef01234567", "A & B");
        assert_eq!(u.scheme(), "magnet");
        assert!(u.as_str().contains("dn=A+%26+B"), "{u}");
    }

    /// #40: a torrent's files get the internet zone, with no address but `about:internet`
    /// (never the magnet link); padding files and files not there are left alone.
    #[cfg(windows)]
    #[test]
    fn torrent_files_are_marked_from_the_internet() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.exe"), b"x").unwrap();
        std::fs::write(dir.path().join(".pad0"), b"").unwrap();
        let entry = |p: &str, padding| mdm_torrent::FileEntry {
            path: PathBuf::from(p),
            size: 1,
            padding,
        };
        mark_files(
            dir.path(),
            &[
                entry("sub/a.exe", false),
                entry(".pad0", true),
                entry("gone.bin", false),
            ],
        );
        let zone = |p: &Path| {
            let mut s = p.as_os_str().to_owned();
            s.push(":Zone.Identifier");
            std::fs::read_to_string(s).ok()
        };
        assert_eq!(
            zone(&dir.path().join("sub/a.exe")).as_deref(),
            Some("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n")
        );
        assert_eq!(
            zone(&dir.path().join(".pad0")),
            None,
            "padding isn't marked"
        );
        assert!(!dir.path().join("gone.bin").exists(), "nothing created");
    }
}

#[cfg(test)]
mod kept_files_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A Delete's changes on a disk that takes its time hold neither the caller nor the other
    /// torrents: `change` returns at once, the changes are made in order, and only what reads the
    /// same torrent's files waits for them.
    #[test]
    fn a_slow_change_holds_only_its_own_torrents_readers() {
        let kept = KeptFiles::new();
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let at = Instant::now();
        for n in 0..3 {
            let (gate, order) = (gate.clone(), order.clone());
            kept.change("ABC", move || {
                drop(gate.lock().unwrap());
                order.lock().unwrap().push(n);
            });
        }
        assert!(
            at.elapsed() < Duration::from_secs(5),
            "the caller never waits"
        );
        assert!(kept.changing() && kept.changing_for("abc"));
        assert!(!kept.changing_for("def"));
        kept.settle("def");
        let kept = Arc::new(kept);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn({
            let kept = kept.clone();
            move || {
                kept.settle("Abc");
                let _ = tx.send(());
            }
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "its reader waits"
        );
        drop(held);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(*order.lock().unwrap(), [0, 1, 2]);
        assert!(!kept.changing());
    }
}
