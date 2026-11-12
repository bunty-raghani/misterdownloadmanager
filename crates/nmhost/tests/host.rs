//! The native host as the browser runs it: the real program, its standard streams framed as
//! native messages, its own data folder (MDM_DATA_DIR) with a fake Mister Download Manager
//! listening where `ipc.json` says, answering as the test wants.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const HOST: &str = env!("CARGO_BIN_EXE_misterdownloadmanager-nmhost");
const CALLER: &str = "chrome-extension://oogbclbbninliongijffeeggalppliok/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/141.0.0.0 Safari/537.36";

/// A data folder of its own, with the app's Options as `settings` (downloads.json).
fn data_dir(name: &str, settings: serde_json::Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mdm-nmhost-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let list = serde_json::json!({ "settings": settings });
    std::fs::write(dir.join("downloads.json"), list.to_string()).unwrap();
    dir
}

/// A fake app at the address `ipc.json` names: each request line it gets goes to the returned
/// receiver, and it answers `answer`.
fn fake_app(dir: &std::path::Path, answer: &'static str) -> mpsc::Receiver<serde_json::Value> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        dir.join("ipc.json"),
        serde_json::json!({ "port": port, "token": "t0k3n" }).to_string(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&conn).read_line(&mut line).is_ok() {
                let _ = tx.send(serde_json::from_str(&line).unwrap_or_default());
                let _ = (&conn).write_all(format!("{answer}\n").as_bytes());
            }
        }
    });
    rx
}

/// Run the host as `caller` with `messages`, returning its replies and exit success.
fn run(
    dir: &std::path::Path,
    caller: &str,
    messages: &[serde_json::Value],
) -> (Vec<serde_json::Value>, bool) {
    let mut child = Command::new(HOST)
        .arg(caller)
        .env("MDM_DATA_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for m in messages {
        let body = m.to_string().into_bytes();
        // A host that refuses its caller exits at once: what's left to send meets a closed
        // pipe, which is its answer, not a failure of the test.
        let sent = stdin
            .write_all(&(body.len() as u32).to_le_bytes())
            .and_then(|()| stdin.write_all(&body));
        if sent.is_err() {
            break;
        }
    }
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let ok = child.wait().unwrap().success();
    let mut replies = Vec::new();
    let mut at = 0;
    while at + 4 <= out.len() {
        let len = u32::from_le_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        replies.push(serde_json::from_slice(&out[at + 4..at + 4 + len]).unwrap());
        at += 4 + len;
    }
    (replies, ok)
}

/// #382: nothing edited in Options. A model is taken, a picture stays with the browser.
#[test]
fn the_default_list_takes_a_model_and_leaves_a_picture() {
    let dir = data_dir("default-types", serde_json::json!({}));
    let app = fake_app(&dir, "ok");
    let offer = |name: &str| {
        serde_json::json!({
            "url": format!("https://huggingface.co/org/repo/resolve/main/{name}"),
            "userAgent": UA, "filename": format!("C:/dl/{name}"),
        })
    };
    let (replies, ok) = run(
        &dir,
        CALLER,
        &[offer("model.Q4_K_M.gguf"), offer("photo.jpg")],
    );
    assert!(ok);
    assert_eq!(replies[0]["ok"], true, "{:?}", replies[0]);
    assert_eq!(
        app.recv_timeout(Duration::from_secs(5)).unwrap()["name"],
        "model.Q4_K_M.gguf"
    );
    assert_ne!(replies[1]["ok"], true, "{:?}", replies[1]);
    assert!(app.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn rules_hand_off_and_the_status_per_browser() {
    let dir = data_dir(
        "handoff",
        serde_json::json!({ "auto_types": "ZIP PDF", "capture_min_kb": 50 }),
    );
    let app = fake_app(&dir, "ok");
    let (replies, ok) = run(
        &dir,
        CALLER,
        &[
            serde_json::json!({ "rules": true, "userAgent": UA, "brands": ["Brave"], "v": "0.2.5" }),
            serde_json::json!({
                "url": "https://x.test/get?id=1", "finalUrl": "https://cdn.test/f/a.zip",
                "referrer": "https://x.test/", "cookies": "sid=1", "userAgent": UA,
                "filename": "C:\\dl\\a.zip", "size": 200000, "mime": "application/zip",
                "post": "token=abc", "postType": "application/x-www-form-urlencoded", "private": true
            }),
        ],
    );
    assert!(ok);
    let rules = &replies[0];
    assert_eq!(rules["ok"], true);
    assert_eq!(rules["types"], "ZIP PDF");
    assert_eq!(rules["minKb"], 50);
    assert_eq!(rules["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(replies[1]["ok"], true, "{:?}", replies[1]);
    let sent = app.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(sent["token"], "t0k3n");
    assert_eq!(sent["url"], "https://x.test/get?id=1");
    assert_eq!(sent["final_url"], "https://cdn.test/f/a.zip");
    assert_eq!(sent["name"], "a.zip");
    assert_eq!(
        (sent["size"].as_u64(), sent["post"].as_str()),
        (Some(200000), Some("token=abc"))
    );
    assert_eq!(sent["private"], true);
    let seen = std::fs::read_to_string(dir.join("extension-seen")).unwrap();
    assert!(seen.contains("Brave Browser\t0.2.5"), "{seen}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn options_and_a_refusing_app_start_nothing() {
    let dir = data_dir("refuse", serde_json::json!({ "capture_min_kb": 100 }));
    // The app says the command is too large: said back, no second app started (none would
    // answer the fake's port anyway; the host would wait 20 s for one).
    let app = fake_app(&dir, "too-large");
    let started = std::time::Instant::now();
    let (replies, _) = run(
        &dir,
        CALLER,
        &[
            serde_json::json!({ "url": "https://x.test/a.zip", "filename": "a.zip", "userAgent": UA }),
            // Left to the browser by Options: an excluded site, a file under the minimum.
            serde_json::json!({ "url": "https://dl.update.microsoft.com/kb.exe", "filename": "kb.exe", "userAgent": UA }),
            serde_json::json!({ "url": "https://x.test/tiny.zip", "filename": "tiny.zip", "size": 2048, "userAgent": UA }),
        ],
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "no wait for an app that answered"
    );
    assert_eq!(replies[0]["why"], "too-large");
    assert_eq!(replies[1]["why"], "options");
    assert_eq!(replies[2]["why"], "options");
    assert!(
        app.recv_timeout(Duration::from_secs(1)).is_ok(),
        "the first was offered"
    );
    assert!(app.try_recv().is_err(), "the others never reached the app");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #208: every download left to the browser is in MDM's log with why, whether the host refused
/// it or the extension left it alone by the same rules (and only told the host); none reaches
/// the app. A private window's is logged without its site or name.
#[test]
fn what_is_left_to_the_browser_is_logged() {
    let dir = data_dir("logged", serde_json::json!({ "auto_types": "ZIP" }));
    let app = fake_app(&dir, "ok");
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::write(dir.join("logs").join("mdm.log"), "an earlier line\n").unwrap();
    let (replies, ok) = run(
        &dir,
        CALLER,
        &[
            serde_json::json!({ "url": "https://x.test/p.png", "filename": "p.png", "userAgent": UA }),
            serde_json::json!({ "url": "https://dl.update.microsoft.com/kb.zip", "filename": "kb.zip", "userAgent": UA }),
            serde_json::json!({ "url": "https://secret.test/s.png", "filename": "s.png", "userAgent": UA, "private": true }),
            serde_json::json!({ "declined": { "why": "site-access", "site": "y.test", "name": "b.zip" }, "userAgent": UA }),
        ],
    );
    assert!(ok);
    let declined: Vec<_> = replies.iter().map(|r| r["declined"].clone()).collect();
    assert_eq!(
        declined,
        [
            serde_json::json!("file-type"),
            serde_json::json!("site"),
            serde_json::json!("file-type"),
            serde_json::Value::Null
        ]
    );
    assert_eq!(replies[3]["ok"], true, "a notice is only noted");
    assert!(
        app.recv_timeout(Duration::from_millis(500)).is_err(),
        "nothing reached the app"
    );
    let log = std::fs::read_to_string(dir.join("logs").join("mdm.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 5, "{log}");
    assert_eq!(lines[0], "an earlier line");
    assert!(
        lines[1].contains("INFO nmhost")
            && lines[1].contains(
                "Google Chrome keeps p.png from x.test: its type isn't in Options > File types"
            ),
        "{log}"
    );
    assert!(
        lines[2].contains("kb.zip from dl.update.microsoft.com: its site is in"),
        "{log}"
    );
    assert!(
        lines[3].contains("a download from a private window") && !log.contains("secret"),
        "{log}"
    );
    assert!(
        lines[4]
            .contains("b.zip from y.test: the browser doesn't let the extension read this site"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn strangers_are_refused() {
    let dir = data_dir("strangers", serde_json::json!({}));
    let (replies, ok) = run(
        &dir,
        "chrome-extension://someoneelse/",
        &[serde_json::json!({ "rules": true })],
    );
    assert!(!ok && replies.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// #41: a user with only Zen and Helium (no ~/.mozilla, no Chrome) gets the manifests where
/// those two read them: Mozilla's folder, created, and Helium's own, with the store's extension
/// offered to Helium (#685). `--unregister` takes them away. (Windows registers in the registry, which every one of them reads.)
#[cfg(not(windows))]
#[test]
fn zen_and_helium_alone_are_registered() {
    let home = data_dir("zen-helium", serde_json::json!({}));
    let mac = cfg!(target_os = "macos");
    let support = home.join("Library/Application Support");
    let (zen, helium, mozilla_hosts, helium_hosts) = if mac {
        (
            support.join("Zen"),
            support.join("net.imput.helium"),
            support.join("Mozilla/NativeMessagingHosts"),
            support.join("net.imput.helium/NativeMessagingHosts"),
        )
    } else {
        (
            home.join(".zen"),
            home.join(".config/net.imput.helium"),
            home.join(".mozilla/native-messaging-hosts"),
            home.join(".config/net.imput.helium/NativeMessagingHosts"),
        )
    };
    std::fs::create_dir_all(&zen).unwrap();
    std::fs::create_dir_all(&helium).unwrap();
    let host = |arg: &str| {
        let out = Command::new(HOST)
            .arg(arg)
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .env("MDM_DATA_DIR", &home)
            .output()
            .unwrap();
        assert!(out.status.success(), "{arg}: {out:?}");
    };
    host("--register");
    let manifest = |dir: &std::path::Path| -> serde_json::Value {
        let file = dir.join("com.misterdownloadmanager.host.json");
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap()
    };
    assert!(manifest(&mozilla_hosts)["allowed_extensions"].is_array());
    // Both copies of the extension connect: the Chrome Web Store's and the app folder's.
    let origins = manifest(&helium_hosts)["allowed_origins"].clone();
    for id in [
        "ejoleppdfgjdndddcjoehopknmlglojp",
        "oogbclbbninliongijffeeggalppliok",
    ] {
        let origin = serde_json::json!(format!("chrome-extension://{id}/"));
        assert!(origins.as_array().unwrap().contains(&origin), "{origins}");
    }
    // #685: Helium offers the store's extension at its next start, from its own folder, through
    // its own services (its name for the store; Google's answers it "no update").
    let offer = helium.join("External Extensions/ejoleppdfgjdndddcjoehopknmlglojp.json");
    let offered: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&offer).unwrap()).unwrap();
    assert_eq!(
        offered["external_update_url"],
        "https://update-url-to-be-replaced.qjz9zk"
    );
    host("--unregister");
    for dir in [&mozilla_hosts, &helium_hosts] {
        assert!(!dir.join("com.misterdownloadmanager.host.json").exists());
    }
    assert!(!offer.exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// #253: a user with only Brave Origin gets the manifest where it reads it (Linux: its own
/// `Brave-Origin` folder; macOS: Chrome's, which every Brave reads), naming this very program.
#[cfg(not(windows))]
#[test]
fn brave_origin_alone_is_registered() {
    let home = data_dir("brave-origin", serde_json::json!({}));
    let mac = cfg!(target_os = "macos");
    let support = home.join("Library/Application Support");
    let (origin, hosts) = if mac {
        (
            support.join("BraveSoftware/Brave-Origin"),
            support.join("Google/Chrome/NativeMessagingHosts"),
        )
    } else {
        (
            home.join(".config/BraveSoftware/Brave-Origin"),
            home.join(".config/BraveSoftware/Brave-Origin/NativeMessagingHosts"),
        )
    };
    std::fs::create_dir_all(&origin).unwrap();
    let out = Command::new(HOST)
        .arg("--register")
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDIR")
        .env("MDM_DATA_DIR", &home)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let file = hosts.join("com.misterdownloadmanager.host.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        std::fs::canonicalize(manifest["path"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(HOST).unwrap()
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// #253: run from inside an AppImage's mount (APPDIR), the host refuses to register itself: the
/// mount is gone once the AppImage quits, and a manifest would name a program that isn't there.
#[cfg(not(windows))]
#[test]
fn a_host_inside_an_appimage_mount_is_not_registered() {
    let home = data_dir("appimage-mount", serde_json::json!({}));
    std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
    let mount = PathBuf::from(HOST).parent().unwrap().to_owned();
    let out = Command::new(HOST)
        .arg("--register")
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env("APPDIR", &mount)
        .env("MDM_DATA_DIR", &home)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("AppImage"));
    assert!(
        !home
            .join(".config/google-chrome/NativeMessagingHosts/com.misterdownloadmanager.host.json")
            .exists()
    );
    let _ = std::fs::remove_dir_all(&home);
}
