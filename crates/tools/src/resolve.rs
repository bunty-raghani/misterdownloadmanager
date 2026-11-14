//! What a page holds, by `yt-dlp -J`: the title and the formats, turned into the few choices a
//! person picks from (one per resolution, plus audio only), as IDM's video panel offers them.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::proc::{self, ProcError};

#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub title: String,
    /// Seconds.
    pub duration: Option<f64>,
    pub formats: Vec<Format>,
    /// The uploader's subtitles, by name (#68).
    pub subtitles: Vec<Track>,
    /// The site's automatic captions (speech recognition, and their machine translations).
    pub automatic: Vec<Track>,
    /// The language the video is spoken in, when the site says (`en`, `pt-BR`).
    pub language: Option<String>,
    /// A plain file at this address, not a page or a stream (yt-dlp's `direct`). yt-dlp says
    /// nothing of its size then: the server is asked (`needs_size`, #320).
    pub direct: bool,
    /// yt-dlp said it had no JavaScript runtime for the site's qualities (#852,
    /// [`proc::missing_js_runtime`]): YouTube's list is short without one.
    pub needs_js_runtime: bool,
}

impl Video {
    /// A plain file whose size nobody has said yet.
    pub fn needs_size(&self) -> bool {
        self.direct && self.formats.iter().all(|f| f.size.is_none())
    }

    /// The size the file's server said.
    pub fn set_size(&mut self, size: u64) {
        for f in &mut self.formats {
            f.size = Some(size);
            f.approx = false;
        }
    }
}

/// One language of subtitles a page has (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// yt-dlp's language code, what `--sub-langs` takes (`en`, `pt-BR`, `en-orig`).
    pub code: String,
    /// The site's name for it, else the code.
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Format {
    pub id: String,
    pub ext: String,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub video: bool,
    pub audio: bool,
    /// Bytes, exact or estimated ([`Format::approx`]).
    pub size: Option<u64>,
    /// The size is an estimate: the site's own guess (`filesize_approx`), or the bitrate times
    /// the length; not the bytes that will come (#801).
    pub approx: bool,
    /// Kbit/s, when the site says (a stream's variants differ by it alone).
    pub tbr: Option<f64>,
    /// Comes as an HLS stream (yt-dlp's `protocol` `m3u8`, `m3u8_native`): its pieces make a
    /// proper file only once ffmpeg has put them into one (#569).
    pub hls: bool,
    /// yt-dlp's codecs (`av01.0.13M.08`, `opus`; `none` for the one that isn't there), when said.
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    /// yt-dlp's `dynamic_range` when not SDR: `HDR10`, `HDR10+`, `HDR12`, `HLG`, `DV` (#755).
    pub range: Option<String>,
    /// yt-dlp's `language_preference` (YouTube's original sound is above its dubs), `quality`
    /// and `audio_channels`: the first things its sorter weighs for the sound (`ba`).
    pub language_preference: Option<f64>,
    pub quality: Option<f64>,
    pub channels: Option<f64>,
    /// yt-dlp's `preference`: the site's own ranking, weighed before anything else.
    pub preference: Option<f64>,
}

/// One line of the choice list: what to show and what to ask yt-dlp for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    /// yt-dlp's `-f` selector: for a video or the sound as the site has it, the very formats the
    /// line was sized from (`401+251`), then, should the page no longer offer them, the nearest
    /// (#801).
    pub selector: String,
    /// The container of the result (for the file name).
    pub ext: String,
    /// The bytes of every stream it downloads (a video's picture and sound), or a conversion's
    /// estimate.
    pub size: Option<u64>,
    /// [`Choice::size`] is an estimate (shown as "~532 MB").
    pub approx: bool,
    /// The picture's height in pixels; none for the sound, or a file known only by its type.
    pub height: Option<u32>,
    /// Frames a second, rounded (`60`, `30`, `24`), when the site says.
    pub fps: Option<u32>,
    /// The picture's codec as people name it (`AV1`, `VP9`, `H.264`; [`codec_name`]).
    pub codec: Option<String>,
    /// `HDR` or `Dolby Vision`; `SDR` only beside an HDR line of the same height (#755).
    pub range: Option<String>,
    /// The sound's bitrate in kbit/s (the sound as the site has it, an MP3 at a constant one).
    pub kbps: Option<u32>,
}

#[derive(Deserialize)]
struct RawVideo {
    title: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(default)]
    subtitles: Option<std::collections::BTreeMap<String, Vec<RawSub>>>,
    #[serde(default)]
    automatic_captions: Option<std::collections::BTreeMap<String, Vec<RawSub>>>,
    language: Option<String>,
    /// `null` from some extractors (Wikimedia Commons'): not a plain file.
    #[serde(default)]
    direct: Option<bool>,
}

/// One file of a subtitle language (its formats: srt, vtt, json3…); only the name is read.
#[derive(Deserialize)]
struct RawSub {
    name: Option<String>,
}

/// Not subtitles, though yt-dlp lists them with them: a live stream's chat replay, comments
/// laid over the picture.
const NOT_SUBTITLES: [&str; 2] = ["live_chat", "danmaku"];

/// A page's subtitle languages, by name (the code when the site gives none).
fn tracks(raw: Option<std::collections::BTreeMap<String, Vec<RawSub>>>) -> Vec<Track> {
    let mut out: Vec<Track> = raw
        .unwrap_or_default()
        .into_iter()
        .filter(|(code, files)| !files.is_empty() && !NOT_SUBTITLES.contains(&code.as_str()))
        .map(|(code, files)| Track {
            name: files
                .iter()
                .find_map(|f| f.name.clone())
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| code.clone()),
            code,
        })
        .collect();
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Subtitle languages that come without names, as yt-dlp lists an HLS stream's (`de`, `und`):
/// what [`tracks`] makes of them (`crate::manifest`).
pub(crate) fn unnamed_tracks(codes: impl IntoIterator<Item = String>) -> Vec<Track> {
    tracks(Some(
        codes
            .into_iter()
            .map(|code| (code, vec![RawSub { name: None }]))
            .collect(),
    ))
}

/// The size of `duration` seconds at `tbr` kbit/s (kbit/s × 125 = bytes/s), as yt-dlp's
/// `filesize_from_tbr` reckons it: the same operations in the same order, so a stream's variants
/// come out the same whether yt-dlp listed them or MDM read the stream itself
/// (`crate::manifest`).
pub(crate) fn estimate(duration: f64, tbr: f64) -> u64 {
    (duration * tbr * 125.0) as u64
}

#[derive(Deserialize)]
struct RawFormat {
    format_id: String,
    ext: Option<String>,
    height: Option<u32>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    #[serde(default, deserialize_with = "size")]
    filesize: Option<u64>,
    #[serde(default, deserialize_with = "size")]
    filesize_approx: Option<u64>,
    tbr: Option<f64>,
    protocol: Option<String>,
    dynamic_range: Option<String>,
    language_preference: Option<f64>,
    quality: Option<f64>,
    audio_channels: Option<f64>,
    preference: Option<f64>,
    /// yt-dlp leaves out DRM formats when it chooses (unless asked to allow them).
    #[serde(default)]
    has_drm: Option<serde_json::Value>,
}

/// A size as yt-dlp gives it: a count of bytes, or nothing known. Wikimedia Commons' transcodes
/// come with a negative estimate (`filesize_approx: -79319`, its bitrate unknown), which made
/// the whole page unreadable ("it gave no video information"): no size then, as for any other
/// that isn't a positive number.
fn size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_f64().filter(|f| *f > 0.0).map(|f| f as u64))
    })
    .filter(|n| *n > 0))
}

/// Whether `title` is only the last part of `page`'s address, which is what yt-dlp calls a video
/// on a site it has no page reader for (#345: on some servers that part is a long hash).
pub fn named_after_address(title: &str, page: &str) -> bool {
    let Some(last) = url::Url::parse(page).ok().and_then(|u| {
        u.path_segments()
            .and_then(|mut p| p.next_back().map(str::to_owned))
    }) else {
        return false;
    };
    let last = percent_encoding::percent_decode_str(&last).decode_utf8_lossy();
    let stem = last.rsplit_once('.').map_or(last.as_ref(), |(s, _)| s);
    !stem.is_empty() && title.trim() == stem
}

/// yt-dlp's `-J` answer with the real type of a file a server's program sends (#695): yt-dlp
/// takes a plain file's extension from its address (`stream.php?file=x.mp4` → `php`), lists it
/// so ("PHP") and then refuses to save it ("The extracted extension (php) is unusual and will
/// be skipped for safety reasons"). Such a format's extension is the one MDM names the file with
/// (`mdm_engine::naming`): its address's `file=` value's, else the server's type, which yt-dlp
/// gives a plain file as its format id (`video/mp4` → `mp4`). The answer is given as it came
/// when nothing in it is such a format, or it isn't JSON.
pub fn typed_scripts(json: Vec<u8>) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&json) else {
        return json;
    };
    let mut changed = retype(&mut v);
    for list in ["formats", "requested_formats"] {
        if let Some(fs) = v.get_mut(list).and_then(serde_json::Value::as_array_mut) {
            for f in fs {
                changed |= retype(f);
            }
        }
    }
    match changed {
        true => serde_json::to_vec(&v).unwrap_or(json),
        false => json,
    }
}

/// [`typed_scripts`] for one format (or the chosen one's fields at the top): true when its
/// extension, a program's or a page's, became its file's type's.
fn retype(f: &mut serde_json::Value) -> bool {
    use mdm_engine::naming;
    let Some(ext) = f.get("ext").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    if !naming::SCRIPT_EXTENSIONS.contains(&ext.as_str())
        && !naming::PAGE_EXTENSIONS.contains(&ext.as_str())
    {
        return false;
    }
    let text = |k: &str| f.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    // The `file=` value's own extension, when the address has one.
    let from_address = url::Url::parse(text("url")).ok().and_then(|u| {
        let name = naming::address_name(&u);
        let (_, e) = name.rsplit_once('.')?;
        (!naming::is_script_name(&name) && naming::is_typed_extension(e))
            .then(|| e.to_ascii_lowercase())
    });
    // A plain file's format id is its type's subtype, as is or after an `f` (yt-dlp 2026.10:
    // `fmp4` for video/mp4; `webm`, `mpeg`): sound first when it has no picture.
    let id = text("format_id").to_ascii_lowercase();
    let families = match text("vcodec") {
        "none" => ["audio", "video"],
        _ => ["video", "audio"],
    };
    let from_type = || {
        [Some(id.as_str()), id.strip_prefix('f')]
            .into_iter()
            .flatten()
            .find_map(|subtype| {
                families
                    .iter()
                    .find_map(|k| naming::type_extension(&format!("{k}/{subtype}")))
            })
            .map(str::to_owned)
    };
    match from_address.or_else(from_type) {
        Some(e) => {
            f["ext"] = serde_json::Value::String(e);
            true
        }
        None => false,
    }
}

/// Parse `yt-dlp -J` output.
pub fn parse(json: &[u8]) -> Option<Video> {
    let raw: RawVideo = serde_json::from_slice(json).ok()?;
    // yt-dlp: "none" means the stream isn't there; a missing codec is just unknown (plain
    // files on generic sites carry both).
    let has = |c: &Option<String>| c.as_deref() != Some("none");
    // An HLS stream's variants give their bitrate (BANDWIDTH) but no size, though yt-dlp knows
    // the duration: about bitrate × duration (kbit/s × 125 = bytes/s), as IDM shows it. A live
    // stream has no duration, and stays without one. Never for a plain file, whose server says
    // its exact size (`needs_size`, #320).
    let duration = raw
        .duration
        .filter(|d| d.is_finite() && *d > 0.0 && raw.direct != Some(true));
    let estimate = |tbr: Option<f64>| {
        let tbr = tbr.filter(|t| t.is_finite() && *t > 0.0)?;
        Some(estimate(duration?, tbr))
    };
    Some(Video {
        title: raw.title.unwrap_or_else(|| "video".into()),
        duration: raw.duration,
        formats: raw
            .formats
            .into_iter()
            // yt-dlp never chooses a DRM format (`has_drm` true; "maybe" it still tries).
            .filter(|f| f.has_drm.as_ref().and_then(serde_json::Value::as_bool) != Some(true))
            .map(|f| Format {
                video: has(&f.vcodec),
                audio: has(&f.acodec),
                id: f.format_id,
                ext: f.ext.unwrap_or_default(),
                height: f.height,
                fps: f.fps.filter(|x| x.is_finite() && *x > 0.0),
                approx: f.filesize.is_none(),
                size: f.filesize.or(f.filesize_approx).or_else(|| estimate(f.tbr)),
                tbr: f.tbr,
                hls: f.protocol.as_deref().is_some_and(|p| p.contains("m3u8")),
                vcodec: f.vcodec,
                acodec: f.acodec,
                // SDR is none: what a stream's own playlist (`crate::manifest`) says too.
                range: f.dynamic_range.filter(|r| !r.eq_ignore_ascii_case("SDR")),
                language_preference: f.language_preference,
                quality: f.quality,
                channels: f.audio_channels,
                preference: f.preference,
            })
            .collect(),
        subtitles: tracks(raw.subtitles),
        automatic: tracks(raw.automatic_captions),
        language: raw.language,
        direct: raw.direct == Some(true),
        needs_js_runtime: false,
    })
}

/// A subtitle language offered in the video window (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubChoice {
    pub code: String,
    pub name: String,
    /// The site's automatic captions, not the uploader's.
    pub automatic: bool,
}

/// The subtitle languages to offer, the uploader's first (all of them), then the automatic
/// captions that are worth a place: the ones in the language spoken (YouTube marks them
/// `-orig`), and their translations into `wanted` (the interface language) and English. A
/// site's automatic list holds a translation into every language it knows, a hundred and more:
/// offered whole, the list would be useless.
pub fn sub_choices(v: &Video, wanted: &str) -> Vec<SubChoice> {
    let mut out: Vec<SubChoice> = v
        .subtitles
        .iter()
        .map(|t| SubChoice {
            code: t.code.clone(),
            name: t.name.clone(),
            automatic: false,
        })
        .collect();
    // `pt-BR` and `pt` are the same language here.
    let base = |c: &str| c.split(['-', '_']).next().unwrap_or(c).to_ascii_lowercase();
    let spoken = v.language.as_deref().map(base);
    let worth = |t: &Track| {
        t.code.ends_with("-orig")
            || spoken.as_deref() == Some(base(&t.code).as_str())
            || base(&t.code) == base(wanted)
            || base(&t.code) == "en"
    };
    for t in v.automatic.iter().filter(|t| worth(t)) {
        // The uploader's own in that language is the better one.
        if v.subtitles.iter().any(|u| u.code == t.code) {
            continue;
        }
        out.push(SubChoice {
            code: t.code.clone(),
            name: t.name.clone(),
            automatic: true,
        });
    }
    out
}

/// A page that lists videos (#69): a playlist, a channel, an album; what one look-up says of
/// it. Its videos are listed afterwards, streamed (`crate::playlist`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListPage {
    pub title: String,
    /// How many videos, when the site says (a playlist does, a channel doesn't).
    pub total: Option<u64>,
}

/// What a page is: one video, or a list of them.
#[derive(Clone, Debug, PartialEq)]
pub enum Looked {
    Video(Video),
    List(ListPage),
}

#[derive(Deserialize)]
struct RawKind {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    playlist_count: Option<u64>,
}

/// Parse `yt-dlp -J` output: a video, or a list's first page (`_type: playlist`).
pub fn parse_looked(json: &[u8]) -> Option<Looked> {
    let kind: RawKind = serde_json::from_slice(json).ok()?;
    if kind.kind.as_deref() == Some("playlist") {
        return Some(Looked::List(ListPage {
            title: kind
                .title
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "playlist".into()),
            total: kind.playlist_count,
        }));
    }
    parse(json).map(Looked::Video)
}

/// How long one yt-dlp look-up may take. `proc::look_up` may run it twice (the second time
/// without the cookies, #352).
pub const LOOK_UP_TIMEOUT: Duration = Duration::from_secs(90);

/// Look `url` up with yt-dlp: one video (`--no-playlist`: a video in a playlist is that video),
/// or, for a page that is a list, only what it is and its first entry, flat (#69: never every
/// video of a 6,000-long playlist at once; `--flat-playlist -I 1:1` doesn't change a video's
/// look-up). yt-dlp's warnings stay on: the one saying it has no JavaScript runtime is how a
/// short YouTube list explains itself (#852, `Video::needs_js_runtime`).
pub fn look_up(exe: &Path, url: &str, browser: &proc::FromBrowser) -> Result<Looked, ProcError> {
    look_up_kept(exe, url, browser).map(|(looked, _)| looked)
}

/// [`look_up`], with yt-dlp's answer as it gave it: the browser's quality menu keeps it for the
/// download chosen there, which then doesn't read the page again (`video::Preloaded`).
pub fn look_up_kept(
    exe: &Path,
    url: &str,
    browser: &proc::FromBrowser,
) -> Result<(Looked, proc::LookedUp), ProcError> {
    let mut args: Vec<String> = [
        "-J",
        "--no-playlist",
        "--flat-playlist",
        "--playlist-items",
        "1:1",
    ]
    .map(String::from)
    .to_vec();
    // Kept until yt-dlp has answered.
    let (theirs, _cookies) = browser.args(url)?;
    args.extend(theirs);
    let out = proc::look_up(exe, &args, url, LOOK_UP_TIMEOUT)?;
    let mut looked = parse_looked(&out.json).ok_or_else(|| ProcError::Failed {
        tool: "yt-dlp".into(),
        message: "it gave no video information".into(),
    })?;
    if let Looked::Video(v) = &mut looked {
        v.needs_js_runtime = proc::missing_js_runtime(&out.warnings, proc::js_runtime_here());
    }
    Ok((looked, out))
}

/// Look `url` up as one video (a list is an error here).
pub fn resolve(exe: &Path, url: &str, browser: &proc::FromBrowser) -> Result<Video, ProcError> {
    match look_up(exe, url, browser)? {
        Looked::Video(v) => Ok(v),
        Looked::List(l) => Err(ProcError::Failed {
            tool: "yt-dlp".into(),
            message: format!("the address is a list of videos ({})", l.title),
        }),
    }
}

/// The audio choice's label starts with this (the app translates it).
pub const AUDIO_ONLY: &str = "Audio only";
/// The sound as the site has it, not converted (#70): its label starts with this.
pub const AUDIO_ORIGINAL: &str = "Audio only original";

/// Opus and FLAC choices' selectors (#70): the best sound converted by ffmpeg (Opus as is when
/// it already is Opus: yt-dlp then only moves it into an `.opus` file).
pub const OPUS: &str = "opus:";
pub const FLAC: &str = "flac:";

/// What an audio choice converts to: the file type and yt-dlp's `--audio-quality`, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOut {
    pub format: &'static str,
    pub quality: Option<String>,
}

/// The conversion an audio choice's selector asks for (MP3 #32, Opus and FLAC #70); none for a
/// video or the original sound.
pub fn audio_conversion(selector: &str) -> Option<AudioOut> {
    if let Some(q) = mp3_quality(selector) {
        return Some(AudioOut {
            format: "mp3",
            quality: Some(q),
        });
    }
    match selector {
        OPUS => Some(AudioOut {
            format: "opus",
            quality: None,
        }),
        FLAC => Some(AudioOut {
            format: "flac",
            quality: None,
        }),
        _ => None,
    }
}

/// An MP3 choice's selector: `mp3:<kbps>`, the best sound converted to MP3 by ffmpeg at that
/// constant bitrate, or `mp3:0` at its best variable one (`VideoJob::start`; #32).
pub const MP3: &str = "mp3:";

/// The MP3 bitrates offered, best first.
pub const MP3_RATES: [u32; 4] = [320, 256, 192, 128];

/// yt-dlp's `--audio-quality` for an MP3 choice's `<kbps>` (`MP3`): `0` (the best variable
/// bitrate) or a bitrate MDM offers, written exactly (`192`, not `+192` or `0192`); none for
/// anything else.
pub fn mp3_quality(selector: &str) -> Option<String> {
    let kbps = selector.strip_prefix(MP3)?;
    if kbps == "0" {
        return Some("0".into());
    }
    MP3_RATES
        .iter()
        .find(|k| k.to_string() == kbps)
        .map(|k| format!("{k}K"))
}

/// Containers that only ever hold sound.
fn audio_container(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "m4a" | "mp3" | "opus" | "aac" | "flac" | "wav" | "weba" | "oga" | "mka"
    )
}

/// The sound as the site has it: `ba`, or a choice's very format before it (`251/ba`, #801).
pub fn original_sound(selector: &str) -> bool {
    selector == "ba" || selector.ends_with("/ba")
}

/// A choice that saves only the sound (no picture to show subtitles on, #68).
pub fn is_audio(c: &Choice) -> bool {
    sound_only(&c.selector, &c.ext)
}

/// A selector and file type that save only the sound: a choice ([`is_audio`]) or a playlist's
/// quality (saved in the Music category's folder, #611).
pub fn sound_only(selector: &str, ext: &str) -> bool {
    original_sound(selector) || audio_conversion(selector).is_some() || audio_container(ext)
}

/// A choice whose file can hold subtitles put into it (yt-dlp's `--embed-subs` with SRT).
pub fn holds_subtitles(c: &Choice) -> bool {
    matches!(
        c.ext.to_ascii_lowercase().as_str(),
        "mp4" | "mkv" | "mov" | "m4v"
    )
}

/// Would merging (ffmpeg) bring a better video than the ones already carrying their sound (#351)?
/// YouTube's only such format is 360p, and without a JavaScript runtime yt-dlp often has none:
/// the list then holds the sound alone, so the window must say why.
pub fn needs_merging(v: &Video) -> bool {
    let best = |merging: bool| {
        v.formats
            .iter()
            .filter(|f| f.video && (merging || f.audio))
            .filter_map(|f| f.height)
            .max()
    };
    best(true) > best(false)
}

/// A format selector for yt-dlp without ffmpeg (#569): each of its "best" picks (`ba`,
/// `b[height=720]`) tried first among the formats that come as a file, then as it was. An HLS
/// stream's pieces are joined as they come: without ffmpeg to put them into the container they
/// were named for, YouTube's HLS sound is raw AAC under an `.mp4` name, which no player opens
/// as one. A merge (`bv*+ba`, which needs ffmpeg anyway) and a format named by its ID (the one
/// asked for) are left as they are.
pub fn without_remuxing(selector: &str) -> String {
    const BEST: [&str; 10] = [
        "b",
        "ba",
        "bv",
        "b*",
        "ba*",
        "bv*",
        "best",
        "bestaudio",
        "bestvideo",
        "best*",
    ];
    selector
        .split('/')
        .map(|alt| {
            let base = alt.split('[').next().unwrap_or(alt).trim();
            if BEST.contains(&base) && !alt.contains('+') && !alt.contains("protocol") {
                format!("{alt}[protocol!*=m3u8]/{alt}")
            } else {
                alt.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// yt-dlp's preference among picture codecs (its format sorter's `vcodec`, best highest): AV1,
/// VP9 profile 2, VP9, H.265, H.264, VP8, MPEG-4 and H.263, Theora; 1 for one it doesn't know,
/// 0 for none.
pub(crate) fn picture_rank(c: Option<&str>) -> u8 {
    let Some(c) = c else { return 1 };
    let c = c.to_ascii_lowercase();
    let any = |p: &[&str]| p.iter().any(|x| c.starts_with(x));
    match c.as_str() {
        "none" => 0,
        _ if any(&["av01", "av1"]) => 10,
        _ if any(&["vp9.2", "vp09.02", "vp9.02", "vp09.2"]) => 9,
        _ if any(&["vp9", "vp09"]) => 8,
        _ if any(&["h265", "x265", "hv", "hev"]) => 7,
        _ if any(&["h264", "x264", "avc"]) => 6,
        _ if any(&["vp8", "vp08"]) => 5,
        _ if any(&["mp4v", "h263"]) => 4,
        _ if any(&["theora"]) => 3,
        _ => 2,
    }
}

/// yt-dlp's preference among sound codecs (its sorter's `acodec`, best highest).
pub(crate) fn sound_rank(c: Option<&str>) -> u8 {
    let Some(c) = c else { return 1 };
    let c = c.to_ascii_lowercase();
    let any = |p: &[&str]| p.iter().any(|x| c.starts_with(x));
    match c.as_str() {
        "none" => 0,
        _ if any(&["flac", "alac"]) => 13,
        _ if any(&["wav", "aiff"]) => 12,
        _ if any(&["opus"]) => 11,
        _ if any(&["vorbis", "ogg"]) => 10,
        _ if any(&["aac"]) => 9,
        _ if any(&["m4", "mp4"]) => 8,
        _ if any(&["mp3"]) => 7,
        _ if any(&["ac4", "ac-4"]) => 6,
        _ if any(&["ec3", "eac3", "ec-3", "eac-3", "e-ac-3", "e-ac3"]) => 5,
        _ if any(&["ac3", "ac-3"]) => 4,
        _ if any(&["dts"]) => 3,
        _ => 2,
    }
}

/// A picture codec as people name it (#755): `AV1`, `VP9`, `H.265` (Dolby Vision's too),
/// `H.264`, `VP8`, `MPEG-4`, `H.263`, `Theora`; none when yt-dlp doesn't say, or names one
/// of its own.
pub fn codec_name(vcodec: Option<&str>) -> Option<&'static str> {
    let c = vcodec?.trim().to_ascii_lowercase();
    // Dolby Vision's names say the codec under it.
    if c.starts_with("dvh") {
        return Some("H.265");
    }
    if c.starts_with("dav1") {
        return Some("AV1");
    }
    if c.starts_with("dva") {
        return Some("H.264");
    }
    match picture_rank(Some(&c)) {
        10 => Some("AV1"),
        8 | 9 => Some("VP9"),
        7 => Some("H.265"),
        6 => Some("H.264"),
        5 => Some("VP8"),
        4 if c.starts_with("mp4v") => Some("MPEG-4"),
        4 => Some("H.263"),
        3 => Some("Theora"),
        _ => None,
    }
}

/// yt-dlp's preference among dynamic ranges (its sorter's `hdr:12`, best highest).
fn range_rank(r: Option<&str>) -> u8 {
    match r.map(str::to_ascii_uppercase).as_deref() {
        Some("HDR12") => 6,
        Some("HDR10+") => 5,
        Some("HDR10") => 4,
        Some("HLG") => 3,
        Some("DV") => 2,
        _ => 1,
    }
}

/// A dynamic range as a line says it (#755): `HDR`, `Dolby Vision`; none for SDR (or unknown).
fn range_name(r: Option<&str>) -> Option<&'static str> {
    match r.map(str::to_ascii_uppercase).as_deref() {
        None | Some("SDR" | "") => None,
        Some("DV") => Some("Dolby Vision"),
        Some(_) => Some("HDR"),
    }
}

/// Whether yt-dlp's `-f` takes `id` as that format's ID: plain characters only (no selector
/// syntax), and none of the words it reads otherwise (`best`, `mp4`: the best of that type).
fn plain_id(id: &str) -> bool {
    const WORDS: [&str; 30] = [
        "b",
        "w",
        "best",
        "worst",
        "b*",
        "w*",
        "bv",
        "ba",
        "wv",
        "wa",
        "bestvideo",
        "bestaudio",
        "worstvideo",
        "worstaudio",
        "all",
        "mergeall",
        "3gp",
        "aac",
        "flv",
        "m4a",
        "mp3",
        "mp4",
        "ogg",
        "wav",
        "webm",
        "mov",
        "mkv",
        "opus",
        "flac",
        "m4v",
    ];
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !WORDS.contains(&id.to_ascii_lowercase().as_str())
}

/// `a` against `b` where a bigger value is better and the unknown is `default`.
fn more(a: Option<f64>, b: Option<f64>, default: f64) -> std::cmp::Ordering {
    a.unwrap_or(default).total_cmp(&b.unwrap_or(default))
}

/// The page's sounds alone, best first, as yt-dlp's `ba` takes them: the spoken language's
/// (YouTube's original before its dubs), the site's quality, channels, codec (Opus before AAC),
/// size, bitrate. Without ffmpeg one that comes as a file goes first (#569: an HLS one stays its
/// raw pieces).
fn sounds(v: &Video, can_merge: bool) -> Vec<&Format> {
    let mut out: Vec<&Format> = v.formats.iter().filter(|f| f.audio && !f.video).collect();
    out.sort_by(|a, b| {
        (can_merge || !b.hls)
            .cmp(&(can_merge || !a.hls))
            .then(more(b.preference, a.preference, 0.0))
            .then(more(b.language_preference, a.language_preference, -1.0))
            .then(more(b.quality, a.quality, -1.0))
            .then(more(b.channels, a.channels, 0.0))
            .then(sound_rank(b.acodec.as_deref()).cmp(&sound_rank(a.acodec.as_deref())))
            .then(b.size.unwrap_or(0).cmp(&a.size.unwrap_or(0)))
            .then(more(b.tbr, a.tbr, 0.0))
            .then(a.hls.cmp(&b.hls))
            .then(b.id.cmp(&a.id))
    });
    out
}

/// One video line before it is labelled: the format it downloads and what it shows.
struct Line<'a> {
    format: &'a Format,
    height: u32,
    fps: Option<u32>,
    codec: Option<&'static str>,
    range: Option<&'static str>,
}

/// A video line's words (#755): its height, its file type when that isn't MP4 (or nothing else
/// is known), the codec, the frame rate and the range: `2160p · VP9 · 60 fps · HDR`.
fn video_label(
    height: u32,
    ext: &str,
    codec: Option<&str>,
    fps: Option<u32>,
    range: Option<&str>,
) -> String {
    let mut parts = vec![format!("{height}p")];
    if !ext.eq_ignore_ascii_case("mp4") || codec.is_none() {
        parts.push(ext.to_uppercase());
    }
    parts.extend(codec.map(str::to_owned));
    parts.extend(fps.map(|f| format!("{f} fps")));
    parts.extend(range.map(str::to_owned));
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// The video lines, best first: one for each height, frame rate, codec and range the page
/// offers (#755: AV1 and VP9 of one height, its HDR and SDR, are each their own), each the very
/// format it was sized from, merged with the best sound (`audio`) when ffmpeg is there (#801:
/// the size shown is the size that comes). Without ffmpeg only formats that carry both.
fn videos(v: &Video, can_merge: bool, audio: Option<&Format>) -> Vec<Choice> {
    let candidates = v
        .formats
        .iter()
        .filter(|f| f.video && f.height.is_some() && (can_merge || f.audio));
    // The format that stands for a line: one that comes as a file (an HLS variant's size is
    // its peak bitrate's guess), with an exact size, the bigger; with ffmpeg, a picture alone
    // (merged with the best sound, not with its own).
    let better = |a: &Format, b: &Format| {
        (can_merge && !a.audio)
            .cmp(&(can_merge && !b.audio))
            .then(more(a.preference, b.preference, 0.0))
            .then((!a.hls).cmp(&!b.hls))
            .then((a.size.is_some() && !a.approx).cmp(&(b.size.is_some() && !b.approx)))
            .then(range_rank(a.range.as_deref()).cmp(&range_rank(b.range.as_deref())))
            .then(picture_rank(a.vcodec.as_deref()).cmp(&picture_rank(b.vcodec.as_deref())))
            .then(a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)))
            .then(more(a.tbr, b.tbr, 0.0))
            .then(a.id.cmp(&b.id))
    };
    let mut lines: Vec<Line> = Vec::new();
    for f in candidates {
        let line = Line {
            format: f,
            height: f.height.unwrap_or(0),
            fps: f.fps.map(|x| x.round() as u32).filter(|x| *x > 0),
            codec: codec_name(f.vcodec.as_deref()),
            range: range_name(f.range.as_deref()),
        };
        let same = |l: &Line| {
            l.height == line.height
                && l.fps == line.fps
                && l.codec == line.codec
                && l.range == line.range
                && (can_merge || l.format.ext == line.format.ext)
        };
        match lines.iter_mut().find(|l| same(l)) {
            Some(l) if better(f, l.format).is_gt() => *l = line,
            Some(_) => {}
            None => lines.push(line),
        }
    }
    // Best first, as yt-dlp sorts them: height, frame rate, HDR, codec, size.
    lines.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then(b.fps.cmp(&a.fps))
            .then(range_rank(b.format.range.as_deref()).cmp(&range_rank(a.format.range.as_deref())))
            .then(
                picture_rank(b.format.vcodec.as_deref())
                    .cmp(&picture_rank(a.format.vcodec.as_deref())),
            )
            .then(b.format.size.unwrap_or(0).cmp(&a.format.size.unwrap_or(0)))
            .then(a.format.id.cmp(&b.format.id))
    });
    // SDR said only where the same height has HDR too.
    let hdr: std::collections::HashSet<u32> = lines
        .iter()
        .filter(|l| l.range.is_some())
        .map(|l| l.height)
        .collect();
    lines
        .iter()
        .map(|l| {
            let f = l.format;
            let h = l.height;
            let range = l.range.or_else(|| hdr.contains(&h).then_some("SDR"));
            // With its sound: merged with the best one, unless it carries its own.
            let sound = audio.filter(|_| can_merge && !f.audio);
            let exact = match sound {
                Some(a) => {
                    (plain_id(&f.id) && plain_id(&a.id)).then(|| format!("{}+{}", f.id, a.id))
                }
                None => plain_id(&f.id).then(|| f.id.clone()),
            };
            // Should the page no longer offer that very format (servers left out, #94; a new
            // look-up's other IDs), the nearest of that height.
            let near = if can_merge {
                format!("bv*[height={h}]+ba/b[height={h}]")
            } else {
                format!("b[height={h}]")
            };
            let ext = if can_merge {
                "mp4".to_owned()
            } else {
                f.ext.clone()
            };
            let size = f.size.map(|s| s + sound.and_then(|a| a.size).unwrap_or(0));
            let approx =
                f.approx || exact.is_none() || sound.is_some_and(|a| a.approx || a.size.is_none());
            Choice {
                label: video_label(h, &ext, l.codec, l.fps, range),
                selector: exact.map_or_else(|| near.clone(), |e| format!("{e}/{near}")),
                ext,
                size,
                approx,
                height: Some(h),
                fps: l.fps,
                codec: l.codec.map(str::to_owned),
                range: range.map(str::to_owned),
                kbps: None,
            }
        })
        .collect()
}

/// The choices, best first: the video lines ([`videos`]), files known only by their type, then
/// the sound: as the site has it (each type it comes in), and converted to MP3, Opus or FLAC.
pub fn choices(v: &Video, can_merge: bool) -> Vec<Choice> {
    let sounds = sounds(v, can_merge);
    let best_audio = sounds.first().copied();
    let mut out = videos(v, can_merge, best_audio);
    // Files whose resolution isn't known (generic sites, a stream's variants with a bitrate
    // only): offered by type, and by bitrate when there are several, best first. Audio
    // containers, and streams with no video codec named, aren't videos.
    let mut plain: Vec<&Format> = v
        .formats
        .iter()
        .filter(|f| f.video && f.audio && f.height.is_none() && !audio_container(&f.ext))
        .collect();
    plain.sort_by(|a, b| b.tbr.unwrap_or(0.0).total_cmp(&a.tbr.unwrap_or(0.0)));
    let several = plain.len() > 1;
    let mut labels = std::collections::HashSet::new();
    for f in plain {
        let label = match f.tbr.filter(|_| several) {
            Some(kbps) => format!("{} {} kbps", f.ext.to_uppercase(), kbps.round()),
            None => f.ext.to_uppercase(),
        };
        // Twins (the same type and rate under two IDs) once.
        if !labels.insert(label.clone()) {
            continue;
        }
        out.push(Choice {
            label,
            selector: f.id.clone(),
            ext: f.ext.clone(),
            size: f.size,
            approx: f.approx,
            ..Default::default()
        });
    }
    // The sound as the site has it (#70), each type it comes in once (YouTube's Opus in WEBM,
    // AAC in M4A), the best first: that very format, else the best sound (`ba`).
    // Without ffmpeg an HLS sound only when no other comes; one of no known size only when
    // none has one (YouTube's HLS sounds beside its files).
    let files = !can_merge && sounds.iter().any(|a| !a.hls);
    let sized = sounds.iter().any(|a| a.size.is_some());
    let mut types = std::collections::HashSet::new();
    for a in sounds
        .iter()
        .filter(|a| !(files && a.hls) && !(sized && a.size.is_none()))
    {
        if !types.insert(a.ext.to_ascii_lowercase()) {
            continue;
        }
        out.push(Choice {
            // The app shows it in its language (videodlg::choice_label): the site's own file,
            // not converted (#70).
            label: format!("{AUDIO_ORIGINAL} {}", a.ext.to_uppercase()),
            selector: if plain_id(&a.id) {
                format!("{}/ba", a.id)
            } else {
                "ba".into()
            },
            ext: a.ext.clone(),
            size: a.size,
            approx: a.approx || !plain_id(&a.id),
            kbps: a
                .tbr
                .filter(|t| t.is_finite() && *t > 0.0)
                .map(|t| t.round() as u32),
            ..Default::default()
        });
    }
    // MP3, converted by ffmpeg from the best sound (#32): its best variable bitrate, then each
    // constant one. Sizes from the duration (a variable one's is about 245 kbit/s).
    let has_audio = v.formats.iter().any(|f| f.audio);
    if can_merge && has_audio {
        let size = |kbps: u32| v.duration.map(|s| (s * f64::from(kbps) * 125.0) as u64);
        out.push(Choice {
            label: format!("{AUDIO_ONLY} MP3 VBR"),
            selector: format!("{MP3}0"),
            ext: "mp3".into(),
            size: size(245),
            approx: true,
            ..Default::default()
        });
        for kbps in MP3_RATES {
            out.push(Choice {
                label: format!("{AUDIO_ONLY} MP3 {kbps} kbps"),
                selector: format!("{MP3}{kbps}"),
                ext: "mp3".into(),
                size: size(kbps),
                approx: true,
                kbps: Some(kbps),
                ..Default::default()
            });
        }
        // Opus (#70): about the size of the best sound when that is Opus already (it is only
        // moved into an .opus file), else of a 160 kbit/s encoding.
        let opus_size = best_audio
            .filter(|a| a.ext.eq_ignore_ascii_case("webm") || a.ext.eq_ignore_ascii_case("opus"))
            .and_then(|a| a.size)
            .or_else(|| size(160));
        out.push(Choice {
            label: format!("{AUDIO_ONLY} OPUS"),
            selector: OPUS.into(),
            ext: "opus".into(),
            size: opus_size,
            approx: true,
            ..Default::default()
        });
        // FLAC (#70): lossless from a lossy source, about 900 kbit/s for 48 kHz stereo.
        out.push(Choice {
            label: format!("{AUDIO_ONLY} FLAC"),
            selector: FLAC.into(),
            ext: "flac".into(),
            size: size(900),
            approx: true,
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_title_that_is_only_the_address() {
        let hash = "SFp1NUcrZFRkRUVrUWlqM3paMitkakJIUW9JSFhOeEZkd0ZMNEc5UHpnZm5hRkY4Vis5MjlJU00xdDVqTk1vaw";
        let page = format!("https://cdn.example/v/{hash}.mp4?sig=1");
        assert!(named_after_address(hash, &page));
        assert!(!named_after_address("A real title", &page));
        assert!(!named_after_address("watch", "not an address"));
        assert!(named_after_address(
            "my clip",
            "https://x.example/my%20clip.mp4"
        ));
    }

    use super::*;

    const PORTRAIT: &str = include_str!("../tests/fixtures/youtube-4k60-portrait.json");
    const HDR: &str = include_str!("../tests/fixtures/youtube-4k60-hdr.json");

    /// The page's own pick (`format_id` beside the formats: yt-dlp's `bv*+ba` with ffmpeg).
    fn ytdlp_pick(page: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(page).unwrap();
        v["format_id"].as_str().unwrap().to_owned()
    }

    /// #801: YouTube's portrait 4K60 video (yt-dlp 2026.08.19): its 3840 lines were one, sized
    /// from the HLS VP9 variant's peak bitrate (3.79 GB), while yt-dlp downloaded AV1 + Opus
    /// (532 MB). Each line is now the very formats it downloads, sized from their bytes.
    #[test]
    fn the_size_shown_is_the_size_that_comes() {
        let v = parse(PORTRAIT.as_bytes()).unwrap();
        let c = choices(&v, true);
        assert_eq!(c[0].label, "3840p · AV1 · 60 fps");
        assert!(
            c[0].selector
                .starts_with(&format!("{}/", ytdlp_pick(PORTRAIT)))
        );
        assert_eq!(c[0].selector, "401+251/bv*[height=3840]+ba/b[height=3840]");
        assert_eq!(c[0].size, Some(555_978_141 + 2_113_634));
        assert!(!c[0].approx);
        // The VP9 one of that height is its own line, from the file (315), not the HLS
        // variant's estimate (628).
        assert_eq!(c[1].label, "3840p · VP9 · 60 fps");
        assert_eq!(c[1].selector, "315+251/bv*[height=3840]+ba/b[height=3840]");
        assert_eq!(c[1].size, Some(1_144_096_984 + 2_113_634));
        assert!(c.iter().all(|c| !c.selector.starts_with("628")));
        // One line for each height, frame rate and codec; every one exact but a variant that
        // only comes as HLS (602, VP9 at 15 fps), said to be about.
        let videos: Vec<&Choice> = c.iter().filter(|c| c.height.is_some()).collect();
        assert_eq!(videos.len(), 23);
        let about: Vec<&str> = videos
            .iter()
            .filter(|c| c.approx)
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(about, ["256p · VP9 · 15 fps"]);
        let mut seen = std::collections::HashSet::new();
        assert!(
            videos.iter().all(|c| seen.insert(&c.label)),
            "no twin lines"
        );
        // The sound as the site has it: YouTube's Opus (what `ba` takes), then its AAC.
        let sound: Vec<(&str, &str, Option<u32>)> = c
            .iter()
            .filter(|c| original_sound(&c.selector))
            .map(|c| (c.label.as_str(), c.selector.as_str(), c.kbps))
            .collect();
        assert_eq!(
            sound,
            [
                ("Audio only original WEBM", "251/ba", Some(93)),
                ("Audio only original M4A", "140/ba", Some(130))
            ]
        );
        // Conversions are estimates.
        assert!(
            c.iter()
                .filter(|c| audio_conversion(&c.selector).is_some())
                .all(|c| c.approx)
        );
    }

    /// #755: a 4K60 HDR video: AV1 and VP9 in HDR, VP9 in SDR, each its own line, the one
    /// yt-dlp picks by itself first; SDR said where the height has HDR too. DRC sounds (YouTube's
    /// quieter copies) are never the one merged.
    #[test]
    fn codec_frame_rate_and_hdr_lines() {
        let v = parse(HDR.as_bytes()).unwrap();
        let c = choices(&v, true);
        let top: Vec<(&str, &str)> = c
            .iter()
            .take(3)
            .map(|c| (c.label.as_str(), c.selector.as_str()))
            .collect();
        assert_eq!(
            top,
            [
                (
                    "2160p · AV1 · 60 fps · HDR",
                    "701+251/bv*[height=2160]+ba/b[height=2160]"
                ),
                (
                    "2160p · VP9 · 60 fps · HDR",
                    "337+251/bv*[height=2160]+ba/b[height=2160]"
                ),
                (
                    "2160p · VP9 · 60 fps · SDR",
                    "315+251/bv*[height=2160]+ba/b[height=2160]"
                ),
            ]
        );
        assert!(c[0].selector.starts_with(&format!("{}/", ytdlp_pick(HDR))));
        assert_eq!(
            (c[0].codec.as_deref(), c[0].fps, c[0].range.as_deref()),
            (Some("AV1"), Some(60), Some("HDR"))
        );
        assert_eq!(c[2].range.as_deref(), Some("SDR"));
        assert_eq!(c[0].size, Some(303_941_999 + 1_612_962));
        assert!(c.iter().all(|c| !c.selector.contains("-drc")));
        let h264: Vec<&str> = c
            .iter()
            .filter(|c| c.codec.as_deref() == Some("H.264"))
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(h264[0], "1080p · H.264 · 60 fps · SDR");
        // Without ffmpeg YouTube has no format with both: only the sound.
        assert!(choices(&v, false).iter().all(|c| c.height.is_none()));
    }

    #[test]
    fn codecs_as_people_name_them() {
        let n = |c: &str| codec_name(Some(c));
        assert_eq!(n("av01.0.13M.10.0.110.09.16.09.0"), Some("AV1"));
        assert_eq!(n("vp09.02.51.10.01.09.16.09.00"), Some("VP9"));
        assert_eq!(n("vp9"), Some("VP9"));
        assert_eq!(n("avc1.64002A"), Some("H.264"));
        assert_eq!(n("hvc1.2.4.L153"), Some("H.265"));
        assert_eq!(n("dvh1.08.06"), Some("H.265"));
        assert_eq!(n("vp8"), Some("VP8"));
        assert_eq!(n("none"), None);
        assert_eq!(n("something"), None);
        assert_eq!(codec_name(None), None);
        assert_eq!(range_name(Some("HDR10")), Some("HDR"));
        assert_eq!(range_name(Some("HLG")), Some("HDR"));
        assert_eq!(range_name(Some("DV")), Some("Dolby Vision"));
        assert_eq!(range_name(Some("SDR")), None);
    }

    /// Only an ID yt-dlp's `-f` reads as that ID is named; else the line keeps the selector by
    /// height, and its size is about.
    #[test]
    fn ids_named_only_when_plain() {
        for id in ["401", "251-drc", "hls-1080p", "dash_v.1"] {
            assert!(plain_id(id), "{id}");
        }
        for id in [
            "",
            "mp4",
            "best",
            "b",
            "a+b",
            "a/b",
            "x[height=1]",
            "v,a",
            "a b",
        ] {
            assert!(!plain_id(id), "{id}");
        }
        let page = r#"{"title": "T", "formats": [
            {"format_id": "a/b", "ext": "mp4", "height": 720, "vcodec": "avc1", "acodec": "none", "filesize": 10},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 1}]}"#;
        let c = choices(&parse(page.as_bytes()).unwrap(), true);
        assert_eq!(c[0].selector, "bv*[height=720]+ba/b[height=720]");
        assert!(c[0].approx);
    }

    /// The size is about when the site only guesses it (`filesize_approx`), or it comes from
    /// the bitrate; exact from `filesize`, or the server's word for a plain file.
    #[test]
    fn about_sizes_are_marked() {
        let page = r#"{"title": "T", "duration": 10, "formats": [
            {"format_id": "1", "ext": "mp4", "height": 720, "vcodec": "avc1", "acodec": "none", "filesize_approx": 1000},
            {"format_id": "2", "ext": "mp4", "height": 480, "vcodec": "avc1", "acodec": "none", "filesize": 500},
            {"format_id": "3", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "none", "tbr": 100},
            {"format_id": "4", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 50}]}"#;
        let v = parse(page.as_bytes()).unwrap();
        let c = choices(&v, true);
        let sized: Vec<(Option<u64>, bool)> =
            c.iter().take(3).map(|c| (c.size, c.approx)).collect();
        assert_eq!(
            sized,
            [
                (Some(1050), true),
                (Some(550), false),
                (Some(125_050), true)
            ]
        );
        let mut direct = parse(
            br#"{"title": "f", "direct": true, "formats": [{"format_id": "mp4", "ext": "mp4"}]}"#,
        )
        .unwrap();
        direct.set_size(9);
        assert!(!direct.formats[0].approx);
    }

    /// Wikimedia Commons' transcodes say a negative size (#769's Commons check): read as none,
    /// not as a page with no video.
    #[test]
    fn a_negative_size_is_no_size() {
        let page = br#"{"title": "Clip", "duration": 10.0, "formats": [
            {"format_id": "2", "ext": "webm", "height": 480, "vcodec": "vp9", "acodec": "opus",
             "filesize_approx": -79319, "tbr": 1218},
            {"format_id": "source", "ext": "webm", "filesize": 2964839055}]}"#;
        let v = parse(page).expect("a video");
        assert_eq!(v.formats.len(), 2);
        assert_ne!(v.formats[0].size, Some(0));
        assert_eq!(v.formats[1].size, Some(2964839055));
    }

    #[test]
    fn a_streams_variants_by_bitrate() {
        let fmt = |id: &str, tbr: f64| Format {
            id: id.into(),
            ext: "mp4".into(),
            height: None,
            fps: None,
            video: true,
            audio: true,
            size: None,
            tbr: Some(tbr),
            ..Default::default()
        };
        let v = Video {
            title: "t".into(),
            duration: None,
            formats: vec![
                fmt("hls-640", 640.0),
                fmt("hls-1280", 1280.0),
                fmt("hls-1280b", 1280.0),
            ],
            subtitles: Vec::new(),
            automatic: Vec::new(),
            language: None,
            direct: false,
            needs_js_runtime: false,
        };
        let labels: Vec<String> = choices(&v, true)
            .into_iter()
            .map(|c| c.label)
            .filter(|l| !l.starts_with("Audio only"))
            .collect();
        assert_eq!(
            labels,
            ["MP4 1280 kbps", "MP4 640 kbps"],
            "best first, twins once"
        );
    }

    const SAMPLE: &str = r#"{
        "title": "A talk", "duration": 61.5,
        "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 1000000},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 5000000},
            {"format_id": "136", "ext": "mp4", "height": 720, "fps": 30, "vcodec": "avc1", "acodec": "none", "filesize": 20000000},
            {"format_id": "299", "ext": "mp4", "height": 1080, "fps": 60, "vcodec": "avc1", "acodec": "none", "filesize_approx": 60000000}
        ]}"#;

    /// An HLS master playlist as yt-dlp reads it (checked with yt-dlp 2026.10.26): each variant's
    /// bitrate and the duration, no size; sized as bitrate × duration. A live one stays unsized.
    #[test]
    fn hls_variants_are_sized_from_their_bitrate() {
        const HLS: &str = r#"{"title": "master", "duration": 600, "formats": [
            {"format_id": "800", "ext": "mp4", "height": 360, "tbr": 800.0, "protocol": "m3u8_native"},
            {"format_id": "2500", "ext": "mp4", "height": 720, "tbr": 2500.0, "protocol": "m3u8_native"}]}"#;
        let v = parse(HLS.as_bytes()).unwrap();
        let sizes: Vec<_> = choices(&v, true)
            .into_iter()
            .filter(|c| !c.label.starts_with("Audio only"))
            .map(|c| (c.label, c.size))
            .collect();
        assert_eq!(
            sizes,
            [
                ("720p · MP4".to_owned(), Some(187_500_000)),
                ("360p · MP4".to_owned(), Some(60_000_000))
            ]
        );
        let live = HLS.replace(r#""duration": 600, "#, "");
        let v = parse(live.as_bytes()).unwrap();
        assert!(v.formats.iter().all(|f| f.size.is_none()), "live: no size");
        // A size yt-dlp gives wins over the estimate.
        let given = HLS.replace(r#""tbr": 800.0"#, r#""tbr": 800.0, "filesize": 123"#);
        let v = parse(given.as_bytes()).unwrap();
        assert_eq!(v.formats[0].size, Some(123));
        // A plain file's size is its server's to say (#320), never estimated.
        let direct = HLS.replace(
            r#""duration": 600, "#,
            r#""duration": 600, "direct": true, "#,
        );
        assert!(parse(direct.as_bytes()).unwrap().needs_size());
        // Wikimedia Commons' extractor says `"direct": null`: a page like any other, not "no
        // video information".
        let commons = HLS.replacen(
            r#""duration": 600, "#,
            r#""duration": 600, "direct": null, "#,
            1,
        );
        let v = parse(commons.as_bytes()).expect("a direct of null");
        assert!(!v.direct);
        // …and an estimate below zero for a size: none.
        let odd = HLS.replacen(
            r#""tbr": 800.0"#,
            r#""tbr": 800.0, "filesize_approx": -79319"#,
            1,
        );
        let v = parse(odd.as_bytes()).expect("a size below zero");
        assert!(v.formats.iter().all(|f| f.size != Some(0)));
    }

    #[test]
    fn choices_with_and_without_merging() {
        let v = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(v.title, "A talk");
        let merged = choices(&v, true);
        let labels: Vec<&str> = merged.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "1080p · H.264 · 60 fps",
                "720p · H.264 · 30 fps",
                "360p · H.264",
                "Audio only original M4A",
                "Audio only MP3 VBR",
                "Audio only MP3 320 kbps",
                "Audio only MP3 256 kbps",
                "Audio only MP3 192 kbps",
                "Audio only MP3 128 kbps",
                "Audio only OPUS",
                "Audio only FLAC",
            ]
        );
        // Opus from M4A is encoded (160 kbit/s); FLAC about 900 kbit/s (61.5 s).
        let opus = &merged[merged.len() - 2];
        assert_eq!((opus.selector.as_str(), opus.size), (OPUS, Some(1_230_000)));
        let flac = &merged[merged.len() - 1];
        assert_eq!((flac.selector.as_str(), flac.size), (FLAC, Some(6_918_750)));
        assert_eq!(merged[0].size, Some(61_000_000), "video + best audio");
        // MP3 (#32): converted from the best sound, sized from the duration (61.5 s).
        let mp3 = &merged[5];
        assert_eq!(
            (mp3.selector.as_str(), mp3.ext.as_str()),
            ("mp3:320", "mp3")
        );
        assert_eq!(mp3.size, Some(2_460_000));
        // Without ffmpeg only the format that has both survives, and no MP3 (it converts).
        let plain = choices(&v, false);
        let labels: Vec<&str> = plain.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["360p · H.264", "Audio only original M4A"]);
        assert_eq!(plain[0].selector, "18/b[height=360]");
    }

    /// Live (#352): `MDM_LIVE_YTDLP=<yt-dlp> MDM_LIVE_PAGE=<video page> cargo test -p mdm-tools
    /// live_look_up -- --ignored --nocapture`. A look-up given cookies; with a yt-dlp that fails
    /// the way a signed-in session does (a wrapper forcing `web_embedded,tv_samsung,web` when it
    /// sees `--cookies`), the second look-up, without them, lists the page's videos.
    #[test]
    #[ignore]
    fn live_look_up_with_cookies() {
        let exe =
            std::path::PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let page = std::env::var("MDM_LIVE_PAGE").expect("MDM_LIVE_PAGE");
        let browser = proc::FromBrowser {
            cookies: Some("YSC=test".into()),
            ..Default::default()
        };
        let (mut args, _file) = browser.args(&page).unwrap();
        args.insert(0, "-J".to_owned());
        args.push("--no-playlist".to_owned());
        let out = proc::look_up(&exe, &args, &page, Duration::from_secs(120)).unwrap();
        let v = parse(&out.json).unwrap();
        let heights: std::collections::BTreeSet<u32> = v
            .formats
            .iter()
            .filter(|f| f.video)
            .filter_map(|f| f.height)
            .collect();
        println!(
            "without_cookies: {}, {} formats, video heights {heights:?}",
            out.without_cookies,
            v.formats.len()
        );
        assert!(!heights.is_empty(), "no video in {:?}", v.title);
    }

    /// #695: a plain file a server's program sends (`stream.php`), as yt-dlp's generic reader
    /// answers for it: the format id is the type's subtype, the extension the address's. It gets
    /// its file's type: the `file=` value's, else the type's; a real one stays.
    #[test]
    fn a_scripts_file_gets_its_real_type() {
        let answer = |url: &str, id: &str, vcodec: &str| {
            format!(
                r#"{{"title": "stream", "direct": true, "ext": "php", "url": "{url}",
                "format_id": "{id}", "vcodec": {vcodec}, "formats": [{{"format_id": "{id}",
                "ext": "php", "url": "{url}", "vcodec": {vcodec}, "protocol": "https"}}]}}"#
            )
        };
        let ext = |json: &[u8]| {
            let v: serde_json::Value = serde_json::from_slice(json).unwrap();
            (
                v["ext"].as_str().unwrap().to_owned(),
                v["formats"][0]["ext"].as_str().unwrap().to_owned(),
            )
        };
        let fixed = typed_scripts(
            answer(
                "https://u.test/stream.php?file=FL262-1E01-4.mp4&t=1",
                "mp4",
                "null",
            )
            .into_bytes(),
        );
        assert_eq!(ext(&fixed), ("mp4".into(), "mp4".into()));
        assert_eq!(parse(&fixed).unwrap().formats[0].ext, "mp4");
        // No file name in the address: the server's type, read as a sound's when it has no
        // picture (audio/mpeg is an MP3, video/mpeg an MPG).
        let fixed = typed_scripts(
            answer("https://u.test/remote_control.php?id=3", "webm", "null").into_bytes(),
        );
        assert_eq!(ext(&fixed), ("webm".into(), "webm".into()));
        let fixed = typed_scripts(answer("https://u.test/a.php", "mpeg", r#""none""#).into_bytes());
        assert_eq!(ext(&fixed).1, "mp3");
        // yt-dlp 2026.10 names it `f` and the subtype (its answer for the lab's /stream.php).
        let fixed =
            typed_scripts(answer("https://u.test/stream.php?id=1", "fmp4", "null").into_bytes());
        assert_eq!(ext(&fixed), ("mp4".into(), "mp4".into()));
        let fixed = typed_scripts(answer("https://u.test/a.php", "flac", r#""none""#).into_bytes());
        assert_eq!(ext(&fixed).1, "flac");
        // Nothing says what it is: as it came. A real extension is never touched.
        let unknown = answer("https://u.test/a.php", "x-unknown", "null").into_bytes();
        assert_eq!(typed_scripts(unknown.clone()), unknown);
        let real = br#"{"title": "t", "ext": "mp4", "formats": [{"format_id": "18", "ext": "mp4", "url": "https://u.test/v.php"}]}"#.to_vec();
        assert_eq!(typed_scripts(real.clone()), real);
        assert_eq!(typed_scripts(b"not json".to_vec()), b"not json".to_vec());
        // A program's type is never one: application/x-msdownload stays as it came.
        let program = answer(
            "https://u.test/a.php?file=setup.exe",
            "x-msdownload",
            "null",
        )
        .into_bytes();
        assert_eq!(typed_scripts(program.clone()), program);
    }

    /// #320: yt-dlp's answer for a plain file (real output, 2026.10.26) says `direct` and no size.
    #[test]
    fn a_direct_file_takes_the_size_the_server_said() {
        let json = r#"{"title": "clip", "direct": true, "ext": "mp4", "formats": [
            {"format_id": "mp4", "ext": "mp4", "vcodec": null, "acodec": null}]}"#;
        let mut v = parse(json.as_bytes()).unwrap();
        assert!(v.needs_size());
        assert_eq!(choices(&v, true)[0].size, None);
        v.set_size(430_019_588);
        assert!(!v.needs_size());
        assert_eq!(choices(&v, true)[0].size, Some(430_019_588));
        // A page's video: no such look (its sizes are the site's to say, or estimated).
        let page = r#"{"title": "t", "formats": [{"format_id": "18", "ext": "mp4"}]}"#;
        assert!(!parse(page.as_bytes()).unwrap().needs_size());
        // A file whose size yt-dlp said keeps it.
        let sized =
            r#"{"title": "t", "direct": true, "formats": [{"format_id": "mp4", "filesize": 9}]}"#;
        assert!(!parse(sized.as_bytes()).unwrap().needs_size());
    }

    /// #351: what yt-dlp lists on YouTube with no JavaScript runtime (real output, 2026.10.26):
    /// video-only streams and sound, nothing carrying both. Without ffmpeg only the sound is
    /// offered, and the window has to say that ffmpeg brings the qualities.
    #[test]
    fn only_the_sound_without_ffmpeg_and_a_muxed_format() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none"},
            {"format_id": "140-0", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "filesize": 900},
            {"format_id": "312", "ext": "mp4", "height": 1080, "vcodec": "avc1.64002A", "acodec": "none", "filesize": 9000},
            {"format_id": "230", "ext": "mp4", "height": 360, "vcodec": "avc1.4D401E", "acodec": "none", "filesize": 1000}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let without: Vec<String> = choices(&v, false).into_iter().map(|c| c.label).collect();
        assert_eq!(without, ["Audio only original M4A"]);
        assert!(needs_merging(&v));
        let with: Vec<String> = choices(&v, true).into_iter().map(|c| c.label).collect();
        assert_eq!(&with[..2], ["1080p · H.264", "360p · H.264"]);
        // A format carrying both, as good as any video: nothing more to get from merging.
        let muxed = r#"{"title": "T", "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"},
            {"format_id": "230", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "none"}]}"#;
        assert!(!needs_merging(&parse(muxed.as_bytes()).unwrap()));
        // Sound alone: nothing to merge.
        let sound = r#"{"title": "T", "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a"}]}"#;
        assert!(!needs_merging(&parse(sound.as_bytes()).unwrap()));
    }

    /// #569: YouTube on a Mac with neither ffmpeg nor a JavaScript runtime: its sound as HLS
    /// streams (the larger) and as a file. Without ffmpeg the file is the choice, and yt-dlp is
    /// asked for a file first; with ffmpeg the best, HLS or not (ffmpeg makes it a proper file).
    #[test]
    fn without_ffmpeg_a_sound_that_comes_as_a_file() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "233", "ext": "mp4", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "m3u8_native", "filesize_approx": 2000},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "https", "filesize": 900}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        assert!(v.formats[0].hls && !v.formats[1].hls);
        let without = choices(&v, false);
        assert_eq!(without[0].label, "Audio only original M4A");
        assert_eq!(without[0].size, Some(900));
        assert_eq!(choices(&v, true)[0].size, Some(2000));
        assert_eq!(without_remuxing("ba"), "ba[protocol!*=m3u8]/ba");
        assert_eq!(
            without_remuxing("b[height=360]"),
            "b[height=360][protocol!*=m3u8]/b[height=360]"
        );
        assert_eq!(
            without_remuxing("bv*[height=720]+ba/b[height=720]"),
            "bv*[height=720]+ba/b[height=720][protocol!*=m3u8]/b[height=720]"
        );
        // A format by its ID, and one already choosing its way, as they are.
        assert_eq!(without_remuxing("hls-1280"), "hls-1280");
        assert_eq!(without_remuxing("18"), "18");
        assert_eq!(without_remuxing("ba[protocol=https]"), "ba[protocol=https]");
        // Only HLS sound: still offered (it is saved for what it is, video.rs).
        let hls_only = r#"{"title": "T", "formats": [
            {"format_id": "233", "ext": "mp4", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "m3u8_native"}]}"#;
        let only = choices(&parse(hls_only.as_bytes()).unwrap(), false);
        assert_eq!(only[0].selector, "233/ba");
    }

    /// #70: what each audio choice converts to; the original and videos convert nothing.
    #[test]
    fn audio_conversions() {
        let c = |s: &str| audio_conversion(s).map(|a| (a.format, a.quality));
        assert_eq!(c("mp3:320"), Some(("mp3", Some("320K".into()))));
        assert_eq!(c(OPUS), Some(("opus", None)));
        assert_eq!(c(FLAC), Some(("flac", None)));
        assert_eq!(c("ba"), None);
        assert_eq!(c("bv*[height=720]+ba/b[height=720]"), None);
        assert_eq!(c("opus:x"), None);
        let flac = Choice {
            label: "x".into(),
            selector: FLAC.into(),
            ext: "flac".into(),
            ..Default::default()
        };
        assert!(is_audio(&flac));
    }

    #[test]
    fn mp3_selectors_are_the_offered_ones_only() {
        assert_eq!(mp3_quality("mp3:0").as_deref(), Some("0"));
        assert_eq!(mp3_quality("mp3:192").as_deref(), Some("192K"));
        for other in [
            "mp3:64",
            "mp3:",
            "mp3:-1",
            "mp3:320; rm",
            "mp3:+192",
            "mp3:0320",
            "mp3:000",
            "mp3: 192",
            "MP3:192",
            "ba",
            "b[height=360]",
        ] {
            assert_eq!(mp3_quality(other), None, "{other}");
        }
    }

    #[test]
    fn plain_files_without_codec_or_resolution() {
        let json = r#"{"title": "Bunny", "formats": [
            {"format_id": "ogv", "ext": "ogv", "filesize": 100},
            {"format_id": "mp4", "ext": "mp4", "filesize": 200}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let c = choices(&v, false);
        let labels: Vec<&str> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["OGV", "MP4"]);
        assert_eq!(c[1].selector, "mp4");
    }

    #[test]
    fn audio_without_codec_info_is_not_a_video_choice() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "773", "ext": "m4a", "filesize": 100},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 90},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 500}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let labels: Vec<String> = choices(&v, false).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["360p · H.264", "Audio only original M4A"]);
    }

    /// A page's subtitles (#68), as yt-dlp -J lists them: the uploader's by name, the chat
    /// replay left out; of the automatic ones only the spoken language's and the translations
    /// into the interface language and English, and none the uploader already has.
    #[test]
    fn subtitle_languages_worth_offering() {
        let json = r#"{"title": "T", "language": "en", "formats": [],
            "subtitles": {
                "live_chat": [{"ext": "json"}],
                "fr": [{"ext": "vtt", "name": "French"}, {"ext": "srt", "name": "French"}],
                "de": [{"ext": "vtt"}],
                "empty": []
            },
            "automatic_captions": {
                "en-orig": [{"ext": "vtt", "name": "English (Original)"}],
                "en": [{"ext": "vtt", "name": "English"}],
                "fr": [{"ext": "vtt", "name": "French"}],
                "ar": [{"ext": "vtt", "name": "Arabic"}],
                "ja": [{"ext": "vtt", "name": "Japanese"}],
                "zu": [{"ext": "vtt", "name": "Zulu"}]
            }}"#;
        let v = parse(json.as_bytes()).unwrap();
        let names: Vec<&str> = v.subtitles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["de", "French"], "by name; no name: the code");
        assert_eq!(v.language.as_deref(), Some("en"));
        let offered: Vec<(String, bool)> = sub_choices(&v, "ar")
            .into_iter()
            .map(|c| (c.code, c.automatic))
            .collect();
        let want: Vec<(String, bool)> = [
            ("de", false),
            ("fr", false),
            ("ar", true),
            ("en", true),
            ("en-orig", true),
        ]
        .iter()
        .map(|(c, a)| (c.to_string(), *a))
        .collect();
        assert_eq!(offered, want);
        // A page without any: nothing to offer.
        let none = parse(br#"{"title": "T", "formats": []}"#).unwrap();
        assert!(sub_choices(&none, "en").is_empty());
    }

    #[test]
    fn junk_is_not_a_video() {
        assert!(parse(b"not json").is_none());
    }
}
