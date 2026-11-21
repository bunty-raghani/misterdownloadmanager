//! A .torrent file (from a page, a folder MDM watches, or the remote control) and a magnet link:
//! its file list and names checked (`mdm_torrent::files_from_bytes`), its magnet link, and its
//! trackers read, filtered and added to (`mdm_torrent::trackers`), which rewrite the file around
//! its info dictionary.
#![no_main]

use std::path::Component;

use libfuzzer_sys::fuzz_target;
use mdm_torrent::librqbit;
use mdm_torrent::trackers;

fn info_hash(bytes: &[u8]) -> Option<String> {
    librqbit::torrent_from_bytes(bytes)
        .ok()
        .map(|m| m.info_hash.as_string())
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(source) = mdm_torrent::Source::magnet(&text) {
        let mdm_torrent::Source::Magnet(link) = source else {
            unreachable!()
        };
        let kept = trackers::in_magnet(&link);
        for proxied in [false, true] {
            let filtered = trackers::filter_magnet(&link, proxied);
            // Filtering only takes trackers away.
            for t in trackers::in_magnet(&filtered) {
                assert!(kept.contains(&t), "{t} came from nowhere");
            }
        }
    }

    if let Ok(files) = mdm_torrent::files_from_bytes(data) {
        // Every name a torrent may write was checked: each path is relative, of plain names
        // (padding files are never written).
        for f in files.iter().filter(|f| !f.padding) {
            assert!(f.path.components().count() > 0, "{:?}", f.path);
            for c in f.path.components() {
                let Component::Normal(part) = c else {
                    panic!("{:?} is not a plain relative path", f.path);
                };
                let part = part.to_string_lossy();
                assert!(!part.contains(['/', '\\', '\0']), "{:?}", f.path);
            }
        }
    }
    let _ = mdm_torrent::magnet_link(data);

    let hash = info_hash(data);
    let listed = trackers::listed(data);
    for proxied in [false, true] {
        if let Some(out) = trackers::filter_torrent(data, proxied) {
            // The same torrent: its info dictionary is kept byte for byte.
            if hash.is_some() {
                assert_eq!(info_hash(&out), hash, "filtering changed the info hash");
            }
            for t in trackers::listed(&out) {
                assert!(listed.contains(&t), "{t} came from nowhere");
            }
        }
    }
    let more = [
        "udp://tracker.example.org:1337/announce".to_owned(),
        "https://t.example.net/announce".to_owned(),
    ];
    if let Some(out) = trackers::add(data, &more)
        && hash.is_some()
    {
        assert_eq!(
            info_hash(&out),
            hash,
            "adding trackers changed the info hash"
        );
    }
});
