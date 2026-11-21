//! Rapidgator. A file's page (`rapidgator.net/file/<id>/<name>.html`, or the short domain's
//! `rg.to/file/<id>/…`) gives a signed-in premium account the file's download link, on one of its
//! file hosts (`s<N>.rapidgator.net/download/<token>`), which lives a while only: a long download
//! (gigabytes) that asks for more of the file later (its parts split as connections free up) was
//! answered 404 Not Found at 90 % and stopped. The file's address names nothing of its page, and
//! the browser's Referer is the site's address alone, so the page comes from the browser: the tab
//! the download was started from (`Options::page`, #607), else a Referer that is one. That page is
//! the link's origin (`download::Ctx::refresh_target`): asked again with the browser's cookies (its
//! signed-in session), it gives a fresh link to the same file, which the download goes on from,
//! its size checked. Pure functions over addresses and page text, asked by `download` through
//! `sites`.
//!
//! Learned from Rapidgator's public pages and the addresses a browser shows (no account's pages
//! were copied, and no other program's code read): the link is looked for by its shape (a file
//! host's `/download/…` address in the page, in a link or a script), never by the page's words, so
//! a page that changes its layout still gives it, and one without it (signed out, a free account's
//! waiting page) gives nothing. A file page that redirects to the link (a premium account's
//! "direct downloads") is followed as any redirect.

use url::Url;

use super::PageSays;
use crate::html;

/// The site's own domain, its pages' host; and its short domain, whose file pages are the same.
const SITE: &str = "rapidgator.net";
const SHORT: &str = "rg.to";

/// `host`, lowercase, is `domain` or one of its subdomains (never a look-alike such as
/// `rapidgator.net.example` or `notrapidgator.net`).
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

/// Rapidgator's own hosts: `rapidgator.net` and its subdomains, `rg.to` and its, over http(s).
pub(crate) fn is_site(url: &Url) -> bool {
    host_of(url).is_some_and(|h| under(&h, SITE) || under(&h, SHORT))
}

/// One of the hosts its files come from: a subdomain of `rapidgator.net` of one label other than
/// `www` (`s118`, `pr12`).
fn is_file_host(url: &Url) -> bool {
    host_of(url).is_some_and(|h| {
        h.strip_suffix(".rapidgator.net").is_some_and(|label| {
            !label.is_empty()
                && label != "www"
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    })
}

/// A file host's download link: `/download/<token>` (today's), or the older
/// `/?r=download/…` form.
fn is_file_link(url: &Url) -> bool {
    if !is_file_host(url) {
        return false;
    }
    let mut parts = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty());
    let by_path = parts.next() == Some("download") && parts.next().is_some();
    by_path
        || url
            .query_pairs()
            .any(|(k, v)| k == "r" && v.starts_with("download"))
}

/// A file's id: letters and digits (32 hexadecimal digits today, a number on older links).
fn is_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A file's page (`/file/<id>[/<name>.html]` on the site's or the short domain's pages' host): the
/// same page on `rapidgator.net`, as it is asked (its scheme and port), where the browser's
/// session cookies for the file hosts' domain go too. None for any other address.
fn file_page(url: &Url) -> Option<Url> {
    let host = host_of(url)?;
    let pages_host = matches!(
        host.as_str(),
        SITE | SHORT | "www.rapidgator.net" | "www.rg.to"
    );
    if !pages_host {
        return None;
    }
    let mut parts = url.path_segments()?;
    if parts.next() != Some("file") || !parts.next().is_some_and(is_id) {
        return None;
    }
    let mut page = url.clone();
    page.set_host(Some(SITE)).ok()?;
    page.set_fragment(None);
    Some(page)
}

/// The page that gives `url`, a file host's link, again (`sites::origin`): the first of `pages`
/// (the browser's tab the download was started from, its Referer) that is one of its files' pages.
/// None for any other address, or when no page is known (a Referer of the site's address alone):
/// the link is then all there is, and its download waits for a fresh one from the browser.
pub(crate) fn origin(url: &Url, pages: &[Url]) -> Option<Url> {
    if !is_file_link(url) {
        return None;
    }
    pages.iter().find_map(file_page)
}

/// A file host's link expires: a dead link's answer there is the link expired, whatever
/// the status (`Error::LinkExpired`).
pub(crate) fn expiring(url: &Url) -> bool {
    is_file_link(url)
}

/// `a` and `b` are both Rapidgator's (`rapidgator.net` and its subdomains, on one port): the
/// browser's cookies for the file hosts' domain (its signed-in session) go to the file's page, as
/// the browser's own do.
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    let ours = |u: &Url| host_of(u).is_some_and(|h| under(&h, SITE));
    ours(a) && ours(b) && a.port_or_known_default() == b.port_or_known_default()
}

/// `at` is Rapidgator's address for a file (a file host's link, a file's page): a web page from
/// it without a file name is the site talking (signed out, a free account's waiting page, an
/// expired link's notice), never the file.
pub(crate) fn expects_file(at: &Url) -> bool {
    is_file_link(at) || file_page(at).is_some()
}

/// What Rapidgator's page `page`, answered 200 at `at`, says: a file's page with its download link
/// (a premium account's: in a link, or in a script's text) leads there; any other page, or a file
/// page without one, says nothing known.
pub(crate) fn read_page(at: &Url, page: &str) -> PageSays {
    if file_page(at).is_none() {
        return PageSays::Nothing;
    }
    html::links(page, at)
        .into_iter()
        .find(is_file_link)
        .or_else(|| script_link(page))
        .map_or(PageSays::Nothing, PageSays::Next)
}

/// The first file host's download link written whole in the page's text: a script's
/// (`var link = 'https://s1.rapidgator.net/download/…';`, its slashes escaped as JSON writes them
/// too), a data attribute's.
fn script_link(page: &str) -> Option<Url> {
    let text = page.replace("\\/", "/");
    let mut rest = text.as_str();
    while let Some(i) = rest.find("://") {
        // The scheme starts after the last character that isn't an ASCII letter (which may be
        // several bytes long: «, a no-break space, a dash).
        let start = rest[..i]
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map_or(0, |(s, c)| s + c.len_utf8());
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '`' | ')')
            })
            .unwrap_or(tail.len());
        let candidate = html::decode(&rest[start..i + end]);
        if let Ok(url) = Url::parse(&candidate)
            && is_file_link(&url)
        {
            return Some(url);
        }
        rest = &rest[i + 3..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    const LINK: &str = "https://s118.rapidgator.net/download/0f3c9a2e-6b1d-4c8e-9a7f-2d5b8e1c4a60";
    const PAGE: &str =
        "https://rapidgator.net/file/6e2d0c8a3a7b4f1e9d5c2b1a0f9e8d7c/Lab.Archive.part1.rar.html";

    /// A premium account's file page as such pages are laid out: other links first (the site's
    /// own pages, a free download's start), the link in a button; written here, not copied.
    fn premium_page(link: &str) -> String {
        format!(
            r#"<!DOCTYPE html><html><head><title>Download file Lab.Archive.part1.rar</title></head>
<body><a href="/article/premium">Premium</a>
<a href="/download/AjaxStartTimer?fid=1">Slow download</a>
<div class="btm"><a class="btn btn-download"
   href="{link}">Download</a></div></body></html>"#
        )
    }

    #[test]
    fn a_file_link_has_its_page_as_origin() {
        let pages = [url("https://rapidgator.net/"), url(PAGE)];
        assert_eq!(origin(&url(LINK), &pages), Some(url(PAGE)));
        // The short domain's page, the www host, the id alone, a numbered file's: the same page
        // on the site's own host, as asked (http, a port: the lab's tests).
        for (page, want) in [
            (
                "https://rg.to/file/6e2d0c8a3a7b4f1e9d5c2b1a0f9e8d7c/a.rar.html",
                "https://rapidgator.net/file/6e2d0c8a3a7b4f1e9d5c2b1a0f9e8d7c/a.rar.html",
            ),
            (
                "https://www.rapidgator.net/file/12345678#top",
                "https://rapidgator.net/file/12345678",
            ),
            (
                "http://RAPIDGATOR.net:8080/file/lab4Mt1500/a.zip.html",
                "http://rapidgator.net:8080/file/lab4Mt1500/a.zip.html",
            ),
        ] {
            assert_eq!(origin(&url(LINK), &[url(page)]), Some(url(want)), "{page}");
        }
        // The older link's form is a file host's link too.
        assert_eq!(
            origin(
                &url("https://pr12.rapidgator.net//?r=download/index&session_id=abc"),
                &[url(PAGE)]
            ),
            Some(url(PAGE))
        );
    }

    /// Without a file's page (the Referer is the site's address alone), or for another address:
    /// no origin, the link is all there is.
    #[test]
    fn no_page_or_no_file_link_no_origin() {
        for pages in [
            vec![],
            vec![url("https://rapidgator.net/")],
            vec![url("https://rapidgator.net/folder/123/Stuff.html")],
            vec![url("https://rapidgator.net/file/")],
            vec![url("https://rapidgator.net/file/not-an-id/a.html")],
            vec![url("https://rapidgator.net.example/file/123/a.html")],
            vec![url("https://example.com/file/123/a.html")],
            vec![url("https://s118.rapidgator.net/file/123/a.html")],
        ] {
            assert_eq!(origin(&url(LINK), &pages), None, "{pages:?}");
        }
        for other in [
            PAGE,
            "https://rapidgator.net/download/AjaxStartTimer?fid=1",
            "https://www.rapidgator.net/download/abc",
            "https://s118.rapidgator.net/",
            "https://s118.rapidgator.net/download/",
            "https://s118.rapidgator.net.example/download/abc",
            "https://s118.notrapidgator.net/download/abc",
            "https://a.b.rapidgator.net/download/abc",
            "ftp://s118.rapidgator.net/download/abc",
        ] {
            assert_eq!(origin(&url(other), &[url(PAGE)]), None, "{other}");
            assert!(!expiring(&url(other)), "{other}");
        }
        assert!(expiring(&url(LINK)));
    }

    #[test]
    fn a_premium_page_gives_its_link() {
        let at = url(PAGE);
        assert_eq!(
            read_page(&at, &premium_page(LINK)),
            PageSays::Next(url(LINK))
        );
        // Entities in the link's address.
        let amp = format!("{LINK}?a=1&amp;b=2");
        assert_eq!(
            read_page(&at, &premium_page(&amp)),
            PageSays::Next(url(&format!("{LINK}?a=1&b=2")))
        );
        // Only in a script: as a string, its slashes escaped or not.
        for script in [
            format!("<script>var premium_download_link = '{LINK}';</script>"),
            format!(
                "<script>window.dl = {{\"url\":\"{}\"}};</script>",
                LINK.replace('/', "\\/")
            ),
            format!("<div data-href=\"{LINK}\"></div>"),
        ] {
            let page = format!(
                "<html><body><a href=\"https://rapidgator.net/\">Home</a>\
                 <script src=\"https://static.rapidgator.net/js/app.js\"></script>{script}</body></html>"
            );
            assert_eq!(read_page(&at, &page), PageSays::Next(url(LINK)), "{script}");
        }
    }

    /// Punctuation right before the scheme is more than one byte long: the link still reads
    /// whole, and the page never panics on a slice inside the character.
    #[test]
    fn a_link_after_multibyte_punctuation_is_read() {
        let at = url(PAGE);
        for before in ["«", "\u{a0}", "—", "“", "日本語", "é", "🦀"] {
            let page =
                format!("<html><body><script>var s = '{before}{LINK}';</script></body></html>");
            assert_eq!(read_page(&at, &page), PageSays::Next(url(LINK)), "{before}");
        }
        // A link at the end of the text, escaped slashes, and a multibyte end delimiter.
        assert_eq!(
            script_link("«https://s1.rapidgator.net/download/x"),
            Some(url("https://s1.rapidgator.net/download/x"))
        );
        for text in [
            format!("«{LINK}"),
            format!("🦀{}\u{2003}after", LINK.replace('/', "\\/")),
            format!("—:// «ftp://s1.rapidgator.net/download/x\u{a0}é{LINK}"),
        ] {
            assert_eq!(script_link(&text), Some(url(LINK)), "{text}");
        }
        assert_eq!(script_link("—://"), None);
    }

    /// A page without the link (signed out, a free account's waiting page), another site's
    /// page, or a page that isn't a file's: nothing known.
    #[test]
    fn other_pages_say_nothing() {
        let free = "<html><a href=\"/download/AjaxStartTimer?fid=1\">Slow download</a>\
                    <a href=\"https://rapidgator.net/auth/login\">Log in</a>\
                    <script>var timer = 'https://rapidgator.net/download/AjaxGetDownloadLink';</script></html>";
        assert_eq!(read_page(&url(PAGE), free), PageSays::Nothing);
        assert_eq!(
            read_page(&url("https://rapidgator.net/"), &premium_page(LINK)),
            PageSays::Nothing
        );
        assert_eq!(
            read_page(
                &url("https://example.com/file/123/a.html"),
                &premium_page(LINK)
            ),
            PageSays::Nothing
        );
        assert_eq!(
            read_page(
                &url(PAGE),
                &premium_page("https://s118.example.net/download/abc")
            ),
            PageSays::Nothing
        );
    }

    #[test]
    fn its_file_hosts_and_pages_expect_files() {
        for at in [LINK, PAGE, "https://rg.to/file/123"] {
            assert!(expects_file(&url(at)), "{at}");
        }
        for at in [
            "https://rapidgator.net/",
            "https://rapidgator.net/article/premium",
            "https://s118.rapidgator.net/",
            "https://example.com/download/abc",
        ] {
            assert!(!expects_file(&url(at)), "{at}");
        }
    }

    #[test]
    fn its_hosts_are_one_party() {
        assert!(same_party(&url(LINK), &url(PAGE)));
        assert!(same_party(
            &url("http://s1.rapidgator.net:9000/download/a"),
            &url("http://rapidgator.net:9000/file/1")
        ));
        for (a, b) in [
            (LINK, "https://rg.to/file/1"),
            (LINK, "https://rapidgator.net.example/file/1"),
            (LINK, "https://example.com/"),
            (
                "http://s1.rapidgator.net:9000/download/a",
                "http://rapidgator.net:9001/file/1",
            ),
        ] {
            assert!(!same_party(&url(a), &url(b)), "{a} {b}");
        }
    }
}
