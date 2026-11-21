//! Windows 11 dark-mode scroll bars, as IDM's list and tree show them
//! (notes/ui-spec/scrollbars.md): 17 px thick, collapsed to a 2 px line until the pointer is
//! over the bar, then a 6 px rounded thumb with arrows. Win32 thumb sizing.

use egui::{Color32, Pos2, Rect, Ui, pos2};

use crate::winui::{fill, px};

/// A part of the bar, for the one held down.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    First,
    Second,
    Thumb,
    Track,
}

/// Bar thickness and arrow-zone length (SM_CXVSCROLL at 96 DPI).
pub const THICK: f32 = 17.0;
use crate::theme::themed;

// The active theme's colours (crate::palette, #45).
themed! {
    /// Light values from the light progress dialog's table (light-dialogs.md): track #f0f0f0,
    /// thumb #858585; corner not captured [?].
    TRACK => scroll_track;
    pub CORNER => scroll_corner;
    INK_C => scroll_thumb;
    /// A pressed arrow or thumb (a dragged thumb too): brighter on the dark track, darker on
    /// the light one, as Windows 11's bars answer a press.
    INK_PRESSED => scroll_thumb_pressed;
    /// The arrow or thumb under the pointer: between rest and pressed.
    INK_HOT => scroll_thumb_hot;
}

/// Measured glyphs over the #17 track, columns = across the bar from v 5 (up/down) or rows =
/// across from v 5 (left/right). Values are grey levels.
const UP: [[u8; 8]; 6] = [
    [0x17, 0x17, 0x2f, 0x86, 0x86, 0x2f, 0x17, 0x17],
    [0x17, 0x1b, 0x81, 0x95, 0x95, 0x81, 0x1b, 0x17],
    [0x17, 0x5c, 0x95, 0x95, 0x95, 0x95, 0x5c, 0x17],
    [0x36, 0x95, 0x95, 0x95, 0x95, 0x95, 0x95, 0x36],
    [0x85, 0x95, 0x95, 0x95, 0x95, 0x95, 0x95, 0x85],
    [0x7f, 0x95, 0x95, 0x95, 0x95, 0x95, 0x95, 0x80],
];
/// Thumb end cap (top / left end), 4 rows along × 6 across (v 6..11).
const CAP: [[u8; 6]; 4] = [
    [0x17, 0x56, 0x8d, 0x8d, 0x5e, 0x17],
    [0x5e, 0x95, 0x95, 0x95, 0x95, 0x55],
    [0x8d, 0x95, 0x95, 0x95, 0x95, 0x8d],
    [0x95, 0x95, 0x95, 0x95, 0x95, 0x95],
];

/// A scroll bar over `track` (its full 17 px band). Units are whatever the caller scrolls in:
/// items for vertical lists (Win32 list views), pixels for horizontal. `page` is how many
/// units are fully visible, `total` the content size, `line` one arrow click. Returns true
/// when `pos` changed. A horizontal bar in a right-to-left surface runs from the right: the
/// start of the content (0) puts the thumb at the right end.
#[allow(clippy::too_many_arguments)]
pub fn bar(
    ui: &mut Ui,
    id: egui::Id,
    track: Rect,
    vertical: bool,
    total: f32,
    page: f32,
    line: f32,
    pos: &mut f32,
) -> bool {
    let p = ui.painter().clone();
    let len = if vertical {
        track.height()
    } else {
        track.width()
    };
    let max = (total - page).max(0.0);
    let avail = (len - 2.0 * THICK).max(0.0);
    // Win32: MulDiv(avail, page, total), rounded; never smaller than a usable grip.
    let thumb = if total > 0.0 {
        (avail * page / total)
            .round()
            .clamp(8.0_f32.min(avail), avail)
    } else {
        avail
    };
    let start = |pos: f32| {
        THICK
            + if max > 0.0 {
                ((avail - thumb) * pos / max).round()
            } else {
                0.0
            }
    };

    let resp = ui.interact(track, id, crate::winui::pointer_drag());
    let from_right = !vertical && crate::winui::rtl();
    // Along-the-bar coordinate of a point.
    let along = |q: Pos2| {
        if vertical {
            q.y - track.top()
        } else if from_right {
            track.right() - q.x
        } else {
            q.x - track.left()
        }
    };
    let before = *pos;
    let grab_id = id.with("grab");
    // The thumb is grabbed where the button went down (taken when egui decides it's a drag,
    // the pointer had moved on: the thumb jumped, a quick flick missed it).
    if resp.is_pointer_button_down_on()
        && ui.input(|i| i.pointer.any_pressed())
        && let Some(q) = ui.input(|i| i.pointer.press_origin())
    {
        let u = along(q);
        let s = start(*pos);
        if u >= s && u < s + thumb {
            ui.ctx().data_mut(|d| d.insert_temp(grab_id, u - s));
        }
    }
    if resp.is_pointer_button_down_on()
        && let Some(grab) = ui.ctx().data(|d| d.get_temp::<f32>(grab_id))
        && let Some(q) = resp.interact_pointer_pos()
        && avail > thumb
    {
        *pos = (along(q) - grab - THICK) / (avail - thumb) * max;
    }
    if !resp.is_pointer_button_down_on() {
        ui.ctx().data_mut(|d| d.remove::<f32>(grab_id));
    }
    // An arrow or the track: on the press, then again while held (not while the thumb is
    // grabbed).
    let grabbing = ui.ctx().data(|d| d.get_temp::<f32>(grab_id)).is_some();
    if !grabbing
        && crate::winui::auto_repeat(ui, &resp)
        && let Some(q) = resp.interact_pointer_pos()
    {
        let u = along(q);
        let s = start(*pos);
        *pos += if u < THICK {
            -line
        } else if u >= len - THICK {
            line
        } else if u < s {
            -page
        } else if u >= s + thumb {
            page
        } else {
            0.0
        };
    }
    *pos = pos.clamp(0.0, max);

    // ---- paint -----------------------------------------------------------------------------
    fill(
        &p,
        track.left(),
        track.top(),
        track.right() - 1.0,
        track.bottom() - 1.0,
        TRACK(),
    );
    let s = start(*pos);
    let e = s + thumb - 1.0;
    // The pixel at (u along, v across).
    let at = |u: f32, v: f32| {
        if vertical {
            pos2(track.left() + v, track.top() + u)
        } else if from_right {
            pos2(track.right() - 1.0 - u, track.top() + v)
        } else {
            pos2(track.left() + u, track.top() + v)
        }
    };
    // The pixels from (u0, v0) to (u1, v1), inclusive.
    let span = |a: Pos2, b: Pos2, c: Color32| fill(&p, a.x.min(b.x), a.y, a.x.max(b.x), b.y, c);
    // Measured greys over the dark track (#17 → ink #95) become coverage, so the same grids
    // draw in both modes.
    // The part held down: the first arrow, the second, or the thumb (also while dragged).
    let held = (resp.is_pointer_button_down_on() || resp.dragged())
        .then(|| resp.interact_pointer_pos().map(along))
        .flatten()
        .map(|u| {
            if resp.dragged() || (u >= s && u <= e && u >= THICK && u < len - THICK) {
                Part::Thumb
            } else if u < THICK {
                Part::First
            } else if u >= len - THICK {
                Part::Second
            } else {
                Part::Track
            }
        });
    // The part under the pointer (while nothing is held).
    let hot = (held.is_none())
        .then(|| resp.hover_pos().map(along))
        .flatten()
        .map(|u| {
            if u < THICK {
                Part::First
            } else if u >= len - THICK {
                Part::Second
            } else if u >= s && u <= e {
                Part::Thumb
            } else {
                Part::Track
            }
        });
    let ink_of = |part: Part| {
        if held == Some(part) {
            INK_PRESSED()
        } else if hot == Some(part) {
            INK_HOT()
        } else {
            INK_C()
        }
    };
    let dot = |u: f32, v: f32, g: u8, ink: Color32| {
        if g != 0x17 {
            let a = ((g as f32 - 23.0) / (149.0 - 23.0)).clamp(0.0, 1.0);
            let q = at(u, v);
            px(&p, q.x, q.y, ink.gamma_multiply(a));
        }
    };
    // Windows XP's: the arrows and the thumb on buttons of their own.
    let look = crate::theme::current().controls.scroll;
    if look == crate::palette::ScrollLook::Luna {
        // The part from `u0` to `u1` along, the bar's whole width across, on screen.
        let tile = |u0: f32, u1: f32| {
            let (a, b) = (at(u0, 0.0), at(u1, THICK - 1.0));
            Rect::from_min_max(
                pos2(a.x.min(b.x), a.y.min(b.y)),
                pos2(a.x.max(b.x) + 1.0, a.y.max(b.y) + 1.0),
            )
        };
        luna_button(&p, tile(0.0, THICK - 1.0), ink_of(Part::First));
        luna_button(&p, tile(len - THICK, len - 1.0), ink_of(Part::Second));
        let glyph = crate::theme::colors().scroll_glyph;
        // Each arrow's tip 6 in from the bar's end.
        for (k, across) in CHEVRON.iter().enumerate() {
            for d in *across {
                for u in [6.0 + k as f32, len - 7.0 - k as f32] {
                    let q = at(u, 8.0 + d);
                    px(&p, q.x, q.y, glyph);
                }
            }
        }
        if thumb > 0.0 {
            let face = ink_of(Part::Thumb);
            luna_button(&p, tile(s, e), face);
            // The grip: four short lines across its middle, each with a light one after it.
            if thumb >= 20.0 {
                let mid = ((s + e) / 2.0).floor();
                let light = crate::derive::mix(face, crate::theme::colors().gloss, 0.7);
                let dark = crate::derive::mix(face, glyph, 0.45);
                for i in 0..4 {
                    let u = mid - 4.0 + 2.0 * i as f32;
                    span(at(u, 5.0), at(u, THICK - 7.0), dark);
                    span(at(u + 1.0, 5.0), at(u + 1.0, THICK - 7.0), light);
                }
            }
        }
        return *pos != before;
    }
    // Windows 8's and the classic bars: arrows always shown, a square thumb the bar's width.
    let classic = look == crate::palette::ScrollLook::Classic;
    let expanded = classic || resp.hovered() || resp.dragged();
    if thumb > 0.0 {
        if expanded {
            let ink = ink_of(Part::Thumb);
            if classic {
                span(at(s, 2.0), at(e, THICK - 3.0), ink);
            } else {
                // 6 px thumb at v 6..11 with rounded caps.
                for (i, row) in CAP.iter().enumerate() {
                    for (j, g) in row.iter().enumerate() {
                        dot(s + i as f32, 6.0 + j as f32, *g, ink);
                        dot(e - i as f32, 6.0 + j as f32, *g, ink);
                    }
                }
                span(at(s + 4.0, 6.0), at(e - 4.0, 11.0), ink);
            }
            // Arrows: up/left tip at u 6 (left: 5), down/right tip at len − 6.
            let first = if vertical { 6.0 } else { 5.0 };
            for (i, row) in UP.iter().enumerate() {
                for (j, g) in row.iter().enumerate() {
                    dot(first + i as f32, 5.0 + j as f32, *g, ink_of(Part::First));
                    dot(
                        len - 6.0 - i as f32,
                        5.0 + j as f32,
                        *g,
                        ink_of(Part::Second),
                    );
                }
            }
        } else {
            // 2 px line at v 8..9, one #85 pixel at each end.
            let ink = INK_C();
            span(at(s, 8.0), at(e, 9.0), ink);
            for v in [8.0, 9.0] {
                dot(s, v, 0x85, ink);
                dot(e, v, 0x85, ink);
            }
        }
    }
    *pos != before
}

/// Windows XP's arrow, row by row from its tip: the pixels across, from the bar's middle (a
/// chevron 9 pixels across, 5 along, its arms 2 thick).
const CHEVRON: [&[f32]; 5] = [
    &[0.0],
    &[-1.0, 0.0, 1.0],
    &[-2.0, -1.0, 1.0, 2.0],
    &[-3.0, -2.0, 2.0, 3.0],
    &[-4.0, -3.0, 3.0, 4.0],
];

/// One of Windows XP's scroll bar buttons (an arrow's or the thumb) in `rc`, on screen: its
/// `face` lighter toward its top, a light line inside its edge, the edge in `scroll.edge`.
fn luna_button(p: &egui::Painter, rc: Rect, face: Color32) {
    use crate::derive::mix;
    let c = crate::theme::colors();
    crate::winui::fill_shaded(
        p,
        rc,
        2.0,
        &[
            (0.0, mix(face, c.gloss, 0.45)),
            (0.5, mix(face, c.gloss, 0.15)),
            (1.0, face),
        ],
    );
    p.rect_stroke(
        rc.shrink(1.0),
        1.0,
        egui::Stroke::new(1.0, mix(face, c.gloss, 0.6)),
        egui::StrokeKind::Inside,
    );
    p.rect_stroke(
        rc,
        2.0,
        egui::Stroke::new(1.0, c.scroll_edge),
        egui::StrokeKind::Inside,
    );
}

/// The 17 × 17 square where a vertical and a horizontal bar meet, its top-left at English
/// (`x`, `y`) — at the bottom-left in RTL, where the vertical bar is.
pub fn corner(ui: &Ui, x: f32, y: f32) {
    let x = crate::winui::at(pos2(x, y), THICK).x;
    fill(
        ui.painter(),
        x,
        y,
        x + THICK - 1.0,
        y + THICK - 1.0,
        CORNER(),
    );
}
