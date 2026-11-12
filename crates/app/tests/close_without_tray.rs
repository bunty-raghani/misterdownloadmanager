//! The close button follows Options' close-to-tray choice and the icon the shell really shows:
//! off quits; on hides; on with no visible icon minimizes to the taskbar instead (downloads
//! continue, and the window stays reachable); with no tray at all it quits, as ever. The real app,
//! off the screens, with data folders of its own; a tray object whose visibility
//! MDM_TEST_TRAY_VISIBLE decides, or no tray object; and a real icon, the shell asked for real,
//! hides the window even when the shell gives it no place (an icon behind ^, #545). Windows only
//! (WM_CLOSE), so it runs on request:
//! `cargo test -p mdm-app --test close_without_tray -- --ignored`.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");
/// The main window's title (main.rs MAIN_TITLE).
const MAIN: &str = concat!("Mister Download Manager ", env!("CARGO_PKG_VERSION"));

/// The app, ended and reaped however the test leaves it (a failed assert too).
struct Process(Child);

impl std::ops::Deref for Process {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Process {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

/// Wait until the app exits, failing if `wait` passes.
fn wait_exit(app: &mut Child, wait: Duration) -> ExitStatus {
    let until = Instant::now() + wait;
    loop {
        if let Some(status) = app.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < until, "the app didn't exit");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The app's environment: off the screens, its own data folder, its tray icon, and whether the
/// test says that icon is visible (MDM_TEST_TRAY_VISIBLE).
fn env<'a>(cmd: &'a mut Command, data: &Path, visible: bool) -> &'a mut Command {
    cmd.env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .env("MDM_TEST_TRAY", "1")
        .env("MDM_TEST_TRAY_VISIBLE", if visible { "1" } else { "0" })
}

/// The app's environment with no tray object at all (off-screen copies have none unless asked).
fn env_no_tray<'a>(cmd: &'a mut Command, data: &Path) -> &'a mut Command {
    cmd.env("MDM_DATA_DIR", data).env("MDM_OFFSCREEN", "1")
}

/// A data folder whose first start asks nothing and checks nothing, with the close-to-tray
/// choice set.
fn data(dir: &Path, name: &str, close_to_tray: bool) -> PathBuf {
    let data = dir.join(name);
    std::fs::create_dir_all(&data).unwrap();
    let settings = serde_json::json!({
        "update_check": false, "helpers_asked": true, "language_asked": true,
        "extension_never_ask": true, "close_to_tray": close_to_tray,
        "language": "en",
        "torrent_dht": false, "torrent_lsd": false, "torrent_upnp": false,
        "torrent_incoming": false,
    });
    std::fs::write(
        data.join("downloads.json"),
        serde_json::json!({ "next_id": 1, "items": [], "version": 2, "settings": settings })
            .to_string(),
    )
    .unwrap();
    data
}

/// Post WM_CLOSE to the app's main window. True when one was found.
fn close_main(pid: u32) -> bool {
    main_window(
        pid,
        "[CW]::PostMessage($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null",
    ) == "True"
}

/// Whether the app's main window is visible ("True" or "False"; "" when there is none).
fn main_visible(pid: u32) -> String {
    main_window(pid, "$script:visible = [CW]::IsWindowVisible($h)")
}

/// Run `action` on the app's main window (`$h`); "True" when one was found, or what the action
/// put in `$script:visible`.
fn main_window(pid: u32, action: &str) -> String {
    let script = r#"
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class CW {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@
$script:closed = $false
$script:visible = $null
[CW]::EnumWindows({
  param($h, $l)
  $p = 0
  [CW]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
  if ($p -eq PID) {
    $t = New-Object System.Text.StringBuilder 512
    [CW]::GetWindowText($h, $t, 512) | Out-Null
    if ($t.ToString() -eq 'TITLE') {
      ACTION
      $script:closed = $true
    }
  }
  $true
}, [IntPtr]::Zero) | Out-Null
if ($script:visible -ne $null) { $script:visible } elseif ($script:closed) { $script:closed } else { '' }
"#
    .replace("PID", &pid.to_string())
    .replace("TITLE", MAIN)
    .replace("ACTION", action);
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Wait until the app's main window is visible or not, failing after `wait`.
fn wait_visible(pid: u32, visible: bool, wait: Duration) {
    let until = Instant::now() + wait;
    let want = if visible { "True" } else { "False" };
    loop {
        let now = main_visible(pid);
        if now == want {
            return;
        }
        assert!(
            Instant::now() < until,
            "the main window's visibility is '{now}', not {want}"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn closing_follows_the_option_and_the_tray_icon() {
    let dir = tempfile::tempdir().unwrap();
    let wait = Duration::from_secs(60);

    // The option off: closing quits, as a window's × does.
    let data1 = data(dir.path(), "off", false);
    let mut app = Process(env(&mut Command::new(APP), &data1, true).spawn().unwrap());
    wait_for(&mut app, &data1, "tray icon shown", wait);
    assert!(close_main(app.id()), "no main window to close");
    let status = wait_exit(&mut app, Duration::from_secs(20));
    assert!(status.success(), "{status}");
    assert!(log(&data1).contains("close: quitting"), "{}", log(&data1));

    // The option on and the icon there: closing hides it, and /exit still quits.
    let data2 = data(dir.path(), "on", true);
    let mut app = Process(env(&mut Command::new(APP), &data2, true).spawn().unwrap());
    wait_for(&mut app, &data2, "first frame drawn", wait);
    assert!(close_main(app.id()), "no main window to close");
    wait_for(
        &mut app,
        &data2,
        "close: hidden to the notification area",
        Duration::from_secs(10),
    );
    assert!(
        app.try_wait().unwrap().is_none(),
        "the app quit instead of hiding"
    );
    let _ = env(&mut Command::new(APP), &data2, true)
        .arg("/exit")
        .status();
    let _ = wait_exit(&mut app, Duration::from_secs(20));

    // The option on but no icon the shell shows: keeping running leaves the window in the
    // taskbar, not hidden and not quitting; downloads carry on and /exit still quits.
    let data3 = data(dir.path(), "no-icon", true);
    let mut app = Process(env(&mut Command::new(APP), &data3, false).spawn().unwrap());
    wait_for(&mut app, &data3, "tray icon shown", wait);
    assert!(close_main(app.id()), "no main window to close");
    wait_for(
        &mut app,
        &data3,
        "close: no visible tray icon; minimizing to the taskbar",
        Duration::from_secs(10),
    );
    assert!(
        app.try_wait().unwrap().is_none(),
        "the app quit instead of minimizing"
    );
    let _ = env(&mut Command::new(APP), &data3, false)
        .arg("/exit")
        .status();
    let _ = wait_exit(&mut app, Duration::from_secs(20));

    // No tray at all: closing quits, whatever the option says (#59: nothing could bring a hidden
    // window back, and scripts ending an off-screen copy rely on it).
    let data4 = data(dir.path(), "no-tray", true);
    let mut app = Process(env_no_tray(&mut Command::new(APP), &data4).spawn().unwrap());
    wait_for(&mut app, &data4, "first frame drawn", wait);
    assert!(close_main(app.id()), "no main window to close");
    let status = wait_exit(&mut app, Duration::from_secs(20));
    assert!(status.success(), "{status}");
    assert!(log(&data4).contains("close: quitting"), "{}", log(&data4));
}

/// The app's environment with its real tray icon, the shell asked for real; `placed` false says
/// the shell gives the icon no place (MDM_TEST_TRAY_RECT=0), as the reporters' did for an icon
/// behind ^ (#750, #718, #754, #545).
fn env_real<'a>(cmd: &'a mut Command, data: &Path, placed: bool) -> &'a mut Command {
    cmd.env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .env("MDM_TEST_TRAY", "1")
        .env("MDM_TEST_TRAY_RECT", if placed { "1" } else { "0" })
}

/// Closing with a real icon the shell holds: hidden there, whether or not the shell says where
/// the icon is (a new icon goes behind ^, where the shell may give it none), and a second start
/// brings the window back. The shell is this machine's, asked by the app's tray thread (#747):
/// its icon is added for the test and removed when the app exits.
#[test]
#[ignore = "opens the app's windows (off-screen) and a tray icon: run with --ignored"]
fn closing_hides_to_an_icon_the_shell_holds() {
    let dir = tempfile::tempdir().unwrap();
    let wait = Duration::from_secs(60);
    for (name, placed) in [("placed", true), ("no-place", false)] {
        let data = data(dir.path(), name, true);
        let mut app = Process(
            env_real(&mut Command::new(APP), &data, placed)
                .spawn()
                .unwrap(),
        );
        wait_for(&mut app, &data, "first frame drawn", wait);
        wait_for(&mut app, &data, "tray icon shown", wait);
        wait_for(
            &mut app,
            &data,
            &format!("tray icon: placed {placed}, held by the shell true"),
            Duration::from_secs(10),
        );
        assert!(close_main(app.id()), "no main window to close");
        wait_for(
            &mut app,
            &data,
            "close: hidden to the notification area (tray icon: There)",
            Duration::from_secs(10),
        );
        wait_visible(app.id(), false, Duration::from_secs(10));
        assert!(
            app.try_wait().unwrap().is_none(),
            "the app quit instead of hiding"
        );
        // A second start brings the hidden window back, icon or not.
        let _ = env_real(&mut Command::new(APP), &data, placed).status();
        wait_visible(app.id(), true, Duration::from_secs(10));
        let _ = env_real(&mut Command::new(APP), &data, placed)
            .arg("/exit")
            .status();
        let _ = wait_exit(&mut app, Duration::from_secs(20));
    }
}
