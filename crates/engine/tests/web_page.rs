//! #533: a web page (a firewall's "The requested URL was rejected") where the download expects
//! another kind of file is an error, never saved as the file; real pages and files still are.

use mdm_engine::{Destination, Download, Error, Options, Request};
use mdm_labserver::{Config, LabServer, content};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn server() -> LabServer {
    LabServer::start(Config::default()).await.unwrap()
}

/// Downloads `url` to `dest` (a name in a fresh folder, or the folder itself), with the
/// browser's type `mime`; the file saved (its bytes) or the error, and what the folder holds.
async fn get(
    url: &str,
    name: Option<&str>,
    mime: Option<&str>,
) -> (Result<Vec<u8>, Error>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let dest = match name {
        Some(n) => Destination::File(dir.path().join(n)),
        None => Destination::Dir(dir.path().to_owned()),
    };
    let req = Request {
        url: url.parse().unwrap(),
        headers: Default::default(),
        dest,
    };
    let opts = Options {
        expected_type: mime.map(str::to_owned),
        retry_delay: std::time::Duration::from_millis(50),
        ..Options::default()
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .map(|o| std::fs::read(o.path).unwrap());
    let mut files: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    (out, files)
}

fn refused(out: &Result<Vec<u8>, Error>) -> bool {
    matches!(out, Err(Error::GotWebPage))
}

#[tokio::test]
async fn a_page_saved_as_a_document_is_an_error() {
    let s = server().await;
    // The reporter's case: the browser named it x.doc, the server sent its refusal page.
    let (out, files) = get(&s.url("/gen/245/x.doc?html=1"), Some("x.doc"), None).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
    assert!(
        files.iter().all(|f| !f.starts_with("x.doc")),
        "nothing saved: {files:?}"
    );
}

#[tokio::test]
async fn a_page_where_the_browser_had_another_type_is_an_error() {
    let s = server().await;
    // An address without a file's name (OWA's attachment.ashx), the browser's own type.
    let url = s.url("/gen/245/attachment.ashx?html=1");
    let (out, files) = get(&url, None, Some("application/msword")).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
    assert!(files.is_empty(), "nothing saved: {files:?}");
    // The file's information window says so too, before anything is saved.
    let opts = Options {
        expected_type: Some("application/msword".into()),
        ..Options::default()
    };
    let probed = mdm_engine::probe_kept(url.parse().unwrap(), Default::default(), &opts).await;
    assert!(matches!(probed, Err(Error::GotWebPage)), "{probed:?}");
    // An address named like a file, added by hand (no browser): the same.
    let (out, _) = get(&s.url("/gen/245/report.pdf?html=1"), None, None).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
}

#[tokio::test]
async fn pages_and_files_are_still_saved() {
    let s = server().await;
    // A page that is one: a .html link, a page the browser saved as one, a name saying nothing.
    for (url, name, mime) in [
        ("/gen/245/page.html?html=1", Some("page.html"), None),
        ("/gen/245/x.doc?html=1", Some("x.doc"), Some("text/html")),
        ("/gen/245/view.php?html=1", None, None),
        ("/gen/245/download?html=1", None, None),
    ] {
        let (out, files) = get(&s.url(url), name, mime).await;
        assert!(out.is_ok(), "{url}: {:?}", out.err());
        assert_eq!(files.len(), 1, "{url}: {files:?}");
    }
    // A document sent as application/octet-stream (the lab's type for .doc), and as its own.
    for (url, name, mime) in [
        (
            "/gen/64K/x.doc?seed=5",
            Some("x.doc"),
            Some("application/msword"),
        ),
        ("/gen/64K/x.pdf?seed=5&inline=bare", Some("x.pdf"), None),
    ] {
        let (out, _) = get(&s.url(url), name, mime).await;
        let out = out.unwrap_or_else(|e| panic!("{url}: {e}"));
        let mut want = vec![0; 64 << 10];
        content::fill(5, 0, &mut want);
        assert!(out == want, "{url}");
    }
    // A server's own name for what it sends (Content-Disposition) is taken at its word.
    let named =
        "Content-Type: text/html\r\nContent-Disposition: attachment; filename=\"x.doc\"\r\n";
    let (base, _) = answers(named, b"<html>a document, as its server calls it</html>").await;
    let (out, _) = get(&format!("{base}/x.doc"), Some("x.doc"), None).await;
    assert!(out.is_ok(), "{:?}", out.err());
}

/// A server that answers every request with `body` and the header lines `head` (each ending in
/// CRLF; none: no Content-Type); how many requests it got.
async fn answers(
    head: &'static str,
    body: &'static [u8],
) -> (String, std::sync::Arc<std::sync::atomic::AtomicU32>) {
    use std::sync::atomic::{AtomicU32, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let asked = std::sync::Arc::new(AtomicU32::new(0));
    let count = asked.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), asked)
}

#[tokio::test]
async fn an_untyped_answer_is_looked_into() {
    let page: &[u8] =
        b"\r\n<!DOCTYPE html><html><body>The requested URL was rejected.</body></html>";
    let (base, _) = answers("", page).await;
    let (out, files) = get(&format!("{base}/x.doc"), Some("x.doc"), None).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
    assert!(files.is_empty(), "nothing saved: {files:?}");
    // A file without a type is the file (asked for again once it has been looked into).
    let file: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 a document";
    let (base, asked) = answers("", file).await;
    let (out, _) = get(&format!("{base}/x.doc"), Some("x.doc"), None).await;
    let out = out.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out, file);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
}
