//! HLS playlists from a server: a master playlist read for the browser's quality menu
//! (`mdm_tools::manifest`), with the media playlist its first format names, and a media playlist
//! whose pieces MDM's engine downloads (`mdm_tools::pieces::hls`).
//!
//! The input is the master playlist, a NUL byte, then the media playlist (without one, the same
//! text is both).
#![no_main]

use libfuzzer_sys::fuzz_target;
use mdm_tools::manifest::Master;
use mdm_tools::pieces;
use url::Url;

fuzz_target!(|data: &[u8]| {
    let (master_bytes, media_bytes) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, data),
    };
    let (Ok(master_text), Ok(media_text)) = (
        std::str::from_utf8(master_bytes),
        std::str::from_utf8(media_bytes),
    ) else {
        return;
    };
    let base = Url::parse("https://media.example.com/streams/show/master.m3u8").unwrap();

    if let Some(master) = Master::parse(master_text, &base) {
        let _ = master.first();
        if let Some(video) = master.video(media_text) {
            // Bounded by the master's own limit on formats.
            assert!(
                video.formats.len() <= 200,
                "{} formats",
                video.formats.len()
            );
        }
    }

    for (extra, extra_key) in [(None, None), (Some("token=a&b=c"), Some("k=1"))] {
        if let Ok(list) = pieces::hls(media_text, &base, extra, extra_key) {
            // Every piece comes from a line of the playlist (a URI line, or `#EXT-X-MAP`).
            assert!(!list.is_empty());
            assert!(list.len() <= media_text.lines().count());
            // Each part is asked as `bytes=start-last`: its last byte must be one a u64 counts.
            for p in &list {
                if let Some((start, len)) = p.range {
                    assert!(start.checked_add(len.max(1)).is_some(), "{:?}", p.range);
                }
            }
        }
    }
});
