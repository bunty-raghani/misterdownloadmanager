//! The yt-dlp and ffmpeg the person chose instead of MDM's own copies (#51: one copy of each
//! for every program that uses them). `<tools>/programs.json` holds them; `installed` and
//! `ffmpeg::installed` read it first, so the app and the command line agree, and MDM's own
//! updates leave a chosen program alone (`install_latest`). The app's Video helpers window
//! writes it.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Installed};

/// One of the helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Helper {
    YtDlp,
    Ffmpeg,
    /// Image galleries (#58).
    GalleryDl,
}

impl Helper {
    pub fn name(self) -> &'static str {
        match self {
            Helper::YtDlp => "yt-dlp",
            Helper::Ffmpeg => "ffmpeg",
            Helper::GalleryDl => "gallery-dl",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Programs {
    #[serde(rename = "yt-dlp", default, skip_serializing_if = "Option::is_none")]
    ytdlp: Option<Installed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ffmpeg: Option<Installed>,
    #[serde(
        rename = "gallery-dl",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    gallery_dl: Option<Installed>,
    /// The JavaScript runtime yt-dlp uses for YouTube (#1570; `jsruntime`).
    #[serde(
        rename = "js-runtime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    js_runtime: Option<ChosenRuntime>,
}

/// A JavaScript runtime the person chose (`jsruntime::choose`): which one (its name to yt-dlp:
/// `node`, `deno`, `quickjs`, `bun`), the version it said (empty: not asked), and where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChosenRuntime {
    pub kind: String,
    #[serde(default)]
    pub version: String,
    pub exe: std::path::PathBuf,
}

const FILE: &str = "programs.json";

fn read(dir: &Path) -> Programs {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|b| {
            // A file edited by hand may start with a UTF-8 byte-order mark (Notepad, PowerShell).
            let json = b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&b);
            serde_json::from_slice(json).ok()
        })
        .unwrap_or_default()
}

fn write(dir: &Path, p: &Programs) -> Result<(), Error> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_vec_pretty(p).map_err(|e| Error::BadRelease(e.to_string()))?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(FILE))?;
    Ok(())
}

fn slot(p: &mut Programs, h: Helper) -> &mut Option<Installed> {
    match h {
        Helper::YtDlp => &mut p.ytdlp,
        Helper::Ffmpeg => &mut p.ffmpeg,
        Helper::GalleryDl => &mut p.gallery_dl,
    }
}

/// The JavaScript runtime the person chose, if any and still there.
pub fn chosen_runtime(dir: &Path) -> Option<ChosenRuntime> {
    read(dir).js_runtime.filter(|c| c.exe.is_file())
}

/// Use `runtime` from now on (`jsruntime::choose` checks it first).
pub fn choose_runtime(dir: &Path, runtime: ChosenRuntime) -> Result<(), Error> {
    let mut p = read(dir);
    p.js_runtime = Some(runtime);
    write(dir, &p)
}

/// Back to the JavaScript runtime MDM finds or installs.
pub fn forget_runtime(dir: &Path) -> Result<(), Error> {
    let mut p = read(dir);
    if p.js_runtime.take().is_some() {
        write(dir, &p)?;
    }
    Ok(())
}

/// The program the person chose for `h`, if any and still there.
pub fn chosen(dir: &Path, h: Helper) -> Option<Installed> {
    slot(&mut read(dir), h).take().filter(|i| i.exe.is_file())
}

/// Use `exe` for `h` from now on, once it has answered as that program (`--version`) in a
/// release MDM can drive (gallery-dl: `gallery::OLDEST` or newer, #236).
pub fn choose(dir: &Path, h: Helper, exe: &Path) -> Result<Installed, Error> {
    let installed = ask(h, exe)?;
    if h == Helper::GalleryDl && crate::gallery::too_old(&installed.version) {
        return Err(Error::TooOld {
            name: h.name(),
            version: installed.version,
            oldest: crate::gallery::OLDEST,
        });
    }
    let mut p = read(dir);
    *slot(&mut p, h) = Some(installed.clone());
    write(dir, &p)?;
    Ok(installed)
}

/// Back to MDM's own copy of `h`.
pub fn forget(dir: &Path, h: Helper) -> Result<(), Error> {
    let mut p = read(dir);
    if slot(&mut p, h).take().is_some() {
        write(dir, &p)?;
    }
    Ok(())
}

/// `exe` as `h`, with the version it says (`--version`), not chosen: for a check or a test.
pub fn ask(h: Helper, exe: &Path) -> Result<Installed, Error> {
    Ok(Installed {
        version: version_of(h, exe)?,
        exe: exe.to_owned(),
    })
}

/// What `exe` says its version is, asked as `h` asks it; not `h` if it doesn't answer as `h`.
fn version_of(h: Helper, exe: &Path) -> Result<String, Error> {
    let mut cmd = Command::new(exe);
    cmd.arg(match h {
        Helper::YtDlp | Helper::GalleryDl => "--version",
        Helper::Ffmpeg => "-version",
    });
    let out = crate::proc::output(cmd, h.name(), Duration::from_secs(20))
        .map_err(|_| Error::NotTheProgram(h.name()))?;
    parse_version(h, &String::from_utf8_lossy(&out)).ok_or(Error::NotTheProgram(h.name()))
}

/// yt-dlp and gallery-dl print their version alone (`2026.10.26`, `1.32.19`; gallery-dl 1.30.0
/// after an empty line, #236); ffmpeg starts with `ffmpeg version 8.0 …`.
fn parse_version(h: Helper, out: &str) -> Option<String> {
    let first = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    let v = match h {
        Helper::YtDlp | Helper::GalleryDl => first,
        Helper::Ffmpeg => first
            .strip_prefix("ffmpeg version ")?
            .split_whitespace()
            .next()?,
    };
    let plausible = !v.is_empty()
        && v.len() <= 64
        && v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-+~".contains(c));
    plausible.then(|| v.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_as_each_program_prints_them() {
        assert_eq!(
            parse_version(Helper::YtDlp, "2026.10.26\n").as_deref(),
            Some("2026.10.26")
        );
        assert_eq!(
            parse_version(
                Helper::Ffmpeg,
                "ffmpeg version 8.0-full_build-www.gyan.dev Copyright (c) 2000-2025\n"
            )
            .as_deref(),
            Some("8.0-full_build-www.gyan.dev")
        );
        assert_eq!(
            parse_version(
                Helper::Ffmpeg,
                "ffmpeg version n9.0.2-47-gb204e94e90 Copyright\n"
            )
            .as_deref(),
            Some("n9.0.2-47-gb204e94e90")
        );
        // Another program's answer.
        assert_eq!(parse_version(Helper::Ffmpeg, "Python 3.13.1\n"), None);
        assert_eq!(
            parse_version(Helper::YtDlp, "Usage: notepad <file> & more\n"),
            None
        );
        assert_eq!(parse_version(Helper::YtDlp, ""), None);
        assert_eq!(
            parse_version(Helper::GalleryDl, "\n1.30.0\n\n").as_deref(),
            Some("1.30.0")
        );
    }

    /// A chosen program is used first, and MDM's updates leave it alone: no download, no
    /// network (an address that can't answer would fail the test otherwise).
    #[tokio::test]
    async fn chosen_programs_are_used_and_not_updated() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("my-yt-dlp");
        std::fs::write(&exe, b"x").unwrap();
        let mine = Installed {
            version: "2026.01.01".into(),
            exe: exe.clone(),
        };
        write(
            dir.path(),
            &Programs {
                ytdlp: Some(mine.clone()),
                ffmpeg: Some(mine.clone()),
                gallery_dl: Some(mine.clone()),
                js_runtime: None,
            },
        )
        .unwrap();
        assert_eq!(crate::installed(dir.path()), Some(mine.clone()));
        assert_eq!(crate::ffmpeg::installed(dir.path()), Some(mine.clone()));
        assert_eq!(crate::gallery::installed(dir.path()), Some(mine.clone()));
        let net = mdm_engine::Options::default();
        assert_eq!(
            crate::install_latest(dir.path(), &net, None).await.unwrap(),
            mine
        );
        assert_eq!(
            crate::ffmpeg::install_latest(dir.path(), &net, None)
                .await
                .unwrap(),
            mine
        );
        assert_eq!(
            crate::gallery::install_latest(dir.path(), &net, None)
                .await
                .unwrap(),
            mine
        );
        assert!(!dir.path().join("yt-dlp").exists(), "nothing downloaded");
        assert!(
            !dir.path().join("gallery-dl").exists(),
            "nothing downloaded"
        );
    }

    /// A chosen runtime is kept beside the other choices, read back only while it is there, and
    /// forgotten alone.
    #[test]
    fn a_chosen_runtime_is_kept_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join("ff");
        let node = dir.path().join("node");
        std::fs::write(&ff, b"x").unwrap();
        std::fs::write(&node, b"x").unwrap();
        choose_runtime(
            dir.path(),
            ChosenRuntime {
                kind: "node".into(),
                version: "24.11.0".into(),
                exe: node.clone(),
            },
        )
        .unwrap();
        let mut p = read(dir.path());
        p.ffmpeg = Some(Installed {
            version: "8.0".into(),
            exe: ff.clone(),
        });
        write(dir.path(), &p).unwrap();
        assert_eq!(chosen_runtime(dir.path()).unwrap().exe, node);
        assert_eq!(chosen(dir.path(), Helper::Ffmpeg).unwrap().exe, ff);
        forget_runtime(dir.path()).unwrap();
        assert_eq!(chosen_runtime(dir.path()), None);
        assert_eq!(chosen(dir.path(), Helper::Ffmpeg).unwrap().exe, ff);
        // Gone from the disk: not used.
        choose_runtime(
            dir.path(),
            ChosenRuntime {
                kind: "node".into(),
                version: String::new(),
                exe: node.clone(),
            },
        )
        .unwrap();
        std::fs::remove_file(&node).unwrap();
        assert_eq!(chosen_runtime(dir.path()), None);
    }

    #[test]
    fn a_file_saved_with_a_byte_order_mark_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("ff");
        std::fs::write(&exe, b"x").unwrap();
        let json = format!(
            "\u{feff}{{\"ffmpeg\": {{\"version\": \"8.0\", \"exe\": {}}}}}",
            serde_json::to_string(&exe).unwrap()
        );
        std::fs::write(dir.path().join(FILE), json).unwrap();
        assert_eq!(chosen(dir.path(), Helper::Ffmpeg).map(|i| i.exe), Some(exe));
    }

    #[test]
    fn chosen_programs_are_kept_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        std::fs::write(&exe, b"x").unwrap();
        let p = Programs {
            ytdlp: None,
            ffmpeg: Some(Installed {
                version: "8.0".into(),
                exe: exe.clone(),
            }),
            gallery_dl: None,
            js_runtime: None,
        };
        write(dir.path(), &p).unwrap();
        assert_eq!(
            chosen(dir.path(), Helper::Ffmpeg).map(|i| i.exe),
            Some(exe.clone())
        );
        assert_eq!(chosen(dir.path(), Helper::YtDlp), None);
        // A program since removed counts as none.
        std::fs::remove_file(&exe).unwrap();
        assert_eq!(chosen(dir.path(), Helper::Ffmpeg), None);
        forget(dir.path(), Helper::Ffmpeg).unwrap();
        assert!(read(dir.path()).ffmpeg.is_none());
    }
}
