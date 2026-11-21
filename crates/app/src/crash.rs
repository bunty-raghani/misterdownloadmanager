//! Crash and freeze reports, the app's side (mdm-crash has the rest). At start MDM launches its
//! monitor (itself with `--crash-monitor`), which writes a minidump when MDM:
//! - crashes natively (win32.rs / unix.rs catch it through crash-handler),
//! - panics (the hook here sends the message, then asks for the dump),
//! - or freezes: a watchdog thread sees a frame of the window run past [`FREEZE_AFTER`] and asks
//!   for a dump of every thread while MDM is still stuck (a deadlock, an endless loop). A frame
//!   is two timed stretches: the app's logic (commands and the clock, which egui runs alone while
//!   the main window is minimized) and its UI. The waits between frames aren't timed, nor, on
//!   macOS, the time a system Open or Save dialog is open ([`system_dialog`]).
//!
//! The monitor then shows the report window (crashui.rs). Without a monitor (it couldn't start)
//! MDM runs on, and a panic still leaves a text report.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mdm_crash::client::Reporter;
use mdm_crash::protocol::{AppState, Busy, Freeze, Panic, Session};

/// A frame running this long is a freeze (Windows calls a window "Not responding" after 5 s).
pub const FREEZE_AFTER: Duration = Duration::from_secs(8);

/// A stretch of a frame running this long is logged (with what it was doing): the window
/// answered, but slowly enough to be felt (#1063 took seconds per frame before it froze).
const SLOW_FRAME: Duration = Duration::from_millis(500);

/// The first minute after the computer wakes is the computer's: its disk, its network and its
/// screen come back, and MDM's memory, given back while it waited in the notification area
/// (#72), is read back in. Two windows hidden there didn't answer for 16 and 23 s right after a
/// wake, and the separate monitor process couldn't even take their dump until the same moment
/// (#574, #613): most likely the computer wasn't running MDM yet. A frame stalled then is
/// logged, and reported only if it is still stuck [`FREEZE_AFTER`] past this.
const WAKE_GRACE: Duration = Duration::from_secs(60);

/// The first three minutes of a start with the session are the computer's too: everything else
/// the login starts (the system's own services, the other programs in the notification area,
/// their updates) wants the disk and the processor at the same time. A window started with
/// Windows didn't answer for 8 s a minute after its start, its thread ran 0.0 s of those 8, and
/// the snapshot found it back waiting for messages (#1126): the computer wasn't running MDM. A
/// frame stalled then is logged, and reported only if it is still stuck [`FREEZE_AFTER`] past
/// this (counted from MDM's start).
/// A first frame late then is taken for the busy login's, once (renderer.rs, #1205).
pub(crate) const LOGIN_GRACE: Duration = Duration::from_secs(180);

/// A stall while every window of MDM's is hidden (in the notification area or minimized to the
/// taskbar, nothing of MDM's on screen: [`out_of_sight`]) and its thread barely runs
/// ([`IDLE_SHARE`]) is the computer's for this long, counted from the stall's start. Windows that
/// started with the session or were closed to the notification area hours before stopped
/// answering for 8 s in one of the hidden window's small ticks, their thread running 0 ms of the
/// 8 with a few dozen page faults in the whole process, back 0.2 to 57 s later with no sleep in
/// between (#1126): the computer kept MDM waiting (most likely its memory, given back while
/// hidden (#72), read back from a busy or sleeping disk), and nobody could see the window stuck.
/// Windows minimized to the taskbar did the same (#1481, #1309). Logged, and reported only if it
/// is still stuck [`FREEZE_AFTER`] past this, or [`FREEZE_AFTER`] after a window was asked for
/// ([`asked`]).
const HIDDEN_GRACE: Duration = Duration::from_secs(60);

/// The window thread barely ran during a stall when it ran less than 1/IDLE_SHARE of it (5 %):
/// it waited on the computer (a disk, the memory it reads back). A thread that runs more is busy
/// with MDM's own work, and its stall is a freeze, hidden or not.
const IDLE_SHARE: u64 = 20;

/// A window asked for this long (ms) before a stall began is still waited for in it ([`asked`]).
const ASKED_BEFORE: u64 = 2_000;

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
/// What the current stretch is doing ([`Step`]), for the log and the report.
static STEP: AtomicU8 = AtomicU8::new(Step::Start as u8);
/// A freeze was reported and the window hasn't answered since.
static FROZEN: AtomicBool = AtomicBool::new(false);
/// A stall in the computer's time was logged (not reported) and the window hasn't answered
/// since: the [`After`] it followed (as u8), 0 when none.
static GRACED: AtomicU8 = AtomicU8::new(0);
/// A panic was reported: later ones (a burst on worker threads) are logged, not dumped again.
static PANICKED: AtomicBool = AtomicBool::new(false);
/// Every window of MDM's is hidden: the main window in the notification area, no other open
/// ([`hidden`]).
static HIDDEN: AtomicBool = AtomicBool::new(false);
/// When a window was last asked for from outside the window thread (ms since EPOCH, + 1); 0
/// before any ([`asked`]).
static ASKED: AtomicU64 = AtomicU64::new(0);
/// The window thread (the dump's id for it; 0 where it can't be known).
static UI_THREAD: OnceLock<u32> = OnceLock::new();
/// System dialogs open now (Open, Save, a folder: [`system_dialog`]).
static SYSTEM_DIALOGS: AtomicUsize = AtomicUsize::new(0);
/// When the last system dialog closed (ms since EPOCH, + 1); 0 before any closed.
static DIALOG_CLOSED: AtomicU64 = AtomicU64::new(0);

/// Whether a system dialog runs on the window thread. macOS's Open and Save panels do: rfd hands
/// them to the main thread, the window's, whatever thread asks for one, and AppKit runs them
/// there in a modal loop of their own until they close (#1192: a person choosing a file for 9 s
/// was reported as a freeze). Windows' dialog (IFileDialog) runs on the thread that asks for it,
/// and Linux's (the desktop portal, or zenity) is waited for on that thread too: the window thread
/// goes on drawing, and a frame stuck meanwhile is still a freeze there.
const DIALOG_ON_WINDOW_THREAD: bool = cfg!(target_os = "macos");

thread_local! {
    /// The windows drawn this frame and last frame (ids), for the log and the report.
    static WINDOWS: RefCell<(BTreeSet<String>, BTreeSet<String>)> =
        RefCell::new(Default::default());
}

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Start the monitor and catch crashes, panics and freezes. Called on the window thread, after
/// this process became MDM's instance; `at_login` when the session started it (the startup
/// entry, #226): its first minutes are the computer's ([`LOGIN_GRACE`]).
pub fn start(data: &Path, session: Session, at_login: bool) {
    let _ = EPOCH.set(Instant::now());
    #[cfg(windows)]
    let _ = UI_THREAD.set(crate::win32::thread_id());
    #[cfg(unix)]
    let _ = UI_THREAD.set(crate::unix::thread_id());
    // macOS: the window thread's use is asked for by its Mach port instead ([`usage`]).
    #[cfg(target_os = "macos")]
    crate::unix::remember_window_thread();
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
        .spawn(move || watchdog(at_login));
    tracing::info!("crash reports on, connecting to the monitor");
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
        // Linux: winit's calls on an X server that went away (logout) panic on the window's
        // thread: the session's end, saved as one (MDM ends there), or, the app busy in its
        // frame, by the app's drop as the panic unwinds; not a crash.
        #[cfg(target_os = "linux")]
        if crate::unix::display_lost_panic(&p.thread) {
            crate::unix::display_lost();
            default(info);
            return;
        }
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
    STEP.store(Step::Start as u8, Ordering::Relaxed);
    FRAME_START.store(now_ms() + 1, Ordering::Release);
}

/// The stretch ended: no freeze (or no more of one); a slow one is logged.
fn stretch_end() {
    let start = FRAME_START.swap(0, Ordering::AcqRel);
    let took = now_ms().saturating_sub(start.saturating_sub(1));
    if start != 0 && took >= SLOW_FRAME.as_millis() as u64 {
        let part = if PART.load(Ordering::Relaxed) == LOGIC {
            "logic"
        } else {
            "UI"
        };
        let (open, closed) = dialogs_now();
        let dialog =
            if dialog_from(start - 1, open, closed, DIALOG_ON_WINDOW_THREAD) != Some(start - 1) {
                ", a system dialog open on the window thread meanwhile"
            } else {
                ""
            };
        tracing::info!(
            "a slow frame: its {part} took {took} ms (at {}){dialog}",
            Step::now().label()
        );
    }
    let graced = After::from_u8(GRACED.swap(0, Ordering::AcqRel));
    if FROZEN.swap(false, Ordering::AcqRel) {
        tracing::warn!("the window answers again");
        if let Some(r) = REPORTER.get() {
            r.recovered();
        }
    } else if let Some(after) = graced {
        tracing::warn!("the window answers again ({})", after.had_just());
    }
}

/// What a frame is doing, set as it goes (`step`): a freeze's log line and report name the one
/// it stopped in, even when the dump comes too late to show it (#574).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    /// The stretch began; no step named yet.
    Start,
    /// The commands that came in (a second launch, the browser, the automation API).
    Commands,
    /// The proxy, DNS and cookie settings handed to the network code.
    NetworkSettings,
    /// Whether a screen reader listens.
    Accessibility,
    /// The network adapter's watch.
    NetworkAdapter,
    /// The download limits and the debrid services.
    Limits,
    /// The downloads' progress and what finished.
    Downloads,
    /// Torrents in other programs (remote control).
    RemoteTorrents,
    /// The update check.
    UpdateCheck,
    /// The clipboard's watch.
    Clipboard,
    /// The notification area's icon.
    Tray,
    /// The taskbar button's progress.
    Taskbar,
    /// The crash report's record of what is open.
    ReportState,
    /// The torrents' session and feeds.
    Torrents,
    /// The theme and the system's light or dark mode.
    Theme,
    /// The title bar's and menus' colour.
    Caption,
    /// Drawing the main window.
    MainWindow,
    /// The dialogs and other windows.
    Dialogs,
}

impl Step {
    const ALL: [Step; 18] = [
        Step::Start,
        Step::Commands,
        Step::NetworkSettings,
        Step::Accessibility,
        Step::NetworkAdapter,
        Step::Limits,
        Step::Downloads,
        Step::RemoteTorrents,
        Step::UpdateCheck,
        Step::Clipboard,
        Step::Tray,
        Step::Taskbar,
        Step::ReportState,
        Step::Torrents,
        Step::Theme,
        Step::Caption,
        Step::MainWindow,
        Step::Dialogs,
    ];

    /// For the log and the report (English, as they are).
    fn label(self) -> &'static str {
        match self {
            Step::Start => "the start of the frame",
            Step::Commands => "the commands that came in",
            Step::NetworkSettings => "the network settings",
            Step::Accessibility => "asking whether a screen reader listens",
            Step::NetworkAdapter => "the network adapter's watch",
            Step::Limits => "the download limits",
            Step::Downloads => "the downloads' progress",
            Step::RemoteTorrents => "the remote torrents",
            Step::UpdateCheck => "the update check",
            Step::Clipboard => "the clipboard's watch",
            Step::Tray => "the tray icon",
            Step::Taskbar => "the taskbar button's progress",
            Step::ReportState => "the crash monitor's record",
            Step::Torrents => "the torrents",
            Step::Theme => "the theme",
            Step::Caption => "the title bar's colour",
            Step::MainWindow => "drawing the main window",
            Step::Dialogs => "the dialogs",
        }
    }

    fn now() -> Step {
        let n = STEP.load(Ordering::Relaxed) as usize;
        Step::ALL.get(n).copied().unwrap_or(Step::Start)
    }
}

/// The window thread's frame goes on to `s` (a plain store: free on the window thread).
pub fn step(s: Step) {
    STEP.store(s as u8, Ordering::Relaxed);
}

/// Whether every window of MDM's is out of sight now ([`out_of_sight`]): set by the window thread
/// at each frame's logic and as the window comes back. A stall then is felt by nobody, and while
/// the window thread barely runs it is the computer's ([`HIDDEN_GRACE`]).
pub fn hidden(all: bool) {
    HIDDEN.store(all, Ordering::Release);
}

/// Whether nothing of MDM's is on screen: the main window `hidden` in the notification area or
/// `minimized` to the taskbar (the Dock, the window list), and no other window open (`windows`,
/// the main one's included). A minimized window is no more on screen than a hidden one: the
/// system stops painting it and may take its memory back, and an idle wait then is the
/// computer's as much (#1481, #1309).
pub fn out_of_sight(hidden: bool, minimized: bool, windows: usize) -> bool {
    (hidden || minimized) && windows <= 1
}

/// Someone asked for a window, or for an answer, from outside the window thread: a command came
/// in (a second launch, the browser, the automation API), the shortcut to show the window was
/// pressed, the tray icon was clicked. A hidden window's stall is no longer left to the computer
/// once someone waits for it ([`HIDDEN_GRACE`]).
pub fn asked() {
    ASKED.store(now_ms() + 1, Ordering::Release);
}

/// Runs `dialog`, which shows a system Open, Save or folder dialog and waits for its answer
/// (winui::start_pick, on a thread of its own): while it runs, the time the window thread spends
/// in that dialog isn't a freeze ([`DIALOG_ON_WINDOW_THREAD`]: macOS runs it there). Logged as it
/// opens and closes, on every system; counted as closed however `dialog` ends, a panic too.
pub fn system_dialog<T>(dialog: impl FnOnce() -> T) -> T {
    /// Counts the dialog closed as it goes, unwinding or not.
    struct Open;
    impl Drop for Open {
        fn drop(&mut self) {
            // The close's time first: a watchdog that sees no dialog open sees when it closed.
            DIALOG_CLOSED.store(now_ms() + 1, Ordering::Release);
            SYSTEM_DIALOGS.fetch_sub(1, Ordering::AcqRel);
            tracing::info!("system dialog closed");
        }
    }
    SYSTEM_DIALOGS.fetch_add(1, Ordering::AcqRel);
    tracing::info!("a system dialog is open");
    let _open = Open;
    dialog()
}

/// The system dialogs open now, for the tests (which take [`DIALOG_TESTS`] around them).
#[cfg(test)]
pub fn dialogs_open() -> usize {
    SYSTEM_DIALOGS.load(Ordering::Acquire)
}

/// The tests that count system dialogs take turns (the count is the process's).
#[cfg(test)]
pub static DIALOG_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether a system dialog is open now, and when the last one closed (ms since EPOCH; 0: none).
fn dialogs_now() -> (bool, u64) {
    let open = SYSTEM_DIALOGS.load(Ordering::Acquire) > 0;
    (
        open,
        DIALOG_CLOSED.load(Ordering::Acquire).saturating_sub(1),
    )
}

/// Where a stretch that started at `start` (ms) is counted from, given the system dialogs. When
/// they run on the window thread (`on_window_thread`): not while one is `open` (None: the window
/// thread is in it, and the time is the person's, choosing a file), and from the last one's
/// close (`closed`, ms) for a stretch that began before it (the dialog's time isn't counted).
/// Elsewhere a dialog changes nothing: the window thread doesn't run it.
fn dialog_from(start: u64, open: bool, closed: u64, on_window_thread: bool) -> Option<u64> {
    if !on_window_thread {
        Some(start)
    } else if open {
        None
    } else {
        Some(start.max(closed))
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

/// MDM is closing normally: the monitor leaves without a report. Once: macOS's own quit says so
/// as the app goes (`App`'s drop), before `main` would.
pub fn clean_exit() {
    if CLOSING.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(r) = REPORTER.get() {
        tracing::info!("closing");
        r.clean();
    }
    // Some ends exit at once after this: the log's last lines first.
    mdm_crash::logs::flush(LOG_FOR_A_REPORT);
}

/// How long a report (or an exit) waits for the log's last lines to be in the file: the log
/// writes on its own thread (#513), and a disk held longer than this leaves them out (a panic's
/// or a freeze's report then takes them from the app's memory, sent with it: #1354).
const LOG_FOR_A_REPORT: Duration = Duration::from_secs(1);

/// The wall clock in ms: unlike `Instant` it counts the time the computer slept everywhere
/// (Linux's and macOS's monotonic clocks stop then; Windows' doesn't).
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The computer slept during a tick of the watchdog's: its tick took more than 3 s by either
/// clock (it asks for 0.5 s). A clock set forward by hand looks the same, and only delays a
/// report by [`WAKE_GRACE`].
fn slept(monotonic_gap: u64, wall_gap: u64) -> bool {
    monotonic_gap > 3_000 || wall_gap > 3_000
}

/// What made the time that follows the computer's: a wake ([`WAKE_GRACE`]), a start with the
/// session ([`LOGIN_GRACE`]), or a stall with every window hidden ([`HIDDEN_GRACE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum After {
    Wake = 1,
    Login = 2,
    /// Every window hidden, the window thread waiting ([`HIDDEN_GRACE`]).
    Hidden = 3,
}

impl After {
    fn from_u8(v: u8) -> Option<After> {
        match v {
            1 => Some(After::Wake),
            2 => Some(After::Login),
            3 => Some(After::Hidden),
            _ => None,
        }
    }

    /// What a logged stall is counted since.
    fn since(self) -> &'static str {
        match self {
            After::Wake => "since the computer woke",
            After::Login => "since MDM started with the session",
            After::Hidden => {
                "with every window of MDM's hidden or minimized and its thread waiting"
            }
        }
    }

    /// What its grace is counted from, for the log.
    fn mark(self) -> &'static str {
        match self {
            After::Wake => "the wake",
            After::Login => "MDM's start",
            After::Hidden => "the stall's start",
        }
    }

    /// Why the window didn't answer, once it does.
    fn had_just(self) -> &'static str {
        match self {
            After::Wake => "the computer had just woken",
            After::Login => "MDM had just started with the session",
            After::Hidden => {
                "every window was hidden or minimized, and the computer kept its thread waiting"
            }
        }
    }
}

/// The computer's time: from `from` (ms since MDM's start), `grace` ms in which a stall is the
/// computer's rather than MDM's, after a wake or a start with the session ([`After`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Computers {
    from: u64,
    grace: u64,
    after: After,
}

impl Computers {
    /// The computer woke at `at` (ms): the time asleep isn't counted, nor [`WAKE_GRACE`] after.
    fn wake(at: u64) -> Computers {
        Computers {
            from: at,
            grace: WAKE_GRACE.as_millis() as u64,
            after: After::Wake,
        }
    }

    /// MDM started with the session: its first [`LOGIN_GRACE`] aren't counted.
    fn login() -> Computers {
        Computers {
            from: 0,
            grace: LOGIN_GRACE.as_millis() as u64,
            after: After::Login,
        }
    }

    /// A stall that started at `start` (ms) with every window hidden and the window thread
    /// barely running: its first [`HIDDEN_GRACE`] aren't counted, but for a window asked for
    /// (`asked`, ms; 0: never) meanwhile, or just before it began ([`ASKED_BEFORE`]): from then
    /// on someone waits, and the stall counts from the ask.
    fn hidden(start: u64, asked: u64) -> Computers {
        let full = HIDDEN_GRACE.as_millis() as u64;
        let grace = if asked != 0 && asked + ASKED_BEFORE >= start {
            asked.saturating_sub(start).min(full)
        } else {
            full
        };
        Computers {
            from: start,
            grace,
            after: After::Hidden,
        }
    }

    /// When its grace ends (ms).
    fn ends(self) -> u64 {
        self.from + self.grace
    }

    /// The same moment without its grace: what the log counts.
    fn no_grace(self) -> Computers {
        Computers { grace: 0, ..self }
    }
}

/// How long the stretch that started at `start` (ms) counts as running at `now`: from `start`,
/// or from the start of the computer's time `computers` when it began before it (the time asleep
/// isn't counted), and not its grace.
fn counted(start: u64, now: u64, computers: Option<Computers>) -> u64 {
    let from = computers.map_or(start, |c| start.max(c.from + c.grace));
    now.saturating_sub(from)
}

/// Whether the window thread barely ran in a stall: `cpu` ms of `wall` ms ([`IDLE_SHARE`]).
fn idle(cpu: u64, wall: u64) -> bool {
    cpu.saturating_mul(IDLE_SHARE) < wall
}

/// The computer's time for a stretch that started at `start` (ms): the wake's or the start's
/// (`computers`) and, when every window is hidden and the window thread barely ran in it
/// (`hidden_idle`), the stall's own ([`Computers::hidden`], cut short by a window `asked` for),
/// whichever ends later.
fn computers_for(
    start: u64,
    computers: Option<Computers>,
    hidden_idle: bool,
    asked: u64,
) -> Option<Computers> {
    let hidden = hidden_idle.then(|| Computers::hidden(start, asked));
    match (computers, hidden) {
        (Some(c), Some(h)) => Some(if h.ends() > c.ends() { h } else { c }),
        (c, h) => c.or(h),
    }
}

/// Watch the window's frames: a stretch of one (its logic or its UI) running past FREEZE_AFTER
/// is reported once. Time the computer spent asleep isn't counted, nor the computer's time: the
/// first minute after it woke ([`WAKE_GRACE`]), the first three of a start with the session
/// (`at_login`, [`LOGIN_GRACE`]) and the first minute of a stall nobody could see, every window
/// hidden and the window thread waiting ([`HIDDEN_GRACE`]), when a stall is logged, and reported
/// only if it lasts past them. Nor are the waits between frames (no stretch runs then), nor the
/// time a system dialog runs on the window thread ([`dialog_from`]: macOS), which is logged
/// instead.
fn watchdog(at_login: bool) {
    let tick = Duration::from_millis(500);
    let freeze_after = FREEZE_AFTER.as_millis() as u64;
    let (mut last, mut last_wall) = (now_ms(), wall_ms());
    // The computer's time: MDM's start with the session, then the last wake.
    let mut computers = at_login.then(Computers::login);
    let mut reported_frame = 0u64;
    // The frame whose stall in the computer's time was logged.
    let mut noted_frame = 0u64;
    // The frame whose time in a system dialog was logged.
    let mut noted_dialog = 0u64;
    let ui = UI_THREAD.get().copied().unwrap_or(0);
    // The window thread's use when this tick first saw the current frame: (frame, ms, use).
    let mut seen: Option<(u64, u64, Option<Usage>)> = None;
    loop {
        std::thread::sleep(tick);
        let (now, wall) = (now_ms(), wall_ms());
        if slept(now.saturating_sub(last), wall.saturating_sub(last_wall)) {
            tracing::info!(
                "the computer woke (the freeze watchdog's tick took {} s)",
                { wall.saturating_sub(last_wall).max(now.saturating_sub(last)) / 1000 }
            );
            computers = Some(Computers::wake(now));
            seen = None;
        }
        (last, last_wall) = (now, wall);
        let start = FRAME_START.load(Ordering::Acquire);
        if start == 0 || start == reported_frame {
            continue;
        }
        if seen.is_none_or(|(frame, _, _)| frame != start) {
            seen = Some((start, now, usage(ui)));
        }
        let part = if PART.load(Ordering::Relaxed) == LOGIC {
            "logic"
        } else {
            "UI"
        };
        let (open, closed) = dialogs_now();
        let Some(from) = dialog_from(start - 1, open, closed, DIALOG_ON_WINDOW_THREAD) else {
            // A system dialog runs on the window thread: the window waits for the person.
            let held = now.saturating_sub(start - 1);
            if noted_dialog != start && held >= freeze_after {
                noted_dialog = start;
                tracing::info!(
                    "the window's {part} has waited {} s for a system dialog (macOS runs it on the window thread): not a freeze",
                    held / 1000
                );
            }
            continue;
        };
        // The window thread's use since this tick first saw the stretch, once it is long (a
        // normal frame is over long before: the thread isn't asked about each one).
        let long = now.saturating_sub(from) >= freeze_after;
        let busy = seen.filter(|_| long).and_then(|(_, at, before)| {
            Some(busy_between(before?, usage(ui)?, now.saturating_sub(at)))
        });
        let told = |b: Busy| {
            let pageins = b
                .pageins
                .map(|n| format!(", {n} page-ins"))
                .unwrap_or_default();
            tracing::warn!(
                "the window thread ran {} of {} ms meanwhile ({} page faults{pageins})",
                b.cpu_ms,
                b.wall_ms,
                b.faults.map_or("unknown".to_owned(), |n| n.to_string())
            );
        };
        // Every window hidden and the window thread barely running: nobody sees the stall, and
        // the computer has the thread (#1126). Unknown use isn't taken for idle.
        let hidden_idle =
            HIDDEN.load(Ordering::Acquire) && busy.is_some_and(|b| idle(b.cpu_ms, b.wall_ms));
        let asked = ASKED.load(Ordering::Acquire).saturating_sub(1);
        let theirs = computers_for(from, computers, hidden_idle, asked);
        // Since the wake (or the start, or the stall's start), for the log; past the grace, for
        // the report.
        let ran = counted(from, now, theirs.map(Computers::no_grace));
        if counted(from, now, theirs) < freeze_after {
            if let Some(c) = theirs
                && ran >= freeze_after
                && noted_frame != start
            {
                noted_frame = start;
                GRACED.store(c.after as u8, Ordering::Release);
                tracing::warn!(
                    "the window hasn't answered for {} s, {} (in its {part}, at {}): reported if it still doesn't {} s after {}",
                    ran / 1000,
                    c.after.since(),
                    Step::now().label(),
                    (c.grace + freeze_after) / 1000,
                    c.after.mark()
                );
                if let Some(b) = busy {
                    told(b);
                }
            }
            continue;
        }
        reported_frame = start;
        FROZEN.store(true, Ordering::Release);
        let step = Step::now().label();
        tracing::error!(
            "the window stopped responding ({} s into a frame, in its {part}, at {step})",
            ran / 1000,
        );
        if let Some(b) = busy {
            told(b);
        }
        mdm_crash::logs::flush(LOG_FOR_A_REPORT);
        if let Some(r) = REPORTER.get() {
            r.freeze(&Freeze {
                ui_thread: ui,
                main: true,
                seconds: ran / 1000,
                busy,
                step: step.to_owned(),
            });
            dump_now();
        }
    }
}

/// A thread's processor time so far and the page faults and page-ins counted so far (see
/// `Busy::faults` and `Busy::pageins`).
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    pub cpu_ms: u64,
    pub faults: Option<u64>,
    pub pageins: Option<u64>,
}

/// What the window thread used between two readings `wall_ms` apart.
fn busy_between(before: Usage, after: Usage, wall_ms: u64) -> Busy {
    let since = |b: Option<u64>, a: Option<u64>| b.zip(a).map(|(b, a)| a.saturating_sub(b));
    Busy {
        wall_ms,
        cpu_ms: after.cpu_ms.saturating_sub(before.cpu_ms),
        faults: since(before.faults, after.faults),
        pageins: since(before.pageins, after.pageins),
    }
}

/// The window thread's use so far: by its id on Windows and Linux; on macOS, where the dump
/// can't name it (`thread_id` is 0), by the Mach port remembered at [`start`] (#1515).
fn usage(ui_thread: u32) -> Option<Usage> {
    #[cfg(windows)]
    return (ui_thread != 0)
        .then(|| crate::win32::thread_usage(ui_thread))
        .flatten();
    #[cfg(target_os = "macos")]
    return crate::unix::thread_usage(ui_thread);
    #[cfg(all(unix, not(target_os = "macos")))]
    return (ui_thread != 0)
        .then(|| crate::unix::thread_usage(ui_thread))
        .flatten();
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
/// With the window hidden in the notification area first ([`testing`], app.rs), a few seconds
/// later, in a tick of the hidden window ([`armed`], #1126): hidden-stall[:s] (the window thread
/// waits, running nothing, 12 s or `s`), hidden-spin[:s] (it runs). minimized-stall[:s] and
/// minimized-spin[:s] do the same with the main window minimized to the taskbar instead (#1481).
pub fn test(what: &str) {
    if !testing() {
        return;
    }
    tracing::warn!("crash test: {what}");
    let (what, secs) = what.split_once(':').unwrap_or((what, ""));
    let secs = secs.parse().unwrap_or(12);
    match what {
        "panic" => panic!("crash test: a panic on purpose"),
        "crash" => {
            #[cfg(windows)]
            crate::win32::crash_now();
            #[cfg(unix)]
            crate::unix::crash_now();
        }
        "freeze" => crash_test_freeze(),
        "spin" => crash_test_spin(12),
        "abort" => std::process::abort(),
        "hidden-stall" | "hidden-spin" | "minimized-stall" | "minimized-spin" => {
            ARMED_SPIN.store(what.ends_with("-spin"), Ordering::Relaxed);
            ARMED_SECS.store(secs, Ordering::Relaxed);
            ARMED_AT.store(now_ms() + 1 + ARMED_AFTER, Ordering::Release);
        }
        _ => {}
    }
}

/// Whether the crash tests are on (MDM_CRASH_TEST=1).
pub fn testing() -> bool {
    std::env::var_os("MDM_CRASH_TEST").is_some_and(|v| v == "1")
}

/// A test stall armed for a later tick ([`test`]): when (ms since EPOCH, + 1; 0: none), how
/// long (s), and whether the thread runs meanwhile.
static ARMED_AT: AtomicU64 = AtomicU64::new(0);
static ARMED_SECS: AtomicU64 = AtomicU64::new(0);
static ARMED_SPIN: AtomicBool = AtomicBool::new(false);
/// How long after it is asked for an armed stall comes: past the frame that hid the window.
const ARMED_AFTER: u64 = 3_000;

/// The window thread's armed test stall, once its time came (top of every frame's logic; a
/// plain load otherwise).
pub fn armed() {
    let at = ARMED_AT.load(Ordering::Acquire);
    if at == 0 || now_ms() + 1 < at {
        return;
    }
    ARMED_AT.store(0, Ordering::Release);
    let secs = ARMED_SECS.load(Ordering::Relaxed);
    if ARMED_SPIN.load(Ordering::Relaxed) {
        tracing::warn!("crash test: the window thread runs for {secs} s");
        crash_test_spin(secs);
    } else {
        tracing::warn!("crash test: the window thread waits {secs} s, running nothing");
        std::thread::sleep(Duration::from_secs(secs));
    }
}

/// The window thread runs for `secs` without a break, as an endless loop would.
#[inline(never)]
fn crash_test_spin(secs: u64) {
    let until = Instant::now() + Duration::from_secs(secs);
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
mod watchdog_tests {
    use super::*;

    /// A tick much longer than asked, by either clock, is the computer sleeping: Windows'
    /// monotonic clock counts the sleep, Linux's and macOS's don't (the wall clock does).
    #[test]
    fn a_sleep_is_seen_by_either_clock() {
        assert!(!slept(500, 500));
        assert!(!slept(2_900, 600));
        assert!(slept(3_600_000, 3_600_000), "Windows");
        assert!(slept(500, 3_600_000), "Linux, macOS");
    }

    /// #574, #613: a window hidden in the notification area didn't answer for 16 and 23 s right
    /// after the computer woke. Not a freeze of MDM's during the minute after a wake; still one
    /// past it, and at any other time.
    #[test]
    fn the_minute_after_a_wake_is_the_computers() {
        let (freeze, grace) = (
            FREEZE_AFTER.as_millis() as u64,
            WAKE_GRACE.as_millis() as u64,
        );
        // No wake: a frame counts from its start.
        assert_eq!(counted(1_000, 10_000, None), 9_000);
        // A frame that began before the sleep, or just after it: not counted in the grace…
        let woke = 5_000_000;
        let wake = Some(Computers::wake(woke));
        assert_eq!(counted(1_000, woke + 23_000, wake), 0);
        assert!(counted(woke + 100, woke + 23_000, wake) < freeze);
        // …but its time since the wake is logged.
        let logged = wake.map(Computers::no_grace);
        assert_eq!(counted(1_000, woke + 23_000, logged), 23_000);
        // Still stuck past the grace: a freeze.
        assert!(counted(1_000, woke + grace + freeze, wake) >= freeze);
        // A frame that begins after the grace counts from its own start.
        let late = woke + grace + 5_000;
        assert_eq!(counted(late, late + freeze, wake), freeze);
    }

    /// #1126: started with Windows, the window didn't answer for 8 s a minute after MDM's start,
    /// its thread idle all along (what else the login started had the computer). Not a freeze of
    /// MDM's during the first three minutes of a start with the session; still one past them, at
    /// any other time, and in a start by hand.
    #[test]
    fn the_first_minutes_of_a_start_with_the_session_are_the_computers() {
        let (freeze, grace) = (
            FREEZE_AFTER.as_millis() as u64,
            LOGIN_GRACE.as_millis() as u64,
        );
        let login = Some(Computers::login());
        // #1126's: 71 s after the start, stuck for 8 s. Logged, not reported.
        let (stall, now) = (71_000, 79_000);
        assert_eq!(counted(stall, now, login), 0);
        assert_eq!(counted(stall, now, login.map(Computers::no_grace)), freeze);
        // The same stall in a start by hand: a freeze.
        assert_eq!(counted(stall, now, None), freeze);
        // Still stuck past the grace: a freeze, not before.
        assert!(counted(stall, grace + freeze - 1, login) < freeze);
        assert!(counted(stall, grace + freeze, login) >= freeze);
        // A frame that begins after the grace counts from its own start.
        let late = grace + 5_000;
        assert_eq!(counted(late, late + freeze, login), freeze);
        // A wake later on has its own minute, from the wake.
        let woke = 5_000_000;
        let wake = Some(Computers::wake(woke));
        assert_eq!(counted(woke + 100, woke + 23_000, wake), 0);
        let past = woke + WAKE_GRACE.as_millis() as u64 + freeze;
        assert_eq!(counted(1_000, past, wake), freeze);
    }

    /// #1126: waiting hidden in the notification area for hours, the window didn't answer for
    /// 8 s in one of its ticks, its thread running 0 ms of the 8, and answered 0.2 to 57 s later.
    /// Not a freeze while every window is hidden and the thread barely runs, for a minute; still
    /// one past it, at once when the thread runs (an endless loop), and from the moment a window
    /// is asked for.
    #[test]
    fn a_stall_nobody_can_see_is_the_computers_for_a_minute() {
        let (freeze, grace) = (
            FREEZE_AFTER.as_millis() as u64,
            HIDDEN_GRACE.as_millis() as u64,
        );
        // The reports': 0 of 8006 ms run, every window hidden. Logged, not reported.
        assert!(idle(0, 8_006));
        let stall = 4_000_000;
        let hidden = computers_for(stall, None, idle(0, 8_006), 0);
        assert_eq!(hidden.map(|c| c.after), Some(After::Hidden));
        assert_eq!(counted(stall, stall + 8_006, hidden), 0);
        assert_eq!(
            counted(stall, stall + 8_006, hidden.map(Computers::no_grace)),
            8_006
        );
        // 57 s on (the longest report's) still not; still stuck a minute past FREEZE_AFTER, yes.
        assert!(counted(stall, stall + 57_000, hidden) < freeze);
        assert!(counted(stall, stall + grace + freeze - 1, hidden) < freeze);
        assert!(counted(stall, stall + grace + freeze, hidden) >= freeze);
        // A window on screen, or the thread running: a freeze, as before.
        assert_eq!(computers_for(stall, None, false, 0), None);
        assert!(!idle(8_006, 8_006));
        assert!(!idle(500, 8_006), "6 % is MDM's own work");
        assert!(idle(300, 8_006), "under 5 % is waiting");
        // A window asked for 20 s into the stall (the tray, a second launch, the browser):
        // counted from the ask.
        let asked = stall + 20_000;
        let waited = computers_for(stall, None, true, asked);
        assert!(counted(stall, asked + freeze - 1, waited) < freeze);
        assert_eq!(counted(stall, asked + freeze, waited), freeze);
        // Asked just before it began: waited for from its start.
        let just = computers_for(stall, None, true, stall - 1_000);
        assert_eq!(counted(stall, stall + freeze, just), freeze);
        // An ask long before it changes nothing.
        assert_eq!(computers_for(stall, None, true, stall - 60_000), hidden);
        // Within a start with the session, or after a wake, whichever grace lasts longer.
        let login = Some(Computers::login());
        assert_eq!(computers_for(71_000, login, true, 0), login);
        let late = computers_for(170_000, login, true, 0);
        assert_eq!(late.map(|c| c.after), Some(After::Hidden));
        assert_eq!(computers_for(71_000, login, false, 0), login);
    }

    /// #1481, #1309: a main window minimized to the taskbar is out of sight as much as one hidden
    /// in the notification area; any other window open (a dialog, a progress window) is not.
    #[test]
    fn a_minimized_window_is_out_of_sight_as_a_hidden_one() {
        assert!(
            out_of_sight(true, false, 1),
            "hidden in the notification area"
        );
        assert!(out_of_sight(false, true, 1), "minimized to the taskbar");
        assert!(out_of_sight(true, true, 1));
        assert!(!out_of_sight(false, false, 1), "on screen");
        assert!(
            !out_of_sight(false, true, 2),
            "minimized, a dialog on screen"
        );
        assert!(
            !out_of_sight(true, false, 3),
            "hidden, two windows on screen"
        );
    }

    /// What the log says of a stall in the computer's time, and the store reads back.
    #[test]
    fn the_log_says_whose_time_it_was() {
        assert_eq!(
            Computers::login().after.since(),
            "since MDM started with the session"
        );
        assert_eq!(Computers::wake(1).after.since(), "since the computer woke");
        assert_eq!(
            Computers::hidden(1, 0).after.since(),
            "with every window of MDM's hidden or minimized and its thread waiting"
        );
        for a in [After::Wake, After::Login, After::Hidden] {
            assert_eq!(After::from_u8(a as u8), Some(a));
        }
        assert_eq!(After::from_u8(0), None);
    }

    /// #1192: on macOS the window thread runs a system Open or Save dialog until the person
    /// answers it; 9 s spent choosing a file were reported as a freeze. The time a dialog is
    /// open there isn't counted, nor the part of a stretch before it closed; a stretch still
    /// stuck past [`FREEZE_AFTER`] after it is a freeze. Elsewhere the dialog runs on a thread
    /// of its own and changes nothing.
    #[test]
    fn a_system_dialog_on_the_window_thread_is_not_a_freeze() {
        let freeze = FREEZE_AFTER.as_millis() as u64;
        // #1192's: a stretch at 100 s, the dialog open for 9 s. Not counted while it is open…
        let (stretch, closed) = (100_000, 109_000);
        assert_eq!(dialog_from(stretch, true, 0, true), None);
        assert_eq!(dialog_from(stretch, true, 50_000, true), None);
        // …nor once it closed: the stretch counts from the close.
        let from = dialog_from(stretch, false, closed, true).unwrap();
        assert_eq!(from, closed);
        assert_eq!(counted(from, closed + 500, None), 500);
        // Still stuck a full FREEZE_AFTER after the dialog: a freeze.
        assert!(counted(from, closed + freeze - 1, None) < freeze);
        assert!(counted(from, closed + freeze, None) >= freeze);
        // A dialog closed before the stretch began changes nothing.
        assert_eq!(dialog_from(stretch, false, 90_000, true), Some(stretch));
        assert_eq!(dialog_from(stretch, false, 0, true), Some(stretch));
        // Windows and Linux: the window thread doesn't run the dialog; a frame stuck while one
        // is open is a freeze.
        assert_eq!(dialog_from(stretch, true, 0, false), Some(stretch));
        assert_eq!(dialog_from(stretch, false, closed, false), Some(stretch));
        assert_eq!(DIALOG_ON_WINDOW_THREAD, cfg!(target_os = "macos"));
    }

    /// A system dialog is counted open while it runs, and closed however it ends: answered,
    /// or by a panic in the code that shows it.
    #[test]
    fn a_system_dialog_is_counted_open_until_it_ends() {
        let _turn = DIALOG_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(dialogs_open(), 0);
        let before = now_ms();
        let answer = system_dialog(|| {
            assert!(dialogs_now().0);
            assert_eq!(dialogs_open(), 1);
            Some(42)
        });
        assert_eq!(answer, Some(42));
        assert_eq!(dialogs_open(), 0);
        let (open, closed) = dialogs_now();
        assert!(!open && closed >= before);
        let panicked = std::panic::catch_unwind(|| {
            system_dialog(|| -> Option<u8> {
                assert_eq!(dialogs_open(), 1);
                panic!("a dialog's code panics");
            })
        });
        assert!(panicked.is_err());
        assert_eq!(dialogs_open(), 0, "closed by the panic too");
    }

    /// #1515: a macOS freeze said nothing of the window thread's use, the dump's thread ids
    /// being the monitor's there. Two readings of the thread (by its Mach port, unix.rs) make
    /// the report's "ran X of Y s" line, with the page-ins of the whole of MDM meanwhile.
    #[test]
    fn two_readings_of_the_window_thread_make_the_reports_lines() {
        let before = Usage {
            cpu_ms: 61_250,
            faults: Some(480_000),
            pageins: Some(9_100),
        };
        let after = Usage {
            cpu_ms: 61_330,
            faults: Some(483_100),
            pageins: Some(9_912),
        };
        let busy = busy_between(before, after, 8_000);
        assert_eq!(
            busy,
            Busy {
                wall_ms: 8_000,
                cpu_ms: 80,
                faults: Some(3_100),
                pageins: Some(812),
            }
        );
        let freeze = Freeze {
            ui_thread: 0,
            main: true,
            seconds: 8,
            busy: Some(busy),
            step: "the torrents".to_owned(),
        };
        let input = mdm_crash::report::Input {
            kind: Some(mdm_crash::protocol::Kind::Freeze),
            notes: mdm_crash::protocol::Notes {
                freeze: Some(freeze),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = mdm_crash::report::text(&input, None);
        assert!(t.contains("Its thread ran 0.1 of 8.0 s."), "{t}");
        assert!(
            t.contains("812 page-ins during the freeze (pages of MDM read from disk"),
            "{t}"
        );
        // A reading without them (Windows, Linux) leaves them unknown, not 0.
        let none = Usage {
            pageins: None,
            ..after
        };
        assert_eq!(busy_between(before, none, 8_000).pageins, None);
    }

    /// Every step has its label, and the store reads back as the same step.
    #[test]
    fn steps_read_back() {
        for (i, s) in Step::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
            assert!(!s.label().is_empty());
        }
    }
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
