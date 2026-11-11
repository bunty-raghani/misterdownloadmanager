//! `misterdownloadmanager --headless` (#157): MDM on a server (a NAS, Docker, Unraid), without a
//! window. The same app runs (its list, queues, Scheduler, limits, torrents, the automation API
//! and the remote control), driven by a loop instead of the window's frames; downloads come from
//! the remote control's page, the browser extension pointed at it, or the command line.
//!
//! Settings come from the data folder's list (`downloads.json`, as on a desktop), then from the
//! environment, then from these flags, which win:
//!
//! | Flag | Environment | |
//! |---|---|---|
//! | `--data <dir>` | `MDM_DATA_DIR` | the data folder (list, settings, certificate, devices) |
//! | `--downloads <dir>` | `MDM_DOWNLOADS` | where downloads go (each category's folder inside) |
//! | `--port <n>` | `MDM_REMOTE_PORT` | the remote control's port (8463) |
//! | `--listen <addr>` | `MDM_REMOTE_LISTEN` | where it listens: "" every local network address |
//! | `--hosts <names>` | `MDM_REMOTE_HOSTS` | names besides addresses it answers to (a NAS's) |
//! | | `MDM_REMOTE_URL` | the page's address as people open it (a container's isn't) |
//!
//! The remote control is on (that is the point); `MDM_REMOTE=off` turns it off. With no device
//! paired yet, a pairing code is printed at start; `misterdownloadmanager-cli pair` prints
//! another.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Is this run headless (`--headless` among the arguments)?
pub fn asked(args: &[String]) -> bool {
    args.iter().any(|a| a == "--headless")
}

/// The value of `--name <value>` or `--name=<value>`, else the environment's `var`.
fn setting(args: &[String], name: &str, var: &str) -> Option<String> {
    let flag = format!("--{name}");
    let given = args.iter().enumerate().find_map(|(i, a)| {
        if *a == flag {
            args.get(i + 1).cloned()
        } else {
            a.strip_prefix(&format!("{flag}=")).map(str::to_owned)
        }
    });
    given
        .or_else(|| std::env::var(var).ok())
        .filter(|v| !v.trim().is_empty())
}

pub fn main(args: Vec<String>) -> ExitCode {
    // A Windows server has no console for a windowed program: the one it was started from.
    #[cfg(windows)]
    crate::win32::cli_console(None);
    if let Some(dir) = setting(&args, "data", "MDM_DATA_DIR") {
        mdm_ipc::set_data_dir(PathBuf::from(dir));
    }
    let Some(data) = mdm_ipc::data_dir() else {
        eprintln!("No data folder: give one with --data <folder>.");
        return ExitCode::FAILURE;
    };
    if let Err(e) = mdm_ipc::private_dir(&data) {
        eprintln!(
            "The data folder {} can't be used: {e}{}",
            data.display(),
            not_ours(&e)
        );
        return ExitCode::FAILURE;
    }
    let Some(_instance) = mdm_ipc::instance_lock() else {
        eprintln!(
            "Another Mister Download Manager uses the data folder {}.",
            data.display()
        );
        return ExitCode::FAILURE;
    };
    mdm_crash::logs::init(&data, mdm_crash::logs::APP);
    // The default browser, looked for once on a thread of its own (logged): its User-Agent is
    // the one links added by hand go with (#668).
    crate::defaultbrowser::start();
    tracing::info!(
        "Mister Download Manager {} ({}) starting without a window",
        crate::help::VERSION,
        crate::help::COMMIT
    );

    // The command channel (the command line, the automation API) and the remote control send
    // their commands down one channel; each wakes the loop.
    let (tx, commands) = std::sync::mpsc::channel();
    let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = wake_tx.try_send(());
    });
    let sink = mdm_ipc::Sink::new(tx.clone(), wake.clone());
    let to_ipc = wake.clone();
    if let Err(e) = mdm_ipc::serve(tx, move || to_ipc()) {
        eprintln!("The command channel could not open: {e}");
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut loaded = crate::store::load();
    let s = &mut loaded.settings;
    s.remote_on = !std::env::var("MDM_REMOTE").is_ok_and(|v| v.eq_ignore_ascii_case("off"));
    if let Some(p) = setting(&args, "port", "MDM_REMOTE_PORT") {
        match p.trim().parse::<u16>() {
            Ok(p) if p > 0 => s.remote_port = p,
            _ => {
                eprintln!("Not a port: {p}");
                return ExitCode::FAILURE;
            }
        }
    }
    if let Some(l) = setting(&args, "listen", "MDM_REMOTE_LISTEN") {
        s.remote_listen = l.trim().to_owned();
    }
    // No one to ask: the language is the system's, and nothing opens a window.
    s.language_asked = true;
    s.show_start_dialog = false;
    s.show_complete = false;
    let hosts = setting(&args, "hosts", "MDM_REMOTE_HOSTS");
    crate::remote::set_hosts(hosts.as_deref());
    let downloads = setting(&args, "downloads", "MDM_DOWNLOADS").map(PathBuf::from);
    if let Some(d) = &downloads
        && let Err(e) = writable(d)
    {
        eprintln!(
            "The downloads folder {} can't be used: {e}{}",
            d.display(),
            not_ours(&e)
        );
        return ExitCode::FAILURE;
    }

    // Stopped by the system (docker stop, systemctl stop, Ctrl+C): the list is saved first.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (stop, wake) = (stop.clone(), wake.clone());
        rt.spawn(async move {
            #[cfg(unix)]
            crate::unix::terminated().await;
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.store(true, Ordering::SeqCst);
            wake();
        });
    }

    let ctx = egui::Context::default();
    let mut app = crate::app::App::new(
        &ctx,
        rt.handle().clone(),
        commands,
        sink,
        loaded,
        crate::Start::Shown,
    )
    .headless(downloads);
    let mut told = false;
    loop {
        let out = ctx.run_pass(egui::RawInput::default(), |ctx| app.tick(ctx));
        let delay = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::from_secs(1), |v| v.repaint_delay);
        // A pass draws nothing: its textures are dropped, not uploaded anywhere.
        let mut out = out;
        out.textures_delta.clear();
        if !told {
            told = tell(&mut app);
        }
        if stop.load(Ordering::SeqCst) || app.wants_exit() {
            break;
        }
        let _ =
            wake_rx.recv_timeout(delay.clamp(Duration::from_millis(100), Duration::from_secs(1)));
    }
    tracing::info!("stopping: the list is saved");
    println!("Stopping: the list is saved.");
    drop(app);
    ExitCode::SUCCESS
}

/// The folder, made if it isn't there, and a file written in it: a folder MDM can't write fails
/// at start, not at each download.
fn writable(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".mdm-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// What to do when a folder belongs to another user: Docker makes a folder it mounts that isn't
/// there yet (`-v /srv/mdm-config:/config`) as root's, and MDM runs as another user.
fn not_ours(e: &std::io::Error) -> &'static str {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "\nIt belongs to another user: make it before the first start, owned by the user MDM runs \
         as (`mkdir`, then `chown` it or give Docker that user with `--user`)."
    } else {
        ""
    }
}

/// Say where the remote control listens, with a pairing code while no device is paired; true
/// once said (or when it is off).
fn tell(app: &mut crate::app::App) -> bool {
    let Some(remote) = app.remote.clone() else {
        println!("The remote control is off (MDM_REMOTE=off).");
        return true;
    };
    match remote.status() {
        mdm_remote::Status::Off => {
            println!("The remote control is off (MDM_REMOTE=off).");
            true
        }
        mdm_remote::Status::Listening(_) => {
            let mut urls = remote.urls();
            if let Some(public) = crate::remote::public_url() {
                urls.insert(0, public);
            }
            println!("The remote control listens at: {}", urls.join("  "));
            if let Some(fp) = remote.fingerprint() {
                println!("Its certificate's SHA-256 fingerprint: {fp}");
            }
            if remote.devices().is_empty() {
                let answer: Result<mdm_ipc::api::PairAnswer, _> =
                    serde_json::from_str(&app.pair_answer());
                if let Ok(a) = answer {
                    println!(
                        "No device is paired yet. Open the address (or scan this) and enter the code {} {} (it works for {} minutes; `misterdownloadmanager-cli pair` shows a new one):\n{}",
                        &a.code[..4],
                        &a.code[4..],
                        a.minutes,
                        a.qr
                    );
                }
            }
            true
        }
        mdm_remote::Status::NoAddress => {
            println!(
                "The remote control has no local network address to listen on yet; it starts when one comes (--listen chooses one)."
            );
            true
        }
        mdm_remote::Status::PortInUse(p) => {
            println!("The port {p} is used by another program: choose another with --port.");
            true
        }
        mdm_remote::Status::Failed(e) => {
            println!("The remote control could not start: {e}");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_win_over_the_environment() {
        let args: Vec<String> = ["--headless", "--port", "9000", "--listen=127.0.0.1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(asked(&args));
        assert!(!asked(&["/d".into(), "x".into()]));
        assert_eq!(
            setting(&args, "port", "MDM_TEST_UNSET_PORT").as_deref(),
            Some("9000")
        );
        assert_eq!(
            setting(&args, "listen", "MDM_TEST_UNSET_LISTEN").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(setting(&args, "hosts", "MDM_TEST_UNSET_HOSTS"), None);
        // An empty value is no value.
        let empty: Vec<String> = vec!["--downloads".into(), " ".into()];
        assert_eq!(setting(&empty, "downloads", "MDM_TEST_UNSET_DL"), None);
    }

    /// A downloads folder is made and written to at start, leaving nothing behind; a folder of
    /// another user's gets the hint.
    #[test]
    fn folders_are_checked_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("Downloads");
        writable(&d).unwrap();
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(not_ours(&denied).contains("chown"));
        assert_eq!(not_ours(&std::io::Error::other("full")), "");
    }
}
