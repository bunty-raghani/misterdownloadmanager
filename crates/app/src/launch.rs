//! Other programs MDM starts and waits for, and the files and pages it hands to the system,
//! never waited for on the window's thread (#222): a frame that waits for a program freezes the
//! window for as long as that program takes, and a program can take forever (a file manager
//! that never answers over D-Bus froze the window until the freeze report caught it). So a
//! window starts them here, on a thread of their own, each with a time limit, and goes on.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Run `cmd` quietly on a thread of its own, waiting at most `limit` for it; when it can't
/// start, fails or runs out of time (it is then ended), `fallback` runs, on that thread.
/// Returns at once.
// Windows' callers start their programs (Explorer) without waiting for them.
#[cfg_attr(windows, allow(dead_code))]
pub fn run_detached_with_fallback(
    mut cmd: Command,
    limit: Duration,
    fallback: impl FnOnce() + Send + 'static,
) {
    let name = program_name(&cmd);
    let started = std::thread::Builder::new()
        .name("launch".into())
        .spawn(move || {
            if !run_within(&mut cmd, limit) {
                fallback();
            }
        });
    if let Err(e) = started {
        tracing::warn!("{name}: no thread to run it on: {e}");
    }
}

/// Hand `target` (a file, a folder or an address) to the system's program for it, on a thread
/// of its own: the `open` crate waits for the launcher it starts (up to 250 ms each on Windows
/// and Linux, until it exits on macOS).
pub fn open(target: impl AsRef<OsStr>) {
    let target = target.as_ref().to_owned();
    spawn("launch", move || {
        let _ = open::that_detached(&target);
    });
}

/// Open a web page in the browser (a service's sign-in, a site's page the download needs, #701).
/// Never in an off-screen test run (MDM_OFFSCREEN), whose clicks come from scripts, not from a
/// person at the browser: its log says what would have opened.
pub fn open_page(url: &str) {
    if crate::offscreen() {
        tracing::info!("open in the browser (off-screen, not opened): {url}");
    } else {
        open(url);
    }
}

/// Run `job` on a thread of its own named `name`, and return at once: for what a window hands
/// over that may wait (a program, a file on a sleeping hard disk, a USB drive or a network
/// folder: #743, #751).
pub fn spawn(name: &str, job: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(job) {
        tracing::warn!("{name}: no thread to run it on: {e}");
    }
}

/// An answer the system may take its time to give (a D-Bus call, a look at a folder that may be
/// on a network share), asked on a thread of its own and kept: a window that needs it every frame
/// reads the last answer, and never waits for more than it says (the same rule as the programs
/// above, for calls).
pub struct Asked<K, T> {
    shared: Arc<(Mutex<AskedState<K, T>>, Condvar)>,
}

struct AskedState<K, T> {
    /// The last answer: for which key, what, and when it came.
    answer: Option<(K, T, Instant)>,
    /// A question is on its way (one at a time: a call that hangs isn't asked again meanwhile).
    asking: bool,
}

impl<K, T> Default for Asked<K, T> {
    fn default() -> Self {
        Asked {
            shared: Arc::new((
                Mutex::new(AskedState {
                    answer: None,
                    asking: false,
                }),
                Condvar::new(),
            )),
        }
    }
}

impl<K, T> Asked<K, T>
where
    K: Clone + PartialEq + Send + 'static,
    T: Clone + PartialEq + Send + 'static,
{
    /// The answer for `key`. The last one is asked again (`ask`, on a thread of its own) once it
    /// is older than `every` or was for another key. None while there is none for `key` yet,
    /// after waiting at most `wait` for it (zero: not at all). `repaint` is repainted when an
    /// answer comes that differs from the last one, so the window shows it.
    pub fn get(
        &self,
        key: &K,
        every: Duration,
        wait: Duration,
        repaint: Option<&egui::Context>,
        ask: impl FnOnce(&K) -> T + Send + 'static,
    ) -> Option<T> {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = s
            .answer
            .as_ref()
            .is_some_and(|(k, _, at)| k == key && at.elapsed() < every);
        if !fresh && !s.asking {
            s.asking = true;
            let shared = self.shared.clone();
            let key = key.clone();
            let repaint = repaint.cloned();
            let started = std::thread::Builder::new()
                .name("asked".into())
                .spawn(move || {
                    let value = ask(&key);
                    let (lock, cv) = &*shared;
                    let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let changed = s
                        .answer
                        .as_ref()
                        .is_none_or(|(k, v, _)| *k != key || *v != value);
                    s.answer = Some((key, value, Instant::now()));
                    s.asking = false;
                    cv.notify_all();
                    drop(s);
                    if changed && let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                });
            if let Err(e) = started {
                tracing::warn!("asked: no thread to ask on: {e}");
                s.asking = false;
            }
        }
        let start = Instant::now();
        loop {
            if let Some((k, v, _)) = &s.answer
                && k == key
            {
                return Some(v.clone());
            }
            let left = wait.saturating_sub(start.elapsed());
            if left.is_zero() || !s.asking {
                return None;
            }
            s = cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// [`Asked::get`] for a window that shows what it read last until the new answer comes
    /// (#1218): the answer for `key`, else the last one, whichever key it was for. Only the call
    /// that asks the very first time waits, at most `first_wait`; the others never wait.
    pub fn get_or_last(
        &self,
        key: &K,
        every: Duration,
        first_wait: Duration,
        repaint: Option<&egui::Context>,
        ask: impl FnOnce(&K) -> T + Send + 'static,
    ) -> Option<T> {
        let lock = || self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        let last = || lock().answer.as_ref().map(|(_, v, _)| v.clone());
        let first = {
            let s = lock();
            s.answer.is_none() && !s.asking
        };
        let wait = if first { first_wait } else { Duration::ZERO };
        self.get(key, every, wait, repaint, ask).or_else(last)
    }

    /// The answer for `key` if it came; never asks, never waits (a window that asked once, with
    /// [`Asked::get`], and looks for its answer each frame after).
    pub fn answer(&self, key: &K) -> Option<T> {
        let s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        s.answer
            .as_ref()
            .filter(|(k, _, _)| k == key)
            .map(|(_, v, _)| v.clone())
    }
}

/// Run `cmd` quietly, waiting at most `limit` for it: true when it ran and succeeded. One that
/// runs out of time is ended. For a thread other than the window's.
pub fn run_within(cmd: &mut Command, limit: Duration) -> bool {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => wait_within(&mut child, limit).is_some_and(|s| s.success()),
        Err(e) => {
            tracing::debug!("{}: {e}", program_name(cmd));
            false
        }
    }
}

/// What `cmd` writes (its output, its exit status), waiting at most `limit` for it; None when it
/// couldn't start or ran out of time (it is then ended). Its errors are dropped.
// Windows reads what these ask (versions, settings) from the registry, without a program.
#[cfg_attr(windows, allow(dead_code))]
pub fn output_within(cmd: &mut Command, limit: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read as it writes, so a long output can't fill the pipe and stall it; a program it
    // started that keeps the pipe open can't hold this caller either (the reader is let go).
    let mut pipe = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("launch-output".into())
        .spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            let _ = tx.send(out);
        })
        .ok()?;
    let start = Instant::now();
    let status = wait_within(&mut child, limit)?;
    let left = limit.saturating_sub(start.elapsed()) + Duration::from_millis(100);
    let stdout = rx.recv_timeout(left).ok()?;
    Some(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

/// Wait for `child` at most `limit`: its exit status, or None when it ran out of time (it is
/// ended and reaped) or couldn't be waited for.
pub(crate) fn wait_within(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                tracing::warn!("a program still running after {limit:?} was ended");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(e) => {
                tracing::warn!("a program couldn't be waited for: {e}");
                return None;
            }
        }
    }
}

fn program_name(cmd: &Command) -> String {
    cmd.get_program().to_string_lossy().into_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A shell script `name` in `dir`.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        file
    }

    /// The script run by `sh`: executing a file just written can fail with "text file busy"
    /// while another test's thread starts a program (it holds the file open for a moment).
    fn sh(file: &Path) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(file);
        cmd
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mdm-launch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Long enough for anything a test waits for to come on the slowest runner: a deadlock
    /// breaker, never a time any check is about.
    const STUCK: Duration = Duration::from_secs(30);

    #[test]
    fn a_hanging_program_never_holds_the_caller_and_falls_back() {
        use std::sync::mpsc::{TryRecvError, channel};
        let dir = temp("hang");
        // A program that runs until the test lets it end (a minute at most), then fails: the
        // call can only be back before it ended if it didn't wait for it.
        let go = dir.join("go");
        let held = script(
            &dir,
            "held",
            &format!(
                "i=0; while [ ! -e '{}' ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; exit 3",
                go.display()
            ),
        );
        let (tx, fell_back) = channel();
        let (back, came_back) = channel();
        std::thread::spawn(move || {
            run_detached_with_fallback(sh(&held), Duration::from_secs(3600), move || {
                let _ = tx.send(());
            });
            let _ = back.send(());
        });
        came_back
            .recv_timeout(STUCK)
            .expect("the caller waited for the program");
        assert_eq!(
            fell_back.try_recv(),
            Err(TryRecvError::Empty),
            "ended early"
        );
        std::fs::write(&go, "").unwrap();
        fell_back
            .recv_timeout(STUCK)
            .expect("no fallback once it failed");
        // A program that never ends: ended at its limit, not after its hour, and the fallback runs.
        let hang = script(&dir, "hang", "exec sleep 3600");
        let limit = Duration::from_millis(500);
        let (tx, fell_back) = channel();
        let start = Instant::now();
        run_detached_with_fallback(sh(&hang), limit, move || {
            let _ = tx.send(Instant::now());
        });
        let at = fell_back
            .recv_timeout(STUCK)
            .expect("not ended at its limit");
        assert!(at - start >= limit, "ended before its limit");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Whether `cmd` fell back, once its run was over: the fallback holds the sender, so a run
    /// that ends without it drops the sender (`Disconnected`), however long it took. A fixed
    /// wait said "no fallback" for a run still under way on a busy machine.
    fn falls_back(cmd: Command) -> bool {
        use std::sync::mpsc::{RecvTimeoutError, channel};
        let (tx, rx) = channel();
        run_detached_with_fallback(cmd, STUCK, move || {
            let _ = tx.send(());
        });
        match rx.recv_timeout(STUCK) {
            Ok(()) => true,
            Err(RecvTimeoutError::Disconnected) => false,
            Err(RecvTimeoutError::Timeout) => panic!("the run never ended"),
        }
    }

    #[test]
    fn a_program_that_succeeds_has_no_fallback_and_one_that_fails_has_one() {
        let dir = temp("exit");
        for (body, expected) in [("exit 0", false), ("exit 3", true)] {
            let prog = script(&dir, "prog", body);
            assert_eq!(falls_back(sh(&prog)), expected, "{body}");
        }
        assert!(
            falls_back(Command::new(dir.join("no-such-program"))),
            "missing"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn output_within_reads_the_output_and_gives_up_on_a_hang() {
        let dir = temp("output");
        let say = script(&dir, "say", "echo blue");
        let out = output_within(&mut sh(&say), Duration::from_secs(5)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "blue");
        // A program that runs for an hour: given up on (ended) long before. Any bound far under
        // the hour proves it; 3 s was passed now and then on a busy machine.
        let hang = script(&dir, "hang", "exec sleep 3600");
        let start = Instant::now();
        assert!(output_within(&mut sh(&hang), Duration::from_millis(300)).is_none());
        assert!(start.elapsed() < STUCK, "{:?}", start.elapsed());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod asked_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const NO_WAIT: Duration = Duration::ZERO;

    /// Long enough for anything a test waits for to come on the slowest runner, and the most a
    /// hanging call hangs: a deadlock breaker, never a time any check is about.
    const STUCK: Duration = Duration::from_secs(30);

    /// An answer kept this long is fresh for the whole test.
    const FRESH: Duration = Duration::from_secs(3600);

    /// Counts the calls [`Asked::get`] didn't make, as they are dropped: one it doesn't ask is
    /// dropped before it is back, on the caller's thread; one it asks goes to the thread that
    /// asks it, which forgets it. So a call asked again shows at once, not once its thread runs.
    struct NotAsked(Arc<AtomicU32>);

    impl Drop for NotAsked {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_call_that_hangs_never_holds_the_window_and_is_asked_once() {
        let asked = Asked::<(), bool>::default();
        let not_asked = Arc::new(AtomicU32::new(0));
        // The call hangs until the test lets it go: a window that waited for it would get its
        // answer, which can't come before then.
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let ask = || {
            let (unasked, gate) = (NotAsked(not_asked.clone()), gate.clone());
            move |_: &()| {
                std::mem::forget(unasked);
                let _ = gate.lock().unwrap().recv_timeout(STUCK);
                true
            }
        };
        let wait = Duration::from_millis(100);
        let start = Instant::now();
        let first = asked.get(&(), Duration::ZERO, wait, None, ask());
        // Waited what it said, not for the call.
        assert_eq!(first, None);
        assert!(start.elapsed() >= wait, "{:?}", start.elapsed());
        assert_eq!(not_asked.load(Ordering::SeqCst), 0, "not asked");
        // The next frames: no answer and no wait, and the hanging call isn't asked again
        // meanwhile.
        for _ in 0..50 {
            assert_eq!(asked.get(&(), Duration::ZERO, NO_WAIT, None, ask()), None);
        }
        assert_eq!(not_asked.load(Ordering::SeqCst), 50, "asked again");
        // Let go, its answer comes, from that one call.
        go.send(()).unwrap();
        assert_eq!(asked.get(&(), FRESH, STUCK, None, ask()), Some(true));
        assert_eq!(not_asked.load(Ordering::SeqCst), 51, "asked again");
    }

    /// #1218: a window that shows what it read last. A slow read never holds it (only the very
    /// first is waited for, as long as it says), the last answer stays shown while a new key's
    /// read is under way, and a read under way is never asked twice.
    #[test]
    fn the_last_answer_shows_while_a_slow_read_is_under_way() {
        let asked = Asked::<u64, &'static str>::default();
        let reads = Arc::new(AtomicU32::new(0));
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let read = |answer: &'static str| {
            let (reads, gate) = (reads.clone(), gate.clone());
            move |_: &u64| {
                reads.fetch_add(1, Ordering::SeqCst);
                let _ = gate.lock().unwrap().recv_timeout(STUCK);
                answer
            }
        };
        let first_wait = Duration::from_millis(50);
        // Nothing read yet: waited `first_wait`, not for the read ("Checking...").
        let start = Instant::now();
        assert_eq!(
            asked.get_or_last(&0, FRESH, first_wait, None, read("a")),
            None
        );
        let took = start.elapsed();
        assert!(took >= first_wait && took < STUCK / 2, "{took:?}");
        // The next frames don't wait at all, and the read isn't asked again: offered a wait as
        // long as STUCK, a frame that took it would show (a busy machine takes some time for
        // any call; a wait as short as the first one was found in that time now and then).
        for _ in 0..20 {
            let start = Instant::now();
            assert_eq!(asked.get_or_last(&0, FRESH, STUCK, None, read("a")), None);
            assert!(start.elapsed() < STUCK / 2, "{:?}", start.elapsed());
        }
        go.send(()).unwrap();
        let start = Instant::now();
        while asked.get_or_last(&0, FRESH, NO_WAIT, None, read("a")) != Some("a") {
            assert!(start.elapsed() < STUCK, "never came");
            std::thread::sleep(Duration::from_millis(5));
        }
        // A new key (Set it up finished): the last answer at once, while its read hangs (offered
        // a wait it must never take, as above).
        for _ in 0..20 {
            let start = Instant::now();
            assert_eq!(
                asked.get_or_last(&1, FRESH, STUCK, None, read("b")),
                Some("a")
            );
            assert!(start.elapsed() < STUCK / 2, "{:?}", start.elapsed());
        }
        // Its read started (on its own thread, a moment later), and only it.
        let start = Instant::now();
        while reads.load(Ordering::SeqCst) < 2 {
            assert!(start.elapsed() < STUCK, "never asked");
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..20 {
            asked.get_or_last(&1, FRESH, first_wait, None, read("b"));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::SeqCst), 2, "one read at a time");
        go.send(()).unwrap();
        let start = Instant::now();
        while asked.get_or_last(&1, FRESH, NO_WAIT, None, read("b")) != Some("b") {
            assert!(start.elapsed() < STUCK, "never came");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn pending_then_done_then_asked_again_when_stale() {
        let asked = Asked::<(), u32>::default();
        let n = Arc::new(AtomicU32::new(0));
        let ask = |n: &Arc<AtomicU32>| {
            let n = n.clone();
            move |_: &()| n.fetch_add(1, Ordering::SeqCst) + 1
        };
        // A quick answer is waited for.
        assert_eq!(asked.get(&(), FRESH, STUCK, None, ask(&n)), Some(1));
        // Fresh (younger than `every`): kept, not asked again.
        assert_eq!(asked.get(&(), FRESH, NO_WAIT, None, ask(&n)), Some(1));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // Stale (older than `every`, which any answer is than zero): the last answer at once, a
        // new one asked meanwhile.
        assert_eq!(
            asked.get(&(), Duration::ZERO, NO_WAIT, None, ask(&n)),
            Some(1)
        );
        let start = Instant::now();
        while asked.get(&(), FRESH, NO_WAIT, None, ask(&n)) != Some(2) {
            assert!(start.elapsed() < STUCK, "never asked again");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn another_key_has_no_answer_until_its_own() {
        let asked = Asked::<String, usize>::default();
        let len = |k: &String| k.len();
        let (wait, long) = (STUCK, FRESH);
        assert_eq!(asked.get(&"ab".into(), long, wait, None, len), Some(2));
        assert_eq!(asked.get(&"abcd".into(), long, wait, None, len), Some(4));
        // An answer that is slow for the new key: none for it (never the old key's).
        let slow = |k: &String| {
            std::thread::sleep(Duration::from_millis(300));
            k.len()
        };
        assert_eq!(asked.get(&"x".into(), long, NO_WAIT, None, slow), None);
        let start = Instant::now();
        while asked.get(&"x".into(), long, NO_WAIT, None, slow).is_none() {
            assert!(start.elapsed() < STUCK);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(asked.get(&"x".into(), long, NO_WAIT, None, slow), Some(1));
    }
}
