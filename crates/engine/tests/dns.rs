//! End to end: a download whose name is answered by the DNS chosen in Options (#36), not the
//! system's. A small DNS server (`support/fake_dns.rs`: plain, over TCP, on 127.0.0.1) answers
//! `*.mdm.test` with 127.0.0.1 and logs every name it is asked; the system's DNS knows no such
//! names.
//!
//! And downloads held to a network adapter (#35): the DNS and the file both reached through it,
//! nothing sent when it's down, an address of a family it lacks refused (even after a redirect).

use std::net::IpAddr;
use std::sync::Arc;

use mdm_engine::{Destination, Dns, DnsChoice, Download, Options, Request};
use mdm_labserver::{Config, LabServer, content};

#[path = "support/fake_dns.rs"]
mod support;
use support::{FakeDns, fake_dns};

fn options(dns: Dns) -> Options {
    Options {
        dns: DnsChoice {
            dns,
            fallback: false,
            label: "the test DNS".into(),
        },
        retry_delay: std::time::Duration::from_millis(50),
        max_retries: 1,
        ..Options::default()
    }
}

#[tokio::test]
async fn a_download_goes_by_the_chosen_dns() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let FakeDns {
        addr: dns, asked, ..
    } = fake_dns().await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: format!("http://files.mdm.test:{port}/gen/1M/dns.bin?seed=36")
            .parse()
            .unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, options(Dns::Plain(vec![dns])))
        .wait()
        .await
        .expect("the chosen DNS finds files.mdm.test");
    let data = std::fs::read(&out.path).unwrap();
    assert_eq!(data.len(), 1 << 20);
    let mut want = vec![0; data.len()];
    content::fill(36, 0, &mut want);
    assert!(data == want, "content mismatch");
    assert!(
        asked.lock().unwrap().iter().any(|n| n == "files.mdm.test"),
        "{:?}",
        asked.lock().unwrap()
    );
}

#[tokio::test]
async fn a_dead_dns_fails_the_download_and_names_itself() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: format!("http://files.mdm.test:{port}/gen/64K/x.bin")
            .parse()
            .unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    // TEST-NET-1 never answers; strict: no system fallback, so the download fails.
    let err = Download::start(
        req,
        options(Dns::Plain(vec!["192.0.2.1:53".parse().unwrap()])),
    )
    .wait()
    .await
    .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(text.contains("the test DNS"), "{text}");
}

/// Options → Proxy / Socks → Network adapter (#35), on this computer's loopback: 127.0.0.2
/// stands for the adapter's address (Windows and Linux answer on all of 127/8; macOS doesn't,
/// so the bound download runs elsewhere). `loopback_too` binds what is otherwise left unbound.
fn bound(v4: [u8; 4]) -> mdm_engine::Route {
    mdm_engine::Route::Bound(Arc::new(mdm_engine::net::Binding {
        label: "the test adapter".into(),
        name: "lo".into(),
        index: 1,
        v4: Some(v4.into()),
        v6: None,
        loopback_too: true,
    }))
}

fn get(url: String, dir: &std::path::Path) -> Request {
    Request {
        url: url.parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.to_owned()),
    }
}

#[tokio::test]
async fn a_bound_download_asks_and_fetches_through_the_adapter() {
    if cfg!(target_os = "macos") {
        return;
    }
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let FakeDns {
        addr: dns,
        asked,
        peers: dns_peers,
    } = fake_dns().await;
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        route: bound([127, 0, 0, 2]),
        ..options(Dns::Plain(vec![dns]))
    };
    let url = format!("http://files.mdm.test:{port}/gen/1M/bound.bin?seed=35");
    let out = Download::start(get(url, dir.path()), opts)
        .wait()
        .await
        .expect("bound download");
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(35, 0, &mut want);
    assert!(data.len() == 1 << 20 && data == want, "content mismatch");
    assert!(asked.lock().unwrap().iter().any(|n| n == "files.mdm.test"));
    // Windows binds the address: every connection, the DNS's and the file's, came from it.
    // (Linux binds the device, `lo`, and the system picks 127.0.0.1 as the source.)
    if cfg!(windows) {
        let two = IpAddr::from([127, 0, 0, 2]);
        let from: Vec<IpAddr> = lab.log().iter().map(|e| e.peer.ip()).collect();
        assert!(
            !from.is_empty() && from.iter().all(|ip| *ip == two),
            "{from:?}"
        );
        let dns_from = dns_peers.lock().unwrap().clone();
        assert!(
            !dns_from.is_empty() && dns_from.iter().all(|ip| *ip == two),
            "{dns_from:?}"
        );
    }
}

#[tokio::test]
async fn a_down_adapter_sends_nothing() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let FakeDns {
        addr: dns, asked, ..
    } = fake_dns().await;
    let dir = tempfile::tempdir().unwrap();
    for url in [
        lab.url("/gen/64K/down.bin"),
        format!("http://files.mdm.test:{port}/gen/64K/down.bin"),
    ] {
        let opts = Options {
            route: mdm_engine::Route::Down {
                label: "WireGuard".into(),
            },
            ..options(Dns::Plain(vec![dns]))
        };
        let err = Download::start(get(url, dir.path()), opts)
            .wait()
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("WireGuard is not connected"),
            "{err}"
        );
    }
    assert!(lab.log().is_empty(), "{:?}", lab.log());
    assert!(asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_address_of_the_family_the_adapter_lacks_is_refused() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let opts = || Options {
        route: bound([127, 0, 0, 2]),
        ..options(Dns::System)
    };
    // An IPv6 address, the adapter IPv4 only: refused before any connection.
    let v6 = format!("http://[::1]:{port}/gen/64K/v6.bin");
    let err = Download::start(get(v6.clone(), dir.path()), opts())
        .wait()
        .await
        .unwrap_err();
    let refused = "can't be reached through the test adapter";
    assert!(err.to_string().contains(refused), "{err}");
    // A redirect to one: refused too (hyper would bind [::] for it and take the normal route).
    let hop = lab.url(&format!(
        "/redirect?to={}",
        percent_encoding::utf8_percent_encode(&v6, percent_encoding::NON_ALPHANUMERIC)
    ));
    let err = Download::start(get(hop, dir.path()), opts())
        .wait()
        .await
        .unwrap_err();
    assert!(format!("{err} {err:?}").contains(refused), "{err:?}");
    assert!(lab.log().is_empty(), "{:?}", lab.log());
}
