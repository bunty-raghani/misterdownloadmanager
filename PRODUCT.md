# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

The website's primary visitor is a Windows user who uses, or used, Internet Download Manager
(IDM), often on a trial that ran out or a cracked copy, and is looking for a free replacement
that works the same way. Many are not technical. Many arrive by searching "IDM alternative" in
their own language: the site is published in 21 languages, Arabic and Persian right to left.

Secondary visitors: Linux users (supported, tested on GNOME, KDE Plasma, Xfce and Cinnamon), Mac
users (experimental builds), people reading the help pages
while using the app, and contributors (code, translations, bug reports) arriving from GitHub.

## Product Purpose

Mister Download Manager (MDM) is a free, open-source (GPL-3.0) download manager for Windows and Linux,
laid out the way IDM users expect: same windows, menus and workflow, rebuilt from scratch in
Rust. The website's job is to get the right visitor to download it with confidence, then to
help them use it (help pages, FAQ, tutorials).

Success: an IDM user recognizes the window, believes it is faster and genuinely free, and
downloads it; a user with a question finds the answer in the help.

## Positioning

IDM's window, but faster and free. The layout is the one the visitor already knows; the speed
is measured, not claimed (a public, reproducible benchmark against IDM 6.42); the price is
nothing, with no ads, no trial and no sign-up. Torrents and video downloads are built in, and
the app speaks 86 languages.

MDM is a separate program, written from scratch. It is never presented as IDM, as made by IDM's
maker (Tonec), or as a crack or copy of it.

## Operating Context

- Visitors compare against IDM, often with IDM installed or remembered.
- Download is from GitHub Releases: a Windows installer, a portable zip, a plain zip; Linux
  packages (.deb, .rpm, AppImage, tar.gz); macOS builds marked experimental. The browser extension ships for Chrome, Edge, Brave, Opera,
  Vivaldi and Firefox.
- Help pages are read alongside the running app, often to find a menu or setting.
- Contact and support happen on GitHub only (issues, discussions); there is no email or form.

## Capabilities and Constraints

- Static site: `tools/website/build.py` builds `website/`, `docs/` (help) and `CHANGELOG.md`
  (news) into `target/site`; GitHub Pages serves it at misterdownloadmanager.com. Plain HTML,
  CSS and JavaScript, no framework, no external scripts except the Statcounter visit counter.
- Every page exists in 21 languages (`tools/website/sitei18n.py`, catalogs in `i18n/site/`);
  layout must hold for long German words, CJK, Thai, Bengali and right-to-left Arabic/Persian.
  Text in scripts is translated through `window.MDM_I18N`.
- Light and dark themes; the visitor's system choice is the default.
- Clean room: nothing from IDM (code, artwork, icons, text) appears; IDM is named only to
  compare.

## Evidence on Hand

- Real, unretouched screenshots of the app: `website/assets/screens/` (light and dark) and
  `docs/images/`.
- Benchmark results, final, 2026-10-29: `website/bench/results.json` (seven scenarios, alternating
  runs, IDM 6.42 vs MDM at defaults); the methodology is on the benchmark page.
- The app's logo (`website/assets/logo-*`), its 71 flag icons (`crates/app/assets/flags/`, lipis
  flag-icons, MIT), browser logos for the extension (`website/assets/browsers/`).
- Real third-party words and figures, checked by hand: `website/content/quotes.tsv` (reviews and
  articles, quoted exactly with their source and date), `website/content/press.tsv` (independent
  articles and videos) and `build.py` FIGURES (the browser stores' users and ratings, GitHub's
  download count, Trendshift's ranking), each with the date it was checked. Nothing beyond these
  may be invented, shortened into a different meaning or combined; no customer or press logos.
  GitHub stars are shown only as the live count.

## Product Principles

1. Familiar first: the visitor should recognize the window before reading a word.
2. Prove, don't claim: every speed statement points to the benchmark; every image is the real app.
3. Free means free: no dark patterns, no upsell, no data collection beyond the visit counter.
4. Every language is first-class: no page reads as a translation of an English layout.
5. Honest about limits: experimental builds (macOS), what Linux lacks yet, missing features and IDM's own strengths are stated
   plainly.

## Accessibility & Inclusion

Non-technical visitors in many languages and scripts: plain words, readable sizes, full keyboard
use, right-to-left layouts that mirror properly, WCAG AA contrast in both themes.
