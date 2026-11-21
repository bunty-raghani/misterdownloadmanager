//! The unfinished downloads' resume states, read beside the window at start.
//!
//! The list (downloads.json) holds each row's size and bytes so far as it was last saved; the
//! resume state beside the row's file (`<file>.mdm.json`, the engine's) is the record of what is
//! on the disk, and after a crash the list may hold less. Reading every state before the window
//! opened held the window for as long as the slowest folder took to answer: one on a network
//! share that doesn't answer, 21 s (Windows gives up on it then; found with #1170). Now the window
//! opens with the list as saved, and each row takes its state when it comes, on a later frame.
//!
//! The rows are read one at a time on a thread per place (a drive or a network share; on Linux
//! and macOS a folder near the root, where mounts are): a place that doesn't answer holds only
//! its own rows, never the window or the rows elsewhere. A row started before its state comes
//! needs none of this: the engine looks for its state as it starts, on its own task
//! (`Download::resume_or_start`).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use mdm_engine::state::{ResumeState, state_path};

/// What reading one row's state found.
pub struct Read {
    pub id: u64,
    /// Where it was read (the row's place then).
    pub dest: PathBuf,
    /// Its state; None when there is none, or none that reads.
    pub state: Option<ResumeState>,
}

/// How a row's state is read: [`from_disk`], or a test's.
pub type Loader = Arc<dyn Fn(&Path) -> Option<ResumeState> + Send + Sync>;

/// The state at `dest` on the disk; files started under the former codename are renamed first,
/// so that they resume.
pub fn from_disk(dest: &Path) -> Option<ResumeState> {
    mdm_engine::state::migrate_legacy(dest);
    ResumeState::load(&state_path(dest)).ok()
}

/// A read this slow is logged (a folder that doesn't answer, a drive waking up).
const SLOW: std::time::Duration = std::time::Duration::from_secs(1);

/// The most threads reading at once: more places than this share them, in turn.
const THREADS: usize = 8;

/// The reads under way. Never waits: [`StateReads::take`] gives what has come.
#[derive(Default)]
pub struct StateReads {
    results: Option<Receiver<Read>>,
    /// The rows whose read hasn't come, and is still wanted.
    pending: HashSet<u64>,
}

impl StateReads {
    /// Read the states of `rows` (id, place) with `load`, beside the caller; `wake` after each
    /// read (the window's repaint, so that it takes it).
    pub fn start(
        rows: Vec<(u64, PathBuf)>,
        load: Loader,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        if rows.is_empty() {
            return Self::default();
        }
        let wake = Arc::new(wake);
        let (tx, rx) = channel();
        let mut groups: Vec<Vec<(u64, PathBuf)>> = Vec::new();
        let mut places: HashMap<PathBuf, usize> = HashMap::new();
        for (id, dest) in rows {
            let n = places.len();
            let g = *places.entry(place_of(&dest)).or_insert(n) % THREADS;
            if g == groups.len() {
                groups.push(Vec::new());
            }
            groups[g].push((id, dest));
        }
        let mut pending = HashSet::new();
        for group in groups {
            let ids: Vec<u64> = group.iter().map(|(id, _)| *id).collect();
            let (tx, load, wake) = (tx.clone(), load.clone(), wake.clone());
            let spawned = std::thread::Builder::new()
                .name("resume states".into())
                .spawn(move || {
                    for (id, dest) in group {
                        let at = std::time::Instant::now();
                        let state = load(&dest);
                        let took = at.elapsed();
                        if took >= SLOW {
                            tracing::info!(
                                "resume states: download {id}'s took {} ms, beside the window",
                                took.as_millis()
                            );
                        }
                        if tx.send(Read { id, dest, state }).is_err() {
                            return;
                        }
                        wake();
                    }
                });
            match spawned {
                Ok(_) => pending.extend(ids),
                // Those rows show what the list saved; each one's start reads its own state.
                Err(e) => tracing::warn!("resume states: no thread to read them ({e})"),
            }
        }
        StateReads {
            results: Some(rx),
            pending,
        }
    }

    /// Row `id`'s state is still to come.
    #[cfg(test)]
    pub fn pending(&self, id: u64) -> bool {
        self.pending.contains(&id)
    }

    /// Row `id`'s read isn't wanted any more (it starts: the engine reads its state itself): it is
    /// dropped when it comes.
    pub fn forget(&mut self, id: u64) {
        self.pending.remove(&id);
    }

    /// The reads that came since the last call, of rows that still want theirs.
    pub fn take(&mut self) -> Vec<Read> {
        let mut out = Vec::new();
        let Some(rx) = &self.results else {
            return out;
        };
        loop {
            match rx.try_recv() {
                Ok(read) => {
                    if self.pending.remove(&read.id) {
                        out.push(read);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.results = None;
                    break;
                }
            }
        }
        out
    }
}

/// The place a file is on, for the threads: its drive or its network share on Windows (`C:`,
/// `\\server\share`); elsewhere its folder's first two names under the root (`/mnt/nas`,
/// `/media/me`, `/home/me`), where mounts usually are.
fn place_of(dest: &Path) -> PathBuf {
    let mut place = PathBuf::new();
    let mut names = 0;
    for c in dest.parent().unwrap_or(dest).components() {
        match c {
            Component::Prefix(p) => return PathBuf::from(p.as_os_str()),
            Component::RootDir => place.push(c),
            Component::Normal(_) if names < 2 => {
                place.push(c);
                names += 1;
            }
            _ => break,
        }
    }
    place
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn state(dest: &Path, done: u64) -> ResumeState {
        let url = url::Url::parse("http://127.0.0.1/a.bin").unwrap();
        let mut s = ResumeState::new(url.clone(), url, dest.to_owned(), 100, Default::default());
        s.segments = vec![mdm_engine::planner::SavedSegment {
            start: 0,
            end: 100,
            cursor: done,
        }];
        s
    }

    fn wait_for(reads: &mut StateReads, n: usize) -> Vec<Read> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < n && Instant::now() < until {
            got.extend(reads.take());
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    /// A place that doesn't answer holds only its own rows: `start` returns at once, the other
    /// place's rows come while it still hangs, and its row comes once it answers.
    #[test]
    fn a_place_that_hangs_holds_only_its_own_rows() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let (dead, fine) = if cfg!(windows) {
            (r"\\192.0.2.1\share\x.bin", r"C:\dl\a.bin")
        } else {
            ("/mnt/nas/share/x.bin", "/home/me/a.bin")
        };
        let load: Loader = {
            let gate = gate.clone();
            Arc::new(move |dest: &Path| {
                if dest.to_string_lossy().contains("x.bin") {
                    drop(gate.lock().unwrap());
                }
                Some(state(dest, 40))
            })
        };
        let at = Instant::now();
        let rows = vec![
            (1, PathBuf::from(dead)),
            (2, PathBuf::from(fine)),
            (3, PathBuf::from(fine).with_file_name("b.bin")),
        ];
        let mut reads = StateReads::start(rows, load, || {});
        assert!(at.elapsed() < Duration::from_secs(5), "start never waits");
        assert!(reads.pending(1) && reads.pending(2) && reads.pending(3));
        let got = wait_for(&mut reads, 2);
        let mut ids: Vec<u64> = got.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, [2, 3], "the other place's rows come while one hangs");
        assert!(reads.pending(1));
        assert_eq!(got[0].state.as_ref().unwrap().downloaded(), 40);
        drop(held);
        let got = wait_for(&mut reads, 1);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].id, &got[0].dest), (1, &PathBuf::from(dead)));
        assert!(!reads.pending(1));
    }

    /// A row that started before its read came (or was deleted) drops it.
    #[test]
    fn a_read_no_longer_wanted_is_dropped() {
        let load: Loader = Arc::new(|dest: &Path| Some(state(dest, 10)));
        let mut reads = StateReads::start(vec![(7, PathBuf::from("a.bin"))], load, || {});
        reads.forget(7);
        std::thread::sleep(Duration::from_millis(100));
        assert!(reads.take().is_empty());
        assert!(!reads.pending(7));
    }

    /// No state there (or one that doesn't read): None; files of the former codename are
    /// renamed and read.
    #[test]
    fn the_disk_loader_reads_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.bin");
        assert!(from_disk(&dest).is_none());
        std::fs::write(state_path(&dest), b"not json").unwrap();
        assert!(from_disk(&dest).is_none());
        let s = state(&dest, 25);
        s.write(&state_path(&dest)).unwrap();
        assert_eq!(from_disk(&dest).unwrap().downloaded(), 25);
    }

    #[test]
    fn places_are_drives_shares_or_folders_near_the_root() {
        if cfg!(windows) {
            assert_eq!(
                place_of(Path::new(r"\\192.0.2.1\share\deep\x.bin")),
                PathBuf::from(r"\\192.0.2.1\share")
            );
            assert_eq!(place_of(Path::new(r"C:\dl\a.bin")), PathBuf::from("C:"));
        } else {
            assert_eq!(
                place_of(Path::new("/mnt/nas/deep/x.bin")),
                PathBuf::from("/mnt/nas")
            );
            assert_eq!(place_of(Path::new("/a.bin")), PathBuf::from("/"));
        }
    }
}
