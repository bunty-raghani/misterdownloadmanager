"""One file of a GitHub release, for the release builds' helpers (dump_syms, appimagetool).

    python tools/installer/fetch_release.py <owner/repo> <tag> <file name> <output path>

With GH_TOKEN or GITHUB_TOKEN set it signs in and goes through GitHub's API: anonymous downloads
from github.com can fail while signed-in ones work. Without a token it uses the public address.
A server error or a dropped connection is tried again for several minutes.
"""
import json
import os
import sys
import time
import urllib.error
import urllib.request

# Seconds to wait before each new try: about seven minutes in all.
WAITS = (5, 15, 30, 60, 90, 90, 120)
UA = "mdm-release-build"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Hands a redirect back, so the token isn't sent on to the storage server."""

    def redirect_request(self, *args, **kwargs):
        return None


def _get(url, token, accept):
    headers = {"User-Agent": UA, "Accept": accept}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
    opener = urllib.request.build_opener(NoRedirect)
    try:
        with opener.open(urllib.request.Request(url, headers=headers), timeout=120) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        if e.code not in (301, 302, 303, 307, 308):
            raise
        target = e.headers["Location"]
    req = urllib.request.Request(target, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.read()


def _once(repo, tag, name, token):
    if not token:
        url = f"https://github.com/{repo}/releases/download/{tag}/{name}"
        return _get(url, None, "application/octet-stream")
    api = f"https://api.github.com/repos/{repo}/releases/tags/{tag}"
    release = json.loads(_get(api, token, "application/vnd.github+json"))
    asset = next((a for a in release["assets"] if a["name"] == name), None)
    if asset is None:
        sys.exit(f"{repo} {tag} has no file named {name}")
    return _get(asset["url"], token, "application/octet-stream")


def fetch(repo, tag, name):
    """The file's bytes; exits when every try failed."""
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    for wait in (*WAITS, None):
        try:
            return _once(repo, tag, name, token)
        except urllib.error.HTTPError as e:
            if e.code < 500 and e.code != 429:
                sys.exit(f"{name}: HTTP {e.code}")
            problem = f"HTTP {e.code}"
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            problem = str(e)
        if wait is None:
            sys.exit(f"{name}: gave up: {problem}")
        print(f"{name}: {problem}; trying again in {wait} s", flush=True)
        time.sleep(wait)


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    with open(sys.argv[4], "wb") as f:
        f.write(fetch(sys.argv[1], sys.argv[2], sys.argv[3]))
