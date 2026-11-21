//! misterdownloadmanager-cli's `--json`, end to end: a download's progress and result as JSON lines, and failures as
//! `{"error"}` with a failing exit code.

use std::process::Command;

use mdm_labserver::{Config, Faults, LabServer};
use serde_json::Value;

/// The app's program, which runs misterdownloadmanager-cli's commands when its launcher
/// (crates/cli) starts it with `--cli`.
const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");

fn lines(out: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_reports_progress_then_done() {
    let s = LabServer::start(Config {
        faults: Faults::parse("rate=4M").unwrap(),
        ..Config::default()
    })
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("json.bin");
    // A data folder of its own: the command line reads the app's proxy settings (#1556), and
    // those of whoever runs the tests stay out of it.
    let data = tempfile::tempdir().unwrap();
    let out = tokio::task::spawn_blocking(move || {
        Command::new(APP)
            .arg(mdm_cli::FLAG)
            .args(["--json", "get", &s.url("/gen/3M?seed=5"), "-o"])
            .arg(&dest)
            .env("MDM_DATA_DIR", data.path())
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events = lines(&out.stdout);
    let (last, progress) = events.split_last().unwrap();
    assert!(!progress.is_empty() && progress.iter().all(|e| e["event"] == "progress"));
    assert!(progress.iter().any(|e| e["size"] == 3 << 20));
    assert_eq!(last["event"], "done");
    assert_eq!(last["size"], 3 << 20);
    assert!(last["path"].as_str().unwrap().ends_with("json.bin"));
}

#[test]
fn failures_are_json_with_a_failing_exit_code() {
    // No app runs on this data folder.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(APP)
        .arg(mdm_cli::FLAG)
        .args(["--json", "status"])
        .env("MDM_DATA_DIR", dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let e = &lines(&out.stdout)[0];
    assert!(
        e["error"].as_str().unwrap().contains("isn't running"),
        "{e}"
    );

    let out = Command::new(APP)
        .arg(mdm_cli::FLAG)
        .args(["--json", "resume"])
        .arg(dir.path().join("missing.mdm.json"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(lines(&out.stdout)[0]["error"].is_string());
}
