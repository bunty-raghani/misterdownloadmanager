//! yt-dlp's `-J` answer, which carries what a site sent (titles, formats, subtitles): read
//! (`mdm_tools::resolve`), its program-typed extensions retyped, turned into the choices the
//! video window offers and picked from, and each DASH format's pieces listed
//! (`mdm_tools::pieces::dash`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use mdm_tools::{pieces, resolve};

fuzz_target!(|data: &[u8]| {
    let retyped = resolve::typed_scripts(data.to_vec());
    // An answer that isn't JSON is given back as it came.
    if serde_json::from_slice::<serde_json::Value>(data).is_err() {
        assert_eq!(retyped, data);
    }

    if let Some(resolve::Looked::Video(video)) = resolve::parse_looked(data) {
        let _ = resolve::needs_merging(&video);
        let _ = resolve::sub_choices(&video, "de");
        for can_merge in [true, false] {
            let choices = resolve::choices(&video, can_merge);
            for height in [None, Some(0), Some(360), Some(1080), Some(u32::MAX)] {
                if let Some(picked) = resolve::pick(&choices, height) {
                    assert!(picked.index < choices.len());
                }
            }
            for c in &choices {
                let _ = resolve::audio_conversion(&c.selector);
                let _ = resolve::mp3_quality(&c.selector);
                let _ = resolve::without_remuxing(&c.selector);
                let _ = resolve::sound_only(&c.selector, &c.ext);
            }
        }
    }
    let _ = resolve::parse(&retyped);

    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        let formats = v.get("formats").and_then(serde_json::Value::as_array);
        for f in formats.into_iter().flatten().chain([&v]) {
            for p in pieces::dash(f).into_iter().flatten() {
                // Each part is asked as `bytes=start-last`: its last byte must be one a u64 counts.
                if let Some((start, len)) = p.range {
                    assert!(start.checked_add(len.max(1)).is_some(), "{:?}", p.range);
                }
            }
        }
    }
});
