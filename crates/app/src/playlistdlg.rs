//! A playlist's or a channel's videos in Download video from a page (#69): listed as they come
//! (mdm_tools::playlist, streamed and bounded), shown as a checklist, one quality for all, added
//! as one download each, in a folder named after the list.
//!
//! Built for lists of six and of six thousand alike: the entries arrive in the background and
//! are taken a frame at a time; only the rows in view are drawn (winui::row_list); a list whose
//! size isn't known (a channel) is listed a page at a time, with Load more; adding thousands is
//! asked first; and the app adds them in batches (App::add_playlist).

use std::path::{Path, PathBuf};

use egui::{Pos2, Ui, pos2};
use mdm_tools::playlist::{Entry, Listing, PAGE};
use mdm_tools::resolve::ListPage;

use crate::dialogs::{Confirm, Outcome};
use crate::winui::{self, ButtonKind, r};

/// Adding more than this many downloads at once is asked first.
pub const CONFIRM_ABOVE: usize = 500;

/// What Download adds: one video download per chosen entry, all alike.
pub struct PlaylistAdd {
    /// Each video's page and title, in the list's order.
    pub entries: Vec<(url::Url, String)>,
    /// yt-dlp's `-f` for every one of them.
    pub selector: String,
    /// The file type the name is made with (yt-dlp may save another).
    pub ext: &'static str,
    /// Where they go: Save to, then a folder named after the list.
    pub folder: PathBuf,
    /// The category each is listed under: Music for a sound-only quality (#611), else Video.
    pub category: crate::model::Category,
    pub start_now: bool,
    /// What the browser's request for the page carried, for each look-up.
    pub context: mdm_ipc::Command,
}

/// One of the qualities offered for a whole list (its videos aren't looked up one by one).
struct Pick {
    label: String,
    selector: String,
    ext: &'static str,
}

impl Pick {
    fn sound_only(&self) -> bool {
        mdm_tools::resolve::sound_only(&self.selector, self.ext)
    }
}

/// The heights offered, as "Up to …p": each video at that height, or the best below it.
const HEIGHTS: [u32; 6] = [2160, 1440, 1080, 720, 480, 360];
/// The one chosen at first: Up to 1080p.
const DEFAULT_PICK: usize = 3;

fn picks(merge: bool) -> Vec<Pick> {
    let mut out = vec![Pick {
        // i18n: a quality in Download video for a whole playlist: each video's best
        label: crate::i18n::t("Best quality").to_owned(),
        selector: if merge { "bv*+ba/b" } else { "b" }.into(),
        ext: "mp4",
    }];
    for h in HEIGHTS {
        out.push(Pick {
            label: crate::i18n::fmt(
                // i18n: a quality in Download video for a whole playlist: each video at this height or the nearest below; {height} is a number (1080)
                crate::i18n::t("Up to {height}p"),
                &[("height", &h.to_string())],
            ),
            selector: if merge {
                format!("bv*[height<={h}]+ba/b[height<={h}]")
            } else {
                format!("b[height<={h}]")
            },
            ext: "mp4",
        });
    }
    out.push(Pick {
        // i18n: a quality in Download video for a whole playlist: only the sound, as the site has it (not converted)
        label: crate::i18n::t("Audio only, original").to_owned(),
        selector: "ba".into(),
        ext: "m4a",
    });
    // MP3 (ffmpeg converts it), as a single video offers it: VBR, then each bitrate.
    if merge {
        use mdm_tools::resolve::{AUDIO_ONLY, MP3, MP3_RATES};
        let vbr = std::iter::once((format!("{AUDIO_ONLY} MP3 VBR"), 0));
        let rates = MP3_RATES
            .into_iter()
            .map(|k| (format!("{AUDIO_ONLY} MP3 {k} kbps"), k));
        for (label, kbps) in vbr.chain(rates) {
            out.push(Pick {
                label: crate::videodlg::choice_label(&label),
                selector: format!("{MP3}{kbps}"),
                ext: "mp3",
            });
        }
        // Opus (YouTube's own moved into an .opus file) and FLAC, as a single video offers
        // them (#70).
        use mdm_tools::resolve::{FLAC, OPUS};
        for (label, selector, ext) in [
            (format!("{AUDIO_ONLY} OPUS"), OPUS, "opus"),
            (format!("{AUDIO_ONLY} FLAC"), FLAC, "flac"),
        ] {
            out.push(Pick {
                label: crate::videodlg::choice_label(&label),
                selector: selector.into(),
                ext,
            });
        }
    }
    out
}

pub struct PlaylistView {
    exe: PathBuf,
    page: String,
    browser: mdm_tools::proc::FromBrowser,
    title: String,
    /// The list's size, when the site says.
    total: Option<u64>,
    /// Its size wasn't known when it was looked up (a channel): listed a page at a time.
    paged: bool,
    listing: Option<Listing>,
    /// How many entries the running listing has handed over so far.
    taken: usize,
    /// The last page came full: there may be more (Load more).
    more: bool,
    capped: bool,
    entries: Vec<Entry>,
    checked: Vec<bool>,
    chosen: usize,
    rows: winui::RowList,
    pick: usize,
    /// The file type the chosen quality gives (each row shows its icon).
    ext: &'static str,
    /// The chosen quality saves only the sound (#611: Save to is then the Music category's).
    sound: bool,
    error: Option<String>,
    confirm: Option<(Confirm, bool)>,
}

impl PlaylistView {
    pub fn start(
        exe: &Path,
        page: &str,
        browser: mdm_tools::proc::FromBrowser,
        list: ListPage,
    ) -> Self {
        let mut v = PlaylistView {
            exe: exe.to_owned(),
            page: page.to_owned(),
            browser,
            title: list.title,
            paged: list.total.is_none(),
            total: list.total,
            listing: None,
            taken: 0,
            more: false,
            capped: false,
            entries: Vec::new(),
            checked: Vec::new(),
            chosen: 0,
            rows: Default::default(),
            pick: DEFAULT_PICK,
            ext: "mp4",
            sound: false,
            error: None,
            confirm: None,
        };
        v.list_from(1);
        v
    }

    /// A listed playlist without yt-dlp or the network (`/show video-playlist`: the window's
    /// check in every language, its screenshots), its quality the one at position `pick` in the
    /// list (the same in every language; not a number: the default). Without ffmpeg the list
    /// ends at "Audio only, original" (7).
    pub fn sample(pick: &str) -> Self {
        let titles = [
            ("Segmented downloads, explained", Some(754.0), false),
            ("Resuming a broken download", Some(512.0), false),
            ("Torrents and magnet links", Some(1103.0), false),
            ("A private video", None, true),
            ("Queues and the scheduler", Some(689.0), false),
            ("Video pages and subtitles", Some(1460.0), false),
        ];
        let entries: Vec<Entry> = titles
            .iter()
            .enumerate()
            .map(|(i, (title, duration, unavailable))| Entry {
                index: i as u64 + 1,
                url: format!("https://video.example/watch/{}", i + 1),
                title: (*title).into(),
                duration: *duration,
                unavailable: *unavailable,
            })
            .collect();
        let checked: Vec<bool> = entries.iter().map(|e| !e.unavailable).collect();
        let picks = picks(true);
        let pick = pick
            .parse::<usize>()
            .ok()
            .filter(|i| *i < picks.len())
            .unwrap_or(DEFAULT_PICK);
        PlaylistView {
            exe: PathBuf::new(),
            page: "https://video.example/playlist".into(),
            browser: Default::default(),
            title: "Download managers, a course".into(),
            total: Some(entries.len() as u64),
            paged: false,
            listing: None,
            taken: entries.len(),
            more: false,
            capped: false,
            chosen: checked.iter().filter(|c| **c).count(),
            entries,
            checked,
            rows: Default::default(),
            pick,
            ext: picks[pick].ext,
            sound: picks[pick].sound_only(),
            error: None,
            confirm: None,
        }
    }

    /// List from entry `from`: to the end when the size is known, else a page.
    fn list_from(&mut self, from: u64) {
        let to = self.paged.then(|| from + PAGE - 1);
        match Listing::start(&self.exe, &self.page, &self.browser, from, to) {
            Ok(l) => {
                self.listing = Some(l);
                self.taken = 0;
                self.more = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn listing(&self) -> bool {
        self.listing.is_some()
    }

    /// Take what the listing has brought since the last frame.
    fn take(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.listing else { return };
        let p = l.progress();
        let new = l.entries_from(self.taken);
        self.taken += new.len();
        for e in new {
            self.checked.push(!e.unavailable);
            self.chosen += usize::from(!e.unavailable);
            self.entries.push(e);
        }
        if self.total.is_none() {
            self.total = p.total;
        }
        if self.title.trim().is_empty()
            && let Some(t) = p.title
        {
            self.title = t;
        }
        if p.done {
            let n = self.entries.len();
            self.capped |= p.capped;
            // More to list: a channel's page that came full (or broke off), a list short of the
            // size the site says (yt-dlp failed part-way). Never past MDM's most.
            let page_full = self.taken as u64 >= PAGE && self.total.is_none_or(|t| (n as u64) < t);
            self.more = n < mdm_tools::playlist::MAX_ENTRIES
                && if self.paged {
                    page_full || p.error.is_some()
                } else {
                    self.short()
                };
            self.error = match p.error {
                // What was listed stays; where it stopped and why is said, never silently.
                Some(e) if n > 0 => Some(crate::i18n::fmt(
                    // i18n: in Download video, yt-dlp stopped part-way through a playlist; {n} = how many were listed, {error} = why
                    crate::i18n::t("Listing stopped after {n} videos: {error}"),
                    &[("n", &crate::i18n::integer(n as u64)), ("error", &e)],
                )),
                Some(e) => Some(e),
                // i18n: in Download video, a playlist or channel that lists no video
                None if n == 0 => Some(crate::i18n::t("This list has no videos").into()),
                None => None,
            };
            self.listing = None;
        } else {
            // Entries keep coming: draw again soon, not on the next mouse move.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    pub fn chosen(&self) -> usize {
        self.chosen
    }

    /// The quality chosen saves only the sound.
    pub fn sound_only(&self) -> bool {
        self.sound
    }

    /// The quality chosen converts the sound (MP3, Opus, FLAC), which keeps no chapters (#827).
    pub fn converts(&self) -> bool {
        let picks = picks(true);
        picks
            .get(self.pick.min(picks.len().saturating_sub(1)))
            .is_some_and(|p| mdm_tools::resolve::audio_conversion(&p.selector).is_some())
    }

    /// Fewer listed than the site says the list has.
    fn short(&self) -> bool {
        self.total.is_some_and(|t| (self.entries.len() as u64) < t)
    }

    /// The question before adding many is open.
    pub fn asking(&self) -> bool {
        self.confirm.is_some()
    }

    /// The status line, up to English x `status` (none: a helper's install has the line), then
    /// the checklist and its buttons, between the address and Save to.
    pub fn middle(&mut self, ui: &mut Ui, o: Pos2, status: Option<f32>) {
        self.take(ui.ctx());
        if let Some(right) = status {
            self.status_line(ui, o, right);
        }
        self.notes(ui, o);
        self.checklist(ui, o);
        self.buttons(ui, o, self.entries.len());
    }

    /// Listing (a spinner, how far) or listed (the title, how many).
    fn status_line(&self, ui: &mut Ui, o: Pos2, right: f32) {
        let p = ui.painter().clone();
        let line_y = o.y + 70.0;
        let n = self.entries.len();
        let status = if self.listing.is_some() {
            crate::torrentdlg::spinner(ui, pos2(o.x + 18.0, line_y - 4.0), 6.0);
            match self.total {
                Some(t) => crate::i18n::fmt(
                    // i18n: in Download video, while a playlist is listed: {n} videos listed so far, {total} in all
                    crate::i18n::t("Listing the videos: {n} of {total}"),
                    &[
                        ("n", &crate::i18n::integer(n as u64)),
                        ("total", &crate::i18n::integer(t)),
                    ],
                ),
                None => crate::i18n::fmt(
                    // i18n: in Download video, while a channel is listed: {n} videos listed so far
                    crate::i18n::t("Listing the videos: {n}"),
                    &[("n", &crate::i18n::integer(n as u64))],
                ),
            }
        } else if let Some(t) = self.total.filter(|_| self.short()) {
            // Stopped short (Stop listing, a failure, a channel's pages): never shown as whole.
            // The form follows {total}; the numbers are written as the interface writes them.
            // i18n: in Download video: a playlist title, then how many of its videos are listed so far ({listed}) of how many ({total})
            let count = crate::i18n::tn(
                t,
                "{title}: {listed} of {total} video listed",
                "{title}: {listed} of {total} videos listed",
            );
            crate::i18n::fmt(
                &count,
                &[
                    ("title", &crate::i18n::isolate(&self.title)),
                    ("listed", &crate::i18n::integer(n as u64)),
                    ("total", &crate::i18n::integer(t)),
                ],
            )
        } else {
            // i18n: in Download video: a playlist title then how many videos it has ({count})
            let count = crate::i18n::tn(
                n as u64,
                "{title}: {count} video",
                "{title}: {count} videos",
            );
            crate::i18n::fmt(
                &count,
                &[
                    ("title", &crate::i18n::isolate(&self.title)),
                    ("count", &crate::i18n::integer(n as u64)),
                ],
            )
        };
        let x = if self.listing.is_some() { 31.0 } else { 11.0 };
        let rc = winui::fit(
            ui,
            &p,
            o.x + x,
            line_y,
            &status,
            winui::TEXT(),
            o.x + right,
            "playlist-status",
        );
        // How far the listing is, or where it stopped, is read out too.
        winui::readable(ui, rc, "playlist-status", &status);
    }

    /// Why nothing is listed, or that the list was cut at the most MDM lists.
    fn notes(&self, ui: &mut Ui, o: Pos2) {
        let p = ui.painter().clone();
        let (text, color) = if let Some(e) = &self.error {
            (e.clone(), winui::ERROR())
        } else if self.capped {
            let text = crate::i18n::fmt(
                // i18n: in Download video, a list longer than MDM lists: only its first {n} videos are shown
                crate::i18n::t("Only the first {n} videos are listed."),
                &[(
                    "n",
                    &crate::i18n::integer(mdm_tools::playlist::MAX_ENTRIES as u64),
                )],
            );
            (text, winui::MUTED())
        } else {
            return;
        };
        let rc = winui::fit(
            ui,
            &p,
            o.x + 11.0,
            o.y + 86.0,
            &text,
            color,
            o.x + 489.0,
            "playlist-note",
        );
        winui::readable(ui, rc, "playlist-note", &text);
    }

    /// The checklist: only the rows in view are drawn, whatever the list's length.
    fn checklist(&mut self, ui: &mut Ui, o: Pos2) {
        let n = self.entries.len();
        // i18n: what a screen reader calls the list of a playlist's videos
        self.rows.name = crate::i18n::t("Videos");
        let (entries, checked) = (&self.entries, &self.checked);
        // Each video after the icon of the file it will give (the quality chosen's type).
        let file = format!("video.{}", self.ext);
        let mut toggle = None;
        let ev = winui::row_list(
            ui,
            "playlist-rows",
            r(o, 11.0, 94.0, 478.0, 120.0),
            n,
            17.0,
            &mut self.rows,
            |ui, clip, i, row, look| {
                let e = &entries[i];
                let glyph = pos2(row.left() + 5.0, row.top() + 2.0);
                let mut on = checked[i];
                let len = e
                    .duration
                    .map(mdm_tools::playlist::clock)
                    .unwrap_or_default();
                look.name = if len.is_empty() {
                    e.title.clone()
                } else {
                    format!("{}, {len}", e.title)
                };
                if !e.unavailable
                    && winui::row_check(ui, clip, glyph, &mut on, &e.title, ("playlist-row", i))
                {
                    toggle = Some(i);
                }
                let ink = if e.unavailable {
                    winui::TEXT_DISABLED()
                } else {
                    look.ink
                };
                let len_w = winui::text_width(clip, &len);
                let name = crate::i18n::isolate(&e.title);
                let kind = crate::fileicons::Kind::File(&file);
                crate::fileicons::paint_at(ui, clip, kind, pos2(glyph.x + 18.0, row.top()), 16.0);
                winui::fit(
                    ui,
                    clip,
                    glyph.x + 18.0 + crate::fileicons::INDENT,
                    row.top() + 12.0,
                    &name,
                    ink,
                    row.right() - 10.0 - len_w - 10.0,
                    ("playlist-title", i),
                );
                winui::text_right(clip, row.right() - 6.0, row.top() + 12.0, &len, ink);
            },
        );
        if let Some(i) = ev.space.or(ev.double_clicked) {
            toggle = Some(i);
        }
        if let Some(i) = toggle
            && !self.entries[i].unavailable
        {
            self.checked[i] = !self.checked[i];
            if self.checked[i] {
                self.chosen += 1;
            } else {
                self.chosen -= 1;
            }
        }
    }

    /// Select all, Select none; Stop listing or Load more; how many are chosen.
    fn buttons(&mut self, ui: &mut Ui, o: Pos2, n: usize) {
        let p = ui.painter().clone();
        // i18n: button in Download video for a channel: list its next videos
        let load_more = crate::i18n::mark("Load more");
        // i18n: button in Download video: go on listing a playlist from where it stopped
        let list_rest = crate::i18n::mark("List the rest");
        // i18n: button in Download video: stop listing a playlist's videos (those listed stay)
        let stop = crate::i18n::mark("Stop listing");
        let third = if self.listing.is_some() {
            stop
        } else if self.paged {
            load_more
        } else {
            list_rest
        };
        // Each as wide as its label in this language (#172), side by side from the left.
        let [all_at, none_at, third_at] = winui::buttons_from_left(
            &p,
            11.0,
            8.0,
            &[("Select all", 90.0), ("Select none", 90.0), (third, 100.0)],
            150.0,
        )[..] else {
            unreachable!("three buttons")
        };
        let all = winui::button(
            ui,
            r(o, all_at.0, 222.0, all_at.1, 21.0),
            // i18n: button in Download video for a playlist: tick every video
            "Select all",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        let none = winui::button(
            ui,
            r(o, none_at.0, 222.0, none_at.1, 21.0),
            // i18n: button in Download video for a playlist: untick every video
            "Select none",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        if all || none {
            self.chosen = 0;
            for (c, e) in self.checked.iter_mut().zip(&self.entries) {
                *c = all && !e.unavailable;
                self.chosen += usize::from(*c);
            }
        }
        if self.listing.is_some() {
            if winui::button(
                ui,
                r(o, third_at.0, 222.0, third_at.1, 21.0),
                stop,
                ButtonKind::Normal,
                false,
                None,
            )
            .clicked()
                && let Some(l) = self.listing.take()
            {
                l.cancel();
                // Those listed stay, and the listing can go on from there.
                self.more = self.entries.len() < mdm_tools::playlist::MAX_ENTRIES
                    && (self.paged || self.short());
            }
        } else if self.more
            && winui::button(
                ui,
                r(o, third_at.0, 222.0, third_at.1, 21.0),
                third,
                ButtonKind::Normal,
                false,
                None,
            )
            .clicked()
        {
            // On from the last one listed (a page stopped part-way goes on from there).
            let next = self.entries.last().map_or(1, |e| e.index + 1);
            self.list_from(next);
        }
        let chosen = crate::i18n::fmt(
            // i18n: in Download video for a playlist: how many videos are ticked, of how many
            crate::i18n::t("{n} of {total} chosen"),
            &[
                ("n", &crate::i18n::integer(self.chosen as u64)),
                ("total", &crate::i18n::integer(n as u64)),
            ],
        );
        winui::text_right(&p, o.x + 489.0, o.y + 236.0, &chosen, winui::MUTED());
    }

    /// The Quality drop-down (where a single video has its File name).
    pub fn quality_row(&mut self, ui: &mut Ui, rc: egui::Rect, merge: bool) {
        let picks = picks(merge);
        let labels: Vec<&str> = picks.iter().map(|p| p.label.as_str()).collect();
        let pick = self.pick.min(labels.len().saturating_sub(1));
        if let Some(i) = winui::combo_light(ui, rc, &labels, pick, true, "playlist-quality") {
            self.pick = i;
        }
        if let Some(p) = picks.get(self.pick.min(picks.len().saturating_sub(1))) {
            self.ext = p.ext;
            self.sound = p.sound_only();
        }
    }

    /// Download Now (`start_now`) or Later was clicked: true when that many are asked about
    /// first ([`Self::answer`] then says when it's Yes).
    pub fn ask(&mut self, start_now: bool) -> bool {
        if self.chosen <= CONFIRM_ABOVE {
            return false;
        }
        let text = crate::i18n::fmt(
            // i18n: asked before a playlist adds many downloads at once; {n} is how many
            crate::i18n::t("Add {n} downloads to the list? Each video gets its own row."),
            &[("n", &crate::i18n::integer(self.chosen as u64))],
        );
        let title = crate::i18n::mark(
            // i18n: title of the question asked before a playlist adds many downloads
            "Add many downloads",
        );
        self.confirm = Some((Confirm::new(title, text), start_now));
        true
    }

    /// The question, while it's asked: `Some(start_now)` once it's answered Yes (the video
    /// window then closes as Download does: by its own Done, so it opens afresh next time).
    pub fn answer(&mut self, ctx: &egui::Context) -> Option<bool> {
        let (c, start_now) = self.confirm.as_mut()?;
        let start_now = *start_now;
        match c.show(ctx) {
            Outcome::Open => None,
            Outcome::Closed => {
                self.confirm = None;
                None
            }
            Outcome::Done(()) => {
                self.confirm = None;
                Some(start_now)
            }
        }
    }

    /// What Download adds: the ticked videos, the quality chosen, into Save to/<list title>,
    /// listed under `category`.
    pub fn add(
        &self,
        folder: &Path,
        merge: bool,
        start_now: bool,
        context: &mdm_ipc::Command,
        category: crate::model::Category,
    ) -> PlaylistAdd {
        let picks = picks(merge);
        let pick = &picks[self.pick.min(picks.len() - 1)];
        let entries = self
            .entries
            .iter()
            .zip(&self.checked)
            .filter(|(_, on)| **on)
            .filter_map(|(e, _)| Some((url::Url::parse(&e.url).ok()?, e.title.clone())))
            .collect();
        PlaylistAdd {
            entries,
            selector: pick.selector.clone(),
            ext: pick.ext,
            folder: folder.join(mdm_engine::naming::sanitize(&self.title)),
            category,
            start_now,
            context: context.clone(),
        }
    }
}

impl Drop for PlaylistView {
    fn drop(&mut self) {
        if let Some(l) = self.listing.take() {
            l.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The qualities for a whole list: with ffmpeg the merged ones and MP3, without it only
    /// single files; heights as "at most".
    #[test]
    fn qualities_for_a_list() {
        let m = picks(true);
        assert_eq!(m[0].selector, "bv*+ba/b");
        assert_eq!(
            m[DEFAULT_PICK].selector,
            "bv*[height<=1080]+ba/b[height<=1080]"
        );
        assert!(m.iter().any(|p| p.selector == "mp3:320" && p.ext == "mp3"));
        assert!(m.iter().any(|p| p.selector == "ba"));
        assert!(m.iter().any(|p| p.selector == "opus:" && p.ext == "opus"));
        assert!(m.iter().any(|p| p.selector == "flac:" && p.ext == "flac"));
        let plain = picks(false);
        assert_eq!(plain[DEFAULT_PICK].selector, "b[height<=1080]");
        assert!(!plain.iter().any(|p| p.selector.starts_with("mp3:")));
        assert!(!plain.iter().any(|p| p.ext == "opus" || p.ext == "flac"));
        // #611: the "Audio only" ones save only the sound, the heights don't.
        for p in m.iter().chain(&plain) {
            let audio = p.selector == "ba" || p.ext != "mp4";
            assert_eq!(p.sound_only(), audio, "{}", p.selector);
        }
    }
}
