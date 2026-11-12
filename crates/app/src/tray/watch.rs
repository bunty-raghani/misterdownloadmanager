//! What the shell says of MDM's notification-area icon, asked off the window's thread (#747).
//! Windows' questions about an icon wait for Explorer: for seconds when it is busy (8 s in #747,
//! the window frozen meanwhile). So a thread of its own asks ([`Watch`]), every [`EVERY`], and
//! sets the icon's tip too; the window takes the last answer ([`Watch::latest`]), which never
//! waits, and is woken when it changes. The rule of the clipboard's reads (`clipboard.rs`) and the
//! torrents' numbers (#693).
//!
//! No answer is not an answer: none yet, one too old ([`STALE`]: the shell stopped answering) or
//! one being written as the window looks ([`Icon::Unknown`]) never decides that the icon is gone.
//! Only the shell saying so does ([`Icon::Gone`]): no taskbar at all (Explorer isn't running), or
//! a taskbar that answers and has no icon of MDM's, in any place. A window hidden while the answer
//! is unknown can still come back: from the icon once it answers, and a second start of MDM always
//! brings the window back (`App::handle_command`).

// The watcher asks Windows' shell; elsewhere only the answers' meaning is used (macOS asks the
// menu bar on the window's thread, which AppKit wants; Linux asks whether a tray host runs).
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

/// How often the shell is asked.
pub const EVERY: Duration = Duration::from_secs(2);

/// An answer this old, from when its question was put, is no answer: the shell has been slow to
/// give the next (three questions' time).
pub const STALE: Duration = Duration::from_secs(6);

/// How long a running shell must have said, answer after answer, that it has no icon of MDM's
/// before a window hidden in the notification area comes back to the taskbar: longer than
/// Explorer takes to start again and be given the icon back (`TaskbarCreated`).
pub const RESCUE_AFTER: Duration = Duration::from_secs(10);

/// One answer of the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// No taskbar: Explorer isn't running (or a shell without a notification area is).
    NoTaskbar,
    /// The taskbar is there and didn't answer (it stopped responding, or the question timed out).
    Silent,
    /// The shell answered: whether it gives the icon a place on screen, and whether it holds it
    /// (an icon among the hidden ones behind ^ may have no place, and is held all the same).
    Answered { placed: bool, held: bool },
}

impl Shell {
    /// Whether this answer says the icon isn't there.
    fn says_gone(self) -> bool {
        matches!(
            self,
            Shell::NoTaskbar
                | Shell::Answered {
                    placed: false,
                    held: false
                }
        )
    }
}

/// What the window makes of the shell's last answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// The shell holds the icon, or gives it a place: it can bring a hidden window back.
    There,
    /// No answer, or none recent: nothing is concluded either way.
    Unknown,
    /// The shell said it has no icon of MDM's, or there is no taskbar.
    Gone,
}

/// The shell's last answer, when its question was put, and since when every answer in a row said
/// the icon is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub shell: Shell,
    pub asked: Instant,
    pub gone_since: Option<Instant>,
}

/// What `seen` (the last answer the window has, if any) says at `now`.
pub fn icon(seen: Option<Seen>, now: Instant) -> Icon {
    let Some(seen) = seen else {
        return Icon::Unknown;
    };
    if now.saturating_duration_since(seen.asked) >= STALE {
        return Icon::Unknown;
    }
    match seen.shell {
        Shell::Silent => Icon::Unknown,
        Shell::Answered { placed, held } if placed || held => Icon::There,
        Shell::NoTaskbar | Shell::Answered { .. } => Icon::Gone,
    }
}

/// Whether a window hidden in the notification area comes back to the taskbar (minimized): only
/// once a running shell has said for [`RESCUE_AFTER`] that it has no icon of MDM's. Never on no
/// answer, an old one, or while there is no taskbar: Explorer gives the icon back as it starts
/// again, and a window brought out then would only be in the way.
pub fn rescue(seen: Option<Seen>, now: Instant) -> bool {
    icon(seen, now) == Icon::Gone
        && seen.is_some_and(|s| {
            matches!(s.shell, Shell::Answered { .. })
                && s.gone_since
                    .is_some_and(|t| now.saturating_duration_since(t) >= RESCUE_AFTER)
        })
}

/// The shell, as the watcher's thread asks it (Windows: `win32::tray_icon_ask`; tests: a fake).
pub trait Port: Send + 'static {
    /// Ask about the icon (it may wait, for ever).
    fn ask(&mut self) -> Shell;
    /// Set the icon's tip; false if the shell didn't take it.
    fn tip(&mut self, tip: &str) -> bool;
}

/// The thread that asks the shell; the window reads its last answer.
pub struct Watch {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// A new tip to set, or the watch closed.
    told: Condvar,
}

#[derive(Default)]
struct State {
    seen: Option<Seen>,
    /// The tip the window wants, and its number (one more for each).
    tip: Option<String>,
    tip_number: u64,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The state if nobody holds it: the window never waits, even for the watcher's short write.
    fn try_lock(&self) -> Option<MutexGuard<'_, State>> {
        match self.state.try_lock() {
            Ok(s) => Some(s),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

impl Watch {
    /// Ask `port` now and every [`EVERY`] on a thread of its own; `wake` is told when an answer
    /// differs from the one before.
    pub fn new(port: impl Port, wake: impl Fn() + Send + 'static) -> Watch {
        Watch::every(EVERY, port, wake)
    }

    /// [`Watch::new`], asking every `every`.
    fn every(every: Duration, mut port: impl Port, wake: impl Fn() + Send + 'static) -> Watch {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            told: Condvar::new(),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("tray icon".into())
            .spawn(move || {
                let mut next = Instant::now();
                let mut last: Option<Shell> = None;
                let mut gone_since: Option<Instant> = None;
                // The tip's number the shell has, if it has the current one.
                let mut set: Option<u64> = None;
                loop {
                    if Instant::now() >= next {
                        // Dated from the question: an answer that took long is already old.
                        let asked = Instant::now();
                        // Not under the lock: the window never waits for the shell.
                        let shell = port.ask();
                        next = Instant::now() + every;
                        gone_since = if shell.says_gone() {
                            gone_since.or(Some(asked))
                        } else {
                            None
                        };
                        if !matches!(shell, Shell::Answered { held: true, .. }) {
                            // An icon added again (Explorer started again) has the first tip.
                            set = None;
                        }
                        let changed = last != Some(shell);
                        last = Some(shell);
                        {
                            let mut s = worker.lock();
                            if s.closed {
                                return;
                            }
                            s.seen = Some(Seen {
                                shell,
                                asked,
                                gone_since,
                            });
                        }
                        if changed {
                            log(shell);
                            wake();
                        }
                    }
                    let (tip, number) = {
                        let s = worker.lock();
                        if s.closed {
                            return;
                        }
                        (s.tip.clone(), s.tip_number)
                    };
                    let held = matches!(last, Some(Shell::Answered { held: true, .. }));
                    if let Some(tip) = tip
                        && held
                        && set != Some(number)
                        && port.tip(&tip)
                    {
                        set = Some(number);
                    }
                    // Until the next question, or a new tip (one the shell didn't take is tried
                    // again with the next question).
                    let mut s = worker.lock();
                    while !s.closed && s.tip_number == number {
                        let left = next.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        s = worker
                            .told
                            .wait_timeout(s, left)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    if s.closed {
                        return;
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("tray icon: no thread to ask the shell ({e}): its answer stays unknown");
        }
        Watch { shared }
    }

    /// The last answer; None before the first, or while the watcher is writing one. Never waits.
    pub fn latest(&self) -> Option<Seen> {
        self.shared.try_lock().and_then(|s| s.seen)
    }

    /// Have the icon's tip set to `tip`. False if the watcher was busy writing an answer: asked
    /// again at the next frame. Never waits.
    pub fn tip(&self, tip: &str) -> bool {
        let Some(mut s) = self.shared.try_lock() else {
            return false;
        };
        if s.tip.as_deref() != Some(tip) {
            s.tip = Some(tip.to_owned());
            s.tip_number += 1;
            self.shared.told.notify_one();
        }
        true
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // Never joined: the thread may be inside a question the shell never answers.
        self.shared.lock().closed = true;
        self.shared.told.notify_one();
    }
}

/// The shell's answer in the log, when it changes: a log in a report says which case it was.
fn log(shell: Shell) {
    match shell {
        Shell::NoTaskbar => tracing::info!("tray icon: no taskbar (Explorer isn't running)"),
        Shell::Silent => tracing::info!("tray icon: the shell didn't answer"),
        Shell::Answered { placed, held } => {
            tracing::info!("tray icon: placed {placed}, held by the shell {held}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn seen(shell: Shell, asked: Instant) -> Option<Seen> {
        Some(Seen {
            shell,
            asked,
            gone_since: shell.says_gone().then_some(asked),
        })
    }

    const HELD: Shell = Shell::Answered {
        placed: false,
        held: true,
    };
    const PLACED: Shell = Shell::Answered {
        placed: true,
        held: false,
    };
    const NONE: Shell = Shell::Answered {
        placed: false,
        held: false,
    };

    /// An icon the shell holds (behind ^, maybe with no place: #750, #718, #754) or places is
    /// there; a running shell that has neither, or no taskbar, says it is gone.
    #[test]
    fn held_or_placed_is_there_and_only_the_shell_says_gone() {
        let now = Instant::now();
        for shell in [
            HELD,
            PLACED,
            Shell::Answered {
                placed: true,
                held: true,
            },
        ] {
            assert_eq!(icon(seen(shell, now), now), Icon::There, "{shell:?}");
        }
        assert_eq!(icon(seen(NONE, now), now), Icon::Gone);
        assert_eq!(icon(seen(Shell::NoTaskbar, now), now), Icon::Gone);
    }

    /// No answer yet, a shell that didn't answer, an answer too old (or a lock the watcher holds:
    /// `latest` gives None, as before the first) is unknown, never gone.
    #[test]
    fn no_answer_or_an_old_one_is_unknown() {
        let now = Instant::now();
        assert_eq!(icon(None, now), Icon::Unknown);
        assert_eq!(icon(seen(Shell::Silent, now), now), Icon::Unknown);
        for shell in [HELD, NONE, Shell::NoTaskbar] {
            let old = now.checked_sub(STALE).unwrap();
            assert_eq!(icon(seen(shell, old), now), Icon::Unknown, "{shell:?}");
            let recent = now.checked_sub(STALE - Duration::from_millis(1)).unwrap();
            assert_ne!(icon(seen(shell, recent), now), Icon::Unknown, "{shell:?}");
        }
    }

    /// A hidden window comes back only once a running shell has said for RESCUE_AFTER, answer
    /// after answer, that it has no icon: not on no answer, an old or a silent one, a short gap
    /// (Explorer starting again), or while there is no taskbar at all.
    #[test]
    fn only_a_lasting_answer_of_no_icon_brings_a_hidden_window_back() {
        let now = Instant::now();
        let long_ago = now.checked_sub(RESCUE_AFTER).unwrap();
        let gone = |shell, since| {
            Some(Seen {
                shell,
                asked: now,
                gone_since: Some(since),
            })
        };
        assert!(rescue(gone(NONE, long_ago), now));
        assert!(!rescue(gone(NONE, now - Duration::from_secs(3)), now));
        assert!(!rescue(gone(Shell::NoTaskbar, long_ago), now));
        assert!(!rescue(None, now));
        assert!(!rescue(seen(Shell::Silent, now), now));
        assert!(!rescue(seen(HELD, now), now));
        // The last of a long run of "no icon", but from long ago: the shell stopped answering.
        let stale = Some(Seen {
            shell: NONE,
            asked: now.checked_sub(STALE).unwrap(),
            gone_since: Some(long_ago.checked_sub(STALE).unwrap()),
        });
        assert!(!rescue(stale, now));
    }

    /// A fake shell: answers what the test sends, waiting for it (for ever, if nothing comes).
    struct Fake {
        answers: mpsc::Receiver<Shell>,
        asked: mpsc::Sender<()>,
        tips: mpsc::Sender<String>,
        take_tips: bool,
    }

    impl Port for Fake {
        fn ask(&mut self) -> Shell {
            let _ = self.asked.send(());
            self.answers.recv().unwrap_or(Shell::Silent)
        }
        fn tip(&mut self, tip: &str) -> bool {
            let _ = self.tips.send(tip.to_owned());
            self.take_tips
        }
    }

    /// Have the tip set, as the window does at each frame until the watch isn't busy.
    fn tip(w: &Watch, tip: &str) {
        while !w.tip(tip) {
            std::thread::yield_now();
        }
    }

    /// How often the tests' watches ask: the fake shell answers when the test says.
    const QUICK: Duration = Duration::from_millis(20);

    fn fake(
        take_tips: bool,
    ) -> (
        Fake,
        mpsc::Sender<Shell>,
        mpsc::Receiver<()>,
        mpsc::Receiver<String>,
    ) {
        let (answer, answers) = mpsc::channel();
        let (asked, was_asked) = mpsc::channel();
        let (tips, tipped) = mpsc::channel();
        (
            Fake {
                answers,
                asked,
                tips,
                take_tips,
            },
            answer,
            was_asked,
            tipped,
        )
    }

    /// #747: a shell that never answers holds up nothing: the window's reads and tips return at
    /// once, the answer stays unknown, and dropping the watch doesn't wait for the thread.
    #[test]
    fn a_shell_that_never_answers_holds_up_nothing() {
        let (port, _answer, was_asked, _tipped) = fake(true);
        let w = Watch::every(QUICK, port, || {});
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        for _ in 0..10_000 {
            assert_eq!(w.latest(), None);
            assert!(w.tip("MDM"));
            assert_eq!(icon(w.latest(), Instant::now()), Icon::Unknown);
        }
        drop(w);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// The window reading while the watcher writes gets None, unknown: never gone.
    #[test]
    fn a_busy_watch_is_no_answer() {
        let (port, answer, was_asked, _tipped) = fake(true);
        let w = Watch::every(QUICK, port, || {});
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(NONE).unwrap();
        let start = Instant::now();
        while w.latest().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        let _writing = w.shared.lock();
        assert_eq!(w.latest(), None);
        assert_eq!(icon(w.latest(), Instant::now()), Icon::Unknown);
        assert!(
            !w.tip("MDM"),
            "a busy watch takes the tip at the next frame"
        );
    }

    /// Answers reach the window, which is woken only when one differs from the one before; the
    /// tip is set once the shell holds the icon, and again once the icon comes back.
    #[test]
    fn answers_wake_the_window_when_they_change_and_the_tip_follows_the_icon() {
        let (port, answer, was_asked, tipped) = fake(true);
        let (woke, woken) = mpsc::channel();
        let w = Watch::every(QUICK, port, move || {
            let _ = woke.send(());
        });
        tip(&w, "MDM 1");
        // Not held yet: no tip set.
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(NONE).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(w.shared.lock().seen.map(|s| s.shell), Some(NONE));
        assert!(tipped.recv_timeout(Duration::from_millis(200)).is_err());
        // Held: the tip goes, and the window is woken.
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(HELD).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            tipped.recv_timeout(Duration::from_secs(5)).unwrap(),
            "MDM 1"
        );
        // The same answer again: no wake, and the tip isn't sent again.
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(HELD).unwrap();
        assert!(woken.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(tipped.try_recv().is_err());
        // A new tip goes with the next answer.
        tip(&w, "MDM 2");
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(HELD).unwrap();
        assert_eq!(
            tipped.recv_timeout(Duration::from_secs(5)).unwrap(),
            "MDM 2"
        );
        // The icon lost and added back (Explorer started again): the tip is set again.
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(Shell::NoTaskbar).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
        answer.send(HELD).unwrap();
        assert_eq!(
            tipped.recv_timeout(Duration::from_secs(5)).unwrap(),
            "MDM 2"
        );
    }

    /// "Gone" is dated from the first of an unbroken run of such answers: one answer that isn't
    /// (the icon back, or no answer) starts it again.
    #[test]
    fn the_run_of_gone_answers_restarts_after_any_other() {
        let (port, answer, was_asked, _tipped) = fake(false);
        let w = Watch::every(QUICK, port, || {});
        let next = |shell| {
            was_asked.recv_timeout(Duration::from_secs(5)).unwrap();
            answer.send(shell).unwrap();
            let start = Instant::now();
            loop {
                if let Some(s) = w.latest()
                    && s.shell == shell
                {
                    return s;
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::yield_now();
            }
        };
        let first = next(NONE);
        assert_eq!(first.gone_since, Some(first.asked));
        let second = next(Shell::NoTaskbar);
        assert_eq!(second.gone_since, Some(first.asked));
        let silent = next(Shell::Silent);
        assert_eq!(silent.gone_since, None);
        let again = next(NONE);
        assert_eq!(again.gone_since, Some(again.asked));
    }
}
