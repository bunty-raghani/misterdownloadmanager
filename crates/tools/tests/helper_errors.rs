//! #816: a helper's download that fails says which server it couldn't reach and how
//! (`Error::network`), and every request of it, the release list's first, goes through the proxy
//! Options sets (here one that isn't there: nothing leaves this computer).

use mdm_engine::Proxy;
use mdm_engine::fault::Fault;

async fn through_a_missing_proxy() -> mdm_engine::Options {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    mdm_tools::net(Proxy::Manual {
        url: format!("http://127.0.0.1:{port}"),
        login: None,
        http: true,
        https: true,
    })
}

#[tokio::test]
async fn ytdlp_names_the_server_and_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let net = through_a_missing_proxy().await;
    let e = mdm_tools::install_latest(dir.path(), &net, None)
        .await
        .expect_err("no proxy there");
    assert_eq!(
        e.network(),
        Some(("api.github.com".to_owned(), Fault::Refused)),
        "{e}"
    );
}

/// gallery-dl has a build of its own for Windows and Linux on x86-64 only.
#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[tokio::test]
async fn gallery_dl_names_the_server_and_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let net = through_a_missing_proxy().await;
    let e = mdm_tools::gallery::install_latest(dir.path(), &net, None)
        .await
        .expect_err("no proxy there");
    assert_eq!(e.network().map(|n| n.1), Some(Fault::Refused), "{e}");
}

/// ffmpeg is installed by MDM on Windows only (elsewhere the system's is used).
#[cfg(windows)]
#[tokio::test]
async fn ffmpeg_names_the_server_and_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let net = through_a_missing_proxy().await;
    let e = mdm_tools::ffmpeg::install_latest(dir.path(), &net, None)
        .await
        .expect_err("no proxy there");
    assert_eq!(
        e.network(),
        Some(("api.github.com".to_owned(), Fault::Refused)),
        "{e}"
    );
}
