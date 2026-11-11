//! Crash and freeze reports, the app's side (mdm-crash has the rest). At start MDM launches its
//! monitor (itself with `--crash-monitor`), which writes a minidump when MDM:
//! - crashes natively (win32.rs / unix.rs catch it through crash-handler),
//! - panics (the hook here sends the message, then asks for the dump),
//! - or freezes: a watchdog thread sees a frame of the window run past [`FREEZE_AFTER`] and asks
//!   for a dump of every thread while MDM is still stuck (a deadlock, an endless loop). A frame
//!   is two timed stretches: the app's logic (commands and the clock, which egui runs alone while
//!   the main window is minimized) and its UI. The waits between frames aren't timed.
//!
//! The monitor then shows the report window (crashui.rs). Without a monitor (it couldn't start)
//! MDM runs on, and a panic still leaves a text report.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mdm_crash::client::Reporter;
use mdm_crash::protocol::{AppState, Busy, Freeze, Panic, Session};

/// A frame running this long is a freeze (Windows calls a window "Not responding" after 5 s).
pub const FREEZE_AFTER: Duration = Duration::from_secs(8);

static REPORTER: OnceLock<Arc<Reporter>> = OnceLock::new();
/// The session as sent, to send again when it learns more (what draws).
static SESSION: std::sync::Mutex<Option<Session>> = std::sync::Mutex::new(None);
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// When the current stretch of a frame started (ms since EPOCH, + 1); 0 between them.
static FRAME_START: AtomicU64 = AtomicU64::new(0);
/// Which stretch that is ([`LOGIC`] or [`UI`]), for the log.
static PART: AtomicU8 = AtomicU8::new(UI);
const LOGIC: u8 = 0;
const UI: u8 = 1;
/// A freeze was reported and the window hasn't answered since.
static FROZEN: AtomicBool = AtomicBool::new(false);
/// A panic was reported: later ones (a burst on worker threads) are logged, not dumped again.
static PANICKED: AtomicBool = AtomicBool::new(false);
/// The window thread (the dump's id for it; 0 where it can't be known).
static UI_THREAD: OnceLock<u32> = OnceLock::new();

thread_local! {
    /// The windows drawn this frame and last frame (ids), for the log and the report.
    static WINDOWS: RefCell<(BTreeSet<String>, BTreeSet<String>)> =
        RefCell::new(Default::default());
}

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Start the monitor and catch crashes, panics and freezes. Called on the window thread, after
/// this process became MDM's instance.
pub fn start(data: &Path, session: Session) {
    let _ = EPOCH.set(Instant::now());
    #[cfg(windows)]
    let _ = UI_THREAD.set(crate::win32::thread_id());
    #[cfg(unix)]
    let _ = UI_THREAD.set(crate::unix::thread_id());
    let Some(exe) = std::env::current_exe().ok() else {
        return;
    };
    let Some(reporter) = Reporter::start(&exe, data) else {
        return;
    };
    reporter.session(&session);
    if let Ok(mut s) = SESSION.lock() {
        *s = Some(session);
    }
    let reporter = Arc::new(reporter);
    #[cfg(windows)]
    let attached = crate::win32::attach_crash_handler(reporter.clone());
    #[cfg(unix)]
    let attached = crate::unix::attach_crash_handler(reporter.clone());
    if !attached {
        tracing::warn!("crash reports: native crashes won't be caught");
    }
    let _ = REPORTER.set(reporter);
    hook_panics();
    let _ = std::thread::Builder::new()
        .name("freeze watchdog".into())
        .spawn(watchdog);
    tracing::info!("crash reports on");
}

/// What draws the windows, for the report (renderer.rs, once the window started).
pub fn renderer(detail: &str) {
    let (Some(r), Ok(mut s)) = (REPORTER.get(), SESSION.lock()) else {
        return;
    };
    if let Some(s) = s.as_mut() {
        s.renderer = detail.to_owned();
        r.session(s);
    }
}

/// Ask the monitor for a dump of every thread now.
fn dump_now() -> bool {
    #[cfg(windows)]
    return crate::win32::dump_now();
    #[cfg(unix)]
    return crate::unix::dump_now();
}

/// A panic: its message and Rust's backtrace to the monitor, then a dump; then the default hook
/// (the message on stderr) and the panic goes on (on the window thread it ends MDM).
fn hook_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let p = Panic {
            message: info
                .payload_as_str()
                .unwrap_or("(a panic without a message)")
                .to_owned(),
            location: info.location().map(|l| l.to_string()).unwrap_or_default(),
            thread: std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        };
        tracing::error!("panic: {} at {} ({})", p.message, p.location, p.thread);
        // In the file before the report reads the log's end (the log writes on its own thread).
        mdm_crash::logs::flush(LOG_FOR_A_REPORT);
        if !panic_reported(CLOSING.load(Ordering::Acquire), &p.thread) {
            default(info);
            return;
        }
        if let Some(r) = REPORTER.get()
            && !PANICKED.swap(true, Ordering::AcqRel)
        {
            r.panic(&p);
            dump_now();
        }
        default(info);
    }));
}

/// The window thread starts a timed stretch.
fn stretch_begin(part: u8) {
    PART.store(part, Ordering::Relaxed);
    FRAME_START.store(now_ms() + 1, Ordering::Release);
}

/// The stretch ended: no freeze (or no more of one).
fn stretch_end() {
    FRAME_START.store(0, Ordering::Release);
    if FROZEN.swap(false, Ordering::AcqRel) {
        tracing::warn!("the window answers again");
        if let Some(r) = REPORTER.get() {
            r.recovered();
        }
    }
}

/// The window starts a frame's logic (top of `App::logic`): commands that came in and the clock.
/// egui runs it every frame, and alone while the main window is minimized (#202).
pub fn logic_begin() {
    stretch_begin(LOGIC);
}

/// The frame's logic is done (end of `App::logic`); its UI, when egui runs one, follows at once.
pub fn logic_end() {
    stretch_end();
}

/// The window starts drawing a frame (top of `App::ui`).
pub fn frame_begin() {
    stretch_begin(UI);
}

/// The window finished a frame: no freeze; windows opened or closed since the last are logged.
pub fn frame_end() {
    stretch_end();
    WINDOWS.with(|w| {
        let mut w = w.borrow_mut();
        let (now, last) = &mut *w;
        if now != last {
            for opened in now.difference(last) {
                tracing::info!("window opened: {opened}");
            }
            for closed in last.difference(now) {
                tracing::info!("window closed: {closed}");
            }
        }
        *last = std::mem::take(now);
    });
}

/// A window is drawn this frame (dialogs::dialog_window, the progress windows…).
pub fn window(id: &str) {
    WINDOWS.with(|w| {
        let mut w = w.borrow_mut();
        if !w.0.contains(id) {
            w.0.insert(id.to_owned());
        }
    });
}

/// What MDM has open and running, for the report (sent when it changes).
pub fn state(downloads: usize, running: usize, queues: usize, scale: f32) {
    let Some(r) = REPORTER.get() else { return };
    let windows = WINDOWS.with(|w| w.borrow().1.iter().cloned().collect());
    r.state(&AppState {
        downloads,
        running,
        queues,
        windows,
        scale,
    });
}

/// MDM has started closing normally: what panics on the runtime's own threads now (a task's
/// timer once the runtime is shutting down, #422) is the runtime going away, not a problem.
static CLOSING: AtomicBool = AtomicBool::new(false);

/// Is this panic worth a report? Not one on a runtime worker while MDM is closing (#422).
fn panic_reported(closing: bool, thread: &str) -> bool {
    !(closing && thread.starts_with("tokio"))
}

/// MDM is closing normally: the monitor leaves without a report.
pub fn clean_exit() {
    CLOSING.store(true, Ordering::Release);
    if let Some(r) = REPORTER.get() {
        tracing::info!("closing");
        r.clean();
    }
    // Some ends exit at once after this: the log's last lines first.
    mdm_crash::logs::flush(LOG_FOR_A_REPORT);
}

/// How long a report (or an exit) waits for the log's last lines to be in the file: the log
/// writes on its own thread (#513), and a disk held longer than this leaves them out.
const LOG_FOR_A_REPORT: Duration = Duration::from_secs(1);

/// Watch the window's frames: a stretch of one (its logic or its UI) running past FREEZE_AFTER
/// is reported once. Time the computer spent asleep (this loop's own tick much longer than
/// asked) isn't counted, nor the waits between frames (no stretch runs then).
fn watchdog() {
    let tick = Duration::from_millis(500);
    let mut last = now_ms();
    // Frames that started before this are past a sleep: judged from `awake_from` instead.
    let mut awake_from = 0u64;
    let mut reported_frame = 0u64;
    let ui = UI_THREAD.get().copied().unwrap_or(0);
    // The window thread's use when this tick first saw the current frame: (frame, ms, use).
    let mut seen: Option<(u64, u64, Option<Usage>)> = None;
    loop {
        std::thread::sleep(tick);
        let now = now_ms();
        if now.saturating_sub(last) > 3_000 {
            awake_from = now;
            seen = None;
        }
        last = now;
        let start = FRAME_START.load(Ordering::Acquire);
        if start == 0 || start == reported_frame {
            continue;
        }
        if seen.is_none_or(|(frame, _, _)| frame != start) {
            seen = Some((start, now, usage(ui)));
        }
        let since = (start - 1).max(awake_from);
        let ran = now.saturating_sub(since);
        if ran < FREEZE_AFTER.as_millis() as u64 {
            continue;
        }
        reported_frame = start;
        FROZEN.store(true, Ordering::Release);
        tracing::error!(
            "the window stopped responding ({} s into a frame, in its {})",
            ran / 1000,
            if PART.load(Ordering::Relaxed) == LOGIC {
                "logic"
            } else {
                "UI"
            }
        );
        let busy = seen.and_then(|(_, at, before)| {
            let (before, after) = (before?, usage(ui)?);
            Some(Busy {
                wall_ms: now.saturating_sub(at),
                cpu_ms: after.cpu_ms.saturating_sub(before.cpu_ms),
                faults: before
                    .faults
                    .zip(after.faults)
                    .map(|(b, a)| a.saturating_sub(b)),
            })
        });
        if let Some(b) = busy {
            tracing::warn!(
                "the window thread ran {} of {} ms meanwhile ({} page faults)",
                b.cpu_ms,
                b.wall_ms,
                b.faults.map_or("unknown".to_owned(), |n| n.to_string())
            );
        }
        mdm_crash::logs::flush(LOG_FOR_A_REPORT);
        if let Some(r) = REPORTER.get() {
            r.freeze(&Freeze {
                ui_thread: ui,
                main: true,
                seconds: ran / 1000,
                busy,
            });
            dump_now();
        }
    }
}

/// A thread's processor time so far and the page faults counted so far (see `Busy::faults`).
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    pub cpu_ms: u64,
    pub faults: Option<u64>,
}

/// The window thread's use so far. None on macOS: the dump can't name the window thread there
/// (`thread_id` is 0), so its use isn't measured either.
fn usage(ui_thread: u32) -> Option<Usage> {
    if ui_thread == 0 {
        return None;
    }
    #[cfg(windows)]
    return crate::win32::thread_usage(ui_thread);
    #[cfg(unix)]
    return crate::unix::thread_usage(ui_thread);
}

/// The session's description for the monitor.
pub fn session(language: &str, dark: bool) -> Session {
    #[cfg(windows)]
    let os_version = crate::win32::windows_version();
    #[cfg(unix)]
    let os_version = crate::unix::os_version();
    Session {
        version: crate::help::VERSION.to_owned(),
        commit: crate::help::COMMIT.to_owned(),
        install: mdm_tools::update::Install::detect().label().to_owned(),
        os: if cfg!(windows) {
            "Windows"
        } else if cfg!(target_os = "macos") {
            "macOS"
        } else {
            "Linux"
        }
        .to_owned(),
        os_version,
        arch: std::env::consts::ARCH.to_owned(),
        language: language.to_owned(),
        dark,
        pid: std::process::id(),
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        started: chrono::Utc::now().timestamp(),
        // Known once the window started (renderer.rs).
        renderer: String::new(),
    }
}

/// MDM_CRASH_TEST=1 lets `/show crash-test:<what>` crash MDM on purpose (the crash reports'
/// own end-to-end check): panic, crash (an access violation / SIGSEGV), freeze (a deadlock on
/// the window thread), spin (the window thread busy for 12 s, then it answers again), abort.
pub fn test(what: &str) {
    if std::env::var_os("MDM_CRASH_TEST").is_none_or(|v| v != "1") {
        return;
    }
    tracing::warn!("crash test: {what}");
    match what {
        "panic" => panic!("crash test: a panic on purpose"),
        "crash" => {
            #[cfg(windows)]
            crate::win32::crash_now();
            #[cfg(unix)]
            crate::unix::crash_now();
        }
        "freeze" => crash_test_freeze(),
        "spin" => crash_test_spin(),
        "abort" => std::process::abort(),
        _ => {}
    }
}

/// The window thread runs for 12 s without a break, as an endless loop would.
#[inline(never)]
fn crash_test_spin() {
    let until = Instant::now() + Duration::from_secs(12);
    let mut n = 0u64;
    while Instant::now() < until {
        n = std::hint::black_box(n.wrapping_add(1));
    }
}

/// The window thread waits on itself, as 0.2.2's Add URL did.
#[inline(never)]
fn crash_test_freeze() {
    let lock = std::sync::Mutex::new(());
    let _held = lock.lock();
    let _never = lock.lock();
}

#[cfg(test)]
mod closing_tests {
    /// #422: a Tokio task panicking while MDM closes is no crash; the window thread's would be.
    #[test]
    fn a_runtime_panic_while_closing_is_not_reported() {
        use super::panic_reported;
        assert!(!panic_reported(true, "tokio-rt-worker"));
        assert!(panic_reported(false, "tokio-rt-worker"));
        assert!(panic_reported(true, "main"));
    }
}
