# Themes

[← Back to docs index](README.md)

A theme changes how MDM looks and keeps everything where it was: the same windows, menus and
buttons in the same places, drawn in other colours, shapes and icons. Each theme has a light and
a dark side.

## Pick one

Open **View → Theme** and click a theme: the whole window changes at once, nothing to restart.
**View → Dark Mode support** and **View → Always dark** then choose its light or dark side, as
for the classic look ([Dark mode](getting-started.md#dark-mode)).

The built-in themes are **Classic** (MDM's own look, the default), **Native** (your system's
accent colour and contrast), **High contrast**, and the style themes **Metro**, **Adwaita**,
**Breeze**, **Luna**, **Aero**, **Aqua** and **Emerald**, each with its own control shapes and
icons, and on Windows its own title bar. [Getting started](getting-started.md#themes) describes
each one; the website's [Themes page](https://misterdownloadmanager.com/themes.html) shows them
all, light and dark.

## Where theme files go

Theme files you make or download go in MDM's themes folder. **View → Theme → Open themes folder**
opens it (and makes it the first time). It is in MDM's data folder:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\misterdownloadmanager\themes` |
| Linux | `~/.local/share/misterdownloadmanager/themes` |
| macOS | `~/Library/Application Support/misterdownloadmanager/themes` |
| [Portable copy](portable.md) | `data\themes` beside the program |

Within two seconds a new file shows in **View → Theme**, under the built-in themes, by the `name`
it gives itself. After you edit a file, pick it again in **View → Theme** to see the change.

A theme can be one file (`harbour.toml`) or a folder of its own (`harbour/theme.toml`) when it
brings icons.

**View → Theme → Add a theme file…** does it for you: pick a file anywhere, and MDM copies it into
the themes folder and uses it at once. A file that isn't a theme stays out, and MDM says why.

## Base16 and pywal colour schemes

Colour schemes made for other programs are themes too, with nothing to convert:

- **A Base16 scheme**, the `.yaml` file the
  [Base16 and Base24 schemes](https://github.com/tinted-theming/schemes) come in: both the
  classic layout (`scheme:` and `base00` to `base0F` at the top) and the newer one (`name:` and a
  `palette:` block). A Base24 scheme reads as its first 16 colours.
- **pywal's `colors.json`**, or any program's file in pywal's shape (wallust's pywal template).

Add it with **View → Theme → Add a theme file…**, or put it in the themes folder yourself. It
shows in **View → Theme** under the scheme's name (`name:` or `scheme:`; pywal's is named after
the wallpaper).

**pywal itself:** once `wal` has run, **View → Theme** lists **pywal**, read straight from where
pywal writes its colours: `~/.cache/wal/colors.json` (on Windows
`%USERPROFILE%\.cache\wal\colors.json`, where winwal and pywal write it; `$PYWAL_CACHE_DIR` or
`$XDG_CACHE_HOME/wal` when you set them). Pick it and MDM follows your wallpaper: each time `wal`
writes new colours, MDM takes them within two seconds. It looks only at the file's date and size,
every two seconds, and only while pywal is the theme. A copy added with **Add a theme file…**
keeps the colours it had.

A scheme changes colours only, on the classic look: every window, button and menu stays where it
was. A dark scheme makes MDM dark and a light one light, whatever **View → Always dark** says. The
scheme's colours go where they mean the same:

| MDM | Base16 | pywal |
|---|---|---|
| The download list, text boxes and menus | `base00` (the background) | `special.background` |
| Windows, dialogs, the toolbar, the menu bar and column headers | `base01` (the lighter background) | the background a step toward the foreground |
| Text | `base05` (the foreground) | `special.foreground` |
| Secondary text (sizes, notes) | `base04` | `color8` |
| Focus, default buttons, check boxes, links, the selection and the progress bar | `base0D` (blue) | `color4` |
| Errors | `base08` (red) | `color1` |
| Warnings | `base0A` (yellow) | `color3` |
| Success, the meters and a torrent's pieces | `base0B` (green) | `color2` |

Borders and the hover and pressed shades are mixed from these, as for a theme file's `[seeds]`.
Text stays readable: text and secondary text at 4.5:1 or more against every background they sit
on (WCAG AA), the accent at 3:1. A scheme short of that has those colours moved a little darker
(lighter on a dark scheme) until they pass; MDM's log says which.

To change more than colours, make a theme file that inherits the scheme by its file name and adds
what you like:

```toml
name = "Gruvbox, my way"
inherits = "gruvbox-dark-medium"   # gruvbox-dark-medium.yaml, beside it
icons = "accent"

[sizes]
radius = 6
```

`inherits = "pywal"` works the same way, and follows `wal` too.

## The file

A theme is one small text file in [TOML](https://toml.io/) (or JSON, `.json`; or a
[colour scheme](#base16-and-pywal-colour-schemes), `.yaml`). It names only what it changes;
everything else comes from the theme it `inherits`:

```toml
name = "Harbour"
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
focus = "ring"
```

- **`name`** — what **View → Theme** lists; the file's name when there is none.
- **`inherits`** — the theme yours starts from: `classic` (the default when it's left out), a
  built-in theme's id (`metro`, `adwaita`, `breeze`, `luna`, `aero`, `aqua`, `high-contrast`), or
  another theme file in the same folder, by its file name without `.toml`. A file that inherits
  another can be inherited in turn; a theme can't inherit itself.
- **`[seeds]`** — a handful of colours (`background`, `surface`, `header`, `text`, `accent`,
  `selection`, `error`, `warning`, `success`) that MDM spreads over every control.
  `[seeds.light]` and `[seeds.dark]` give each side its own.
- **`[colors]`** — single colour roles (such as `window`, `accent` or `"segments.done"`) on both
  sides; **`[light]`** and **`[dark]`** on one side only. A colour is `#rrggbb`, or `#rrggbbaa`
  with transparency. The roles a file sets win over its seeds.
- **`[sizes]`** — corner radii, border widths, font sizes and row heights, in pixels.
- **`[controls]`** — the shape of text boxes, check boxes, buttons, progress bars, scroll bars,
  the focus mark and the title bar.
- **`icons`** — `color`, `mono` (one colour) or `accent`.
- **`contrast = "high"`** — strengthens every edge and text colour on top of the rest.

Every colour role, size and control style, with what it sets, is listed on the website's
[Themes page](https://misterdownloadmanager.com/themes.html#make-your-own) (read from MDM's own
code, so it never drifts). The built-in themes are written the same way:
[read them on GitHub](https://github.com/anefzaoui/misterdownloadmanager/tree/main/crates/app/assets/themes).

A key MDM doesn't know, or a value it can't read, is skipped and written to MDM's log (the `logs`
folder beside the themes folder), so a file made for a newer MDM still loads. A file that isn't
valid TOML or JSON doesn't load, and the log says why. A file named like a built-in theme
(`metro.toml`) is passed over: give yours a name of its own.

### Icons

A theme in a folder of its own (`harbour/theme.toml`) can bring its icons in `icons/16` and
`icons/24` (and, for PNG pictures, `icons/32` and `icons/48`, used on a screen scaled to 150 % and
more), each named for what it shows (`tb-add-url.svg`, `cat-music.png`…). SVG and PNG files keep
their colours; `-symbolic.svg` and `-symbolic.png` ones, as GNOME and KDE name them, take the
theme's text colour. An icon the set lacks is drawn as the classic look draws it. The full list of
names is in [themeicons.rs](https://github.com/anefzaoui/misterdownloadmanager/blob/main/crates/app/src/themeicons.rs) (`NEEDS`).

## Make one

1. **View → Theme → Open themes folder**, and make a new `.toml` file there (any text editor).
2. Start from the example above: change the seeds' colours, then add roles and sizes as you go.
3. Pick it in **View → Theme**, and look at both sides: **View → Always dark** on and off. Check
   that text is readable and the focus mark visible in both.

The website's [Themes page](https://misterdownloadmanager.com/themes.html#ai-prompt) has a prompt
you can give an AI assistant, with the format, every key and the rules, to write a theme for you
from a description.

## Share it

Made something others would like? Open a
[pull request](https://github.com/anefzaoui/misterdownloadmanager/pulls) that adds it to
`crates/app/assets/themes`, with:

- the `.toml` file, and its folder of icons if it has one;
- screenshots of the main window in its light and its dark side;
- a `SOURCES.txt` beside any icons, saying who drew them and under which licence. Icons must be
  your own or under a licence that lets MDM share them (GPL-3.0), never copied from another
  program.

Not sure yet? Show it in the
[discussions](https://github.com/anefzaoui/misterdownloadmanager/discussions) first.
[CONTRIBUTING.md](../CONTRIBUTING.md) explains how pull requests work here.

## Next steps

- [Getting started →](getting-started.md)
- [Options →](options.md)
