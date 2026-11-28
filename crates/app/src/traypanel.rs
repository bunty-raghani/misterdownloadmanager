//! The notification-area icon's panel (#1365): a click on the icon shows, over it, what is
//! downloading (each download's name, its progress bar, how much has come of how much, its speed
//! and time left) with the progress window's Pause/Resume (Stop and save for a live recording)
//! and Cancel, and a button that opens the main window. A double click on the icon opens the
//! window straight away. Options → General → "Show downloads when the notification area icon is
//! clicked" (`Settings::tray_panel`): ticked for new lists, unticked for lists saved before the
//! panel, where the click keeps showing or hiding the window.
//!
//! A borderless window of ours, as the drop target is ([`crate::chrome::popup_viewport`], one
//! frame for both: [`crate::chrome::floating_frame`]). It takes the focus and closes once it loses
//! it (a click elsewhere), on Escape, on its × or on another click on the icon. The × is the way
//! that never needs the focus: a Wayland compositor may refuse a new window the focus (its
//! focus-stealing prevention), and a window that never had it can't lose it.

use std::time::Duration;

use egui::{Pos2, Rect, Vec2, ViewportId, pos2, vec2};
use mdm_tools::video::LiveState;

use crate::model::{self, Item, Status};
use crate::winui::{self, ButtonKind, r};

/// The panel's width, and the heights of its parts, in points (English layout).
const W: f32 = 380.0;
const HEAD: f32 = 32.0;
const ROW: f32 = 60.0;
/// The line saying there is nothing to show, or how many more there are.
const NOTE: f32 = 30.0;
const FOOT: f32 = 44.0;
/// The most downloads listed; the others are counted under them.
const MOST: usize = 5;
/// Between the icon and the panel, and between the panel and the screen's edge.
const GAP: f32 = 8.0;
/// Where no work area is known (Linux, macOS) and the tray gave no place for the click: the
/// height kept free at the top for a desktop's top bar (GNOME's, macOS's menu bar), whose tray
/// sits at the top right.
const TOP_BAR: f32 = 40.0;
/// How often its figures are drawn again while it shows.
pub const BEAT: Duration = Duration::from_millis(500);

/// The panel's name: its window's title, which screen readers announce it by (never shown: it
/// has no title bar; Windows finds it by this name to style it, `chrome::style_popup`).
fn title() -> &'static str {
    // i18n: the notification-area icon's panel (#1365): its title, over the downloads now running (also the tray menu's submenu)
    crate::i18n::t("Downloads in progress")
}

/// Its viewport.
fn viewport() -> ViewportId {
    ViewportId::from_hash_of("tray-panel")
}

/// What the panel asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The progress window's Pause: a live recording is stopped and saved
    /// (`App::stop_by_hand`).
    Pause(u64),
    Resume(u64),
    /// The progress window's Cancel (`App::cancel_download`).
    Cancel(u64),
    /// The main window, and the panel closed.
    OpenMain,
    /// Closed: its ×, Escape, or it lost the focus.
    Close,
}

/// The panel while it shows.
pub struct Panel {
    /// Where the icon was clicked (screen, points); None where the tray didn't say.
    at: Option<Pos2>,
    /// Downloads paused from the panel: still listed, with Resume, while it shows.
    paused: Vec<u64>,
    /// Its window had the focus: losing it from then on closes the panel.
    focused: bool,
}

/// One download as the panel lists it.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub name: String,
    /// How far, 0 to 1; None without a size.
    pub fraction: Option<f64>,
    /// "12.000 MB of 200.000 MB · 1.500 MB/sec · 2 min 6 sec" (as far as known).
    pub line: String,
    /// What its Pause button says and whether it can be pressed (`progress::pause_button`).
    pub status: Status,
    pub live: Option<LiveState>,
    /// Whether the server lets it continue: `Some(false)` greys out Pause/Resume.
    pub resumable: Option<bool>,
    /// A live stream: its button may read "Stop and save" (`progress::pause_labels`).
    pub live_stream: bool,
}

impl Panel {
    /// A panel for a click on the icon at `at` (screen, points; None where the tray didn't say).
    pub fn new(at: Option<Pos2>) -> Panel {
        Panel {
            at,
            paused: Vec::new(),
            focused: false,
        }
    }

    /// Whether its window is on the screen, as the window says (its viewport's `visible`): from
    /// then on the main window's pass runs with every frame of it (eframe draws a window's
    /// viewports while one of them shows), and needs no asking (`App::tray`). Not a count of
    /// passes: on Windows it is made hidden and shown once styled, which a slow machine may take
    /// longer to do, and with the main window hidden nothing else would run the pass meanwhile.
    pub fn up(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| {
            i.raw
                .viewports
                .get(&viewport())
                .and_then(|v| v.visible())
                .unwrap_or(false)
        })
    }

    /// Paused from the panel: kept in its list, with Resume.
    pub fn paused(&mut self, id: u64) {
        if !self.paused.contains(&id) {
            self.paused.push(id);
        }
    }

    /// Resumed or cancelled: listed again only while it runs.
    pub fn forget(&mut self, id: u64) {
        self.paused.retain(|p| *p != id);
    }

    /// What it lists, in the list's order: every running download, and those paused from it
    /// (not finished since). In that order a paused row stays where it was, under the pointer
    /// that paused it, rather than going after the running ones (a click on Resume then landed
    /// on another download's Pause).
    pub fn rows(&self, items: &[Item]) -> Vec<Row> {
        items
            .iter()
            .filter(|i| {
                i.live.is_some() || (self.paused.contains(&i.id) && i.status != Status::Complete)
            })
            .map(row)
            .collect()
    }

    /// Show the panel; what was asked in it this frame.
    pub fn show(&mut self, ctx: &egui::Context, items: &[Item]) -> Vec<Event> {
        let rows = self.rows(items);
        let size = vec2(W, height(rows.len()));
        // The screen the click was on; with no click's place, the main window's.
        let near = self.at.unwrap_or_else(|| {
            ctx.input(|i| i.viewport().outer_rect)
                .map_or(Pos2::ZERO, |r| r.center())
        });
        let screen = ctx.input(|i| i.viewport().monitor_size);
        let at = place(self.at, size, crate::scale::work_area(ctx, near), screen);
        let name = title();
        let builder = crate::chrome::popup_viewport(name)
            .with_position(at)
            .with_inner_size(size);
        let vid = viewport();
        let mut events = Vec::new();
        let mut styled = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _| {
            styled = crate::chrome::style_popup(name, crate::chrome::PopupLook::Floating);
            crate::dialogs::keyboard_to_click(ui.ctx());
            let o = ui.max_rect().min;
            let _surface = winui::Surface::enter(o.x, o.x + W);
            let p = ui.painter().clone();
            crate::chrome::floating_frame(&p, Rect::from_min_size(o, size), false);
            draw(ui, &p, o, &rows, &mut events);
            // Escape, or the system asked it to close (Alt+F4, a test closing its windows).
            if winui::escape_pressed(ui) || ui.ctx().input(|i| i.viewport().close_requested()) {
                events.push(Event::Close);
            }
        });
        if styled {
            crate::chrome::show_popup(ctx, vid);
            crate::dialogs::focus(ctx, vid);
        }
        // Closed once it loses the focus it had (a click elsewhere, the taskbar's included). Not
        // in an off-screen test copy, whose windows are kept from the foreground (win32.rs).
        match ctx
            .input_for(vid, |i| i.viewport().focused)
            .filter(|_| !crate::offscreen())
        {
            Some(true) => self.focused = true,
            Some(false) if self.focused => events.push(Event::Close),
            _ => {}
        }
        // Its figures move: drawn again on its beat while it shows.
        ctx.request_repaint_after(BEAT);
        events
    }
}

/// The panel's height with `rows` downloads.
fn height(rows: usize) -> f32 {
    let listed = rows.min(MOST);
    let note = if rows == 0 || rows > MOST { NOTE } else { 0.0 };
    HEAD + listed as f32 * ROW + note + FOOT
}

/// Where the panel goes for a click at `at`: centred over it and above it, inside the screen's
/// work area (`area`: the screen without the taskbar), so it sits beside the taskbar wherever
/// that is (at the bottom, the top or a side). Where the work area isn't known (Linux, macOS),
/// below the click when there is no room above it (macOS's menu bar, a panel at the top).
///
/// Where the tray gave no place for the click (a StatusNotifierItem host may send 0, 0): in the
/// work area's corner at the bottom on the reading side's end, where Windows' own flyouts open
/// for its taskbar; with no work area either, at the top on that side under a top bar (GNOME's
/// tray and macOS's menu bar are at the top right), on the screen `screen` (its size). Native
/// Wayland places windows itself: there it goes where the compositor puts it.
pub fn place(at: Option<Pos2>, size: Vec2, area: Option<Rect>, screen: Option<Vec2>) -> Pos2 {
    let end = |left: f32, right: f32| {
        if crate::i18n::rtl() {
            left + GAP
        } else {
            right - size.x - GAP
        }
    };
    match (at, area) {
        (Some(at), Some(a)) => {
            crate::scale::keep_inside(pos2(at.x - size.x / 2.0, at.y - size.y - GAP), size, a)
        }
        (Some(at), None) => {
            let wanted = pos2(at.x - size.x / 2.0, at.y - size.y - GAP);
            pos2(
                wanted.x.max(0.0),
                if wanted.y < 0.0 { at.y + GAP } else { wanted.y },
            )
        }
        (None, Some(a)) => pos2(end(a.min.x, a.max.x), a.max.y - size.y - GAP),
        (None, None) => {
            let right = screen
                .filter(|s| s.x > size.x)
                .map_or(size.x + GAP, |s| s.x);
            pos2(end(0.0, right), TOP_BAR)
        }
    }
    .round()
}

/// `item` as a row: its progress as the progress window gives it.
fn row(item: &Item) -> Row {
    let done = model::size3(item.downloaded);
    let mut parts = vec![match item.size.filter(|s| *s > 0) {
        Some(size) => crate::i18n::fmt(
            crate::i18n::t("{done} of {size}"),
            &[("done", &done), ("size", &model::size3(size))],
        ),
        None => done,
    }];
    if item.live.is_some() {
        parts.push(model::rate(item.rate()));
        let left = item.time_left();
        if !left.is_empty() {
            parts.push(left);
        }
    }
    let live = item.live_state();
    Row {
        id: item.id,
        name: item.file_name.clone(),
        fraction: item.fraction(),
        line: parts.join(" · "),
        status: item.status,
        live,
        resumable: item.resumable,
        live_stream: live.is_some()
            || matches!(item.video_helper(), Some(mdm_tools::streams::Helper::Live)),
    }
}

/// "And 3 more": the downloads running past the five listed.
fn more(n: usize) -> String {
    // i18n: the notification-area icon's panel (#1365), under the downloads it lists: how many others are running; {n} a number
    crate::i18n::tn(n as u64, "And {n} more", "And {n} more")
}

/// The panel's contents at `o`: its title and ×, each download, and the button to the main
/// window.
fn draw(ui: &mut egui::Ui, p: &egui::Painter, o: Pos2, rows: &[Row], events: &mut Vec<Event>) {
    winui::text_font(
        p,
        winui::tahoma_bold(),
        o.x + 12.0,
        o.y + 20.0,
        title(),
        winui::TEXT(),
    );
    // Its ×: the way to close it that never needs the focus (see the module's comment).
    let close = r(o, W - 8.0 - 24.0, 4.0, 24.0, 24.0);
    if winui::close_button(ui, close, "tray-panel-close", "Close").clicked() {
        events.push(Event::Close);
    }
    let mut y = HEAD;
    winui::etched_h(p, o.x + 8.0, o.y + y - 2.0, W - 16.0);
    // Pause and Cancel as wide as their labels in this language: Pause laid out for the widest
    // of the labels it can show, so it keeps its size as its label changes.
    let live_stream = rows.iter().any(|r| r.live_stream);
    let pause_for = winui::widest_label(p, crate::progress::pause_labels(live_stream));
    let buttons = winui::button_row(
        p,
        &[
            (pause_for, W - 12.0 - 73.0 - 6.0 - 73.0, 73.0),
            ("Cancel", W - 12.0 - 73.0, 73.0),
        ],
        6.0,
        W / 2.0,
        160.0,
    );
    for row in rows.iter().take(MOST) {
        draw_row(ui, p, o, y, row, &buttons, events);
        y += ROW;
        winui::etched_h(p, o.x + 8.0, o.y + y - 2.0, W - 16.0);
    }
    let note = if rows.is_empty() {
        // i18n: the notification-area icon's panel (#1365) when no download is running
        Some(crate::i18n::t("No downloads are running").to_owned())
    } else if rows.len() > MOST {
        Some(more(rows.len() - MOST))
    } else {
        None
    };
    if let Some(note) = note {
        winui::fit(
            ui,
            p,
            o.x + 12.0,
            o.y + y + 19.0,
            &note,
            winui::MUTED(),
            o.x + W - 12.0,
            // Its own id for each of the two notes (nothing running, or how many more).
            ("tray-panel-note", rows.is_empty()),
        );
        y += NOTE;
    }
    // i18n: the notification-area icon's panel (#1365): its button that opens the main window (also the tray menu's first item)
    let open = crate::i18n::mark("Open Mister Download Manager");
    let rc = winui::button_rect_for(
        p,
        r(o, W - 12.0 - 200.0, y + 11.0, 200.0, 23.0),
        open,
        W - 24.0,
    );
    if winui::button(ui, rc, open, ButtonKind::Default, false, None).clicked() {
        events.push(Event::OpenMain);
    }
}

/// One download's row, from `y` down: its name, its bar with the progress window's Pause (or
/// Resume, Stop and save) and Cancel beside it at `buttons` (English x, width), and how far it is
/// under them; read to screen readers as one line.
fn draw_row(
    ui: &mut egui::Ui,
    p: &egui::Painter,
    o: Pos2,
    y: f32,
    row: &Row,
    buttons: &[(f32, f32)],
    events: &mut Vec<Event>,
) {
    let [(pause_x, pause_w), (cancel_x, cancel_w)] = buttons[..] else {
        return;
    };
    winui::fit(
        ui,
        p,
        o.x + 12.0,
        o.y + y + 16.0,
        &crate::i18n::isolate(&row.name),
        winui::TEXT(),
        o.x + W - 12.0,
        ("tray-panel-name", row.id),
    );
    let bar = r(o, 12.0, y + 24.0, pause_x - 12.0 - 10.0, 14.0);
    winui::meter(p, bar, row.fraction.unwrap_or(0.0) as f32);
    let (label, kind) = crate::progress::pause_button(row.status, row.live, row.resumable);
    let event = if row.status == Status::Downloading {
        Event::Pause(row.id)
    } else {
        Event::Resume(row.id)
    };
    ui.push_id(("tray-panel-row", row.id), |ui| {
        let at = r(o, pause_x, y + 20.0, pause_w, 21.0);
        if winui::button(ui, at, label, kind, false, None).clicked() {
            events.push(event);
        }
        let at = r(o, cancel_x, y + 20.0, cancel_w, 21.0);
        if winui::button(ui, at, "Cancel", ButtonKind::Normal, false, None).clicked() {
            events.push(Event::Cancel(row.id));
        }
    });
    winui::fit(
        ui,
        p,
        o.x + 12.0,
        o.y + y + 54.0,
        &row.line,
        winui::MUTED(),
        o.x + W - 12.0,
        ("tray-panel-line", row.id),
    );
    // The row for screen readers: its name, how far, and the line under the bar.
    let whole = r(o, 0.0, y, W, ROW);
    let resp = ui.interact(
        whole,
        ui.scope_id().with(("tray-panel-read", row.id)),
        egui::Sense::hover(),
    );
    let percent = row
        .fraction
        .map(|f| format!(", {}", crate::i18n::percent(f, 0)))
        .unwrap_or_default();
    winui::expose(
        &resp,
        egui::Role::ListItem,
        &format!("{}{percent}, {}", row.name, row.line),
        true,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over the icon, inside the work area: above a taskbar at the bottom, under one at the top,
    /// beside one at a side; the screen's edge never cuts it.
    #[test]
    fn the_panel_sits_beside_the_taskbar_wherever_it_is() {
        let size = vec2(380.0, 200.0);
        let at = |x, y| Some(pos2(x, y));
        // A 1920 × 1080 screen, the taskbar at the bottom (40 tall): the icon near the right.
        let bottom = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1040.0));
        let p = place(at(1800.0, 1060.0), size, Some(bottom), None);
        assert_eq!(p, pos2(1920.0 - 380.0, 1040.0 - 200.0));
        // In the middle of the taskbar: centred over the click, above the taskbar.
        let p = place(at(960.0, 1060.0), size, Some(bottom), None);
        assert_eq!(p, pos2(770.0, 840.0));
        // The taskbar at the top: under it.
        let top = Rect::from_min_max(pos2(0.0, 40.0), pos2(1920.0, 1080.0));
        assert_eq!(place(at(1800.0, 20.0), size, Some(top), None).y, 40.0);
        // At the left: beside it.
        let left = Rect::from_min_max(pos2(60.0, 0.0), pos2(1920.0, 1080.0));
        assert_eq!(place(at(30.0, 1000.0), size, Some(left), None).x, 60.0);
        // No work area (macOS's menu bar): under the click when there is no room above.
        assert_eq!(
            place(at(1500.0, 12.0), size, None, None),
            pos2(1310.0, 20.0)
        );
        assert_eq!(place(at(100.0, 900.0), size, None, None), pos2(0.0, 692.0));
    }

    /// No place for the click (a tray that sent 0, 0): never the screen's top-left corner, but
    /// the work area's bottom corner, or with none the top corner under a top bar.
    #[test]
    fn with_no_place_for_the_click_it_goes_to_the_trays_corner() {
        let size = vec2(380.0, 200.0);
        let bottom = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1040.0));
        assert_eq!(
            place(None, size, Some(bottom), None),
            pos2(1920.0 - 380.0 - GAP, 1040.0 - 200.0 - GAP)
        );
        assert_eq!(
            place(None, size, None, Some(vec2(2560.0, 1440.0))),
            pos2(2560.0 - 380.0 - GAP, TOP_BAR)
        );
        // Not even the screen's size: still on the screen.
        assert_eq!(place(None, size, None, None), pos2(0.0, TOP_BAR));
    }

    /// Room for the downloads listed (five at most), and a line when none runs or more do.
    #[test]
    fn the_panel_grows_with_its_downloads_up_to_five() {
        assert_eq!(height(0), HEAD + NOTE + FOOT);
        assert_eq!(height(1), HEAD + ROW + FOOT);
        assert_eq!(height(5), HEAD + 5.0 * ROW + FOOT);
        assert_eq!(height(9), HEAD + 5.0 * ROW + NOTE + FOOT);
    }

    fn item(id: u64, size: Option<u64>, downloaded: u64, status: Status) -> Item {
        let mut i = Item::new(
            id,
            "https://x.example/big.iso".parse().unwrap(),
            std::path::PathBuf::from("dl").join("big.iso"),
            crate::model::Category::General,
        );
        (i.size, i.downloaded, i.status) = (size, downloaded, status);
        i
    }

    /// A row says how much of how much has come, or only how much without a size (no share of
    /// it, no bar's fill); a stopped one has no speed or time left, and its button resumes.
    #[test]
    fn a_row_says_what_is_known_of_its_download() {
        let known = row(&item(
            1,
            Some(4 * 1024 * 1024),
            1024 * 1024,
            Status::Stopped,
        ));
        assert_eq!(known.fraction, Some(0.25));
        assert_eq!(known.line, "1.000 MB of 4.000 MB");
        assert_eq!(known.status, Status::Stopped);
        assert!(known.live.is_none() && !known.live_stream);
        let unknown = row(&item(2, None, 3 * 1024, Status::Stopped));
        assert_eq!(unknown.fraction, None);
        assert_eq!(unknown.line, "3.000 KB");
        assert_eq!(
            crate::progress::pause_button(known.status, known.live, known.resumable).0,
            "Resume"
        );
    }

    /// Paused from the panel, a download stays listed (with Resume) until it is resumed,
    /// cancelled or finished; running ones are listed without asking.
    #[test]
    fn a_download_paused_from_it_stays_listed() {
        let mut panel = Panel::new(None);
        let items = vec![
            item(1, Some(100), 10, Status::Stopped),
            item(2, Some(100), 100, Status::Complete),
        ];
        assert!(panel.rows(&items).is_empty());
        panel.paused(1);
        panel.paused(2);
        let ids: Vec<u64> = panel.rows(&items).iter().map(|r| r.id).collect();
        assert_eq!(ids, [1], "a finished one isn't listed");
        panel.forget(1);
        assert!(panel.rows(&items).is_empty());
        // In the list's order, whichever was paused first: a row doesn't move under the pointer.
        let items = vec![
            item(1, Some(100), 10, Status::Stopped),
            item(2, Some(100), 20, Status::Stopped),
        ];
        panel.paused(2);
        panel.paused(1);
        let ids: Vec<u64> = panel.rows(&items).iter().map(|r| r.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    /// The count under the list in every plural form a language has (Arabic has six).
    #[test]
    fn the_count_of_the_others_is_a_plural() {
        assert_eq!(more(1), "And 1 more");
        assert_eq!(more(12), "And 12 more");
    }
}
