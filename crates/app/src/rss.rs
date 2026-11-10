//! RSS feeds with auto-download rules (#39; beyond IDM). A feed is fetched every so often (RSS
//! 2.0 or Atom); its articles are listed in the RSS feeds window, where any of them can be
//! downloaded by hand; and each article new since the last fetch goes through the download
//! rules: one a rule takes is added as a torrent, to the rule's folder, at once.
//!
//! A rule: words the title must contain (all of them; `|` between alternatives; `*` and `?`
//! wildcards) and must not contain, or regular expressions; an episode filter (`1x2;1x5-8;2x1-`)
//! read from `S01E02` or `1x02` in the title; and no episode twice. Everything is kept in
//! `rss.json` in the data folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The most a feed may be: a feed of a few hundred articles is well under a megabyte.
pub const MAX_FEED: u64 = 5 << 20;
/// Articles kept per feed (the newest).
pub const KEEP_ARTICLES: usize = 200;
/// Article ids remembered per feed as already seen (more than are kept: a feed that drops an
/// article and lists it again doesn't make it new).
const KEEP_SEEN: usize = 1000;
/// How often feeds are fetched by default, minutes.
pub const DEFAULT_MINUTES: u32 = 30;

/// One article of a feed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    /// Its guid (or Atom id), else its torrent's address, else its title: what makes it the same
    /// article from one fetch to the next.
    pub id: String,
    pub title: String,
    /// The torrent: a magnet link or a .torrent's address. Empty: the article has none.
    pub torrent: String,
    /// Its page, when it has one besides the torrent.
    #[serde(default)]
    pub page: String,
    /// Its date as the feed wrote it (RFC 822 or RFC 3339), shown as it is.
    #[serde(default)]
    pub date: String,
    /// The torrent's size, when the feed says it.
    #[serde(default)]
    pub size: Option<u64>,
}

/// What a feed holds: its title and its articles, in the feed's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub articles: Vec<Article>,
}

fn is_torrent_link(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("magnet:?")
        || ((l.starts_with("http://") || l.starts_with("https://"))
            && l.split(['?', '#'])
                .next()
                .is_some_and(|p| p.ends_with(".torrent")))
}

/// Read an RSS 2.0 or Atom feed. roxmltree refuses DTDs (no entity expansion), and its node
/// count is capped, so a hostile feed can't blow up.
pub fn parse(xml: &str) -> Result<Parsed, String> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 200_000,
    };
    let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let local = |n: &roxmltree::Node| n.tag_name().name().to_ascii_lowercase();
    let text_of = |n: &roxmltree::Node, name: &str| -> String {
        n.children()
            .find(|c| c.is_element() && local(c) == name)
            .map(|c| c.text().unwrap_or("").trim().to_owned())
            .unwrap_or_default()
    };
    let mut out = Parsed::default();
    match local(&root).as_str() {
        "rss" | "rdf" => {
            let channel = root
                .children()
                .find(|c| c.is_element() && local(c) == "channel")
                .ok_or("an RSS feed without a channel")?;
            out.title = text_of(&channel, "title");
            // RSS 1.0 (RDF) puts its items beside the channel; 2.0 inside it.
            let items = channel
                .children()
                .chain(root.children())
                .filter(|c| c.is_element() && local(c) == "item");
            for item in items {
                let title = text_of(&item, "title");
                let link = text_of(&item, "link");
                let guid = text_of(&item, "guid");
                // The torrent: an enclosure that is one, the ezRSS / Torznab magnet fields,
                // else the link when it is one.
                let mut torrent = String::new();
                let mut size = None;
                for c in item.children().filter(|c| c.is_element()) {
                    let name = local(&c);
                    if name == "enclosure" {
                        let url = c.attribute("url").unwrap_or("").trim();
                        let kind = c.attribute("type").unwrap_or("");
                        if torrent.is_empty()
                            && (kind.eq_ignore_ascii_case("application/x-bittorrent")
                                || is_torrent_link(url))
                        {
                            torrent = url.to_owned();
                            size = c.attribute("length").and_then(|l| l.trim().parse().ok());
                        }
                    } else if name == "magneturi" && torrent.is_empty() {
                        torrent = c.text().unwrap_or("").trim().to_owned();
                    } else if name == "contentlength" && size.is_none() {
                        size = c.text().and_then(|t| t.trim().parse().ok());
                    } else if name == "attr"
                        && c.attribute("name") == Some("magneturl")
                        && torrent.is_empty()
                    {
                        torrent = c.attribute("value").unwrap_or("").trim().to_owned();
                    } else if name == "attr"
                        && c.attribute("name") == Some("size")
                        && size.is_none()
                    {
                        size = c.attribute("value").and_then(|v| v.trim().parse().ok());
                    }
                }
                if torrent.is_empty() && is_torrent_link(&link) {
                    torrent = link.clone();
                }
                let page = if link != torrent { link } else { String::new() };
                let date = match text_of(&item, "pubdate") {
                    d if d.is_empty() => text_of(&item, "date"),
                    d => d,
                };
                push(&mut out, guid, title, torrent, page, date, size);
            }
        }
        "feed" => {
            out.title = text_of(&root, "title");
            for entry in root
                .children()
                .filter(|c| c.is_element() && local(c) == "entry")
            {
                let mut torrent = String::new();
                let mut page = String::new();
                let mut size = None;
                for l in entry
                    .children()
                    .filter(|c| c.is_element() && local(c) == "link")
                {
                    let href = l.attribute("href").unwrap_or("").trim();
                    let rel = l.attribute("rel").unwrap_or("alternate");
                    let kind = l.attribute("type").unwrap_or("");
                    if torrent.is_empty()
                        && (kind.eq_ignore_ascii_case("application/x-bittorrent")
                            || (rel == "enclosure" && is_torrent_link(href))
                            || is_torrent_link(href))
                    {
                        torrent = href.to_owned();
                        size = l.attribute("length").and_then(|v| v.trim().parse().ok());
                    } else if rel == "alternate" && page.is_empty() {
                        page = href.to_owned();
                    }
                }
                let date = match text_of(&entry, "updated") {
                    d if d.is_empty() => text_of(&entry, "published"),
                    d => d,
                };
                push(
                    &mut out,
                    text_of(&entry, "id"),
                    text_of(&entry, "title"),
                    torrent,
                    page,
                    date,
                    size,
                );
            }
        }
        other => return Err(format!("not an RSS or Atom feed (<{other}>)")),
    }
    Ok(out)
}

fn push(
    out: &mut Parsed,
    id: String,
    title: String,
    torrent: String,
    page: String,
    date: String,
    size: Option<u64>,
) {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let id = [&id, &torrent, &page, &title]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default();
    if id.is_empty() {
        return;
    }
    out.articles.push(Article {
        id,
        title,
        torrent,
        page,
        date,
        size,
    });
}

/// A feed the person subscribed to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub id: u64,
    pub url: String,
    /// The feed's own title (from its last fetch).
    #[serde(default)]
    pub title: String,
    /// Its articles, newest first (at most `KEEP_ARTICLES`).
    #[serde(default)]
    pub articles: Vec<Article>,
    /// The ids of the articles already seen, newest first: only the others go through the
    /// rules. Empty with `fetched` None: never fetched (its first articles are the backlog).
    #[serde(default)]
    pub seen: Vec<String>,
    /// When it was last fetched (Unix seconds), and why that failed if it did.
    #[serde(default)]
    pub fetched: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Feed {
    /// The name the window shows: its title, else its address.
    pub fn name(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            self.title.trim()
        }
    }

    /// Take a fetch's articles: kept newest first, and the ones never seen before returned
    /// (none the first time: a feed's first articles are its backlog, not news).
    pub fn take(&mut self, parsed: Parsed, now: i64) -> Vec<Article> {
        let first = self.fetched.is_none();
        self.fetched = Some(now);
        self.error = None;
        if !parsed.title.trim().is_empty() {
            self.title = parsed.title.trim().to_owned();
        }
        let seen: HashSet<&str> = self.seen.iter().map(String::as_str).collect();
        let fresh: Vec<Article> = parsed
            .articles
            .iter()
            .filter(|a| !seen.contains(a.id.as_str()))
            .cloned()
            .collect();
        let mut ids: Vec<String> = fresh.iter().map(|a| a.id.clone()).collect();
        ids.append(&mut self.seen);
        ids.truncate(KEEP_SEEN);
        self.seen = ids;
        let known: HashSet<String> = parsed.articles.iter().map(|a| a.id.clone()).collect();
        let mut kept = parsed.articles;
        kept.extend(self.articles.drain(..).filter(|a| !known.contains(&a.id)));
        kept.truncate(KEEP_ARTICLES);
        self.articles = kept;
        if first { Vec::new() } else { fresh }
    }
}

/// A download rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    /// What the title must contain, and must not: words (all of them), `|` between
    /// alternatives, `*` and `?` wildcards; or regular expressions (`regex`). Case is ignored.
    #[serde(default)]
    pub must: String,
    #[serde(default)]
    pub must_not: String,
    #[serde(default)]
    pub regex: bool,
    /// Episodes to take: `1x2;1x5-8;2x1-` (season x episode, a range, or from there on).
    /// Empty: any article, episode or not.
    #[serde(default)]
    pub episodes: String,
    /// The feeds it watches (ids); empty: every feed.
    #[serde(default)]
    pub feeds: Vec<u64>,
    /// Where its torrents go; empty: the Torrents category's folder.
    #[serde(default)]
    pub folder: String,
    /// Add them without starting (Download later).
    #[serde(default)]
    pub later: bool,
    /// The episodes it has already added (`S1E2`): each is taken once, whichever release.
    #[serde(default)]
    pub done: Vec<String>,
}

impl Rule {
    pub fn new(id: u64, name: &str) -> Self {
        Rule {
            id,
            name: name.to_owned(),
            enabled: true,
            must: String::new(),
            must_not: String::new(),
            regex: false,
            episodes: String::new(),
            feeds: Vec::new(),
            folder: String::new(),
            later: false,
            done: Vec::new(),
        }
    }

    /// Whether it watches feed `feed`.
    pub fn watches(&self, feed: u64) -> bool {
        self.feeds.is_empty() || self.feeds.contains(&feed)
    }

    /// Whether `title` passes its words (or expressions) and its episode filter. An episode it
    /// already took doesn't pass again.
    pub fn takes(&self, title: &str) -> Result<bool, String> {
        if !self.words_match(title)? {
            return Ok(false);
        }
        let ep = episode(title);
        if let Some((s, e)) = ep
            && self.done.contains(&episode_key(s, e))
        {
            return Ok(false);
        }
        if self.episodes.trim().is_empty() {
            return Ok(true);
        }
        let filter = parse_episodes(&self.episodes)?;
        Ok(ep.is_some_and(|(s, e)| filter.iter().any(|r| r.contains(s, e))))
    }

    fn words_match(&self, title: &str) -> Result<bool, String> {
        if self.regex {
            let re = |p: &str| {
                regex::RegexBuilder::new(p)
                    .case_insensitive(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| e.to_string())
            };
            let must = self.must.trim().is_empty() || re(self.must.trim())?.is_match(title);
            let not = !self.must_not.trim().is_empty() && re(self.must_not.trim())?.is_match(title);
            return Ok(must && !not);
        }
        let t = title.to_lowercase();
        let must = self.must.trim().is_empty() || any_group(&self.must, &t);
        let not = !self.must_not.trim().is_empty() && any_group(&self.must_not, &t);
        Ok(must && !not)
    }

    /// Note an episode it added (so a second release of it isn't taken).
    pub fn took(&mut self, title: &str) {
        if let Some((s, e)) = episode(title) {
            let key = episode_key(s, e);
            if !self.done.contains(&key) {
                self.done.push(key);
            }
        }
    }
}

/// `a b|c` on a lower-case title: every word of one alternative is in it.
fn any_group(spec: &str, title: &str) -> bool {
    spec.to_lowercase().split('|').any(|alt| {
        let words: Vec<&str> = alt.split_whitespace().collect();
        !words.is_empty() && words.iter().all(|w| contains_wild(title, w))
    })
}

/// Whether `text` holds `pat` somewhere, `*` matching any run and `?` any one character.
fn contains_wild(text: &str, pat: &str) -> bool {
    if !pat.contains(['*', '?']) {
        return text.contains(pat);
    }
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    (0..=t.len()).any(|start| wild_prefix(&t[start..], &p))
}

/// Whether `p` matches a prefix of `t`.
fn wild_prefix(t: &[char], p: &[char]) -> bool {
    match p.first() {
        None => true,
        Some('*') => (0..=t.len()).any(|k| wild_prefix(&t[k..], &p[1..])),
        Some('?') => !t.is_empty() && wild_prefix(&t[1..], &p[1..]),
        Some(c) => t.first() == Some(c) && wild_prefix(&t[1..], &p[1..]),
    }
}

/// The season and episode in a title: `S01E02`, `s1e2` or `1x02`.
pub fn episode(title: &str) -> Option<(u32, u32)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:\bS(\d{1,4})[ ._-]?E(\d{1,4})|\b(\d{1,2})x(\d{1,3})\b)")
            .expect("valid")
    });
    let c = re.captures(title)?;
    let num = |i: usize| c.get(i).and_then(|m| m.as_str().parse().ok());
    num(1).zip(num(2)).or_else(|| num(3).zip(num(4)))
}

fn episode_key(s: u32, e: u32) -> String {
    format!("S{s}E{e}")
}

/// One part of an episode filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    season: u32,
    from: u32,
    /// None: to the season's end, and every later season too (`2x5-`).
    to: Option<u32>,
}

impl Range {
    fn contains(&self, s: u32, e: u32) -> bool {
        match self.to {
            None => s > self.season || (s == self.season && e >= self.from),
            Some(to) => s == self.season && (self.from..=to).contains(&e),
        }
    }
}

/// `1x2;1x5-8;2x1-` (a lone season, `3`, is all of it).
fn parse_episodes(spec: &str) -> Result<Vec<Range>, String> {
    let bad = |p: &str| format!("not an episode filter: {p}");
    let mut out = Vec::new();
    for part in spec.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (season, eps) = match part.split_once(['x', 'X']) {
            Some((s, e)) => (s, e.trim()),
            None => (part, "1-"),
        };
        let season: u32 = season.trim().parse().map_err(|_| bad(part))?;
        let num = |s: &str| s.trim().parse::<u32>().map_err(|_| bad(part));
        let range = match eps.split_once('-') {
            None => {
                let e = num(eps)?;
                Range {
                    season,
                    from: e,
                    to: Some(e),
                }
            }
            Some((a, b)) if b.trim().is_empty() => Range {
                season,
                from: num(a)?,
                to: None,
            },
            Some((a, b)) => {
                let (from, to) = (num(a)?, num(b)?);
                if to < from {
                    return Err(bad(part));
                }
                Range {
                    season,
                    from,
                    to: Some(to),
                }
            }
        };
        out.push(range);
    }
    Ok(out)
}

/// Whether an episode filter can be read (the rule editor says so as it is typed).
pub fn episodes_ok(spec: &str) -> bool {
    parse_episodes(spec).is_ok()
}

/// Everything RSS keeps (`rss.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub feeds: Vec<Feed>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// How often feeds are fetched, minutes.
    #[serde(default = "default_minutes")]
    pub minutes: u32,
    #[serde(default)]
    next_id: u64,
    /// What the rules did lately (Unix seconds, text), newest last: shown under the rules.
    #[serde(default)]
    pub log: Vec<(i64, String)>,
}

fn default_minutes() -> u32 {
    DEFAULT_MINUTES
}

/// `rss.json` written as `json` through a temporary file ([`Store::encode`]).
pub fn write(path: &Path, json: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

impl Default for Store {
    fn default() -> Self {
        Store {
            feeds: Vec::new(),
            rules: Vec::new(),
            minutes: DEFAULT_MINUTES,
            next_id: 1,
            log: Vec::new(),
        }
    }
}

impl Store {
    pub fn path() -> Option<PathBuf> {
        mdm_ipc::data_dir().map(|d| d.join("rss.json"))
    }

    /// The saved one; a missing or unreadable file is an empty store (a broken one is kept
    /// aside as `rss.json.bad`, not overwritten).
    pub fn load(path: &Path) -> Store {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Store::default();
        };
        match serde_json::from_str::<Store>(&text) {
            Ok(mut s) => {
                let top = s
                    .feeds
                    .iter()
                    .map(|f| f.id)
                    .chain(s.rules.iter().map(|r| r.id))
                    .max()
                    .unwrap_or(0);
                s.next_id = s.next_id.max(top + 1);
                s.minutes = s.minutes.max(5);
                s
            }
            Err(_) => {
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                Store::default()
            }
        }
    }

    /// As `rss.json` holds it ([`write`]; the window hands it to a saver thread: app.rs).
    pub fn encode(&self) -> std::io::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(std::io::Error::other)
    }

    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    /// Subscribe to `url` (an http(s) address; one already there isn't added twice).
    pub fn add_feed(&mut self, url: &str) -> Result<u64, String> {
        let url = url.trim();
        let parsed = url::Url::parse(url).map_err(|_| format!("not a web address: {url}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("not a web address: {url}"));
        }
        if let Some(f) = self.feeds.iter().find(|f| f.url == url) {
            return Ok(f.id);
        }
        let id = self.new_id();
        self.feeds.push(Feed {
            id,
            url: url.to_owned(),
            ..Default::default()
        });
        Ok(id)
    }

    /// Forget a feed (and take it out of the rules that named it).
    pub fn remove_feed(&mut self, id: u64) {
        self.feeds.retain(|f| f.id != id);
        for r in &mut self.rules {
            r.feeds.retain(|f| *f != id);
        }
    }

    /// The feeds due for a fetch at `now` (Unix seconds).
    pub fn due(&self, now: i64) -> Vec<u64> {
        let every = i64::from(self.minutes.max(5)) * 60;
        self.feeds
            .iter()
            .filter(|f| f.fetched.is_none_or(|t| now - t >= every || now < t))
            .map(|f| f.id)
            .collect()
    }

    /// A fetch of feed `id` came back: its new articles go through the enabled rules that
    /// watch it. What the rules take, each article once (the first rule that takes it), and
    /// its episode noted on the rule; a rule that can't be read is skipped.
    pub fn fetched(&mut self, id: u64, parsed: Parsed, now: i64) -> Vec<(u64, Article)> {
        let Some(feed) = self.feeds.iter_mut().find(|f| f.id == id) else {
            return Vec::new();
        };
        let fresh = feed.take(parsed, now);
        let mut out = Vec::new();
        for a in fresh.into_iter().filter(|a| !a.torrent.is_empty()) {
            let rule = self
                .rules
                .iter_mut()
                .filter(|r| r.enabled && r.watches(id))
                .find(|r| r.takes(&a.title).unwrap_or(false));
            if let Some(r) = rule {
                r.took(&a.title);
                out.push((r.id, a));
            }
        }
        out
    }

    /// Note what a rule did (the last 50 are kept).
    pub fn note(&mut self, now: i64, text: String) {
        self.log.push((now, text));
        let over = self.log.len().saturating_sub(50);
        self.log.drain(..over);
    }

    /// A fetch of feed `id` failed.
    pub fn failed(&mut self, id: u64, why: String, now: i64) {
        if let Some(f) = self.feeds.iter_mut().find(|f| f.id == id) {
            f.fetched = Some(now);
            f.error = Some(why);
        }
    }

    /// The articles of the feeds rule `r` watches that it would take now (the rule editor's
    /// preview), newest first.
    pub fn preview(&self, r: &Rule) -> Vec<(String, Article)> {
        self.feeds
            .iter()
            .filter(|f| r.watches(f.id))
            .flat_map(|f| f.articles.iter().map(move |a| (f.name().to_owned(), a)))
            .filter(|(_, a)| r.takes(&a.title).unwrap_or(false))
            .map(|(n, a)| (n, a.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:torrent="http://xmlns.ezrss.it/0.1/">
 <channel><title> Open Shows </title>
  <item><title>Open Show S01E01 1080p</title><guid>a1</guid>
   <enclosure url="http://127.0.0.1:8899/rss/files/s01e01.torrent" type="application/x-bittorrent" length="1234"/>
   <link>http://127.0.0.1:8899/page/1</link><pubDate>Thu, 05 Nov 2026 10:00:00 GMT</pubDate></item>
  <item><title>Open   Show 1x02 720p</title>
   <torrent:magnetURI>magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567</torrent:magnetURI></item>
  <item><title>No torrent here</title><link>http://127.0.0.1:8899/page/3</link></item>
 </channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom shows</title>
 <entry><id>tag:x,1</id><title>Other Show S02E10</title><updated>2026-11-05T10:00:00Z</updated>
  <link rel="alternate" href="http://127.0.0.1:8899/page/9"/>
  <link rel="enclosure" type="application/x-bittorrent" href="http://127.0.0.1:8899/rss/files/o.torrent" length="99"/></entry>
</feed>"#;

    #[test]
    fn rss_and_atom_are_read() {
        let p = parse(RSS).unwrap();
        assert_eq!(p.title, "Open Shows");
        assert_eq!(p.articles.len(), 3);
        let a = &p.articles[0];
        assert_eq!(
            (a.id.as_str(), a.title.as_str(), a.size),
            ("a1", "Open Show S01E01 1080p", Some(1234))
        );
        assert_eq!(a.torrent, "http://127.0.0.1:8899/rss/files/s01e01.torrent");
        assert_eq!(a.page, "http://127.0.0.1:8899/page/1");
        assert_eq!(p.articles[1].title, "Open Show 1x02 720p", "spaces folded");
        assert!(p.articles[1].torrent.starts_with("magnet:?"));
        assert_eq!(
            p.articles[1].id, p.articles[1].torrent,
            "no guid: its torrent"
        );
        assert!(p.articles[2].torrent.is_empty());
        let p = parse(ATOM).unwrap();
        assert_eq!(p.title, "Atom shows");
        assert_eq!(
            p.articles[0].torrent,
            "http://127.0.0.1:8899/rss/files/o.torrent"
        );
        assert_eq!(p.articles[0].page, "http://127.0.0.1:8899/page/9");
        assert_eq!(p.articles[0].size, Some(99));
        assert!(parse("<html><body/></html>").is_err());
        assert!(parse("not xml").is_err());
        // An entity bomb is refused, not expanded.
        let bomb = r#"<?xml version="1.0"?><!DOCTYPE r [<!ENTITY a "aaaa"><!ENTITY b "&a;&a;&a;">]><rss><channel><title>&b;</title></channel></rss>"#;
        assert!(parse(bomb).is_err());
    }

    #[test]
    fn rules_match_words_wildcards_and_regexes() {
        let mut r = Rule::new(1, "x");
        r.must = "open show 1080p|open show 2160p".into();
        r.must_not = "cam|sample".into();
        assert!(r.takes("open show s01e01 1080p").unwrap());
        assert!(!r.takes("Open Show S01E01 720p").unwrap());
        assert!(!r.takes("Open Show S01E01 1080p CAM").unwrap());
        r.must = "op?n sh*w".into();
        r.must_not.clear();
        assert!(r.takes("OPEN SHOW 1x01").unwrap());
        assert!(r.takes("Opan Shoooow").unwrap());
        assert!(!r.takes("Other Show").unwrap());
        r.regex = true;
        r.must = r"^open show s\d+e\d+".into();
        r.must_not = "720p$".into();
        assert!(r.takes("Open Show S01E02 1080p").unwrap());
        assert!(!r.takes("Open Show S01E02 720p").unwrap());
        r.must = "(".into();
        assert!(r.takes("x").is_err(), "a broken expression says so");
    }

    #[test]
    fn episodes_are_filtered_and_taken_once() {
        assert_eq!(episode("Show S01E02 x"), Some((1, 2)));
        assert_eq!(episode("show.s3.e14"), Some((3, 14)));
        assert_eq!(episode("show s3e14"), Some((3, 14)));
        assert_eq!(episode("Show 2x05 HDTV"), Some((2, 5)));
        assert_eq!(
            episode("Film 1920x1080"),
            None,
            "a picture size is no episode"
        );
        assert_eq!(episode("Show 2026"), None);
        let mut r = Rule::new(1, "x");
        r.episodes = "1x2; 1x5-8; 3x4-".into();
        let ok = |r: &Rule, t: &str| r.takes(t).unwrap();
        assert!(ok(&r, "S01E02") && !ok(&r, "S01E03") && ok(&r, "S01E06"));
        assert!(!ok(&r, "S01E09") && !ok(&r, "S02E01") && !ok(&r, "S03E03"));
        assert!(ok(&r, "S03E04") && ok(&r, "S04E01"), "from there on");
        assert!(!ok(&r, "no episode in this title"));
        r.took("S01E02 1080p");
        assert!(!ok(&r, "S01E02 720p REPACK"), "each episode once");
        assert!(!episodes_ok("1x") && !episodes_ok("1x5-2") && !episodes_ok("a"));
        assert!(episodes_ok("") && episodes_ok("2") && episodes_ok("1x1;"));
        r.episodes = "2".into();
        assert!(ok(&r, "S02E09") && ok(&r, "S03E01"));
    }

    #[test]
    fn only_new_articles_go_through_the_rules() {
        let mut s = Store::default();
        let f = s.add_feed("http://127.0.0.1:8899/rss/shows.xml").unwrap();
        assert_eq!(
            s.add_feed("http://127.0.0.1:8899/rss/shows.xml").unwrap(),
            f
        );
        assert!(s.add_feed("ftp://x/y").is_err() && s.add_feed("nope").is_err());
        let mut r = Rule::new(s.new_id(), "Open Show");
        r.must = "open show".into();
        s.rules.push(r);
        let art = |id: &str, title: &str| Article {
            id: id.into(),
            title: title.into(),
            torrent: format!("magnet:?xt=urn:btih:{id}"),
            ..Default::default()
        };
        let feed = |v: Vec<Article>| Parsed {
            title: "Shows".into(),
            articles: v,
        };
        // The first fetch is the backlog: nothing taken.
        assert!(
            s.fetched(f, feed(vec![art("1", "Open Show S01E01")]), 100)
                .is_empty()
        );
        assert_eq!(s.due(100 + 60), Vec::<u64>::new());
        assert_eq!(s.due(100 + 30 * 60), vec![f]);
        // Then only what is new, each episode once.
        let got = s.fetched(
            f,
            feed(vec![
                art("3", "Open Show S01E02 720p"),
                art("2", "Open Show S01E02 1080p"),
                art("4", "Other Show S01E02"),
                art("1", "Open Show S01E01"),
            ]),
            200,
        );
        let ids: Vec<&str> = got.iter().map(|(_, a)| a.id.as_str()).collect();
        assert_eq!(ids, ["3"]);
        assert_eq!(s.feeds[0].articles.len(), 4);
        assert_eq!(s.feeds[0].title, "Shows");
        assert!(
            s.fetched(f, feed(vec![art("3", "Open Show S01E02 720p")]), 300)
                .is_empty()
        );
        // A rule watching another feed, or disabled, takes nothing.
        s.rules[0].feeds = vec![f + 100];
        assert!(
            s.fetched(f, feed(vec![art("5", "Open Show S01E03")]), 400)
                .is_empty()
        );
        s.rules[0].feeds.clear();
        s.rules[0].enabled = false;
        assert!(
            s.fetched(f, feed(vec![art("6", "Open Show S01E04")]), 500)
                .is_empty()
        );
        // The preview lists what it would take among the articles kept.
        s.rules[0].enabled = true;
        let pv = s.preview(&s.rules[0].clone());
        assert!(pv.iter().any(|(_, a)| a.id == "6") && pv.iter().all(|(_, a)| a.id != "4"));
        // Saved and read back; removing a feed takes it out of the rules.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rss.json");
        write(&path, &s.encode().unwrap()).unwrap();
        let back = Store::load(&path);
        assert_eq!(back, s);
        let mut back = back;
        back.rules[0].feeds = vec![f];
        back.remove_feed(f);
        assert!(back.feeds.is_empty() && back.rules[0].feeds.is_empty());
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(Store::load(&path), Store::default());
        assert!(path.with_extension("json.bad").exists(), "kept aside");
    }
}
