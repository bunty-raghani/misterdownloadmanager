//! The end-to-end test's program (tests/e2e.rs): `crashy <freeze|panic|crash|thread-crash> <data
//! dir>` starts its monitor (itself with `--crash-monitor`), attaches the crash handler as MDM
//! does, and then freezes its main thread, panics, or crashes (on a thread of its own with
//! `thread-crash`). The monitor writes the dump and the report
//! (`<data>/crashes/*.txt`) and exits when this process is gone. With `CRASHY_MONITOR_LATE_MS` set,
//! the monitor listens that much later.
//!
//! Test code: it may use `unsafe` (the crash handler, a null write, the thread id), MDM's own code
//! keeps that in win32.rs / unix.rs.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_crash::client::Reporter;
use mdm_crash::crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
use mdm_crash::monitor::{self, Event};
use mdm_crash::protocol::{Freeze, Panic, Session};

fn main() {
    // What the connection and the monitor say goes to stderr, which the test leaves to the
    // terminal: a run that fails shows how far it got ("listening, 84 ms after start", "crash
    // reports are off for this session (…)"). MDM_LOG chooses more.
    // (Reading the dump, minidump's crates warn of what a report can do without.)
    let filter = std::env::var("MDM_LOG").unwrap_or_else(|_| {
        "warn,minidump=off,minidump_processor=off,minidump_unwind=off,mdm_crash=info".into()
    });
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_thread_names(true)
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .try_init();
    let args: Vec<String> = std::env::args().collect();
    if let Some(m) = mdm_crash::MonitorArgs::parse(&args) {
        run_monitor(m);
        return;
    }
    let mode = args[1].clone();
    let data = PathBuf::from(&args[2]);
    let exe = std::env::current_exe().unwrap();
    let reporter = Arc::new(Reporter::start(&exe, &data).expect("the monitor starts"));
    reporter.session(&Session {
        version: "test".into(),
        pid: std::process::id(),
        ..Default::default()
    });
    let r = reporter.clone();
    let handler = Arc::new(
        CrashHandler::attach(unsafe {
            make_crash_event(move |cc| CrashEventResult::Handled(r.dump(cc)))
        })
        .unwrap(),
    );
    #[cfg(target_os = "linux")]
    {
        handler.set_ptracer(reporter.monitor_pid());
        assert!(in_frame::keep_handler_in_frame());
        // All six go through it, with crash-handler's alternate stack and SA_SIGINFO.
        for sig in in_frame::SIGNALS {
            let sa = in_frame::action(sig).unwrap();
            assert!(in_frame::is_ours(&sa), "signal {sig}");
            assert_ne!(sa.sa_flags & libc::SA_ONSTACK, 0);
            assert_ne!(sa.sa_flags & libc::SA_SIGINFO, 0);
        }
        // A second call changes nothing.
        assert!(in_frame::keep_handler_in_frame());
    }
    // The monitor answers on a thread of its own once the first half second has passed (#1073):
    // a crash before then has no one to dump it, as in the app. The tests are about the reports,
    // so they wait for the connection first (a slow CI runner can take seconds to start it).
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while !reporter.connected() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    if !reporter.connected() {
        eprintln!("crashy: never connected to its monitor; {mode} goes unreported");
    }

    match mode.as_str() {
        "freeze" => {
            let lock = Arc::new(Mutex::new(()));
            let ui = os_thread_id();
            let (l, h, rep) = (lock.clone(), handler.clone(), reporter.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("watchdog".into())
                .spawn(move || {
                    let _held = l.lock().unwrap();
                    tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(500));
                    rep.freeze(&Freeze {
                        ui_thread: ui,
                        main: true,
                        seconds: 1,
                        busy: None,
                        step: String::new(),
                    });
                    simulate(&h);
                    // The dump is written while this thread waits for the answer: done.
                    rep.clean();
                    std::thread::sleep(Duration::from_millis(200));
                    std::process::exit(0);
                })
                .unwrap();
            rx.recv().unwrap();
            crashy_frozen_here(&lock);
        }
        "panic" => {
            let (h, rep) = (handler.clone(), reporter.clone());
            std::panic::set_hook(Box::new(move |info| {
                rep.panic(&Panic {
                    message: info.payload_as_str().unwrap_or("").to_owned(),
                    location: info.location().map(|l| l.to_string()).unwrap_or_default(),
                    thread: std::thread::current().name().unwrap_or("?").to_owned(),
                    backtrace: std::backtrace::Backtrace::force_capture().to_string(),
                });
                simulate(&h);
            }));
            crashy_panics_here();
        }
        "thread-crash" => {
            // A thread's alternate signal stack is crash-handler's own, mapped as it starts.
            let _ = std::thread::Builder::new()
                .name("crasher".into())
                .spawn(crashy_crashes_here)
                .unwrap()
                .join();
        }
        _ => crashy_crashes_here(),
    }
}

#[inline(never)]
fn crashy_frozen_here(lock: &Mutex<()>) {
    let _never = lock.lock();
    unreachable!();
}

#[inline(never)]
fn crashy_panics_here() {
    panic!("crashy: a test panic");
}

#[inline(never)]
fn crashy_crashes_here() {
    // A write through a null pointer: an access violation / SIGSEGV, caught natively.
    unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
}

fn simulate(h: &CrashHandler) {
    #[cfg(target_os = "linux")]
    h.simulate_signal(libc::SIGUSR2 as u32);
    #[cfg(any(windows, target_os = "macos"))]
    h.simulate_exception(None);
}

#[cfg(windows)]
fn os_thread_id() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(target_os = "linux")]
fn os_thread_id() -> u32 {
    unsafe { libc::gettid() as u32 }
}

#[cfg(target_os = "macos")]
fn os_thread_id() -> u32 {
    0
}

/// `CRASHY_MONITOR_LATE_MS`: the monitor waits this long before it listens, as one slow to start
/// does (a cold disk, a virus scan of a new program, a busy CI runner).
const MONITOR_LATE: &str = "CRASHY_MONITOR_LATE_MS";

fn run_monitor(m: mdm_crash::MonitorArgs) {
    if let Some(ms) = std::env::var(MONITOR_LATE)
        .ok()
        .and_then(|v| v.parse().ok())
    {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (socket, data) = (m.socket.clone(), m.data.clone());
    let (failed, gone) = (tx.clone(), tx.clone());
    std::thread::spawn(move || {
        if let Err(e) = monitor::serve(&socket, &data, tx) {
            let _ = failed.send(Event::ServeFailed(e));
        }
    });
    // Leave once crashy is gone, as MDM's monitor does: on macOS the connection a dump was
    // written for isn't seen to close (minidumper drops it without a word), and crashy's
    // monitors stayed behind the tests.
    #[cfg(unix)]
    {
        let parent = m.parent as libc::pid_t;
        std::thread::spawn(move || {
            // SAFETY: kill with signal 0 only asks whether the process is there.
            while unsafe { libc::kill(parent, 0) } == 0 {
                std::thread::sleep(Duration::from_millis(500));
            }
            let _ = gone.send(Event::Gone {
                clean: true,
                notes: Default::default(),
            });
        });
    }
    #[cfg(not(unix))]
    drop(gone);
    while let Ok(e) = rx.recv() {
        match e {
            Event::Dump { path, kind, notes } => {
                // The example's own debug info is beside it (no .sym store needed).
                mdm_crash::report::make(Some(&path), kind, notes, &m.data, &[]);
            }
            Event::DumpFailed { error, .. } => eprintln!("monitor: dump failed: {error}"),
            Event::Recovered => {}
            Event::ServeFailed(e) => {
                eprintln!("monitor: {e}");
                break;
            }
            Event::Gone { .. } => break,
        }
    }
}

// MDM's handler in front of crash-handler's, as the app has it (crates/app/src/unix.rs), so the
// crashes here go through it as MDM's do. A copy, not shared: the app keeps `unsafe` in its
// unix.rs (CLAUDE.md, rule 3); tests/in_frame.rs checks the two are the same.
// ---- The handler in front of crash-handler's (#1239). examples/crashy.rs in crates/crash keeps
// a copy of this module, from this line to the end mark, that crates/crash/tests/in_frame.rs
// checks is the same.
/// crash-handler's signal handler, kept inside the signal's frame where the kernel's is smaller
/// than the context it copies (no XSAVE: mdm_crash::frame says how). Linux only: crash-handler
/// takes Mach exceptions on macOS, with no signal frame.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub mod in_frame {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The signals crash-handler catches (its `EXCEPTION_SIGNALS`).
    pub const SIGNALS: [libc::c_int; 6] = [
        libc::SIGABRT,
        libc::SIGBUS,
        libc::SIGFPE,
        libc::SIGILL,
        libc::SIGSEGV,
        libc::SIGTRAP,
    ];

    type Handler = unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void);

    /// crash-handler's own handler, which [`in_frame`] calls; 0 until [`keep_handler_in_frame`]
    /// found it.
    static THEIRS: AtomicUsize = AtomicUsize::new(0);

    /// Put [`in_frame`] in front of crash-handler's handler; call it right after
    /// `CrashHandler::attach`. True when the signals go through it.
    pub fn keep_handler_in_frame() -> bool {
        let mut routed = false;
        for sig in SIGNALS {
            let Some(mut sa) = action(sig) else {
                continue;
            };
            if is_ours(&sa) {
                routed = true;
                continue;
            }
            // crash-handler installs one handler, with SA_SIGINFO, for all six: the first one
            // found is it (SIGABRT's, just attached); a signal left to another isn't touched.
            if sa.sa_flags & libc::SA_SIGINFO == 0 {
                continue;
            }
            let theirs = match THEIRS.compare_exchange(
                0,
                sa.sa_sigaction,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => sa.sa_sigaction,
                Err(found) => found,
            };
            if sa.sa_sigaction != theirs {
                continue;
            }
            // The same mask and flags (SA_ONSTACK, SA_SIGINFO), only the function is ours.
            sa.sa_sigaction = in_frame as Handler as usize;
            // SAFETY: `sa` is crash-handler's own action with `in_frame`, an `extern "C"`
            // handler of the SA_SIGINFO kind, in its place; THEIRS was set above, before a
            // signal can reach it.
            if unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) } == 0 {
                routed = true;
            }
        }
        routed
    }

    /// Signal `sig`'s current action.
    pub fn action(sig: libc::c_int) -> Option<libc::sigaction> {
        // SAFETY: a zeroed `sigaction` is a valid value for it (no handler, empty mask).
        let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: sigaction only reads the signal's current action into `sa`.
        (unsafe { libc::sigaction(sig, std::ptr::null(), &mut sa) } == 0).then_some(sa)
    }

    /// True when action `sa` is [`in_frame`]'s.
    pub fn is_ours(sa: &libc::sigaction) -> bool {
        sa.sa_sigaction == in_frame as Handler as usize
    }

    /// The handler in front of crash-handler's: the kernel's context, or the part of it inside
    /// the alternate stack.
    unsafe extern "C" fn in_frame(
        sig: libc::c_int,
        info: *mut libc::siginfo_t,
        uc: *mut libc::c_void,
    ) {
        let theirs = THEIRS.load(Ordering::Acquire);
        // SAFETY: THEIRS holds crash-handler's handler, a function of this type, from before
        // this handler was installed (keep_handler_in_frame).
        let theirs = unsafe { std::mem::transmute::<usize, Handler>(theirs) };
        // SAFETY: a zeroed `stack_t` is a valid value for it (no stack).
        let mut ss: libc::stack_t = unsafe { std::mem::zeroed() };
        // SAFETY: sigaltstack (a plain system call, safe in a signal handler) only reads this
        // thread's alternate stack into `ss`.
        let on_alt_stack = unsafe { libc::sigaltstack(std::ptr::null(), &mut ss) } == 0
            && ss.ss_flags & libc::SS_ONSTACK != 0;
        let stack = on_alt_stack.then(|| ss.ss_sp as usize..ss.ss_sp as usize + ss.ss_size);
        let len = mdm_crash::frame::context_len();
        let readable = mdm_crash::frame::readable(uc as usize, len, stack);
        if readable == len {
            // SAFETY: crash-handler's handler with the kernel's own arguments, as without this.
            unsafe { theirs(sig, info, uc) };
            return;
        }
        mdm_crash::frame::with_copy(|copy| {
            // SAFETY: the first `readable` bytes at `uc` are the kernel's frame, inside the
            // alternate stack; `copy` is a zeroed context of the type crash-handler reads, `len`
            // bytes, more than `readable`.
            unsafe {
                std::ptr::copy_nonoverlapping(uc.cast::<u8>(), copy.cast::<u8>(), readable);
                theirs(sig, info, copy);
            }
        });
    }
}
// ---- End of the handler in front of crash-handler's.
