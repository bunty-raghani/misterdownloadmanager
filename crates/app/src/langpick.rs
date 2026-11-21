//! Choosing the interface language (D-019): on the first start when the installer didn't say
//! (IDM asks in its setup), and from View → Language... (86 languages are too many for a
//! submenu). Every language of i18n/languages.tsv by its flag, its own name and its English
//! name; typing filters the list; the arrows, Page Up/Down, Home/End move; Enter or a double
//! click chooses.

use egui::{Rect, pos2, vec2};

use crate::dialogs::{Outcome, window};
use crate::winui::{self, ButtonKind, r};

crate::theme::themed! {}

const W: f32 = 380.0;
const H: f32 = 452.0;
const LIST: [f32; 4] = [12.0, 62.0, 356.0, 340.0];
const ROW: f32 = 26.0;

/// One row: `None` is "Windows' language" (follow Windows), else a language's code.
type Entry = Option<&'static str>;

pub struct Picker {
    /// The first start: no "Windows' language" row (Windows' own is preselected instead).
    first: bool,
    filter: String,
    selected: Entry,
    /// First visible row.
    top: f32,
    focus_asked: bool,
    /// The keys moved the selection since the search box last changed: a screen reader then
    /// follows the rows, not the box (#1190).
    keys_moved: bool,
}

impl Picker {
    /// `current`: the language setting (`None` follows Windows).
    pub fn new(first: bool, current: Option<&str>) -> Self {
        let pick = |code: &str| crate::i18n::info(code).map(|i| i.code);
        let selected = if first {
            pick(current.unwrap_or_else(|| crate::i18n::system_code())).or(Some("en"))
        } else {
            current.and_then(pick)
        };
        Picker {
            first,
            filter: String::new(),
            selected,
            top: 0.0,
            focus_asked: false,
            keys_moved: false,
        }
    }

    /// The rows the filter lets through: Windows' language (not on the first start), then
    /// every offered language by English name.
    fn rows(&self) -> Vec<Entry> {
        let needle = self.filter.trim().to_lowercase();
        let mut langs: Vec<crate::i18n::Info> = crate::i18n::languages()
            .into_iter()
            .filter_map(|(_, code, _)| crate::i18n::info(code))
            .filter(|i| {
                needle.is_empty()
                    || i.native.to_lowercase().contains(&needle)
                    || i.english.to_lowercase().contains(&needle)
                    || i.code.contains(&needle)
            })
            .collect();
        langs.sort_by_key(|i| i.english);
        let mut rows: Vec<Entry> = Vec::new();
        if !self.first && needle.is_empty() {
            rows.push(None);
        }
        rows.extend(langs.into_iter().map(|i| Some(i.code)));
        rows
    }

    /// `Done(None)`: follow Windows; `Done(Some(code))`: that language; `Closed`: cancelled.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Entry> {
        // Every language shows by its own name: load every script's font.
        crate::theme::all_scripts(ctx);
        // i18n: window title
        let title = if self.first {
            crate::i18n::mark("Choose a language")
        } else {
            // i18n: window title (the interface language)
            crate::i18n::mark("Language")
        };
        window(
            ctx,
            "language",
            title,
            [W, H],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                winui::paragraph(
                    &p,
                    o.x + 12.0,
                    o.y + 20.0,
                    crate::i18n::t(
                        "Choose the language Mister Download Manager shows its windows in:",
                    ),
                    ink,
                    o.x + W - 12.0,
                    13.0,
                );
                winui::text(&p, o.x + 12.0, o.y + 46.0, crate::i18n::t("Search:"), ink);
                let search_x = 12.0 + winui::text_width(&p, crate::i18n::t("Search:")) + 8.0;
                let search = winui::edit(
                    ui,
                    r(o, search_x, 34.0, W - 12.0 - search_x, 21.0),
                    &mut self.filter,
                    true,
                    false,
                );
                if search.changed() {
                    self.keys_moved = false;
                }
                if !self.focus_asked {
                    search.request_focus();
                    self.focus_asked = true;
                }

                let rows = self.rows();
                // A filter that hides the selection selects the first match.
                if !rows.contains(&self.selected)
                    && let Some(&first) = rows.first()
                {
                    self.selected = first;
                }
                let shown_list = r(o, LIST[0], LIST[1], LIST[2], LIST[3]);
                p.rect_filled(shown_list, 0.0, winui::FIELD());
                p.rect_stroke(
                    shown_list,
                    0.0,
                    egui::Stroke::new(1.0, winui::FIELD_EDGE()),
                    egui::StrokeKind::Inside,
                );
                // Rows laid out in English, made where they are on screen: the flag and the
                // language's own name from the leading edge, its English name at the trailing one,
                // the scroll bar after them.
                let list = winui::rect(shown_list);
                let page = ((LIST[3] - 2.0) / ROW).floor();
                let total = rows.len() as f32;
                let need_bar = total > page;
                let inner_right = list.right()
                    - 1.0
                    - if need_bar {
                        crate::scrollbar::THICK
                    } else {
                        0.0
                    };

                // Keys move the selection (also while typing in the search box).
                let at = rows.iter().position(|e| *e == self.selected);
                let (up, down, pgup, pgdn, home, end, enter, esc) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                        i.key_pressed(egui::Key::PageUp),
                        i.key_pressed(egui::Key::PageDown),
                        i.key_pressed(egui::Key::Home) && i.modifiers.ctrl,
                        i.key_pressed(egui::Key::End) && i.modifiers.ctrl,
                        i.key_pressed(egui::Key::Enter),
                        i.key_pressed(egui::Key::Escape),
                    )
                });
                let last = rows.len().saturating_sub(1);
                let mut moved = None;
                if let Some(a) = at {
                    let step = page as usize;
                    if up {
                        moved = Some(a.saturating_sub(1));
                    } else if down {
                        moved = Some((a + 1).min(last));
                    } else if pgup {
                        moved = Some(a.saturating_sub(step));
                    } else if pgdn {
                        moved = Some((a + step).min(last));
                    }
                } else if (down || up) && !rows.is_empty() {
                    moved = Some(0);
                }
                if home && !rows.is_empty() {
                    moved = Some(0);
                } else if end && !rows.is_empty() {
                    moved = Some(last);
                }
                if let Some(m) = moved {
                    self.keys_moved = true;
                    self.selected = rows[m];
                    // Keep the selection in view.
                    let m = m as f32;
                    if m < self.top {
                        self.top = m;
                    } else if m >= self.top + page {
                        self.top = m - page + 1.0;
                    }
                }
                self.top = self.top.clamp(0.0, (total - page).max(0.0));

                // The wheel scrolls the system's lines a notch (three unless changed). The
                // pointer is over a row's own widget, so the list asks where it is, not whether
                // it is the widget hovered (that one is the row: the wheel did nothing).
                if ui.rect_contains_pointer(shown_list) {
                    let rows = winui::wheel_rows(ui, ROW, page);
                    if rows != 0.0 {
                        self.top = (self.top + rows).clamp(0.0, (total - page).max(0.0));
                    }
                }

                let mut chosen = false;
                let clip = p.with_clip_rect(winui::rect(Rect::from_min_max(
                    list.min + vec2(1.0, 1.0),
                    pos2(inner_right, list.bottom() - 1.0),
                )));
                let first = self.top.floor() as usize;
                // The list as one node, its rows inside it (#102): a screen reader says "4 of
                // 87", and UI Automation's Selection finds them under it. Hover only, so the rows
                // keep their clicks.
                let list_resp = ui.interact(
                    shown_list,
                    egui::Id::unique("language-list"),
                    egui::Sense::hover(),
                );
                // Reachable by a screen reader's focus and click, as the shared lists are (the
                // keys already act on the list wherever the focus is); it senses only hover, so
                // neither takes the focus from the filter box.
                ui.ctx().accesskit_node_builder(list_resp.id, |n| {
                    n.add_action(egui::accesskit::Action::Focus);
                    n.add_action(egui::accesskit::Action::Click);
                });
                // i18n: window title (the interface language)
                winui::expose(
                    &list_resp,
                    egui::Role::ListBox,
                    crate::i18n::t("Language"),
                    true,
                    None,
                );
                let count = rows.len();
                // A row's "of M" is read from its list.
                ui.ctx()
                    .accesskit_node_builder(list_resp.id, |n| n.set_size_of_set(count));
                let rows_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(shown_list)
                        .accessibility_parent(list_resp.id),
                );
                // Read as its name (and its English name), selected or not (#95: the rows were
                // blank to screen readers), with its place in the list.
                let expose_row = |resp: &egui::Response, name: &str, english: &str, on, k| {
                    let spoken = if english.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{name}, {english}")
                    };
                    winui::expose(resp, egui::Role::ListBoxOption, &spoken, true, None);
                    resp.ctx.accesskit_node_builder(resp.id, |n| {
                        n.set_selected(on);
                        // AccessKit counts from 0 (the platforms say k + 1).
                        n.set_position_in_set(k);
                        n.set_size_of_set(count);
                        n.add_action(egui::accesskit::Action::ScrollIntoView);
                    });
                };
                // The rows out of view are the list's too (#111), named and in their place:
                // made above and below the view, clipped to nothing (no pointer reaches them). A
                // screen reader's Select (a click) or Scroll into view brings one into view.
                let mut out_of_view = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(shown_list)
                        .accessibility_parent(list_resp.id),
                );
                out_of_view.set_clip_rect(Rect::NOTHING);
                let in_view = first.min(count)..(first + page as usize + 1).min(count);
                let mut bring = None;
                let mut hidden_row = |k: usize, entry: Entry, selected: &mut Entry| {
                    let y = list.top() + 1.0 + (k as f32 - first as f32) * ROW;
                    let row =
                        Rect::from_min_max(pos2(list.left() + 1.0, y), pos2(inner_right, y + ROW));
                    let resp = out_of_view.interact(
                        winui::rect(row),
                        egui::Id::unique(("language-row", k)),
                        winui::pointer_only(),
                    );
                    let scroll = egui::accesskit::Action::ScrollIntoView;
                    // A screen reader's focus selects the row, as its click does (#1190).
                    if resp.clicked() || winui::focus_target(&resp.ctx, resp.id) {
                        *selected = entry;
                        bring = Some(k);
                    } else if resp
                        .ctx
                        .input(|i| i.has_accesskit_action_request(resp.id, scroll))
                    {
                        bring = Some(k);
                    }
                    let (name, english) = row_names(entry);
                    expose_row(&resp, &name, &english, *selected == entry, k);
                };
                for (k, entry) in rows.iter().enumerate().take(in_view.start) {
                    hidden_row(k, *entry, &mut self.selected);
                }
                for (k, entry) in rows.iter().enumerate().skip(first).take(page as usize + 1) {
                    let y = list.top() + 1.0 + (k - first) as f32 * ROW;
                    let row =
                        Rect::from_min_max(pos2(list.left() + 1.0, y), pos2(inner_right, y + ROW));
                    let resp = rows_ui.interact(
                        winui::rect(row),
                        egui::Id::unique(("language-row", k)),
                        winui::pointer_only(),
                    );
                    // A screen reader's focus selects the row, as a click does (#1190).
                    if resp.clicked() || winui::focus_target(ui.ctx(), resp.id) {
                        self.selected = *entry;
                    }
                    if resp.double_clicked() {
                        self.selected = *entry;
                        chosen = true;
                    }
                    let on = self.selected == *entry;
                    // The shared list's colours: the selection (its dark value in dark mode),
                    // the row under the pointer tinted.
                    if on {
                        clip.rect_filled(winui::rect(row), 0.0, winui::ROW_SELECTED());
                    } else if let Some(c) =
                        winui::subtle_fill(winui::State::of(&resp, true), crate::theme::colors())
                    {
                        clip.rect_filled(winui::rect(row), 0.0, c);
                    }
                    let (fg, muted) = if on {
                        (
                            winui::SELECTION_TEXT(),
                            crate::theme::colors().selection_muted,
                        )
                    } else {
                        (ink, winui::MUTED())
                    };
                    let code = entry.unwrap_or_else(|| crate::i18n::system_code());
                    let info = crate::i18n::info(code);
                    // Images aren't clipped by the list: a flag only where it fits inside it (the
                    // part-shown last row painted its flag under the frame).
                    let flag =
                        Rect::from_min_size(pos2(row.left() + 8.0, y + 5.5), vec2(20.0, 15.0));
                    if let Some(i) = &info
                        && flag.bottom() <= list.bottom() - 1.0
                    {
                        egui::Image::from_bytes(format!("bytes://flag-{}.svg", i.code), i.flag)
                            .paint_at(ui, winui::rect(flag));
                    }
                    let (name, english) = row_names(*entry);
                    let right = row.right() - 8.0;
                    let english_w = if english.is_empty() {
                        0.0
                    } else {
                        winui::text_width(&clip, &english) + 12.0
                    };
                    let room = right - (row.left() + 36.0) - english_w;
                    expose_row(&resp, &name, &english, on, k);
                    let shown = winui::ellipsize(&clip, &winui::tahoma(), &name, room);
                    winui::text(&clip, row.left() + 36.0, y + 17.0, &shown, fg);
                    if !english.is_empty() {
                        winui::text_right(&clip, right, y + 17.0, &english, muted);
                    }
                }
                for (k, entry) in rows.iter().enumerate().skip(in_view.end) {
                    hidden_row(k, *entry, &mut self.selected);
                }
                // Once the keys move the selection, the selected row is what a screen reader says
                // has the focus while the search box has it (a combo box's list; #1190: it said
                // nothing); typing gives the box back.
                if self.keys_moved
                    && search.has_focus()
                    && let Some(k) = rows.iter().position(|e| *e == self.selected)
                {
                    let row = egui::Id::unique(("language-row", k)).accesskit_id();
                    ui.ctx()
                        .accesskit_node_builder(search.id, |n| n.set_active_descendant(row));
                }
                if let Some(k) = bring {
                    let k = k as f32;
                    self.top = if k < self.top { k } else { k - page + 1.0 };
                    ui.ctx().request_repaint();
                }
                if rows.is_empty() {
                    winui::text(
                        &p,
                        list.left() + 10.0,
                        list.top() + 20.0,
                        crate::i18n::t("No language matches."),
                        winui::MUTED(),
                    );
                }
                if need_bar {
                    let track = winui::rect(Rect::from_min_max(
                        pos2(inner_right, list.top() + 1.0),
                        pos2(list.right() - 1.0, list.bottom() - 1.0),
                    ));
                    if crate::scrollbar::bar(
                        ui,
                        egui::Id::unique("language-bar"),
                        track,
                        true,
                        total,
                        page,
                        1.0,
                        &mut self.top,
                    ) {
                        self.top = self.top.round();
                        ui.ctx().request_repaint();
                    }
                }

                let ok = winui::button(
                    ui,
                    r(o, W - 12.0 - 73.0 - 8.0 - 73.0, H - 33.0, 73.0, 21.0),
                    "OK",
                    if rows.is_empty() {
                        ButtonKind::Disabled
                    } else {
                        ButtonKind::Default
                    },
                    false,
                    None,
                );
                let ok = winui::why_disabled(
                    ok,
                    rows.is_empty(),
                    // i18n: tooltip of the greyed OK button in the language picker
                    "No language matches the search",
                )
                .clicked();
                let cancel = winui::button(
                    ui,
                    r(o, W - 12.0 - 73.0, H - 33.0, 73.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                // Enter, unless a focused button (Cancel) took it.
                let enter = enter && winui::enter_pressed(ui);
                if (ok || enter || chosen) && !rows.is_empty() {
                    Outcome::Done(self.selected)
                } else if cancel || esc {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// A row's name and its English name (empty when it is the same), as the list shows and a
/// screen reader says them. Each name in its own direction: Arabic's right to left, English left
/// to right, in any interface.
fn row_names(entry: Entry) -> (String, String) {
    let info = crate::i18n::info(entry.unwrap_or_else(crate::i18n::system_code));
    match (entry, info) {
        (None, Some(i)) => (
            crate::i18n::fmt(
                crate::i18n::t("Windows' language ({language})"),
                &[("language", i.native)],
            ),
            String::new(),
        ),
        (None, None) => (
            crate::i18n::t("Windows' language").to_owned(),
            String::new(),
        ),
        (Some(_), Some(i)) => (
            crate::i18n::isolate(i.native),
            if i.english == i.native {
                String::new()
            } else {
                crate::i18n::ltr(i.english)
            },
        ),
        (Some(c), None) => (c.to_owned(), String::new()),
    }
}
