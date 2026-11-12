//! End to end: downloads through a service account (#44), with a fake unlocker standing for a
//! debrid service. The host's link (`hoster.test`) resolves nowhere: a download that completes
//! came through the unlocker's direct link (the lab server).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use mdm_engine::{
    Destination, Download, Error, Options, Refusal, Request, ServiceFault, Unlocked, Unlocker,
};
use mdm_labserver::{Config, LabServer, content};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use tokio_util::sync::CancellationToken;
use url::Url;

/// What the fake service does with a link.
#[derive(Clone)]
enum Answer {
    /// The lab's `/gen` path for it; `ttl_ms`: the direct link expires that long after each unlock.
    Direct {
        path: String,
        ttl_ms: Option<u64>,
        chunks: Option<u32>,
    },
    /// Not a host the service supports.
    Declined,
    Refused(ServiceFault),
}

#[derive(Debug)]
struct Fake {
    lab: String,
    answer: Mutex<Option<Answer>>,
    calls: AtomicU32,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Answer")
    }
}

impl Unlocker for Fake {
    fn name(&self) -> &str {
        "Fake-Debrid"
    }

    fn unlock<'a>(
        &'a self,
        _url: &'a Url,
        note: &'a (dyn Fn(String, Option<f32>) + Send + Sync),
        _cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Unlocked>, ServiceFault>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            note("Fake-Debrid is preparing the link".into(), None);
            match self.answer.lock().unwrap().clone().expect("an answer") {
                Answer::Declined => Ok(None),
                Answer::Refused(f) => Err(f),
                Answer::Direct {
                    path,
                    ttl_ms,
                    chunks,
                } => {
                    let mut url = format!("{}{path}", self.lab);
                    if let Some(ttl) = ttl_ms {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                        let sep = if url.contains('?') { '&' } else { '?' };
                        url = format!("{url}{sep}expires={}", now.as_millis() as u64 + ttl);
                    }
                    Ok(Some(Unlocked {
                        url: url.parse().unwrap(),
                        file_name: Some("from-the-service.bin".into()),
                        size: None,
                        max_connections: chunks,
                        headers: Default::default(),
                    }))
                }
            }
        })
    }
}

fn fake(lab: &LabServer, answer: Answer) -> Arc<Fake> {
    Arc::new(Fake {
        lab: lab.url("").trim_end_matches('/').to_owned(),
        answer: Mutex::new(Some(answer)),
        calls: AtomicU32::new(0),
    })
}

fn options(unlocker: Arc<Fake>) -> Options {
    Options {
        unlocker: Some(unlocker),
        retry_delay: Duration::from_millis(50),
        max_retries: 2,
        ..Options::default()
    }
}

fn request(dir: &std::path::Path, cookie: bool) -> Request {
    let mut headers = HeaderMap::new();
    if cookie {
        headers.insert(COOKIE, HeaderValue::from_static("host_session=secret"));
    }
    Request {
        url: "http://hoster.test/file/abc/movie.bin".parse().unwrap(),
        headers,
        dest: Destination::Dir(dir.to_owned()),
    }
}

fn same(data: &[u8], seed: u64) -> bool {
    let mut want = vec![0; data.len()];
    content::fill(seed, 0, &mut want);
    data == want
}

#[tokio::test]
async fn a_host_link_comes_through_the_service_capped_and_without_the_hosts_cookies() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let u = fake(
        &lab,
        Answer::Direct {
            path: "/gen/4M/d?seed=44".into(),
            ttl_ms: None,
            chunks: Some(2),
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        max_connections: 8,
        ..options(u.clone())
    };
    let dl = Download::start(request(dir.path(), true), opts);
    let out = dl.wait().await.expect("through the service");
    // The server's own name for it (Content-Disposition) comes first.
    assert_eq!(out.path.file_name().unwrap(), "d");
    let data = std::fs::read(&out.path).unwrap();
    assert!(data.len() == 4 << 20 && same(&data, 44), "content");
    assert_eq!(u.calls.load(Ordering::SeqCst), 1);
    let log = lab.log();
    // At most 2 requests at the same moment (a connection that ends may be replaced).
    let most = log
        .iter()
        .map(|a| {
            log.iter()
                .filter(|b| b.t_start_ms <= a.t_start_ms && a.t_start_ms < b.t_end_ms)
                .count()
        })
        .max()
        .unwrap_or(0);
    assert!(
        most <= 2,
        "the service's cap of 2 connections: {most} at once"
    );
    assert!(
        log.iter().all(|e| e.cookie.is_none()),
        "the host's cookie went to the service's link"
    );
}

#[tokio::test]
async fn an_expired_direct_link_is_unlocked_again_and_the_download_completes() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    // Connections dropped every 512 KB at 512 KB/s reconnect after the 1.5 s link has died.
    let u = fake(
        &lab,
        Answer::Direct {
            path: "/gen/2M/e?seed=45&rate=512K&drop_after=512K".into(),
            ttl_ms: Some(1500),
            chunks: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        max_retries: 4,
        max_connections: 2,
        ..options(u.clone())
    };
    let dl = Download::start(request(dir.path(), false), opts);
    // The service's note, each time it is asked: the first link, and the refreshed one too
    // (the event log keeps the last 50 events, so they are gathered as they come).
    let mut notes = std::collections::BTreeSet::new();
    loop {
        let s = dl.snapshot();
        notes.extend(
            s.events
                .iter()
                .filter(|e| e.text == "Fake-Debrid is preparing the link")
                .map(|e| e.seq),
        );
        if matches!(
            s.state,
            mdm_engine::download::State::Done | mdm_engine::download::State::Failed
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let out = dl
        .wait()
        .await
        .expect("unlocked again when the link expired");
    let data = std::fs::read(&out.path).unwrap();
    assert!(same(&data, 45), "content after the link changed");
    assert!(notes.len() >= 2, "the refresh is noted too: {notes:?}");
    assert!(
        u.calls.load(Ordering::SeqCst) >= 2,
        "{}",
        u.calls.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn a_host_the_service_declines_is_downloaded_directly() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<Url>().unwrap().port().unwrap();
    let u = fake(&lab, Answer::Declined);
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        hosts: vec![("hoster.test".into(), vec!["127.0.0.1".parse().unwrap()])],
        ..options(u.clone())
    };
    let req = Request {
        url: format!("http://hoster.test:{port}/gen/1M/plain.bin?seed=46")
            .parse()
            .unwrap(),
        ..request(dir.path(), false)
    };
    let out = Download::start(req, opts).wait().await.unwrap();
    assert!(same(&std::fs::read(&out.path).unwrap(), 46));
    assert_eq!(u.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_service_refusal_stops_the_download_before_any_file() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let u = fake(&lab, Answer::Refused(ServiceFault::FileUnavailable));
    let dir = tempfile::tempdir().unwrap();
    let err = Download::start(request(dir.path(), false), options(u))
        .wait()
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::Refused(Refusal::Service {
                fault: ServiceFault::FileUnavailable,
                ..
            })
        ),
        "{err:?}"
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "no file made"
    );
    assert!(lab.log().is_empty());
}

#[tokio::test]
async fn the_look_names_and_sizes_the_file_through_the_service() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let u = fake(
        &lab,
        Answer::Direct {
            path: "/gen/2M/d?seed=47".into(),
            ttl_ms: None,
            chunks: None,
        },
    );
    let info = mdm_engine::probe(
        "http://hoster.test/file/xyz/name.bin".parse().unwrap(),
        HeaderMap::new(),
        &options(u),
    )
    .await
    .unwrap();
    assert_eq!(info.size, Some(2 << 20));
    assert_eq!(info.file_name, "d");
}

#[tokio::test]
async fn a_download_begun_directly_continues_through_the_service_once_set_to() {
    // File Properties' "Through …" ticked on a download that went directly until now (#44):
    // resumed, the service is asked at once, and the file continues from its link.
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<Url>().unwrap().port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hosts = vec![("hoster.test".to_owned(), vec!["127.0.0.1".parse().unwrap()])];
    let req = Request {
        url: format!("http://hoster.test:{port}/gen/2M/d.bin?seed=47&rate=512K")
            .parse()
            .unwrap(),
        ..request(dir.path(), false)
    };
    let direct = Options {
        hosts: hosts.clone(),
        max_connections: 1,
        ..Options::default()
    };
    let dl = Download::start(req, direct);
    let file = loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let s = dl.snapshot();
        if s.downloaded > 0
            && let Some(f) = s.file
        {
            break f;
        }
    };
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(Error::Cancelled)));
    let u = fake(
        &lab,
        Answer::Direct {
            path: "/gen/2M/d.bin?seed=47".into(),
            ttl_ms: None,
            chunks: None,
        },
    );
    let opts = Options {
        hosts,
        ..options(u.clone())
    };
    let state = mdm_engine::state::state_path(&file);
    let out = Download::resume(state, HeaderMap::new(), opts)
        .unwrap()
        .wait()
        .await
        .expect("continued through the service");
    assert!(same(&std::fs::read(&out.path).unwrap(), 47));
    assert_eq!(
        u.calls.load(Ordering::SeqCst),
        1,
        "the service asked at once"
    );
}
