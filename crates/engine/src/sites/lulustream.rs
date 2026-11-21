//! LuluStream, a video host whose pages come under several domains (`lulustream.com`,
//! `luluvdo.com`, `lplaygo.com`), each a mirror of the others. A video's link is its watch page
//! (`/<code>`) or its player (`/e/<code>`), never the file. Its download goes through pages:
//!
//! 1. the download page, `/d/<code>`, lists a link per quality it has (`/d/<code>_<mode>`, the
//!    mode one letter: `o` the original, `h` high, `n` normal, `l` low), not always every one;
//! 2. a quality's page holds a form (`op=download_orig`, the code, the mode and a `hash` given
//!    with each page) that the browser posts back to the same page;
//! 3. the answer to that POST is a page whose link (a signed address on a file server of another
//!    domain, with the video's `<code>_<mode>` in its path) gives the file for a while, to the
//!    address that asked for it.
//!
//! A watch or player link is asked at its download page (`sites::rewrite`), the best quality the
//! page lists is followed, its form posted (`PageSays::Form`) and the file's link taken from the
//! answer. The site answers about one post in three with the form again ("Security error"),
//! whatever the request's headers: posted again, it gives the link. Those links expire: a download given one by the browser has the quality's page as
//! its origin when the tab it came from (or its Referer) is one of the video's pages, and a
//! download given the video's link asks that link again (`download::origin_of`). A form with a
//! captcha (`data-sitekey`) needs the browser: refused as such, never posted.
//!
//! Learned from LuluStream's public pages, a free video's, without signing in. Pages are matched
//! on their structure (paths, the form's fields, a link's path), never on their words.

use url::Url;

use super::{PageSays, Refusal};
use crate::html;

/// The site's name, as the interface says it.
const NAME: &str = "LuluStream";

/// The domains its pages come under. Mirrors appear as others are blocked: one more line here.
const DOMAINS: &[&str] = &["lulustream.com", "luluvdo.com", "lplaygo.com"];

/// The qualities a download page may list, best first.
const MODES: &[char] = &['o', 'h', 'n', 'l'];

/// The form's `op` that asks for the file.
const OP: &str = "download_orig";

/// `host`, lowercase, is `domain` or one of its subdomains (never a look-alike such as
/// `lulustream.com.example` or `notlulustream.com`).
fn under(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.'))
}

/// An http(s) address's host, lowercase.
fn host_of(url: &Url) -> Option<String> {
    matches!(url.scheme(), "http" | "https")
        .then(|| url.host_str().map(str::to_ascii_lowercase))
        .flatten()
}

/// One of LuluStream's own hosts, over http(s).
pub(crate) fn is_site(url: &Url) -> bool {
    host_of(url).is_some_and(|h| DOMAINS.iter().any(|d| under(&h, d)))
}

/// A video's code: 12 lowercase letters and digits (the site's own pages, `premium`, `tos`,
/// `api.html`, `make_money`, never are).
fn is_code(s: &str) -> bool {
    s.len() == 12
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// `s` as `<code>_<mode>`, the mode one lowercase letter.
fn code_mode(s: &str) -> Option<(&str, char)> {
    let (code, mode) = s.rsplit_once('_')?;
    let mut m = mode.chars();
    let mode = m.next().filter(|c| c.is_ascii_lowercase())?;
    (m.next().is_none() && is_code(code)).then_some((code, mode))
}

/// Which of the site's pages an address is.
#[derive(Debug, PartialEq, Eq)]
enum Page<'a> {
    /// The watch page (`/<code>`) or the player (`/e/<code>`).
    Watch(&'a str),
    /// The download page, listing the qualities (`/d/<code>`).
    Download(&'a str),
    /// A quality's page, with its form, and its answer to it (`/d/<code>_<mode>`).
    Quality(&'a str, char),
}

/// The page `url` is on the site, or None (another site, the site's other pages).
fn page_of(url: &Url) -> Option<Page<'_>> {
    if !is_site(url) {
        return None;
    }
    let parts: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [code] if is_code(code) => Some(Page::Watch(code)),
        ["e", code] if is_code(code) => Some(Page::Watch(code)),
        ["d", code] if is_code(code) => Some(Page::Download(code)),
        ["d", last] => code_mode(last).map(|(code, mode)| Page::Quality(code, mode)),
        _ => None,
    }
}

/// `url` with `path`, without its query and fragment.
fn with_path(url: &Url, path: &str) -> Url {
    let mut next = url.clone();
    next.set_path(path);
    next.set_query(None);
    next.set_fragment(None);
    next
}

/// A watch page's or player's link: its download page, on the same host (`sites::rewrite`).
pub(crate) fn rewrite(url: &Url) -> Option<Url> {
    match page_of(url)? {
        Page::Watch(code) => Some(with_path(url, &format!("/d/{code}"))),
        _ => None,
    }
}

/// `a` and `b` are both LuluStream's (any of its domains, on one port): one site under several
/// names, whose pages link to each other's (a download page on one lists qualities on another),
/// so the cookies a page sets go on with the download.
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    is_site(a) && is_site(b) && a.port_or_known_default() == b.port_or_known_default()
}

/// `at` is one of the site's video pages: a web page from it is the site talking (a removed
/// video, a changed page), never the file.
pub(crate) fn expects_file(at: &Url) -> bool {
    page_of(at).is_some()
}

/// A file server's link for the video `code` in quality `mode`: an http(s) address off the
/// site's own pages, one of whose path's folders is `<code>_<mode>`, ending in a file's name.
fn is_file_link(url: &Url, code: &str, mode: char) -> bool {
    if is_site(url) || host_of(url).is_none() {
        return false;
    }
    let Some(parts) = url.path_segments() else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let folder = format!("{code}_{mode}");
    let named = parts.last().is_some_and(|n| n.contains('.'));
    named
        && parts.len() >= 2
        && parts[..parts.len() - 1]
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&folder))
}

/// A file server's link from a download that the page `pages` lists (its tab, its Referer) gave:
/// that video's quality page, which gives a fresh one (`sites::origin`). None for any other
/// address, or when none of `pages` is the same video's.
pub(crate) fn origin(url: &Url, pages: &[Url]) -> Option<Url> {
    if is_site(url) || host_of(url).is_none() {
        return None;
    }
    let folders: Vec<&str> = url.path_segments()?.collect();
    let (code, mode) = folders.iter().rev().skip(1).find_map(|p| code_mode(p))?;
    if !is_file_link(url, code, mode) {
        return None;
    }
    pages.iter().find_map(|page| {
        let same = match page_of(page)? {
            Page::Watch(c) | Page::Download(c) | Page::Quality(c, _) => c == code,
        };
        same.then(|| with_path(page, &format!("/d/{code}_{mode}")))
    })
}

/// The site still answered its form with the form after the last post: it didn't give the file
/// this time (`sites::form_refused`).
pub(crate) fn form_refused(at: &Url) -> Option<Refusal> {
    is_site(at).then(|| Refusal::NotTaken {
        site: NAME.to_owned(),
    })
}

/// What the site's page `page`, answered 200 at `at`, says:
/// 1. a download page: its best quality's page among those it lists;
/// 2. a quality's page with its form: the form, to post (a captcha's: refused, needs the browser);
/// 3. the answer to that form: its file server's link;
/// 4. nothing known (a removed video, any other page).
pub(crate) fn read_page(at: &Url, page: &str) -> PageSays {
    match page_of(at) {
        Some(Page::Download(code)) => best_quality(at, page, code),
        Some(Page::Quality(code, mode)) => match form(at, page, code) {
            Some(Ok((action, body))) => PageSays::Form { action, body },
            Some(Err(refusal)) => PageSays::Refused(refusal),
            None => html::links(page, at)
                .into_iter()
                .find(|l| is_file_link(l, code, mode))
                .map_or(PageSays::Nothing, PageSays::Next),
        },
        _ => PageSays::Nothing,
    }
}

/// The best of the qualities' pages for `code` that the download page lists (rule 1).
fn best_quality(at: &Url, page: &str, code: &str) -> PageSays {
    let listed: Vec<(usize, Url)> = html::links(page, at)
        .into_iter()
        .filter_map(|l| match page_of(&l) {
            Some(Page::Quality(c, mode)) if c == code => {
                let rank = MODES.iter().position(|m| *m == mode)?;
                Some((rank, with_path(&l, l.path())))
            }
            _ => None,
        })
        .collect();
    listed
        .into_iter()
        .min_by_key(|(rank, _)| *rank)
        .map_or(PageSays::Nothing, |(_, next)| PageSays::Next(next))
}

/// A quality page's download form for `code` (rule 2): its action, on the site (the page itself
/// when it names none), and its hidden fields as a form posts them; a refusal when the form wants
/// a captcha. None when the page has no such form (the answer to it, a removed video).
fn form(at: &Url, page: &str, code: &str) -> Option<Result<(Url, String), Refusal>> {
    let lower = page.to_ascii_lowercase();
    for (start, tag) in html::tags(page, "form") {
        let inner = start + tag.len();
        let end = lower[inner..]
            .find("</form")
            .map_or(page.len(), |e| inner + e);
        let fields: Vec<(String, String)> = html::tags(&page[inner..end], "input")
            .filter(|(_, input)| {
                html::attribute(input, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden"))
            })
            .filter_map(|(_, input)| {
                let name = html::decode(&html::attribute(input, "name")?);
                let value = html::decode(&html::attribute(input, "value").unwrap_or_default());
                (!name.is_empty()).then_some((name, value))
            })
            .collect();
        let field = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        if field("op") != Some(OP) || field("id") != Some(code) {
            continue;
        }
        let post = html::attribute(tag, "method").is_some_and(|m| m.eq_ignore_ascii_case("post"));
        let action = html::decode(&html::attribute(tag, "action").unwrap_or_default());
        // Posted where the browser posts it: the page itself, else its action with its query,
        // never a fragment.
        let mut action = match action.trim() {
            "" => at.clone(),
            a => at.join(a).ok()?,
        };
        action.set_fragment(None);
        // Never a form off the site, nor one that isn't a POST (the site's are).
        if !post || !is_site(&action) {
            return None;
        }
        if lower[inner..end].contains("data-sitekey") {
            return Some(Err(Refusal::NeedsBrowser {
                site: NAME.to_owned(),
            }));
        }
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&fields)
            .finish();
        return Some(Ok((action, body)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    const CODE: &str = "xmgvk2758mtu";
    const FILE: &str = "https://s3vzcp6anxcc.files.example/v/03/04719/xmgvk2758mtu_h/clip.mp4?t=TOKEN&s=1795180204&e=28800&f=23598187";

    /// A download page as the site's are laid out: the site's links first, a quality's link,
    /// the report form; written here, not copied.
    fn download_page(qualities: &[&str]) -> String {
        let links: String = qualities
            .iter()
            .map(|q| format!("<a href=\"{q}\" class=\"btn\">Download</a>\n"))
            .collect();
        format!(
            r#"<html><body><a href="/premium">Premium</a><a href="/tos">Terms</a>
{links}<form method="POST"><input type="hidden" name="op" value="report_file">
<input type="hidden" name="file_code" value="{CODE}"></form></body></html>"#
        )
    }

    /// A quality's page with its form, as the site's: the hidden fields, a button.
    fn quality_page(extra: &str) -> String {
        format!(
            r#"<html><body><a href="/d/{CODE}_h#">here</a>
<form method="POST" id="F1" action="">
<input type="hidden" name="op" value="download_orig">
<input type="hidden" name="id" value="{CODE}">
<input type="hidden" name="mode" value="h">
<input type="hidden" name="hash" value="23598187-103-25-1795180181-f8afb6cf15776bddbd595a70fcd91b83">
{extra}<button class="btn submit-btn">clip.mp4</button></form></body></html>"#
        )
    }

    #[test]
    fn its_hosts_and_no_look_alikes() {
        for at in [
            "https://lulustream.com/x",
            "https://www.luluvdo.com/x",
            "http://LPLAYGO.com/x",
        ] {
            assert!(is_site(&url(at)), "{at}");
        }
        for at in [
            "https://lulustream.com.example/x",
            "https://notlulustream.com/x",
            "ftp://lulustream.com/x",
            "https://example.com/lulustream.com",
        ] {
            assert!(!is_site(&url(at)), "{at}");
        }
    }

    #[test]
    fn a_watch_or_player_link_is_asked_at_its_download_page() {
        for (link, page) in [
            (
                "https://lplaygo.com/xmgvk2758mtu",
                "https://lplaygo.com/d/xmgvk2758mtu",
            ),
            (
                "https://luluvdo.com/e/xmgvk2758mtu?autoplay=1#t",
                "https://luluvdo.com/d/xmgvk2758mtu",
            ),
            (
                "https://www.lulustream.com/xmgvk2758mtu/",
                "https://www.lulustream.com/d/xmgvk2758mtu",
            ),
        ] {
            assert_eq!(rewrite(&url(link)), Some(url(page)), "{link}");
        }
        for other in [
            // Its download and quality pages are asked as they are.
            "https://lplaygo.com/d/xmgvk2758mtu",
            "https://lplaygo.com/d/xmgvk2758mtu_h",
            // Its other pages, and codes that aren't.
            "https://lplaygo.com/",
            "https://lplaygo.com/premium",
            "https://lplaygo.com/make_money",
            "https://lplaygo.com/api.html",
            "https://lplaygo.com/XMGVK2758MTU",
            "https://lplaygo.com/xmgvk2758mt",
            // Another site.
            "https://example.com/xmgvk2758mtu",
        ] {
            assert_eq!(rewrite(&url(other)), None, "{other}");
        }
    }

    #[test]
    fn a_download_page_leads_to_its_best_quality() {
        let at = url("https://lplaygo.com/d/xmgvk2758mtu");
        // Its only one, on another of its domains (as the site links them).
        assert_eq!(
            read_page(
                &at,
                &download_page(&["https://lulustream.com/d/xmgvk2758mtu_h"])
            ),
            PageSays::Next(url("https://lulustream.com/d/xmgvk2758mtu_h"))
        );
        // The best of several, whatever their order; relative links too.
        assert_eq!(
            read_page(
                &at,
                &download_page(&[
                    "/d/xmgvk2758mtu_l",
                    "/d/xmgvk2758mtu_n",
                    "/d/xmgvk2758mtu_h"
                ])
            ),
            PageSays::Next(url("https://lplaygo.com/d/xmgvk2758mtu_h"))
        );
        assert_eq!(
            read_page(
                &at,
                &download_page(&["/d/xmgvk2758mtu_n", "/d/xmgvk2758mtu_o"])
            ),
            PageSays::Next(url("https://lplaygo.com/d/xmgvk2758mtu_o"))
        );
        // Another video's, an unknown quality's, another site's, none: nothing known.
        for links in [
            vec!["/d/aaaabbbbcccc_h"],
            vec!["/d/xmgvk2758mtu_z"],
            vec!["https://example.com/d/xmgvk2758mtu_h"],
            vec![],
        ] {
            assert_eq!(
                read_page(&at, &download_page(&links)),
                PageSays::Nothing,
                "{links:?}"
            );
        }
    }

    #[test]
    fn a_quality_page_gives_its_form_to_post() {
        let at = url("https://lplaygo.com/d/xmgvk2758mtu_h");
        assert_eq!(
            read_page(&at, &quality_page("")),
            PageSays::Form {
                action: at.clone(),
                body: "op=download_orig&id=xmgvk2758mtu&mode=h&hash=23598187-103-25-1795180181-f8afb6cf15776bddbd595a70fcd91b83".to_owned(),
            }
        );
        // An action of its own, on the site, entities decoded.
        let page =
            quality_page("").replace(r#"action="""#, r#"action="/d/xmgvk2758mtu_h?a=1&amp;b=2""#);
        match read_page(&at, &page) {
            PageSays::Form { action, .. } => {
                assert_eq!(action, url("https://lplaygo.com/d/xmgvk2758mtu_h?a=1&b=2"))
            }
            other => panic!("{other:?}"),
        }
        // A form off the site, or that isn't a POST: never posted.
        for page in [
            quality_page("").replace(r#"action="""#, r#"action="https://example.com/x""#),
            quality_page("").replace(r#"method="POST""#, r#"method="GET""#),
        ] {
            assert_eq!(read_page(&at, &page), PageSays::Nothing);
        }
        // Another video's form: not this one's.
        assert_eq!(
            read_page(
                &url("https://lplaygo.com/d/aaaabbbbcccc_h"),
                &quality_page("")
            ),
            PageSays::Nothing
        );
    }

    #[test]
    fn a_form_with_a_captcha_needs_the_browser() {
        let at = url("https://lplaygo.com/d/xmgvk2758mtu_h");
        assert_eq!(
            read_page(
                &at,
                &quality_page(r#"<div class="g-recaptcha" data-sitekey="KEY"></div>"#)
            ),
            PageSays::Refused(Refusal::NeedsBrowser {
                site: NAME.to_owned()
            })
        );
        // The class alone, on the button as today's pages have it, is no captcha.
        let page = quality_page("").replace("btn submit-btn", "g-recaptcha btn submit-btn");
        assert!(matches!(read_page(&at, &page), PageSays::Form { .. }));
    }

    #[test]
    fn the_forms_answer_gives_the_file_link() {
        let at = url("https://lplaygo.com/d/xmgvk2758mtu_h");
        let answer = format!(
            r#"<html><a href="/premium">Premium</a><a href="/d/{CODE}_h#">again</a>
<table><tr><td><a href="{}" class="btn">Direct Download Link</a></td></tr></table></html>"#,
            FILE.replace('&', "&amp;")
        );
        assert_eq!(read_page(&at, &answer), PageSays::Next(url(FILE)));
        // Another quality's or video's link, one without a file name: nothing known.
        for other in [
            FILE.replace("_h/", "_n/"),
            FILE.replace(CODE, "aaaabbbbcccc"),
            "https://s3vzcp6anxcc.files.example/v/03/xmgvk2758mtu_h/".to_owned(),
        ] {
            let page = format!("<a href=\"{other}\">x</a>");
            assert_eq!(read_page(&at, &page), PageSays::Nothing, "{other}");
        }
    }

    #[test]
    fn a_file_link_has_its_quality_page_as_origin() {
        let file = url(FILE);
        for page in [
            "https://lplaygo.com/d/xmgvk2758mtu_h",
            "https://lplaygo.com/d/xmgvk2758mtu",
            "https://lplaygo.com/xmgvk2758mtu",
        ] {
            assert_eq!(
                origin(&file, &[url("https://example.com/"), url(page)]),
                Some(url("https://lplaygo.com/d/xmgvk2758mtu_h")),
                "{page}"
            );
        }
        // No page of the same video known, or not a file link: none.
        assert_eq!(
            origin(&file, &[url("https://lplaygo.com/d/aaaabbbbcccc_h")]),
            None
        );
        assert_eq!(origin(&file, &[]), None);
        assert_eq!(
            origin(
                &url("https://lplaygo.com/d/xmgvk2758mtu_h"),
                &[url("https://lplaygo.com/d/xmgvk2758mtu_h")]
            ),
            None
        );
    }

    #[test]
    fn a_form_never_taken_is_refused_in_its_name() {
        assert_eq!(
            form_refused(&url("https://lplaygo.com/d/xmgvk2758mtu_h")),
            Some(Refusal::NotTaken {
                site: NAME.to_owned()
            })
        );
        assert_eq!(form_refused(&url("https://example.com/d/x_h")), None);
    }

    #[test]
    fn its_pages_are_one_party_and_expect_files() {
        assert!(same_party(
            &url("https://lplaygo.com/d/x"),
            &url("https://lulustream.com/d/y")
        ));
        assert!(!same_party(
            &url("https://lplaygo.com/d/x"),
            &url("https://s3vzcp6anxcc.files.example/v/x.mp4")
        ));
        for at in [
            "https://lplaygo.com/xmgvk2758mtu",
            "https://lplaygo.com/d/xmgvk2758mtu",
            "https://lplaygo.com/d/xmgvk2758mtu_h",
        ] {
            assert!(expects_file(&url(at)), "{at}");
        }
        for at in ["https://lplaygo.com/", "https://lplaygo.com/premium", FILE] {
            assert!(!expects_file(&url(at)), "{at}");
        }
    }
}
