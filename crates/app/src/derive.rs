//! A whole palette from a few colours (#45): the background, the surface lists and boxes sit
//! on, the text and the accent. Every other role is mixed from them in OKLab (Björn Ottosson's
//! perceptual space), so hover and pressed shades step evenly whatever the colours, and text
//! keeps WCAG 2.1's contrast: 4.5:1 for text, 3:1 for the accent and a control's edges.
//!
//! The classic look isn't derived: its roles are written out (`palette.rs`). Derived themes are
//! the other built-ins and theme files that give `[light]` / `[dark]` seeds instead of roles.

use egui::Color32;

use crate::palette::{Palette, Segments};

/// The colours a derived palette starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seeds {
    /// Windows' and dialogs' background.
    pub background: Color32,
    /// Lists, text boxes and menus.
    pub surface: Color32,
    /// The menu bar and column headers.
    pub header: Color32,
    pub text: Color32,
    pub accent: Color32,
    /// A selection's fill (rows, a chosen day): the accent unless set.
    pub selection: Option<Color32>,
    pub error: Color32,
    pub warning: Color32,
    pub success: Color32,
}

impl Seeds {
    /// The seeds a palette was made from (or would be): its own roles of the same meaning.
    pub fn of(p: &Palette) -> Self {
        Seeds {
            background: p.window,
            surface: p.field,
            header: p.header,
            text: p.text,
            accent: p.accent,
            selection: Some(p.selection),
            error: p.error,
            warning: p.warning,
            success: p.success,
        }
    }
}

// ---- colour arithmetic -----------------------------------------------------------------------

fn to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// OKLab: lightness and two colour axes.
#[derive(Clone, Copy, Debug)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

fn lab(c: Color32) -> Lab {
    let (r, g, b) = (to_linear(c.r()), to_linear(c.g()), to_linear(c.b()));
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    Lab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

fn rgb_of(c: Lab) -> Color32 {
    let l = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    Color32::from_rgb(
        to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

/// `a` moved `t` (0..=1) of the way to `b`, perceptually.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let (x, y) = (lab(a), lab(b));
    rgb_of(Lab {
        l: x.l + (y.l - x.l) * t,
        a: x.a + (y.a - x.a) * t,
        b: x.b + (y.b - x.b) * t,
    })
}

/// `c` lighter (`dl` > 0) or darker, in OKLab lightness.
pub fn shade(c: Color32, dl: f32) -> Color32 {
    let mut x = lab(c);
    x.l = (x.l + dl).clamp(0.0, 1.0);
    rgb_of(x)
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// WCAG 2.1's relative luminance.
pub fn luminance(c: Color32) -> f32 {
    0.2126 * to_linear(c.r()) + 0.7152 * to_linear(c.g()) + 0.0722 * to_linear(c.b())
}

/// WCAG 2.1's contrast ratio, 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `fg` moved in lightness, away from `bg`, until it has `ratio` against it (or can't).
pub fn readable(fg: Color32, bg: Color32, ratio: f32) -> Color32 {
    if contrast(fg, bg) >= ratio {
        return fg;
    }
    let up = lab(bg).l < 0.6;
    let mut c = fg;
    for _ in 0..100 {
        c = shade(c, if up { 0.01 } else { -0.01 });
        if contrast(c, bg) >= ratio {
            return c;
        }
    }
    if up { Color32::WHITE } else { Color32::BLACK }
}

/// Black or white, whichever reads better on `bg`.
pub fn ink_on(bg: Color32) -> Color32 {
    if contrast(Color32::BLACK, bg) >= contrast(Color32::WHITE, bg) {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

// ---- the palette -----------------------------------------------------------------------------

/// Every role from the seeds.
pub fn derive(s: &Seeds) -> Palette {
    let dark = luminance(s.background) < 0.18;
    let (bg, surface, text) = (s.background, s.surface, s.text);
    // Toward the text: a hover's, a border's direction on either kind of surface.
    let toward = |c: Color32, t: f32| mix(c, text, t);
    let accent = readable(s.accent, bg, 3.0);
    let selection = s.selection.unwrap_or(accent);
    let selection_text = ink_on(selection);
    let accent_text = ink_on(accent);
    // The accent a step lighter (dark) or darker (light) for hover, two for pressed: Windows 11's.
    let step = if dark { -0.05 } else { 0.06 };
    let accent_hot = shade(accent, -step * 0.6);
    let accent_pressed = shade(accent, -step * 1.2);
    let disabled = readable(mix(text, bg, 0.55), bg, 2.5);
    let muted = readable(mix(text, bg, 0.3), bg, 4.5);
    let border = toward(surface, if dark { 0.16 } else { 0.14 });
    let edge = readable(toward(surface, 0.4), surface, 3.0);
    let button = if dark {
        toward(bg, 0.06)
    } else {
        mix(surface, bg, 0.15)
    };
    let tint = |c: Color32, t: f32| mix(c, accent, t);
    let missing = |i: usize| mix(surface, text, 0.04 + i as f32 * 0.006);
    let done = |i: usize| shade(accent, 0.12 - i as f32 * 0.018);
    let start = |i: usize| shade(s.error, 0.08 - i as f32 * 0.012);
    let mut segments = Segments {
        missing: [Color32::BLACK; 13],
        done: [Color32::BLACK; 13],
        start: [Color32::BLACK; 13],
    };
    for i in 0..13 {
        segments.missing[i] = missing(i);
        segments.done[i] = done(i);
        segments.start[i] = start(i);
    }
    let scroll_track = mix(bg, surface, 0.5);
    let thumb = readable(toward(scroll_track, 0.45), scroll_track, 3.0);
    Palette {
        dark,
        focus_line_straight: false,
        access_keys_always: false,
        tree_chevrons: dark,
        segments,
        window: bg,
        text,
        text_disabled: disabled,
        muted,
        link: readable(accent, bg, 4.5),
        error: readable(s.error, bg, 4.5),
        error_strong: readable(s.error, bg, 4.5),
        warning: readable(s.warning, bg, 4.5),
        success: readable(s.success, bg, 4.5),
        // A server's round trip in Properties: near, medium, far as success, warning, error.
        reach_near: readable(s.success, bg, 4.5),
        reach_medium: readable(s.warning, bg, 4.5),
        reach_far: readable(s.error, bg, 4.5),
        flag_edge: Color32::from_black_alpha(40),
        accent,
        focus_ring: toward(bg, 0.85),
        selection,
        selection_text,
        selection_muted: mix(selection_text, selection, 0.2),
        selection_inactive: toward(surface, 0.14),
        selection_focus: readable(s.warning, selection, 3.0),
        group_line: toward(bg, 0.16),
        etch_shadow: toward(bg, 0.3),
        etch_light: if dark {
            toward(bg, 0.08)
        } else {
            mix(bg, Color32::WHITE, 0.8)
        },
        tab_page: mix(bg, surface, 0.5),
        tab: toward(bg, 0.03),
        tab_hot: mix(bg, surface, 0.8),
        chevron: muted,
        chevron_disabled: disabled,
        subtle_hot: toward(bg, 0.06),
        subtle_pressed: toward(bg, 0.11),
        highlight_hot: tint(surface, 0.14),
        highlight_pressed: tint(surface, 0.26),
        field: surface,
        field_border: border,
        field_bottom: edge,
        field_disabled: mix(bg, surface, 0.5),
        field_disabled_border: toward(mix(bg, surface, 0.5), 0.08),
        field_readonly: mix(bg, surface, 0.3),
        field_readonly_border: toward(mix(bg, surface, 0.3), 0.1),
        combo_chevron: muted,
        combo_arrow_hot: toward(surface, 0.06),
        combo_arrow_pressed: toward(surface, 0.11),
        button,
        button_hot: toward(button, 0.05),
        button_pressed: toward(button, if dark { -0.02 } else { 0.03 }),
        button_border: toward(button, 0.16),
        button_bottom: toward(button, 0.24),
        button_text: text,
        button_text_pressed: mix(text, button, 0.3),
        button_default_border: accent,
        button_default_bottom: accent_pressed,
        button_disabled: mix(bg, button, 0.5),
        button_disabled_border: toward(mix(bg, button, 0.5), 0.08),
        button_accent: accent,
        button_accent_hot: accent_hot,
        button_accent_pressed: accent_pressed,
        button_accent_bottom: shade(accent, -0.08),
        button_accent_text: accent_text,
        button_accent_text_pressed: mix(accent_text, accent_pressed, 0.15),
        button_glow: Color32::TRANSPARENT,
        gloss: Color32::WHITE,
        check: toward(surface, 0.03),
        check_hot: toward(surface, 0.08),
        check_pressed: toward(surface, 0.13),
        check_border: edge,
        check_border_hot: edge,
        check_border_pressed: mix(edge, surface, 0.3),
        check_border_disabled: disabled,
        check_on: accent,
        check_on_hot: accent_hot,
        check_on_pressed: accent_pressed,
        check_on_disabled: disabled,
        check_mark: accent_text,
        progress_track: toward(bg, 0.4),
        progress_bar: accent,
        meter_track: mix(bg, surface, 0.6),
        meter_edge: border,
        meter_fill: readable(s.success, surface, 3.0),
        list: surface,
        list_frame: toward(surface, 0.24),
        grid: toward(surface, 0.06),
        list_grid: toward(surface, 0.07),
        header: s.header,
        header_line: toward(s.header, 0.1),
        header_sep: toward(s.header, 0.14),
        table_line: toward(surface, 0.12),
        sorted: toward(s.header, 0.05),
        sort_arrow: muted,
        row_pressed: toward(surface, 0.12),
        chip_web: readable(s.success, Color32::WHITE, 3.0),
        chip_torrent: Color32::from_rgb(0x7e, 0x4f, 0xd0),
        chip_text: Color32::WHITE,
        drag_ghost: with_alpha(toward(surface, 0.4), 160),
        marquee: with_alpha(accent, 0x40),
        marquee_border: accent,
        scroll_track,
        scroll_corner: scroll_track,
        scroll_thumb: thumb,
        scroll_thumb_hot: toward(thumb, 0.2),
        scroll_thumb_pressed: toward(thumb, 0.4),
        scroll_edge: toward(thumb, 0.25),
        scroll_glyph: text,
        menubar: s.header,
        menubar_bottom: s.header,
        menubar_line: toward(s.header, 0.08),
        menubar_hot: toward(s.header, 0.07),
        menubar_open: toward(s.header, 0.1),
        menu: surface,
        menu_item: surface,
        menu_hot: toward(surface, 0.07),
        menu_border: toward(surface, 0.14),
        menu_sep: toward(surface, 0.12),
        menu_text: text,
        menu_glyph: text,
        menu_text_disabled: readable(mix(text, surface, 0.55), surface, 2.5),
        toolbar_line: toward(bg, 0.12),
        toolbar_line_light: bg,
        toolbar_hot: tint(bg, 0.12),
        toolbar_hot_faint: tint(bg, 0.05),
        toolbar_pressed: tint(bg, 0.22),
        toolbar_hot_edge: tint(bg, 0.3),
        toolbar_hot_border: tint(bg, 0.3),
        toolbar_split_shadow: toward(bg, 0.2),
        toolbar_split_light: bg,
        toolbar_text_disabled: disabled,
        toolbar_arrow: muted,
        icon_disabled: disabled,
        icon_disabled_end: disabled,
        tree: surface,
        tree_expander: text,
        tree_lines: toward(surface, 0.35),
        tree_frame: toward(bg, 0.16),
        status_value: text,
        connections: Color32::TRANSPARENT,
        progress_label_disabled: disabled,
        segment_border: toward(surface, 0.4),
        segment_corner: toward(surface, 0.25),
        piece: readable(s.success, surface, 3.0),
        piece_partial: mix(s.success, surface, 0.55),
        piece_missing: toward(surface, 0.12),
        caption: s.header,
        caption_text: text,
        caption_text_inactive: mix(text, s.header, 0.45),
        caption_hot: with_alpha(text, 0x10),
        caption_pressed: with_alpha(text, 0x0a),
        caption_button: Color32::TRANSPARENT,
        caption_close: Color32::from_rgb(0xc4, 0x2b, 0x1c),
        caption_close_text: Color32::WHITE,
        caption_inactive: Color32::TRANSPARENT,
        drop_edge: toward(bg, 0.4),
        egui_panel: bg,
        egui_window: surface,
        egui_window_border: toward(surface, 0.14),
        egui_text: text,
        egui_selection_text: selection_text,
        egui_extreme: surface,
        egui_faint: toward(surface, 0.03),
        egui_widget: toward(surface, 0.06),
        egui_widget_border: toward(surface, 0.24),
    }
}

/// High contrast (`contrast = "high"` in a theme file, and the system's high-contrast mode):
/// every control's edge in the text's colour, focus in the accent, secondary and greyed text
/// at 7:1 and 4.5:1.
pub fn strengthen(p: &mut Palette) {
    let edge = p.text;
    for c in [
        &mut p.field_border,
        &mut p.field_bottom,
        &mut p.field_disabled_border,
        &mut p.field_readonly_border,
        &mut p.button_border,
        &mut p.list_frame,
        &mut p.group_line,
        &mut p.menu_border,
        &mut p.menu_sep,
        &mut p.check_border,
        &mut p.check_border_hot,
        &mut p.check_border_pressed,
        &mut p.header_sep,
        &mut p.header_line,
        &mut p.tree_frame,
        &mut p.meter_edge,
        &mut p.segment_border,
        &mut p.toolbar_line,
        &mut p.menubar_line,
        &mut p.egui_window_border,
    ] {
        *c = edge;
    }
    // Grid lines and dividers clearly seen, quieter than the edges.
    for c in [
        &mut p.grid,
        &mut p.list_grid,
        &mut p.table_line,
        &mut p.tree_lines,
    ] {
        *c = mix(p.window, p.text, 0.45);
    }
    p.focus_ring = p.accent;
    // A button's bottom edge in the accent: the edge all round stays strong, the face still
    // reads as a button that can be pressed (its bottom flattens then).
    p.button_bottom = p.accent;
    p.button_default_border = p.accent;
    p.muted = readable(p.muted, p.window, 7.0);
    p.chevron = p.text;
    p.combo_chevron = p.text;
    p.sort_arrow = p.text;
    p.scroll_thumb = p.text;
    p.scroll_thumb_hot = p.accent;
    p.scroll_thumb_pressed = p.accent;
    p.text_disabled = readable(p.text_disabled, p.window, 4.5);
    p.menu_text_disabled = readable(p.menu_text_disabled, p.menu, 4.5);
    p.toolbar_text_disabled = p.text_disabled;
    p.check_border_disabled = p.text_disabled;
    p.chevron_disabled = p.text_disabled;
    p.icon_disabled = p.text_disabled;
    p.icon_disabled_end = p.text_disabled;
    // Hot and pressed: the accent's tint, clearly not the rest colour.
    p.subtle_hot = mix(p.window, p.accent, 0.25);
    p.subtle_pressed = mix(p.window, p.accent, 0.4);
    p.menu_hot = mix(p.menu, p.accent, 0.3);
    p.menubar_hot = mix(p.menubar, p.accent, 0.3);
    p.menubar_open = mix(p.menubar, p.accent, 0.4);
    p.button_hot = mix(p.button, p.accent, 0.2);
    p.button_pressed = mix(p.button, p.accent, 0.35);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_ratios() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        let blue = Color32::from_rgb(0x00, 0x78, 0xd4);
        assert!((contrast(blue, Color32::WHITE) - 4.53).abs() < 0.02);
    }

    #[test]
    fn oklab_round_trips() {
        for c in [
            Color32::from_rgb(0x00, 0x78, 0xd4),
            Color32::from_rgb(0x12, 0x34, 0x56),
            Color32::from_gray(0x80),
            Color32::WHITE,
            Color32::BLACK,
        ] {
            assert_eq!(rgb_of(lab(c)), c);
        }
    }

    #[test]
    fn readable_reaches_the_ratio() {
        let bg = Color32::from_gray(0x30);
        let c = readable(Color32::from_gray(0x50), bg, 4.5);
        assert!(contrast(c, bg) >= 4.5);
    }
}
