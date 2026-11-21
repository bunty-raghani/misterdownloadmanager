"""Vitrine, the site's lightbox and dialogs (https://github.com/anefzaoui/vitrine and
https://github.com/anefzaoui/vitrine-modal), vendored in website/vitrine/ and pinned there by
vitrine.lock.json (each repository's commit, version and every file's SHA-256).

    python tools/website/vitrine.py check                     # the copy matches its lock (build.py runs this)
    python tools/website/vitrine.py fetch --lightbox=<ref> --modal=<ref> [--from-lightbox=<dir>] [--from-modal=<dir>]
                                                             # a new version: download it (or take it from a
                                                             # local clone at that commit) and write the lock

The files are never edited here: change Vitrine upstream, then fetch the new version. The lock's
format is the one Vitrine's own tools/vendor.mjs writes and checks.
"""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DIR = ROOT / "website" / "vitrine"
LIBS = {
    "anefzaoui/vitrine": ["vitrine-core.js", "vitrine.js", "vitrine.css"],
    "anefzaoui/vitrine-modal": ["vitrine-core.js", "vitrine-modal.js", "vitrine-modal.css"],
}
FILES = ["vitrine-core.js", "vitrine.js", "vitrine.css", "vitrine-modal.js", "vitrine-modal.css"]
HEADER = re.compile(r"^/\*! Vitrine(?: core| modal)? v(\d+\.\d+\.\d+) · https://github\.com/anefzaoui/(vitrine|vitrine-modal) · GPL-3\.0-or-later\.")


def digest(text: str) -> str:
    return hashlib.sha256(text.replace("\r\n", "\n").encode("utf-8")).hexdigest()


def version_of(text: str) -> str | None:
    m = HEADER.match(text.split("\n", 1)[0])
    return m.group(1) if m else None


def problems(folder: Path = DIR) -> list[str]:
    lock_file = folder / "vitrine.lock.json"
    if not lock_file.is_file():
        return [f"{lock_file} is missing"]
    out = []
    for repo, entry in json.loads(lock_file.read_text("utf-8")).items():
        for name, want in entry.get("files", {}).items():
            f = folder / name
            if not f.is_file():
                out.append(f"{name} ({repo}) is missing")
                continue
            text = f.read_text("utf-8")
            if digest(text) != want:
                out.append(f"{name} differs from {repo} {entry['version']} ({entry['ref']}): fetch it again, don't edit it")
            if name != "vitrine-core.js" and version_of(text) != entry["version"]:
                out.append(f"{name}: its first line names {version_of(text)}, the lock {entry['version']}")
    return out


def read_at(repo: str, ref: str, name: str, local: str | None) -> str:
    if local:
        return subprocess.run(["git", "-C", local, "show", f"{ref}:{name}"], check=True,
                              capture_output=True).stdout.decode("utf-8")
    url = f"https://raw.githubusercontent.com/{repo}/{ref}/{name}"
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode("utf-8")


def fetch(opts: dict[str, str]) -> None:
    lock = {}
    core = None
    for repo, key in (("anefzaoui/vitrine", "lightbox"), ("anefzaoui/vitrine-modal", "modal")):
        ref = opts.get(key)
        if not ref:
            continue
        files, version = {}, None
        for name in LIBS[repo]:
            text = read_at(repo, ref, name, opts.get(f"from-{key}"))
            if not version_of(text):
                sys.exit(f"{repo}@{ref}: {name} isn't a Vitrine file")
            if name == "vitrine-core.js":
                if core is not None and digest(core) != digest(text):
                    sys.exit("vitrine-core.js differs between the two refs: pick versions that share their core")
                core = text
            else:
                version = version_of(text)
            (DIR / name).write_text(text, "utf-8", newline="\n")
            files[name] = digest(text)
        lock[repo] = {"ref": ref, "version": version, "files": files}
    DIR.mkdir(parents=True, exist_ok=True)
    (DIR / "vitrine.lock.json").write_text(json.dumps(lock, indent=2) + "\n", "utf-8", newline="\n")
    print("Vitrine -> website/vitrine: " + ", ".join(f"{r} {e['version']} ({e['ref'][:12]})" for r, e in lock.items()))


def main() -> None:
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    opts = dict(a[2:].split("=", 1) for a in sys.argv[2:] if a.startswith("--") and "=" in a)
    if cmd == "check":
        bad = problems()
        if bad:
            sys.exit("website/vitrine/ doesn't match vitrine.lock.json:\n  " + "\n  ".join(bad))
        lock = json.loads((DIR / "vitrine.lock.json").read_text("utf-8"))
        print("Vitrine: " + ", ".join(f"{r} {e['version']}" for r, e in lock.items()) + ": unchanged")
    elif cmd == "fetch" and (opts.get("lightbox") or opts.get("modal")):
        fetch(opts)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
