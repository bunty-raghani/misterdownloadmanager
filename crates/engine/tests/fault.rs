//! #816: a failed request says what kind of failure it was (`mdm_engine::fault`), from real
//! connections to servers on this computer: refused, an HTTP status, a secure connection that
//! isn't one, no answer in time, an answer cut short, a name that isn't found (asked of a DNS
//! server here, `support/fake_dns.rs`); and MDM's own requests go through the proxy Options sets.

use std::time::Duration;

use mdm_engine::fault::{Fault, of_http};
use mdm_engine::{Dns, DnsChoice, Options, Proxy, http_client};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[path = "support/fake_dns.rs"]
mod support;
use support::{fake_dns, silent_dns};

/// A server on this computer that answers every connection with `answer` (after reading the
/// request), then closes it; None: it reads and never answers. Its port.
async fn serve(answer: Option<&'static [u8]>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                match answer {
                    Some(a) => {
                        let _ = s.write_all(a).await;
                        let _ = s.shutdown().await;
                    }
                    None => tokio::time::sleep(Duration::from_secs(30)).await,
                }
            });
        }
    });
    port
}

/// A port nothing listens on.
async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

/// Windows answers a connection to a port nothing listens on after about 2 s of trying.
fn opts() -> Options {
    Options {
        connect_timeout: Duration::from_secs(10),
        ..Options::default()
    }
}

/// The failure of a GET of `url` (its body read whole, the status checked), by kind.
async fn fault_of(url: &str, opts: &Options) -> Fault {
    let url: url::Url = url.parse().unwrap();
    let client = http_client(opts, &url).unwrap();
    let res = async {
        client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await;
    of_http(&res.expect_err("the request fails"))
}

#[tokio::test]
async fn refused() {
    let port = closed_port().await;
    let got = fault_of(&format!("http://127.0.0.1:{port}/"), &opts()).await;
    assert_eq!(got, Fault::Refused);
}

#[tokio::test]
async fn rate_limited() {
    let port = serve(Some(
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ))
    .await;
    let got = fault_of(&format!("http://127.0.0.1:{port}/"), &opts()).await;
    assert_eq!(got, Fault::Status(403));
}

/// A server that answers a secure connection in plain HTTP: the handshake fails, as it does
/// against a filter that isn't the server.
#[tokio::test]
async fn not_a_secure_connection() {
    let port = serve(Some(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ))
    .await;
    let got = fault_of(&format!("https://127.0.0.1:{port}/"), &opts()).await;
    assert_eq!(got, Fault::Secure);
}

#[tokio::test]
async fn no_answer_in_time() {
    let port = serve(None).await;
    let opts = Options {
        read_timeout: Duration::from_millis(500),
        ..opts()
    };
    let got = fault_of(&format!("http://127.0.0.1:{port}/"), &opts).await;
    assert_eq!(got, Fault::TimedOut);
}

#[tokio::test]
async fn cut_short() {
    let port = serve(Some(
        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nonly a part",
    ))
    .await;
    let got = fault_of(&format!("http://127.0.0.1:{port}/"), &opts()).await;
    assert_eq!(got, Fault::CutShort);
}

/// Options with the DNS at `dns` (`support/fake_dns.rs`) answering every name: the lookups
/// below never reach the system's DNS or the network.
fn asking(dns: std::net::SocketAddr, opts: Options) -> Options {
    Options {
        dns: DnsChoice {
            dns: Dns::Plain(vec![dns]),
            fallback: false,
            label: "the test DNS".into(),
        },
        ..opts
    }
}

/// A DNS that answers "no such name" (NXDOMAIN). Asked of the system's DNS, a `.invalid` name
/// depended on the runner's resolver: macOS's took longer than the connect timeout to give up
/// now and then, which made a timeout of it. Not `.invalid` here: hickory answers those itself
/// (RFC 6761) without asking the DNS.
#[tokio::test]
async fn no_such_host() {
    let dns = fake_dns().await;
    let got = fault_of("http://no-such-host.test/", &asking(dns.addr, opts())).await;
    assert_eq!(got, Fault::NoSuchHost);
    // The test DNS said so, not the system's.
    let asked = dns.asked.lock().unwrap();
    assert!(asked.iter().any(|n| n == "no-such-host.test"), "{asked:?}");
}

/// A DNS that never answers: the name isn't found, it is not a server that didn't answer (the
/// server was never asked). The connect timeout covers the lookup too; before, it ran out first
/// and called it `TimedOut`.
#[tokio::test]
async fn a_dns_that_never_answers_is_no_such_host() {
    let dns = silent_dns().await;
    let opts = Options {
        connect_timeout: Duration::from_secs(1),
        ..opts()
    };
    let got = fault_of("http://no-such-host.test/", &asking(dns.addr, opts)).await;
    assert_eq!(got, Fault::NoSuchHost);
    assert!(
        !dns.asked.lock().unwrap().is_empty(),
        "the DNS wasn't asked"
    );
}

/// MDM's own requests take Options' proxy: through one that isn't there, the request fails
/// connecting to it, without looking up the server's name (which doesn't exist).
#[tokio::test]
async fn through_the_proxy_set_in_options() {
    let port = closed_port().await;
    let opts = Options {
        proxy: Proxy::Manual {
            url: format!("http://127.0.0.1:{port}"),
            login: None,
            http: true,
            https: true,
        },
        ..opts()
    };
    let got = fault_of("https://mdm-test.invalid/releases", &opts).await;
    assert_eq!(got, Fault::Refused);
}
