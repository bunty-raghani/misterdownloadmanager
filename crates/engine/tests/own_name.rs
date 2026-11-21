//! #1422: a site whose bot check refuses whatever claims to be a browser without connecting like
//! one (SourceForge's), and lets programs that say what they are through. When the browser's
//! User-Agent is refused, and the browser-like connection too (#668), MDM asks once more under
//! its own name (`Options::own_agent`), with the same cookies, Referer and range; the site is
//! then remembered for the session, and a resume goes on under it. The lab server's
//! `refuse_browsers` fault stands in for the check: it refuses every request whose User-Agent
//! has `Mozilla/`, on any connection.

use std::time::{Duration, Instant};

use mdm_engine::agent::OWN_USER_AGENT;
use mdm_engine::browserlike::AVAILABLE;
use mdm_engine::download::State;
use mdm_engine::{Destination, Download, Error, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content};
use reqwest::header::{COOKIE, HeaderMap, REFERER, USER_AGENT};

const SETTLE: Duration = Duration::from_secs(20);
const FIREFOX: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:160.0) Gecko/20100101 Firefox/160.0";
const REFUSED: &str = "fault:refuse_browsers";

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

/// A download the browser handed over, as the app starts it: the browser's User-Agent, and MDM's
/// own name for the last try.
fn opts() -> Options {
    Options {
        retry_delay: Duration::from_millis(50),
        user_agent: FIREFOX.to_owned(),
        own_agent: Some(OWN_USER_AGENT.to_owned()),
        ..Options::default()
    }
}

/// The browser's hand-over: its session cookie and the page as Referer.
fn handed_over() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(COOKIE, "sid=1".parse().unwrap());
    h.insert(REFERER, "http://127.0.0.1/files/".parse().unwrap());
    h
}

/// The site's check, behind it a file that wants the browser's session and page.
const SITE: &str =
    "refuse_browsers=1&require_cookie=sid=1&require_referer=http%3A%2F%2F127.0.0.1%2Ffiles%2F";

fn same_content(path: &std::path::Path, seed: u64, len: usize) {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; len];
    content::fill(seed, 0, &mut want);
    assert!(data == want, "content mismatch");
}

async fn settled_log(s: &LabServer) -> Vec<LogEntry> {
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    s.log()
}

/// A download to its end, with its event log's lines.
async fn to_the_end(dl: Download) -> (Result<mdm_engine::Outcome, Error>, Vec<String>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let events = loop {
        let snap = dl.snapshot();
        if matches!(snap.state, State::Done | State::Failed | State::Cancelled) {
            break snap.events.iter().map(|e| e.text.clone()).collect();
        }
        assert!(Instant::now() < deadline, "never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    (dl.wait().await, events)
}

/// The refusals the ladder meets before MDM's own name: MDM's connection with the browser's
/// User-Agent, then the browser-like connection (when this build has it).
fn refusals_first() -> usize {
    1 + usize::from(AVAILABLE)
}

fn start(s: &LabServer, path: &str, dir: &std::path::Path, opts: Options) -> Download {
    Download::start(
        Request {
            url: s.url(path).parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.to_owned()),
        },
        opts,
    )
}

/// Refused → the browser-like connection → MDM's own name → the file, with the browser's cookie
/// and Referer on every request, on every connection.
#[tokio::test]
async fn refused_as_a_browser_it_gets_through_under_its_own_name() {
    let s = server(SITE).await;
    let dir = tempfile::tempdir().unwrap();
    let (out, events) = to_the_end(start(
        &s,
        "/gen/3M/SHA256SUMS.txt?seed=1422",
        dir.path(),
        opts(),
    ))
    .await;
    let out = out.expect("MDM's own name gets through");
    same_content(&out.path, 1422, 3 << 20);
    let log = settled_log(&s).await;
    let first = refusals_first();
    assert!(log.len() > first, "{log:?}");
    for (i, e) in log[..first].iter().enumerate() {
        assert_eq!(e.outcome, REFUSED, "{log:?}");
        assert_eq!(e.user_agent.as_deref(), Some(FIREFOX), "{e:?}");
        // The browser-like connection's comes second.
        assert_eq!(e.sec_fetch_mode.is_some(), i == 1, "{e:?}");
    }
    for e in &log[first..] {
        assert_eq!(e.user_agent.as_deref(), Some(OWN_USER_AGENT), "{e:?}");
        assert_eq!(e.cookie.as_deref(), Some("sid=1"), "{e:?}");
        assert_eq!(
            e.referer.as_deref(),
            Some("http://127.0.0.1/files/"),
            "{e:?}"
        );
        assert!(e.sec_fetch_mode.is_none(), "MDM's own connection: {e:?}");
        assert!(e.status == 200 || e.status == 206, "{e:?}");
    }
    assert_eq!(
        log.iter().filter(|e| e.outcome == REFUSED).count(),
        first,
        "asked once more, not again and again: {log:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("asking once more as Mister Download Manager itself")),
        "{events:?}"
    );
}

/// Every identity refused: one request each, then the bot check's error, the one users know
/// (Alt+click: the browser downloads it itself).
#[tokio::test]
async fn refused_under_every_name_it_tells_the_bot_check() {
    // Programs refused for not being browsers, browsers for their name: no way in.
    let s = server("browser_only=1&refuse_browsers=1").await;
    let dir = tempfile::tempdir().unwrap();
    let (out, events) = to_the_end(start(&s, "/gen/64K/no.bin?seed=1", dir.path(), opts())).await;
    let err = out.unwrap_err();
    assert!(matches!(err, Error::BotCheck), "{err:?}");
    let log = settled_log(&s).await;
    assert_eq!(log.len(), refusals_first() + 1, "{log:?}");
    let own = log.last().unwrap();
    assert_eq!(own.user_agent.as_deref(), Some(OWN_USER_AGENT), "{log:?}");
    assert_eq!(own.outcome, "fault:browser_only", "{log:?}");
    assert!(
        events
            .iter()
            .any(|e| e.contains("asked as Mister Download Manager")),
        "{events:?}"
    );
}

/// Once its site let MDM through under its own name, the next download there asks so at once:
/// no refusal again.
#[tokio::test]
async fn the_site_is_remembered_for_the_next_download() {
    let s = server(SITE).await;
    let dir = tempfile::tempdir().unwrap();
    let (out, _) = to_the_end(start(&s, "/gen/256K/a.zip?seed=2", dir.path(), opts())).await;
    out.unwrap();
    settled_log(&s).await;
    s.clear_log();
    let (out, events) = to_the_end(start(&s, "/gen/256K/b.zip?seed=3", dir.path(), opts())).await;
    same_content(&out.unwrap().path, 3, 256 << 10);
    let log = settled_log(&s).await;
    assert!(!log.is_empty());
    assert!(
        log.iter()
            .all(|e| e.user_agent.as_deref() == Some(OWN_USER_AGENT) && e.outcome != REFUSED),
        "{log:?}"
    );
    assert!(
        events.iter().any(|e| e.contains("earlier; asking so")),
        "{events:?}"
    );
    // A look there too.
    s.clear_log();
    let info = mdm_engine::probe(
        s.url("/gen/1M/c.zip?seed=4").parse().unwrap(),
        handed_over(),
        &opts(),
    )
    .await
    .unwrap();
    assert_eq!(info.size, Some(1 << 20));
    let log = settled_log(&s).await;
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].user_agent.as_deref(), Some(OWN_USER_AGENT));
}

/// A stopped download goes on under the name that got through, from its state: even once the
/// session's memory of the site is gone (a restart of MDM).
#[tokio::test]
async fn a_resume_goes_on_under_its_own_name() {
    let s = server(&format!("{SITE}&rate=1M&stall_at=1M")).await;
    let dir = tempfile::tempdir().unwrap();
    let url: url::Url = s.url("/gen/3M/r.bin?seed=7").parse().unwrap();
    let dl = start(
        &s,
        "/gen/3M/r.bin?seed=7",
        dir.path(),
        Options {
            save_interval: Duration::from_millis(100),
            ..opts()
        },
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded >= 256 << 10) {
            break f;
        }
        assert!(Instant::now() < deadline, "the download never started");
    };
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(Error::Cancelled)));
    s.set_faults(Faults::parse(&format!("{SITE}&rate=4M")).unwrap());
    tokio::time::timeout(SETTLE, s.settled()).await.unwrap();
    let state = mdm_engine::state::state_path(&file);
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert!(saved.asked_as_own, "the state says how it got through");
    assert!(!saved.browser_like);
    // As after a restart: the session's memory is gone, the state still knows.
    mdm_engine::agent::forget_own(&url);
    s.clear_log();
    let (out, events) = to_the_end(Download::resume(state, handed_over(), opts()).unwrap()).await;
    same_content(&out.expect("the resume goes on").path, 7, 3 << 20);
    let log = settled_log(&s).await;
    assert!(!log.is_empty());
    for e in &log {
        assert_eq!(e.user_agent.as_deref(), Some(OWN_USER_AGENT), "{e:?}");
        assert_eq!(e.cookie.as_deref(), Some("sid=1"), "{e:?}");
        assert!(e.status == 200 || e.status == 206, "{e:?}");
    }
    assert!(events.iter().any(|e| e.contains("own name")), "{events:?}");
}

/// A look that got through under MDM's own name hands its answer to the download, which goes on
/// under it: no request refused again.
#[tokio::test]
async fn a_look_and_its_download_go_on_under_its_own_name() {
    let s = server(SITE).await;
    let url: url::Url = s.url("/gen/1M/kept.zip?seed=5").parse().unwrap();
    let (info, kept) = mdm_engine::probe_kept(url.clone(), handed_over(), &opts())
        .await
        .expect("the look gets through");
    assert_eq!(info.size, Some(1 << 20));
    assert!(kept.is_some(), "its answer is kept");
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start_with(
        Request {
            url,
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
        kept,
    )
    .wait()
    .await
    .unwrap();
    same_content(&out.path, 5, 1 << 20);
    let log = settled_log(&s).await;
    let first = refusals_first();
    assert!(log[..first].iter().all(|e| e.outcome == REFUSED), "{log:?}");
    assert!(
        log[first..]
            .iter()
            .all(|e| e.user_agent.as_deref() == Some(OWN_USER_AGENT) && e.outcome != REFUSED),
        "{log:?}"
    );
}

/// A User-Agent the caller put in the request (the command line's `--user-agent`) is its
/// choice: never replaced by MDM's own name. Nor is one a download has without `own_agent`
/// (the user's in Options).
#[tokio::test]
async fn a_user_agent_chosen_is_never_replaced() {
    let s = server("refuse_browsers=1").await;
    let dir = tempfile::tempdir().unwrap();
    let mut headers = handed_over();
    headers.insert(USER_AGENT, FIREFOX.parse().unwrap());
    let chosen = Download::start(
        Request {
            url: s.url("/gen/64K/chosen.bin?seed=1").parse().unwrap(),
            headers,
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    );
    assert!(matches!(chosen.wait().await, Err(Error::BotCheck)));
    let none = start(
        &s,
        "/gen/64K/none.bin?seed=1",
        dir.path(),
        Options {
            own_agent: None,
            ..opts()
        },
    );
    assert!(matches!(none.wait().await, Err(Error::BotCheck)));
    let log = settled_log(&s).await;
    assert_eq!(log.len(), 2 * refusals_first(), "{log:?}");
    assert!(
        log.iter()
            .all(|e| e.user_agent.as_deref() != Some(OWN_USER_AGENT)),
        "{log:?}"
    );
}
