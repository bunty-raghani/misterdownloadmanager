#!/usr/bin/env python3
"""Regenerates the site's screenshots (website/assets/screens/*.webp, website/assets/win/*.webp)
from the app's own PNG shots, so the site never shows a window the app no longer draws and nobody
converts or crops one by hand. The day versions come from a folder of tools/lab/docs-shots.ps1's
shots (--light: a Docs screenshots run's docs/ folder, .github/workflows/docs-shots.yml), the night
versions from the same shots taken with -Dark (its site-dark/ folder):

    python -I tools/website/screens.py <run>\\site-dark --light <run>\\docs
    python -I tools/website/screens.py D:\\shots\\dark --light D:\\shots\\light --density 2

A shot that isn't there is skipped and its WebP kept: the video quality window, for one, needs a
real video page and can't be shot offline, so screens/video-*.webp stay as they are. Each WebP is
lossless (#186: text and one-pixel lines stay sharp), written only when its bytes change.

Every <img> in website/*.html that shows a WebP written here gets its width and height set to the
picture's size in CSS pixels (its pixels / --density: docs-shots.ps1 shoots at 1x, so 1 unless the
shots were taken with -Zoom 2), which is also how the lightbox (Vitrine, set up in main.js)
knows the scale to show it at. The sizes are printed; an <img> that names a missing WebP is reported.

Needs: pip install pillow
"""
from __future__ import annotations

import argparse
import io
import re
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
WEB = ROOT / "website"
ASSETS = WEB / "assets"

# The shot's name (docs-shots.ps1's, without .png) -> its day WebP and its night WebP (None: one
# picture for both, like the Arabic window, whose point is the layout, not the theme).
SHOTS = [
    ("main-light", "screens/main-light.webp", None),
    ("progress", "screens/progress-light.webp", "screens/progress-dark.webp"),
    ("progress-torrent-pieces", "screens/torrent-light.webp", "screens/torrent-dark.webp"),
    ("add-torrent", "screens/add-torrent-light.webp", "screens/add-torrent-dark.webp"),
    ("scheduler", "screens/scheduler-light.webp", "screens/scheduler-dark.webp"),
    ("options-connection", "screens/options-light.webp", "screens/options-dark.webp"),
    ("arabic-main", "screens/arabic-main.webp", None),
    ("language-picker", "win/language-picker.webp", None),
]
# docs-shots.ps1 names the main window's night shot main-dark (in docs/images and in a -Dark run).
NIGHT_MAIN = ("main-dark", "screens/main-dark.webp")


def encode(png: Path) -> tuple[bytes, str, tuple[int, int]]:
    """The smaller of a quality-90 and a lossless WebP of the shot. Screenshots are opaque: an
    alpha channel that is all 255 is dropped rather than stored."""
    im = Image.open(png)
    im.load()
    if im.mode in ("RGBA", "LA", "P"):
        im = im.convert("RGBA")
        if im.getchannel("A").getextrema() == (255, 255):
            im = im.convert("RGB")
    elif im.mode != "RGB":
        im = im.convert("RGB")
    buf = io.BytesIO()
    im.save(buf, "WEBP", lossless=True, quality=100, method=6)
    return buf.getvalue(), "lossless", im.size


def write(src: Path, out: str, density: float, sizes: dict[str, tuple[int, int]]) -> None:
    data, kind, (w, h) = encode(src)
    dst = ASSETS / out
    dst.parent.mkdir(parents=True, exist_ok=True)
    changed = not dst.is_file() or dst.read_bytes() != data
    if changed:
        dst.write_bytes(data)
    css = (round(w / density), round(h / density))
    sizes["/assets/" + out] = css
    rel = src.relative_to(ROOT) if src.is_relative_to(ROOT) else src
    print(f"  {out:34} {css[0]:>4} x {css[1]:<4} {len(data) // 1024:>4} KB {kind:8} "
          f"{'written' if changed else 'unchanged':9} <- {rel}")


IMG = re.compile(r"<img\b[^>]*>", re.S)
SRC = re.compile(r'\ssrc="(/assets/(?:screens|win)/[^"]+\.webp)"')


def fix_html(sizes: dict[str, tuple[int, int]]) -> None:
    """Sets width and height on every <img> showing a WebP written now; reports the ones that
    name a WebP that doesn't exist."""
    for page in sorted(WEB.rglob("*.html")):
        text = page.read_text("utf-8")

        def one(m: re.Match) -> str:
            tag = m.group(0)
            src = SRC.search(tag)
            if not src:
                return tag
            if not (WEB / src.group(1).lstrip("/")).is_file():
                print(f"  ! {page.relative_to(ROOT)}: {src.group(1)} doesn't exist")
                return tag
            if src.group(1) not in sizes:
                # A WebP kept from before: its width and height should still have its shape.
                w = re.search(r'\swidth="(\d+)"', tag)
                h = re.search(r'\sheight="(\d+)"', tag)
                with Image.open(WEB / src.group(1).lstrip("/")) as im:
                    nw, nh = im.size
                if w and h and abs(nh * int(w.group(1)) / nw - int(h.group(1))) > 1:
                    print(f"  ! {page.relative_to(ROOT)}: {src.group(1)} is {nw} x {nh}, "
                          f"not the shape of width {w.group(1)} height {h.group(1)}")
                return tag
            w, h = sizes[src.group(1)]
            for attr, v in (("width", w), ("height", h)):
                if re.search(rf'\s{attr}="\d+"', tag):
                    tag = re.sub(rf'(\s{attr}=")\d+(")', rf"\g<1>{v}\g<2>", tag, count=1)
                else:
                    tag = tag.replace(src.group(0), f'{src.group(0)} {attr}="{v}"', 1)
            return tag

        new = IMG.sub(one, text)
        if new != text:
            page.write_text(new, "utf-8", newline="\n")
            print(f"  sizes updated in {page.relative_to(ROOT)}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("dark", nargs="?", default=str(ROOT / "target" / "site-shots" / "dark"),
                    help="the -Dark shots (default target/site-shots/dark)")
    ap.add_argument("--light", default=str(ROOT / "target" / "site-shots" / "light"),
                    help="the day shots (default target/site-shots/light)")
    ap.add_argument("--density", type=float, default=1.0,
                    help="the shots' pixels per CSS pixel: 1 for docs-shots.ps1's 1x, 2 with -Zoom 2")
    a = ap.parse_args()
    light, dark = Path(a.light), Path(a.dark)
    sizes: dict[str, tuple[int, int]] = {}
    missing = []
    print(f"day: {light}\nnight: {dark}")
    for name, out_light, out_dark in SHOTS:
        for folder, out in ((light, out_light), (dark, out_dark)):
            if not out:
                continue
            src = folder / f"{name}.png"
            if src.is_file():
                write(src, out, a.density, sizes)
            else:
                missing.append((out, src))
    name, out = NIGHT_MAIN
    src = next((p for p in (dark / f"{name}.png", light / f"{name}.png") if p.is_file()), None)
    if src:
        write(src, out, a.density, sizes)
    else:
        missing.append((out, dark / f"{name}.png"))
    for out, src in missing:
        print(f"  {'kept' if (ASSETS / out).is_file() else 'none:'} {out}, no {src}")
    fix_html(sizes)


if __name__ == "__main__":
    main()
