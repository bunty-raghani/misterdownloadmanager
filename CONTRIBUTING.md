# Contributing

Thanks for helping with Mister Download Manager. Bug reports, ideas, translation fixes and pull
requests are all welcome.

## Getting set up

You need Windows 10 or 11 and Rust stable 1.98 or later (the MSVC toolchain), and CMake, LLVM
and NASM for BoringSSL, which the engine's browser-like connection builds from source
(`winget install Kitware.CMake LLVM.LLVM NASM.NASM`).

```
cargo build
cargo run -p mdm-app
```

To try changes without touching your own downloads, give the app a separate data folder:

```
set MDM_DATA_DIR=C:\some\empty\folder
cargo run -p mdm-app
```

`cargo run -p mdm-labserver -- --faults "rate=2M"` starts a local test server that serves
generated files with whatever trouble you ask for (slow connections, cuts, busy answers, no
ranges; `GET /` lists them), so downloads can be tested without the internet.

## Before you open a pull request

These are the checks CI runs; each works on a clean clone:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python tools/i18n/extract.py      # then: git diff --exit-code i18n/en.tsv
node tools/extension/test.mjs
```

Keep changes focused and match the code around them. A few rules hold everywhere:

- **Clean room.** MDM contains no code, binaries, artwork, icons, sounds or text from Internet
  Download Manager or any other download manager. Never disassemble, decompile or patch another
  program, and never bypass anyone's licensing. See
  [Not a reverse-engineering project](README.md#not-a-reverse-engineering-project).
- **Every text the user sees goes through `crate::i18n`** (`t`, `tc`, `tn`, `fmt`); after adding
  one, run `python tools/i18n/extract.py`. Data inside a sentence (a file name, an address, a
  size) goes in through `i18n::ltr` or `i18n::isolate`, so it reads right in right-to-left
  languages.
- **Windows-only calls live in `crates/app/src/win32.rs`**, their Linux and macOS counterparts
  in `unix.rs`; the rest of the workspace is safe, portable Rust (`unsafe_code = "deny"`).
  On Windows, `tools\dev\check-cross.ps1` checks that it still builds for Linux and macOS.
- **Fix causes, not cases**, and keep producers and consumers of shared data (the download list,
  the browser extension's messages, the catalogs) in step.

## Fuzzing

The parsers that read what servers, web pages and downloaded files send have fuzz targets in
[`fuzz/`](fuzz) ([cargo fuzz](https://github.com/rust-fuzz/cargo-fuzz), libFuzzer): HLS
playlists (`hls`), yt-dlp's answers (`ytdlp_json`), web pages' links and videos (`web_page`),
file names (`file_name`), .torrent files and magnet links (`torrent`), release signatures
(`signature`), Click'n'Load posts (`click_n_load`) and archives (`archive`). Each calls the real
function and checks what must always hold (no panic, a file name that is one safe path part, an
info hash kept). They are a workspace of their own, so the checks above never build them; they
need a nightly toolchain and run best on Linux (on Windows, the MSVC toolchain with
AddressSanitizer):

```
cargo install cargo-fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run hls fuzz/corpus/hls fuzz/seeds/hls -- -dict=fuzz/dicts/hls.dict -max_total_time=300
```

The run grows `fuzz/corpus/<target>` from the committed seeds (`fuzz/seeds`, made from the test
fixtures by `python fuzz/make_seeds.py`), with the target's dictionary when it has one
(`fuzz/dicts`: the format's keywords, and numbers at the edge of a type); an input that crashes is saved in `fuzz/artifacts/` and
replays with `cargo +nightly fuzz run <target> <file>`. Fix what it finds where the input is read,
with a unit test of that input beside the parser. CI runs every target for two minutes on each
pull request that changes the crates they read through, and for longer every week
(ClusterFuzzLite, `.clusterfuzzlite/`).

## Translations

The interface is in every language Windows 11 offers. The catalogs in [`i18n/`](i18n) were made
by machine translation, and each row marked `#, machine` waits for a native speaker:

1. Open `i18n/<code>.tsv` (the codes are in [`i18n/languages.tsv`](i18n/languages.tsv)).
2. Fix the translation in the row's third column and delete its `#, machine` line.
3. Keep `{placeholders}` and the one `&` (the keyboard shortcut letter) as they are.

A row without its `#, machine` line is yours from then on: `tools/i18n/translate.py` never sends
it to machine translation again, and CI (`tools/i18n/reviewed_guard.py`) fails any pull request
that drops it or marks it `#, machine` again.

`cargo test -p mdm-app every_catalog` checks every catalog. To see whether your texts fit, set
`MDM_FIT_AUDIT` to a file and take the screenshots in your language
(`tools\lab\docs-shots.ps1 ... -Lang <code>`): every text that runs past its window or over
another one is listed there. See [Languages](docs/languages.md)
for how the language list, the installer and the right-to-left layout fit together.

The website is in 21 languages too, also machine-translated. Its texts are in
`i18n/site/<code>.json`, one row per sentence: fix a row's `"text"` (keep tags like `<g1>…</g1>`
and `<x2/>` as they are, they are the links and icons). `python tools/website/translate_site.py
--status` shows what each language still lacks, and the same tool with `--langs <code>` translates
only that.

## Reporting problems

Use **Help → Report a problem** in the app: it opens the bug report form with your version,
system, how MDM was installed and its language filled in. The form asks for the same when you
open it yourself. Security problems: see [SECURITY.md](SECURITY.md).

Pull requests are accepted under the project's licence, the
[GNU GPL, version 3 or later](LICENSE).
