//! The app's side: start the monitor, connect to it, tell it what it needs to know, ask it for
//! dumps. (Catching native crashes is the app's platform layer, win32.rs / unix.rs: it hands
//! [`Reporter::dump`] to crash-handler.)

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::protocol::{AppState, Freeze, Panic, Session, kind};

/// How long the window thread tries the connection itself. The first moments are the riskiest
/// stretch (the renderer starting, a graphics driver loading): a crash there is reported when
/// the monitor answers promptly, as it usually does. Past this the waiting goes to a thread of
/// its own, and the window goes on ([`CONNECT_FOR`] keeps the try going beside it).
const FIRST_TRY: Duration = Duration::from_millis(500);
/// How long the connection is tried in all. The window never waits past [`FIRST_TRY`], so a
/// slow monitor (a cold disk at log-on) is given a minute; past it MDM runs on without crash
/// reports, as when the monitor couldn't start at all.
const CONNECT_FOR: Duration = Duration::from_secs(60);
/// Between two tries to connect, doubling up to [`RETRY_MAX`].
const RETRY: Duration = Duration::from_millis(25);
const RETRY_MAX: Duration = Duration::from_millis(1000);
/// The most of the log's last lines sent with a panic or a freeze, about the size of a panic's
/// backtrace (one message over the socket).
const LOG_BYTES: usize = 24 * 1024;
/// The same refusal this many times in a row (about twenty seconds with the back-off) is the
/// system's answer about this socket, not a slow monitor: the try is given up instead of ending
/// the same way a minute later.
const SAME_REFUSALS: u32 = 20;

/// The app's connection to its monitor.
pub struct Reporter {
    /// The connection, published once it is made. Read without a lock, so a crash handler can
    /// ask for its dump while any other thread holds any other lock.
    client: OnceLock<minidumper::Client>,
    monitor: Mutex<Option<Child>>,
    /// What was sent before the connection, and whether the app has quit normally. This lock
    /// keeps the order of what reaches the monitor: the connection's first sends go out under
    /// it, before the client is published, and `session` and `state` store and send under it.
    kept: Mutex<Kept>,
    /// Crash reports are off for this session (the monitor could never be reached): nothing
    /// more is tried.
    off: AtomicBool,
}

/// What was sent before the connection (sent again when it is made), and whether the app quit
/// normally.
#[derive(Default)]
struct Kept {
    session: Option<Session>,
    state: Option<AppState>,
    clean: bool,
}

/// How the tries to connect have gone so far, carried from the window thread's first try to
/// the connector thread's.
struct Tries {
    started: Instant,
    /// The wait before the next try, doubling up to [`RETRY_MAX`].
    wait: Duration,
    last_refusal: Option<std::io::ErrorKind>,
    same_refusals: u32,
    /// The last try's error, for the log when the time is up.
    error: String,
}

impl Tries {
    fn new() -> Tries {
        Tries {
            started: Instant::now(),
            wait: RETRY,
            last_refusal: None,
            same_refusals: 0,
            error: String::new(),
        }
    }
}

impl Reporter {
    /// Start `exe --crash-monitor <socket> --data <dir> --parent <pid>` and connect to it: the
    /// window thread tries for a moment, then a thread of its own takes the try over. None when
    /// it can't start: MDM runs on without crash reports.
    pub fn start(exe: &Path, data: &Path) -> Option<Arc<Reporter>> {
        let socket = crate::monitor::socket_path(std::process::id());
        let mut cmd = Command::new(exe);
        cmd.arg(crate::MONITOR_ARG)
            .arg(&socket)
            .arg("--data")
            .arg(data)
            .arg("--parent")
            .arg(std::process::id().to_string());
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("crash monitor didn't start: {e}");
                return None;
            }
        };
        let reporter = Arc::new(Reporter {
            client: OnceLock::new(),
            monitor: Mutex::new(Some(child)),
            kept: Mutex::new(Kept::default()),
            off: AtomicBool::new(false),
        });
        Self::connect(&reporter, socket);
        Some(reporter)
    }

    /// Connect to the monitor: on the caller's thread for [`FIRST_TRY`], then, if it hasn't
    /// answered yet, from a thread of its own until [`CONNECT_FOR`] is up. The first try's end
    /// only hands the try over: a monitor that listens past it (a slow start) is still reached.
    fn connect(reporter: &Arc<Self>, socket: PathBuf) {
        let mut tries = Tries::new();
        let deadline = tries.started + CONNECT_FOR;
        if reporter.connect_until(&socket, tries.started + FIRST_TRY, &mut tries) {
            Self::connect_in_background(reporter, socket, deadline, tries);
        }
    }

    /// Connect to the monitor from a thread of its own. It listens a moment after it starts,
    /// and a slow start (or a socket the system refuses for a while: a Windows Server was seen
    /// refusing MDM's for five seconds with os error 10022, #476) must neither hold the window
    /// back nor take the monitor away: it may answer later, and what was sent meanwhile goes
    /// over then. Past `deadline` it is given up.
    fn connect_in_background(
        reporter: &Arc<Self>,
        socket: PathBuf,
        deadline: Instant,
        mut tries: Tries,
    ) {
        let connector = Arc::clone(reporter);
        let spawned = std::thread::Builder::new()
            .name("monitor connector".into())
            .spawn(move || {
                if connector.connect_until(&socket, deadline, &mut tries) {
                    connector.give_up(&format!("crash monitor didn't answer: {}", tries.error));
                }
            });
        if let Err(e) = spawned {
            reporter.give_up(&format!("no thread to connect with ({e})"));
        }
    }

    /// Try to connect until it answers, the app quits, the monitor goes on its own, or the
    /// system refuses the same way once too often: false then. True when `deadline` came first,
    /// the monitor still not answering: the caller hands the try over or gives up. Never waits
    /// past `deadline`.
    fn connect_until(&self, socket: &Path, deadline: Instant, tries: &mut Tries) -> bool {
        loop {
            if self.off() || self.is_clean() {
                return false;
            }
            match minidumper::Client::with_name(minidumper::SocketName::Path(socket)) {
                Ok(client) => {
                    // What was sent before the connection goes again, under the kept lock and
                    // before the client is published: a `session` or `state` arriving while this
                    // runs waits for the lock, so what reaches the monitor keeps its order, and
                    // the newer of two sessions is the one sent last.
                    let kept = self.lock_kept();
                    if kept.clean {
                        // The app quit as we connected: a connection now would look to the
                        // monitor like a fresh app that is about to die.
                        return false;
                    }
                    if let Some(s) = &kept.session {
                        Self::send_json(&client, kind::SESSION, s);
                    }
                    if let Some(s) = &kept.state {
                        Self::send_json(&client, kind::STATE, s);
                    }
                    let _ = self.client.set(client);
                    drop(kept);
                    tracing::info!(
                        "crash monitor {} listening, {} ms after start",
                        self.monitor_pid().unwrap_or_default(),
                        tries.started.elapsed().as_millis()
                    );
                    return false;
                }
                Err(e) => {
                    // A monitor that couldn't open its socket has already said why in its own
                    // log and gone: don't try for a process that is already gone.
                    if self.monitor_exited() {
                        self.give_up("crash monitor exited before it listened");
                        return false;
                    }
                    // The same refusal over and over is the system's answer about this socket:
                    // trying the whole minute would only end the same way. The socket simply
                    // not being there yet (the monitor starting) is not a refusal, and doesn't
                    // count: that is what the minute is for.
                    let refusal = refusal_of(&e);
                    tries.same_refusals =
                        refusals_after(tries.same_refusals, &tries.last_refusal, &refusal);
                    tries.last_refusal = refusal;
                    tries.error = e.to_string();
                    if tries.same_refusals >= SAME_REFUSALS {
                        self.give_up(&format!("crash monitor keeps being refused: {e}"));
                        return false;
                    }
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return true;
                    }
                    // Up to the deadline, not past it: the window thread is held no longer
                    // than its first try, and the last try is made at the deadline.
                    std::thread::sleep(tries.wait.min(left));
                    tries.wait = (tries.wait * 2).min(RETRY_MAX);
                }
            }
        }
    }

    /// Crash reports are off for the session: the monitor can never be reached, or is gone. It
    /// is ended, so no second MDM process stays behind.
    fn give_up(&self, why: &str) {
        tracing::warn!("crash reports are off for this session ({why})");
        self.off.store(true, Ordering::Release);
        if let Some(mut child) = self.lock_monitor().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The monitor's process id (Linux lets only it read this process's memory: ptrace).
    pub fn monitor_pid(&self) -> Option<u32> {
        self.lock_monitor().as_ref().map(Child::id)
    }

    /// Connected to the monitor? (The connection is made first on the window thread, then from
    /// a thread of its own.)
    pub fn connected(&self) -> bool {
        self.client.get().is_some()
    }

    /// The app has quit normally: nothing more is sent, so a connection made after this can't
    /// be taken for a fresh app, and a monitor that never connected is ended at once instead of
    /// lingering beside a closed app.
    pub fn clean(&self) {
        self.lock_kept().clean = true;
        if let Some(client) = self.client.get() {
            Self::send_json(client, kind::CLEAN, &());
            let _ = client.ping();
        } else if let Some(mut child) = self.lock_monitor().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn session(&self, s: &Session) {
        let mut kept = self.lock_kept();
        kept.session = Some(s.clone());
        if let Some(client) = self.client.get() {
            Self::send_json(client, kind::SESSION, s);
        }
    }

    /// Send the state when it changed since the last time.
    pub fn state(&self, s: &AppState) {
        let mut kept = self.lock_kept();
        if kept.state.as_ref() != Some(s) {
            kept.state = Some(s.clone());
            if let Some(client) = self.client.get() {
                Self::send_json(client, kind::STATE, s);
            }
        }
    }

    pub fn panic(&self, p: &Panic) {
        if let Some(client) = self.client.get() {
            Self::send_json(client, kind::PANIC, p);
            Self::send_log(client);
            // The messages are in before the dump request (on macOS they travel separately).
            let _ = client.ping();
        }
    }

    pub fn freeze(&self, f: &Freeze) {
        if let Some(client) = self.client.get() {
            Self::send_json(client, kind::FREEZE, f);
            Self::send_log(client);
            let _ = client.ping();
        }
    }

    /// The last lines logged, from memory, for the report (the disk may still hold the file's,
    /// #1354): the newest that fit in [`LOG_BYTES`].
    fn send_log(client: &minidumper::Client) {
        let mut lines = crate::logs::recent();
        let mut bytes = 0;
        let keep = lines
            .iter()
            .rev()
            .take_while(|l| {
                bytes += l.len() + 4;
                bytes <= LOG_BYTES
            })
            .count();
        lines.drain(..lines.len() - keep);
        if !lines.is_empty() {
            Self::send_json(client, kind::LOG, &lines);
        }
    }

    pub fn recovered(&self) {
        if let Some(client) = self.client.get() {
            Self::send_json(client, kind::RECOVERED, &());
        }
    }

    /// Ask the monitor for a dump of this process, `context` being the thread that asks (from
    /// crash-handler). True when it was written. Takes no lock: a fault can come while any
    /// other thread is anywhere else.
    pub fn dump(&self, context: &crash_handler::CrashContext) -> bool {
        self.client
            .get()
            .is_some_and(|client| client.request_dump(context).is_ok())
    }

    fn send_json(client: &minidumper::Client, kind: u32, body: &impl serde::Serialize) {
        if let Ok(b) = serde_json::to_vec(body) {
            let _ = client.send_message(kind, b);
        }
    }

    fn lock_monitor(&self) -> MutexGuard<'_, Option<Child>> {
        self.monitor.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_clean(&self) -> bool {
        self.lock_kept().clean
    }

    fn off(&self) -> bool {
        self.off.load(Ordering::Acquire)
    }

    /// The monitor is gone on its own (it couldn't open its socket, or the computer took it).
    fn monitor_exited(&self) -> bool {
        self.lock_monitor()
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
    }
}

/// What a failed try means for [`SAME_REFUSALS`]: `None` when the socket simply isn't there yet
/// (the monitor is starting: nothing at its path, or a path nobody accepts — what the minute is
/// for), or the system's refusal of the socket, by kind, to tell one kind from another.
fn refusal_of(e: &minidumper::Error) -> Option<std::io::ErrorKind> {
    match e {
        minidumper::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => None,
            kind => Some(kind),
        },
        _ => Some(std::io::ErrorKind::Other),
    }
}

/// The count of refusals in a row, after one more: the same refusal as last time counts up
/// ([`SAME_REFUSALS`] of them and the try is given up), another kind starts over, and the
/// socket not being there yet was never one.
fn refusals_after(
    count: u32,
    last: &Option<std::io::ErrorKind>,
    refusal: &Option<std::io::ErrorKind>,
) -> u32 {
    match (refusal, last) {
        (Some(r), Some(l)) if r == l => count + 1,
        (Some(_), _) => 1,
        (None, _) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    /// A long-lived program in the monitor's place, so a test can end it (a real one exits only
    /// with the app).
    fn slow_program() -> &'static str {
        if cfg!(windows) { "ping.exe" } else { "sleep" }
    }

    fn slow_program_args() -> &'static [&'static str] {
        if cfg!(windows) {
            &["-n", "30", "127.0.0.1"]
        } else {
            &["30"]
        }
    }

    /// A monitor starting is not the system refusing the socket: the missing file and the file
    /// nobody accepts yet don't count, the real refusals do, and the count runs to the give-up.
    #[test]
    fn a_slow_start_is_not_a_refusal() {
        fn by_kind(kind: std::io::ErrorKind) -> minidumper::Error {
            minidumper::Error::Io(std::io::Error::from(kind))
        }
        fn by_raw(raw: i32) -> minidumper::Error {
            minidumper::Error::Io(std::io::Error::from_raw_os_error(raw))
        }
        let refused = std::io::ErrorKind::InvalidInput;
        // The monitor starting: no file at its path yet (os error 2), or one nobody accepts yet.
        assert_eq!(refusal_of(&by_raw(2)), None);
        assert_eq!(refusal_of(&by_kind(std::io::ErrorKind::NotFound)), None);
        assert_eq!(
            refusal_of(&by_kind(std::io::ErrorKind::ConnectionRefused)),
            None
        );
        // The system refusing the socket (#476's machine: os error 10022, an invalid argument).
        assert_eq!(refusal_of(&by_kind(refused)), Some(refused));
        assert!(refusal_of(&by_raw(10022)).is_some());
        // None of the starts above count toward the give-up…
        assert_eq!(refusals_after(0, &None, &None), 0);
        // …a first refusal does…
        assert_eq!(refusals_after(0, &None, &Some(refused)), 1);
        // …the same one counts up to it…
        assert_eq!(
            refusals_after(SAME_REFUSALS - 1, &Some(refused), &Some(refused)),
            SAME_REFUSALS
        );
        // …another kind starts over, and a start that comes back resets it.
        assert_eq!(
            refusals_after(
                SAME_REFUSALS - 1,
                &Some(refused),
                &Some(std::io::ErrorKind::TimedOut)
            ),
            1
        );
        assert_eq!(refusals_after(SAME_REFUSALS - 1, &Some(refused), &None), 0);
    }

    /// A monitor that couldn't open its socket exits at once: `start` answers within its first
    /// try on the window thread (the window is not held for the old five seconds), and nothing
    /// ever connects.
    #[test]
    fn a_monitor_that_exits_before_listening_is_not_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        // A system program that starts and exits at once; the monitor's arguments mean nothing
        // to it.
        let exe = if cfg!(windows) {
            "whoami.exe"
        } else {
            // macOS has no /bin/true: found on the PATH.
            "true"
        };
        let started = Instant::now();
        let reporter =
            Reporter::start(Path::new(exe), dir.path()).expect("the monitor program starts");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited {:?} to start the monitor",
            started.elapsed()
        );
        assert!(!reporter.connected());
        assert!(
            reporter.off(),
            "a gone monitor is not kept being waited for"
        );
    }

    /// What is sent before the connection is made is kept for it: a monitor that listens late
    /// (a slow start at log-on, or a socket the system refuses for a while) misses nothing, and
    /// what arrives keeps its order.
    #[test]
    fn what_is_sent_before_the_connection_is_kept_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("late.sock");
        /// What the monitor's side has received, as (kind, body).
        type Received = Vec<(u32, Vec<u8>)>;
        // The monitor's side, beside the test: it records what arrives.
        struct Record(Arc<std::sync::Mutex<Received>>);
        impl minidumper::ServerHandler for Record {
            fn create_minidump_file(
                &self,
            ) -> Result<(std::fs::File, std::path::PathBuf), std::io::Error> {
                Err(std::io::Error::other("no dumps are asked for here"))
            }
            fn on_minidump_created(
                &self,
                _: Result<minidumper::MinidumpBinary, minidumper::Error>,
            ) -> minidumper::LoopAction {
                minidumper::LoopAction::Continue
            }
            fn on_message(&self, kind: u32, buffer: Vec<u8>) {
                self.0.lock().unwrap().push((kind, buffer));
            }
        }
        let seen: Arc<std::sync::Mutex<Received>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        // A reporter with no monitor process of its own: only its connection is tested. The
        // session and the state are sent while nothing is listening, the way they are when the
        // monitor is slow.
        let reporter = Arc::new(Reporter {
            client: OnceLock::new(),
            monitor: Mutex::new(None),
            kept: Mutex::new(Kept::default()),
            off: AtomicBool::new(false),
        });
        let session = Session {
            version: "test".into(),
            ..Default::default()
        };
        reporter.session(&session);
        let state = AppState {
            downloads: 2,
            running: 1,
            ..Default::default()
        };
        reporter.state(&state);
        assert!(!reporter.connected());
        Reporter::connect_in_background(
            &reporter,
            socket.clone(),
            Instant::now() + CONNECT_FOR,
            Tries::new(),
        );
        // The monitor comes up late, past the first refusals the connector has already met.
        std::thread::sleep(Duration::from_millis(200));
        let mut server =
            minidumper::Server::with_name(minidumper::SocketName::Path(&socket)).unwrap();
        {
            let seen = Arc::clone(&seen);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                let _ = server.run(Box::new(Record(seen)), &shutdown, None);
            });
        }
        let limit = Instant::now() + Duration::from_secs(5);
        while !reporter.connected() && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reporter.connected(), "the connection never came");
        let arrived = |wanted: u32| seen.lock().unwrap().iter().any(|(k, _)| *k == wanted);
        // The messages reach the monitor once it has read them (a moment after the connect).
        let limit = Instant::now() + Duration::from_secs(5);
        while !arrived(kind::SESSION) && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            arrived(kind::SESSION),
            "the session didn't reach the monitor"
        );
        let limit = Instant::now() + Duration::from_secs(5);
        while !arrived(kind::STATE) && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(arrived(kind::STATE), "the state didn't reach the monitor");
        // And what is sent once connected goes once, not again with the kept ones.
        reporter.clean();
        let count = |wanted: u32| {
            seen.lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k == wanted)
                .count()
        };
        assert_eq!(count(kind::SESSION), 1);
        assert_eq!(count(kind::STATE), 1);
        shutdown.store(true, AtomicOrdering::Relaxed);
    }

    /// A monitor that never answers is ended when the time is up, and nothing is left behind.
    #[test]
    fn a_monitor_that_never_answers_is_ended_when_the_time_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("stuck.sock");
        // A program that stays alive and never listens, in the monitor's place.
        let child = std::process::Command::new(slow_program())
            .args(slow_program_args())
            .spawn()
            .expect("the stand-in monitor starts");
        let reporter = Arc::new(Reporter {
            client: OnceLock::new(),
            monitor: Mutex::new(Some(child)),
            kept: Mutex::new(Kept::default()),
            off: AtomicBool::new(false),
        });
        let started = Instant::now();
        Reporter::connect_in_background(
            &reporter,
            socket,
            started + Duration::from_millis(300),
            Tries::new(),
        );
        while !reporter.off() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reporter.off(), "the give-up is not remembered");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited {:?} past the deadline",
            started.elapsed()
        );
        assert!(!reporter.connected());
        assert!(
            reporter.monitor_pid().is_none(),
            "the monitor was not ended"
        );
    }

    /// A monitor that listens only after the window thread's first try is still connected to,
    /// from the connector thread: the first try's end hands the try over, it gives nothing up.
    /// (It did give up, and ended the monitor: a monitor slower than half a second to start, a
    /// cold start or a busy computer, left the whole session without crash reports.)
    #[test]
    fn a_monitor_slower_than_the_first_try_is_still_connected_to() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("slow.sock");
        // The monitor's process, alive and not listening yet.
        let child = std::process::Command::new(slow_program())
            .args(slow_program_args())
            .spawn()
            .expect("the stand-in monitor starts");
        let reporter = Arc::new(Reporter {
            client: OnceLock::new(),
            monitor: Mutex::new(Some(child)),
            kept: Mutex::new(Kept::default()),
            off: AtomicBool::new(false),
        });
        let started = Instant::now();
        Reporter::connect(&reporter, socket.clone());
        // The window thread is held for its first try only, then goes on.
        let held = started.elapsed();
        assert!(
            held >= FIRST_TRY && held < FIRST_TRY + Duration::from_millis(1500),
            "the window thread was held {held:?}"
        );
        assert!(!reporter.connected());
        assert!(!reporter.off(), "the first try's end gave the monitor up");
        assert!(
            reporter.monitor_pid().is_some(),
            "the slow monitor was ended"
        );
        // The monitor listens a second after the first try ended.
        std::thread::sleep(Duration::from_secs(1));
        let mut server =
            minidumper::Server::with_name(minidumper::SocketName::Path(&socket)).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        {
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                let _ = server.run(Box::new(Ignore), &shutdown, None);
            });
        }
        let limit = Instant::now() + Duration::from_secs(10);
        while !reporter.connected() && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            reporter.connected(),
            "the slow monitor was never connected to"
        );
        assert!(!reporter.off());
        shutdown.store(true, AtomicOrdering::Relaxed);
        if let Some(mut child) = reporter.lock_monitor().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// A monitor's side that only answers (pings, connections).
    struct Ignore;
    impl minidumper::ServerHandler for Ignore {
        fn create_minidump_file(
            &self,
        ) -> Result<(std::fs::File, std::path::PathBuf), std::io::Error> {
            Err(std::io::Error::other("no dumps are asked for here"))
        }
        fn on_minidump_created(
            &self,
            _: Result<minidumper::MinidumpBinary, minidumper::Error>,
        ) -> minidumper::LoopAction {
            minidumper::LoopAction::Continue
        }
        fn on_message(&self, _: u32, _: Vec<u8>) {}
    }

    /// A clean exit ends a monitor that never connected, and nothing is sent after it: a
    /// connection made then would look to the monitor like a fresh app about to die.
    #[test]
    fn a_clean_exit_ends_a_monitor_that_never_connected_and_sends_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("clean.sock");
        /// What the monitor's side has received, as (kind, body).
        type Received = Vec<(u32, Vec<u8>)>;
        struct Record(Arc<std::sync::Mutex<Received>>);
        impl minidumper::ServerHandler for Record {
            fn create_minidump_file(
                &self,
            ) -> Result<(std::fs::File, std::path::PathBuf), std::io::Error> {
                Err(std::io::Error::other("no dumps are asked for here"))
            }
            fn on_minidump_created(
                &self,
                _: Result<minidumper::MinidumpBinary, minidumper::Error>,
            ) -> minidumper::LoopAction {
                minidumper::LoopAction::Continue
            }
            fn on_message(&self, kind: u32, buffer: Vec<u8>) {
                self.0.lock().unwrap().push((kind, buffer));
            }
        }
        let seen: Arc<std::sync::Mutex<Received>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut server =
            minidumper::Server::with_name(minidumper::SocketName::Path(&socket)).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        {
            let seen = Arc::clone(&seen);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                let _ = server.run(Box::new(Record(seen)), &shutdown, None);
            });
        }
        // A monitor in the monitor's place, alive and never listening, and a session sent while
        // nothing is connected.
        let child = std::process::Command::new(slow_program())
            .args(slow_program_args())
            .spawn()
            .expect("the stand-in monitor starts");
        let reporter = Arc::new(Reporter {
            client: OnceLock::new(),
            monitor: Mutex::new(Some(child)),
            kept: Mutex::new(Kept::default()),
            off: AtomicBool::new(false),
        });
        let session = Session {
            version: "test".into(),
            ..Default::default()
        };
        reporter.session(&session);
        // The app quits normally: the never-connected monitor is ended at once.
        reporter.clean();
        assert!(
            reporter.monitor_pid().is_none(),
            "the never-connected monitor was left behind"
        );
        // The connector that was on its way stops without connecting or sending.
        let timed_out = reporter.connect_until(
            &socket,
            Instant::now() + Duration::from_secs(1),
            &mut Tries::new(),
        );
        assert!(!timed_out, "the connector kept trying after a clean exit");
        assert!(!reporter.connected(), "connected after a clean exit");
        assert!(
            seen.lock().unwrap().is_empty(),
            "something was sent after the clean exit"
        );
        shutdown.store(true, AtomicOrdering::Relaxed);
    }
}
