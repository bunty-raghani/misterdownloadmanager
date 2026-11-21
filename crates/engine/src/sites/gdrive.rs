//! Google Drive and Google Docs links (#34). A Drive link is the viewer's address, not the
//! file's: the file comes from `drive.usercontent.google.com/download`, after a "can't scan this
//! file for viruses" page for large files; Docs, Sheets and Slides come as an export (an Office
//! file). Pure functions over addresses and page text, asked by `download::get` through `sites`.
//!
//! Learned from Google's public answers (checked without signing in) and the open-source gdown
//! and yt-dlp, which were read, not copied. Pages are matched on their structure (the form's
//! `id`, hidden inputs, the `uc-error-*` classes, the address an answer ends at), never on their
//! words, which Google translates.

use url::Url;

use super::{PageSays, Refusal};
use crate::html;

/// Drive's own hosts: links, the file host, and Docs.
const DRIVE: &str = "drive.google.com";
const DOCS: &str = "docs.google.com";
const USERCONTENT: &str = "drive.usercontent.google.com";
const HOSTS: [&str; 3] = [DRIVE, DOCS, USERCONTENT];
/// Where Google sends a visitor who must sign in.
const SIGN_IN: &str = "accounts.google.com";
/// The longest Drive message kept (its error pages say a paragraph at most).
const MESSAGE_MAX: usize = 300;

/// The User-Agent Drive's hosts are asked with: a current browser's, as a browser asks them.
/// Google's file host answers MDM's default (D-007's Internet Explorer 11 agent) with "500
/// Internal Server Error" for every file, so the maintainer decided (PR #67): Drive's three hosts
/// get a current Chrome-style agent; every other site keeps D-007's, and an agent the browser
/// handed over (the extension's downloads) wins everywhere (`download::request_agent`). Keep the
/// version a recent stable Chrome's.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// What a Drive link names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An uploaded file (downloaded as it was uploaded).
    File,
    /// A Google Docs, Sheets, Slides or Drawings file (exported).
    Doc(DocKind),
    /// A folder.
    Folder,
}

/// A Google editors' file, exported as an Office file (Drawings as PNG).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Document,
    Spreadsheet,
    Presentation,
    Drawing,
}

impl DocKind {
    /// The first path segment of its address on docs.google.com.
    fn path(self) -> &'static str {
        match self {
            DocKind::Document => "document",
            DocKind::Spreadsheet => "spreadsheets",
            DocKind::Presentation => "presentation",
            DocKind::Drawing => "drawings",
        }
    }

    fn from_path(s: &str) -> Option<DocKind> {
        Some(match s {
            "document" => DocKind::Document,
            "spreadsheets" => DocKind::Spreadsheet,
            "presentation" => DocKind::Presentation,
            "drawings" => DocKind::Drawing,
            _ => return None,
        })
    }

    /// The export format asked for: the Office one (a format picker can come later).
    fn format(self) -> &'static str {
        match self {
            DocKind::Document => "docx",
            DocKind::Spreadsheet => "xlsx",
            DocKind::Presentation => "pptx",
            DocKind::Drawing => "png",
        }
    }
}

/// A recognised Drive or Docs link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub kind: Kind,
    /// `resourcekey=`: older shared links need it.
    pub resource_key: Option<String>,
    /// Which signed-in account (`authuser=`, or the `N` of `/u/N/`).
    pub authuser: Option<String>,
}

/// One of Drive's own hosts (an exact name, never a suffix: `drive.google.com.example` isn't),
/// over http(s).
pub fn is_drive_host(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|h| HOSTS.iter().any(|d| h.eq_ignore_ascii_case(d)))
}

fn host_is(url: &Url, host: &str) -> bool {
    url.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host))
}

/// Drive's IDs: letters, digits, `_` and `-`, 25 or more.
pub fn is_id(s: &str) -> bool {
    s.len() >= 25
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The query's first non-empty `key`.
fn query(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.into_owned())
}

/// The query's `id=`, when there is exactly one and it is an ID.
fn query_id(url: &Url) -> Option<String> {
    let mut ids = url.query_pairs().filter(|(k, _)| k == "id");
    let (_, id) = ids.next()?;
    (ids.next().is_none() && is_id(&id)).then(|| id.into_owned())
}

/// `segs` without a leading `u/N` (an account number), and N.
fn account<'a, 'b>(segs: &'a [&'b str]) -> (Option<&'b str>, &'a [&'b str]) {
    match segs {
        ["u", n, rest @ ..] if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (Some(n), rest)
        }
        _ => (None, segs),
    }
}

/// Recognise a Drive or Docs link. None: not one, or already a file's address (an export, a
/// published copy, the browser's own confirmed download link), which is downloaded as it is.
pub fn parse(url: &Url) -> Option<Link> {
    if !is_drive_host(url) {
        return None;
    }
    let (id, kind, user) = locate(url)?;
    Some(Link {
        id,
        kind,
        resource_key: query(url, "resourcekey"),
        authuser: query(url, "authuser").or(user),
    })
}

/// A Drive link's ID, what it names, and the account in its path (`parse`).
fn locate(url: &Url) -> Option<(String, Kind, Option<String>)> {
    let mut segs: Vec<&str> = url.path_segments()?.collect();
    if segs.last() == Some(&"") {
        segs.pop();
    }
    let link =
        |id: &str, kind, user: Option<&str>| Some((id.to_owned(), kind, user.map(str::to_owned)));
    // Folders.
    if host_is(url, DRIVE) {
        if let ["drive", rest @ ..] = segs.as_slice() {
            let (user, rest) = account(rest);
            let rest = rest.strip_prefix(&["mobile"]).unwrap_or(rest);
            if let ["folders", id, ..] = rest
                && is_id(id)
            {
                return link(id, Kind::Folder, user);
            }
            return None;
        }
        if let ["folderview" | "embeddedfolderview"] = segs.as_slice() {
            return query_id(url).and_then(|id| link(&id, Kind::Folder, None));
        }
    }
    // Docs, Sheets, Slides, Drawings (their export and published addresses aren't links).
    if host_is(url, DOCS)
        && let Some((first, rest)) = segs.split_first()
        && let Some(doc) = DocKind::from_path(first)
    {
        let (user, rest) = account(rest);
        return match rest {
            ["d", id]
            | [
                "d",
                id,
                "edit" | "view" | "htmlview" | "preview" | "copy" | "mobilebasic",
            ] if is_id(id) => link(id, Kind::Doc(doc), user),
            _ => None,
        };
    }
    // A file by its path (the viewer).
    if (host_is(url, DRIVE) || host_is(url, DOCS))
        && let ["file", rest @ ..] = segs.as_slice()
    {
        let (user, rest) = account(rest);
        return match rest {
            ["d", id] | ["d", id, "view" | "edit" | "preview"] if is_id(id) => {
                link(id, Kind::File, user)
            }
            _ => None,
        };
    }
    // A file by its query: `uc`, `open`, `download`.
    if let ["uc" | "open" | "download"] = segs.as_slice() {
        // The browser's own link after "Download anyway": the file's address already.
        if host_is(url, USERCONTENT) && url.query_pairs().any(|(k, _)| k == "confirm") {
            return None;
        }
        return query_id(url).and_then(|id| link(&id, Kind::File, None));
    }
    None
}

/// A folder link: refused, it isn't a file (listing folders is a separate step).
pub(crate) fn not_a_file(url: &Url) -> Option<Refusal> {
    parse(url)
        .filter(|l| l.kind == Kind::Folder)
        .map(|_| Refusal::Folder)
}

/// `url` with another host and path, its scheme and port kept (production is https on 443;
/// the lab's tests are http on their own port), its query `pairs`.
fn on(url: &Url, host: &str, path: &str, pairs: &[(&str, &str)]) -> Option<Url> {
    let mut next = url.clone();
    next.set_host(Some(host)).ok()?;
    next.set_path(path);
    next.set_fragment(None);
    let _ = next.set_username("");
    let _ = next.set_password(None);
    next.set_query(None);
    if !pairs.is_empty() {
        next.query_pairs_mut().extend_pairs(pairs);
    }
    Some(next)
}

/// The address the file behind a Drive or Docs link comes from:
/// - a file: `drive.usercontent.google.com/download?id=ID&export=download&confirm=t`, the
///   confirmation given up front (any value works today), so a large file skips the virus-scan
///   page's round trip; the page's form is still followed if Google ever wants more;
/// - a Doc: its export (`/document/d/ID/export?format=docx`; Drawings `…/export/png`).
///
/// A folder isn't a file: None, and `not_a_file` refuses it.
pub(crate) fn rewrite(url: &Url) -> Option<Url> {
    let l = parse(url)?;
    let key = l.resource_key.as_deref();
    let next = match l.kind {
        Kind::File => {
            let mut pairs = vec![
                ("id", l.id.as_str()),
                ("export", "download"),
                ("confirm", "t"),
            ];
            pairs.extend(key.map(|k| ("resourcekey", k)));
            pairs.extend(l.authuser.as_deref().map(|u| ("authuser", u)));
            on(url, USERCONTENT, "/download", &pairs)?
        }
        Kind::Doc(DocKind::Drawing) => {
            let path = format!("/drawings/d/{}/export/{}", l.id, DocKind::Drawing.format());
            let pairs: Vec<_> = key.map(|k| ("resourcekey", k)).into_iter().collect();
            on(url, DOCS, &path, &pairs)?
        }
        Kind::Doc(doc) => {
            let path = format!("/{}/d/{}/export", doc.path(), l.id);
            let mut pairs = vec![("format", doc.format())];
            pairs.extend(key.map(|k| ("resourcekey", k)));
            on(url, DOCS, &path, &pairs)?
        }
        Kind::Folder => return None,
    };
    (next != *url).then_some(next)
}

/// The file ID of Drive's file address (`drive.usercontent.google.com/download?id=…`, confirmed
/// or not), or of a file link.
fn file_id(url: &Url) -> Option<(String, Option<String>)> {
    if host_is(url, USERCONTENT)
        && matches!(url.path(), "/download" | "/uc")
        && let Some(id) = query_id(url)
    {
        return Some((id, query(url, "resourcekey")));
    }
    parse(url)
        .filter(|l| l.kind == Kind::File)
        .map(|l| (l.id, l.resource_key))
}

/// Before Drive's error page for a file is reported: `drive.google.com/open?id=ID`, which leads a
/// Google Doc to its editor (Drive's file host answers a Doc's ID with its quota page) and a
/// file back to its viewer. Asked once; it goes to drive.google.com, not to the limited file.
pub(crate) fn second_look(at: &Url) -> Option<Url> {
    let (id, key) = file_id(at)?;
    let mut pairs = vec![("id", id.as_str())];
    pairs.extend(key.as_deref().map(|k| ("resourcekey", k)));
    on(at, DRIVE, "/open", &pairs)
}

/// A Drive address that should answer with a file (a link, or the file host's address).
pub(crate) fn expects_file(at: &Url) -> bool {
    file_id(at).is_some() || parse(at).is_some_and(|l| l.kind != Kind::Folder)
}

/// What Drive's page `page`, answered 200 at `at` (a Drive host), says, tried in this order:
/// 1. the virus-scan warning's `<form id="download-form">`: its action (on a Drive host only)
///    with every hidden input set on its query, each replacing the same name (the confirmation,
///    `uuid`, a signed-in user's `at` token), asked with GET as the form says;
/// 2. an older page's `href="/uc?export=download&…confirm=…"`, on docs.google.com;
/// 3. a `"downloadUrl":"https:…"` in the page's data (Drive's or `*.googleusercontent.com`);
/// 4. an error page (`uc-error-subcaption`, else `uc-error-caption`): a refusal with Google's
///    words (the quota page: "Too many users have viewed or downloaded this file recently");
/// 5. nothing known.
pub(crate) fn read_page(at: &Url, page: &str) -> PageSays {
    if let Some(next) = form_target(at, page)
        .or_else(|| old_link(at, page))
        .or_else(|| data_link(page))
    {
        return PageSays::Next(next);
    }
    match html::text_of_class(page, "uc-error-subcaption")
        .or_else(|| html::text_of_class(page, "uc-error-caption"))
    {
        Some(message) => PageSays::Refused(Refusal::Drive {
            message: shorten(message),
        }),
        None => PageSays::Nothing,
    }
}

fn shorten(s: String) -> String {
    match s.char_indices().nth(MESSAGE_MAX) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s,
    }
}

/// The virus-scan page's form, filled in (rule 1 of `read_page`).
fn form_target(at: &Url, page: &str) -> Option<Url> {
    let lower = page.to_ascii_lowercase();
    let (start, tag) = html::tags(page, "form").find(|(_, tag)| {
        html::attribute(tag, "id").is_some_and(|id| html::decode(&id) == "download-form")
    })?;
    let action = html::decode(&html::attribute(tag, "action").unwrap_or_default());
    let mut next = at.join(action.trim()).ok()?;
    // Never a form off Google's Drive.
    if !is_drive_host(&next) {
        return None;
    }
    next.set_fragment(None);
    let body = start + tag.len();
    let end = lower[body..]
        .find("</form")
        .map_or(page.len(), |e| body + e);
    let mut pairs: Vec<(String, String)> = next.query_pairs().into_owned().collect();
    for (_, input) in html::tags(&page[body..end], "input") {
        let hidden =
            html::attribute(input, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden"));
        let name = html::attribute(input, "name").map(|n| html::decode(&n));
        let Some(name) = name.filter(|n| hidden && !n.is_empty()) else {
            continue;
        };
        let value = html::decode(&html::attribute(input, "value").unwrap_or_default());
        match pairs.iter().position(|(k, _)| *k == name) {
            Some(i) => {
                pairs[i].1 = value;
                let mut seen = false;
                pairs.retain(|(k, _)| *k != name || !std::mem::replace(&mut seen, true));
            }
            None => pairs.push((name, value)),
        }
    }
    next.set_query(None);
    if !pairs.is_empty() {
        next.query_pairs_mut().extend_pairs(&pairs);
    }
    // A form that only asks for its own page again would loop.
    (!same_address(&next, at)).then_some(next)
}

/// `a` and `b` are the same address, whatever the order of their query's parameters.
fn same_address(a: &Url, b: &Url) -> bool {
    let sorted = |u: &Url| {
        let mut p: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        p.sort();
        p
    };
    let base = |u: &Url| {
        let mut u = u.clone();
        u.set_query(None);
        u.set_fragment(None);
        u
    };
    base(a) == base(b) && sorted(a) == sorted(b)
}

/// An older page's confirmed link (rule 2 of `read_page`), on docs.google.com with `at`'s scheme
/// and port.
fn old_link(at: &Url, page: &str) -> Option<Url> {
    const HREF: &str = "href=\"/uc?export=download";
    let mut rest = page;
    while let Some(i) = rest.find(HREF) {
        let value = &rest[i + "href=\"".len()..];
        rest = &rest[i + HREF.len()..];
        let Some(close) = value.find('"') else { break };
        let value = html::decode(&value[..close]);
        if value.contains("confirm=") {
            let base = on(at, DOCS, "/", &[])?;
            return base.join(&value).ok();
        }
    }
    None
}

/// A `"downloadUrl":"https:…"` in the page's data (rule 3 of `read_page`), JSON-escaped.
fn data_link(page: &str) -> Option<Url> {
    const KEY: &str = "\"downloadUrl\":\"";
    let i = page.find(KEY)? + KEY.len();
    let raw = &page[i..];
    let raw = &raw[..raw.find('"')?];
    let s = raw
        .replace("\\u003d", "=")
        .replace("\\u003D", "=")
        .replace("\\u0026", "&")
        .replace("\\/", "/");
    if !s.starts_with("https:") {
        return None;
    }
    let url = Url::parse(&s).ok()?;
    let google_content = url
        .host_str()
        .is_some_and(|h| h.to_ascii_lowercase().ends_with(".googleusercontent.com"));
    (is_drive_host(&url) || google_content).then_some(url)
}

/// An answer that is Drive's refusal by where it ended and its status:
/// - a Drive address that ended at Google's sign-in page: the file isn't shared with everyone;
/// - 401 or 403 from a Drive host: this account has no access (or downloading is turned off).
pub(crate) fn refused(asked: &Url, at: &Url, status: u16) -> Option<Refusal> {
    if is_drive_host(asked) && host_is(at, SIGN_IN) {
        return Some(Refusal::SignIn);
    }
    (is_drive_host(at) && matches!(status, 401 | 403)).then_some(Refusal::NoAccess)
}

/// The agent a request to `url` goes with when it is one of Drive's hosts (`USER_AGENT`).
pub(crate) fn user_agent(url: &Url) -> Option<&'static str> {
    is_drive_host(url).then_some(USER_AGENT)
}

/// Two of Drive's hosts on the same port: one party. The browser sends its `.google.com` sign-in
/// cookies to all three, so the cookies given for a Drive link go to the file host and to Docs.
/// The extension hands over every cookie of the link's host, so drive.google.com's own (host-only)
/// cookies reach its two siblings too: a small over-share that never leaves Drive.
/// `accounts.google.com` and `*.googleusercontent.com` (where exports come from) are never one
/// party with them: their cookies cause redirect loops there, and nothing needs them.
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    is_drive_host(a) && is_drive_host(b) && a.port_or_known_default() == b.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu";

    fn url(s: &str) -> Url {
        Url::parse(&s.replace("ID", ID)).unwrap()
    }

    fn kind(s: &str) -> Option<Kind> {
        parse(&url(s)).map(|l| {
            assert_eq!(l.id, ID, "{s}");
            l.kind
        })
    }

    #[test]
    fn every_link_form_is_recognised() {
        use DocKind::*;
        use Kind::*;
        let cases: &[(&str, Option<Kind>)] = &[
            (
                "https://drive.google.com/file/d/ID/view?usp=sharing",
                Some(File),
            ),
            ("https://drive.google.com/file/d/ID", Some(File)),
            ("https://drive.google.com/file/d/ID/", Some(File)),
            ("https://drive.google.com/file/d/ID/preview#t=1", Some(File)),
            ("https://drive.google.com/file/u/1/d/ID/view", Some(File)),
            ("https://docs.google.com/file/d/ID/edit", Some(File)),
            ("http://drive.google.com/file/d/ID/view", Some(File)),
            ("https://DRIVE.Google.com/file/d/ID/view", Some(File)),
            (
                "https://drive.google.com/uc?id=ID&export=download",
                Some(File),
            ),
            (
                "https://docs.google.com/uc?export=download&id=ID",
                Some(File),
            ),
            ("https://drive.google.com/open?id=ID", Some(File)),
            (
                "https://drive.usercontent.google.com/download?id=ID&export=download",
                Some(File),
            ),
            ("https://drive.usercontent.google.com/uc?id=ID", Some(File)),
            (
                "https://docs.google.com/document/d/ID/edit",
                Some(Doc(Document)),
            ),
            ("https://docs.google.com/document/d/ID", Some(Doc(Document))),
            (
                "https://docs.google.com/spreadsheets/u/0/d/ID/htmlview",
                Some(Doc(Spreadsheet)),
            ),
            (
                "https://docs.google.com/presentation/d/ID/preview#slide=1",
                Some(Doc(Presentation)),
            ),
            (
                "https://docs.google.com/drawings/d/ID/edit",
                Some(Doc(Drawing)),
            ),
            (
                "https://drive.google.com/drive/folders/ID?usp=sharing",
                Some(Folder),
            ),
            (
                "https://drive.google.com/drive/u/0/folders/ID",
                Some(Folder),
            ),
            (
                "https://drive.google.com/drive/mobile/folders/ID/",
                Some(Folder),
            ),
            ("https://drive.google.com/folderview?id=ID", Some(Folder)),
            (
                "https://drive.google.com/embeddedfolderview?id=ID",
                Some(Folder),
            ),
            // Already a file's address: downloaded as it is.
            (
                "https://docs.google.com/document/d/ID/export?format=pdf",
                None,
            ),
            (
                "https://docs.google.com/spreadsheets/d/ID/pub?output=csv",
                None,
            ),
            (
                "https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t&uuid=x",
                None,
            ),
            // Not Drive, or not a link to something.
            ("https://drive.google.com.evil.test/file/d/ID/view", None),
            ("https://evil.test/drive.google.com/file/d/ID/view", None),
            ("https://drive.google.com/file/d/short/view", None),
            ("https://drive.google.com/file/d/ID/view/more", None),
            ("https://drive.google.com/drive/my-drive", None),
            ("https://drive.google.com/", None),
            ("https://drive.google.com/uc?id=ID&id=ID", None),
            ("https://drive.google.com/uc?export=download", None),
            ("https://docs.google.com/forms/d/ID/viewform", None),
            ("ftp://drive.google.com/file/d/ID/view", None),
        ];
        for (link, want) in cases {
            assert_eq!(kind(link), *want, "{link}");
        }
    }

    #[test]
    fn keys_and_accounts_are_kept() {
        let l = parse(&url(
            "https://drive.google.com/file/u/2/d/ID/view?resourcekey=0-K&usp=sharing",
        ))
        .unwrap();
        assert_eq!(l.resource_key.as_deref(), Some("0-K"));
        assert_eq!(l.authuser.as_deref(), Some("2"));
        let l = parse(&url("https://drive.google.com/uc?id=ID&authuser=1")).unwrap();
        assert_eq!(l.authuser.as_deref(), Some("1"));
        assert_eq!(l.resource_key, None);
    }

    #[test]
    fn links_lead_to_the_file_keeping_scheme_and_port() {
        let at = |s: &str| rewrite(&url(s)).map(|u| u.to_string().replace(ID, "ID"));
        assert_eq!(
            at("https://drive.google.com/file/d/ID/view?usp=sharing").as_deref(),
            Some("https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t")
        );
        assert_eq!(
            at("http://drive.google.com:8080/open?id=ID&resourcekey=0-K&authuser=1").as_deref(),
            Some(
                "http://drive.usercontent.google.com:8080/download?id=ID&export=download&confirm=t&resourcekey=0-K&authuser=1"
            )
        );
        assert_eq!(
            at("https://docs.google.com/document/d/ID/edit?usp=sharing").as_deref(),
            Some("https://docs.google.com/document/d/ID/export?format=docx")
        );
        assert_eq!(
            at("https://docs.google.com/spreadsheets/d/ID/edit#gid=0").as_deref(),
            Some("https://docs.google.com/spreadsheets/d/ID/export?format=xlsx")
        );
        assert_eq!(
            at("https://docs.google.com/presentation/d/ID/edit?resourcekey=0-K").as_deref(),
            Some("https://docs.google.com/presentation/d/ID/export?format=pptx&resourcekey=0-K")
        );
        assert_eq!(
            at("https://docs.google.com/drawings/d/ID/edit").as_deref(),
            Some("https://docs.google.com/drawings/d/ID/export/png")
        );
        assert_eq!(at("https://drive.google.com/drive/folders/ID"), None);
        assert_eq!(
            not_a_file(&url("https://drive.google.com/drive/folders/ID")),
            Some(Refusal::Folder)
        );
        assert_eq!(
            not_a_file(&url("https://drive.google.com/file/d/ID/view")),
            None
        );
        // The file host's address without a confirmation gets one; with one, it is the file.
        assert!(at("https://drive.usercontent.google.com/download?id=ID").is_some());
        assert_eq!(
            at("https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t"),
            None
        );
    }

    #[test]
    fn a_doc_asked_as_a_file_is_looked_up_once() {
        let at =
            url("http://drive.usercontent.google.com:81/download?id=ID&confirm=t&resourcekey=0-K");
        assert_eq!(
            second_look(&at).unwrap().as_str(),
            format!("http://drive.google.com:81/open?id={ID}&resourcekey=0-K")
        );
        assert!(expects_file(&at));
        assert!(expects_file(&url(
            "https://docs.google.com/document/d/ID/edit"
        )));
        assert!(!expects_file(&url(
            "https://drive.google.com/drive/folders/ID"
        )));
        assert!(!expects_file(&url(
            "https://docs.google.com/forms/d/ID/viewform"
        )));
    }

    fn page(name: &str) -> String {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    fn next(at: &str, html: &str) -> Option<String> {
        match read_page(&url(at), html) {
            PageSays::Next(u) => Some(u.to_string().replace(ID, "ID")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_virus_scan_page_is_confirmed() {
        let at = "https://drive.usercontent.google.com/download?id=ID&export=download";
        assert_eq!(
            next(at, &page("drive-warning.html")).as_deref(),
            Some(
                "https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t&uuid=c45fcb99-500b-4ea1-8988-c8099948304d"
            )
        );
        // A signed-in user's page carries its token, which goes too.
        assert_eq!(
            next(at, &page("drive-warning-at.html")).as_deref(),
            Some(
                "https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t&uuid=0f1e2d3c-0000-4000-8000-000000000001&at=AENtkX%3Atoken%261"
            )
        );
        // The older page: its link, on docs.google.com.
        assert_eq!(
            next(
                "https://docs.google.com/uc?export=download&id=ID",
                &page("drive-old.html")
            )
            .as_deref(),
            Some("https://docs.google.com/uc?export=download&confirm=AbCd&id=ID")
        );
        // The address in the page's data.
        assert_eq!(
            next(at, &page("drive-json.html")).as_deref(),
            Some("https://drive.usercontent.google.com/download?id=ID&export=download&confirm=t")
        );
    }

    #[test]
    fn a_form_off_drive_or_back_to_itself_is_not_followed() {
        let at = url("https://drive.usercontent.google.com/download?id=ID&export=download");
        assert_eq!(
            read_page(&at, &page("drive-offsite.html")),
            PageSays::Nothing
        );
        let itself =
            r#"<form id="download-form" action="/download?export=download&amp;id=ID"></form>"#
                .replace("ID", ID);
        assert_eq!(read_page(&at, &itself), PageSays::Nothing);
        assert_eq!(
            read_page(&at, "<html><p>Drive</p></html>"),
            PageSays::Nothing
        );
    }

    #[test]
    fn drives_error_page_is_a_refusal_in_its_words() {
        let at = url("https://drive.usercontent.google.com/download?id=ID&export=download");
        match read_page(&at, &page("drive-quota.html")) {
            PageSays::Refused(Refusal::Drive { message }) => {
                assert!(
                    message.starts_with("Too many users have viewed or downloaded"),
                    "{message}"
                );
                assert!(message.contains("can't"), "references decoded: {message}");
            }
            other => panic!("{other:?}"),
        }
        let caption_only =
            r#"<p class="uc-error-caption">Sorry, the file you have requested does not exist.</p>"#;
        assert_eq!(
            read_page(&at, caption_only),
            PageSays::Refused(Refusal::Drive {
                message: "Sorry, the file you have requested does not exist.".into()
            })
        );
        let long = format!(
            r#"<p class="uc-error-subcaption">{}</p>"#,
            "word ".repeat(200)
        );
        match read_page(&at, &long) {
            PageSays::Refused(Refusal::Drive { message }) => {
                assert!(message.chars().count() <= MESSAGE_MAX + 1 && message.ends_with('…'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sign_in_and_no_access_answers() {
        let link = url("https://drive.google.com/file/d/ID/view");
        let login = url("https://accounts.google.com/ServiceLogin?continue=x");
        assert_eq!(refused(&link, &login, 200), Some(Refusal::SignIn));
        // Another site's sign-in through Google isn't Drive's.
        assert_eq!(refused(&url("https://site.test/f"), &login, 200), None);
        let file = url("https://drive.usercontent.google.com/download?id=ID");
        assert_eq!(refused(&link, &file, 403), Some(Refusal::NoAccess));
        assert_eq!(refused(&link, &file, 401), Some(Refusal::NoAccess));
        assert_eq!(refused(&link, &file, 404), None);
        assert_eq!(refused(&link, &url("https://cdn.test/f"), 403), None);
    }

    #[test]
    fn drives_hosts_are_one_party() {
        let drive = url("https://drive.google.com/file/d/ID/view");
        let file = url("https://drive.usercontent.google.com/download?id=ID");
        assert!(same_party(&drive, &file));
        assert!(same_party(
            &drive,
            &url("https://docs.google.com/document/d/ID/export")
        ));
        assert!(!same_party(
            &drive,
            &url("https://drive.usercontent.google.com:8443/x")
        ));
        assert!(!same_party(
            &drive,
            &url("https://accounts.google.com/ServiceLogin")
        ));
        assert!(!same_party(
            &drive,
            &url("https://doc-0.docs.googleusercontent.com/export/x")
        ));
        assert!(!same_party(
            &drive,
            &url("https://drive.google.com.evil.test/x")
        ));
    }
}
