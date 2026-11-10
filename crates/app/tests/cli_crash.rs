//! Crash safety, end to end: kill the command line's process mid-download (no clean shutdown, like a
//! crash or power cut for the process), resume from the state file, verify every byte.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mdm_engine::state::{ResumeState, state_path};
use mdm_labserver::{Config, Faults, LabServer, content};

/// The app's program, which runs misterdownloadmanager-cli's commands when its launcher
/// (crates/cli) starts it with `--cli`.
const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");

#[tokio::test(flavor = "multi_thread")]
async fn killed_download_resumes_correctly() {
    let s = LabServer::start(Config {
        faults: Faults::parse("rate=1M").unwrap(),
        ..Config::default()
    })
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("crash.bin");
    let size: u64 = 24 << 20;

    let mut child = Command::new(APP)
        .arg(mdm_cli::FLAG)
        .args(["get", &s.url("/gen/24M?seed=11"), "-o"])
        .arg(&dest)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    // Wait until saved progress shows real data, then kill without warning.
    let state = state_path(&dest);
    let deadline = Instant::now() + Duration::from_secs(30);
    let saved_before = loop {
        assert!(Instant::now() < deadline, "no progress saved");
        if let Ok(st) = ResumeState::load(&state)
            && st.downloaded() > 4 << 20
        {
            break st.downloaded();
        }
        // A download that ends first never saves progress for this test to find (#518).
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the download ended ({status}) before saving any progress");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!dest.exists(), "not complete yet");

    let status = Command::new(APP)
        .arg(mdm_cli::FLAG)
        .arg("resume")
        .arg(&state)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());

    let data = std::fs::read(&dest).unwrap();
    let mut want = vec![0; size as usize];
    content::fill(11, 0, &mut want);
    assert!(data == want, "content mismatch after crash + resume");
    assert!(!state.exists());
    assert!(saved_before < size);
}
