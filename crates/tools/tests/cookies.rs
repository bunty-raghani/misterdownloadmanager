//! Whose cookies the real yt-dlp sends (#229), run through `mdm_tools` as the app runs it,
//! against the lab server (its log keeps each request's `Cookie` header). Nothing leaves this
//! computer, and no real browser profile is read: Firefox's cookies come from a throwaway
//! profile folder made for the test.
//!
//! The cookies of a page on one host of a site reach yt-dlp's request to another host of it
//! (music.youtube.com's page, www.youtube.com's request): yt-dlp then goes through the lab as its
//! HTTP proxy, which answers for every host name, so those names never reach the network.
//!
//! Ignored: it needs yt-dlp's program, and for the browser part a Firefox profile folder whose
//! `cookies.sqlite` has the cookie `ff=from-firefox` for 127.0.0.1:
//!
//!   MDM_TEST_YTDLP=…\yt-dlp.exe [MDM_TEST_FIREFOX_PROFILE=<folder>] cargo test -p mdm-tools --test cookies -- --ignored --nocapture

use std::time::{Duration, Instant};

use mdm_ipc::BrowserCookie;
use mdm_labserver::{Config, LabServer};
use mdm_tools::proc::{BrowserCookies, FromBrowser, set_browser_cookies};
use mdm_tools::video::{State, Subs, VideoJob};

/// The cookies the lab saw on each request for `path` since `from` (the log's index).
fn cookies_seen(lab: &LabServer, path: &str, from: usize) -> Vec<String> {
    lab.log()
        .into_iter()
        .skip(from)
        .filter(|e| e.path == path)
        .map(|e| e.cookie.unwrap_or_default())
        .collect()
}

/// yt-dlp's look-up (`-J`) of `url` with `browser`'s cookies as the app gives them, through the
/// lab as its HTTP proxy (any host name is the lab's: nothing leaves this computer), its generic
/// extractor forced (YouTube's would take a youtube.com address as a channel's); then the host
/// and cookies of each request for the lab's clip.
fn hosts_and_cookies(
    exe: &std::path::Path,
    lab: &LabServer,
    url: &str,
    browser: &FromBrowser,
) -> Vec<(String, String)> {
    let proxy = lab.url("");
    let extras = ["--proxy".to_owned(), proxy.trim_end_matches('/').to_owned()];
    let mut ours: Vec<String> = ["-J", "--no-playlist", "--force-generic-extractor"]
        .map(String::from)
        .to_vec();
    let (theirs, _file) = browser.args(url).unwrap();
    ours.extend(theirs);
    let at = lab.log().len();
    let cmd = mdm_tools::proc::ytdlp(exe, &extras, &ours, url).unwrap();
    let out = mdm_tools::proc::output(cmd, "yt-dlp", Duration::from_secs(90));
    assert!(out.is_ok(), "{url}: {:?}", out.err());
    lab.log()
        .into_iter()
        .skip(at)
        .filter(|e| e.path == "/gen/1M/clip.mp4")
        .map(|e| {
            let host = e.host.unwrap_or_default();
            let host = host.split(':').next().unwrap_or_default().to_owned();
            (host, e.cookie.unwrap_or_default())
        })
        .collect()
}

/// A download as the app runs it: yt-dlp finds the clip, MDM's engine downloads it (#769) with
/// the cookies yt-dlp would send.
fn download(exe: &std::path::Path, url: &str, browser: &FromBrowser) -> (State, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: Default::default(),
    };
    let job = VideoJob::start(
        exe,
        url,
        "b",
        dir.path(),
        "clip",
        None,
        None,
        None,
        browser,
        &Subs::default(),
        Some(engine),
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(120);
    let p = loop {
        let p = job.progress();
        if p.state != State::Running || Instant::now() > end {
            break p;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    (p.state, p.events.into_iter().map(|(_, e)| e).collect())
}

#[test]
#[ignore]
fn whose_cookies_ytdlp_sends() {
    let exe = std::path::PathBuf::from(std::env::var("MDM_TEST_YTDLP").expect("MDM_TEST_YTDLP"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let path = "/gen/1M/clip.mp4";
    let url = lab.url(path);
    let ext = FromBrowser {
        cookies: Some("ext=from-extension".into()),
        ..Default::default()
    };

    // 1. The extension's cookies: on every request, the download's own look-up included
    //    (it had none before #229, and a page that wants a sign-in failed there).
    set_browser_cookies(None);
    let at = lab.log().len();
    mdm_tools::resolve::look_up(&exe, &url, &ext).unwrap();
    let seen = cookies_seen(&lab, path, at);
    println!("1. look-up, extension's cookies: {seen:?}");
    assert!(!seen.is_empty() && seen.iter().all(|c| c.contains("ext=from-extension")));
    let at = lab.log().len();
    let (state, events) = download(&exe, &url, &ext);
    let seen = cookies_seen(&lab, path, at);
    println!("1. download, extension's cookies: {state:?} {seen:?} {events:?}");
    assert!(matches!(state, State::Done(_)), "{state:?}");
    assert!(seen.len() >= 2, "the look-up and the download: {seen:?}");
    assert!(
        seen.iter().all(|c| c.contains("ext=from-extension")),
        "{seen:?}"
    );

    // 2. No cookies and the option off: none sent.
    let at = lab.log().len();
    mdm_tools::resolve::look_up(&exe, &url, &FromBrowser::default()).unwrap();
    let seen = cookies_seen(&lab, path, at);
    println!("2. look-up, nothing: {seen:?}");
    assert!(seen.iter().all(String::is_empty));

    // 3. A YouTube Music page's cookies, the look-up asking www.youtube.com (the lab's redirect
    //    stands in for yt-dlp going there): a newer extension's, each with its domain: the
    //    site's sign-in (`.youtube.com`) goes there, the page host's own cookie doesn't.
    let port = lab
        .url("")
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .unwrap()
        .to_owned();
    let via = |from: &str, to: &str| {
        format!("http://{from}:{port}/redirect?to=http://{to}:{port}/gen/1M/clip.mp4")
    };
    let music = via("music.youtube.com", "www.youtube.com");
    let jar = FromBrowser {
        cookies: Some("SID=signed-in; pref=music-only".into()),
        jar: vec![
            BrowserCookie {
                name: "SID".into(),
                value: "signed-in".into(),
                domain: ".youtube.com".into(),
                path: "/".into(),
                ..Default::default()
            },
            BrowserCookie {
                name: "pref".into(),
                value: "music-only".into(),
                domain: "music.youtube.com".into(),
                host_only: true,
                path: "/".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let seen = hosts_and_cookies(&exe, &lab, &music, &jar);
    println!("3. music.youtube.com's page, each cookie's domain: {seen:?}");
    assert!(!seen.is_empty(), "the clip asked for");
    for (host, cookie) in &seen {
        assert_eq!(host, "www.youtube.com");
        assert!(cookie.contains("SID=signed-in"), "{cookie:?}");
        assert!(!cookie.contains("pref="), "{cookie:?}");
    }
    //    An older extension's header (no domains): filed for youtube.com, it goes there too.
    let header = FromBrowser {
        cookies: Some("SID=signed-in".into()),
        ..Default::default()
    };
    let seen = hosts_and_cookies(&exe, &lab, &music, &header);
    println!("3. music.youtube.com's page, a header: {seen:?}");
    assert!(!seen.is_empty());
    for (host, cookie) in &seen {
        assert_eq!(host, "www.youtube.com");
        assert!(cookie.contains("SID=signed-in"), "{cookie:?}");
    }
    //    Another site's header stays with the page's host: none for its other host.
    let other = via("music.lab.test", "www.lab.test");
    let seen = hosts_and_cookies(&exe, &lab, &other, &header);
    println!("3. music.lab.test's page, a header: {seen:?}");
    assert!(!seen.is_empty());
    for (host, cookie) in &seen {
        assert_eq!(host, "www.lab.test");
        assert!(!cookie.contains("SID="), "{cookie:?}");
    }

    let Ok(profile) = std::env::var("MDM_TEST_FIREFOX_PROFILE") else {
        println!("MDM_TEST_FIREFOX_PROFILE not set: the browser's part not run");
        return;
    };
    // 4. Options → Cookies for video sites: Firefox's (the throwaway profile) when the extension
    //    sent none: a pasted address, the look-up and the download.
    set_browser_cookies(BrowserCookies::new("firefox", &profile));
    let at = lab.log().len();
    mdm_tools::resolve::look_up(&exe, &url, &FromBrowser::default()).unwrap();
    let seen = cookies_seen(&lab, path, at);
    println!("4. look-up, Firefox's: {seen:?}");
    assert!(!seen.is_empty() && seen.iter().all(|c| c.contains("ff=from-firefox")));
    let at = lab.log().len();
    let (state, events) = download(&exe, &url, &FromBrowser::default());
    let seen = cookies_seen(&lab, path, at);
    println!("4. download, Firefox's: {state:?} {seen:?} {events:?}");
    assert!(matches!(state, State::Done(_)), "{state:?}");
    assert!(
        seen.iter().all(|c| c.contains("ff=from-firefox")),
        "{seen:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.starts_with("Cookies read from firefox on this computer")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.contains(&profile) || e.contains("from-firefox")),
        "the log names the browser only"
    );

    // 5. Both: the extension's alone (that page's, that session's).
    let at = lab.log().len();
    let (state, events) = download(&exe, &url, &ext);
    let seen = cookies_seen(&lab, path, at);
    println!("5. download, both: {state:?} {seen:?} {events:?}");
    assert!(matches!(state, State::Done(_)), "{state:?}");
    assert!(
        seen.iter()
            .all(|c| c.contains("ext=from-extension") && !c.contains("ff=")),
        "{seen:?}"
    );

    // 6. A profile folder with no cookies: yt-dlp's refusal, kept on the error.
    let empty = tempfile::tempdir().unwrap();
    set_browser_cookies(BrowserCookies::new(
        "firefox",
        &empty.path().display().to_string(),
    ));
    let err = mdm_tools::resolve::look_up(&exe, &url, &FromBrowser::default()).unwrap_err();
    println!("6. look-up, empty profile: {err}");
    assert!(
        err.to_string()
            .contains("could not find firefox cookies database"),
        "{err}"
    );
    set_browser_cookies(None);
}
