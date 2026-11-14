//! Linux's tray icon, set up and kept on a thread of its own (#828). Its library (tray-icon over
//! ksni) connects to the session bus as the icon is made, and waits for the bus at each change of
//! the icon after: for ever where the bus doesn't answer (a session started without D-Bus, a bus
//! that hangs), and the window froze while it waited. So a thread beside the window makes the icon
//! and keeps it ([`Beside`]), the window hands it each change as a job and never waits for one, and
//! it reads how far the making got ([`Beside::made`]), which never waits either. The rule of the
//! notification-area icon's questions on Windows (`watch.rs`, #747) and of the clipboard's reads.
//!
//! Until the icon is made the window behaves as with no tray host (closing quits, #59). After
//! [`WAIT`] it says in the log that it carries on without one ([`Phase::GaveUp`]); a bus that
//! answers later still brings the icon, and closing then hides the window in it.

// Only Linux makes its tray here; the state machine is tested everywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

/// How long the window waits for the icon before it says it carries on without it.
pub const WAIT: Duration = Duration::from_secs(5);

/// How far the thread got in making the icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Made {
    /// Still making it (waiting for the bus).
    Not,
    /// Made: it takes jobs.
    Up,
    /// The system refused it (no bus at all, no picture): logged by the maker.
    Failed,
}

/// What the window makes of [`Made`] after `waited`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Being made, for less than [`WAIT`]: the window behaves as with no tray host.
    Waiting,
    /// Not made after [`WAIT`]: the window carries on without it, and still takes it if it comes.
    GaveUp,
    /// The icon is there to use.
    Up,
    /// No icon this run.
    Failed,
}

/// The window's reading of `made` after `waited` since the thread started.
pub fn phase(made: Made, waited: Duration) -> Phase {
    match made {
        Made::Up => Phase::Up,
        Made::Failed => Phase::Failed,
        Made::Not if waited >= WAIT => Phase::GaveUp,
        Made::Not => Phase::Waiting,
    }
}

const NOT: u8 = 0;
const UP: u8 = 1;
const FAILED: u8 = 2;

/// A change for the thing the thread keeps.
type Job<T> = Box<dyn FnOnce(&T) + Send>;

/// A thing made and kept on a thread of its own (`T` never leaves it: tray-icon's icon and menu
/// can't cross threads); the window hands it jobs.
pub struct Beside<T> {
    made: Arc<AtomicU8>,
    jobs: Sender<Job<T>>,
}

impl<T: 'static> Beside<T> {
    /// Make the thing with `make` on a thread named `name`, then run the jobs the window hands it
    /// until the window drops this (the thing is then dropped there too, never waited for).
    /// `wake` is told once `make` returns.
    pub fn start(
        name: &str,
        make: impl FnOnce() -> Option<T> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> Beside<T> {
        let made = Arc::new(AtomicU8::new(NOT));
        let (jobs, rx) = channel::<Job<T>>();
        let state = made.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let thing = make();
                state.store(if thing.is_some() { UP } else { FAILED }, Ordering::Release);
                wake();
                if let Some(thing) = thing {
                    for job in rx {
                        job(&thing);
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("{name}: no thread to make it on ({e})");
            made.store(FAILED, Ordering::Release);
        }
        Beside { made, jobs }
    }

    /// How far the making got. Never waits.
    pub fn made(&self) -> Made {
        match self.made.load(Ordering::Acquire) {
            UP => Made::Up,
            FAILED => Made::Failed,
            _ => Made::Not,
        }
    }

    /// Have `job` run on the thing, in the order given. Never waits (a thread stuck on the bus
    /// only holds up the jobs after it).
    pub fn run(&self, job: impl FnOnce(&T) + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Instant;

    /// The window's reading of the thread's progress: waiting, then giving up after WAIT; made
    /// or refused whenever it comes, a late icon included.
    #[test]
    fn the_window_waits_a_few_seconds_then_carries_on_and_still_takes_a_late_icon() {
        assert_eq!(phase(Made::Not, Duration::ZERO), Phase::Waiting);
        assert_eq!(
            phase(Made::Not, WAIT - Duration::from_millis(1)),
            Phase::Waiting
        );
        assert_eq!(phase(Made::Not, WAIT), Phase::GaveUp);
        assert_eq!(phase(Made::Not, Duration::from_secs(3600)), Phase::GaveUp);
        for waited in [Duration::ZERO, WAIT, Duration::from_secs(3600)] {
            assert_eq!(phase(Made::Up, waited), Phase::Up, "{waited:?}");
            assert_eq!(phase(Made::Failed, waited), Phase::Failed, "{waited:?}");
        }
    }

    /// A thing whose making waits for the test (for ever, if the test never says).
    fn held(until: mpsc::Receiver<bool>) -> impl FnOnce() -> Option<Cell<u32>> + Send + 'static {
        move || until.recv().ok().and_then(|up| up.then(|| Cell::new(0)))
    }

    /// A bus that never answers holds up nothing: the window reads "not made" at once, every
    /// time, its jobs are taken at once, and dropping the handle doesn't wait for the thread.
    #[test]
    fn a_bus_that_never_answers_holds_up_nothing() {
        let (_never, until) = mpsc::channel();
        let b = Beside::start("test tray", held(until), || {});
        let start = Instant::now();
        for _ in 0..10_000 {
            assert_eq!(b.made(), Made::Not);
            b.run(|n: &Cell<u32>| n.set(n.get() + 1));
        }
        drop(b);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// A late answer: the window is woken, reads "up", and the jobs it handed meanwhile and
    /// after run on the thing, in order.
    #[test]
    fn a_late_answer_brings_the_icon_and_runs_the_jobs_in_order() {
        let (answer, until) = mpsc::channel();
        let (woke, woken) = mpsc::channel();
        let b = Beside::start("test tray", held(until), move || {
            let _ = woke.send(());
        });
        let (seen, saw) = mpsc::channel();
        let tell = seen.clone();
        b.run(move |n: &Cell<u32>| {
            n.set(n.get() + 1);
            let _ = tell.send(n.get());
        });
        assert!(woken.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(b.made(), Made::Not);
        answer.send(true).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(b.made(), Made::Up);
        b.run(move |n: &Cell<u32>| {
            n.set(n.get() + 10);
            let _ = seen.send(n.get());
        });
        assert_eq!(saw.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(saw.recv_timeout(Duration::from_secs(5)).unwrap(), 11);
    }

    /// A refused icon (no bus at all): the window is woken and reads "failed"; jobs go nowhere.
    #[test]
    fn a_refused_icon_is_failed_and_jobs_go_nowhere() {
        let (answer, until) = mpsc::channel();
        let (woke, woken) = mpsc::channel();
        let b = Beside::start("test tray", held(until), move || {
            let _ = woke.send(());
        });
        answer.send(false).unwrap();
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(b.made(), Made::Failed);
        b.run(|_: &Cell<u32>| panic!("no thing to run on"));
    }
}
