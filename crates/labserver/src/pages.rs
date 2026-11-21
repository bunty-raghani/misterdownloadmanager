//! Web pages for the browser extension's tests (tools/extension/e2e.mjs): `/page` (links, videos,
//! audio, adaptive streams; optionally under a strict Content-Security-Policy), `/formpage`
//! (forms that post to `/form`), `/cookie` (sets cookies) and their `/assets`; and `/declared`, a
//! page that declares one video and hides another (#1076, the video look-up's tests). Styles and scripts
//! are files, never inline, so the pages work under `csp=strict`.

use std::collections::HashMap;

use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::faults::parse_flag;

const PAGE_CSS: &str = include_str!("../assets/page.css");
const STREAM_JS: &str = include_str!("../assets/stream.js");
const FEED_JS: &str = include_str!("../assets/feed.js");

/// What `video=1` (and a page that asks for nothing else) plays.
const DEFAULT_VIDEO: &str = "/gen/1M/clip.mp4?inline=1";
/// What `audio=1` plays.
const DEFAULT_AUDIO: &str = "/gen/512K/sound.mp3?inline=1";

/// Escapes text for HTML content and double-quoted attributes.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The query as ordered pairs (a bad query reads as empty).
fn pairs(q: Option<String>) -> Vec<(String, String)> {
    q.and_then(|q| serde_urlencoded::from_str(&q).ok())
        .unwrap_or_default()
}

fn head(title: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>{}</title>\
         <link rel=stylesheet href=/assets/page.css><body>",
        esc(title)
    )
}

fn html(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

fn video_tag(id: &str, src: &str, (w, h): (u32, u32), muted_loop: bool) -> String {
    let how = if muted_loop {
        "muted loop autoplay playsinline"
    } else {
        "controls"
    };
    format!(
        "<video id={id} src=\"{}\" width={w} height={h} {how}></video>",
        esc(src)
    )
}

/// The most links `files=N` lists, and the longest path `pathlen` gives each.
const MAX_FILES: u32 = 200_000;
const MAX_PATH: u32 = 8_000;

/// `files=N`: N links to small generated files, one per line in a list, after a line of text
/// with no link (`#blank`: where a right-click is on the page, not on a link).
///
/// Without `pathlen`, as a big page's "Download all links" gives Link grabber (#1063): two in
/// five a split archive's volumes (`setK.part1.rar`, `setK.part2.rar`: a package each), the
/// others zips, videos and documents. With `pathlen`, `/gen/1K/<folder>/fileK.zip?seed=K`, each
/// path at least `pathlen` characters long (the folder's name made long enough): a list too big
/// for one message to the host, sent in parts (#1063).
fn file_links(n: u32, pathlen: usize) -> String {
    use std::fmt::Write;
    let mut out = format!("<p id=blank>{n} files</p><ul id=files>");
    for k in 0..n {
        let (folder, name, seed) = if pathlen == 0 {
            let name = match k % 5 {
                0 | 1 => format!("set{}.part{}.rar", k / 5, k % 5 + 1),
                2 => format!("file{k}.zip"),
                3 => format!("clip{k}.mp4"),
                _ => format!("doc{k}.pdf"),
            };
            (String::new(), name, k)
        } else {
            let name = format!("file{}.zip", k + 1);
            let fixed = "/gen/1K//".len() + name.len();
            let folder = "d".repeat(pathlen.saturating_sub(fixed).max(1));
            (format!("{folder}/"), name, k + 1)
        };
        let _ = write!(
            out,
            "<li><a href=\"/gen/1K/{folder}{name}?seed={seed}\">{name}</a>"
        );
    }
    out + "</ul>"
}

/// `/page?title=TEXT&link=PATH&files=N&pathlen=L&video=PATH|1&w=&h=&muted_loop=1&iframe=1&shadow=1&several=N&audio=PATH|1&
/// hls=1&dash=1&drm=1&hls2=1&dash2=1&hlsg=SITE&csp=strict`: a web page with what the browser
/// extension acts on (see `HELP`).
pub(crate) async fn page(RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let pairs = pairs(q);
    let m: HashMap<&str, &str> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let flag = |k: &str| m.get(k).and_then(|v| parse_flag(v)).unwrap_or(false);
    let num = |k: &str, default: u32| m.get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    let size = (num("w", 640), num("h", 360));
    let (muted_loop, iframe, shadow) = (flag("muted_loop"), flag("iframe"), flag("shadow"));
    let several = num("several", 0).min(16);
    let csp = m.get("csp") == Some(&"strict");
    // #900: the stream on another site (`hlsg=SITE`), served only to this page's Origin and
    // Referer: SITE's address for this host's guarded stream.
    let guarded = m
        .get("hlsg")
        .zip(headers.get(header::HOST).and_then(|v| v.to_str().ok()))
        .map(|(site, host)| format!("http://{site}/hlsg/{host}/master.m3u8"));
    // A player for each stream asked for, in this order.
    let mut streams: Vec<(&str, &str)> = [
        ("hls", "/hls/master.m3u8"),
        ("dash", "/dash/manifest.mpd"),
        ("drm", "/dash/protected.mpd"),
        ("hls2", "/hls2/playlist?id=lab"),
        ("dash2", "/dash2/manifest?id=lab"),
    ]
    .into_iter()
    .filter(|(k, _)| flag(k))
    .collect();
    if let Some(manifest) = &guarded {
        streams.push(("hlsg", manifest));
    }
    // `link` may come again: one link each, in order.
    let links: Vec<&str> = pairs
        .iter()
        .filter(|(k, _)| k == "link")
        .map(|(_, v)| v.as_str())
        .collect();
    let link = links.first();
    // `files=N`: N more links, each its own file (a big page's, for Link grabber, #1063); with
    // `pathlen`, paths that long (a page listing a big folder: "Download all links" sends them
    // all, in parts).
    let files = num("files", 0).min(MAX_FILES);
    let audio = m.get("audio").map(|a| {
        if parse_flag(a) == Some(true) {
            DEFAULT_AUDIO
        } else {
            a
        }
    });
    // A video when asked for, or when the page would otherwise be empty.
    let video = match m.get("video") {
        Some(v) if parse_flag(v) == Some(true) => Some(DEFAULT_VIDEO),
        Some(v) => Some(*v),
        None if muted_loop
            || iframe
            || shadow
            || (link.is_none()
                && files == 0
                && audio.is_none()
                && several == 0
                && streams.is_empty()) =>
        {
            Some(DEFAULT_VIDEO)
        }
        None => None,
    };

    let mut body = head(m.get("title").copied().unwrap_or("labserver page"));
    for (k, link) in links.iter().enumerate() {
        let id = if k == 0 {
            "link".to_owned()
        } else {
            format!("link{}", k + 1)
        };
        let name = link.rsplit('/').next().unwrap_or("download");
        let text = if k == 0 { "download" } else { name };
        body += &format!("<p><a id={id} href=\"{}\">{}</a></p>", esc(link), esc(text));
    }
    if files > 0 {
        body += &file_links(files, num("pathlen", 0).min(MAX_PATH) as usize);
    }
    if let Some(src) = video {
        if iframe {
            // The same page without the iframe, holding just the video.
            let inner: Vec<(&str, &str)> = pairs
                .iter()
                .filter(|(k, _)| ["muted_loop", "w", "h", "shadow", "csp"].contains(&k.as_str()))
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .chain([("video", src)])
                .collect();
            let inner = serde_urlencoded::to_string(inner).unwrap_or_default();
            body += &format!(
                "<iframe id=frame title=video src=\"/page?{}\" width={} height={}></iframe>",
                esc(&inner),
                size.0 + 40,
                size.1 + 40
            );
        } else if shadow {
            body += &format!(
                "<div id=host><template shadowrootmode=open>\
                 <link rel=stylesheet href=/assets/page.css>{}</template></div>",
                video_tag("video", src, size, muted_loop)
            );
        } else {
            body += &video_tag("video", src, size, muted_loop);
        }
    }
    for k in 1..=several {
        body += &format!(
            "<video id=video{k} width={} height={} controls>\
             <source src=\"/gen/1M/clip{k}.mp4?seed={k}&amp;inline=1\" type=video/mp4></video>",
            size.0, size.1
        );
    }
    if let Some(src) = audio {
        body += &format!("<audio id=audio src=\"{}\" controls></audio>", esc(src));
    }
    for (k, (kind, manifest)) in streams.iter().enumerate() {
        let id = if k == 0 {
            "player".to_owned()
        } else {
            format!("player{}", k + 1)
        };
        // #171's have no .m3u8 or .mpd to tell: their kind, and a text answer first that is no
        // manifest (a player's settings).
        let more = match *kind {
            "hls2" => " data-kind=hls data-decoy=\"/hls2/config?id=lab\"",
            "dash2" => " data-kind=dash data-decoy=\"/hls2/config?id=lab\"",
            _ => "",
        };
        let manifest = esc(manifest);
        body += &format!(
            "<video id={id} width={} height={} controls></video>\
             <script src=/assets/stream.js data-manifest=\"{manifest}\" data-player={id}{more}>\
             </script>",
            size.0, size.1
        );
    }
    let mut resp = html(body);
    // `referrer=POLICY` (#607): the page's own referrer policy; `origin` makes its links'
    // requests carry the site's address only, as a link to another site's file does by default.
    if let Some(policy) = m
        .get("referrer")
        .and_then(|p| header::HeaderValue::from_str(p).ok())
    {
        resp.headers_mut().insert(header::REFERRER_POLICY, policy);
    }
    if csp {
        let media = if !streams.is_empty() {
            "'self' blob:"
        } else {
            "'self'"
        };
        let mut policy = format!(
            "default-src 'none'; style-src 'self'; img-src 'self'; media-src {media}; \
             script-src 'self'"
        );
        if !streams.is_empty() {
            policy += "; connect-src 'self'";
        }
        if iframe {
            policy += "; frame-src 'self'";
        }
        resp.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            policy.parse().expect("ASCII policy"),
        );
    }
    resp
}

/// `/leadon?to=PATH`: a page that goes to `to` by script as it loads, as a download site's
/// thank-you page does (#374).
pub(crate) async fn leadon(RawQuery(q): RawQuery) -> Response {
    let pairs = pairs(q);
    let to = pairs
        .iter()
        .find(|(k, _)| k == "to")
        .map_or("/", |(_, v)| v.as_str());
    let to = to.replace('\\', "\\\\").replace('"', "\\\"");
    html(format!(
        "{}Thank you<script>location.href = \"{to}\";</script>",
        head("Thank you")
    ))
}

/// What `/declared` declares and what it hides, by default.
const DECLARED_FILM: &str = "/gen/2M/film.mp4?seed=3&inline=1";
const DECLARED_HIDDEN: &str = "/gen/1M/welcome.mp4?seed=2&inline=1";

/// `/declared?film=PATH&hidden=PATH&name=TEXT&title=TEXT&how=jsonld|og|twitter|none`: a page that
/// declares its video (#1076) but whose HTML plays only another, hidden one (a site's welcome
/// video in a closed help box): `hidden` in a `<video>` inside `<div hidden>`, and `film`
/// declared as the page's own by its JSON-LD `VideoObject` (`contentUrl`, `name`), its
/// `og:video` (`og:title`) or its `twitter:player` (`twitter:title`), or not at all.
pub(crate) async fn declared(RawQuery(q): RawQuery) -> Response {
    let m: HashMap<String, String> = pairs(q).into_iter().collect();
    let get = |k: &str, default: &str| m.get(k).cloned().unwrap_or_else(|| default.to_owned());
    let (film, hidden, name, title, how) = (
        get("film", DECLARED_FILM),
        get("hidden", DECLARED_HIDDEN),
        get("name", "The Film"),
        get("title", "The Film | Lab"),
        get("how", "jsonld"),
    );
    let mut body = format!(
        "<!doctype html><meta charset=utf-8><title>{}</title>",
        esc(&title)
    );
    match how.as_str() {
        "og" => {
            body += &format!(
                "<meta property=og:title content=\"{}\"><meta property=og:video content=\"{}\">\
                 <meta property=og:video:type content=video/mp4>",
                esc(&name),
                esc(&film)
            );
        }
        "twitter" => {
            body += &format!(
                "<meta name=twitter:card content=player><meta name=twitter:title content=\"{}\">\
                 <meta name=twitter:player content=\"{}\">",
                esc(&name),
                esc(&film)
            );
        }
        "none" => {}
        _ => {
            let ld = serde_json::json!({
                "@context": "https://schema.org",
                "@type": "VideoObject",
                "name": name,
                "contentUrl": film,
            });
            // `</` can't end the script early.
            let ld = ld.to_string().replace("</", "<\\/");
            body += &format!("<script type=application/ld+json>{ld}</script>");
        }
    }
    body += &format!(
        "<link rel=stylesheet href=/assets/page.css><body><h1>{}</h1>\
         <div class=help hidden><video src=\"{}\" controls></video></div>",
        esc(&name),
        esc(&hidden)
    );
    html(body)
}

/// `/formpage?size=1M&name=report.zip&token=abc&with=QUERY`: three forms that post to `/form`
/// (urlencoded, multipart, and urlencoded answered with 303).
pub(crate) async fn formpage(RawQuery(q): RawQuery) -> Response {
    let m: HashMap<String, String> = pairs(q).into_iter().collect();
    let get = |k: &str, default: &str| m.get(k).cloned().unwrap_or_else(|| default.to_owned());
    let (size, name, token) = (
        get("size", "1M"),
        get("name", "report.zip"),
        get("token", "abc"),
    );
    let with = m
        .get("with")
        .filter(|w| !w.is_empty())
        .map(|w| format!("&{w}"))
        .unwrap_or_default();
    let fields = format!(
        "<input type=hidden name=token value=\"{}\"><input name=note value=\"hello world\">",
        esc(&token)
    );
    let mut body = head("labserver forms");
    for (id, enctype, file, query, label) in [
        (
            "form",
            "application/x-www-form-urlencoded",
            name.clone(),
            "seed=5",
            "Download",
        ),
        (
            "multipart",
            "multipart/form-data",
            format!("multipart-{name}"),
            "seed=6",
            "Download (multipart)",
        ),
        (
            "see-other",
            "application/x-www-form-urlencoded",
            format!("see-other-{name}"),
            "seed=7&see_other=1",
            "Download (303)",
        ),
    ] {
        body += &format!(
            "<form id={id} method=post enctype={enctype} action=\"{}\">{fields}\
             <button id=submit-{id}>{label}</button></form>",
            esc(&format!("/form/{size}/{file}?{query}{with}"))
        );
    }
    html(body)
}

/// `/cookie?set=NAME=VALUE&httponly=NAME=VALUE&clear=NAME` (each repeatable): a small page that
/// sets (or clears) cookies for the whole site.
pub(crate) async fn cookie(RawQuery(q): RawQuery) -> Response {
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b))
    };
    let value_ok = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_graphic() && !b"\",;\\".contains(&b))
    };
    let mut cookies = Vec::new();
    for (k, v) in pairs(q) {
        let line = match k.as_str() {
            "set" | "httponly" => match v.split_once('=') {
                Some((n, val)) if token(n) && value_ok(val) => {
                    let http_only = if k == "httponly" { "; HttpOnly" } else { "" };
                    format!("{n}={val}; Path=/{http_only}")
                }
                _ => {
                    return (StatusCode::BAD_REQUEST, format!("bad cookie `{v}`\n"))
                        .into_response();
                }
            },
            "clear" if token(&v) => format!("{v}=; Path=/; Max-Age=0"),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("bad parameter `{k}={v}`\n"),
                )
                    .into_response();
            }
        };
        cookies.push(line);
    }
    if cookies.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "/cookie?set=NAME=VALUE&httponly=NAME=VALUE&clear=NAME\n",
        )
            .into_response();
    }
    let mut body = head("labserver cookies");
    body += "<ul id=cookies>";
    for c in &cookies {
        body += &format!("<li>{}</li>", esc(c));
    }
    body += "</ul>";
    let mut resp = html(body);
    for c in cookies {
        resp.headers_mut()
            .append(header::SET_COOKIE, c.parse().expect("checked cookie"));
    }
    resp
}

/// What `/feed`'s posts play (#1092): the first post's file and the next post's, MP4 movies of 21 s
/// and 33 s (their header says so; the rest is the lab's bytes).
const FEED_FILE_A: &str = "/gen/2M/feed-a.mp4?seed=21&inline=bare&movie=21000";
const FEED_FILE_B: &str = "/gen/3M/feed-b.mp4?seed=33&inline=bare&movie=33000";

/// `/feed` (#1092): a feed whose page names no video (yt-dlp finds none in it, nor in its posts'
/// pages), and whose players play from a MediaSource (blob:), as a feed's do: the first post's
/// fetches its whole MP4 file in ranges (21 s; its post's page, /feed/post/1, has no video), the
/// second's the HLS stream (/hls/master.m3u8, 12 s), the third's is the next video, whose file
/// (33 s) is only begun. Each player starts a moment after the last (`/assets/feed.js`), and
/// `<body data-feed=done>` once all have loaded what they load.
pub(crate) async fn feed() -> Response {
    let mut body = head("labserver feed");
    let posts = [
        (
            "feed1",
            Some("/feed/post/1"),
            format!("data-file=\"{}\" data-secs=21", esc(FEED_FILE_A)),
        ),
        (
            "feed2",
            None,
            "data-hls=/hls/master.m3u8 data-secs=12".to_owned(),
        ),
        (
            "feed3",
            None,
            format!(
                "data-file=\"{}\" data-secs=33 data-preload=1",
                esc(FEED_FILE_B)
            ),
        ),
    ];
    body += "<main class=feed>";
    for (id, post, play) in posts {
        body += &format!(
            "<article class=post><video id={id} width=640 height=360 controls {play}></video>"
        );
        if let Some(p) = post {
            body += &format!("<p><a href={p}>the post</a></p>");
        }
        body += "</article>";
    }
    body += "</main><script src=/assets/feed.js></script>";
    html(body)
}

/// `/feed/post/{n}`: a feed's post's own page, without its video (yt-dlp finds none there).
pub(crate) async fn feed_post(Path(n): Path<u32>) -> Response {
    let mut body = head(&format!("post {n}"));
    body += &format!("<p>Post {n}: its video plays in the feed only.</p>");
    html(body)
}

/// `/assets/{file}`: the pages' stylesheet and the players' scripts.
pub(crate) async fn asset(Path(file): Path<String>) -> Response {
    let (ty, text) = match file.as_str() {
        "page.css" => ("text/css; charset=utf-8", PAGE_CSS),
        "stream.js" => ("text/javascript; charset=utf-8", STREAM_JS),
        "feed.js" => ("text/javascript; charset=utf-8", FEED_JS),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, ty)], text).into_response()
}
