#!/usr/bin/env python3
"""The people who make MDM possible, from GitHub's public record: everyone who opened a pull
request (merged or not) or an issue, or commented on one. One module for every place that thanks
them: the website (tools/website/build.py: the home page, /contributors.html, each release on the
News page), the README (`readme` below) and each release's notes on GitHub (`thanks`, run by
.github/workflows/release.yml).

Who counts, and in what order (the maintainer's decision, 2026-11-08):
- Not counted: the maintainer (MAINTAINER), Claude (any account whose login contains "claude" or
  "anthropic"), bots (a login ending in "[bot]", an account of type Bot, dependabot,
  github-actions) and deleted accounts ("ghost").
- People with pull requests first, then those who opened issues, then those who commented; each
  person once, in their highest group. Within a group, by how much they did (pull requests, then
  issues, then comments), then by login.
- A release thanks everyone active between the previous release's publication and its own: a
  pull request or an issue opened, or a comment written, in that window.

Only public GitHub data is read and shown: the login, the avatar, the profile's address and the
counts. Nothing from commits (no names, no emails), nothing from anywhere else.

GitHub's REST API: issues (pull requests included), issue comments (pull requests' conversations
included) and pull request review comments, page by page. With GITHUB_TOKEN (or GH_TOKEN) set,
5,000 requests an hour; without, 60, enough for a build or two. A rate limit or a network error
gives None, never half a list: the caller goes on without the people and says so.

    python tools/contributors.py list [--since ISO] [--until ISO]   # the table, for a look
    python tools/contributors.py thanks vX.Y.Z    # release notes' "Thanks to" section (Markdown)
    python tools/contributors.py readme           # refresh the README's block and its avatars
"""
from __future__ import annotations

import argparse
import datetime
import io
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = "anefzaoui/misterdownloadmanager"
MAINTAINER = "anefzaoui"
API = "https://api.github.com"
# The README's round avatars (`readme`), committed: GitHub can't round an image in a README.
README_AVATARS = ROOT / "art" / "contributors"
README_SIZE = 96  # pixels; shown at 48
# The order of the groups, highest first, and what each counts.
GROUPS = ("pr", "issue", "comment")


def log(msg: str) -> None:
    print(f"contributors: {msg}", file=sys.stderr)


# ---- GitHub ---------------------------------------------------------------------------------------

def _token() -> str:
    return os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN") or ""


def _get(url: str) -> tuple[object, dict]:
    """One GET of the API, as (JSON, headers). Waits out a short secondary rate limit (up to a
    minute, three times) and GitHub's own passing errors; raises on anything else."""
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "misterdownloadmanager-contributors",
               "X-GitHub-Api-Version": "2022-11-28"}
    if _token():
        headers["Authorization"] = f"Bearer {_token()}"
    for attempt in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=20) as r:
                return json.load(r), dict(r.headers)
        except urllib.error.HTTPError as e:
            wait = e.headers.get("Retry-After")
            if e.code in (403, 429) and e.headers.get("X-RateLimit-Remaining") == "0":
                reset = e.headers.get("X-RateLimit-Reset")
                when = datetime.datetime.fromtimestamp(int(reset), datetime.timezone.utc).strftime("%H:%M UTC") if reset else "later"
                raise RuntimeError(f"GitHub's rate limit is used up until {when}"
                                   f"{'' if _token() else ' (set GITHUB_TOKEN for more)'}") from e
            if e.code in (403, 429) and wait and wait.isdigit() and int(wait) <= 60 and attempt < 3:
                time.sleep(int(wait))
                continue
            # A passing hiccup on GitHub's side (502, 503, 504): try again shortly.
            if e.code in (500, 502, 503, 504) and attempt < 3:
                time.sleep(2 * (attempt + 1))
                continue
            raise
    raise RuntimeError("GitHub kept asking to wait")


def _pages(path: str, params: dict | None = None) -> list:
    """Every item of a listed endpoint, following its Link: rel="next" pages."""
    q = {"per_page": "100", **(params or {})}
    url = f"{API}/{path}?{urllib.parse.urlencode(q)}"
    items: list = []
    while url:
        data, headers = _get(url)
        if not isinstance(data, list):
            raise RuntimeError(f"{path}: not a list")
        items.extend(data)
        m = re.search(r'<([^>]+)>;\s*rel="next"', headers.get("Link", "") or headers.get("link", ""))
        url = m.group(1) if m else ""
    return items


def excluded(user: dict | None) -> bool:
    """The maintainer, Claude, bots and deleted accounts: not in the thanks."""
    if not user or not user.get("login"):
        return True
    login = user["login"].lower()
    return (login == MAINTAINER.lower() or "claude" in login or "anthropic" in login
            or login.endswith("[bot]") or user.get("type") == "Bot"
            or login in ("dependabot", "github-actions", "ghost"))


_ACTIVITY: dict[str, list[dict] | None] = {}


def activity(repo: str = REPO, since: str = "") -> list[dict] | None:
    """Everything the counted people did: one {login, avatar, kind, at} per pull request, issue
    and comment ("pr", "issue", "comment"), at its creation time (ISO, UTC). `since` (ISO) leaves
    out what hasn't changed since then (GitHub's own filter, by update time; the caller still
    filters by creation). None when GitHub can't be read. Kept for the run: one read per build."""
    key = f"{repo}@{since}"
    if key in _ACTIVITY:
        return _ACTIVITY[key]
    p = {"since": since} if since else {}
    try:
        issues = _pages(f"repos/{repo}/issues", {"state": "all", **p})
        comments = _pages(f"repos/{repo}/issues/comments", p)
        reviews = _pages(f"repos/{repo}/pulls/comments", p)
    except Exception as e:  # noqa: BLE001 — every caller goes on without the people
        log(f"GitHub couldn't be read ({e}); going on without the contributors")
        _ACTIVITY[key] = None
        return None
    out = []
    for i in issues:
        out.append((i.get("user"), "pr" if "pull_request" in i else "issue", i.get("created_at", "")))
    for c in comments + reviews:
        out.append((c.get("user"), "comment", c.get("created_at", "")))
    events = [{"login": u["login"], "avatar": u.get("avatar_url", ""), "kind": kind, "at": at}
              for u, kind, at in out if not excluded(u)]
    _ACTIVITY[key] = events
    return events


def releases(repo: str = REPO) -> list[dict] | None:
    """The published releases, oldest first: {tag, version, at} (drafts left out). None when
    GitHub can't be read."""
    try:
        rel = _pages(f"repos/{repo}/releases")
    except Exception as e:  # noqa: BLE001
        log(f"the releases couldn't be read ({e})")
        return None
    out = [{"tag": r["tag_name"], "version": r["tag_name"].lstrip("v"), "at": r["published_at"]}
           for r in rel if not r.get("draft") and r.get("published_at")]
    return sorted(out, key=lambda r: r["at"])


def vparts(v: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", v)[:3])


def window(rels: list[dict], tag: str) -> tuple[str, str]:
    """A release's window, (since, until) in ISO: from the publication of the release before it
    (by version) to its own; until is now when it isn't published yet (the release workflow,
    just before it publishes). The first release's window starts at the beginning."""
    v = vparts(tag.lstrip("v"))
    own = next((r for r in rels if vparts(r["version"]) == v), None)
    before = [r for r in rels if vparts(r["version"]) < v]
    since = max((r["at"] for r in before), default="")
    until = own["at"] if own else datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return since, until


# ---- Counting -------------------------------------------------------------------------------------

def tally(events: list[dict], since: str = "", until: str = "") -> list[dict]:
    """The people, ranked: {login, avatar, profile, prs, issues, comments, group}, for what they
    did after `since` and up to `until` (ISO; either may be empty)."""
    people: dict[str, dict] = {}
    for e in events:
        if (since and e["at"] <= since) or (until and e["at"] > until):
            continue
        p = people.setdefault(e["login"].lower(), {
            "login": e["login"], "avatar": e["avatar"], "profile": f"https://github.com/{e['login']}",
            "prs": 0, "issues": 0, "comments": 0})
        p[{"pr": "prs", "issue": "issues", "comment": "comments"}[e["kind"]]] += 1
    for p in people.values():
        p["group"] = "pr" if p["prs"] else "issue" if p["issues"] else "comment"
    # (pull requests, issues, comments) highest first puts the groups in order by itself.
    return sorted(people.values(), key=lambda p: (-p["prs"], -p["issues"], -p["comments"], p["login"].lower()))


def avatar(url: str, size: int) -> str:
    """The avatar's address at `size` pixels (GitHub's own image service)."""
    if not url:
        return ""
    return url + ("&" if "?" in url else "?") + f"s={size}"


def everyone() -> list[dict] | None:
    ev = activity()
    return None if ev is None else tally(ev)


def for_release(tag: str, rels: list[dict] | None = None) -> list[dict] | None:
    """The people a release thanks (see window())."""
    rels = releases() if rels is None else rels
    if rels is None:
        return None
    since, until = window(rels, tag)
    ev = activity()
    return None if ev is None else tally(ev, since, until)


# ---- Release notes --------------------------------------------------------------------------------

def thanks_markdown(people: list[dict]) -> str:
    """The release notes' closing section: each person's avatar linked to their profile, then
    their @-mentions (which notify them: wanted, it's a thank-you)."""
    if not people:
        return ""
    faces = " ".join(f'<a href="{p["profile"]}"><img src="{avatar(p["avatar"], 64)}" width="32" height="32" '
                     f'alt="@{p["login"]}"></a>' for p in people)
    names = ", ".join(f"@{p['login']}" for p in people)
    return f"## Thanks to\n\nThe people who made this release possible with a pull request, an issue or a comment:\n\n{faces}\n\n{names}\n"


# ---- README ---------------------------------------------------------------------------------------

README_START = "<!-- contributors:start (python tools/contributors.py readme) -->"
README_END = "<!-- contributors:end -->"


def readme_block(people: list[dict]) -> str:
    """Each person's round avatar (committed under art/contributors/, made by `readme`), linked to
    their profile, their login as its alternative text and tooltip."""
    rel = README_AVATARS.relative_to(ROOT).as_posix()
    faces = "\n".join(
        f'<a href="{p["profile"]}" title="{p["login"]}"><img src="{rel}/{p["login"].lower()}.png" '
        f'width="48" height="48" alt="{p["login"]}"></a>' for p in people)
    return f"{README_START}\n<p>\n{faces}\n</p>\n{README_END}"


def round_png(data: bytes, size: int) -> bytes:
    """An avatar cut to a circle (transparent outside, antialiased edge), as PNG."""
    from PIL import Image, ImageDraw  # only for the README refresh: pip install pillow

    im = Image.open(io.BytesIO(data)).convert("RGBA").resize((size, size), Image.LANCZOS)
    big = size * 4
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, big - 1, big - 1), fill=255)
    im.putalpha(mask.resize((size, size), Image.LANCZOS))
    # 256 colours with their transparency: a third of the size, no visible difference at 48 px.
    im = im.quantize(256, method=Image.Quantize.FASTOCTREE)
    out = io.BytesIO()
    im.save(out, "PNG", optimize=True)
    return out.getvalue()


def refresh_readme() -> int:
    people = everyone()
    if people is None:
        return 1
    README_AVATARS.mkdir(parents=True, exist_ok=True)
    keep = set()
    for p in people:
        name = f"{p['login'].lower()}.png"
        keep.add(name)
        req = urllib.request.Request(avatar(p["avatar"], README_SIZE), headers={"User-Agent": "misterdownloadmanager-contributors"})
        with urllib.request.urlopen(req, timeout=20) as r:
            png = round_png(r.read(), README_SIZE)
        target = README_AVATARS / name
        if not target.is_file() or target.read_bytes() != png:
            target.write_bytes(png)
    for old in README_AVATARS.glob("*.png"):
        if old.name not in keep:
            old.unlink()
    readme = ROOT / "README.md"
    text = readme.read_text("utf-8")
    block = readme_block(people)
    pattern = re.compile(re.escape(README_START) + r".*?" + re.escape(README_END), re.S)
    if not pattern.search(text):
        log("README.md has no contributors block (README_START ... README_END)")
        return 1
    readme.write_text(pattern.sub(lambda _: block, text), "utf-8", newline="\n")
    print(f"{len(people)} contributors in README.md, avatars in {README_AVATARS.relative_to(ROOT).as_posix()}/")
    return 0


# ---- Command line ---------------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    ls = sub.add_parser("list", help="the people and their counts")
    ls.add_argument("--since", default="")
    ls.add_argument("--until", default="")
    ls.add_argument("--json", action="store_true")
    th = sub.add_parser("thanks", help="the release notes' Thanks to section for a tag")
    th.add_argument("tag")
    sub.add_parser("readme", help="refresh README.md's contributors and their avatars")
    a = ap.parse_args()
    if a.cmd == "readme":
        return refresh_readme()
    if a.cmd == "thanks":
        # Never fails the release: no people (or no GitHub) is simply no section.
        people = for_release(a.tag)
        sys.stdout.write(thanks_markdown(people or []))
        return 0
    ev = activity()
    if ev is None:
        return 1
    people = tally(ev, a.since, a.until)
    if a.json:
        print(json.dumps(people, indent=1))
        return 0
    for g in GROUPS:
        group = [p for p in people if p["group"] == g]
        print(f"{g}: {len(group)}")
        for p in group:
            print(f"  {p['login']:<28} prs {p['prs']:>3}  issues {p['issues']:>3}  comments {p['comments']:>4}")
    print(f"total: {len(people)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
