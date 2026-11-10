#!/usr/bin/env python3
"""Builds misterdownloadmanager.com into target/site (GitHub Pages publishes it, .github/workflows/
pages.yml). No framework: website/ holds the hand-written home page, styles, scripts and assets;
website/content/ the pages written as Markdown (FAQ, legal); the rest comes from the repository
itself so the site never drifts from it: docs/*.md become the help pages, CHANGELOG.md the news
page, i18n/languages.tsv the languages page. Every page gets the shared header and footer, its
own title, description, canonical address, social card and structured data, and goes into
sitemap.xml.

    python tools/website/build.py            # build target/site
    python tools/website/build.py --serve    # build, then serve it on http://127.0.0.1:8765

The repository's stars, forks and open issues are read from GitHub's API while building (with
GITHUB_TOKEN when set), so visitors' browsers never call GitHub; offline, the numbers are left
out. Every page carries the Statcounter visit counter; MDM_SITE_NO_STATS=1 leaves it out of a
local build so previews aren't counted.

Needs: pip install markdown
"""
from __future__ import annotations

import datetime
import html
import json
import os
import re
import shutil
import sys
import urllib.parse
import urllib.request
from pathlib import Path

import markdown

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import barcode  # noqa: E402
import contributors  # noqa: E402  (tools/contributors.py: the people the site thanks)
import sitei18n  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
WEB = ROOT / "website"
CONTENT = WEB / "content"
OUT = ROOT / "target" / "site"
SITE = "https://misterdownloadmanager.com"
REPO = "https://github.com/anefzaoui/misterdownloadmanager"
# The Firefox add-on's listing (crates/ipc/src/extension.rs FIREFOX_PAGE): {{AMO}} in pages, and
# <!--@amo--> for its Add to Firefox button.
AMO = "https://addons.mozilla.org/firefox/addon/mister-download-manager/"
# The extension's listing in the Chrome Web Store (crates/ipc/src/extension.rs STORE_ID), for
# Chrome, Edge, Brave, Opera and Vivaldi: {{CWS}} in pages; <!--@stores--> is both buttons.
CWS = "https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp"
# The creator's GitHub Sponsors page: the header, the footer and the home page's Support MDM
# (the app's Help menu and About, the extension's popup and .github/FUNDING.yml point there too).
SPONSORS = "https://github.com/sponsors/anefzaoui"
NAME = "Mister Download Manager"
# The next release, as the site announces it. What is in the source but not in a download yet
# says "Coming with version <this>" (coming_banner(): the feature pages, the Themes page, the
# features' tiles), shown for a feature whose `since` is this version or later. At that release
# set it to "" and every banner goes at once; a published release at or past it hides them too.
UPCOMING = "0.5.2"
CARGO_VERSION = re.search(r'(?m)^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text("utf-8")).group(1)


def github(path: str):
    """GET api.github.com/<path> as JSON, or None (offline, rate-limited)."""
    req = urllib.request.Request(f"https://api.github.com/{path}", headers={
        "Accept": "application/vnd.github+json", "User-Agent": "misterdownloadmanager-site"})
    if os.environ.get("GITHUB_TOKEN"):
        req.add_header("Authorization", f"Bearer {os.environ['GITHUB_TOKEN']}")
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return json.load(r)
    except Exception as e:  # noqa: BLE001 — the site builds without the numbers
        print(f"github {path}: {e}", file=sys.stderr)
        return None


LATEST = github(f"repos/{REPO.split('github.com/')[1]}/releases/latest") or {}


def published_version() -> str:
    """The latest published release's version: what the download links and the version shown
    point at. Cargo.toml's is ahead of it between a merged version bump and its release's
    files (release.yml rebuilds the site once they are published). Not known: on GitHub's
    runners (GITHUB_ACTIONS) the build stops, as Cargo.toml's version would deploy download
    links that don't exist yet; built here (offline), Cargo.toml's, with a warning."""
    tag = LATEST.get("tag_name", "").lstrip("v")
    if tag:
        return tag
    if os.environ.get("GITHUB_ACTIONS"):
        sys.exit(
            "error: the latest release couldn't be read from GitHub (see above), so the site "
            "isn't built: with Cargo.toml's version "
            f"({CARGO_VERSION}) its download links could point at files not published yet."
        )
    print(
        f"warning: the latest release couldn't be read from GitHub; the download links use "
        f"Cargo.toml's version ({CARGO_VERSION}), which may not be published yet",
        file=sys.stderr,
    )
    return CARGO_VERSION


def setup_size() -> str:
    """The Windows installer's size as the label shows it ("12.4 MB"), or "" offline."""
    for a in LATEST.get("assets", []):
        if a.get("name", "").endswith("-setup.exe"):
            return f"{a['size'] / 1_000_000:.1f} MB"
    return ""


def mac_file(arch: str) -> str:
    """The .dmg for a Mac with `arch` (arm64, x86_64): one per processor (#25), or the one
    universal .dmg of a release from before."""
    universal = f"MisterDownloadManager-{VERSION}-macos-universal-experimental.dmg"
    if any(a.get("name") == universal for a in LATEST.get("assets", [])):
        return f"{RELEASE}-macos-universal-experimental.dmg"
    return f"{RELEASE}-macos-{arch}-experimental.dmg"


def linux_file(ext: str) -> str:
    """The Linux file ending in `ext` (.AppImage, .deb, .rpm, .tar.gz): named without
    "-experimental" from the release after 0.2.4 (tools/installer/build-linux.sh), with it in a
    release from before."""
    old = f"MisterDownloadManager-{VERSION}-linux-x86_64-experimental{ext}"
    if any(a.get("name") == old for a in LATEST.get("assets", [])):
        return f"{RELEASE}-linux-x86_64-experimental{ext}"
    return f"{RELEASE}-linux-x86_64{ext}"


VERSION = published_version()
SETUP_SIZE = setup_size()
TODAY = datetime.date.today().isoformat()
RELEASE = f"{REPO}/releases/download/v{VERSION}/MisterDownloadManager-{VERSION}"
FILES = {
    "setup": f"{RELEASE}-setup.exe",
    "portable": f"{RELEASE}-portable.zip",
    "zip": f"{RELEASE}-x64.zip",
    "dmg_arm64": mac_file("arm64"),
    "dmg_intel": mac_file("x86_64"),
    "appimage": linux_file(".AppImage"),
    "deb": linux_file(".deb"),
    "rpm": linux_file(".rpm"),
    "tar": linux_file(".tar.gz"),
    "chromium": f"{RELEASE}-extension-chromium.zip",
    "firefox": f"{RELEASE}-extension-firefox.xpi",
    "sums": f"{REPO}/releases/download/v{VERSION}/SHA256SUMS.txt",
}

# Every page built, for the sitemap: (path, priority).
PAGES: list[tuple[str, str]] = []
# For llms.txt and llms-full.txt: the help pages as Markdown (path, title, description, text)
# and the FAQ (question, path, answer), as the builders read them.
DOCS_MD: list[tuple[str, str, str, str]] = []
FAQ_MD: list[tuple[str, str, str]] = []

# The visit counter (privacy.md says what it records). Statcounter's own code, as given.
STATCOUNTER = """<!-- Default Statcounter code for Mister Download Manager
https://misterdownloadmanager.com/ -->
<script type="text/javascript">
var sc_project=13364153;
var sc_invisible=1;
var sc_security="c94415aa";
</script>
<script type="text/javascript"
src="https://www.statcounter.com/counter/counter.js"
async></script>
<noscript><div class="statcounter"><a title="Web Analytics"
href="https://statcounter.com/" target="_blank"><img
class="statcounter"
src="https://c.statcounter.com/13364153/0/c94415aa/1/"
alt="Web Analytics"
referrerPolicy="no-referrer-when-downgrade"></a></div></noscript>
<!-- End of Statcounter Code -->"""


def stats_code() -> str:
    return "" if os.environ.get("MDM_SITE_NO_STATS") == "1" else STATCOUNTER


def short_num(n: int) -> str:
    return f"{n / 1000:.1f}k".replace(".0k", "k") if n >= 1000 else str(n)


def repo_stats() -> dict[str, str]:
    """The repository's numbers for the header pill and the open-source card; empty offline."""
    slug_ = REPO.split("github.com/")[1]
    repo = github(f"repos/{slug_}")
    if not repo:
        return {}
    issues = github(f"search/issues?q=repo:{slug_}+type:issue+state:open&per_page=1")
    out = {
        "stars": short_num(repo.get("stargazers_count", 0)),
        "forks": short_num(repo.get("forks_count", 0)),
        "watchers": short_num(repo.get("subscribers_count", 0)),
    }
    if issues is not None:
        out["issues"] = short_num(issues.get("total_count", 0))
    return out


STATS: dict[str, str] = {}

# The people who made MDM possible (tools/contributors.py), read once per build: everyone, and the
# releases for each one's own. None when GitHub can't be read: the home page then leaves its
# section out, /contributors.html says so, and the News page thanks nobody (with a warning).
CREW: list[dict] | None = None
CREW_RELEASES: list[dict] | None = None


def load_crew() -> None:
    global CREW, CREW_RELEASES
    CREW = contributors.everyone()
    CREW_RELEASES = contributors.releases() if CREW is not None else None
    if CREW is None:
        print("warning: the contributors couldn't be read from GitHub; the site is built without "
              "them (set GITHUB_TOKEN if the rate limit ran out)", file=sys.stderr)


def face(p: dict, size: int) -> str:
    """One contributor's round photo, linked to their GitHub profile; the login is the link's
    name, shown over the photo on hover and focus (never translated)."""
    return (f'<li translate="no"><a class="face" href="{esc(p["profile"])}"><img src="{esc(contributors.avatar(p["avatar"], size * 2))}" '
            f'width="{size}" height="{size}" alt="" loading="lazy" decoding="async">'
            f'<span class="face-name">{esc(p["login"])}</span></a></li>')


def faces(people: list[dict], size: int = 56) -> str:
    """Round photos of `people` (a release on the News page), row under row."""
    return f'<ul class="faces" style="--face: {size}px" role="list">{"".join(face(p, size) for p in people)}</ul>'


def people_count(n: int) -> str:
    """"26 people", the number kept out of the text to translate (one text for every count)."""
    return f'<span translate="no">{n}</span> {"person" if n == 1 else "people"}'


# How many photos the home page's row shows, by the screen's width (styles.css .pile, the same
# breakpoints): (from this width up, photos). The rest are "+N", which opens /contributors.html.
PILE_TIERS = [(0, 7), (561, 11), (1200, 15)]


def crew_pile() -> str:
    """The home page's thanks, on the board beside the hero's label (under the window; after the
    label on a phone): "Made possible by these amazing humans", a row of round photos, overlapping,
    and "+N" for the ones the row has no room for."""
    if not CREW:
        return ""
    n = len(CREW)
    shown = max(cap for _, cap in PILE_TIERS)
    # "+N" for each width where the row is full: its count there, and only there.
    more = "".join(f'<span class="pile-n pile-n-{cap}">+{n - cap}</span>' for _, cap in PILE_TIERS if n > cap)
    more_li = (f'<li class="pile-more"><a href="/contributors.html"><span translate="no" dir="ltr">{more}</span> '
               f'<span class="sr-only">Everyone who helped</span></a></li>' if more else "")
    full = " ".join(f"pile-full-{cap}" for _, cap in PILE_TIERS if n > cap)
    return f"""<div class="parcel-crew">
          <p class="crew-title" id="crew-title">Made possible by these amazing humans</p>
          <ul class="faces pile {full}" style="--face: 44px" role="list" aria-labelledby="crew-title">{"".join(face(p, 44) for p in CREW[:shown])}{more_li}</ul>
          <p class="crew-fine">{people_count(n)} who sent a pull request, reported a problem or joined a conversation on GitHub. <a href="/contributors.html">Meet them all</a></p>
        </div>"""


def esc(s: str) -> str:
    return html.escape(s, quote=True)


def github_slug(text: str, separator: str = "-") -> str:
    """A heading's id as GitHub makes it, so a docs link such as `options.md#proxy--socks` works
    on GitHub and here alike: lower case, punctuation dropped (letters of every script, digits,
    `-` and `_` kept), each space a dash, runs of dashes kept."""
    return re.sub(r"[^\w\- ]", "", text.strip().lower()).replace(" ", separator)


def old_aliases(out: str) -> str:
    """Each heading whose GitHub id differs from the id this site gave it before (Python
    Markdown's own: ASCII only, a run of dashes made one) keeps that one reachable too, for the
    links already out there (`options.html#proxy-socks`)."""
    from markdown.extensions.toc import slugify

    def alias(m: re.Match) -> str:
        text = html.unescape(re.sub(r"<[^>]+>", "", m.group(3)))
        old = slugify(text, "-")
        # A heading with an id of its own (`{#not-running}`) never had another.
        if not old or old == m.group(2) or not m.group(2).startswith(github_slug(text)):
            return m.group(0)
        # Before the heading, not inside it: the heading's text stays one segment, translated
        # as it was (sitei18n).
        return f'<span id="{esc(old)}" class="anchor-alias"></span>{m.group(0)}'

    return re.sub(r'(<h[1-6] id="([^"]*)">)(.*?)(</h[1-6]>)', alias, out, flags=re.S)


def md(text: str) -> str:
    out = markdown.markdown(
        text, extensions=["tables", "fenced_code", "toc", "attr_list", "sane_lists", "md_in_html"],
        extension_configs={"toc": {"permalink": False, "slugify": github_slug}},
    )
    out = old_aliases(out)
    # A table wider than the column scrolls inside its own box, not the page (#169).
    return re.sub(r"<table\b.*?</table>", lambda m: f'<div class="table-wrap">{m.group(0)}</div>', out, flags=re.S)


def plain(text: str) -> str:
    """Markdown or HTML to one line of plain text (descriptions)."""
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", text)
    text = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"[*_`>#]", "", text)
    return " ".join(html.unescape(text).split())


def clip(text: str, n: int = 158) -> str:
    text = plain(text)
    if len(text) <= n:
        return text
    cut = text[: n - 1].rsplit(" ", 1)[0].rstrip(",;:—-")
    return cut + "…"


# ---- The frame every page shares ---------------------------------------------------------------

ICONS = """<svg width="0" height="0" class="defs" aria-hidden="true">
  <symbol id="os-windows" viewBox="0 0 24 24"><path d="M2 4.6 10 3.5v7.9H2zM11 3.4 22 2v9.4H11zM2 12.4h8v7.9l-8-1.1zM11 12.4h11V22l-11-1.5z"/></symbol>
  <symbol id="os-macos" viewBox="0 0 24 24"><path d="M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701"/></symbol>
  <symbol id="os-linux" viewBox="0 0 24 24"><path d="M12 1.5c-2.6 0-4.3 2-4.3 4.6 0 1.4.3 2.3-.6 3.6C5.6 11.9 4.6 14 4.6 16.3c0 3.2 3.1 5.7 7.4 5.7s7.4-2.5 7.4-5.7c0-2.3-1-4.4-2.5-6.6-.9-1.3-.6-2.2-.6-3.6 0-2.6-1.7-4.6-4.3-4.6z"/><path d="M12 9.6c-2.4 0-4.1 2.9-4.1 6.2 0 2.6 1.8 4.4 4.1 4.4s4.1-1.8 4.1-4.4c0-3.3-1.7-6.2-4.1-6.2z" fill="var(--paper)"/><circle cx="10.5" cy="5.6" r=".95" fill="var(--paper)"/><circle cx="13.5" cy="5.6" r=".95" fill="var(--paper)"/><path d="M10.3 7.3h3.4L12 9.2z" fill="#f2b630"/><path d="M3.6 21.6c.4-2.2 2.6-3.3 4.6-2.3-.2 2-2.6 3.3-4.6 2.3zm16.8 0c-.4-2.2-2.6-3.3-4.6-2.3.2 2 2.6 3.3 4.6 2.3z" fill="#f2b630"/></symbol>
  <symbol id="i-github" viewBox="0 0 24 24"><path d="M12 .5a11.5 11.5 0 0 0-3.64 22.41c.58.1.79-.25.79-.56v-2c-3.2.7-3.88-1.37-3.88-1.37-.52-1.33-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.7 1.26 3.36.96.1-.75.4-1.26.73-1.55-2.55-.29-5.24-1.28-5.24-5.69 0-1.26.45-2.29 1.19-3.09-.12-.29-.52-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.77 0c2.2-1.49 3.17-1.18 3.17-1.18.63 1.59.23 2.76.11 3.05.74.8 1.19 1.83 1.19 3.09 0 4.42-2.69 5.39-5.26 5.68.41.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56A11.5 11.5 0 0 0 12 .5z"/></symbol>
  <symbol id="i-star" viewBox="0 0 24 24"><path d="M12 2.8l2.84 5.76 6.36.92-4.6 4.49 1.08 6.33L12 17.31l-5.68 2.99 1.08-6.33-4.6-4.49 6.36-.92z"/></symbol>
  <symbol id="i-fork" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" d="M6 3v4a4 4 0 0 0 4 4h4a4 4 0 0 0 4-4V3M12 11v10"/><circle cx="6" cy="4" r="2"/><circle cx="18" cy="4" r="2"/><circle cx="12" cy="20" r="2"/></symbol>
  <symbol id="i-issue" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="12" cy="12" r="2.2"/></symbol>
  <symbol id="i-chevron" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" d="M6 9l6 6 6-6"/></symbol>
  <symbol id="i-out" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M7 17L17 7M9 7h8v8"/></symbol>
  <symbol id="i-edit" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4"/></symbol>
  <symbol id="i-code" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M8 7l-5 5 5 5M16 7l5 5-5 5M13.5 4l-3 16"/></symbol>
  <symbol id="i-heart" viewBox="0 0 24 24"><path d="M12 20.5s-8.5-5-8.5-11A4.5 4.5 0 0 1 12 6.6a4.5 4.5 0 0 1 8.5 2.9c0 6-8.5 11-8.5 11z"/></symbol>
  <symbol id="i-tag" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" d="M3 12V4h8l10 10-8 8z"/><circle cx="7.5" cy="8.5" r="1.5"/></symbol>
  <symbol id="i-moon" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/></symbol>
  <symbol id="i-clock" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></g></symbol>
  <symbol id="i-plus" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" d="M12 5v14M5 12h14"/></symbol>
  <symbol id="i-copy" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/></g></symbol>
  <symbol id="i-arrow" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M5 12h14M13 6l6 6-6 6"/></symbol>
  <symbol id="i-sun" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></g></symbol>
  <symbol id="p-https" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5M12 14.5v2.5"/></g></symbol>
  <symbol id="p-ftp" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7h6l2 2h10v10H3z"/><path d="M12 16v-5m-2.5 2.5L12 11l2.5 2.5"/></g></symbol>
  <symbol id="p-torrent" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="17" r="2.2"/><circle cx="19" cy="17" r="2.2"/><path d="M10.8 6.9 6.2 15M13.2 6.9 17.8 15M7.2 17h9.6"/></g></symbol>
  <symbol id="p-magnet" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 4v8a6 6 0 0 0 12 0V4h-4v8a2 2 0 0 1-4 0V4z"/><path d="M6 8h4M14 8h4"/></g></symbol>
  <symbol id="p-proxy" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z"/><path d="M9 12h6m-2-2 2 2-2 2"/></g></symbol>
  <symbol id="p-pac" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M8 4c-2 0-2 1.5-2 3s0 3-2 5c2 2 2 3.5 2 5s0 3 2 3M16 4c2 0 2 1.5 2 3s0 3 2 5c-2 2-2 3.5-2 5s0 3-2 3"/></g></symbol>
  <symbol id="p-import" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v11m-4-4 4 4 4-4"/><path d="M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4"/></g></symbol>
</svg>"""

NAV = [
    ("Features", "/features/", "features"),
    ("Themes", "/themes.html", "themes"),
    ("Download", "/download.html", "download"),
    ("Help", "/help/", "help"),
    ("FAQ", "/faq/", "faq"),
    ("Benchmark", "/benchmark.html", "bench"),
    ("News", "/news.html", "news"),
]


def gh_pill(cls: str = "gh-pill") -> str:
    """The GitHub link with its star count (when known): header, hero, open-source card."""
    stars = STATS.get("stars")
    count = f'<span class="gh-count">{stars}</span>' if stars else ""
    return (f'<a class="{cls}" href="{REPO}" aria-label="Mister Download Manager on GitHub'
            f'{f", {stars} stars" if stars else ""}">'
            f'<svg aria-hidden="true"><use href="#i-github"/></svg><span class="gh-label">GitHub</span>{count}</a>')


def os_menu() -> str:
    """The split download button's list: every system and file (hero, final call)."""
    rows = [
        ("windows", "Windows installer", "setup.exe · Windows 10 and 11", FILES["setup"]),
        ("windows", "Windows portable", "portable.zip · runs from a folder or USB stick", FILES["portable"]),
        ("macos", "macOS Apple Silicon", ".dmg · M1 and later · experimental", FILES["dmg_arm64"]),
        ("macos", "macOS Intel", ".dmg · Macs with an Intel processor · experimental", FILES["dmg_intel"]),
        ("linux", "Linux AppImage", "any distribution · experimental", FILES["appimage"]),
        ("linux", "Linux .deb / .rpm", "Debian, Ubuntu · Fedora, openSUSE", "/download.html#linux"),
    ]
    items = "".join(
        f'<li><a href="{href}"><svg class="os-ico" aria-hidden="true"><use href="#os-{os_}"/></svg>'
        f'<span><b>{label}</b><small>{sub}</small></span></a></li>' for os_, label, sub, href in rows)
    return (f'<ul class="split-menu" role="list">{items}'
            f'<li class="split-all"><a href="/download.html">All files, extensions and checksums →</a></li></ul>')


def split_button(extra_class: str = "") -> str:
    """Download for <your system>, with the other systems under its arrow."""
    return f"""<div class="split{extra_class}">
          <a class="btn btn-go split-main" href="/download.html" data-os-cta>
            <svg class="os-ico" aria-hidden="true"><use href="#os-windows" data-os-cta-icon/></svg>
            <span data-os-label>Download for Windows</span>
          </a>
          <details class="split-more">
            <summary class="btn btn-go" aria-label="Other systems and files"><svg aria-hidden="true"><use href="#i-chevron"/></svg></summary>
            {os_menu()}
          </details>
        </div>"""


def amo_button(extra_class: str = "") -> str:
    """Add to Firefox: the add-on's listing on addons.mozilla.org, drawn in line (never green:
    green is Download's), with Firefox's own logo; in words, so it's translated (Mozilla's badge
    is an English picture)."""
    return (f'<a class="btn btn-line store-btn{extra_class}" href="{AMO}">'
            '<img src="/assets/browsers/firefox.svg" width="22" height="22" alt=""> Add to Firefox</a>')


def cws_button(extra_class: str = "") -> str:
    """Add to Chrome: the extension's listing in the Chrome Web Store (Edge, Brave, Opera and
    Vivaldi install from it too), drawn as Add to Firefox is."""
    return (f'<a class="btn btn-line store-btn{extra_class}" href="{CWS}">'
            '<img src="/assets/browsers/chrome.svg" width="22" height="22" alt=""> Add to Chrome</a>')


def store_buttons() -> str:
    """Both stores' buttons side by side: Firefox's, then the Chrome Web Store's."""
    return f'<span class="store-btns">{amo_button()}{cws_button()}</span>'


def header(section: str = "") -> str:
    links = "\n        ".join(
        f'<a href="{href}"{" aria-current=\"page\"" if key == section else ""}>{label}</a>'
        for label, href, key in NAV
    )
    return f"""<a class="skip-link" href="#main">Skip to content</a>
  {ICONS}
  <header class="top">
    <div class="wrap top-inner">
      <a class="brand" href="/">
        <img src="/assets/logo-64.png" width="34" height="34" alt="">
        <span>{NAME}</span>
      </a>
      <nav class="nav" aria-label="Main">
        {links}
      </nav>
      <div class="top-actions">
        <!--@langs-->
        <button class="icon-btn theme" type="button" aria-label="Switch between light and dark" title="Light / dark">
          <svg class="for-light-i" aria-hidden="true"><use href="#i-moon"/></svg><svg class="for-dark-i" aria-hidden="true"><use href="#i-sun"/></svg>
        </button>
        <a class="icon-btn top-gh" href="{REPO}" aria-label="Mister Download Manager on GitHub" title="GitHub"><svg aria-hidden="true"><use href="#i-github"/></svg></a>
        <a class="icon-btn top-sponsor" href="{SPONSORS}" aria-label="Sponsor the creator on GitHub" title="Sponsor"><svg aria-hidden="true"><use href="#i-heart"/></svg></a>
        <a class="btn btn-go top-dl" href="/download.html">Download</a>
        <details class="menu">
          <summary class="icon-btn" aria-label="Menu"><svg aria-hidden="true" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" d="M4 7h16M4 12h16M4 17h16"/></svg></summary>
          <nav class="menu-panel" aria-label="Main">
            {links}
            <a href="/languages.html">Languages</a>
            <a href="/contributing.html">Contribute</a>
            <a href="{REPO}">GitHub</a>
            <a href="{SPONSORS}">Sponsor</a>
          </nav>
        </details>
      </div>
    </div>
  </header>
  {HEADER_FIT}"""


# Right after the header, before the first paint: the header gives up, one step at a time, what
# doesn't fit its row in this language at this width (the Sponsor heart, then some spacing, then
# the links for the menu, then the language's name beside its flag), so no language runs past the
# page (#168). The media queries in styles.css are the floor, and all there is without scripts.
HEADER_FIT = """<script>
    (function () {
      var top = document.currentScript.previousElementSibling, row = top.firstElementChild;
      var steps = ['fit-sponsor', 'fit-tight', 'fit-menu', 'fit-flag'];
      // Its pieces, end to end, against the row's content box (overflow into the padding counts).
      function over() {
        var s = getComputedStyle(row), lo = Infinity, hi = -Infinity;
        for (var k = row.firstElementChild; k; k = k.nextElementSibling) {
          var b = k.getBoundingClientRect();
          if (b.width) { lo = Math.min(lo, b.left); hi = Math.max(hi, b.right); }
        }
        return hi - lo > row.clientWidth - parseFloat(s.paddingLeft) - parseFloat(s.paddingRight) + 0.5;
      }
      function fit() {
        for (var i = 0; i < steps.length; i++) top.classList.remove(steps[i]);
        for (var j = 0; j < steps.length && over(); j++) top.classList.add(steps[j]);
      }
      fit();
      if (window.ResizeObserver) new ResizeObserver(fit).observe(row);
      // Archivo may arrive after this runs (or start loading only once laid out): fit again then.
      if (document.fonts) { document.fonts.ready.then(fit); document.fonts.addEventListener('loadingdone', fit); }
    })();
  </script>"""


def footer() -> str:
    year = datetime.date.today().year
    return f"""<footer class="foot">
    <div class="wrap">
      <div class="foot-cols">
        <div class="foot-brand">
          <a class="brand" href="/"><img src="/assets/logo-64.png" width="34" height="34" alt=""><span>{NAME}</span></a>
          <p class="foot-tag">The download manager you already know, rebuilt to be faster. Free, open source and written from scratch in Rust.</p>
          <a class="text-link foot-star" href="{REPO}"><svg aria-hidden="true"><use href="#i-star"/></svg> Star it on GitHub</a>
          <a class="text-link foot-star" href="{SPONSORS}"><svg aria-hidden="true"><use href="#i-heart"/></svg> Sponsor the creator</a>
        </div>
        <div>
          <h2>Product</h2>
          <ul>
            <li><a href="/download.html">Download</a></li>
            <li><a href="/download.html#windows">Windows</a></li>
            <li><a href="/download.html#macos">macOS</a></li>
            <li><a href="/download.html#linux">Linux</a></li>
            <li><a href="/download.html#extension">Browser extension</a></li>
            <li><a href="/features/">Features</a></li>
            <li><a href="/themes.html">Themes</a></li>
            <li><a href="/benchmark.html">Benchmark</a></li>
            <li><a href="/idm-alternative.html">Free IDM alternative</a></li>
          </ul>
        </div>
        <div>
          <h2>Help</h2>
          <ul>
            <li><a href="/help/">User guide</a></li>
            <li><a href="/help/#tutorials">Tutorials</a></li>
            <li><a href="/faq/">FAQ</a></li>
            <li><a href="/help/troubleshooting.html">Troubleshooting</a></li>
            <li><a href="/help/keyboard-shortcuts.html">Keyboard shortcuts</a></li>
            <li><a href="/languages.html">Languages</a></li>
          </ul>
        </div>
        <div>
          <h2>Project</h2>
          <ul>
            <li><a href="{REPO}">Source code</a></li>
            <li><a href="{REPO}/issues">Issues</a></li>
            <li><a href="{REPO}/issues/new?template=bug.yml">Report a bug</a></li>
            <li><a href="{REPO}/releases">Releases</a></li>
            <li><a href="/news.html">What's new</a></li>
            <li><a href="/contributing.html">Contributing</a></li>
            <li><a href="/contributors.html">Contributors</a></li>
            <li><a href="/news.xml">News feed</a></li>
          </ul>
        </div>
        <div>
          <h2>Legal</h2>
          <ul>
            <li><a href="/privacy.html">Privacy</a></li>
            <li><a href="/terms.html">Terms of use</a></li>
            <li><a href="/license.html">License</a></li>
            <li><a href="/security.html">Security</a></li>
          </ul>
        </div>
      </div>
      <div class="foot-bottom">
        <p>GPL-3.0 · built in the open on <a href="{REPO}">GitHub</a></p>
        <p>© {year} Ahmed Nefzaoui and contributors</p>
      </div>
      <p class="legal">Not affiliated with Tonec Inc.; Internet Download Manager and IDM are trademarks of their owner, named here only to compare.</p>
    </div>
  </footer>
  {stats_code()}"""


# In every head (templated pages get it through <!--@theme-->): the font, then the theme before
# the first paint.
THEME_SCRIPT = """<link rel="preload" href="/vendor/archivo/archivo-latin.woff2" as="font" type="font/woff2" crossorigin>
  <script>
    (function () {
      var d = document.documentElement;
      d.classList.remove('js-off');
      var q = /[?&]theme=(light|dark)/.exec(location.search);
      try {
        var t = q ? q[1] : localStorage.getItem('mdm-theme');
        if (t === 'light' || t === 'dark') d.setAttribute('data-theme', t);
      } catch (e) { if (q) d.setAttribute('data-theme', q[1]); }
    })();
  </script>"""


def head(path: str, title: str, desc: str, *, jsonld: list | None = None, og_type: str = "website",
         extra: str = "", noindex: bool = False) -> str:
    url = SITE + path
    ld = "".join(
        f'\n  <script type="application/ld+json">{json.dumps(x, ensure_ascii=False, separators=(",", ":"))}</script>'
        for x in (jsonld or [])
    )
    robots = '\n  <meta name="robots" content="noindex">' if noindex else ""
    return f"""<meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{esc(title)}</title>
  <meta name="description" content="{esc(desc)}">{robots}
  <meta name="theme-color" content="#1f63d6" media="(prefers-color-scheme: light)">
  <meta name="theme-color" content="#0c1a33" media="(prefers-color-scheme: dark)">
  <meta name="color-scheme" content="light dark">
  <link rel="canonical" href="{url}">
  <meta property="og:type" content="{og_type}">
  <meta property="og:site_name" content="{NAME}">
  <meta property="og:title" content="{esc(title)}">
  <meta property="og:description" content="{esc(desc)}">
  <meta property="og:url" content="{url}">
  <meta property="og:image" content="{SITE}/assets/social.png">
  <meta property="og:image:width" content="1280">
  <meta property="og:image:height" content="640">
  <meta property="og:image:alt" content="{NAME}: the download manager you already know, rebuilt to be faster">
  <meta name="twitter:card" content="summary_large_image">
  <link rel="icon" href="/favicon.ico" sizes="16x16 32x32 48x48 64x64">
  <link rel="icon" href="/assets/favicon-32.png" type="image/png" sizes="32x32">
  <link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">
  <link rel="manifest" href="/site.webmanifest">
  <link rel="alternate" type="application/atom+xml" title="{NAME} news" href="/news.xml">
  <link rel="stylesheet" href="/styles.css">
  {THEME_SCRIPT}
  <script src="/main.js" defer></script>{ld}{extra}"""


def crumbs_ld(trail: list[tuple[str, str]]) -> dict:
    return {
        "@context": "https://schema.org", "@type": "BreadcrumbList",
        "itemListElement": [
            {"@type": "ListItem", "position": i + 1, "name": n, "item": SITE + p}
            for i, (n, p) in enumerate(trail)
        ],
    }


def crumbs_html(trail: list[tuple[str, str]]) -> str:
    items = []
    for i, (n, p) in enumerate(trail):
        last = i == len(trail) - 1
        items.append(f'<li aria-current="page">{esc(n)}</li>' if last else f'<li><a href="{p}">{esc(n)}</a></li>')
    return f'<ol class="crumbs" aria-label="Breadcrumb">{"".join(items)}</ol>'


def write(path: str, title: str, desc: str, body: str, *, section: str = "", jsonld: list | None = None,
          og_type: str = "website", priority: str = "0.6", noindex: bool = False, extra: str = "") -> None:
    doc = f"""<!doctype html>
<html lang="en" class="js-off">
<head>
  {head(path, title, desc, jsonld=jsonld, og_type=og_type, noindex=noindex, extra=extra)}
</head>
<body>
  {header(section)}
  <main id="main">
{body}
  </main>
  {footer()}
</body>
</html>
"""
    target = OUT / (path.lstrip("/") + ("index.html" if path.endswith("/") else ""))
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(doc, "utf-8", newline="\n")
    if not noindex:
        PAGES.append((path, priority))


def page_head(title: str, lede: str = "", trail: list[tuple[str, str]] | None = None, fine: str = "") -> str:
    c = crumbs_html(trail) if trail else ""
    l = f'\n        <p class="lede small">{lede}</p>' if lede else ""
    f = f'\n        <p class="fine">{fine}</p>' if fine else ""
    return f"""    <section class="page-head">
      <div class="wrap">
        {c}
        <h1>{title}</h1>{l}{f}
      </div>
    </section>"""


def page_tools(src: str, path: str, title: str) -> str:
    """Under an article written in the repository: edit its source, or report a problem with it."""
    q = urllib.parse.quote
    # The Website problem form (.github/ISSUE_TEMPLATE/website.yml), its page filled in.
    issue = f"{REPO}/issues/new?template=website.yml&title={q('Website: ' + title)}&page={q(SITE + path)}"
    return f"""<div class="page-tools">
          <a href="{REPO}/edit/main/{src}"><svg aria-hidden="true"><use href="#i-edit"/></svg>Edit this page on GitHub</a>
          <a href="{issue}"><svg aria-hidden="true"><use href="#i-issue"/></svg>Report a problem with this page</a>
          <a href="{REPO}/commits/main/{src}"><svg aria-hidden="true"><use href="#i-code"/></svg>Page history</a>
        </div>"""


def cta_band(text: str = "Free, open source, no ads, no trial.") -> str:
    return f"""<div class="cta-band"><p>{text}</p>
          <a class="btn btn-go" href="/download.html"><svg class="os-ico" aria-hidden="true"><use href="#os-windows"/></svg> Download {NAME}</a></div>"""


def waybill_fields() -> str:
    """The release's facts in the label's field grammar (the home page's waybill and the
    download page): what the visitor gets, for what, at what postage."""
    size = f' <small>{SETUP_SIZE}</small>' if SETUP_SIZE else ""
    return (f'<dl class="fields">'
            f'<div><dt>Postage</dt><dd>Free</dd></div>'
            f'<div><dt>Delivery</dt><dd><a href="/benchmark.html">1 GiB in 1.0 s</a> <small>IDM: 5.5 s</small></dd></div>'
            f'<div><dt>System</dt><dd>Windows 10 and 11</dd></div>'
            f'<div><dt>Version</dt><dd><a href="/news.html">{VERSION}</a>{size}</dd></div></dl>')


SOFTWARE_LD = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": NAME,
    "alternateName": "MDM",
    "applicationCategory": "UtilitiesApplication",
    "applicationSubCategory": "Download manager",
    "operatingSystem": "Windows 10, Windows 11, macOS (experimental), Linux (experimental)",
    "softwareVersion": VERSION,
    "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
    "license": "https://www.gnu.org/licenses/gpl-3.0.html",
    "url": SITE + "/",
    "downloadUrl": SITE + "/download.html",
    "installUrl": FILES["setup"],
    "screenshot": SITE + "/assets/screens/main-light.webp",
    "image": SITE + "/assets/logo-640.png",
    "author": {"@type": "Person", "name": "Ahmed Nefzaoui", "url": "https://github.com/anefzaoui"},
    "codeRepository": REPO,
    "featureList": [
        "Up to 128 connections per download with dynamic re-splitting",
        "Crash-safe resume", "HTTP, HTTPS, FTP and FTPS", "BitTorrent and magnet links",
        "Video downloads from web pages (yt-dlp)", "Browser extension for Chrome, Edge, Brave, Opera, Vivaldi and Firefox",
        "Queues and scheduler", "Speed limiter", "86 interface languages", "Portable mode",
        "Command line and automation API", "MCP server for AI assistants",
    ],
    "inLanguage": "en",
    "isAccessibleForFree": True,
}


# ---- Help: docs/*.md -------------------------------------------------------------------------

# Repository files that have a page here: links to them stay on the site.
ON_SITE = {
    "CONTRIBUTING.md": "/contributing.html",
    "SECURITY.md": "/security.html",
    "LICENSE": "/license.html",
    "CHANGELOG.md": "/news.html",
    "docs/bench/run5/REPORT.md": "/benchmark.html#report",
    "docs/bench": "/benchmark.html",
    "docs/automation-api.schema.json": "/automation-api.schema.json",
}


def doc_link(target: str, here: Path) -> str:
    """A link inside a docs page, made to work on the site."""
    if re.match(r"^[a-z]+:", target) or target.startswith("#"):
        return target
    path, _, frag = target.partition("#")
    frag = "#" + frag if frag else ""
    if path.endswith(".md") and "/" not in path and here.parent == ROOT / "docs":
        return ("/help/" if path == "README.md" else f"/help/{path[:-3]}.html") + frag
    if path.startswith("images/"):
        return f"/help/{path}"
    resolved = (here.parent / path).resolve()
    try:
        rel = resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return target
    if rel in ON_SITE:
        return ON_SITE[rel] + frag
    if rel.startswith("docs/") and rel.endswith(".md") and rel.count("/") == 1:
        return doc_link(rel[5:], ROOT / "docs" / "x.md") + frag
    kind = "tree" if resolved.is_dir() else "blob"
    return f"{REPO}/{kind}/main/{rel}{frag}"


def doc_sections() -> list[tuple[str, list[tuple[str, str]]]]:
    """docs/README.md's sections and their pages: the help sidebar."""
    out: list[tuple[str, list[tuple[str, str]]]] = []
    for line in (ROOT / "docs" / "README.md").read_text("utf-8").splitlines():
        if line.startswith("## "):
            out.append((line[3:].strip(), []))
        elif out:
            m = re.match(r"- \*\*\[([^\]]+)\]\(([^)#]+\.md)\)\*\*", line)
            if m and m.group(2) not in [p for _, p in out[-1][1]]:
                out[-1][1].append((m.group(1), m.group(2)))
    return [(s, pages) for s, pages in out if pages]


def image_size(path: Path) -> tuple[int, int] | None:
    """A PNG's or WebP's width and height, read from its header (no imaging library)."""
    try:
        head = path.read_bytes()[:40]
    except OSError:
        return None
    if head[:8] == b"\x89PNG\r\n\x1a\n":
        return int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        kind = head[12:16]
        if kind == b"VP8L":
            b = int.from_bytes(head[21:25], "little")
            return (b & 0x3FFF) + 1, ((b >> 14) & 0x3FFF) + 1
        if kind == b"VP8X":
            return int.from_bytes(head[24:27], "little") + 1, int.from_bytes(head[27:30], "little") + 1
        if kind == b"VP8 ":
            return int.from_bytes(head[26:28], "little") & 0x3FFF, int.from_bytes(head[28:30], "little") & 0x3FFF
    return None


def dark_of(name: str) -> str | None:
    """The night version of docs/images/<name>, when there is one: x-dark.webp beside x.webp
    (the help pictures, tools/shots/make.py), or main-dark.webp beside main-light.webp."""
    p = Path(name)
    stem = p.stem[: -len("-light")] if p.stem.endswith("-light") else p.stem
    dark = p.with_name(f"{stem}-dark{p.suffix}").as_posix()
    return dark if dark != name and (ROOT / "docs" / "images" / dark).is_file() else None


IMG = re.compile(r'<img\b([^>]*?)\s*/?>')


def shot_html(attrs: str, lazy: bool) -> str:
    """A help picture as the page shows it: its real size (width and height, so nothing moves as
    it loads), lazy below the first one, and its night version beside it, the site's theme
    choosing which shows (.for-light / .for-dark, as on the home page: they follow the system's
    choice and the header's switch alike; a hidden lazy picture isn't fetched)."""
    a = dict(re.findall(r'(\w[\w-]*)="([^"]*)"', attrs))
    src = a.get("src", "")
    m = re.match(r"/help/images/([\w./-]+)$", src)
    if not m:
        return f"<img{attrs}>"
    name = m.group(1)
    size = image_size(ROOT / "docs" / "images" / name)
    dims = f' width="{size[0]}" height="{size[1]}"' if size else ""
    alt = a.get("alt", "")
    load = ' loading="lazy" decoding="async"' if lazy else ' decoding="async"'
    dark = dark_of(name)
    if not dark:
        return f'<img src="{src}"{dims} alt="{alt}"{load}>'
    dsize = image_size(ROOT / "docs" / "images" / dark)
    ddims = f' width="{dsize[0]}" height="{dsize[1]}"' if dsize else dims
    return (f'<img class="for-light" src="{src}"{dims} alt="{alt}"{load}>'
            f'<img class="for-dark" src="/help/images/{dark}"{ddims} alt="{alt}" loading="lazy" decoding="async">')


def help_figures(body: str) -> str:
    """docs/ pictures into figures. The Markdown stays plain for GitHub:

        ![What it shows](images/help/add-url.webp)
        *The caption.*

    is a figure with its caption (an image, then an italic line, in one paragraph); a picture
    alone in its paragraph is a figure without one. A numbered list after `<!-- steps -->` whose
    items are each a picture and its caption is a step strip: the pictures side by side, in
    order, each under its step; a bulleted list after `<!-- pictures -->`, pictures side by
    side without numbers. Every help picture gets its size, lazy loading after the first, and
    its night version."""
    seen = [0]

    def pic(attrs: str) -> str:
        seen[0] += 1
        return shot_html(attrs, lazy=seen[0] > 1)

    def figure(inner: str, cls: str = "shot") -> str | None:
        m = re.fullmatch(r"\s*(?:<p>)?\s*<img\b([^>]*?)\s*/?>\s*(?:<br\s*/?>)?\s*(?:<em>((?:(?!</?p>).)*?)</em>)?\s*(?:</p>)?\s*", inner, re.S)
        if not m:
            return None
        cap = f"<figcaption>{m.group(2).strip()}</figcaption>" if m.group(2) else ""
        # A step reads what to do, then shows it.
        if cls == "step":
            return f'<figure class="{cls}">{cap}{pic(m.group(1))}</figure>'
        return f'<figure class="{cls}">{pic(m.group(1))}{cap}</figure>'

    def strip(m: re.Match) -> str:
        """A step strip (<!-- steps --> and a numbered list) or pictures side by side
        (<!-- pictures --> and a bulleted list)."""
        numbered = m.group(1) is not None
        items = re.findall(r"<li>(.*?)</li>", m.group(1) if numbered else m.group(2), re.S)
        figs = [figure(i, "step" if numbered else "shot") for i in items]
        if not items or None in figs:
            return re.sub(r"<!-- (steps|pictures) -->\s*", "", m.group(0))
        lis = "".join(f"<li>{f}</li>" for f in figs)
        tag, cls = ("ol", "steps") if numbered else ("ul", "pics")
        return f'<{tag} class="{cls} {cls}-{len(items)}">{lis}</{tag}>'

    # In page order, so the first picture is the one not loaded lazily.
    out = []
    pos = 0
    pat = re.compile(r"<!-- steps -->\s*<ol>(.*?)</ol>|<!-- pictures -->\s*<ul>(.*?)</ul>|<p>\s*<img\b[^>]*>\s*(?:<br\s*/?>)?\s*(?:<em>(?:(?!</?p>).)*?</em>)?\s*</p>|<img\b[^>]*>", re.S)
    for m in pat.finditer(body):
        out.append(body[pos:m.start()])
        text = m.group(0)
        if text.startswith("<!--"):
            out.append(strip(m))
        elif text.startswith("<p>"):
            out.append(figure(text) or text)
        else:
            out.append(pic(IMG.match(text).group(1)))
        pos = m.end()
    out.append(body[pos:])
    return "".join(out)


def build_help() -> None:
    sections = doc_sections()
    docs = sorted((ROOT / "docs").glob("*.md"))
    titles = {}
    for f in docs:
        first = f.read_text("utf-8").splitlines()[0]
        titles[f.name] = first.lstrip("# ").strip()
    used_images: set[str] = set()
    for f in docs:
        text = f.read_text("utf-8")
        text = re.sub(r"(?m)^\[← Back to docs index\]\(README\.md\)\s*\n", "", text)
        text = re.sub(r"(!?\[[^\]]*\])\(([^)\s]+)\)", lambda m: f"{m.group(1)}({doc_link(m.group(2), f)})", text)
        used_images.update(re.findall(r"/help/images/([\w./-]+)", text))
        lines = text.splitlines()
        h1 = lines[0].lstrip("# ").strip()
        rest = "\n".join(lines[1:])
        first_para = next((p for p in rest.split("\n\n") if p.strip() and not p.lstrip().startswith(("!", "#", "-", "|", ">"))), h1)
        is_index = f.name == "README.md"
        path = "/help/" if is_index else f"/help/{f.stem}.html"
        heading = "Help and tutorials" if is_index else re.sub(r"^Tutorial: (.)", lambda m: m.group(1).upper(), h1)
        title = (f"{NAME} help: user guide and tutorials" if is_index
                 else f"{heading} — {NAME} help")
        desc = clip(first_para)
        trail = [("Home", "/"), ("Help", "/help/")] + ([] if is_index else [(heading, path)])
        nav = ['<nav class="doc-nav" aria-label="Help pages">']
        for sec, pages in sections:
            nav.append(f"<p>{esc(sec)}</p>")
            for label, p in pages:
                href = f"/help/{p[:-3]}.html"
                cur = ' aria-current="page"' if p == f.name else ""
                nav.append(f'<a href="{href}"{cur}>{esc(label)}</a>')
        nav.append("</nav>")
        related = ""
        if not is_index:
            sib = next((pages for _, pages in sections if any(p == f.name for _, p in pages)), [])
            others = [(l, p) for l, p in sib if p != f.name][:5]
            if others:
                related = '<aside class="related"><h2>Related</h2><ul>' + "".join(
                    f'<li><a href="/help/{p[:-3]}.html">{esc(l)}</a></li>' for l, p in others) + "</ul></aside>"
        body = f"""{page_head(esc(heading), trail=trail)}
    <div class="wrap doc">
      {"".join(nav)}
      <article class="prose">
        {help_figures(md(rest))}
        {related}
        {page_tools(f"docs/{f.name}", path, heading)}
        {cta_band()}
      </article>
    </div>"""
        md_path = "/help/index.md" if is_index else f"/help/{f.stem}.md"
        text_md = re.sub(r"\]\((/[^)\s]*)\)", lambda m: f"]({SITE}{m.group(1)})", f"# {heading}\n{rest}")
        (OUT / md_path.lstrip("/")).parent.mkdir(parents=True, exist_ok=True)
        (OUT / md_path.lstrip("/")).write_text(text_md.strip() + "\n", "utf-8", newline="\n")
        DOCS_MD.append((md_path, heading, desc, text_md))
        ld = [crumbs_ld(trail)]
        if not is_index:
            ld.append({"@context": "https://schema.org", "@type": "TechArticle", "headline": heading,
                       "description": desc, "inLanguage": "en", "dateModified": TODAY,
                       "about": {"@type": "SoftwareApplication", "name": NAME},
                       "author": {"@type": "Person", "name": "Ahmed Nefzaoui"}})
        write(path, title, desc, body, section="help", jsonld=ld, og_type="article",
              priority="0.7" if f.stem.startswith("guide-") or is_index else "0.6",
              extra=f'\n  <link rel="alternate" type="text/markdown" href="{md_path}">')
    dst = OUT / "help" / "images"
    used_images |= {d for d in map(dark_of, used_images) if d}
    for name in used_images:
        src = ROOT / "docs" / "images" / name
        if src.is_file():
            (dst / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst / name)


# ---- FAQ: website/content/faq.md ----------------------------------------------------------------

def slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def build_faq() -> None:
    text = (CONTENT / "faq.md").read_text("utf-8").replace("{{REPO}}", REPO)
    groups: list[tuple[str, list[tuple[str, str, str]]]] = []  # (group, [(q, slug, answer md)])
    q = None
    for block in re.split(r"(?m)^(#{2,3} .+)$", text)[1:]:
        if block.startswith("## "):
            groups.append((block[3:].strip(), []))
        elif block.startswith("### "):
            m = re.match(r"### (.+?)(?:\s*\{#([\w-]+)\})?$", block.strip())
            q = (m.group(1).strip(), m.group(2) or slug(m.group(1)))
        elif q:
            groups[-1][1].append((q[0], q[1], block.strip()))
            q = None
    allq = [x for _, qs in groups for x in qs]
    FAQ_MD.extend((question, f"/faq/{s}.html", a) for question, s, a in allq)
    trail = [("Home", "/"), ("FAQ", "/faq/")]
    parts = []
    for g, qs in groups:
        parts.append(f'<h2 id="{slug(g)}">{esc(g)}</h2><ul class="faq-list">')
        parts += [f'<li><a href="/faq/{s}.html">{esc(question)}</a></li>' for question, s, _ in qs]
        parts.append("</ul>")
    ld = [crumbs_ld(trail), {
        "@context": "https://schema.org", "@type": "FAQPage",
        "mainEntity": [{"@type": "Question", "name": question,
                        "acceptedAnswer": {"@type": "Answer", "text": md(a)}} for question, _, a in allq],
    }]
    write("/faq/", f"Questions and answers — {NAME}",
          f"Answers about {NAME}: is it free, is it safe, how it compares with IDM, browsers, videos, torrents, speed, Mac and Linux, and more.",
          f"""{page_head("Questions and answers", "Short answers to what people ask most, each with a link to the full guide.", trail)}
    <div class="wrap narrow prose page-body">
      {"".join(parts)}
      {page_tools("website/content/faq.md", "/faq/", "Questions and answers")}
      {cta_band()}
    </div>""", section="faq", jsonld=ld, priority="0.8")
    for gi, (g, qs) in enumerate(groups):
        for question, s, answer in qs:
            path = f"/faq/{s}.html"
            t = trail + [(question, path)]
            others = [x for x in qs if x[1] != s][:6]
            rel = '<aside class="related"><h2>More questions</h2><ul>' + "".join(
                f'<li><a href="/faq/{o[1]}.html">{esc(o[0])}</a></li>' for o in others) + "</ul></aside>"
            ld = [crumbs_ld(t), {"@context": "https://schema.org", "@type": "QAPage",
                                 "mainEntity": {"@type": "Question", "name": question, "answerCount": 1,
                                                "acceptedAnswer": {"@type": "Answer", "text": plain(md(answer))}}}]
            write(path, f"{question} — {NAME}", clip(md(answer)), f"""{page_head(esc(question), trail=t)}
    <div class="wrap narrow prose page-body">
      {md(answer)}
      {rel}
      {page_tools("website/content/faq.md", path, question)}
      {cta_band()}
    </div>""", section="faq", jsonld=ld, og_type="article")


# ---- News: CHANGELOG.md -------------------------------------------------------------------------

def release_thanks(ver: str) -> str:
    """A release's closing thanks: everyone active on GitHub between the release before it and
    this one (tools/contributors.py window()), as round photos. Empty for a release GitHub doesn't
    list, or one nobody but the maintainer worked on."""
    if CREW is None or not CREW_RELEASES:
        return ""
    rel = next((r for r in CREW_RELEASES if contributors.vparts(r["version"]) == contributors.vparts(ver)), None)
    people = contributors.for_release(rel["tag"], CREW_RELEASES) if rel else None
    if not people:
        return ""
    before = [r["version"] for r in CREW_RELEASES if contributors.vparts(r["version"]) < contributors.vparts(ver)]
    since = (f'<p class="fine">Everyone who sent a pull request, opened an issue or commented on GitHub '
             f'since version <span translate="no">{esc(max(before, key=contributors.vparts))}</span>.</p>' if before else "")
    return f"""<div class="release-thanks">
          <h3>Thanks to</h3>
          {faces(people, 40)}
          {since}
        </div>"""


def build_news() -> None:
    text = (ROOT / "CHANGELOG.md").read_text("utf-8")
    releases = []
    for m in re.finditer(r"(?ms)^## \[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}))?\s*\n(.*?)(?=^## |\Z)", text):
        ver, date, body = m.group(1), m.group(2), m.group(3).strip()
        # Changes not released yet aren't news: a version past the published one comes with its
        # release (the release rebuilds the site).
        if ver.lower() == "unreleased" or (re.fullmatch(r"[\d.]+", ver) and vparts(ver) > vparts(VERSION)):
            continue
        body = re.sub(r"\]\((docs/[^)]+)\)", lambda x: "](" + doc_link(x.group(1).split("/", 1)[1], ROOT / "docs" / "x.md") + ")", body)
        releases.append((ver, date, body))
    trail = [("Home", "/"), ("News", "/news.html")]
    parts = []
    for ver, date, body in releases:
        when = f'<time datetime="{date}">{date}</time>' if date else '<time>Coming soon</time>'
        parts.append(f"""<section class="release" id="v{esc(ver)}">
        <h2>What's new in version {esc(ver)} {when}</h2>
        {md(body)}
        <p><a href="{REPO}/releases/tag/v{esc(ver)}">Release page and files on GitHub</a></p>
        {release_thanks(ver)}
      </section>""")
    write("/news.html", f"What's new — {NAME} version history",
          f"Every {NAME} release and what it changed, newest first: new features, fixes and downloads.",
          f"""{page_head("What's new", f"Every release and what it changed, newest first. Follow along with the <a href=\"/news.xml\">news feed</a> or watch the project on GitHub.", trail)}
    <div class="wrap narrow prose page-body">
      {"".join(parts)}
      {page_tools("CHANGELOG.md", "/news.html", "What's new")}
      {cta_band()}
    </div>""", section="news", jsonld=[crumbs_ld(trail)], priority="0.8")
    # Atom feed
    entries = []
    for ver, date, body in releases:
        updated = (date or TODAY) + "T00:00:00Z"
        entries.append(f"""  <entry>
    <title>{esc(NAME)} {esc(ver)}</title>
    <id>{SITE}/news.html#v{esc(ver)}</id>
    <link href="{SITE}/news.html#v{esc(ver)}"/>
    <updated>{updated}</updated>
    <content type="html">{esc(md(body))}</content>
  </entry>""")
    feed = f"""<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>{esc(NAME)} news</title>
  <id>{SITE}/news.xml</id>
  <link href="{SITE}/news.xml" rel="self"/>
  <link href="{SITE}/news.html"/>
  <updated>{TODAY}T00:00:00Z</updated>
  <author><name>Ahmed Nefzaoui</name></author>
{chr(10).join(entries)}
</feed>
"""
    (OUT / "news.xml").write_text(feed, "utf-8", newline="\n")


# ---- Contributors: tools/contributors.py ----------------------------------------------------------

def build_contributors() -> None:
    """/contributors.html: everyone the home page thanks, in their groups, with what they did."""
    path = "/contributors.html"
    trail = [("Home", "/"), ("Contributors", path)]
    groups = [("pr", "Pull requests"), ("issue", "Issues"), ("comment", "Comments")]
    words = {"prs": ("pull request", "pull requests"), "issues": ("issue", "issues"), "comments": ("comment", "comments")}
    sheets = []
    for key, name in groups:
        people = [p for p in CREW or [] if p["group"] == key]
        if not people:
            continue
        rows = []
        for p in people:
            counts = "".join(
                f'<li><b translate="no">{p[k]}</b> {w[0] if p[k] == 1 else w[1]}</li>'
                for k, w in words.items() if p[k])
            # The login in a block of its own marked translate="no": an inline one alone in its
            # block would still go to the translators (sitei18n unwraps a lone element).
            # The photo links there too, for the pointer only (the login is the link to tab to).
            rows.append(f'<li><a class="roster-pic" href="{esc(p["profile"])}" tabindex="-1" aria-hidden="true"><img src="{esc(contributors.avatar(p["avatar"], 96))}" '
                        f'width="48" height="48" alt="" loading="lazy" decoding="async"></a>'
                        f'<p class="roster-who" translate="no"><a href="{esc(p["profile"])}">{esc(p["login"])}</a></p>'
                        f'<ul class="roster-counts" role="list">{counts}</ul></li>')
        sheets.append(f"""<section class="sheet sheet-labelled roster-sheet" aria-labelledby="crew-{key}">
        <h2 class="sheet-band" id="crew-{key}"><b>{name}</b><span>{people_count(len(people))}</span></h2>
        <ul class="roster" role="list">{"".join(rows)}</ul>
      </section>""")
    if CREW is None:
        body = f"""<div class="page-body prose">
        <p>The list couldn't be read from GitHub when this page was built. Everyone is on GitHub meanwhile, in the project's <a href="{REPO}/pulls?q=is%3Apr">pull requests</a> and <a href="{REPO}/issues?q=is%3Aissue">issues</a>.</p>
      </div>"""
    else:
        body = "\n      ".join(sheets)
    lede = ("Made possible by these amazing humans: everyone who sent a pull request, opened an issue or "
            "joined a conversation on GitHub. Thank you, every one of you.")
    fine = (f"From GitHub's public record, <time datetime=\"{TODAY}\" translate=\"no\">{TODAY}</time>: people with pull requests "
            "first, then those who opened issues, then those who commented, each once. The maintainer and "
            "automated accounts aren't counted.")
    write(path, f"Contributors — {NAME}",
          f"The people who made {NAME} possible: everyone who sent a pull request, opened an issue or joined a conversation on GitHub.",
          f"""{page_head("Contributors", lede, trail, fine)}
    <div class="wrap crew-page">
      {body}
      <p class="crew-join"><a class="btn btn-line" href="/contributing.html">Join them: the contributing guide</a></p>
    </div>""", jsonld=[crumbs_ld(trail)], priority="0.5")


# ---- Languages: i18n/languages.tsv ---------------------------------------------------------------

def build_languages() -> None:
    rows = [l.split("\t") for l in (ROOT / "i18n" / "languages.tsv").read_text("utf-8").splitlines()
            if l and not l.startswith("#")]
    flags = ROOT / "crates" / "app" / "assets" / "flags"
    (OUT / "assets" / "flags").mkdir(parents=True, exist_ok=True)
    items = []
    for code, english, native, flag, *_ in sorted(rows, key=lambda r: r[1]):
        src = flags / f"{flag}.svg"
        img = ""
        if src.is_file():
            shutil.copy2(src, OUT / "assets" / "flags" / src.name)
            img = f'<img src="/assets/flags/{flag}.svg" width="24" height="18" alt="" loading="lazy">'
        tsv = f"{REPO}/blob/main/i18n/{code}.tsv"
        items.append(f'<li>{img}<span><a href="{tsv}" lang="{esc(code)}">{esc(native)}</a><small>{esc(english)}</small></span></li>')
    trail = [("Home", "/"), ("Languages", "/languages.html")]
    body = f"""{page_head(f"{NAME} in {len(rows)} languages", "Every display language Windows 11 offers, chosen by flag on the first start or any time in View → Language. Arabic, Hebrew, Persian, Urdu and Uyghur are laid out right to left.", trail)}
    <div class="wrap prose page-body" style="max-width:1180px">
      <h2>Every language</h2>
      <ul class="lang-grid" translate="no">{"".join(items)}</ul>
      <h2>Help translate</h2>
      <p>Most translations were made by machine and are waiting for a native speaker. If MDM speaks your language a little oddly, you can fix it in a few minutes, no programming needed:</p>
      <ol>
        <li>Click your language above: it opens its file on GitHub, one text per row.</li>
        <li>Correct the translation in the third column and delete the row's <code>#, machine</code> line.</li>
        <li>Propose the change with GitHub's pencil button. Your name goes into the release notes.</li>
      </ol>
      <p>The full guide is in the <a href="/contributing.html">contributing guide</a>, and <a href="/help/languages.html">Languages</a> in the help explains the picker and right-to-left layouts.</p>
      {cta_band()}
    </div>"""
    write("/languages.html", f"{NAME} in {len(rows)} languages — translations",
          f"{NAME} speaks {len(rows)} languages, every one Windows 11 offers, right to left included. See them all and help improve your own.",
          body, jsonld=[crumbs_ld(trail)], priority="0.6")


# ---- Repository documents shown as pages --------------------------------------------------------

def build_repo_pages() -> None:
    """CONTRIBUTING.md as /contributing.html, its links kept working on the site."""
    f = ROOT / "CONTRIBUTING.md"
    text = f.read_text("utf-8")
    text = re.sub(r"(!?\[[^\]]*\])\(([^)\s]+)\)", lambda m: f"{m.group(1)}({doc_link(m.group(2), f)})", text)
    lines = text.splitlines()
    # The page's lede says what the file's first paragraph does.
    rest = "\n".join(lines[1:]).strip().split("\n\n", 1)[1]
    path = "/contributing.html"
    trail = [("Home", "/"), ("Contributing", path)]
    lede = ("Bug reports, ideas, translation fixes and pull requests are all welcome. "
            f'Everything happens in the open on <a href="{REPO}">GitHub</a>.')
    body = f"""{page_head("Contribute to Mister Download Manager", lede, trail)}
    <div class="wrap narrow prose page-body">
      <div class="contrib-ways">
        <a href="{REPO}/issues/new?template=bug.yml"><b>Report a bug</b><span>Something broken or odd? Tell us what you saw.</span></a>
        <a href="/languages.html"><b>Fix a translation</b><span>Five minutes on GitHub, no programming.</span></a>
        <a href="{REPO}/issues"><b>Pick an issue</b><span>See what's open and lend a hand.</span></a>
        <a href="{REPO}"><b>Star the project</b><span>It helps other people find it.</span></a>
      </div>
      {md(rest)}
      {page_tools("CONTRIBUTING.md", path, "Contributing")}
    </div>"""
    write(path, f"Contribute — {NAME}",
          "How to help Mister Download Manager: report bugs, fix translations, build it from source and send pull requests. The checks CI runs and the project's rules.",
          body, jsonld=[crumbs_ld(trail)], priority="0.5")


def bench_report() -> str:
    """docs/bench/run5/REPORT.md's table, for the benchmark page."""
    text = (ROOT / "docs" / "bench" / "run5" / "REPORT.md").read_text("utf-8")
    table = "\n".join(l for l in text.splitlines() if l.startswith("|"))
    env = next((l for l in text.splitlines()[1:] if l.strip()), "")
    return md(table) + f'<p class="fine">{esc(env)}</p>'


# ---- Pages written as Markdown (legal) -----------------------------------------------------------

def front(text: str) -> tuple[dict, str]:
    m = re.match(r"(?s)^---\n(.*?)\n---\n(.*)$", text)
    if not m:
        return {}, text
    meta = dict(l.split(": ", 1) for l in m.group(1).splitlines() if ": " in l)
    return meta, m.group(2)


def build_content_pages() -> None:
    for f in sorted(CONTENT.glob("*.md")):
        if f.name == "faq.md":
            continue
        raw = f.read_text("utf-8").replace("{{VERSION}}", VERSION).replace("{{REPO}}", REPO)
        meta, text = front(raw.replace("{{TODAY}}", TODAY))
        if "{{THIRD_PARTY}}" in text:
            text = text.replace("{{THIRD_PARTY}}", third_party_table())
        path = f"/{f.stem}.html"
        trail = [("Home", "/"), (meta["title"], path)]
        body = f"""{page_head(esc(meta["title"]), meta.get("lede", ""), trail, meta.get("fine", ""))}
    <div class="wrap narrow prose page-body">
      {md(text)}
      {page_tools(f"website/content/{f.name}", path, meta["title"])}
    </div>"""
        write(path, f'{meta["title"]} — {NAME}', meta["description"], body,
              jsonld=[crumbs_ld(trail)], priority=meta.get("priority", "0.3"))


def third_party_table() -> str:
    rows = [l.split("\t") for l in (CONTENT / "third-party.tsv").read_text("utf-8").splitlines()
            if l and not l.startswith("#")]
    body = "".join(
        f'<tr><td>{f"<a href={chr(34)}{esc(r[3])}{chr(34)}>{esc(r[0])}</a>" if len(r) > 3 and r[3] else esc(r[0])}</td>'
        f"<td>{esc(r[1])}</td><td>{esc(r[2])}</td></tr>" for r in rows)
    return (f'<div class="table-wrap"><table class="crates" translate="no"><thead><tr><th>Library</th><th>Version</th>'
            f'<th>Licence</th></tr></thead><tbody>{body}</tbody></table></div>\n\n'
            f'{len(rows)} libraries.')


# ---- Hand-written pages: the home page, download, comparison, 404 --------------------------------

def fill(text: str) -> str:
    for k, v in FILES.items():
        text = text.replace("{{FILE_" + k.upper() + "}}", v)
    if "{{BENCH_REPORT}}" in text:
        text = text.replace("{{BENCH_REPORT}}", bench_report())
    for k in ("stars", "forks", "issues", "watchers"):
        text = text.replace("{{" + k.upper() + "}}", STATS.get(k, "–"))
    text = text.replace("<!--@split-->", split_button()).replace("<!--@gh-->", gh_pill("btn btn-ghost btn-big gh-big"))
    text = text.replace("<!--@stores-->", store_buttons()).replace("<!--@amo-->", amo_button())
    text = text.replace("<!--@cws-->", cws_button())
    text = text.replace("{{AMO}}", AMO).replace("{{CWS}}", CWS)
    if "<!--@features-->" in text:
        text = text.replace("<!--@features-->", manifest(feature_list()))
    text = coming_marks(text.replace("<!--@coming-->", coming_banner()))
    text = text.replace("<!--@crew-->", crew_pile())
    text = text.replace("<!--@fields-->", waybill_fields()).replace("<!--@barcode-->", barcode.svg(f"MDM {VERSION}"))
    return (text.replace("{{VERSION}}", VERSION).replace("{{REPO}}", REPO).replace("{{SITE}}", SITE)
            .replace("{{SPONSORS}}", SPONSORS).replace("{{SOFTWARE_LD}}", json.dumps(SOFTWARE_LD, ensure_ascii=False, separators=(",", ":"))))


def build_templated(name: str, path: str, section: str, priority: str) -> None:
    """website/<name>: a full page whose <!--@header--> and <!--@footer--> are filled in."""
    text = fill((WEB / name).read_text("utf-8"))
    text = text.replace("<!--@header-->", header(section)).replace("<!--@footer-->", footer())
    text = text.replace("<!--@theme-->", THEME_SCRIPT)
    target = OUT / (path.lstrip("/") or "index.html")
    if path.endswith("/"):
        target = OUT / path.lstrip("/") / "index.html"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, "utf-8", newline="\n")
    if name != "404.html":
        PAGES.append((path, priority))


# ---- Features: website/content/features/*.md, and the Themes page --------------------------------

FEATURES_DIR = CONTENT / "features"
# The features' groups, in the order the tiles and the index show them.
GROUPS = [("downloading", "Downloading"), ("organising", "Organising"), ("network", "Network"),
          ("app", "The app")]
# docs/images files the feature and theme pages show (copied to /help/images/ as the help's are).
FEATURE_IMAGES: set[str] = set()


def vparts(v: str) -> tuple[int, ...]:
    """A version's numbers, padded to three: "0.5" and "0.5.0" are the same version."""
    parts = tuple(int(x) for x in re.findall(r"\d+", v or ""))
    return parts + (0,) * (3 - len(parts))


def upcoming() -> str:
    """UPCOMING while it is still to come: "" once it is unset or a release at or past it is out."""
    return UPCOMING if UPCOMING and vparts(VERSION) < vparts(UPCOMING) else ""


def is_coming(since: str) -> bool:
    """First in UPCOMING (or later): in the source, not in the download yet."""
    up = upcoming()
    return bool(up) and vparts(since) >= vparts(up)


def coming_banner(kind: str = "page") -> str:
    """The one "Coming with version 0.5" mark, wherever something isn't in the download yet: on
    a page ("page"), on a part of one ("section"), on a tile ("tag"). Empty once UPCOMING is out
    (see UPCOMING), so every one goes at once."""
    up = upcoming()
    if not up:
        return ""
    if kind == "tag":
        return f'<span class="coming coming-tag"><svg aria-hidden="true"><use href="#i-clock"/></svg>Coming with version {up}</span>'
    note = (f"It's in the source code now, not yet in the download (version {VERSION})." if kind == "page"
            else f"This part isn't in the download (version {VERSION}) yet.")
    return (f'<p class="coming coming-{kind}"><svg aria-hidden="true"><use href="#i-clock"/></svg>'
            f'<span><b>Coming with version {up}</b> <small>{note}</small></span></p>')


def coming_marks(text: str) -> str:
    """<!--@coming 0.5--> in a page: the section's banner while that version is to come."""
    return re.sub(r"(?:<p>\s*)?<!--@coming ([\d.]+)-->(?:\s*</p>)?",
                  lambda m: coming_banner("section") if is_coming(m.group(1)) else "", text)


def feature_list() -> list[dict]:
    """Every feature file's front matter (and its body), by group, then `order`."""
    out = []
    for f in sorted(FEATURES_DIR.glob("*.md")):
        meta, body = front(f.read_text("utf-8"))
        meta = {k.strip(): v.strip() for k, v in meta.items()}
        for key in ("name", "line", "group", "order", "since"):
            if not meta.get(key):
                sys.exit(f"{f.relative_to(ROOT)}: no {key}")
        if meta["group"] not in dict(GROUPS):
            sys.exit(f"{f.relative_to(ROOT)}: group {meta['group']!r} isn't one of {[g for g, _ in GROUPS]}")
        meta.update(slug=f.stem, body=body, file=f, href=meta.get("href") or f"/features/{f.stem}.html")
        out.append(meta)
    rank = {g: i for i, (g, _) in enumerate(GROUPS)}
    return sorted(out, key=lambda m: (rank[m["group"]], int(m["order"])))


def picture(src: str, alt: str, lazy: bool = False) -> str:
    """A screenshot and its night version, the site's theme choosing which shows: `images/...`
    (docs/images, as the help pages have them) or `/assets/screens/<x>-light.webp` (the home
    page's)."""
    load = ' loading="lazy" decoding="async"' if lazy else ' decoding="async"'
    if src.startswith("/assets/"):
        dark = src.replace("-light.", "-dark.")
        size = image_size(WEB / src.lstrip("/"))
        dsize = image_size(WEB / dark.lstrip("/")) if dark != src else None
        url, durl = src, dark
    else:
        name = src.removeprefix("images/")
        size = image_size(ROOT / "docs" / "images" / name)
        d = dark_of(name)
        dsize = image_size(ROOT / "docs" / "images" / d) if d else None
        url, durl = f"/help/images/{name}", f"/help/images/{d}"
        FEATURE_IMAGES.update({name} | ({d} if d else set()))
    if not size:
        sys.exit(f"picture {src}: no such picture")
    dims = f' width="{size[0]}" height="{size[1]}"'
    if not dsize:
        return f'<img src="{url}"{dims} alt="{esc(alt)}"{load}>'
    return (f'<img class="for-light" src="{url}"{dims} alt="{esc(alt)}"{load}>'
            f'<img class="for-dark" src="{durl}" width="{dsize[0]}" height="{dsize[1]}" alt="{esc(alt)}" loading="lazy" decoding="async">')


def manifest(feats: list[dict], heading: str = "h3") -> str:
    """Every feature in one ruled packing list, by group, each a link to its page: the home
    page's contents and /features/."""
    rows = []
    for g, label in GROUPS:
        items = "".join(
            f'<li><a href="{m["href"]}"><b>{esc(m["name"])}</b><span>{esc(m["line"])}</span>'
            f'{coming_banner("tag") if is_coming(m["since"]) else ""}'
            f'<svg class="mf-arrow" aria-hidden="true"><use href="#i-arrow"/></svg></a></li>'
            for m in feats if m["group"] == g)
        rows.append(f'<div class="mf-group"><{heading} class="mf-name">{label}</{heading}>'
                    f'<ul class="mf-list" role="list">{items}</ul></div>')
    soon = sum(is_coming(m["since"]) for m in feats)
    fine = f"{len(feats)} features, every one free" + (f" · {soon} coming with version {upcoming()}" if soon else "")
    return (f'<div class="sheet sheet-labelled manifest"><p class="sheet-band"><b>Contents</b>'
            f'<span>{fine}</span></p>{"".join(rows)}</div>')


def site_md(text: str) -> str:
    """A feature page's Markdown, written as docs/ pages are (their links and pictures), as HTML."""
    here = ROOT / "docs" / "x.md"
    text = re.sub(r"(!?\[[^\]]*\])\(([^)\s]+)\)", lambda m: f"{m.group(1)}({doc_link(m.group(2), here)})", text)
    used = set(re.findall(r"/help/images/([\w./-]+)", text))
    FEATURE_IMAGES.update(used | {d for d in map(dark_of, used) if d})
    return coming_marks(help_figures(md(text)))


def help_href(ref: str) -> str:
    page, _, frag = ref.partition("#")
    return doc_link(page, ROOT / "docs" / "x.md") + (f"#{frag}" if frag else "")


def feature_label(m: dict) -> str:
    """A feature's facts in the waybill's field grammar, stuck on its picture."""
    if is_coming(m["since"]):
        since = f'<dd>Version {esc(upcoming())} <small>coming</small></dd>'
    else:
        since = f'<dd><a href="/news.html#v{esc(m["since"])}">Version {esc(m["since"])}</a> <small>and later</small></dd>'
    rows = []
    if m.get("where"):
        where = re.sub(r"`([^`]+)`", r'<code translate="no">\1</code>', esc(m["where"]))
        rows.append(f'<div class="wide"><dt>In the app</dt><dd>{where}</dd></div>')
    rows.append(f"<div><dt>Since</dt>{since}</div>")
    rows.append('<div class="pair"><dt>Postage</dt><dd>Free</dd></div>')
    if m.get("help"):
        rows.append(f'<div class="wide"><dt>Full guide</dt><dd><a href="{help_href(m["help"])}">Step by step in the help</a></dd></div>')
    return (f'<aside class="waybill feature-label" aria-label="At a glance"><div class="wb-band">'
            f'<img src="/assets/logo-64.png" width="32" height="32" alt=""><span class="wb-from"><b>{esc(m["name"])}</b>'
            f'<small>Mister Download Manager</small></span></div><dl class="fields">{"".join(rows)}</dl></aside>')


def build_features() -> None:
    feats = feature_list()
    index_trail = [("Home", "/"), ("Features", "/features/")]
    for m in feats:
        if m["href"] != f"/features/{m['slug']}.html":
            continue  # a feature with a page of its own elsewhere (Themes)
        for key in ("title", "headline", "lede", "description"):
            if not m.get(key):
                sys.exit(f"{m['file'].relative_to(ROOT)}: no {key}")
        path = m["href"]
        trail = index_trail + [(m["name"], path)]
        shot = (f'<figure class="feature-shot" data-lightbox>{picture(m["shot"], m.get("shot_alt") or m["name"])}</figure>'
                if m.get("shot") else "")
        others = [x for x in feats if x["group"] == m["group"] and x is not m]
        more = "".join(f'<li><a href="{x["href"]}">{esc(x["name"])}</a></li>' for x in others)
        group = dict(GROUPS)[m["group"]]
        body = f"""{page_head(esc(m["headline"]), esc(m["lede"]), trail)}
    <div class="wrap feature-top">
      {coming_banner() if is_coming(m["since"]) else ""}
      <div class="feature-parcel{"" if shot else " no-shot"}">{shot}{feature_label(m)}</div>
    </div>
    <div class="wrap narrow page-body prose feature-body">
      {site_md(m["body"])}
      <aside class="related more-features"><h2>More in {esc(group)}</h2><ul>{more}<li><a href="/features/">Every feature</a></li></ul></aside>
      {page_tools(f"website/content/features/{m['slug']}.md", path, m["name"])}
      {cta_band()}
    </div>"""
        ld = [crumbs_ld(trail), {"@context": "https://schema.org", "@type": "WebPage", "name": m["title"],
                                 "description": m["description"], "inLanguage": "en",
                                 "about": {"@type": "SoftwareApplication", "name": NAME}}]
        write(path, f'{m["title"]} — {NAME}', m["description"], body, section="features", jsonld=ld, priority="0.7")
    body = f"""{page_head("Everything it does", "Every feature has a page of its own: what it does, where it is in the app, and the real window. All of it is free, in every copy.", index_trail)}
    <div class="wrap features-index">
      {manifest(feats, "h2")}
    </div>"""
    write("/features/", f"Features — {NAME}",
          f"Every feature of {NAME}: acceleration and resume, the browser extension, video, torrents, the scheduler, the link grabber, themes and more.",
          body, section="features", jsonld=[crumbs_ld(index_trail)], priority="0.8")


# ---- Themes: website/content/themes.toml, and the theme file format from the app's own code -----

THEME_SAMPLE = '''name = "Harbour"
inherits = "classic"
icons = "color"

[seeds.light]
background = "#eef3f6"
surface = "#ffffff"
text = "#11202b"
accent = "#0f6c8c"

[seeds.dark]
background = "#14212a"
surface = "#0e171d"
text = "#e8f0f4"
accent = "#5cc3e6"

[sizes]
radius = 6

[controls]
focus = "ring"'''


def theme_keys() -> dict[str, list[tuple[str, str]]]:
    """The keys a theme file can set, read from crates/app/src/palette.rs (the colour roles and
    the sizes with their descriptions, the control styles), so the page never drifts from what
    the app reads (themefile.rs)."""
    src = (ROOT / "crates" / "app" / "src" / "palette.rs").read_text("utf-8")
    keys = re.findall(r'(?m)^\s+[a-z_0-9]+: "([a-z0-9.-]+)", "([^"]+)" =', src)
    sizes = [k for k in keys if k[0] in ("border", "focus-line") or k[0].startswith(("radius", "chevron.width", "font.", "list.row", "list.header", "tooltip."))]
    colors = [k for k in keys if k not in sizes]
    styles: dict[str, list[str]] = {}
    for k, v in re.findall(r'\("([a-z-]+)", "([a-z-]+)"\) =>', src):
        styles.setdefault(k, []).append(v)
    if len(colors) < 50 or len(sizes) < 5 or len(styles) < 4:
        sys.exit("crates/app/src/palette.rs: the theme keys couldn't be read (build.py theme_keys)")
    return {"colors": colors, "sizes": sizes, "controls": [(k, ", ".join(v)) for k, v in styles.items()]}


def keys_table(rows: list[tuple[str, str]], head: str) -> str:
    body = "".join(f'<tr><td><code translate="no">{esc(k)}</code></td><td>{esc(d)}</td></tr>' for k, d in rows)
    return f'<div class="table-wrap"><table class="keys"><thead><tr><th>Key</th><th>{head}</th></tr></thead><tbody>{body}</tbody></table></div>'


def build_themes() -> None:
    import tomllib
    data = tomllib.loads((CONTENT / "themes.toml").read_text("utf-8"))
    themes = data["theme"]
    keys = theme_keys()
    since = next(m["since"] for m in feature_list() if m["href"] == "/themes.html")
    tiles = []
    for i, th in enumerate(themes):
        tag = coming_banner("tag") if th.get("tag") and is_coming(since) else ""
        note = f'<p class="theme-note">{esc(th["note"])}</p>' if th.get("note") else ""
        tiles.append(f"""<li class="theme-tile">
          <figure class="theme-shot" data-lightbox>{picture(th["shot"], f"The main window in the {th['name']} theme", lazy=i > 3)}{note}</figure>
          <div class="theme-text"><h3>{esc(th["name"])}</h3><p>{esc(th["feel"])}</p>{tag}</div>
        </li>""")
    tiles.append("""<li class="theme-tile theme-own"><a href="#make-your-own">
          <span class="theme-plus"><svg aria-hidden="true"><use href="#i-plus"/></svg></span>
          <span class="theme-text"><b>Make your own theme</b><span>A text file of colours and sizes, tried in seconds. Three steps.</span></span></a></li>""")
    prompt = (CONTENT / "theme-prompt.txt").read_text("utf-8").strip()
    builtin = ", ".join(f"<code>{esc(t['id'])}</code>" for t in themes if t["id"] != "native"
                        and (ROOT / "crates" / "app" / "assets" / "themes" / f"{t['id']}.toml").is_file())
    trail = [("Home", "/"), ("Themes", "/themes.html")]
    themes_dir = f"{REPO}/tree/main/crates/app/assets/themes"
    body = f"""{page_head("The same window, your way", "View → Theme changes how MDM looks at once and keeps everything where it was. Pick one of the built-in looks, each with a light and a dark side, or make your own in a text file.", trail)}
    <div class="wrap themes">
      {coming_banner()}
      <div class="themes-bar">
        <p>{len(themes)} built-in themes · light and dark</p>
        <div class="variant-seg" role="group" aria-label="Show the themes" data-variant-switch hidden>
          <button type="button" aria-pressed="true" data-variant="light">Light</button><button type="button" aria-pressed="false" data-variant="dark">Dark</button>
        </div>
      </div>
      <ul class="theme-grid" role="list" data-variant-grid>
        {"".join(tiles)}
      </ul>
    </div>
    <div class="wrap narrow page-body prose make-own" id="make-your-own">
      <h2>Make your own theme</h2>
      <p>A theme is one small text file, in <a href="https://toml.io/">TOML</a> (or JSON). It names only what it changes; everything else comes from the theme it builds on. You don't need to build MDM or know how to program.</p>
      <ol class="make-steps">
        <li id="make-it"><h3>Make it</h3>
          <p>Start with a few colours, then add more as you go:</p>
          <pre><code translate="no">{esc(THEME_SAMPLE)}</code></pre>
          <ul>
            <li><code>inherits</code> is the theme yours starts from: <code>classic</code> (the default), {builtin}, or another theme file in the same folder.</li>
            <li><code>[seeds]</code> are a handful of colours (<code>background</code>, <code>surface</code>, <code>header</code>, <code>text</code>, <code>accent</code>, <code>selection</code>, <code>error</code>, <code>warning</code>, <code>success</code>) that MDM spreads over every control. <code>[seeds.light]</code> and <code>[seeds.dark]</code> give each side its own.</li>
            <li><code>[colors]</code> sets single colour roles on both sides, <code>[light]</code> and <code>[dark]</code> on one side only: <code>#rrggbb</code>, or <code>#rrggbbaa</code> with transparency.</li>
            <li><code>[sizes]</code> are corner radii, border widths, font sizes and row heights, in pixels. <code>[controls]</code> chooses the shape of text boxes, check boxes, buttons, progress bars, scroll bars, the focus mark and the title bar.</li>
            <li><code>icons</code> is <code>color</code>, <code>mono</code> or <code>accent</code>. A theme in a folder of its own (<code>my-theme/theme.toml</code>) can bring its icons in <code>icons/16</code> and <code>icons/24</code>, each named for what it shows (<code>tb-add-url.svg</code>, <code>cat-music.png</code>); <code>-symbolic</code> ones take the theme's text colour.</li>
            <li><code>contrast = "high"</code> strengthens every edge and text colour on top.</li>
          </ul>
          <p>A key MDM doesn't know, or a value it can't read, is skipped and written to MDM's log (the <code>logs</code> folder beside the themes folder), so a file made for a newer MDM still loads. The built-in themes are written the same way: <a href="{themes_dir}">read them on GitHub</a>.</p>
          <details class="keys-ref"><summary>Every colour role ({len(keys["colors"])})</summary>{keys_table(keys["colors"], "What it colours")}</details>
          <details class="keys-ref"><summary>Every size ({len(keys["sizes"])})</summary>{keys_table(keys["sizes"], "What it sets")}</details>
          <details class="keys-ref"><summary>Every control style ({len(keys["controls"])})</summary>{keys_table(keys["controls"], "Its styles")}</details>
          <h4 id="ai-prompt">Or let an AI assistant write it</h4>
          <p>Copy this prompt into your AI assistant and describe the look you want at its end. It gives the assistant the format, the keys and the rules: readable text and a visible focus in both sides, and nothing copied from anyone else's artwork. Look at both sides yourself before you share the result.</p>
          <div class="prompt-box">
            <button class="btn btn-line copy-btn" type="button" data-copy="theme-prompt" hidden><svg class="os-ico" aria-hidden="true"><use href="#i-copy"/></svg><span data-copy-label>Copy the prompt</span></button>
            <pre id="theme-prompt"><code translate="no">{esc(prompt)}</code></pre>
          </div>
        </li>
        <li id="try-it"><h3>Try it, without building MDM</h3>
          <p>Put the file in MDM's themes folder. <b>View → Theme → Open themes folder</b> opens it, and makes it the first time. It is in MDM's data folder:</p>
          <dl class="fields os-paths">
            <div><dt>Windows</dt><dd><code translate="no">%APPDATA%\\misterdownloadmanager\\themes</code></dd></div>
            <div><dt>Linux</dt><dd><code translate="no">~/.local/share/misterdownloadmanager/themes</code></dd></div>
            <div><dt>macOS</dt><dd><code translate="no">~/Library/Application Support/misterdownloadmanager/themes</code></dd></div>
            <div><dt>Portable copy</dt><dd><code translate="no">data\\themes</code> beside the program</dd></div>
          </dl>
          <p>Within two seconds it shows in <b>View → Theme</b>, under the built-in themes, by the <code>name</code> it gives itself. Pick it and the whole window changes at once. After you edit the file, pick it again in <b>View → Theme</b> to see the change. <b>View → Dark Mode support</b> and <b>Always dark</b> switch between its light and dark side.</p>
          <p><b>View → Theme → Add a theme file…</b> does it for you: pick the file anywhere, and MDM copies it into the themes folder and uses it at once.</p>
          <h4 id="schemes">Already have a Base16 or pywal scheme?</h4>
          <p>It's a theme already. A <a href="https://github.com/tinted-theming/schemes">Base16 or Base24 scheme</a> (<code>.yaml</code>) or pywal's <code>colors.json</code>, added the same way, changes MDM's colours and nothing else: the scheme's backgrounds behind the list and the windows, its text kept readable (4.5:1 or more), its blue on the focus, the selection and the progress bar. Once <code>wal</code> has run, <b>View → Theme</b> lists <b>pywal</b>, read from <code>~/.cache/wal/colors.json</code> (<code translate="no">%USERPROFILE%\\.cache\\wal\\colors.json</code> on Windows), and MDM follows each new wallpaper's colours within two seconds. <a href="{REPO}/blob/main/docs/themes.md#base16-and-pywal-colour-schemes">The guide</a> shows which colour goes where.</p>
        </li>
        <li id="share-it"><h3>Share it</h3>
          <p>Made something others would like? Open a <a href="{REPO}/pulls">pull request on GitHub</a> that adds it to <a href="{themes_dir}"><code>crates/app/assets/themes</code></a>, with:</p>
          <ul>
            <li>the <code>.toml</code> file, and its folder of icons if it has one;</li>
            <li>screenshots of the main window in its light and its dark side;</li>
            <li>a <code>SOURCES.txt</code> beside any icons, saying who drew them and under which licence. Icons must be your own or under a licence that lets MDM share them (GPL-3.0), never copied from another program.</li>
          </ul>
          <p>Not sure yet? <a href="{REPO}/discussions">Show it in the discussions</a> first. The <a href="/contributing.html">contributing guide</a> explains how pull requests work here.</p>
        </li>
      </ol>
      {page_tools("website/content/themes.toml", "/themes.html", "Themes")}
      {cta_band()}
    </div>"""
    write("/themes.html", f"Themes — {NAME}",
          f"{len(themes)} built-in looks for {NAME}, each light and dark: "
          + ", ".join(t["name"] for t in themes) + ". Or make your own in a text file.",
          body, section="themes", jsonld=[crumbs_ld(trail)], priority="0.7")


def copy_feature_images() -> None:
    dst = OUT / "help" / "images"
    for name in FEATURE_IMAGES:
        src = ROOT / "docs" / "images" / name
        if src.is_file():
            (dst / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst / name)


# ---- For AI assistants and search: llms.txt, llms-full.txt (llmstxt.org) ----------------------

def build_llms() -> None:
    """llms.txt: website/llms.txt (what MDM is, the main pages) and, made here, every help page
    (as Markdown) and question. llms-full.txt: all of it in one file. Built from the same
    sources as the pages, so it never drifts from them."""
    head_md = (WEB / "llms.txt").read_text("utf-8").strip()
    order = [p for _, pages in doc_sections() for _, p in pages]
    rank = {f"/help/{p[:-3]}.md": i for i, p in enumerate(order)}
    docs = sorted((d for d in DOCS_MD if d[0] in rank), key=lambda d: rank[d[0]])
    lines = [head_md, "", "## Help pages", ""]
    lines += [f"- [{t}]({SITE}{p}): {d}" for p, t, d, _ in docs]
    lines += ["", "## Questions and answers", ""]
    lines += [f"- [{q}]({SITE}{p})" for q, p, _ in FAQ_MD]
    lines += ["", "## Optional", "",
              f"- [Everything above in one file]({SITE}/llms-full.txt): the help pages and answers, as Markdown"]
    if (OUT / "automation-api.schema.json").is_file():
        lines.append(f"- [Automation API schema]({SITE}/automation-api.schema.json): JSON Schema of the local "
                     "channel other programs use to list, add and control downloads")
    (OUT / "llms.txt").write_text("\n".join(lines) + "\n", "utf-8", newline="\n")
    full = [head_md, ""]
    for p, _, _, text in docs:
        full += ["", "---", "", f"Source: {SITE}{p}", "", text.strip(), ""]
    full += ["", "---", "", "# Questions and answers", ""]
    for q, p, a in FAQ_MD:
        a = re.sub(r"\]\((/[^)\s]*)\)", lambda m: f"]({SITE}{m.group(1)})", a.replace("{{REPO}}", REPO))
        full += [f"## {q}", "", f"Source: {SITE}{p}", "", a.strip(), ""]
    (OUT / "llms-full.txt").write_text("\n".join(full) + "\n", "utf-8", newline="\n")


# ---- Static files, sitemap, robots ------------------------------------------------------------

def copy_static() -> None:
    # Emptied rather than removed: a preview server may be running in it.
    OUT.mkdir(parents=True, exist_ok=True)
    for old in OUT.iterdir():
        shutil.rmtree(old) if old.is_dir() else old.unlink()
    for item in WEB.iterdir():
        if item.name in ("content",) or item.suffix == ".html":
            continue
        if item.is_dir():
            shutil.copytree(item, OUT / item.name)
        else:
            shutil.copy2(item, OUT / item.name)
    shutil.copy2(ROOT / "docs" / "images" / "social-preview.png", OUT / "assets" / "social.png")
    # The automation API's JSON Schema, at the address its "$id" names.
    schema = ROOT / "docs" / "automation-api.schema.json"
    if schema.is_file():
        shutil.copy2(schema, OUT / "automation-api.schema.json")
    (OUT / "CNAME").write_text("misterdownloadmanager.com\n", "utf-8")
    (OUT / ".nojekyll").write_text("", "utf-8")
    (OUT / "site.webmanifest").write_text(json.dumps({
        "name": NAME, "short_name": "MDM", "start_url": "/", "display": "browser",
        "background_color": "#ffffff", "theme_color": "#0f7b0f",
        "icons": [{"src": "/assets/logo-640.png", "sizes": "640x640", "type": "image/png"},
                  {"src": "/assets/apple-touch-icon.png", "sizes": "180x180", "type": "image/png"}],
    }, indent=2), "utf-8")


def bust_caches() -> None:
    """Every page asks for /styles.css, /main.js and /bench.js by their content hash, so a
    browser never pairs a new page with an old cached stylesheet."""
    import hashlib
    tags = {}
    for name in ("styles.css", "main.js", "bench.js"):
        tags[name] = hashlib.sha256((OUT / name).read_bytes()).hexdigest()[:10]
    for page in OUT.rglob("*.html"):
        text = page.read_text("utf-8")
        new = text
        for name, h in tags.items():
            new = new.replace(f'"/{name}"', f'"/{name}?v={h}"')
        if new != text:
            page.write_text(new, "utf-8", newline="\n")


def finish(available: dict[str, list[str]]) -> None:
    bust_caches()
    # One sitemap per language under an index, each page with its languages (sitei18n).
    sitei18n.sitemaps(OUT, SITE, PAGES, available, TODAY)
    (OUT / "robots.txt").write_text(f"User-agent: *\nAllow: /\n\nSitemap: {SITE}/sitemap.xml\n", "utf-8")


def main() -> None:
    STATS.update(repo_stats())
    load_crew()
    copy_static()
    build_templated("index.html", "/", "home", "1.0")
    build_templated("download.html", "/download.html", "download", "0.9")
    build_templated("benchmark.html", "/benchmark.html", "bench", "0.8")
    build_templated("idm-alternative.html", "/idm-alternative.html", "compare", "0.9")
    build_templated("404.html", "/404.html", "", "0")
    build_help()
    build_faq()
    build_news()
    build_languages()
    build_repo_pages()
    build_contributors()
    build_content_pages()
    build_features()
    build_themes()
    copy_feature_images()
    build_llms()
    # The same pages in every language with a catalog (i18n/site/<lang>.json), under /<lang>/.
    # MDM_SITE_ENGLISH_ONLY=1 (translate_site.py's own build): English pages only.
    only = ["none"] if os.environ.get("MDM_SITE_ENGLISH_ONLY") == "1" else None
    # MDM_SITE_PREVIEW=1: the English pages with their language picker, nothing translated (fast).
    available = sitei18n.localize(OUT, SITE, only, translated=os.environ.get("MDM_SITE_PREVIEW") != "1")
    finish(available)
    langs = len({c for v in available.values() for c in v})
    print(f"{len(PAGES)} pages in {langs} language(s) -> {OUT}")
    if "--serve" in sys.argv:
        import functools
        import http.server
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(OUT))
        print("http://127.0.0.1:8765/")
        http.server.ThreadingHTTPServer(("127.0.0.1", 8765), handler).serve_forever()


if __name__ == "__main__":
    main()
