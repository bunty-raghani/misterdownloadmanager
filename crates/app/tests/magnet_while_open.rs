//! Issue #507: a magnet link handed to MDM while Add torrent was already open was dropped
//! without a word (the channel answered ok, nothing happened). It now waits its turn: Add torrent
//! opens again for it once the first one closes. The real app, off the screens (MDM_OFFSCREEN),
//! with a data folder of its own: two magnet links handed over as Windows starts the program for
//! one, the first Add torrent closed as its title bar's × does (WM_CLOSE), then the second one
//! must open.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test magnet_while_open -- --ignored`.
//!
//! Windows only: the window is closed with `PostMessage(WM_CLOSE)` through PowerShell; the
//! queue itself is the same code on every system (app.rs `torrent_waiting`).
#![cfg(windows)]

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

/// Wait until the log has `line` `times` times, failing if the app quits first or `wait` passes.
fn wait_for(app: &mut Child, data: &Path, line: &str, times: usize, wait: Duration) {
    let until = Instant::now() + wait;
    loop {
        let text = log(data);
        assert!(!text.contains("panic"), "the app panicked:\n{text}");
        if text.matches(line).count() >= times {
            return;
        }
        if let Some(status) = app.try_wait().unwrap() {
            panic!("the app closed ({status}) before '{line}' x{times}:\n{text}");
        }
        assert!(
            Instant::now() < until,
            "no '{line}' x{times} after {wait:?}:\n{text}"
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

/// `WM_CLOSE` to `pid`'s visible window titled `title`, as its title bar's × sends.
fn close_window(pid: u32, title: &str) {
    let script = format!(
        r#"Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class W {{
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static int Close(uint pid, string title) {{
    int n = 0;
    EnumWindows((h, l) => {{ uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) {{ var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
        if (sb.ToString() == title) {{ PostMessage(h, 0x10, IntPtr.Zero, IntPtr.Zero); n++; }} }}
      return true; }}, IntPtr.Zero);
    return n;
  }}
}}
"@
[W]::Close({pid}, '{title}')"#
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .unwrap();
    let closed = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert_eq!(closed, "1", "windows closed: {closed} ({:?})", out.stderr);
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_magnet_handed_over_while_add_torrent_is_open_opens_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    // No update check, no first-start questions, English titles; the torrent engine stays on
    // this computer (the magnets name no tracker, so their look-ups just wait).
    let settings = serde_json::json!({
        "update_check": false, "helpers_asked": true, "language_asked": true, "language": "en",
        "extension_never_ask": true,
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
    let opened = "window opened: add-torrent";
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for(&mut app, data, "window opened: main", 1, wait);
        let first =
            "magnet:?xt=urn:btih:5b4f0c3a9e1d27f86c0b3a4e9d1f2a7c6e8b0d14&dn=MDM%20test%201";
        let second =
            "magnet:?xt=urn:btih:7c1e9a2b4d6f8e0a1c3b5d7f9e2a4c6b8d0f1e23&dn=MDM%20test%202";
        second_launch(data, &[first]);
        wait_for(&mut app, data, opened, 1, wait);
        // Its first frames drawn (a close asked before then isn't seen by the window yet).
        std::thread::sleep(Duration::from_secs(2));
        // Handed over while it is open: it waits (the second launch returns once the app took
        // it).
        second_launch(data, &[second]);
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            log(data).matches(opened).count(),
            1,
            "one Add torrent at a time"
        );
        // The first closed: Add torrent opens again, for the second.
        close_window(app.id(), "Add torrent");
        wait_for(&mut app, data, "window closed: add-torrent", 1, wait);
        wait_for(&mut app, data, opened, 2, wait);
        assert!(log(data).contains("Add torrent is open: a torrent waits (1 waiting)"));
        // Closed too: nothing more was waiting.
        close_window(app.id(), "Add torrent");
        wait_for(&mut app, data, "window closed: add-torrent", 2, wait);
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(log(data).matches(opened).count(), 2, "nothing else waited");
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
