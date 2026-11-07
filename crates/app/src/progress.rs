//! The per-download progress dialog, painted to IDM 6.42's pixels
//! (notes/ui-spec/progress-dialog.md). Coordinates are client pixels from the spec.

use egui::{Color32, Pos2, Ui, ViewportBuilder, ViewportId, vec2};
use mdm_engine::Snapshot;
use mdm_engine::download::ConnStatus;

use crate::dialogs::display_path;
use crate::model::{self, Item, Status, size3};
// Laid out in English pixels: `fill` and `px` make each one where it is on screen (bars fill
// from the right in a right-to-left interface), `text` starts at an English pen.
use crate::winui::{self, ButtonKind, fill_l as fill, px_l as px, r, text};

use crate::winui::caption::CaptionButton;

/// The drawn title bar's height (#63): Windows only, where the frame is ours
/// (win32::progress_frame); Linux and macOS keep the system's decorations, which take no buttons
/// of ours (Wayland even places them itself), so the hide button sits in the tab row there.
const TITLE_H: f32 = if cfg!(windows) {
    crate::winui::caption::CAPTION_H
} else {
    0.0
};

/// The frame applied to a progress window (Windows): its client size, pixels per point × 1000
/// and its system menu's "Hide to the notification area" (translated, when there is one).
type Framed = ([f32; 2], u32, Option<String>);

/// The title bar's buttons, from the window's end: Close, Maximize (greyed: a fixed size, as
/// IDM's), Minimize, and with a tray our "hide to the notification area". None where the system
/// draws the title bar.
fn caption_buttons(tray: bool) -> Vec<(CaptionButton, bool)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut b = vec![
        (CaptionButton::Close, true),
        (CaptionButton::Maximize, false),
        (CaptionButton::Minimize, true),
    ];
    if tray {
        b.push((CaptionButton::ToTray, true));
    }
    b
}

/// The title bar where it's ours (Windows), `width` points wide at `o`; the button pressed. Where
/// its buttons and icon are goes to the window's hit test (win32::set_caption_hits), in client
/// pixels, each frame (`hwnd`: the window, once its frame is applied).
fn draw_caption(
    ui: &mut Ui,
    o: Pos2,
    width: f32,
    title: &str,
    buttons: &[(CaptionButton, bool)],
    hwnd: Option<isize>,
) -> Option<CaptionButton> {
    if !cfg!(windows) {
        return None;
    }
    // The one drawn title bar of every window (dialogs.rs), with its hit test.
    crate::dialogs::drawn_title(ui, o, width, title, buttons, hwnd)
}

/// A greyed label (the limits' captions while off).
fn disabled() -> Color32 {
    crate::theme::colors().progress_label_disabled
}

/// The segment / piece bar's 13 rows, top to bottom: (not here yet, here, a connection's
/// start), from the theme (IDM's gradients in the classic look; its dark variant dims the
/// "not here yet" rows, a bright white strip in the dark window).
fn seg_rows() -> impl Iterator<Item = (Color32, Color32, Color32)> {
    let s = &crate::theme::colors().segments;
    (0..13).map(|i| (s.missing[i], s.done[i], s.start[i]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Limiter,
    Completion,
}

impl Progress {
    /// Open on a tab by its English name in lower case: a top tab ("status", "limit", "done") or
    /// a torrent's details tab ("peers"; /show progress:peers, for the docs' screenshots and the
    /// fit audit); any other name leaves it as it is.
    pub fn show_tab(&mut self, name: &str) {
        let top = match name {
            "status" => Some(Tab::Status),
            "limit" => Some(Tab::Limiter),
            "done" => Some(Tab::Completion),
            _ => None,
        };
        if let Some(t) = top {
            self.tab = t;
            return;
        }
        let tab = match name {
            "files" => TTab::Files,
            "peers" => TTab::Peers,
            "trackers" => TTab::Trackers,
            "pieces" => TTab::Pieces,
            "general" => TTab::General,
            _ => return,
        };
        self.ttab = tab;
        self.details = true;
    }
}

/// A torrent's details tabs (M15, D-014), in the place of the connections table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TTab {
    Files,
    Peers,
    Trackers,
    Pieces,
    General,
}

/// Why "Hang up modem when done" is greyed off Windows.
pub const NO_DIAL_UP: &str = crate::i18n::mark("This system has no dial-up connections to hang up");

pub struct Progress {
    pub id: u64,
    details: bool,
    tab: Tab,
    ttab: TTab,
    limiter: bool,
    /// KBytes/sec typed on the Speed Limiter tab.
    limit_kb: String,
    /// The tab has been initialised from the download's current limit.
    limit_init: bool,
    /// A limit change to report to the app (`Action::Limit`), and whether to remember it.
    pending_limit: Option<(Option<u64>, bool)>,
    remember: bool,
    /// A torrent's own upload limit on the Speed Limiter tab: on, and its KBytes/sec box.
    up_on: bool,
    up_kb: String,
    /// An upload limit change to report (`Action::UploadLimit`).
    pending_upload: Option<Option<u64>>,
    /// Files ticked or unticked in a torrent's Files tab, to report (`Action::Choose`).
    pending_choose: Option<Action>,
    show_complete: bool,
    /// Torrent settings' "Keep sharing torrents after they finish" (the General tab says so).
    pub keep_sharing: bool,
    /// The torrent session (the app's), for a finished torrent still shared without a `Run`:
    /// its tabs read the session directly.
    pub host: Option<crate::torrents::Host>,
    /// Hidden in the notification area (#63): not shown, its tab and choices kept for when it
    /// comes back (the tray's "Downloads in progress", or the download double-clicked).
    pub hidden: bool,
    /// The window has been brought to the front once, when it opened; a later show (from the
    /// tray) leaves the focus where the person put it.
    raised: bool,
    /// A tray shows MDM's icon, so the window can be hidden there (set by the app each frame).
    pub tray: bool,
    /// Options → General → "Minimize to the notification area": Minimize hides it there too.
    pub minimize_to_tray: bool,
    /// Options on completion, used when "Show download complete dialog" is off.
    hang_up: bool,
    exit: bool,
    turn_off_on: bool,
    turn_off: usize,
    force: bool,
}

pub enum Action {
    None,
    Pause,
    Resume,
    /// Stop the download and close the dialog.
    Cancel,
    /// Set this download's speed limit (bytes/s, None = off).
    /// This download's limit (None: unlimited); `true`: remember it for the file.
    Limit(Option<u64>, bool),
    /// A torrent's own upload limit (bytes/s, None: off), kept with it (`TorrentSpec`).
    UploadLimit(Option<u64>),
    /// A torrent's files to download from now on (#39, its Files tab): indexes into its list,
    /// whether that is every file, and their size together.
    Choose {
        files: Vec<usize>,
        all: bool,
        size: u64,
    },
    /// Close the dialog, keep downloading.
    Close,
    /// Hide the dialog in the notification area, keep downloading (#63).
    Hide,
}

impl Progress {
    /// `show_complete`: Options' "Show download complete dialog", this file's first state.
    pub fn new(id: u64, show_complete: bool) -> Self {
        Progress {
            id,
            details: true,
            tab: Tab::Status,
            ttab: TTab::Files,
            limiter: false,
            limit_kb: "10".into(),
            limit_init: false,
            pending_limit: None,
            remember: false,
            up_on: false,
            up_kb: "50".into(),
            pending_upload: None,
            pending_choose: None,
            show_complete,
            keep_sharing: false,
            host: None,
            hidden: false,
            raised: false,
            tray: false,
            minimize_to_tray: false,
            hang_up: false,
            exit: false,
            turn_off_on: false,
            turn_off: 0,
            force: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, item: &Item, snap: Option<&Snapshot>) -> Action {
        crate::crash::window("progress");
        let frac = item
            .size
            .filter(|t| *t > 0)
            .map_or(0.0, |t| (item.downloaded as f64 / t as f64).clamp(0.0, 1.0));
        // IDM truncates the percentage in the title (44.51 % → "44%").
        // The name keeps its own direction (an Arabic one doesn't pull the percentage after it).
        let title = format!(
            "{}% {}",
            (frac * 100.0).floor() as u32,
            crate::i18n::isolate(&item.file_name)
        );
        let body = if self.details {
            [548.0, 412.0 + torrent_drop(item)]
        } else {
            [548.0, 241.0]
        };
        // Windows: the title bar is ours (#63), drawn in the client above IDM's layout, so the
        // hide button sits in it, left of Minimize, as in IDM. Elsewhere the system draws it.
        let client = [body[0], body[1] + TITLE_H];
        // As large as the screen allows at the interface size (#262): the rest scrolls.
        let shown = crate::dialogs::shown_size(ctx, client);
        let vid = ViewportId::from_hash_of(("progress", self.id));
        let buttons = caption_buttons(self.tray);
        let key = egui::Id::unique(("progress-pos", self.id));
        let title_now = title.clone();
        // Hidden until dialogs::keep_theme has given it its theme.
        let mut builder = crate::quiet(ViewportBuilder::default())
            .with_visible(false)
            .with_title(title)
            .with_inner_size(shown)
            // IDM's progress window has 2 px borders and a fixed size. On Windows the sizing
            // frame is asked for and win32::progress_frame trims its borders, disables resizing
            // and gives the caption to the client, which draws it; elsewhere the window is simply
            // not resizable (its contents are laid out for one size: Show/Hide details switches
            // between two).
            .with_resizable(cfg!(windows))
            .with_icon(crate::app_icon())
            .with_maximize_button(false);
        if let Some(p) = crate::dialogs::place(ctx, key, [548.0, 412.0]) {
            builder = builder.with_position(p);
        }
        // Minimize hides it in the notification area when asked (and a tray can bring it back).
        let minimize_hides = self.tray && self.minimize_to_tray;
        // The window, once its frame is applied (Windows: its caption parts go to its hit test).
        let hwnd = ctx.data(|d| d.get_temp::<isize>(key.with("hwnd")));
        let action = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            // Painted in the pass that closes it too (see dialogs::close_requested).
            let close = crate::dialogs::close_requested(ui);
            if !close && minimize_hides && ui.ctx().input(|i| i.viewport().minimized == Some(true))
            {
                return Action::Hide;
            }
            let action = egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(winui::BG()))
                .show(ui, |ui| {
                    let o = ui.max_rect().min;
                    // The window's width, laid out from the right in a right-to-left
                    // interface (the title bar; the layout under it has its own).
                    let _surface = winui::Surface::enter(o.x, o.x + shown[0]);
                    let caption = draw_caption(ui, o, shown[0], &title_now, &buttons, hwnd);
                    // The layout under the title bar, scrolled when the window is smaller.
                    let below = egui::Rect::from_min_max(o + vec2(0.0, TITLE_H), ui.max_rect().max);
                    let mut rest = ui.new_child(egui::UiBuilder::new().max_rect(below));
                    let action = crate::dialogs::scroll(&mut rest, key, body, |ui, o| {
                        let _surface = winui::Surface::enter(o.x, o.x + body[0]);
                        self.body(ui, o, item, snap, frac)
                    });
                    // The system menu's "Hide to the notification area" (Alt+Space).
                    #[cfg(windows)]
                    if let Some(h) = hwnd
                        && crate::win32::take_hide_asked(h)
                    {
                        return Action::Hide;
                    }
                    match caption {
                        Some(CaptionButton::Close) => Action::Close,
                        Some(CaptionButton::ToTray) => Action::Hide,
                        Some(CaptionButton::Minimize) if minimize_hides => Action::Hide,
                        Some(CaptionButton::Minimize) => {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            action
                        }
                        _ => action,
                    }
                })
                .inner;
            if close {
                return Action::Close;
            }
            // Escape hides the window (the download goes on), as the × does.
            if matches!(action, Action::None) && winui::escape_pressed(ui) {
                Action::Close
            } else {
                action
            }
        });
        // Elsewhere the window manager is asked for its size at a new interface size (#262).
        crate::dialogs::follow_scale(ctx, vid, key, shown);
        // Apply the frame while the window is still hidden, and again whenever the client size
        // (details), the scale (another display, the interface size) or the system menu's item
        // changes.
        #[cfg(windows)]
        {
            let scale = ctx.input_for(vid, |i| i.pixels_per_point);
            let hide = buttons
                .iter()
                .any(|(b, _)| *b == CaptionButton::ToTray)
                .then(|| crate::i18n::t("Hide to the notification area").to_owned());
            let want: Framed = (shown, (scale * 1000.0).round() as u32, hide);
            if ctx
                .data(|d| d.get_temp::<Framed>(key.with("framed")))
                .as_ref()
                != Some(&want)
                && let Some(h) =
                    crate::win32::thin_frame(&title_now, shown, scale, want.2.as_deref())
            {
                ctx.data_mut(|d| {
                    d.insert_temp(key.with("framed"), want);
                    d.insert_temp(key.with("hwnd"), h);
                });
            }
        }
        let closing = matches!(action, Action::Close | Action::Cancel | Action::Hide);
        crate::dialogs::keep_theme(
            ctx,
            vid,
            key,
            &title_now,
            crate::theme::dark_surface(),
            !closing,
        );
        // A download started from the browser leaves the browser in front; the progress window
        // must come over it (IDM's does), or the person sees no sign that it started. Only when
        // it opens: a hidden one brought back from the tray leaves the focus where it is.
        if !self.raised && !closing {
            crate::dialogs::raise(ctx, ViewportId::from_hash_of(("progress", self.id)));
            self.raised = true;
        }
        let action = match (action, self.pending_limit.take()) {
            (Action::None, Some((limit, remember))) => Action::Limit(limit, remember),
            (a, _) => a,
        };
        // An upload change waits a frame when a download one goes first (one action a frame);
        // so does a change of files.
        let action = match action {
            Action::None => self
                .pending_upload
                .take()
                .map_or(Action::None, Action::UploadLimit),
            a => a,
        };
        let action = match action {
            Action::None => self.pending_choose.take().unwrap_or(Action::None),
            a => a,
        };
        if matches!(action, Action::Close | Action::Cancel) {
            crate::dialogs::forget_place(ctx, key);
        }
        // Gone (hidden keeps its place): the next window gets its thin frame again.
        if closing {
            #[cfg(windows)]
            if let Some(h) = hwnd {
                crate::win32::forget_caption(h);
            }
            ctx.data_mut(|d| {
                d.remove::<Framed>(key.with("framed"));
                d.remove::<isize>(key.with("hwnd"));
            });
        }
        action
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        item: &Item,
        snap: Option<&Snapshot>,
        frac: f64,
    ) -> Action {
        let p = ui.painter().clone();
        let hide = self.tabs(ui, o);
        match self.tab {
            Tab::Status => status_page(ui, o, item, snap, frac),
            Tab::Limiter => {
                if self.limiter_page(ui, o, item) {
                    self.tab = Tab::Status;
                }
            }
            Tab::Completion => {
                if self.completion_page(ui, o, item) {
                    self.tab = Tab::Status;
                }
            }
        }

        // The progress bar: IDM's green on the shared, themed track.
        winui::meter(&p, r(o, 17.0, 180.0, 513.0, 16.0), frac as f32);

        let mut action = if hide { Action::Hide } else { Action::None };
        let details_label = if self.details {
            crate::i18n::mark("<< Hide details")
        } else {
            crate::i18n::mark("Show details >>")
        };
        let other_tab = self.tab != Tab::Status;
        let kind = if other_tab {
            ButtonKind::Default
        } else {
            ButtonKind::Normal
        };
        // The row as wide as its labels in this language need (#192), each laid out for the
        // longer of its two labels so it keeps its size when they change; Cancel stays at the
        // window's edge.
        let [details_at, pause_at, cancel_at] = winui::button_row(
            &p,
            &[
                (
                    winui::widest_label(&p, &["<< Hide details", "Show details >>"]),
                    30.0,
                    138.0,
                ),
                (winui::widest_label(&p, &["Pause", "Resume"]), 311.0, 82.0),
                ("Cancel", 422.0, 73.0),
            ],
            8.0,
            30.0,
            200.0,
        )[..] else {
            unreachable!("three buttons")
        };
        if winui::button(
            ui,
            r(o, details_at.0, 206.0, details_at.1, 22.0),
            details_label,
            kind,
            false,
            None,
        )
        .clicked()
        {
            self.details = !self.details;
        }
        let running = item.status == Status::Downloading;
        let can_resume = matches!(item.status, Status::Stopped | Status::Error);
        let (label, pkind) = if running {
            // i18n: verb: pause the download
            (crate::i18n::mark("Pause"), ButtonKind::Normal)
        } else if can_resume {
            // i18n: verb: continue the download
            (crate::i18n::mark("Resume"), ButtonKind::Normal)
        } else {
            (crate::i18n::mark("Pause"), ButtonKind::Disabled)
        };
        if winui::button(
            ui,
            r(o, pause_at.0, 206.0, pause_at.1, 22.0),
            label,
            pkind,
            false,
            None,
        )
        .clicked()
        {
            action = if running {
                Action::Pause
            } else {
                Action::Resume
            };
        }
        if winui::button(
            ui,
            r(o, cancel_at.0, 206.0, cancel_at.1, 22.0),
            "Cancel",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            action = Action::Cancel;
        }

        if self.details && item.torrent.is_some() {
            self.torrent_details(ui, &p, o + egui::vec2(0.0, torrent_drop(item)), item);
        } else if self.details {
            winui::text_in_window(
                ui,
                &p,
                o.x + 137.0,
                o.y + 242.0,
                crate::i18n::t("Start positions and download progress by connections"),
                winui::TEXT(),
            );
            segment_bar(&p, o, item, snap);
            connections(ui, &p, o, snap);
        }
        action
    }

    /// Classic 3-D tabs over a raised page frame (spec "Tab strip"); with a tray, the "hide to
    /// the notification area" button at the row's end. True when that button is pressed.
    fn tabs(&mut self, ui: &mut Ui, o: Pos2) -> bool {
        // The shared tab strip (the torrent tabs' too): one look in both themes, one Tab stop with
        // the arrows, sized to the translated labels. It had two of its own (classic 3-D in dark).
        let tabs = [
            // i18n: tab title: how the download is going
            (Tab::Status, crate::i18n::mark("Download status")),
            // i18n: tab title: the tool that limits the download speed
            (Tab::Limiter, crate::i18n::mark("Speed Limiter")),
            // i18n: tab title: what happens when the download finishes
            (Tab::Completion, crate::i18n::mark("Options on completion")),
        ];
        let p = ui.painter().clone();
        let english: Vec<&str> = tabs.iter().map(|t| t.1).collect();
        // The hide button sits here where the system draws the title bar (Linux, macOS; on
        // Windows it's in ours); the tabs end before it (long translations shorten first).
        let tab_button = self.tray && !cfg!(windows);
        let right = if tab_button { 500.0 } else { 525.0 };
        let light: Vec<winui::LightTab> = winui::tab_row(&p, &english, 20.0, right, 2.0, 8.0)
            .into_iter()
            .zip(tabs.iter())
            .map(|((x0, x1, _), (_, label))| winui::LightTab {
                label,
                sel: (x0 - 2.0, x1 + 2.0),
                unsel: (x0, x1),
            })
            .collect();
        let current = tabs.iter().position(|t| t.0 == self.tab).unwrap_or(0);
        // The page: x 18..529, y 28..171 (tabs_light's pane starts 20 px under its top).
        if let Some(i) = winui::tabs_light(ui, o, r(o, 18.0, 8.0, 514.0, 165.0), &light, current) {
            self.tab = tabs[i].0;
        }
        // MDM's (#63): IDM puts this in its title bar, which here is the system's own (it takes
        // no buttons of ours), so it sits at the tab row's end, over the page's corner.
        tab_button
            && winui::to_tray_button(
                ui,
                r(o, 507.0, 6.0, 22.0, 20.0),
                "progress-hide",
                // i18n: tooltip of a button in a download's progress window: hides the window to the notification area (system tray); the download goes on
                crate::i18n::mark("Hide to the notification area"),
            )
            .clicked()
    }

    /// The Speed Limiter tab, live for this download. Returns true when "Hide tab" is pressed.
    fn limiter_page(&mut self, ui: &mut Ui, o: Pos2, item: &Item) -> bool {
        let p = ui.painter().clone();
        if !self.limit_init {
            if let Some(rate) = item.torrent.as_ref().and_then(|t| t.upload_limit) {
                self.up_on = true;
                self.up_kb = (rate / 1024).max(1).to_string();
            }
            if let Some(rate) = item.live.as_ref().and_then(|l| l.dl.speed_limit()) {
                self.limiter = true;
                self.limit_kb = (rate / 1024).max(1).to_string();
            }
            // A remembered limit: the box is ticked (and the limit shows even while stopped).
            if let Some(rate) = item.own_limit {
                self.limiter = true;
                self.limit_kb = (rate / 1024).max(1).to_string();
                self.remember = true;
            }
            self.limit_init = true;
        }
        text(
            &p,
            o.x + 33.0,
            o.y + 51.0,
            crate::i18n::t("Transfer rate"),
            winui::TEXT(),
        );
        let rate = if item.status == Status::Downloading {
            model::rate(item.rate())
        } else {
            String::new()
        };
        text(&p, o.x + 136.0, o.y + 51.0, &rate, winui::TEXT());
        let was_on = self.limiter;
        // A torrent's upload limit takes the right half (x 290): the download side's captions
        // stop before it.
        let torrent = item.torrent.is_some();
        winui::checkbox_until(
            ui,
            o + vec2(33.0, 64.0),
            &mut self.limiter,
            "Use Speed Limiter",
            true,
            torrent.then_some(286.0),
        );
        let label = if self.limiter {
            winui::TEXT()
        } else {
            disabled()
        };
        winui::fit(
            ui,
            &p,
            o.x + 33.0,
            o.y + 92.0,
            crate::i18n::t("Maximum download speed:"),
            label,
            o.x + if torrent { 284.0 } else { 525.0 },
            ("limit-label", self.id),
        );
        let rate_box = winui::edit_right(
            ui,
            r(o, 33.0, 100.0, 77.0, 23.0),
            &mut self.limit_kb,
            self.limiter,
        );
        self.limit_kb.retain(|c| c.is_ascii_digit());
        // A typed rate applies once typed (focus leaves, Enter), not at each digit (typing 100
        // would throttle to 1, then 10 KB/s on the way). A rate a screen reader or UI Automation
        // sets (Set value) applies at once: there is no typing to wait for, and often no focus
        // to leave, so it never applied.
        let typed = rate_box.lost_focus()
            || (rate_box.has_focus() && winui::enter_pressed(ui))
            || winui::value_set_by_assistive(&rate_box);
        // i18n: unit after an input box: kilobytes per second
        text(
            &p,
            o.x + 118.0,
            o.y + 113.0,
            crate::i18n::t("KBytes/sec"),
            label,
        );
        let remembered = self.remember;
        let remember_box = winui::checkbox(ui, o + vec2(33.0, 134.0), &mut self.remember, "", true);
        // Its caption is drawn apart (two lines, below): screen readers get it as its name.
        let name = format!(
            "{} {}",
            crate::i18n::t("Remember Speed Limiter settings for this file"),
            crate::i18n::t("on download stop/resume")
        );
        winui::expose(
            &remember_box,
            egui::Role::CheckBox,
            &name,
            true,
            Some(self.remember),
        );
        // Its caption is two lines drawn below; a click on them ticks it too.
        winui::caption_click(
            ui,
            r(o, 51.0, 127.0, 450.0, 27.0),
            "remember-limit",
            &mut self.remember,
            true,
        );
        // An empty or zero rate isn't "unlimited" while the box is ticked: the limit stays
        // as it was until a rate is typed.
        let kb: u64 = self.limit_kb.parse().unwrap_or(0);
        if (was_on != self.limiter || (self.limiter && typed) || remembered != self.remember)
            && !(self.limiter && kb == 0)
        {
            let limit = self.limiter.then_some(kb * 1024);
            self.pending_limit = Some((limit, self.remember));
        }
        text(
            &p,
            o.x + 51.0,
            o.y + 137.0,
            crate::i18n::t("Remember Speed Limiter settings for this file"),
            winui::TEXT(),
        );
        text(
            &p,
            o.x + 51.0,
            o.y + 150.0,
            crate::i18n::t("on download stop/resume"),
            winui::TEXT(),
        );
        if item.torrent.is_some() {
            self.upload_limit(ui, &p, o);
        }
        hide_tab(ui, o)
    }

    /// A torrent's own upload limit, right of the download one (#39): kept with the torrent,
    /// so it applies again whenever it runs or shares.
    fn upload_limit(&mut self, ui: &mut Ui, p: &egui::Painter, o: Pos2) {
        let was_on = self.up_on;
        winui::checkbox_until(
            ui,
            o + vec2(290.0, 64.0),
            &mut self.up_on,
            // i18n: a torrent's progress window, Speed Limiter tab: a limit for what this torrent sends to others
            "Limit uploading",
            true,
            Some(525.0),
        );
        let label = if self.up_on {
            winui::TEXT()
        } else {
            disabled()
        };
        winui::fit(
            ui,
            p,
            o.x + 290.0,
            o.y + 92.0,
            // i18n: a torrent's progress window, Speed Limiter tab: label above the upload speed box
            crate::i18n::t("Maximum upload speed:"),
            label,
            o.x + 525.0,
            ("upload-label", self.id),
        );
        let rate_box = winui::edit_right(
            ui,
            r(o, 290.0, 100.0, 77.0, 23.0),
            &mut self.up_kb,
            self.up_on,
        );
        self.up_kb.retain(|c| c.is_ascii_digit());
        let typed = rate_box.lost_focus()
            || (rate_box.has_focus() && winui::enter_pressed(ui))
            || winui::value_set_by_assistive(&rate_box);
        text(
            p,
            o.x + 375.0,
            o.y + 113.0,
            crate::i18n::t("KBytes/sec"),
            label,
        );
        let kb: u64 = self.up_kb.parse().unwrap_or(0);
        if (was_on != self.up_on || (self.up_on && typed)) && !(self.up_on && kb == 0) {
            self.pending_upload = Some(self.up_on.then_some(kb * 1024));
        }
    }

    /// Options on completion → "Show download complete dialog" for this file.
    pub fn shows_complete(&self) -> bool {
        self.show_complete
    }

    /// Options on completion's other settings, when they apply (the complete dialog is off).
    pub fn when_done(&self) -> Option<crate::queue::WhenDone> {
        (!self.show_complete).then(|| crate::queue::WhenDone {
            hang_up: self.hang_up,
            exit: self.exit,
            turn_off: self
                .turn_off_on
                .then(|| crate::queue::TurnOff::ALL[self.turn_off]),
            force: self.turn_off_on && self.force,
        })
    }

    /// Returns true when "Hide tab" is pressed.
    fn completion_page(&mut self, ui: &mut Ui, o: Pos2, item: &Item) -> bool {
        let p = ui.painter().clone();
        // The label as wide as its language makes it (half the row at most), then the path to
        // the row's end, shortened in the middle so the file's name stays in sight (#62).
        let (left, right) = (o.x + 33.0, o.x + 33.0 + 474.0);
        let label = winui::fit(
            ui,
            &p,
            left,
            o.y + 45.0,
            crate::i18n::t("Save To:"),
            winui::TEXT(),
            left + 237.0,
            "save-to-label",
        );
        // From the label's width: its rect is mirrored in a right-to-left interface.
        let at = (left + label.width() + 6.0).max(o.x + 90.0);
        crate::fileicons::paint_at(
            ui,
            &p,
            crate::fileicons::Kind::File(&item.file_name),
            egui::pos2(at, o.y + 33.0),
            16.0,
        );
        winui::fit_middle(
            ui,
            &p,
            at + crate::fileicons::INDENT,
            o.y + 45.0,
            &display_path(&item.dest),
            winui::TEXT(),
            right,
            "save-to-path",
        );
        winui::checkbox(
            ui,
            o + vec2(33.0, 55.0),
            &mut self.show_complete,
            "Show download complete dialog",
            true,
        );
        winui::etched_h(&p, o.x + 33.0, o.y + 74.0, 474.0);
        text(
            &p,
            o.x + 33.0,
            o.y + 89.0,
            crate::i18n::t(
                "These settings are unavailable when \"Show download complete dialog\" is turned on",
            ),
            winui::TEXT(),
        );
        // Used when the complete dialog is off (it asks what to do itself).
        let on = !self.show_complete;
        // Dial-up is hung up through Windows' rasdial; macOS and Linux have no such command.
        let hang_up = winui::checkbox(
            ui,
            o + vec2(33.0, 96.0),
            &mut self.hang_up,
            "Hang up modem when done",
            on && cfg!(windows),
        );
        winui::why_disabled(hang_up, on && !cfg!(windows), NO_DIAL_UP);
        winui::checkbox(
            ui,
            o + vec2(33.0, 114.0),
            &mut self.exit,
            "Exit Mister Download Manager when done",
            on,
        );
        winui::checkbox_until(
            ui,
            o + vec2(33.0, 132.0),
            &mut self.turn_off_on,
            "Turn off computer when done",
            on,
            Some(o.x + 267.0),
        );
        winui::checkbox(
            ui,
            o + vec2(52.0, 148.0),
            &mut self.force,
            "Force processes to terminate",
            on && self.turn_off_on,
        );
        if let Some(i) = winui::combo_light(
            ui,
            r(o, 267.0, 134.0, 119.0, 21.0),
            &crate::queue::TurnOff::LABELS,
            self.turn_off,
            on && self.turn_off_on,
            "progress-turn-off",
        ) {
            self.turn_off = i;
        }
        hide_tab(ui, o)
    }
}

/// "Hide tab" (401,135,96,21) with its 1 px #f9f9f9 focus ring.
fn hide_tab(ui: &mut Ui, o: Pos2) -> bool {
    // A longer translation ("Registerkarte ausblenden") widens it to the left, into the free
    // room right of the Remember caption (which ends before x 300).
    let rc = winui::button_rect_for(
        ui.painter(),
        r(o, 401.0, 135.0, 96.0, 21.0),
        "Hide tab",
        190.0,
    );
    winui::button(ui, rc, "Hide tab", ButtonKind::Normal, false, None).clicked()
}

/// Right end for text on the tab page (its frame is x 18..529).
const PAGE_TEXT_RIGHT: f32 = 520.0;

fn status_page(ui: &Ui, o: Pos2, item: &Item, snap: Option<&Snapshot>, frac: f64) {
    let p = &ui.painter().clone();
    // Long values are cut with "..." at the page edge and shown whole on hover.
    let fit = |key: &str, x: f32, base: f32, s: &str, color| {
        let (rect, cut) = winui::text_fit(p, o.x + x, o.y + base, s, color, o.x + PAGE_TEXT_RIGHT);
        winui::copyable(ui, rect, key, s, cut);
    };
    let status = match item.status {
        // A torrent its debrid service is downloading (#44).
        Status::Downloading if item.at_service().is_some() => item.at_service().unwrap_or_default(),
        Status::Downloading => {
            // A video (yt-dlp) has no engine connections: data arriving is the sign.
            let receiving = snap.is_some_and(|s| {
                s.connections
                    .iter()
                    .any(|c| matches!(c.status, ConnStatus::Receiving))
            }) || ((item.video.is_some() || item.torrent.is_some())
                && item.rate() > 0.0);
            if receiving {
                crate::i18n::t("Receiving data...")
            } else {
                crate::i18n::t("Connecting...")
            }
            .to_owned()
        }
        // i18n: a download's status (state it is in, not a command): finished
        Status::Complete => crate::i18n::t("Complete").to_owned(),
        // i18n: a download's status (state it is in, not a command): stopped for now
        Status::Stopped => crate::i18n::t("Paused").to_owned(),
        // i18n: a download's status (state it is in, not a command): waiting in a queue
        Status::Queued => crate::i18n::t("Waiting").to_owned(),
        Status::Error => crate::i18n::fmt(
            // i18n: a download's status: it failed, with the reason
            crate::i18n::t("Error: {error}"),
            &[(
                "error",
                // i18n: the reason a download failed is not known
                item.error.as_deref().unwrap_or(crate::i18n::t("unknown")),
            )],
        ),
    };
    fit(
        "url",
        33.0,
        47.0,
        &crate::i18n::ltr(item.url.as_str()),
        winui::TEXT(),
    );
    let rows: [(&str, f32, f32, String); 6] = [
        // i18n: noun: label before the download's state
        (crate::i18n::mark("Status"), 64.0, 102.0, status),
        (
            crate::i18n::mark("File size"),
            86.0,
            135.0,
            item.size.map_or(String::new(), size3),
        ),
        (
            // i18n: label before the amount received so far
            crate::i18n::mark("Downloaded"),
            102.0,
            135.0,
            if crate::i18n::code() == "en" {
                format!("{} ( {:.2} % )", size3(item.downloaded), frac * 100.0) // not translated: English path (IDM's spacing)
            } else {
                format!(
                    "{} ( {} )",
                    size3(item.downloaded),
                    crate::i18n::percent(frac, 2)
                )
            },
        ),
        (
            crate::i18n::mark("Transfer rate"),
            118.0,
            135.0,
            if item.status == Status::Downloading {
                model::rate(item.rate())
            } else {
                String::new()
            },
        ),
        (
            crate::i18n::mark("Time left"),
            134.0,
            135.0,
            item.time_left(),
        ),
        (
            // i18n: label: whether the server lets a stopped download continue (Yes / No)
            crate::i18n::mark("Resume capability"),
            151.0,
            135.0,
            match item.resumable {
                Some(true) => crate::i18n::t("Yes"),
                Some(false) => crate::i18n::t("No"),
                None => "",
            }
            .to_owned(),
        ),
    ];
    // A value starts 6 px after its label at the least (a long translation); the rows that share
    // the column at 135 (all but Status) keep sharing one.
    let after = |label: &str| 33.0 + winui::text_width(p, crate::i18n::t(label)).ceil() + 6.0;
    let column = rows
        .iter()
        .filter(|r| r.2 == 135.0)
        .map(|r| after(r.0))
        .fold(135.0, f32::max);
    for (label, base, vx, value) in rows {
        let vx = if vx == 135.0 {
            column
        } else {
            vx.max(after(label))
        };
        text(
            p,
            o.x + 33.0,
            o.y + base,
            crate::i18n::t(label),
            winui::TEXT(),
        );
        // Light mode shows the status value in pure blue (light-dialogs.md).
        let vc = if label == "Status" {
            crate::theme::colors().status_value
        } else {
            winui::TEXT()
        };
        fit(label, vx, base, &value, vc);
    }
    // Where its server is (#140): a file download's (a torrent, a video or a gallery has no
    // single server of MDM's own). As the running download sees it, else as its last run did.
    if item.torrent.is_none() && item.video.is_none() && item.gallery.is_none() {
        // i18n: label before where a download's server is (its country and round trip)
        let label = crate::i18n::t("Server");
        text(p, o.x + 33.0, o.y + 167.0, label, winui::TEXT());
        let seen = snap
            .and_then(|s| s.server.as_ref())
            .map(crate::whereis::Seen::from)
            .or_else(|| item.server.clone());
        crate::whereis::line(
            ui,
            o.x + column.max(33.0 + winui::text_width(p, label).ceil() + 6.0),
            o.y + 167.0,
            o.x + PAGE_TEXT_RIGHT,
            seen.as_ref(),
            &format!("progress-{}", item.id),
        );
    }
}

/// How much lower a torrent's details sit than a download's (and how much taller its window
/// is): its tab strip, where the connections caption is, would touch the buttons above.
fn torrent_drop(item: &Item) -> f32 {
    if item.torrent.is_some() { 6.0 } else { 0.0 }
}

impl Progress {
    /// A torrent's details: a tab strip where the connections caption is, the piece map in
    /// the segment bar, and the tab's table below.
    fn torrent_details(&mut self, ui: &mut Ui, p: &egui::Painter, o: Pos2, item: &Item) {
        let d = match &item.live {
            Some(crate::model::Live {
                dl: crate::model::Job::Torrent(run),
                ..
            }) => Some(run.details()),
            // Finished and still shared (no Run): the session has it all the same, so the
            // tabs stay, the General tab's share ratio included.
            _ => self
                .host
                .as_ref()
                .and_then(|h| h.get())
                .zip(item.torrent.as_ref())
                .and_then(|(t, spec)| crate::torrents::details_of(&t, &spec.hash)),
        };
        let tabs = [
            // i18n: tab title: the torrent's files
            (TTab::Files, crate::i18n::mark("Files")),
            // i18n: tab title: the other computers sharing the torrent
            (TTab::Peers, crate::i18n::mark("Peers")),
            // i18n: tab title: the servers that list the peers
            (TTab::Trackers, crate::i18n::mark("Trackers")),
            // i18n: tab title: the parts the torrent is made of
            (TTab::Pieces, crate::i18n::mark("Pieces")),
            // i18n: tab title: general information
            (TTab::General, crate::i18n::mark("General")),
        ];
        // The usual tab control (the Scheduler's and Options' look): tabs sized to their
        // labels from the leading edge, the page under them holding the piece map and the
        // table.
        let english: Vec<&str> = tabs.iter().map(|t| t.1).collect();
        let light: Vec<winui::LightTab> = winui::tab_row(p, &english, 16.0, 530.0, 2.0, 8.0)
            .into_iter()
            .zip(tabs.iter())
            .map(|((x0, x1, _), (_, label))| winui::LightTab {
                label,
                sel: (x0 - 2.0, x1 + 2.0),
                unsel: (x0, x1),
            })
            .collect();
        let current = tabs.iter().position(|t| t.0 == self.ttab).unwrap_or(0);
        if let Some(i) = winui::tabs_light(ui, o, r(o, 14.0, 226.0, 521.0, 180.0), &light, current)
        {
            self.ttab = tabs[i].0;
        }
        let pieces = d.as_ref().map(|d| d.pieces.as_slice()).unwrap_or(&[]);
        piece_bar(p, o, pieces);
        let Some(d) = &d else {
            table(ui, p, o, "t-none", &[(25.0, "")], &[], &[], false, None);
            text(
                p,
                o.x + 25.0,
                o.y + 312.0,
                crate::i18n::t("Shown while the torrent is downloading."),
                winui::TEXT(),
            );
            return;
        };
        match self.ttab {
            TTab::Files => {
                let rows: Vec<Vec<String>> = d
                    .files
                    .iter()
                    .map(|(_, f, on, done)| {
                        let pct = if !on {
                            // i18n: a torrent file the user chose not to download
                            crate::i18n::t("Skipped").to_owned()
                        } else if f.size == 0 {
                            crate::i18n::percent(1.0, 0)
                        } else {
                            crate::i18n::percent(*done as f64 / f.size as f64, 1)
                        };
                        vec![f.path.display().to_string(), model::size(f.size), pct]
                    })
                    .collect();
                let ticks: Vec<bool> = d.files.iter().map(|f| f.2).collect();
                let clicked = table(
                    ui,
                    p,
                    o,
                    "t-files",
                    // i18n: column titles: File (name), Size, Done (how much is downloaded)
                    &[
                        (25.0, crate::i18n::mark("File")),
                        (335.0, crate::i18n::mark("Size")),
                        (430.0, crate::i18n::mark("Done")),
                    ],
                    &[330.0, 425.0],
                    &rows,
                    true,
                    Some(&ticks),
                );
                // A box ticked or unticked (#39): the torrent downloads those files from now
                // on; the last one ticked can't be unticked (a torrent of nothing).
                if let Some(row) = clicked {
                    let mut on: Vec<bool> = ticks.clone();
                    on[row] = !on[row];
                    if on.iter().any(|b| *b) {
                        let chosen = d.files.iter().zip(&on).filter(|(_, b)| **b);
                        self.pending_choose = Some(Action::Choose {
                            files: chosen.clone().map(|(f, _)| f.0).collect(),
                            all: on.iter().all(|b| *b),
                            size: chosen.map(|(f, _)| f.1.size).sum(),
                        });
                    }
                }
            }
            TTab::Peers => {
                let rows: Vec<Vec<String>> = d
                    .peers
                    .iter()
                    .map(|q| {
                        vec![
                            q.addr.clone(),
                            q.client.clone().unwrap_or_default(),
                            model::size(q.fetched),
                            model::size(q.uploaded),
                        ]
                    })
                    .collect();
                table(
                    ui,
                    p,
                    o,
                    "t-peers",
                    &[
                        // i18n: column title: the web address (URL)
                        (25.0, crate::i18n::mark("Address")),
                        // i18n: column title: the peer's torrent program
                        (190.0, crate::i18n::mark("Client")),
                        // i18n: column title: how much came from the peer
                        (340.0, crate::i18n::mark("Received")),
                        // i18n: column title: how much went to the peer
                        (425.0, crate::i18n::mark("Sent")),
                    ],
                    &[185.0, 335.0, 420.0],
                    &rows,
                    false,
                    None,
                );
            }
            TTab::Trackers => {
                let rows: Vec<Vec<String>> = if d.trackers.is_empty() {
                    vec![vec![
                        crate::i18n::t("None: peers come from the DHT and other peers").into(),
                    ]]
                } else {
                    d.trackers.iter().map(|t| vec![t.clone()]).collect()
                };
                // i18n: column title: a server that lists the peers
                let head = [(25.0, crate::i18n::mark("Tracker"))];
                table(ui, p, o, "t-trackers", &head, &[], &rows, false, None);
            }
            TTab::Pieces => {
                let have = pieces.iter().filter(|b| **b).count();
                let head = crate::i18n::fmt(
                    // i18n: {have} and {total} = numbers of pieces; {size} = one piece's size
                    crate::i18n::t("{have} of {total} pieces ({size} each)"),
                    &[
                        ("have", &crate::i18n::integer(have as u64)),
                        ("total", &crate::i18n::integer(pieces.len() as u64)),
                        ("size", &model::size(u64::from(d.piece_length))),
                    ],
                );
                table(
                    ui,
                    p,
                    o,
                    "t-pieces",
                    &[(25.0, head.as_str())],
                    &[],
                    &[],
                    false,
                    None,
                );
                piece_legend(p, o);
                piece_grid(p, o, pieces);
            }
            TTab::General => {
                let st = d.stats.as_ref();
                let have = pieces.iter().filter(|b| **b).count();
                // What it sent in all its runs (#39; librqbit counts only this one), against what
                // it has: the same after a pause or a restart.
                let spec = item.torrent.as_ref();
                let sent = spec.map_or(0, |t| t.uploaded);
                let done = st.map_or(item.downloaded, |s| s.done);
                let ratio = if done > 0 {
                    crate::i18n::decimal(sent as f64 / done as f64, 2)
                } else {
                    crate::i18n::decimal(0.0, 2)
                };
                let shared = spec.map_or(0, |t| t.seeded_secs);
                let hash = item
                    .torrent
                    .as_ref()
                    .map_or(String::new(), |t| t.hash.clone());
                let folder = item
                    .torrent
                    .as_ref()
                    .map_or(String::new(), |t| t.folder.display().to_string());
                let t = |s: &str| crate::i18n::t(s).to_owned();
                let rows = vec![
                    // i18n: the torrent's identifier
                    vec![t("Info hash"), hash],
                    vec![
                        t("Pieces"),
                        crate::i18n::fmt(
                            // i18n: {have} and {total} = numbers of pieces; {size} = one piece's size
                            crate::i18n::t("{have} of {total} ({size} each)"),
                            &[
                                ("have", &crate::i18n::integer(have as u64)),
                                ("total", &crate::i18n::integer(pieces.len() as u64)),
                                ("size", &model::size(u64::from(d.piece_length))),
                            ],
                        ),
                    ],
                    vec![
                        t("Peers"),
                        st.map_or(String::new(), |s| {
                            crate::i18n::fmt(
                                // i18n: {live} = peers connected now; {known} = peers heard of
                                crate::i18n::t("{live} connected, {known} known"),
                                &[
                                    ("live", &crate::i18n::integer(s.peers_live as u64)),
                                    ("known", &crate::i18n::integer(s.peers_seen as u64)),
                                ],
                            )
                        }),
                    ],
                    vec![
                        // i18n: how much was sent to other peers
                        t("Sent"),
                        model::size(sent),
                    ],
                    // i18n: sent divided by received
                    vec![t("Share ratio"), ratio],
                    vec![
                        // i18n: a torrent's General tab: how long it has been shared once finished, in all
                        t("Shared for"),
                        model::time(shared as f64),
                    ],
                    // i18n: label before the folder the torrent is saved in
                    vec![t("Saved in"), folder],
                    vec![
                        // i18n: noun: sharing the torrent with others after it is downloaded
                        t("Sharing"),
                        if self.keep_sharing {
                            // i18n: a torrent's sharing, when Torrent settings keep it on
                            t("Goes on after the download finishes")
                        } else {
                            t("Stops when the download finishes")
                        },
                    ],
                ];
                table(
                    ui,
                    p,
                    o,
                    "t-general",
                    &[
                        (25.0, crate::i18n::mark("Property")),
                        (130.0, crate::i18n::mark("Value")),
                    ],
                    &[125.0],
                    &rows,
                    false,
                    None,
                );
            }
        }
    }
}

/// The segment bar as a piece map: a column is filled when the pieces under it are there.
fn piece_bar(p: &egui::Painter, o: Pos2, pieces: &[bool]) {
    let (x0, y0) = (o.x + 17.0, o.y + 249.0);
    let border = crate::theme::colors().segment_border;
    fill(p, x0, y0, x0 + 512.0, y0, border);
    fill(p, x0, y0 + 14.0, x0 + 512.0, y0 + 14.0, border);
    fill(p, x0, y0, x0, y0 + 14.0, border);
    fill(p, x0 + 512.0, y0, x0 + 512.0, y0 + 14.0, border);
    let ix = o.x + 18.0;
    // Runs of equal columns, painted as one rectangle per row.
    let have = |c: usize| {
        if pieces.is_empty() {
            return false;
        }
        let a = c * pieces.len() / 511;
        let b = ((c + 1) * pieces.len() / 511).max(a + 1).min(pieces.len());
        pieces[a..b].iter().all(|x| *x)
    };
    let mut c = 0usize;
    while c < 511 {
        let on = have(c);
        let mut e = c;
        while e + 1 < 511 && have(e + 1) == on {
            e += 1;
        }
        for (row, cols) in seg_rows().enumerate() {
            let v = if on { cols.1 } else { cols.0 };
            let y = y0 + 1.0 + row as f32;
            fill(p, ix + c as f32, y, ix + e as f32, y, v);
        }
        c = e + 1;
    }
}

/// The Pieces tab's key, at the header's right: have / some / missing.
fn piece_legend(p: &egui::Painter, o: Pos2) {
    let (done, partial, missing) = piece_colors();
    let mut x = o.x + 505.0;
    for (c, label) in [
        // i18n: legend: pieces not downloaded yet
        (missing, crate::i18n::mark("Missing")),
        // i18n: legend: pieces partly downloaded
        (partial, crate::i18n::mark("Some")),
        // i18n: legend: pieces downloaded
        (done, crate::i18n::mark("Have")),
    ] {
        let label = crate::i18n::t(label);
        x -= winui::text_width(p, label);
        text(p, x, o.y + 290.0, label, winui::TEXT());
        x -= 10.0;
        p.rect_filled(
            winui::rect(egui::Rect::from_min_size(
                egui::pos2(x, o.y + 282.0),
                egui::vec2(7.0, 7.0),
            )),
            1.0,
            c,
        );
        x -= 14.0;
    }
}

/// Pieces there, partly there (a square standing for several), missing.
fn piece_colors() -> (Color32, Color32, Color32) {
    let c = crate::theme::colors();
    (c.piece, c.piece_partial, c.piece_missing)
}

/// The Pieces tab: one square per piece (or per group of pieces when there are many).
fn piece_grid(p: &egui::Painter, o: Pos2, pieces: &[bool]) {
    if pieces.is_empty() {
        return;
    }
    let (left, top, w, h) = (o.x + 22.0, o.y + 302.0, 498.0, 94.0);
    let cols = (w / 7.0) as usize;
    let rows = (h / 7.0) as usize;
    let cells = (cols * rows).min(pieces.len());
    let (done, partial, missing) = piece_colors();
    for k in 0..cells {
        let a = k * pieces.len() / cells;
        let b = ((k + 1) * pieces.len() / cells)
            .max(a + 1)
            .min(pieces.len());
        let got = pieces[a..b].iter().filter(|x| **x).count();
        let c = if got == b - a {
            done
        } else if got > 0 {
            partial
        } else {
            missing
        };
        let (x, y) = (
            left + (k % cols) as f32 * 7.0,
            top + (k / cols) as f32 * 7.0,
        );
        p.rect_filled(
            winui::rect(egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(6.0, 6.0),
            )),
            1.0,
            c,
        );
    }
}

/// A table in the connections table's frame: headers at x, column lines at `seps`, rows of
/// text cut to their column (whole on hover), scrolled by the wheel. `checks`: a check box
/// before each row's icon, ticked or not (the Files tab); the row whose box was clicked.
#[allow(clippy::too_many_arguments)]
fn table(
    ui: &mut Ui,
    p: &egui::Painter,
    o: Pos2,
    id: &str,
    heads: &[(f32, &str)],
    seps: &[f32],
    rows: &[Vec<String>],
    icons: bool,
    checks: Option<&[bool]>,
) -> Option<usize> {
    let mut clicked = None;
    let outer = r(o, 17.0, 273.0, 513.0, 128.0);
    winui::solid(p, outer, winui::FIELD_EDGE());
    let (bg, head, line) = (
        winui::FIELD(),
        winui::LIST_HEADER(),
        crate::theme::colors().header_sep,
    );
    fill(p, o.x + 18.0, o.y + 274.0, o.x + 528.0, o.y + 399.0, bg);
    fill(p, o.x + 19.0, o.y + 275.0, o.x + 527.0, o.y + 298.0, head);
    for x in seps {
        fill(p, o.x + x, o.y + 275.0, o.x + x, o.y + 298.0, line);
    }
    for (x, h) in heads {
        text(p, o.x + x, o.y + 290.0, crate::i18n::t(h), winui::TEXT());
    }
    let visible = 6usize;
    // Per window: each progress window scrolls its own tables.
    let key = egui::Id::unique(("torrent-table", id, ui.ctx().viewport_id()));
    let max_first = rows.len().saturating_sub(visible);
    let mut first = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(key))
        .unwrap_or(0)
        .min(max_first);
    let body = r(o, 19.0, 299.0, 508.0, 100.0);
    if ui.rect_contains_pointer(body) {
        let step = winui::wheel_steps(ui, key, 100.0 / visible as f32, visible as f32);
        first = first.saturating_add_signed(step).min(max_first);
        ui.ctx().data_mut(|d| d.insert_temp(key, first));
    }
    let clip = p.with_clip_rect(body);
    let right_of = |c: usize| seps.get(c).map_or(o.x + 520.0, |x| o.x + x - 4.0);
    for (k, row) in rows.iter().skip(first).take(visible + 1).enumerate() {
        let top = o.y + 299.0 + 17.0 * k as f32;
        for (c, cell) in row.iter().enumerate() {
            let mut x = heads.get(c).map_or(o.x + 25.0, |h| o.x + h.0);
            // A table of files (the Files tab): each name after its type's icon, and its
            // check box first when files can be chosen (#39). Only rows wholly shown take a
            // click (the one cut off under the frame doesn't).
            if icons
                && c == 0
                && let Some(&ticked) = checks.and_then(|t| t.get(first + k))
            {
                let mut on = ticked;
                let at = egui::pos2(x - 4.0, top + 2.0);
                if k < visible && winui::row_check(ui, &clip, at, &mut on, cell, (id, first + k)) {
                    clicked = Some(first + k);
                }
                x += 17.0;
            }
            if icons && c == 0 {
                crate::fileicons::paint_at(
                    ui,
                    &clip,
                    crate::fileicons::Kind::File(cell),
                    egui::pos2(x - 4.0, top),
                    16.0,
                );
                x += crate::fileicons::INDENT - 4.0;
            }
            let (rect, cut) =
                winui::text_fit(&clip, x, top + 12.0, cell, winui::TEXT(), right_of(c));
            // Cells hold info hashes, folders, trackers, peers: copyable (whole on hover).
            winui::copyable(ui, rect.intersect(body), (id, first + k, c), cell, cut);
        }
    }
    // The shared scroll bar (it was a thumb drawn alone, that couldn't be dragged).
    if rows.len() > visible {
        let mut pos = first as f32;
        if crate::scrollbar::bar(
            ui,
            key.with("bar"),
            r(o, 511.0, 275.0, 17.0, 124.0),
            true,
            rows.len() as f32,
            visible as f32,
            1.0,
            &mut pos,
        ) {
            let first = (pos.round().max(0.0) as usize).min(max_first);
            ui.ctx().data_mut(|d| d.insert_temp(key, first));
        }
    }
    clicked
}

/// The custom-drawn segment bar (spec "Segment bar"): 13-row gradients, red start columns.
fn segment_bar(p: &egui::Painter, o: Pos2, item: &Item, snap: Option<&Snapshot>) {
    let (x0, y0) = (o.x + 17.0, o.y + 249.0);
    let border = crate::theme::colors().segment_border;
    fill(p, x0, y0, x0 + 512.0, y0, border);
    fill(p, x0, y0 + 14.0, x0 + 512.0, y0 + 14.0, border);
    fill(p, x0, y0, x0, y0 + 14.0, border);
    fill(p, x0 + 512.0, y0, x0 + 512.0, y0 + 14.0, border);
    for (x, y) in [
        (x0, y0),
        (x0 + 512.0, y0),
        (x0, y0 + 14.0),
        (x0 + 512.0, y0 + 14.0),
    ] {
        px(p, x, y, crate::theme::colors().segment_corner);
    }
    // Paint a run of columns [a, b] with one of the three row profiles.
    let run = |a: f32, b: f32, which: usize| {
        if b < a {
            return;
        }
        for (row, cols) in seg_rows().enumerate() {
            let c = match which {
                0 => cols.0,
                1 => cols.1,
                _ => cols.2,
            };
            fill(p, a, y0 + 1.0 + row as f32, b, y0 + 1.0 + row as f32, c);
        }
    };
    let ix = o.x + 18.0;
    run(ix, ix + 510.0, 0);
    let Some(total) = item.size.filter(|t| *t > 0) else {
        return;
    };
    let col = |v: u64| (v as f64 / total as f64 * 511.0).floor() as f32;
    let segments: Vec<(u64, u64)> = match snap {
        Some(s) if !s.segments.is_empty() => s
            .segments
            .iter()
            .map(|g| (g.start, g.cursor - g.start))
            .collect(),
        _ => vec![(0, item.downloaded)],
    };
    for (start, done) in segments {
        let sx = ix + col(start);
        let n = col(done);
        if n > 1.0 {
            run(sx + 1.0, (sx + n - 1.0).min(ix + 510.0), 1);
        }
        run(sx, sx, 2);
    }
}

/// The connections table (spec "Connections table"): N. · Downloaded · Info.
fn connections(ui: &mut Ui, p: &egui::Painter, o: Pos2, snap: Option<&Snapshot>) {
    let outer = r(o, 17.0, 273.0, 513.0, 128.0);
    winui::solid(p, outer, winui::FIELD_EDGE());
    fill(
        p,
        o.x + 18.0,
        o.y + 274.0,
        o.x + 528.0,
        o.y + 399.0,
        // Every list's background (IDM's light one is mauve, #beb4be: the only one of its kind).
        winui::FIELD(),
    );
    fill(
        p,
        o.x + 19.0,
        o.y + 275.0,
        o.x + 510.0,
        o.y + 298.0,
        winui::LIST_HEADER(),
    );
    for x in [48.0, 148.0, 498.0] {
        fill(
            p,
            o.x + x,
            o.y + 275.0,
            o.x + x,
            o.y + 298.0,
            crate::theme::colors().header_sep,
        );
    }
    // Column titles end in "..." before their column's line, whole on hover; a long "N." starts
    // at the list's edge.
    let head = |x: f32, right: f32, label: &str| {
        let (rc, cut) = winui::text_fit(p, o.x + x, o.y + 290.0, label, winui::TEXT(), o.x + right);
        if cut {
            winui::full_on_hover(ui, rc, ("connections-head", x as i32), label);
        }
    };
    // i18n: column title: connection number (abbreviation of Number)
    let number = crate::i18n::t("N.");
    let x = if winui::text_width(p, number) > 21.0 {
        21.0
    } else {
        25.0
    };
    head(x, 46.0, number);
    // i18n: column title: bytes this connection received
    head(55.0, 146.0, crate::i18n::t("Downloaded"));
    // i18n: column title: what the connection is doing
    head(155.0, 496.0, crate::i18n::t("Info"));
    // Listed by connection number, like IDM (the engine reports them in arrival order).
    let mut conns: Vec<_> = snap
        .map(|s| s.connections.iter().collect())
        .unwrap_or_default();
    conns.sort_by_key(|c| c.id);
    let key = egui::Id::unique(("conn-scroll", ui.ctx().viewport_id()));
    let visible = 6usize;
    let max_first = conns.len().saturating_sub(visible);
    let mut first = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(key))
        .unwrap_or(0)
        .min(max_first);
    let body = r(o, 19.0, 299.0, 492.0, 100.0);
    if ui.rect_contains_pointer(body) {
        let step = winui::wheel_steps(ui, key, 100.0 / visible as f32, visible as f32);
        first = first.saturating_add_signed(step).min(max_first);
        ui.ctx().data_mut(|d| d.insert_temp(key, first));
    }
    let clip = p.with_clip_rect(body);
    for (k, c) in conns.iter().skip(first).take(visible + 1).enumerate() {
        let top = o.y + 299.0 + 17.0 * k as f32;
        winui::text_right(
            &clip,
            o.x + 33.0,
            top + 12.0,
            &(c.id + 1).to_string(),
            winui::TEXT(),
        );
        text(
            &clip,
            o.x + 56.0,
            top + 12.0,
            &size3(c.downloaded),
            winui::TEXT(),
        );
        let info = match &c.status {
            ConnStatus::Connecting => crate::i18n::t("Connecting...").to_owned(),
            ConnStatus::Receiving => crate::i18n::t("Receiving data...").to_owned(),
            ConnStatus::Retrying(e) => crate::i18n::fmt(
                // i18n: a connection's state: it failed and tries again, with the reason
                crate::i18n::t("Retrying: {error}"),
                &[("error", &crate::errors::describe(e))],
            ),
            ConnStatus::Finished => String::new(),
            ConnStatus::Failed(e) => crate::i18n::fmt(
                crate::i18n::t("Error: {error}"),
                &[("error", &crate::errors::describe(e))],
            ),
            ConnStatus::Retired { host, cap } => crate::i18n::fmt(
                // i18n: a connection closed because the server allows only that many
                &crate::i18n::tn(
                    u64::from(*cap),
                    "{host} allows {n} connection",
                    "{host} allows {n} connections",
                ),
                &[("host", &crate::i18n::ltr(host))],
            ),
        };
        // The Info column ends at the scrollbar.
        let (rect, cut) = winui::text_fit(
            &clip,
            o.x + 155.0,
            top + 12.0,
            &info,
            winui::TEXT(),
            o.x + 507.0,
        );
        if cut {
            winui::full_on_hover(ui, rect.intersect(body), ("conn", c.id), &info);
        }
    }
    // The shared scroll bar (a track and thumb were drawn here, the thumb not draggable).
    if conns.len() > visible {
        let mut pos = first as f32;
        if crate::scrollbar::bar(
            ui,
            key.with("bar"),
            r(o, 511.0, 275.0, 17.0, 124.0),
            true,
            conns.len() as f32,
            visible as f32,
            1.0,
            &mut pos,
        ) {
            let first = (pos.round().max(0.0) as usize).min(max_first);
            ui.ctx().data_mut(|d| d.insert_temp(key, first));
        }
    }
}
