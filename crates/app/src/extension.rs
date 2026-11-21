//! The browser extension's offer: at each start, until a browser's extension has reached the
//! app (the native host notes it: mdm_ipc::extension::seen), a window lists the browsers
//! installed and opens the extension's page in the one picked (addons.mozilla.org for Firefox
//! and its family, the Chrome Web Store for Chrome, Edge, Brave, Opera and Vivaldi), with the
//! tutorial that adds it by hand as the fallback. Chrome may ask by itself too: registering the
//! host also names the store's extension to it (crates/nmhost). "Don't ask again" stops the
//! offer; `/show extension` brings it back.
//!
//! Safari (macOS) has its extension in the app already (tools/safari): its row says "How to
//! Add", which opens the steps that turn on an extension Apple hasn't notarized.
//!
//! Where it stands (the browsers' folders, the extension's notes) is read beside the window,
//! never on its thread (#1218): a browser's folder can take seconds to answer (a sleeping disk,
//! a network home folder, macOS asking the person whether MDM may read another app's data), and
//! the window shows what it read last meanwhile.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use egui::{Pos2, Ui};

use crate::dialogs::{Outcome, window};
use crate::winui::{self, ButtonKind, r};

const W: f32 = 520.0;
const BANNER: f32 = 84.0;
const ROW: f32 = 34.0;
/// Safari's row (macOS), as Options and the native host name it.
const SAFARI: &str = "Apple Safari";

/// Whether to offer the extension at this start: browser integration on and registered (not a
/// portable copy, which registers nothing), no browser's extension seen yet, not refused.
pub fn worth_asking(settings: &crate::settings::Settings) -> bool {
    !settings.extension_never_ask
        && settings.advanced_integration
        && mdm_ipc::portable_dir().is_none()
        && !mdm_ipc::extension::seen()
}

pub struct Extension {
    /// The browsers installed: name and program. Empty: one row for the system's browser.
    browsers: Vec<(String, Option<PathBuf>)>,
    /// Where it stands, read beside the window (once a second, and again once a Set it up
    /// finished: its key is [`set_up_runs`]), and what was read last. None before the first
    /// answer ("Checking...").
    status: crate::launch::Asked<u64, Status>,
    shown: Option<Status>,
    /// What reads it ([`Status::read`]; a test's slow one).
    read: fn() -> Status,
    /// The steps that add Safari's extension, while that window is open.
    steps: Option<SafariSteps>,
}

/// How often the window reads where it stands again: the host marks the extension seen when a
/// browser starts it.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How long a window waits for its very first read (none after): a read that answers at once,
/// as nearly all do, is shown at once, without a "Checking..." frame first.
const FIRST_WAIT: Duration = Duration::from_millis(100);

/// What the extension's window shows of where it stands, read beside it ([`Status::read`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Status {
    /// A browser's extension has reached the app.
    connected: bool,
    /// The browsers whose extension has (Options' names, mdm_ipc::extension::seen_browsers).
    seen: Vec<String>,
    /// The browsers can find the app (the native host is registered).
    registered: bool,
    /// macOS kept MDM out of a browser's folder, when reading it or setting it up.
    refused: Option<Refused>,
}

impl Status {
    /// Read from the disk: the extension's notes in the data folder, the browsers' folders. May
    /// wait (#1218): never on the window's thread.
    fn read() -> Status {
        let registration = registration();
        let registered = registration == Registration::Ready;
        Status {
            connected: mdm_ipc::extension::seen(),
            seen: seen_names(),
            registered,
            refused: if registered {
                None
            } else {
                refused_now(&registration)
            },
        }
    }
}

/// Whether `row` (a browser as the window lists it) is among the browsers seen (as Options
/// names them). Vivaldi, Chromium, Helium, Thorium and Arc say they are Chrome (no brand of
/// their own, or Chrome's on purpose): they share Chrome's status. Firefox's forks give their
/// own name (`capture::browser_named`).
fn is_seen(seen: &[String], row: &str) -> bool {
    let named = match row {
        "Brave" => "Brave Browser",
        "Vivaldi" | "Chromium" | "Helium" | "Thorium" | "Arc" => "Google Chrome",
        other => other,
    };
    seen.iter().any(|b| b == named)
}

/// A row for a browser of Firefox's family (Firefox, Zen, LibreWolf …): it gets the add-on's
/// page on addons.mozilla.org, not the Chrome Web Store.
fn is_gecko(row: &str) -> bool {
    mdm_ipc::browsers::named(row).is_some_and(|b| b.family == mdm_ipc::browsers::Family::Gecko)
}

impl Extension {
    pub fn new() -> Self {
        #[cfg(windows)]
        let found = crate::win32::browsers();
        #[cfg(unix)]
        let found = crate::unix::browsers();
        let mut browsers: Vec<_> = found.into_iter().map(|(n, p)| (n, Some(p))).collect();
        // The table's order (Chrome first, its button the accent one), others after.
        browsers.sort_by_key(|(n, _)| {
            mdm_ipc::browsers::BROWSERS
                .iter()
                .position(|b| b.name == n.as_str())
                .unwrap_or(usize::MAX)
        });
        // Safari comes with the app (macOS only): a row of its own, after the others.
        if safari_here() {
            browsers.push((SAFARI.to_owned(), None));
        }
        if browsers.is_empty() {
            browsers.push((String::new(), None));
        }
        Self::with_reader(browsers, Status::read)
    }

    /// The window for `browsers`, where it stands read by `read`.
    fn with_reader(browsers: Vec<(String, Option<PathBuf>)>, read: fn() -> Status) -> Self {
        Extension {
            browsers,
            status: crate::launch::Asked::default(),
            shown: None,
            read,
            steps: None,
        }
    }

    /// Where it stands, never waiting for the disk (only for the very first read, at most
    /// [`FIRST_WAIT`]): the last read's answer while the next is under way, one at a time, and
    /// None before the first. `repaint` is woken when an answer that differs comes.
    fn follow(&mut self, repaint: Option<&egui::Context>) -> Option<&Status> {
        let read = self.read;
        if let Some(status) =
            self.status
                .get_or_last(&set_up_runs(), LOOK_EVERY, FIRST_WAIT, repaint, move |_| {
                    read()
                })
        {
            self.shown = Some(status);
        }
        self.shown.as_ref()
    }

    /// The Safari steps, open at once (`/show safari-steps`: screenshots).
    pub fn open_safari_steps(&mut self) {
        self.steps = Some(SafariSteps);
    }

    /// Open until closed; Done when "Don't ask again" was pressed.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        // The host marks the extension seen when a browser starts it: look again now and then
        // (each browser's own, as each adds it), beside the window.
        self.follow(Some(ctx));
        ctx.request_repaint_after(LOOK_EVERY);
        // A long translation's subline makes the banner taller (winui::banner), a long
        // paragraph of where it stands its bottom (macOS's refusal, #1218).
        let h = BANNER
            + 16.0
            + self.browsers.len() as f32 * ROW
            + 104.0
            + crate::dialogs::grow_extra(ctx, "extension-status");
        let extra = crate::dialogs::grow_extra(ctx, "extension-banner");
        let mut answer = None;
        let mut open_steps = false;
        let outcome = window(
            ctx,
            "extension",
            // i18n: title of the window that offers the browser extension
            "Add Mister Download Manager to your browser",
            [W, h + extra],
            false,
            winui::BG(),
            |ui, o| {
                self.body(ui, o, h, &mut answer, &mut open_steps);
                match answer.take() {
                    Some(true) => Outcome::Done(()),
                    Some(false) => Outcome::Closed,
                    // Escape closes the steps first, when they are open.
                    None if self.steps.is_none() && winui::escape_pressed(ui) => Outcome::Closed,
                    None => Outcome::Open,
                }
            },
        );
        if open_steps && self.steps.is_none() {
            self.steps = Some(SafariSteps);
        }
        if let Some(steps) = &mut self.steps
            && !matches!(steps.show(ctx), Outcome::Open)
        {
            self.steps = None;
        }
        outcome
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        h: f32,
        answer: &mut Option<bool>,
        open_steps: &mut bool,
    ) {
        let p = ui.painter().clone();
        let (ink, muted) = (winui::TEXT(), winui::MUTED());
        // The banner: the logo, what the extension does, in one line, and how.
        let extra = winui::banner(
            ui,
            &p,
            o,
            W,
            "extension-banner",
            // i18n: the headline of the window that offers the browser extension
            crate::i18n::t("Catch downloads from your browser"),
            crate::i18n::t(
                "Add the free extension: files you download in your browser come here, and videos get a Download button.",
            ),
        );
        // Everything below it moves down with a taller banner.
        let o = o + egui::vec2(0.0, extra);
        // What was read last (beside the window); before the first answer, nothing said wrong.
        let status = self.shown.clone();
        let known = status.is_some();
        let Status {
            connected,
            seen,
            registered,
            refused,
        } = status.unwrap_or(Status {
            registered: true,
            ..Status::default()
        });

        // One row per browser: its name, and the button that opens the extension's page in it:
        // Firefox's on addons.mozilla.org, the Chrome Web Store's for the others (the tutorial
        // while STORE_ID is a placeholder, as in a build of its own).
        let store = mdm_ipc::extension::store_page();
        for (k, (name, program)) in self.browsers.iter().enumerate() {
            let y = BANNER + 16.0 + k as f32 * ROW;
            let page = if is_gecko(name) {
                Some(mdm_ipc::extension::FIREFOX_PAGE.to_owned())
            } else {
                store.clone()
            };
            winui::text_font(
                &p,
                winui::tahoma_bold(),
                o.x + 22.0,
                o.y + y + 16.0,
                if name.is_empty() {
                    // i18n: a row of the browser extension's window when no browser was found by name
                    crate::i18n::t("Your browser")
                } else {
                    name.as_str()
                },
                ink,
            );
            let safari = name == SAFARI;
            let label = if safari {
                // i18n: button in the browser extension's window, on Safari's row: open the steps that turn the extension on
                "How to Add"
            } else if page.is_none() {
                // i18n: button in the browser extension's window: open the tutorial that adds the extension
                "Show me how"
            } else {
                // i18n: button in the browser extension's window: open the extension's page in that browser
                "Add the extension"
            };
            // As wide as its label in this language (#192).
            let (add_x, add_w) =
                winui::buttons_to_right(&p, W - 18.0, 8.0, &[(label, 150.0)], 220.0)[0];
            // This browser's extension is there already: said, before its button.
            if is_seen(&seen, name) {
                winui::text_right(
                    &p,
                    o.x + add_x - 10.0,
                    o.y + y + 16.0,
                    &format!(
                        "\u{2714} {}",
                        // i18n: in the browser extension's window, beside a browser whose extension reached MDM
                        crate::i18n::t("Connected")
                    ),
                    winui::SUCCESS(),
                );
            }
            let add = winui::button(
                ui,
                r(o, add_x, y, add_w, 25.0),
                label,
                if k == 0 {
                    ButtonKind::Accent
                } else {
                    ButtonKind::Normal
                },
                false,
                None,
            );
            if add.clicked() && safari {
                *open_steps = true;
            } else if add.clicked() {
                // No store page: the tutorial on the website, in the interface language.
                let url = page.clone().unwrap_or_else(|| {
                    crate::help::page_url(mdm_ipc::extension::GUIDE_DOC, crate::i18n::code())
                });
                let url = url.as_str();
                match program {
                    #[cfg(windows)]
                    Some(program) => crate::win32::open_in(program, url),
                    #[cfg(unix)]
                    Some(program) => crate::unix::open_in(program, url),
                    None => {
                        crate::launch::open(url);
                    }
                }
            }
        }

        // Where it stands, then the answer. Not set up at all (the setup's box unticked): no
        // extension could reach the app, so that comes first, with the fix.
        // A paragraph longer than the three lines planned grows the window (its height counts
        // this already): it starts as high as before.
        let grown = crate::dialogs::grow_extra(ui.ctx(), "extension-status");
        let y = h - 84.0 - grown;
        let mut lines = 0;
        if !registered {
            // As wide as its label in this language (#192); the paragraph ends 10 px before it.
            let (fix_x, fix_w) =
                winui::buttons_to_right(&p, W - 18.0, 8.0, &[("Set it up", 110.0)], 180.0)[0];
            // macOS kept MDM out of a browser's folder (#1218, #1173): what to allow, and where.
            let why = match &refused {
                Some(refused) => format!("{} {}", refused_line(refused), refused_how()),
                None => crate::i18n::t(
                    "Browser integration isn't set up on this computer, so no browser can reach Mister Download Manager yet.",
                )
                .to_owned(),
            };
            lines = winui::paragraph(
                &p,
                o.x + 18.0,
                o.y + y,
                &why,
                winui::ERROR(),
                o.x + fix_x - 10.0,
                14.0,
            );
            let fix = winui::button(
                ui,
                r(o, fix_x, y - 4.0, fix_w, 25.0),
                // i18n: button in the browser extension's window: register MDM's helper with the browsers
                "Set it up",
                ButtonKind::Accent,
                false,
                None,
            );
            if fix.clicked() {
                set_up(ui.ctx());
            }
        }
        let (line, color) = if !registered {
            (String::new(), muted)
        } else if !known {
            // i18n: in the browser extension's window, while it reads whether the browsers can reach MDM (a moment, usually)
            (crate::i18n::t("Checking...").to_owned(), muted)
        } else if connected {
            (
                crate::i18n::t("Connected: downloads from your browser now come here.").to_owned(),
                winui::SUCCESS(),
            )
        } else {
            (
                crate::i18n::t(
                    "Once it is added, this window says so. Chrome may also ask by itself at its next start.",
                )
                .to_owned(),
                muted,
            )
        };
        if !line.is_empty() {
            lines = winui::paragraph(&p, o.x + 18.0, o.y + y, &line, color, o.x + W - 18.0, 14.0);
        }
        crate::dialogs::grow_px(
            ui.ctx(),
            "extension-status",
            14.0 * lines.saturating_sub(3) as f32,
        );
        winui::etched_h(&p, o.x + 16.0, o.y + h - 46.0, W - 32.0);
        let y = h - 36.0;
        // The fallback, when a store can't be used (a managed browser, a store turned off): the
        // tutorial, which adds it by hand.
        if store.is_some()
            && winui::link(
                ui,
                &p,
                o.x + 18.0,
                o.y + y + 17.0,
                // i18n: link in the browser extension's window: the tutorial that adds it without the browser's store
                "Install it by hand",
                "by-hand",
            )
        {
            crate::help::open_page(mdm_ipc::extension::GUIDE_DOC);
        }
        // As wide as their labels in this language (#192), Don't ask again only while no
        // extension has reached MDM.
        let row: Vec<(&str, f32)> = if connected {
            vec![("Close", 90.0)]
        } else {
            vec![("Don't ask again", 130.0), ("Close", 90.0)]
        };
        let placed = winui::buttons_to_right(&p, W - 18.0, 8.0, &row, 200.0);
        let close_at = placed[placed.len() - 1];
        let close = winui::button(
            ui,
            r(o, close_at.0, y, close_at.1, 25.0),
            "Close",
            ButtonKind::Default,
            false,
            None,
        );
        if close.clicked() || winui::enter_pressed(ui) {
            *answer = Some(false);
        }
        if !connected {
            // i18n: button in the browser extension's window: stop offering it at each start
            let never = winui::button(
                ui,
                r(o, placed[0].0, y, placed[0].1, 25.0),
                "Don't ask again",
                ButtonKind::Normal,
                false,
                None,
            );
            if never.clicked() {
                *answer = Some(true);
            }
        }
    }
}

/// Is Safari on this computer (macOS)? Its extension is in the app.
fn safari_here() -> bool {
    cfg!(target_os = "macos")
        && [
            "/Applications/Safari.app",
            "/System/Cryptexes/App/System/Applications/Safari.app",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
}

/// The steps that turn Safari's extension on. It is in the app, but an app Apple has not
/// notarized (a build of our own, the experimental downloads) has Safari list its extension only
/// while it allows unsigned ones, which it forgets each time it quits.
pub struct SafariSteps;

impl SafariSteps {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        // The banner and the steps are taller than planned in a language that wraps them.
        let extra = crate::dialogs::grow_extra(ctx, "safari-banner")
            + crate::dialogs::grow_extra(ctx, "safari-steps");
        let h = BANNER + 18.0 + STEPS.len() as f32 * STEP + 10.0 + NOTE + 54.0;
        window(
            ctx,
            "safari-steps",
            // i18n: title of the window that shows how to turn on the Safari extension
            "Add the extension to Safari",
            [W, h + extra],
            false,
            winui::BG(),
            |ui, o| {
                let h = h + extra;
                let p = ui.painter().clone();
                let banner_extra = winui::banner(
                    ui,
                    &p,
                    o,
                    W,
                    "safari-banner",
                    // i18n: the headline of the window that shows how to turn on the Safari extension
                    crate::i18n::t("Turn on the extension in Safari"),
                    crate::i18n::t(
                        "The extension is already in this app. Apple hasn't notarized this build, so Safari needs these steps to allow it.",
                    ),
                );
                let o = o + egui::vec2(0.0, banner_extra);
                let (ink, muted) = (winui::TEXT(), winui::MUTED());
                let mut y = BANNER + 18.0;
                let mut lines_extra = 0.0;
                for (k, step) in STEPS.iter().enumerate() {
                    winui::text(
                        &p,
                        o.x + 22.0,
                        o.y + y + 12.0,
                        &format!("{}.", k + 1),
                        muted,
                    );
                    let lines = winui::paragraph(
                        &p,
                        o.x + 48.0,
                        o.y + y + 12.0,
                        crate::i18n::t(step),
                        ink,
                        o.x + W - 18.0,
                        14.0,
                    );
                    let more = 14.0 * (lines.max(1) as f32 - 1.0);
                    y += STEP + more;
                    lines_extra += more;
                }
                // Safari forgets the unsigned setting when it quits.
                let note = winui::paragraph(
                    &p,
                    o.x + 22.0,
                    o.y + y + 14.0,
                    crate::i18n::t(
                        "Safari turns \"Allow unsigned extensions\" off each time it quits: do steps 1 to 6 again after restarting it.",
                    ),
                    muted,
                    o.x + W - 18.0,
                    14.0,
                );
                // NOTE plans two lines for it; a language that wraps it more grows the window.
                lines_extra += 14.0 * (note.max(2) as f32 - 2.0);
                crate::dialogs::grow_px(ui.ctx(), "safari-steps", lines_extra);
                // `o` moved down with the banner, which `h` already counts: the bottom row is
                // placed from the window's own top.
                winui::etched_h(&p, o.x + 16.0, o.y + h - 46.0 - banner_extra, W - 32.0);
                let placed = winui::buttons_to_right(&p, W - 18.0, 8.0, &[("Close", 90.0)], 200.0);
                let close = winui::button(
                    ui,
                    r(o, placed[0].0, h - 36.0 - banner_extra, placed[0].1, 25.0),
                    "Close",
                    ButtonKind::Default,
                    false,
                    None,
                );
                if close.clicked() || winui::enter_pressed(ui) || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// A step's height when it fits one line, and the note's under them.
const STEP: f32 = 24.0;
const NOTE: f32 = 34.0;

/// The steps, in Safari's own words for its tabs and switches.
const STEPS: [&str; 8] = [
    // i18n: step 1 of turning on the Safari extension
    "Open Safari.",
    // i18n: step 2 of turning on the Safari extension; Cmd is the Command key
    "Open Safari's settings (Cmd+,).",
    // i18n: step 3 of turning on the Safari extension; the quoted tab name is what Safari's settings show in this language: use Apple's own word for it, or keep the English name when unsure
    "Switch to the \"Advanced\" tab (the cogwheels).",
    // i18n: step 4 of turning on the Safari extension; the quoted switch name is what Safari's settings show in this language: use Apple's own words for it, or keep the English name when unsure
    "Turn on \"Show features for web developers\".",
    // i18n: step 5 of turning on the Safari extension; the quoted tab name is what Safari's settings show in this language: use Apple's own word for it, or keep the English name when unsure
    "Switch to the \"Developer\" tab (the screwdriver and spanner).",
    // i18n: step 6 of turning on the Safari extension; the quoted switch name is what Safari's settings show in this language: use Apple's own words for it, or keep the English name when unsure
    "Turn on \"Allow unsigned extensions\".",
    // i18n: step 7 of turning on the Safari extension; the quoted tab name is what Safari's settings show in this language: use Apple's own word for it, or keep the English name when unsure
    "Switch to the \"Extensions\" tab (the puzzle piece).",
    // i18n: step 8 of turning on the Safari extension
    "Turn on \"Mister Download Manager\" and allow it on all websites.",
];

/// The browsers whose extension reached the app, by Options' names.
fn seen_names() -> Vec<String> {
    mdm_ipc::extension::seen_browsers()
        .into_iter()
        .map(|(b, _)| b)
        .collect()
}

/// Where browser integration's registration stands on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Registration {
    /// The browsers find the native host's manifest, and the helper program it names is there.
    Ready,
    /// A browser's folder (or the registry) has no manifest.
    Missing,
    /// No browser MDM knows was found to register with (Linux and macOS find them by their
    /// folders; Windows registers for every browser at once).
    #[cfg_attr(windows, allow(dead_code))]
    NoBrowser,
    /// A manifest names a helper program that isn't there or can't be run (#253).
    HelperMissing(PathBuf),
    /// The browsers are pointed at a manifest that is gone (Windows' registry), with the copy
    /// that wrote it: one run from inside a zip, whose temporary folder was deleted (#818).
    #[cfg_attr(unix, allow(dead_code))]
    Gone(PathBuf),
    /// macOS refused to let MDM read this browser folder ("Operation not permitted", #1218):
    /// its protection of other apps' data. Other systems never say it (their folders don't
    /// refuse that way); an off-screen test copy can, for its screenshots (MDM_TEST_REFUSED).
    Refused(PathBuf),
}

/// Where browser integration's registration stands: the native host's manifests where the
/// browsers look, and the helper they name. It reads the browsers' folders, which can take
/// seconds (#1218): never on the window's thread ([`Status::read`], [`Tester`]).
///
/// Off-screen test copies only: MDM_TEST_SLOW_REGISTRATION=<seconds> makes it take that long
/// first (a folder that doesn't answer), MDM_TEST_REFUSED=<folder> says macOS refused that one.
pub fn registration() -> Registration {
    if crate::offscreen() {
        if let Some(secs) = std::env::var("MDM_TEST_SLOW_REGISTRATION")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s > 0.0)
        {
            tracing::info!(
                "browser integration: a slow read ({secs} s, MDM_TEST_SLOW_REGISTRATION)"
            );
            std::thread::sleep(Duration::from_secs_f64(secs.min(600.0)));
        }
        if let Some(folder) = std::env::var_os("MDM_TEST_REFUSED").filter(|f| !f.is_empty()) {
            return Registration::Refused(PathBuf::from(folder));
        }
    }
    #[cfg(windows)]
    return crate::win32::host_registration();
    #[cfg(unix)]
    return crate::unix::host_registration();
}

/// macOS kept MDM out of a browser's folder ("Operation not permitted", EPERM, #1218, #1173):
/// which browser's (its name in the app; empty when the folder is no browser's MDM knows by
/// name), and the folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refused {
    pub browser: String,
    pub folder: PathBuf,
}

impl Refused {
    /// The folder `folder` refused: its browser named by its place under Application Support.
    fn at(folder: &std::path::Path) -> Refused {
        Refused {
            browser: browser_of_folder(&folder.to_string_lossy())
                .unwrap_or_default()
                .to_owned(),
            folder: folder.to_owned(),
        }
    }
}

/// The browser whose folder `path` is in (under `Library/Application Support/`, as MDM's table
/// names its data and host folders on macOS: `Google/Chrome/NativeMessagingHosts` is Chrome's,
/// `Mozilla/...` Firefox's), None for another folder.
fn browser_of_folder(path: &str) -> Option<&'static str> {
    const SUPPORT: &str = "Library/Application Support/";
    let under = &path[path.rfind(SUPPORT)? + SUPPORT.len()..];
    mdm_ipc::browsers::BROWSERS
        .iter()
        .find(|b| {
            b.mac_data.iter().chain(b.mac_hosts.iter()).any(|d| {
                under
                    .strip_prefix(d)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
        })
        .map(|b| b.name)
}

/// Whether the native host's error `error` (its `--register`, or the window's own) says macOS
/// kept MDM out of a browser's folder: "<folder>: Operation not permitted (os error 1)", the
/// folder under `Library/Application Support/`. Only on macOS (`macos`): elsewhere EPERM means
/// other things, and the error is shown as it is.
fn refused_in(error: &str, macos: bool) -> Option<Refused> {
    const EPERM: &str = ": Operation not permitted (os error 1)";
    if !macos {
        return None;
    }
    // Several folders' errors are joined with "; " (crates/nmhost): the first refused one.
    let at = error.find(EPERM)?;
    let before = &error[..at];
    let head = &before[..before.rfind("/Library/Application Support/")?];
    // The folder starts after the words before it ("misterdownloadmanager-nmhost: /Users/…"),
    // or the error before it ("…; /Users/…").
    let start = [head.rfind(": "), head.rfind("; ")]
        .into_iter()
        .flatten()
        .max()
        .map_or(0, |s| s + 2);
    Some(Refused::at(std::path::Path::new(before[start..].trim())))
}

/// macOS keeping MDM out of a browser's folder now: the folder it refused to read, else the last
/// Set it up's error, if it says so.
fn refused_now(registration: &Registration) -> Option<Refused> {
    match registration {
        Registration::Refused(folder) => Some(Refused::at(folder)),
        _ => register_error().and_then(|e| refused_in(&e, cfg!(target_os = "macos"))),
    }
}

/// What macOS refusing a folder means, in a line: "macOS keeps MDM out of Google Chrome's folder".
fn refused_line(refused: &Refused) -> String {
    if refused.browser.is_empty() {
        // i18n: a check of Test browser integration, and the browser extension's window (macOS): macOS refused to let MDM read or write a browser's folder (where the browser finds MDM's helper)
        crate::i18n::t("macOS keeps Mister Download Manager out of a browser's folder.").to_owned()
    } else {
        crate::i18n::fmt(
            // i18n: a check of Test browser integration, and the browser extension's window (macOS): macOS refused to let MDM read or write {browser}'s folder (where the browser finds MDM's helper); {browser} is a browser's name
            crate::i18n::t("macOS keeps Mister Download Manager out of {browser}'s folder."),
            &[("browser", &refused.browser)],
        )
    }
}

/// What to do about it, in Apple's words for its settings (System Settings → Privacy & Security
/// → Full Disk Access: "Allow apps to access all files on a user's Mac, including data from other
/// apps", Apple's Platform Support guide; the prompt macOS shows is "… would like to access data
/// from other apps").
fn refused_how() -> &'static str {
    // i18n: under "macOS keeps Mister Download Manager out of …'s folder" (Test browser integration, the browser extension's window): how to allow it. The quoted words and System Settings → Privacy & Security → Full Disk Access are macOS's own: use Apple's words in this language, or keep the English ones when unsure
    crate::i18n::t(
        "If macOS asks whether it may access data from other apps, allow it. Or turn on Mister Download Manager in System Settings → Privacy & Security → Full Disk Access. Then press Set it up.",
    )
}

/// Why the last registration failed (at a start, or Set it up's), in the native host's words;
/// None once one succeeded.
static REGISTER_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Why the last registration failed, if it did (Test browser integration says it).
fn register_error() -> Option<String> {
    REGISTER_ERROR.lock().ok().and_then(|e| e.clone())
}

/// Note how a registration went: logged, and its failure kept for Test browser integration.
pub(crate) fn registered_as(result: &Result<String, String>) {
    match result {
        Ok(said) => tracing::info!("browser integration set up: {said}"),
        Err(e) => tracing::warn!("browser integration not set up: {e}"),
    }
    if let Ok(mut last) = REGISTER_ERROR.lock() {
        *last = result.as_ref().err().cloned();
    }
}

/// The native host `host` run with `arg` (`--register`, `--repair`), waiting up to `limit`:
/// what it says it did, or why it failed.
pub(crate) fn run_host(
    host: &std::path::Path,
    arg: &str,
    limit: Duration,
) -> Result<String, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(host);
    cmd.arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    crate::win32::no_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {e}", host.display()))?;
    let status = crate::launch::wait_within(&mut child, limit);
    // A few lines at most: they fit the pipes, so the helper never waits on them.
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    if let Some(mut err) = child.stderr.take() {
        said.push('\n');
        let _ = err.read_to_string(&mut said);
    }
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    match status {
        Some(s) if s.success() => Ok(said),
        Some(s) => Err(format!("{} {arg}: {s}: {said}", host.display())),
        None => Err(format!("{} {arg}: no answer in {limit:?}", host.display())),
    }
}

/// This copy runs from a folder that is deleted later (#818): an archive run without extracting
/// it first (WinRAR, 7-Zip and Windows Explorer extract it to a temporary folder, deleted once it
/// quits), or anything under the temporary folder. It registers nothing with the browsers, which
/// would be left with a helper that is gone. Linux's AppImage counts by where the AppImage is:
/// its mount is always under /tmp, and its helper is registered from a copy outside it.
pub fn temporary_copy() -> bool {
    static TEMPORARY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TEMPORARY.get_or_init(|| {
        let image = std::env::var_os("APPIMAGE").filter(|i| cfg!(unix) && !i.is_empty());
        let program = image
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok());
        program.is_some_and(|p| mdm_ipc::browsers::in_temporary_folder(&p))
    })
}

/// At each start, on a thread of its own (not in a portable copy, which registers nothing, nor an
/// off-screen test run, whose browsers are the person's): a browser's registration that leads
/// nowhere is repaired for this copy, one that works is left alone, whichever copy it names
/// (#818). Linux and macOS, which have no installer step, also register with a browser that has
/// none yet.
pub fn repair_at_start() {
    if mdm_ipc::portable_dir().is_some() || crate::offscreen() {
        return;
    }
    if temporary_copy() {
        tracing::warn!(
            "browser integration: not set up, this copy runs from a temporary folder ({}), \
             deleted once it quits: extract it to a folder of its own",
            std::env::current_exe().unwrap_or_default().display()
        );
        return;
    }
    #[cfg(windows)]
    crate::win32::repair_browsers();
    #[cfg(unix)]
    crate::unix::register_browsers();
}

/// Set it up's runs finished so far: a window that shows the checks reads them again when this
/// changes ([`set_up_finished`]).
static SET_UP_DONE: AtomicU64 = AtomicU64::new(0);
/// A Set it up is running (a second press waits for it).
static SETTING_UP: AtomicBool = AtomicBool::new(false);
/// How long the native host's `--register` has (it writes a few files, or registry keys).
pub(crate) const SET_UP_LIMIT: Duration = Duration::from_secs(20);

/// Register it now, for a copy installed without it (the setup's box unticked), or to take the
/// browsers back from another copy: the native host's own `--register`, for this user, on a
/// thread of its own (a program waited for on the window's thread freezes it for as long as the
/// program takes, #222). The window repaints when it's done, and [`set_up_finished`] tells the
/// checks to read the result, and why it failed if it did.
pub fn set_up(ctx: &egui::Context) {
    if SETTING_UP.swap(true, Ordering::AcqRel) {
        return;
    }
    let ctx = ctx.clone();
    let started = std::thread::Builder::new()
        .name("set-up".into())
        .spawn(move || {
            #[cfg(unix)]
            crate::unix::register_browsers_now(SET_UP_LIMIT, "--register");
            #[cfg(windows)]
            registered_as(
                &std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| {
                        run_host(
                            &exe.with_file_name("misterdownloadmanager-nmhost.exe"),
                            "--register",
                            SET_UP_LIMIT,
                        )
                    }),
            );
            SETTING_UP.store(false, Ordering::Release);
            SET_UP_DONE.fetch_add(1, Ordering::AcqRel);
            ctx.request_repaint();
        });
    if started.is_err() {
        SETTING_UP.store(false, Ordering::Release);
    }
}

/// Set it up's runs finished so far: a window that shows where it stands reads it again when
/// this changes (its read's key).
pub fn set_up_runs() -> u64 {
    SET_UP_DONE.load(Ordering::Acquire)
}

/// One line of Options → General → Test browser integration: what was checked, whether it
/// holds, and the help page's section when it doesn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub ok: bool,
    pub text: String,
    pub why: Option<&'static str>,
    /// A path the check names, on a line of its own under it: shortened in the middle when it
    /// doesn't fit, whole on hover.
    pub detail: Option<String>,
    /// What to do about it, a paragraph under it (macOS's refusal, #1218).
    pub how: Option<String>,
}

/// Test browser integration's checks, read beside the window (#1218): the browsers' folders can
/// take seconds to answer. Read when it opens, and again once a Set it up finished; the window
/// shows the last ones meanwhile, and "Checking..." before the first.
pub struct Tester {
    asked: crate::launch::Asked<(u64, bool), Vec<Check>>,
    /// Options' "Advanced browser integration" as the window opened (the first check).
    advanced: bool,
    /// What reads them ([`checks`]; a test's slow one).
    read: fn(bool) -> Vec<Check>,
}

impl Tester {
    pub fn new(advanced: bool) -> Self {
        Tester {
            asked: crate::launch::Asked::default(),
            advanced,
            read: checks,
        }
    }

    /// The checks, never waiting for the disk (only for the very first read, at most
    /// [`FIRST_WAIT`]); None before the first answer. `repaint` is woken when they come.
    pub fn checks(&self, repaint: Option<&egui::Context>) -> Option<Vec<Check>> {
        let read = self.read;
        self.asked.get_or_last(
            &(set_up_runs(), self.advanced),
            Duration::MAX,
            FIRST_WAIT,
            repaint,
            move |&(_, advanced)| read(advanced),
        )
    }
}

/// The checks in the order a connection needs them, with Options' "Advanced browser
/// integration" `advanced`; the first failing one is what to fix. It reads the browsers'
/// folders: never on the window's thread ([`Tester`]).
pub fn checks(advanced: bool) -> Vec<Check> {
    let check = |ok: bool, text: &str, why: Option<&'static str>| Check {
        ok,
        text: crate::i18n::t(text).to_owned(),
        why: (!ok).then_some(why).flatten(),
        detail: None,
        how: None,
    };
    let mut out = vec![
        check(
            advanced,
            // i18n: a check of Test browser integration (Options → General)
            "Advanced browser integration is on",
            Some("options"),
        ),
        check(
            mdm_ipc::portable_dir().is_none(),
            // i18n: a check of Test browser integration: a portable copy can't be connected
            "This is the installed copy, not a portable one",
            Some("not-installed"),
        ),
    ];
    if temporary_copy() {
        out.push(check(
            false,
            // i18n: a check of Test browser integration: MDM was started from inside a zip (or from the temporary folder), which is deleted after it quits, so it can't be connected to the browsers
            "This copy runs from a temporary folder that will be deleted",
            Some("not-installed"),
        ));
    }
    let registration = registration();
    out.push(match &registration {
        Registration::Ready | Registration::Missing => check(
            registration == Registration::Ready,
            // i18n: a check of Test browser integration: the browsers find MDM's helper program
            "The browsers can find Mister Download Manager",
            Some("not-installed"),
        ),
        Registration::NoBrowser => check(
            false,
            // i18n: a check of Test browser integration (Linux, macOS): MDM found no browser it can connect to (one installed as a Flatpak isn't found)
            "No browser MDM can connect to was found",
            Some("not-installed"),
        ),
        Registration::HelperMissing(path) => {
            tracing::warn!("browser integration: {} is missing", path.display());
            check(
                false,
                // i18n: a check of Test browser integration: the browsers are set up, but the small helper program they start (misterdownloadmanager-nmhost) is gone
                "The helper program the browsers start is missing",
                Some("not-installed"),
            )
        }
        Registration::Gone(manifest) => {
            tracing::warn!("browser integration: {} is gone", manifest.display());
            let folder = manifest.parent().unwrap_or(manifest);
            Check {
                ok: false,
                // i18n: a check of Test browser integration: the browsers are pointed at a copy of MDM that was deleted (often one started from inside a zip); that copy's folder shows on the line under it
                text: crate::i18n::t("The browsers are set up for a copy that is gone:").to_owned(),
                why: Some("not-installed"),
                detail: Some(folder.display().to_string()),
                how: None,
            }
        }
        Registration::Refused(folder) => {
            tracing::warn!(
                "browser integration: macOS refused {} (Operation not permitted)",
                folder.display()
            );
            refused_check(&Refused::at(folder))
        }
    });
    let error = (registration != Registration::Ready)
        .then(register_error)
        .flatten();
    // Setting it up was refused by macOS (#1173): said what it means, once (not again when the
    // read was refused already).
    let refused = error
        .as_deref()
        .and_then(|e| refused_in(e, cfg!(target_os = "macos")));
    if let Some(refused) = refused {
        if !matches!(registration, Registration::Refused(_)) {
            out.push(refused_check(&refused));
        }
    } else if let Some(error) = error {
        out.push(Check {
            ok: false,
            text: crate::i18n::fmt(
                // i18n: a check of Test browser integration: setting it up failed; {error} is the system's own message, in English
                crate::i18n::t("Setting it up failed: {error}"),
                &[("error", &error)],
            ),
            why: Some("not-installed"),
            detail: None,
            how: None,
        });
    }
    let seen = mdm_ipc::extension::seen_browsers();
    if seen.is_empty() {
        out.push(check(
            false,
            // i18n: a check of Test browser integration: no browser's extension has reached MDM
            "No browser's extension has connected yet",
            None,
        ));
    }
    for (browser, version) in seen {
        let name = if browser.is_empty() {
            crate::i18n::t("Your browser").to_owned()
        } else {
            browser
        };
        let text = if version.is_empty() {
            crate::i18n::fmt(
                // i18n: a check of Test browser integration: {browser} is a browser's name
                crate::i18n::t("{browser}: the extension connected"),
                &[("browser", &name)],
            )
        } else {
            crate::i18n::fmt(
                // i18n: a check of Test browser integration: {browser} is a browser's name, {version} the extension's version number
                crate::i18n::t("{browser}: the extension {version} connected"),
                &[("browser", &name), ("version", &version)],
            )
        };
        out.push(Check {
            ok: true,
            text,
            why: None,
            detail: None,
            how: None,
        });
    }
    out
}

/// Test browser integration's line for a folder macOS refused: what it means, the folder, what
/// to do (Set it up is offered, as for every registration that failed).
fn refused_check(refused: &Refused) -> Check {
    Check {
        ok: false,
        text: refused_line(refused),
        why: Some("not-installed"),
        detail: Some(refused.folder.display().to_string()),
        how: Some(refused_how().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #41: Helium says it is Chrome; Zen gives its own name; each row reads its own status.
    #[test]
    fn rows_match_the_names_browsers_report() {
        let seen = vec!["Google Chrome".to_owned(), "Zen Browser".to_owned()];
        assert!(is_seen(&seen, "Helium") && is_seen(&seen, "Google Chrome"));
        assert!(is_seen(&seen, "Zen Browser"));
        assert!(!is_seen(&seen, "Mozilla Firefox") && !is_seen(&seen, "Brave"));
        assert!(is_seen(&["Brave Browser".to_owned()], "Brave"));
    }

    /// Safari's steps: a sentence each, all there, and Safari is named as the native host and
    /// Options name it (the row reads its status by that name).
    #[test]
    fn safari_steps_are_complete() {
        assert_eq!(STEPS.len(), 8);
        assert!(STEPS.iter().all(|s| s.ends_with('.')));
        assert!(is_seen(&[SAFARI.to_owned()], SAFARI));
        assert_eq!(SAFARI, "Apple Safari");
        assert!(
            mdm_ipc::capture::browser_of(
                "Mozilla/5.0 (Macintosh) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15"
            ) == SAFARI
        );
    }

    /// Firefox's family gets the tutorial, whatever the store has.
    #[test]
    fn gecko_rows_are_firefoxs_family() {
        for b in [
            "Mozilla Firefox",
            "Zen Browser",
            "LibreWolf",
            "Floorp",
            "Waterfox",
        ] {
            assert!(is_gecko(b), "{b}");
        }
        for b in ["Google Chrome", "Helium", "Brave", "Your browser", ""] {
            assert!(!is_gecko(b), "{b}");
        }
    }

    /// Long enough for anything a test waits for to come on the slowest runner: a deadlock
    /// breaker, never a time any check is about.
    const STUCK: Duration = Duration::from_secs(30);
    /// How long the fake slow reads take: far longer than a frame may.
    const SLOW: Duration = Duration::from_millis(1500);

    fn slow_status() -> Status {
        std::thread::sleep(SLOW);
        Status {
            connected: true,
            seen: vec!["Google Chrome".into()],
            registered: true,
            refused: None,
        }
    }

    /// #1218: the extension's window never waits for a slow read: before the first answer it
    /// says "Checking..." (None) after a short wait at most, then shows the answer, and keeps
    /// showing it at once while the next read (a second later) is under way.
    #[test]
    fn the_window_never_waits_for_a_slow_read() {
        let mut window = Extension::with_reader(vec![], slow_status);
        let start = std::time::Instant::now();
        assert_eq!(window.follow(None), None);
        assert!(start.elapsed() < SLOW / 2, "{:?}", start.elapsed());
        let start = std::time::Instant::now();
        loop {
            let frame = std::time::Instant::now();
            let shown = window.follow(None).cloned();
            assert!(
                frame.elapsed() < SLOW / 2,
                "a frame waited {:?}",
                frame.elapsed()
            );
            if let Some(shown) = shown {
                assert!(shown.connected && shown.registered);
                break;
            }
            assert!(start.elapsed() < STUCK, "never came");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Over the next reads (each a second after the last came, each 1.5 s long): every
        // frame at once, the last answer shown throughout.
        let start = std::time::Instant::now();
        while start.elapsed() < LOOK_EVERY + SLOW * 2 {
            let frame = std::time::Instant::now();
            assert!(window.follow(None).is_some_and(|s| s.connected));
            assert!(
                frame.elapsed() < SLOW / 2,
                "a frame waited {:?}",
                frame.elapsed()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn slow_checks(advanced: bool) -> Vec<Check> {
        std::thread::sleep(SLOW);
        vec![Check {
            ok: advanced,
            text: "checked".into(),
            why: None,
            detail: None,
            how: None,
        }]
    }

    /// #1218: Test browser integration's checks, the same way: "Checking..." (None) at once,
    /// then the checks.
    #[test]
    fn test_browser_integration_never_waits_for_a_slow_read() {
        let tester = Tester {
            asked: crate::launch::Asked::default(),
            advanced: true,
            read: slow_checks,
        };
        let start = std::time::Instant::now();
        assert_eq!(tester.checks(None), None);
        assert!(start.elapsed() < SLOW / 2, "{:?}", start.elapsed());
        let start = std::time::Instant::now();
        let checks = loop {
            let frame = std::time::Instant::now();
            let checks = tester.checks(None);
            assert!(
                frame.elapsed() < SLOW / 2,
                "a frame waited {:?}",
                frame.elapsed()
            );
            if let Some(checks) = checks {
                break checks;
            }
            assert!(start.elapsed() < STUCK, "never came");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(checks.len() == 1 && checks[0].ok);
    }

    /// #1218, #1173: the native host's error for a folder macOS refused names its browser and
    /// the folder; other errors, other folders and other systems keep the error as it is.
    #[test]
    fn a_folder_macos_refused_is_named_with_its_browser() {
        let chrome = "/Users/a b/Library/Application Support/Google/Chrome/NativeMessagingHosts";
        let error = format!(
            "/Applications/Mister Download Manager.app/Contents/MacOS/misterdownloadmanager-nmhost \
             --repair: exit status: 1: registered com.misterdownloadmanager.host in /Users/a b/\
             Library/Application Support/Vivaldi/NativeMessagingHosts misterdownloadmanager-nmhost: \
             {chrome}: Operation not permitted (os error 1)"
        );
        let refused = refused_in(&error, true).unwrap();
        assert_eq!(refused.browser, "Google Chrome");
        assert_eq!(refused.folder, PathBuf::from(chrome));
        // Not on Linux or Windows, where EPERM means other things.
        assert_eq!(refused_in(&error, false), None);
        // Another error: shown as it is ("Setting it up failed: …").
        assert_eq!(
            refused_in(
                &error.replace(
                    "Operation not permitted (os error 1)",
                    "Permission denied (os error 13)"
                ),
                true
            ),
            None
        );
        // Several folders: the refused one, after another's error.
        let two = "x: /Users/a/Library/Application Support/Zen/NativeMessagingHosts: Permission \
             denied (os error 13); /Users/a/Library/Application Support/Mozilla/\
             NativeMessagingHosts: Operation not permitted (os error 1)";
        let refused = refused_in(two, true).unwrap();
        assert_eq!(refused.browser, "Mozilla Firefox");
        assert_eq!(
            refused.folder,
            PathBuf::from("/Users/a/Library/Application Support/Mozilla/NativeMessagingHosts")
        );
        // A folder outside Application Support: not a browser's.
        assert_eq!(
            refused_in("/tmp/x: Operation not permitted (os error 1)", true),
            None
        );
    }

    /// The browser of a folder, by MDM's table: each of Chrome's channels has a folder of its
    /// own, all Chrome's; Brave's hosts folder is Chrome's; another folder is none.
    #[test]
    fn folders_name_their_browser() {
        let at = |d: &str| browser_of_folder(&format!("/Users/a/Library/Application Support/{d}"));
        assert_eq!(
            at("Google/Chrome/NativeMessagingHosts"),
            Some("Google Chrome")
        );
        assert_eq!(
            at("Google/Chrome Beta/NativeMessagingHosts"),
            Some("Google Chrome")
        );
        assert_eq!(at("Microsoft Edge"), Some("Microsoft Edge"));
        assert_eq!(at("BraveSoftware/Brave-Browser/x"), Some("Brave"));
        assert_eq!(at("Mozilla/NativeMessagingHosts"), Some("Mozilla Firefox"));
        assert_eq!(at("Google/ChromeX/NativeMessagingHosts"), None);
        assert_eq!(at("Something Else/NativeMessagingHosts"), None);
        assert_eq!(browser_of_folder("/home/a/.config/google-chrome"), None);
    }

    /// What macOS's refusal says, in English: the browser named, Apple's setting, the fix.
    #[test]
    fn the_refusal_says_what_to_do() {
        let chrome = Refused::at(std::path::Path::new(
            "/Users/a/Library/Application Support/Google/Chrome/NativeMessagingHosts",
        ));
        let line = refused_line(&chrome);
        let unknown = refused_line(&Refused::default());
        println!("{line}\n{unknown}\n{}", refused_how());
        assert_eq!(
            line,
            "macOS keeps Mister Download Manager out of Google Chrome's folder."
        );
        assert_eq!(
            unknown,
            "macOS keeps Mister Download Manager out of a browser's folder."
        );
        assert!(refused_how().contains("Privacy & Security → Full Disk Access"));
        let check = refused_check(&chrome);
        assert!(!check.ok && check.why == Some("not-installed") && check.how.is_some());
        assert_eq!(
            check.detail.as_deref(),
            Some(chrome.folder.to_str().unwrap())
        );
    }
}
