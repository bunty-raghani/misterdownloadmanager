//! A release's detached OpenPGP signature, downloaded beside its checksum list before MDM
//! installs yt-dlp, gallery-dl or its own update (`mdm_tools::signed_by*`, a parser of MDM's own
//! for the packets): checked against the pinned keys, and with a key from the input too.
//!
//! The input: one byte for the key (0 MDM's, 1 yt-dlp's, 2 gallery-dl's, else one from the
//! input), then the parts separated by [`SEP`]: `signature SEP data`, or `key SEP signature SEP
//! data` for a key from the input.
#![no_main]

use libfuzzer_sys::fuzz_target;

const SEP: &[u8] = b"\n--mdm-fuzz--\n";

fn split(data: &[u8]) -> (&[u8], &[u8]) {
    match data.windows(SEP.len()).position(|w| w == SEP) {
        Some(i) => (&data[..i], &data[i + SEP.len()..]),
        None => (data, &[]),
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    match which {
        0..=2 => {
            let (sig, signed) = split(rest);
            let _ = match which {
                0 => mdm_tools::signed_by_mdm(signed, sig),
                1 => mdm_tools::signed_by_ytdlp(signed, sig),
                _ => mdm_tools::gallery::signed_by_gallery_dl(signed, sig),
            };
        }
        _ => {
            let (key, rest) = split(rest);
            let (sig, signed) = split(rest);
            let key = String::from_utf8_lossy(key);
            let _ = mdm_tools::signed_by(&key, signed, sig);
        }
    }
});
