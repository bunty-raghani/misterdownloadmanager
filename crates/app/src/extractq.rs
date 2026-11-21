//! Extracting archives when their download completes (#37): a Link grabber package's that says
//! so, or every archive with Options → Archives → "Extract archives when their download
//! completes"; and right-click → Extract on a finished one. A split archive waits for its last
//! volume. One archive at a time, on a thread of its own (mdm-extract does the work, into a
//! staging folder, so a failure leaves only the archive); each extracted file is marked as
//! downloaded from the archive's site, as the archive is. Afterwards the archive goes to the
//! Recycle Bin (the Trash) only when its package or Options says so.
//!
//! Every archive is tried without a password first. Only one that turns out to need one is
//! tried with its package's password, then with each of Options → Archives → Archive passwords
//! in turn, and only then is its password asked. A password that opened it moves to the top of
//! that list, as does one typed with "Remember it" (unticked at first). 7-Zip gets each one on
//! its standard input, never on its command line (mdm-extract).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mdm_extract::{Error, Report, volume_key};

use egui::vec2;

use crate::dialogs::{Outcome, window};
use crate::model::{Item, Status};
use crate::settings::Settings;
use crate::winui::{self, ButtonKind, LIGHT_BG, LIGHT_TEXT, r};

/// One archive to extract: its downloads (a split one's every volume) and where it goes.
#[derive(Clone)]
struct Job {
    /// The downloads of its volumes, the first volume's first (its log tells the outcome).
    ids: Vec<u64>,
    /// The first volume (what is extracted).
    archive: PathBuf,
    /// Every volume's file (what goes to the Recycle Bin afterwards).
    files: Vec<PathBuf>,
    /// The password being tried: none at first, so an archive that needs none never meets one.
    password: Option<String>,
    /// The passwords still to try, in order, once it turns out to need one: the package's
    /// (typed in Link grabber or sent with the links), then Options'.
    vault: Vec<String>,
    /// The package's password, if it has one.
    package: Option<String>,
    /// The package's password is one of Options' archive passwords already.
    package_saved: bool,
    /// The password being tried came from `vault`.
    from_vault: bool,
    /// The log said "Trying the saved archive passwords...".
    noted: bool,
    /// Keep the password in Options' list once it opens the archive: one that is in the list
    /// already (it moves to the top), or one typed in the password window with "Remember it"
    /// ticked. Never a package's own that isn't in the list: nobody chose to save it (a web
    /// page may have sent it).
    keep: bool,
    delete: bool,
    /// Options → Archives: in place with HatchZip, when it is installed.
    in_place: bool,
    /// Right-click → Extract (said again, a missing 7-Zip, each time).
    by_hand: bool,
    subfolder: mdm_extract::Subfolder,
    /// Where the archive came from, for the mark on what it holds.
    url: url::Url,
    referer: Option<String>,
}

/// An archive's outcome, from its thread: the report, and with "delete the archive" whether it
/// went to the Trash (moved there on that thread too: `gio trash`, `trash`, the Finder, which
/// may ask first, are programs the window's thread doesn't wait for, #222).
type Slot = Arc<Mutex<Option<(Result<Report, Error>, Option<bool>)>>>;

struct Running {
    job: Job,
    result: Slot,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Extracts {
    queue: VecDeque<Job>,
    running: Option<Running>,
    /// First volumes queued, running or extracted this session: a set is extracted once (a
    /// failure lets it be tried again, by another volume arriving or by Extract).
    seen: HashSet<PathBuf>,
    /// An archive that needs its password (or another one): asked in a small window.
    pub ask: Option<PasswordDialog>,
    /// A 7z or RAR archive and no 7-Zip: said in a small window (once a session when it was
    /// extracted by itself; each time Extract is chosen).
    pub needs_7zip: Option<ToolDialog>,
    told_7zip: bool,
    /// Passwords to put first in Options' archive passwords, in the order they opened an
    /// archive (none typed without "Remember it"). The app takes them (`take_vault`).
    to_vault: Vec<String>,
}

/// Is this file name an archive's (or one volume of one)? By name only: the first bytes decide
/// once it is extracted.
pub fn archive_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    volume_key(&lower).is_some()
        || [
            ".zip", ".7z", ".rar", ".tar", ".tgz", ".tar.gz", ".txz", ".tar.xz", ".gz", ".xz",
        ]
        .iter()
        .any(|e| lower.ends_with(e) && lower.len() > e.len())
}

/// The volumes of the archive `item` is part of, among `items`: the downloads in its folder (and
/// its package, if it has one) whose names are volumes of the same set, in volume order. One
/// item for an archive in one file.
fn volume_set<'a>(item: &'a Item, items: &'a [Item]) -> Vec<(u32, &'a Item)> {
    let Some(v) = volume_key(&item.file_name) else {
        return vec![(1, item)];
    };
    let dir = item.dest.parent();
    let pkg = item.package.as_ref().map(|p| p.id);
    let mut set: Vec<(u32, &Item)> = items
        .iter()
        .filter(|i| i.dest.parent() == dir && i.package.as_ref().map(|p| p.id) == pkg)
        .filter_map(|i| {
            let w = volume_key(&i.file_name)?;
            (w.kind == v.kind && w.base.to_lowercase() == v.base.to_lowercase())
                .then_some((w.index, i))
        })
        .collect();
    set.sort_by_key(|(n, _)| *n);
    set.dedup_by_key(|(n, _)| *n);
    set
}

impl Extracts {
    /// Download `id` just finished: queue its archive when it is to be extracted and its last
    /// volume is there.
    pub fn finished(&mut self, id: u64, items: &[Item], settings: &Settings) {
        let Some(item) = items.iter().find(|i| i.id == id) else {
            return;
        };
        let wanted = match &item.package {
            Some(p) => p.extract,
            None => settings.extract_auto,
        };
        if wanted {
            self.offer(item, items, settings, false);
        }
    }

    /// Right-click → Extract (or a download finishing): queue `item`'s archive. Extract
    /// (`by_hand`) tries again an archive already extracted.
    pub fn offer(&mut self, item: &Item, items: &[Item], settings: &Settings, by_hand: bool) {
        if item.status != Status::Complete
            || item.torrent.is_some()
            || item.video.is_some()
            || !archive_name(&item.file_name)
        {
            return;
        }
        let set = volume_set(item, items);
        // Every volume downloaded, numbered from 1 with none missing in between (a later one
        // may still be to come: the extractor says so then).
        if set.iter().any(|(_, i)| i.status != Status::Complete)
            || set.iter().enumerate().any(|(k, (n, _))| *n != k as u32 + 1)
        {
            return;
        }
        let (_, first) = set[0];
        let archive = first.dest.clone();
        let waiting = self.queue.iter().any(|j| j.archive == archive)
            || self
                .running
                .as_ref()
                .is_some_and(|r| r.job.archive == archive);
        if by_hand && !waiting {
            self.seen.remove(&archive);
        }
        if !self.seen.insert(archive.clone()) {
            return;
        }
        let pkg = item.package.as_ref();
        let package = pkg
            .and_then(|p| p.password.clone())
            .filter(|p| !p.is_empty());
        let saved = settings.archive_password_list();
        let package_saved = package.as_ref().is_some_and(|p| saved.contains(p));
        let vault = package
            .iter()
            .cloned()
            .chain(saved.into_iter().filter(|p| Some(p) != package.as_ref()))
            .collect();
        self.queue.push_back(Job {
            ids: set.iter().map(|(_, i)| i.id).collect(),
            files: set.iter().map(|(_, i)| i.dest.clone()).collect(),
            archive,
            password: None,
            vault,
            package,
            package_saved,
            from_vault: false,
            noted: false,
            keep: false,
            delete: pkg.map_or(settings.extract_delete, |p| p.delete_after),
            in_place: settings.extract_in_place,
            by_hand,
            subfolder: settings.extract_into(),
            url: first.url.clone(),
            referer: first.context.referer.clone(),
        });
    }

    /// Is download `id`'s archive (one of its volumes) being extracted now?
    pub fn extracting(&self, id: u64) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.job.ids.contains(&id))
    }

    /// Stop extracting the archive of any of `ids` (they are being deleted), and forget the
    /// ones queued.
    pub fn forget(&mut self, ids: &[u64]) {
        if let Some(r) = &self.running
            && r.job.ids.iter().any(|i| ids.contains(i))
        {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.queue
            .retain(|j| !j.ids.iter().any(|i| ids.contains(i)));
    }

    /// Closing: the archive being extracted stops (its staging folder goes).
    pub fn stop(&mut self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.queue.clear();
    }

    /// An archive done: its outcome into the log; the next one started. True when an item
    /// changed (the list is saved).
    pub fn poll(&mut self, items: &mut [Item], ctx: &egui::Context) -> bool {
        let mut changed = false;
        let done = self
            .running
            .as_ref()
            .and_then(|r| r.result.lock().unwrap().take());
        if let Some((result, trashed)) = done
            && let Some(run) = self.running.take()
        {
            self.finish(run.job, result, trashed, items);
            changed = true;
        }
        if self.running.is_none()
            && let Some(job) = self.queue.pop_front()
        {
            if let Some(i) = items.iter_mut().find(|i| i.id == job.ids[0]) {
                i.note(crate::i18n::t("Extracting the archive..."));
            }
            self.running = Some(start(job, ctx));
            changed = true;
        }
        changed
    }

    fn finish(
        &mut self,
        job: Job,
        result: Result<Report, Error>,
        trashed: Option<bool>,
        items: &mut [Item],
    ) {
        let name = job
            .archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let note = |items: &mut [Item], text: String| {
            if let Some(i) = items.iter_mut().find(|i| i.id == job.ids[0]) {
                i.note(text);
            }
        };
        let report = match result {
            Ok(r) => r,
            Err(Error::NeedsPassword | Error::WrongPassword) if !job.vault.is_empty() => {
                // It needs a password: the package's, then each of Options' archive passwords,
                // before asking; the log says so once, at the first saved one.
                let saved = Some(&job.vault[0]) != job.package.as_ref();
                if saved && !job.noted {
                    // i18n: in a download's log: an encrypted archive is being tried with the passwords saved in Options → Archives
                    note(
                        items,
                        crate::i18n::t("Trying the saved archive passwords...").to_owned(),
                    );
                }
                let mut job = job;
                job.noted |= saved;
                job.password = Some(job.vault.remove(0));
                job.from_vault = true;
                job.keep = saved || job.package_saved;
                self.queue.push_front(job);
                return;
            }
            Err(e @ (Error::NeedsPassword | Error::WrongPassword)) => {
                self.seen.remove(&job.archive);
                note(items, failed(&name, &e));
                // Asked once at a time; a second archive waiting for its password is told in its
                // log, to try again with Extract. None of the saved ones fit: "it needs a
                // password", the last one tried never shown.
                if self.ask.is_none() {
                    let wrong = matches!(e, Error::WrongPassword) && !job.from_vault;
                    self.ask = Some(PasswordDialog::new(job, wrong));
                }
                return;
            }
            Err(e) => {
                self.seen.remove(&job.archive);
                note(items, failed(&name, &e));
                if matches!(e, Error::NoTool) && (job.by_hand || !self.told_7zip) {
                    self.told_7zip = true;
                    self.needs_7zip = Some(ToolDialog { name });
                }
                return;
            }
        };
        if job.keep
            && let Some(pw) = &job.password
        {
            self.to_vault.push(pw.clone());
        }
        let folder = crate::i18n::ltr(&report.folder.display().to_string());
        note(
            items,
            crate::i18n::fmt(
                // i18n: in a download's log: {n} is a number of files, {folder} where they went
                &crate::i18n::tn(
                    report.files,
                    "Extracted {n} file to {folder}",
                    "Extracted {n} files to {folder}",
                ),
                &[("folder", &folder)],
            ),
        );
        if !report.skipped.is_empty() {
            note(
                items,
                // i18n: in a download's log, after extracting: entries of the archive not written (links, names that would leave the folder)
                crate::i18n::tn(
                    report.skipped.len() as u64,
                    "{n} entry was left out (a link, or a name leading outside the folder)",
                    "{n} entries were left out (links, or names leading outside the folder)",
                ),
            );
            for (entry, why) in &report.skipped {
                tracing::info!("extract: left out {entry:?}: {why}");
            }
        }
        if report.renamed > 0 {
            note(
                items,
                // i18n: in a download's log, after extracting
                crate::i18n::tn(
                    report.renamed,
                    "{n} file had the name of a file there already and was numbered",
                    "{n} files had the name of a file there already and were numbered",
                ),
            );
        }
        if report.in_place {
            note(
                items,
                // i18n: in a download's log: HatchZip (a program's name) extracted the archive in place, removing it as it went
                crate::i18n::t(
                    "HatchZip extracted it in place: the archive was removed as its files came out",
                )
                .to_owned(),
            );
        }
        if let Some(left) = &report.left {
            note(
                items,
                crate::i18n::fmt(
                    // i18n: in a download's log, after HatchZip (a program's name) extracted in place: the entries left out (links, unsafe names) were not deleted, they are still in {folder}
                    crate::i18n::t("The entries left out are still in {folder}"),
                    &[("folder", &crate::i18n::ltr(&left.display().to_string()))],
                ),
            );
        }
        if let Some(gone) = trashed {
            let text = match (gone, cfg!(windows)) {
                // i18n: in a download's log: the downloaded archive, after it was extracted
                (true, true) => crate::i18n::t("The archive was moved to the Recycle Bin"),
                // i18n: in a download's log (Linux and macOS): the downloaded archive, after it was extracted
                (true, false) => crate::i18n::t("The archive was moved to the Trash"),
                // i18n: in a download's log: the downloaded archive, after it was extracted
                (false, true) => {
                    crate::i18n::t("The archive could not be moved to the Recycle Bin: it was kept")
                }
                // i18n: in a download's log (Linux and macOS): the downloaded archive, after it was extracted
                (false, false) => {
                    crate::i18n::t("The archive could not be moved to the Trash: it was kept")
                }
            };
            note(items, text.to_owned());
        }
    }

    /// The password window's answer: extract again with it (kept in Options' list when asked
    /// to, once it fits), or leave the archive.
    pub fn answer(&mut self, answer: Option<(String, bool)>) {
        let Some(ask) = self.ask.take() else { return };
        if let Some((pw, remember)) = answer {
            let mut job = ask.job;
            job.password = Some(pw);
            job.vault.clear();
            job.from_vault = false;
            job.keep = remember;
            if self.seen.insert(job.archive.clone()) {
                self.queue.push_front(job);
            }
        }
    }

    /// The passwords for the top of Options' archive passwords, each once.
    pub fn take_vault(&mut self) -> Vec<String> {
        std::mem::take(&mut self.to_vault)
    }
}

/// Tests only: the 7-Zip program to use instead of the one installed; empty or `none`: as if
/// 7-Zip weren't installed (the "needs 7-Zip" message). Never set for users.
const TEST_7ZIP: &str = "MDM_TEST_7ZIP";

/// The 7-Zip program chosen in Options → Archives (`Settings::seven_zip_path`, #1525; empty:
/// none), kept here for the extractions, which run beside the window.
static CHOSEN_7ZIP: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// Keep the chosen 7-Zip program as the settings have it (written only when it changes).
pub fn share_chosen_7zip(path: &str) {
    if *CHOSEN_7ZIP.read().unwrap() != path {
        *CHOSEN_7ZIP.write().unwrap() = path.to_owned();
    }
}

/// 7-Zip's program, which extracts 7z and RAR archives: the one chosen in Options → Archives,
/// else the installed one (Options → Archives says which), or a test's `MDM_TEST_7ZIP`.
pub fn seven_zip() -> Option<PathBuf> {
    let chosen = CHOSEN_7ZIP.read().unwrap().clone();
    seven_zip_for(&chosen)
}

/// [`seven_zip`] with `chosen` as the chosen program (Options' draft): it while it is there,
/// else the one found, so a chosen program removed since doesn't leave archives unextracted.
pub fn seven_zip_for(chosen: &str) -> Option<PathBuf> {
    match std::env::var_os(TEST_7ZIP) {
        Some(v) if v.is_empty() || v.eq_ignore_ascii_case("none") => None,
        Some(v) => Some(PathBuf::from(v)),
        None => Some(PathBuf::from(chosen))
            .filter(|p| !chosen.is_empty() && p.is_file())
            .or_else(mdm_extract::find_7zip),
    }
}

/// Whether `exe` answers as 7-Zip's command line (#1525: a program chosen in Options →
/// Archives): run with nothing to do, it lists its commands, the one MDM uses among them. An
/// archiver's window (`peazip.exe`, `7zFM.exe`) doesn't, and is closed after 5 s. Never on the
/// window's thread.
pub fn is_7zip(exe: &Path) -> bool {
    let run = std::process::Command::new(exe);
    mdm_tools::proc::output(run, "7-Zip", std::time::Duration::from_secs(5))
        .is_ok_and(|out| lists_extract(&String::from_utf8_lossy(&out)))
}

/// 7-Zip's list of commands has the one MDM extracts with, as 7z, 7za, 7zz and NanaZipC print
/// it.
fn lists_extract(usage: &str) -> bool {
    usage.contains("x : eXtract files with full paths")
}

/// Tests only: the HatchZip program to use instead of the one installed; empty or `none`: as
/// if it weren't installed. Never set for users.
const TEST_HATCHZIP: &str = "MDM_TEST_HATCHZIP";

/// HatchZip's command line program (hatchzip.dev), optional: the installed one, or a test's
/// `MDM_TEST_HATCHZIP`.
pub fn hatch_zip() -> Option<PathBuf> {
    match std::env::var_os(TEST_HATCHZIP) {
        Some(v) if v.is_empty() || v.eq_ignore_ascii_case("none") => None,
        Some(v) => Some(PathBuf::from(v)),
        None => mdm_extract::find_hatchzip(),
    }
}

/// Tests only: archives "moved to the Recycle Bin" go into this folder instead, so a test never
/// fills the real (per-user, shared) Recycle Bin or Trash. Never set for users.
const TEST_TRASH: &str = "MDM_TEST_TRASH_DIR";

/// Move `files` to the Recycle Bin (the Trash), or into `MDM_TEST_TRASH_DIR` when a test set it.
fn to_trash(files: &[PathBuf]) -> bool {
    if let Some(dir) = std::env::var_os(TEST_TRASH).filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        return std::fs::create_dir_all(&dir).is_ok()
            && files.iter().all(|f| {
                let name = f.file_name().unwrap_or_default();
                std::fs::rename(f, dir.join(name)).is_ok()
            });
    }
    #[cfg(windows)]
    return crate::win32::to_recycle_bin(files);
    #[cfg(unix)]
    return crate::unix::to_trash(files);
}

/// Why `name` wasn't extracted, for its download's log.
fn failed(name: &str, e: &Error) -> String {
    let why = match e {
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::NeedsPassword => crate::i18n::t("it needs a password").to_owned(),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::WrongPassword => crate::i18n::t("the password is wrong").to_owned(),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::MissingVolume(0) => crate::i18n::t("a volume of it is missing").to_owned(),
        Error::MissingVolume(n) => crate::i18n::fmt(
            // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: "); {n} is the volume's number
            crate::i18n::t("volume {n} of it is missing"),
            &[("n", &crate::i18n::integer(u64::from(*n)))],
        ),
        Error::Damaged(why) => {
            tracing::info!("extract: {name}: {why}");
            // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
            crate::i18n::t("it is damaged, or not the archive its name says").to_owned()
        }
        Error::TooBig { would, limit } => crate::i18n::fmt(
            // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: "); sizes like "4.00 GB"
            crate::i18n::t(
                "it would write {size}, more than the {limit} there is room or reason for",
            ),
            &[
                ("size", &crate::model::size(*would)),
                ("limit", &crate::model::size(*limit)),
            ],
        ),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::TooMany => crate::i18n::t("it holds too many files or folders").to_owned(),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: "); 7-Zip is a program's name
        Error::NoTool => {
            crate::i18n::t("7z and RAR archives need 7-Zip, which isn't installed").to_owned()
        }
        Error::NoRoom(_) => {
            // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: "); HatchZip is a program's name
            crate::i18n::t(
                "there isn't enough free space for its files, HatchZip says; nothing was changed",
            )
            .to_owned()
        }
        Error::Partial(folder) => crate::i18n::fmt(
            // i18n: why an archive wasn't extracted in place (after "Couldn't extract {name}: "); {folder} holds the files extracted so far
            crate::i18n::t(
                "it stopped partway. The files extracted so far are in {folder}, the rest is still in the archive: Extract continues",
            ),
            &[("folder", &crate::i18n::ltr(&folder.display().to_string()))],
        ),
        Error::Unplaced { to, left, why } => crate::i18n::fmt(
            // i18n: why an archive wasn't extracted in place (after "Couldn't extract {name}: "); HatchZip took its files out (the archive is gone), but moving them stopped: {to} holds those moved, {left} the others, {error} the system's reason
            crate::i18n::t(
                "its files came out, but moving them stopped ({error}). Those moved are in {to}, the others still in {left}; nothing was deleted",
            ),
            &[
                ("error", why.as_str()),
                ("to", &crate::i18n::ltr(&to.display().to_string())),
                ("left", &crate::i18n::ltr(&left.display().to_string())),
            ],
        ),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::Unsupported => crate::i18n::t("it is not an archive MDM extracts").to_owned(),
        // i18n: why an archive wasn't extracted (after "Couldn't extract {name}: ")
        Error::Cancelled => crate::i18n::t("it was stopped").to_owned(),
        Error::Io(e) => crate::errors::describe(&mdm_engine::Error::Io(Arc::new(
            std::io::Error::new(e.kind(), e.to_string()),
        ))),
    };
    crate::i18n::fmt(
        // i18n: in a download's log: {name} is the archive's file name, {why} the reason
        crate::i18n::t("Couldn't extract {name}: {why}"),
        &[("name", &crate::i18n::isolate(name)), ("why", &why)],
    )
}

fn start(job: Job, ctx: &egui::Context) -> Running {
    let result: Slot = Arc::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let (slot, stop, ctx) = (result.clone(), cancel.clone(), ctx.clone());
    let j = job.clone();
    let spawned = std::thread::Builder::new()
        .name("extract".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&j, &stop)))
                .unwrap_or_else(|_| Err(Error::Damaged("the decoder failed".into())));
            // Extracted: the archive (and its volumes) to the Trash, when asked.
            let in_place = r.as_ref().is_ok_and(|r| r.in_place);
            let trashed = (r.is_ok() && j.delete && !in_place).then(|| {
                let present: Vec<PathBuf> =
                    j.files.iter().filter(|f| f.exists()).cloned().collect();
                to_trash(&present)
            });
            *slot.lock().unwrap() = Some((r, trashed));
            ctx.request_repaint();
        });
    if let Err(e) = spawned {
        *result.lock().unwrap() = Some((Err(Error::Io(e)), None));
    }
    Running {
        job,
        result,
        cancel,
    }
}

fn run(job: &Job, cancel: &AtomicBool) -> Result<Report, Error> {
    let dest = job
        .archive
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let size: u64 = job
        .files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();
    let mut limits = mdm_extract::Limits::for_archive(size);
    // No more than the disk has room for (keeping the engine's margin free).
    if let Some(v) = mdm_engine::disk::volume_of(&dest) {
        limits.total = limits
            .total
            .min(v.free.saturating_sub(mdm_engine::disk::MARGIN));
    }
    let hook = crate::app::mark_hook(&job.url, job.referer.as_deref());
    let mut on_file = |p: &Path| (hook.0)(p);
    let mut policy = mdm_extract::Policy {
        dest,
        subfolder: job.subfolder,
        password: job.password.clone(),
        limits,
        tool: seven_zip(),
        hatchzip: hatch_zip(),
        in_place: job.in_place,
        runs: mdm_ipc::data_dir().map(|d| d.join("hatchzip-runs")),
        on_file: &mut on_file,
        cancel: Some(cancel),
    };
    mdm_extract::extract(&job.archive, &mut policy)
}

/// "7-Zip needed": a 7z or RAR archive, and 7-Zip isn't installed. Get 7-Zip opens its site.
pub struct ToolDialog {
    name: String,
}

impl ToolDialog {
    /// `Done(())` or Closed: the window goes.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        window(
            ctx,
            "needs-7zip",
            // i18n: window title: 7-Zip (a program's name) must be installed
            "7-Zip needed",
            [400.0, 118.0],
            false,
            LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let line = crate::i18n::fmt(
                    // i18n: in the "7-Zip needed" window: {name} is the archive's file name; 7-Zip is a program's name
                    crate::i18n::t(
                        "{name} is a 7z or RAR archive. MDM extracts those with 7-Zip, which isn't installed.",
                    ),
                    &[("name", &crate::i18n::isolate(&self.name))],
                );
                winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    &line,
                    LIGHT_TEXT(),
                    o.x + 389.0,
                    14.0,
                );
                winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 56.0,
                    if cfg!(target_os = "linux") {
                        // i18n: in the "7-Zip needed" window, under why (Linux: 7-Zip comes from the distribution's packages, named 7zip or p7zip)
                        crate::i18n::t(
                            "Install your system's 7zip (or p7zip) package, then right-click the download and choose Extract.",
                        )
                    } else {
                        // i18n: in the "7-Zip needed" window, under why
                        crate::i18n::t(
                            "Install 7-Zip (free), then right-click the download and choose Extract.",
                        )
                    },
                    winui::MUTED(),
                    o.x + 389.0,
                    14.0,
                );
                let row = winui::buttons_light_right(
                    ui,
                    o,
                    389.0,
                    86.0,
                    &[
                        // i18n: button: opens 7-Zip's website (7-Zip is a program's name)
                        (crate::i18n::mark("Get 7-Zip"), ButtonKind::Normal),
                        (crate::i18n::mark("OK"), ButtonKind::Default),
                    ],
                );
                if row[0].clicked() {
                    crate::launch::open("https://www.7-zip.org/");
                }
                if row[1].clicked() || winui::escape_pressed(ui) || winui::enter_pressed(ui) {
                    Outcome::Done(())
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// "Password for an archive": asked when one is encrypted (again when the password was wrong).
pub struct PasswordDialog {
    job: Job,
    wrong: bool,
    password: String,
    /// "Remember it in Options → Archives": unticked until the user ticks it.
    remember: bool,
    focused: bool,
}

impl PasswordDialog {
    fn new(job: Job, wrong: bool) -> Self {
        let password = if wrong {
            job.password.clone().unwrap_or_default()
        } else {
            String::new()
        };
        PasswordDialog {
            job,
            wrong,
            password,
            // Unticked: a password is kept only when the user chooses to.
            remember: false,
            focused: false,
        }
    }

    /// `Done(Some((password, remember)))`: extract with it; `Done(None)` or Closed: leave the
    /// archive.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Option<(String, bool)>> {
        let name = self
            .job
            .archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        window(
            ctx,
            "archive-password",
            "Archive password",
            [380.0, 136.0],
            false,
            LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let line = crate::i18n::fmt(
                    if self.wrong {
                        // i18n: in Archive password: {name} is the archive's file name
                        crate::i18n::t("Wrong password for {name}. Type it again:")
                    } else {
                        // i18n: in Archive password: {name} is the archive's file name
                        crate::i18n::t("{name} is protected by a password:")
                    },
                    &[("name", &crate::i18n::isolate(&name))],
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    &line,
                    if self.wrong {
                        winui::ERROR()
                    } else {
                        LIGHT_TEXT()
                    },
                    o.x + 369.0,
                    "archive-password-line",
                );
                let edit = winui::password_light(
                    ui,
                    r(o, 11.0, 31.0, 358.0, 23.0),
                    &mut self.password,
                    true,
                );
                // i18n: what a screen reader calls the Archive password window's box
                let label = crate::i18n::t("Archive password");
                ui.ctx().accesskit_node_builder(edit.id, |n| {
                    n.set_label(label);
                    // UI Automation's id (a password box's name is not read out to tools).
                    n.set_author_id("archive-password");
                });
                if !self.focused {
                    edit.request_focus();
                    self.focused = true;
                }
                winui::checkbox_light(
                    ui,
                    o + vec2(11.0, 62.0),
                    &mut self.remember,
                    // i18n: in Archive password: check box: keep the password typed in Options → Archives → Archive passwords, to try it on the next encrypted archives
                    "Remember it for other archives",
                    true,
                );
                let has = !self.password.is_empty();
                // As wide as their labels in this language (#192).
                let [ok_at, skip_at] = winui::buttons_to_right(
                    &ui.painter().clone(),
                    369.0,
                    8.0,
                    &[("Extract", 73.0), ("Cancel", 73.0)],
                    160.0,
                )[..] else {
                    unreachable!("two buttons")
                };
                let ok = winui::button_light(
                    ui,
                    r(o, ok_at.0, 104.0, ok_at.1, 21.0),
                    // i18n: button: extract the archive with the password typed
                    "Extract",
                    if has {
                        ButtonKind::Default
                    } else {
                        ButtonKind::Disabled
                    },
                    false,
                )
                .clicked();
                let skip = winui::button_light(
                    ui,
                    r(o, skip_at.0, 104.0, skip_at.1, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if skip || winui::escape_pressed(ui) {
                    Outcome::Done(None)
                } else if (ok || winui::enter_pressed(ui)) && has {
                    Outcome::Done(Some((std::mem::take(&mut self.password), self.remember)))
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, PackageTag};
    use mdm_extract::VolumeKind;

    /// #1525: a program chosen in Options → Archives is 7-Zip's command line when it lists the
    /// command MDM extracts with, as 7z, 7zz and NanaZipC do; anything else isn't.
    #[test]
    fn a_program_is_7zip_when_it_lists_its_extract_command() {
        let usage = "\n7-Zip 24.08 (x64) : Copyright (c) 1999-2024 Igor Pavlov : 2024-08-11\n\n\
            Usage: 7z <command> [<switches>...] <archive_name> [<file_names>...] [@listfile]\n\n\
            <Commands>\n  a : Add files to archive\n  x : eXtract files with full paths\n";
        assert!(lists_extract(usage));
        assert!(lists_extract(&usage.replace("7-Zip 24.08", "NanaZip 5.0")));
        assert!(!lists_extract(""));
        assert!(!lists_extract("ffmpeg version 8.0 Copyright (c) 2000-2025"));
    }

    /// The 7-Zip found on this computer, when there is one, answers as 7-Zip's command line
    /// (the real program, run as Options → Archives runs a chosen one).
    #[test]
    fn the_7zip_found_here_answers_as_7zip() {
        if let Some(exe) = mdm_extract::find_7zip() {
            assert!(is_7zip(&exe), "{}", exe.display());
        }
    }

    /// #1525: the chosen program is the one used while its file is there; none chosen, or one
    /// removed since, is the one MDM finds.
    #[test]
    fn the_chosen_7zip_is_used_while_it_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("7z.exe");
        std::fs::write(&own, b"").unwrap();
        assert_eq!(seven_zip_for(&own.display().to_string()), Some(own.clone()));
        let found = mdm_extract::find_7zip();
        assert_eq!(seven_zip_for(""), found);
        std::fs::remove_file(&own).unwrap();
        assert_eq!(seven_zip_for(&own.display().to_string()), found);
    }

    fn item(id: u64, name: &str, status: Status, pkg: Option<u64>) -> Item {
        let mut i = Item::new(
            id,
            format!("https://x.org/{name}").parse().unwrap(),
            PathBuf::from("dl").join(name),
            Category::Compressed,
        );
        i.status = status;
        i.package = pkg.map(|id| PackageTag {
            id,
            name: "p".into(),
            extract: true,
            ..Default::default()
        });
        i
    }

    #[test]
    fn names_of_archives() {
        for n in [
            "a.zip",
            "A.7Z",
            "b.part2.rar",
            "c.r00",
            "d.7z.003",
            "e.tar.gz",
            "f.tgz",
            "g.xz",
        ] {
            assert!(archive_name(n), "{n}");
        }
        for n in ["a.exe", "zip", ".zip", "notes.txt", "movie.mkv"] {
            assert!(!archive_name(n), "{n}");
        }
    }

    #[test]
    fn a_split_archive_waits_for_every_volume() {
        let s = Settings {
            extract_auto: true,
            ..Default::default()
        };
        let mut items = vec![
            item(1, "m.part1.rar", Status::Complete, Some(7)),
            item(2, "m.part2.rar", Status::Downloading, Some(7)),
            item(3, "m.part3.rar", Status::Complete, Some(7)),
            // Another package's volume of the same name is not this set's.
            item(4, "m.part2.rar", Status::Complete, Some(8)),
        ];
        let mut q = Extracts::default();
        q.finished(1, &items, &s);
        q.finished(3, &items, &s);
        assert!(q.queue.is_empty(), "volume 2 is still downloading");
        items[1].status = Status::Complete;
        q.finished(2, &items, &s);
        assert_eq!(q.queue.len(), 1);
        let job = &q.queue[0];
        assert_eq!(job.ids, [1, 2, 3]);
        assert_eq!(job.archive, PathBuf::from("dl").join("m.part1.rar"));
        assert_eq!(job.files.len(), 3);
        // Once only, whichever volume finishes last.
        q.finished(3, &items, &s);
        assert_eq!(q.queue.len(), 1);
        // A gap (volume 2 of another set missing) waits.
        let gap = vec![
            item(5, "g.7z.001", Status::Complete, None),
            item(6, "g.7z.003", Status::Complete, None),
        ];
        q.finished(6, &gap, &s);
        assert_eq!(q.queue.len(), 1);
        assert_eq!(
            volume_key("g.7z.001").map(|v| v.kind),
            Some(VolumeKind::Split)
        );
    }

    #[test]
    fn saved_passwords_are_tried_before_asking_and_the_one_that_fits_goes_first() {
        let settings = Settings {
            archive_passwords: "one\ntwo\npkg".into(),
            ..Default::default()
        };
        let mut items = vec![item(1, "a.7z", Status::Complete, Some(5))];
        items[0].package.as_mut().unwrap().password = Some("pkg".into());
        let mut q = Extracts::default();
        q.finished(1, &items, &settings);
        // Without a password first; then the package's, then the list's, without it twice.
        let job = q.queue.pop_front().unwrap();
        assert_eq!(job.password, None);
        assert_eq!(job.vault, vec!["pkg", "one", "two"]);
        q.finish(job, Err(Error::NeedsPassword), None, &mut items);
        let job = q.queue.pop_front().unwrap();
        assert_eq!(job.password.as_deref(), Some("pkg"));
        let tries = |items: &[Item]| {
            items[0]
                .log
                .iter()
                .filter(|(_, t)| t.contains("saved archive passwords"))
                .count()
        };
        assert_eq!(tries(&items), 0, "the package's isn't a saved one");
        let wrong = || Err(Error::WrongPassword);
        q.finish(job, wrong(), None, &mut items);
        let job = q.queue.pop_front().unwrap();
        assert_eq!(
            (job.password.as_deref(), job.from_vault),
            (Some("one"), true)
        );
        assert!(q.ask.is_none(), "not asked while saved ones are left");
        q.finish(job, wrong(), None, &mut items);
        let job = q.queue.pop_front().unwrap();
        assert_eq!(job.password.as_deref(), Some("two"));
        // The log says it once.
        assert_eq!(tries(&items), 1);
        // "two" fits: it goes first in Options' list.
        q.finish(job.clone(), Ok(Report::default()), None, &mut items);
        assert_eq!(q.take_vault(), vec!["two"]);
        let mut s = settings.clone();
        s.remember_archive_password("two");
        assert_eq!(s.archive_password_list(), vec!["two", "one", "pkg"]);

        // None fits: asked, the last one tried not shown in the box, "Remember it" unticked.
        let mut job = job;
        job.vault.clear();
        q.finish(job, wrong(), None, &mut items);
        let ask = q.ask.as_ref().unwrap();
        assert!(ask.password.is_empty() && !ask.wrong && !ask.remember);
        // Typed, with "Remember it": tried, and kept for the list once it fits.
        q.answer(Some(("typed".into(), true)));
        assert!(
            q.take_vault().is_empty(),
            "not before it opened the archive"
        );
        let job = q.queue.pop_front().unwrap();
        assert_eq!(job.password.as_deref(), Some("typed"));
        assert!(job.vault.is_empty());
        q.finish(job.clone(), Ok(Report::default()), None, &mut items);
        assert_eq!(q.take_vault(), vec!["typed"]);
        // Without "Remember it": used once, never kept.
        let mut job = job;
        q.finish(job.clone(), wrong(), None, &mut items);
        q.answer(Some(("once".into(), false)));
        job = q.queue.pop_front().unwrap();
        q.finish(job, Ok(Report::default()), None, &mut items);
        assert!(q.take_vault().is_empty());
    }

    #[test]
    fn a_package_password_is_never_saved_by_itself() {
        // Sent with the links by a page, or typed in Link grabber: used, not kept, unless it is
        // one of Options' archive passwords already (then it moves to the top).
        let mut items = vec![
            item(1, "a.rar", Status::Complete, Some(5)),
            item(2, "b.rar", Status::Complete, Some(6)),
            item(3, "c.rar", Status::Complete, Some(7)),
        ];
        for (i, pw) in [(0, "from-the-site"), (1, "other-site"), (2, "unused")] {
            items[i].package.as_mut().unwrap().password = Some(pw.into());
        }
        let mut q = Extracts::default();
        let s = Settings {
            archive_passwords: "first\nother-site".into(),
            ..Default::default()
        };
        // a and b need theirs, and both open in the same pass. a's isn't in the list: not
        // kept. b's is: it goes to the top.
        q.finished(1, &items, &s);
        q.finished(2, &items, &s);
        let (a, b) = (q.queue.pop_front().unwrap(), q.queue.pop_front().unwrap());
        q.finish(a, Err(Error::NeedsPassword), None, &mut items);
        let a = q.queue.pop_front().unwrap();
        q.finish(b, Err(Error::NeedsPassword), None, &mut items);
        let b = q.queue.pop_front().unwrap();
        q.finish(a, Ok(Report::default()), None, &mut items);
        q.finish(b, Ok(Report::default()), None, &mut items);
        assert_eq!(q.take_vault(), vec!["other-site"]);
        // c opens without a password: its package's is never tried, nor kept.
        q.finished(3, &items, &s);
        let job = q.queue.pop_front().unwrap();
        assert_eq!(job.password, None);
        q.finish(job, Ok(Report::default()), None, &mut items);
        assert!(q.take_vault().is_empty());
        // One that doesn't fit isn't kept.
        let mut items = vec![item(4, "d.rar", Status::Complete, Some(8))];
        items[0].package.as_mut().unwrap().password = Some("wrong".into());
        q.finished(4, &items, &s);
        let job = q.queue.pop_front().unwrap();
        q.finish(job, Err(Error::NeedsPassword), None, &mut items);
        let job = q.queue.pop_front().unwrap();
        q.finish(job, Err(Error::WrongPassword), None, &mut items);
        assert!(q.take_vault().is_empty());
    }

    #[test]
    fn only_what_is_asked_for_is_extracted() {
        let off = Settings::default();
        let items = vec![
            item(1, "a.zip", Status::Complete, None),
            item(2, "b.zip", Status::Complete, Some(3)),
            item(3, "c.exe", Status::Complete, Some(3)),
        ];
        let mut q = Extracts::default();
        // Off in Options: an archive outside a package stays as it is.
        q.finished(1, &items, &off);
        assert!(q.queue.is_empty());
        // Its package says extract.
        q.finished(2, &items, &off);
        assert_eq!(q.queue.len(), 1);
        // Not an archive.
        q.finished(3, &items, &off);
        assert_eq!(q.queue.len(), 1);
        // By hand, whatever Options says (not twice while it waits).
        q.offer(&items[0], &items, &off, true);
        q.offer(&items[0], &items, &off, true);
        assert_eq!(q.queue.len(), 2);
        assert!(!q.queue[0].delete, "Options' default keeps the archive");
    }
}
