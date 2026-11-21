//! Rapidgator's links against the lab's fake Rapidgator (`mdm_labserver::rapidgator`): its host
//! names resolve to the lab server, which answers as Rapidgator does for a premium account (a
//! file's page with a download link on a file host, links that answer 404 once they expire), over
//! plain http on the lab's port. The browser hands over the file host's link itself, its Referer
//! the site's address alone; the file's page comes as the tab the download was started from
//! (`Options::page`).

use std::time::{Duration, Instant};

use mdm_engine::{Destination, Download, Error, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content, rapidgator};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, REFERER};
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

/// Rapidgator's hosts at the lab server; the tab the download came from, when given.
fn opts(page: Option<&str>) -> Options {
    Options {
        hosts: rapidgator::HOSTS
            .iter()
            .map(|h| (h.to_string(), vec!["127.0.0.1".parse().unwrap()]))
            .collect(),
        retry_delay: Duration::from_millis(50),
        max_connections: 2,
        page: page.map(|p| p.parse().unwrap()),
        ..Options::default()
    }
}

/// The browser's headers: its Referer, the site's address alone (its referrer policy), and the
/// premium account's session cookie (`premium`), as it sends them with the file host's link.
fn browser_headers(s: &LabServer, premium: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    let referer = format!("http://{}:{}/", rapidgator::SITE, s.addr().port());
    h.insert(REFERER, HeaderValue::from_str(&referer).unwrap());
    let cookie = if premium {
        format!("lang=en; {}", rapidgator::COOKIE)
    } else {
        "lang=en".to_owned()
    };
    h.insert(COOKIE, HeaderValue::from_str(&cookie).unwrap());
    h
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The browser's link to the file `id`, good until `expires` (unix ms).
fn link(s: &LabServer, id: &str, expires: u64) -> Url {
    rapidgator::file_link(s.addr().port(), id, expires)
        .parse()
        .unwrap()
}

/// The browser's link to the file `id`, just handed over: good for `ttl_ms` from the download's
/// first request, however late that comes on a busy computer (`rapidgator::lasting_link`).
fn fresh_link(s: &LabServer, id: &str, ttl_ms: u64) -> Url {
    rapidgator::lasting_link(s.addr().port(), id, ttl_ms)
        .parse()
        .unwrap()
}

fn page(s: &LabServer, id: &str) -> String {
    rapidgator::file_page(s.addr().port(), id)
}

/// The file is the fake Rapidgator's `id`, byte for byte, of `size` bytes.
fn check(path: &std::path::Path, id: &str, size: usize) {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; size];
    content::fill(rapidgator::seed(id), 0, &mut want);
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
    e.host
        .as_deref()
        .is_some_and(|h| h.split(':').next() == Some(host))
}

/// The file's page asked (`/file/…`).
fn page_asks(log: &[LogEntry]) -> Vec<&LogEntry> {
    log.iter()
        .filter(|e| on(e, rapidgator::SITE) && e.path.starts_with("/file/"))
        .collect()
}

/// The user's case: a big file's link, handed over by the browser, expires while the download
/// still asks for parts of it (connections that broke and reconnect, parts split late) and is
/// answered 404 Not Found. The tab's page, the file's, is asked with the browser's cookies (the
/// premium session) for a fresh link, which the download goes on from to the end. Before, it
/// failed with "the server answered 404 Not Found".
#[tokio::test]
async fn a_link_that_expires_mid_download_is_renewed_from_its_page() {
    let s = server("rate=1M&drop_after=384K").await;
    let id = rapidgator::id("3M", 700, None, false);
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: fresh_link(&s, &id, 700),
            headers: browser_headers(&s, true),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(Some(&page(&s, &id))),
    )
    .wait()
    .await
    .unwrap();
    assert_eq!(out.path, dir.path().join(rapidgator::FILE_NAME));
    check(&out.path, &id, 3 << 20);
    let log = log(&s).await;
    assert!(
        log.iter()
            .any(|e| on(e, rapidgator::FILES) && e.status == 404),
        "the link expired during the download"
    );
    let asks = page_asks(&log);
    assert!(!asks.is_empty(), "the file's page asked for a fresh link");
    for e in &asks {
        assert!(
            e.cookie
                .as_deref()
                .is_some_and(|c| c.contains(rapidgator::COOKIE)),
            "the page asked with the session: {:?}",
            e.cookie
        );
        assert_eq!(e.status, 200);
    }
    // Every refresh but the last gave a link that served before it expired in turn: no page
    // asked twice for one expiry (the links last 700 ms; the download takes about 3 s).
    assert!(asks.len() <= 8, "{} page asks", asks.len());
}

/// The page redirects to the link (a premium account's "direct downloads"), and the download is
/// resumed (after a pause, a restart) once its link has expired: its connections meet 404, the page
/// gives a fresh link, and only what was missing is asked of it.
#[tokio::test]
async fn a_download_resumed_after_its_link_expired_goes_on() {
    let s = server("rate=1M").await;
    let id = rapidgator::id("4M", 600_000, None, true);
    let size = 4usize << 20;
    let expires = now_ms() + 1500;
    let faults = s.faults();
    s.set_faults(Faults {
        stall_at: Some(2 << 20),
        ..faults.clone()
    });
    let dir = tempfile::tempdir().unwrap();
    let headers = browser_headers(&s, true);
    let dl = Download::start(
        Request {
            url: link(&s, &id, expires),
            headers: headers.clone(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            save_interval: Duration::from_millis(100),
            ..opts(Some(&page(&s, &id)))
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
    assert!(matches!(dl.wait().await, Err(Error::Cancelled)));
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
    while now_ms() <= expires {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    s.clear_log();

    let out = Download::resume(state.clone(), headers, opts(Some(&page(&s, &id))))
        .unwrap()
        .wait()
        .await
        .unwrap();
    check(&out.path, &id, size);
    assert!(!state.exists(), "state file removed on completion");
    let log = log(&s).await;
    let asks = page_asks(&log);
    assert_eq!(asks.len(), 1, "the page asked once: {asks:?}");
    assert_eq!(asks[0].status, 302, "the page redirected to the link");
    let overlap = opts(None).verify_overlap;
    let asked: Vec<&LogEntry> = log
        .iter()
        .filter(|e| on(e, rapidgator::FILES) && matches!(e.status, 200 | 206))
        .collect();
    assert!(!asked.is_empty(), "the file asked of the fresh link");
    for e in asked {
        let again = on_disk(e.range.as_deref(), &saved);
        assert!(
            again <= overlap,
            "asked again for {again} bytes it had kept: {:?}, {kept} kept",
            e.range
        );
    }
}

/// Without the file's page (the Referer is the site's address alone, no tab known), or with a
/// page that has no link (signed out: no premium session), the link's expiry ends the download
/// as an expired link, which a fresh address from the browser continues; the page is asked at
/// most once.
#[tokio::test]
async fn without_a_page_or_a_session_the_expired_link_says_so() {
    let s = server("rate=1M&drop_after=256K").await;
    let id = rapidgator::id("2M", 300, None, false);
    for (page_known, premium) in [(false, true), (true, false)] {
        s.clear_log();
        let dir = tempfile::tempdir().unwrap();
        let tab = page_known.then(|| page(&s, &id));
        let err = Download::start(
            Request {
                url: fresh_link(&s, &id, 300),
                headers: browser_headers(&s, premium),
                dest: Destination::Dir(dir.path().to_owned()),
            },
            opts(tab.as_deref()),
        )
        .wait()
        .await
        .err()
        .unwrap_or_else(|| panic!("page {page_known}, premium {premium}: finished"));
        assert!(
            matches!(err, Error::LinkExpired),
            "page {page_known}, premium {premium}: {err:?}"
        );
        assert!(err.wants_new_address());
        let log = log(&s).await;
        let asks = page_asks(&log).len();
        if page_known {
            assert_eq!(asks, 1, "a page without a link asked once");
        } else {
            assert_eq!(asks, 0, "no page to ask");
        }
    }
}

/// A page that gives links the file host then refuses (an account's limit, a link bound
/// elsewhere): each passes the refresh's own look (one request) and is refused after. The
/// download doesn't ask the page again and again, connection after connection: once the fresh
/// link is refused before it served a part, it stops as an expired link.
#[tokio::test]
async fn a_fresh_link_refused_at_once_is_not_renewed_again_and_again() {
    let s = server("rate=1M&drop_after=256K").await;
    // Each link answers one request: the browser's, the probe; a fresh one, the refresh's look.
    let id = rapidgator::id("2M", 600_000, Some(1), false);
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let err = Download::start(
        Request {
            url: link(&s, &id, now_ms() + 600_000),
            headers: browser_headers(&s, true),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(Some(&page(&s, &id))),
    )
    .wait()
    .await
    .expect_err("the download fails");
    assert!(matches!(err, Error::LinkExpired), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let log = log(&s).await;
    assert_eq!(page_asks(&log).len(), 1, "the page asked once");
    let refused = log
        .iter()
        .filter(|e| on(e, rapidgator::FILES) && e.status == 404)
        .count();
    assert!(refused <= 6, "{refused} refused requests");
}

/// The user's second case: the expired link's fresh one leads to another of Rapidgator's servers,
/// whose copy of the file has its own ETag and Last-Modified (12 s apart), the same bytes. The
/// parts still missing are asked there with the first server's date, which it doesn't match,
/// and it answers with the whole file. Before, the download failed at 100% with "the file
/// changed on the server"; now that server's validators are taken and it goes on to the end.
#[tokio::test]
async fn a_fresh_link_on_another_server_with_its_own_copy_goes_on() {
    let s = server("rate=1M&drop_after=384K").await;
    let id = rapidgator::mirrored_id("3M", 700, None, false);
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: fresh_link(&s, &id, 700),
            headers: browser_headers(&s, true),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(Some(&page(&s, &id))),
    )
    .wait()
    .await
    .unwrap();
    check(&out.path, &id, 3 << 20);
    let log = log(&s).await;
    assert!(
        log.iter().any(|e| {
            on(e, rapidgator::FILES2)
                && e.status == 200
                && e.range.is_some()
                && e.if_range.as_deref().is_some_and(|v| !v.starts_with('"'))
        }),
        "the other server rejected the original date and answered with its whole copy"
    );
    assert!(
        log.iter().any(|e| {
            on(e, rapidgator::FILES2)
                && e.status == 206
                && e.if_range.as_deref().is_some_and(|v| v.ends_with("-n1\""))
        }),
        "parts came from the other server under its own ETag"
    );
}
