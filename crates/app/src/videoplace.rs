//! A video's place, looked at beside the window (found with #1170).
//!
//! yt-dlp takes any `<stem>.*` in its folder as its own (#32), so a new video's name is numbered
//! on past what its folder holds (`naming::stem_destination`), and a video that starts takes a
//! free stem when another row holds its own (#442, `place_to_take`). Both read the folder, and
//! did so on the window's thread: a sleeping drive or a network folder that doesn't answer held
//! the window. As Download File Info does since #743, the row now shows its name as given
//! (fitted, not numbered: `naming::stem_guess`) and takes the numbered one when the answer comes;
//! a video started before then waits for it (asked already, it usually comes within a few
//! milliseconds), then starts under it. Each question is asked on a thread of its own: one that
//! hangs on a dead share holds up nothing else.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::model::Item;

/// How a video's place is looked at: `app::video_place` on the disk, or a test's. Given copies
/// of the row and of the other rows in its folder ([`Item::place_copy`]); None: it stays.
pub type Look = Arc<dyn Fn(&Item, &[Item]) -> Option<PathBuf> + Send + Sync>;

/// A look's answer, for the window to take.
pub struct Answer {
    pub id: u64,
    /// The row's place it was asked for.
    pub asked: PathBuf,
    /// Where it goes (None: where it is).
    pub place: Option<PathBuf>,
    /// The row was waiting to start (`Some(show)`: its progress window shown).
    pub start: Option<bool>,
}

/// The looks under way, and the starts waiting for them.
pub struct VideoPlaces {
    look: Look,
    tx: Sender<(u64, PathBuf, Option<PathBuf>)>,
    rx: Receiver<(u64, PathBuf, Option<PathBuf>)>,
    /// The place each row's last question was asked for.
    asked: HashMap<u64, PathBuf>,
    /// The rows waiting to start, and whether their progress window shows then.
    waiting: HashMap<u64, bool>,
    /// The rows whose place was just taken for their start: the start goes on (once).
    placed: HashSet<u64>,
}

impl VideoPlaces {
    pub fn new(look: Look) -> Self {
        let (tx, rx) = channel();
        VideoPlaces {
            look,
            tx,
            rx,
            asked: HashMap::new(),
            waiting: HashMap::new(),
            placed: HashSet::new(),
        }
    }

    /// Ask where `item` goes (its copy, and copies of the other rows in its folder), beside the
    /// caller; `wake` when the answer is there. Asked once for a place.
    pub fn ask(&mut self, item: Item, others: Vec<Item>, wake: impl Fn() + Send + 'static) {
        if self.asked.get(&item.id) == Some(&item.dest) {
            return;
        }
        let (id, dest) = (item.id, item.dest.clone());
        self.asked.insert(id, dest.clone());
        let (look, tx) = (self.look.clone(), self.tx.clone());
        let spawned = std::thread::Builder::new()
            .name("video place".into())
            .spawn(move || {
                let place = look(&item, &others);
                let _ = tx.send((id, dest, place));
                wake();
            });
        if let Err(e) = spawned {
            tracing::warn!("a video's place: no thread to look ({e})");
            self.asked.remove(&id);
        }
    }

    /// Row `id` starts once its place has come (`show`: with its progress window).
    pub fn wait(&mut self, id: u64, show: bool) {
        *self.waiting.entry(id).or_insert(show) |= show;
    }

    /// Row `id` waits to start.
    pub fn waiting(&self, id: u64) -> bool {
        self.waiting.contains_key(&id)
    }

    /// The rows waiting to start.
    pub fn waiting_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.waiting.keys().copied()
    }

    /// Row `id` doesn't start after all (stopped while it waited).
    pub fn unwait(&mut self, id: u64) {
        self.waiting.remove(&id);
    }

    /// Row `id`'s place was taken for the start that follows.
    pub fn placed(&mut self, id: u64) {
        self.placed.insert(id);
    }

    /// Whether row `id` has its place for this start (taken: asked again for the next).
    pub fn take_placed(&mut self, id: u64) -> bool {
        self.placed.remove(&id)
    }

    /// The answers that came since the last call, each for the place its row was last asked
    /// about. Never waits.
    pub fn take(&mut self) -> Vec<Answer> {
        let mut out = Vec::new();
        while let Ok((id, asked, place)) = self.rx.try_recv() {
            if self.asked.get(&id) != Some(&asked) {
                continue; // asked again since, for another place
            }
            self.asked.remove(&id);
            let start = self.waiting.remove(&id);
            out.push(Answer {
                id,
                asked,
                place,
                start,
            });
        }
        out
    }
}
