//! The clipboard's text, read off the window's thread (#418, #661). A read waits for the program
//! that owns the clipboard: on X11 (and Wayland, through XWayland) until it answers or a time
//! limit runs out, on Windows until a program that hands its copy over late does so, which a
//! hung program never does. Read on the window's thread, that wait froze the window.
//!
//! One thread reads, for everyone: the clipboard monitor, a text box's Paste and the dialogs
//! that open with a copied link ([`Prefill`]) ask ([`ask`]) and take the answer on a later
//! frame ([`answer`]); the window is woken when it comes. A read that never answers holds up
//! nothing but the next read.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// One ask for the clipboard's text, answered by a read that starts after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask(u64);

/// The thread that reads the clipboard when asked, one read at a time. Asks made while a read is
/// under way are answered together by the next one.
pub struct Reader {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    asked: Condvar,
}

#[derive(Default)]
struct State {
    /// The newest ask, and the newest one a read has answered.
    asked: u64,
    answered: u64,
    /// What the last read found.
    text: Option<String>,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Reader {
    /// `read` reads the clipboard (it may wait, for ever); `wake` is told when an answer has
    /// come (the window takes it on its next frame).
    pub fn new(
        read: impl Fn() -> Option<String> + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            asked: Condvar::new(),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("clipboard read".into())
            .spawn(move || {
                loop {
                    let ask = {
                        let mut s = worker.lock();
                        while s.asked == s.answered && !s.closed {
                            s = worker.asked.wait(s).unwrap_or_else(PoisonError::into_inner);
                        }
                        if s.closed {
                            return;
                        }
                        s.asked
                    };
                    // Not under the lock: an ask is never held up by the read under way.
                    let text = read();
                    {
                        let mut s = worker.lock();
                        s.text = text;
                        s.answered = ask;
                    }
                    wake();
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("clipboard: no thread to read it ({e}), it is not read");
        }
        Reader { shared }
    }

    /// Ask for the clipboard's text, at once (never waits for a read).
    pub fn ask(&self) -> Ask {
        let mut s = self.shared.lock();
        s.asked += 1;
        self.shared.asked.notify_one();
        Ask(s.asked)
    }

    /// The clipboard's text once a read started after `ask` has answered (None inside: no
    /// text); None while it hasn't.
    pub fn answer(&self, ask: Ask) -> Option<Option<String>> {
        let s = self.shared.lock();
        (s.answered >= ask.0).then(|| s.text.clone())
    }

    /// What the last read found (None: no text, or nothing read yet).
    pub fn latest(&self) -> Option<String> {
        self.shared.lock().text.clone()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.asked.notify_one();
    }
}

/// The window woken when an answer comes (set once the window exists).
static WINDOW: OnceLock<egui::Context> = OnceLock::new();

/// MDM's reader, of the system clipboard ([`crate::clipboard_text`]); started at its first ask.
fn reader() -> &'static Reader {
    static READER: OnceLock<Reader> = OnceLock::new();
    READER.get_or_init(|| {
        Reader::new(crate::clipboard_text, || {
            // A pass of the main window's UI, which draws the dialogs, even minimized.
            if let Some(ctx) = WINDOW.get() {
                ctx.request_ui_pass(egui::ViewportId::ROOT);
            }
        })
    })
}

/// The window to wake when a read answers.
pub fn wake(ctx: &egui::Context) {
    let _ = WINDOW.set(ctx.clone());
}

/// [`Reader::ask`] of the system clipboard.
pub fn ask() -> Ask {
    reader().ask()
}

/// [`Reader::answer`] of the system clipboard.
pub fn answer(ask: Ask) -> Option<Option<String>> {
    reader().answer(ask)
}

/// [`Reader::latest`] of the system clipboard.
pub fn latest() -> Option<String> {
    reader().latest()
}

/// A dialog's box filled from the clipboard as it opens (Add URL, Batch download, Add torrent,
/// Download video): asked when the dialog opens, and the text handed over once, on the frame
/// its answer comes. The dialog fills its box then if the user hasn't typed in it.
#[derive(Default)]
pub struct Prefill(Option<(Ask, &'static Reader)>);

impl Prefill {
    /// Ask the system clipboard now.
    pub fn ask() -> Self {
        Self::ask_of(reader())
    }

    /// Ask `reader` now (a test's).
    pub fn ask_of(reader: &'static Reader) -> Self {
        Prefill(Some((reader.ask(), reader)))
    }

    /// Whether the answer is still to come.
    #[cfg(test)]
    pub fn pending(&self) -> bool {
        self.0.is_some()
    }

    /// The clipboard's text, on the frame it comes, once (None before and after; and when the
    /// clipboard holds no text).
    pub fn take(&mut self) -> Option<String> {
        let (ask, reader) = self.0?;
        let got = reader.answer(ask)?;
        self.0 = None;
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Wait (at most 5 s) for `ask`'s answer.
    fn wait(r: &Reader, ask: Ask) -> Option<Option<String>> {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Some(got) = r.answer(ask) {
                return Some(got);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    /// #418, #661: a clipboard owner that never answers holds up no frame: asking, looking for
    /// the answer, the last text and a dialog's prefill all return at once, the answer just
    /// isn't there. When the owner answers at last, the window is woken and the text taken.
    #[test]
    fn a_clipboard_owner_that_never_answers_holds_up_no_frame() {
        let (release, stuck) = std::sync::mpsc::channel::<()>();
        let stuck = Mutex::new(stuck);
        let (woke, woken) = std::sync::mpsc::channel::<()>();
        let r: &'static Reader = Box::leak(Box::new(Reader::new(
            move || {
                let _ = stuck.lock().unwrap().recv();
                Some("https://late.example/a.zip".to_owned())
            },
            move || {
                let _ = woke.send(());
            },
        )));
        let started = Instant::now();
        let first = r.ask();
        let mut prefill = Prefill::ask_of(r);
        for _ in 0..1000 {
            assert_eq!(r.answer(first), None);
            assert_eq!(prefill.take(), None);
            assert_eq!(r.latest(), None);
            // A text box's menu asks each time it opens: no second thread, no wait.
            let _ = r.ask();
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?} for 1000 frames",
            started.elapsed()
        );
        assert!(woken.try_recv().is_err(), "no answer, no wake");
        // The owner answers at last (and every read after it at once).
        drop(release);
        assert_eq!(
            wait(r, first),
            Some(Some("https://late.example/a.zip".into()))
        );
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        // The asks made meanwhile are answered by the next read.
        let last = r.ask();
        assert!(wait(r, last).is_some());
        assert_eq!(
            prefill.take().as_deref(),
            Some("https://late.example/a.zip")
        );
        assert_eq!(prefill.take(), None, "handed over once");
    }

    /// An ask is answered by a read that started after it: a text copied just before a dialog
    /// opens is the one it gets, not an older read's.
    #[test]
    fn an_answer_is_never_older_than_its_ask() {
        let now = Arc::new(Mutex::new("old".to_owned()));
        let clip = now.clone();
        let r = Reader::new(move || Some(clip.lock().unwrap().clone()), || {});
        let first = r.ask();
        assert_eq!(wait(&r, first), Some(Some("old".into())));
        *now.lock().unwrap() = "new".into();
        assert_eq!(
            r.answer(first),
            Some(Some("old".into())),
            "still the old one"
        );
        let second = r.ask();
        assert_eq!(wait(&r, second), Some(Some("new".into())));
        assert_eq!(r.latest().as_deref(), Some("new"));
    }
}
