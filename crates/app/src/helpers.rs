//! The first start's offer of the video helpers (user request 2026-11-01): yt-dlp finds a page's
//! video, ffmpeg joins its picture and sound, and, only where the computer has no JavaScript
//! runtime, Node.js solves YouTube's challenges (#1570; YairDies' idea, #1541). They are asked for
//! once, explained, and downloaded on Yes (through MDM's own engine, checked against their
//! publishers' signed checksums: see mdm_tools); Not now leaves them to Tasks → Download video
//! from a page, which offers them again.

use std::sync::Arc;
use std::sync::Mutex;

use egui::{Pos2, Ui};

use crate::dialogs::{Outcome, window};
use crate::icons::Icon;
use crate::winui::{self, ButtonKind, r};

/// The helpers' download, one after the other (user decision 2026-11-01: one at a time, one
/// progress bar for both).
#[derive(Default)]
struct State {
    /// The helper downloading now: its name, its place (1-based) and how many there are.
    current: Option<(&'static str, usize, usize)>,
    /// The helpers done: name and version.
    ready: Vec<(&'static str, String)>,
    /// The helper that failed, why, and what to do about it (#816; the ones after it are not
    /// tried).
    error: Option<(&'static str, String, Option<String>)>,
    done: bool,
}

struct Job {
    /// The current helper's bytes.
    progress: Arc<mdm_tools::update::Progress>,
    state: Arc<Mutex<State>>,
}

pub struct Helpers {
    dir: Option<std::path::PathBuf>,
    /// Once Yes was pressed.
    job: Option<Job>,
    /// Node.js is offered too: the computer has no JavaScript runtime yt-dlp can use, and MDM
    /// has a Node.js for it. What can be told without running anything when the window opens
    /// (`jsruntime::current_now`); when a runtime on the computer must first be asked for its
    /// version, that is done off the window's thread ([`RuntimeProbe`]) and the section comes
    /// (or not) when it answers, before Yes.
    node: bool,
    /// The runtime lookup waiting for an answer.
    probe: RuntimeProbe,
}

/// A look for the JavaScript runtime yt-dlp would use (`jsruntime::current`), which may run the
/// computer's runtimes (`--version`, up to 20 s each): on a thread of its own, started from a
/// window's `show` (never when the window is made), its answer read back on a later frame.
#[derive(Default)]
struct RuntimeProbe {
    /// It needs doing.
    wanted: bool,
    /// Its answer, once there.
    slot: Option<Arc<Mutex<Option<Option<mdm_tools::jsruntime::Runtime>>>>>,
}

impl RuntimeProbe {
    /// Start it, if wanted and not started yet.
    fn start(
        &mut self,
        dir: Option<std::path::PathBuf>,
        ytdlp: Option<std::path::PathBuf>,
        ctx: &egui::Context,
    ) {
        if !self.wanted || self.slot.is_some() {
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        let (out, wake) = (slot.clone(), ctx.clone());
        std::thread::spawn(move || {
            let found = mdm_tools::jsruntime::current(dir.as_deref(), ytdlp.as_deref());
            *out.lock().unwrap() = Some(found);
            wake.request_repaint();
        });
        self.slot = Some(slot);
    }

    /// Its answer, once (then it is done).
    fn answer(&mut self) -> Option<Option<mdm_tools::jsruntime::Runtime>> {
        let found = self.slot.as_ref()?.lock().unwrap().take()?;
        self.wanted = false;
        self.slot = None;
        Some(found)
    }
}

/// Whether to offer the helpers at this start: never asked, and one of them is missing (ffmpeg
/// counts only where MDM can install one, Windows; elsewhere the system's is used).
pub fn worth_asking() -> bool {
    let Some(dir) = mdm_tools::tools_dir() else {
        return false;
    };
    mdm_tools::installed(&dir).is_none()
        || (cfg!(windows) && mdm_tools::ffmpeg::installed(&dir).is_none())
}

impl Helpers {
    /// Runs no program: a runtime that must be asked is asked from `show`, off its thread.
    pub fn new() -> Self {
        Self::with_dir(mdm_tools::tools_dir())
    }

    /// [`Helpers::new`] for the tools folder `dir`.
    fn with_dir(dir: Option<std::path::PathBuf>) -> Self {
        let mut probe = RuntimeProbe::default();
        let node = mdm_tools::jsruntime::available()
            && dir.as_deref().is_some_and(|d| {
                let ytdlp = mdm_tools::installed(d).map(|i| i.exe);
                match mdm_tools::jsruntime::current_now(Some(d), ytdlp.as_deref()) {
                    Some(found) => found.is_none(),
                    // A runtime is there, to be asked: none offered until it answers.
                    None => {
                        probe.wanted = true;
                        false
                    }
                }
            });
        Helpers {
            dir,
            job: None,
            node,
            probe,
        }
    }

    /// Download the missing helpers one after the other, in the background: they go on when
    /// the window is closed. Try again starts over with the ones still missing.
    fn start(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let mut list = Vec::new();
        if mdm_tools::installed(&dir).is_none() {
            list.push("yt-dlp");
        }
        // ffmpeg: MDM's own on Windows (it has no build elsewhere, where the system's is used).
        if cfg!(windows) && mdm_tools::ffmpeg::installed(&dir).is_none() {
            list.push("ffmpeg");
        }
        if self.node && mdm_tools::node::installed(&dir).is_none() {
            list.push("Node.js");
        }
        let job = Job {
            progress: Arc::default(),
            state: Arc::default(),
        };
        let (progress, state, ctx) = (job.progress.clone(), job.state.clone(), ctx.clone());
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let total = list.len();
            for (k, name) in list.into_iter().enumerate() {
                {
                    use std::sync::atomic::Ordering::Relaxed;
                    progress.got.store(0, Relaxed);
                    progress.total.store(0, Relaxed);
                }
                state.lock().unwrap().current = Some((name, k + 1, total));
                let res = if name == "ffmpeg" {
                    mdm_tools::ffmpeg::install_latest(&dir, &net, Some(&progress)).await
                } else if name == "Node.js" {
                    mdm_tools::jsruntime::install_latest(&dir, &net, Some(&progress)).await
                } else {
                    mdm_tools::install_latest(&dir, &net, Some(&progress)).await
                };
                let mut s = state.lock().unwrap();
                match res {
                    Ok(installed) => s.ready.push((name, installed.version)),
                    Err(e) => {
                        tracing::warn!("helper download failed: {name}: {e}");
                        s.error = Some((
                            name,
                            crate::errors::tools(&e),
                            crate::errors::tools_advice(&e, name),
                        ));
                        break;
                    }
                }
            }
            let mut s = state.lock().unwrap();
            s.current = None;
            s.done = true;
            drop(s);
            ctx.request_repaint();
        });
        self.job = Some(job);
    }

    /// How much taller the sections are than with yt-dlp's and ffmpeg's alone (one more for
    /// Node.js).
    fn rows_extra(&self) -> f32 {
        if self.node { 72.0 } else { 0.0 }
    }

    /// Open while shown (after Yes too, with the downloads' progress); Closed on Not now, Close
    /// or the window's X.
    pub fn show(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Handle) -> Outcome<()> {
        // The computer's runtime, asked off this thread: Node.js offered if none will do (only
        // before Yes, so the downloads' list stays as it was shown).
        let ytdlp = self
            .dir
            .as_deref()
            .and_then(mdm_tools::installed)
            .map(|i| i.exe);
        self.probe.start(self.dir.clone(), ytdlp, ctx);
        if let Some(found) = self.probe.answer()
            && self.job.is_none()
        {
            self.node = mdm_tools::jsruntime::available() && found.is_none();
        }
        let (running, failed) = match &self.job {
            Some(job) => {
                let s = job.state.lock().unwrap();
                (!s.done, s.error.is_some())
            }
            None => (false, false),
        };
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        let mut answer = None;
        // A long translation's subline makes the banner taller (winui::banner).
        let extra = crate::dialogs::grow_extra(ctx, "helpers-banner")
            + crate::dialogs::grow_extra(ctx, "helpers-error")
            + crate::dialogs::grow_extra(ctx, "helpers-advice")
            + self.rows_extra();
        window(
            ctx,
            "helpers",
            // i18n: title of the first start's window that offers yt-dlp and ffmpeg
            "Get ready for video downloads",
            // Taller once Yes is pressed: the progress under the last section.
            [
                520.0,
                if self.job.is_some() { 418.0 } else { 382.0 } + extra,
            ],
            false,
            winui::BG(),
            |ui, o| {
                self.body(ui, o, running, failed, &mut answer);
                match answer.take() {
                    Some(true) => {
                        self.start(rt, ui.ctx());
                        Outcome::Open
                    }
                    Some(false) => Outcome::Closed,
                    None if winui::escape_pressed(ui) => Outcome::Closed,
                    None => Outcome::Open,
                }
            },
        )
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        running: bool,
        failed: bool,
        answer: &mut Option<bool>,
    ) {
        let p = ui.painter().clone();
        let (ink, muted) = (winui::TEXT(), winui::MUTED());
        let w = 520.0;
        // Everything under the sections moves down by the ones added (Node.js).
        let shift = self.rows_extra();
        // The banner: MDM's own logo, what this is for, in one line, and why now.
        let extra = winui::banner(
            ui,
            &p,
            o,
            w,
            "helpers-banner",
            // i18n: the headline of the first start's helpers window
            crate::i18n::t("Download videos from YouTube and 1,000+ sites"),
            if self.node {
                crate::i18n::t(
                    "Three free helpers make it work. Get them now, and saving a video is one click away when you find one.",
                )
            } else {
                crate::i18n::t(
                    "Two free helpers make it work. Get them now, and saving a video is one click away when you find one.",
                )
            },
        );
        // Everything below it moves down with a taller banner.
        let o = o + egui::vec2(0.0, extra);

        let ffmpeg_here = self
            .dir
            .as_deref()
            .and_then(mdm_tools::ffmpeg::installed)
            .is_some();
        let mut rows: Vec<(Icon, &str, String)> = vec![
            (
                Icon::HelperVideo,
                // i18n: helper name and what it does (yt-dlp is a program's name)
                "yt-dlp: finds the video",
                crate::i18n::t(
                    "Reads a page and lists every quality the site offers, up to 4K, from YouTube, Vimeo and over a thousand other sites.",
                )
                .to_owned(),
            ),
            (
                Icon::HelperMerge,
                // i18n: helper name and what it does (ffmpeg is a program's name)
                "ffmpeg: joins picture and sound",
                if cfg!(windows) || ffmpeg_here {
                    crate::i18n::t(
                        "Sites send their best quality as separate video and audio. ffmpeg puts them together into one file that plays anywhere.",
                    )
                    .to_owned()
                } else {
                    crate::i18n::t(
                        "Sites send their best quality as separate video and audio, and ffmpeg joins them. On this system MDM uses yours: install ffmpeg with your package manager.",
                    )
                    .to_owned()
                },
            ),
        ];
        if self.node {
            rows.push((
                Icon::HelperScript,
                // i18n: helper name and what it does (Node.js and YouTube are names)
                "Node.js: unlocks YouTube's qualities",
                crate::i18n::t(
                    "YouTube makes yt-dlp run a piece of JavaScript before it lists its videos, and this computer has nothing to run it with. Node.js does.",
                )
                .to_owned(),
            ));
        }
        rows.push((
            Icon::HelperChecked,
            // i18n: a promise in the helpers window: the helpers are free, official and checked
            "Free, official and checked",
            (if self.node {
                crate::i18n::t(
                    "All three are open source. MDM gets them from their official releases with its own downloader and checks each file's signature before it runs. About 140 MB, once.",
                )
            } else {
                crate::i18n::t(
                    "Both are open source. MDM gets them from their official releases with its own downloader and checks each file's signature before it runs. About 100 MB, once.",
                )
            })
            .to_owned(),
        ));
        for (k, (icon, name, what)) in rows.iter().enumerate() {
            let y = 100.0 + k as f32 * 72.0;
            icon.image(true)
                .paint_at(ui, r(o, 22.0, y + 2.0, 32.0, 32.0));
            let name_rc = winui::text_font(
                &p,
                winui::tahoma_bold(),
                o.x + 68.0,
                o.y + y + 11.0,
                crate::i18n::t(name),
                ink,
            );
            // Painted text a screen reader couldn't see before (#1407).
            winui::readable(ui, name_rc, ("helpers-name", k), crate::i18n::t(name));
            winui::readable_paragraph(
                ui,
                &p,
                o.x + 68.0,
                o.y + y + 29.0,
                what,
                muted,
                o.x + w - 18.0,
                14.0,
                ("helpers-what", k),
            );
        }

        // After Yes: one bar for all, centred under the last section, and what it is doing.
        let mut y = 322.0 + shift;
        if let Some(job) = &self.job {
            let st = job.state.lock().unwrap();
            let f = job.progress.fraction().unwrap_or(0.0);
            let (fraction, line, color) = if let Some((name, error, _)) = &st.error {
                (
                    st.ready.len() as f32 / (st.ready.len() + 1) as f32,
                    crate::i18n::fmt(
                        // i18n: {name} = yt-dlp, ffmpeg or Node.js; {error} = why
                        crate::i18n::t("{name} could not be downloaded: {error}"),
                        &[("name", name), ("error", error)],
                    ),
                    winui::ERROR(),
                )
            } else if st.done {
                let list = st
                    .ready
                    .iter()
                    .map(|(n, v)| format!("{n} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    1.0,
                    crate::i18n::fmt(
                        // i18n: the helpers are downloaded, checked and ready; {list} = names and versions, e.g. "yt-dlp 2026.10.26, ffmpeg 9.0"
                        crate::i18n::t("Ready: {list}"),
                        &[("list", &list)],
                    ),
                    winui::SUCCESS(),
                )
            } else if let Some((name, k, total)) = st.current
                && f >= 1.0
            {
                (
                    k as f32 / total as f32,
                    crate::i18n::fmt(
                        // i18n: a downloaded helper's signature and checksum are being checked; {name} = yt-dlp or ffmpeg
                        crate::i18n::t("Checking {name} ({n} of {total})..."),
                        &[
                            ("name", name),
                            ("n", &crate::i18n::integer(k as u64)),
                            ("total", &crate::i18n::integer(total as u64)),
                        ],
                    ),
                    ink,
                )
            } else if let Some((name, k, total)) = st.current {
                (
                    ((k - 1) as f32 + f) / total as f32,
                    crate::i18n::fmt(
                        // i18n: {name} = yt-dlp, ffmpeg or Node.js; {n} of {total} = which helper of how many; {percent} = how far, e.g. 45%
                        crate::i18n::t("Downloading {name} ({n} of {total})... {percent}"),
                        &[
                            ("name", name),
                            ("n", &crate::i18n::integer(k as u64)),
                            ("total", &crate::i18n::integer(total as u64)),
                            ("percent", &format!("{}%", (f * 100.0).floor())),
                        ],
                    ),
                    ink,
                )
            } else {
                (0.0, crate::i18n::t("Starting...").to_owned(), ink)
            };
            drop(st);
            let bar_w = 320.0;
            winui::meter(
                &p,
                r(o, (w - bar_w) / 2.0, 312.0 + shift, bar_w, 8.0),
                fraction,
            );
            let room = w - 40.0;
            let mut more = 0.0;
            if failed && winui::text_width(&p, &line) > room {
                // A failure is read whole (#816): wrapped, the window growing by its lines past
                // the first.
                let n = winui::readable_paragraph(
                    ui,
                    &p,
                    o.x + 20.0,
                    o.y + 340.0 + shift,
                    &line,
                    color,
                    o.x + w - 20.0,
                    16.0,
                    ("helpers", "status"),
                );
                more = 16.0 * n.saturating_sub(1) as f32;
            } else {
                // Centred on screen (the same place in both directions); cut to the window.
                let shown = winui::ellipsize(&p, &winui::tahoma(), &line, room);
                let tw = winui::text_width(&p, &shown);
                let rc = winui::text_at(
                    &p,
                    winui::tahoma(),
                    (o.x + (w - tw) / 2.0).round(),
                    o.y + 340.0 + shift,
                    &shown,
                    color,
                );
                // Whole for a screen reader too, even when the window cut it (#1407).
                winui::readable(ui, rc, ("helpers", "status"), &line);
                if shown != line {
                    winui::full_on_hover(ui, rc, "helpers-status", &line);
                }
            }
            crate::dialogs::grow_px(ui.ctx(), "helpers-error", more);
            y += 36.0 + more;
        }

        // The way back later, then the answer.
        winui::etched_h(&p, o.x + 16.0, o.y + y, w - 32.0);
        // What to do about the failure (#816), when it says.
        let advice = self.job.as_ref().and_then(|job| {
            let st = job.state.lock().unwrap();
            st.error.as_ref().and_then(|(_, _, a)| a.clone())
        });
        let later = if self.job.is_some() && running {
            crate::i18n::t("They keep downloading if you close this window.")
        } else if self.job.is_some() && !failed {
            crate::i18n::t(
                "All set: on a page with a video, use Tasks → Download video from a page.",
            )
        } else if let Some(advice) = &advice {
            advice.as_str()
        } else if self.job.is_some() {
            crate::i18n::t(
                "Check your connection, then try again, or get them later from the video window.",
            )
        } else {
            crate::i18n::t("Not now? Get them later from Tasks → Download video from a page.")
        };
        // Wrapped: the window grows by the lines past the first.
        let lines = winui::readable_paragraph(
            ui,
            &p,
            o.x + 16.0,
            o.y + y + 20.0,
            later,
            muted,
            o.x + w - 16.0,
            16.0,
            ("helpers", "later"),
        );
        crate::dialogs::grow_px(
            ui.ctx(),
            "helpers-advice",
            16.0 * lines.saturating_sub(1) as f32,
        );
        let y = y + 28.0 + 16.0 * lines.saturating_sub(1) as f32;
        if self.job.is_none() {
            // As wide as their labels in this language (#192), the row ending where it ends in
            // English.
            let [not_now_at, yes_at] = winui::button_row(
                &p,
                &[
                    ("Not now", 196.0, 90.0),
                    ("Yes, download them (recommended)", 292.0, 212.0),
                ],
                6.0,
                16.0,
                360.0,
            )[..] else {
                unreachable!("two buttons")
            };
            let not_now = winui::button(
                ui,
                r(o, not_now_at.0, y, not_now_at.1, 25.0),
                "Not now",
                ButtonKind::Normal,
                false,
                None,
            );
            // i18n: the first start's helpers window: download yt-dlp and ffmpeg (the recommended answer)
            let yes = winui::button(
                ui,
                r(o, yes_at.0, y, yes_at.1, 25.0),
                "Yes, download them (recommended)",
                ButtonKind::Accent,
                false,
                None,
            );
            // The window opens with the focus on its default button: a screen reader says it,
            // not just the title (#1407).
            if ui.memory(|m| m.focused().is_none()) {
                yes.request_focus();
            }
            if yes.clicked() || winui::enter_pressed(ui) {
                *answer = Some(true);
            } else if not_now.clicked() {
                *answer = Some(false);
            }
        } else {
            let [again_at, close_at] = winui::button_row(
                &p,
                &[("Try again", 318.0, 90.0), ("Close", 414.0, 90.0)],
                6.0,
                16.0,
                200.0,
            )[..] else {
                unreachable!("two buttons")
            };
            if failed && !running {
                let again = winui::button(
                    ui,
                    r(o, again_at.0, y, again_at.1, 25.0),
                    "Try again",
                    ButtonKind::Normal,
                    false,
                    None,
                );
                if again.clicked() {
                    *answer = Some(true);
                }
            }
            let close = winui::button(
                ui,
                r(o, close_at.0, y, close_at.1, 25.0),
                "Close",
                ButtonKind::Default,
                false,
                None,
            );
            // When Yes goes away its focus goes with it: Close takes it, so the reader lands
            // somewhere (#1407).
            if ui.memory(|m| m.focused().is_none()) {
                close.request_focus();
            }
            if close.clicked() || winui::enter_pressed(ui) {
                *answer = Some(false);
            }
        }
    }
}

// ---- Video helpers: MDM's own copies or the person's (#51) ------------------------------------

use mdm_tools::autoupdate::{Settled, Tool};
use mdm_tools::chosen::Helper;

/// Helpers (Download video from a page or a gallery → Helpers...): for yt-dlp, ffmpeg and
/// gallery-dl (#58), where MDM takes it from (its own copy, or a program of the person's), and the
/// way to choose another: one copy of each for every program that uses them (#51). Then the
/// JavaScript runtime yt-dlp needs for YouTube (#1448, #1470, #1570): one the person chose, else
/// the computer's own (Deno, Node.js, QuickJS, Bun), else Node.js, MDM's own copy, downloaded and
/// checked here (`mdm_tools::jsruntime`). MDM's own copies are kept up to date
/// (#1544, `mdm_tools::autoupdate`): each row says how its copy stands, with Update and Go back to
/// the previous version; Check for updates checks them all, and the switch below turns the
/// automatic updates off.
pub struct Programs {
    dir: Option<std::path::PathBuf>,
    /// The helpers as found (read again after each change, not every frame: it reads files).
    found: [Option<mdm_tools::Installed>; 3],
    /// The JavaScript runtime yt-dlp uses (`jsruntime::current`), read with `found`: at once when
    /// that runs nothing (`current_now`), else from `probe`, off the window's thread.
    runtime: Option<mdm_tools::jsruntime::Runtime>,
    probe: RuntimeProbe,
    /// MDM's own copy of each tool (`Tool::ALL`'s order) as its updates stand, read with `found`.
    own: [Own; 4],
    /// The update state (`<tools>/updates.json`), read with `found`.
    updates: mdm_tools::autoupdate::State,
    /// `autoupdate::generation()` when they were read: read again when it moves.
    seen: u64,
    /// A chosen program being checked (`--version`), and what it answered.
    checking: Option<(Helper, Answer)>,
    /// MDM's own yt-dlp or gallery-dl being downloaded in place of the system's (#452, #571),
    /// and how that went.
    installing: Option<(Helper, Answer)>,
    /// MDM's own Node.js being downloaded, and how that went.
    installing_runtime: Option<Answer>,
    /// A runtime just chosen being checked, and what it answered.
    checking_runtime: Option<Answer>,
    /// Check for updates, an Update or a Go back under way, and how it went.
    updating: Option<Answer>,
    error: Option<String>,
}

/// MDM's own copy of a tool, as its updates stand.
#[derive(Default)]
struct Own {
    /// The version in use, the one kept to go back to (older), and a newer one waiting for the
    /// downloads using the tool to end.
    current: Option<mdm_tools::Installed>,
    previous: Option<mdm_tools::Installed>,
    staged: Option<mdm_tools::Installed>,
}

const HELPERS: [Helper; 3] = [Helper::YtDlp, Helper::Ffmpeg, Helper::GalleryDl];

/// The tool a helper row is.
fn tool_of(h: Helper) -> Tool {
    match h {
        Helper::YtDlp => Tool::YtDlp,
        Helper::Ffmpeg => Tool::Ffmpeg,
        Helper::GalleryDl => Tool::GalleryDl,
    }
}

/// How long "Updated to …" is said after an update.
const UPDATED_SHOWN: i64 = 3 * 24 * 3600;

/// A check's answer, once it has one (Err: why the program wasn't taken, in words).
type Answer = Arc<Mutex<Option<Result<(), String>>>>;

impl Programs {
    pub fn new() -> Self {
        Self::with_dir(mdm_tools::tools_dir())
    }

    /// [`Programs::new`] for the tools folder `dir`; runs no program (the runtime, when the
    /// computer's must be asked, is asked from `show`, off the window's thread).
    fn with_dir(dir: Option<std::path::PathBuf>) -> Self {
        let mut p = Programs {
            dir,
            found: [None, None, None],
            runtime: None,
            own: Default::default(),
            updates: Default::default(),
            seen: 0,
            checking: None,
            installing: None,
            installing_runtime: None,
            checking_runtime: None,
            updating: None,
            error: None,
            probe: RuntimeProbe::default(),
        };
        p.look();
        p
    }

    fn look(&mut self) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        self.seen = mdm_tools::autoupdate::generation();
        self.found = [
            mdm_tools::installed(dir),
            mdm_tools::ffmpeg::installed(dir),
            mdm_tools::gallery::installed(dir),
        ];
        let ytdlp = self.found[0].as_ref().map(|i| i.exe.as_path());
        match mdm_tools::jsruntime::current_now(Some(dir), ytdlp) {
            Some(found) => {
                self.runtime = found;
                self.probe = RuntimeProbe::default();
            }
            // Asked from `show`, off the window's thread; the row says it is looking meanwhile.
            None => {
                self.probe = RuntimeProbe {
                    wanted: true,
                    slot: None,
                };
            }
        }
        self.updates = mdm_tools::autoupdate::read(dir);
        for t in Tool::ALL {
            use mdm_tools::autoupdate as au;
            let root = t.root(dir);
            let current = au::own(dir, t);
            let newer_than_current = |i: &mdm_tools::Installed, older: bool| {
                current.as_ref().is_some_and(|c| {
                    if older {
                        mdm_tools::autoupdate::is_newer(&c.version, &i.version)
                    } else {
                        mdm_tools::autoupdate::is_newer(&i.version, &c.version)
                    }
                })
            };
            self.own[t as usize] = Own {
                previous: au::previous_in(&root).filter(|p| newer_than_current(p, true)),
                staged: au::staged_in(&root).filter(|s| newer_than_current(s, false)),
                current,
            };
        }
    }

    /// MDM's own Node.js (#1570): downloaded and checked against its release's signed checksums,
    /// from then on the runtime yt-dlp is told of when the computer has none (`mdm_tools::jsruntime`).
    /// It goes on if the window is closed.
    fn install_runtime(
        &mut self,
        dir: std::path::PathBuf,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) {
        let slot: Answer = Arc::new(Mutex::new(None));
        let (out, wake) = (slot.clone(), ctx.clone());
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let res = mdm_tools::jsruntime::install_latest(&dir, &net, None).await;
            if let Err(e) = &res {
                tracing::warn!("Node.js download failed: {e}");
            }
            *out.lock().unwrap() = Some(res.map(|_| ()).map_err(|e| crate::errors::tools(&e)));
            wake.request_repaint();
        });
        self.installing_runtime = Some(slot);
        self.error = None;
    }

    /// MDM's own yt-dlp or gallery-dl in place of the system's (#452, #571): downloaded and
    /// checked as usual, from then on the one used (`mdm_tools::installed`,
    /// `mdm_tools::gallery::installed`). It goes on if the window is closed.
    fn install_own(
        &mut self,
        h: Helper,
        dir: std::path::PathBuf,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) {
        let slot: Answer = Arc::new(Mutex::new(None));
        let (out, wake) = (slot.clone(), ctx.clone());
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let res = if h == Helper::GalleryDl {
                mdm_tools::gallery::install_latest(&dir, &net, None).await
            } else if h == Helper::Ffmpeg {
                mdm_tools::ffmpeg::install_latest(&dir, &net, None).await
            } else {
                mdm_tools::install_latest(&dir, &net, None).await
            };
            *out.lock().unwrap() = Some(res.map(|_| ()).map_err(|e| crate::errors::tools(&e)));
            wake.request_repaint();
        });
        self.installing = Some((h, slot));
        self.error = None;
    }

    /// Check for updates (`tools`: every own copy) or one row's Update, now, in the background:
    /// each newer version checked as its first install was. It goes on if the window is closed.
    fn update(
        &mut self,
        tools: Vec<Tool>,
        dir: std::path::PathBuf,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) {
        let slot: Answer = Arc::new(Mutex::new(None));
        let (out, wake) = (slot.clone(), ctx.clone());
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let mut failed = None;
            for t in tools {
                let now = mdm_tools::autoupdate::unix_now();
                let res = mdm_tools::autoupdate::check(
                    &dir,
                    t,
                    &net,
                    mdm_tools::autoupdate::Why::Asked,
                    now,
                )
                .await;
                wake.request_repaint();
                if let Err(e) = res {
                    failed.get_or_insert_with(|| {
                        // i18n: Helpers: an update check failed; {name} = yt-dlp, ffmpeg, gallery-dl or node, {error} = why
                        crate::i18n::fmt(
                            crate::i18n::t("{name}: {error}"),
                            &[("name", t.name()), ("error", &crate::errors::tools(&e))],
                        )
                    });
                }
            }
            *out.lock().unwrap() = Some(failed.map_or(Ok(()), Err));
            wake.request_repaint();
        });
        self.updating = Some(slot);
        self.error = None;
    }

    /// Go back to the previous version of `t`, off the window's thread (a switch removes older
    /// folders).
    fn go_back(&mut self, t: Tool, dir: std::path::PathBuf, ctx: &egui::Context) {
        let slot: Answer = Arc::new(Mutex::new(None));
        let (out, wake) = (slot.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = mdm_tools::autoupdate::go_back(&dir, t);
            match &res {
                Ok(Settled::Switched(i)) => tracing::info!("{} back to {}", t.name(), i.version),
                Ok(Settled::Staged(i)) => {
                    tracing::info!("{} back to {} once its downloads end", t.name(), i.version)
                }
                Err(e) => tracing::warn!("{}: going back failed: {e}", t.name()),
            }
            *out.lock().unwrap() = Some(res.map(|_| ()).map_err(|e| crate::errors::tools(&e)));
            wake.request_repaint();
        });
        self.updating = Some(slot);
        self.error = None;
    }

    /// Open until Close.
    pub fn show(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Handle) -> Outcome<()> {
        // The runtime, when the computer's must be asked: off this thread.
        let ytdlp = self.found[0].as_ref().map(|i| i.exe.clone());
        self.probe.start(self.dir.clone(), ytdlp, ctx);
        if let Some(found) = self.probe.answer() {
            self.runtime = found;
        }
        let answered = self
            .checking
            .as_ref()
            .and_then(|(_, slot)| slot.lock().unwrap().take());
        if let Some(res) = answered {
            self.error = res.err();
            self.checking = None;
            self.look();
        }
        let installed = self
            .installing
            .as_ref()
            .and_then(|(_, slot)| slot.lock().unwrap().take());
        if let Some(res) = installed {
            self.error = res.err();
            self.installing = None;
            self.look();
        }
        let mut runtime_done = None;
        for slot in [&mut self.installing_runtime, &mut self.checking_runtime] {
            let res = slot.as_ref().and_then(|a| a.lock().unwrap().take());
            if let Some(res) = res {
                *slot = None;
                runtime_done = Some(res);
            }
        }
        if let Some(res) = runtime_done {
            self.error = res.err();
            self.look();
        }
        if let Some(path) = winui::picked(ctx, "helper-js-runtime")
            && let Some(dir) = self.dir.clone()
        {
            let slot: Answer = Arc::new(Mutex::new(None));
            let (out, wake) = (slot.clone(), ctx.clone());
            std::thread::spawn(move || {
                let res = mdm_tools::jsruntime::choose(&dir, &path)
                    .map(|_| ())
                    .map_err(|e| crate::errors::tools(&e));
                *out.lock().unwrap() = Some(res);
                wake.request_repaint();
            });
            self.checking_runtime = Some(slot);
            self.error = None;
        }
        let updated = self
            .updating
            .as_ref()
            .and_then(|slot| slot.lock().unwrap().take());
        if let Some(res) = updated {
            self.error = res.err();
            self.updating = None;
            self.look();
        }
        // An update under way (here or by itself), or done since: the rows follow it.
        if mdm_tools::autoupdate::generation() != self.seen {
            self.look();
        }
        if self.any_update_running() || self.updating.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        for h in HELPERS {
            if let Some(path) = winui::picked(ctx, &format!("helper-{}", h.name()))
                && let Some(dir) = self.dir.clone()
            {
                let slot = Arc::new(Mutex::new(None));
                let (out, wake) = (slot.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let res = mdm_tools::chosen::choose(&dir, h, &path)
                        .map(|_| ())
                        .map_err(|e| crate::errors::tools(&e));
                    *out.lock().unwrap() = Some(res);
                    wake.request_repaint();
                });
                self.checking = Some((h, slot));
                self.error = None;
            }
        }
        window(
            ctx,
            "video-helpers",
            // i18n: window title: where MDM takes its helper programs (yt-dlp, ffmpeg, gallery-dl) from
            "Helpers",
            [500.0, 402.0],
            false,
            winui::BG(),
            |ui, o| self.body(ui, o, rt),
        )
    }

    /// How MDM's own copy of `t` stands, for its row's first line: an update under way, one
    /// waiting, one done lately, one found, up to date. None before its first check.
    fn update_status(&self, tool: Tool) -> Option<String> {
        use crate::i18n::{fmt, t};
        let own = &self.own[tool as usize];
        let current = own.current.as_ref()?;
        if let Some(p) = self
            .dir
            .as_deref()
            .and_then(|d| mdm_tools::autoupdate::checking(d, tool))
        {
            return Some(match p.fraction() {
                Some(f) => fmt(
                    // i18n: Helpers: a newer version of a helper is being downloaded; {percent} = how far, a number
                    t("Downloading the update... {percent}%"),
                    &[("percent", &format!("{:.0}", f * 100.0))],
                ),
                // i18n: Helpers: MDM is asking whether a newer version of a helper exists
                None => t("Checking for updates...").to_owned(),
            });
        }
        if let Some(s) = &own.staged {
            return Some(fmt(
                // i18n: Helpers: a newer version is downloaded and checked, and is used as soon as the downloads using this helper end; {version} = its version
                t("Version {version} is ready: used once its downloads end"),
                &[("version", &s.version)],
            ));
        }
        let state = self.updates.of(tool);
        let now = mdm_tools::autoupdate::unix_now();
        if let Some(u) = &state.updated
            && u.to == current.version
            && (0..UPDATED_SHOWN).contains(&(now - u.at))
        {
            return Some(fmt(
                // i18n: Helpers: MDM updated its own copy of a helper lately; {version} = the new version
                t("Updated to {version}"),
                &[("version", &u.to)],
            ));
        }
        if state.failed.is_some() {
            // i18n: Helpers: the last update check failed (no network...); MDM checks again later by itself
            return Some(t("Couldn't check for updates: MDM tries again later").to_owned());
        }
        match &state.latest {
            Some(latest) if mdm_tools::autoupdate::is_newer(latest, &current.version) => {
                Some(fmt(
                    // i18n: Helpers: a newer version of a helper exists ("Update" downloads it); {version} = its version
                    t("Version {version} available"),
                    &[("version", latest)],
                ))
            }
            // i18n: Helpers: MDM's own copy of a helper is the newest version
            Some(_) => Some(t("Up to date").to_owned()),
            None => None,
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2, rt: &tokio::runtime::Handle) -> Outcome<()> {
        let p = ui.painter().clone();
        for (k, h) in HELPERS.into_iter().enumerate() {
            let y = o.y + 8.0 + k as f32 * 78.0;
            winui::text_font(
                &p,
                winui::tahoma_bold(),
                o.x + 14.0,
                y + 16.0,
                h.name(),
                winui::TEXT(),
            );
            let found = self.found[k].as_ref();
            let chosen = self
                .dir
                .as_deref()
                .and_then(|d| mdm_tools::chosen::chosen(d, h));
            // One of the person's, maybe passed over (a gallery-dl too old, #236).
            let mine = chosen.is_some();
            let too_old = chosen
                .as_ref()
                .filter(|c| h == Helper::GalleryDl && mdm_tools::gallery::too_old(&c.version));
            let checking = self.checking.as_ref().is_some_and(|(c, _)| *c == h);
            let downloading = self.installing.as_ref().is_some_and(|(i, _)| *i == h);
            // Whether MDM downloads a copy of its own on this system and processor: yt-dlp
            // almost everywhere, gallery-dl not on macOS nor Linux on ARM, ffmpeg on 64-bit
            // Windows only (BtbN builds none for 32-bit Windows).
            let has_own = match h {
                Helper::YtDlp => mdm_tools::ytdlp_asset().is_some(),
                Helper::GalleryDl => mdm_tools::gallery::asset().is_some(),
                Helper::Ffmpeg => mdm_tools::ffmpeg::available(),
            };
            // The system's copy may be the person's to keep up to date: MDM's own can replace it
            // (#452, #571), where MDM has one for this system and processor.
            let replaceable = has_own;
            let system = replaceable && found.is_some_and(mdm_tools::Installed::is_system);
            let status = match (too_old, found) {
                _ if downloading => {
                    // i18n: Video helpers: MDM's own yt-dlp is being downloaded
                    crate::i18n::t("Downloading MDM's own copy...").to_owned()
                }
                _ if checking => {
                    // i18n: Video helpers: a program just chosen is being checked
                    crate::i18n::t("Checking the program...").to_owned()
                }
                (Some(old), _) => crate::i18n::fmt(
                    // i18n: Video helpers: the person's own gallery-dl is too old and not used; {version} = its version, {oldest} = the oldest MDM can use
                    crate::i18n::t(
                        "Your own program ({version}) is too old: MDM needs {oldest} or newer",
                    ),
                    &[
                        ("version", &old.version),
                        ("oldest", mdm_tools::gallery::OLDEST),
                    ],
                ),
                (None, Some(i)) if mine => crate::i18n::fmt(
                    // i18n: Video helpers: {path} = the program the person chose, {version} = its version
                    crate::i18n::t("Your own program: {path} ({version})"),
                    &[
                        ("path", &crate::i18n::ltr(&i.exe.display().to_string())),
                        ("version", &i.version),
                    ],
                ),
                (None, Some(i)) if i.is_system() => crate::i18n::fmt(
                    // i18n: Video helpers: {path} = the system's yt-dlp, ffmpeg or gallery-dl
                    crate::i18n::t("Your system's: {path}"),
                    &[("path", &crate::i18n::ltr(&i.exe.display().to_string()))],
                ),
                (None, Some(i)) => crate::i18n::fmt(
                    // i18n: Video helpers: {version} = the version of MDM's own copy
                    crate::i18n::t("Mister Download Manager's own copy, version {version}"),
                    &[("version", &i.version)],
                ),
                // Linux and macOS as before: there the system's package manager (or Homebrew)
                // installs what MDM has no copy of, and MDM finds it.
                // i18n: Video helpers: no copy of this helper yet
                (None, None) if has_own || !cfg!(windows) => {
                    crate::i18n::t("Not installed yet").to_owned()
                }
                // i18n: Video helpers: MDM can't download this helper on this Windows (ffmpeg on 32-bit Windows); "Use another program" is the button below
                (None, None) => crate::i18n::t(
                    "MDM has no copy for this system: install it, or use another program",
                )
                .to_owned(),
            };
            winui::fit(
                ui,
                &p,
                o.x + 14.0,
                y + 36.0,
                &status,
                winui::MUTED(),
                o.x + 486.0,
                ("helper", k),
            );
            // Its updates: MDM's own copy's, or the person's to make (#1544).
            let name_end =
                o.x + 14.0 + winui::text_width_font(&p, winui::tahoma_bold(), h.name()) + 10.0;
            let updates = if found.is_some_and(|i| mine || i.is_system()) {
                // i18n: Helpers: a program the person chose, or the system's copy: MDM doesn't update it, the person (or their package manager) does
                Some(crate::i18n::t("Updated by you").to_owned())
            } else {
                self.update_status(tool_of(h))
            };
            self.update_row(ui, &p, o, y, name_end, tool_of(h), updates, rt);
            let busy = self.busy();
            // As wide as their labels in this language (#192), side by side from the left.
            let [other_at, own_at] = winui::button_row_from_left(
                &p,
                &[
                    ("Use another program...", 14.0, 180.0),
                    ("Use MDM's own copy", 202.0, 180.0),
                ],
                8.0,
                486.0,
                260.0,
            )[..] else {
                unreachable!("two buttons")
            };
            if winui::button(
                ui,
                r(o, other_at.0, y - o.y + 46.0, other_at.1, 21.0),
                // i18n: Video helpers: button: pick a program (yt-dlp or ffmpeg) already on the computer
                "Use another program...",
                if busy {
                    ButtonKind::Disabled
                } else {
                    ButtonKind::Normal
                },
                false,
                None,
            )
            .clicked()
            {
                // i18n: file picker title; {name} = yt-dlp or ffmpeg
                let title =
                    crate::i18n::fmt(crate::i18n::t("Choose {name}"), &[("name", h.name())]);
                winui::start_pick(ui.ctx(), &format!("helper-{}", h.name()), move || {
                    crate::winui::file_dialog(&title).pick_file()
                });
            }
            if winui::button(
                ui,
                r(o, own_at.0, y - o.y + 46.0, own_at.1, 21.0),
                // i18n: Video helpers: button: go back to the copy MDM downloads and checks itself
                "Use MDM's own copy",
                if (mine || system) && !busy {
                    ButtonKind::Normal
                } else {
                    ButtonKind::Disabled
                },
                false,
                None,
            )
            .clicked()
                && let Some(dir) = self.dir.clone()
            {
                if mine {
                    self.error = mdm_tools::chosen::forget(&dir, h)
                        .err()
                        .map(|e| crate::errors::tools(&e));
                    self.look();
                } else {
                    self.install_own(h, dir, rt, ui.ctx());
                }
            }
            // A line under each, the JavaScript runtime's row below them.
            winui::etched_h(&p, o.x + 14.0, o.y + 82.0 + k as f32 * 78.0, 472.0);
        }
        self.runtime_row(ui, &p, o, HELPERS.len(), rt);
        winui::etched_h(&p, o.x + 14.0, o.y + 8.0 + 4.0 * 78.0 + 4.0, 472.0);
        // Automatic updates, on unless turned off (#1544).
        let mut auto = self.updates.auto;
        let enabled = self.dir.is_some();
        winui::checkbox(
            ui,
            o + egui::vec2(14.0, 332.0),
            &mut auto,
            // i18n: Helpers: checkbox: MDM checks for and installs newer versions of its own yt-dlp, ffmpeg, gallery-dl and Node.js by itself
            "Keep MDM's helpers up to date automatically",
            enabled,
        );
        if auto != self.updates.auto
            && let Some(dir) = self.dir.as_deref()
        {
            mdm_tools::autoupdate::set_auto(dir, auto);
            self.updates.auto = auto;
        }
        let [check_at, close_at] = winui::button_row(
            &p,
            &[("Check for updates", 284.0, 123.0), ("Close", 413.0, 77.0)],
            6.0,
            14.0,
            240.0,
        )[..] else {
            unreachable!("two buttons")
        };
        // MDM's earlier Deno counts as its runtime's own copy: its update brings Node.js (#1570).
        let earlier_deno = self.runtime.as_ref().is_some_and(|r| {
            r.from == mdm_tools::jsruntime::Origin::Own
                && r.kind == mdm_tools::jsruntime::Kind::Deno
        });
        let own: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|t| {
                self.own[*t as usize].current.is_some() || (*t == Tool::Node && earlier_deno)
            })
            .collect();
        let y = 370.0;
        // What failed, left of the buttons.
        if let Some(e) = &self.error {
            winui::fit(
                ui,
                &p,
                o.x + 14.0,
                o.y + y + 15.0,
                e,
                winui::ERROR(),
                o.x + check_at.0 - 8.0,
                "helper-error",
            );
        }
        if winui::button(
            ui,
            r(o, check_at.0, y, check_at.1, 21.0),
            // i18n: Helpers: button: look for newer versions of MDM's own helpers now, and install them
            "Check for updates",
            if own.is_empty() || self.busy() || self.any_update_running() {
                ButtonKind::Disabled
            } else {
                ButtonKind::Normal
            },
            false,
            None,
        )
        .clicked()
            && let Some(dir) = self.dir.clone()
        {
            self.update(own, dir, rt, ui.ctx());
        }
        let close = winui::button(
            ui,
            r(o, close_at.0, y, close_at.1, 21.0),
            "Close",
            ButtonKind::Default,
            false,
            None,
        );
        if close.clicked() || winui::escape_pressed(ui) || winui::enter_pressed(ui) {
            Outcome::Done(())
        } else {
            Outcome::Open
        }
    }

    /// A row's update status (`status`, after its name, from `from`) and, for MDM's own copy,
    /// Update and Go back to its previous version, at the row's right end.
    #[allow(clippy::too_many_arguments)]
    fn update_row(
        &mut self,
        ui: &mut Ui,
        p: &egui::Painter,
        o: Pos2,
        y: f32,
        from: f32,
        t: Tool,
        status: Option<String>,
        rt: &tokio::runtime::Handle,
    ) {
        let own = &self.own[t as usize];
        let mut right = o.x + 486.0;
        let mut back = None;
        if own.current.is_some() {
            let back_label = own.previous.as_ref().map(|prev| {
                crate::i18n::fmt(
                    // i18n: Helpers: button: use the previous version of a helper again (if the new one misbehaves); {version} = that version
                    crate::i18n::t("Go back to {version}"),
                    &[("version", &prev.version)],
                )
            });
            // i18n: Helpers: button: check for a newer version of this helper now and install it
            let update_label = crate::i18n::t("Update");
            let mut row: Vec<(&str, f32, f32)> = Vec::new();
            if let Some(l) = &back_label {
                row.push((l.as_str(), 270.0, 130.0));
            }
            row.push((update_label, 406.0, 80.0));
            let placed = winui::button_row(p, &row, 6.0, from - o.x, 200.0);
            let busy = self.busy()
                || self
                    .dir
                    .as_deref()
                    .is_some_and(|d| mdm_tools::autoupdate::checking(d, t).is_some());
            let kind = if busy {
                ButtonKind::Disabled
            } else {
                ButtonKind::Normal
            };
            let mut at = placed.iter();
            if let Some(l) = &back_label
                && let Some(&(x, w)) = at.next()
            {
                right = right.min(o.x + x - 8.0);
                if winui::button(ui, r(o, x, y - o.y + 4.0, w, 21.0), l, kind, false, None)
                    .clicked()
                {
                    back = Some(t);
                }
            }
            if let Some(&(x, w)) = at.next() {
                right = right.min(o.x + x - 8.0);
                if winui::button(
                    ui,
                    r(o, x, y - o.y + 4.0, w, 21.0),
                    update_label,
                    kind,
                    false,
                    None,
                )
                .clicked()
                    && let Some(dir) = self.dir.clone()
                {
                    self.update(vec![t], dir, rt, ui.ctx());
                }
            }
        }
        if let Some(s) = &status {
            winui::fit(
                ui,
                p,
                from,
                y + 16.0,
                s,
                winui::MUTED(),
                right,
                ("helper-update", t as usize),
            );
        }
        if let (Some(t), Some(dir)) = (back, self.dir.clone()) {
            self.go_back(t, dir, ui.ctx());
        }
    }

    /// A check or a download is under way: the buttons wait.
    fn busy(&self) -> bool {
        self.checking.is_some()
            || self.installing.is_some()
            || self.installing_runtime.is_some()
            || self.checking_runtime.is_some()
            || self.updating.is_some()
    }

    /// An update check runs in this copy of MDM (the window's, or MDM's own by itself).
    fn any_update_running(&self) -> bool {
        self.dir.as_deref().is_some_and(|d| {
            Tool::ALL
                .iter()
                .any(|t| mdm_tools::autoupdate::checking(d, *t).is_some())
        })
    }

    /// The JavaScript runtime's row, the `k`th (#1448, #1470, #1570): YouTube makes yt-dlp solve a
    /// JavaScript challenge, and without a runtime its look-ups are slow, list few qualities or
    /// fail. It says which runtime yt-dlp uses and where it comes from: one the person chose
    /// (**Use another program...**, any runtime yt-dlp supports: YairDies' idea, #1541), the
    /// computer's own, or MDM's own Node.js, which **Use MDM's own copy** downloads where the
    /// computer has none (and which replaces the Deno earlier versions of MDM installed).
    fn runtime_row(
        &mut self,
        ui: &mut Ui,
        p: &egui::Painter,
        o: Pos2,
        k: usize,
        rt: &tokio::runtime::Handle,
    ) {
        use mdm_tools::jsruntime::{Kind, Origin};
        let y = o.y + 8.0 + k as f32 * 78.0;
        // i18n: Video helpers: the row of the JavaScript runtime (Node.js, Deno...) yt-dlp needs for YouTube's videos
        let title = crate::i18n::t("JavaScript runtime (for YouTube)");
        winui::text_font(
            p,
            winui::tahoma_bold(),
            o.x + 14.0,
            y + 16.0,
            title,
            winui::TEXT(),
        );
        let runtime = self.runtime.clone();
        // MDM's own Node.js, the one in use.
        let own_node = runtime
            .as_ref()
            .is_some_and(|r| r.from == Origin::Own && r.kind == Kind::Node);
        // What it is for, in one line after its name; for MDM's own copy, Update and Go back there
        // instead, and how its updates stand after its version below (the title is long in some
        // languages, the room beside it short).
        let after = winui::text_width_font(p, winui::tahoma_bold(), title) + 10.0;
        let updates = own_node.then(|| self.update_status(Tool::Node)).flatten();
        if own_node {
            self.update_row(ui, p, o, y, o.x + 14.0 + after, Tool::Node, None, rt);
        } else {
            winui::fit(
                ui,
                p,
                o.x + 14.0 + after,
                y + 16.0,
                // i18n: Video helpers: what the JavaScript runtime is for, after its name (yt-dlp is a program's name)
                crate::i18n::t("lets yt-dlp list all of YouTube's qualities"),
                winui::MUTED(),
                o.x + 486.0,
                ("helper-what", k),
            );
        }
        let available = mdm_tools::jsruntime::available();
        let path_of =
            |r: &mdm_tools::jsruntime::Runtime| crate::i18n::ltr(&r.exe.display().to_string());
        let named = |r: &mdm_tools::jsruntime::Runtime| match &r.version {
            Some(v) => format!("{} {v}", r.kind.label()),
            None => r.kind.label().to_owned(),
        };
        let looking = self.probe.wanted;
        let status = if self.installing_runtime.is_some() {
            crate::i18n::t("Downloading MDM's own copy...").to_owned()
        } else if looking {
            // i18n: Video helpers: MDM is asking the JavaScript runtimes on the computer (Node.js, Deno...) for their versions, to see which one yt-dlp can use
            crate::i18n::t("Looking for a JavaScript runtime on this computer...").to_owned()
        } else if self.checking_runtime.is_some() {
            crate::i18n::t("Checking the program...").to_owned()
        } else if let Some(r) = &runtime {
            match (r.from, r.kind) {
                (Origin::Chosen, _) => crate::i18n::fmt(
                    // i18n: Video helpers: the JavaScript runtime the person chose; {runtime} = its name and version ("Node.js 24.11.0"), {path} = where it is
                    crate::i18n::t("{runtime}, your own program: {path}"),
                    &[("runtime", &named(r)), ("path", &path_of(r))],
                ),
                (Origin::System, _) => crate::i18n::fmt(
                    // i18n: Video helpers: the computer's own JavaScript runtime, used as it is (MDM needn't download one); {runtime} = its name and version ("Node.js 24.11.0", "Deno 2.5.6"), {path} = where it is
                    crate::i18n::t("{runtime}, your system's, nothing to download: {path}"),
                    &[("runtime", &named(r)), ("path", &path_of(r))],
                ),
                (Origin::Own, Kind::Node) => {
                    let own = crate::i18n::fmt(
                        // i18n: Video helpers: MDM's own Node.js is the JavaScript runtime; {runtime} = "Node.js" and its version
                        crate::i18n::t("{runtime}, Mister Download Manager's own copy"),
                        &[("runtime", &named(r))],
                    );
                    match &updates {
                        Some(u) => format!("{own} · {u}"),
                        None => own,
                    }
                }
                (Origin::Own, _) => crate::i18n::fmt(
                    // i18n: Video helpers: the Deno an earlier version of MDM installed, still used until MDM's own Node.js replaces it (by the next update, or Use MDM's own copy); {runtime} = "Deno" and its version
                    crate::i18n::t(
                        "{runtime}, MDM's earlier copy: Node.js takes its place at the next update",
                    ),
                    &[("runtime", &named(r))],
                ),
            }
        } else if available {
            crate::i18n::t("Not installed yet").to_owned()
        } else {
            // i18n: Video helpers: MDM can't download Node.js on this system (an unusual processor); Node.js and Deno are program names; "Use another program" is the button below
            crate::i18n::t(
                "MDM has no copy for this system: install Node.js or Deno, or use another program",
            )
            .to_owned()
        };
        winui::fit(
            ui,
            p,
            o.x + 14.0,
            y + 36.0,
            &status,
            winui::MUTED(),
            o.x + 486.0,
            ("helper", k),
        );
        let chosen = runtime.as_ref().is_some_and(|r| r.from == Origin::Chosen);
        // MDM's own copy, when it is what the button would bring: back from a chosen runtime,
        // or downloaded where the computer has none (or only MDM's earlier Deno).
        let own_possible = chosen
            || (available
                && runtime
                    .as_ref()
                    .is_none_or(|r| r.from == Origin::Own && r.kind != Kind::Node));
        let busy = self.busy() || looking;
        let [other_at, own_at] = winui::button_row_from_left(
            p,
            &[
                ("Use another program...", 14.0, 180.0),
                ("Use MDM's own copy", 202.0, 180.0),
            ],
            8.0,
            486.0,
            260.0,
        )[..] else {
            unreachable!("two buttons")
        };
        if winui::button(
            ui,
            r(o, other_at.0, y - o.y + 46.0, other_at.1, 21.0),
            "Use another program...",
            if busy {
                ButtonKind::Disabled
            } else {
                ButtonKind::Normal
            },
            false,
            None,
        )
        .clicked()
        {
            // i18n: file picker title: choose a JavaScript runtime's program (node.exe, deno.exe, qjs.exe, bun.exe)
            let title =
                crate::i18n::t("Choose a JavaScript runtime (Node.js, Deno, QuickJS or Bun)")
                    .to_owned();
            winui::start_pick(ui.ctx(), "helper-js-runtime", move || {
                crate::winui::file_dialog(&title).pick_file()
            });
        }
        if winui::button(
            ui,
            r(o, own_at.0, y - o.y + 46.0, own_at.1, 21.0),
            "Use MDM's own copy",
            if own_possible && !busy {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked()
            && let Some(dir) = self.dir.clone()
        {
            if chosen {
                self.error = mdm_tools::jsruntime::forget(&dir)
                    .err()
                    .map(|e| crate::errors::tools(&e));
                self.look();
                // Nothing found once the choice is gone: MDM's own is downloaded (once the
                // computer's runtimes, if they must be asked, have answered: Use MDM's own copy
                // again then).
                if self.runtime.is_none() && !self.probe.wanted && available {
                    self.install_runtime(dir, rt, ui.ctx());
                }
            } else {
                self.install_runtime(dir, rt, ui.ctx());
            }
        }
    }
}

/// MDM's own helpers kept up to date while it runs (#1544): a few seconds after the start, then
/// every [`Keeper::EVERY`], the checks that are due (`mdm_tools::autoupdate::run_due`: each tool
/// at most as often as its own interval, remembered across starts), off the window's thread.
#[derive(Default)]
pub struct Keeper {
    /// When the last round started.
    last: Option<std::time::Instant>,
    /// A round is running.
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl Keeper {
    /// How often what is due is looked at.
    pub const EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);

    /// Start a round when one is due: `started` is when MDM started. Off-screen test runs (and
    /// tests) only when MDM_HELPER_UPDATES=1 says so: their tools folder is a test's.
    pub fn tick(
        &mut self,
        rt: &tokio::runtime::Handle,
        started: std::time::Instant,
        ctx: Option<&egui::Context>,
    ) {
        use std::sync::atomic::Ordering;
        let asked = std::env::var_os("MDM_HELPER_UPDATES").is_some_and(|v| v == "1");
        if (cfg!(test) || crate::offscreen()) && !asked {
            return;
        }
        let first_at = crate::help::CHECK_AFTER_START;
        let wait = match self.last {
            None => first_at.saturating_sub(started.elapsed()),
            Some(at) => Self::EVERY.saturating_sub(at.elapsed()),
        };
        if !wait.is_zero() || self.running.load(Ordering::Acquire) {
            if let Some(ctx) = ctx
                && !wait.is_zero()
            {
                ctx.request_repaint_after(wait);
            }
            return;
        }
        let Some(dir) = mdm_tools::tools_dir() else {
            return;
        };
        self.last = Some(std::time::Instant::now());
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let now = mdm_tools::autoupdate::unix_now();
            for (tool, found) in mdm_tools::autoupdate::run_due(&dir, &net, now).await {
                tracing::info!("helper update check: {}: {found:?}", tool.name());
            }
            running.store(false, Ordering::Release);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening the first start's window or Helpers runs no program on the window's thread: a
    /// runtime on the computer that must be asked for its version is asked from `show`, on a
    /// thread of its own. (A runtime chosen in the tools folder, the one case told at once, is
    /// read from its file.)
    #[test]
    fn opening_the_helpers_windows_runs_no_program() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        let before = mdm_tools::jsruntime::probes();
        let first = Helpers::with_dir(Some(tools.clone()));
        let helpers = Programs::with_dir(Some(tools.clone()));
        assert_eq!(mdm_tools::jsruntime::probes(), before);
        // Whatever this computer has, it is either known now or left to the probe.
        assert!(first.probe.slot.is_none() && helpers.probe.slot.is_none());
        if helpers.probe.wanted {
            assert_eq!(helpers.runtime, None);
        }
    }
}
