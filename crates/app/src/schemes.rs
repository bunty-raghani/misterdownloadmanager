//! Colour schemes from other programs, read as themes (#45): a Base16 scheme (YAML, the classic
//! layout with `scheme:` and `base00`…`base0F` at the top, or tinted-theming's newer one with
//! `name:` and a `palette:` block; Base24's extra colours are passed over) and pywal's
//! `colors.json` (`special` and `color0`…`color15`; wallust's pywal-shaped file too).
//!
//! A scheme becomes the same value tree a theme file is read into: the classic look
//! `inherits`ed, a few `[seeds]` MDM spreads over every control (`crate::derive`), and the
//! secondary text's colour. Only colours change, never the layout. The text is checked against
//! every background it sits on (WCAG 2.1 AA, 4.5:1) and moved in lightness when a scheme falls
//! short, the accent the same at 3:1; each change is a warning, written to the log.

use egui::Color32;
use serde_json::{Map, Value};

use crate::derive::{contrast, mix, readable};
use crate::palette::to_hex;

/// Body text against what it sits on (WCAG 2.1, 1.4.3).
const TEXT_RATIO: f32 = 4.5;
/// The accent (focus lines, check boxes, the progress bar) against its background (1.4.11).
const ACCENT_RATIO: f32 = 3.0;

/// A scheme's colour: `#rgb` or `#rrggbb`, the `#` optional (Base16 writes none).
pub fn hex(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok();
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match h.len() {
        3 => Some(Color32::from_rgb(
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
        )),
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        _ => None,
    }
}

/// The colours a scheme gives MDM, before its contrast is checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapped {
    /// Windows, dialogs and the toolbar's band.
    pub background: Color32,
    /// The download list, text boxes and menus.
    pub surface: Color32,
    /// The menu bar and column headers.
    pub header: Color32,
    pub text: Color32,
    /// Secondary text: notes, sizes, versions.
    pub muted: Color32,
    /// Focus lines, default buttons, check boxes, links, the progress bar and the selection.
    pub accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    /// Success messages, the torrent pieces and the meters.
    pub success: Color32,
}

impl Mapped {
    /// Text readable on every background it sits on (4.5:1) and the accent seen on them (3:1):
    /// a colour that falls short is moved in lightness, away from them, until it passes. Says
    /// what it changed.
    pub fn fix_contrast(&mut self) -> Vec<String> {
        let grounds = [self.background, self.surface, self.header];
        let mut changed = Vec::new();
        for (what, c, ratio) in [
            ("text", &mut self.text, TEXT_RATIO),
            ("text.muted", &mut self.muted, TEXT_RATIO),
            ("accent", &mut self.accent, ACCENT_RATIO),
        ] {
            let was = *c;
            *c = readable_on(was, &grounds, ratio);
            if *c != was {
                changed.push(format!(
                    "contrast: {what} {} made {} to read at {ratio}:1",
                    to_hex(was),
                    to_hex(*c)
                ));
            }
        }
        changed
    }

    /// The theme-file value tree of this scheme: the classic look, seeded with its colours.
    pub fn theme_value(&self, name: &str) -> Value {
        let mut seeds = Map::new();
        for (key, c) in [
            ("background", self.background),
            ("surface", self.surface),
            ("header", self.header),
            ("text", self.text),
            ("accent", self.accent),
            ("selection", self.accent),
            ("error", self.error),
            ("warning", self.warning),
            ("success", self.success),
        ] {
            seeds.insert(key.into(), Value::String(to_hex(c)));
        }
        let mut colors = Map::new();
        colors.insert("text.muted".into(), Value::String(to_hex(self.muted)));
        let mut top = Map::new();
        top.insert("name".into(), Value::String(name.to_owned()));
        top.insert("inherits".into(), Value::String("classic".into()));
        top.insert("seeds".into(), Value::Object(seeds));
        top.insert("colors".into(), Value::Object(colors));
        Value::Object(top)
    }
}

/// `fg` moved until it has `ratio` against each of `grounds` (they are close: a scheme's
/// backgrounds), or as near as lightness allows.
fn readable_on(fg: Color32, grounds: &[Color32], ratio: f32) -> Color32 {
    let mut c = fg;
    for _ in 0..4 {
        if grounds.iter().all(|g| contrast(c, *g) >= ratio) {
            break;
        }
        for g in grounds {
            c = readable(c, *g, ratio);
        }
    }
    c
}

// ---- Base16 ------------------------------------------------------------------------------------

/// A Base16 scheme: its name and its 16 colours, base00 (the background) to base0F.
#[derive(Clone, Debug, PartialEq)]
pub struct Base16 {
    pub name: Option<String>,
    pub base: [Option<Color32>; 16],
}

/// The colours MDM reads; a scheme without one of them isn't read.
const BASE16_USED: [usize; 8] = [0x0, 0x1, 0x4, 0x5, 0x8, 0xA, 0xB, 0xD];

/// A Base16 (or Base24) scheme file. Its YAML is flat `key: "value"` lines, `palette:`'s
/// indented under it in the newer layout, so it is read here, without a YAML library.
pub fn base16(text: &str) -> Result<Base16, String> {
    let mut name = None;
    let mut base = [None; 16];
    let mut in_palette = false;
    let mut found = false;
    for (n, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = raw.trim_end();
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') || body == "---" || body == "..." {
            continue;
        }
        let indented = body.len() != line.len();
        if indented && !in_palette {
            // Something else's nested lines (a newer field MDM doesn't use, a folded text).
            continue;
        }
        let Some((key, value)) = body.split_once(':') else {
            return Err(format!("line {}: not a key: value line", n + 1));
        };
        let key = unquote(key.trim()).to_ascii_lowercase();
        let value = yaml_scalar(value).map_err(|e| format!("line {}: {e}", n + 1))?;
        if !indented {
            in_palette = key == "palette" && value.is_empty();
        }
        if let Some(i) = key
            .strip_prefix("base")
            .filter(|d| d.len() == 2)
            .and_then(|d| usize::from_str_radix(d, 16).ok())
        {
            found = true;
            // Base24's base10 to base17 are passed over.
            if i < 16 {
                let c = hex(&value)
                    .ok_or_else(|| format!("line {}: {key}: {value} is not a colour", n + 1))?;
                base[i] = Some(c);
            }
            continue;
        }
        if !indented && (key == "name" || key == "scheme") && !value.is_empty() {
            // The newer layout's `name` wins over a `scheme` left in for older builders.
            if key == "name" || name.is_none() {
                name = Some(value);
            }
        }
    }
    if !found {
        return Err("not a Base16 scheme: no base00 to base0F".into());
    }
    let missing: Vec<String> = BASE16_USED
        .iter()
        .filter(|&&i| base[i].is_none())
        .map(|i| format!("base{i:02X}"))
        .collect();
    if !missing.is_empty() {
        return Err(format!("the scheme lacks {}", missing.join(", ")));
    }
    Ok(Base16 { name, base })
}

/// A YAML value on a `key:` line: quoted (`"…"` or `'…'`, a comment after it) or plain (cut at
/// ` #`). A plain `#rrggbb` is read as the colour it means, though YAML would see a comment.
fn yaml_scalar(v: &str) -> Result<String, String> {
    let v = v.trim();
    if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let rest = &v[1..];
        let end = rest.find(q).ok_or("a quote isn't closed")?;
        let after = rest[end + 1..].trim();
        if !after.is_empty() && !after.starts_with('#') {
            return Err("text after a quoted value".into());
        }
        return Ok(rest[..end].to_owned());
    }
    if v.starts_with('#') {
        return Ok(v.split_whitespace().next().unwrap_or("").to_owned());
    }
    let plain = v.split(" #").next().unwrap_or("").trim();
    Ok(plain.to_owned())
}

fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

/// Base16's colours in MDM (Stylix's guide to them: base00 the default background, base01 the
/// lighter one, base04 and base05 the foregrounds, base0D the focus colour). The list and boxes
/// take base00, as an editor's text does, and the window around them base01, as in the classic
/// look (white boxes on grey, darker ones on dark grey).
pub fn map_base16(s: &Base16) -> Mapped {
    let c = |i: usize| s.base[i].unwrap_or(Color32::BLACK);
    Mapped {
        background: c(0x1),
        surface: c(0x0),
        header: c(0x1),
        text: c(0x5),
        muted: c(0x4),
        accent: c(0xD),
        error: c(0x8),
        warning: c(0xA),
        success: c(0xB),
    }
}

// ---- pywal ------------------------------------------------------------------------------------

/// A pywal `colors.json`: `special` (background, foreground) and `colors` (color0 to color15).
pub fn is_pywal(v: &Value) -> bool {
    v.get("special").is_some_and(Value::is_object)
        || v.get("colors").and_then(|c| c.get("color0")).is_some()
}

/// The colours MDM reads from pywal's file, read; a file without one of them isn't read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pywal {
    pub background: Color32,
    pub foreground: Color32,
    /// color0 to color15: black, red, green, yellow, blue, magenta, cyan, white, then bright.
    pub colors: [Option<Color32>; 16],
}

pub fn pywal(v: &Value) -> Result<Pywal, String> {
    let mut missing = Vec::new();
    let mut get = |table: &str, key: &str| -> Result<Option<Color32>, String> {
        match v.get(table).and_then(|t| t.get(key)) {
            None => {
                missing.push(format!("{table}.{key}"));
                Ok(None)
            }
            Some(c) => c
                .as_str()
                .and_then(hex)
                .map(Some)
                .ok_or_else(|| format!("{table}.{key}: {c} is not a colour")),
        }
    };
    let background = get("special", "background")?;
    let foreground = get("special", "foreground")?;
    let mut colors = [None; 16];
    for (i, c) in colors.iter_mut().enumerate() {
        *c = match v.get("colors").and_then(|t| t.get(format!("color{i}"))) {
            None => None,
            Some(_) => get("colors", &format!("color{i}"))?,
        };
    }
    for i in [1, 2, 3, 4] {
        if colors[i].is_none() {
            missing.push(format!("colors.color{i}"));
        }
    }
    if !missing.is_empty() {
        return Err(format!("pywal's colours lack {}", missing.join(", ")));
    }
    Ok(Pywal {
        background: background.unwrap_or(Color32::BLACK),
        foreground: foreground.unwrap_or(Color32::WHITE),
        colors,
    })
}

/// pywal's colours in MDM: its background for the list and boxes, a step toward its foreground
/// for the window (pywal has no second background), color4 (blue) for the accent, color1, 3
/// and 2 for errors, warnings and success, color8 (bright black) for secondary text.
pub fn map_pywal(p: &Pywal) -> Mapped {
    let c = |i: usize| p.colors[i].unwrap_or(p.foreground);
    let background = mix(p.background, p.foreground, 0.06);
    Mapped {
        background,
        surface: p.background,
        header: background,
        text: p.foreground,
        muted: p.colors[8].unwrap_or_else(|| mix(p.foreground, p.background, 0.35)),
        accent: c(4),
        error: c(1),
        warning: c(3),
        success: c(2),
    }
}

/// The name a pywal file's theme takes: `pywal`, with its wallpaper's file name when it says.
pub fn pywal_name(v: &Value) -> String {
    let wall = v
        .get("wallpaper")
        .and_then(Value::as_str)
        .and_then(|w| w.rsplit(['/', '\\']).next())
        .filter(|w| !w.is_empty() && *w != "None");
    match wall {
        Some(w) => format!("pywal: {w}"),
        None => "pywal".into(),
    }
}

// ---- a scheme as a theme ----------------------------------------------------------------------

/// A Base16 YAML file's text as a theme's value tree (named after the scheme, else `stem`),
/// with what its contrast check changed.
pub fn base16_theme(text: &str, stem: &str) -> Result<(Value, Vec<String>), String> {
    let s = base16(text)?;
    let mut m = map_base16(&s);
    let warnings = m.fix_contrast();
    let name = s.name.unwrap_or_else(|| stem.to_owned());
    Ok((m.theme_value(&name), warnings))
}

/// A pywal `colors.json`'s value as a theme's value tree, with what its contrast check changed.
pub fn pywal_theme(v: &Value) -> Result<(Value, Vec<String>), String> {
    let p = pywal(v)?;
    let mut m = map_pywal(&p);
    let warnings = m.fix_contrast();
    Ok((m.theme_value(&pywal_name(v)), warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(h: u32) -> Color32 {
        Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
    }

    /// A dark scheme in the classic layout (Gruvbox-like, colours of our own picking).
    const DARK_CLASSIC: &str = r##"
scheme: "Test Dark Classic"
author: "MDM's tests"
base00: "282828" # ----
base01: "3c3836" # ---
base02: "504945"
base03: '665c54'
base04: bdae93
base05: "d5c4a1"
base06: "ebdbb2"
base07: "fbf1c7"
base08: "fb4934"
base09: "fe8019"
base0A: "fabd2f"
base0B: "b8bb26"
base0C: "8ec07c"
base0D: "83a598"
base0E: "d3869b"
base0F: "d65d0e"
"##;

    /// A light scheme in tinted-theming's layout (Solarized-like), with Base24's extras.
    const LIGHT_PALETTE: &str = r##"system: "base24"
name: "Test Light"
slug: "test-light"
scheme: "Old name"
author: "MDM's tests (https://example.org/tests)"
description: >
  A folded text over two lines,
  which MDM passes over.
variant: "light"
palette:
  base00: "#fdf6e3"
  base01: "#eee8d5"
  base02: "#93a1a1"
  base03: "#839496"
  base04: "#657b83"
  base05: "#586e75"
  base06: "#073642"
  base07: "#002b36"
  base08: "#dc322f"
  base09: "#cb4b16"
  base0A: "#b58900"
  base0B: "#859900"
  base0C: "#2aa198"
  base0D: "#268bd2"
  base0E: "#6c71c4"
  base0F: "#d33682"
  base10: "#ffffff"
  base17: "#000000"
"##;

    /// A file as an editor on Windows may save it: a byte-order mark, CR LF line ends.
    fn windows_file(text: &str) -> String {
        format!("\u{feff}{}", text.replace('\n', "\r\n"))
    }

    #[test]
    fn hex_reads_three_and_six_digits() {
        assert_eq!(hex("#0078d4"), Some(rgb(0x0078d4)));
        assert_eq!(hex("0078D4"), Some(rgb(0x0078d4)));
        assert_eq!(hex("#abc"), Some(rgb(0xaabbcc)));
        assert_eq!(hex("fff"), Some(Color32::WHITE));
        for bad in [
            "",
            "#",
            "#12",
            "#1234",
            "#12345",
            "#0078d4ff",
            "#ggg",
            "red",
            "#00 78d",
        ] {
            assert_eq!(hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn base16_classic_layout() {
        let s = base16(DARK_CLASSIC).unwrap();
        assert_eq!(s.name.as_deref(), Some("Test Dark Classic"));
        assert_eq!(s.base[0x0], Some(rgb(0x282828)));
        assert_eq!(s.base[0x3], Some(rgb(0x665c54)));
        assert_eq!(s.base[0x4], Some(rgb(0xbdae93)));
        assert_eq!(s.base[0xD], Some(rgb(0x83a598)));
        assert_eq!(s.base[0xF], Some(rgb(0xd65d0e)));
    }

    #[test]
    fn base16_palette_layout_with_base24_extras() {
        let s = base16(&windows_file(LIGHT_PALETTE)).unwrap();
        assert_eq!(s.name.as_deref(), Some("Test Light"));
        assert_eq!(s.base[0x0], Some(rgb(0xfdf6e3)));
        assert_eq!(s.base[0xD], Some(rgb(0x268bd2)));
        assert!(s.base.iter().all(Option::is_some));
    }

    #[test]
    fn base16_palette_keys_outside_palette_are_passed_over() {
        let text = format!("{DARK_CLASSIC}\nextra:\n  base00: \"#ffffff\"\n");
        assert_eq!(base16(&text).unwrap().base[0], Some(rgb(0x282828)));
    }

    #[test]
    fn base16_plain_hash_values_and_three_digits() {
        let text = DARK_CLASSIC
            .replace("\"282828\" # ----", "#222 # a comment")
            .replace("\"d5c4a1\"", "#ddd");
        let s = base16(&text).unwrap();
        assert_eq!(s.base[0], Some(rgb(0x222222)));
        assert_eq!(s.base[5], Some(rgb(0xdddddd)));
    }

    #[test]
    fn base16_malformed_and_missing() {
        // Not a scheme at all.
        assert!(base16("name: \"x\"\nauthor: \"y\"\n").is_err());
        assert!(base16("").is_err());
        // A line that isn't key: value, a quote left open, text after a quoted value.
        assert!(base16(&format!("{DARK_CLASSIC}\nwhat is this\n")).is_err());
        assert!(base16(&DARK_CLASSIC.replace("\"fb4934\"", "\"fb4934")).is_err());
        assert!(base16(&DARK_CLASSIC.replace("\"fb4934\"", "\"fb4934\" x")).is_err());
        // A colour that isn't one.
        let e = base16(&DARK_CLASSIC.replace("\"83a598\"", "\"83a5\"")).unwrap_err();
        assert!(e.contains("base0d"), "{e}");
        // Colours MDM needs, missing: named.
        let lacking: String = DARK_CLASSIC
            .lines()
            .filter(|l| !l.starts_with("base0D") && !l.starts_with("base05"))
            .collect::<Vec<_>>()
            .join("\n");
        let e = base16(&lacking).unwrap_err();
        assert!(e.contains("base05") && e.contains("base0D"), "{e}");
        // One MDM doesn't use, missing: still read.
        let no_0e: String = DARK_CLASSIC
            .lines()
            .filter(|l| !l.starts_with("base0E"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(base16(&no_0e).unwrap().base[0xE], None);
    }

    #[test]
    fn base16_maps_backgrounds_text_and_accents() {
        let m = map_base16(&base16(DARK_CLASSIC).unwrap());
        assert_eq!(m.surface, rgb(0x282828));
        assert_eq!(m.background, rgb(0x3c3836));
        assert_eq!(m.header, rgb(0x3c3836));
        assert_eq!(m.text, rgb(0xd5c4a1));
        assert_eq!(m.muted, rgb(0xbdae93));
        assert_eq!(m.accent, rgb(0x83a598));
        assert_eq!(m.error, rgb(0xfb4934));
        assert_eq!(m.warning, rgb(0xfabd2f));
        assert_eq!(m.success, rgb(0xb8bb26));
    }

    const PYWAL: &str = r##"{
        "wallpaper": "/home/someone/Pictures/lake.jpg",
        "alpha": "100",
        "special": { "background": "#1b1d24", "foreground": "#c8ccd4", "cursor": "#c8ccd4" },
        "colors": {
            "color0": "#1b1d24", "color1": "#b7605e", "color2": "#7fa173", "color3": "#c9a95c",
            "color4": "#5f87b5", "color5": "#9d7bb0", "color6": "#5fa3a8", "color7": "#c8ccd4",
            "color8": "#5c6370", "color9": "#b7605e", "color10": "#7fa173", "color11": "#c9a95c",
            "color12": "#5f87b5", "color13": "#9d7bb0", "color14": "#5fa3a8", "color15": "#fff"
        }
    }"##;

    #[test]
    fn pywal_reads_and_maps() {
        let v: Value = serde_json::from_str(PYWAL).unwrap();
        assert!(is_pywal(&v));
        let p = pywal(&v).unwrap();
        assert_eq!(p.background, rgb(0x1b1d24));
        assert_eq!(p.colors[15], Some(Color32::WHITE));
        let m = map_pywal(&p);
        assert_eq!(m.surface, rgb(0x1b1d24));
        assert_eq!(m.text, rgb(0xc8ccd4));
        assert_eq!(m.accent, rgb(0x5f87b5));
        assert_eq!(
            (m.error, m.warning, m.success),
            (rgb(0xb7605e), rgb(0xc9a95c), rgb(0x7fa173))
        );
        assert_eq!(m.muted, rgb(0x5c6370));
        // The window a step toward the text: lighter on a dark scheme.
        assert!(crate::derive::luminance(m.background) > crate::derive::luminance(m.surface));
        assert_eq!(pywal_name(&v), "pywal: lake.jpg");
    }

    #[test]
    fn pywal_malformed_and_missing() {
        let v: Value = serde_json::from_str(r#"{ "name": "An MDM theme" }"#).unwrap();
        assert!(!is_pywal(&v));
        // MDM's own JSON theme files use `colors` too, for roles: not pywal's.
        let v: Value = serde_json::from_str(r##"{ "colors": { "accent": "#ff0000" } }"##).unwrap();
        assert!(!is_pywal(&v));
        let v: Value = serde_json::from_str(r##"{ "special": {}, "colors": {} }"##).unwrap();
        let e = pywal(&v).unwrap_err();
        assert!(
            e.contains("special.background") && e.contains("colors.color4"),
            "{e}"
        );
        let bad = PYWAL.replace("\"#5f87b5\", \"color5\"", "\"blue\", \"color5\"");
        let v: Value = serde_json::from_str(&bad).unwrap();
        assert!(pywal(&v).unwrap_err().contains("color4"));
        // color8 missing: secondary text mixed from the text instead.
        let v: Value =
            serde_json::from_str(&PYWAL.replace("\"color8\": \"#5c6370\",", "")).unwrap();
        assert!(map_pywal(&pywal(&v).unwrap()).muted != rgb(0x5c6370));
        let v: Value = serde_json::from_str(r#"{ "wallpaper": "None" }"#).unwrap();
        assert_eq!(pywal_name(&v), "pywal");
    }

    #[test]
    fn readable_schemes_are_left_as_they_are() {
        let mut m = map_base16(&base16(DARK_CLASSIC).unwrap());
        let before = m;
        assert!(m.fix_contrast().is_empty());
        assert_eq!(m, before);
    }

    #[test]
    fn a_scheme_short_of_aa_is_made_readable() {
        // Solarized-like light's text (base05) on its base01 is just under 4.5:1.
        let mut m = map_base16(&base16(LIGHT_PALETTE).unwrap());
        let before = m;
        assert!(contrast(before.text, before.background) < TEXT_RATIO);
        let changed = m.fix_contrast();
        assert!(
            changed.iter().any(|c| c.starts_with("contrast: text ")),
            "{changed:?}"
        );
        for g in [m.background, m.surface, m.header] {
            assert!(contrast(m.text, g) >= TEXT_RATIO);
            assert!(contrast(m.muted, g) >= TEXT_RATIO);
            assert!(contrast(m.accent, g) >= ACCENT_RATIO);
        }
        // Darker, and only a little: still the scheme's colour.
        assert!(crate::derive::luminance(m.text) < crate::derive::luminance(before.text));
        assert!(contrast(m.text, before.text) < 1.5);
        assert_eq!(
            (m.background, m.surface, m.error),
            (before.background, before.surface, before.error)
        );
    }

    #[test]
    fn contrast_fix_ups_reach_aa_on_every_background() {
        // A dark scheme whose text and accent are too dim, and a light one too pale.
        for (bg, surface, text, accent) in [
            (0x303030, 0x262626, 0x5a5a5a, 0x3a3a50),
            (0xf4f4f4, 0xffffff, 0xc0c0c0, 0xd0e0ff),
        ] {
            let mut m = Mapped {
                background: rgb(bg),
                surface: rgb(surface),
                header: rgb(bg),
                text: rgb(text),
                muted: rgb(text),
                accent: rgb(accent),
                error: rgb(0xff0000),
                warning: rgb(0xffff00),
                success: rgb(0x00ff00),
            };
            let changed = m.fix_contrast();
            assert_eq!(changed.len(), 3, "{changed:?}");
            for g in [m.background, m.surface, m.header] {
                assert!(contrast(m.text, g) >= TEXT_RATIO);
                assert!(contrast(m.muted, g) >= TEXT_RATIO);
                assert!(contrast(m.accent, g) >= ACCENT_RATIO);
            }
            // Backgrounds are never changed.
            assert_eq!((m.background, m.surface), (rgb(bg), rgb(surface)));
        }
    }

    #[test]
    fn a_scheme_is_a_theme_on_the_classic_look() {
        let (v, _) = base16_theme(DARK_CLASSIC, "file-name").unwrap();
        let l = crate::themefile::parse("gruv", &v, |_| {
            Ok(crate::themefile::Loaded {
                theme: crate::palette::CLASSIC.clone(),
                warnings: Vec::new(),
            })
        })
        .unwrap();
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        let t = l.theme;
        assert_eq!(t.name, "Test Dark Classic");
        // Both sides take the scheme; the layout and sizes stay the classic look's.
        assert_eq!(t.light, t.dark);
        assert_eq!(t.metrics, crate::palette::CLASSIC.metrics);
        assert_eq!(t.controls, crate::palette::CLASSIC.controls);
        let p = &t.dark;
        assert!(p.dark);
        assert_eq!(p.window, rgb(0x3c3836));
        assert_eq!(p.field, rgb(0x282828));
        assert_eq!(p.list, rgb(0x282828));
        assert_eq!(p.text, rgb(0xd5c4a1));
        assert_eq!(p.accent, rgb(0x83a598));
        assert_eq!(p.selection, rgb(0x83a598));
        assert_eq!(p.progress_bar, rgb(0x83a598));
        assert!(contrast(p.selection_text, p.selection) >= TEXT_RATIO);
        assert!(contrast(p.muted, p.window) >= TEXT_RATIO);
        // A light scheme is a light theme; a nameless one is named after its file.
        let nameless: String = LIGHT_PALETTE
            .lines()
            .filter(|l| !l.starts_with("name") && !l.starts_with("scheme"))
            .collect::<Vec<_>>()
            .join("\n");
        let (v, _) = base16_theme(&nameless, "my-scheme").unwrap();
        assert_eq!(v["name"], "my-scheme");
        let l = crate::themefile::parse("light", &v, |_| {
            Ok(crate::themefile::Loaded {
                theme: crate::palette::CLASSIC.clone(),
                warnings: Vec::new(),
            })
        })
        .unwrap();
        assert!(!l.theme.light.dark);
        assert!(contrast(l.theme.light.text, l.theme.light.window) >= TEXT_RATIO);
    }
}
