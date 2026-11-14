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

/// Downloads `url` as [`get`] does, into a folder (named by MDM), with the browser's announced
/// size `size`.
async fn get_sized(url: &str, size: Option<u64>) -> (Result<Vec<u8>, Error>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: url.parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        expected_size: size,
        retry_delay: std::time::Duration::from_millis(50),
        ..Options::default()
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .map(|o| std::fs::read(o.path).unwrap());
    let files = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    (out, files)
}

/// #701, #692: a file host's waiting page (DropMeFiles', dailyuploads') where an archive was
/// expected is never saved under its name, nor called finished, whatever type it comes as.
#[tokio::test]
async fn a_waiting_page_is_never_the_archive() {
    let s = server().await;
    for faults in [
        // As text/html (#533's case).
        "html=1",
        // As the file's own type, its blank lines first: told by its bytes.
        "html=1&html_type=application/octet-stream",
        "html=1&html_type=application/zip",
        // Under the archive's own name (Content-Disposition), as text/html or as a file.
        "html=1&html_cd=1",
        "html=1&html_cd=1&html_type=application/octet-stream",
    ] {
        let url = s.url(&format!("/gen/37K/KtK.zip?{faults}"));
        // Named by the browser, and named by MDM (from the address or the server).
        for name in [Some("KtK.zip"), None] {
            let (out, files) = get(&url, name, None).await;
            assert!(
                refused(&out),
                "{faults} {name:?}: {:?}",
                out.map(|b| b.len())
            );
            assert!(
                files.is_empty(),
                "{faults} {name:?}: nothing saved: {files:?}"
            );
        }
        // The file's information window says so, before anything is saved.
        let probed = mdm_engine::probe_kept(
            url.parse().unwrap(),
            Default::default(),
            &Options::default(),
        )
        .await;
        assert!(
            matches!(probed, Err(Error::GotWebPage)),
            "{faults}: {probed:?}"
        );
    }
    // A link that names nothing, the browser's own type for it (a host's /dl/<code>).
    let url = s.url("/gen/37K/dl?html=1&html_type=application/octet-stream");
    let (out, files) = get(&url, None, Some("application/x-zip-compressed")).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
    assert!(files.is_empty(), "nothing saved: {files:?}");
}

/// #701: the size the browser announced (320 MB) says a file is expected where the link names
/// nothing; without it, such a page is a page, saved as one.
#[tokio::test]
async fn the_browsers_size_says_a_file_is_expected() {
    let s = server().await;
    let url = s.url("/gen/37K/AbC123?html=1");
    let (out, files) = get_sized(&url, Some(320 << 20)).await;
    assert!(refused(&out), "{:?}", out.map(|b| b.len()));
    assert!(files.is_empty(), "nothing saved: {files:?}");
    let (out, files) = get_sized(&url, None).await;
    assert!(out.unwrap().starts_with(b"<!doctype html>"));
    assert_eq!(files.len(), 1, "{files:?}");
}

/// Real files still come whole, among them pages under the old Office names programs export.
#[tokio::test]
async fn archives_and_exported_pages_still_download() {
    let s = server().await;
    let (out, _) = get(&s.url("/gen/600K/KtK.zip?seed=7"), None, None).await;
    let mut want = vec![0; 600 << 10];
    content::fill(7, 0, &mut want);
    assert!(out.unwrap() == want);
    // A report "exported to Excel": a page, as the browser's own type for it says.
    let (out, files) = get(
        &s.url("/gen/1K/report.xls?html=1&html_type=application/vnd.ms-excel&html_cd=1"),
        Some("report.xls"),
        Some("application/vnd.ms-excel"),
    )
    .await;
    assert!(out.is_ok(), "{:?}", out.err());
    assert_eq!(files, ["report.xls"]);
}

/// A download of `path` stopped after a MiB, on one connection (one request: `html_after=1`
/// then answers every later one with the page); its state file.
async fn paused_on_one(s: &LabServer, path: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url(path).parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        max_connections: 1,
        save_interval: std::time::Duration::from_millis(100),
        retry_delay: std::time::Duration::from_millis(50),
        ..Options::default()
    };
    let dl = Download::start(req, opts);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded >= 1 << 20) {
            break f;
        }
        assert!(std::time::Instant::now() < deadline, "never started");
    };
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(Error::Cancelled)));
    (dir, mdm_engine::state::state_path(&file))
}

/// #701: a link that leads to the site's page once the download is resumed (`html_after`):
/// the resume stops with the reason, and the data on disk stays as it was (nothing written over
/// it, nothing added), for a later resume.
#[tokio::test]
async fn a_resume_that_meets_a_page_keeps_the_data() {
    let s = server().await;
    for (seed, faults) in [
        (31, "html_after=1"),
        (32, "html_after=1&html_type=application/octet-stream"),
        (33, "html_after=1&html_cd=1&html_type=application/zip"),
    ] {
        let path = format!("/gen/8M/KtK.zip?seed={seed}&rate=1M&{faults}");
        let (_dir, state) = paused_on_one(&s, &path).await;
        let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
        let part = mdm_engine::naming::part_path(&saved.dest);
        let before = std::fs::read(&part).unwrap();
        let err = Download::resume(state.clone(), Default::default(), Options::default())
            .unwrap()
            .wait()
            .await
            .unwrap_err();
        assert!(matches!(err, Error::GotWebPage), "{faults}: {err}");
        assert!(state.exists(), "{faults}: the state is kept");
        assert!(
            std::fs::read(&part).unwrap() == before,
            "{faults}: the part file is as it was"
        );
        assert!(!saved.dest.exists(), "{faults}: never finished");
    }
}
