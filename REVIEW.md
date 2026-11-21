# Review checklist

What to check in a pull request, by what its diff touches. Check only the items a diff
triggers; most PRs trigger a few. The reviewer reads this file from `main`, never from the PR
under review. A PR that changes this file, CI workflows, tests or the verify skill
(`.claude/skills/verify`) is reviewed against `main`'s copy, and more strictly.

An item is added when the same kind of miss has shown up in two pull requests and can be seen
from a diff. Items that stop earning their place are removed.

## Every PR

- [ ] It was run the way a user runs it, as the verify skill says, and the PR says how
      (`Verified:`) and what wasn't (`Not verified:`).
- [ ] ROADMAP.md moves its item; CHANGELOG.md says what changed for users, in public-safe words.
- [ ] Every visible change has screenshots (light, dark, de, ar).
- [ ] The review and the merge are for the same commit (merge with that commit's SHA).

## Files and paths

- **Triggered by:** a path built from anything a server, page, file, torrent or yt-dlp sent
  (names, ids, format ids, titles), joined to a folder.
  - [ ] Only a plain file name gets through: no `/`, `\`, `..`, drive letters or device names,
        on Windows as well as Linux and macOS. Prefer matching a folder's own entries by exact
        name over building a name to open.
- **Triggered by:** removing, replacing or renaming a file.
  - [ ] It can only touch a file MDM made for this download. What if the file is the user's
        finished one, is open, or changed since?

## Translations

- **Triggered by:** a new or changed user-visible text in the app.
  - [ ] It goes through `crate::i18n`; `tools/i18n/extract.py` was run and the catalogs carry it.
- **Triggered by:** a change under `extension/` that adds or renames a text.
  - [ ] `en/messages.json`, the catalogs and every `extension/_locales/*/messages.json` agree:
        `python tools/extension/locales.py` leaves no diff.

## Windows and their sizes

- **Triggered by:** a window's or dialog's size, frame, title bar or scale code.
  - [ ] The size is measured on the real window, not only worked out: check a theme that draws
        its own title bar as well as Classic, at more than 100% display scale, and on Windows 10
        frames if the code differs by version.

## Merges and conflicts

- **Triggered by:** a merge of `main` into a branch with conflicts.
  - [ ] Only CHANGELOG.md and ROADMAP.md are resolved by keeping both sides; code conflicts are
        resolved by hand, compared with `main`, and CI compiles the result before the merge.

## Tests

- **Triggered by:** a new or changed test that waits on time, a process or the network.
  - [ ] It doesn't pass by luck: no fixed sleeps, a margin that holds on a slow CI runner, and it
        was run several times. A flaky test is fixed or removed with the reason, never retried.

## Outside contributions

- **Triggered by:** a PR from outside the maintainers that touches `.github/`, build scripts,
  `Cargo.toml`, a test's expectations, this file or the verify skill.
  - [ ] Read every line; nothing reaches the network, secrets or the release, and no check got
        looser. Its code runs only in CI or on the Linux test machine.
