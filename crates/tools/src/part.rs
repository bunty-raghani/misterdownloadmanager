//! A part of a video or of a sound (#1662): from one time to another, cut quickly at the
//! keyframes (the streams copied, as they are) or exactly (the part encoded again).
//!
//! yt-dlp downloads it (`--download-sections "*<from>-<to>"`): its ffmpeg seeks in each stream by
//! the stream's own index (an MP4's, a WebM's cues, an HLS playlist) and reads only what the part
//! needs, whatever the container. A quick cut copies the streams from the keyframe at or before
//! From (it may start a little early, never late); an exact one (`--force-keyframes-at-cuts`) has
//! ffmpeg encode the part again, with the encoders [`exact_args`] picks for the file it makes.
//! Where MDM's engine runs, what that ffmpeg reads comes through MDM's connections
//! ([`crate::partserve`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The part of a video or a sound to download: from `from` to `to`, whole seconds into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub from: u32,
    pub to: u32,
    /// Cut exactly (the part encoded again) rather than at the keyframes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
}

/// Why a part can't be downloaded as typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A time that isn't one (`1:75`, `abc`, empty).
    NotATime,
    /// To isn't after From.
    Backwards,
    /// A time past the video's end (its length in whole seconds, rounded up).
    PastEnd(u32),
    /// From where it starts to where it ends: the whole video, not a part of it.
    Whole,
}

impl Part {
    /// The part as typed (`from`, `to`: [`parse_time`]'s forms), checked against the video's
    /// length when it is known (`duration`, seconds).
    pub fn checked(
        from: &str,
        to: &str,
        exact: bool,
        duration: Option<f64>,
    ) -> Result<Part, Problem> {
        let (Some(from), Some(to)) = (parse_time(from), parse_time(to)) else {
            return Err(Problem::NotATime);
        };
        if to <= from {
            return Err(Problem::Backwards);
        }
        if let Some(end) = length(duration) {
            if to > end {
                return Err(Problem::PastEnd(end));
            }
            if from == 0 && to == end {
                return Err(Problem::Whole);
            }
        }
        Ok(Part { from, to, exact })
    }

    /// How long the part is, in seconds.
    pub fn seconds(&self) -> u32 {
        self.to.saturating_sub(self.from)
    }

    /// yt-dlp's `--download-sections` for it: `*90-250`.
    pub fn sections(&self) -> String {
        format!("*{}-{}", self.from, self.to)
    }

    /// What the file's name says of it: `1m30s-4m10s`.
    pub fn tag(&self) -> String {
        format!("{}-{}", tag(self.from), tag(self.to))
    }

    /// Its share of a video `duration` seconds long (none when that isn't known).
    pub fn share(&self, duration: Option<f64>) -> Option<f64> {
        let d = duration.filter(|d| d.is_finite() && *d > 0.0)?;
        Some((f64::from(self.seconds()) / d).clamp(0.0, 1.0))
    }

    /// About how big it is, of a whole `size` bytes `duration` seconds long: its share of it.
    pub fn estimate(&self, size: Option<u64>, duration: Option<f64>) -> Option<u64> {
        let share = self.share(duration)?;
        size.map(|s| ((s as f64 * share).round() as u64).max(1))
    }
}

/// A video's length as a part's end is checked against it: whole seconds, rounded up (a video
/// of 888.06 s ends at 14:49), none when unknown.
pub fn length(duration: Option<f64>) -> Option<u32> {
    duration
        .filter(|d| d.is_finite() && *d >= 1.0 && *d < f64::from(u32::MAX))
        .map(|d| d.ceil() as u32)
}

/// A time as typed: `h:mm:ss`, `m:ss` or plain seconds (`1:02:03`, `1:30`, `90`), spaces around
/// it taken off. Minutes and seconds after a colon are under 60; the first number has no limit
/// of its own (`90:00` is an hour and a half).
pub fn parse_time(s: &str) -> Option<u32> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(':').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|p| p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let nums: Vec<u64> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if nums.iter().skip(1).any(|n| *n >= 60) {
        return None;
    }
    let secs = nums.iter().fold(0u64, |acc, n| acc * 60 + n);
    u32::try_from(secs).ok()
}

/// A time as the window shows it: `1:30`, or `0:01:30` beside a video of an hour or more
/// (`hours`), so both boxes read alike.
pub fn clock(secs: u32, hours: bool) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 || hours {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A time as a file name says it: `45s`, `4m10s`, `1h02m03s`.
pub fn tag(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// `name` (a file name with its extension, `Talk.mp4`) for the part: `Talk (1m30s-4m10s).mp4`.
/// A name already ending in the part's words isn't named again.
pub fn named(name: &str, part: &Part) -> String {
    let words = format!("({})", part.tag());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains(' ') && ext.len() <= 5 => {
            (stem, Some(ext))
        }
        _ => (name, None),
    };
    let stem = stem.trim_end();
    if stem.ends_with(&words) {
        return name.to_owned();
    }
    match ext {
        Some(ext) => format!("{stem} {words}.{ext}"),
        None => format!("{stem} {words}"),
    }
}

/// `name` without a part's words at its end (` (1m30s-4m10s)` before the extension), as
/// [`named`] put them there: the name the whole video has.
pub fn unnamed(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains(' ') && ext.len() <= 5 => {
            (stem, Some(ext))
        }
        _ => (name, None),
    };
    let bare = stem
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(_, words)| is_tag_pair(words))
        .map_or(stem, |(s, _)| s);
    match ext {
        Some(ext) => format!("{bare}.{ext}"),
        None => bare.to_owned(),
    }
}

/// `1m30s-4m10s` as [`Part::tag`] writes it.
fn is_tag_pair(words: &str) -> bool {
    let one = |t: &str| {
        let t = t.strip_suffix('s').unwrap_or("");
        !t.is_empty()
            && t.chars()
                .all(|c| c.is_ascii_digit() || c == 'h' || c == 'm')
            && t.starts_with(|c: char| c.is_ascii_digit())
    };
    words.split_once('-').is_some_and(|(a, b)| one(a) && one(b))
}

/// The encoders an ffmpeg has (`ffmpeg -encoders`), asked once for each program.
pub fn encoders(ffmpeg: &Path) -> Vec<String> {
    static SEEN: Mutex<Option<HashMap<PathBuf, Vec<String>>>> = Mutex::new(None);
    if let Some(known) = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(ffmpeg))
    {
        return known.clone();
    }
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-encoders"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up from the window's app.
        cmd.creation_flags(0x0800_0000);
    }
    let found = crate::proc::output(cmd, "ffmpeg", Duration::from_secs(20))
        .map(|out| encoder_names(&String::from_utf8_lossy(&out)))
        .unwrap_or_default();
    if !found.is_empty() {
        SEEN.lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(ffmpeg.to_owned(), found.clone());
    }
    found
}

/// The names in `ffmpeg -encoders`' list: ` V....D libx264  libx264 H.264 …`, after its legend.
fn encoder_names(listing: &str) -> Vec<String> {
    listing
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("------"))
        .skip(1)
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let flags = words.next()?;
            let name = words.next()?;
            (flags.len() == 6 && flags.starts_with(['V', 'A', 'S'])).then(|| name.to_owned())
        })
        .collect()
}

/// What the part becomes, for its exact cut's encoders: the file yt-dlp makes (`ext`), and the
/// bitrates of what it comes from (kbit/s, as yt-dlp says them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
    pub ext: String,
    pub video: bool,
    pub audio: bool,
    pub video_kbps: Option<f64>,
    pub audio_kbps: Option<f64>,
}

impl Made {
    /// yt-dlp's choice (`-O "%()j"`): the file's type, and each chosen format's picture and sound.
    pub fn of(chosen: &serde_json::Value) -> Made {
        fn text<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
            v.get(k).and_then(|x| x.as_str())
        }
        let num = |v: &serde_json::Value, k: &str| {
            v.get(k)
                .and_then(serde_json::Value::as_f64)
                .filter(|n| *n > 0.0)
        };
        let formats: Vec<&serde_json::Value> = chosen
            .get("requested_formats")
            .and_then(|f| f.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_else(|| vec![chosen]);
        let has = |v: &serde_json::Value, k: &str| text(v, k).is_some_and(|c| c != "none");
        let mut made = Made {
            ext: text(chosen, "ext").unwrap_or("").to_ascii_lowercase(),
            ..Default::default()
        };
        for f in formats {
            let (v, a) = (has(f, "vcodec"), has(f, "acodec"));
            // A format that says neither (a site's plain file): a picture with its sound, unless
            // its type is a sound's.
            let (v, a) = if text(f, "vcodec").is_none() && text(f, "acodec").is_none() {
                (!audio_ext(&made.ext), true)
            } else {
                (v, a)
            };
            if v {
                made.video = true;
                made.video_kbps =
                    made.video_kbps
                        .or(num(f, "vbr"))
                        .or(if a { None } else { num(f, "tbr") });
            }
            if a {
                made.audio = true;
                made.audio_kbps =
                    made.audio_kbps
                        .or(num(f, "abr"))
                        .or(if v { None } else { num(f, "tbr") });
            }
        }
        made
    }
}

/// A sound's file type.
fn audio_ext(ext: &str) -> bool {
    matches!(
        ext,
        "m4a" | "mp3" | "opus" | "ogg" | "oga" | "flac" | "wav" | "aac" | "weba" | "mka"
    )
}

/// The exact cut's encoders and their settings for ffmpeg's output (yt-dlp's
/// `--downloader-args "ffmpeg_o:…"`), for the file `made` with an ffmpeg that has `encoders`.
/// Without them ffmpeg takes its own defaults, poor in an LGPL build (MPEG-4 Part 2 at 200 kbit/s
/// for an MP4, as MDM's own Windows build has no x264). The picture: x264 when there is one
/// (CRF 18, fast), else OpenH264 (MDM's build) at twice the source's bitrate, else ffmpeg's MPEG-4
/// at its best quality; VP9 for a WebM; 4:2:0, as players expect. The sound: AAC in an MP4,
/// Opus in a WebM or Ogg, MP3 in an MP3, as good as the source's (192 kbit/s at least).
pub fn exact_args(made: &Made, encoders: &[String]) -> Vec<String> {
    let has = |e: &str| encoders.iter().any(|x| x == e);
    let kbps = |k: Option<f64>, least: f64, most: f64| k.unwrap_or(least).clamp(least, most);
    let audio_rate = format!(
        "{}k",
        kbps(made.audio_kbps.map(|k| k * 1.25), 192.0, 320.0) as u32
    );
    let mut out: Vec<String> = Vec::new();
    let push =
        |out: &mut Vec<String>, args: &[&str]| out.extend(args.iter().map(|a| a.to_string()));
    let webm = matches!(made.ext.as_str(), "webm" | "weba");
    let ogg = matches!(made.ext.as_str(), "ogg" | "oga" | "opus");
    if made.video {
        if webm && has("libvpx-vp9") {
            push(
                &mut out,
                &[
                    "-c:v",
                    "libvpx-vp9",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                    "-row-mt",
                    "1",
                ],
            );
            push(&mut out, &["-crf", "28", "-b:v", "0"]);
        } else if has("libx264") {
            push(
                &mut out,
                &[
                    "-c:v", "libx264", "-preset", "fast", "-crf", "18", "-pix_fmt", "yuv420p",
                ],
            );
        } else if has("libopenh264") {
            let rate = kbps(made.video_kbps.map(|k| k * 2.0), 1000.0, 60000.0) as u32;
            push(&mut out, &["-c:v", "libopenh264", "-pix_fmt", "yuv420p"]);
            push(
                &mut out,
                &[
                    "-b:v",
                    &format!("{rate}k"),
                    "-maxrate",
                    &format!("{}k", rate * 2),
                ],
            );
            push(&mut out, &["-bufsize", &format!("{}k", rate * 2)]);
        } else if has("mpeg4") {
            push(
                &mut out,
                &["-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p"],
            );
        }
    }
    if made.audio {
        match made.ext.as_str() {
            "mp3" if has("libmp3lame") => push(&mut out, &["-c:a", "libmp3lame", "-q:a", "2"]),
            "flac" | "wav" => {}
            _ if (webm || ogg) && has("libopus") => {
                push(&mut out, &["-c:a", "libopus", "-b:a", &audio_rate]);
            }
            _ if !webm && !ogg && has("aac") => {
                push(&mut out, &["-c:a", "aac", "-b:a", &audio_rate])
            }
            _ => {}
        }
    }
    out
}

/// `args` as one of yt-dlp's `--downloader-args` (`ffmpeg_o:<args>`), each quoted as yt-dlp's
/// shlex reads it.
pub fn downloader_args(key: &str, args: &[String]) -> String {
    let quoted: Vec<String> = args
        .iter()
        .map(|a| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_:.+/".contains(c))
            {
                a.clone()
            } else {
                format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect();
    format!("{key}:{}", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_as_people_type_them() {
        assert_eq!(parse_time("90"), Some(90));
        assert_eq!(parse_time("1:30"), Some(90));
        assert_eq!(parse_time(" 01:30 "), Some(90));
        assert_eq!(parse_time("1:02:03"), Some(3723));
        assert_eq!(parse_time("0:00"), Some(0));
        assert_eq!(parse_time("90:00"), Some(5400));
        for bad in [
            "", "1:75", "1:2:60", "a", "1:", ":30", "1::30", "-5", "1.5", "1:2:3:4",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
        assert_eq!(parse_time("99999999999"), None);
    }

    #[test]
    fn times_are_shown_and_named_alike_in_both_boxes() {
        assert_eq!(clock(90, false), "1:30");
        assert_eq!(clock(5, false), "0:05");
        assert_eq!(clock(90, true), "0:01:30");
        assert_eq!(clock(3723, false), "1:02:03");
        assert_eq!(tag(45), "45s");
        assert_eq!(tag(250), "4m10s");
        assert_eq!(tag(3723), "1h02m03s");
        for s in [0, 5, 59, 60, 61, 3599, 3600, 3723, 86399] {
            assert_eq!(parse_time(&clock(s, false)), Some(s));
            assert_eq!(parse_time(&clock(s, true)), Some(s));
        }
    }

    #[test]
    fn a_part_is_checked_against_the_videos_length() {
        let d = Some(888.06);
        let p = Part::checked("1:30", "4:10", false, d).unwrap();
        assert_eq!((p.from, p.to, p.seconds()), (90, 250, 160));
        assert_eq!(p.sections(), "*90-250");
        assert_eq!(p.tag(), "1m30s-4m10s");
        // The end rounds up: 888.06 s ends at 14:49.
        assert!(Part::checked("14:00", "14:49", true, d).is_ok());
        assert_eq!(
            Part::checked("14:00", "14:50", false, d),
            Err(Problem::PastEnd(889))
        );
        assert_eq!(
            Part::checked("4:10", "1:30", false, d),
            Err(Problem::Backwards)
        );
        assert_eq!(
            Part::checked("1:30", "1:30", false, d),
            Err(Problem::Backwards)
        );
        assert_eq!(
            Part::checked("0:00", "14:49", false, d),
            Err(Problem::Whole)
        );
        assert_eq!(
            Part::checked("1:75", "2:00", false, d),
            Err(Problem::NotATime)
        );
        // Without a length, only the order.
        assert!(Part::checked("0:00", "99:00", false, None).is_ok());
    }

    #[test]
    fn a_parts_size_is_its_share_of_the_whole() {
        let p = Part {
            from: 90,
            to: 250,
            exact: false,
        };
        assert_eq!(p.estimate(Some(88_800_000), Some(888.0)), Some(16_000_000));
        assert_eq!(p.estimate(None, Some(888.0)), None);
        assert_eq!(p.estimate(Some(1000), None), None);
    }

    #[test]
    fn the_files_name_says_the_part() {
        let p = Part {
            from: 90,
            to: 250,
            exact: false,
        };
        assert_eq!(named("Sintel.mp4", &p), "Sintel (1m30s-4m10s).mp4");
        assert_eq!(
            named("Sintel (1m30s-4m10s).mp4", &p),
            "Sintel (1m30s-4m10s).mp4"
        );
        assert_eq!(named("A talk. Lisbon", &p), "A talk. Lisbon (1m30s-4m10s)");
        assert_eq!(unnamed("Sintel (1m30s-4m10s).mp4"), "Sintel.mp4");
        assert_eq!(unnamed("Sintel (1h02m03s-1h10m00s).webm"), "Sintel.webm");
        // Words of the title's own stay.
        assert_eq!(unnamed("Talk (2019).mp4"), "Talk (2019).mp4");
        assert_eq!(unnamed("Talk (live-set).mp4"), "Talk (live-set).mp4");
    }

    #[test]
    fn the_encoders_list_is_read_past_its_legend() {
        let listing = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n \
            V....D libopenh264          OpenH264 H.264\n V.S..D mpeg4   MPEG-4 part 2\n \
            A....D aac                  AAC (Advanced Audio Coding)\n";
        assert_eq!(encoder_names(listing), ["libopenh264", "mpeg4", "aac"]);
    }

    #[test]
    fn an_exact_cut_encodes_with_what_the_ffmpeg_has() {
        let own = [
            "libopenh264",
            "mpeg4",
            "aac",
            "libopus",
            "libvpx-vp9",
            "libmp3lame",
        ]
        .map(String::from);
        let mp4 = Made {
            ext: "mp4".into(),
            video: true,
            audio: true,
            video_kbps: Some(1500.0),
            audio_kbps: Some(129.0),
        };
        assert_eq!(
            exact_args(&mp4, &own).join(" "),
            "-c:v libopenh264 -pix_fmt yuv420p -b:v 3000k -maxrate 6000k -bufsize 6000k -c:a aac -b:a 192k"
        );
        let mut gpl = own.to_vec();
        gpl.push("libx264".into());
        assert!(
            exact_args(&mp4, &gpl)
                .join(" ")
                .starts_with("-c:v libx264 -preset fast -crf 18")
        );
        let webm = Made {
            ext: "webm".into(),
            ..mp4.clone()
        };
        assert!(
            exact_args(&webm, &own)
                .join(" ")
                .contains("-c:v libvpx-vp9")
        );
        assert!(
            exact_args(&webm, &own)
                .join(" ")
                .ends_with("-c:a libopus -b:a 192k")
        );
        let opus = Made {
            ext: "webm".into(),
            video: false,
            audio: true,
            audio_kbps: Some(256.0),
            ..Default::default()
        };
        assert_eq!(exact_args(&opus, &own).join(" "), "-c:a libopus -b:a 320k");
        let mp3 = Made {
            ext: "mp3".into(),
            video: false,
            audio: true,
            ..Default::default()
        };
        assert_eq!(exact_args(&mp3, &own).join(" "), "-c:a libmp3lame -q:a 2");
        // Nothing known to encode with: ffmpeg's defaults.
        assert!(exact_args(&mp4, &[]).is_empty());
    }

    #[test]
    fn what_a_choice_makes_is_read_from_its_formats() {
        let chosen = serde_json::json!({"ext": "mp4", "requested_formats": [
            {"format_id": "136", "vcodec": "avc1", "acodec": "none", "vbr": 531.3, "tbr": 531.3},
            {"format_id": "140", "vcodec": "none", "acodec": "mp4a.40.2", "abr": 129.5}]});
        let m = Made::of(&chosen);
        assert_eq!(
            (m.ext.as_str(), m.video, m.audio, m.video_kbps, m.audio_kbps),
            ("mp4", true, true, Some(531.3), Some(129.5))
        );
        let sound =
            serde_json::json!({"ext": "webm", "vcodec": "none", "acodec": "opus", "tbr": 130.0});
        let m = Made::of(&sound);
        assert_eq!((m.video, m.audio, m.audio_kbps), (false, true, Some(130.0)));
        let plain = serde_json::json!({"ext": "mp3", "tbr": 128.0});
        assert_eq!(
            (Made::of(&plain).video, Made::of(&plain).audio),
            (false, true)
        );
    }

    #[test]
    fn downloader_args_are_quoted_for_shlex() {
        let a = ["-c:v".to_string(), "libopenh264".into(), "a b".into()];
        assert_eq!(
            downloader_args("ffmpeg_o", &a),
            "ffmpeg_o:-c:v libopenh264 \"a b\""
        );
    }
}
