//! The little of HTML the engine reads: a waiting page's `<meta http-equiv="refresh">`
//! (`download::meta_refresh`), a site's own download page (`sites`) and a page's links for Link
//! grabber (`links`, #705). Tags, their attributes and an element's text, as browsers read
//! well-formed pages; not a parser.

use std::collections::HashSet;

use url::Url;

/// Each `<name …>` tag in `page` (the name in any case, as a whole word), from `<` to its `>`
/// (or the end), with its byte offset.
pub(crate) fn tags<'a>(page: &'a str, name: &'a str) -> impl Iterator<Item = (usize, &'a str)> {
    // ASCII lowercasing keeps every byte offset.
    let lower = page.to_ascii_lowercase();
    let open = format!("<{}", name.to_ascii_lowercase());
    let mut at = 0;
    std::iter::from_fn(move || {
        while let Some(i) = lower[at..].find(&open) {
            let start = at + i;
            let after = start + open.len();
            at = after;
            // `<meta` but not `<metadata`.
            if lower[after..]
                .chars()
                .next()
                .is_some_and(|c| !(c.is_ascii_whitespace() || c == '>' || c == '/'))
            {
                continue;
            }
            let end = lower[after..]
                .find('>')
                .map_or(lower.len(), |e| after + e + 1);
            at = end;
            return Some((start, &page[start..end]));
        }
        None
    })
}

/// The value of attribute `name` (any case) in the tag `tag` (`<input type=hidden name="id">`),
/// quoted or not, as written: entities are left for `decode`. Attributes are read in order, so a
/// name inside another's value or a longer name (`data-id`) doesn't count.
pub(crate) fn attribute(tag: &str, name: &str) -> Option<String> {
    let body = tag.strip_prefix('<').unwrap_or(tag);
    // Past the tag's own name.
    let mut rest = body.trim_start_matches(|c: char| !(c.is_whitespace() || c == '>' || c == '/'));
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if rest.is_empty() || rest.starts_with('>') {
            return None;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
            .unwrap_or(rest.len());
        if end == 0 {
            // A stray `=`: step over it.
            rest = &rest[1..];
            continue;
        }
        let key = &rest[..end];
        rest = rest[end..].trim_start();
        let value = match rest.strip_prefix('=') {
            Some(after) => {
                let after = after.trim_start();
                match after.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let inner = &after[1..];
                        let close = inner.find(q).unwrap_or(inner.len());
                        rest = inner.get(close + 1..).unwrap_or("");
                        &inner[..close]
                    }
                    _ => {
                        let close = after
                            .find(|c: char| c.is_whitespace() || c == '>')
                            .unwrap_or(after.len());
                        rest = &after[close..];
                        &after[..close]
                    }
                }
            }
            None => "",
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value.to_owned());
        }
    }
}

/// `s` with its character references decoded: `&amp; &lt; &gt; &quot; &apos; &nbsp;`, `&#NN;`
/// and `&#xNN;`. Anything else stays as written.
pub(crate) fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let reference = rest[1..].find(';').filter(|&n| n <= 10).and_then(|n| {
            let name = &rest[1..1 + n];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => match name.strip_prefix('#') {
                    Some(hex) if hex.starts_with(['x', 'X']) => u32::from_str_radix(&hex[1..], 16)
                        .ok()
                        .and_then(char::from_u32),
                    Some(dec) => dec.parse().ok().and_then(char::from_u32),
                    None => None,
                },
            };
            c.map(|c| (c, n + 2))
        });
        match reference {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of the first element whose `class` list holds `class`: tags stripped, references
/// decoded, white space collapsed. Up to the first closing tag of its own name (no nesting of
/// the same element, which the pages read here don't have). None when there is none, or it's
/// empty.
pub(crate) fn text_of_class(page: &str, class: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = lower[at..].find('<') {
        let start = at + i;
        at = start + 1;
        let Some(end) = lower[start..].find('>').map(|e| start + e + 1) else {
            break;
        };
        let tag = &page[start..end];
        let has =
            attribute(tag, "class").is_some_and(|c| c.split_ascii_whitespace().any(|c| c == class));
        if !has {
            continue;
        }
        let name: String = lower[start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let close = format!("</{name}");
        let inner_end = lower[end..].find(&close).map_or(page.len(), |e| end + e);
        let text = decode(&strip_tags(&page[end..inner_end]));
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        return (!text.is_empty()).then_some(text);
    }
    None
}

/// Every link on the web page `page` (#705), as a browser lists a page's links and media for
/// "Download all links": each `<a href>` and `<area href>`, and each `<video src>`, `<audio src>`
/// and `<source src>`, in the page's order. Each is read as the browser reads it: references
/// decoded, against the page's `<base href>` or else `at` (the address the page came from),
/// without its `#fragment`, and once. Only what can be downloaded: http(s), ftp(s) and magnet
/// links (not `mailto:`, `javascript:`, `data:`). Comments, scripts and styles aren't markup:
/// an address in them isn't a link.
pub fn links(page: &str, at: &Url) -> Vec<Url> {
    let page = markup(page);
    let base = tags(&page, "base")
        .find_map(|(_, t)| attribute(t, "href"))
        .and_then(|h| at.join(decode(&h).trim()).ok())
        .unwrap_or_else(|| at.clone());
    let mut found: Vec<(usize, String)> = Vec::new();
    for (name, attr) in [
        ("a", "href"),
        ("area", "href"),
        ("video", "src"),
        ("audio", "src"),
        ("source", "src"),
    ] {
        found.extend(tags(&page, name).filter_map(|(i, t)| attribute(t, attr).map(|v| (i, v))));
    }
    found.sort_by_key(|(i, _)| *i);
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter_map(|(_, value)| {
            let mut url = base.join(decode(&value).trim()).ok()?;
            if !(crate::supported(&url) || url.scheme() == "magnet") {
                return None;
            }
            url.set_fragment(None);
            seen.insert(url.clone()).then_some(url)
        })
        .collect()
}

/// `page` without what isn't markup: its comments, and the insides of its `<script>` and
/// `<style>` elements (their tags too: a script's `src` isn't a link).
fn markup(page: &str) -> String {
    const SKIPPED: [(&str, &str); 3] = [
        ("<!--", "-->"),
        ("<script", "</script"),
        ("<style", "</style"),
    ];
    // ASCII lowercasing keeps every byte offset.
    let lower = page.to_ascii_lowercase();
    let mut out = String::with_capacity(page.len());
    let mut at = 0;
    loop {
        let next = SKIPPED
            .iter()
            .filter_map(|&(open, close)| opening(&lower, at, open).map(|i| (i, open, close)))
            .min_by_key(|&(i, ..)| i);
        let Some((start, open, close)) = next else {
            break;
        };
        out.push_str(&page[at..start]);
        let inside = start + open.len();
        at = match lower[inside..].find(close) {
            // A comment ends at its `-->`; an element past its closing tag's `>`.
            Some(e) if close == "-->" => inside + e + close.len(),
            Some(e) => {
                let end = inside + e + close.len();
                lower[end..].find('>').map_or(lower.len(), |g| end + g + 1)
            }
            None => lower.len(),
        };
    }
    out.push_str(&page[at..]);
    out
}

/// Where `open` (`<!--`, or a tag's `<name` as a whole name) is next in `lower` from `at`.
fn opening(lower: &str, at: usize, open: &str) -> Option<usize> {
    let mut from = at;
    while let Some(i) = lower[from..].find(open) {
        let start = from + i;
        let after = start + open.len();
        let whole = open == "<!--"
            || lower[after..]
                .chars()
                .next()
                .is_none_or(|c| c.is_ascii_whitespace() || c == '>' || c == '/');
        if whole {
            return Some(start);
        }
        from = after;
    }
    None
}

/// `html` without its tags (`<a href=…>name</a>` → `name`).
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_are_read_in_order() {
        let tag = r#"<input type="hidden" name='id' value=abc data-value="x">"#;
        assert_eq!(attribute(tag, "type").as_deref(), Some("hidden"));
        assert_eq!(attribute(tag, "NAME").as_deref(), Some("id"));
        assert_eq!(attribute(tag, "value").as_deref(), Some("abc"));
        assert_eq!(attribute(tag, "id"), None, "a value isn't a name");
        assert_eq!(
            attribute("<form id=download-form>", "id").as_deref(),
            Some("download-form")
        );
        assert_eq!(
            attribute("<input disabled name=x>", "name").as_deref(),
            Some("x")
        );
        assert_eq!(attribute("<br/>", "x"), None);
        // Entities stay for `decode`.
        assert_eq!(
            attribute(r#"<a href="/uc?a=1&amp;b=2">"#, "href").as_deref(),
            Some("/uc?a=1&amp;b=2")
        );
    }

    #[test]
    fn tags_are_whole_names() {
        let page = "<META a=1><metadata><meta\nb=2/><p>";
        let found: Vec<_> = tags(page, "meta").map(|(_, t)| t).collect();
        assert_eq!(found, ["<META a=1>", "<meta\nb=2/>"]);
    }

    #[test]
    fn references_decode() {
        assert_eq!(
            decode("a&amp;b &lt;c&gt; &quot;d&quot; &#39;e&#x27; &copy; & f&amp"),
            "a&b <c> \"d\" 'e' &copy; & f&amp"
        );
        assert_eq!(decode("can&#39;t"), "can't");
    }

    #[test]
    fn an_elements_text() {
        let page = r#"<p class="uc-error-caption">Sorry, you can&#39;t
            view this</p><p class="x uc-error-subcaption"><b>Too many</b> users</p>"#;
        assert_eq!(
            text_of_class(page, "uc-error-caption").as_deref(),
            Some("Sorry, you can't view this")
        );
        assert_eq!(
            text_of_class(page, "uc-error-subcaption").as_deref(),
            Some("Too many users")
        );
        assert_eq!(text_of_class(page, "uc-error"), None);
        assert_eq!(text_of_class("<p class=e>  </p>", "e"), None);
    }

    fn found(page: &str, at: &str) -> Vec<String> {
        links(page, &Url::parse(at).unwrap())
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn a_pages_links_as_the_browser_reads_them() {
        let page = r##"<html><body>
            <A HREF="setup.exe">Setup</A>
            <a class=x href='/files/manual.pdf#page=2'>Manual</a>
            <a href=../up.zip>Up</a>
            <a href="https://cdn.example.net/a.zip?x=1&amp;y=2">CDN</a>
            <a href="setup.exe">again</a>
            <a href="#top">top</a> <a name="anchor">no href</a> <abbr href="x.bin">not a link</abbr>
            <a href="mailto:me@example.com">mail</a> <a href="javascript:void(0)">js</a>
            <a href="  ftp://ftp.example.org/pub/f.tar.gz ">ftp</a>
            <a href="magnet:?xt=urn:btih:abc&amp;dn=x">magnet</a>
            <map><area shape=rect href="map.zip"></map>
            <video src="clip.mp4"><source src="clip.webm"></video><audio src="/a.mp3"></audio>
        </body></html>"##;
        assert_eq!(
            found(page, "https://example.com/dir/page.html"),
            [
                "https://example.com/dir/setup.exe",
                "https://example.com/files/manual.pdf",
                "https://example.com/up.zip",
                "https://cdn.example.net/a.zip?x=1&y=2",
                "https://example.com/dir/page.html",
                "ftp://ftp.example.org/pub/f.tar.gz",
                "magnet:?xt=urn:btih:abc&dn=x",
                "https://example.com/dir/map.zip",
                "https://example.com/dir/clip.mp4",
                "https://example.com/dir/clip.webm",
                "https://example.com/a.mp3",
            ]
        );
    }

    #[test]
    fn links_follow_the_pages_base() {
        let page = r#"<head><base href="https://files.example.org/v2/"></head><a href="x.zip">"#;
        assert_eq!(
            found(page, "https://example.com/"),
            ["https://files.example.org/v2/x.zip"]
        );
    }

    #[test]
    fn comments_scripts_and_styles_hold_no_links() {
        let page = r#"<!-- <a href="old.zip"> --><script src="/app.js">
            document.write('<a href="w.zip">'); if (a<b) {}</script>
            <style>a[href="s.zip"] { color: red }</style><SCRIPT>"<a href='x.zip'>"</SCRIPT>
            <scripts><a href="kept.zip"></a>"#;
        assert_eq!(
            found(page, "https://example.com/"),
            ["https://example.com/kept.zip"]
        );
        // Never closed: nothing after it is read.
        assert!(found("<!-- <a href=a.zip>", "https://example.com/").is_empty());
    }

    /// #705: EPA's EPANET page (saved 2026-11-12) lists its installers and archives as plain
    /// links among the site's own pages; Link grabber showed only the page itself.
    #[test]
    fn the_epanet_page_lists_its_downloads() {
        let page = include_str!("../tests/data/epanet.html");
        let all = found(page, "https://www.epa.gov/water-research/epanet");
        for download in [
            "https://github.com/USEPA/EPANET2.2/releases/download/2.2.0/epanet2.2_setup.exe",
            "https://github.com/USEPA/EPANET2.2/releases/download/2.2.0/epanet2.2.zip",
            "https://github.com/USEPA/EPANET2.2/releases/download/2.2.0/epanet2.2_toolkit.zip",
            "https://github.com/USEPA/EPANET2.2/archive/2.2.0.zip",
            "https://www.epa.gov/sites/default/files/2018-10/en2setup2.00.12.01.exe",
            "https://www.epa.gov/sites/default/files/2014-06/en2toolkit.zip",
            "https://www.epa.gov/sites/default/files/2018-10/en2source.zip",
        ] {
            assert!(all.iter().any(|u| u == download), "{download} not found");
        }
        // The site's own pages, relative there, come whole; each link once.
        assert!(
            all.iter()
                .any(|u| u == "https://www.epa.gov/environmental-topics")
        );
        let unique: HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
    }
}
