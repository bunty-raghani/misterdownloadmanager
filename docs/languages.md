# Languages

[← Back to docs index](README.md)

MDM's interface comes in 86 languages: every display language Windows 11 offers, from Afrikaans to Vietnamese, including both Chinese scripts, Spanish for Spain and Mexico, French for France and Canada, Portuguese for Portugal and Brazil, and Serbian in Cyrillic and Latin.

## Choosing the language

- **With the setup:** the setup asks for its language first, with Windows' display language selected. MDM starts in the language you choose there.
- **Without the setup** (a zip copy): on the first start, MDM shows every language with its flag and its own name. Pick yours.

![Choosing a language on first start](images/language-picker.webp)

## Changing it later

Open **View → Language...**. The same window as on the first start opens: every language by its flag, its own name and its English name. Type in **Search** to narrow the list, then double-click a language or select it and click **OK**. The window switches at once; there is nothing to restart.

**Windows' language**, at the top, follows Windows' display language. When Windows uses a language MDM doesn't have, MDM shows English.

## Right-to-left languages

In Arabic, Hebrew, Persian, Urdu and Uyghur the whole window is mirrored: the menus, toolbar and categories run from right to left, as Windows does for these languages. File names, addresses and folder paths keep their own direction inside translated sentences, so they stay readable.

![MDM in Arabic](images/arabic-main.webp)

## Numbers and dates

**Numbers** (counts, sizes, speeds, percentages and the list's columns) are written the way the interface language writes them, whatever the Windows region: German `10.200` and `1,50 MB`, French `10 200`, Hindi `1,00,000`. When your system's region is in that same language, its own way wins (Swiss German `10’200`, or a separator you set in Windows). In English, MDM writes plain digits, as IDM does (`10200`, `1.50 MB`).

**Dates and times** in the list (**Last Try Date**, **Added**) and the Scheduler follow the system's region by default (on Windows **Settings → Time & language → Language & region → Regional format**; on macOS and Linux, your language's usual way), or your own date order, clock and first day of the week in [Options → Regional](options.md#regional). In English the list keeps IDM's own date text (`Oct 30 21:44:12 2026`), on the system's 12- or 24-hour clock (`Oct 30 9:44:12 PM 2026`).

A translation longer than the English ends in **...** where it doesn't fit; point at it to read it whole.

## Missing or odd translations

A text that hasn't been translated yet shows in English. Most translations were written by machine and are waiting for a native speaker to check them, so some wording may sound wrong.

## Help translate

Native speakers are very welcome to check and improve a language.

Each language has its own catalog in the repository: `i18n/<code>.tsv`, for example `i18n/de.tsv` for German or `i18n/pt-br.tsv` for Portuguese (Brazil). The codes are listed in `i18n/languages.tsv`. Each catalog is a text file with one row per text, in three columns separated by tabs:

```
context	source	translation
```

- **source** is the English text; leave it as it is. Change only **translation**.
- A row with a `#, machine` line above it was translated by machine. When you have checked it (and corrected it if needed), delete that `#, machine` line. From then on the row is yours: machine translation never replaces it, and a check on every pull request makes sure no change drops it or marks it machine again.
- Lines starting with `#.` are notes for translators: where the text appears and what it means.
- Keep `{name}` slots, such as `{n}` or `{file}`, exactly as they are. You can move them within the sentence.
- `&` marks the keyboard letter of a menu item or button. Keep one `&` before a letter of your translation.
- `tools/i18n/glossary.tsv` explains the key terms (download, queue, category, connection, resume…). Translate each one the same way everywhere, the way Windows says it in your language. `i18n/style/<code>.md` is each language's style guide: how it writes commands, questions and labels, and the terms reviewers settled.

To send your changes:

- Open a pull request on [GitHub](https://github.com/anefzaoui/misterdownloadmanager) with the edited catalog, or
- [open an issue](https://github.com/anefzaoui/misterdownloadmanager/issues) describing what should change, if you'd rather not edit the file.

## Next steps

- [Options →](options.md)
- [Getting started →](getting-started.md)
- [Report a problem with a translation →](troubleshooting.md#reporting-a-problem)
