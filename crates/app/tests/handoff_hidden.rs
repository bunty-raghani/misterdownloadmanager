//! Issue #145: what the browser hands over while the main window is hidden in the notification
//! area; #162: or minimized (egui ran only the app's logic then, and the commands waited in its
//! UI until the window was restored). 0.2.4 closed on it ("egui backend is implemented incorrectly - the user callback was
//! never called"): Add torrent or Download File Info, opened in a pass that ran while no window
//! was being drawn, had no event loop to create its window with. The real app, off the screens
//! (MDM_OFFSCREEN), with a data folder of its own: hidden with `/show tray`, then handed a magnet
//! link (as Windows starts the program for one) and a download (`/d`, the channel the browser's
//! host uses). Both dialogs must open and the app must keep running.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test handoff_hidden -- --ignored`. tools/lab/magnet-hidden.ps1 plays
//! the same on a release build, through the browser's native host too, in more window states.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");

/// The app's log so far (every file of `logs/`).
fn log(data: &Path) -> String {
    let mut all = String::new();
    if let Ok(dir) = std::fs::read_dir(data.join("logs")) {
        for f in dir.flatten() {
            all += &std::fs::read_to_string(f.path()).unwrap_or_default();
        }
    }
    all
}

/// Wait until the log has `line`, failing if the app quits first or `wait` passes.
fn wait_for(app: &mut Child, data: &Path, line: &str, wait: Duration) {
    let until = Instant::now() + wait;
    loop {
        let text = log(data);
        assert!(!text.contains("panic"), "the app panicked:\n{text}");
        if text.contains(line) {
            return;
        }
        if let Some(status) = app.try_wait().unwrap() {
            panic!("the app closed ({status}) before '{line}':\n{text}");
        }
        assert!(
            Instant::now() < until,
            "no '{line}' after {wait:?}:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A second launch with `args`: it hands them to the running app and quits.
fn second_launch(data: &Path, args: &[&str]) {
    let status = Command::new(APP)
        .args(args)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .status()
        .unwrap();
    assert!(status.success(), "second launch {args:?}: {status}");
}

/// The app, off the screens, with a data folder of its own; `away` puts the main window out of
/// sight (`/show tray` or `/show minimized`) before each hand-over: a magnet link (as Windows
/// starts the program for one) and a download (`/d`, the channel the browser's host uses). Both
/// dialogs must open, and the app must keep running.
fn hand_overs_while(away: &str) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    // No update check, no first-start questions; the torrent engine stays on this computer
    // (the magnet names no tracker, so its look-up just waits). Minimize stays a minimize (not
    // the notification area), for `/show minimized`.
    let settings = serde_json::json!({
        "update_check": false, "helpers_asked": true, "language_asked": true,
        "extension_never_ask": true, "minimize_to_tray": false,
        "torrent_dht": false, "torrent_lsd": false, "torrent_upnp": false,
        "torrent_incoming": false,
    });
    std::fs::write(
        data.join("downloads.json"),
        serde_json::json!({ "next_id": 1, "items": [], "version": 2, "settings": settings })
            .to_string(),
    )
    .unwrap();
    let mut app = Command::new(APP)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .spawn()
        .unwrap();
    let wait = Duration::from_secs(60);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for(&mut app, data, "window opened: main", wait);

        second_launch(data, &["/show", away]);
        std::thread::sleep(Duration::from_millis(1500));
        let magnet = "magnet:?xt=urn:btih:5b4f0c3a9e1d27f86c0b3a4e9d1f2a7c6e8b0d14&dn=MDM%20test";
        second_launch(data, &[magnet]);
        wait_for(&mut app, data, "window opened: add-torrent", wait);

        second_launch(data, &["/show", away]);
        std::thread::sleep(Duration::from_millis(1500));
        // Port 9 on this computer: nothing answers, and nothing leaves it.
        second_launch(data, &["/d", "http://127.0.0.1:9/mdm-test-145.zip"]);
        wait_for(&mut app, data, "window opened: file-info", wait);

        // Still running a moment later (the dialogs drew their first frames).
        std::thread::sleep(Duration::from_secs(2));
        wait_for(&mut app, data, "window opened: file-info", wait);
    }));
    let _ = Command::new(APP)
        .arg("/exit")
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .status();
    let until = Instant::now() + Duration::from_secs(15);
    while app.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = app.kill();
    let _ = app.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn browser_hand_overs_while_hidden_in_the_tray() {
    hand_overs_while("tray");
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn browser_hand_overs_while_minimized() {
    hand_overs_while("minimized");
}

/// #1333: MDM started by the native host with /fromhost boots hidden and opens only the dialog.
#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn browser_hand_over_when_started_from_host() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let settings = serde_json::json!({
        "update_check": false,
        "helpers_asked": true,
        "language_asked": true,
        "extension_never_ask": true,
        "minimize_to_tray": false,
        "torrent_dht": false,
        "torrent_lsd": false,
        "torrent_upnp": false,
        "torrent_incoming": false,
    });
    std::fs::write(
        data.join("downloads.json"),
        serde_json::json!({ "next_id": 1, "items": [], "version": 2, "settings": settings })
            .to_string(),
    )
    .unwrap();
    let mut app = Command::new(APP)
        .arg(mdm_ipc::FROM_HOST)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .spawn()
        .unwrap();
    let wait = Duration::from_secs(60);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for(&mut app, data, "started from host: hidden", wait);

        second_launch(data, &["/d", "http://127.0.0.1:9/mdm-test-fromhost.zip"]);
        wait_for(&mut app, data, "window opened: file-info", wait);
    }));
    let _ = Command::new(APP)
        .arg("/exit")
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .status();
    let until = Instant::now() + Duration::from_secs(15);
    while app.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = app.kill();
    let _ = app.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
