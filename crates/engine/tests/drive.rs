//! Google Drive links (#34) against the lab's fake Drive (`mdm_labserver::drive`): Google's host
//! names resolve to the lab server, which answers as Drive does, so every step runs as it would
//! against Google, over plain http on the lab's port.

use std::time::Duration;

use mdm_engine::{Destination, Download, Error, Options, Outcome, Refusal, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content, drive};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use url::Url;

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

/// Google's hosts at the lab server.
fn opts() -> Options {
    Options {
        hosts: drive::HOSTS
            .iter()
            .map(|h| (h.to_string(), vec!["127.0.0.1".parse().unwrap()]))
            .collect(),
        retry_delay: Duration::from_millis(50),
        ..Options::default()
    }
}

/// `http://{host}:{lab port}{path}`, `ID` in `path` replaced by `id`.
fn link(s: &LabServer, host: &str, path: &str, id: &str) -> Url {
    format!(
        "http://{host}:{}{}",
        s.addr().port(),
        path.replace("ID", id)
    )
    .parse()
    .unwrap()
}

fn signed_in() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(COOKIE, HeaderValue::from_static("SID=lab; HSID=x"));
    h
}

async fn download(
    url: Url,
    headers: HeaderMap,
    opts: Options,
) -> (tempfile::TempDir, Result<Outcome, Error>) {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url,
        headers,
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts).wait().await;
    (dir, out)
}

/// The file is the fake Drive's `id`, byte for byte, of `size` bytes.
fn check(out: &Outcome, id: &str, size: usize) {
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; size];
    content::fill(drive::seed(id), 0, &mut want);
    assert_eq!(data.len(), size, "{}", out.path.display());
    assert!(data == want, "content mismatch: {}", out.path.display());
}

/// The log once the server has logged the bodies the client dropped.
async fn log(s: &LabServer) -> Vec<LogEntry> {
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.log()
}

fn files_in(dir: &tempfile::TempDir) -> Vec<String> {
    std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// Every way a Drive file is linked to: the file arrives byte for byte, over several
/// connections, named by Drive, without asking for the viewer or the virus-scan page.
#[tokio::test]
async fn every_link_form_downloads_the_file() {
    let s = server("rate=8M").await;
    let forms = [
        (drive::HOSTS[0], "/file/d/ID/view?usp=sharing"),
        (drive::HOSTS[0], "/file/u/1/d/ID/view"),
        (drive::HOSTS[0], "/uc?id=ID&export=download"),
        (drive::HOSTS[0], "/open?id=ID"),
        (drive::HOSTS[2], "/file/d/ID/edit"),
        (drive::HOSTS[1], "/download?id=ID&export=download"),
    ];
    for (kind, size) in [("small", 64 << 10), ("big", 8 << 20)] {
        let id = drive::id(kind, if kind == "big" { "8M" } else { "64K" }, &[]);
        for (host, path) in forms {
            s.clear_log();
            let url = link(&s, host, path, &id);
            let (_dir, out) = download(url.clone(), HeaderMap::new(), opts()).await;
            let out = out.unwrap_or_else(|e| panic!("{url}: {e}"));
            check(&out, &id, size);
            assert_eq!(
                out.path.file_name().unwrap().to_str(),
                Some(format!("{kind}.bin").as_str())
            );
            let log = log(&s).await;
            assert!(
                log.iter()
                    .all(|e| e.path == "/download" && e.outcome != "page"),
                "{url}: only the file host's file was asked: {:?}",
                log.iter()
                    .map(|e| (&e.path, &e.outcome))
                    .collect::<Vec<_>>()
            );
            if kind == "big" {
                let ranged = log.iter().filter(|e| e.status == 206).count();
                assert!(ranged >= 4, "{url}: {ranged} ranged requests");
            }
        }
    }
}

/// Drive's hosts are asked with a browser's agent (PR #67), every other site with MDM's automatic
/// one (the default browser's, #668), and an agent handed over by the browser wins.
#[tokio::test]
async fn drive_is_asked_with_a_browsers_agent() {
    let automatic = mdm_engine::agent::automatic();
    use mdm_engine::sites::gdrive::USER_AGENT;
    let s = server("").await;
    let id = drive::id("big", "2M", &["uuid"]);
    let url = link(&s, drive::HOSTS[0], "/file/d/ID/view", &id);
    let (_dir, out) = download(url.clone(), HeaderMap::new(), opts()).await;
    check(&out.unwrap(), &id, 2 << 20);
    let other = s.url("/gen/64K/other.bin?seed=3");
    let (_dir, out) = download(other.parse().unwrap(), HeaderMap::new(), opts()).await;
    out.unwrap();
    let entries = log(&s).await;
    let agents = |pick: &dyn Fn(&LogEntry) -> bool| -> Vec<Option<String>> {
        entries
            .iter()
            .filter(|e| pick(e))
            .map(|e| e.user_agent.clone())
            .collect()
    };
    let drive_agents = agents(&|e| e.path == "/download");
    assert!(drive_agents.len() >= 2, "the warning page, then the file");
    assert!(
        drive_agents
            .iter()
            .all(|a| a.as_deref() == Some(USER_AGENT)),
        "{drive_agents:?}"
    );
    let others = agents(&|e| e.path.starts_with("/gen/"));
    assert!(
        !others.is_empty()
            && others
                .iter()
                .all(|a| a.as_deref() == Some(automatic.as_str())),
        "{others:?}"
    );

    s.clear_log();
    let browser = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
    let opts = Options {
        user_agent: browser.into(),
        automatic_agent: false,
        ..opts()
    };
    let (_dir, out) = download(url, HeaderMap::new(), opts).await;
    check(&out.unwrap(), &id, 2 << 20);
    let entries = log(&s).await;
    assert!(
        entries
            .iter()
            .filter(|e| e.path == "/download")
            .all(|e| e.user_agent.as_deref() == Some(browser)),
        "the browser's agent wins"
    );
}

/// A short link (another site's redirect) to a Drive file: the viewer it lands on leads to the
/// file.
#[tokio::test]
async fn a_short_link_to_a_drive_file_gets_the_file() {
    let s = server("").await;
    let id = drive::id("small", "64K", &[]);
    let to = link(&s, drive::HOSTS[0], "/file/d/ID/view?usp=sharing", &id);
    let short = s.url(&format!(
        "/redirect?to={}",
        url::form_urlencoded::byte_serialize(to.as_str().as_bytes()).collect::<String>()
    ));
    let (_dir, out) = download(short.parse().unwrap(), HeaderMap::new(), opts()).await;
    let out = out.unwrap();
    check(&out, &id, 64 << 10);
    assert_eq!(out.path.file_name().unwrap().to_str(), Some("small.bin"));
}

/// When the confirmation given up front isn't enough, the virus-scan page's form is followed.
#[tokio::test]
async fn the_virus_scan_page_is_confirmed_through_its_form() {
    let s = server("").await;
    let id = drive::id("big", "2M", &["uuid"]);
    let url = link(&s, drive::HOSTS[0], "/file/d/ID/view", &id);
    let (_dir, out) = download(url, HeaderMap::new(), opts()).await;
    check(&out.unwrap(), &id, 2 << 20);
    let log = log(&s).await;
    let pages: Vec<_> = log.iter().filter(|e| e.outcome == "page").collect();
    assert_eq!(pages.len(), 1, "one warning page");
    assert_eq!(pages[0].status, 200);
    let after = log.iter().find(|e| e.req > pages[0].req).unwrap();
    assert!(
        after.query.as_deref().is_some_and(|q| q.contains("uuid=")),
        "then the file, with the page's uuid: {after:?}"
    );
}

/// Download File Info's look: the name Drive gives (a non-ASCII one too) and the exact size.
#[tokio::test]
async fn the_look_gets_drives_name_and_size() {
    let s = server("").await;
    for (kind, size, name) in [
        ("small", 64u64 << 10, "small.bin"),
        ("uni", 1 << 20, "naïve résumé.bin"),
        ("big", 3 << 20, "big.bin"),
    ] {
        let id = drive::id(kind, &format!("{}K", size >> 10), &[]);
        for path in ["/file/d/ID/view", "/uc?id=ID", "/open?id=ID"] {
            let url = link(&s, drive::HOSTS[0], path, &id);
            let info = mdm_engine::probe(url.clone(), HeaderMap::new(), &opts())
                .await
                .unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(info.file_name, name, "{url}");
            assert_eq!(info.size, Some(size), "{url}");
            assert_eq!(info.final_url.host_str(), Some(drive::HOSTS[1]));
        }
    }
}

/// "Too many users have downloaded this file": Drive's words, no file, no retry.
#[tokio::test]
async fn a_quota_page_is_a_refusal_and_nothing_is_written() {
    let s = server("").await;
    let id = drive::id("quota", "1M", &[]);
    let url = link(&s, drive::HOSTS[0], "/file/d/ID/view?usp=sharing", &id);
    let (dir, out) = download(url, HeaderMap::new(), opts()).await;
    match out {
        Err(Error::Refused(Refusal::Drive { message })) => {
            assert!(message.contains("Too many users"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));
    let log = log(&s).await;
    assert_eq!(
        log.iter().filter(|e| e.path == "/download").count(),
        1,
        "the file host is asked once"
    );
    // The look at what the ID is: a file, so the refusal stands.
    assert!(log.iter().any(|e| e.path == "/open"));
    // The look (probe) says the same.
    let err = mdm_engine::probe(
        link(&s, drive::HOSTS[0], "/uc?id=ID", &id),
        HeaderMap::new(),
        &opts(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, Error::Refused(Refusal::Drive { .. })),
        "{err}"
    );
}

/// A Google Doc comes as its Office export, whether linked as a Doc or as a Drive file (whose
/// file host answers a Doc's ID with the quota page); the export's own host gets no cookie.
#[tokio::test]
async fn a_google_doc_is_exported() {
    let s = server("").await;
    let id = drive::id("doc", "64K", &[]);
    for (host, path) in [
        (drive::HOSTS[0], "/file/d/ID/view"),
        (drive::HOSTS[2], "/document/d/ID/edit?usp=sharing"),
    ] {
        s.clear_log();
        let url = link(&s, host, path, &id);
        let (_dir, out) = download(url.clone(), signed_in(), opts()).await;
        let out = out.unwrap_or_else(|e| panic!("{url}: {e}"));
        check(&out, &id, 64 << 10);
        assert_eq!(
            out.path.file_name().unwrap().to_str(),
            Some("Lab doc.docx"),
            "{url}"
        );
        let log = log(&s).await;
        let export = log
            .iter()
            .find(|e| e.path.starts_with("/export/tok/"))
            .expect("the export");
        assert!(export.query.as_deref().unwrap().contains("format=docx"));
        assert_eq!(export.cookie, None, "no Google cookie on the export's host");
        assert!(
            log.iter()
                .filter(|e| e.path.contains("/document/"))
                .all(|e| e.cookie.as_deref().is_some_and(|c| c.contains("SID=lab"))),
            "Docs is Drive's party: the cookie goes there"
        );
    }
}

/// A file shared with some people only: sign in. With the browser's cookies, it downloads,
/// the cookies going on to Drive's file host.
#[tokio::test]
async fn a_private_file_needs_the_browsers_sign_in() {
    let s = server("").await;
    let id = drive::id("private", "2M", &[]);
    let url = link(&s, drive::HOSTS[0], "/file/d/ID/view", &id);
    let (dir, out) = download(url.clone(), HeaderMap::new(), opts()).await;
    assert!(
        matches!(out, Err(Error::Refused(Refusal::SignIn))),
        "{out:?}"
    );
    assert!(files_in(&dir).is_empty());
    s.clear_log();
    let (_dir, out) = download(url, signed_in(), opts()).await;
    check(&out.unwrap(), &id, 2 << 20);
    let log = log(&s).await;
    let file_host: Vec<_> = log.iter().filter(|e| e.path == "/download").collect();
    assert!(!file_host.is_empty());
    assert!(
        file_host
            .iter()
            .all(|e| e.cookie.as_deref().is_some_and(|c| c.contains("SID=lab"))),
        "{file_host:?}"
    );
}

#[tokio::test]
async fn no_access_and_folders_are_refusals() {
    let s = server("").await;
    let id = drive::id("noaccess", "1M", &[]);
    let (_dir, out) = download(
        link(&s, drive::HOSTS[0], "/file/d/ID/view", &id),
        signed_in(),
        opts(),
    )
    .await;
    assert!(
        matches!(out, Err(Error::Refused(Refusal::NoAccess))),
        "{out:?}"
    );
    let folder = drive::id("folder", "1K", &[]);
    let (dir, out) = download(
        link(
            &s,
            drive::HOSTS[0],
            "/drive/folders/ID?usp=sharing",
            &folder,
        ),
        HeaderMap::new(),
        opts(),
    )
    .await;
    assert!(
        matches!(out, Err(Error::Refused(Refusal::Folder))),
        "{out:?}"
    );
    assert!(files_in(&dir).is_empty());
    assert!(
        log(&s).await.iter().all(|e| !e.path.contains("folders")),
        "a folder link isn't asked for"
    );
}

/// An older link's resource key goes with it.
#[tokio::test]
async fn a_resource_key_is_kept() {
    let s = server("").await;
    let id = drive::id("rk", "1M", &[]);
    let with = format!("/file/d/ID/view?resourcekey={}", drive::RESOURCE_KEY);
    let (_dir, out) = download(
        link(&s, drive::HOSTS[0], &with, &id),
        HeaderMap::new(),
        opts(),
    )
    .await;
    check(&out.unwrap(), &id, 1 << 20);
    let (_dir, out) = download(
        link(&s, drive::HOSTS[0], "/file/d/ID/view", &id),
        HeaderMap::new(),
        opts(),
    )
    .await;
    assert!(
        matches!(out, Err(Error::Status(s)) if s.as_u16() == 404),
        "{out:?}"
    );
}

/// Pause, then resume: the download goes on from Drive's confirmed address, byte for byte.
#[tokio::test]
async fn a_paused_drive_download_resumes() {
    let s = server("rate=1M").await;
    let id = drive::id("big", "8M", &[]);
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: link(&s, drive::HOSTS[0], "/file/d/ID/view?usp=sharing", &id),
        headers: HeaderMap::new(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let dl = Download::start(
        req,
        Options {
            save_interval: Duration::from_millis(100),
            ..opts()
        },
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded > 0) {
            break f;
        }
        assert!(std::time::Instant::now() < deadline, "never started");
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(Error::Cancelled)));
    let state = mdm_engine::state::state_path(&file);
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert_eq!(saved.final_url.host_str(), Some(drive::HOSTS[1]));
    assert!(saved.downloaded() > 0 && saved.downloaded() < 8 << 20);
    tokio::time::sleep(Duration::from_millis(500)).await;
    s.clear_log();
    let out = Download::resume(state, HeaderMap::new(), opts())
        .unwrap()
        .wait()
        .await
        .unwrap();
    check(&out, &id, 8 << 20);
    let log = log(&s).await;
    assert!(
        log.iter()
            .all(|e| e.path == "/download" && e.outcome != "page"),
        "{:?}",
        log.iter()
            .map(|e| (&e.path, &e.outcome))
            .collect::<Vec<_>>()
    );
}

/// Drive's confirmed address expires while connections still ask for parts: the original link
/// is asked again, through its page and form, and the download goes on at the new address.
#[tokio::test]
async fn an_expired_confirmed_address_is_refreshed_through_the_link() {
    let s = server("rate=1M&drop_after=256K").await;
    let id = drive::id("big", "4M", &["uuid", "ttl500"]);
    let url = link(&s, drive::HOSTS[0], "/file/d/ID/view", &id);
    let (_dir, out) = download(
        url,
        HeaderMap::new(),
        Options {
            max_connections: 4,
            ..opts()
        },
    )
    .await;
    check(&out.unwrap(), &id, 4 << 20);
    let log = log(&s).await;
    assert!(
        log.iter().any(|e| e.outcome == "expired"),
        "the address expired during the download"
    );
    assert!(
        log.iter().filter(|e| e.outcome == "page").count() >= 2,
        "the warning page was read again for a fresh address"
    );
}
