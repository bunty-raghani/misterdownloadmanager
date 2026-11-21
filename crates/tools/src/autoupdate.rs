//! Keeping MDM's own helpers up to date (#1544): yt-dlp, gallery-dl, ffmpeg and Node.js (#1570), the copies
//! MDM downloaded and checked itself. Sites change all the time, and a newer yt-dlp (or
//! gallery-dl) is usually what fixes a page that stopped working.
//!
//! - **When:** a check a few seconds after MDM starts, then again while it runs: yt-dlp and
//!   gallery-dl at most every 6 hours, Node.js once a day, ffmpeg once a week (BtbN rebuilds it each
//!   day, about 80 MB, for changes a merge seldom needs). When the next check is due is kept in
//!   `<tools>/updates.json`, so starting MDM many times asks GitHub nothing more. A check that
//!   failed is tried again an hour later; it is logged, never shown as an error.
//! - **How:** each tool's own install (`install_latest` and the like): the release's checksum list
//!   checked against its publisher's key (yt-dlp, gallery-dl, Node.js) or its published checksums
//!   (ffmpeg), the program against it, never an older version, installed beside the current one in
//!   its version's folder.
//! - **The switch** to the new version happens only while no download, look-up or other run of MDM
//!   (of any copy of MDM on this computer) is using that helper: a run holds the helper's
//!   `.locks/<tool>.in-use` file locked, shared ([`Hold`]), and the switch takes it alone. While it can't, the
//!   new version waits in `staged.json` and is switched to the moment the last run using the helper
//!   ends. The previous version is kept to go back to (`previous.json`, [`go_back`]); older ones
//!   are removed at the switch.
//! - **Only MDM's own copies:** a program the person chose (`chosen`) or the system's (`system`)
//!   is theirs to update.
//! - **Self-heal:** a video or gallery that fails with words that usually mean the helper is out of
//!   date ([`outdated`]) has MDM look for a newer one at once ([`heal`]), the app then trying that
//!   download again once with it.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{Error, Installed, newer};

/// One of MDM's own helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    GalleryDl,
    /// The JavaScript runtime MDM installs for YouTube (#1570): it takes the place of the Deno
    /// earlier versions installed (`check_with`, `jsruntime::retire_deno`).
    Node,
}

/// How long after a failed check the next one is tried.
pub const RETRY_AFTER_FAILURE: i64 = 3600;
/// The shortest time between two checks for a failing download ([`heal`]): a queue that tries a
/// failing video again and again asks GitHub once in that time.
pub const HEAL_EVERY: i64 = 600;

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::YtDlp, Tool::Ffmpeg, Tool::GalleryDl, Tool::Node];

    /// Its name, which is also its folder's in the tools folder.
    pub fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::GalleryDl => "gallery-dl",
            Tool::Node => "node",
        }
    }

    /// The tool whose folder is named `name`.
    pub fn of_folder(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The shortest time between two automatic checks, in seconds.
    pub fn every(self) -> i64 {
        match self {
            Tool::YtDlp | Tool::GalleryDl => 6 * 3600,
            Tool::Node => 24 * 3600,
            Tool::Ffmpeg => 7 * 24 * 3600,
        }
    }

    /// Its folder in the tools folder `dir`.
    pub fn root(self, dir: &Path) -> PathBuf {
        dir.join(self.name())
    }

    /// The person's own program chosen for it (Helpers → Use another program...): then MDM's own
    /// copy isn't used, and isn't updated.
    fn chosen(self, dir: &Path) -> Option<Installed> {
        use crate::chosen::Helper;
        let h = match self {
            Tool::YtDlp => Helper::YtDlp,
            Tool::Ffmpeg => Helper::Ffmpeg,
            Tool::GalleryDl => Helper::GalleryDl,
            // A runtime the person chose: MDM's own Node.js isn't used, nor updated.
            Tool::Node => {
                return crate::jsruntime::chosen(dir).map(|r| Installed {
                    version: r.version.unwrap_or_default(),
                    exe: r.exe,
                });
            }
        };
        crate::chosen::chosen(dir, h).filter(|c| {
            // A gallery-dl too old for MDM is passed over (#236): MDM's own is used.
            !(self == Tool::GalleryDl && crate::gallery::too_old(&c.version))
        })
    }
}

// ---- Which version is where -------------------------------------------------------------------

fn read_json(path: &Path) -> Option<Installed> {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice::<Installed>(&text)
        .ok()
        .filter(|i| i.exe.is_file())
}

fn write_json(path: &Path, i: &Installed) -> Result<(), Error> {
    let json = serde_json::to_vec_pretty(i).map_err(std::io::Error::other)?;
    craft_atomic::atomic_write(path, &json)?;
    Ok(())
}

/// The version in use of a tool whose folder is `root` (`current.json`), if its program is there.
pub fn current_in(root: &Path) -> Option<Installed> {
    read_json(&root.join("current.json"))
}

/// A newer version installed beside the current one, waiting for the runs using the helper to end
/// before it is switched to (`staged.json`).
pub fn staged_in(root: &Path) -> Option<Installed> {
    read_json(&root.join("staged.json"))
}

/// The version kept to go back to (`previous.json`, written at each switch; for an install made
/// before it was, the newest other version's folder holding the same program).
pub fn previous_in(root: &Path) -> Option<Installed> {
    if let Some(p) = read_json(&root.join("previous.json")) {
        return Some(p);
    }
    let current = current_in(root)?;
    let program = current.exe.file_name()?.to_owned();
    let folder = current.exe.parent()?.to_owned();
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| e.path() != folder && e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let exe = e.path().join(&program);
            // A same version's second folder (`<version>-2`, `folder`) is that version.
            let version = name.split('-').next().unwrap_or(&name).to_owned();
            (!name.starts_with('.') && exe.is_file() && newer(&current.version, &version))
                .then_some(Installed { version, exe })
        })
        .max_by(|a, b| {
            if newer(&a.version, &b.version) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        })
}

/// MDM's own copy of `tool` in the tools folder `dir`, when it is the one used (not overridden by
/// a program the person chose).
pub fn own(dir: &Path, tool: Tool) -> Option<Installed> {
    if tool.chosen(dir).is_some() {
        return None;
    }
    current_in(&tool.root(dir))
}

/// The newest version of MDM's own `tool` ready to run: the one waiting to be switched to, else
/// the current one. A download tried again after [`heal`] runs it at once.
pub fn ready(dir: &Path, tool: Tool) -> Option<Installed> {
    let current = own(dir, tool)?;
    match staged_in(&tool.root(dir)) {
        Some(s) if newer(&s.version, &current.version) => Some(s),
        _ => Some(current),
    }
}

/// A version already on the disk, staged or kept, that is `version`: switched to instead of
/// downloading it again (after going back, Update finds the newer one still there).
pub(crate) fn kept(root: &Path, version: &str) -> Option<Installed> {
    [staged_in(root), read_json(&root.join("previous.json"))]
        .into_iter()
        .flatten()
        .find(|i| i.version == version)
}

// ---- Locks -------------------------------------------------------------------------------------

/// The lock file `kind` of the tool whose folder is `root`: `<tools>/.locks/<tool><kind>`, beside
/// the tools' folders, never in them (a folder made only for a lock would pass for an install).
fn lock_file(root: &Path, kind: &str) -> std::io::Result<File> {
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    let locks = root.parent().unwrap_or(root).join(".locks");
    open_lock(&locks, &format!("{name}{kind}"))
}

/// `dir/name`, made if need be, opened to be locked.
fn open_lock(dir: &Path, name: &str) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(name))
}

/// A lock taken on a lock file, let go when dropped: unlocked first (`flock(LOCK_UN)`,
/// `UnlockFile`), then closed. Closing alone isn't enough on Linux and macOS: their locks belong
/// to the open file, which every process started meanwhile shares for a moment (a child holds a
/// copy of each descriptor from the moment it is created until it runs its program), so a lock
/// let go only by closing could outlive its holder there for that moment, and a switch tried
/// right then (as the last download using a helper ends) found it taken. Unlocking ends it for
/// every copy at once.
pub(crate) struct Locked(File);

impl Locked {
    /// A copy of its file handle (a child process's inherited copy, in tests).
    #[cfg(test)]
    fn copy(&self) -> File {
        self.0.try_clone().unwrap()
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The lock a run of a helper holds (shared) while it uses it, and the switch takes alone.
const IN_USE: &str = ".in-use";
/// The lock an install holds while it downloads and unpacks a version.
const INSTALLING: &str = ".installing";

/// A helper's folder held for an install: one at a time, in this copy of MDM or another.
pub(crate) struct Installing(#[allow(dead_code)] Locked);

/// Take `root`'s install lock, or say another install has it.
pub(crate) fn installing(root: &Path, tool: &str) -> Result<Installing, Error> {
    let f = lock_file(root, INSTALLING)?;
    match f.try_lock() {
        Ok(()) => Ok(Installing(Locked(f))),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::Busy(tool.to_owned())),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The helpers a run uses, held for as long as it lives: none of them is switched to a new version
/// meanwhile (in this copy of MDM or another). Dropped, a new version waiting for them is switched
/// to at once, as the last run using it ends ([`apply_staged`]).
#[derive(Default)]
pub struct Hold {
    held: Vec<(PathBuf, Locked)>,
}

impl Hold {
    /// Hold MDM's own helpers among `paths` (programs, their folders, arguments naming them): each
    /// path inside a tool's folder (one with a `current.json`) holds that tool. Anything else (a
    /// program the person chose, the system's) holds nothing.
    pub fn of<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Hold {
        let mut hold = Hold::default();
        for p in paths {
            if let Some(root) = tool_root(p)
                && !hold.held.iter().any(|(r, _)| *r == root)
                && let Ok(f) = lock_file(&root, IN_USE)
            {
                // Waits only while a switch writes `current.json` (a moment).
                if f.lock_shared().is_ok() {
                    hold.held.push((root, Locked(f)));
                }
            }
        }
        hold
    }

    /// The tools held, for tests and the log.
    pub fn tools(&self) -> Vec<Tool> {
        self.held
            .iter()
            .filter_map(|(r, _)| Tool::of_folder(&r.file_name()?.to_string_lossy()))
            .collect()
    }

    /// Whether the tool whose folder is `root` is held.
    #[cfg(test)]
    pub(crate) fn holds(&self, root: &Path) -> bool {
        self.held.iter().any(|(r, _)| r == root)
    }

    /// Hold the helpers a command runs: its program and every argument that names one (yt-dlp's
    /// `--ffmpeg-location`, `--js-runtimes node:<path>`).
    pub fn of_command(cmd: &std::process::Command) -> Hold {
        let program = Path::new(cmd.get_program());
        let args: Vec<&Path> = cmd
            .get_args()
            .map(|a| {
                let a = a.to_str().unwrap_or_default();
                // `node:<path>`
                Path::new(
                    a.split_once(':')
                        .filter(|(k, _)| k.len() > 1)
                        .map_or(a, |(_, p)| p),
                )
            })
            .filter(|p| p.is_absolute())
            .collect();
        Hold::of(std::iter::once(program).chain(args))
    }
}

impl Hold {
    /// Let go here and now, switching to what waited for it on this thread: the versions switched
    /// to. (Dropped, it does the same on a thread of its own.)
    pub fn release(mut self) -> Vec<Installed> {
        let mut switched = Vec::new();
        for (root, f) in std::mem::take(&mut self.held) {
            drop(f);
            switched.extend(apply_staged(&root));
        }
        switched
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        for (root, f) in std::mem::take(&mut self.held) {
            drop(f); // unlocked, then closed
            if root.join("staged.json").exists() {
                // Switched here (a moment: two small files written), even as the process ends
                // (the command line's last download); the oldest version's folder goes on a
                // thread of its own.
                switch_staged(&root, Tidy::Later);
            }
        }
    }
}

/// Folders of MDM's own helpers that are no tool it updates any more, held all the same while a
/// run uses them, so that their removal waits for it: the Deno earlier versions installed (#1570).
const RETIRED: [&str; 1] = ["deno"];

/// The tool's folder `path` is in: an ancestor named after one of the tools (or a retired one),
/// with a `current.json`.
fn tool_root(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    path.ancestors()
        .skip(1)
        .find(|a| {
            a.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| Tool::of_folder(n).is_some() || RETIRED.contains(&n))
                && a.join("current.json").is_file()
        })
        .map(Path::to_owned)
}

/// The tool in `root` held alone (no run of any copy of MDM uses it) until dropped; None while one
/// does.
pub(crate) fn alone(root: &Path) -> Option<Locked> {
    let f = lock_file(root, IN_USE).ok()?;
    f.try_lock().ok().map(|()| Locked(f))
}

/// Whether a run of this copy of MDM or another uses the tool in `root` now.
pub fn in_use(root: &Path) -> bool {
    let Ok(f) = lock_file(root, IN_USE) else {
        return false;
    };
    match f.try_lock() {
        Ok(()) => {
            drop(Locked(f));
            false
        }
        Err(_) => true,
    }
}

// ---- The switch --------------------------------------------------------------------------------

/// What became of a version just installed beside the current one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    /// It is the current version now.
    Switched(Installed),
    /// Runs use the helper: it waits in `staged.json`, switched to when they end.
    Staged(Installed),
}

/// Make `new` (installed in `root`'s folder for its version) current when no run uses the tool,
/// else leave it staged for when they end. The caller holds `root`'s install lock.
pub(crate) fn settle(root: &Path, new: Installed) -> Result<Settled, Error> {
    settle_tidy(root, new, Tidy::Now)
}

/// When the folders a switch leaves behind are removed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tidy {
    Now,
    /// On a thread of its own.
    Later,
}

/// [`settle`], the older folders removed as `tidy` says.
fn settle_tidy(root: &Path, new: Installed, tidy: Tidy) -> Result<Settled, Error> {
    let lock = lock_file(root, IN_USE)?;
    match lock.try_lock() {
        Ok(()) => {
            let lock = Locked(lock);
            let keep = crate::switch_current(root, &new)?;
            let _ = std::fs::remove_file(root.join("staged.json"));
            // New runs take the new version from now on; the older folders go after.
            drop(lock);
            match tidy {
                Tidy::Now => crate::remove_others(root, &keep),
                Tidy::Later => {
                    let root = root.to_owned();
                    std::thread::spawn(move || crate::remove_others(&root, &keep));
                }
            }
            bump();
            Ok(Settled::Switched(new))
        }
        Err(_) => {
            write_json(&root.join("staged.json"), &new)?;
            bump();
            tracing::info!(
                "{} {} is ready: switched to once the runs using it end",
                root.file_name().unwrap_or_default().to_string_lossy(),
                new.version
            );
            Ok(Settled::Staged(new))
        }
    }
}

/// Switch to the version waiting in `root`'s `staged.json`, if any and nothing uses the tool now
/// (nor installs it). Returns it once switched.
pub fn apply_staged(root: &Path) -> Option<Installed> {
    switch_staged(root, Tidy::Now)
}

/// [`apply_staged`], the older folders removed as `tidy` says.
fn switch_staged(root: &Path, tidy: Tidy) -> Option<Installed> {
    let staged = staged_in(root)?;
    let tool = root.file_name()?.to_string_lossy().into_owned();
    let _installing = installing(root, &tool).ok()?;
    let from = current_in(root).map(|c| c.version);
    match settle_tidy(root, staged, tidy) {
        Ok(Settled::Switched(now)) => {
            tracing::info!("{tool} switched to {}", now.version);
            if let (Some(dir), Some(from)) = (root.parent(), from)
                && newer(&now.version, &from)
            {
                let at = unix_now();
                let (to, tool) = (now.version.clone(), tool.clone());
                change(dir, |s| {
                    let t = s.tools.entry(tool).or_default();
                    t.updated = Some(Updated { from, to, at });
                });
            }
            Some(now)
        }
        _ => None,
    }
}

/// [`apply_staged`] for every tool in the tools folder `dir`: what waited for runs that ended in
/// another copy of MDM.
pub fn apply_all_staged(dir: &Path) {
    for t in Tool::ALL {
        apply_staged(&t.root(dir));
    }
}

/// Go back to the version kept before the current one: switched to now, or once the runs using
/// the helper end. Automatic updates then wait for a version newer than the one left (`skip`);
/// Update installs the newest again.
pub fn go_back(dir: &Path, tool: Tool) -> Result<Settled, Error> {
    let root = tool.root(dir);
    let current =
        own(dir, tool).ok_or_else(|| Error::Missing(format!("{} of MDM's own", tool.name())))?;
    let previous = previous_in(&root)
        .filter(|p| newer(&current.version, &p.version))
        .ok_or_else(|| Error::Missing("an earlier version to go back to".into()))?;
    let _installing = installing(&root, tool.name())?;
    let settled = settle(&root, previous)?;
    change(dir, |s| {
        let t = s.tools.entry(tool.name().to_owned()).or_default();
        t.skip = Some(current.version.clone());
        t.updated = None;
    });
    tracing::info!("{} going back from {}", tool.name(), current.version);
    Ok(settled)
}

// ---- What is remembered across starts ----------------------------------------------------------

/// `<tools>/updates.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// "Keep MDM's helpers up to date automatically" (Helpers), on unless turned off.
    #[serde(default = "yes")]
    pub auto: bool,
    /// Each tool's, by its name.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolState>,
}

fn yes() -> bool {
    true
}

impl Default for State {
    fn default() -> Self {
        State {
            auto: true,
            tools: BTreeMap::new(),
        }
    }
}

impl State {
    pub fn of(&self, tool: Tool) -> ToolState {
        self.tools.get(tool.name()).cloned().unwrap_or_default()
    }
}

/// One tool's checks.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolState {
    /// When the next automatic check is due (seconds since 1970).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
    /// When it was last checked, and the newest version found then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// The person went back from this version: automatic updates wait for a newer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    /// The last update, shown in Helpers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<Updated>,
    /// Why the last check failed (it is tried again later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// When a failing download last had it checked ([`heal`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healed: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Updated {
    pub from: String,
    pub to: String,
    pub at: i64,
}

const STATE: &str = "updates.json";

/// The tools folder's update state (defaults when there is none or it can't be read).
pub fn read(dir: &Path) -> State {
    std::fs::read(dir.join(STATE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Change the update state under its lock (copies of MDM may change it at once) and write it.
/// Returns it as written.
pub fn change(dir: &Path, f: impl FnOnce(&mut State)) -> State {
    let lock = open_lock(&dir.join(".locks"), "updates")
        .ok()
        .and_then(|l| l.lock().is_ok().then_some(Locked(l)));
    let mut state = read(dir);
    f(&mut state);
    match serde_json::to_vec_pretty(&state) {
        Ok(json) => {
            if let Err(e) = craft_atomic::atomic_write(&dir.join(STATE), &json) {
                tracing::warn!("helper updates: couldn't save {STATE}: {e}");
            }
        }
        Err(e) => tracing::warn!("helper updates: {e}"),
    }
    drop(lock);
    bump();
    state
}

/// Turn automatic updates on or off.
pub fn set_auto(dir: &Path, on: bool) {
    change(dir, |s| s.auto = on);
}

/// Whether an automatic check of a tool (`t`, checked at most `every` seconds apart) is due at
/// `now`. A next check further away than `every` (the clock was turned back) is due too.
pub fn due(t: &ToolState, every: i64, now: i64) -> bool {
    match t.next {
        None => true,
        Some(next) => now >= next || next - now > every,
    }
}

/// Whether version `a` is newer than `b` (numeric parts in order: `2026.11.13` > `2026.10.26`).
pub fn is_newer(a: &str, b: &str) -> bool {
    newer(a, b)
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// ---- What is happening now, for the Helpers window ----------------------------------------------

/// Changes to the state, the helpers' versions or the checks under way: the Helpers window reads
/// the files again when it moves.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn bump() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The checks under way in this copy of MDM: the tools folder and the tool, each with its
/// download's progress.
type Checks = Vec<(PathBuf, Tool, Arc<crate::update::Progress>)>;
static LIVE: Mutex<Checks> = Mutex::new(Vec::new());

/// A check of `tool` in the tools folder `dir` under way here: its download's progress (nothing
/// yet while it asks).
pub fn checking(dir: &Path, tool: Tool) -> Option<Arc<crate::update::Progress>> {
    LIVE.lock()
        .unwrap()
        .iter()
        .find(|(d, t, _)| d == dir && *t == tool)
        .map(|(_, _, p)| p.clone())
}

/// A check marked as under way until dropped.
struct Live(PathBuf, Tool);

impl Live {
    fn start(dir: &Path, tool: Tool) -> Option<(Live, Arc<crate::update::Progress>)> {
        let mut live = LIVE.lock().unwrap();
        if live.iter().any(|(d, t, _)| d == dir && *t == tool) {
            return None;
        }
        let p = Arc::new(crate::update::Progress::default());
        live.push((dir.to_owned(), tool, p.clone()));
        drop(live);
        bump();
        Some((Live(dir.to_owned(), tool), p))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        LIVE.lock()
            .unwrap()
            .retain(|(d, t, _)| !(*d == self.0 && *t == self.1));
        bump();
    }
}

// ---- Checks ------------------------------------------------------------------------------------

/// Why a check runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// MDM's own, while it runs: only when due ([`due`]) and turned on, never past a version the
    /// person went back from.
    Auto,
    /// Check for updates, Update, `update-helpers`: now, the newest whatever was gone back from.
    Asked,
    /// A download failed with words that usually mean the helper is out of date ([`heal`]).
    Heal,
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// The person's own program or the system's, or none: not MDM's to update.
    NotOurs,
    /// Not due yet, or automatic updates are off.
    NotDue,
    /// The newest is the one in use (`latest`: the newest release's version).
    UpToDate { now: Installed, latest: String },
    /// A newer version, verified, is in use from now on.
    Updated { from: String, now: Installed },
    /// A newer version, verified, waits for the runs using the helper to end.
    Ready { now: Installed, ready: Installed },
    /// The newest is the version the person went back from: kept as it is.
    Skipped { now: Installed, latest: String },
    /// Another check or install of it is under way.
    Busy,
}

/// What a tool's install found and did ([`Install`]).
#[derive(Clone, Debug)]
pub struct Got {
    /// The newest release's version.
    pub latest: String,
    /// What became of it, when it was newer than `floor` (installed or found kept).
    pub settled: Option<Settled>,
}

/// A tool's install: from its release, verified, never at or below `floor` (when given; the
/// current version always counts), its download counted into `progress`.
pub type Install = for<'a> fn(
    &'a Path,
    &'a mdm_engine::Options,
    Option<&'a crate::update::Progress>,
    Option<&'a str>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Got, Error>> + Send + 'a>,
>;

/// Each tool's own install.
fn install_of(tool: Tool) -> Install {
    match tool {
        Tool::YtDlp => |d, n, p, f| Box::pin(crate::install_with(d, n, p, f)),
        Tool::Ffmpeg => |d, n, p, f| Box::pin(crate::ffmpeg::install_with(d, n, p, f)),
        Tool::GalleryDl => |d, n, p, f| Box::pin(crate::gallery::install_with(d, n, p, f)),
        Tool::Node => |d, n, p, f| Box::pin(crate::node::install_with(d, n, p, f)),
    }
}

/// Check `tool` (MDM's own copy in the tools folder `dir`) for a newer version and install it,
/// as `why` says, at `now` (seconds since 1970).
pub async fn check(
    dir: &Path,
    tool: Tool,
    net: &mdm_engine::Options,
    why: Why,
    now: i64,
) -> Result<Checked, Error> {
    check_with(dir, tool, net, why, now, install_of(tool)).await
}

/// [`check`] with the tool's install given (tests: one from a release on a test server).
pub async fn check_with(
    dir: &Path,
    tool: Tool,
    net: &mdm_engine::Options,
    why: Why,
    now: i64,
    install: Install,
) -> Result<Checked, Error> {
    let Some(current) = own(dir, tool).or_else(|| deno_to_replace(dir, tool)) else {
        return Ok(Checked::NotOurs);
    };
    let every = tool.every();
    // Claimed before asking: copies of MDM started together ask once.
    let mut go = true;
    let state = change(dir, |s| {
        let auto = s.auto;
        let t = s.tools.entry(tool.name().to_owned()).or_default();
        go = match why {
            Why::Auto => auto && due(t, every, now),
            Why::Asked | Why::Heal => true,
        };
        if go {
            t.next = Some(now + RETRY_AFTER_FAILURE.min(every));
        }
        if why == Why::Heal {
            t.healed = Some(now);
        }
    });
    if !go {
        return Ok(Checked::NotDue);
    }
    let Some((_live, progress)) = Live::start(dir, tool) else {
        return Ok(Checked::Busy);
    };
    let skip = match why {
        Why::Asked => None,
        Why::Auto | Why::Heal => state.of(tool).skip,
    };
    let got = install(dir, net, Some(progress.as_ref()), skip.as_deref()).await;
    match got {
        Ok(Got { latest, settled }) => {
            let latest_seen = latest.clone();
            let found = match settled {
                Some(Settled::Switched(now)) if now.version != current.version => {
                    Checked::Updated {
                        from: current.version.clone(),
                        now,
                    }
                }
                Some(Settled::Staged(ready)) => Checked::Ready {
                    now: current.clone(),
                    ready,
                },
                _ if skip.as_deref().is_some_and(|s| !newer(&latest, s))
                    && newer(&latest, &current.version) =>
                {
                    Checked::Skipped {
                        now: current.clone(),
                        latest,
                    }
                }
                _ => Checked::UpToDate {
                    now: own(dir, tool).unwrap_or(current.clone()),
                    latest,
                },
            };
            change(dir, |s| {
                let t = s.tools.entry(tool.name().to_owned()).or_default();
                t.next = Some(now + every);
                t.checked = Some(now);
                t.latest = Some(latest_seen);
                t.failed = None;
                if why == Why::Asked {
                    t.skip = None;
                }
                if let Checked::Updated { from, now: n } = &found {
                    t.updated = Some(Updated {
                        from: from.clone(),
                        to: n.version.clone(),
                        at: now,
                    });
                }
            });
            if tool == Tool::Node {
                crate::jsruntime::retire_deno(dir);
            }
            match &found {
                Checked::Updated { from, now } => {
                    tracing::info!("{} updated: {from} -> {}", tool.name(), now.version)
                }
                Checked::Ready { ready, .. } => tracing::info!(
                    "{} {} downloaded and verified: in use once the runs using it end",
                    tool.name(),
                    ready.version
                ),
                _ => {}
            }
            Ok(found)
        }
        Err(e) => {
            tracing::warn!("{} update check failed: {e}", tool.name());
            let why_failed = e.to_string();
            change(dir, |s| {
                let t = s.tools.entry(tool.name().to_owned()).or_default();
                t.failed = Some(why_failed);
            });
            Err(e)
        }
    }
}

/// #1570: MDM's own Deno, from an earlier version, standing for its Node.js in a check of
/// [`Tool::Node`] (`from` "Deno <version>"): the check installs Node.js as an update would, and
/// the Deno is then removed. Not when the person chose a runtime, nor where Node.js has no build.
fn deno_to_replace(dir: &Path, tool: Tool) -> Option<Installed> {
    if tool != Tool::Node || tool.chosen(dir).is_some() || !crate::node::available() {
        return None;
    }
    crate::deno::installed(dir).map(|d| Installed {
        version: format!("Deno {}", d.version),
        exe: d.exe,
    })
}

/// The automatic checks due at `now`, one tool after the other, and the versions waiting for runs
/// that ended elsewhere. Nothing is said: each is logged.
pub async fn run_due(dir: &Path, net: &mdm_engine::Options, now: i64) -> Vec<(Tool, Checked)> {
    apply_all_staged(dir);
    // A Deno of MDM's own left by a run that used it as its Node.js came (#1570).
    crate::jsruntime::retire_deno(dir);
    let mut out = Vec::new();
    for tool in Tool::ALL {
        if let Ok(c) = check(dir, tool, net, Why::Auto, now).await
            && c != Checked::NotDue
            && c != Checked::NotOurs
        {
            out.push((tool, c));
        }
    }
    out
}

/// A download run with `tool` at `used` failed with words that usually mean it is out of date
/// ([`outdated`]): a newer version ready to run, if there is one ([`ready`]: one installed meanwhile,
/// else one checked for now, at most once in [`HEAL_EVERY`]). None: the failure stands.
pub async fn heal(
    dir: &Path,
    tool: Tool,
    used: &str,
    net: &mdm_engine::Options,
    now: i64,
) -> Result<Option<Installed>, Error> {
    let newer_ready = || ready(dir, tool).filter(|r| newer(&r.version, used));
    if own(dir, tool).is_none() {
        return Ok(None);
    }
    if let Some(r) = newer_ready() {
        return Ok(Some(r));
    }
    let t = read(dir).of(tool);
    if t.healed
        .is_some_and(|h| (0..HEAL_EVERY).contains(&(now - h)))
    {
        tracing::info!(
            "{}: checked for a failing download moments ago",
            tool.name()
        );
        return Ok(None);
    }
    check(dir, tool, net, Why::Heal, now).await?;
    Ok(newer_ready())
}

// ---- Words that mean "out of date" --------------------------------------------------------------

/// Whether a failure `message` of `tool` usually means the tool is out of date with the site: a
/// newer release is then worth looking for. yt-dlp's words, from its source:
/// - `Unable to extract …` (an extractor's pattern no longer matches the page);
/// - `Unsupported URL` for a page it looked up before;
/// - its bug-report hint, `… Confirm you are on the latest version using yt-dlp -U`;
/// - YouTube's `Sign in to confirm you're not a bot` (new releases change how it asks);
/// - `HTTP Error 403` on the streams or the player API;
/// - YouTube's `n challenge solving failed`, `Signature solving failed`, `nsig extraction failed`,
///   `Failed to extract any player response`, `Requested format is not available`,
///   `No video formats found`, and `The following content is not available on this app` (what an
///   old release is told: `tests/fixtures/ytdlp-refusals/outdated-client.stderr`).
///
/// gallery-dl's: `An unexpected error occurred: … report this issue` (an extractor that broke),
/// `Failed to parse JSON data`, `Unsupported URL`, `No suitable extractor`.
///
/// Never a network failure (a timeout, a server's 5xx) nor a refusal a newer version can't help
/// (a private, age-restricted or members-only video, a missing file).
pub fn outdated(tool: Tool, message: &str) -> bool {
    if crate::proc::network_failure(message) || message.contains("CERTIFICATE_VERIFY_FAILED") {
        return false;
    }
    let m = message.to_lowercase().replace('\u{2019}', "'");
    let any = |words: &[&str]| words.iter().any(|w| m.contains(w));
    match tool {
        Tool::YtDlp => {
            any(&[
                "unable to extract",
                "unsupported url",
                "confirm you are on the latest version",
                "yt-dlp -u",
                "sign in to confirm you're not a bot",
                "http error 403",
                "challenge solving failed",
                "signature solving failed",
                "nsig extraction failed",
                "signature extraction failed",
                "failed to extract any player response",
                "requested format is not available",
                "no video formats found",
                "is older than 90 days",
                // YouTube to a player client it no longer serves (yt-dlp 2025.06.30, 2026-11-20).
                "not available on this app",
            ]) && !any(&[
                "private video",
                "confirm your age",
                "members-only",
                "channel's members",
                "geo restriction",
                "geo-restriction",
                "not available in your country",
            ])
        }
        Tool::GalleryDl => any(&[
            "an unexpected error occurred",
            "report this issue",
            "failed to parse json data",
            "unsupported url",
            "no suitable extractor",
        ]),
        Tool::Ffmpeg | Tool::Node => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    #[test]
    fn checks_are_due_by_their_own_clock() {
        let every = Tool::YtDlp.every();
        assert_eq!(every, 6 * H);
        let fresh = ToolState::default();
        assert!(due(&fresh, every, 1_000_000), "never checked: due");
        let t = ToolState {
            next: Some(1_000_000 + every),
            ..Default::default()
        };
        assert!(!due(&t, every, 1_000_000));
        assert!(!due(&t, every, 1_000_000 + every - 1));
        assert!(due(&t, every, 1_000_000 + every));
        // The clock turned back a day: due, not a day late.
        assert!(due(&t, every, 1_000_000 - 24 * H));
        assert!(Tool::Node.every() == 24 * H && Tool::Ffmpeg.every() >= 24 * H);
    }

    /// #1570: a copy of MDM with its own Deno (an earlier version's) gets Node.js by the
    /// runtime's update check, its Deno kept until Node.js is in place, then removed; a Deno of
    /// the person's own is never touched, and with a runtime the person chose nothing is
    /// installed.
    #[tokio::test]
    async fn mdms_deno_is_replaced_by_its_node() {
        if !crate::node::available() {
            return; // no Node.js build for this system: the Deno stays
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let deno_root = d.join("deno");
        let deno = {
            let exe = deno_root
                .join("2.5.0")
                .join(format!("deno{}", std::env::consts::EXE_SUFFIX));
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"deno").unwrap();
            crate::make_current(
                &deno_root,
                Installed {
                    version: "2.5.0".into(),
                    exe,
                },
            )
            .unwrap()
        };
        // The person's own Deno, elsewhere.
        let theirs = tempfile::tempdir().unwrap();
        let their_deno = theirs.path().join("deno");
        std::fs::write(&their_deno, b"theirs").unwrap();
        fn installs_node<'a>(
            d: &'a Path,
            _: &'a mdm_engine::Options,
            _: Option<&'a crate::update::Progress>,
            _: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Got, Error>> + Send + 'a>>
        {
            Box::pin(async move {
                // Meanwhile, the Deno is still what yt-dlp is told of.
                assert_eq!(
                    crate::jsruntime::own(d).unwrap().kind,
                    crate::jsruntime::Kind::Deno
                );
                let root = Tool::Node.root(d);
                let exe = root.join("26.13.1").join("node");
                std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
                std::fs::write(&exe, b"node").unwrap();
                let settled = settle(
                    &root,
                    Installed {
                        version: "26.13.1".into(),
                        exe,
                    },
                )?;
                Ok(Got {
                    latest: "26.13.1".into(),
                    settled: Some(settled),
                })
            })
        }
        let net = crate::net(mdm_engine::Proxy::None);
        // With a runtime the person chose: not MDM's to install.
        crate::chosen::choose_runtime(
            d,
            crate::chosen::ChosenRuntime {
                kind: "deno".into(),
                version: "2.6.0".into(),
                exe: their_deno.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            check_with(d, Tool::Node, &net, Why::Auto, 0, installs_node)
                .await
                .unwrap(),
            Checked::NotOurs
        );
        assert!(deno.exe.is_file());
        crate::jsruntime::forget(d).unwrap();
        // Without: Node.js comes as an update of the runtime, and the Deno goes.
        let found = check_with(d, Tool::Node, &net, Why::Auto, 0, installs_node)
            .await
            .unwrap();
        match found {
            Checked::Updated { from, now } => {
                assert_eq!(from, "Deno 2.5.0");
                assert_eq!(now.version, "26.13.1");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            crate::jsruntime::own(d).unwrap().kind,
            crate::jsruntime::Kind::Node
        );
        assert!(!deno_root.exists(), "MDM's own Deno removed");
        assert_eq!(std::fs::read(&their_deno).unwrap(), b"theirs");
        assert_eq!(own(d, Tool::Node).unwrap().version, "26.13.1");
    }

    /// Restarts don't ask again: what is due is read from `updates.json` each time, the clock
    /// given (a fake one here), and a check claims its next time before asking.
    #[tokio::test]
    async fn the_rate_limit_holds_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fake_current(d, Tool::YtDlp, "2026.01.01");
        static ASKED: AtomicU64 = AtomicU64::new(0);
        fn counted<'a>(
            _: &'a Path,
            _: &'a mdm_engine::Options,
            _: Option<&'a crate::update::Progress>,
            _: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Got, Error>> + Send + 'a>>
        {
            Box::pin(async {
                ASKED.fetch_add(1, Ordering::SeqCst);
                Ok(Got {
                    latest: "2026.01.01".into(),
                    settled: None,
                })
            })
        }
        let net = crate::net(mdm_engine::Proxy::None);
        let t0 = 2_000_000_000;
        let run = |now| check_with(d, Tool::YtDlp, &net, Why::Auto, now, counted);
        assert!(matches!(run(t0).await.unwrap(), Checked::UpToDate { .. }));
        // Ten "restarts" within the hour, then within six: nothing asked.
        for k in 0..10 {
            assert_eq!(run(t0 + k * 60).await.unwrap(), Checked::NotDue);
        }
        assert_eq!(run(t0 + 6 * H - 1).await.unwrap(), Checked::NotDue);
        assert_eq!(ASKED.load(Ordering::SeqCst), 1);
        assert!(matches!(
            run(t0 + 6 * H).await.unwrap(),
            Checked::UpToDate { .. }
        ));
        assert_eq!(ASKED.load(Ordering::SeqCst), 2);
        // Asked for (Check for updates), it goes at once.
        check_with(d, Tool::YtDlp, &net, Why::Asked, t0 + 6 * H + 5, counted)
            .await
            .unwrap();
        assert_eq!(ASKED.load(Ordering::SeqCst), 3);
        // Turned off: never by itself.
        set_auto(d, false);
        assert_eq!(run(t0 + 100 * H).await.unwrap(), Checked::NotDue);
        assert_eq!(ASKED.load(Ordering::SeqCst), 3);
        assert!(!read(d).auto);
    }

    /// A check that failed is tried again an hour later, not at once and not six hours later;
    /// what failed is kept for Helpers.
    #[tokio::test]
    async fn a_failed_check_is_tried_again_later() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fake_current(d, Tool::GalleryDl, "1.30.0");
        fn failing<'a>(
            _: &'a Path,
            _: &'a mdm_engine::Options,
            _: Option<&'a crate::update::Progress>,
            _: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Got, Error>> + Send + 'a>>
        {
            Box::pin(async { Err(Error::Missing("the release".into())) })
        }
        let net = crate::net(mdm_engine::Proxy::None);
        let t0 = 2_000_000_000;
        assert!(
            check_with(d, Tool::GalleryDl, &net, Why::Auto, t0, failing)
                .await
                .is_err()
        );
        let t = read(d).of(Tool::GalleryDl);
        assert_eq!(t.next, Some(t0 + RETRY_AFTER_FAILURE));
        assert!(t.failed.unwrap().contains("the release"));
        assert_eq!(
            check_with(d, Tool::GalleryDl, &net, Why::Auto, t0 + 60, failing)
                .await
                .unwrap(),
            Checked::NotDue
        );
    }

    /// Only MDM's own copies: one the person chose, or none, is never checked.
    #[tokio::test]
    async fn chosen_programs_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let net = crate::net(mdm_engine::Proxy::None);
        assert_eq!(
            check(d, Tool::YtDlp, &net, Why::Asked, 0).await.unwrap(),
            Checked::NotOurs
        );
        fake_current(d, Tool::YtDlp, "2026.01.01");
        let mine = d.join("my-yt-dlp");
        std::fs::write(&mine, b"x").unwrap();
        std::fs::write(
            d.join("programs.json"),
            serde_json::to_vec(&serde_json::json!({
                "yt-dlp": {"version": "2025.01.01", "exe": mine}
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(own(d, Tool::YtDlp), None);
        assert_eq!(
            check(d, Tool::YtDlp, &net, Why::Asked, 0).await.unwrap(),
            Checked::NotOurs
        );
    }

    /// `tool`'s `version` as MDM's own current copy in the tools folder `d`.
    fn fake_current(d: &Path, tool: Tool, version: &str) -> Installed {
        let root = tool.root(d);
        let folder = root.join(version);
        std::fs::create_dir_all(folder.join("_internal")).unwrap();
        let exe = folder.join("prog");
        std::fs::write(&exe, version).unwrap();
        crate::make_current(
            &root,
            Installed {
                version: version.into(),
                exe,
            },
        )
        .unwrap()
    }

    /// A version installed beside a helper in use waits; the moment the last run holding it ends,
    /// it is switched to, the previous kept, the one before removed.
    #[test]
    fn the_switch_waits_for_the_runs_using_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let root = Tool::YtDlp.root(d);
        fake_current(d, Tool::YtDlp, "2026.01.01");
        let one = fake_current(d, Tool::YtDlp, "2026.02.01");
        // A job running 2026.02.01, and a look-up running it too.
        let job = Hold::of([one.exe.as_path()]);
        let lookup = Hold::of([one.exe.parent().unwrap()]);
        assert_eq!(job.tools(), vec![Tool::YtDlp]);
        assert!(in_use(&root));
        // Something else holds nothing.
        assert!(
            Hold::of([d.join("elsewhere/yt-dlp.exe").as_path()])
                .tools()
                .is_empty()
        );

        let folder = root.join("2026.03.01");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("prog"), b"three").unwrap();
        let three = Installed {
            version: "2026.03.01".into(),
            exe: folder.join("prog"),
        };
        let lock = installing(&root, "yt-dlp").unwrap();
        assert_eq!(
            settle(&root, three.clone()).unwrap(),
            Settled::Staged(three.clone())
        );
        drop(lock);
        assert_eq!(current_in(&root).unwrap().version, "2026.02.01");
        assert_eq!(staged_in(&root), Some(three.clone()));
        assert_eq!(ready(d, Tool::YtDlp), Some(three.clone()));
        assert!(
            root.join("2026.01.01").is_dir(),
            "nothing removed meanwhile"
        );
        // Not while one run is left; at once when none is (`release`: what a drop does on a
        // thread of its own, done here).
        assert_eq!(job.release(), vec![]);
        assert_eq!(current_in(&root).unwrap().version, "2026.02.01");
        assert_eq!(lookup.release(), vec![three.clone()]);
        assert_eq!(current_in(&root), Some(three.clone()));
        assert_eq!(staged_in(&root), None);
        assert_eq!(previous_in(&root).unwrap().version, "2026.02.01");
        assert!(!root.join("2026.01.01").exists(), "the oldest goes");
        assert!(root.join("2026.02.01").is_dir(), "the previous stays");
    }

    /// #1565 (macOS CI): a lock is let go by unlocking it, not only by closing it. A process
    /// started while a download held yt-dlp holds a copy of that lock's file handle for a moment
    /// (until it runs its program; here, a copy left open); with the lock let go only by closing,
    /// that copy kept it, so the switch as the download ended found yt-dlp in use and left the
    /// new version waiting. Likewise an install's lock: another install must not find it taken.
    #[test]
    fn a_copy_of_a_lock_left_open_elsewhere_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let root = Tool::YtDlp.root(d);
        let one = fake_current(d, Tool::YtDlp, "2026.01.01");
        let job = Hold::of([one.exe.as_path()]);
        let inherited = job.held[0].1.copy();
        let folder = root.join("2026.02.01");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("prog"), b"two").unwrap();
        let two = Installed {
            version: "2026.02.01".into(),
            exe: folder.join("prog"),
        };
        let installing_copy = {
            let lock = installing(&root, "yt-dlp").unwrap();
            let copy = lock.0.copy();
            assert_eq!(
                settle(&root, two.clone()).unwrap(),
                Settled::Staged(two.clone())
            );
            copy
        };
        // The install's copy still open: another install goes ahead all the same.
        assert!(
            installing(&root, "yt-dlp").is_ok(),
            "the install lock is free"
        );
        assert_eq!(
            job.release(),
            vec![two.clone()],
            "switched as the run ended"
        );
        assert_eq!(current_in(&root), Some(two));
        assert!(!in_use(&root));
        drop((inherited, installing_copy));
    }

    /// Go back: the previous version is used again, the newer one kept, and automatic updates
    /// don't bring the version left back; asking for an update does, without downloading it.
    #[test]
    fn going_back_and_forth() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let root = Tool::Node.root(d);
        fake_current(d, Tool::Node, "2.5.0");
        let two = fake_current(d, Tool::Node, "2.6.0");
        assert_eq!(previous_in(&root).unwrap().version, "2.5.0");
        assert!(
            matches!(go_back(d, Tool::Node).unwrap(), Settled::Switched(i) if i.version == "2.5.0")
        );
        assert_eq!(current_in(&root).unwrap().version, "2.5.0");
        assert_eq!(read(d).of(Tool::Node).skip.as_deref(), Some("2.6.0"));
        // Nothing older to go back to now.
        assert!(go_back(d, Tool::Node).is_err());
        // The newer one is still there to switch to again.
        assert_eq!(kept(&root, "2.6.0"), Some(two));
    }

    /// Without `previous.json` (an install from before it was written), the newest other
    /// version's folder is the previous one.
    #[test]
    fn the_previous_version_of_an_older_install() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let root = Tool::YtDlp.root(d);
        fake_current(d, Tool::YtDlp, "2026.01.01");
        fake_current(d, Tool::YtDlp, "2026.02.01");
        std::fs::remove_file(root.join("previous.json")).unwrap();
        assert_eq!(previous_in(&root).unwrap().version, "2026.01.01");
    }

    /// The lab server's address for [`from_lab`].
    static LAB: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    /// The fake release's list is signed by the test's own throwaway key (never yt-dlp's).
    fn signed_by_test_key(sums: &[u8], sig: &[u8]) -> bool {
        crate::signed_by(
            include_str!("../tests/fixtures/helper-update/test-key.asc"),
            sums,
            sig,
        )
    }

    /// yt-dlp's install, from the fake GitHub on the lab server ([`LAB`]).
    fn from_lab<'a>(
        dir: &'a Path,
        net: &'a mdm_engine::Options,
        progress: Option<&'a crate::update::Progress>,
        floor: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Got, Error>> + Send + 'a>> {
        Box::pin(async move {
            let base = LAB.get().unwrap();
            let api = format!("{base}/gh/api/repos/yt-dlp/yt-dlp/releases/latest");
            let web = format!("{base}/gh/web");
            let source = crate::Source {
                api: &api,
                web: &web,
                signed: signed_by_test_key,
            };
            crate::install_from(dir, net, progress, floor, &source).await
        })
    }

    /// End to end against a fake release server: yt-dlp installed from a release whose checksum
    /// list is signed (here by a test key), a newer release found by the automatic check while a
    /// download holds yt-dlp (it waits, then is switched to as the download ends), going back, the
    /// automatic check then leaving the version gone back from alone, and Update bringing it back
    /// without downloading it again. A list signed by another key installs nothing.
    #[tokio::test]
    async fn updates_from_a_signed_release_end_to_end() {
        let Some(asset) = crate::ytdlp_asset() else {
            return; // no yt-dlp build for this system
        };
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/helper-update");
        let files = tempfile::tempdir().unwrap();
        let repo = files.path().join("gh/yt-dlp/yt-dlp");
        for tag in ["2026.10.26", "2026.11.13"] {
            std::fs::create_dir_all(repo.join(tag)).unwrap();
            for name in ["SHA2-256SUMS", "SHA2-256SUMS.sig", asset] {
                std::fs::copy(fixtures.join(name), repo.join(tag).join(name)).unwrap();
            }
        }
        let latest = |tag: &str| std::fs::write(repo.join("LATEST"), format!("{tag}\n")).unwrap();
        latest("2026.10.26");
        let lab = mdm_labserver::LabServer::start(mdm_labserver::Config {
            files: Some(files.path().to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
        LAB.set(lab.url("")).unwrap();
        let net = crate::net(mdm_engine::Proxy::None);
        let tools = tempfile::tempdir().unwrap();
        let d = tools.path();
        let root = Tool::YtDlp.root(d);

        // Signed by another key (yt-dlp's real one doesn't sign this list): nothing installed.
        let base = LAB.get().unwrap();
        let (api, web) = (
            format!("{base}/gh/api/repos/yt-dlp/yt-dlp/releases/latest"),
            format!("{base}/gh/web"),
        );
        let forged = crate::Source {
            api: &api,
            web: &web,
            signed: crate::signed_by_ytdlp,
        };
        let refused = crate::install_from(d, &net, None, None, &forged).await;
        assert!(matches!(refused, Err(Error::BadSignature)), "{refused:?}");
        assert_eq!(current_in(&root), None);

        // The first install.
        let first = from_lab(d, &net, None, None).await.unwrap();
        let Some(Settled::Switched(old)) = first.settled else {
            panic!("{first:?}")
        };
        assert_eq!(old.version, "2026.10.26");
        assert_eq!(own(d, Tool::YtDlp), Some(old.clone()));

        // A newer release while a download runs yt-dlp: downloaded, verified, waiting.
        latest("2026.11.13");
        let job = Hold::of([old.exe.as_path()]);
        let t0 = 2_000_000_000;
        let found = check_with(d, Tool::YtDlp, &net, Why::Auto, t0, from_lab)
            .await
            .unwrap();
        let Checked::Ready { now, ready: new } = found else {
            panic!("{found:?}")
        };
        assert_eq!(
            (now.version.as_str(), new.version.as_str()),
            ("2026.10.26", "2026.11.13")
        );
        assert_eq!(current_in(&root), Some(old.clone()), "not while it runs");
        assert_eq!(ready(d, Tool::YtDlp), Some(new.clone()));
        // Checked again before its time: nothing asked.
        assert_eq!(
            check_with(d, Tool::YtDlp, &net, Why::Auto, t0 + 60, from_lab)
                .await
                .unwrap(),
            Checked::NotDue
        );
        // The download ends: switched to.
        assert_eq!(job.release(), vec![new.clone()]);
        assert_eq!(own(d, Tool::YtDlp), Some(new.clone()));
        assert_eq!(previous_in(&root), Some(old.clone()));
        let s = read(d).of(Tool::YtDlp);
        assert_eq!(s.latest.as_deref(), Some("2026.11.13"));
        assert_eq!(
            s.updated.map(|u| (u.from, u.to)),
            Some(("2026.10.26".into(), "2026.11.13".into()))
        );

        // Go back: the older one in use, the newer kept; the automatic check leaves it so.
        assert_eq!(
            go_back(d, Tool::YtDlp).unwrap(),
            Settled::Switched(old.clone())
        );
        let later = t0 + Tool::YtDlp.every();
        let again = check_with(d, Tool::YtDlp, &net, Why::Auto, later, from_lab)
            .await
            .unwrap();
        assert!(matches!(again, Checked::Skipped { .. }), "{again:?}");
        assert_eq!(own(d, Tool::YtDlp), Some(old.clone()));
        // Asked for: the newest again, switched to from where it was kept, not downloaded.
        let before = lab.log().len();
        let asked = check_with(d, Tool::YtDlp, &net, Why::Asked, later + 1, from_lab)
            .await
            .unwrap();
        assert!(
            matches!(&asked, Checked::Updated { from, now } if from == "2026.10.26" && now.version == "2026.11.13"),
            "{asked:?}"
        );
        let downloads = lab.log()[before..]
            .iter()
            .filter(|e| e.path.ends_with(asset))
            .count();
        assert_eq!(downloads, 0, "kept, not downloaded again");
        assert_eq!(read(d).of(Tool::YtDlp).skip, None);
    }

    #[test]
    fn versions_never_go_down() {
        assert!(newer("2026.11.13", "2026.10.26"));
        assert!(newer("2026.10.26.1", "2026.10.26"));
        assert!(!newer("2026.10.26", "2026.10.26"));
        assert!(!newer("2026.9.30", "2026.10.26"), "older");
        assert!(newer("1.31.0", "1.30.10"));
        assert!(newer("2.6.0", "2.5.10"));
        assert!(newer("9.0.20261120", "9.0.20261113"));
        assert!(!newer("8.1.20261120", "9.0.20261113"), "an older line");
    }

    /// The words of failures as yt-dlp and gallery-dl write them (their sources, and runs saved in
    /// tests/fixtures/ytdlp-refusals): which mean "look for a newer version".
    #[test]
    fn failures_that_mean_out_of_date() {
        let fixture = |name: &str| {
            std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/ytdlp-refusals")
                    .join(name),
            )
            .unwrap()
        };
        let yes = [
            "yt-dlp failed: [youtube] jNQXAC9IVRw: Unable to extract yt initial data; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U".to_owned(),
            "yt-dlp failed: [vimeo] 123: Unable to extract info section; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U".to_owned(),
            "yt-dlp failed: [youtube] x: Failed to extract any player response; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U".to_owned(),
            "yt-dlp failed: Unsupported URL: https://www.example-video-site.com/watch/123".to_owned(),
            "yt-dlp failed: [youtube] jNQXAC9IVRw: Requested format is not available. Use --list-formats for a list of available formats".to_owned(),
            "yt-dlp failed: [youtube] jNQXAC9IVRw: n challenge solving failed: Some formats may be missing. Ensure you have a supported JavaScript runtime and challenge solver script distribution installed.".to_owned(),
            "yt-dlp failed: [youtube] jNQXAC9IVRw: Signature solving failed: Some formats may be missing.".to_owned(),
            "yt-dlp failed: No video formats found!".to_owned(),
            fixture("bot-check.stderr"),
            fixture("api-403.stderr"),
            fixture("stream-403.stderr"),
            fixture("outdated-client.stderr"),
            "gallery-dl failed: [twitter][error] An unexpected error occurred: KeyError - 'legacy'. Please run gallery-dl again with the --verbose flag, copy its output and report this issue on https://github.com/mikf/gallery-dl/issues .".to_owned(),
            "gallery-dl failed: [deviantart][error] Failed to parse JSON data:  JSONDecodeError: Expecting value: line 1 column 1 (char 0)".to_owned(),
            "gallery-dl failed: [gallery-dl][error] Unsupported URL 'https://example.org/gallery/1'".to_owned(),
        ];
        for m in &yes {
            let tool = if m.starts_with("gallery-dl") {
                Tool::GalleryDl
            } else {
                Tool::YtDlp
            };
            assert!(outdated(tool, m), "{m}");
        }
        let no = [
            fixture("private.stderr"),
            fixture("age.stderr"),
            fixture("members.stderr"),
            "yt-dlp failed: [youtube] x: Unable to download API page: The read operation timed out (caused by TransportError('The read operation timed out')); please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U".to_owned(),
            "yt-dlp failed: [youtube] X6ek6j1gTQA: Unable to download API page: [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1085); please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U".to_owned(),
            "yt-dlp failed: unable to download video data: HTTP Error 503: Service Unavailable".to_owned(),
            "yt-dlp failed: [generic] x: This video is not available from your location due to geo restriction".to_owned(),
            "yt-dlp failed: [Errno 28] No space left on device".to_owned(),
            "gallery-dl failed: [pixiv][error] AuthRequired: 'refresh-token' needed to access 'ugoira'".to_owned(),
            "gallery-dl failed: [flickr][error] NotFoundError: Requested photo could not be found".to_owned(),
        ];
        for m in &no {
            assert!(
                !outdated(Tool::YtDlp, m) && !outdated(Tool::GalleryDl, m),
                "{m}"
            );
        }
        assert!(!outdated(Tool::Ffmpeg, &yes[0]));
    }
}
