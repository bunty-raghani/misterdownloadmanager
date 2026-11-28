//! Tasks → Show drop target (D-021): IDM's small floating box that takes links dragged onto
//! it. Always on top, moved by dragging it, double-click brings the main window back,
//! right-click hides it. Browsers hand dragged links over through OLE drag and drop (the
//! "UniformResourceLocatorW" format, else text), which egui doesn't expose (it only takes
//! dropped files): the window gets its own `IDropTarget` (`win32::ole`, D-008; elsewhere it
//! shows but takes nothing yet).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};

use egui::{Pos2, ViewportCommand, ViewportId, pos2, vec2};

/// The window's title (its OLE target is found by it).
const TITLE: &str = "Mister Download Manager drop target"; // not translated: an id
const SIZE: f32 = 56.0;

/// What was dropped: an address or a file path.
pub type Dropped = String;

pub enum Event {
    /// Something to download (an address, or a .torrent file's path).
    Drop(Dropped),
    /// Double-click: the main window.
    ShowMain,
    /// Right-click at this screen point: the app shows the drop target's menu there.
    Context(Pos2),
    /// Moved: where it is now (screen, logical points).
    Moved(Pos2),
}

/// Links and files dropped on one of the app's windows (the drop target, the main window).
/// On Windows through the window's own OLE `IDropTarget` (browsers hand links over that way);
/// macOS and Linux have no such hook here: winit passes dropped files only, which the main
/// window reads from egui itself.
pub struct Drops {
    rx: Receiver<Dropped>,
    #[cfg_attr(not(windows), allow(dead_code))]
    tx: Sender<Dropped>,
    /// Something droppable is being dragged over the window (highlight).
    over: Arc<AtomicBool>,
    #[cfg(windows)]
    ole: Option<crate::win32::ole::Registration>,
}

impl Default for Drops {
    fn default() -> Self {
        let (tx, rx) = channel();
        Drops {
            rx,
            tx,
            over: Arc::new(AtomicBool::new(false)),
            #[cfg(windows)]
            ole: None,
        }
    }
}

impl Drops {
    /// What was dropped on the window titled `title` since the last call (it is registered
    /// as a drop target once it exists).
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn poll(&mut self, ctx: &egui::Context, title: &str) -> Vec<Dropped> {
        #[cfg(windows)]
        if self.ole.is_none() {
            self.ole = crate::win32::ole::Registration::new(
                title,
                self.tx.clone(),
                self.over.clone(),
                ctx,
            );
        }
        self.rx.try_iter().collect()
    }

    /// Something droppable is over the window.
    pub fn over(&self) -> bool {
        self.over.load(Ordering::Relaxed)
    }
}

pub struct DropTarget {
    drops: Drops,
    pos: Pos2,
    /// MDM_TEST_DROP's link was dropped (once).
    #[cfg(windows)]
    test_dropped: bool,
}

impl DropTarget {
    pub fn new(pos: Option<Pos2>) -> Self {
        DropTarget {
            drops: Drops::default(),
            pos: pos.unwrap_or(pos2(200.0, 200.0)),
            #[cfg(windows)]
            test_dropped: false,
        }
    }

    /// The interface size changed (#262; `ratio`: the old zoom over the new): kept where it is
    /// on the screen, its place being in points, and made its size at the new one.
    pub fn rescale(&mut self, ctx: &egui::Context, ratio: f32) {
        self.pos = (self.pos.to_vec2() * ratio).round().to_pos2();
        ctx.send_viewport_cmd_to(
            ViewportId::from_hash_of("drop-target"),
            ViewportCommand::InnerSize(vec2(SIZE, SIZE)),
        );
    }

    pub fn show(&mut self, ctx: &egui::Context, icon: &egui::ImageSource<'static>) -> Vec<Event> {
        let mut events: Vec<Event> = self
            .drops
            .poll(ctx, TITLE)
            .into_iter()
            .map(Event::Drop)
            .collect();
        // Off-screen test copies: MDM_TEST_DROP=<link> is dropped on the box once, through its
        // OLE drop target, as a browser drops one (#208's checks: it still takes links as a
        // popup window). Taken at the next poll, as a real drop is.
        #[cfg(windows)]
        if !self.test_dropped
            && let Some(ole) = &self.drops.ole
            && crate::offscreen()
            && let Ok(link) = std::env::var("MDM_TEST_DROP")
        {
            self.test_dropped = true;
            ole.test_drop(&link);
        }
        // A borderless window as menus are (a plain popup on Windows, #208).
        let builder = crate::chrome::popup_viewport(TITLE)
            // Windows: our own OLE target takes links and files; elsewhere winit's own
            // (files only: macOS and Linux hand links over no other way here).
            .with_drag_and_drop(!cfg!(windows))
            .with_position(self.pos)
            .with_inner_size(vec2(SIZE, SIZE));
        let over = self.drops.over();
        let vid = ViewportId::from_hash_of("drop-target");
        let mut styled = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _| {
            // Styled in its own pass, before anything is drawn in it.
            styled = crate::chrome::style_popup(TITLE, crate::chrome::PopupLook::Floating);
            let rect = ui.max_rect();
            let p = ui.painter();
            // Something dragged over it: the shared highlight and accent, in the mode.
            crate::chrome::floating_frame(p, rect, over);
            egui::Image::new(icon.clone()).paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), vec2(40.0, 40.0)),
            );
            let resp = ui
                .interact(
                    rect,
                    ui.scope_id().with("drop-target"),
                    crate::winui::pointer_drag(),
                )
                .on_hover_text(if cfg!(windows) {
                    crate::i18n::t("Drag links here to download them")
                } else {
                    // i18n: the drop target's tooltip on macOS and Linux (they hand over files only)
                    crate::i18n::t("Drag .torrent files or lists of addresses here")
                });
            #[cfg(not(windows))]
            for f in ui.ctx().input(|i| i.raw.dropped_files.clone()) {
                let path = f.path().display().to_string();
                if !path.is_empty() {
                    events.push(Event::Drop(path));
                }
            }
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if resp.double_clicked() {
                events.push(Event::ShowMain);
            }
            // The menu is the app's (a 56 px window can't hold one).
            if resp.secondary_clicked()
                && let (Some(at), Some(win)) = (
                    resp.interact_pointer_pos(),
                    ui.ctx().input(|i| i.viewport().inner_rect),
                )
            {
                events.push(Event::Context(win.min + at.to_vec2()));
            }
            if let Some(r) = ui.ctx().input(|i| i.viewport().outer_rect)
                && (r.min - self.pos).length() > 0.5
            {
                self.pos = r.min;
                events.push(Event::Moved(r.min));
            }
        });
        if styled {
            crate::chrome::show_popup(ctx, vid);
        }
        events
    }

    /// The middle of the box (screen, logical points): where a right-click opens its menu in
    /// the off-screen checks (`/show drop-target-menu`).
    pub fn center(&self) -> Pos2 {
        self.pos + vec2(SIZE, SIZE) / 2.0
    }
}
