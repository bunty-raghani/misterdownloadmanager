//! A page's own declared video (#1076), against the lab server's `/declared` page: it plays only
//! a hidden welcome video in its HTML (`/gen/1M/welcome.mp4`) and declares another as its own
//! (`/gen/2M/film.mp4`, "The Film") in its JSON-LD, its `og:video` or its `twitter:player`.
//!
//! The page's reading (MDM's engine, as the look-up reads it) runs with `cargo test`. The look-up
//! and the download through the real yt-dlp are ignored: they need yt-dlp's program.
//!
//!   MDM_TEST_YTDLP=…\yt-dlp.exe cargo test -p mdm-tools --test declared -- --ignored --nocapture

use std::time::{Duration, Instant};

use mdm_labserver::{Config, LabServer};
use mdm_tools::declared::{self, Declared};
use mdm_tools::proc::FromBrowser;
use mdm_tools::resolve::Looked;
use mdm_tools::video::{State, Subs, VideoJob};

/// What MDM's engine reads `page` declaring, as [`declared::look_up`] reads it.
fn read(rt: &tokio::runtime::Runtime, page: &str) -> Option<Declared> {
    let url = reqwest::Url::parse(page).unwrap();
    let opts = mdm_engine::Options::default();
    let (at, videos) = rt
        .block_on(mdm_engine::page_videos(url, Default::default(), &opts))
        .unwrap();
    declared::chosen(&videos, &at)
}

#[test]
fn the_lab_page_declares_its_film() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let film = lab.url("/gen/2M/film.mp4?seed=3&inline=1");
    for how in ["jsonld", "og", "twitter"] {
        let page = lab.url(&format!("/declared?how={how}&title=The+Film+%7C+Lab"));
        let d = read(&rt, &page).unwrap_or_else(|| panic!("{how}: nothing declared"));
        assert_eq!(d.address, film, "{how}");
        assert_eq!(d.page, page, "{how}");
        assert_eq!(d.name.as_deref(), Some("The Film"), "{how}");
    }
    // Nothing declared: the hidden video is no declaration.
    assert_eq!(read(&rt, &lab.url("/declared?how=none")), None);
    // A file isn't a page: its body isn't read.
    assert_eq!(read(&rt, &lab.url("/gen/1M/clip.mp4?inline=1")), None);
}

/// The look-up as the app makes it: yt-dlp, then (it read the page generically) the page's
/// declared video.
fn look_up(
    exe: &std::path::Path,
    rt: &tokio::runtime::Runtime,
    page: &str,
) -> (Looked, Option<Declared>) {
    let reader = mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: Default::default(),
    };
    let (looked, _, declared) =
        declared::look_up(exe, page, &FromBrowser::default(), &reader).unwrap();
    (looked, declared)
}

/// The title and the formats' addresses of a video look-up.
fn video(looked: &Looked) -> (String, Vec<String>) {
    match looked {
        Looked::Video(v) => (
            v.title.clone(),
            v.formats.iter().map(|f| f.id.clone()).collect(),
        ),
        Looked::List(l) => panic!("a list: {}", l.title),
    }
}

#[test]
#[ignore]
fn the_declared_video_is_looked_up_and_downloaded() {
    let exe = std::path::PathBuf::from(std::env::var("MDM_TEST_YTDLP").expect("MDM_TEST_YTDLP"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    // The film is served only with the page as its Referer, as an embed-only video is.
    let prefix = lab.url("/declared");
    let film = format!(
        "/gen/2M/film.mp4?seed=3&inline=1&require_referer={}",
        urlencode(&prefix)
    );
    let page = lab.url(&format!("/declared?film={}", urlencode(&film)));

    // Before: yt-dlp alone takes the hidden welcome video.
    let alone = mdm_tools::resolve::look_up(&exe, &page, &FromBrowser::default()).unwrap();
    println!("yt-dlp alone: {:?}", video(&alone));

    let at = lab.log().len();
    let (looked, declared) = look_up(&exe, &rt, &page);
    let (title, formats) = video(&looked);
    println!("with the declaration: {title} {formats:?} {declared:?}");
    let found = declared.expect("the page's declared video");
    assert_eq!(title, "The Film");
    assert_eq!(found.address, lab.url(&film));
    let asked: Vec<_> = lab.log().into_iter().skip(at).collect();
    assert!(
        asked
            .iter()
            .any(|e| e.path == "/gen/2M/film.mp4" && e.referer.as_deref() == Some(page.as_str())),
        "the film asked with the page as its Referer: {:?}",
        asked
            .iter()
            .map(|e| (&e.path, &e.referer, e.status))
            .collect::<Vec<_>>()
    );

    // No declaration: yt-dlp's own find stands.
    let (looked, declared) = look_up(&exe, &rt, &lab.url("/declared?how=none"));
    assert_eq!(declared, None);
    println!("nothing declared: {:?}", video(&looked));

    // The download, as the app starts it: the declared address, the page as its Referer.
    let dir = tempfile::tempdir().unwrap();
    let engine = mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: Default::default(),
    };
    let job = VideoJob::start(
        &exe,
        &found.address,
        "b",
        dir.path(),
        "The Film",
        None,
        None,
        None,
        &found.browser(&FromBrowser::default()),
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
    println!("download: {:?} {:?}", p.state, p.events);
    let State::Done(file) = p.state else {
        panic!("{:?}", p.state);
    };
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 2 << 20);
    assert_eq!(file.file_name().unwrap().to_string_lossy(), "The Film.mp4");
}

/// `s` %-encoded as a query's value.
fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}
