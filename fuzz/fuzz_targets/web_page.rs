//! A web page MDM reads: its links for Link grabber (`mdm_engine::html::links`) and the videos
//! it declares (`mdm_engine::html::declared_videos`, JSON-LD, Open Graph, Twitter's card), and
//! the one MDM would follow (`mdm_tools::declared::chosen`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use mdm_engine::html;
use url::Url;

fuzz_target!(|data: &[u8]| {
    let page = String::from_utf8_lossy(data);
    let at = Url::parse("https://www.example.com/watch/clip.html?id=7").unwrap();

    for link in html::links(&page, &at) {
        // Downloadable, from a server it names (or a magnet link), without its fragment.
        if link.scheme() != "magnet" {
            assert!(mdm_engine::supported(&link), "{link}");
            assert!(link.host_str().is_some_and(|h| !h.is_empty()), "{link}");
        }
        assert!(link.fragment().is_none(), "{link}");
    }

    let videos = html::declared_videos(&page, &at);
    for v in videos
        .json_ld
        .iter()
        .chain(&videos.open_graph)
        .chain(&videos.twitter)
    {
        assert!(matches!(v.address.scheme(), "http" | "https"));
    }
    let _ = videos.main();
    if let Some(d) = mdm_tools::declared::chosen(&videos, &at) {
        // Given to yt-dlp as an argument: one line, of a reasonable length.
        assert!(!d.address.contains(['\r', '\n', '\0']));
        assert!(d.address.len() <= 4096);
    }
});
