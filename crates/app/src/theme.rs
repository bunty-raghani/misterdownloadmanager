//! The look: light or dark (the mode), the active theme's colours and sizes (#45), the fonts,
//! and egui's own visuals built from them.

use egui::{CornerRadius, Stroke, Visuals};

use std::cell::Cell;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::palette::{CLASSIC, Metrics, Palette, Theme};

static DARK: AtomicBool = AtomicBool::new(true);

/// Is the UI in dark mode? IDM: dark when "View → Dark Mode support" is on and Windows' app
/// mode is dark (IDM 6.37); light otherwise. It picks the active theme's dark or light variant
/// ([`colors`]); whether that variant is a dark surface is [`dark_surface`].
pub fn dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

// ---- the active theme ------------------------------------------------------------------------
//
// Painters read colours and sizes hundreds of times a frame, from every window's thread: each
// thread keeps a reference to the active theme and looks again only when the generation moved
// (one atomic load per read, no lock, no allocation). A theme set from a file is leaked: it
// lives as long as the program (a few KB per change), so a reference to it never dangles.

static ACTIVE: RwLock<&'static Theme> = RwLock::new(&CLASSIC);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static SEEN: Cell<(u64, &'static Theme)> = const { Cell::new((0, &CLASSIC)) };
}

/// The active theme.
pub fn current() -> &'static Theme {
    let generation = GENERATION.load(Ordering::Acquire);
    SEEN.with(|seen| {
        let (at, theme) = seen.get();
        if at == generation {
            return theme;
        }
        let theme = *ACTIVE.read().unwrap_or_else(|e| e.into_inner());
        seen.set((generation, theme));
        theme
    })
}

/// The active theme's colours in the current mode.
pub fn colors() -> &'static Palette {
    current().palette(dark())
}

/// The active theme's sizes.
pub fn metrics() -> &'static Metrics {
    &current().metrics
}

/// Is what is drawn now a dark surface (light text on dark)? The classic look's dark variant
/// is; a theme says for each of its variants. Glyph rendering and the window captions follow it.
pub fn dark_surface() -> bool {
    colors().dark
}

/// Makes `theme` the active one.
pub fn set(theme: Theme) {
    let theme: &'static Theme = match crate::palette::builtins()
        .into_iter()
        .find(|b| **b == theme)
    {
        Some(builtin) => builtin,
        None => Box::leak(Box::new(theme)),
    };
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = theme;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Makes the theme with this id the active one: a built-in, or a file in the themes folder
/// (`themefile`), or a theme file's path; Native is made from what the system says. A theme
/// that can't be read leaves the classic look, logged. Returns what was skipped or went wrong.
pub fn select(id: &str, ctx: &egui::Context) -> Vec<String> {
    let id = id.to_owned();
    // pywal's file is followed again only if this theme is made from it (`themefile::load_in`).
    crate::themefile::forget_pywal();
    *SELECTED.lock().unwrap_or_else(|e| e.into_inner()) = id.clone();
    if id.is_empty() || id == CLASSIC.id {
        set(CLASSIC.clone());
        return Vec::new();
    }
    if id == "native" {
        let system = crate::native::system(ctx);
        *NATIVE_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(system);
        set(crate::native::theme(&system));
        return Vec::new();
    }
    match load(&id) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                tracing::warn!("theme {id}: {w}");
            }
            set(loaded.theme);
            loaded.warnings
        }
        Err(e) => {
            tracing::warn!("theme {id}: {e}; the classic look instead");
            set(CLASSIC.clone());
            vec![e]
        }
    }
}

/// A theme file's theme, by its id or its path; pywal's file followed from now on when the
/// theme is made from it.
fn load(id: &str) -> Result<crate::themefile::Loaded, String> {
    let (loaded, followed) = crate::themefile::read(id);
    if loaded.is_ok() {
        crate::themefile::follow_pywal(followed);
    }
    loaded
}

/// The id (or path) [`select`] was last given.
static SELECTED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The id (or path) of the theme chosen last.
pub fn selected() -> String {
    SELECTED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// While the theme is made from pywal's colours (#45): read again when `wal` wrote them anew,
/// as it does for each new wallpaper; `lookwatch`'s thread reads it (#719), this takes what it
/// read while the theme and the file it follows are still those it was read for. A file that
/// doesn't read (half written as it was read) keeps the theme as it is, and is read again when
/// it is next written. True when the theme changed (the caller applies it: `apply_theme`,
/// `apply_mode`).
pub fn follow_pywal(files: Option<&crate::lookwatch::ThemeFiles>) -> bool {
    let Some(r) = files.and_then(|f| f.pywal.as_ref()) else {
        return false;
    };
    if selected() != r.id || crate::themefile::following_pywal().as_ref() != Some(&r.followed) {
        return false;
    }
    let id = &r.id;
    match &r.read {
        Ok((theme, _)) => {
            tracing::info!("theme {id}: pywal's colours changed, read again");
            set(theme.clone());
            crate::themefile::follow_pywal(r.follow.clone());
            true
        }
        Err(e) => {
            tracing::warn!("theme {id}: {e}; the theme kept until pywal's file is written again");
            crate::themefile::pywal_seen_as(r.written);
            false
        }
    }
}

/// The theme to start with: `MDM_THEME` (tests and screenshots: an id or a theme file's path),
/// else the setting.
pub fn startup_id(setting: &str) -> String {
    std::env::var("MDM_THEME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| setting.to_owned())
}

/// What the system said when Native was last made.
static NATIVE_SEEN: std::sync::Mutex<Option<crate::native::System>> = std::sync::Mutex::new(None);

/// While Native is the theme: made again when the system's accent or contrast changed. True
/// when it was (the caller applies it: `apply_theme`, `apply_mode`).
pub fn follow_system(ctx: &egui::Context) -> bool {
    if current().id != "native" {
        return false;
    }
    let system = crate::native::system(ctx);
    let mut seen = NATIVE_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if *seen == Some(system) {
        return false;
    }
    *seen = Some(system);
    drop(seen);
    set(crate::native::theme(&system));
    true
}

/// Match egui's glyph rendering to the surface: a dark one thickens light-on-dark text
/// (`TwoCoverageMinusCoverageSq`), which makes black-on-white text look heavy, so a light one
/// uses plain coverage. The glyph atlas is shared, so this follows the main window's.
pub fn apply_mode(ctx: &egui::Context) {
    let dark_surface = dark_surface();
    let tf = if dark_surface {
        egui::epaint::FontColorTransferFunction::DARK_MODE_DEFAULT
    } else {
        egui::epaint::FontColorTransferFunction::LIGHT_MODE_DEFAULT
    };
    ctx.all_styles_mut(|s| s.visuals.text_options.color_transfer_function = tf);
    // egui's dark or light style is the mode's variant (`apply_theme` puts each variant into its
    // egui theme). It leaves the windows' captions alone (`apply`: `sync_window_theme`).
    ctx.set_theme(if dark() {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    // The window's own theme (caption, DWM): otherwise winit follows the system theme.
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::SetTheme(system_theme(dark_surface)),
    );
}

/// The winit window theme for a dark (true) or light caption.
pub fn system_theme(dark: bool) -> egui::SystemTheme {
    if dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    }
}

/// Decide the mode from the setting and the system's: Windows' app mode; on Linux the desktop's
/// preference (the freedesktop portal), else what the window system reports; on macOS the
/// appearance the window system reports (light when nothing says). True if it changed.
/// `always_dark` (View → Always dark, #60): dark whatever the system says, for desktops that
/// report no preference.
/// On Windows the registry is read by `lookwatch`'s thread, never this one (#719).
pub fn update(dark_mode_support: bool, always_dark: bool, ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    let system_dark = crate::lookwatch::system(ctx).dark;
    #[cfg(target_os = "linux")]
    let system_dark = crate::unix::desktop_dark(ctx)
        .unwrap_or_else(|| ctx.system_theme() == Some(egui::Theme::Dark));
    #[cfg(not(any(windows, target_os = "linux")))]
    let system_dark = ctx.system_theme() == Some(egui::Theme::Dark);
    let want = wanted(dark_mode_support, always_dark, system_dark);
    DARK.swap(want, Ordering::Relaxed) != want
}

/// Did the desktop say its look changed since the last call, or the theme files? The watchers
/// count each change: on Linux the portal's and the theme settings' (`unix::desktop_generation`),
/// everywhere `lookwatch`'s (Windows' registry, the themes folder, pywal's file). The app's theme
/// check, otherwise every 2 s, then runs in the frame the change woke (#156, #719).
pub fn system_changed() -> bool {
    static WATCHED: AtomicU64 = AtomicU64::new(0);
    let now = crate::lookwatch::generation();
    let watched = WATCHED.swap(now, Ordering::Relaxed) != now;
    #[cfg(target_os = "linux")]
    {
        static SEEN: AtomicU64 = AtomicU64::new(0);
        let now = crate::unix::desktop_generation();
        (SEEN.swap(now, Ordering::Relaxed) != now) | watched
    }
    #[cfg(not(target_os = "linux"))]
    watched
}

/// Dark or not: View → Always dark, else the system's mode while Dark Mode support is on.
fn wanted(dark_mode_support: bool, always_dark: bool, system_dark: bool) -> bool {
    always_dark || (dark_mode_support && system_dark)
}

/// A module's names for the active theme's colours: `pub NAME => role;` becomes `NAME()`, the
/// role's colour in the current mode ([`colors`]).
macro_rules! themed {
    ($($(#[$m:meta])* $vis:vis $name:ident => $role:ident;)*) => {$(
        $(#[$m])*
        #[allow(non_snake_case)]
        $vis fn $name() -> egui::Color32 {
            $crate::theme::colors().$role
        }
    )*};
}
pub(crate) use themed;

/// The system's fonts for the scripts the UI font lacks (D-019; nothing bundled): a key, the
/// files to try (in the fonts folder), the interface languages that need it. CJK fonts are large
/// (13–20 MB), so only the interface language's are loaded, and all of them while the language
/// picker shows every language by its own name ([`all_scripts`]). Windows: Segoe UI and Tahoma
/// already cover Arabic, Hebrew, Armenian and Georgian.
#[cfg(windows)]
const SCRIPT_FONTS: &[(&str, &[&str], &[&str])] = &[
    ("han-sc", HAN_SC, &["zh-hans"]),
    ("han-tc", HAN_TC, &["zh-hant"]),
    ("japanese", JAPANESE, &["ja"]),
    ("hangul", HANGUL, &["ko"]),
    (
        "indic",
        &["Nirmala.ttc", "Nirmala.ttf"],
        &[
            "as", "bn", "gu", "hi", "kn", "kok", "ml", "mr", "ne", "or", "pa", "ta", "te",
        ],
    ),
    ("southeast-asian", &["LeelawUI.ttf"], &["km", "lo", "th"]),
    ("ethiopic", &["ebrima.ttf"], &["am"]),
    ("cherokee", &["gadugi.ttf"], &["chr"]),
];

/// Windows' Simplified Chinese fonts, best first (#1605): Microsoft YaHei (a collection since
/// Windows 8, a single font on Windows 7), DengXian (Windows 10 and later), SimSun, SimHei. A
/// Windows without the first (Windows 7's is `msyh.ttf`; a copy whose fonts were stripped) has
/// one of the others, which have every Simplified character; a Japanese font has the characters
/// Chinese shares with Japanese, in their Japanese shapes, and few of the Simplified-only ones.
#[cfg(windows)]
const HAN_SC: &[&str] = &[
    "msyh.ttc",
    "msyh.ttf",
    "Deng.ttf",
    "simsun.ttc",
    "simhei.ttf",
];
/// Traditional Chinese: Microsoft JhengHei (a single font on Windows 7), then MingLiU.
#[cfg(windows)]
const HAN_TC: &[&str] = &["msjh.ttc", "msjh.ttf", "mingliu.ttc"];
/// Japanese: Yu Gothic, Meiryo, MS Gothic.
#[cfg(windows)]
const JAPANESE: &[&str] = &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"];
/// Korean: Malgun Gothic.
#[cfg(windows)]
const HANGUL: &[&str] = &["malgun.ttf"];

/// Characters only Simplified Chinese writes this way (载链浏览动设选项这们时为发开关; the
/// Traditional forms are [`TRADITIONAL_ONLY`]'s): a font that has them all is a Simplified
/// Chinese font, which a Japanese or Traditional one isn't. The reporter's boxes were these
/// (#1605).
#[cfg_attr(not(windows), allow(dead_code))]
const SIMPLIFIED_ONLY: &str = "\u{8F7D}\u{94FE}\u{6D4F}\u{89C8}\u{52A8}\u{8BBE}\u{9009}\u{9879}\u{8FD9}\u{4EEC}\u{65F6}\u{4E3A}\u{53D1}\u{5F00}\u{5173}";
/// The same words' Traditional forms (載鏈瀏覽動設選項這們時為發開關).
#[cfg_attr(not(windows), allow(dead_code))]
const TRADITIONAL_ONLY: &str = "\u{8F09}\u{93C8}\u{700F}\u{89BD}\u{52D5}\u{8A2D}\u{9078}\u{9805}\u{9019}\u{5011}\u{6642}\u{70BA}\u{767C}\u{958B}\u{95DC}";

/// What a script font loaded up front must cover to be taken ([`script_fonts_in`]), and the
/// script's name in the log: the Chinese fonts must be the Chinese they are for. The other
/// scripts take the first of their files egui can draw with.
#[cfg(windows)]
fn script_probe(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "han-sc" => Some(("Chinese (Simplified)", SIMPLIFIED_ONLY)),
        "han-tc" => Some(("Chinese (Traditional)", TRADITIONAL_ONLY)),
        _ => None,
    }
}

/// Whether face `index` of `bytes` has a glyph for every character of `text`.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn covers(bytes: &[u8], index: u32, text: &str) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| {
        let map = font.charmap();
        text.chars().all(|c| map.map(c).is_some())
    })
}

/// The font files for Chinese and Japanese characters (Han), in the order they are tried
/// (#1605), for an interface in `code`: its own script's first, then the other Chinese, then
/// the Japanese; so a Chinese interface never draws a character Chinese shares with Japanese in
/// a Japanese face while a Chinese font is there. A Simplified Chinese interface, and any whose
/// language isn't Chinese, Japanese or Korean, starts with the Simplified fonts; a Korean one
/// with its own font (Malgun Gothic has Hanja). One list for every way MDM finds a font for a
/// Han character: the ones loaded up front start it ([`SCRIPT_FONTS`]), the fonts found on
/// demand follow it ([`ScriptFallback`]), and so does DirectWrite's classic text
/// ([`cjk_font_files_for`]).
#[cfg(windows)]
pub(crate) fn han_files(code: &str) -> Vec<&'static str> {
    let lists: &[&[&str]] = match code {
        "zh-hant" => &[HAN_TC, HAN_SC, JAPANESE],
        "ja" => &[JAPANESE, HAN_SC, HAN_TC],
        "ko" => &[HANGUL, HAN_SC, HAN_TC, JAPANESE],
        _ => &[HAN_SC, HAN_TC, JAPANESE],
    };
    let mut files = Vec::new();
    for file in lists.iter().copied().flatten() {
        if !files.contains(file) {
            files.push(*file);
        }
    }
    files
}

/// macOS's fonts, in /System/Library/Fonts: the specific ones first, then Arial Unicode for
/// every other script the system font lacks.
#[cfg(target_os = "macos")]
const SCRIPT_FONTS: &[(&str, &[&str], &[&str])] = &[
    (
        "han",
        &[
            "PingFang.ttc",
            "Hiragino Sans GB.ttc",
            "STHeiti Light.ttc",
            "Supplemental/Songti.ttc",
        ],
        &["zh-hans", "zh-hant"],
    ),
    ("japanese", &["ヒラギノ角ゴシック W3.ttc"], &["ja"]),
    ("hangul", &["AppleSDGothicNeo.ttc"], &["ko"]),
    ("khmer", &["Supplemental/Khmer Sangam MN.ttf"], &["km"]),
    ("lao", &["Supplemental/Lao Sangam MN.ttf"], &["lo"]),
    ("ethiopic", &["Supplemental/Kefa.ttc", "Kefa.ttc"], &["am"]),
    (
        "cherokee",
        &["Supplemental/Plantagenet Cherokee.ttf"],
        &["chr"],
    ),
    (
        "unicode",
        &["Supplemental/Arial Unicode.ttf"],
        &[
            "ar", "fa", "ur", "ug", "he", "hy", "ka", "as", "bn", "gu", "hi", "kn", "kok", "ml",
            "mr", "ne", "or", "pa", "ta", "te", "th", "ja", "ko", "zh-hans", "zh-hant",
        ],
    ),
];

/// Linux: fontconfig names the font for each language (`fc-match :lang=…`), whatever the
/// distribution installs; these are the languages whose script needs one, as fontconfig names
/// them (one per script is enough for the picker's list of all languages).
#[cfg(not(any(windows, target_os = "macos")))]
const SCRIPT_LANGS: &[(&str, &str)] = &[
    ("ar", "ar"),
    ("fa", "fa"),
    ("ur", "ur"),
    ("ug", "ug"),
    ("he", "he"),
    ("hy", "hy"),
    ("ka", "ka"),
    ("am", "am"),
    ("chr", "chr"),
    ("km", "km"),
    ("lo", "lo"),
    ("th", "th"),
    ("ja", "ja"),
    ("ko", "ko"),
    ("zh-hans", "zh-cn"),
    ("zh-hant", "zh-tw"),
    ("hi", "hi"),
    ("mr", "mr"),
    ("ne", "ne"),
    ("kok", "kok"),
    ("as", "as"),
    ("bn", "bn"),
    ("gu", "gu"),
    ("pa", "pa"),
    ("or", "or"),
    ("ta", "ta"),
    ("te", "te"),
    ("kn", "kn"),
    ("ml", "ml"),
];

/// Marks that neither the interface font nor the script fonts have (✔ ✖ ↩ ⇧ ⌘ ⌥ ⌦, and → on
/// Linux: #185): the system's symbol fonts, last in every family's chain, in every language,
/// each one that is there. macOS keeps ✔ and ✖ in Zapf Dingbats, not in Apple Symbols.
#[cfg(windows)]
const SYMBOL_FONTS: &[&str] = &["seguisym.ttf"];
#[cfg(target_os = "macos")]
const SYMBOL_FONTS: &[&str] = &["Apple Symbols.ttf", "ZapfDingbats.ttf"];
/// Linux: the font fontconfig picks for those marks (glyph_tests lists every one the windows
/// draw; a new one goes here too).
#[cfg(not(any(windows, target_os = "macos")))]
const SYMBOL_PATTERN: &str = ":charset=2024 2192 21a9 21e7 2318 2325 2326 2714 2716";

/// The symbol fonts' bytes, in fallback order ([`SYMBOL_FONTS`], [`SYMBOL_PATTERN`]).
fn symbol_fonts() -> Vec<Vec<u8>> {
    #[cfg(any(windows, target_os = "macos"))]
    return SYMBOL_FONTS
        .iter()
        .filter_map(|f| read_font(&font_dir().join(f)))
        .collect();
    #[cfg(not(any(windows, target_os = "macos")))]
    return fc_font(SYMBOL_PATTERN)
        .map(|(_, bytes)| bytes)
        .into_iter()
        .collect();
}

/// Whether egui can draw with `bytes`' face `index`: a TrueType or OpenType font, or a face of a
/// collection of them, as the parser egui reads fonts with (skrifa) reads it. A bitmap font (PCF or
/// BDF, gzipped or not), a Type 1 font, a file cut short, a face the collection doesn't have or
/// anything else is not one, and egui stops on a font it was given and can't read (#931:
/// fontconfig named a gzipped PCF font for Japanese; #1412: a BDF font for the symbols).
fn loadable_font(bytes: &[u8], index: u32) -> bool {
    use skrifa::raw::TableProvider as _;
    let Ok(font) = skrifa::FontRef::from_index(bytes, index) else {
        return false;
    };
    // A font whose list of tables is whole but whose tables are cut short (a copy that stopped)
    // passes the first look: every table it lists is in the file, and those every font has read.
    font.table_directory
        .table_records()
        .iter()
        .all(|table| font.table_data(table.tag()).is_some())
        && font.head().is_ok()
        && font.hhea().is_ok()
        && font.maxp().is_ok()
        && font.cmap().is_ok()
}

/// The face of `bytes` the interface draws with (#1483). A single font has one, face 0. A
/// collection (.ttc) holds a family's faces in the order its maker chose, and its first can be a
/// display weight: macOS's Songti.ttc starts with Songti SC Black, which made Chinese text heavy
/// and bold. So face 0 is kept when it reads as regular text ([`regular_face`]: upright, normal
/// width, from Light to Medium, as the files the tables name on purpose are: Yu Gothic Medium,
/// Hiragino's W3), and otherwise the collection's face closest to Regular (400) is taken, the
/// first of equals. A collection with no such face (a Bold-only file) keeps face 0.
pub(crate) fn text_face(bytes: &[u8]) -> u32 {
    use skrifa::raw::FileRef;
    let Ok(FileRef::Collection(collection)) = FileRef::new(bytes) else {
        return 0;
    };
    if regular_face(bytes, 0).is_some() {
        return 0;
    }
    let mut best: Option<(u32, f32)> = None;
    for index in 1..collection.len() {
        if let Some(off) = regular_face(bytes, index)
            && best.is_none_or(|(_, b)| off < b)
        {
            best = Some((index, off));
        }
    }
    best.map_or(0, |(index, _)| index)
}

/// How far face `index` of `bytes` is from Regular (its weight's distance from 400), when it is
/// a face egui can draw with ([`loadable_font`]) that reads as regular text: upright, normal
/// width, weight 300–500 (Light, W3, Regular, Book, Medium), and a subfamily name that doesn't
/// call it a display face (a collection whose weights all say 400 still names its Black one).
fn regular_face(bytes: &[u8], index: u32) -> Option<f32> {
    use skrifa::attribute::{Stretch, Style};
    use skrifa::{MetadataProvider as _, string::StringId};
    if !loadable_font(bytes, index) {
        return None;
    }
    let font = skrifa::FontRef::from_index(bytes, index).ok()?;
    let attributes = font.attributes();
    let weight = attributes.weight.value();
    if attributes.style != Style::Normal
        || attributes.stretch != Stretch::NORMAL
        || !(300.0..=500.0).contains(&weight)
    {
        return None;
    }
    const DISPLAY: &[&str] = &[
        "black",
        "heavy",
        "bold",
        "thin",
        "hairline",
        "ultra",
        "extra",
        "italic",
        "oblique",
        "condensed",
        "expanded",
    ];
    for id in [
        StringId::TYPOGRAPHIC_SUBFAMILY_NAME,
        StringId::SUBFAMILY_NAME,
    ] {
        if let Some(name) = font.localized_strings(id).english_or_first() {
            let name = name.to_string().to_ascii_lowercase();
            if DISPLAY.iter().any(|word| name.contains(word)) {
                return None;
            }
        }
    }
    Some((weight - 400.0).abs())
}

/// `bytes` as egui draws with them: at the face the interface uses ([`text_face`]).
pub(crate) fn font_data(bytes: Vec<u8>) -> egui::FontData {
    let index = text_face(&bytes);
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    data
}

/// The faces [`read_font`] chose, by file: Windows' DirectWrite draws CJK text with the same
/// face egui has ([`font_face_index`]).
static FACES: std::sync::Mutex<Option<std::collections::HashMap<std::path::PathBuf, u32>>> =
    std::sync::Mutex::new(None);

/// The face of the font file at `path` the interface draws with ([`text_face`]): the one egui
/// was given, read again only for a file egui wasn't given.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn font_face_index(path: &std::path::Path) -> u32 {
    let known = FACES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|faces| faces.get(path).copied());
    known
        .or_else(|| read_font(path).map(|bytes| text_face(&bytes)))
        .unwrap_or(0)
}

/// A font file's bytes, when egui can draw with them ([`loadable_font`], at the face
/// [`text_face`] chooses). Any other file is passed over, and the log says so (once a run: the
/// fonts are loaded again with each change of language): the next font in line is used instead.
pub(crate) fn read_font(path: &std::path::Path) -> Option<Vec<u8>> {
    static SAID: std::sync::Mutex<Option<std::collections::HashSet<std::path::PathBuf>>> =
        std::sync::Mutex::new(None);
    let bytes = std::fs::read(path).ok()?;
    // Given to egui by `font_data`, at the same face.
    let face = text_face(&bytes);
    if loadable_font(&bytes, face) {
        FACES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Default::default)
            .insert(path.to_owned(), face);
        return Some(bytes);
    }
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said
        .get_or_insert_with(Default::default)
        .insert(path.to_owned())
    {
        tracing::warn!(
            "fonts: {} is not a TrueType or OpenType font, passed over",
            path.display()
        );
    }
    None
}

/// The last look at the fonts before egui gets them: each one egui can read ([`loadable_font`], at
/// the face it names) stays, any other is left out of the fonts and of every family that names it,
/// and the log says so. Every system font MDM adds came through [`read_font`] already; this keeps
/// a font from any other path (or a face of a collection egui can't find) from stopping MDM as it
/// starts, which egui does with a font it was given and can't parse (#1412).
fn only_loadable_fonts(defs: &mut egui::FontDefinitions) {
    defs.font_data.retain(|name, data| {
        let loadable = loadable_font(data.bytes(), data.index);
        if !loadable {
            tracing::warn!(
                "fonts: {name} is not a TrueType or OpenType font egui can read, left out"
            );
        }
        loadable
    });
    for list in defs.families.values_mut() {
        list.retain(|name| defs.font_data.contains_key(name));
    }
}

/// The folder the font tables name files in.
#[cfg(any(windows, target_os = "macos"))]
fn font_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    return std::env::var_os("WINDIR")
        .map(|w| std::path::PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows\Fonts"));
    #[cfg(target_os = "macos")]
    return std::path::PathBuf::from("/System/Library/Fonts");
}

/// Resolves a font file path in `dir`. On macOS 15+ (Sequoia / Tahoe), `PingFang.ttc` moved out
/// of `/System/Library/Fonts/PingFang.ttc` into `/System/Library/AssetsV2/com_apple_MobileAsset_Font*`.
#[cfg(any(windows, target_os = "macos"))]
fn font_path(dir: &std::path::Path, rel: &str) -> std::path::PathBuf {
    let direct = dir.join(rel);
    #[cfg(target_os = "macos")]
    if rel == "PingFang.ttc" && !direct.exists() {
        for base in [
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font8",
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font7",
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font6",
        ] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path().join("AssetData/PingFang.ttc");
                    if p.is_file() {
                        return p;
                    }
                }
            }
        }
    }
    direct
}

/// The fonts for scripts outside the interface language, loaded the first time a character
/// needs one (#405): a file or video named in Bangla, Japanese, Thai... The interface language's
/// own script fonts are loaded up front ([`SCRIPT_FONTS`], kept small for #72); anything else
/// comes from the system on demand, only when text in that script is shown.
///
/// Windows and macOS: the blocks below name the system's font for each script. Linux: fontconfig
/// is asked for a font that has the character (`:charset=`), one question per Unicode block.
/// The fonts folder and the interface language are the system's and the app's, unless a test
/// names its own.
#[derive(Default)]
struct ScriptFallback {
    dir: Option<std::path::PathBuf>,
    code: Option<String>,
}

/// One script's fonts on demand: a key, its code points, the files to try (in order).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
struct FallbackFont {
    /// Read on Windows (the Han list, [`fallback_files`]) and by the tests.
    #[cfg_attr(not(windows), allow(dead_code))]
    key: &'static str,
    ranges: &'static [(u32, u32)],
    files: &'static [&'static str],
}

/// Windows' fonts for the scripts egui's fonts and Segoe UI don't cover.
#[cfg(windows)]
const FALLBACK_FONTS: &[FallbackFont] = &[
    FallbackFont {
        key: "indic",
        ranges: &[(0x0900, 0x0DFF), (0x1C50, 0x1C7F)],
        files: &["Nirmala.ttc", "Nirmala.ttf"],
    },
    FallbackFont {
        key: "thai-lao-khmer",
        ranges: &[(0x0E00, 0x0EFF), (0x1780, 0x17FF), (0x19E0, 0x19FF)],
        files: &["LeelawUI.ttf", "leelawdb.ttf", "tahoma.ttf"],
    },
    FallbackFont {
        key: "myanmar",
        ranges: &[(0x1000, 0x109F), (0xA9E0, 0xA9FF), (0xAA70, 0xAA7F)],
        files: &["mmrtext.ttf"],
    },
    FallbackFont {
        key: "tibetan",
        ranges: &[(0x0F00, 0x0FFF)],
        files: &["himalaya.ttf"],
    },
    FallbackFont {
        key: "ethiopic",
        ranges: &[(0x1200, 0x139F), (0x2D80, 0x2DDF), (0xAB00, 0xAB2F)],
        files: &["ebrima.ttf"],
    },
    FallbackFont {
        key: "cherokee",
        ranges: &[(0x13A0, 0x13FF), (0xAB70, 0xABBF)],
        files: &["gadugi.ttf"],
    },
    FallbackFont {
        key: "hangul",
        ranges: &[
            (0x1100, 0x11FF),
            (0x3130, 0x318F),
            (0xA960, 0xA97F),
            (0xAC00, 0xD7FF),
        ],
        files: &["malgun.ttf"],
    },
    FallbackFont {
        key: "japanese",
        ranges: &[(0x3040, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)],
        files: &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
    },
    FallbackFont {
        key: "han",
        ranges: &[
            (0x2E80, 0x2FFF),
            (0x3000, 0x303F),
            (0x3100, 0x312F),
            (0x31A0, 0x31BF),
            (0x3200, 0x33FF),
            (0x3400, 0x4DBF),
            (0x4E00, 0x9FFF),
            (0xF900, 0xFAFF),
            (0xFE30, 0xFE4F),
            (0xFF00, 0xFF65),
            (0xFFE0, 0xFFEF),
            (0x20000, 0x3FFFF),
        ],
        // In the interface language's order ([`fallback_files`], [`han_files`]).
        files: &[],
    },
];

/// The files to try for a character of `font`'s, in order, for an interface in `code`: the
/// table's, and on Windows the Han list in the interface language's order ([`han_files`]).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn fallback_files(font: &FallbackFont, code: &str) -> Vec<&'static str> {
    #[cfg(windows)]
    if font.key == "han" {
        return han_files(code);
    }
    let _ = code;
    font.files.to_vec()
}

/// macOS's fonts for the same scripts; Arial Unicode for what isn't listed. Compiled everywhere,
/// so the tests check it on every system.
#[cfg(target_os = "macos")]
const FALLBACK_FONTS: &[FallbackFont] = MACOS_FALLBACK_FONTS;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS_FALLBACK_FONTS: &[FallbackFont] = &[
    FallbackFont {
        key: "japanese",
        ranges: &[(0x3040, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)],
        files: &["ヒラギノ角ゴシック W3.ttc", "Hiragino Sans GB.ttc"],
    },
    FallbackFont {
        key: "hangul",
        ranges: &[
            (0x1100, 0x11FF),
            (0x3130, 0x318F),
            (0xA960, 0xA97F),
            (0xAC00, 0xD7FF),
        ],
        files: &["AppleSDGothicNeo.ttc"],
    },
    FallbackFont {
        key: "han",
        ranges: &[
            (0x2E80, 0x303F),
            (0x3100, 0x312F),
            (0x3200, 0x4DBF),
            (0x4E00, 0x9FFF),
            (0xF900, 0xFAFF),
            (0xFE30, 0xFE4F),
            (0xFF00, 0xFFEF),
            (0x20000, 0x3FFFF),
        ],
        files: &[
            "PingFang.ttc",
            "Hiragino Sans GB.ttc",
            "STHeiti Light.ttc",
            "Supplemental/Songti.ttc",
        ],
    },
    FallbackFont {
        key: "khmer",
        ranges: &[(0x1780, 0x17FF), (0x19E0, 0x19FF)],
        files: &["Supplemental/Khmer Sangam MN.ttf"],
    },
    FallbackFont {
        key: "lao",
        ranges: &[(0x0E80, 0x0EFF)],
        files: &["Supplemental/Lao Sangam MN.ttf"],
    },
    FallbackFont {
        key: "ethiopic",
        ranges: &[(0x1200, 0x139F), (0x2D80, 0x2DDF), (0xAB00, 0xAB2F)],
        files: &["Supplemental/Kefa.ttc", "Kefa.ttc"],
    },
    // Sinhala by name, not left to the system's cascade ([`SystemFallback`]): that is asked with
    // a sample of the script, whose first letter (U+0D81) macOS's Sinhala fonts lack. On a Mac
    // that has a font with it (November India, one macOS downloads on demand) the cascade names
    // that font, which isn't among the installed ones it is then looked up in, and Sinhala names
    // were boxes (#1592).
    FallbackFont {
        key: "sinhala",
        ranges: &[(0x0D80, 0x0DFF)],
        files: &[
            "Supplemental/Sinhala Sangam MN.ttc",
            "Supplemental/Sinhala MN.ttc",
        ],
    },
    // Armenian, Hebrew and the Arabic script (Persian, Urdu: #533) too: the system font (SF)
    // has none of them, and Windows' Segoe UI, which has them, isn't there. The same font an
    // Arabic or Persian interface loads ([`SCRIPT_FONTS`]).
    FallbackFont {
        key: "unicode",
        ranges: &[
            (0x0530, 0x13FF),
            (0x1800, 0x1FFF),
            (0xFB1D, 0xFDFF),
            (0xFE70, 0xFEFF),
        ],
        files: &["Supplemental/Arial Unicode.ttf"],
    },
];

/// The font in `fonts` for `c`: the first that lists its code point.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn fallback_entry(fonts: &'static [FallbackFont], c: char) -> Option<&'static FallbackFont> {
    let cp = c as u32;
    fonts
        .iter()
        .find(|f| f.ranges.iter().any(|&(a, b)| (a..=b).contains(&cp)))
}

/// The CJK script `c` belongs to, or `None`: Chinese and Japanese characters (`"han"`), kana
/// (`"japanese"`) and Hangul (`"hangul"`). The combining tone marks Japanese and Chinese put on
/// kana and letters (U+302A–302F, U+3099–309A) are `None`: a shaper must place them with their
/// base, so egui keeps them (as it keeps every multi-codepoint cluster).
#[cfg(windows)]
pub(crate) fn cjk_script(c: char) -> Option<&'static str> {
    let cp = c as u32;
    if matches!(cp, 0x302A..=0x302F | 0x3099..=0x309A) {
        return None;
    }
    if matches!(
        cp,
        0x3040..=0x30FF // hiragana, katakana
        | 0x31F0..=0x31FF // katakana phonetic extensions
        | 0xFF66..=0xFF9D // halfwidth katakana
    ) {
        return Some("japanese");
    }
    if matches!(cp, 0x3130..=0x318F | 0xAC00..=0xD7A3) {
        return Some("hangul");
    }
    if matches!(
        cp,
        0x2E80..=0x2FFF // radicals
        | 0x3000..=0x303F // CJK punctuation
        | 0x3100..=0x312F // bopomofo
        | 0x31A0..=0x31BF // bopomofo extended
        | 0x3200..=0x33FF // enclosed and compatibility characters
        | 0x3400..=0x4DBF // Han extension A
        | 0x4E00..=0x9FFF // Han
        | 0xF900..=0xFAFF // compatibility ideographs
        | 0xFE30..=0xFE4F // compatibility forms
        | 0xFF00..=0xFF65 // fullwidth forms and punctuation
        | 0xFFE0..=0xFFE6 // fullwidth signs
        | 0x20000..=0x3FFFF // Han extensions B and later, and the compatibility supplement
    ) {
        return Some("han");
    }
    None
}

/// The system font files to try for a CJK character, in order (Windows): the interface language's
/// own face first, then the fallback table's. It is the same one source the loaded script fonts
/// use ([`SCRIPT_FONTS`]), so a Chinese, Japanese or Korean interface keeps its language's glyph
/// variants. Empty for a character [`cjk_script`] leaves to egui.
#[cfg(windows)]
pub(crate) fn cjk_font_files(c: char) -> Vec<&'static str> {
    cjk_font_files_for(c, crate::i18n::code())
}

/// [`cjk_font_files`] for an interface language `code` (the production path and its tests).
#[cfg(windows)]
pub(crate) fn cjk_font_files_for(c: char, code: &str) -> Vec<&'static str> {
    let Some(script) = cjk_script(c) else {
        return Vec::new();
    };
    // Chinese and Japanese characters: the one Han list, in the language's order (#1605).
    if script == "han" {
        return han_files(code);
    }
    let own = script;
    let mut files = Vec::new();
    let mut add = |list: &'static [&'static str]| {
        for file in list {
            if !files.contains(file) {
                files.push(*file);
            }
        }
    };
    if let Some((_, own_files, _)) = SCRIPT_FONTS.iter().find(|(key, ..)| *key == own) {
        add(own_files);
    }
    if let Some(entry) = fallback_entry(FALLBACK_FONTS, c) {
        add(entry.files);
    }
    files
}

/// The bytes of the font for `c`, loaded once per font, shared by every family that asks.
/// Windows and macOS: the first file of the character's list (in the fonts folder `dir`, in the
/// interface language `code`'s order: [`fallback_files`]) whose face has it, named
/// `fallback:<file>`; so a character one file lacks comes from the next (#1605: MS Gothic, the
/// last of the Han list, was the one font of the Han characters on a Windows without the first
/// ones, and lacks the Simplified-only ones). None when no file of the list has it: the system's
/// own choice is asked next ([`SystemFallback`]).
fn fallback_font(c: char, dir: &std::path::Path, code: &str) -> Option<(String, egui::FontData)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LOADED: Mutex<Option<HashMap<std::path::PathBuf, Option<egui::FontData>>>> =
        Mutex::new(None);
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let loaded = loaded.get_or_insert_with(HashMap::new);
    let mut load = |path: &std::path::Path| {
        loaded
            .entry(path.to_owned())
            .or_insert_with(|| {
                let bytes = read_font(path)?;
                let data = font_data(bytes).tweak(on_demand_tweak());
                tracing::info!(
                    "fonts: {} face {} loaded on demand (U+{:04X})",
                    path.display(),
                    data.index,
                    c as u32
                );
                Some(data)
            })
            .clone()
    };

    #[cfg(any(windows, target_os = "macos"))]
    {
        let font = fallback_entry(FALLBACK_FONTS, c)?;
        let text = c.to_string();
        fallback_files(font, code).into_iter().find_map(|file| {
            let data = load(&font_path(dir, file))?;
            covers(data.bytes(), data.index, &text).then(|| (format!("fallback:{file}"), data))
        })
    }
    // Linux: one font per Unicode block, the first file fontconfig names for its first
    // character asked (a script's letters don't each need a font of their own), kept under the
    // block's name.
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        static BLOCKS: Mutex<Option<HashMap<u32, Option<std::path::PathBuf>>>> = Mutex::new(None);
        let _ = (dir, code);
        let block = (c as u32) >> 8;
        let mut blocks = BLOCKS.lock().unwrap_or_else(|e| e.into_inner());
        let file = blocks
            .get_or_insert_with(HashMap::new)
            .entry(block)
            .or_insert_with(|| {
                fc_lookup(&format!(":charset={:x}", c as u32))
                    .into_iter()
                    .find(|f| load(f).is_some())
            })
            .clone()?;
        Some((format!("fallback:{block:x}"), load(&file)?))
    }
}

/// How a font found on demand draws: on the baseline of the family's first font (as the script
/// fonts loaded up front, [`fonts`]), and on Windows hinted as [`sharp_text`] hints those.
fn on_demand_tweak() -> egui::FontTweak {
    let mut tweak = egui::FontTweak::default();
    tweak.set_baseline_fallback(true);
    #[cfg(windows)]
    {
        tweak.hinting_target =
            egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
                light: false,
                symmetric_rendering: false,
                preserve_linear_metrics: false,
            });
        tweak.subpixel_binning = Some(false);
    }
    tweak
}

/// `fc-match` for one pattern, asked right now (Linux, a character no loaded font has): the font
/// files in fontconfig's order ([`fc_match`]).
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_lookup(pattern: &str) -> Vec<std::path::PathBuf> {
    match fc_match(pattern).output() {
        Ok(out) if out.status.success() => fc_candidates(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// `fc-match` for `pattern`: every font that matches it, best first (`-s`), one file a line. Its
/// best answer alone can be a font egui can't draw with, a bitmap font fontconfig prefers for a
/// language (#931: a gzipped PCF font for Japanese). So the fonts that scale come first
/// (`scalable=true` outranks the family and the language in fontconfig's order: Noto Sans CJK
/// before a preferred bitmap font), and MDM takes the first it can read ([`pick_font`]: a
/// scalable Type 1 font is no more readable than a bitmap one).
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_match(pattern: &str) -> std::process::Command {
    let mut command = std::process::Command::new("fc-match");
    command
        .args(["-s", "-f", "%{file}\\n"])
        .arg(format!("{pattern}:scalable=true"));
    command
}

/// The font files in `fc-match`'s answer ([`fc_match`]), in its order, each once.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn fc_candidates(out: &str) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let file = std::path::PathBuf::from(line);
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// The first of `candidates` (fontconfig's files, best first) egui can draw with: its file and
/// bytes. `looked` holds the files looked at already and whether each was one: a file loaded
/// already (several languages share a font: Devanagari, the CJK collection) gives nothing new,
/// and one that wasn't a font egui can draw with is not read again.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn pick_font(
    candidates: &[std::path::PathBuf],
    looked: &mut std::collections::HashMap<std::path::PathBuf, bool>,
) -> Option<(std::path::PathBuf, Vec<u8>)> {
    for file in candidates {
        match looked.get(file) {
            Some(true) => return None,
            Some(false) => continue,
            None => {}
        }
        let bytes = read_font(file);
        looked.insert(file.clone(), bytes.is_some());
        if let Some(bytes) = bytes {
            return Some((file.clone(), bytes));
        }
    }
    None
}

impl egui::FontProvider for ScriptFallback {
    fn font_for(&self, request: &egui::FallbackRequest<'_>) -> Option<egui::FontInsert> {
        let c = request.cluster.chars().next()?;
        #[cfg(any(windows, target_os = "macos"))]
        let system = font_dir();
        #[cfg(not(any(windows, target_os = "macos")))]
        let system = std::path::PathBuf::new();
        let dir = self.dir.as_deref().unwrap_or(&system);
        let code = self.code.as_deref().unwrap_or_else(|| crate::i18n::code());
        let (key, data) = fallback_font(c, dir, code)?;
        Some(egui::FontInsert::new(&key, data, Vec::new()))
    }
}

/// Windows and macOS: the system's own font for a letter neither the interface's fonts nor
/// [`ScriptFallback`]'s lists have (#1125): a script the lists don't name (Syriac, Thaana,
/// Mongolian, N'Ko, Tifinagh...), or a letter the font they name lacks (Myanmar isn't in
/// macOS's Arial Unicode). The system chooses as it does for its own apps
/// (DirectWrite's font fallback, CoreText's cascade), among every font installed, through
/// fontique; the font's file is mapped, not read into memory. Asked last, so the lists keep
/// their faces (a Chinese interface's Chinese glyphs). Made the first time a letter needs it:
/// listing the system's fonts takes a moment, which a start whose text the lists cover never
/// spends. Linux has no need: fontconfig is asked for any letter ([`fallback_font`]).
#[cfg(any(windows, target_os = "macos"))]
#[derive(Default)]
struct SystemFallback(std::sync::OnceLock<egui_system_font_provider::SystemFontProvider>);

#[cfg(any(windows, target_os = "macos"))]
impl egui::FontProvider for SystemFallback {
    fn font_for(&self, request: &egui::FallbackRequest<'_>) -> Option<egui::FontInsert> {
        let c = request.cluster.chars().next()?;
        let started = std::time::Instant::now();
        let system = self.0.get_or_init(|| {
            // The interface language picks among a script's fonts (Chinese or Japanese faces).
            egui_system_font_provider::SystemFontProvider::new()
                .with_locale(Some(crate::i18n::code()))
        });
        let found = system.font_for(request);
        let ms = started.elapsed().as_millis();
        match &found {
            Some(font) => tracing::info!(
                "fonts: U+{:04X} from the system's {} ({ms} ms)",
                c as u32,
                font.name
            ),
            None => tracing::info!("fonts: no font on this system has U+{:04X}", c as u32),
        }
        let mut font = found?;
        font.data.tweak = on_demand_tweak();
        Some(font)
    }
}

/// The fonts for text outside the interface language, in the order they are asked: MDM's lists
/// ([`ScriptFallback`]), then the system's choice ([`SystemFallback`]).
fn on_demand_fonts() -> Vec<std::sync::Arc<dyn egui::FontProvider>> {
    vec![
        std::sync::Arc::new(ScriptFallback::default()),
        #[cfg(any(windows, target_os = "macos"))]
        std::sync::Arc::new(SystemFallback::default()),
    ]
}

/// Tell the text system where the scripts outside the interface language come from, once.
fn add_script_fallback(ctx: &egui::Context) {
    static ADDED: std::sync::Once = std::sync::Once::new();
    ADDED.call_once(|| {
        for provider in on_demand_fonts() {
            ctx.add_font_provider(provider);
        }
    });
}

/// The interface font: Segoe UI as IDM has it on Windows, the system's own elsewhere.
fn ui_font() -> Option<Vec<u8>> {
    #[cfg(windows)]
    return read_font(&font_dir().join("segoeui.ttf"));
    #[cfg(target_os = "macos")]
    return ["SFNS.ttf", "Helvetica.ttc"]
        .iter()
        .find_map(|f| read_font(&font_dir().join(f)));
    #[cfg(not(any(windows, target_os = "macos")))]
    return fc_font("sans-serif").map(|(_, bytes)| bytes);
}

/// The script fonts for `code` (every language's with `all`): (key, bytes), in fallback order.
fn script_fonts(code: &str, all: bool) -> Vec<(String, Vec<u8>)> {
    #[cfg(any(windows, target_os = "macos"))]
    return script_fonts_in(&font_dir(), code, all);
    #[cfg(not(any(windows, target_os = "macos")))]
    return script_fonts_in(std::path::Path::new(""), code, all);
}

/// [`script_fonts`] from the fonts folder `dir` (Windows and macOS; Linux asks fontconfig).
fn script_fonts_in(dir: &std::path::Path, code: &str, all: bool) -> Vec<(String, Vec<u8>)> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        SCRIPT_FONTS
            .iter()
            .filter(|(_, _, langs)| all || langs.contains(&code))
            // The first file there is used (older systems name some differently); a .ttc's
            // regular face is the one drawn with ([`text_face`], #1483). A Chinese one must have
            // its script's own characters (#1605: [`script_probe`]), and the log says which.
            .filter_map(|(key, files, _)| {
                #[cfg(windows)]
                if let Some((script, probe)) = script_probe(key) {
                    let found = files.iter().find_map(|f| {
                        let bytes = read_font(&font_path(dir, f))?;
                        let face = text_face(&bytes);
                        covers(&bytes, face, probe).then_some((*f, face, bytes))
                    });
                    return match found {
                        Some((file, face, bytes)) => {
                            tracing::info!("fonts: {script} from {file} face {face}");
                            Some(((*key).to_owned(), bytes))
                        }
                        None => {
                            tracing::warn!(
                                "fonts: no {script} font with its characters in {} (looked for {}): its text comes from other fonts, or shows as boxes",
                                dir.display(),
                                files.join(", ")
                            );
                            None
                        }
                    };
                }
                let bytes = files.iter().find_map(|f| read_font(&font_path(dir, f)))?;
                Some(((*key).to_owned(), bytes))
            })
            .collect()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = dir;
        // Several languages share a font (Devanagari, the CJK collection): loaded once.
        let mut looked = std::collections::HashMap::new();
        SCRIPT_LANGS
            .iter()
            .filter(|(ours, _)| all || *ours == code)
            .filter_map(|(_, fc)| pick_font(fc_files().get(&format!(":lang={fc}"))?, &mut looked))
            .map(|(f, bytes)| (format!("fc:{}", f.display()), bytes))
            .collect()
    }
}

/// The font fontconfig picks for `pattern` (one of [`fc_files`]'s): the first of its answers egui
/// can draw with, its file and bytes.
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_font(pattern: &str) -> Option<(std::path::PathBuf, Vec<u8>)> {
    pick_font(fc_files().get(pattern)?, &mut Default::default())
}

/// Every font file the windows may need, as fontconfig picks it: the interface font
/// (`sans-serif`) and each script's (`:lang=…`). Each answer is an `fc-match` program; asked one
/// after the other on the window's thread, the thirty of them held up MDM's first frame for 2 s
/// on an idle computer and past the freeze report's 8 s on a busy one (#164). So they are asked
/// once, all together (a third of the time), and [`prepare_fonts`] starts that while the window
/// opens; the fonts only wait for what is still running. A font installed while MDM runs is
/// used from its next start.
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_files() -> &'static std::collections::HashMap<String, Vec<std::path::PathBuf>> {
    use std::process::Stdio;
    static FILES: std::sync::OnceLock<std::collections::HashMap<String, Vec<std::path::PathBuf>>> =
        std::sync::OnceLock::new();
    FILES.get_or_init(|| {
        let running: Vec<_> = ["sans-serif", SYMBOL_PATTERN]
            .into_iter()
            .map(str::to_owned)
            .chain(SCRIPT_LANGS.iter().map(|(_, fc)| format!(":lang={fc}")))
            .filter_map(|pattern| {
                let child = fc_match(&pattern)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()?;
                Some((pattern, child))
            })
            .collect();
        running
            .into_iter()
            .filter_map(|(pattern, child)| {
                let out = child.wait_with_output().ok()?;
                let files = fc_candidates(&String::from_utf8_lossy(&out.stdout));
                (out.status.success() && !files.is_empty()).then_some((pattern, files))
            })
            .collect()
    })
}

/// Start looking up the system's fonts while the window opens (Linux: [`fc_files`]); Windows and
/// macOS read theirs from known files, quickly, when the fonts load.
pub fn prepare_fonts() {
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::thread::Builder::new().name("fonts".into()).spawn(|| {
        fc_files();
    });
}

/// The script fonts loaded now: the interface language's, or all of them.
static LOADED: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

/// Load the fonts the interface language needs (after it changed).
pub fn language_fonts(ctx: &egui::Context) {
    let all = LOADED.lock().unwrap().as_ref().is_some_and(|l| l.1);
    load_fonts(ctx, all);
}

/// Load every script font, for a list of all languages by their own names, until
/// [`interface_fonts`] lets them go.
pub fn all_scripts(ctx: &egui::Context) {
    load_fonts(ctx, true);
}

/// Back to the interface language's script fonts only, once no list of every language shows
/// (#72: CJK fonts alone are 13–20 MB each; the language picker had kept them all loaded).
pub fn interface_fonts(ctx: &egui::Context) {
    load_fonts(ctx, false);
}

fn load_fonts(ctx: &egui::Context, all: bool) {
    #[cfg(windows)]
    classic_text(ctx);
    let code = crate::i18n::code().to_owned();
    let mut loaded = LOADED.lock().unwrap();
    // Only the script fonts depend on the language: rebuild when that set changes.
    let needs = |code: &str| -> Vec<&str> {
        #[cfg(any(windows, target_os = "macos"))]
        return SCRIPT_FONTS
            .iter()
            .filter(|(_, _, langs)| all || langs.contains(&code))
            .map(|(key, ..)| *key)
            .collect();
        #[cfg(not(any(windows, target_os = "macos")))]
        return SCRIPT_LANGS
            .iter()
            .filter(|(ours, _)| all || *ours == code)
            .map(|(ours, _)| *ours)
            .collect();
    };
    if let Some((old, old_all)) = loaded.as_ref()
        && *old_all == all
        && needs(old) == needs(&code)
    {
        *loaded = Some((code, all));
        return;
    }
    fonts(ctx, &code, all);
    *loaded = Some((code, all));
}

/// IDM draws with Segoe UI 9 pt (12 px at 100 %): the system's own copy, nothing bundled
/// (elsewhere the system's interface font, [`ui_font`]; else egui's default font). Every family
/// falls back to it (Tahoma has no Armenian or Georgian), then to the script fonts for `code`
/// (all with `all`).
fn fonts(ctx: &egui::Context, code: &str, all: bool) {
    let mut defs = egui::FontDefinitions::default();
    let ui = ui_font();
    if let Some(bytes) = &ui {
        defs.font_data.insert(
            "segoe-ui".into(),
            std::sync::Arc::new(font_data(bytes.clone())),
        );
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families
                .entry(fam)
                .or_default()
                .insert(0, "segoe-ui".into());
        }
    }
    #[cfg(windows)]
    crate::winui::install_fonts(&mut defs, Some(&font_dir()));
    #[cfg(not(windows))]
    crate::winui::install_fonts(&mut defs, None);
    // A fallback's glyphs sit on the baseline of the family's first font (Japanese next to
    // Latin digits, Georgian in a Tahoma line), not centred in its row.
    let on_baseline = |bytes: Vec<u8>| {
        let mut tweak = egui::FontTweak::default();
        tweak.set_baseline_fallback(true);
        std::sync::Arc::new(font_data(bytes).tweak(tweak))
    };
    let mut fallbacks: Vec<String> = Vec::new();
    if let Some(bytes) = ui {
        defs.font_data
            .insert("segoe-ui-fallback".into(), on_baseline(bytes));
        fallbacks.push("segoe-ui-fallback".into());
    }
    for (key, bytes) in script_fonts(code, all) {
        defs.font_data.insert(key.clone(), on_baseline(bytes));
        fallbacks.push(key);
    }
    for (k, bytes) in symbol_fonts().into_iter().enumerate() {
        let key = format!("symbols-{k}");
        defs.font_data.insert(key.clone(), on_baseline(bytes));
        fallbacks.push(key);
    }
    for list in defs.families.values_mut() {
        for f in &fallbacks {
            // Segoe UI already leads the two built-in families (as itself, not a fallback).
            if f == "segoe-ui-fallback" && list.first().is_some_and(|l| l == "segoe-ui") {
                continue;
            }
            if !list.contains(f) {
                list.push(f.clone());
            }
        }
    }
    #[cfg(windows)]
    sharp_text(&mut defs);
    only_loadable_fonts(&mut defs);
    ctx.set_fonts(defs);
    text_styles(ctx);
}

/// Latin, Greek and Cyrillic text drawn the way Windows' classic controls draw it (#262, #323,
/// #334), by DirectWrite ([`crate::win32::text_rasterizer`]): the fonts' shapes and, with them,
/// whole-pixel widths, so a letter's width and the place it is drawn for finally agree. That
/// disagreement is what glued some letters together and pushed others apart. Set again when the
/// interface's direction changes: a right-to-left line needs its brackets mirrored by the shaper.
#[cfg(windows)]
fn classic_text(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static DRAWN: AtomicU64 = AtomicU64::new(u64::MAX);
    let rtl = crate::i18n::rtl();
    let smooth = SMOOTH_TEXT.load(Ordering::Relaxed);
    // A CJK face depends on the interface language (its glyph variants), and the glyph atlas
    // keeps what was rasterized: a change of CJK language installs the rasterizer again.
    let lang = match crate::i18n::code() {
        "zh-hans" => 1,
        "zh-hant" => 2,
        "ja" => 3,
        "ko" => 4,
        _ => 0,
    };
    let now = u64::from(rtl) | (u64::from(smooth) << 1) | (lang << 2);
    if DRAWN.swap(now, Ordering::Relaxed) != now {
        ctx.set_glyph_rasterizers(
            crate::win32::text_rasterizer(font_dir(), rtl, smooth)
                .into_iter()
                .collect(),
        );
    }
}

/// View → Text: smooth (DirectWrite's natural look) or classic (hinted whole-pixel text, as
/// Windows' old controls, the default). Only Windows has the choice: elsewhere egui draws the text.
static SMOOTH_TEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The setting's value, before the first frame (it is read at start).
pub fn init_smooth_text(on: bool) {
    SMOOTH_TEXT.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Draw the text smooth (`true`) or classic from now on, in every window. Windows only: View →
/// Text exists only there, where DirectWrite draws the text; elsewhere egui draws it.
#[cfg(windows)]
pub fn set_smooth_text(ctx: &egui::Context, on: bool) {
    init_smooth_text(on);
    classic_text(ctx);
}

/// The text [`classic_text`] leaves to egui (Arabic, Hebrew, Indic, Thai, CJK; marks clustered
/// with their letter), fitted to whole pixels (#262) with hinting for egui's anti-aliased output.
/// Mono hinting targets black-and-white bitmaps: at small sizes it can distort curves and make
/// diagonals jagged. Smooth hinting keeps those outlines suited to coverage rendering, while
/// allowing horizontal grid-fitting so upright strokes stay sharp. Keep whole-pixel glyph origins
/// and the shaper's advances, so this changes neither layout nor the interface size. Linux and
/// macOS keep egui's defaults.
#[cfg(windows)]
fn sharp_text(defs: &mut egui::FontDefinitions) {
    for data in defs.font_data.values_mut() {
        let data = std::sync::Arc::make_mut(data);
        data.tweak.hinting_target =
            egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
                light: false,
                symmetric_rendering: false,
                preserve_linear_metrics: false,
            });
        data.tweak.subpixel_binning = Some(false);
    }
}

/// egui's text sizes from the theme (tooltips, multi-line boxes).
fn text_styles(ctx: &egui::Context) {
    let m = metrics();
    ctx.all_styles_mut(|s| {
        use egui::{FontId, TextStyle};
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::proportional(font_size(m.font_small)),
            ),
            (
                TextStyle::Body,
                FontId::proportional(font_size(m.font_body)),
            ),
            (
                TextStyle::Button,
                FontId::proportional(font_size(m.font_body)),
            ),
            (
                TextStyle::Monospace,
                FontId::monospace(font_size(m.font_body)),
            ),
            (
                TextStyle::Heading,
                FontId::proportional(font_size(m.font_heading)),
            ),
        ]
        .into();
    });
}

/// The size a role is drawn at: in a Chinese, Japanese or Korean interface, no smaller than the
/// theme's `font.cjk` (their glyphs are denser than Latin's at the same size, #837). Other
/// languages keep the role's size exactly.
pub fn font_size(size: f32) -> f32 {
    font_size_for(crate::i18n::code(), size, metrics().font_cjk)
}

/// [`font_size`] for an interface language `code` and a theme's CJK floor (also the production
/// path, so the test covers what the UI calls).
fn font_size_for(code: &str, size: f32, cjk: f32) -> f32 {
    if crate::i18n::cjk_code(code) {
        size.max(cjk)
    } else {
        size
    }
}

pub fn apply(ctx: &egui::Context) {
    add_script_fallback(ctx);
    load_fonts(ctx, false);
    // A press held for a while is still a click when let go over the control, as in Windows
    // (egui gives it up after 0.8 s).
    ctx.options_mut(|o| {
        o.input_options.max_click_duration = f64::INFINITY;
        // Ctrl + / Ctrl − / Ctrl 0 step through View → Interface size and are saved (scale.rs),
        // not egui's own 10 % steps.
        o.zoom_with_keyboard = false;
        // Every window's theme (its caption) is MDM's to give: the main window's in
        // `apply_mode`, each dialog's in `dialogs::keep_theme`, the caption the theme's surface.
        // egui's own would set the mode's instead, behind them, and the caption would have to
        // be checked with DWM again and again (#633).
        o.sync_window_theme = false;
    });
    egui_extras::install_image_loaders(ctx);
    apply_theme(ctx);
}

/// egui's own visuals from the active theme: each variant into its egui theme (the mode
/// switches between them), so tooltips and a multi-line box's scroll bar follow the theme too.
pub fn apply_theme(ctx: &egui::Context) {
    let theme = current();
    let m = &theme.metrics;
    let radius = |r: f32| CornerRadius::same(r.round().clamp(0.0, 255.0) as u8);
    let visuals = |base: Visuals, c: &Palette| {
        let mut v = base;
        v.panel_fill = c.egui_panel;
        v.window_fill = c.egui_window;
        v.extreme_bg_color = c.egui_extreme;
        v.faint_bg_color = c.egui_faint;
        v.override_text_color = Some(c.egui_text);
        v.selection.bg_fill = c.selection;
        v.selection.stroke = Stroke::new(1.0, c.egui_selection_text);
        // Square-ish Windows controls rather than egui's rounded ones.
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius(m.widget_radius);
        }
        v.widgets.inactive.bg_fill = c.egui_widget;
        v.widgets.inactive.weak_bg_fill = c.egui_widget;
        // Transparent: egui's own (light widgets have none).
        if c.egui_widget_border != egui::Color32::TRANSPARENT {
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, c.egui_widget_border);
        }
        v.window_corner_radius = CornerRadius::same(0);
        // Tooltips as Windows 11's: rounded corners, a hairline border, roomy padding.
        v.menu_corner_radius = radius(m.popup_radius);
        v.window_stroke = Stroke::new(1.0, c.egui_window_border);
        v
    };
    ctx.set_visuals_of(egui::Theme::Dark, visuals(Visuals::dark(), &theme.dark));
    ctx.set_visuals_of(egui::Theme::Light, visuals(Visuals::light(), &theme.light));
    let margin = |v: f32| v.round().clamp(0.0, 127.0) as i8;
    ctx.all_styles_mut(|s| {
        s.spacing.tooltip_width = m.tooltip_width;
        s.spacing.menu_margin = egui::Margin {
            left: margin(m.tooltip_margin_x),
            right: margin(m.tooltip_margin_x),
            top: margin(m.tooltip_margin_top),
            bottom: margin(m.tooltip_margin_bottom),
        };
    });
    text_styles(ctx);
}

#[cfg(test)]
mod mode_tests {
    use super::{font_size_for, wanted};

    /// #60: Always dark wins whatever the system says; otherwise Dark Mode support follows it.
    #[test]
    fn always_dark_wins_else_the_system_decides() {
        for system in [false, true] {
            for support in [false, true] {
                assert!(wanted(support, true, system), "always dark");
            }
            assert_eq!(wanted(true, false, system), system, "following the system");
            assert!(!wanted(false, false, system), "support off: light");
        }
    }

    /// #837: Chinese, Japanese and Korean text keeps the theme's floor; English and the other
    /// languages keep their measured size. A role already larger keeps it.
    #[test]
    fn cjk_text_keeps_the_themes_floor() {
        assert_eq!(crate::palette::Metrics::CLASSIC.font_cjk, 12.0);
        for code in ["zh-hans", "zh-hant", "ja", "ko"] {
            assert_eq!(font_size_for(code, 11.0, 12.0), 12.0, "{code}");
            assert_eq!(font_size_for(code, 13.0, 12.0), 13.0, "{code}");
        }
        for code in ["en", "de", "ar", "ru", "th"] {
            assert_eq!(font_size_for(code, 11.0, 12.0), 11.0, "{code}");
        }
        // A theme that asks for a larger floor gets it; one that asks for less than the role
        // doesn't shrink the role.
        assert_eq!(font_size_for("zh-hans", 11.0, 14.0), 14.0);
        assert_eq!(font_size_for("zh-hans", 16.0, 14.0), 16.0);
    }
}

/// #931: a font the system names that egui can't read (fontconfig answered a gzipped PCF bitmap
/// font for Japanese) is passed over for the next one, never given to egui, which stops on it.
#[cfg(test)]
mod font_file_tests {
    use super::{fc_candidates, loadable_font, pick_font, read_font};
    use std::collections::HashMap;

    /// A real TrueType font, and another: egui's own (Ubuntu Light, Hack).
    fn real_ttf(name: &str) -> Vec<u8> {
        egui::FontDefinitions::default().font_data[name]
            .bytes()
            .to_vec()
    }

    #[test]
    fn only_fonts_egui_can_read_are_taken() {
        let ttf = real_ttf("Ubuntu-Light");
        assert!(loadable_font(&ttf, 0), "a real TrueType font");
        assert!(loadable_font(&real_ttf("Hack"), 0), "another");

        // The start of 12x13ja.pcf.gz: gzip's magic, read as the sfnt version 0x1F8B0800.
        let mut pcf_gz = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x02, 0x03];
        pcf_gz.extend((0..4000u32).map(|i| (i * 31 % 251) as u8));
        // An uncompressed PCF ("\x01fcp"), a BDF, a Type 1 (.pfb) font.
        let mut pcf = b"\x01fcp".to_vec();
        pcf.extend([8, 0, 0, 0].iter().chain(&[0u8; 200]));
        let bdf =
            b"STARTFONT 2.1\nFONT -misc-fixed-medium-r-normal--13-120-75-75-c-120-iso10646-1\n";
        let mut pfb = vec![0x80, 0x01, 0x10, 0x00, 0x00, 0x00];
        pfb.extend(b"%!PS-AdobeFont-1.0: Test 001.000\n");
        let random = random_bytes();
        for (what, bytes) in [
            ("a gzipped PCF font", pcf_gz.as_slice()),
            ("a PCF font", &pcf),
            ("a BDF font", bdf),
            ("a Type 1 font", &pfb),
            ("random bytes", &random),
            ("an empty file", &[]),
            ("a TrueType font cut in half", &ttf[..ttf.len() / 2]),
            (
                "a TrueType font cut after its table list",
                &ttf[..ttf.len().min(400)],
            ),
        ] {
            assert!(!loadable_font(bytes, 0), "{what} was taken");
        }
    }

    /// fontconfig's answer, best first: its first font egui can read is the one taken. A font
    /// loaded already for another language gives nothing new; a file refused once isn't read again.
    #[test]
    fn the_first_font_egui_can_read_in_fontconfigs_answer_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let pcf = file("12x13ja.pcf.gz", &[0x1f, 0x8b, 0x08, 0x00, 1, 2, 3, 4]);
        let bdf = file("fixed.bdf", b"STARTFONT 2.1\n");
        let ubuntu = file("Ubuntu-Light.ttf", &real_ttf("Ubuntu-Light"));
        let hack = file("Hack-Regular.ttf", &real_ttf("Hack"));
        let gone = dir.path().join("gone.ttf");
        let answer = [&pcf, &bdf, &pcf, &ubuntu, &hack]
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect::<String>();

        // As fc-match -s prints it: one file a line (blank lines and repeats ignored).
        let candidates = fc_candidates(&format!("\n{answer}\n"));
        assert_eq!(
            candidates,
            [pcf.clone(), bdf.clone(), ubuntu.clone(), hack.clone()]
        );

        let mut looked = HashMap::new();
        let (picked, bytes) = pick_font(&candidates, &mut looked).expect("a font");
        assert_eq!(picked, ubuntu);
        assert_eq!(bytes, real_ttf("Ubuntu-Light"));
        assert_eq!(looked.get(&pcf), Some(&false));
        assert_eq!(looked.get(&bdf), Some(&false));
        assert!(
            !looked.contains_key(&hack),
            "read no further than the first"
        );

        // Another language whose best answer is the same font: loaded already, nothing new.
        assert!(pick_font(&[pcf.clone(), ubuntu.clone()], &mut looked).is_none());
        // A refused file and a missing one are passed over, and the next font is taken.
        let (picked, _) = pick_font(&[pcf.clone(), gone, hack.clone()], &mut looked).unwrap();
        assert_eq!(picked, hack);
        // Nothing egui can read: nothing.
        assert!(pick_font(std::slice::from_ref(&bdf), &mut HashMap::new()).is_none());
        assert!(read_font(&bdf).is_none());
        assert!(read_font(&ubuntu).is_some());
    }

    /// Random bytes (a fixed xorshift, so the tests are the same every run).
    fn random_bytes() -> Vec<u8> {
        let mut x = 0x9e37_79b9u32;
        (0..8192)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    /// The start of a BDF bitmap font: read as an sfnt version, "STAR" (0x53544152), #1412's.
    const BDF: &[u8] = b"STARTFONT 2.1\nFONT -misc-fixed-medium-r-normal--13-120-75-75-c-120-iso10646-1\nSIZE 13 75 75\n";

    /// #1412: fontconfig's answer for the symbols, a BDF font first (as on CachyOS), then an
    /// empty file and random bytes named as fonts: only the TrueType font after them is taken.
    #[test]
    fn a_bdf_an_empty_and_a_random_file_are_passed_over_for_the_truetype_font() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let candidates = [
            file("symbols.bdf", BDF),
            file("empty.ttf", &[]),
            file("random.otf", &random_bytes()),
            file("Ubuntu-Light.ttf", &real_ttf("Ubuntu-Light")),
        ];
        let mut looked = HashMap::new();
        let (picked, bytes) = pick_font(&candidates, &mut looked).expect("a font");
        assert_eq!(picked, candidates[3]);
        assert_eq!(bytes, real_ttf("Ubuntu-Light"));
        for refused in &candidates[..3] {
            assert_eq!(looked.get(refused), Some(&false), "{}", refused.display());
            assert!(read_font(refused).is_none(), "{}", refused.display());
        }
    }

    /// #1412: the fonts egui gets are each one it can read, whatever path added them: a BDF font,
    /// an empty file, random bytes and a TrueType font named at a face it doesn't have are left
    /// out (of the fonts and of the families), the TrueType font stays, and egui lays text out.
    /// Without that last look egui stops on the first of them, as MDM 0.5.2 did as it started.
    #[test]
    fn a_font_egui_cant_read_never_reaches_it() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontData, FontFamily, FontId};
        use std::sync::Arc;

        let mut defs = egui::FontDefinitions::default();
        let mut missing_face = FontData::from_owned(real_ttf("Ubuntu-Light"));
        missing_face.index = 3;
        let added = [
            ("symbols-0", FontData::from_owned(BDF.to_vec())),
            ("empty", FontData::from_owned(Vec::new())),
            ("random", FontData::from_owned(random_bytes())),
            ("missing-face", missing_face),
            ("valid", FontData::from_owned(real_ttf("Ubuntu-Light"))),
        ];
        for (name, data) in added {
            defs.font_data.insert(name.into(), Arc::new(data));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families
                    .entry(family)
                    .or_default()
                    .insert(0, name.into());
            }
        }

        // What egui does with them as given: it stops (the panic MDM 0.5.2 started with).
        let unchecked = defs.clone();
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut fonts = Fonts::new(TextOptions::default(), unchecked);
            fonts.has_glyphs(&FontId::proportional(13.0), "a")
        }));
        let why = stopped.expect_err("egui took a font it can't read");
        let why = why
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| why.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(why.contains("TTF/OTF font file"), "{why}");

        super::only_loadable_fonts(&mut defs);
        for name in ["symbols-0", "empty", "random", "missing-face"] {
            assert!(!defs.font_data.contains_key(name), "{name} reached egui");
            for list in defs.families.values() {
                assert!(!list.iter().any(|n| n == name), "a family names {name}");
            }
        }
        assert!(defs.font_data.contains_key("valid"));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(
                defs.families[&family].first().map(String::as_str),
                Some("valid")
            );
        }
        // egui's own fonts are all still there.
        for name in egui::FontDefinitions::default().font_data.keys() {
            assert!(defs.font_data.contains_key(name), "{name} was left out");
        }

        let mut fonts = Fonts::new(TextOptions::default(), defs);
        assert!(fonts.has_glyphs(&FontId::proportional(13.0), "Mister Download Manager"));
        assert!(fonts.has_glyphs(&FontId::monospace(13.0), "0123456789"));
    }

    /// A font file's tables (sfnt): tag and bytes, as its table list names them.
    fn sfnt_tables(font: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let be32 = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        (0..count)
            .map(|i| {
                let record = 12 + 16 * i;
                let (at, len) = (be32(record + 8), be32(record + 12));
                (
                    font[record..record + 4].try_into().unwrap(),
                    font[at..at + len].to_vec(),
                )
            })
            .collect()
    }

    /// A font collection (.ttc) of `faces`, in order, laid out as font tools write one: the
    /// header, each face's table list, then the tables.
    fn collection(faces: &[Vec<u8>]) -> Vec<u8> {
        let faces: Vec<_> = faces.iter().map(|f| (&f[..4], sfnt_tables(f))).collect();
        let header = 12 + 4 * faces.len();
        let lists: usize = faces.iter().map(|(_, t)| 12 + 16 * t.len()).sum();
        let mut out = b"ttcf".to_vec();
        out.extend(0x0001_0000u32.to_be_bytes());
        out.extend((faces.len() as u32).to_be_bytes());
        let mut list_at = header;
        for (_, tables) in &faces {
            out.extend((list_at as u32).to_be_bytes());
            list_at += 12 + 16 * tables.len();
        }
        let mut data = Vec::new();
        for (version, tables) in &faces {
            out.extend(*version);
            out.extend((tables.len() as u16).to_be_bytes());
            out.extend([0u8; 6]);
            for (tag, bytes) in tables {
                out.extend(tag);
                out.extend(0u32.to_be_bytes());
                out.extend(((header + lists + data.len()) as u32).to_be_bytes());
                out.extend((bytes.len() as u32).to_be_bytes());
                data.extend(bytes);
                data.resize(data.len().next_multiple_of(4), 0);
            }
        }
        assert_eq!(out.len(), header + lists);
        out.extend(data);
        out
    }

    /// `font` made into another face of its family: its weight (OS/2 usWeightClass), italic or
    /// upright (fsSelection), and, given one, the subfamily its name table says.
    fn face(font: &[u8], weight: u16, italic: bool, subfamily: Option<&str>) -> Vec<u8> {
        let mut font = font.to_vec();
        let be16 = |f: &[u8], at: usize| u16::from_be_bytes([f[at], f[at + 1]]) as usize;
        let table = |f: &[u8], tag: &[u8; 4]| {
            (0..be16(f, 4))
                .map(|i| 12 + 16 * i)
                .find(|&r| &f[r..r + 4] == tag)
                .map(|r| u32::from_be_bytes(f[r + 8..r + 12].try_into().unwrap()) as usize)
                .expect("the table")
        };
        let os2 = table(&font, b"OS/2");
        font[os2 + 4..os2 + 6].copy_from_slice(&weight.to_be_bytes());
        // fsSelection: italic (bit 0), oblique (bit 9), regular (bit 6).
        let mut selection = be16(&font, os2 + 62) as u16 & !0x0241;
        selection |= if italic { 0x0001 } else { 0x0040 };
        font[os2 + 62..os2 + 64].copy_from_slice(&selection.to_be_bytes());
        if let Some(subfamily) = subfamily {
            let name = table(&font, b"name");
            let strings = name + be16(&font, name + 4);
            for i in 0..be16(&font, name + 2) {
                let record = name + 6 + 12 * i;
                if ![2, 17].contains(&be16(&font, record + 6)) {
                    continue;
                }
                let text: Vec<u8> = if be16(&font, record) == 1 {
                    subfamily.bytes().collect()
                } else {
                    subfamily
                        .encode_utf16()
                        .flat_map(u16::to_be_bytes)
                        .collect()
                };
                assert!(text.len() <= be16(&font, record + 8), "fits in place");
                let at = strings + be16(&font, record + 10);
                font[at..at + text.len()].copy_from_slice(&text);
                font[record + 8..record + 10].copy_from_slice(&(text.len() as u16).to_be_bytes());
            }
        }
        font
    }

    /// #1483: a collection whose first face isn't its regular one (macOS's Songti.ttc starts with
    /// Songti SC Black) is drawn with its face closest to Regular: upright, normal weight, not
    /// named as a display face. A first face that reads as regular text (Yu Gothic Medium,
    /// Hiragino W3) stays, and a collection with no such face keeps its first.
    #[test]
    fn a_collection_is_drawn_with_its_regular_face() {
        use super::{font_data, font_face_index, text_face};
        let hack = real_ttf("Hack");
        let ubuntu = real_ttf("Ubuntu-Light");
        let w = |weight| face(&hack, weight, false, None);

        // As Songti.ttc: Black, Bold, Light, Regular (twice: SC and TC).
        let songti = collection(&[
            w(900),
            w(700),
            face(&ubuntu, 300, false, None),
            w(400),
            w(400),
        ]);
        assert_eq!(text_face(&songti), 3);
        // Italic, and a face whose name says Bold though its weight says 400, are passed over.
        let named = collection(&[
            w(900),
            face(&hack, 400, true, None),
            face(&hack, 400, false, Some("Bold")),
            face(&hack, 400, false, Some("Black")),
            w(400),
        ]);
        assert_eq!(text_face(&named), 4);
        // The closest to Regular; of two as close (Light 300, Medium 500), the first.
        assert_eq!(text_face(&collection(&[w(900), w(300), w(400)])), 2);
        assert_eq!(text_face(&collection(&[w(900), w(300), w(500)])), 1);
        // As YuGothM.ttc (Yu Gothic Medium, then Yu Gothic UI Regular) and Hiragino's W3, W6.
        assert_eq!(text_face(&collection(&[w(500), w(400)])), 0);
        assert_eq!(text_face(&collection(&[w(300), w(600)])), 0);
        // Bold faces only (msyhbd.ttc): the first.
        assert_eq!(text_face(&collection(&[w(700), w(900)])), 0);
        // One font, not a collection, and anything else: face 0.
        assert_eq!(text_face(&w(900)), 0);
        assert_eq!(text_face(&hack), 0);
        assert_eq!(text_face(BDF), 0);
        assert_eq!(text_face(&[]), 0);

        // Read from its file: the face egui is given, the face DirectWrite is told, and egui draws
        // with it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Songti.ttc");
        std::fs::write(&path, &songti).unwrap();
        let bytes = read_font(&path).expect("a collection egui can read");
        assert_eq!(font_face_index(&path), 3);
        let data = font_data(bytes);
        assert_eq!(data.index, 3);
        let mut defs = egui::FontDefinitions::default();
        defs.font_data
            .insert("songti".into(), std::sync::Arc::new(data));
        defs.families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "songti".into());
        super::only_loadable_fonts(&mut defs);
        assert!(defs.font_data.contains_key("songti"));
        let mut fonts = egui::epaint::text::Fonts::new(Default::default(), defs);
        assert!(fonts.has_glyphs(&egui::FontId::proportional(13.0), "Regular"));
    }

    /// The system's collections MDM names on Windows keep the face they had: each one's first
    /// face is its regular one already (Yu Gothic Medium is YuGothM.ttc's), or the file has no
    /// regular face (the Bold and Light ones). The faces are printed: the PR's proof (#1483).
    #[cfg(windows)]
    #[test]
    fn windows_collections_keep_their_first_face() {
        let dir = super::font_dir();
        for file in [
            "msyh.ttc",
            "msjh.ttc",
            "YuGothM.ttc",
            "meiryo.ttc",
            "msgothic.ttc",
            "Nirmala.ttc",
            "simsun.ttc",
            "YuGothR.ttc",
            "YuGothB.ttc",
            "msyhbd.ttc",
            "msyhl.ttc",
        ] {
            let Some(bytes) = read_font(&dir.join(file)) else {
                continue;
            };
            let face = super::text_face(&bytes);
            println!("{file}: face {face}");
            assert_eq!(face, 0, "{file}");
        }
    }
}

/// Every mark a window draws (punctuation, arrows, ✔, quotation marks…: what isn't a letter or a
/// digit) has a glyph in every language's font chain, so none comes out as an empty box (#185:
/// "✔ Connected" in the browser offer window; "→" on Linux).
#[cfg(test)]
mod glyph_tests {

    /// A fonts folder holding only these files of Windows' own (copied from C:\Windows\Fonts),
    /// or None where one of them isn't installed.
    #[cfg(windows)]
    fn fonts_folder_with(files: &[&str]) -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            let from = super::font_dir().join(file);
            if !from.is_file() {
                eprintln!("{} isn't installed here: not checked", from.display());
                return None;
            }
            std::fs::copy(&from, dir.path().join(file)).unwrap();
        }
        Some(dir)
    }

    /// #1605: a Windows whose Chinese fonts are SimSun alone (no Microsoft YaHei: Windows 7 has
    /// it as msyh.ttf, a stripped copy has none), beside MS Gothic. A Simplified Chinese
    /// interface draws its words with SimSun, the characters Chinese shares with Japanese (下)
    /// too: before, MS Gothic was the one font found for Han characters, which lacks the
    /// Simplified-only ones (载, 链, 浏, 览), and they were boxes where the system's own fallback
    /// had nothing better (Windows 7 has none). The system's fallback isn't asked here.
    #[cfg(windows)]
    #[test]
    fn simplified_chinese_without_yahei_is_drawn_with_simsun() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let Some(dir) = fonts_folder_with(&["msgothic.ttc", "simsun.ttc"]) else {
            return;
        };
        let words = "\u{4E0B}\u{8F7D}\u{94FE}\u{63A5}\u{6D4F}\u{89C8}\u{5668}"; // 下载链接浏览器
        let provider = super::ScriptFallback {
            dir: Some(dir.path().to_owned()),
            code: Some("zh-hans".into()),
        };
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(vec![std::sync::Arc::new(provider)]);
        let id = FontId::new(13.0, FontFamily::Proportional);
        assert!(fonts.has_glyphs(&id, "\u{4E0B}"), "下");
        let found = |fonts: &Fonts| -> Vec<String> {
            fonts
                .discovered_fonts()
                .iter()
                .map(|f| f.name.clone())
                .collect()
        };
        assert_eq!(found(&fonts), ["fallback:simsun.ttc"], "下 from SimSun");
        assert!(fonts.has_glyphs(&id, words), "{words}");
        assert!(fonts.has_glyphs(&id, super::SIMPLIFIED_ONLY));
        assert_eq!(
            found(&fonts),
            ["fallback:simsun.ttc"],
            "all of it from SimSun"
        );
        // Loaded up front for the interface too: SimSun, which has the Simplified-only ones.
        let up_front = super::script_fonts_in(dir.path(), "zh-hans", false);
        let keys: Vec<&str> = up_front.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["han-sc"]);
        let bytes = &up_front[0].1;
        assert!(super::covers(bytes, super::text_face(bytes), words));
        // Traditional Chinese has no font of its own here: none is loaded for it up front (MS
        // Gothic lacks its own characters), and its words come on demand from SimSun.
        assert!(super::script_fonts_in(dir.path(), "zh-hant", false).is_empty());
        // MS Gothic alone: still nothing Japanese passes for Chinese up front.
        let japanese_only = fonts_folder_with(&["msgothic.ttc"]).unwrap();
        assert!(super::script_fonts_in(japanese_only.path(), "zh-hans", false).is_empty());
    }

    /// #1605: for a Simplified Chinese interface, no Traditional Chinese or Japanese font comes
    /// before a Simplified one, wherever a Han character's font is looked for (on demand, and
    /// DirectWrite's classic text); a Traditional interface starts with its own, a Japanese one
    /// with its own, a Korean one with Malgun Gothic (which has Hanja).
    #[cfg(windows)]
    #[test]
    fn a_chinese_interface_tries_its_own_chinese_fonts_first() {
        let han = [
            '\u{4E0B}',  // 下, Chinese and Japanese alike
            '\u{8F7D}',  // 载, Simplified only
            '\u{3001}',  // 、 CJK punctuation
            '\u{3400}',  // extension A
            '\u{FF01}',  // fullwidth !
            '\u{20000}', // extension B
        ];
        for (code, own, first) in [
            ("zh-hans", super::HAN_SC, "msyh.ttc"),
            ("zh-hant", super::HAN_TC, "msjh.ttc"),
            ("ja", super::JAPANESE, "YuGothM.ttc"),
            ("ko", super::HANGUL, "malgun.ttf"),
        ] {
            for c in han {
                let entry = super::fallback_entry(super::FALLBACK_FONTS, c).unwrap();
                let lists = [
                    super::fallback_files(entry, code),
                    super::cjk_font_files_for(c, code),
                ];
                for files in lists {
                    assert_eq!(files[0], first, "{code} U+{:04X}: {files:?}", c as u32);
                    let others = files.iter().position(|f| !own.contains(f)).unwrap();
                    assert!(
                        files[others..].iter().all(|f| !own.contains(f)),
                        "{code} U+{:04X}: {files:?}",
                        c as u32
                    );
                    assert_eq!(others, own.len(), "{code}: {files:?}");
                }
            }
        }
        // Simplified Chinese, and every language that isn't Chinese, Japanese or Korean: every
        // Simplified file, then the Traditional ones, then the Japanese ones.
        for code in ["zh-hans", "en", "de", "ar"] {
            let files = super::han_files(code);
            let rank = |f: &&str| {
                if super::HAN_SC.contains(f) {
                    0
                } else if super::HAN_TC.contains(f) {
                    1
                } else {
                    2
                }
            };
            assert!(files.is_sorted_by_key(rank), "{code}: {files:?}");
            assert_eq!(files.len(), 11, "{code}: {files:?}");
        }
    }

    /// #405: a script the interface language doesn't use: Bangla in a file name, Japanese in a
    /// playlist, under an English interface. Before: nothing loaded it, so boxes.
    #[cfg(windows)]
    #[test]
    fn text_in_other_scripts_gets_its_font_on_demand() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(super::on_demand_fonts());
        let id = FontId::new(13.0, FontFamily::Proportional);
        for text in [
            "\u{09AC}\u{09BE}\u{0982}\u{09B2}\u{09BE}", // Bangla
            "\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{30BF}\u{30A4}\u{30C8}\u{30EB}", // Japanese
            "\u{D55C}\u{AD6D}\u{C5B4}",                 // Korean
            "\u{0E44}\u{0E17}\u{0E22}",                 // Thai
            "\u{4E2D}\u{6587}",                         // Chinese
        ] {
            assert!(fonts.has_glyphs(&id, text), "no font for {text}");
        }
    }

    /// #1125: file names in the scripts the report and the lists name, under an English
    /// interface, and in scripts MDM's lists don't name, which only the system's own fallback
    /// finds: before, those stayed boxes. egui's fonts alone have none of them (no Segoe UI here,
    /// so even Arabic and Hebrew must come from a fallback).
    /// (Linux asks fontconfig, whose answer depends on the fonts a distribution installs.)
    #[cfg(any(windows, target_os = "macos"))]
    const NAMES: [(&str, &str); 10] = [
        (
            "Arabic",
            "\u{062A}\u{0642}\u{0631}\u{064A}\u{0631} \u{0627}\u{0644}\u{0645}\u{0628}\u{064A}\u{0639}\u{0627}\u{062A}",
        ),
        (
            "Hebrew",
            "\u{05E1}\u{05D9}\u{05DB}\u{05D5}\u{05DD} \u{05D4}\u{05E4}\u{05D2}\u{05D9}\u{05E9}\u{05D4}",
        ),
        ("Chinese", "\u{5E74}\u{5EA6}\u{62A5}\u{544A}"),
        (
            "Hindi",
            "\u{0939}\u{093F}\u{0902}\u{0926}\u{0940} \u{0917}\u{093E}\u{0928}\u{0947}",
        ),
        ("Thaana", "\u{078B}\u{07A8}\u{0788}\u{07AC}\u{0780}\u{07A8}"),
        (
            "Mongolian",
            "\u{182E}\u{1823}\u{1829}\u{182D}\u{1823}\u{182F}",
        ),
        ("N'Ko", "\u{07D2}\u{07DE}\u{07CF}"),
        (
            "Tifinagh",
            "\u{2D5C}\u{2D49}\u{2D3C}\u{2D49}\u{2D4F}\u{2D30}\u{2D56}",
        ),
        (
            "Canadian syllabics",
            "\u{14C0}\u{1426}\u{1403}\u{152D}\u{140D}\u{140F}\u{1423}",
        ),
        ("Yi", "\u{A188}\u{A320}\u{A071}\u{A0B7}"),
    ];

    #[cfg(windows)]
    #[test]
    fn every_script_the_system_has_a_font_for_shows_its_letters() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(super::on_demand_fonts());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let id = FontId::new(13.0, family);
            for (script, text) in NAMES {
                assert!(fonts.has_glyphs(&id, text), "no font for {script}: {text}");
            }
        }
    }

    /// The fallback's choice (#1125): MDM's lists first (Nirmala UI for Hindi, the face the
    /// classic text and the interface languages that use the script have), the system only for
    /// what the lists don't name (Thaana), and the system's font never replaces a list's.
    #[cfg(windows)]
    #[test]
    fn the_lists_choose_first_and_the_system_after() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(super::on_demand_fonts());
        let id = FontId::new(13.0, FontFamily::Proportional);
        let found = |fonts: &Fonts| -> Vec<String> {
            fonts
                .discovered_fonts()
                .iter()
                .map(|f| f.name.clone())
                .collect()
        };
        assert!(fonts.has_glyphs(&id, NAMES[3].1), "Hindi");
        // Named by its file (Nirmala UI, a collection since Windows 10 1709).
        let names = found(&fonts);
        assert!(
            names.len() == 1 && names[0].starts_with("fallback:Nirmala."),
            "{names:?}"
        );
        assert!(fonts.has_glyphs(&id, NAMES[4].1), "Thaana");
        let names = found(&fonts);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[1].starts_with("system:"), "{names:?}");
        // Hindi again, and Bangla (the same list's font): nothing new is looked for.
        assert!(fonts.has_glyphs(&id, "\u{0915}\u{09AC}"));
        assert_eq!(found(&fonts).len(), 2);
    }

    /// macOS (#1125): what Arial Unicode, the list's font for its blocks, lacks comes from the
    /// system's cascade (Myanmar), as does a script no list names (Mongolian, Yi). Sinhala has a
    /// font for every letter too: from the list ([`sinhala_names_get_a_font_on_macos`]).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_finds_what_arial_unicode_lacks() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(super::on_demand_fonts());
        let id = FontId::new(13.0, FontFamily::Proportional);
        for (script, text) in [
            (
                "Myanmar",
                "\u{1019}\u{103C}\u{1014}\u{103A}\u{1019}\u{102C}",
            ),
            ("Sinhala", "\u{0DC3}\u{0DD2}\u{0D82}\u{0DC4}\u{0DBD}"),
            NAMES[0],
            NAMES[1],
            NAMES[2],
            NAMES[3],
        ] {
            // Which fonts were found says where a failure comes from: the list's
            // (`fallback:…`) or the system's (`system:<family>…`).
            assert!(
                fonts.has_glyphs(&id, text),
                "no font for {script}: {text}; found {:?}",
                font_names(&fonts)
            );
        }
    }

    /// The fonts found on demand so far, by name, in the order they were found.
    #[cfg(target_os = "macos")]
    fn font_names(fonts: &egui::epaint::text::Fonts) -> Vec<String> {
        fonts
            .discovered_fonts()
            .iter()
            .map(|f| f.name.clone())
            .collect()
    }

    /// Names in the Arabic script (a Persian file name, #533), Hebrew and Armenian, under an
    /// English interface on macOS: its system font has none of them, so the fallback must name a
    /// font for every letter. Before, macOS's list started at Devanagari, and they were boxes.
    /// Checked on every system (the table is data); the font itself in the macOS test below.
    const OTHER_SCRIPTS: [&str; 5] = [
        "\u{0641}\u{0627}\u{06CC}\u{0644} \u{067E}\u{0698}\u{0648}\u{0647}\u{0634}\u{06CC} \u{06F1}\u{06F4}\u{06F0}\u{06F3}", // Persian, with Persian digits
        "\u{06AF}\u{0632}\u{0627}\u{0631}\u{0634} \u{0686}\u{06A9}", // Persian
        "\u{0645}\u{0644}\u{0641}",                                  // Arabic
        "\u{05E7}\u{05D5}\u{05D1}\u{05E5}",                          // Hebrew
        "\u{0586}\u{0561}\u{0575}\u{056C}",                          // Armenian
    ];

    #[test]
    fn macos_names_a_font_for_the_arabic_script() {
        for text in OTHER_SCRIPTS {
            for c in text.chars().filter(|c| !c.is_whitespace()) {
                let font = super::fallback_entry(super::MACOS_FALLBACK_FONTS, c)
                    .unwrap_or_else(|| panic!("no macOS font for U+{:04X} in {text}", c as u32));
                // The file macOS ships (/System/Library/Fonts/Supplemental), which an Arabic or
                // Persian interface already loads.
                assert!(
                    font.files.contains(&"Supplemental/Arial Unicode.ttf"),
                    "U+{:04X}: {:?}",
                    c as u32,
                    font.files
                );
            }
        }
    }

    /// A Sinhala file name on macOS: the list names macOS's Sinhala font for every letter of the
    /// block, before Arial Unicode (which has none of them). Left to the system's cascade, they
    /// were boxes on a Mac with a downloaded font that has U+0D81 (`MACOS_FALLBACK_FONTS` says
    /// why). Checked on every system (the table is data); the font itself in the macOS test below.
    const SINHALA: &str = "\u{0DC3}\u{0DD2}\u{0D82}\u{0DC4}\u{0DBD}";

    #[test]
    fn macos_names_its_sinhala_font() {
        for c in ('\u{0D80}'..='\u{0DFF}').chain(SINHALA.chars()) {
            let font = super::fallback_entry(super::MACOS_FALLBACK_FONTS, c)
                .unwrap_or_else(|| panic!("no macOS font for U+{:04X}", c as u32));
            assert_eq!(font.key, "sinhala", "U+{:04X}", c as u32);
        }
        // Its neighbours keep theirs: Malayalam before it, Thai after.
        for c in ['\u{0D7F}', '\u{0E01}'] {
            let key = super::fallback_entry(super::MACOS_FALLBACK_FONTS, c).map(|f| f.key);
            assert_ne!(key, Some("sinhala"), "U+{:04X}", c as u32);
        }
    }

    /// Drawn with the list's font alone, the system's cascade not asked.
    #[cfg(target_os = "macos")]
    #[test]
    fn sinhala_names_get_a_font_on_macos() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(vec![std::sync::Arc::new(super::ScriptFallback::default())]);
        let id = FontId::new(13.0, FontFamily::Proportional);
        assert!(
            fonts.has_glyphs(&id, SINHALA),
            "no font for {SINHALA}; found {:?}",
            font_names(&fonts)
        );
        let names = font_names(&fonts);
        assert!(
            names.len() == 1 && names[0].starts_with("fallback:Supplemental/Sinhala"),
            "{names:?}"
        );
    }

    /// The same names drawn with macOS's own fonts: the fallback finds a file that is there and
    /// has the letters.
    #[cfg(target_os = "macos")]
    #[test]
    fn persian_names_get_a_font_on_macos() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(vec![std::sync::Arc::new(super::ScriptFallback::default())]);
        let id = FontId::new(13.0, FontFamily::Proportional);
        for text in OTHER_SCRIPTS {
            assert!(fonts.has_glyphs(&id, text), "no font for {text}");
        }
    }
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use unicode_segmentation::UnicodeSegmentation;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    /// Drawn as nothing on purpose: bidi controls and isolates, joiners, the soft hyphen, the
    /// BOM (craft-i18n's isolates around data, the translations' own marks).
    fn invisible(c: char) -> bool {
        matches!(c, '\u{00ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{fe00}'..='\u{fe0f}')
    }

    /// The marks in `text`: each grapheme's first character that is neither a letter, a digit,
    /// a space nor invisible (a grapheme's other characters are its accents and joiners).
    fn marks(text: &str, out: &mut BTreeSet<char>) {
        for g in text.graphemes(true) {
            if let Some(c) = g.chars().next()
                && !c.is_alphanumeric()
                && !c.is_whitespace()
                && !c.is_control()
                && !invisible(c)
            {
                out.insert(c);
            }
        }
    }

    /// A catalog's texts: the translations (and the English sources, which a missing row shows).
    fn catalog(code: &str) -> String {
        let file = Path::new(ROOT).join("i18n").join(format!("{code}.tsv"));
        let text = std::fs::read_to_string(&file).unwrap();
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('@'))
            .flat_map(|l| l.split('\t').skip(1))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The marks the code draws itself, beside the catalogs' ("\u{2714} {}", "→"): every
    /// string literal in the app's sources, escapes read.
    fn literals() -> BTreeSet<char> {
        let mut out = BTreeSet::new();
        let dir = Path::new(ROOT).join("crates/app/src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // The tests' own data (other scripts, emoji in file names) is not drawn.
            let src = src.split("#[cfg(test)]").next().unwrap_or_default();
            for lit in src.split('"').skip(1).step_by(2) {
                let mut s = String::new();
                let mut rest = lit;
                while let Some(at) = rest.find("\\u{") {
                    s.push_str(&rest[..at]);
                    let end = rest[at..].find('}').map_or(rest.len(), |e| at + e);
                    if let Some(c) = u32::from_str_radix(&rest[at + 3..end], 16)
                        .ok()
                        .and_then(char::from_u32)
                    {
                        s.push(c);
                    }
                    rest = &rest[(end + 1).min(rest.len())..];
                }
                s.push_str(rest);
                // Only what isn't plain ASCII: every font has that.
                let mut found = BTreeSet::new();
                marks(&s, &mut found);
                out.extend(found.into_iter().filter(|c| !c.is_ascii()));
            }
        }
        out
    }

    fn languages() -> Vec<String> {
        let file = Path::new(ROOT).join("i18n/languages.tsv");
        std::fs::read_to_string(file)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| l.split('\t').next().map(str::to_owned))
            .collect()
    }

    #[test]
    fn every_language_draws_its_marks() {
        let code_marks = literals();
        let families = [
            egui::FontFamily::Name("tahoma".into()),
            egui::FontFamily::Name("tahoma-bold".into()),
            egui::FontFamily::Proportional,
        ];
        let mut missing: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
        let mut skipped = Vec::new();
        for code in languages() {
            let text = catalog(&code);
            let mut needed = code_marks.clone();
            marks(&text, &mut needed);
            let ctx = egui::Context::default();
            super::fonts(&ctx, &code, false);
            let mut out = ctx.run_ui(Default::default(), |_| {});
            out.textures_delta.clear();
            // A computer without the language's script font draws none of it (a CI image without
            // CJK fonts): nothing to check there.
            let letters: Vec<char> = text
                .chars()
                .filter(|c| c.is_alphabetic() && !c.is_ascii())
                .take(200)
                .collect();
            let has_script = ctx.fonts_mut(|f| {
                let id = egui::FontId::new(11.0, families[0].clone());
                letters.is_empty()
                    || letters.iter().filter(|c| f.has_glyph(&id, **c)).count() * 10
                        >= letters.len() * 9
            });
            // A Chinese interface whose own script has a file here must draw its own characters
            // (#1605: Simplified Chinese drawn as boxes, with a Japanese font standing in): not
            // skipped, and the characters only its script writes so are checked too.
            #[cfg(windows)]
            let own = match code.as_str() {
                "zh-hans" => Some((super::HAN_SC, super::SIMPLIFIED_ONLY)),
                "zh-hant" => Some((super::HAN_TC, super::TRADITIONAL_ONLY)),
                _ => None,
            }
            .filter(|(files, _)| files.iter().any(|f| super::font_dir().join(f).is_file()));
            #[cfg(not(windows))]
            let own: Option<(&[&str], &str)> = None;
            if let Some((_, probe)) = own {
                needed.extend(probe.chars());
            } else if !has_script {
                skipped.push(code);
                continue;
            }
            for family in &families {
                let id = egui::FontId::new(11.0, family.clone());
                let lacks: Vec<char> = ctx.fonts_mut(|f| {
                    needed
                        .iter()
                        .copied()
                        .filter(|c| !f.has_glyph(&id, *c))
                        .collect()
                });
                if !lacks.is_empty() {
                    missing
                        .entry(format!("{code} ({family:?})"))
                        .or_default()
                        .extend(lacks);
                }
            }
        }
        if !skipped.is_empty() {
            eprintln!(
                "no font for these languages' script on this computer, not checked: {skipped:?}"
            );
        }
        let report: Vec<String> = missing
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k}: {}",
                    v.iter()
                        .map(|c| format!("{c} U+{:04X}", *c as u32))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect();
        assert!(
            report.is_empty(),
            "marks without a glyph:\n{}",
            report.join("\n")
        );
    }
}

#[cfg(test)]
mod status_width_tests {
    /// #736: the list's Status cell while a torrent's files or a finished file are checked or a
    /// video joined, a short word and how far ("Checking 45%", "Merging 63%"), fits its column
    /// at the default width in English, German, French, Russian and Arabic, at 100 % and at
    /// 150 % (View → Interface size), the widest percentage ("100 %") and "..." before it is
    /// known included. "63% merged" was cut to "63% me..." at 65 px, and a torrent's "Wird
    /// überprüft 45 %" in German.
    ///
    /// In the list's font as this system draws it: Segoe UI on Windows, the system font on
    /// macOS, fontconfig's interface font on Linux (DejaVu Sans on a bare Ubuntu, where German
    /// needs 147 px). The default grows with the font there ([`crate::list::status_width`]); on
    /// Windows it stays 132 px, so the list's other columns keep their room.
    #[test]
    fn finishing_fits_the_status_column() {
        let i18n = crate::i18n::i18n();
        #[cfg(not(any(windows, target_os = "macos")))]
        println!(
            "interface font: {:?}",
            super::fc_font("sans-serif").map(|(f, _)| f)
        );
        let mut widest = (0.0f32, String::new());
        for code in ["en", "de", "fr", "ru", "ar"] {
            let lang = i18n.from_code(code).expect("a catalog");
            let full = craft_i18n::Format::from_tag(code).percent(1.0, 0);
            for zoom in [1.0, 1.5] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(zoom);
                super::fonts(&ctx, code, false);
                let mut out = ctx.run_ui(Default::default(), |_| {});
                out.textures_delta.clear();
                let font = crate::winui::tahoma();
                let measure = |text: &str| {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                            .size()
                            .x
                    })
                };
                let column = crate::list::status_width(code, &full, measure);
                let room = column - crate::list::CELL_PAD;
                println!("{code} at {zoom}: Status {column} px");
                #[cfg(windows)]
                assert_eq!(
                    column,
                    crate::list::DEFAULT_WIDTHS[crate::list::STATUS],
                    "{code} at {zoom}: Segoe UI fits in the default"
                );
                for source in ["Checking {percent}", "Merging {percent}"] {
                    for percent in [full.as_str(), "100%", "..."] {
                        let text = i18n.t(lang, source).replace("{percent}", percent);
                        let width = measure(&text);
                        if width > widest.0 {
                            widest = (width, format!("{code} at {zoom}: {text}"));
                        }
                        assert!(
                            width <= room,
                            "{code} at {zoom}: {text}: {width} px, {room} px of room"
                        );
                    }
                }
            }
        }
        println!("widest: {} px, {}", widest.0, widest.1);
    }
}
