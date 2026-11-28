//! Tasks → Download video from a page… (M9, ours: IDM's video panel is fed by its browser
//! module, which comes with M8). Paste a page, look it up with yt-dlp, pick a quality, choose
//! where it goes. yt-dlp is installed (verified, D-011) from here on first use, and ffmpeg (the
//! LGPL build) when the best qualities need merging. It follows the app's mode, like Add torrent.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Pos2, Ui, pos2};
use mdm_tools::declared::Declared;
use mdm_tools::resolve::{Choice, Looked, SubChoice, Video};
use mdm_tools::video::{ALL_UPLOADERS, Subs};

use crate::dialogs::{Outcome, window};
use crate::model::Category;
use crate::winui::{self, ButtonKind, r, text};

type Slot<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// What Download Now or Later adds: a video, or a playlist's (#69). Both boxed: each is large,
/// and they differ in size.
pub enum VideoOut {
    One(Box<VideoAdd>),
    Many(Box<crate::playlistdlg::PlaylistAdd>),
}

/// What OK adds.
pub struct VideoAdd {
    pub page: url::Url,
    pub selector: String,
    pub dest: PathBuf,
    /// The category it is listed under: Music for a sound-only choice (#611), else Video.
    pub category: Category,
    pub size: Option<u64>,
    pub start_now: bool,
    /// What the browser's request for the page carried (its button), for yt-dlp to ask the
    /// same; none for an address typed or edited here.
    pub context: mdm_ipc::Command,
    /// The subtitles to save with it (#68; none for a sound-only choice).
    pub subs: Subs,
    /// The Subtitles choice to remember for the next video (settings.video_subs); none when
    /// the page had no subtitles to choose from (the last real choice stays).
    pub remember: Option<Subs>,
    /// The browser's quality menu's look-up the choice was made from, for the download's first
    /// run (videolookup.rs); none from this window, whose download looks the page up itself.
    pub preloaded: Option<mdm_tools::video::Preloaded>,
    /// The user typed the file name (#1102): kept as typed; else it is the page's title, which
    /// is shortened to fit the path limits.
    pub named: bool,
    /// The video's length (whole seconds) as looked up, for the duplicate check
    /// ([`whole_seconds`], `model::VideoSpec::duration`).
    pub duration: Option<u32>,
    /// Only this part of it (#1662); none for the whole video.
    pub part: Option<mdm_tools::part::Part>,
}

/// A looked-up length (`resolve::Video::duration`, seconds) as a video download keeps it: whole
/// seconds; none under a second, or not a number.
pub fn whole_seconds(length: Option<f64>) -> Option<u32> {
    length
        .filter(|s| s.is_finite() && *s >= 1.0 && *s < f64::from(u32::MAX))
        .map(|s| s.round() as u32)
}

/// Which helper is being installed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    YtDlp,
    Ffmpeg,
    /// The JavaScript runtime YouTube needs (#1448, #1470): MDM's own Node.js (#1570).
    JsRuntime,
}

impl Tool {
    /// The program's name, in messages.
    fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::JsRuntime => "Node.js",
        }
    }
}

pub struct VideoDialog {
    page: String,
    /// The browser's request context for `page` as it came (VideoAdd::context).
    context: mdm_ipc::Command,
    from_browser: String,
    /// Save to.
    folder: String,
    /// The Video and Music categories' folders MDM puts in Save to (#611): Music's while a
    /// sound-only choice is chosen, as long as Save to holds one of them (a folder typed or
    /// picked stays). No Music folder: the Music category was deleted, all goes as video.
    video_dir: String,
    music_dir: Option<String>,
    /// Save to was last set for a sound-only choice.
    sound: bool,
    name: String,
    lookup: Option<Slot<(Looked, Option<Declared>)>>,
    /// The video the page looked up declares its own (#1076): what the download is added with
    /// (its address, the page as its Referer), while the address is still the one looked up.
    declared: Option<Declared>,
    /// The yt-dlp and the browser's context the look-up ran with (a list is listed with them).
    looking: Option<(PathBuf, mdm_tools::proc::FromBrowser)>,
    /// The page is a playlist or a channel: its videos (#69).
    list: Option<crate::playlistdlg::PlaylistView>,
    /// Adding that many was answered Yes: Download Now (true) or Later goes ahead.
    confirmed: Option<bool>,
    install: Option<(Tool, Slot<mdm_tools::Installed>)>,
    /// The helpers found (read again when an install ends, not every frame: it reads files).
    tools: Option<Tools>,
    /// The address the choices were looked up for (an edited one needs Look up again).
    looked_up: String,
    video: Option<(Video, Vec<Choice>)>,
    chosen: usize,
    /// First choice shown (the list scrolls by whole rows).
    /// The choices list's scroll and selection.
    rows: winui::RowList,
    error: Option<String>,
    focused: bool,
    /// Download Now (true) or Later, once chosen.
    start_now: Option<bool>,
    /// Look the page up as soon as yt-dlp is there (opened from Add URL).
    auto_look: bool,
    /// Asking to create a missing folder.
    asking: crate::dialogs::FolderGate<()>,
    /// Helpers...: where yt-dlp and ffmpeg come from (#51).
    helpers: Option<crate::helpers::Programs>,
    /// The looked-up video's subtitle languages (#68) and the one chosen in the Subtitles list
    /// (0: none, then "All the uploader's" when offered, then `subs` in order).
    subs: Vec<SubChoice>,
    sub_pick: usize,
    /// Put the subtitles into the video rather than beside it.
    embed: bool,
    /// Embed chapters (#827; settings.video_chapters).
    chapters: bool,
    /// The last video's choice (settings.video_subs), applied to each video looked up.
    pref: Subs,
    /// The clipboard's page address, read off the window's thread (#661).
    clip: crate::clipboard::Prefill,
    /// Part (#1662): only from one time to another, as typed, cut quickly at the keyframes or
    /// exactly.
    part_on: bool,
    part_from: String,
    part_to: String,
    part_exact: bool,
    /// From the browser's quality menu: Part ticked, From where the player was (seconds), for
    /// the video looked up.
    part_at: Option<u32>,
    /// The file name MDM last gave (the choice's, with the part's words): File name follows the
    /// choice and the part while it still holds it.
    auto_name: String,
}

/// How much lower than before Part (#1662) the rows under Subtitles are: its two lines.
const PART_ROWS: f32 = 52.0;

/// The top of the window's bottom row of buttons (Helpers..., Download Now, Later, Cancel),
/// under Embed chapters.
const BUTTONS_Y: f32 = 389.0 + PART_ROWS;

/// What is installed in the tools folder: (folder, yt-dlp, ffmpeg).
type Tools = (
    Option<PathBuf>,
    Option<mdm_tools::Installed>,
    Option<mdm_tools::Installed>,
);

fn tools() -> Tools {
    let dir = mdm_tools::tools_dir();
    let ytdlp = dir.as_deref().and_then(mdm_tools::installed);
    let ffmpeg = dir.as_deref().and_then(mdm_tools::ffmpeg::installed);
    (dir, ytdlp, ffmpeg)
}

impl VideoDialog {
    /// Opened from the menu: a video page's address on the clipboard is taken (read off the
    /// window's thread, put in the box on the frame it comes).
    pub fn new(folder: PathBuf) -> Self {
        VideoDialog {
            clip: crate::clipboard::Prefill::ask(),
            ..Self::blank(folder)
        }
    }

    /// The clipboard's text as the page, when it is a web address and the box is still empty
    /// (nothing typed meanwhile).
    fn fill_from_clipboard(&mut self, clip: &str) {
        let clip = clip.trim();
        if self.page.is_empty()
            && url::Url::parse(clip).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
        {
            self.page = clip.to_owned();
        }
    }

    /// No address yet, saving into `folder` (the Video category's).
    fn blank(folder: PathBuf) -> Self {
        let folder = folder.display().to_string();
        VideoDialog {
            page: String::new(),
            context: Default::default(),
            from_browser: String::new(),
            video_dir: folder.clone(),
            music_dir: None,
            sound: false,
            folder,
            name: String::new(),
            lookup: None,
            declared: None,
            looking: None,
            list: None,
            confirmed: None,
            install: None,
            tools: None,
            looked_up: String::new(),
            video: None,
            chosen: 0,
            rows: Default::default(),
            error: None,
            focused: false,
            start_now: None,
            auto_look: false,
            asking: Default::default(),
            helpers: None,
            subs: Vec::new(),
            sub_pick: 0,
            embed: false,
            chapters: true,
            pref: Subs::default(),
            clip: Default::default(),
            part_on: false,
            part_from: String::new(),
            part_to: String::new(),
            part_exact: false,
            part_at: None,
            auto_name: String::new(),
        }
    }

    /// Embed chapters ticked or not (settings.video_chapters, #827).
    pub fn with_chapters(mut self, on: bool) -> Self {
        self.chapters = on;
        self
    }

    /// Offer `pref` (the last video's subtitles, settings.video_subs) for the videos looked up.
    pub fn with_subs(mut self, pref: Subs) -> Self {
        self.embed = pref.embed;
        self.pref = pref;
        self
    }

    /// Save to switches to `dir`, the Music category's folder, while a sound-only choice is
    /// chosen (#611); None (the Music category was deleted): sound goes as video does.
    pub fn with_music_folder(mut self, dir: Option<PathBuf>) -> Self {
        self.music_dir = dir.map(|d| d.display().to_string());
        self
    }

    /// Opened for `page` (Add URL was given a video's page, or the browser's button was
    /// clicked with `context`): looks it up at once.
    pub fn for_page(folder: PathBuf, page: &str, mut context: mdm_ipc::Command) -> Self {
        let own = mdm_tools::pages::video_address(page);
        // An embedded player taken to the video's own page (#359): the site that embedded it
        // isn't that page's referer, and Dailymotion refuses its own page asked with one (403).
        if own != page {
            context.referer = None;
        }
        let page = own;
        // The browser's "Download a part from here..." (#1662): Part ticked, From there.
        let part_at = context.video_part_from;
        VideoDialog {
            page: page.clone(),
            from_browser: page,
            context,
            auto_look: true,
            part_at,
            ..Self::blank(folder)
        }
    }

    /// The browser's context, while the address is still the one it came with.
    fn context(&self) -> mdm_ipc::Command {
        if self.page.trim() == self.from_browser {
            self.context.clone()
        } else {
            mdm_ipc::Command::default()
        }
    }

    /// The choices for a looked-up video: merged qualities when ffmpeg is there.
    fn set_video(&mut self, mut v: Video) {
        // No page reader for this site: the page's own title rather than a hash (#345).
        if self.page.trim() == self.from_browser
            && let Some(title) = page_title(&self.context, &v, &self.page)
        {
            v.title = title.to_owned();
        }
        let merge = self.tools.get_or_insert_with(tools).2.is_some();
        let choices = mdm_tools::resolve::choices(&v, merge);
        // The best, as `video-get` takes it by default (#1240).
        self.chosen = mdm_tools::resolve::pick(&choices, None).map_or(0, |p| p.index);
        self.rows = Default::default();
        self.error = choices
            .is_empty()
            .then(|| crate::i18n::t("No video this can download on that page").into());
        self.subs = mdm_tools::resolve::sub_choices(&v, crate::i18n::code());
        self.sub_pick = pick_for(&self.subs, &self.pref);
        // Part's times (#1662): the whole video, From where the browser's player was when asked.
        let length = mdm_tools::part::length(v.duration);
        let hours = length.is_some_and(|l| l >= 3600);
        let at = self.part_at.take();
        self.part_from =
            mdm_tools::part::clock(at.unwrap_or(0).min(length.unwrap_or(u32::MAX)), hours);
        self.part_to = length.map_or_else(String::new, |l| mdm_tools::part::clock(l, hours));
        if at.is_some() && length.is_some() {
            self.part_on = true;
        }
        self.video = Some((v, choices));
        self.name_from_choice();
    }

    /// The part as typed (#1662): none while Part isn't ticked; why it can't be, else.
    pub fn part(&self) -> Result<Option<mdm_tools::part::Part>, mdm_tools::part::Problem> {
        if !self.part_on || self.list.is_some() {
            return Ok(None);
        }
        let duration = self.video.as_ref().and_then(|(v, _)| v.duration);
        mdm_tools::part::Part::checked(&self.part_from, &self.part_to, self.part_exact, duration)
            .map(Some)
    }

    /// Part (#1662) ticked with these times and cut (`/show video-part`: its screenshots).
    pub fn show_part(&mut self, from: &str, to: &str, exact: bool) {
        self.part_on = true;
        self.part_from = from.into();
        self.part_to = to.into();
        self.part_exact = exact;
        self.name_from_choice();
    }

    /// The Subtitles list's choice as yt-dlp is asked for it.
    fn chosen_subs(&self) -> Subs {
        subs_at(&self.subs, self.sub_pick, self.embed)
    }

    /// A looked-up video with subtitles in four languages, without yt-dlp or the network
    /// (`/show video-subs`: the window's check in every language, its screenshots).
    pub fn show_sample(&mut self) {
        // Its 4K in AV1 and VP9, HDR and SDR, each its own line (#755); 720p sized only
        // about (#801).
        const SAMPLE: &str = r#"{"title": "A conference talk about download managers, recorded in Lisbon",
            "duration": 1830, "language": "en", "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "quality": 3, "tbr": 129.5, "filesize": 29600000},
            {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "quality": 3, "tbr": 121.8, "filesize": 27900000},
            {"format_id": "18", "ext": "mp4", "height": 360, "fps": 30, "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "filesize": 61000000},
            {"format_id": "701", "ext": "mp4", "height": 2160, "fps": 60, "vcodec": "av01.0.13M.10", "acodec": "none", "dynamic_range": "HDR10", "filesize": 1951000000},
            {"format_id": "337", "ext": "webm", "height": 2160, "fps": 60, "vcodec": "vp09.02.51.10", "acodec": "none", "dynamic_range": "HDR10", "filesize": 2248000000},
            {"format_id": "315", "ext": "webm", "height": 2160, "fps": 60, "vcodec": "vp9", "acodec": "none", "dynamic_range": "SDR", "filesize": 1870000000},
            {"format_id": "299", "ext": "mp4", "height": 1080, "fps": 60, "vcodec": "avc1.64002A", "acodec": "none", "dynamic_range": "SDR", "filesize": 412000000},
            {"format_id": "136", "ext": "mp4", "height": 720, "fps": 30, "vcodec": "avc1.4D401F", "acodec": "none", "dynamic_range": "SDR", "filesize_approx": 201000000}],
            "subtitles": {"de": [{"name": "German"}], "pt-PT": [{"name": "Portuguese (Portugal)"}],
                "ar": [{"name": "Arabic"}]},
            "automatic_captions": {"en-orig": [{"name": "English (Original)"}], "en": [{"name": "English"}]}}"#;
        if let Some(v) = mdm_tools::resolve::parse(SAMPLE.as_bytes()) {
            self.page = "https://video.example/talk".into();
            self.looked_up = self.page.clone();
            self.auto_look = false;
            self.set_video(v);
        }
    }

    /// A page like YouTube's when yt-dlp has no JavaScript runtime (#351): videos and sound
    /// apart, nothing carrying both (`/show video-adaptive`); `runtime_said`: yt-dlp said why
    /// (#852, `/show video-adaptive:runtime`).
    pub fn show_sample_adaptive(&mut self, runtime_said: bool) {
        const SAMPLE: &str = r#"{"title": "A conference talk about download managers, recorded in Lisbon",
            "duration": 1830, "language": "en", "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 29000000},
            {"format_id": "137", "ext": "mp4", "height": 1080, "vcodec": "avc1", "acodec": "none", "filesize": 412000000},
            {"format_id": "136", "ext": "mp4", "height": 720, "vcodec": "avc1", "acodec": "none", "filesize": 201000000}]}"#;
        if let Some(mut v) = mdm_tools::resolve::parse(SAMPLE.as_bytes()) {
            v.needs_js_runtime = runtime_said;
            self.page = "https://video.example/talk".into();
            self.looked_up = self.page.clone();
            self.auto_look = false;
            self.set_video(v);
        }
    }

    /// A listed playlist, as [`crate::playlistdlg::PlaylistView::sample`] makes it, its quality
    /// the one at position `pick` (`/show video-playlist:7`).
    pub fn show_sample_list(&mut self, pick: &str) {
        self.page = "https://video.example/playlist".into();
        self.looked_up = self.page.clone();
        self.auto_look = false;
        self.list = Some(crate::playlistdlg::PlaylistView::sample(pick));
    }

    /// The sample's choice whose label starts with `prefix` (any case), chosen and scrolled into
    /// view (`/show video-subs:flac`: a screenshot of a choice below the list's first rows;
    /// `video-subs:most`, the most compatible one, #1439).
    pub fn pick(&mut self, prefix: &str) {
        let prefix = prefix.to_lowercase();
        let found = self.video.as_ref().and_then(|(_, choices)| {
            choices.iter().position(|c| {
                let label = c.label.to_lowercase();
                label.starts_with(&prefix)
                    || label
                        .strip_prefix("audio only ")
                        .is_some_and(|l| l.starts_with(&prefix))
            })
        });
        if let Some(i) = found {
            self.chosen = i;
            // The list shows 8 rows.
            self.rows.show_row(i, 8);
            self.name_from_choice();
        }
    }

    /// Helpers..., as its button does (`/show video-helpers`).
    pub fn open_helpers(&mut self) {
        if self.helpers.is_none() {
            self.helpers = Some(crate::helpers::Programs::new());
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Handle) -> Outcome<VideoOut> {
        if let Some(clip) = self.clip.take() {
            self.fill_from_clipboard(&clip);
        }
        // Results from the background work.
        let looked = self.lookup.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(res) = looked {
            self.lookup = None;
            self.declared = None;
            match res {
                Ok((Looked::Video(v), declared)) => {
                    self.declared = declared;
                    self.set_video(v);
                }
                Ok((Looked::List(l), _)) => {
                    if let Some((exe, browser)) = self.looking.take() {
                        self.list = Some(crate::playlistdlg::PlaylistView::start(
                            &exe,
                            &self.looked_up,
                            browser,
                            l,
                        ));
                    }
                }
                Err(e) => {
                    let browser = self
                        .looking
                        .as_ref()
                        .map(|(_, b)| b)
                        .cloned()
                        .unwrap_or_default();
                    self.error = Some(request_error(&e, &browser));
                }
            }
        }
        let installed = self
            .install
            .as_ref()
            .and_then(|(t, s)| s.lock().unwrap().take().map(|r| (*t, r)));
        if let Some((tool, res)) = installed {
            self.install = None;
            self.tools = None;
            match res {
                Err(e) => {
                    self.error = Some(crate::i18n::fmt(
                        // i18n: an error: {what} = a helper program (yt-dlp, ffmpeg), {error} = why
                        crate::i18n::t("{what} could not be installed: {error}"),
                        &[("what", tool.name()), ("error", &e)],
                    ));
                }
                // With a JavaScript runtime YouTube lists its qualities: the page is looked up
                // again (#1448).
                Ok(_) if tool == Tool::JsRuntime => self.auto_look = true,
                // With ffmpeg the qualities that need merging appear.
                Ok(_) if tool == Tool::Ffmpeg => {
                    if let Some((v, _)) = self.video.take() {
                        self.set_video(v);
                    }
                }
                Ok(_) => {}
            }
        }
        // A playlist's question before adding many (#69): Yes goes ahead in the window's frame.
        if let Some(list) = &mut self.list
            && let Some(now) = list.answer(ctx)
        {
            self.confirmed = Some(now);
            ctx.request_repaint();
        }
        let out = window(
            ctx,
            "video-page",
            "Download video from a page",
            [500.0, 422.0 + PART_ROWS],
            false,
            winui::BG(),
            |ui, o| self.body(ui, o, rt),
        );
        // A playlist's quality chosen, a list or a video looked up: Save to follows (#611).
        self.follow_sound();
        // Helpers... open: on Close, the helpers are read again (another program, or MDM's own).
        if let Some(h) = &mut self.helpers
            && !matches!(h.show(ctx, rt), Outcome::Open)
        {
            self.helpers = None;
            self.tools = None;
            if let Some((v, _)) = self.video.take() {
                self.set_video(v);
            }
        }
        let folder = crate::dialogs::resolve_path(&self.folder);
        let out = crate::dialogs::ensure_folder(ctx, &mut self.asking, out, &folder);
        match out {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(()) => self.added().map_or(Outcome::Closed, Outcome::Done),
        }
    }

    /// What Download Now or Later adds: a playlist's videos, each its own download, or the
    /// video looked up (none when there is nothing to add).
    fn added(&self) -> Option<VideoOut> {
        let folder = self.save_folder();
        let start_now = self.start_now.unwrap_or(true);
        if let Some(list) = &self.list {
            let merge = self.tools.as_ref().is_some_and(|t| t.2.is_some());
            return Some(VideoOut::Many(Box::new(list.add(
                &folder,
                merge,
                start_now,
                &self.context(),
                self.category(),
            ))));
        }
        let (Some((_, choices)), Ok(page)) = (&self.video, url::Url::parse(self.page.trim()))
        else {
            return None;
        };
        let c = choices.get(self.chosen.min(choices.len().checked_sub(1)?))?;
        let name = mdm_engine::naming::sanitize(self.name.trim());
        let part = self.part().ok()?;
        // Not the name the choice gave it (the page's title, with the part's words): the user's
        // (#1102).
        let given = file_name(&self.video.as_ref()?.0.title, c);
        let given = match &part {
            Some(p) => mdm_tools::part::named(&given, p),
            None => given,
        };
        let named = name != mdm_engine::naming::sanitize(given.trim());
        let duration = self.video.as_ref().and_then(|v| v.0.duration);
        let chosen = self.chosen_subs();
        // The video the page declares (#1076): its address, the page as its Referer.
        let declared = self
            .declared
            .as_ref()
            .filter(|_| self.looked_up == self.page.trim())
            .and_then(|d| Some((url::Url::parse(&d.address).ok()?, d)));
        let (page, context) = match declared {
            Some((address, d)) => (address, declared_context(&self.context(), d)),
            None => (page, self.context()),
        };
        Some(VideoOut::One(Box::new(VideoAdd {
            page,
            selector: c.selector.clone(),
            dest: folder.join(name),
            category: self.category(),
            // A part's: its share of the whole (#1662).
            size: match &part {
                Some(p) => p.estimate(c.size, duration),
                None => c.size,
            },
            start_now,
            context,
            subs: if part.is_some() {
                Subs::default()
            } else {
                subs_for_choice(c, &chosen)
            },
            remember: (!self.subs.is_empty()).then_some(chosen),
            preloaded: None,
            named,
            duration: whole_seconds(duration),
            part,
        })))
    }

    /// The choice (or the playlist's quality) saves only the sound; None while there is none.
    fn sound_only(&self) -> Option<bool> {
        if let Some(list) = &self.list {
            return Some(list.sound_only());
        }
        let (_, choices) = self.video.as_ref()?;
        choices.get(self.chosen).map(mdm_tools::resolve::is_audio)
    }

    /// The category the download is listed under (#611), and whose folder "Change folder for
    /// this category on last selected" keeps: Music for a sound-only choice, else Video.
    pub fn category(&self) -> Category {
        if self.music_dir.is_some() && self.sound_only() == Some(true) {
            Category::Music
        } else {
            Category::Video
        }
    }

    /// Save to goes to the Music category's folder when the choice turns sound-only, back to
    /// Video's when it turns video again, but only while it holds the folder MDM put there: a
    /// folder the user typed or picked is theirs (#611).
    fn follow_sound(&mut self) {
        let Some(sound) = self.sound_only() else {
            return;
        };
        if sound == self.sound {
            return;
        }
        self.sound = sound;
        let Some(music) = &self.music_dir else {
            return;
        };
        let (from, to) = if sound {
            (&self.video_dir, music)
        } else {
            (music, &self.video_dir)
        };
        if crate::dialogs::resolve_path(&self.folder) == crate::dialogs::resolve_path(from) {
            self.folder = to.clone();
        }
    }

    /// The Save to folder: the one a playlist's own folder goes under, too.
    pub fn save_folder(&self) -> PathBuf {
        crate::dialogs::resolve_path(&self.folder)
    }

    /// The file name, and Save to (#611), follow the choice, and the part's words its times
    /// (#1662).
    fn name_from_choice(&mut self) {
        if let Some((v, choices)) = &self.video
            && let Some(c) = choices.get(self.chosen)
        {
            self.name = file_name(&v.title, c);
            self.auto_name.clear();
            self.name_for_part();
        }
        self.follow_sound();
    }

    /// File name says the part (`Talk (1m30s-4m10s).mp4`, #1662) while it holds the name MDM
    /// gave (a name typed stays as typed): its words follow the times, and go with Part.
    fn name_for_part(&mut self) {
        if !self.auto_name.is_empty() && self.name != self.auto_name {
            return;
        }
        let bare = mdm_tools::part::unnamed(&self.name);
        let want = match self.part() {
            Ok(Some(p)) => mdm_tools::part::named(&bare, &p),
            // Times being typed: the name waits for them.
            Err(_) => return,
            Ok(None) => bare,
        };
        self.name = want.clone();
        self.auto_name = want;
    }

    /// Install a helper in the background.
    fn install(
        &mut self,
        tool: Tool,
        dir: PathBuf,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) {
        let slot: Slot<mdm_tools::Installed> = Arc::new(Mutex::new(None));
        let (out, ctx) = (slot.clone(), ctx.clone());
        let net = crate::settings::own_net();
        rt.spawn(async move {
            let res = match tool {
                Tool::YtDlp => mdm_tools::install_latest(&dir, &net, None).await,
                Tool::Ffmpeg => mdm_tools::ffmpeg::install_latest(&dir, &net, None).await,
                Tool::JsRuntime => mdm_tools::jsruntime::install_latest(&dir, &net, None).await,
            };
            let name = tool.name();
            // What failed and what to do (#816): wrapped in the empty list below.
            *out.lock().unwrap() = Some(res.map_err(|e| crate::errors::tools_and_advice(&e, name)));
            ctx.request_repaint();
        });
        self.install = Some((tool, slot));
        self.error = None;
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2, rt: &tokio::runtime::Handle) -> Outcome<()> {
        let p = ui.painter().clone();
        let (dir, ytdlp, ffmpeg) = self.tools.get_or_insert_with(tools).clone();
        text(
            &p,
            o.x + 11.0,
            o.y + 21.0,
            crate::i18n::t("Page address"),
            winui::TEXT(),
        );
        // Look up is as wide as its label in this language (#172); the address ends 8 px before.
        let (look_x, look_w) =
            winui::buttons_to_right(&p, 489.0, 8.0, &[("Look up", 90.0)], 160.0)[0];
        let edit = winui::edit(
            ui,
            r(o, 11.0, 28.0, look_x - 8.0 - 11.0, 21.0),
            &mut self.page,
            true,
            false,
        );
        if !self.focused {
            edit.request_focus();
            self.focused = true;
        }
        let busy = self.lookup.is_some() || self.install.is_some();
        let page_ok =
            url::Url::parse(self.page.trim()).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        let can_look = page_ok && !busy && ytdlp.is_some();
        let look = winui::button(
            ui,
            r(o, look_x, 28.0, look_w, 21.0),
            "Look up",
            if can_look {
                ButtonKind::Default
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked()
            || (edit.has_focus()
                && can_look
                && winui::enter_pressed(ui)
                && winui::take_key(ui, egui::Key::Enter));
        // Opened for a page: look it up as soon as yt-dlp is there.
        let look = look || (can_look && std::mem::take(&mut self.auto_look));
        if look
            && can_look
            && let Some(tool) = &ytdlp
        {
            // A typed or pasted address too (#359); the box shows what is looked up.
            self.page = mdm_tools::pages::video_address(self.page.trim());
            let slot: Slot<(Looked, Option<Declared>)> = Arc::new(Mutex::new(None));
            let (out, exe, page, ctx, rt) = (
                slot.clone(),
                tool.exe.clone(),
                self.page.trim().to_owned(),
                ui.ctx().clone(),
                rt.clone(),
            );
            let browser = from_browser(&self.context());
            self.looking = Some((exe.clone(), browser.clone()));
            std::thread::spawn(move || {
                let res = look_up(&exe, &page, &browser, &rt);
                *out.lock().unwrap() = Some(res);
                ctx.request_repaint();
            });
            self.lookup = Some(slot);
            self.looked_up = self.page.trim().to_owned();
            self.video = None;
            // A list being listed stops (dropped).
            self.list = None;
            self.error = None;
        }

        // The status line: a helper missing or installing / looking up / the title.
        let line_y = o.y + 70.0;
        let installing = self.install.as_ref().map(|(t, _)| *t);
        // #852: YouTube looked up without a JavaScript runtime lists only some of its qualities
        // (on some days only the sound): said on the title's line and the one under it, with
        // the help page's section on installing one.
        let runtime_note =
            self.error.is_none() && self.video.as_ref().is_some_and(|(v, _)| v.needs_js_runtime);
        let offer = match (&ytdlp, &ffmpeg) {
            (None, _) => Some((
                Tool::YtDlp,
                // i18n: button: download and install the yt-dlp program
                crate::i18n::mark("Install yt-dlp"),
                crate::i18n::t(
                    "Video lookups use yt-dlp (about 23 MB, checked against its signature).",
                ),
            )),
            // #1448, #1470, #1570: YouTube said yt-dlp had no JavaScript runtime, and MDM has a
            // Node.js of its own to offer for this system (looked up again once it is installed).
            // Before ffmpeg: the line says what's missing is the runtime, and without one YouTube
            // may list nothing ffmpeg could join.
            (Some(_), _)
                if runtime_note
                    && mdm_tools::jsruntime::available()
                    && dir
                        .as_deref()
                        .and_then(mdm_tools::node::installed)
                        .is_none() =>
            {
                Some((
                    Tool::JsRuntime,
                    // i18n: button in Download video: download and install Node.js, the JavaScript runtime YouTube needs (Node.js is a program's name)
                    crate::i18n::mark("Install Node.js"),
                    js_runtime_note(),
                ))
            }
            // MDM installs ffmpeg on 64-bit Windows only (its builds are Windows ones, x64 and
            // ARM64); elsewhere the system's package manager has it, and the note says so instead
            // of a button.
            (Some(_), None) if mdm_tools::ffmpeg::available() => Some((
                Tool::Ffmpeg,
                // i18n: button: download and install the ffmpeg program
                crate::i18n::mark("Install ffmpeg"),
                crate::i18n::t("The best qualities, MP3, Opus and FLAC need ffmpeg (about 80 MB)."),
            )),
            (Some(_), None) if cfg!(windows) => Some((
                Tool::Ffmpeg,
                "",
                // i18n: in Download video, on 32-bit Windows (no button: no ffmpeg build MDM can check exists for it); Helpers... is the button below
                crate::i18n::t(
                    "The best qualities, MP3, Opus and FLAC need ffmpeg, which MDM has no copy of for 32-bit Windows: choose your own with Helpers...",
                ),
            )),
            (Some(_), None) => Some((
                Tool::Ffmpeg,
                "",
                // i18n: in Download video, on macOS and Linux (no button: the system installs it)
                crate::i18n::t(
                    "The best qualities, MP3, Opus and FLAC need ffmpeg: install it with your system's package manager (or Homebrew).",
                ),
            )),
            _ => None,
        };
        // The Install button, as wide as its label in this language, and the status line up to
        // 9 px before it (to the window's edge without one).
        let install_at = offer
            .as_ref()
            .filter(|(_, label, _)| !label.is_empty() && installing.is_none())
            .map(|(_, label, _)| {
                winui::buttons_to_right(&p, 489.0, 8.0, &[(label, 90.0)], 200.0)[0]
            });
        let status_right = install_at.map_or(489.0, |(x, _)| x - 9.0);
        // #351: a page whose better videos need ffmpeg says so in place of its title (still in
        // File name): YouTube without a JavaScript runtime leaves nothing but the sound to offer
        // here, and the list alone gives no reason.
        let merge_note = self
            .video
            .as_ref()
            .filter(|(v, _)| ffmpeg.is_none() && mdm_tools::resolve::needs_merging(v))
            .and(offer.as_ref())
            .map(|(_, _, msg)| *msg);
        if let Some(t) = installing {
            crate::torrentdlg::spinner(ui, pos2(o.x + 18.0, line_y - 4.0), 6.0);
            let msg = match t {
                Tool::YtDlp => crate::i18n::t("Downloading and verifying yt-dlp..."),
                Tool::Ffmpeg => crate::i18n::t("Downloading and verifying ffmpeg (about 80 MB)..."),
                // i18n: in Download video: MDM downloads Node.js (a program's name) and checks it against its signed checksums
                Tool::JsRuntime => {
                    crate::i18n::t("Downloading and verifying Node.js (40 to 60 MB)...")
                }
            };
            text(&p, o.x + 31.0, line_y, msg, winui::TEXT());
        } else if self.lookup.is_some() {
            crate::torrentdlg::spinner(ui, pos2(o.x + 18.0, line_y - 4.0), 6.0);
            text(
                &p,
                o.x + 31.0,
                line_y,
                crate::i18n::t("Looking the page up..."),
                winui::TEXT(),
            );
        } else if self.list.is_some() {
            // The list draws its own (PlaylistView::middle).
        } else if runtime_note {
            if winui::note_with_link(
                ui,
                &p,
                o.x + 11.0,
                line_y,
                16.0,
                js_runtime_note(),
                winui::ERROR(),
                o.x + status_right,
                // i18n: link after the note that YouTube needs a JavaScript runtime: opens the help page that says how to install Node.js or Deno
                "How to install one",
                "video-runtime",
            ) {
                crate::help::open_page(JS_RUNTIME_HELP);
            }
        } else if let Some(msg) = merge_note {
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                line_y,
                msg,
                winui::MUTED(),
                o.x + status_right,
                "video-offer",
            );
        } else if let Some((v, _)) = &self.video {
            let (rect, cut) = winui::text_fit(
                &p,
                o.x + 11.0,
                line_y,
                &v.title,
                winui::TEXT(),
                o.x + status_right,
            );
            if cut {
                winui::full_on_hover(ui, rect, "video-title", &v.title);
            }
        } else if let Some((_, _, msg)) = offer {
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                line_y,
                msg,
                winui::MUTED(),
                o.x + status_right,
                "video-offer",
            );
        }
        if let Some((tool, label, _)) = offer
            && let Some((x, w)) = install_at
            && winui::button(
                ui,
                r(o, x, 57.0, w, 21.0),
                label,
                ButtonKind::Normal,
                false,
                None,
            )
            .clicked()
            && let Some(dir) = dir.clone()
        {
            self.install(tool, dir, rt, ui.ctx());
        }
        // An error too long for its line, with no choices below it: read whole in their place.
        let error_in_list = self.video.is_none()
            && self.list.is_none()
            && self
                .error
                .as_ref()
                .is_some_and(|e| winui::text_width(&p, e) > 478.0);
        if let Some(e) = self.error.as_ref().filter(|_| !error_in_list) {
            let (rect, cut) =
                winui::text_fit(&p, o.x + 11.0, o.y + 86.0, e, winui::ERROR(), o.x + 489.0);
            if cut {
                winui::full_on_hover(ui, rect, "video-error", e);
            }
        } else if !runtime_note
            && self
                .video
                .as_ref()
                .and_then(|(_, c)| c.get(self.chosen))
                .is_some_and(|c| c.selector == mdm_tools::resolve::FLAC)
        {
            // The honest word on FLAC (#70): the site's sound is compressed with losses already.
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                o.y + 86.0,
                // i18n: a note in Download video while FLAC is chosen: FLAC keeps the site's (already lossy) sound exactly, it can't make it better, the file is only bigger
                crate::i18n::t(
                    "FLAC keeps the sound exactly, but it can't add quality the site's sound doesn't have: the file is only bigger.",
                ),
                winui::MUTED(),
                o.x + 489.0,
                "video-flac",
            );
        }

        // A playlist's or a channel's videos (#69), or the video's choices.
        if let Some(list) = &mut self.list {
            // The status line stops short of an Install button beside it.
            list.middle(ui, o, installing.is_none().then_some(status_right));
        } else {
            // The choices: the shared row list.
            let list = r(o, 11.0, 94.0, 478.0, 150.0);
            let mut picked = None;
            let choices = self.video.as_ref().map_or(&[][..], |(_, c)| c.as_slice());
            self.rows.selected = Some(self.chosen).filter(|c| *c < choices.len());
            // i18n: what a screen reader calls the list of a video's formats (1080p, 720p, audio only…)
            self.rows.name = crate::i18n::t("Quality");
            let ev = winui::row_list(
                ui,
                "video-choices",
                list,
                choices.len(),
                17.0,
                &mut self.rows,
                |ui, clip, i, row, look| {
                    let c = &choices[i];
                    // The icon of the file this choice gives, then the format ("2160p MP4 ·
                    // VP9 · 60 fps · HDR", left to right); its size at the trailing edge: exactly
                    // that of the streams it downloads, or "~" before an estimate (#801).
                    let file = format!("video.{}", c.ext);
                    let kind = crate::fileicons::Kind::File(&file);
                    crate::fileicons::paint_at(
                        ui,
                        clip,
                        kind,
                        pos2(row.left() + 4.0, row.top()),
                        16.0,
                    );
                    let label = choice_label(&c.label);
                    let x = row.left() + 4.0 + crate::fileicons::INDENT;
                    text(clip, x, row.top() + 12.0, &label, look.ink);
                    let (size, said) = choice_size(c);
                    // Read as the format and its size (#95: the choices were blank to a screen reader
                    // and alike to UI Automation), in the user's language, as drawn.
                    look.name = format!("{label}, {said}");
                    winui::text_right(clip, row.right() - 6.0, row.top() + 12.0, &size, look.ink);
                },
            );
            if ev.focused
                && self.rows.type_to(ui, "video-choices", choices.len(), |i| {
                    choices[i].label.clone()
                })
            {
                picked = self.rows.selected;
            }
            if let Some(i) = ev.picked {
                picked = Some(i);
            }
            if let Some(i) = picked {
                self.chosen = i;
                self.name_from_choice();
            }
            // Wrapped in the empty list (#317).
            if let Some(e) = self.error.as_ref().filter(|_| error_in_list) {
                winui::paragraph(
                    &p.with_clip_rect(list),
                    list.left() + 8.0,
                    list.top() + 18.0,
                    e,
                    winui::ERROR(),
                    list.right() - 8.0,
                    16.0,
                );
            }
        }

        // The labels before the boxes: a column as wide as the longest of them in this
        // language (German's "Speichern unter" is past the English 59 px), up to 150 px, the
        // boxes after it; longer still, a label is cut and shown whole on hover.
        let labels = [
            crate::i18n::t("Save to"),
            if self.list.is_some() {
                // i18n: label before the quality chosen for all of a playlist's videos
                crate::i18n::t("Quality")
            } else {
                crate::i18n::t("File name")
            },
            // i18n: label before the list of a video's subtitle languages
            crate::i18n::t("Subtitles"),
            // i18n: label in Download video before the choice to download only a part of the video or sound (from one time to another)
            crate::i18n::t("Part"),
        ];
        let widest = labels
            .iter()
            .map(|l| winui::text_width(&p, l))
            .fold(0.0, f32::max);
        let bx = (11.0 + widest + 6.0).clamp(70.0, 150.0);
        let label = |ui: &Ui, y: f32, s: &str, color| {
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                o.y + y,
                s,
                color,
                o.x + bx - 5.0,
                ("video-label", y as i32),
            );
        };
        label(ui, 266.0, labels[0], winui::TEXT());
        winui::edit(
            ui,
            r(o, bx, 253.0, 459.0 - bx, 21.0),
            &mut self.folder,
            true,
            false,
        );
        if winui::button(
            ui,
            r(o, 467.0, 253.0, 22.0, 21.0),
            "...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            let from = self.folder.clone();
            winui::start_pick(ui.ctx(), "video-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        if let Some(d) = winui::picked(ui.ctx(), "video-folder") {
            self.folder = d.display().to_string();
        }
        label(ui, 297.0, labels[1], winui::TEXT());
        if let Some(list) = &mut self.list {
            // One quality for every video; their names are their titles (no subtitles).
            list.quality_row(ui, r(o, bx, 284.0, 489.0 - bx, 21.0), ffmpeg.is_some());
        } else {
            winui::edit(
                ui,
                r(o, bx, 284.0, 489.0 - bx, 21.0),
                &mut self.name,
                true,
                false,
            );
            let on = self.subtitles_row(ui, o, bx, ffmpeg.is_some());
            let shade = if on {
                winui::TEXT()
            } else {
                winui::TEXT_DISABLED()
            };
            label(ui, 328.0, labels[2], shade);
        }
        let part_shade = if self.part_row(ui, o, bx, ffmpeg.is_some()) {
            winui::TEXT()
        } else {
            winui::TEXT_DISABLED()
        };
        label(ui, 359.0, labels[3], part_shade);
        self.chapters_row(ui, o, bx, ffmpeg.is_some());

        // The choices belong to the address looked up: an edited address is looked up first
        // (it downloaded the new page with the old one's format and name).
        let stale =
            (self.video.is_some() || self.list.is_some()) && self.page.trim() != self.looked_up;
        let listing = self.list.as_ref().is_some_and(|l| l.listing());
        let chosen = match &self.list {
            Some(l) => l.chosen() > 0 && !listing,
            None => {
                self.video.as_ref().is_some_and(|(_, c)| !c.is_empty())
                    && !self.name.trim().is_empty()
            }
        };
        // A part whose times can't be (#1662): said on Download Now.
        let part_problem = self.part().err();
        let ready = chosen && !busy && !stale && part_problem.is_none();
        let kind = |default: bool| match (ready, default) {
            (false, _) => ButtonKind::Disabled,
            (true, true) => ButtonKind::Default,
            (true, false) => ButtonKind::Normal,
        };
        // The bottom row, each button as wide as its label in this language (#172): Helpers at the
        // left, as wide as the room before Download Now allows.
        let [now_at, later_at, cancel_at] = winui::buttons_to_right(
            &p,
            489.0,
            8.0,
            &[("Download Now", 105.0), ("Later", 73.0), ("Cancel", 73.0)],
            160.0,
        )[..] else {
            unreachable!("three buttons")
        };
        let helpers_at = winui::buttons_from_left(
            &p,
            11.0,
            8.0,
            &[("Helpers...", 90.0)],
            now_at.0 - 8.0 - 11.0,
        )[0];
        // Where yt-dlp and ffmpeg come from: MDM's own copies or the person's (#51).
        if winui::button(
            ui,
            r(o, helpers_at.0, BUTTONS_Y, helpers_at.1, 21.0),
            // i18n: button in Download video: where MDM takes yt-dlp and ffmpeg from
            "Helpers...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            self.open_helpers();
        }
        let now = winui::button(
            ui,
            r(o, now_at.0, BUTTONS_Y, now_at.1, 21.0),
            "Download Now",
            kind(true),
            false,
            None,
        );
        // i18n: why Download Now is greyed: the page address was changed after the look-up
        let now = winui::why_disabled(now, stale, "Look up the new address first");
        // i18n: why Download Now is greyed: a playlist's videos are still being listed
        let now = winui::why_disabled(now, listing, "Wait for the list, or stop listing");
        let now = match part_problem {
            Some(problem) if chosen && !busy && !stale => {
                winui::why_disabled(now, true, part_problem_text(problem))
            }
            _ => now,
        };
        let now = now.clicked() || (ready && winui::enter_pressed(ui));
        let later = winui::button(
            ui,
            r(o, later_at.0, BUTTONS_Y, later_at.1, 21.0),
            // i18n: button: add it now, start downloading later
            "Later",
            kind(false),
            false,
            None,
        )
        .clicked();
        let cancel = winui::button(
            ui,
            r(o, cancel_at.0, BUTTONS_Y, cancel_at.1, 21.0),
            "Cancel",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        // Escape answers the playlist's question first, when it's asked.
        let asking = self.list.as_ref().is_some_and(|l| l.asking());
        if cancel || (winui::escape_pressed(ui) && !asking) {
            return Outcome::Closed;
        }
        if ready && (now || later) {
            // A playlist of many asks first; Yes comes back as `confirmed`.
            if let Some(list) = &mut self.list
                && list.ask(now)
            {
                return Outcome::Open;
            }
            self.start_now = Some(now);
            return Outcome::Done(());
        }
        if let Some(now) = self.confirmed.take()
            && ready
        {
            self.start_now = Some(now);
            return Outcome::Done(());
        }
        Outcome::Open
    }
}

impl VideoDialog {
    /// Embed chapters (#827), under Subtitles, for a video and a playlist alike: the chapters a
    /// video's page lists go into its file (ffmpeg writes them; on by default). Greyed without
    /// ffmpeg, and for a choice converted to MP3, Opus or FLAC; its tick stays as it was.
    fn chapters_row(&mut self, ui: &mut Ui, o: Pos2, bx: f32, ffmpeg: bool) {
        let converts = match &self.list {
            Some(list) => list.converts(),
            None => self
                .video
                .as_ref()
                .and_then(|(_, c)| c.get(self.chosen))
                .is_some_and(|c| mdm_tools::resolve::audio_conversion(&c.selector).is_some()),
        };
        let part = self.part_on && self.list.is_none();
        let can = ffmpeg && !converts && !part;
        let mut on = self.chapters && can;
        let resp = winui::checkbox_until(
            ui,
            pos2(o.x + bx, o.y + 350.0 + PART_ROWS),
            &mut on,
            // i18n: checkbox in Download video: the chapters a video's page lists (YouTube's, for one) go into the video file, where players show them and skip between them
            "Embed chapters",
            can,
            Some(o.x + 489.0),
        );
        let resp = winui::why_disabled(
            resp,
            !ffmpeg,
            // i18n: why "Embed chapters" is greyed: ffmpeg (a helper program) is not installed
            "This needs ffmpeg",
        );
        let resp = winui::why_disabled(
            resp,
            ffmpeg && converts,
            // i18n: why "Embed chapters" is greyed in Download video: the choice is a sound converted to MP3, Opus or FLAC, which MDM saves without chapters
            "MP3, Opus and FLAC files are saved without chapters",
        );
        winui::why_disabled(
            resp,
            ffmpeg && !converts && part,
            // i18n: why "Embed chapters" (and the subtitles) are greyed in Download video: only a part of the video is downloaded, and its chapters and subtitles would be the whole video's
            "A part is saved without the video's chapters and subtitles",
        );
        if can {
            self.chapters = on;
        }
    }

    /// Part (#1662), under Subtitles: only from one time to another of the video or the sound.
    /// Ticked, From and To take the times (`1:30`, `1:02:03`; the video's length the bound, the
    /// whole video at first), and the cut is quick (at the keyframes, the streams copied: it may
    /// start a little before From) or exact (the part encoded again). Its estimated size beside
    /// it. Greyed without ffmpeg (it does the cut), for a playlist, before a video is looked up and
    /// for one of no known length (a live stream). True when it can be used (its label is greyed
    /// otherwise).
    fn part_row(&mut self, ui: &mut Ui, o: Pos2, bx: f32, ffmpeg: bool) -> bool {
        let p = ui.painter().clone();
        let duration = self.video.as_ref().and_then(|(v, _)| v.duration);
        let length = mdm_tools::part::length(duration);
        let hours = length.is_some_and(|l| l >= 3600);
        let looked = self.video.as_ref().is_some_and(|(_, c)| !c.is_empty());
        let can = ffmpeg && self.list.is_none() && looked && length.is_some();
        // i18n: checkbox in Download video: download only a part of the video or sound; the From and To boxes follow it
        const ONLY: &str = crate::i18n::mark("Only a part");
        let mut on = self.part_on && can;
        let tick = winui::checkbox_until(
            ui,
            pos2(o.x + bx, o.y + 350.0),
            &mut on,
            ONLY,
            can,
            Some(o.x + 489.0),
        );
        let tick = winui::why_disabled(
            tick,
            !ffmpeg,
            // i18n: why Part is greyed in Download video: ffmpeg (a helper program) is not installed
            "This needs ffmpeg",
        );
        let tick = winui::why_disabled(
            tick,
            ffmpeg && self.list.is_some(),
            // i18n: why Part is greyed in Download video: the page is a playlist, whose videos are downloaded whole
            "A playlist's videos are downloaded whole",
        );
        winui::why_disabled(
            tick,
            ffmpeg && self.list.is_none() && looked && length.is_none(),
            // i18n: why Part is greyed in Download video: the site doesn't say how long the video is (a live stream, for one)
            "The site doesn't say how long this video is",
        );
        if can && on != self.part_on {
            self.part_on = on;
        }
        let active = can && self.part_on;
        let problem = if active { self.part().err() } else { None };
        // From {from} to {to}, after the box's caption; the boxes red while their times can't be.
        let template =
            // i18n: in Download video after "Only a part": {from} and {to} are boxes where times are typed (1:30, 1:02:03), drawn where they stand in the translation
            crate::i18n::t("from {from} to {to}");
        let after = winui::checkbox_caption_end(&p, o.x + bx, ONLY) + 6.0;
        let shade = if active {
            winui::TEXT()
        } else {
            winui::TEXT_DISABLED()
        };
        let boxes = winui::sentence(
            ui,
            &p,
            template,
            &[winui::SentenceLine {
                lead: winui::Lead::Left(after),
                lead_baseline: o.y + 359.0,
                boxes: pos2(after, o.y + 346.0),
                gap: 5.0,
                baseline: o.y + 359.0,
            }],
            &[
                ("from", egui::vec2(64.0, 21.0)),
                ("to", egui::vec2(64.0, 21.0)),
            ],
            |p, x, baseline, s| {
                text(p, x, baseline, s, shade);
            },
        );
        let names = winui::sentence_names(template, &["from", "to"]);
        use mdm_tools::part::Problem;
        let from_ok = !matches!(problem, Some(Problem::NotATime))
            || mdm_tools::part::parse_time(&self.part_from).is_some();
        let to_ok = match problem {
            Some(Problem::NotATime) => mdm_tools::part::parse_time(&self.part_to).is_some(),
            Some(Problem::Backwards | Problem::PastEnd(_)) => false,
            _ => true,
        };
        let from = winui::time_field(
            ui,
            boxes[0],
            &mut self.part_from,
            length,
            hours,
            active,
            from_ok && problem != Some(Problem::Whole),
        );
        winui::name(&from, &names[0]);
        let to = winui::time_field(
            ui,
            boxes[1],
            &mut self.part_to,
            length,
            hours,
            active,
            to_ok && problem != Some(Problem::Whole),
        );
        winui::name(&to, &names[1]);
        // Its size, about: its share of the choice's (#1662).
        let ends = boxes[1].max.x.max(boxes[0].max.x);
        let ends = if winui::rtl() {
            winui::mx(boxes[1].min.x.min(boxes[0].min.x))
        } else {
            ends
        };
        if let (true, Ok(Some(part))) = (active, self.part())
            && let Some(c) = self.video.as_ref().and_then(|(_, c)| c.get(self.chosen))
            && let Some(bytes) = part.estimate(c.size, duration)
        {
            let size = crate::dialogs::dialog_size(bytes);
            winui::fit(
                ui,
                &p,
                ends + 10.0,
                o.y + 359.0,
                &crate::i18n::fmt(
                    // i18n: in Download video beside the part's From and To: the part's size, estimated from the whole video's; {size} is "12.3 MB"
                    crate::i18n::t("about {size}"),
                    &[("size", &size)],
                ),
                winui::MUTED(),
                o.x + 489.0,
                "video-part-size",
            );
        }
        // Quick or exact, under the box's caption.
        let x = o.x + bx + 16.0;
        // i18n: choice in Download video's Part: cut where the video has a keyframe, the streams copied as they are (fast; it may start a little before From)
        const QUICK: &str = crate::i18n::mark("Quick cut, at keyframes");
        // i18n: choice in Download video's Part: the part is encoded again so it starts and ends exactly at From and To (slower)
        const EXACT: &str = crate::i18n::mark("Exact cut, encoded again");
        let exact_x = winui::checkbox_caption_end(&p, x, QUICK) + 16.0;
        let quick = winui::radio_light_until(
            ui,
            pos2(x, o.y + 376.0),
            !self.part_exact,
            QUICK,
            active,
            Some(exact_x - 8.0),
        );
        let quick = quick.on_hover_text(crate::i18n::t(
            // i18n: tooltip of "Quick cut, at keyframes" in Download video: what it does
            "Fast: the streams are copied as they are, so the part may start a little before From",
        ));
        if quick.clicked() && active {
            self.part_exact = false;
        }
        let exact = winui::radio_light_until(
            ui,
            pos2(exact_x, o.y + 376.0),
            self.part_exact,
            EXACT,
            active,
            Some(o.x + 489.0),
        );
        let exact = exact.on_hover_text(crate::i18n::t(
            // i18n: tooltip of "Exact cut, encoded again" in Download video: what it does
            "Starts and ends exactly at From and To: ffmpeg encodes the part again, which takes longer",
        ));
        if exact.clicked() && active {
            self.part_exact = true;
        }
        // File name says the part while it holds the name MDM gave.
        self.name_for_part();
        can
    }

    /// Embed chapters' tick, for the settings (`video_chapters`).
    pub fn chapters(&self) -> bool {
        self.chapters
    }

    /// Subtitles (#68): a language the video has, from the uploader or the site's automatic
    /// captions, saved beside the video (`<name>.<language>.srt`) or put into it (ffmpeg).
    /// The list starts at `bx`, after the labels' column; true when it can be chosen from (its
    /// label is greyed otherwise).
    fn subtitles_row(&mut self, ui: &mut Ui, o: Pos2, bx: f32, ffmpeg: bool) -> bool {
        let choice = self
            .video
            .as_ref()
            .and_then(|(_, c)| c.get(self.chosen))
            .cloned();
        let audio = choice.as_ref().is_some_and(mdm_tools::resolve::is_audio);
        // A part's would be the whole video's, out of step with it (#1662).
        let on = !self.subs.is_empty() && !audio && !self.part_on;
        let items = sub_items(&self.subs, self.video.is_some());
        let shown: Vec<&str> = items.iter().map(String::as_str).collect();
        let pick = if on { self.sub_pick } else { 0 };
        if let Some(i) = winui::combo_light(
            ui,
            r(o, bx, 315.0, 200.0, 21.0),
            &shown,
            pick.min(shown.len().saturating_sub(1)),
            on,
            "video-subs",
        ) {
            self.sub_pick = i;
        }
        let chosen = on && self.sub_pick > 0;
        let holds = choice
            .as_ref()
            .is_some_and(mdm_tools::resolve::holds_subtitles);
        let mut embed = self.embed && chosen && ffmpeg && holds;
        let resp = winui::checkbox_until(
            ui,
            pos2(o.x + bx + 212.0, o.y + 319.0),
            &mut embed,
            // i18n: checkbox in Download video: subtitles go inside the video file instead of a separate .srt file beside it
            "Put them into the video",
            chosen && ffmpeg && holds,
            Some(o.x + 489.0),
        );
        let resp = winui::why_disabled(
            resp,
            chosen && !ffmpeg,
            // i18n: why "Put them into the video" is greyed: ffmpeg (a helper program) is not installed
            "This needs ffmpeg",
        );
        winui::why_disabled(
            resp,
            chosen && ffmpeg && !holds,
            // i18n: why "Put them into the video" is greyed: the chosen file type can't hold subtitles
            "Only MP4 and MKV videos can hold subtitles",
        );
        if chosen && ffmpeg && holds {
            self.embed = embed;
        }
        on
    }
}

/// Why Download Now is greyed while Part's times can't be (#1662), in English (`why_disabled`
/// translates it).
fn part_problem_text(problem: mdm_tools::part::Problem) -> &'static str {
    use mdm_tools::part::Problem;
    match problem {
        // i18n: why Download Now is greyed in Download video: a Part box holds something that isn't a time
        Problem::NotATime => crate::i18n::mark("Type the times as 1:30 or 1:02:03"),
        // i18n: why Download Now is greyed in Download video: Part's To time isn't after its From time
        Problem::Backwards => crate::i18n::mark("To must come after From"),
        // i18n: why Download Now is greyed in Download video: Part's To time is past the video's end
        Problem::PastEnd(_) => crate::i18n::mark("To is past the end of the video"),
        Problem::Whole => {
            // i18n: why Download Now is greyed in Download video: Part's times are the whole video's (untick Only a part to download it whole)
            crate::i18n::mark("That's the whole video: change From or To, or untick Only a part")
        }
    }
}

/// "All the uploader's" is offered when they gave two languages or more.
fn all_offered(subs: &[SubChoice]) -> bool {
    subs.iter().filter(|s| !s.automatic).count() >= 2
}

/// The Subtitles list (English where it is MDM's words; combo_light shows them translated):
/// None, All the uploader's, then each language, the automatic ones marked.
fn sub_items(subs: &[SubChoice], looked_up: bool) -> Vec<String> {
    let mut out = vec![if looked_up && subs.is_empty() {
        // i18n: in the Subtitles list of Download video: the video has no subtitles
        crate::i18n::mark("None on this page").to_owned()
    } else {
        // i18n: in the Subtitles list of Download video: download no subtitles
        crate::i18n::mark("None").to_owned()
    }];
    if all_offered(subs) {
        // i18n: in the Subtitles list of Download video: every language the video's uploader gave
        out.push(crate::i18n::mark("All the uploader's").to_owned());
    }
    for s in subs {
        out.push(if s.automatic {
            crate::i18n::fmt(
                // i18n: in the Subtitles list of Download video: {language} = a language's name; the site made these subtitles automatically (speech recognition)
                crate::i18n::t("{language}, automatic"),
                &[("language", &s.name)],
            )
        } else {
            s.name.clone()
        });
    }
    out
}

/// The entry of the Subtitles list that `pref` (the last video's choice) stands for in this
/// video's: the same language (the uploader's before the automatic one, then any variant of
/// it: `pt` for `pt-BR`), every uploader's language again, or none.
fn pick_for(subs: &[SubChoice], pref: &Subs) -> usize {
    let all = all_offered(subs);
    let first = 1 + usize::from(all);
    let Some(want) = pref.langs.first() else {
        return 0;
    };
    if want == ALL_UPLOADERS {
        return if all {
            1
        } else {
            subs.iter()
                .position(|s| !s.automatic)
                .map_or(0, |i| i + first)
        };
    }
    let base = |c: &str| c.split(['-', '_']).next().unwrap_or(c).to_ascii_lowercase();
    subs.iter()
        .position(|s| s.code == *want && s.automatic == pref.automatic)
        .or_else(|| subs.iter().position(|s| s.code == *want))
        .or_else(|| subs.iter().position(|s| base(&s.code) == base(want)))
        .map_or(0, |i| i + first)
}

/// What the Subtitles list's entry `pick` asks yt-dlp for (0: none, then "All the uploader's"
/// when offered, then `subs` in order), put into the video when `embed`.
fn subs_at(subs: &[SubChoice], pick: usize, embed: bool) -> Subs {
    let all = all_offered(subs);
    match pick {
        0 => Subs {
            embed,
            ..Default::default()
        },
        1 if all => Subs {
            langs: vec![ALL_UPLOADERS.into()],
            automatic: false,
            embed,
        },
        i => match subs.get(i - 1 - usize::from(all)) {
            Some(c) => Subs {
                langs: vec![c.code.clone()],
                automatic: c.automatic,
                embed,
            },
            None => Subs {
                embed,
                ..Default::default()
            },
        },
    }
}

/// The subtitles saved with choice `c`, from the Subtitles list's choice `chosen`: none for a
/// sound-only choice; into the video only where its file type holds them (and with ffmpeg,
/// which VideoJob checks).
pub(crate) fn subs_for_choice(c: &Choice, chosen: &Subs) -> Subs {
    if mdm_tools::resolve::is_audio(c) {
        Subs::default()
    } else {
        Subs {
            embed: chosen.embed && mdm_tools::resolve::holds_subtitles(c),
            ..chosen.clone()
        }
    }
}

/// The subtitles the window would save with choice `c` of video `v` as it opens, untouched:
/// the last video's choice (`pref`, settings.video_subs) where this video has that language
/// ([`pick_for`]), else none. The browser's quality menu adds its choice with these.
pub(crate) fn default_subs(v: &Video, c: &Choice, pref: &Subs) -> Subs {
    let subs = mdm_tools::resolve::sub_choices(v, crate::i18n::code());
    subs_for_choice(c, &subs_at(&subs, pick_for(&subs, pref), pref.embed))
}

/// #852: the help page's section on installing a JavaScript runtime for yt-dlp.
pub(crate) const JS_RUNTIME_HELP: &str = "video.md#javascript-runtime";

/// #852: what a YouTube look-up yt-dlp made without a JavaScript runtime says: in the Download
/// video window, and in the browser's quality menu.
pub(crate) fn js_runtime_note() -> &'static str {
    // i18n: in Download video and the browser's video menu: yt-dlp had no JavaScript runtime, so YouTube listed only some qualities (often only the sound); Node.js and Deno are program names
    crate::i18n::t(
        "YouTube needs a JavaScript runtime to list its video qualities: install Node.js or Deno, then look it up again",
    )
}

/// The title of the page the browser played video `v` on (`mdm_ipc::Command::video_title`,
/// trimmed), when yt-dlp knows `v` only by the address it looked up, `page` (on some servers
/// that part is a long hash, #345), or only by the page's own title (`Video::page_titled`: it has
/// no reader for the site, and a player embedded in another site's page has its own: "Watch
/// file.mp4 - VOE", where the tab names the episode, #1351). None otherwise: the title a site's
/// reader knows the video by stays (a YouTube video in an article's frame keeps its own). The
/// Download video window and the browser's quality menu both name a video by it.
///
/// A page that shows one file is often titled with the file's name (Dropbox's, a server's own
/// listing: `Episode 31.mp4`): its video or sound extension is left out, as the choice's type
/// follows the name (it was `Episode 31.mp4.mp4`, #622).
pub(crate) fn page_title<'a>(
    context: &'a mdm_ipc::Command,
    v: &Video,
    page: &str,
) -> Option<&'a str> {
    context
        .video_title
        .as_deref()
        .map(|t| without_media_extension(t.trim()))
        .filter(|t| {
            !t.is_empty()
                && (v.page_titled || mdm_tools::resolve::named_after_address(&v.title, page))
        })
}

/// `title` without a video or sound file's extension at its end (`Episode 31.mp4` → `Episode
/// 31`); as it is otherwise, or when nothing would be left.
fn without_media_extension(title: &str) -> &str {
    let media = |ext: &str| {
        mdm_engine::naming::TYPES.iter().any(|(t, e)| {
            e.eq_ignore_ascii_case(ext)
                && (t.starts_with("video/") || t.starts_with("audio/") || *t == "application/ogg")
        })
    };
    match title.rsplit_once('.') {
        Some((stem, ext)) if media(ext) && !stem.trim().is_empty() => stem.trim_end(),
        _ => title,
    }
}

/// The file name a choice of type `c` gets for a video titled `title`: made safe, with the
/// choice's type.
pub(crate) fn file_name(title: &str, c: &Choice) -> String {
    let stem = mdm_engine::naming::sanitize(title);
    format!("{stem}.{}", c.ext)
}

/// The Download video window's look-up of `page` with the browser's context: yt-dlp's, or, when
/// yt-dlp read the page generically, the video the page declares its own (#1076,
/// `mdm_tools::declared`, the page read as Options says MDM's own requests go), which the
/// download then keeps as its address. A plain file's size is asked of its server, as yt-dlp
/// doesn't say it (#320).
pub(crate) fn look_up(
    exe: &std::path::Path,
    page: &str,
    browser: &mdm_tools::proc::FromBrowser,
    rt: &tokio::runtime::Handle,
) -> Result<(Looked, Option<Declared>), String> {
    let reader = mdm_tools::streams::Engine {
        handle: rt.clone(),
        opts: crate::settings::own_net(),
    };
    let (mut looked, _, declared) =
        mdm_tools::declared::look_up(exe, page, browser, &reader).map_err(|e| e.to_string())?;
    if let Looked::Video(v) = &mut looked
        && v.needs_size()
    {
        let size = match &declared {
            Some(d) => direct_size(rt, &d.address, &d.browser(browser)),
            None => direct_size(rt, page, browser),
        };
        if let Some(size) = size {
            v.set_size(size);
        }
    }
    Ok((looked, declared))
}

/// The browser's request context `context` for the video a page declares (#1076, `d`), as its
/// download keeps it: the page as its Referer, the browser's User-Agent and cookie jar, not the
/// page's own Cookie header nor a stream's Origin (`Declared::browser`).
fn declared_context(context: &mdm_ipc::Command, d: &Declared) -> mdm_ipc::Command {
    mdm_ipc::Command {
        referer: Some(d.page.clone()),
        origin: None,
        cookies: None,
        ..context.clone()
    }
}

/// yt-dlp's look-up of `page` with the browser's context, as the browser's quality menu makes it
/// (videolookup.rs), with its answer as it gave it, when it gave one (kept for the download
/// chosen there, `mdm_tools::video::Preloaded`): a plain file's size is asked of its server, as
/// yt-dlp doesn't say it (#320).
pub(crate) fn look_up_kept(
    exe: &std::path::Path,
    page: &str,
    browser: &mdm_tools::proc::FromBrowser,
    rt: &tokio::runtime::Handle,
) -> (Result<Looked, String>, Option<mdm_tools::proc::LookedUp>) {
    let (mut res, kept) = match mdm_tools::resolve::look_up_kept(exe, page, browser) {
        Ok((looked, out)) => (Ok(looked), Some(out)),
        Err(e) => (Err(e.to_string()), None),
    };
    if let Ok(Looked::Video(v)) = &mut res
        && v.needs_size()
        && let Some(size) = direct_size(rt, page, browser)
    {
        v.set_size(size);
    }
    (res, kept)
}

/// yt-dlp's view of the browser's request context.
pub fn from_browser(c: &mdm_ipc::Command) -> mdm_tools::proc::FromBrowser {
    mdm_tools::proc::FromBrowser {
        private: c.private,
        referer: c.referer.clone(),
        origin: c.origin.clone(),
        user_agent: c.user_agent.clone(),
        cookies: c.cookies.clone(),
        jar: c.cookie_jar.clone(),
    }
}

/// A saved row keeps its private origin when its memory-only browser context is gone.
pub fn from_item(item: &crate::model::Item) -> mdm_tools::proc::FromBrowser {
    mdm_tools::proc::FromBrowser {
        private: item.is_private_window(),
        ..from_browser(&item.context)
    }
}

/// Explain an authentication refusal without suggesting the normal profile for a private row.
pub(crate) fn private_auth_error(e: &str) -> Option<String> {
    let lower = e.to_ascii_lowercase();
    [
        "sign in", "log in", "login", "private video", "members-only", "members only",
        "authentication", "anti-bot", "not a bot", "confirm your age",
    ]
    .iter()
    .any(|s| lower.contains(s))
    .then(|| {
        let hint = crate::i18n::t(
            // i18n: a private-window video's authentication failure: only cookies supplied by that private window may be used, even after restarting MDM
            "Private-window downloads do not use Cookies for video sites from Options. Open the page in a private window, sign in there, and use its Download video button again.",
        );
        format!("{e}\n\n{hint}")
    })
}

/// Whose cookies a look-up actually used, for its error message as well as its arguments.
pub(crate) fn request_error(e: &str, browser: &mdm_tools::proc::FromBrowser) -> String {
    if browser.private
        && let Some(message) = private_auth_error(e)
    {
        return message;
    }
    let sent = !browser.jar.is_empty()
        || browser
            .cookies
            .as_deref()
            .is_some_and(|c| !c.trim().is_empty());
    let store = (!browser.private && !sent)
        .then(mdm_tools::proc::browser_cookies)
        .flatten();
    lookup_error(e, sent, store.as_ref())
}

/// The size the server says for a plain file at `page`, asked as the browser would (its Referer,
/// its Origin, its cookies, its user agent). None when it won't say: the window then says "size unknown", as
/// it always did. A cookie jar (a whole site's cookies, for yt-dlp) isn't used: the header is.
fn direct_size(
    rt: &tokio::runtime::Handle,
    page: &str,
    browser: &mdm_tools::proc::FromBrowser,
) -> Option<u64> {
    use reqwest::header::{COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER};
    let url = url::Url::parse(page).ok()?;
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (REFERER, &browser.referer),
        (ORIGIN, &browser.origin),
        (COOKIE, &browser.cookies),
    ] {
        if let Some(v) = value.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
            headers.insert(name, v);
        }
    }
    let mut opts = crate::settings::own_net();
    if let Some(agent) = &browser.user_agent {
        opts.speak_as(agent.clone());
    }
    rt.block_on(mdm_engine::probe(url, headers, &opts))
        .ok()?
        .size
}

/// A browser's name as people know it, from yt-dlp's (`chrome`: Chrome).
pub fn browser_label(name: &str) -> &'static str {
    match name {
        "firefox" => "Firefox",
        "chrome" => "Chrome",
        "edge" => "Edge",
        "brave" => "Brave",
        "opera" => "Opera",
        "vivaldi" => "Vivaldi",
        "chromium" => "Chromium",
        "whale" => "Whale",
        "safari" => "Safari",
        _ => "?",
    }
}

/// yt-dlp's reason in plain words where it is one users meet often; otherwise its own text.
/// `sent`: the browser's extension passed the page's cookies; `store`: else, the browser whose
/// cookies yt-dlp read (Options → Sites Logins → Cookies for video sites, #229).
pub(crate) fn lookup_error(
    e: &str,
    sent: bool,
    store: Option<&mdm_tools::proc::BrowserCookies>,
) -> String {
    let lower = e.to_ascii_lowercase();
    let browser = store.map_or("?", |b| browser_label(&b.browser));
    let named = |text: &str| crate::i18n::fmt(crate::i18n::t(text), &[("browser", browser)]);
    // The browser's cookies yt-dlp couldn't read come first: the site's "sign in" that follows
    // is only their result.
    if store.is_some() && mdm_tools::proc::cookie_trouble(&lower) {
        return if lower.contains("could not copy chrome cookie database") {
            // i18n: the Download video window: yt-dlp couldn't read the cookies of the browser chosen in Options (Chrome, Edge...), which holds them while it runs; {browser} = its name
            named(
                "yt-dlp can't read {browser}'s cookies while {browser} is open. Close {browser} and look the page up again, or choose Firefox in Options → Sites Logins.",
            )
        } else if lower.contains("could not find") {
            // i18n: the Download video window: the browser chosen in Options has no cookies on this computer (not installed, or another profile); {browser} = its name
            named(
                "yt-dlp found no cookies of {browser} on this computer. Check the browser and its profile in Options → Sites Logins.",
            )
        } else if cfg!(windows)
            && (lower.contains("decrypt") || lower.contains("unknown cookie version"))
        {
            // i18n: the Download video window: Chrome 127 and later (and Edge, Brave) on Windows encrypt their cookies so only they can read them; {browser} = its name
            named(
                "{browser} keeps its cookies locked to itself on Windows, so yt-dlp can't read them. Choose Firefox in Options → Sites Logins, or use the Download video button over the video in {browser}.",
            )
        } else if ["keyring", "secretstorage", "kwallet", "decrypt"]
            .iter()
            .any(|k| lower.contains(k))
        {
            // i18n: the Download video window (Linux, macOS): the key to the browser's cookies is in the system's keyring (GNOME Keyring, KWallet, the macOS keychain), which didn't give it; {browser} = its name
            named(
                "yt-dlp couldn't get the key to {browser}'s cookies from the system's keyring. Unlock the keyring and look the page up again.",
            )
        } else {
            crate::i18n::fmt(
                // i18n: the Download video window: yt-dlp couldn't read the cookies of the browser chosen in Options; {browser} = its name, {error} = yt-dlp's words
                crate::i18n::t("yt-dlp couldn't read {browser}'s cookies: {error}"),
                &[("browser", browser), ("error", e)],
            )
        };
    }
    // A site's bot check turned yt-dlp away (#317).
    let plain = if lower.contains("anti-bot") {
        if store.is_some() {
            // i18n: the Download video window: the site's bot check (Cloudflare) turned yt-dlp away though it had the cookies of the browser chosen in Options; {browser} = its name
            return named(
                "The site's bot check (Cloudflare) refused yt-dlp, even with {browser}'s cookies. Use the Download video button over the video in {browser}, or try the page again later.",
            );
        }
        if sent {
            // i18n: the Download video window: the site's bot check (Cloudflare) turned yt-dlp away though the browser's extension passed the page's cookies
            "The site's bot check (Cloudflare) refused yt-dlp, even with your browser's cookies for this page. Try again later, or from another browser."
        } else {
            // i18n: the Download video window: the site's bot check (Cloudflare) turned yt-dlp away and no browser cookies were used
            "The site's bot check (Cloudflare) refused yt-dlp. If the page opens in your browser, choose that browser in Options → Sites Logins → Cookies for video sites and look the page up again, or use the Download video button over the video."
        }
    } else if mdm_tools::proc::untrusted_certificate(e) {
        // i18n: the Download video window: the site's certificate is trusted neither by yt-dlp nor by the computer (asked again with the computer's certificates, #955); usually an antivirus or a proxy that reads secure (HTTPS) connections signs sites with a certificate of its own
        "The site's certificate isn't trusted, by yt-dlp nor by this computer: an antivirus's web filter or a proxy that reads HTTPS may be in the way. Turn its HTTPS scanning off, or add its certificate to the computer's, then look the page up again."
    } else if lower.contains("drm") {
        // i18n: the Download video window: the page's video is copy-protected
        "This video is protected (DRM): no tool can save it."
    } else if lower.contains("unsupported url") {
        // i18n: the Download video window: yt-dlp found no video on the page
        "No video found on this page. Play the video, then use the Download video button over it."
    } else if lower.contains("sign in")
        || lower.contains("login")
        || lower.contains("private video")
    {
        if store.is_some() {
            // i18n: the Download video window: the site asked for a sign-in although yt-dlp had the cookies of the browser chosen in Options; {browser} = its name
            return named(
                "The site wants you signed in, even with {browser}'s cookies. Sign in to it in {browser}, then look the page up again.",
            );
        }
        if sent {
            // i18n: the Download video window: the site asked for a sign-in although the browser's extension passed the page's cookies
            "The site wants you signed in, even with your browser's cookies for this page. Sign in to it in your browser, then try again."
        } else {
            // i18n: the Download video window: the video needs the user to be signed in
            "This video needs you to be signed in. Use the Download video button over it in your browser, which passes your sign-in on, or choose your browser in Options → Sites Logins → Cookies for video sites."
        }
    } else {
        return e.to_owned();
    };
    crate::i18n::t(plain).to_owned()
}

/// A choice's size as its line shows it, and as a screen reader says it: the bytes of the
/// streams it downloads, or an estimate marked as one ("~532 MB", "about 532 MB"; #801).
pub(crate) fn choice_size(c: &Choice) -> (String, String) {
    let Some(bytes) = c.size else {
        // i18n: the video's file size is not known
        let unknown = crate::i18n::t("size unknown").to_owned();
        return (unknown.clone(), unknown);
    };
    let size = crate::dialogs::dialog_size(bytes);
    if !c.approx {
        return (size.clone(), size);
    }
    (
        crate::i18n::ltr(&format!("~{size}")),
        crate::i18n::fmt(
            // i18n: a file size that is only an estimate, as a screen reader says it in the Download video window's list of qualities; {size} is "532 MB"
            crate::i18n::t("about {size}"),
            &[("size", &size)],
        ),
    )
}

/// A choice's label in the user's language: the audio choice's words translated, the rest (a
/// resolution, a type) as they are, kept left to right.
pub(crate) fn choice_label(label: &str) -> String {
    if let Some(rest) = label.strip_prefix(mdm_tools::resolve::COMPATIBLE) {
        let height = rest.trim_start_matches([' ', '·']).trim();
        return crate::i18n::fmt(
            // i18n: a choice in the Download video window: the best H.264 video with AAC sound, an MP4 that every video editor, TV and phone plays; keep "(H.264 + AAC)" as it is; {height} is the resolution (1080p)
            crate::i18n::t("Most compatible (H.264 + AAC) · {height}"),
            &[("height", &crate::i18n::ltr(height))],
        );
    }
    if let Some(kind) = label.strip_prefix(mdm_tools::resolve::AUDIO_ORIGINAL) {
        return crate::i18n::fmt(
            // i18n: a choice in the Download video window: only the sound, the site's own file as it is (not converted); {type} is WEBM, M4A…
            crate::i18n::t("Audio only, original {type}"),
            &[("type", &crate::i18n::ltr(kind.trim()))],
        );
    }
    match label.strip_prefix(mdm_tools::resolve::AUDIO_ONLY) {
        Some(kind) => crate::i18n::fmt(
            // i18n: a choice in the Download video window: only the sound; {type} is MP3, M4A…
            crate::i18n::t("Audio only {type}"),
            &[("type", &crate::i18n::ltr(kind.trim()))],
        ),
        None => crate::i18n::ltr(label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builder start_video uses must remember a kept row's origin after serde drops its
    /// request context; in-session private context and ordinary rows keep their own behavior.
    #[test]
    fn private_video_cookie_policy_survives_a_restart() {
        let page: url::Url = "https://videos.test/watch".parse().unwrap();
        for private in [false, true] {
            let mut item = crate::model::Item::new(
                1,
                page.clone(),
                PathBuf::from("Clip.mp4"),
                Category::Video,
            );
            item.video = Some(crate::model::VideoSpec {
                selector: "best".into(),
                subs: Default::default(),
                parts: vec![],
                named: false,
                duration: None,
                part: None,
            });
            item.private_window = private;
            item.context.private = private;
            item.context.cookies = Some("sid=private-secret".into());
            let saved = serde_json::to_string(&item).unwrap();
            assert!(!saved.contains("private-secret"));
            let reloaded: crate::model::Item = serde_json::from_str(&saved).unwrap();
            assert!(!reloaded.context.private);
            assert!(reloaded.context.cookies.is_none());
            let browser = from_item(&reloaded);
            assert_eq!(browser.private, private);
            let (args, file) = browser.args(page.as_str()).unwrap();
            assert!(file.is_none());
            assert_eq!(
                args.iter().any(|a| a == "--no-cookies-from-browser"),
                private
            );
            if private {
                let cmd = mdm_tools::proc::ytdlp(
                    std::path::Path::new("yt-dlp"),
                    &[],
                    &args,
                    page.as_str(),
                )
                .unwrap();
                assert!(!cmd.get_args().any(|a| a == "--cookies-from-browser"));
                assert!(!mdm_tools::proc::has_cookies(&args));
            } else {
                assert!(args.is_empty(), "normal rows still allow Options' fallback");
            }
        }
        let mut item =
            crate::model::Item::new(2, page.clone(), PathBuf::from("Clip.mp4"), Category::Video);
        item.context.private = true;
        assert!(
            !item.private_window,
            "also protect a memory-only private context"
        );
        assert!(
            from_item(&item)
                .args(page.as_str())
                .unwrap()
                .0
                .contains(&"--no-cookies-from-browser".into())
        );
        item.context.cookies = Some("sid=private".into());
        let browser = from_item(&item);
        let (args, file) = browser.args(page.as_str()).unwrap();
        assert!(file.is_some());
        assert!(args.contains(&"--cookies".into()));
        assert!(!args.contains(&"--no-cookies-from-browser".into()));
        assert!(
            from_browser(&item.context).private,
            "fresh private look-ups too"
        );
    }

    #[test]
    fn a_private_video_authentication_error_explains_its_cookie_policy() {
        let refusal = "ERROR: Private video. Sign in to watch.";
        let browser = mdm_tools::proc::FromBrowser {
            private: true,
            ..Default::default()
        };
        let message = request_error(refusal, &browser);
        assert!(message.starts_with(refusal), "{message}");
        assert!(message.contains("Private-window downloads do not use Cookies for video sites"));
        assert!(message.contains("private window"));
        assert!(!request_error(refusal, &Default::default()).contains("Private-window downloads"));
        assert_eq!(private_auth_error("HTTP Error 500"), None);
    }

    /// #359: a Dailymotion player embedded in another site is looked up at the video's own page,
    /// without that site as its Referer (Dailymotion answers 403); any other page keeps it.
    #[test]
    fn an_embedded_player_taken_to_its_page_drops_the_embedding_sites_referer() {
        let from = |page: &str| {
            let context = mdm_ipc::Command {
                referer: Some("https://news.test/article".into()),
                ..Default::default()
            };
            VideoDialog::for_page(PathBuf::from("v"), page, context)
        };
        let embed = from("https://geo.dailymotion.com/player/x1abc.html?video=x9xyz12");
        assert_eq!(embed.page, "https://www.dailymotion.com/video/x9xyz12");
        assert_eq!(embed.context().referer, None);
        let plain = from("https://player.vimeo.test/video/1");
        assert_eq!(
            plain.context().referer.as_deref(),
            Some("https://news.test/article")
        );
    }

    /// #317: Cloudflare's check in plain words, saying what to try for each kind of cookies had.
    #[test]
    fn a_bot_check_says_what_to_try() {
        let check = "yt-dlp failed: [generic] Got HTTP Error 403 caused by Cloudflare anti-bot challenge; try again with --extractor-args \"generic:impersonate\"";
        let none = lookup_error(check, false, None);
        assert!(none.contains("bot check"), "{none}");
        assert!(none.contains("Cookies for video sites"), "{none}");
        let sent = lookup_error(check, true, None);
        assert!(sent.contains("even with your browser's cookies"), "{sent}");
        let firefox = mdm_tools::proc::BrowserCookies::new("firefox", "").unwrap();
        let chrome_had = lookup_error(check, false, Some(&firefox));
        assert!(
            chrome_had.contains("even with Firefox's cookies"),
            "{chrome_had}"
        );
        assert_eq!(
            lookup_error("yt-dlp failed: Video unavailable", false, None),
            "yt-dlp failed: Video unavailable"
        );
    }

    /// #955: a certificate trusted neither by yt-dlp nor by the computer (asked again with its
    /// certificates) says what reads HTTPS on the way; an expired one stays yt-dlp's own words.
    #[test]
    fn an_untrusted_certificate_says_what_may_be_in_the_way() {
        let untrusted = "yt-dlp failed: [youtube] X6ek6j1gTQA: Unable to download API page: [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1085) (caused by CertificateVerifyError('[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: unable to get local issuer certificate (_ssl.c:1085)'))";
        let said = lookup_error(untrusted, false, None);
        assert!(said.contains("antivirus's web filter"), "{said}");
        let expired = "yt-dlp failed: [generic] Unable to download webpage: [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: certificate has expired (_ssl.c:1006)";
        assert_eq!(lookup_error(expired, false, None), expired);
    }

    /// #229: yt-dlp's refusals and its trouble with a browser's cookies, in MDM's words, saying
    /// which cookies it had.
    #[test]
    fn cookie_and_sign_in_errors_in_plain_words() {
        let bot = "yt-dlp failed: [youtube] A1WG3x-Ebpk: Sign in to confirm you're not a bot. Use --cookies-from-browser or --cookies for the authentication.";
        assert!(lookup_error(bot, false, None).contains("Cookies for video sites"));
        assert!(lookup_error(bot, true, None).contains("even with your browser's cookies"));
        let firefox = mdm_tools::proc::BrowserCookies::new("firefox", "").unwrap();
        let chrome = mdm_tools::proc::BrowserCookies::new("chrome", "").unwrap();
        assert_eq!(
            lookup_error(bot, false, Some(&firefox)),
            "The site wants you signed in, even with Firefox's cookies. Sign in to it in Firefox, then look the page up again."
        );
        // Chrome's app-bound cookies: only a warning, kept on the error (proc::with_cookie_trouble).
        let v20 = format!("{bot} (unknown cookie version: \"b'v20'\")");
        let said = lookup_error(&v20, false, Some(&chrome));
        if cfg!(windows) {
            assert!(
                said.starts_with("Chrome keeps its cookies locked"),
                "{said}"
            );
        } else {
            // No app-bound encryption elsewhere: yt-dlp's own words.
            assert!(
                said.starts_with("yt-dlp couldn't read Chrome's cookies: "),
                "{said}"
            );
        }
        // Linux: the key wasn't in the keyring.
        let v11 = format!("{bot} (cannot decrypt v11 cookies: no key found)");
        let said = lookup_error(&v11, false, Some(&chrome));
        if cfg!(windows) {
            assert!(
                said.starts_with("Chrome keeps its cookies locked"),
                "{said}"
            );
        } else {
            assert!(said.contains("system's keyring"), "{said}");
        }
        assert!(
            lookup_error(
                "yt-dlp failed: failed to load cookies (Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info)",
                false,
                Some(&chrome)
            )
            .starts_with("yt-dlp can't read Chrome's cookies while Chrome is open")
        );
        assert!(
            lookup_error(
                "yt-dlp failed: failed to load cookies (could not find firefox cookies database in 'C:\\p')",
                false,
                Some(&firefox)
            )
            .starts_with("yt-dlp found no cookies of Firefox")
        );
        let keyring = lookup_error(
            "yt-dlp failed: Sign in (failed to read from keyring)",
            false,
            Some(&chrome),
        );
        assert!(keyring.contains("system's keyring"), "{keyring}");
        // Anything else stays yt-dlp's own words.
        assert_eq!(
            lookup_error("yt-dlp failed: Video unavailable", false, Some(&firefox)),
            "yt-dlp failed: Video unavailable"
        );
    }

    fn sub(code: &str, automatic: bool) -> SubChoice {
        SubChoice {
            code: code.into(),
            name: code.into(),
            automatic,
        }
    }

    fn want(langs: &[&str], automatic: bool) -> Subs {
        Subs {
            langs: langs.iter().map(|l| l.to_string()).collect(),
            automatic,
            embed: false,
        }
    }

    /// The Subtitles list (#68): None, All the uploader's when they gave two or more, then each
    /// language, the automatic ones marked.
    #[test]
    fn subtitle_list_entries() {
        let subs = [sub("de", false), sub("fr", false), sub("en", true)];
        assert_eq!(
            sub_items(&subs, true),
            ["None", "All the uploader's", "de", "fr", "en, automatic"]
        );
        assert_eq!(sub_items(&subs[2..], true), ["None", "en, automatic"]);
        assert_eq!(sub_items(&[], true), ["None on this page"]);
        assert_eq!(sub_items(&[], false), ["None"]);
    }

    /// The last video's choice finds its place in the next video's list: the same language
    /// (the uploader's first), a variant of it, every uploader's again, or none.
    #[test]
    fn the_last_choice_is_offered_again() {
        let subs = [sub("de", false), sub("pt-BR", false), sub("en", true)];
        assert_eq!(pick_for(&subs, &Subs::default()), 0);
        assert_eq!(pick_for(&subs, &want(&[ALL_UPLOADERS], false)), 1);
        assert_eq!(pick_for(&subs, &want(&["de"], false)), 2);
        assert_eq!(pick_for(&subs, &want(&["pt"], false)), 3, "pt for pt-BR");
        assert_eq!(
            pick_for(&subs, &want(&["en"], false)),
            4,
            "the automatic one"
        );
        assert_eq!(pick_for(&subs, &want(&["ja"], false)), 0);
        // One uploader's language only: "all" stands for it.
        let one = [sub("fr", false), sub("en", true)];
        assert_eq!(pick_for(&one, &want(&[ALL_UPLOADERS], false)), 1);
    }

    /// Each entry stands for what yt-dlp is asked.
    #[test]
    fn entries_ask_for_their_subtitles() {
        let mut d = VideoDialog::blank(PathBuf::from("."));
        d.subs = vec![sub("de", false), sub("fr", false), sub("en-orig", true)];
        d.embed = true;
        let mut at = |i: usize| {
            d.sub_pick = i;
            d.chosen_subs()
        };
        assert!(at(0).langs.is_empty());
        assert_eq!(at(1).langs, [ALL_UPLOADERS]);
        assert_eq!(at(3).langs, ["fr"]);
        let auto = at(4);
        assert_eq!(auto.langs, ["en-orig"]);
        assert!(auto.automatic && auto.embed);
    }

    /// #345: a stream the browser's button sent (its manifest, which yt-dlp names after its
    /// address) is saved under the title of the page that played it; a video yt-dlp titled
    /// itself keeps yt-dlp's title.
    #[test]
    fn a_streams_file_is_named_after_its_page() {
        const STREAM: &str = r#"{"title": "master", "formats": [
            {"format_id": "hls-720", "ext": "mp4", "height": 720, "vcodec": "avc1", "acodec": "mp4a"}]}"#;
        let video = || mdm_tools::resolve::parse(STREAM.as_bytes()).unwrap();
        let src = "https://cdn.test/v/hls/master.m3u8";
        let context = mdm_ipc::Command {
            video_page: Some(src.into()),
            video_title: Some("A talk: about download managers? ".into()),
            ..Default::default()
        };
        let mut d = VideoDialog::for_page(PathBuf::from("."), src, context.clone());
        d.set_video(video());
        assert_eq!(d.name, "A talk_ about download managers_.mp4");
        // No page title came (an older extension, or a page yt-dlp read itself): yt-dlp's.
        let mut d = VideoDialog::for_page(PathBuf::from("."), src, Default::default());
        d.set_video(video());
        assert_eq!(d.name, "master.mp4");
        // The address edited: the browser's title is not for it.
        let mut d = VideoDialog::for_page(PathBuf::from("."), src, context.clone());
        d.page = "https://cdn.test/other/index.m3u8".into();
        d.set_video(video());
        assert_eq!(d.name, "master.mp4");
        // A page yt-dlp read itself: its title, not the browser's.
        const TITLED: &str = r#"{"title": "Its own title", "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"}]}"#;
        let page = "https://videos.test/watch?v=1";
        let mut d = VideoDialog::for_page(PathBuf::from("."), page, context);
        d.set_video(mdm_tools::resolve::parse(TITLED.as_bytes()).unwrap());
        assert_eq!(d.name, "Its own title.mp4");
    }

    /// The browser's quality menu adds its choice as this window would, untouched: the same
    /// file name, and the subtitles it preselects from the last video's choice.
    #[test]
    fn the_quality_menu_adds_what_the_window_would() {
        const TALK: &str = r#"{"title": "A talk: in Lisbon", "language": "en", "duration": 60, "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 29000000},
            {"format_id": "43", "ext": "webm", "height": 480, "vcodec": "vp8", "acodec": "vorbis"},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 61000000}],
            "subtitles": {"de": [{"name": "German"}], "fr": [{"name": "French"}]}}"#;
        let v = mdm_tools::resolve::parse(TALK.as_bytes()).unwrap();
        let page = "https://videos.test/talk";
        let with_title = mdm_ipc::Command {
            video_page: Some(page.into()),
            video_title: Some(" The page's title ".into()),
            ..Default::default()
        };
        let embed = |mut s: Subs| {
            s.embed = true;
            s
        };
        for merge in [false, true] {
            let choices = mdm_tools::resolve::choices(&v, merge);
            assert!(choices.len() >= 2);
            for context in [mdm_ipc::Command::default(), with_title.clone()] {
                for pref in [
                    Subs::default(),
                    want(&["de"], false),
                    embed(want(&["fr"], false)),
                    want(&["ja"], false),
                    embed(want(&[ALL_UPLOADERS], false)),
                ] {
                    let mut d = VideoDialog::for_page(PathBuf::from("."), page, context.clone())
                        .with_subs(pref.clone());
                    // As set_video leaves it, with or without ffmpeg (not this computer's):
                    // yt-dlp's title is the page's own, which no browser title replaces.
                    assert_eq!(page_title(&context, &v, page), None);
                    d.video = Some((v.clone(), choices.clone()));
                    d.subs = mdm_tools::resolve::sub_choices(&v, crate::i18n::code());
                    d.sub_pick = pick_for(&d.subs, &d.pref);
                    for (i, c) in choices.iter().enumerate() {
                        d.chosen = i;
                        d.name_from_choice();
                        assert_eq!(
                            subs_for_choice(c, &d.chosen_subs()),
                            default_subs(&v, c, &pref),
                            "{pref:?} {c:?}"
                        );
                        assert_eq!(d.name, file_name(&v.title, c));
                    }
                }
            }
        }
        let choices = mdm_tools::resolve::choices(&v, false);
        let mp4 = choices.iter().find(|c| c.ext == "mp4").unwrap();
        let webm = choices.iter().find(|c| c.ext == "webm").unwrap();
        let audio = choices
            .iter()
            .find(|c| mdm_tools::resolve::is_audio(c))
            .unwrap();
        assert_eq!(default_subs(&v, mp4, &want(&["de"], false)).langs, ["de"]);
        let fr = default_subs(&v, webm, &embed(want(&["fr"], false)));
        assert_eq!(fr.langs, ["fr"]);
        assert!(!fr.embed, "a WebM can't hold them: beside it");
        assert!(default_subs(&v, mp4, &embed(want(&["fr"], false))).embed);
        assert!(
            default_subs(&v, audio, &want(&["de"], false))
                .langs
                .is_empty(),
            "none with the sound alone"
        );
        assert!(
            default_subs(&v, mp4, &want(&["ja"], false))
                .langs
                .is_empty(),
            "not on this page: none"
        );
        assert_eq!(file_name(&v.title, mp4), "A talk_ in Lisbon.mp4");
        // Named after its address (`talk`): the page's title, trimmed (#345).
        let hashed = Video {
            title: "talk".into(),
            ..v.clone()
        };
        assert_eq!(
            page_title(&with_title, &hashed, page),
            Some("The page's title")
        );
        assert_eq!(
            page_title(&mdm_ipc::Command::default(), &hashed, page),
            None
        );
    }

    /// #1351: a player embedded in a frame of another site's page (VOE's on an episode's page),
    /// which yt-dlp has no reader for: its generic reader names the video by the frame's own
    /// title, and the tab's names it instead. A site's reader's title (YouTube's, in an
    /// article's frame) stays.
    #[test]
    fn an_embedded_player_yt_dlp_reads_generically_is_named_by_the_tabs_title() {
        let frame = "https://player.test/e/dzA1YmtiODgyWDhCQi9NMW85K1Nq";
        let answer = |key: &str| {
            format!(
                r#"{{"title": "Watch file.mp4 - VOE | Content Delivery Network (CDN) & Video Cloud",
                    "extractor_key": "{key}",
                    "formats": [{{"format_id": "0", "ext": "mp4", "url": "https://cdn.test/v.mp4"}}]}}"#
            )
        };
        let context = mdm_ipc::Command {
            video_title: Some("Renegade Immortal Episodio 167 Sub Español".into()),
            ..Default::default()
        };
        let generic = mdm_tools::resolve::parse(answer("Generic").as_bytes()).unwrap();
        assert!(generic.page_titled);
        let title = page_title(&context, &generic, frame).unwrap();
        assert_eq!(title, "Renegade Immortal Episodio 167 Sub Español");
        let mp4 = Choice {
            ext: "mp4".into(),
            ..Default::default()
        };
        assert_eq!(
            file_name(title, &mp4),
            "Renegade Immortal Episodio 167 Sub Español.mp4"
        );
        // A page's plain `<video>` in the frame: yt-dlp's HTML5 reader, the page's title too.
        let html5 = mdm_tools::resolve::parse(answer("HTML5MediaEmbed").as_bytes()).unwrap();
        assert_eq!(
            page_title(&context, &html5, frame),
            Some("Renegade Immortal Episodio 167 Sub Español")
        );
        // Nothing from the browser: yt-dlp's title stays.
        assert_eq!(
            page_title(&mdm_ipc::Command::default(), &generic, frame),
            None
        );
        // A site yt-dlp has a reader for: its title stays.
        let known = mdm_tools::resolve::parse(answer("Youtube").as_bytes()).unwrap();
        assert!(!known.page_titled);
        assert_eq!(page_title(&context, &known, frame), None);
    }

    /// #622: a Dropbox page is titled with the file's name, extension and all; the video named
    /// by it got the choice's extension once more (`Episode 31.mp4.mp4`).
    #[test]
    fn a_page_titled_with_the_file_name_names_it_once() {
        let page = "https://www.dropbox.com/s/abc/Episode%2031.mp4?dl=0";
        let v = Video {
            title: "Episode 31".into(),
            duration: None,
            formats: Vec::new(),
            subtitles: Vec::new(),
            automatic: Vec::new(),
            language: None,
            direct: false,
            needs_js_runtime: false,
            page_titled: false,
        };
        let mp4 = Choice {
            ext: "mp4".into(),
            ..Default::default()
        };
        for (title, want) in [
            ("Episode 31.mp4", "Episode 31"),
            (" Episode 31.MKV ", "Episode 31"),
            ("Episode 31.m4a", "Episode 31"),
            // Not a media file's extension: a title as it is.
            ("Episode 31. The end", "Episode 31. The end"),
            ("Report v2.0", "Report v2.0"),
            (".mp4", ".mp4"),
        ] {
            let context = mdm_ipc::Command {
                video_title: Some(title.into()),
                ..Default::default()
            };
            let got = page_title(&context, &v, page).unwrap();
            assert_eq!(got, want, "{title}");
            if want == "Episode 31" {
                assert_eq!(file_name(got, &mp4), "Episode 31.mp4");
            }
        }
    }

    /// A film whose video plays in another site's player, in a frame (Vimeo's on a film site),
    /// as a browser saw it (`tests/data/embedded-player-title.json`): yt-dlp knows the stream
    /// the button sends only by its address (`playlist`), and the title the extension sends
    /// with it (the tab's, without the site's name: tools/extension/test.mjs) names the video
    /// in the window and its file. It was the player frame's own, "Before The - Short Film from
    /// Justin Barry on Vimeo".
    #[test]
    fn a_stream_from_an_embedded_player_is_named_by_the_pages_title() {
        let film: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/embedded-player-title.json")).unwrap();
        let text = |k: &str| film[k].as_str().unwrap().to_owned();
        let v = mdm_tools::resolve::parse(film["ytdlp"].to_string().as_bytes()).unwrap();
        assert_eq!(v.title, "playlist");
        let context = mdm_ipc::Command {
            video_title: Some(text("sent")),
            ..Default::default()
        };
        let title = page_title(&context, &v, &text("stream")).unwrap();
        assert_eq!(title, text("sent"));
        let best = &mdm_tools::resolve::choices(&v, true)[0];
        assert_eq!(best.height, Some(2160));
        assert_eq!(file_name(title, best), text("file"));
    }

    /// #661: a page's address from the clipboard comes a frame or more after the window opens:
    /// it fills an empty box only, and only with a web address.
    #[test]
    fn the_clipboard_fills_only_an_empty_page_box() {
        let (video, _) = dirs();
        let mut d = VideoDialog::blank(video.clone());
        d.fill_from_clipboard(
            " https://video.example/talk 
",
        );
        assert_eq!(d.page, "https://video.example/talk");
        d.fill_from_clipboard("https://video.example/other");
        assert_eq!(
            d.page, "https://video.example/talk",
            "not over what is there"
        );
        let mut d = VideoDialog::blank(video);
        d.fill_from_clipboard("ftp://files.example/a.mp4");
        assert_eq!(d.page, "");
    }

    /// The Video and Music categories' folders (not made: only their names are compared).
    fn dirs() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join("mdm-611");
        (base.join("Video"), base.join("Music"))
    }

    /// Download video as the app opens it (Save to: Video's folder, Music's offered), a video
    /// with a sound-only choice and video ones looked up, without yt-dlp or ffmpeg.
    fn looked_up(music: bool) -> VideoDialog {
        const SAMPLE: &str = r#"{"title": "A talk", "duration": 60, "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 2900},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 6100}]}"#;
        let (video, music_dir) = dirs();
        let mut d = VideoDialog::for_page(video, "https://video.example/talk", Default::default())
            .with_music_folder(music.then_some(music_dir));
        // No helpers looked for on this computer: no merged qualities.
        d.tools = Some((None, None, None));
        d.looked_up = d.page.clone();
        d.set_video(mdm_tools::resolve::parse(SAMPLE.as_bytes()).unwrap());
        d
    }

    /// The choices' rows: (a sound-only one, a video one).
    fn rows(d: &VideoDialog) -> (usize, usize) {
        let choices = &d.video.as_ref().unwrap().1;
        let sound = choices.iter().position(mdm_tools::resolve::is_audio);
        let video = choices
            .iter()
            .position(|c| !mdm_tools::resolve::is_audio(c));
        (sound.unwrap(), video.unwrap())
    }

    /// A row clicked in the choices list, as the list does it.
    fn choose(d: &mut VideoDialog, row: usize) {
        d.chosen = row;
        d.name_from_choice();
    }

    /// What Download Now adds for a single video: (its folder, its category).
    fn one(d: &VideoDialog) -> (PathBuf, Category) {
        match d.added() {
            Some(VideoOut::One(add)) => (add.dest.parent().unwrap().to_owned(), add.category),
            _ => panic!("a video is added"),
        }
    }

    /// #1662: Part ticked, its times typed: the download is that part (its share of the size,
    /// no subtitles), File name says it and follows the times while it is MDM's, and goes back
    /// with Part; a name typed stays. Times that can't be hold Download Now.
    #[test]
    fn a_part_is_added_with_its_name_and_size() {
        let mut d = looked_up(false);
        let (_, video) = rows(&d);
        choose(&mut d, video);
        assert_eq!((d.part_from.as_str(), d.part_to.as_str()), ("0:00", "1:00"));
        assert_eq!(d.part(), Ok(None));
        d.show_part("0:15", "0:45", true);
        assert_eq!(d.name, "A talk (15s-45s).mp4");
        let Some(VideoOut::One(add)) = d.added() else {
            panic!("a video is added")
        };
        assert_eq!(
            add.part,
            Some(mdm_tools::part::Part {
                from: 15,
                to: 45,
                exact: true
            })
        );
        assert_eq!(add.size, Some(3050), "half of 6100");
        assert!(!add.named, "MDM's name, with the part's words");
        assert!(add.subs.langs.is_empty());
        assert!(add.dest.ends_with("A talk (15s-45s).mp4"));
        // The times change: the name follows.
        d.part_to = "0:50".into();
        d.name_for_part();
        assert_eq!(d.name, "A talk (15s-50s).mp4");
        // Times that can't be: nothing added, the name waits.
        d.part_to = "0:10".into();
        d.name_for_part();
        assert_eq!(d.part(), Err(mdm_tools::part::Problem::Backwards));
        assert!(d.added().is_none());
        assert_eq!(d.name, "A talk (15s-50s).mp4");
        d.part_to = "1:01".into();
        assert_eq!(d.part(), Err(mdm_tools::part::Problem::PastEnd(60)));
        d.part_from = "0:00".into();
        d.part_to = "1:00".into();
        assert_eq!(d.part(), Err(mdm_tools::part::Problem::Whole));
        // Untick: the whole video's name again.
        d.part_on = false;
        d.name_for_part();
        assert_eq!(d.name, "A talk.mp4");
        // A name typed is kept as typed.
        d.part_on = true;
        d.name = "Mine.mp4".into();
        d.part_from = "0:20".into();
        d.name_for_part();
        assert_eq!(d.name, "Mine.mp4");
    }

    /// #1662: the browser's "Download a part from here...": Part ticked, From where the player
    /// was, To the end.
    #[test]
    fn the_browsers_part_starts_where_the_player_was() {
        let cmd = mdm_ipc::Command {
            video_part_from: Some(20),
            ..Default::default()
        };
        let (video, _) = dirs();
        let mut d = VideoDialog::for_page(video, "https://video.example/talk", cmd);
        d.tools = Some((None, None, None));
        d.looked_up = d.page.clone();
        d.set_video(
            mdm_tools::resolve::parse(
                br#"{"title": "A talk", "duration": 60, "formats": [
                {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 6100}]}"#,
            )
            .unwrap(),
        );
        assert!(d.part_on);
        assert_eq!((d.part_from.as_str(), d.part_to.as_str()), ("0:20", "1:00"));
        assert_eq!(d.name, "A talk (20s-1m00s).mp4");
    }

    /// #1076: the video a page declares is what the download keeps (its address, the page as its
    /// Referer, the browser's User-Agent, not the page's Cookie header), so the download's own
    /// look-up, when it starts, finds that one again; until the address is edited.
    #[test]
    fn a_declared_video_is_added_at_its_own_address() {
        let page = "https://films.example/films/before";
        let mut d = looked_up(false);
        d.page = page.into();
        d.from_browser = page.into();
        d.looked_up = page.into();
        d.context = mdm_ipc::Command {
            user_agent: Some("UA".into()),
            cookies: Some("session=1".into()),
            referer: Some("https://search.example/".into()),
            ..Default::default()
        };
        d.declared = Some(Declared {
            address: "https://player.vimeo.com/video/1244858801".into(),
            page: page.into(),
            name: Some("Before".into()),
        });
        let added = |d: &VideoDialog| match d.added() {
            Some(VideoOut::One(add)) => add,
            _ => panic!("a video is added"),
        };
        let add = added(&d);
        assert_eq!(
            add.page.as_str(),
            "https://player.vimeo.com/video/1244858801"
        );
        assert_eq!(add.context.referer.as_deref(), Some(page));
        assert_eq!(add.context.user_agent.as_deref(), Some("UA"));
        assert_eq!(add.context.cookies, None);
        // An address edited since: that address, as typed.
        d.page = "https://films.example/films/other".into();
        let add = added(&d);
        assert_eq!(add.page.as_str(), "https://films.example/films/other");
        assert_eq!(add.context.referer, None);
    }

    /// #611: a sound-only choice saves in the Music category's folder and is listed under Music;
    /// a video choice again goes back to Video's.
    #[test]
    fn a_sound_only_choice_saves_in_music_and_back() {
        let (video, music) = dirs();
        let mut d = looked_up(true);
        let (sound, film) = rows(&d);
        choose(&mut d, film);
        assert_eq!(d.save_folder(), video);
        assert_eq!(one(&d), (video.clone(), Category::Video));
        choose(&mut d, sound);
        assert_eq!(
            d.save_folder(),
            music,
            "Save to follows a sound-only choice"
        );
        assert_eq!(one(&d), (music.clone(), Category::Music));
        assert_eq!(d.category(), Category::Music, "the folder kept is Music's");
        choose(&mut d, film);
        assert_eq!(d.save_folder(), video, "and goes back for a video");
        assert_eq!(one(&d), (video, Category::Video));
    }

    /// #611: a folder the user typed or picked stays whatever is chosen; a sound-only download
    /// into it is still listed under Music.
    #[test]
    fn a_folder_the_user_chose_stays() {
        let mut d = looked_up(true);
        let (sound, film) = rows(&d);
        let own = std::env::temp_dir().join("mdm-611").join("Talks");
        d.folder = own.display().to_string();
        choose(&mut d, sound);
        assert_eq!(d.save_folder(), own);
        assert_eq!(one(&d), (own.clone(), Category::Music));
        choose(&mut d, film);
        assert_eq!(d.save_folder(), own);
        assert_eq!(one(&d), (own, Category::Video));
    }

    /// The Music category deleted (#93): sound-only goes where video does, as before.
    #[test]
    fn without_a_music_category_sound_goes_as_video() {
        let (video, _) = dirs();
        let mut d = looked_up(false);
        let (sound, _) = rows(&d);
        choose(&mut d, sound);
        assert_eq!(d.save_folder(), video);
        assert_eq!(one(&d), (video, Category::Video));
    }

    /// #611: a playlist downloaded as sound only goes into the Music category's folder (then the
    /// list's own folder) and each of its videos is listed under Music; a video quality, Video's.
    #[test]
    fn a_playlist_as_sound_saves_in_music() {
        let (video, music) = dirs();
        let many = |d: &VideoDialog| match d.added() {
            Some(VideoOut::Many(add)) => (add.folder.parent().unwrap().to_owned(), add.category),
            _ => panic!("a playlist is added"),
        };
        // "Audio only, original" (8), then "Up to 1080p" (4).
        for (pick, folder, category) in
            [("8", music, Category::Music), ("4", video, Category::Video)]
        {
            let mut d = looked_up(true);
            d.show_sample_list(pick);
            // As each frame of the window does.
            d.follow_sound();
            assert_eq!(d.save_folder(), folder, "quality {pick}");
            assert_eq!(many(&d), (folder, category), "quality {pick}");
        }
    }

    /// #1439: the most compatible choice is said in the user's language, its height as data.
    #[test]
    fn the_most_compatible_choice_is_labelled() {
        let l = choice_label("Most compatible (H.264 + AAC) · 1080p");
        assert!(l.starts_with("Most compatible (H.264 + AAC) · "), "{l}");
        assert!(l.contains("1080p") && !l.contains("·  "), "{l}");
        // The sample's list has it second, picked by its name for the screenshots.
        let mut d = VideoDialog::blank(PathBuf::from("."));
        // ffmpeg there (never looked for on this computer): the merged qualities.
        let ffmpeg = mdm_tools::Installed {
            version: "test".into(),
            exe: PathBuf::from("ffmpeg"),
        };
        d.tools = Some((None, None, Some(ffmpeg)));
        d.show_sample();
        d.pick("most");
        let (_, choices) = d.video.as_ref().unwrap();
        assert_eq!(d.chosen, 1);
        assert_eq!(choices[1].label, "Most compatible (H.264 + AAC) · 1080p");
        assert!(choices[1].selector.starts_with("299+140/"));
    }
}
