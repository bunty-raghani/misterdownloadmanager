//! Toolbar icons: Lucide outlines (ISC licence, assets/icons/lucide/, v1.63.0) composed into
//! IDM 6.42's toolbar roles, stroked with the two-tone diagonal gradients measured from IDM's
//! "Neon" toolbar (tools/pixel/gradients.py). IDM's own images are never used (D-001).

use egui::{Image, ImageSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    AddUrl,
    /// Add Torrent (ours, M15): Lucide "magnet".
    AddTorrent,
    Resume,
    Stop,
    StopAll,
    Delete,
    DeleteCompleted,
    Options,
    Scheduler,
    StartQueue,
    StopQueue,
    /// Download complete's drag handle: drag the file out (IDM's hand-and-arrows grip). Windows
    /// only, as the handle is (dialogs.rs).
    #[cfg_attr(not(windows), allow(dead_code))]
    DragOut,
    /// Options page headers (IDM draws its own artwork there; ours are Lucide outlines).
    PageGeneral,
    PageFileTypes,
    PageSaveTo,
    PageDownloads,
    PageConnection,
    PageProxy,
    PageLogins,
    PageSounds,
    PageRegional,
    PageArchives,
    PageRemote,
    /// The first-start helpers window (ours): yt-dlp finds a page's video,
    HelperVideo,
    /// ffmpeg joins its picture and sound,
    HelperMerge,
    /// and both are checked before they run;
    HelperChecked,
    /// Node.js runs YouTube's JavaScript (#1570).
    HelperScript,
}

impl Icon {
    /// SVG body (24 × 24 user space, Lucide path data).
    fn body(self) -> &'static str {
        match self {
            // An outlined plus, like IDM's Add URL.
            Icon::AddUrl => r#"<path d="M9.5 2.5h5v7h7v5h-7v7h-5v-7h-7v-5h7z"/>"#,
            Icon::AddTorrent => concat!(
                r#"<path d="m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15"/>"#,
                r#"<path d="m5 8 4 4"/><path d="m12 15 4 4"/>"#,
            ),
            Icon::Resume => {
                r#"<circle cx="12" cy="12" r="10"/><path d="M12 7v10"/><path d="m8 13 4 4 4-4"/>"#
            }
            Icon::Stop => {
                r#"<circle cx="12" cy="12" r="10"/><rect x="8.5" y="8.5" width="7" height="7" rx="1.5"/>"#
            }
            Icon::StopAll => concat!(
                r#"<path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2"/>"#,
                r#"<path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2"/>"#,
                r#"<path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8"/>"#,
                r#"<path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"/>"#,
            ),
            Icon::Delete => concat!(
                r#"<path d="M10 11v6"/><path d="M14 11v6"/>"#,
                r#"<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/>"#,
                r#"<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#,
            ),
            Icon::DeleteCompleted => concat!(
                r#"<path d="m9 14 2 2 4-4.5"/>"#,
                r#"<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/>"#,
                r#"<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#,
            ),
            Icon::Options => concat!(
                r#"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/>"#,
                r#"<circle cx="12" cy="12" r="3"/>"#,
            ),
            Icon::Scheduler => concat!(
                r#"<circle cx="12" cy="13" r="8"/><path d="M5 3 2 6"/><path d="m22 6-3-3"/>"#,
                r#"<path d="M6.38 18.7 4 21"/><path d="M17.64 18.67 20 21"/><path d="m9 13 2 2 4-4"/>"#,
            ),
            Icon::StartQueue => concat!(
                r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>"#,
                r#"<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
                r#"<path d="M15 11v7"/><path d="m12 15.5 3 3 3-3"/>"#,
            ),
            Icon::StopQueue => concat!(
                r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>"#,
                r#"<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
                r#"<rect width="6" height="6" x="12" y="12" rx="1"/>"#,
            ),
            // Lucide "move": four arrows.
            Icon::DragOut => concat!(
                r#"<path d="M12 2v20"/><path d="m15 19-3 3-3-3"/><path d="m19 9 3 3-3 3"/>"#,
                r#"<path d="M2 12h20"/><path d="m5 9-3 3 3 3"/><path d="m9 5 3-3 3 3"/>"#,
            ),
            // Lucide "globe".
            Icon::PageGeneral => concat!(
                r#"<circle cx="12" cy="12" r="10"/><path d="M2 12h20"/>"#,
                r#"<path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/>"#,
            ),
            // Lucide "file-type".
            Icon::PageFileTypes => concat!(
                r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/>"#,
                r#"<path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M9 13v-1h6v1"/><path d="M12 12v6"/><path d="M11 18h2"/>"#,
            ),
            // Lucide "folder-open".
            Icon::PageSaveTo => {
                r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#
            }
            Icon::PageDownloads => {
                r#"<circle cx="12" cy="12" r="10"/><path d="M12 7v10"/><path d="m8 13 4 4 4-4"/>"#
            }
            // Lucide "network".
            Icon::PageConnection => concat!(
                r#"<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/>"#,
                r#"<rect x="9" y="2" width="6" height="6" rx="1"/>"#,
                r#"<path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>"#,
            ),
            // Lucide "shield".
            Icon::PageProxy => {
                r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>"#
            }
            // Lucide "key-round".
            Icon::PageLogins => concat!(
                r#"<path d="M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"/>"#,
                r#"<circle cx="16.5" cy="7.5" r=".5"/>"#,
            ),
            // Lucide "volume-2".
            // Lucide "calendar-clock".
            Icon::PageRegional => concat!(
                r#"<path d="M16 14v2.2l1.6 1"/><path d="M16 2v3"/><path d="M8 2v3"/>"#,
                r#"<path d="M21 7.338V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h2.338"/>"#,
                r#"<path d="M3 9h5.859"/><circle cx="16" cy="16" r="6"/>"#,
            ),
            // Lucide "package".
            Icon::PageArchives => concat!(
                r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z"/>"#,
                r#"<path d="M12 22V12"/><path d="m3.3 7 7.703 4.734a2 2 0 0 0 1.994 0L20.7 7"/><path d="m7.5 4.27 9 5.15"/>"#,
            ),
            // Lucide "smartphone" (Options → Remote, #141).
            Icon::PageRemote => {
                r#"<rect width="14" height="20" x="5" y="2" rx="2" ry="2"/><path d="M12 18h.01"/>"#
            }
            // A screen with a play mark.
            Icon::HelperVideo => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3"/><path d="M10 8.5v7l6-3.5z"/>"#
            }
            // Two streams (picture, sound) joining into one file.
            Icon::HelperMerge => concat!(
                r#"<path d="M2 6h5c2.5 0 3.5 1.5 4.5 3.5L13 12h9"/>"#,
                r#"<path d="M2 18h5c2.5 0 3.5-1.5 4.5-3.5"/><path d="m18 8 4 4-4 4"/>"#,
            ),
            // Lucide "shield" with a check.
            Icon::HelperChecked => concat!(
                r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>"#,
                r#"<path d="m9 12 2 2 4-4"/>"#,
            ),
            // Lucide "braces": a piece of code to run.
            Icon::HelperScript => concat!(
                r#"<path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1"/>"#,
                r#"<path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1"/>"#,
            ),
            Icon::PageSounds => concat!(
                r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/>"#,
                r#"<path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>"#,
            ),
        }
    }

    /// The enabled gradient, top-left → bottom-right (measured from IDM 6.42).
    fn gradient(self) -> (&'static str, &'static str) {
        match self {
            Icon::AddUrl => ("#9ad9b0", "#7fb4c0"),
            // The Torrent pill's purple.
            Icon::AddTorrent => ("#b07ad9", "#7d5fd0"),
            Icon::Resume | Icon::Stop | Icon::StopAll | Icon::Delete => ("#e2e2e2", "#b4b4b4"),
            Icon::DeleteCompleted => ("#df6a9c", "#8180c0"),
            Icon::Options => ("#7883c2", "#9b84d0"),
            Icon::Scheduler => ("#a986d2", "#c0957a"),
            Icon::StartQueue => ("#93b6f2", "#90c6b6"),
            Icon::StopQueue => ("#a2c2f2", "#c4688f"),
            // Ours: IDM's grip has orange arrows.
            Icon::DragOut => ("#f5a623", "#d9661c"),
            Icon::PageGeneral | Icon::PageConnection => ("#3d8ee6", "#2bb3a3"),
            Icon::PageFileTypes | Icon::PageDownloads => ("#3fa864", "#2f7fcf"),
            Icon::PageSaveTo => ("#e6a817", "#c77a12"),
            Icon::PageProxy => ("#5b6fd6", "#8a5fd0"),
            Icon::PageLogins => ("#d9a514", "#9a7b2e"),
            Icon::PageSounds => ("#8a5fd0", "#d05f9a"),
            Icon::PageRegional => ("#2f7fcf", "#3fa864"),
            Icon::PageArchives => ("#c77a12", "#8a5fd0"),
            Icon::PageRemote => ("#3d8ee6", "#5b6fd6"),
            Icon::HelperVideo => ("#df6a9c", "#b07ad9"),
            Icon::HelperMerge => ("#3d8ee6", "#2bb3a3"),
            Icon::HelperChecked => ("#3fa864", "#2f7fcf"),
            Icon::HelperScript => ("#e6a817", "#3fa864"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Icon::AddUrl => "add-url",
            Icon::AddTorrent => "add-torrent",
            Icon::Resume => "resume",
            Icon::Stop => "stop",
            Icon::StopAll => "stop-all",
            Icon::Delete => "delete",
            Icon::DeleteCompleted => "delete-completed",
            Icon::Options => "options",
            Icon::Scheduler => "scheduler",
            Icon::StartQueue => "start-queue",
            Icon::StopQueue => "stop-queue",
            Icon::DragOut => "drag-out",
            Icon::PageGeneral => "page-general",
            Icon::PageFileTypes => "page-file-types",
            Icon::PageSaveTo => "page-save-to",
            Icon::PageDownloads => "page-downloads",
            Icon::PageConnection => "page-connection",
            Icon::PageProxy => "page-proxy",
            Icon::PageLogins => "page-logins",
            Icon::PageSounds => "page-sounds",
            Icon::PageRegional => "page-regional",
            Icon::PageArchives => "page-archives",
            Icon::PageRemote => "page-remote",
            Icon::HelperVideo => "helper-video",
            Icon::HelperMerge => "helper-merge",
            Icon::HelperChecked => "helper-checked",
            Icon::HelperScript => "helper-script",
        }
    }

    /// The ink's two stops, top-left to bottom-right: enabled, its gradient, one colour or
    /// the accent as the theme's icon style says; disabled, the theme's flat grey (as IDM).
    fn stops(self, enabled: bool) -> (String, String) {
        use crate::palette::{IconStyle, to_hex};
        let c = crate::theme::colors();
        if !enabled {
            return (to_hex(c.icon_disabled), to_hex(c.icon_disabled_end));
        }
        match crate::theme::current().icons {
            IconStyle::Color => {
                let (a, b) = self.gradient();
                (a.to_owned(), b.to_owned())
            }
            IconStyle::Mono => (to_hex(c.text), to_hex(c.text)),
            IconStyle::Accent => (to_hex(c.accent), to_hex(c.accent)),
        }
    }

    /// The SVG for this icon, inked from `a` to `b`.
    fn svg(self, a: &str, b: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="url(#g)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x1="2" y1="2" x2="22" y2="22"><stop offset="0" stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient></defs>{}</svg>"#,
            self.body()
        )
    }

    /// The theme icon set's name for this icon (`crate::themeicons`), for the ones a set can
    /// replace: the toolbar's.
    fn need(self) -> Option<&'static str> {
        Some(match self {
            Icon::AddUrl => "tb-add-url",
            Icon::AddTorrent => "tb-add-torrent",
            Icon::Resume => "tb-resume",
            Icon::Stop => "tb-stop",
            Icon::StopAll => "tb-stop-all",
            Icon::Delete => "tb-delete",
            Icon::DeleteCompleted => "tb-delete-completed",
            Icon::Options => "tb-options",
            Icon::Scheduler => "tb-scheduler",
            Icon::StartQueue => "tb-start-queue",
            Icon::StopQueue => "tb-stop-queue",
            _ => return None,
        })
    }

    /// The icon, enabled or greyed: the theme's own where its icon set has one (a one-colour
    /// icon in the text colour), else the classic outline.
    pub fn image(self, enabled: bool) -> Image<'static> {
        let ink = crate::themeicons::ink(enabled);
        if let Some((src, tint)) = self
            .need()
            .and_then(|need| crate::themeicons::source(need, 32.0, ink, enabled))
        {
            return Image::new(src).tint(tint);
        }
        Image::new(self.source(enabled))
    }

    /// The icon as an image source (fileicons.rs shows Add Torrent's magnet for a .torrent).
    /// Made once per icon and ink (a theme's change makes new ones): drawing it again each frame
    /// allocates nothing.
    pub fn source(self, enabled: bool) -> ImageSource<'static> {
        use crate::palette::IconStyle;
        use std::cell::RefCell;
        use std::collections::HashMap;
        type Key = (Icon, bool, IconStyle, [u8; 4], [u8; 4]);
        thread_local! {
            static MADE: RefCell<HashMap<Key, ImageSource<'static>>> = RefCell::default();
        }
        let c = crate::theme::colors();
        let style = crate::theme::current().icons;
        let (x, y) = match (enabled, style) {
            (false, _) => (c.icon_disabled, c.icon_disabled_end),
            (true, IconStyle::Color) => (egui::Color32::TRANSPARENT, egui::Color32::TRANSPARENT),
            (true, IconStyle::Mono) => (c.text, c.text),
            (true, IconStyle::Accent) => (c.accent, c.accent),
        };
        let key = (self, enabled, style, x.to_array(), y.to_array());
        MADE.with(|made| {
            made.borrow_mut()
                .entry(key)
                .or_insert_with(|| {
                    let (a, b) = self.stops(enabled);
                    // Keyed by its ink too: another theme's icon is another texture. Kept for
                    // the program's life, as its texture is (a few per theme).
                    let uri: &'static str = Box::leak(
                        format!(
                            "bytes://mdm/toolbar/{}-{}-{}{}.svg",
                            self.name(),
                            if enabled { "on" } else { "off" },
                            a.trim_start_matches('#'),
                            b.trim_start_matches('#'),
                        )
                        .into_boxed_str(),
                    );
                    ImageSource::Bytes {
                        uri: uri.into(),
                        bytes: egui::load::Bytes::Shared(self.svg(&a, &b).into_bytes().into()),
                    }
                })
                .clone()
        })
    }
}
