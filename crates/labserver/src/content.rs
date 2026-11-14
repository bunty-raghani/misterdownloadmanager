//! Deterministic file content: byte `i` of a file with seed `s` is a pure function of `(s, i)`,
//! so any range can be generated without storage and verified by any client.

/// Fill `buf` with the content of the file `seed` starting at byte `offset`.
pub fn fill(seed: u64, offset: u64, buf: &mut [u8]) {
    let mut pos = offset;
    let mut i = 0;
    while i < buf.len() {
        let word = splitmix64(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ (pos / 8)).to_le_bytes();
        let start = (pos % 8) as usize;
        let n = (8 - start).min(buf.len() - i);
        buf[i..i + n].copy_from_slice(&word[start..start + n]);
        i += n;
        pos += n as u64;
    }
}

/// The first bytes of a media file of `kind` (`mp4`, `webm`, `mp3`), which a client that looks
/// at a file's bytes for its type reads (#800): a generated file with `magic=` starts with them.
pub fn media_head(kind: &str) -> Option<&'static [u8]> {
    Some(match kind {
        "mp4" => &b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41"[..],
        "webm" => &b"\x1A\x45\xDF\xA3\x9F\x42\x86\x81\x01\x42\xF7\x81\x01\x42\x82\x84webm"[..],
        "mp3" => &b"ID3\x04\0\0\0\0\0\0"[..],
        _ => return None,
    })
}

/// [`fill`], with `head` over the file's first bytes.
pub fn fill_with_head(seed: u64, offset: u64, buf: &mut [u8], head: &[u8]) {
    fill(seed, offset, buf);
    let Some(rest) = usize::try_from(offset).ok().and_then(|o| head.get(o..)) else {
        return;
    };
    for (b, h) in buf.iter_mut().zip(rest) {
        *b = *h;
    }
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Parse a size like `12345`, `64K`, `100M`, `1G` (binary multiples, case-insensitive).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_offset_matches_whole_file() {
        let mut whole = vec![0u8; 1000];
        fill(7, 0, &mut whole);
        for off in [0usize, 1, 7, 8, 9, 333, 999] {
            let mut part = vec![0u8; 1000 - off];
            fill(7, off as u64, &mut part);
            assert_eq!(part, whole[off..]);
        }
    }

    #[test]
    fn a_media_head_covers_the_first_bytes_only() {
        let head = media_head("mp4").unwrap();
        let mut whole = vec![0u8; 100];
        fill_with_head(3, 0, &mut whole, head);
        assert_eq!(&whole[..head.len()], head);
        let mut plain = vec![0u8; 100];
        fill(3, 0, &mut plain);
        assert_eq!(whole[head.len()..], plain[head.len()..]);
        // From any offset: the same bytes.
        for off in [1usize, 7, 31, 32, 60] {
            let mut part = vec![0u8; 100 - off];
            fill_with_head(3, off as u64, &mut part, head);
            assert_eq!(part, whole[off..]);
        }
        assert!(media_head("exe").is_none());
    }

    #[test]
    fn seeds_differ() {
        let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
        fill(1, 0, &mut a);
        fill(2, 0, &mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("12345"), Some(12345));
        assert_eq!(parse_size("64k"), Some(65536));
        assert_eq!(parse_size("100M"), Some(100 << 20));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size(""), None);
    }
}
