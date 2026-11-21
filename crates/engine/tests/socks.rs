//! Downloads through a SOCKS proxy (#1558): HTTP and FTP, SOCKS5 with and without a login,
//! SOCKS4, a proxy script's SOCKS answer; the site's name looked up by the proxy (socks5h,
//! socks4a: this computer never asks for it) or here (socks5, socks4: the proxy gets the
//! address). The lab's SOCKS proxy records what it was asked for.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use mdm_engine::{Destination, Download, Options, Proxy, Request};
use mdm_labserver::ftp::{FtpConfig, FtpServer};
use mdm_labserver::socks::{SocksConfig, SocksServer};
use mdm_labserver::{Config, LabServer, content};

/// The name the sites go by: only the proxy knows it (or `Options::hosts`, for the tests where
/// this computer looks it up).
const NAME: &str = "lab.test";

fn localhost() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}

async fn proxy(login: Option<(&str, &str)>) -> SocksServer {
    SocksServer::start(
        "127.0.0.1:0".parse().unwrap(),
        SocksConfig {
            login: login.map(|(u, p)| (u.into(), p.into())),
            names: vec![(NAME.into(), localhost())],
            ..SocksConfig::default()
        },
    )
    .await
    .unwrap()
}

async fn lab() -> LabServer {
    LabServer::start(Config::default()).await.unwrap()
}

fn manual(url: String, login: Option<(&str, &str)>) -> Proxy {
    Proxy::Manual {
        url,
        login: login.map(|(u, p)| (u.into(), p.into())),
        // A SOCKS proxy is ticked for both, as Options sets it.
        http: true,
        https: true,
    }
}

fn opts(proxy: Proxy) -> Options {
    Options {
        proxy,
        max_connections: 2,
        retry_delay: Duration::from_millis(50),
        max_retries: 1,
        ..Options::default()
    }
}

/// `url` downloaded with `opts`, checked against the generator's `seed`: Ok(size), or the
/// error's words.
async fn fetch(url: String, seed: u64, opts: Options) -> Result<u64, String> {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: url.parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .map_err(|e| e.to_string())?;
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(seed, 0, &mut want);
    assert!(data == want, "content mismatch");
    Ok(out.size)
}

fn port_of(s: &LabServer) -> u16 {
    s.addr().port()
}

/// socks5h: the proxy gets the site's name, which nothing on this computer could look up.
#[tokio::test]
async fn socks5h_hands_the_name_to_the_proxy() {
    let (lab, socks) = (lab().await, proxy(None).await);
    let url = format!("http://{NAME}:{}/gen/512K/a.bin?seed=5", port_of(&lab));
    let size = fetch(
        url,
        5,
        opts(manual(format!("socks5h://{}", socks.addr()), None)),
    )
    .await
    .unwrap();
    assert_eq!(size, 512 << 10);
    let seen = socks.seen();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|r| r.by_name && r.target == NAME && r.ok),
        "{seen:?}"
    );
}

/// socks5: the name is looked up here (Options::hosts stands for this computer's DNS) and the
/// proxy gets the address.
#[tokio::test]
async fn socks5_looks_the_name_up_here() {
    let (lab, socks) = (lab().await, proxy(None).await);
    let url = format!("http://{NAME}:{}/gen/512K/a.bin?seed=6", port_of(&lab));
    let o = Options {
        hosts: vec![(NAME.into(), vec![localhost()])],
        ..opts(manual(format!("socks5://{}", socks.addr()), None))
    };
    fetch(url, 6, o).await.unwrap();
    let seen = socks.seen();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|r| !r.by_name && r.target == "127.0.0.1"),
        "{seen:?}"
    );
}

/// A SOCKS5 login: the right one gets through, a wrong one is refused (and nothing goes around
/// the proxy).
#[tokio::test]
async fn socks5_with_a_login() {
    let (lab, socks) = (lab().await, proxy(Some(("ann", "s3cret"))).await);
    let url = format!("http://{NAME}:{}/gen/256K/a.bin?seed=7", port_of(&lab));
    let at = format!("socks5h://{}", socks.addr());
    fetch(
        url.clone(),
        7,
        opts(manual(at.clone(), Some(("ann", "s3cret")))),
    )
    .await
    .unwrap();
    assert!(
        socks
            .seen()
            .iter()
            .all(|r| r.ok && r.user.as_deref() == Some("ann"))
    );
    let wrong = fetch(url, 7, opts(manual(at, Some(("ann", "nope"))))).await;
    assert!(wrong.is_err(), "a wrong login got through");
    assert!(socks.seen().iter().any(|r| !r.ok));
}

/// SOCKS4a hands the name over, SOCKS4 the address (each through a proxy of its own: a
/// connection the first download opened late can't count for the second).
#[tokio::test]
async fn socks4_and_socks4a() {
    let lab = lab().await;
    let (socks4a, socks4) = (proxy(None).await, proxy(None).await);
    let url = format!("http://{NAME}:{}/gen/256K/a.bin?seed=8", port_of(&lab));
    fetch(
        url.clone(),
        8,
        opts(manual(format!("socks4a://{}", socks4a.addr()), None)),
    )
    .await
    .unwrap();
    let seen = socks4a.seen();
    assert!(
        !seen.is_empty() && seen.iter().all(|r| r.version == 4 && r.by_name),
        "{seen:?}"
    );
    let o = Options {
        hosts: vec![(NAME.into(), vec![localhost()])],
        ..opts(manual(format!("socks4://{}", socks4.addr()), None))
    };
    fetch(url, 8, o).await.unwrap();
    let seen = socks4.seen();
    assert!(
        !seen.is_empty()
            && seen
                .iter()
                .all(|r| r.version == 4 && !r.by_name && r.target == "127.0.0.1"),
        "{seen:?}"
    );
}

/// A proxy script's `SOCKS5` answer: through the proxy, the name handed over as browsers do.
#[tokio::test]
async fn a_proxy_scripts_socks_answer() {
    let (lab, socks) = (lab().await, proxy(None).await);
    let script = format!(
        "function FindProxyForURL(url, host) {{ return 'SOCKS5 {}'; }}",
        socks.addr()
    );
    let url = format!("http://{NAME}:{}/gen/256K/a.bin?seed=9", port_of(&lab));
    fetch(
        url,
        9,
        opts(Proxy::Pac {
            script: Arc::from(script.as_str()),
        }),
    )
    .await
    .unwrap();
    assert!(socks.seen().iter().all(|r| r.by_name && r.target == NAME));
}

/// FTP through SOCKS5: the control connection and every data connection go through the proxy,
/// to the server's name; passive even when active mode is set (the proxy can't take the
/// server's connections back).
#[tokio::test]
async fn ftp_goes_through_the_proxy() {
    let ftp = FtpServer::start("127.0.0.1:0".parse().unwrap(), FtpConfig::default())
        .await
        .unwrap();
    let socks = proxy(Some(("ann", "s3cret"))).await;
    let url = format!("ftp://{NAME}:{}/gen/2M/s4/a.bin", ftp.addr().port());
    let o = Options {
        max_connections: 2,
        ftp_passive: false,
        ..opts(manual(
            format!("socks5h://{}", socks.addr()),
            Some(("ann", "s3cret")),
        ))
    };
    assert_eq!(fetch(url, 4, o).await.unwrap(), 2 << 20);
    let seen = socks.seen();
    let control = seen.iter().filter(|r| r.port == ftp.addr().port()).count();
    let data = seen.len() - control;
    assert!(control >= 1 && data >= 1, "{seen:?}");
    assert!(seen.iter().all(|r| r.by_name && r.target == NAME && r.ok));
}

/// An HTTP proxy carries no FTP: the download goes straight to the server, as before.
#[tokio::test]
async fn ftp_skips_an_http_proxy() {
    let ftp = FtpServer::start("127.0.0.1:0".parse().unwrap(), FtpConfig::default())
        .await
        .unwrap();
    let url = ftp.url("/gen/256K/s3/a.bin");
    // Nothing listens there: had FTP gone to it, the download would fail.
    let o = opts(manual("http://127.0.0.1:9".into(), None));
    fetch(url, 3, o).await.unwrap();
}
