//! MediaFire links (#952) against the lab's fake MediaFire (`mdm_labserver::mediafire`): its host
//! names resolve to the lab server, which answers as MediaFire does (a file's page with its
//! download button, file host addresses whose tickets expire, the repair page after), over plain
//! http on the lab's port.

use std::time::{Duration, Instant};

use mdm_engine::{Destination, Download, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content, mediafire};
use url::Url;

#[path = "support/resume.rs"]
mod resume;
use resume::on_disk;

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

/// MediaFire's hosts at the lab server.
fn opts() -> Options {
    Options {
        hosts: mediafire::HOSTS
            .iter()
            .map(|h| (h.to_string(), vec!["127.0.0.1".parse().unwrap()]))
            .collect(),
        retry_delay: Duration::from_millis(50),
        ..Options::default()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The file `key`'s address on the file host, good until `expires` (unix ms), as the browser
/// hands it over.
fn file_address(s: &LabServer, key: &str, expires: u64) -> Url {
    mediafire::file_address(s.addr().port(), key, expires)
        .parse()
        .unwrap()
}

/// The file is the fake MediaFire's `key`, byte for byte, of `size` bytes.
fn check(path: &std::path::Path, key: &str, size: usize) {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; size];
    content::fill(mediafire::seed(key), 0, &mut want);
    assert_eq!(data.len(), size, "{}", path.display());
    assert!(data == want, "content mismatch: {}", path.display());
}

/// The log once the server has logged every request, the bodies the client dropped too.
async fn log(s: &LabServer) -> Vec<LogEntry> {
    tokio::time::timeout(Duration::from_secs(20), s.settled())
        .await
        .expect("the lab server settles");
    s.log()
}

fn on(e: &LogEntry, host: &str) -> bool {
    e.host.as_deref().is_some_and(|h| h.starts_with(host))
}

/// A file's page, by any of its addresses, gives the file (its button's address), named by the
/// file host, over several connections.
#[tokio::test]
async fn a_files_page_gives_the_file() {
    let s = server("").await;
    let key = mediafire::key("2M", 600_000);
    for path in [
        format!("/file/{key}/Lab%20file.zip/file"),
        format!("/file_premium/{key}/Lab%20file.zip"),
        format!("/download/{key}"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://{}:{}{path}", mediafire::WWW, s.addr().port());
        let out = Download::start(
            Request {
                url: url.parse().unwrap(),
                headers: Default::default(),
                dest: Destination::Dir(dir.path().to_owned()),
            },
            opts(),
        )
        .wait()
        .await
        .unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(out.path, dir.path().join(mediafire::FILE_NAME), "{path}");
        check(&out.path, &key, 2 << 20);
    }
}

/// The browser's address, expired before MDM asks it (a download added for later): the repair
/// page it leads to goes back to the file's page, whose button has today's address.
#[tokio::test]
async fn an_expired_address_leads_to_todays() {
    let s = server("").await;
    let key = mediafire::key("1M", 600_000);
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: file_address(&s, &key, 1),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await
    .unwrap();
    check(&out.path, &key, 1 << 20);
    let log = log(&s).await;
    assert!(
        log.iter()
            .any(|e| on(e, mediafire::WWW) && e.path == "/download_repair.php"),
        "the repair page"
    );
    assert!(
        log.iter()
            .any(|e| on(e, mediafire::WWW) && e.path == format!("/file/{key}")),
        "the file's page"
    );
    assert!(
        log.iter()
            .any(|e| on(e, mediafire::FILES) && e.status == 200 && !e.path.starts_with("/t1/")),
        "the file from today's address"
    );
}

/// #952: a download the browser handed over, stopped, and resumed once its address has expired
/// (MediaFire's last a few hours): its connections meet the repair page, a web page, and the
/// download asks the file's page for today's address and goes on from there, the data it had
/// kept. Before, it failed with "the site sent a web page instead of the file".
#[tokio::test]
async fn a_download_resumed_after_its_address_expired_goes_on() {
    let s = server("rate=1M").await;
    let key = mediafire::key("4M", 600_000);
    let size = 4usize << 20;
    let expires = now_ms() + 1500;
    let faults = s.faults();
    s.set_faults(Faults {
        stall_at: Some(2 << 20),
        ..faults.clone()
    });
    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url: file_address(&s, &key, expires),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            save_interval: Duration::from_millis(100),
            ..opts()
        },
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if snap.file.is_some() && snap.downloaded >= 512 << 10 {
            break;
        }
        assert!(Instant::now() < deadline, "the download never started");
    }
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(mdm_engine::Error::Cancelled)));
    // The held-back answers go on (into connections the download closed), and end.
    s.set_faults(Faults::default());
    tokio::time::timeout(Duration::from_secs(10), s.settled())
        .await
        .expect("the lab server settles");
    let state = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".mdm.json"))
        .expect("a resume state");
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    let kept = saved.downloaded();
    assert!(kept > 0 && kept < size as u64, "partly downloaded: {kept}");
    // Its address expires meanwhile.
    while now_ms() <= expires {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.clear_log();

    let out = Download::resume(state.clone(), Default::default(), opts())
        .unwrap()
        .wait()
        .await
        .unwrap();
    check(&out.path, &key, size);
    assert!(!state.exists(), "state file removed on completion");
    let log = log(&s).await;
    assert!(
        log.iter()
            .any(|e| on(e, mediafire::WWW) && e.path == format!("/file/{key}")),
        "today's address asked of the file's page"
    );
    // Only what was missing is asked again of today's address: of what was on disk before the
    // refresh, a request takes at most the few bytes each part re-reads to check the file is the
    // same (`verify_overlap`; the refresh's look at the new address, `bytes=0-0`, fits too).
    // Not a total of what the server sent: a part split between two connections while its bytes
    // are on their way comes twice, and on a busy machine more of them are (#1034).
    let overlap = opts().verify_overlap;
    let asked: Vec<&LogEntry> = log
        .iter()
        .filter(|e| on(e, mediafire::FILES) && matches!(e.status, 200 | 206))
        .collect();
    assert!(!asked.is_empty(), "the file asked of today's address");
    for e in asked {
        let again = on_disk(e.range.as_deref(), &saved);
        assert!(
            again <= overlap,
            "asked again for {again} bytes it had kept: {:?}, {kept} kept",
            e.range
        );
    }
}
