//! A page's own declared video (#1076). yt-dlp's generic reader, for a site it has no reader of
//! its own for, takes the player written in the page's HTML; a page whose script builds its
//! player has none there but the ones it hides (a film site's welcome video, in a help box). The
//! page says which video it is about itself, for search engines and link previews: its JSON-LD
//! `VideoObject`, its `og:video`, its `twitter:player` ([`mdm_engine::DeclaredVideos`]). When
//! yt-dlp read a page generically (or found nothing there), MDM reads that declaration and, when
//! it names one video clearly, looks that one up instead, with the page as its Referer, and
//! names it as the page names it; that video's error is told rather than another video offered.
//! A site yt-dlp has a reader for is never read here.

use std::net::IpAddr;

use reqwest::Url;

use crate::proc::{self, FromBrowser, LookedUp, ProcError};
use crate::resolve::{self, Looked};

/// How long reading the page for its declared video may take.
const PAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The video a page declares, as MDM looks it up and downloads it: what a download's item keeps
/// as its address (with the page as its Referer), so it is the one looked up again when the
/// download starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    /// The address yt-dlp is given ([`lookable`]).
    pub address: String,
    /// The page that declared it: its Referer.
    pub page: String,
    /// The page's name for it, without the site's name ([`without_site_name`]).
    pub name: Option<String>,
}

impl Declared {
    /// The browser's context `from` (the page's) as yt-dlp asks for the declared video: the page
    /// as its Referer, the same User-Agent and cookie jar (each cookie for its own site), but
    /// not the page's own Cookie header nor a stream's Origin, which are the page's site's.
    pub fn browser(&self, from: &FromBrowser) -> FromBrowser {
        FromBrowser {
            private: from.private,
            referer: Some(self.page.clone()),
            origin: None,
            user_agent: from.user_agent.clone(),
            cookies: None,
            jar: from.jar.clone(),
        }
    }
}

/// Did yt-dlp's look-up `first` of a page read it with its generic reader: a video it found
/// there (yt-dlp says "Falling back on generic information extractor"; the video may be an
/// embedded player's, read by that site's reader after), or nothing (an unsupported address)?
/// A site yt-dlp has a reader for (YouTube, Vimeo's own pages…) says neither; nor does a list
/// of videos, which stays a list.
pub fn read_generically(first: &Result<(Looked, LookedUp), ProcError>) -> bool {
    match first {
        Ok((Looked::Video(_), out)) => {
            out.warnings.iter().any(|w| {
                let w = w.to_ascii_lowercase();
                w.contains("[generic]") && w.contains("generic information extractor")
            }) || serde_json::from_slice::<serde_json::Value>(&out.json).is_ok_and(|v| {
                v.get("extractor_key")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|k| k.eq_ignore_ascii_case("generic"))
            })
        }
        Ok((Looked::List(_), _)) => false,
        Err(e) => e.to_string().contains("Unsupported URL"),
    }
}

/// The video `videos` (what the page at `page` declares) names clearly ([`DeclaredVideos::main`]),
/// as MDM may follow it: an http(s) address, not the page itself, and on this computer or its
/// network only when the page is too (a page on the web can't send MDM to a local address).
///
/// [`DeclaredVideos::main`]: mdm_engine::DeclaredVideos::main
pub fn chosen(videos: &mdm_engine::DeclaredVideos, page: &Url) -> Option<Declared> {
    let v = videos.main()?;
    let address = lookable(&v.address);
    let mut plain = page.clone();
    plain.set_fragment(None);
    if !matches!(address.scheme(), "http" | "https")
        || address == plain
        || (local(&address) && !local(page))
        || address.as_str().len() > 4096
        || proc::safe(address.as_str()).is_err()
    {
        return None;
    }
    Some(Declared {
        address: address.into(),
        page: page.as_str().to_owned(),
        name: v
            .name
            .as_deref()
            .map(|n| without_site_name(n, page))
            .map(|n| n.chars().take(300).collect::<String>())
            .filter(|n| !n.trim().is_empty()),
    })
}

/// Look the page `page` up as [`resolve::look_up_kept`] does, with the browser's context
/// `browser`; when yt-dlp read it generically ([`read_generically`]), the page itself is read
/// (`reader`: Options' proxy, DNS and network adapter, as MDM's own requests) and the video it
/// declares ([`chosen`]) is looked up in its place ([`settle`]). The page declaring nothing
/// clearly, its own look-up stands. A known video site's address (`pages::is_video_page`) is
/// never read.
pub fn look_up(
    exe: &std::path::Path,
    page: &str,
    browser: &FromBrowser,
    reader: &crate::streams::Engine,
) -> Result<(Looked, LookedUp, Option<Declared>), ProcError> {
    let first = resolve::look_up_kept(exe, page, browser);
    let Some(url) = Url::parse(page)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .filter(|u| !crate::pages::is_video_page(u))
    else {
        return first.map(|(l, o)| (l, o, None));
    };
    if !read_generically(&first) {
        return first.map(|(l, o)| (l, o, None));
    }
    let Some(declared) = read(&url, browser, reader) else {
        return first.map(|(l, o)| (l, o, None));
    };
    tracing::info!(page = %page, video = %declared.address, "the page declares its video");
    let theirs = resolve::look_up_kept(exe, &declared.address, &declared.browser(browser));
    if let Err(e) = &theirs {
        tracing::info!(video = %declared.address, "the page's declared video: {e}");
    }
    settle(first, declared, theirs)
}

/// What a page's look-up answers once its declared video was looked up too (`theirs`): that
/// video, named as the page names it; the page's own look-up (`first`) when yt-dlp found no
/// video it knows at the declared address (an unsupported address: a site's own player page it
/// can't read) or a list there; otherwise the declared video's error. The page said which video
/// it is about: another one yt-dlp found in its HTML (a hidden welcome video) never comes in its
/// place.
fn settle(
    first: Result<(Looked, LookedUp), ProcError>,
    declared: Declared,
    theirs: Result<(Looked, LookedUp), ProcError>,
) -> Result<(Looked, LookedUp, Option<Declared>), ProcError> {
    match theirs {
        Ok((Looked::Video(mut v), out)) => {
            if let Some(name) = &declared.name {
                v.title = name.clone();
                v.page_titled = true;
            }
            Ok((Looked::Video(v), out, Some(declared)))
        }
        Ok((Looked::List(_), _)) => first.map(|(l, o)| (l, o, None)),
        Err(e) if e.to_string().contains("Unsupported URL") => first.map(|(l, o)| (l, o, None)),
        Err(e) => Err(e),
    }
}

/// The video the page at `url` declares, read with the browser's Referer, User-Agent and Cookie
/// header for it; none when the page can't be read or declares none clearly.
fn read(url: &Url, browser: &FromBrowser, reader: &crate::streams::Engine) -> Option<Declared> {
    use reqwest::header::{COOKIE, HeaderMap, HeaderValue, REFERER};
    let mut headers = HeaderMap::new();
    for (name, value) in [(REFERER, &browser.referer), (COOKIE, &browser.cookies)] {
        if let Some(v) = value.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
            headers.insert(name, v);
        }
    }
    let mut opts = reader.opts.clone();
    if let Some(agent) = browser
        .user_agent
        .as_deref()
        .filter(|a| !a.trim().is_empty())
    {
        opts.speak_as(agent.to_owned());
    }
    let read = mdm_engine::page_videos(url.clone(), headers, &opts);
    // On the caller's thread (never a runtime's own), within `PAGE_TIMEOUT`.
    let (at, videos) = reader
        .handle
        .block_on(async { tokio::time::timeout(PAGE_TIMEOUT, read).await })
        .ok()?
        .ok()?;
    chosen(&videos, &at)
}

/// The address yt-dlp reads a declared video at: Vimeo's own page of a video
/// (`vimeo.com/<id>`, `vimeo.com/<id>/<hash>` for an unlisted one) is its player
/// (`player.vimeo.com/video/<id>`), which yt-dlp reads without a sign-in and with the page as its
/// Referer (a video shown only on the sites its owner chose); any other as it is.
pub fn lookable(url: &Url) -> Url {
    let vimeo = url.host_str().is_some_and(|h| {
        h.eq_ignore_ascii_case("vimeo.com") || h.eq_ignore_ascii_case("www.vimeo.com")
    });
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let hash = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    let segs: Vec<&str> = url
        .path_segments()
        .map_or(Vec::new(), |s| s.filter(|s| !s.is_empty()).collect());
    let player = match segs.as_slice() {
        [id] if vimeo && digits(id) => format!("https://player.vimeo.com/video/{id}"),
        [id, h] if vimeo && digits(id) && hash(h) => {
            format!("https://player.vimeo.com/video/{id}?h={h}")
        }
        _ => return url.clone(),
    };
    Url::parse(&player).unwrap_or_else(|_| url.clone())
}

/// An address on this computer or its local network: `localhost` and its names, or a loopback,
/// private, link-local or unspecified IP address.
fn local(url: &Url) -> bool {
    let ip = match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            return d == "localhost" || d.ends_with(".localhost");
        }
        Some(url::Host::Ipv4(a)) => IpAddr::V4(a),
        Some(url::Host::Ipv6(a)) => a.to_ipv4_mapped().map_or(IpAddr::V6(a), IpAddr::V4),
        None => return false,
    };
    match ip {
        IpAddr::V4(a) => {
            a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified()
        }
        IpAddr::V6(a) => {
            let s = a.segments()[0];
            a.is_loopback()
                || a.is_unspecified()
                || (s & 0xfe00) == 0xfc00
                || (s & 0xffc0) == 0xfe80
        }
    }
}

/// A site's own name in its address, as the browser extension reads it (#1075, `siteNameOf`):
/// the part before its domain's ending (www.shortverse.com: `shortverse`; news.bbc.co.uk: `bbc`,
/// a country's second level left out). None for an IP address.
fn site_name(url: &Url) -> Option<String> {
    let Some(url::Host::Domain(host)) = url.host() else {
        return None;
    };
    let host = host.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let mut at = labels.len().checked_sub(2)?;
    let second = [
        "co", "com", "org", "net", "gov", "edu", "ac", "or", "ne", "go",
    ];
    if at > 0 && labels[labels.len() - 1].len() == 2 && second.contains(&labels[at]) {
        at -= 1;
    }
    labels.get(at).map(|l| (*l).to_owned())
}

/// `s`'s letters and digits, lower-cased.
fn letters(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Where `s` has a separator between two parts of a title: ` | `, ` - `, ` – `, ` — `, ` · `,
/// ` • `, ` » `, ` : ` or ` :: `, with white space on both sides (the extension's
/// `TITLE_SEPARATOR`): each match's start (its white space's) and end, left to right.
fn separators(s: &str) -> Vec<(usize, usize)> {
    let space_end = |from: usize| {
        s[from..]
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map_or(s.len(), |(i, _)| from + i)
    };
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((i, _)) = s[at..].char_indices().find(|(_, c)| c.is_whitespace()) {
        let start = at + i;
        let after_space = space_end(start);
        let rest = &s[after_space..];
        let sep = ["::", ":", "|", "-", "–", "—", "·", "•", "»"]
            .into_iter()
            .find(|sep| {
                rest.strip_prefix(sep)
                    .and_then(|r| r.chars().next())
                    .is_some_and(char::is_whitespace)
            });
        match sep {
            Some(sep) => {
                let end = space_end(after_space + sep.len());
                out.push((start, end));
                at = end;
            }
            None => at = after_space,
        }
        if at >= s.len() {
            break;
        }
    }
    out
}

/// `title` without the site's name of the page at `page`, by the browser extension's rule
/// (#1075, `withoutSiteName`): "Before The - Short Film | Shortverse" on shortverse.com is
/// "Before The - Short Film". The name goes where it ends the title after a separator
/// ([`separators`]), when its letters and digits are the address's own name's ([`site_name`])
/// or its first word's are ("BBC News" on bbc.co.uk); where it starts the title, only when it is
/// that name. The title stays whole when nothing else would be left.
pub fn without_site_name(title: &str, page: &Url) -> String {
    let whole = title.trim();
    let Some(site) = site_name(page)
        .map(|s| letters(&s))
        .filter(|s| !s.is_empty())
    else {
        return whole.to_owned();
    };
    let mut rest = whole;
    if let Some(&(start, end)) = separators(rest).last() {
        let tail = &rest[end..];
        let first_word = tail.split_whitespace().next().unwrap_or("");
        if letters(tail) == site || letters(first_word) == site {
            rest = rest[..start].trim();
        }
    }
    if let Some(&(start, end)) = separators(rest).first()
        && letters(&rest[..start]) == site
    {
        rest = rest[end..].trim();
    }
    if rest.is_empty() {
        whole.to_owned()
    } else {
        rest.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn named(title: &str, page: &str) -> String {
        without_site_name(title, &url(page))
    }

    #[test]
    fn the_sites_name_leaves_the_title_as_the_extension_takes_it_off() {
        let film = "https://www.shortverse.com/films/before-the-short-film";
        assert_eq!(
            named("Before The - Short Film | Shortverse", film),
            "Before The - Short Film"
        );
        assert_eq!(
            named("Before The - Short Film", film),
            "Before The - Short Film"
        );
        let bbc = "https://news.bbc.co.uk/1/hi/world/europe/1.stm";
        assert_eq!(named("A story - BBC News", bbc), "A story");
        assert_eq!(named("BBC :: A story", bbc), "A story");
        assert_eq!(
            named("A story | The Verge", "https://www.theverge.com/a"),
            "A story"
        );
        // Another site's name, an IP address, a title that is only the site's name.
        assert_eq!(
            named("A story | Elsewhere", "https://example.com/a"),
            "A story | Elsewhere"
        );
        assert_eq!(named("A story | 10", "http://10.0.0.1/a"), "A story | 10");
        assert_eq!(named("Shortverse", film), "Shortverse");
        assert_eq!(named(" - Shortverse", film), "- Shortverse");
        // A hyphen in a word is no separator.
        assert_eq!(named("Short-verse", film), "Short-verse");
    }

    #[test]
    fn vimeos_own_page_is_looked_up_at_its_player() {
        assert_eq!(
            lookable(&url("https://vimeo.com/1244858801?fl=ip&fe=ec")).as_str(),
            "https://player.vimeo.com/video/1244858801"
        );
        assert_eq!(
            lookable(&url("https://vimeo.com/1244858801/abc123")).as_str(),
            "https://player.vimeo.com/video/1244858801?h=abc123"
        );
        for same in [
            "https://player.vimeo.com/video/1244858801",
            "https://vimeo.com/channels/staffpicks",
            "https://example.com/1244858801",
        ] {
            assert_eq!(lookable(&url(same)).as_str(), same);
        }
    }

    fn declares(address: &str, name: Option<&str>) -> mdm_engine::DeclaredVideos {
        mdm_engine::DeclaredVideos {
            json_ld: vec![mdm_engine::DeclaredVideo {
                address: url(address),
                name: name.map(str::to_owned),
                main: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_declared_video_is_followed_within_limits() {
        let page = url("https://www.shortverse.com/films/before-the-short-film");
        let d = chosen(
            &declares(
                "https://vimeo.com/1244858801?fl=ip&fe=ec",
                Some("Before The - Short Film | Shortverse"),
            ),
            &page,
        )
        .unwrap();
        assert_eq!(d.address, "https://player.vimeo.com/video/1244858801");
        assert_eq!(d.page, page.as_str());
        assert_eq!(d.name.as_deref(), Some("Before The - Short Film"));
        let browser = FromBrowser {
            private: true,
            referer: Some("https://search.example/".into()),
            origin: Some("https://www.shortverse.com".into()),
            user_agent: Some("UA".into()),
            cookies: Some("session=1".into()),
            jar: Vec::new(),
        };
        let asked = d.browser(&browser);
        assert!(asked.private);
        assert!(
            asked
                .args(&d.address)
                .unwrap()
                .0
                .contains(&"--no-cookies-from-browser".into())
        );
        assert_eq!(asked.referer.as_deref(), Some(page.as_str()));
        assert_eq!(asked.user_agent.as_deref(), Some("UA"));
        assert!(asked.cookies.is_none() && asked.origin.is_none());
        // Never the page itself, nor a local address from a page on the web.
        assert!(chosen(&declares(page.as_str(), None), &page).is_none());
        for local in [
            "http://127.0.0.1:8080/v.mp4",
            "http://localhost/v.mp4",
            "http://192.168.1.2/v.mp4",
            "http://[::1]/v.mp4",
            "http://169.254.169.254/latest",
            "http://0.0.0.0/v.mp4",
        ] {
            assert!(chosen(&declares(local, None), &page).is_none(), "{local}");
        }
        // A page on this computer may declare a video on it (the lab's).
        let lab = url("http://127.0.0.1:8765/declared");
        assert!(chosen(&declares("http://127.0.0.1:8765/gen/v.mp4", None), &lab).is_some());
        // Nothing declared, or no name.
        assert!(chosen(&Default::default(), &page).is_none());
        let unnamed = chosen(&declares("https://cdn.example/v.mp4", None), &page).unwrap();
        assert_eq!(unnamed.name, None);
    }

    fn looked(warnings: &[&str], json: &str) -> Result<(Looked, LookedUp), ProcError> {
        Ok((
            resolve::parse_looked(json.as_bytes()).unwrap(),
            LookedUp {
                json: json.as_bytes().to_vec(),
                without_cookies: false,
                warnings: warnings.iter().map(|w| (*w).to_owned()).collect(),
            },
        ))
    }

    #[test]
    fn the_declared_video_comes_in_place_of_the_pages_find() {
        const WELCOME: &str = r#"{"title": "Shortverse : Get Started", "formats": []}"#;
        const FILM: &str =
            r#"{"title": "Before The - Short Film from Justin Barry on Vimeo", "formats": []}"#;
        let film = || Declared {
            address: "https://player.vimeo.com/video/1244858801".into(),
            page: "https://www.shortverse.com/films/before-the-short-film".into(),
            name: Some("Before The - Short Film".into()),
        };
        let failed = |message: &str| -> Result<(Looked, LookedUp), ProcError> {
            Err(ProcError::Failed {
                tool: "yt-dlp".into(),
                message: message.into(),
            })
        };
        let title = |r: &Result<(Looked, LookedUp, Option<Declared>), ProcError>| match r {
            Ok((Looked::Video(v), _, d)) => format!("{} {}", v.title, d.is_some()),
            Ok((Looked::List(l), _, _)) => format!("list {}", l.title),
            Err(e) => e.to_string(),
        };
        // Found: named as the page names it.
        let got = settle(looked(&[], WELCOME), film(), looked(&[], FILM));
        assert_eq!(title(&got), "Before The - Short Film true");
        // Refused (Vimeo's bot check): its error, not the welcome video.
        let got = settle(
            looked(&[], WELCOME),
            film(),
            failed("ERROR: [vimeo] 1244858801: Unable to download webpage: HTTP Error 401"),
        );
        assert!(title(&got).contains("HTTP Error 401"), "{}", title(&got));
        // Nothing yt-dlp knows there, or a list: the page's own find stands.
        let got = settle(
            looked(&[], WELCOME),
            film(),
            failed("ERROR: Unsupported URL: https://player.vimeo.com/video/1244858801"),
        );
        assert_eq!(title(&got), "Shortverse : Get Started false");
        let got = settle(
            looked(&[], WELCOME),
            film(),
            looked(&[], r#"{"_type": "playlist", "title": "Showcase"}"#),
        );
        assert_eq!(title(&got), "Shortverse : Get Started false");
    }

    #[test]
    fn only_a_page_yt_dlp_read_generically_is_read_again() {
        const VIDEO: &str =
            r#"{"title": "Shortverse : Get Started", "extractor_key": "Vimeo", "formats": []}"#;
        // The film page's look-up, as yt-dlp 2026.08 answered it.
        assert!(read_generically(&looked(
            &["WARNING: [generic] Falling back on generic information extractor"],
            VIDEO
        )));
        assert!(read_generically(&looked(
            &[],
            r#"{"title": "clip", "extractor_key": "Generic", "formats": []}"#
        )));
        // A site of its own (YouTube's warnings are others), a list.
        assert!(!read_generically(&looked(
            &["WARNING: [youtube] No supported JavaScript runtime could be found"],
            r#"{"title": "A talk", "extractor_key": "Youtube", "formats": []}"#
        )));
        assert!(!read_generically(&looked(
            &["WARNING: [generic] Falling back on generic information extractor"],
            r#"{"_type": "playlist", "title": "Several"}"#
        )));
        // Nothing found on the page at all, or another failure.
        let failed = |message: &str| {
            Err(ProcError::Failed {
                tool: "yt-dlp".into(),
                message: message.into(),
            })
        };
        assert!(read_generically(&failed(
            "ERROR: Unsupported URL: https://www.example.com/films/a"
        )));
        assert!(!read_generically(&failed(
            "ERROR: [youtube] x: Video unavailable"
        )));
    }
}
