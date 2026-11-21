//! The little of HTML the engine reads: a waiting page's `<meta http-equiv="refresh">`
//! (`download::meta_refresh`), a site's own download page (`sites`), a page's links for Link
//! grabber (`links`, #705) and the video a page declares its own (`declared_videos`, #1076).
//! Tags, their attributes and an element's text, as browsers read well-formed pages; not a
//! parser.

use std::collections::HashSet;

use serde_json::Value;
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
        // The element's own closing tag, not a longer one that starts with it (`</picture`
        // begins with `</p`): the name ends at `>`, whitespace or `/`.
        let inner_end = lower[end..]
            .match_indices(close.as_str())
            .find(|(i, _)| {
                lower[end + i + close.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_ascii_whitespace() || c == '>' || c == '/')
            })
            .map_or(page.len(), |(i, _)| end + i);
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
            // A download names its server: `ftps:x`, a scheme browsers don't know, names none.
            let served = crate::supported(&url) && url.host_str().is_some_and(|h| !h.is_empty());
            if !(served || url.scheme() == "magnet") {
                return None;
            }
            url.set_fragment(None);
            seen.insert(url.clone()).then_some(url)
        })
        .collect()
}

/// One video a web page declares (#1076): its address and the page's name for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredVideo {
    /// The video's own address: a JSON-LD `VideoObject`'s `contentUrl` (the file) or else its
    /// `embedUrl` (its player), `og:video`'s, `twitter:player`'s. http(s) only, without its
    /// `#fragment`.
    pub address: Url,
    /// The page's name for it (a `VideoObject`'s `name`, else `og:title` or `twitter:title`),
    /// references decoded and white space collapsed, as written: the site's name may be in it.
    pub name: Option<String>,
    /// The page says this one is the one it is about: the `mainEntity` of its JSON-LD, or a
    /// `VideoObject` whose `url` or `mainEntityOfPage` is the page's own address.
    pub main: bool,
}

/// What a web page says its videos are (#1076), each source in the page's order, each address
/// once: its JSON-LD `VideoObject`s (also inside an `@graph`, a list of them, or another item
/// such as an article's `video`), its Open Graph videos (`og:video`, `og:video:url`,
/// `og:video:secure_url`; a Flash one left out), its `twitter:player`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredVideos {
    pub json_ld: Vec<DeclaredVideo>,
    pub open_graph: Vec<DeclaredVideo>,
    pub twitter: Vec<DeclaredVideo>,
}

impl DeclaredVideos {
    /// The video the page is about, when it says so clearly: from the first source that names
    /// any (JSON-LD, then Open Graph, then Twitter's card), the only one there, else the only
    /// one of them marked `main`. None when it declares none, or several with none main.
    pub fn main(&self) -> Option<&DeclaredVideo> {
        let list = [&self.json_ld, &self.open_graph, &self.twitter]
            .into_iter()
            .find(|l| !l.is_empty())?;
        if let [only] = list.as_slice() {
            return Some(only);
        }
        let mut main = list.iter().filter(|v| v.main);
        match (main.next(), main.next()) {
            (Some(v), None) => Some(v),
            _ => None,
        }
    }
}

/// The videos the web page `page`, which came from `at`, declares (#1076): see
/// [`DeclaredVideos`]. Addresses are read against `at`; a JSON-LD block that isn't JSON is
/// skipped, and comments, templates and other scripts hold no `<meta>`.
pub fn declared_videos(page: &str, at: &Url) -> DeclaredVideos {
    let address = |s: &str| {
        let mut url = at.join(decode(s).trim()).ok()?;
        url.set_fragment(None);
        matches!(url.scheme(), "http" | "https").then_some(url)
    };
    let mut out = DeclaredVideos::default();
    for block in json_ld_blocks(page) {
        let Ok(value) = serde_json::from_str::<Value>(block.trim()) else {
            continue;
        };
        let mut found = Vec::new();
        video_objects(&value, false, &mut found);
        for (v, main) in found {
            let link = |k: &str| link_of(v.get(k)?).and_then(|s| address(&s));
            let Some(url) = link("contentUrl").or_else(|| link("embedUrl")) else {
                continue;
            };
            let own = |k: &str| link(k).is_some_and(|u| same_page(&u, at));
            let main = main || own("url") || own("mainEntityOfPage");
            let name = ["name", "headline"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str).and_then(text));
            push(&mut out.json_ld, url, name, main);
        }
    }
    // Meta tags in the markup only: not in a script's template or a comment.
    let markup = markup(page);
    let metas: Vec<(String, String)> = tags(&markup, "meta")
        .filter_map(|(_, t)| {
            let key = attribute(t, "property").or_else(|| attribute(t, "name"))?;
            let value = attribute(t, "content")?;
            Some((decode(&key).trim().to_ascii_lowercase(), value))
        })
        .collect();
    let meta = |k: &str| {
        metas
            .iter()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| text(v))
    };
    // Open Graph: each `og:video` (or `og:video:url`) starts a video, unless the one before has
    // no address yet; `og:video:secure_url` and `og:video:type` are the current one's.
    #[derive(Default)]
    struct Og {
        url: Option<String>,
        secure: Option<String>,
        kind: Option<String>,
    }
    let mut og: Vec<Og> = Vec::new();
    for (key, value) in &metas {
        let value = Some(value.clone());
        match key.as_str() {
            "og:video" | "og:video:url" => match og.last_mut() {
                Some(last) if last.url.is_none() => last.url = value,
                _ => og.push(Og {
                    url: value,
                    ..Og::default()
                }),
            },
            "og:video:secure_url" | "og:video:type" => {
                if og.is_empty() {
                    og.push(Og::default());
                }
                if let Some(last) = og.last_mut() {
                    match key.as_str() {
                        "og:video:secure_url" => last.secure = value,
                        _ => last.kind = value,
                    }
                }
            }
            _ => {}
        }
    }
    let title = meta("og:title");
    for v in og {
        let flash = v.kind.is_some_and(|k| {
            decode(&k)
                .trim()
                .eq_ignore_ascii_case("application/x-shockwave-flash")
        });
        let url =
            (v.secure.as_deref().and_then(address)).or_else(|| v.url.as_deref().and_then(address));
        if let (Some(url), false) = (url, flash) {
            push(&mut out.open_graph, url, title.clone(), false);
        }
    }
    if let Some(url) = metas
        .iter()
        .find(|(k, _)| k == "twitter:player")
        .and_then(|(_, v)| address(v))
    {
        let name = meta("twitter:title").or(title);
        push(&mut out.twitter, url, name, false);
    }
    out
}

/// `address` added to `list` unless it is there already (then marked main if this one is).
fn push(list: &mut Vec<DeclaredVideo>, address: Url, name: Option<String>, main: bool) {
    match list.iter_mut().find(|v| v.address == address) {
        Some(v) => v.main |= main,
        None => list.push(DeclaredVideo {
            address,
            name,
            main,
        }),
    }
}

/// `a` is the page `b`: the same address, but for a fragment and a final `/`.
fn same_page(a: &Url, b: &Url) -> bool {
    let plain = |u: &Url| {
        let mut u = u.clone();
        u.set_fragment(None);
        u.as_str().trim_end_matches('/').to_owned()
    };
    plain(a) == plain(b)
}

/// A text value as a name: references decoded (some sites write them in JSON-LD too), white
/// space collapsed; None when empty.
fn text(s: &str) -> Option<String> {
    let s = decode(s);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// A JSON-LD link: a string, the first string of a list, or a node's `@id` or `url`.
fn link_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(link_of),
        Value::Object(o) => o.get("@id").or_else(|| o.get("url")).and_then(link_of),
        _ => None,
    }
}

/// Whether node `o`'s `@type` is (or lists) `VideoObject`, in any of the ways it's written
/// (`schema:VideoObject`, `https://schema.org/VideoObject`).
fn is_video_object(o: &Node) -> bool {
    let video = |t: &Value| {
        t.as_str().is_some_and(|t| {
            t == "VideoObject" || t.ends_with(":VideoObject") || t.ends_with("/VideoObject")
        })
    };
    match o.get("@type") {
        Some(Value::Array(a)) => a.iter().any(video),
        Some(t) => video(t),
        None => false,
    }
}

/// A JSON-LD node.
type Node = serde_json::Map<String, Value>;

/// Every `VideoObject` node in `v`, in order, each with whether it is a `mainEntity` (`main`).
/// A video's own parts (its `hasPart` clips) are inside it, not more videos of the page.
fn video_objects<'a>(v: &'a Value, main: bool, out: &mut Vec<(&'a Node, bool)>) {
    match v {
        Value::Array(a) => a.iter().for_each(|v| video_objects(v, main, out)),
        Value::Object(o) if is_video_object(o) => out.push((o, main)),
        Value::Object(o) => {
            for (k, v) in o {
                video_objects(v, k == "mainEntity", out);
            }
        }
        _ => {}
    }
}

/// The insides of `page`'s `<script type="application/ld+json">` elements, outside comments.
/// A script's body is skipped as it is read: a `<script` inside one (a template, a JS string)
/// is text, not an element, and holds no declaration of the page's.
fn json_ld_blocks(page: &str) -> Vec<&str> {
    // ASCII lowercasing keeps every byte offset.
    let lower = page.to_ascii_lowercase();
    let comments = comment_spans(&lower);
    let mut out = Vec::new();
    let mut after = 0;
    for (start, tag) in tags(page, "script") {
        // A tag inside the script read last, or in a comment, is not an element.
        if start < after || comments.iter().any(|&(a, b)| a <= start && start < b) {
            continue;
        }
        let from = start + tag.len();
        let to = lower[from..]
            .find("</script")
            .map_or(page.len(), |e| from + e);
        after = (to + "</script".len()).min(page.len());
        let json_ld = attribute(tag, "type").is_some_and(|t| {
            decode(&t)
                .split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"))
        });
        if json_ld {
            out.push(&page[from..to]);
        }
    }
    out
}

/// Where `lower`'s comments are (`<!--` to past `-->`).
fn comment_spans(lower: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<!--") {
        let start = at + i;
        let end = lower[start + 4..]
            .find("-->")
            .map_or(lower.len(), |e| start + 4 + e + 3);
        out.push((start, end));
        at = end;
    }
    out
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
        // `</picture` starts with `</p`: it doesn't end the paragraph.
        let nested = r#"<p class="e">a <picture>x</picture> b</p>"#;
        assert_eq!(text_of_class(nested, "e").as_deref(), Some("a x b"));
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
    fn links_without_a_server_are_none() {
        // `ftps:` and `ftpes:` aren't schemes a browser knows: written so, they name no server.
        let page = r#"<a href="ftpes:player">a</a><a href="ftps:/x.bin">b</a>
            <a href="ftps://ftp.example.org/f.bin">c</a><a href="ftpes://ftp.example.org/g.bin">d</a>"#;
        assert_eq!(
            found(page, "https://example.com/"),
            [
                "ftps://ftp.example.org/f.bin",
                "ftpes://ftp.example.org/g.bin"
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

    fn declared(page: &str, at: &str) -> DeclaredVideos {
        declared_videos(page, &Url::parse(at).unwrap())
    }

    /// A script that builds or holds a JSON-LD block in its text (a template, a JS string) is
    /// not the page declaring that video.
    #[test]
    fn a_script_that_builds_the_block_holds_no_declaration() {
        let page = r#"<script>var t = '<script type="application/ld+json">{"@type":"VideoObject","contentUrl":"https://cdn.test/t.mp4"}</script>';</script>"#;
        assert_eq!(
            declared(page, "https://site.test/v/1"),
            DeclaredVideos::default()
        );
        // The real block after it is read as before.
        let page = format!(
            r#"{page}<script type="application/ld+json">{{"@type":"VideoObject","contentUrl":"https://cdn.test/real.mp4"}}</script>"#
        );
        assert_eq!(
            addresses(&declared(&page, "https://site.test/v/1").json_ld),
            ["https://cdn.test/real.mp4"]
        );
    }

    /// The addresses of `list`, as text.
    fn addresses(list: &[DeclaredVideo]) -> Vec<&str> {
        list.iter().map(|v| v.address.as_str()).collect()
    }

    #[test]
    fn the_film_page_declares_its_film_not_the_welcome_video() {
        // #1076: the only player in its HTML is the hidden welcome video's (689791758).
        let page = include_str!("../tests/data/declared-video.html");
        let at = "https://www.shortverse.com/films/before-the-short-film";
        let d = declared(page, at);
        assert_eq!(
            addresses(&d.json_ld),
            ["https://vimeo.com/1244858801?fl=ip&fe=ec"]
        );
        assert!(d.open_graph.is_empty() && d.twitter.is_empty());
        let main = d.main().unwrap();
        assert_eq!(main.name.as_deref(), Some("Before The - Short Film"));
        // Its `url` is the page's own.
        assert!(main.main);
    }

    #[test]
    fn json_ld_in_its_forms() {
        let at = "https://site.test/watch/1";
        // A list of nodes, and an `@graph` with the video in an article; the file before its
        // player; a relative address; a type written in full.
        let page = r#"<script type="application/ld+json">[{"@type":"WebSite","name":"Site"},
            {"@type":"https://schema.org/VideoObject","name":"First &amp; only",
             "embedUrl":"https://player.test/e/1","contentUrl":"/media/1.mp4"}]</script>"#;
        let d = declared(page, at);
        assert_eq!(addresses(&d.json_ld), ["https://site.test/media/1.mp4"]);
        assert_eq!(d.main().unwrap().name.as_deref(), Some("First & only"));
        let page = r#"<script type='application/ld+json; charset=utf-8'>{"@context":"https://schema.org",
            "@graph":[{"@type":"WebPage","@id":"https://site.test/watch/1"},
            {"@type":"NewsArticle","headline":"News","video":{"@type":["VideoObject"],
             "headline":"The clip","embedUrl":["https://player.test/e/2"]}}]}</script>"#;
        let d = declared(page, at);
        assert_eq!(addresses(&d.json_ld), ["https://player.test/e/2"]);
        assert_eq!(d.main().unwrap().name.as_deref(), Some("The clip"));
        // Several: none is the page's, unless one is its main entity, or its `url` is the page.
        let several = r#"<script type="application/ld+json">{"@type":"ItemList","itemListElement":[
            {"@type":"VideoObject","name":"A","contentUrl":"https://cdn.test/a.mp4"},
            {"@type":"VideoObject","name":"B","contentUrl":"https://cdn.test/b.mp4"}]}</script>"#;
        let d = declared(several, at);
        assert_eq!(d.json_ld.len(), 2);
        assert_eq!(d.main(), None);
        let main = r#"<script type="application/ld+json">{"@type":"WebPage","mainEntity":
            {"@type":"VideoObject","name":"Main","contentUrl":"https://cdn.test/m.mp4"},
            "relatedLink":[{"@type":"VideoObject","name":"Other","contentUrl":"https://cdn.test/o.mp4"}]}
            </script>"#;
        assert_eq!(
            declared(main, at).main().unwrap().address.as_str(),
            "https://cdn.test/m.mp4"
        );
        let own = r#"<script type="application/ld+json">[
            {"@type":"VideoObject","name":"Other","contentUrl":"https://cdn.test/o.mp4"},
            {"@type":"VideoObject","name":"Own","url":"https://site.test/watch/1/",
             "contentUrl":"https://cdn.test/own.mp4"}]</script>"#;
        assert_eq!(
            declared(own, at).main().unwrap().name.as_deref(),
            Some("Own")
        );
        // The same video twice is one; a block that isn't JSON, one in a comment, one without an
        // address, one that isn't on the web and another type are none.
        let page = r#"<script type="application/ld+json">{"@type":"VideoObject","contentUrl":"https://cdn.test/a.mp4"}</script>
            <script type="application/ld+json">{"@type":"VideoObject","contentUrl":"https://cdn.test/a.mp4#t=5"}</script>
            <script type="application/ld+json">{"@type": "VideoObject", broken</script>
            <!-- <script type="application/ld+json">{"@type":"VideoObject","contentUrl":"https://cdn.test/c.mp4"}</script> -->
            <script type="application/ld+json">{"@type":"VideoObject","name":"No address"}</script>
            <script type="application/ld+json">{"@type":"VideoObject","contentUrl":"file:///etc/passwd"}</script>
            <script type="application/json">{"@type":"VideoObject","contentUrl":"https://cdn.test/d.mp4"}</script>"#;
        assert_eq!(
            addresses(&declared(page, at).json_ld),
            ["https://cdn.test/a.mp4"]
        );
    }

    #[test]
    fn open_graph_and_twitters_card() {
        let at = "https://site.test/v/1";
        let page = r#"<head><meta property="og:title" content="A clip &amp; more | Site">
            <meta property="og:video" content="http://cdn.test/v/1.mp4">
            <meta property="og:video:secure_url" content="https://cdn.test/v/1.mp4">
            <meta property="og:video:type" content="video/mp4">
            <meta property="og:video:url" content="https://cdn.test/v/1.mp4">
            <meta name="twitter:player" content="https://site.test/embed/1"></head>"#;
        let d = declared(page, at);
        assert_eq!(addresses(&d.open_graph), ["https://cdn.test/v/1.mp4"]);
        assert_eq!(addresses(&d.twitter), ["https://site.test/embed/1"]);
        let main = d.main().unwrap();
        assert_eq!(main.address.as_str(), "https://cdn.test/v/1.mp4");
        assert_eq!(main.name.as_deref(), Some("A clip & more | Site"));
        // A Flash video isn't one; two others are several.
        let page = r#"<meta property="og:video" content="https://site.test/player.swf">
            <meta property="og:video:type" content="application/x-shockwave-flash">
            <meta property="og:video" content="https://cdn.test/a.mp4">
            <meta property="og:video" content="https://cdn.test/b.mp4">"#;
        let d = declared(page, at);
        assert_eq!(
            addresses(&d.open_graph),
            ["https://cdn.test/a.mp4", "https://cdn.test/b.mp4"]
        );
        assert_eq!(d.main(), None);
        // Twitter's card alone, named by its own title; JSON-LD comes first.
        let page = r#"<meta name="twitter:title" content="Card"><meta name="twitter:player" content="/embed/2">"#;
        let d = declared(page, at);
        let main = d.main().unwrap();
        assert_eq!(main.address.as_str(), "https://site.test/embed/2");
        assert_eq!(main.name.as_deref(), Some("Card"));
        let both = format!(
            r#"{page}<script type="application/ld+json">{{"@type":"VideoObject","contentUrl":"https://cdn.test/ld.mp4"}}</script>"#
        );
        assert_eq!(
            declared(&both, at).main().unwrap().address.as_str(),
            "https://cdn.test/ld.mp4"
        );
        // Meta tags in a template or a comment aren't the page's.
        let page = r#"<script type="text/x-template"><meta property="og:video" content="https://cdn.test/t.mp4"></script>
            <!-- <meta property="og:video" content="https://cdn.test/c.mp4"> -->"#;
        assert_eq!(declared(page, at), DeclaredVideos::default());
    }

    #[test]
    fn a_page_without_a_declaration() {
        let page = r#"<title>A page</title><video src="/v.mp4"></video>
            <iframe src="https://player.vimeo.com/video/1"></iframe>"#;
        let d = declared(page, "https://site.test/");
        assert_eq!(d, DeclaredVideos::default());
        assert_eq!(d.main(), None);
    }
}
