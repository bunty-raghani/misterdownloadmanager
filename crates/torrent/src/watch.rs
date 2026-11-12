//! The session's numbers, read off the window's thread (#693). librqbit answers a torrent's stats
//! under its state lock, and a pause holds that lock while the torrent's files finish the writes
//! under way (an add holds the session's list while it opens the files): on a busy or slow disk,
//! or with an antivirus scanning each write, that is seconds, and the window, reading the stats
//! each frame, froze that long.
//!
//! One thread reads every torrent for everyone, every [`EVERY`], and after each change made
//! through [`crate::Torrents`] the one changed is read again at once ([`Watch::put`]). The window
//! takes the last numbers ([`Watch::get`]) without waiting: a read that hangs holds up nothing
//! but the next one.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::{Handle, Stats, Swarm};

/// How often the thread reads the session again.
pub(crate) const EVERY: Duration = Duration::from_millis(200);

/// How long a torrent's peers, trackers and pieces are read for after they were last asked for
/// ([`Watch::want`]): only the progress window shows them, and only for its own torrent.
const WANTED_FOR: Duration = Duration::from_secs(2);

/// One torrent as last read.
pub(crate) struct Seen {
    pub stats: Stats,
    /// The files chosen (None: all).
    pub chosen: Option<Vec<usize>>,
    /// librqbit's handle, for what doesn't wait on its locks (its metadata, its limits, its
    /// piece order). None only in tests.
    pub handle: Option<Handle>,
    /// Its peers, trackers and pieces, while they are wanted.
    pub swarm: Option<Swarm>,
}

/// The last reading of every torrent, and the thread that keeps it.
pub(crate) struct Watch {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the thread early: closed, or a torrent's details newly wanted.
    wake: Condvar,
}

#[derive(Default)]
struct State {
    seen: HashMap<String, Arc<Seen>>,
    /// Counts the puts; each torrent put, with the count when it was. A reading that started
    /// before a put keeps what the put said about that torrent (it read older numbers).
    version: u64,
    put: HashMap<String, u64>,
    /// The torrents whose details were asked for, and when last.
    wanted: HashMap<String, Instant>,
    /// Read again now, not at the next [`EVERY`] (details newly wanted).
    kicked: bool,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Info hashes as keys: lowercase, as librqbit writes them.
fn key(hash: &str) -> String {
    hash.to_ascii_lowercase()
}

impl Watch {
    /// Keep `first` (a reading made before), and read again with `read` every `every` on a
    /// thread of its own. `read` gets the torrents whose details are wanted and returns every
    /// torrent there is, by info hash; it may wait (for ever).
    pub(crate) fn start(
        first: HashMap<String, Arc<Seen>>,
        every: Duration,
        mut read: impl FnMut(&HashSet<String>) -> HashMap<String, Arc<Seen>> + Send + 'static,
    ) -> Watch {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                seen: first,
                ..Default::default()
            }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("torrent stats".into())
            .spawn(move || {
                loop {
                    let (start, wanted) = {
                        let mut s = worker.lock();
                        if s.closed {
                            return;
                        }
                        s.wanted.retain(|_, at| at.elapsed() < WANTED_FOR);
                        (s.version, s.wanted.keys().cloned().collect::<HashSet<_>>())
                    };
                    // Not under the lock: the window never waits for this.
                    let fresh = read(&wanted);
                    let s = worker.lock();
                    let mut s = publish(s, start, fresh);
                    if s.closed {
                        return;
                    }
                    s = worker
                        .wake
                        .wait_timeout_while(s, every, |s| !s.closed && !s.kicked)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                    s.kicked = false;
                    drop(s);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("torrents: can't start the thread that reads their numbers: {e}");
        }
        Watch { shared }
    }

    /// The torrent as last read (None: not in the session). Never waits on the engine.
    pub(crate) fn get(&self, hash: &str) -> Option<Arc<Seen>> {
        self.shared.lock().seen.get(&key(hash)).cloned()
    }

    /// Read its peers, trackers and pieces too, for [`WANTED_FOR`] from now.
    pub(crate) fn want(&self, hash: &str) {
        let mut s = self.shared.lock();
        if s.wanted.insert(key(hash), Instant::now()).is_none() {
            s.kicked = true;
            self.shared.wake.notify_all();
        }
    }

    /// What a change just made says about the torrent (None: it left the session); its details
    /// stay as last read until the next reading.
    pub(crate) fn put(&self, hash: &str, seen: Option<Seen>) {
        let mut s = self.shared.lock();
        let k = key(hash);
        s.version += 1;
        let v = s.version;
        s.put.insert(k.clone(), v);
        match seen {
            Some(mut new) => {
                if new.swarm.is_none() {
                    new.swarm = s.seen.get(&k).and_then(|old| old.swarm.clone());
                }
                s.seen.insert(k, Arc::new(new));
            }
            None => {
                s.seen.remove(&k);
            }
        }
    }

    /// Stop reading (the session stops).
    pub(crate) fn close(&self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.close();
    }
}

/// Take a reading begun at put count `start`: the torrents put since keep what the put said.
fn publish(
    mut s: MutexGuard<'_, State>,
    start: u64,
    mut fresh: HashMap<String, Arc<Seen>>,
) -> MutexGuard<'_, State> {
    let newer: Vec<String> = s
        .put
        .iter()
        .filter(|(_, at)| **at > start)
        .map(|(h, _)| h.clone())
        .collect();
    for h in newer {
        match s.seen.get(&h) {
            Some(kept) => fresh.insert(h, kept.clone()),
            None => fresh.remove(&h),
        };
    }
    s.put.retain(|_, at| *at > start);
    s.seen = fresh;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Phase;
    use std::sync::mpsc;

    fn stats(done: u64) -> Stats {
        Stats {
            phase: Phase::Downloading,
            checked: None,
            done,
            total: 100,
            uploaded: 0,
            fetched: done,
            down_speed: 0.0,
            up_speed: 0.0,
            file_done: vec![done],
            peers_live: 1,
            peers_seen: 1,
            error: None,
        }
    }

    fn seen(done: u64) -> Arc<Seen> {
        Arc::new(Seen {
            stats: stats(done),
            chosen: None,
            handle: None,
            swarm: None,
        })
    }

    /// #693: while the engine holds a torrent's lock (here, a reading that waits until the
    /// test lets it go), the window's frames keep going: each takes the last numbers at once.
    #[test]
    fn a_frame_never_waits_for_the_engine() {
        let (entered_tx, entered) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let mut reads = 0u64;
        let w = Watch::start(
            HashMap::from([("aa".to_owned(), seen(10))]),
            Duration::from_millis(5),
            move |_| {
                reads += 1;
                if reads == 2 {
                    // The second reading finds the lock held, for as long as the test says.
                    let _ = entered_tx.send(());
                    let _ = released.lock().unwrap().recv();
                }
                HashMap::from([("aa".to_owned(), seen(10 + reads))])
            },
        );
        entered
            .recv_timeout(Duration::from_secs(5))
            .expect("the reading reached the held lock");
        // Twenty frames while the lock is held: each takes the last numbers at once (a busy
        // runner may sleep longer between them; only the frames themselves are timed).
        for _ in 0..20 {
            let frame = Instant::now();
            let got = w.get("AA").expect("the torrent as last read");
            assert_eq!(got.stats.done, 11, "the reading before the stall");
            assert!(
                frame.elapsed() < Duration::from_millis(50),
                "a frame waited {:?} on the engine",
                frame.elapsed()
            );
            std::thread::sleep(Duration::from_millis(16));
        }
        // The lock let go: the numbers move again.
        release.send(()).unwrap();
        let at = Instant::now();
        while w.get("aa").unwrap().stats.done <= 11 {
            assert!(at.elapsed() < Duration::from_secs(5), "never read again");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A change made meanwhile wins over a reading that began before it (which read older
    /// numbers); a torrent removed stays removed.
    #[test]
    fn a_put_wins_over_an_older_reading() {
        let s = Mutex::new(State {
            seen: HashMap::from([("aa".to_owned(), seen(1)), ("bb".to_owned(), seen(1))]),
            ..Default::default()
        });
        let start = s.lock().unwrap().version;
        // A reading starts; meanwhile aa is paused (put) and bb removed.
        let w = Watch {
            shared: Arc::new(Shared {
                state: s,
                wake: Condvar::new(),
            }),
        };
        w.shared.lock().closed = true;
        w.put(
            "aa",
            Some(Seen {
                stats: stats(50),
                chosen: None,
                handle: None,
                swarm: None,
            }),
        );
        w.put("bb", None);
        let fresh = HashMap::from([
            ("aa".to_owned(), seen(2)),
            ("bb".to_owned(), seen(2)),
            ("cc".to_owned(), seen(2)),
        ]);
        drop(publish(w.shared.lock(), start, fresh));
        assert_eq!(w.get("aa").unwrap().stats.done, 50, "the put's numbers");
        assert!(w.get("bb").is_none(), "removed meanwhile");
        assert_eq!(w.get("cc").unwrap().stats.done, 2, "new in the reading");
        // The next reading began after the puts: it is taken whole.
        let start = w.shared.lock().version;
        drop(publish(
            w.shared.lock(),
            start,
            HashMap::from([("aa".to_owned(), seen(60))]),
        ));
        assert_eq!(w.get("aa").unwrap().stats.done, 60);
        assert!(w.get("cc").is_none());
    }

    /// Details are read only while asked for, and a put keeps the last ones.
    #[test]
    fn details_while_wanted() {
        let (tx, rx) = mpsc::channel();
        let w = Watch::start(HashMap::new(), Duration::from_millis(5), move |wanted| {
            let _ = tx.send(wanted.clone());
            HashMap::from([(
                "aa".to_owned(),
                Arc::new(Seen {
                    stats: stats(1),
                    chosen: None,
                    handle: None,
                    swarm: wanted.contains("aa").then(|| Swarm {
                        trackers: vec!["udp://t".into()],
                        ..Default::default()
                    }),
                }),
            )])
        });
        assert!(rx.recv().unwrap().is_empty());
        w.want("AA");
        let at = Instant::now();
        while w.get("aa").and_then(|s| s.swarm.clone()).is_none() {
            assert!(at.elapsed() < Duration::from_secs(5), "details never read");
            std::thread::sleep(Duration::from_millis(5));
        }
        w.put(
            "aa",
            Some(Seen {
                stats: stats(2),
                chosen: None,
                handle: None,
                swarm: None,
            }),
        );
        let got = w.get("aa").unwrap();
        assert_eq!(got.stats.done, 2);
        assert_eq!(got.swarm.as_ref().unwrap().trackers, ["udp://t"]);
    }
}
