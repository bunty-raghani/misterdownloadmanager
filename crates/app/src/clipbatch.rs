//! Tasks → Add batch download from clipboard (#709): the copied text read when the command is
//! chosen (only then: the clipboard belongs to the person at the computer), and what it holds
//! decides the window. Every download address in it (http, https, ftp, magnet; one per line or
//! in a forum post, Markdown or HTML) goes to Link grabber, each once, grouped and ticked; one
//! address with a number in its file name (`…/ep01.mp4`), or with a `*` already, goes to Batch
//! download with that part as its pattern.

use crate::transfer::Imported;

/// What the clipboard held, and so what opens.
#[derive(Debug, PartialEq)]
pub enum Clip {
    /// No text (empty, or a picture, files).
    Empty,
    /// Text without a download address in it.
    NoLinks,
    /// One address with a numbered part: Batch download, filled in.
    Pattern(Pattern),
    /// Links to tick in Link grabber.
    Links(Vec<Imported>),
}

/// Batch download's address with its `*`, and the range it starts with.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub address: String,
    pub from: u64,
    pub to: u64,
    /// The wildcard size: the number's digits when it has leading zeros (`01`), else 1.
    pub width: usize,
}

/// What the clipboard's text (`None`: no text) holds.
pub fn read(text: Option<&str>) -> Clip {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Clip::Empty;
    };
    let mut found = crate::transfer::import(text);
    match found.len() {
        0 => Clip::NoLinks,
        1 => {
            let only = &found[0];
            // A pattern copied as it is (`…/img*.jpg`), else one made of its number.
            let typed = only.url.contains('*').then(|| Pattern {
                address: only.url.clone(),
                from: 0,
                to: 10,
                width: 1,
            });
            match typed.or_else(|| numbered(&only.url)) {
                Some(p) => Clip::Pattern(p),
                None => Clip::Links(std::mem::take(&mut found)),
            }
        }
        _ => Clip::Links(found),
    }
}

/// The longest number taken for a numbered part: longer ones are ids or dates, not a count.
const MOST_DIGITS: usize = 6;

/// An http(s) or ftp address with a number in its file name: the last one, before the
/// extension (`ep01.mp4`, `part3.rar`; a numbered extension, `movie.7z.001`, is the number);
/// never inside a `%20`. Its pattern runs from that number to that number: To is the one the
/// user changes (Batch download puts the keys there).
pub fn numbered(address: &str) -> Option<Pattern> {
    let url = url::Url::parse(address).ok()?;
    if !mdm_engine::supported(&url) {
        return None;
    }
    let path = url.path();
    let name_at = path.rfind('/').map_or(0, |i| i + 1);
    let name = &path[name_at..];
    let stem = match name.rfind('.') {
        Some(dot)
            if dot > 0
                && !name[dot + 1..].is_empty()
                && !name[dot + 1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            &name[..dot]
        }
        _ => name,
    };
    let bytes = stem.as_bytes();
    let (mut i, mut last) = (0, None);
    while i < bytes.len() {
        if bytes[i] == b'%'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            i += 3;
        } else if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            last = Some((start, i));
        } else {
            i += 1;
        }
    }
    let (a, b) = last?;
    let digits = &stem[a..b];
    if digits.len() > MOST_DIGITS {
        return None;
    }
    let from: u64 = digits.parse().ok()?;
    let width = if digits.len() > 1 && digits.starts_with('0') {
        digits.len()
    } else {
        1
    };
    let s = url.as_str();
    let at = url[..url::Position::BeforePath].len() + name_at + a;
    Some(Pattern {
        address: format!("{}*{}", &s[..at], &s[at + digits.len()..]),
        from,
        to: from,
        width,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(clip: &Clip) -> Vec<&str> {
        match clip {
            Clip::Links(l) => l.iter().map(|d| d.url.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    fn pattern(address: &str, from: u64, width: usize) -> Clip {
        Clip::Pattern(Pattern {
            address: address.into(),
            from,
            to: from,
            width,
        })
    }

    /// Nothing, or nothing to download: a note, not an empty window.
    #[test]
    fn an_empty_clipboard_or_one_without_links() {
        assert_eq!(read(None), Clip::Empty);
        assert_eq!(read(Some("  \r\n\t")), Clip::Empty);
        assert_eq!(read(Some("just some words, no address")), Clip::NoLinks);
        assert_eq!(read(Some("mailto:a@b.org file:///C:/x.zip")), Clip::NoLinks);
    }

    /// Fifteen links in a block of text (a forum post): fifteen downloads, in order, each once.
    #[test]
    fn fifteen_links_in_a_block_of_text() {
        let mut post = String::from("Here are the files, enjoy!\n\n");
        for k in 1..=15 {
            post.push_str(&format!(
                "Episode {k}: https://media.example.org/show/ep{k:02}.mp4\n"
            ));
        }
        post.push_str("Again the first: https://media.example.org/show/ep01.mp4.\nThanks.");
        let clip = read(Some(&post));
        let got = urls(&clip);
        assert_eq!(got.len(), 15);
        assert_eq!(got[0], "https://media.example.org/show/ep01.mp4");
        assert_eq!(got[14], "https://media.example.org/show/ep15.mp4");
    }

    /// Links and a magnet link: all to Link grabber; one link without a number too.
    #[test]
    fn links_and_magnets_go_to_link_grabber() {
        let clip = read(Some(
            "<https://a.example.org/setup.exe>\nmagnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Show",
        ));
        assert_eq!(
            urls(&clip),
            [
                "https://a.example.org/setup.exe",
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Show"
            ]
        );
        assert_eq!(
            urls(&read(Some("https://a.example.org/setup.exe"))),
            ["https://a.example.org/setup.exe"]
        );
        let one_magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
        assert_eq!(urls(&read(Some(one_magnet))), [one_magnet]);
    }

    /// One address with a numbered part: Batch download with that number as the pattern.
    #[test]
    fn a_numbered_address_is_a_pattern() {
        assert_eq!(
            read(Some(" https://media.example.org/show/ep01.mp4\n")),
            pattern("https://media.example.org/show/ep*.mp4", 1, 2)
        );
        assert_eq!(
            read(Some("Get it: http://x.example/img_12.jpg.")),
            pattern("http://x.example/img_*.jpg", 12, 1)
        );
        // The last number of the file name; the folder's and the extension's are kept.
        assert_eq!(
            read(Some("https://x.example/s2/video_720p_part3.mkv?t=10")),
            pattern("https://x.example/s2/video_720p_part*.mkv?t=10", 3, 1)
        );
        // A numbered extension is the number (a split archive's volumes).
        assert_eq!(
            read(Some("https://x.example/movie.7z.001")),
            pattern("https://x.example/movie.7z.*", 1, 3)
        );
        // A space written %20 is not a number.
        assert_eq!(
            read(Some("https://x.example/My%20Show%2005.mp4")),
            pattern("https://x.example/My%20Show%20*.mp4", 5, 2)
        );
        // A pattern copied as it is.
        assert_eq!(
            read(Some("http://x.example/img*.jpg")),
            Clip::Pattern(Pattern {
                address: "http://x.example/img*.jpg".into(),
                from: 0,
                to: 10,
                width: 1
            })
        );
        // No number, a number too long to be a count, a magnet: one row in Link grabber.
        for one in [
            "https://x.example/setup.exe",
            "https://x.example/mp4/video.mp4",
            "https://x.example/IMG_20241231.jpg",
            "https://x.example/%20.zip",
        ] {
            assert_eq!(urls(&read(Some(one))), [one], "{one}");
        }
        assert_eq!(numbered("magnet:?xt=urn:btih:1234"), None);
    }

    /// The pattern made of a numbered address gives that address back first.
    #[test]
    fn the_pattern_starts_with_the_copied_address() {
        for one in [
            "https://media.example.org/show/ep01.mp4",
            "https://x.example/My%20Show%2005.mp4",
            "https://x.example/movie.7z.001",
            "ftp://x.example/pub/disk9.iso",
        ] {
            let p = numbered(one).unwrap();
            let first = p
                .address
                .replacen('*', &format!("{:0w$}", p.from, w = p.width), 1);
            assert_eq!(first, one);
        }
    }
}
