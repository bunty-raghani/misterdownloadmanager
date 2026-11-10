//! #226: started with the session, MDM waits in the notification area. The real app, off the
//! screens (MDM_OFFSCREEN), with a data folder of its own, started as its startup entry starts
//! it (`/startup`), with its tray icon (MDM_TEST_TRAY: off-screen copies have none otherwise):
//! the icon is shown and no window is, the list's clock runs, and a second launch by the user
//! shows the window as always. On the way, the startup entry versions before 0.5.0 wrote (the
//! program alone) is written again with `/startup`: on Windows a Run value of the test's own name
//! (MDM_TEST_RUN_VALUE, deleted after; the user's own is never read nor written by an off-screen
//! copy), on Linux an autostart entry in a configuration folder of the test's own.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test startup_tray -- --ignored`.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");
/// The main window's title (main.rs MAIN_TITLE).
const MAIN: &str = concat!("Mister Download Manager ", env!("CARGO_PKG_VERSION"));

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

/// The app's environment: off the screens, its own data folder, its tray icon, and (Windows) a
/// Run value or (Linux) an autostart folder of the test's own.
fn env<'a>(cmd: &'a mut Command, data: &Path, startup: &Startup) -> &'a mut Command {
    cmd.env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .env("MDM_TEST_TRAY", "1")
        // A test's shell isn't asked where the icon is: say it is shown (app.rs tray_shown).
        .env("MDM_TEST_TRAY_VISIBLE", "1");
    #[cfg(windows)]
    cmd.env("MDM_TEST_RUN_VALUE", &startup.name);
    #[cfg(unix)]
    cmd.env("XDG_CONFIG_HOME", &startup.config);
    cmd
}

/// Where the test's startup entry is, deleted when the test ends (passed or not).
struct Startup {
    #[cfg(windows)]
    name: String,
    #[cfg(unix)]
    config: std::path::PathBuf,
}

#[cfg(windows)]
const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

impl Startup {
    /// The entry versions before 0.5.0 wrote for this program: the program alone, no argument.
    fn old(_dir: &Path) -> Startup {
        #[cfg(windows)]
        {
            let name = format!("MisterDownloadManagerTest{}", std::process::id());
            let status = Command::new("reg")
                .args(["add", RUN, "/v", &name, "/t", "REG_SZ", "/f", "/d"])
                .arg(format!("\"{APP}\""))
                .status()
                .unwrap();
            assert!(status.success());
            Startup { name }
        }
        #[cfg(unix)]
        {
            let config = _dir.join("config");
            let auto = config.join("autostart");
            std::fs::create_dir_all(&auto).unwrap();
            std::fs::write(
                auto.join("misterdownloadmanager.desktop"),
                format!(
                    "[Desktop Entry]\nType=Application\nName=Mister Download Manager\n\
                     Exec=\"{APP}\"\nIcon=misterdownloadmanager\nTerminal=false\n\
                     X-GNOME-Autostart-enabled=true\n"
                ),
            )
            .unwrap();
            Startup { config }
        }
    }

    /// The entry as it is now.
    fn read(&self) -> String {
        #[cfg(windows)]
        {
            let out = Command::new("reg")
                .args(["query", RUN, "/v", &self.name])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.lines()
                .find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim().to_owned()))
                .unwrap_or_default()
        }
        #[cfg(unix)]
        std::fs::read_to_string(self.config.join("autostart/misterdownloadmanager.desktop"))
            .unwrap_or_default()
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        #[cfg(windows)]
        let _ = Command::new("reg")
            .args(["delete", RUN, "/v", &self.name, "/f"])
            .output();
    }
}

/// How many of the process's top-level windows whose title starts with `title` are visible (on
/// the screens or off them), or None where it can't be told here. On Windows winit's own message window, "Winit Thread
/// Event Target", isn't counted: winit makes it visible, but it is a 6 px, fully transparent
/// window that lets every click through, in every program made with winit.
fn visible_windows(pid: u32, title: &str) -> Option<usize> {
    #[cfg(windows)]
    {
        let script = format!(
            "Add-Type @'\n\
             using System; using System.Runtime.InteropServices;\n\
             public static class V {{\n\
               public delegate bool P(IntPtr h, IntPtr l);\n\
               [DllImport(\"user32.dll\")] public static extern bool EnumWindows(P f, IntPtr l);\n\
               [DllImport(\"user32.dll\")] public static extern bool IsWindowVisible(IntPtr h);\n\
               [DllImport(\"user32.dll\")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);\n\
               [DllImport(\"user32.dll\", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);\n\
               [DllImport(\"user32.dll\", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);\n\
             }}\n\
             '@\n\
             $n = 0\n\
             [V]::EnumWindows({{ param($h, $l) $p = 0; [V]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null; \
             $c = New-Object System.Text.StringBuilder 256; [V]::GetClassName($h, $c, 256) | Out-Null; \
             $t = New-Object System.Text.StringBuilder 256; [V]::GetWindowText($h, $t, 256) | Out-Null; \
             if ($p -eq {pid} -and [V]::IsWindowVisible($h) -and $c.ToString() -ne 'Winit Thread Event Target' \
             -and $t.ToString().StartsWith('{title}')) {{ $script:n++ }}; $true }}, [IntPtr]::Zero) | Out-Null\n\
             $n"
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(unix)]
    {
        // xdotool, where the lab has it (X11, Xvfb).
        let out = Command::new("xdotool")
            .args([
                "search",
                "--onlyvisible",
                "--pid",
                &pid.to_string(),
                "--name",
            ])
            .arg(format!("^{title}"))
            .output()
            .ok()?;
        out.status.code()?;
        Some(String::from_utf8_lossy(&out.stdout).lines().count())
    }
}

#[test]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
fn a_start_with_the_session_waits_in_the_tray() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let settings = serde_json::json!({
        "update_check": false, "helpers_asked": true, "language_asked": true,
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
    let startup = Startup::old(dir.path());
    assert!(!startup.read().contains("/startup"), "{}", startup.read());

    let mut app = env(&mut Command::new(APP), &data, &startup)
        .arg("/startup")
        .spawn()
        .unwrap();
    let wait = Duration::from_secs(60);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Its first frame drawn (Direct3D's watchdog content), hidden.
        wait_for(&mut app, &data, "first frame drawn", wait);
        // Where it settled (app.rs at_login): the notification area, or, with no tray host here
        // (#59) after its grace period, minimized.
        let until = Instant::now() + wait;
        let tray = loop {
            let text = log(&data);
            if text.contains("started with the session: in the notification area") {
                break true;
            }
            if text.contains("notification area within") {
                break false;
            }
            assert!(Instant::now() < until, "not settled:\n{text}");
            std::thread::sleep(Duration::from_millis(200));
        };
        assert!(tray || cfg!(unix), "Windows always has a notification area");
        if tray {
            wait_for(&mut app, &data, "tray icon shown", wait);
            // A few seconds on: still no window (the main one, or any other).
            std::thread::sleep(Duration::from_secs(3));
            if let Some(n) = visible_windows(app.id(), "") {
                assert_eq!(n, 0, "visible windows after a start with the session");
            }
        }
        // The old entry, written again with the argument.
        wait_for(&mut app, &data, "startup entry written again", wait);
        let entry = startup.read();
        assert!(entry.contains(&format!("\"{APP}\" /startup")), "{entry}");

        // A download handed over while it waits there (`/d`, the channel the browser's host
        // uses; port 9 on this computer: nothing answers, nothing leaves it): Download File
        // Info opens (#145, #173).
        let status = env(&mut Command::new(APP), &data, &startup)
            .args(["/d", "http://127.0.0.1:9/mdm-test-226.zip"])
            .status()
            .unwrap();
        assert!(status.success());
        wait_for(&mut app, &data, "window opened: file-info", wait);

        // A second launch by the user (the icon clicked): the window, as always.
        let status = env(&mut Command::new(APP), &data, &startup)
            .status()
            .unwrap();
        assert!(status.success());
        if tray {
            let until = Instant::now() + Duration::from_secs(20);
            loop {
                let n = visible_windows(app.id(), MAIN);
                if n.is_none_or(|n| n >= 1) {
                    break;
                }
                assert!(Instant::now() < until, "the window didn't show");
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }));
    let _ = env(&mut Command::new(APP), &data, &startup)
        .arg("/exit")
        .status();
    let until = Instant::now() + Duration::from_secs(15);
    while app.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = app.kill();
    let _ = app.wait();
    drop(startup);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
