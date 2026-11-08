//! What draws the windows (#89, #206): OpenGL (eframe's glow), or the system's own graphics API
//! through eframe's wgpu with that one backend only ([`GPU`]): Direct3D 12 on Windows, Vulkan on
//! Linux (Wayland and X11 windows alike), Metal on macOS. View → Draw with picks one, from the
//! next start. Windows draws with Direct3D by default (#89); Linux and macOS keep OpenGL by
//! default until Vulkan and Metal have clean reports from real graphics cards (#206), as
//! Direct3D had before it became Windows' default.
//!
//! The windows always open: when the chosen way can't start (no driver for it, a virtual
//! machine, Remote Desktop), the other is tried, and the reason is logged. A start with the
//! system's API that ends before its first frame (a crash or a kill inside the driver) is
//! remembered, and so is one whose first frame doesn't come within [`FIRST_FRAME_WITHIN`] (MDM
//! starts again, with OpenGL): the next starts draw with OpenGL until that API is chosen again,
//! and View → Draw with says why. A graphics card lost while MDM runs (a driver update or reset)
//! starts MDM again; a second loss soon after counts as the API failing. The same code does all
//! of this on every system: only [`GPU`] and the backend in `gpu::options` differ.
//!
//! Testing overrides, in this order: `/renderer <name>` on the command line, then
//! `MDM_RENDERER=<name>`. Names: `opengl` (`gl`); `direct3d` (`d3d`) on Windows, `vulkan` (`vk`)
//! on Linux, `metal` on macOS, or `gpu` for whichever this system has; `warp` (Windows),
//! `vulkan-software` (`lavapipe`, Linux) or `software`: the system's API drawn by a software
//! renderer (Microsoft's WARP, Mesa's lavapipe), as on a computer without a graphics driver.
//! Another system's name is ignored.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// A choice of View → Draw with, as saved in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Api {
    #[serde(rename = "opengl")]
    OpenGl,
    #[serde(rename = "direct3d")]
    Direct3D,
    #[serde(rename = "vulkan")]
    Vulkan,
    #[serde(rename = "metal")]
    Metal,
}

impl Api {
    /// Its name in the log and the crash reports (the menu's are translated texts, app.rs).
    pub fn name(self) -> &'static str {
        match self {
            Api::OpenGl => "OpenGL",
            Api::Direct3D => "Direct3D 12",
            Api::Vulkan => "Vulkan",
            Api::Metal => "Metal",
        }
    }

    /// The first part of its files' names in the data folder (`direct3d-failed.txt`…).
    fn key(self) -> &'static str {
        match self {
            Api::OpenGl => "opengl",
            Api::Direct3D => "direct3d",
            Api::Vulkan => "vulkan",
            Api::Metal => "metal",
        }
    }
}

/// The system's own graphics API, through wgpu (build.rs sets `gpu` where wgpu is built):
/// Direct3D 12 on Windows, Vulkan on Linux, Metal on macOS. None elsewhere (the BSDs: OpenGL
/// only).
pub const GPU: Option<Api> = if !cfg!(gpu) {
    None
} else if cfg!(windows) {
    Some(Api::Direct3D)
} else if cfg!(target_os = "macos") {
    Some(Api::Metal)
} else {
    Some(Api::Vulkan)
};

/// What draws when the user chose nothing (the settings' `renderer: None`): Direct3D on Windows
/// (#89), OpenGL elsewhere (#206: Vulkan and Metal are choices until real cards have tried them).
pub const DEFAULT: Api = if cfg!(windows) {
    Api::Direct3D
} else {
    Api::OpenGl
};

/// What the setting means here: OpenGL, or this system's API; [`DEFAULT`] when nothing was
/// chosen, or another system's API was (a settings file brought from another computer).
pub fn chosen(setting: Option<Api>) -> Api {
    setting
        .filter(|&a| a == Api::OpenGl || Some(a) == GPU)
        .unwrap_or(DEFAULT)
}

/// How long a watched start with the system's API may take to its first frame before MDM starts
/// again with OpenGL (a driver stuck in its first present, a window that never shows). Generous:
/// a slow computer just after it starts (#57) can take seconds.
#[cfg(gpu)]
const FIRST_FRAME_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Which graphics adapters a start with the system's API may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapters {
    /// A graphics card's driver only.
    Hardware,
    /// A software renderer only (Microsoft's WARP, Mesa's lavapipe).
    Software,
    /// A card first, else a software renderer: the last way to open the windows at all.
    Any,
    /// None at all: the API fails to start (`MDM_RENDERER=gpu-none`, or `direct3d-none`…, in
    /// off-screen test copies, to check the way back to OpenGL).
    Nothing,
}

/// One way to start drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    OpenGl,
    /// The system's API ([`GPU`]).
    Gpu(Adapters),
}

impl Attempt {
    pub fn api(self) -> Api {
        match self {
            Attempt::OpenGl => Api::OpenGl,
            Attempt::Gpu(_) => GPU.unwrap_or(Api::OpenGl),
        }
    }

    fn name(self) -> &'static str {
        match (self, GPU) {
            (Attempt::OpenGl, _) | (_, None) => "OpenGL",
            (Attempt::Gpu(Adapters::Software), Some(Api::Direct3D)) => "Direct3D 12 (WARP)",
            (Attempt::Gpu(Adapters::Software), Some(Api::Vulkan)) => "Vulkan (software)",
            (Attempt::Gpu(Adapters::Software), Some(Api::Metal)) => "Metal (software)",
            (Attempt::Gpu(_), Some(api)) => api.name(),
        }
    }
}

/// A testing override's name. Another system's API isn't built here: no attempt.
fn parse(name: &str) -> Option<Attempt> {
    let name = name.trim().to_ascii_lowercase();
    // `<name>-none` (off-screen test copies): that API, with no adapter at all.
    if let Some(base) = name.strip_suffix("-none") {
        return match parse(base) {
            Some(Attempt::Gpu(_)) if crate::offscreen() => Some(Attempt::Gpu(Adapters::Nothing)),
            _ => None,
        };
    }
    let (api, adapters) = match name.as_str() {
        "opengl" | "gl" | "glow" => return Some(Attempt::OpenGl),
        "direct3d" | "d3d" | "d3d12" | "dx12" => (Some(Api::Direct3D), Adapters::Hardware),
        "warp" => (Some(Api::Direct3D), Adapters::Software),
        "vulkan" | "vk" => (Some(Api::Vulkan), Adapters::Hardware),
        "vulkan-software" | "lavapipe" => (Some(Api::Vulkan), Adapters::Software),
        "metal" => (Some(Api::Metal), Adapters::Hardware),
        "gpu" | "wgpu" => (GPU, Adapters::Hardware),
        "software" => (GPU, Adapters::Software),
        _ => return None,
    };
    (api.is_some() && api == GPU).then_some(Attempt::Gpu(adapters))
}

/// The ways to try, in order, and whether the first is the user's choice of the system's API
/// (whose start is watched: see [`run`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub attempts: Vec<Attempt>,
    pub watched: bool,
    /// Why the first way is the one tried first, for the log and the crash reports.
    pub because: Because,
}

/// Why a plan's first way is tried first (#280: a report drawn with OpenGL where Direct3D is the
/// default must say why).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Because {
    /// Chosen in View → Draw with.
    Chosen,
    /// Nothing chosen (or another system's API): this system's default.
    Default,
    /// `/renderer` or `MDM_RENDERER`.
    Override,
    /// The system's API is the choice, but it failed here before: OpenGL until it is chosen
    /// again.
    FailedBefore,
}

impl Because {
    /// Its words after "Drawing with: …" (`api`: the system's API's name).
    fn words(self, api: &str) -> String {
        match self {
            Because::Chosen => "chosen in View → Draw with".into(),
            Because::Default => "the default here".into(),
            Because::Override => "the testing override (/renderer, MDM_RENDERER)".into(),
            Because::FailedBefore => format!(
                "{api} failed here before: {}",
                FAILED_DETAILS
                    .lock()
                    .ok()
                    .and_then(|d| d.clone())
                    .unwrap_or_default()
            ),
        }
    }
}

/// What to try: the override if any, else the setting (or [`DEFAULT`]) unless the system's API
/// failed here before; then the other way. A graphics card's API falls back to OpenGL, then to
/// the API on a software renderer (Remote Desktop, a virtual machine: no card's driver, and
/// Windows' own OpenGL 1.1 can't draw MDM). Only OpenGL where wgpu isn't built.
pub fn plan(setting: Option<Api>, overridden: Option<&str>, failed_before: bool) -> Plan {
    let forced = overridden.and_then(|o| {
        let a = parse(o);
        if a.is_none() {
            tracing::warn!(
                "unknown renderer {o:?} (here: opengl{}): not used",
                GPU.map(|g| format!(", {}, software", g.key()))
                    .unwrap_or_default()
            );
        }
        a
    });
    let wants_gpu = Some(chosen(setting)) == GPU;
    let (first, because) = match forced {
        Some(a) => (a, Because::Override),
        None if wants_gpu && failed_before => (Attempt::OpenGl, Because::FailedBefore),
        None => (
            if wants_gpu {
                Attempt::Gpu(Adapters::Hardware)
            } else {
                Attempt::OpenGl
            },
            // Only this system's own choices count as chosen (chosen()).
            if setting.is_some_and(|s| s == chosen(setting)) {
                Because::Chosen
            } else {
                Because::Default
            },
        ),
    };
    let then: &[Attempt] = match first {
        Attempt::OpenGl => &[Attempt::Gpu(Adapters::Any)],
        Attempt::Gpu(Adapters::Hardware) if forced.is_none() => {
            &[Attempt::OpenGl, Attempt::Gpu(Adapters::Software)]
        }
        Attempt::Gpu(_) => &[Attempt::OpenGl],
    };
    let attempts: Vec<Attempt> = std::iter::once(first)
        .chain(then.iter().copied())
        .filter(|a| GPU.is_some() || *a == Attempt::OpenGl)
        .collect();
    Plan {
        watched: forced.is_none() && attempts.first() == Some(&Attempt::Gpu(Adapters::Hardware)),
        attempts,
        because,
    }
}

/// The testing override: `/renderer <name>` (or `--renderer`) among the arguments, else
/// `MDM_RENDERER`.
pub fn overridden(args: &[String]) -> Option<String> {
    args.iter()
        .position(|a| {
            a.trim_start_matches(['/', '-'])
                .eq_ignore_ascii_case("renderer")
        })
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("MDM_RENDERER").ok())
        .filter(|s| !s.trim().is_empty())
}

// ---- Remembering a start that never drew --------------------------------------------------

/// Written before a watched start with the system's API, removed once its first frame is drawn.
const STARTING: &str = "starting";
/// Why the system's API couldn't draw here; while it is there, OpenGL draws.
const FAILED: &str = "failed.txt";

/// Why the system's API couldn't draw here, for View → Draw with's note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// No adapter for it (no driver; only a software renderer when a card was wanted), or no
    /// way to draw to the window with it.
    NoAdapter,
    /// Its first frame didn't come within [`FIRST_FRAME_WITHIN`].
    NoFirstFrame,
    /// A start with it ended before its first frame (a crash or a kill in the driver).
    EndedEarly,
    /// The graphics card was lost twice within 10 minutes.
    LostTwice,
    /// Anything else (the log has eframe's error).
    Other,
}

impl Why {
    const ALL: [Why; 5] = [
        Why::NoAdapter,
        Why::NoFirstFrame,
        Why::EndedEarly,
        Why::LostTwice,
        Why::Other,
    ];

    /// Its first line in the failure file.
    fn key(self) -> &'static str {
        match self {
            Why::NoAdapter => "no-adapter",
            Why::NoFirstFrame => "no-first-frame",
            Why::EndedEarly => "ended-early",
            Why::LostTwice => "lost-twice",
            Why::Other => "other",
        }
    }

    /// A failure file's text: its reason's key, then the details on the next line. An older
    /// one (one line of details, from test builds before 0.5.0 on Windows) is `Other`.
    fn read(text: &str) -> (Why, &str) {
        let (first, rest) = text.trim().split_once('\n').unwrap_or((text.trim(), ""));
        match Why::ALL.into_iter().find(|w| w.key() == first.trim()) {
            Some(w) => (w, rest.trim()),
            None => (Why::Other, text.trim()),
        }
    }
}

/// The system's API failed here (before this start, or in it) and wasn't chosen again since:
/// why.
static GPU_FAILED: Mutex<Option<Why>> = Mutex::new(None);

/// The details of that failure, as its file has them (for the crash reports).
static FAILED_DETAILS: Mutex<Option<String>> = Mutex::new(None);

fn set_failed(why: Option<Why>) {
    if let Ok(mut f) = GPU_FAILED.lock() {
        *f = why;
    }
}

/// For View → Draw with's note: why the system's API doesn't draw here, if it failed.
#[cfg(gpu)]
pub fn gpu_failed() -> Option<Why> {
    GPU_FAILED.lock().ok().and_then(|f| *f)
}

/// The system's API's file `name` in the data folder (`direct3d-failed.txt`,
/// `vulkan-starting`…). None where there is no such API.
fn file(name: &str) -> Option<PathBuf> {
    let key = GPU?.key();
    mdm_ipc::data_dir().map(|d| d.join(format!("{key}-{name}")))
}

/// Why the system's API failed here before, if it did. A start that left its marker behind
/// ended before its first frame: that becomes the reason.
pub fn failed_before() -> Option<String> {
    let (starting, failed) = (file(STARTING)?, file(FAILED)?);
    let api = GPU?.name();
    if starting.exists() {
        let why = format!("the last start with {api} ended before its first frame");
        tracing::warn!("{api}: {why}; drawing with OpenGL until it is chosen again");
        remember_failure(Why::EndedEarly, &why);
        let _ = std::fs::remove_file(&starting);
    }
    let text = std::fs::read_to_string(&failed).ok()?;
    let (why, details) = Why::read(&text);
    tracing::info!("{api} failed here before ({why:?}: {details})");
    set_failed(Some(why));
    if let Ok(mut d) = FAILED_DETAILS.lock() {
        *d = Some(details.to_owned());
    }
    Some(details.to_owned())
}

/// Has the system's API failed here, or is a start with it not yet drawn? Only looks (the crash
/// monitor: its report may be about that start).
pub fn suspect() -> bool {
    [STARTING, FAILED]
        .into_iter()
        .filter_map(file)
        .any(|f| f.exists())
}

/// The system's API could not start: remembered with its reason.
fn remember_failure(why: Why, details: &str) {
    if let Some(f) = file(FAILED) {
        let _ = std::fs::write(f, format!("{}\n{details}", why.key()));
    }
}

/// The system's API chosen again (View → Draw with): its next start is tried.
#[cfg(gpu)]
pub fn forget_failure() {
    set_failed(None);
    for name in [FAILED, STARTING] {
        if let Some(f) = file(name) {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// A test's failure asked for (`name`=1), honoured only by off-screen test copies.
fn testing(name: &str) -> bool {
    crate::offscreen() && std::env::var_os(name).is_some_and(|v| v == "1")
}

/// MDM_TEST_SLOW_START=<ms> (off-screen test copies): making the app takes that much longer, as
/// on a slow computer just after it starts (#280, the first frame's clock).
pub fn test_slow_start() {
    let ms = std::env::var("MDM_TEST_SLOW_START")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|_| crate::offscreen());
    if let Some(ms) = ms {
        tracing::info!("testing: making the app {ms} ms slower");
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// The start of every main-window pass: the system's API's device checked (see `gpu::poll`).
/// The pass itself is MDM's own work, not the API's (see [`FirstFrameClock`]).
pub fn frame_begin() {
    own_work_begin();
    #[cfg(gpu)]
    gpu::poll();
}

/// The first frame was drawn (presented).
static DRAWN: AtomicBool = AtomicBool::new(false);

/// The end of every main-window pass. Once a frame has been completed before this one (egui's
/// frame count, not its passes: a first frame can run two), the first was drawn: a watched
/// start made it.
pub fn frame_done(ctx: &egui::Context) {
    own_work_end();
    if DRAWN.load(Ordering::Relaxed) || ctx.cumulative_frame_nr() == 0 {
        return;
    }
    // MDM_TEST_NO_FIRST_FRAME=1 (off-screen test copies): a first frame that never comes (the
    // watchdog's check).
    if testing("MDM_TEST_NO_FIRST_FRAME") {
        return;
    }
    DRAWN.store(true, Ordering::Relaxed);
    let api = clock_counted().map_or(String::new(), |d| {
        format!(" ({} ms of it counted for the API)", d.as_millis())
    });
    tracing::info!("first frame drawn, {} ms after start{api}", since_start());
    if let Some(f) = file(STARTING) {
        let _ = std::fs::remove_file(f);
    }
}

// ---- The first frame's clock ----------------------------------------------------------------

/// How long a watched start has been the graphics API's to bring its first frame: the time from
/// the start of [`run`] to the first frame, less MDM's own work meanwhile (making the app:
/// loading the list, the fonts, the icons; and its passes' logic and layout), which isn't the
/// API's. A slow computer just after it starts can spend more than [`FIRST_FRAME_WITHIN`] there
/// alone, and that was counted as Direct3D drawing no first frame: MDM started again with OpenGL
/// and kept it (#280).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FirstFrameClock {
    /// Counted before the stretch now running.
    counted: std::time::Duration,
    /// Since when it runs; None while MDM's own work pauses it.
    since: Option<std::time::Instant>,
}

impl FirstFrameClock {
    fn started(now: std::time::Instant) -> Self {
        FirstFrameClock {
            counted: std::time::Duration::ZERO,
            since: Some(now),
        }
    }

    /// MDM's own work begins: not counted.
    fn pause(&mut self, now: std::time::Instant) {
        if let Some(since) = self.since.take() {
            self.counted += now.saturating_duration_since(since);
        }
    }

    /// MDM's own work is done: the API's turn again.
    fn resume(&mut self, now: std::time::Instant) {
        self.since.get_or_insert(now);
    }

    fn counted(&self, now: std::time::Instant) -> std::time::Duration {
        self.counted
            + self.since.map_or(std::time::Duration::ZERO, |s| {
                now.saturating_duration_since(s)
            })
    }
}

/// The watched start's clock (None when no start is watched).
static CLOCK: Mutex<Option<FirstFrameClock>> = Mutex::new(None);

fn with_clock<R>(f: impl FnOnce(&mut FirstFrameClock, std::time::Instant) -> R) -> Option<R> {
    let mut c = CLOCK.lock().ok()?;
    let now = std::time::Instant::now();
    c.as_mut().map(|c| f(c, now))
}

/// MDM's own work begins (the app being made, a pass's logic and layout): the first-frame clock
/// stops until [`own_work_end`].
pub fn own_work_begin() {
    if !DRAWN.load(Ordering::Relaxed) {
        with_clock(|c, now| c.pause(now));
    }
}

/// MDM's own work is done: the first-frame clock runs again.
pub fn own_work_end() {
    if !DRAWN.load(Ordering::Relaxed) {
        with_clock(|c, now| c.resume(now));
    }
}

/// The time counted for the API so far (a watched start only).
fn clock_counted() -> Option<std::time::Duration> {
    with_clock(|c, now| c.counted(now))
}

// ---- What draws now ---------------------------------------------------------------------------

static START: OnceLock<std::time::Instant> = OnceLock::new();

/// The program started (first thing in `main`).
pub fn mark_start() {
    let _ = START.set(std::time::Instant::now());
}

fn since_start() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

/// What draws now (set once the window started), for View → Draw with.
static ACTIVE: OnceLock<Api> = OnceLock::new();

#[cfg(gpu)]
pub fn active() -> Option<Api> {
    ACTIVE.get().copied()
}

/// Why a start with the system's API failed, from eframe's error: no adapter or surface for it
/// (no driver, `VK_ICD_FILENAMES` pointing at nothing, only a software renderer when a card was
/// wanted), or something else.
fn why_failed(e: &eframe::Error) -> Why {
    #[cfg(gpu)]
    {
        use eframe::egui_wgpu::WgpuError;
        if let eframe::Error::Wgpu(
            WgpuError::CustomNativeAdapterSelectionError(_)
            | WgpuError::RequestAdapterError(_)
            | WgpuError::CreateSurfaceError(_),
        ) = e
        {
            return Why::NoAdapter;
        }
    }
    let _ = e;
    Why::Other
}

/// Run eframe's window with each way of the plan until one starts. `options` makes the window's
/// options (each try needs its own); `creator` makes the app, once (a try that fails before
/// that leaves it for the next). Returns the last try's result.
pub fn run<A: eframe::App + 'static>(
    name: &str,
    plan: &Plan,
    options: impl Fn() -> eframe::NativeOptions,
    creator: impl FnOnce(&eframe::CreationContext<'_>) -> A,
) -> eframe::Result {
    let creator = std::rc::Rc::new(std::cell::RefCell::new(Some(creator)));
    let mut result = Ok(());
    let mut instead: Option<String> = None;
    for (n, &attempt) in plan.attempts.iter().enumerate() {
        if n == 0
            && plan.watched
            && let Some(f) = file(STARTING)
        {
            let _ = std::fs::write(f, "");
        }
        // A watched start must show its first frame in time, or MDM starts again.
        #[cfg(gpu)]
        if n == 0 && plan.watched {
            if let Ok(mut c) = CLOCK.lock() {
                *c = Some(FirstFrameClock::started(std::time::Instant::now()));
            }
            gpu::watch_first_frame();
        }
        #[cfg(gpu)]
        gpu::TRYING.store(attempt != Attempt::OpenGl, Ordering::Relaxed);
        let mut opts = options();
        configure(&mut opts, attempt);
        tracing::info!("drawing with {}", attempt.name());
        let make = creator.clone();
        // Why this way: the plan's reason for its first, the last one's failure for another.
        let why_not = match &instead {
            Some(w) => w.clone(),
            None => plan.because.words(GPU.map_or("", |g| g.name())),
        };
        result = eframe::run_native(
            name,
            opts,
            Box::new(move |cc| {
                let make = make.borrow_mut().take().ok_or("the app was already made")?;
                started(cc, attempt, why_not);
                // Making the app is MDM's own work, not the API's: off the first frame's clock.
                own_work_begin();
                let app = make(cc);
                own_work_end();
                Ok(Box::new(app))
            }),
        );
        // A failure after the app was made is the app's own (or a later one): no retry.
        let Err(e) = &result else { break };
        if creator.borrow().is_none() {
            break;
        }
        let why = format!("{} didn't start: {e}", attempt.name());
        tracing::warn!("{why}");
        if attempt != Attempt::OpenGl {
            let reason = why_failed(e);
            set_failed(Some(reason));
            if n == 0 && plan.watched {
                remember_failure(reason, &format!("{e}"));
                if let Some(f) = file(STARTING) {
                    let _ = std::fs::remove_file(f);
                }
            }
        }
        instead = Some(why);
    }
    result
}

/// The window's options for one way of drawing.
fn configure(opts: &mut eframe::NativeOptions, attempt: Attempt) {
    match attempt {
        Attempt::OpenGl => {
            opts.renderer = eframe::Renderer::Glow;
            // MDM_TEST_NO_OPENGL=1 (off-screen test copies): a pixel format no OpenGL has, as on
            // a computer whose only OpenGL is Windows' own 1.1 (Remote Desktop, a VM).
            if testing("MDM_TEST_NO_OPENGL") {
                opts.multisampling = 255;
                opts.depth_buffer = 255;
                opts.stencil_buffer = 255;
            }
        }
        #[cfg(gpu)]
        Attempt::Gpu(adapters) => {
            opts.renderer = eframe::Renderer::Wgpu;
            opts.wgpu_options = gpu::options(adapters);
        }
        // Not built here (plan() never asks for it).
        #[cfg(not(gpu))]
        Attempt::Gpu(_) => opts.renderer = eframe::Renderer::Glow,
    }
}

/// The app is being made: what draws is known.
/// `why`: why this way (the plan's reason, or why the way before it didn't start).
fn started(cc: &eframe::CreationContext<'_>, attempt: Attempt, why: String) {
    #[cfg(gpu)]
    let gpu = cc
        .wgpu_render_state
        .as_ref()
        .map(|rs| gpu::describe(&rs.adapter.get_info()));
    #[cfg(not(gpu))]
    let gpu: Option<String> = None;
    // OpenGL names its card and driver too (#280: a freeze inside Intel's OpenGL driver came
    // with "OpenGL" alone).
    #[cfg(unix)]
    use crate::unix::gl_driver;
    #[cfg(windows)]
    use crate::win32::gl_driver;
    let detail = gpu
        .or_else(|| {
            cc.gl
                .as_ref()
                .map(|gl| format!("{} on {}", attempt.name(), gl_driver(gl)))
        })
        .unwrap_or_else(|| attempt.name().to_owned());
    tracing::info!(
        "drawing with {detail}, {} ms after start ({why})",
        since_start()
    );
    // The crash reports say it, and why ("OpenGL on Intel(R) UHD Graphics 620 (4.6.0 - Build …)
    // — Direct3D 12 failed here before: …").
    crate::crash::renderer(&format!("{detail} — {why}"));
    // Errors are logged, not panics, and a lost graphics card starts MDM again.
    #[cfg(gpu)]
    if let Some(rs) = &cc.wgpu_render_state {
        gpu::watch_device(&rs.device);
    }
    let _ = ACTIVE.set(attempt.api());
}

/// `/show renderer-lost` (off-screen test copies): the system's API's device lost on purpose
/// (MDM starts again).
#[cfg(gpu)]
pub fn test_device_lost() {
    gpu::lose_device();
}

/// eframe's wgpu set up for the system's API only: Direct3D 12, Vulkan or Metal.
#[cfg(gpu)]
mod gpu {
    use super::{
        Adapters, DRAWN, FIRST_FRAME_WITHIN, GPU, Why, clock_counted, file, remember_failure,
        testing,
    };
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime};

    /// The API's name in the log and the crash reports.
    fn api() -> &'static str {
        GPU.map_or("wgpu", |g| g.name())
    }

    /// The try under way is the system's API's (a failed one goes on to OpenGL in the same
    /// process).
    pub static TRYING: AtomicBool = AtomicBool::new(false);

    /// The first frame of this start must come within [`FIRST_FRAME_WITHIN`]; if it doesn't (a
    /// driver stuck in its first present, a window left black), the API is remembered as failed
    /// and MDM starts again, drawing with OpenGL.
    pub fn watch_first_frame() {
        let _ = std::thread::Builder::new()
            .name("first frame watch".into())
            .spawn(|| {
                // The API's time only: MDM's own work meanwhile isn't counted (#280).
                let over = || clock_counted().is_some_and(|d| d >= FIRST_FRAME_WITHIN);
                while !over() {
                    if DRAWN.load(Ordering::Relaxed) || !TRYING.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if DRAWN.load(Ordering::Relaxed) || !TRYING.load(Ordering::Relaxed) {
                    return;
                }
                let why = format!(
                    "{} drew no first frame within {} s ({} s after start)",
                    api(),
                    FIRST_FRAME_WITHIN.as_secs(),
                    super::since_start() / 1000
                );
                remember_failure(Why::NoFirstFrame, &why);
                // Remembered as this reason, not as a start that ended before its first frame.
                if let Some(f) = file(super::STARTING) {
                    let _ = std::fs::remove_file(f);
                }
                start_again(&why);
            });
    }

    /// When the graphics card is lost twice this close together, the API counts as failing.
    const LOST_AGAIN_WITHIN: Duration = Duration::from_secs(600);
    /// When the card was last lost (its time inside).
    const LOST: &str = "lost";

    /// wgpu's errors go to the log instead of ending MDM (its default handler panics), and a
    /// device lost while MDM runs (a driver update or reset: none of the three APIs recovers
    /// by itself) starts MDM again, with the same API, or with OpenGL after a second loss
    /// within 10 minutes.
    pub fn watch_device(device: &wgpu::Device) {
        device.on_uncaptured_error(Arc::new(|e| {
            tracing::error!("{}: {e}", api());
        }));
        device.set_device_lost_callback(|reason, message| {
            let testing = TEST_LOSS.load(Ordering::Relaxed);
            if reason != wgpu::DeviceLostReason::Unknown && !testing {
                return;
            }
            let why = format!("the graphics card was lost ({reason:?}: {message})");
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let last = file(LOST)
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|t| t.trim().parse::<u64>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOST_AGAIN_WITHIN.as_secs()) {
                remember_failure(Why::LostTwice, &format!("{why}, twice within 10 minutes"));
            } else if let Some(f) = file(LOST) {
                let _ = std::fs::write(f, now.to_string());
            }
            start_again(&why);
        });
        let _ = DEVICE.set(device.clone());
    }

    static DEVICE: std::sync::OnceLock<wgpu::Device> = std::sync::OnceLock::new();

    /// Each main-window pass, before its frame is painted: wgpu notices a lost device here and
    /// calls [`watch_device`]'s callback, before painting with it would fail.
    pub fn poll() {
        if let Some(d) = DEVICE.get() {
            let _ = d.poll(wgpu::PollType::Poll);
        }
    }
    static TEST_LOSS: AtomicBool = AtomicBool::new(false);

    /// `/show renderer-lost` (off-screen test copies): lose the device on purpose.
    pub fn lose_device() {
        if let Some(d) = DEVICE.get() {
            TEST_LOSS.store(true, Ordering::Relaxed);
            d.destroy();
            poll();
        }
    }

    /// End this copy and start MDM again: the new one waits for this one to be gone (as after
    /// an update), and draws as the failure markers now say.
    fn start_again(why: &str) {
        tracing::warn!("{why}: starting MDM again");
        // The AppImage file itself on Linux (this program sits in its temporary mount).
        #[cfg(windows)]
        let program = std::env::current_exe().ok();
        #[cfg(unix)]
        let program = crate::unix::program();
        let started = program.and_then(|exe| {
            std::process::Command::new(exe)
                .arg(crate::update::UPDATED)
                // An override would bring the same failure back.
                .env_remove("MDM_RENDERER")
                .spawn()
                .ok()
        });
        if started.is_none() {
            tracing::error!("MDM couldn't start again");
        }
        crate::crash::clean_exit();
        // The window thread may be stuck in the driver: end at once, without unwinding.
        #[cfg(windows)]
        crate::win32::end_process(std::process::id());
        #[cfg(unix)]
        crate::unix::end_process(std::process::id());
        std::process::exit(1);
    }

    pub fn options(adapters: Adapters) -> WgpuConfiguration {
        let mut setup = WgpuSetupCreateNew::without_display_handle();
        let desc = &mut setup.instance_descriptor;
        // The system's API only, and nothing from WGPU_* variables: what draws is MDM's choice.
        desc.flags = wgpu::InstanceFlags::from_build_config();
        #[cfg(windows)]
        {
            desc.backends = wgpu::Backends::DX12;
            desc.backend_options = wgpu::BackendOptions {
                dx12: wgpu::Dx12BackendOptions {
                    // Windows' own compiler (d3dcompiler_47.dll): no dxcompiler.dll looked for
                    // on the PATH, where any program could have put one.
                    shader_compiler: wgpu::Dx12Compiler::Fxc,
                    ..Default::default()
                },
                ..Default::default()
            };
        }
        // Vulkan's loader still reads its own variables (VK_ICD_FILENAMES, VK_LOADER_*): the
        // system's way to pick a driver, and the lab's to take it away (#206).
        #[cfg(target_os = "linux")]
        {
            desc.backends = wgpu::Backends::VULKAN;
        }
        #[cfg(target_os = "macos")]
        {
            desc.backends = wgpu::Backends::METAL;
        }
        setup.power_preference = wgpu::PowerPreference::LowPower;
        setup.native_adapter_selector = Some(Arc::new(move |list, surface| {
            let kinds: Vec<(wgpu::DeviceType, bool)> = list
                .iter()
                .map(|a| {
                    let fits = surface.is_none_or(|s| a.is_surface_supported(s));
                    (a.get_info().device_type, fits)
                })
                .collect();
            match super::choose(&kinds, adapters, cfg!(target_os = "macos")) {
                Some(i) => Ok(list[i].clone()),
                None => Err(format!(
                    "no {} {} adapter among [{}]",
                    match adapters {
                        Adapters::Hardware => "hardware",
                        Adapters::Software => "software",
                        Adapters::Any => "usable",
                        Adapters::Nothing => "(none wanted)",
                    },
                    api(),
                    list.iter()
                        .map(|a| describe(&a.get_info()))
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
            }
        }));
        setup.device_descriptor = Arc::new(|_adapter| wgpu::DeviceDescriptor {
            label: Some("MDM"),
            required_limits: wgpu::Limits {
                // As eframe asks: windows as large as 4K and more.
                max_texture_dimension_2d: 8192,
                // Direct3D 12 keeps one descriptor heap this large, in the graphics card's memory
                // and a copy in MDM's: wgpu's default (a million, 32 MB each) is for games. egui
                // uses one per texture (icons, fonts, file icons): a few hundred. (Vulkan and
                // Metal size their pools by use; the cap costs them nothing.)
                max_non_sampler_bindings: 65_536,
                ..wgpu::Limits::default()
            },
            // Small memory blocks: a download manager's windows need little, and it often sits
            // in the tray.
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        });
        // A software renderer as a card (MDM_TEST_SOFTWARE_CARD=1) is said in the log.
        if testing("MDM_TEST_SOFTWARE_CARD") {
            tracing::info!("testing: a software renderer counts as a graphics card");
        }
        WgpuConfiguration {
            wgpu_setup: WgpuSetup::CreateNew(setup),
            ..Default::default()
        }
    }

    pub fn describe(info: &wgpu::AdapterInfo) -> String {
        let mut s = format!("{} on {}", api(), info.name);
        let driver = [&info.driver, &info.driver_info]
            .into_iter()
            .filter(|d| !d.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if !driver.is_empty() {
            s.push_str(&format!(" (driver {driver})"));
        }
        if info.device_type == wgpu::DeviceType::Cpu {
            s.push_str(" [software]");
        }
        s
    }
}

/// The adapter to draw with, among (kind, can draw to the window) in the system's order (the
/// one showing the main screen first on Windows; Mesa's device-select layer puts the default
/// one, or `DRI_PRIME`'s, first on Linux): the first fitting one of the kinds `adapters`
/// allows. `integrated_first` (macOS, whose order puts no card first): of the cards, a built-in
/// one before a separate one, as macOS itself picks for a program that asks for little, so a
/// Mac's separate card can sleep.
#[cfg(gpu)]
fn choose(
    list: &[(eframe::wgpu::DeviceType, bool)],
    adapters: Adapters,
    integrated_first: bool,
) -> Option<usize> {
    use eframe::wgpu::DeviceType;
    // MDM_TEST_NO_HARDWARE=1 (off-screen test copies): no graphics card's driver, only a
    // software renderer, as over Remote Desktop. MDM_TEST_SOFTWARE_CARD=1: the other way, a
    // software renderer taken for a card (lavapipe in the lab's virtual machines, which have no
    // graphics card, to check what a card's start does).
    let no_card = testing("MDM_TEST_NO_HARDWARE");
    let soft_card = testing("MDM_TEST_SOFTWARE_CARD");
    let hardware = |t: DeviceType| (t != DeviceType::Cpu || soft_card) && !no_card;
    let integrated = |t: DeviceType| t == DeviceType::IntegratedGpu && !no_card;
    let software = |t: DeviceType| t == DeviceType::Cpu;
    let first =
        |want: &dyn Fn(DeviceType) -> bool| list.iter().position(|&(t, fits)| fits && want(t));
    let card = || {
        integrated_first
            .then(|| first(&integrated))
            .flatten()
            .or_else(|| first(&hardware))
    };
    match adapters {
        Adapters::Hardware => card(),
        Adapters::Software => first(&software),
        Adapters::Any => card().or_else(|| first(&software)),
        Adapters::Nothing => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_or_an_override_then_the_other_way() {
        let gpu = Attempt::Gpu(Adapters::Hardware);
        let any = Attempt::Gpu(Adapters::Any);
        let soft = Attempt::Gpu(Adapters::Software);
        let Some(api) = GPU else {
            // No wgpu here: OpenGL whatever was asked.
            for p in [
                plan(None, None, false),
                plan(Some(Api::Vulkan), None, false),
                plan(None, Some("gpu"), false),
            ] {
                assert_eq!(p.attempts, [Attempt::OpenGl]);
                assert!(!p.watched);
            }
            return;
        };
        // The system's API chosen: it, watched; OpenGL behind it, then a software renderer.
        let p = plan(Some(api), None, false);
        assert_eq!(p.attempts, [gpu, Attempt::OpenGl, soft]);
        assert!(p.watched);
        // ... unless it failed here before.
        let p = plan(Some(api), None, true);
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        assert!(!p.watched);
        // Nothing chosen: Direct3D on Windows (#89), OpenGL on Linux and macOS (#206).
        let p = plan(None, None, false);
        if cfg!(windows) {
            assert_eq!(p.attempts, [gpu, Attempt::OpenGl, soft]);
            assert!(p.watched);
            assert_eq!(plan(None, None, true).attempts, [Attempt::OpenGl, any]);
        } else {
            assert_eq!(p.attempts, [Attempt::OpenGl, any]);
            assert!(!p.watched);
        }
        // OpenGL chosen: kept, the system's API only if OpenGL can't start.
        let p = plan(Some(Api::OpenGl), None, false);
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        assert!(!p.watched);
        // Another system's API chosen (a settings file from another computer): the default.
        for other in [Api::Direct3D, Api::Vulkan, Api::Metal]
            .into_iter()
            .filter(|&a| a != api)
        {
            assert_eq!(plan(Some(other), None, false), plan(None, None, false));
            assert_eq!(chosen(Some(other)), DEFAULT);
        }
        // Overrides win, failure or not, and aren't watched: by the API's own name or `gpu`.
        for name in [api.key(), "gpu", "GPU"] {
            let p = plan(Some(Api::OpenGl), Some(name), true);
            assert_eq!(p.attempts, [gpu, Attempt::OpenGl]);
            assert!(!p.watched);
        }
        let p = plan(Some(api), Some("gl"), false);
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        let p = plan(None, Some("software"), false);
        assert_eq!(p.attempts, [soft, Attempt::OpenGl]);
        // An unknown name, or another system's API, is ignored.
        let default = plan(None, None, false).attempts[0];
        assert_eq!(plan(None, Some("glide"), false).attempts[0], default);
        for other in ["direct3d", "vulkan", "metal"]
            .into_iter()
            .filter(|&n| n != api.key())
        {
            assert_eq!(plan(None, Some(other), false).attempts[0], default);
        }
        // Each system's own names.
        let names: &[(&str, Attempt)] = match api {
            Api::Direct3D => &[("d3d", gpu), ("dx12", gpu), ("warp", soft)],
            Api::Vulkan => &[("vk", gpu), ("lavapipe", soft), ("vulkan-software", soft)],
            _ => &[("metal", gpu)],
        };
        for &(name, want) in names {
            assert_eq!(plan(None, Some(name), false).attempts[0], want, "{name}");
        }
    }

    /// Windows' plans are what they were before Vulkan and Metal (#206): the same attempts for
    /// every setting, failure and override.
    #[cfg(windows)]
    #[test]
    fn windows_plans_unchanged() {
        let d3d = Attempt::Gpu(Adapters::Hardware);
        let any = Attempt::Gpu(Adapters::Any);
        let warp = Attempt::Gpu(Adapters::Software);
        let gl = Attempt::OpenGl;
        for setting in [None, Some(Api::Direct3D)] {
            assert_eq!(plan(setting, None, false).attempts, [d3d, gl, warp]);
            assert_eq!(plan(setting, None, true).attempts, [gl, any]);
        }
        assert_eq!(plan(Some(Api::OpenGl), None, false).attempts, [gl, any]);
        assert_eq!(
            plan(Some(Api::OpenGl), Some("D3D"), true).attempts,
            [d3d, gl]
        );
        assert_eq!(plan(None, Some("warp"), false).attempts, [warp, gl]);
        assert_eq!(plan(None, Some("vulkan"), false).attempts[0], d3d);
        // Its files keep their names.
        if let Some(f) = file(FAILED) {
            assert!(f.ends_with("direct3d-failed.txt"));
        }
        assert_eq!(
            Attempt::Gpu(Adapters::Software).name(),
            "Direct3D 12 (WARP)"
        );
    }

    #[test]
    fn override_from_the_command_line() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(overridden(&a("/renderer d3d")).as_deref(), Some("d3d"));
        assert_eq!(
            overridden(&a("--renderer warp /n")).as_deref(),
            Some("warp")
        );
    }

    #[test]
    fn settings_names() {
        for (api, name) in [
            (Api::OpenGl, "opengl"),
            (Api::Direct3D, "direct3d"),
            (Api::Vulkan, "vulkan"),
            (Api::Metal, "metal"),
        ] {
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&api).unwrap(), json);
            assert_eq!(serde_json::from_str::<Api>(&json).unwrap(), api);
        }
    }

    #[test]
    fn failure_reasons_in_their_file() {
        for why in Why::ALL {
            let text = format!("{}\nsome details", why.key());
            assert_eq!(Why::read(&text), (why, "some details"));
        }
        // The files of test builds before 0.5.0 on Windows: one line of details.
        let old = "Direct3D drew no first frame within 10 s";
        assert_eq!(Why::read(old), (Why::Other, old));
        assert_eq!(Why::read("  \n"), (Why::Other, ""));
    }

    #[cfg(gpu)]
    #[test]
    fn adapters_in_the_systems_order() {
        use eframe::wgpu::DeviceType::*;
        // The main screen's card first; one that can't draw to the window is passed over.
        let l = [(IntegratedGpu, false), (DiscreteGpu, true), (Cpu, true)];
        assert_eq!(choose(&l, Adapters::Hardware, false), Some(1));
        assert_eq!(choose(&l, Adapters::Software, false), Some(2));
        assert_eq!(choose(&l, Adapters::Any, false), Some(1));
        // No card: a software renderer only when allowed.
        let warp = [(Cpu, true)];
        assert_eq!(choose(&warp, Adapters::Hardware, false), None);
        assert_eq!(choose(&warp, Adapters::Any, false), Some(0));
        assert_eq!(choose(&l, Adapters::Nothing, false), None);
        // macOS: the built-in card before a separate one, whatever the order; the separate one
        // when it is the only one that fits.
        let mac = [(DiscreteGpu, true), (IntegratedGpu, true)];
        assert_eq!(choose(&mac, Adapters::Hardware, true), Some(1));
        assert_eq!(choose(&mac, Adapters::Hardware, false), Some(0));
        assert_eq!(choose(&l, Adapters::Hardware, true), Some(1));
        assert_eq!(choose(&[(VirtualGpu, true)], Adapters::Any, true), Some(0));
    }

    /// The first frame's clock counts the API's time only: MDM's own work (making the app, its
    /// passes) is left out, so a slow start of MDM's isn't taken for Direct3D failing (#280: 10.7
    /// s from OpenGL's context to the first frame on the reporter's computer, at the 10 s limit).
    #[test]
    fn the_first_frame_clock_leaves_out_mdms_own_work() {
        use std::time::{Duration, Instant};
        let s = |n: u64| Duration::from_secs(n);
        let t0 = Instant::now();
        let mut c = FirstFrameClock::started(t0);
        // The window and the device: 2 s, the API's.
        assert_eq!(c.counted(t0 + s(2)), s(2));
        // The app made in 12 s: not counted, however long.
        c.pause(t0 + s(2));
        assert_eq!(c.counted(t0 + s(14)), s(2));
        c.resume(t0 + s(14));
        // Its first pass, 3 s of MDM's, then 1 s of painting and presenting.
        c.pause(t0 + s(14));
        c.resume(t0 + s(17));
        assert_eq!(c.counted(t0 + s(18)), s(3));
        assert!(c.counted(t0 + s(18)) < FIRST_FRAME_WITHIN);
        // A driver stuck in its first present is still caught: only its time runs.
        assert!(c.counted(t0 + s(25)) >= FIRST_FRAME_WITHIN);
        // Pausing twice, or resuming a running clock, changes nothing.
        let mut d = FirstFrameClock::started(t0);
        d.pause(t0 + s(1));
        d.pause(t0 + s(5));
        d.resume(t0 + s(6));
        d.resume(t0 + s(8));
        assert_eq!(d.counted(t0 + s(9)), s(4));
    }

    /// Why the first way is tried, for the log and the crash reports (#280).
    #[test]
    fn a_plan_says_why() {
        let Some(api) = GPU else {
            assert_eq!(
                plan(Some(Api::OpenGl), None, false).because,
                Because::Chosen
            );
            return;
        };
        assert_eq!(plan(None, None, false).because, Because::Default);
        assert_eq!(plan(Some(api), None, false).because, Because::Chosen);
        assert_eq!(
            plan(Some(Api::OpenGl), None, false).because,
            Because::Chosen
        );
        assert_eq!(plan(Some(Api::OpenGl), None, true).because, Because::Chosen);
        assert_eq!(plan(Some(api), None, true).because, Because::FailedBefore);
        assert_eq!(
            plan(Some(api), Some("opengl"), true).because,
            Because::Override
        );
        // The default fails like a choice where the default is the system's API (Windows).
        let failed_default = plan(None, None, true).because;
        if DEFAULT == api {
            assert_eq!(failed_default, Because::FailedBefore);
        } else {
            assert_eq!(failed_default, Because::Default);
        }
        assert_eq!(
            Because::Chosen.words("Direct3D 12"),
            "chosen in View → Draw with"
        );
        assert!(
            Because::FailedBefore
                .words("Direct3D 12")
                .starts_with("Direct3D 12 failed here before: ")
        );
    }
}
