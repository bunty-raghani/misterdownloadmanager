---
name: verify
description: How to verify a change to Mister Download Manager for real before it's called done or merged - build, tests, run the app in isolation, the browser add-on in real browsers, the Linux test machine, screenshots, and cleaning up. Use before every commit you call finished, before opening or merging a PR, and in every agent brief ("follow the verify skill").
---

# Verify a change to MDM

Unit tests aren't proof that a feature works. A change is verified when it ran the way a user
runs it, on the systems it touches, and the evidence (numbers, logs, screenshots) is written in
the PR. Check what the diff touches against [REVIEW.md](../../../REVIEW.md) as well.

## 0. Isolation first

- Work in your own git worktree (`git worktree add <dir> -b <branch> origin/main`), never in a
  checkout someone else is using. Never `git stash` there: the stash list is shared by every
  worktree of the repo.
- A test copy of MDM gets its own data folder and its own download folder:
  `MDM_DATA_DIR=<empty folder>` (or `--data <folder>` headless), and a download folder of its
  own. Never run tests against an installed MDM, a real user's list, Downloads or clipboard.
- Outside contributors' code runs only in CI or on the Linux test machine, never on a
  maintainer's own computer.

## 1. Cheap checks (before any push)

```
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test -p <every crate the diff touches>
python tools/i18n/extract.py            # after any new user-visible text; then translate
python tools/extension/locales.py       # after any add-on text; git diff --exit-code -- extension/_locales
node tools/extension/test.mjs           # any change under extension/
tools/dev/check-cross.ps1               # clippy for Linux and macOS from Windows
```

A test that fails now and then is a bug: never rerun it until it passes, never add a sleep or
skip it. Find why, or delete it and say why in the PR.

## 2. Run it for real

Pick what the diff touches:

| The change touches | Run |
|---|---|
| the engine, protocols, errors | `cargo run -p mdm-labserver -- --faults "<fault>"` (GET / lists them) and a real download through it: `cargo run -p mdm-app -- --cli get <url> -o <dir>` and the app's window |
| a window, a dialog, a theme | the app with `MDM_DATA_DIR`, the window opened with `/show <window>` (as `tools/shots/make.py` does), in light and dark, a long language (de) and right to left (ar), at more than one interface size; Classic and a theme that draws its own title bar |
| the browser add-on | `tools/extension/pack.ps1`, then `node tools/extension/e2e.mjs <browsers> --only=<cases>` (real headless browsers, their own profiles); add a case for the new behaviour |
| Linux or macOS code, fonts, the desktop | the Linux test machine (`tools/lab/linux-vm/vm.ps1`, its guest scripts `build.sh`, `restart-lab.sh`, `shots.sh`); macOS on GitHub's macOS runners (below) |
| videos, live streams | a local stream (ffmpeg to HLS, a local http server) through the real yt-dlp and ffmpeg |
| the website | `python tools/website/build.py --serve`, checked at phone width and desktop |
| the website's lightbox or dialogs (`website/vitrine/`) | they are Vitrine: change them upstream ([anefzaoui/vitrine](https://github.com/anefzaoui/vitrine), [anefzaoui/vitrine-modal](https://github.com/anefzaoui/vitrine-modal)), where `node test/e2e.mjs` runs them in Chromium and Firefox; then `python tools/website/vitrine.py fetch --lightbox=<sha> --modal=<sha>` and the built site in both browsers |

Write down what you ran and what you saw: commands, numbers, log lines, before and after.

### macOS: the runner

Nobody here has a Mac; GitHub's macOS runners are the Mac. The **macOS showcase** workflow
(`.github/workflows/macos-showcase.yml`, scripts in `tools/lab/macos/`) builds a commit on Apple
Silicon (`macos-latest`) and Intel (`macos-15-intel`) and runs it there as a person would: its
own data folder and download folder, the lab server at `http://downloads.example`, windows opened
with `/show`, pictures taken with `screencapture`. A run builds only the commit it runs on; to
check a pull request, push its head commit to a branch of its own and the push starts it:

```
git push origin <head sha>:refs/heads/macos-showcase/pr-<n>
gh run list --workflow macos-showcase.yml --branch macos-showcase/pr-<n>
gh run watch <run id>; gh run download <run id> -n macos-arm64 -n macos-x86_64
git push origin --delete macos-showcase/pr-<n>          # when done
```

Never give it a ref of other code to build on `main` (it has no such input on purpose): a run on
`main` shares main's caches, and an outside contributor's code there could plant something in
them that main's builds would read. On its own branch it can't: main never reads that branch's
caches. By hand on a branch: `gh workflow run macos-showcase.yml --ref <branch> -f jobs=verify`.

A push runs everything. `jobs=verify` (about 12 minutes on Apple Silicon, 20 on Intel) starts
MDM, runs a 200 MB download over 8 connections to its end and checks its SHA-256 against the server's, asks the
automation API about it, checks `--cli get` the same way, takes the main and progress windows
(`verify/*.png`), keeps MDM's log (`verify/logs/`) and quits with `/exit`, timed, twice (main
window only; a dialog open and the main window minimized, #1651). `verify/verify.txt` has a
`PASS` or `FAIL` line for each; the job fails if any failed. A change to a window adds
`jobs=all` (or `showcase`): light and dark, every style theme, Options, the menu bar item and a
recording, under `showcase/`. `jobs=dmg` installs the latest release's `.dmg` files and checks
them (`dmg_from_commit=true`: this commit's too, as the release builds them). To try changes to
the scripts themselves, push them to a `macos-showcase/<name>` branch: that runs everything
(`macos-showcase/dmg<name>`: the `.dmg` jobs only, with the commit's own).

The PR then says `Verified: macOS runner (Apple Silicon, Intel; run <link>)`. What a runner can't
show stays under `Not verified:` — a real GPU (runners draw with Apple's software OpenGL), a
Retina panel (the showcase's HiDPI screen is a virtual display), Gatekeeper's first-start
question (an unsigned app is started with its quarantine removed), Safari itself, and the menu bar
item's panel and menu (a click posted there opens neither on a runner).

## 3. Screenshots

Every visible change: before and after, light and dark, de and ar, phone width for the website.
Neutral test file names and folders, no other download manager's artwork. They go on the
`pr-shots` branch as `<pr>/<name>.png`, embedded in the PR from
`raw.githubusercontent.com/anefzaoui/misterdownloadmanager/pr-shots/`. A change with nothing to
see shows its proof (tables, logs) instead and says so.

## 4. Say what was and wasn't verified

The PR body ends its verification part with two lines:

```
Verified: <how: real window / e2e in Chrome, Firefox / Linux test machine / labserver / tests only>
Not verified: <what wasn't: an OS, a setup, a screen reader, a real site> (or "nothing left")
```

## 5. Three strikes

If the same check fails three rounds in a row, stop: report what you tried, what you saw and
what you think is wrong, and ask. No fourth blind attempt.

## 6. Clean up

Quit every MDM you started properly (`/exit`, File → Exit), close the browsers and windows you
opened, stop lab servers and stray ffmpeg/yt-dlp, release any lock you took, delete your build's
`target/` folder when done (keep the worktree), and leave the test machine as you found it.
