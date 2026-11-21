//! LuluStream links against the lab's fake LuluStream (`mdm_labserver::lulustream`): its host
//! names resolve to the lab server, which answers as LuluStream does (a watch page, a download
//! page listing the qualities on a mirror, a quality's form posted back for the file's link, a
//! file server whose links expire), over plain http on the lab's port.

use std::time::Duration;

use mdm_engine::{Destination, Download, Error, Options, Refusal, Request};
use mdm_labserver::lulustream::{self, Form};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content};

async fn server() -> LabServer {
    LabServer::start(Config::default()).await.unwrap()
}

/// LuluStream's hosts at the lab server.
fn opts() -> Options {
    Options {
        hosts: lulustream::HOSTS
            .iter()
            .map(|h| (h.to_string(), vec!["127.0.0.1".parse().unwrap()]))
            .collect(),
        retry_delay: Duration::from_millis(50),
        ..Options::default()
    }
}

/// The address `path` on `host`, at the lab's port.
fn at(s: &LabServer, host: &str, path: &str) -> url::Url {
    format!("http://{host}:{}{path}", s.addr().port())
        .parse()
        .unwrap()
}

/// The download of `url` into a new folder.
async fn download(url: url::Url) -> (tempfile::TempDir, Result<mdm_engine::Outcome, Error>) {
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url,
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await;
    (dir, out)
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

/// A video's watch link and its player's give the video: its download page's best quality (high,
/// on the mirror), its form posted once, the file from the link the answer gives, byte for byte,
/// named by the file server. The watch page itself, and the lesser quality, are never asked.
#[tokio::test]
async fn a_video_link_gives_its_best_quality() {
    let s = server().await;
    let code = lulustream::code(Form::Plain, "2M", 600);
    for path in [format!("/{code}"), format!("/e/{code}")] {
        let (dir, out) = download(at(&s, lulustream::SITE, &path)).await;
        let out = out.unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(out.path, dir.path().join(lulustream::FILE_NAME), "{path}");
        let data = std::fs::read(&out.path).unwrap();
        let mut want = vec![0; 2 << 20];
        content::fill(lulustream::seed(&code), 0, &mut want);
        assert!(data == want, "{path}: content mismatch");
    }
    let log = log(&s).await;
    let posts: Vec<_> = log.iter().filter(|e| e.method == "POST").collect();
    assert_eq!(posts.len(), 2, "one form posted per download: {posts:?}");
    assert!(
        posts
            .iter()
            .all(|e| on(e, lulustream::MIRROR) && e.path == format!("/d/{code}_h")),
        "the high quality's page, on the mirror: {posts:?}"
    );
    assert!(
        !log.iter().any(|e| e.path == format!("/{code}")
            || e.path == format!("/e/{code}")
            || e.path.ends_with("_n")),
        "neither the watch page nor the normal quality asked"
    );
    assert!(
        log.iter()
            .any(|e| on(e, lulustream::FILES) && (e.status == 200 || e.status == 206)),
        "the file from the file server"
    );
}

/// A form that wants a captcha isn't posted: the download says to use the browser.
#[tokio::test]
async fn a_captcha_needs_the_browser() {
    let s = server().await;
    let code = lulustream::code(Form::Captcha, "64K", 600);
    let (dir, out) = download(at(&s, lulustream::SITE, &format!("/{code}"))).await;
    assert!(
        matches!(
            &out,
            Err(Error::Refused(Refusal::NeedsBrowser { site })) if site == "LuluStream"
        ),
        "{out:?}"
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    assert!(!log(&s).await.iter().any(|e| e.method == "POST"));
}

/// A site that answers its form with the form again now and then (the real one's "Security
/// error"): posted again, the third post gives the file.
#[tokio::test]
async fn a_form_not_taken_at_first_is_posted_again() {
    let s = server().await;
    let code = lulustream::code(Form::Flaky, "64K", 600);
    let (dir, out) = download(at(&s, lulustream::SITE, &format!("/{code}"))).await;
    let out = out.unwrap();
    assert_eq!(out.path, dir.path().join(lulustream::FILE_NAME));
    let posts = log(&s)
        .await
        .into_iter()
        .filter(|e| e.method == "POST")
        .count();
    assert_eq!(posts, 3);
}

/// A site that never takes its form: posted a few times, then refused in the site's name (try
/// again in a moment), nothing saved.
#[tokio::test]
async fn a_form_never_taken_is_refused_in_the_sites_name() {
    let s = server().await;
    let code = lulustream::code(Form::Refusing, "64K", 600);
    let (dir, out) = download(at(&s, lulustream::SITE, &format!("/{code}"))).await;
    assert!(
        matches!(
            &out,
            Err(Error::Refused(Refusal::NotTaken { site })) if site == "LuluStream"
        ),
        "{out:?}"
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    let posts = log(&s)
        .await
        .into_iter()
        .filter(|e| e.method == "POST")
        .count();
    assert_eq!(posts, 5);
}

/// The link the form's answer gives expires during the download (it lasts a second here; the
/// real one, a while): the file server refuses it, and the video's link is asked again, its pages
/// and form, for a fresh one, which the download goes on from to the end, byte for byte.
#[tokio::test]
async fn a_link_that_expires_mid_download_is_asked_for_again() {
    let s = LabServer::start(Config {
        faults: Faults::parse("rate=1M&drop_after=384K").unwrap(),
        ..Config::default()
    })
    .await
    .unwrap();
    let code = lulustream::code(Form::Expiring, "3M", 1);
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: at(&s, lulustream::SITE, &format!("/{code}")),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            max_connections: 2,
            ..opts()
        },
    )
    .wait()
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out.path, dir.path().join(lulustream::FILE_NAME));
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; 3 << 20];
    content::fill(lulustream::seed(&code), 0, &mut want);
    assert!(data == want, "content mismatch");
    let log = log(&s).await;
    assert!(
        log.iter()
            .any(|e| on(e, lulustream::FILES) && e.status == 403),
        "the link expired during the download"
    );
    let posts = log.iter().filter(|e| e.method == "POST").count();
    assert!(
        (2..=4).contains(&posts),
        "the form posted again for a fresh link, not over and over: {posts}"
    );
}
