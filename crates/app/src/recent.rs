//! The browser popup's list of downloads (#1496): what MDM answers its native host. Each
//! download as the automation API lists it ([`info`], shared with `query: list`), whether a
//! finished one's file is still where it was saved ([`OnDisk`]), and the key of its file type's
//! icon ([`icon_key`]), sent once per type and size. The app's side (the answer, the buttons) is
//! in app.rs: `recent_answer`, `recent_act`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_ipc::api::DownloadInfo;

use crate::fileicons::{Kind, What};
use crate::model::{Item, Status};

/// One download as the automation API and the browser popup list it. `remote`: for a paired
/// device (#141), which sees no folder of this computer's, nor a password in an address.
/// `exists`: whether a finished one's file is still there, when known ([`OnDisk`]).
pub fn info(i: &Item, remote: bool, exists: Option<bool>) -> DownloadInfo {
    let url = if remote {
        let mut u = i.url.clone();
        let _ = u.set_password(None);
        u.to_string()
    } else {
        i.url.to_string()
    };
    let complete = i.status == Status::Complete;
    DownloadInfo {
        id: i.id,
        file: i.file_name.clone(),
        url,
        path: if remote {
            String::new()
        } else {
            i.dest.to_string_lossy().into_owned()
        },
        status: format!("{:?}", i.status),
        size: i.size,
        downloaded: i.downloaded,
        speed: i.rate() as u64,
        added: i.added.clone(),
        category: i.category.key().into_owned(),
        error: i.error.clone().filter(|_| i.status == Status::Error),
        left: i.seconds_left().map(|s| s.round().max(0.0) as u64),
        exists: exists.filter(|_| complete),
        // Its last event is its completion (or what came after: a rename, a check).
        finished: i.log.last().map(|(at, _)| *at).filter(|_| complete),
        kind: kind(i).map(str::to_owned),
        redownload: i.live.is_none()
            && i.torrent.is_none()
            && i.video.is_none()
            && i.remote.is_none(),
        icon: None,
    }
}

/// What downloads `i` when it isn't a plain file ([`DownloadInfo::kind`]).
fn kind(i: &Item) -> Option<&'static str> {
    if i.torrent.is_some() || i.remote.is_some() {
        Some("torrent")
    } else if i.video.is_some() {
        Some("video")
    } else if i.gallery.is_some() {
        Some("gallery")
    } else {
        None
    }
}

/// The popup's key for the icon of a download's file type: its extension, lowercase (`zip`);
/// `_` for a file without one; `folder` for a gallery's folder and a torrent named without an
/// extension (its folder).
pub fn icon_key(i: &Item) -> String {
    let folder = i.gallery.is_some();
    match What::of(Kind::File(&i.file_name)) {
        _ if folder => "folder".into(),
        What::File(Some(ext)) => ext,
        _ if i.torrent.is_some() || i.remote.is_some() => "folder".into(),
        _ => "_".into(),
    }
}

/// What the system is asked for an icon key ([`icon_key`]).
pub fn icon_what(key: &str) -> What {
    match key {
        "folder" => What::Folder,
        "_" => What::File(None),
        ext => What::File(Some(ext.to_owned())),
    }
}

/// Before opening something at the browser popup's request (a file, its folder, the share
/// sheet, a progress window): pass on the right to come to the front that the browser's native
/// host gave MDM, so it doesn't open behind the browser (Windows; Linux and macOS have no such
/// lock to lift).
pub fn pass_foreground() {
    #[cfg(windows)]
    crate::win32::pass_foreground();
}

/// How long what was seen of a file stands before it is looked at again: the popup asks every
/// second or two while it is open.
const FRESH: Duration = Duration::from_secs(4);
/// Paths kept at most; past it, those not asked about for a minute are forgotten.
const KEPT: usize = 2000;

/// Whether finished downloads' files are still where they were saved, looked at off the
/// window's thread (a sleeping disk or a network folder can take seconds to answer, #751) and
/// kept a few seconds ([`FRESH`]).
#[derive(Clone, Default)]
pub struct OnDisk {
    seen: Arc<Mutex<HashMap<PathBuf, Seen>>>,
}

#[derive(Clone, Copy)]
struct Seen {
    /// None until first looked at.
    there: Option<bool>,
    /// When it was last looked at (None: never).
    at: Option<Instant>,
    /// A look is under way.
    looking: bool,
    /// When it was last asked about.
    asked: Instant,
}

impl OnDisk {
    /// What is known of each of `paths` now (None: not looked at yet). Those not looked at for
    /// [`FRESH`] are looked at again, together, by `look` (on the runtime's blocking threads in
    /// the app: [`OnDisk::there_all`]).
    pub fn known(&self, paths: &[&Path], look: impl FnOnce(Vec<PathBuf>)) -> Vec<Option<bool>> {
        let now = Instant::now();
        let mut stale = Vec::new();
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() > KEPT {
            seen.retain(|_, s| now.duration_since(s.asked) < Duration::from_secs(60));
        }
        let known = paths
            .iter()
            .map(|p| {
                let s = seen.entry(p.to_path_buf()).or_insert(Seen {
                    there: None,
                    at: None,
                    looking: false,
                    asked: now,
                });
                s.asked = now;
                if !s.looking && s.at.is_none_or(|at| now.duration_since(at) >= FRESH) {
                    s.looking = true;
                    stale.push(p.to_path_buf());
                }
                s.there
            })
            .collect();
        drop(seen);
        if !stale.is_empty() {
            look(stale);
        }
        known
    }

    /// [`OnDisk::known`], the stale ones looked at on `rt`'s blocking threads.
    pub fn there_all(&self, paths: &[&Path], rt: &tokio::runtime::Handle) -> Vec<Option<bool>> {
        let me = self.clone();
        self.known(paths, move |stale| {
            rt.spawn_blocking(move || me.look_now(&stale));
        })
    }

    /// Look at `paths` now (waits on the disk) and keep what was seen.
    pub fn look_now(&self, paths: &[PathBuf]) {
        let found: Vec<bool> = paths.iter().map(|p| p.exists()).collect();
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        for (p, there) in paths.iter().zip(found) {
            if let Some(s) = seen.get_mut(p) {
                *s = Seen {
                    there: Some(there),
                    at: Some(now),
                    looking: false,
                    asked: s.asked,
                };
            }
        }
    }

    /// What was last seen of `path` (None: never looked at).
    pub fn last(&self, path: &Path) -> Option<bool> {
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.get(path).and_then(|s| s.there)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item::new(
            7,
            "https://files.example.com/a".parse().unwrap(),
            PathBuf::from(format!("/tmp/{name}")),
            crate::model::Category::General,
        )
    }

    #[test]
    fn icon_keys_go_by_type() {
        assert_eq!(icon_key(&item("Report.PDF")), "pdf");
        assert_eq!(icon_key(&item("README")), "_");
        let mut g = item("cats.zip");
        g.gallery = Some(Default::default());
        assert_eq!(icon_key(&g), "folder");
        assert_eq!(icon_what("folder"), What::Folder);
        assert_eq!(icon_what("_"), What::File(None));
        assert_eq!(icon_what("zip"), What::File(Some("zip".into())));
    }

    /// A finished download as the popup gets it: its state, when it finished, whether its file
    /// is there; the same row for a paired device without its folder or the address's password.
    #[test]
    fn a_download_as_listed() {
        let mut i = item("a.zip");
        i.url = "https://user:secret@files.example.com/a.zip"
            .parse()
            .unwrap();
        i.status = Status::Complete;
        i.log.push((1_800_000_000, "Complete".into()));
        let d = info(&i, false, Some(false));
        assert_eq!(d.status, "Complete");
        assert_eq!((d.exists, d.finished), (Some(false), Some(1_800_000_000)));
        assert!(d.redownload && d.kind.is_none() && d.left.is_none());
        assert!(d.url.contains("secret") && d.path.ends_with("a.zip"));
        let r = info(&i, true, None);
        assert!(!r.url.contains("secret") && r.path.is_empty() && r.exists.is_none());
        // Not finished: neither when, nor whether its file is there.
        i.status = Status::Stopped;
        let d = info(&i, false, Some(true));
        assert_eq!((d.exists, d.finished), (None, None));
    }

    /// Each path is looked at once while fresh, then again; what was seen is kept.
    #[test]
    fn files_are_looked_at_once_while_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path().join("here.bin");
        std::fs::write(&here, b"x").unwrap();
        let gone = dir.path().join("gone.bin");
        let disk = OnDisk::default();
        let mut asked = Vec::new();
        let first = disk.known(&[&here, &gone], |stale| asked.push(stale));
        assert_eq!(first, [None, None]);
        assert_eq!(asked.len(), 1);
        disk.look_now(&asked[0]);
        let mut again = 0;
        let second = disk.known(&[&here, &gone], |_| again += 1);
        assert_eq!(second, [Some(true), Some(false)]);
        assert_eq!(again, 0, "fresh: not looked at again");
        assert_eq!(disk.last(&gone), Some(false));
        // A look under way isn't asked for twice.
        let other = dir.path().join("other.bin");
        let mut looks = 0;
        disk.known(&[&other], |_| looks += 1);
        disk.known(&[&other], |_| looks += 1);
        assert_eq!(looks, 1);
    }
}
