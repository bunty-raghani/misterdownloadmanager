//! #202: a freeze while MDM handles a command must still be reported. Since #176 the incoming
//! commands and the clock run in the app's `logic`, which egui calls every frame (and alone while
//! the main window is minimized); the freeze watchdog timed only `ui`, so a command that hung
//! never opened the crash report window nor wrote a freeze report. The real app, off the screens
//! (MDM_OFFSCREEN), with a data folder of its own: MDM_CRASH_TEST=1, then `/show crash-test:…`
//! (a deadlock, or 12 s of a busy window thread) with the main window visible or minimized. The
//! crash monitor must open its window and write the freeze report, which names the kind of freeze.
//! #1126: with every window hidden in the notification area, a stall whose thread runs nothing
//! is the computer's (logged, not reported), a busy one is still a freeze, and so is one that
//! goes on once a window is asked for.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test freeze_report -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");

/// The app's and its monitor's logs so far (every file of `logs/`).
fn log(data: &Path) -> String {
    let mut all = String::new();
    if let Ok(dir) = std::fs::read_dir(data.join("logs")) {
        for f in dir.flatten() {
            all += &std::fs::read_to_string(f.path()).unwrap_or_default();
        }
    }
    all
}

/// The freeze report's text (`crashes/<when>-freeze.txt`), once written.
fn freeze_report(data: &Path) -> Option<String> {
    let dir = std::fs::read_dir(data.join("crashes")).ok()?;
    let path: PathBuf = dir
        .flatten()
        .map(|f| f.path())
        .find(|p| p.to_string_lossy().ends_with("-freeze.txt"))?;
    std::fs::read_to_string(path).ok()
}

/// Wait until `done` holds, failing if `wait` passes. The app may be frozen meanwhile: only a
/// panic or its end before `done` fails sooner.
fn wait_until(app: &mut Child, data: &Path, what: &str, wait: Duration, done: impl Fn() -> bool) {
    let until = Instant::now() + wait;
    loop {
        let text = log(data);
        assert!(!text.contains("panic"), "the app panicked:\n{text}");
        if done() {
            return;
        }
        if let Some(status) = app.try_wait().unwrap() {
            panic!("the app closed ({status}) before {what}:\n{text}");
        }
        assert!(Instant::now() < until, "no {what} after {wait:?}:\n{text}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A second launch with `args`: it hands them to the running app and quits.
fn second_launch(data: &Path, args: &[&str]) {
    let status = Command::new(APP)
        .args(args)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .env("MDM_CRASH_TEST", "1")
        .status()
        .unwrap();
    assert!(status.success(), "second launch {args:?}: {status}");
}

/// The crash monitor's process id, from its log ("crash monitor (process N) for …").
fn monitor_pid(data: &Path) -> Option<u32> {
    let text = log(data);
    let at = text.find("crash monitor (process ")? + "crash monitor (process ".len();
    text[at..].split(')').next()?.parse().ok()
}

/// End a process this test started indirectly (the monitor, the app's child).
fn end(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

/// The app with the main window `state` ("visible", or "minimized" with `/show minimized`), then
/// `/show crash-test:<what>`: the report window must open and the report name the freeze's kind
/// (`kind`, the report's own words). With `recovers`, the window must answer again afterwards.
/// The app, off the screens, with `data` as its data folder (and a download folder in it), the
/// crash tests on.
fn start(data: &Path) -> Child {
    // No update check, no first-start questions; the torrent engine stays on this computer.
    // Minimize stays a minimize (not the notification area), for `/show minimized`.
    let downloads = data.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let settings = serde_json::json!({
        "language": "en", "update_check": false, "helpers_asked": true, "language_asked": true,
        "extension_never_ask": true, "minimize_to_tray": false, "clipboard_autostart": false,
        "torrent_dht": false, "torrent_lsd": false, "torrent_upnp": false,
        "torrent_incoming": false,
        "category_dirs": [["General", downloads.to_string_lossy()]],
    });
    std::fs::write(
        data.join("downloads.json"),
        serde_json::json!({ "next_id": 1, "items": [], "version": 2, "settings": settings })
            .to_string(),
    )
    .unwrap();
    Command::new(APP)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .env("MDM_CRASH_TEST", "1")
        .spawn()
        .unwrap()
}

fn freeze_while(state: &str, what: &str, kind: &str, recovers: bool) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let mut app = start(data);
    let wait = Duration::from_secs(60);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_until(&mut app, data, "main window", wait, || {
            let l = log(data);
            l.contains("window opened: main") && l.contains("crash reports on")
        });
        if state == "minimized" {
            second_launch(data, &["/show", "minimized"]);
            // Its frames are logic-only from now on (eframe's update_logic_only).
            std::thread::sleep(Duration::from_secs(2));
        }
        let asked = Instant::now();
        second_launch(data, &["/show", &format!("crash-test:{what}")]);
        // The command reached the app (crash.rs logs the test as it begins).
        wait_until(&mut app, data, "the crash test", wait, || {
            log(data).contains(&format!("crash test: {what}"))
        });
        wait_until(
            &mut app,
            data,
            "freeze report from the watchdog",
            wait,
            || log(data).contains("the window stopped responding"),
        );
        let seen = asked.elapsed();
        // Not before the 8 s a freeze takes (no false alarm), and not much after.
        assert!(
            seen >= Duration::from_secs(7) && seen < Duration::from_secs(20),
            "the freeze was reported {seen:?} after it began"
        );
        wait_until(&mut app, data, "crash report window", wait, || {
            log(data).contains("window opened: crash-report")
        });
        wait_until(&mut app, data, "freeze report", wait, || {
            freeze_report(data).is_some()
        });
        let report = freeze_report(data).unwrap();
        // Shown with --nocapture: the watchdog's line and the report's head.
        for line in log(data)
            .lines()
            .filter(|l| l.contains("stopped responding"))
        {
            println!("{line}");
        }
        for line in report.lines().take(12) {
            println!("  | {line}");
        }
        assert!(
            report.contains("The window did not answer for") && report.contains(kind),
            "the freeze report doesn't say \"{kind}\":\n{report}"
        );
        if recovers {
            wait_until(&mut app, data, "the window answering again", wait, || {
                log(data).contains("the window answers again")
            });
        }
    }));
    // Frozen: it can't be asked to quit. Its monitor goes too (its report window stays open).
    if let Some(pid) = monitor_pid(data) {
        end(pid);
    }
    let _ = app.kill();
    let _ = app.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_command_that_deadlocks_is_reported() {
    freeze_while("visible", "freeze", "Its thread was waiting", false);
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_command_that_deadlocks_is_reported_while_minimized() {
    freeze_while("minimized", "freeze", "Its thread was waiting", false);
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_busy_command_is_reported_while_minimized_and_answers_again() {
    freeze_while("minimized", "spin", "Its thread ran all along", true);
}

/// The watchdog's lines and the crash tests' in the log, for the output (--nocapture).
fn show_watchdog(data: &Path) {
    for line in log(data).lines().filter(|l| {
        l.contains("freeze watchdog")
            || l.contains("crash test")
            || l.contains("answers again")
            || l.contains("window opened")
            || l.contains("window closed")
    }) {
        println!("{line}");
    }
}

/// #1126: `/show crash-test:hidden-<what>` hides the main window in the notification area (no
/// other window open), and a few seconds later a tick of the hidden window stalls: `stall`
/// waits, running nothing (as the reports' 0 ms of 8 s), `spin` runs. With `ask`, a second
/// launch asks for a window that long into the stall. Returns when the watchdog has reported
/// a freeze or the window answers again, with how long after the stall began it reported one
/// (None: not reported).
fn hidden_stall(what: &str, ask: Option<Duration>) -> (Option<Duration>, Option<String>) {
    out_of_sight_stall(&format!("hidden-{what}"), ask)
}

/// [`hidden_stall`] for any of the crash tests that put the window out of sight first:
/// `hidden-<what>` (the notification area) or `minimized-<what>` (the taskbar, #1481).
fn out_of_sight_stall(test: &str, ask: Option<Duration>) -> (Option<Duration>, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let mut app = start(data);
    let wait = Duration::from_secs(90);
    let mut reported = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_until(&mut app, data, "main window", wait, || {
            let l = log(data);
            l.contains("window opened: main") && l.contains("crash reports on")
        });
        second_launch(data, &["/show", &format!("crash-test:{test}")]);
        wait_until(&mut app, data, "the stall", wait, || {
            log(data).contains("crash test: the window thread")
        });
        let began = Instant::now();
        if let Some(ask) = ask {
            std::thread::sleep(ask);
            // A second launch asks for the window (as a click on MDM's shortcut does).
            second_launch(data, &["/show", "about"]);
        }
        wait_until(
            &mut app,
            data,
            "a report or the window's answer",
            wait,
            || {
                let l = log(data);
                l.contains("the window stopped responding")
                    || l.contains("the window answers again")
            },
        );
        if log(data).contains("the window stopped responding") {
            reported = Some(began.elapsed());
            wait_until(&mut app, data, "freeze report", wait, || {
                freeze_report(data).is_some()
            });
        } else {
            // Not reported a little later either.
            std::thread::sleep(Duration::from_secs(3));
        }
        show_watchdog(data);
    }));
    let report = freeze_report(data);
    if report.is_none() && app.try_wait().unwrap().is_none() {
        // It answers: asked to quit as File → Exit does.
        let _ = Command::new(APP)
            .arg("/exit")
            .env("MDM_DATA_DIR", data)
            .env("MDM_OFFSCREEN", "1")
            .status();
        let until = Instant::now() + Duration::from_secs(30);
        while app.try_wait().unwrap().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Some(pid) = monitor_pid(data) {
        end(pid);
    }
    let _ = app.kill();
    let _ = app.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    (reported, report)
}

/// #1126: hidden in the notification area, the window thread waits 12 s running nothing (the
/// computer holds it): logged with how little it ran, not reported, and it answers again.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_hidden_window_kept_waiting_is_logged_not_reported() {
    let (reported, report) = hidden_stall("stall:12", None);
    assert_eq!(reported, None, "reported: {report:?}");
    assert!(report.is_none());
}

/// The same window busy for 12 s (an endless loop of MDM's own) is a freeze, hidden or not.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_hidden_window_busy_is_reported() {
    let (reported, report) = hidden_stall("spin:12", None);
    let seen = reported.expect("a busy hidden window is a freeze");
    assert!(seen < Duration::from_secs(12), "reported {seen:?} in");
    let report = report.unwrap();
    assert!(report.contains("Its thread ran all along"), "{report}");
}

/// Kept waiting while hidden, but asked for a window 15 s in: reported 8 s after the ask, not
/// at the minute's end.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_hidden_stall_is_reported_once_a_window_is_asked_for() {
    let (reported, _) = hidden_stall("stall:40", Some(Duration::from_secs(15)));
    let seen = reported.expect("asked for, the stall is a freeze");
    assert!(
        seen >= Duration::from_secs(22) && seen < Duration::from_secs(30),
        "reported {seen:?} after the stall began (asked at 15 s)"
    );
}

/// #1481, #1309: minimized to the taskbar (not the notification area), the window thread waits
/// 12 s running nothing: nothing of MDM's is on screen, so it is the computer's as when hidden:
/// logged, not reported, and it answers again.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_minimized_window_kept_waiting_is_logged_not_reported() {
    let (reported, report) = out_of_sight_stall("minimized-stall:12", None);
    assert_eq!(reported, None, "reported: {report:?}");
    assert!(report.is_none());
}

/// The same minimized window busy for 12 s is a freeze, minimized or not.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_minimized_window_busy_is_reported() {
    let (reported, report) = out_of_sight_stall("minimized-spin:12", None);
    let seen = reported.expect("a busy minimized window is a freeze");
    assert!(seen < Duration::from_secs(12), "reported {seen:?} in");
    let report = report.unwrap();
    assert!(report.contains("Its thread ran all along"), "{report}");
}

/// Kept waiting while minimized, but asked for a window 15 s in: reported 8 s after the ask.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_minimized_stall_is_reported_once_a_window_is_asked_for() {
    let (reported, _) = out_of_sight_stall("minimized-stall:40", Some(Duration::from_secs(15)));
    let seen = reported.expect("asked for, the stall is a freeze");
    assert!(
        seen >= Duration::from_secs(22) && seen < Duration::from_secs(30),
        "reported {seen:?} after the stall began (asked at 15 s)"
    );
}
