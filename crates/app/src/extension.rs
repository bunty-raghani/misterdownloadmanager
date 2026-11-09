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
    /// A browser's extension has reached the app since the window opened.
    connected: bool,
    /// The browsers whose extension has (Options' names, mdm_ipc::extension::seen_browsers).
    seen: Vec<String>,
    /// The browsers can find the app (the native host is registered).
    registered: bool,
    /// When the status files were read last (once a second, not every frame).
    looked: std::time::Instant,
    /// Set it up's runs seen finished ([`set_up_finished`]).
    set_up_seen: u64,
    /// The steps that add Safari's extension, while that window is open.
    steps: Option<SafariSteps>,
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
        Extension {
            browsers,
            connected: mdm_ipc::extension::seen(),
            seen: seen_names(),
            registered: registered(),
            looked: std::time::Instant::now(),
            set_up_seen: 0,
            steps: None,
        }
    }

    /// The Safari steps, open at once (`/show safari-steps`: screenshots).
    pub fn open_safari_steps(&mut self) {
        self.steps = Some(SafariSteps);
    }

    /// Open until closed; Done when "Don't ask again" was pressed.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        // The host marks the extension seen when a browser starts it: look again now and then
        // (each browser's own, as each adds it).
        if self.looked.elapsed() >= Duration::from_secs(1) || set_up_finished(&mut self.set_up_seen)
        {
            self.looked = std::time::Instant::now();
            self.connected = mdm_ipc::extension::seen();
            self.seen = seen_names();
            self.registered = registered();
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        let h = BANNER + 16.0 + self.browsers.len() as f32 * ROW + 104.0;
        // A long translation's subline makes the banner taller (winui::banner).
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
            if is_seen(&self.seen, name) {
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
        let y = h - 84.0;
        if !self.registered {
            // As wide as its label in this language (#192); the paragraph ends 10 px before it.
            let (fix_x, fix_w) =
                winui::buttons_to_right(&p, W - 18.0, 8.0, &[("Set it up", 110.0)], 180.0)[0];
            winui::paragraph(
                &p,
                o.x + 18.0,
                o.y + y,
                crate::i18n::t(
                    "Browser integration isn't set up on this computer, so no browser can reach Mister Download Manager yet.",
                ),
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
        let (line, color) = if !self.registered {
            (String::new(), muted)
        } else if self.connected {
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
            winui::paragraph(&p, o.x + 18.0, o.y + y, &line, color, o.x + W - 18.0, 14.0);
        }
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
        let row: Vec<(&str, f32)> = if self.connected {
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
        if !self.connected {
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
    HelperMissing(std::path::PathBuf),
}

/// Where browser integration's registration stands: the native host's manifests where the
/// browsers look, and the helper they name.
pub fn registration() -> Registration {
    #[cfg(windows)]
    return crate::win32::host_registration();
    #[cfg(unix)]
    return crate::unix::host_registration();
}

/// Is browser integration registered on this computer, ready for the browsers?
pub fn registered() -> bool {
    registration() == Registration::Ready
}

/// Why the last registration failed, in the system's words (Linux and macOS, where MDM
/// registers itself at each start).
fn register_error() -> Option<String> {
    #[cfg(windows)]
    return None;
    #[cfg(unix)]
    return crate::unix::register_error();
}

/// Set it up's runs finished so far: a window that shows the checks reads them again when this
/// changes ([`set_up_finished`]).
static SET_UP_DONE: AtomicU64 = AtomicU64::new(0);
/// A Set it up is running (a second press waits for it).
static SETTING_UP: AtomicBool = AtomicBool::new(false);
/// How long the native host's `--register` has (it writes a few files, or registry keys).
const SET_UP_LIMIT: Duration = Duration::from_secs(20);

/// Register it now, for a copy installed without it (the setup's box unticked): the native
/// host's own `--register`, for this user, on a thread of its own (a program waited for on the
/// window's thread freezes it for as long as the program takes, #222). The window repaints when
/// it's done, and [`set_up_finished`] tells the checks to read the result.
pub fn set_up(ctx: &egui::Context) {
    if SETTING_UP.swap(true, Ordering::AcqRel) {
        return;
    }
    let ctx = ctx.clone();
    let started = std::thread::Builder::new()
        .name("set-up".into())
        .spawn(move || {
            #[cfg(unix)]
            crate::unix::register_browsers_now(SET_UP_LIMIT);
            #[cfg(windows)]
            if let Ok(exe) = std::env::current_exe() {
                crate::launch::run_within(
                    std::process::Command::new(
                        exe.with_file_name("misterdownloadmanager-nmhost.exe"),
                    )
                    .arg("--register"),
                    SET_UP_LIMIT,
                );
            }
            SETTING_UP.store(false, Ordering::Release);
            SET_UP_DONE.fetch_add(1, Ordering::AcqRel);
            ctx.request_repaint();
        });
    if started.is_err() {
        SETTING_UP.store(false, Ordering::Release);
    }
}

/// Whether a Set it up finished since `seen` was last given here (it is then brought up to
/// date): the caller reads the checks again.
pub fn set_up_finished(seen: &mut u64) -> bool {
    let done = SET_UP_DONE.load(Ordering::Acquire);
    std::mem::replace(seen, done) != done
}

/// One line of Options → General → Test browser integration: what was checked, whether it
/// holds, and the help page's section when it doesn't.
pub struct Check {
    pub ok: bool,
    pub text: String,
    pub why: Option<&'static str>,
}

/// The checks in the order a connection needs them; the first failing one is what to fix.
pub fn checks(settings: &crate::settings::Settings) -> Vec<Check> {
    let check = |ok: bool, text: &str, why: Option<&'static str>| Check {
        ok,
        text: crate::i18n::t(text).to_owned(),
        why: (!ok).then_some(why).flatten(),
    };
    let mut out = vec![
        check(
            settings.advanced_integration,
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
    });
    if registration != Registration::Ready
        && let Some(error) = register_error()
    {
        out.push(Check {
            ok: false,
            text: crate::i18n::fmt(
                // i18n: a check of Test browser integration: setting it up failed; {error} is the system's own message, in English
                crate::i18n::t("Setting it up failed: {error}"),
                &[("error", &error)],
            ),
            why: Some("not-installed"),
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
        });
    }
    out
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
}
