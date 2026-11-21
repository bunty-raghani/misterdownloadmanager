//! The command line's downloads go through Options → Proxy / Socks (#1556): the app's settings
//! read from the data folder (a manual proxy, an automatic configuration script), and `--proxy`
//! for one run. The lab server stands for an HTTP proxy: a request for another host reaches it
//! as `GET http://host/…`, its log keeps that host, and it serves it by its path.

use std::path::Path;
use std::process::{Command, Output};

use mdm_labserver::{Config, LabServer};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");
/// A host nothing on this computer can look up: only through the proxy can it be reached.
const SITE: &str = "mdm-test.invalid";

async fn lab() -> LabServer {
    LabServer::start(Config::default()).await.unwrap()
}

/// A data folder whose settings are `settings` (JSON), as the app saves them.
fn data_with(settings: serde_json::Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("downloads.json"),
        serde_json::json!({ "settings": settings }).to_string(),
    )
    .unwrap();
    dir
}

/// `get url` with these extra arguments and this data folder, into a folder of its own.
async fn get(url: String, data: &Path, extra: &[&str]) -> Output {
    let (data, extra) = (
        data.to_owned(),
        extra.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    tokio::task::spawn_blocking(move || {
        let out = tempfile::tempdir().unwrap();
        Command::new(APP)
            .arg(mdm_cli::FLAG)
            .args(["--json", "get", &url, "-c", "2", "-o"])
            .arg(out.path())
            .args(&extra)
            .env("MDM_DATA_DIR", &data)
            // The proxy variables of whoever runs the tests stay out of it.
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy")
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}

fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every request the lab served was for `SITE`: it came through it as a proxy.
fn proxied(lab: &LabServer) {
    let log = lab.log();
    assert!(!log.is_empty(), "nothing reached the proxy");
    assert!(
        log.iter().all(|e| e.host.as_deref() == Some(SITE)),
        "{:?}",
        log.iter().map(|e| &e.host).collect::<Vec<_>>()
    );
}

/// Options → Proxy / Socks → Manual: the download goes through it.
#[tokio::test(flavor = "multi_thread")]
async fn options_manual_proxy() {
    let lab = lab().await;
    let data = data_with(serde_json::json!({
        "proxy_from_browser": false,
        "proxy_mode": 3,
        "proxy_host": "127.0.0.1",
        "proxy_port": lab.addr().port().to_string(),
        "proxy_http": true,
        "proxy_https": true,
    }));
    ok(&get(format!("http://{SITE}/gen/1M?seed=3"), data.path(), &[]).await);
    proxied(&lab);
}

/// Options → Proxy / Socks → Use automatic configuration script: fetched before the download,
/// which goes where it says.
#[tokio::test(flavor = "multi_thread")]
async fn options_automatic_configuration_script() {
    let lab = lab().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("proxy.pac");
    std::fs::write(
        &script,
        format!(
            "function FindProxyForURL(url, host) {{ return 'PROXY 127.0.0.1:{}'; }}",
            lab.addr().port()
        ),
    )
    .unwrap();
    let data = data_with(serde_json::json!({
        "proxy_from_browser": false,
        "proxy_mode": 2,
        "proxy_script": script.to_string_lossy(),
    }));
    ok(&get(format!("http://{SITE}/gen/512K?seed=4"), data.path(), &[]).await);
    proxied(&lab);
}

/// `--proxy` for one run, over the settings: an address, or none (straight to the server,
/// though Options' proxy isn't there).
#[tokio::test(flavor = "multi_thread")]
async fn the_proxy_switch_wins_over_options() {
    let lab = lab().await;
    // Options' proxy: a port nothing listens on.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = closed.local_addr().unwrap().port();
    drop(closed);
    let data = data_with(serde_json::json!({
        "proxy_mode": 3,
        "proxy_host": "127.0.0.1",
        "proxy_port": dead.to_string(),
        "proxy_http": true,
        "proxy_https": true,
    }));
    let through = format!("http://127.0.0.1:{}", lab.addr().port());
    ok(&get(
        format!("http://{SITE}/gen/512K?seed=6"),
        data.path(),
        &["--proxy", &through],
    )
    .await);
    proxied(&lab);
    // Options' proxy alone: it isn't there, so the download fails rather than going around it.
    let direct = lab.url("/gen/256K?seed=7");
    let failed = get(direct.clone(), data.path(), &[]).await;
    assert!(!failed.status.success());
    // `--proxy none`: straight to the lab.
    ok(&get(direct, data.path(), &["--proxy", "none"]).await);
}

/// A password is never taken on the command line; an unknown scheme is refused.
#[test]
fn a_password_in_the_proxy_switch_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for (proxy, says) in [
        ("http://ann:secret@127.0.0.1:8080", "MDM_PROXY_PASSWORD"),
        ("ftp://127.0.0.1:21", "not a proxy's"),
    ] {
        let out = Command::new(APP)
            .arg(mdm_cli::FLAG)
            .args(["get", "http://127.0.0.1:9/a", "--proxy", proxy])
            .env("MDM_DATA_DIR", dir.path())
            .output()
            .unwrap();
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(says), "{err}");
    }
}
