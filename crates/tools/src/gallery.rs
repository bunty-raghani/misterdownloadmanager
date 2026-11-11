//! gallery-dl (#58): image galleries (Wikimedia Commons, Flickr, DeviantArt, Pixiv…) as yt-dlp
//! is for video pages. An optional helper, installed the same way: a release from its own page
//! (Codeberg), its `SHA256SUMS` checked against its author's OpenPGP key (pinned in
//! `keys/gallery-dl.asc`), the program against that list, kept in `tools/gallery-dl/<version>/`
//! with the previous version, never older; or the person's own copy (`chosen`). It runs in the
//! same job as yt-dlp does (`proc::spawn`), so Stop and MDM's end take it and all it started.
//!
//! A download is a folder: gallery-dl prints each file it saves (one path a line, `# path` for
//! one already there), and skips what is already there when started again, so a gallery started
//! again fetches only its new images.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::proc::{self, FromBrowser, ProcError, Spawned};
use crate::video::{Progress, State};
use crate::{Error, Installed, chosen};

/// gallery-dl's author's signing key (Mike Fährmann, fingerprint
/// 3E09F5908333DD83DBDCE7375680CA389D365A88): the same key on keys.openpgp.org, on his GitHub
/// account and on his Codeberg account. It signs each release's `SHA256SUMS`.
const KEY: &str = include_str!("../keys/gallery-dl.asc");
/// Where releases come from (Gitea's API, as GitHub's: `tag_name`, `assets`).
const RELEASES: &str = "https://codeberg.org/api/v1/repos/mikf/gallery-dl/releases/latest";
/// MDM as sites see it: gallery-dl's own name is refused by some (Wikimedia's servers answer
/// 429 to unnamed tools); this one says who asks and where to read about it.
pub const USER_AGENT: &str = concat!(
    "MisterDownloadManager/",
    env!("CARGO_PKG_VERSION"),
    " (+https://misterdownloadmanager.com) gallery-dl"
);

/// gallery-dl's program for this system in its releases. None on macOS (it publishes no build
/// for it: Homebrew's `gallery-dl`, chosen in Helpers, works there) and on Linux other than
/// x86-64 (the same, from the system's packages or pip).
pub fn asset() -> Option<&'static str> {
    if cfg!(windows) {
        Some("gallery-dl.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("gallery-dl.bin")
    } else {
        None
    }
}

/// Whether `sums` carries a valid signature `sig` by gallery-dl's author's key.
pub fn signed_by_gallery_dl(sums: &[u8], sig: &[u8]) -> bool {
    crate::signed_by(KEY, sums, sig)
}

/// The oldest gallery-dl MDM can drive: `--directory` (the files right in the gallery's own
/// folder) came in 1.20.0. Debian 11's 1.17 lacks it; Ubuntu 22.04's 1.20.5 and every later
/// distribution's has it (#236).
pub const OLDEST: &str = "1.20.0";

/// What a gallery-dl release accepts of the options MDM passes, past those every release from
/// [`OLDEST`] has (`--extractor-info`, `--sleep`, `--range`, `--limit-rate`, `--cookies`,
/// `--proxy`, `--ignore-config`). Each release from 1.17 to 1.30 was asked in mdm-lab (#236):
/// argparse refuses an option it doesn't know, so one too many fails the whole download, as
/// `--no-input` did with Ubuntu 24.04's 1.26.9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepts {
    /// `--no-input` (1.27.1): never a question on the console. Without it the same holds, as
    /// MDM gives it no input to read (`proc::spawn`): a question ends it instead of waiting.
    pub no_input: bool,
    /// `--user-agent` (1.24.0); before it, the same setting as `-o user-agent=…`.
    pub user_agent: bool,
}

impl Accepts {
    /// For `version` as gallery-dl prints it (`1.26.9`, `1.31.0-dev`). A version that can't be
    /// read is taken as a recent one, as before #236. The system's copy (#571), whose version
    /// isn't asked (finding it starts no program), gets what every release MDM drives accepts:
    /// a distribution's may be one of the oldest.
    pub fn of(version: &str) -> Accepts {
        if version == crate::SYSTEM {
            return Accepts {
                no_input: false,
                user_agent: false,
            };
        }
        let at_least = |v: &str| release(version).is_none_or(|have| Some(have) >= release(v));
        Accepts {
            no_input: at_least("1.27.1"),
            user_agent: at_least("1.24.0"),
        }
    }
}

/// `1.26.9` as (1, 26, 9): the leading digits of its first three parts (`0-dev` is 0).
fn release(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.trim().trim_start_matches('v').split('.').map(|p| {
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next()??;
    let minor = parts.next()??;
    Some((major, minor, parts.next().flatten().unwrap_or(0)))
}

/// Whether a gallery-dl of `version` is older than [`OLDEST`] (one that can't be read isn't).
pub fn too_old(version: &str) -> bool {
    release(version).is_some_and(|have| Some(have) < release(OLDEST))
}

/// The gallery-dl to use: the person's own (`chosen`), else MDM's install in `dir`; on Linux
/// and macOS, else the system's (#571, as yt-dlp's: a package manager's or Homebrew's, kept up
/// to date by it). A chosen one too old for MDM ([`too_old`], chosen before #236 checked) is
/// passed over; [`chosen_too_old`] names it, for the Helpers window to say so.
pub fn installed(dir: &Path) -> Option<Installed> {
    let own = chosen::chosen(dir, chosen::Helper::GalleryDl)
        .filter(|mine| !too_old(&mine.version))
        .or_else(|| own(dir));
    #[cfg(unix)]
    return own.or_else(|| crate::system::installed("gallery-dl"));
    #[cfg(not(unix))]
    own
}

/// The person's own gallery-dl when it is too old for MDM, so passed over.
pub fn chosen_too_old(dir: &Path) -> Option<Installed> {
    chosen::chosen(dir, chosen::Helper::GalleryDl).filter(|mine| too_old(&mine.version))
}

fn own(dir: &Path) -> Option<Installed> {
    let text = std::fs::read(dir.join("gallery-dl").join("current.json")).ok()?;
    let i: Installed = serde_json::from_slice(&text).ok()?;
    i.exe.is_file().then_some(i)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Install the latest gallery-dl into `dir` unless the installed one is as new, downloaded with
/// MDM's engine (`net`), counted into `progress`. The person's own copy is theirs to update. A
/// system copy doesn't count as installed here: this is the way to MDM's own instead (Helpers,
/// #571), which then wins ([`installed`]).
pub async fn install_latest(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
) -> Result<Installed, Error> {
    if let Some(mine) = chosen::chosen(dir, chosen::Helper::GalleryDl) {
        return Ok(mine);
    }
    let Some(asset) = asset() else {
        return Err(Error::Missing(
            "build for this system: install gallery-dl with the system's packages (Homebrew, pip) and choose it in Helpers".into(),
        ));
    };
    let client = mdm_engine::net::client_builder(&net.dns, &net.route, RELEASES)?
        .user_agent(crate::USER_AGENT)
        .build()?;
    let body = crate::get_bytes(&client, RELEASES).await?;
    let release: Release =
        serde_json::from_slice(&body).map_err(|e| Error::BadRelease(e.to_string()))?;
    let version = release.tag_name.trim_start_matches('v').to_owned();
    if let Some(current) = own(dir)
        && !crate::newer(&version, &current.version)
    {
        return Ok(current); // never the same again, never older
    }
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| Error::Missing(name.to_owned()))
    };
    let sums = crate::signed_list(
        &client,
        &url("SHA256SUMS")?,
        &url("SHA256SUMS.sig")?,
        signed_by_gallery_dl,
    )
    .await?;
    let expected = crate::sum_for(&sums, asset)
        .ok_or_else(|| Error::Missing(format!("checksum for {asset}")))?;
    let exe_url = url(asset)?;
    let root = dir.join("gallery-dl");
    std::fs::create_dir_all(&root)?;
    // Named by its checksum: one release's cut-off download resumes, never another's.
    let download = root.join(format!(
        "gallery-dl-{}.download",
        hex::encode(&expected[..8])
    ));
    crate::fetch_verified(&exe_url, &download, expected, net, progress).await?;
    let program = std::fs::read(&download)?;
    let _ = std::fs::remove_file(&download);
    if Sha256::digest(&program).as_slice() != expected {
        return Err(Error::BadChecksum);
    }
    crate::put(dir, "gallery-dl", &version, asset, &program, &exe_url)
}

/// Whether gallery-dl has a way to read `url` (a gallery, a user's page, a post…). Asked
/// offline: its extractors are matched against the address, nothing is fetched (`-E`, about a
/// second, most of it the program starting).
pub fn handles(tool: &Installed, url: &str) -> Result<bool, ProcError> {
    let cmd = proc::gallery_dl(tool, &["--extractor-info".into()], url)?;
    match proc::output(cmd, "gallery-dl", Duration::from_secs(30)) {
        Ok(_) => Ok(true),
        // Its words since 1.24, and before (#236: a distribution's may be that old).
        Err(ProcError::Failed { message, .. })
            if message.contains("Unsupported URL")
                || message.contains("No suitable extractor found") =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// A gallery's name, for its folder and its row: its site and the address's last part
/// (`commons.wikimedia.org - Category_Birds`), else its site; safe as a folder name.
pub fn name_of(page: &url::Url) -> String {
    let last = page
        .path_segments()
        .and_then(|mut s| s.rfind(|s| !s.is_empty()))
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .replace('_', " ")
        });
    let host = page
        .host_str()
        .unwrap_or("gallery")
        .trim_start_matches("www.");
    let name = match last {
        Some(l) => format!("{host} - {l}"),
        None => host.to_owned(),
    };
    mdm_engine::naming::sanitize(&name)
}

/// What a gallery download is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    /// At most this many files, the first ones gallery-dl lists (on most sites the newest).
    pub limit: Option<u64>,
    /// Bytes/s, for each file.
    pub rate: Option<u64>,
}

/// Paths kept of the files saved (to mark them as downloaded from the web at the end); a
/// gallery past this many still downloads, its later files unmarked.
const KEEP_PATHS: usize = 100_000;

#[derive(Default)]
struct Shared {
    progress: Progress,
    /// Files saved this run, files already there (skipped).
    saved: u64,
    there: u64,
    /// The files saved this run (at most `KEEP_PATHS`).
    files: Vec<PathBuf>,
}

impl Default for Progress {
    fn default() -> Self {
        Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events: Vec::new(),
            side_files: Vec::new(),
            sub_problems: Vec::new(),
        }
    }
}

/// A gallery downloading into a folder.
pub struct GalleryJob {
    shared: Arc<Mutex<Shared>>,
    child: Arc<Mutex<Option<Spawned>>>,
}

impl GalleryJob {
    /// Download `url`'s files into `folder`, the gallery's own.
    pub fn start(
        tool: &Installed,
        url: &str,
        folder: &Path,
        ask: &Ask,
        browser: &FromBrowser,
    ) -> Result<GalleryJob, ProcError> {
        let mut ours: Vec<String> = vec![
            // The files right in `folder` (the gallery's own), not in subfolders of the site's.
            "--directory".into(),
            folder.to_string_lossy().into_owned(),
            // One file a second at most: gentle with the sites (Wikimedia answers 429 to
            // faster tools) and with the person's line.
            "--sleep".into(),
            "1".into(),
        ];
        if let Some(n) = ask.limit.filter(|n| *n > 0) {
            ours.extend(["--range".into(), format!("1-{n}")]);
        }
        if let Some(r) = ask.rate.filter(|r| *r > 0) {
            ours.extend(["--limit-rate".into(), r.to_string()]);
        }
        // The browser's cookies and User-Agent (gallery-dl has no Referer option: left out).
        let (theirs, cookies) = browser.args(url)?;
        ours.extend(browser_args(theirs, Accepts::of(&tool.version)));
        std::fs::create_dir_all(folder).map_err(|e| ProcError::Start("gallery-dl".into(), e))?;
        let cmd = proc::gallery_dl(tool, &ours, url)?;
        let mut spawned = proc::spawn(cmd, "gallery-dl")?;
        let stdout = spawned.child.stdout.take().expect("piped");
        let shared = Arc::new(Mutex::new(Shared::default()));
        let child = Arc::new(Mutex::new(Some(spawned)));
        let (sh, ch, folder) = (shared.clone(), child.clone(), folder.to_owned());
        std::thread::spawn(move || {
            let _cookies = cookies;
            read(stdout, &sh);
            finish(&sh, &ch, folder);
        });
        Ok(GalleryJob { shared, child })
    }

    pub fn progress(&self) -> Progress {
        self.shared.lock().unwrap().progress.clone()
    }

    /// The files saved so far this run (asked once, at the end: not every frame).
    pub fn saved(&self) -> Vec<PathBuf> {
        self.shared.lock().unwrap().files.clone()
    }

    /// End gallery-dl and all it started; what was saved stays.
    pub fn stop(&self) {
        if let Some(mut s) = self.child.lock().unwrap().take() {
            s.kill();
            let mut sh = self.shared.lock().unwrap();
            if sh.progress.state == State::Running {
                sh.progress.state = State::Stopped;
            }
        }
    }
}

impl Drop for GalleryJob {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The browser's context (`FromBrowser::args`, made for yt-dlp) as gallery-dl takes it: its
/// cookies, and its User-Agent or MDM's (`--user-agent`, or `-o user-agent=…` before 1.24); no
/// Referer, which gallery-dl has no option for.
fn browser_args(theirs: Vec<String>, accepts: Accepts) -> Vec<String> {
    let mut out = Vec::new();
    let mut agent = None;
    let mut theirs = theirs.into_iter();
    while let Some(a) = theirs.next() {
        match a.as_str() {
            "--referer" => {
                theirs.next();
            }
            "--user-agent" => agent = theirs.next(),
            "--cookies" => {
                out.push(a);
                out.extend(theirs.next());
            }
            _ => out.push(a),
        }
    }
    let agent = agent.unwrap_or_else(|| USER_AGENT.to_owned());
    if accepts.user_agent {
        out.extend(["--user-agent".into(), agent]);
    } else {
        out.extend(["-o".into(), format!("user-agent={agent}")]);
    }
    out
}

/// What one line of gallery-dl's says: a file saved, or one already there.
#[derive(Debug, PartialEq)]
enum Line {
    Saved(PathBuf),
    There(PathBuf),
}

fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(p) = line.strip_prefix("# ") {
        return Some(Line::There(PathBuf::from(p)));
    }
    let p = Path::new(line);
    p.is_absolute().then(|| Line::Saved(p.to_owned()))
}

/// Count gallery-dl's files as it prints them. Lines are read whole but kept short: a path is
/// at most a few thousand bytes, and a longer line is no path.
fn read(stdout: impl std::io::Read, shared: &Mutex<Shared>) {
    let started = Instant::now();
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match std::io::Read::take(&mut reader, 64 * 1024).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        let Some(line) = parse_line(&text) else {
            continue;
        };
        let mut sh = shared.lock().unwrap();
        match line {
            Line::Saved(p) => {
                sh.saved += 1;
                sh.progress.downloaded += std::fs::metadata(&p).map_or(0, |m| m.len());
                if sh.files.len() < KEEP_PATHS {
                    sh.files.push(p);
                }
            }
            Line::There(_) => sh.there += 1,
        }
        let secs = started.elapsed().as_secs_f64().max(0.5);
        sh.progress.speed = sh.progress.downloaded as f64 / secs;
    }
}

/// gallery-dl's end: what it saved and what it couldn't, said in the download's log.
fn finish(shared: &Mutex<Shared>, child: &Mutex<Option<Spawned>>, folder: PathBuf) {
    // Polled with the lock free in between, so `stop` never waits on it.
    let (code, failure) = loop {
        let mut slot = child.lock().unwrap();
        let Some(s) = slot.as_mut() else {
            return; // stopped
        };
        match s.child.try_wait() {
            Ok(Some(st)) => {
                // Let the stderr reader catch up with the last lines.
                std::thread::sleep(Duration::from_millis(50));
                break (st.code(), s.failure().to_string());
            }
            Ok(None) => {}
            Err(e) => break (None, e.to_string()),
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(100));
    };
    child.lock().unwrap().take();
    let mut sh = shared.lock().unwrap();
    let (saved, there) = (sh.saved, sh.there);
    sh.progress.speed = 0.0;
    sh.progress.events.push((
        SystemTime::now(),
        format!("gallery-dl saved {saved} files ({there} already there)"),
    ));
    sh.progress.state = match code {
        Some(0) => State::Done(folder),
        // Some files couldn't be had (4: a download failed): the rest is saved, and starting
        // it again tries those only.
        _ if saved + there > 0 => {
            sh.progress
                .events
                .push((SystemTime::now(), format!("Some files failed: {failure}")));
            State::Done(folder)
        }
        _ => State::Failed(failure),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &[u8] = include_bytes!("../tests/fixtures/gallery-dl-SHA256SUMS");
    const SIG: &[u8] = include_bytes!("../tests/fixtures/gallery-dl-SHA256SUMS.sig");

    #[test]
    fn a_real_release_list_is_signed_by_the_pinned_key() {
        assert!(signed_by_gallery_dl(SUMS, SIG));
        let mut forged = SUMS.to_vec();
        forged[0] = if forged[0] == b'0' { b'1' } else { b'0' };
        assert!(!signed_by_gallery_dl(&forged, SIG));
        // Each key signs for itself only.
        assert!(!crate::signed_by_ytdlp(SUMS, SIG));
        let sums = String::from_utf8_lossy(SUMS);
        assert_eq!(
            hex::encode(crate::sum_for(&sums, "gallery-dl.exe").unwrap()),
            "4dda3f14efbc8a8e616af1fc7e1e88d795995326b47692453e620117254dec18"
        );
    }

    /// #236: each release gets only the options it accepts, as found by asking every release
    /// from 1.17 to 1.30 (Ubuntu 24.04's 1.26.9 refused `--no-input`).
    #[test]
    fn each_release_gets_only_the_options_it_accepts() {
        let accepts = |v: &str| {
            let a = Accepts::of(v);
            (a.no_input, a.user_agent)
        };
        assert_eq!(accepts("1.20.0"), (false, false));
        assert_eq!(accepts("1.23.5"), (false, false));
        assert_eq!(accepts("1.24.0"), (false, true));
        assert_eq!(accepts("1.26.9"), (false, true));
        assert_eq!(accepts("1.27.0"), (false, true));
        assert_eq!(accepts("1.27.1"), (true, true));
        assert_eq!(accepts("1.32.19"), (true, true));
        assert_eq!(accepts("1.31.0-dev"), (true, true));
        assert_eq!(accepts("2.0"), (true, true));
        // One that can't be read: as recent as MDM's own.
        assert_eq!(accepts("unknown"), (true, true));
        // The system's, not asked (#571): what every release from 1.20.0 accepts.
        assert_eq!(accepts("system"), (false, false));
        assert!(!too_old("system"));
        assert!(too_old("1.17.0") && too_old("1.19.3") && too_old("0.9"));
        assert!(!too_old("1.20.0") && !too_old("1.26.9") && !too_old("unknown"));

        let args = |v: &str| -> Vec<String> {
            let tool = Installed {
                version: v.into(),
                exe: "gallery-dl".into(),
            };
            proc::gallery_dl(&tool, &["--sleep".into(), "1".into()], "https://x.test/g")
                .unwrap()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            args("1.26.9"),
            ["--sleep", "1", "--ignore-config", "--", "https://x.test/g"]
        );
        assert_eq!(
            args("1.27.1"),
            [
                "--sleep",
                "1",
                "--ignore-config",
                "--no-input",
                "--",
                "https://x.test/g"
            ]
        );
    }

    /// The browser's User-Agent (else MDM's) and cookies, as each release takes them.
    #[test]
    fn the_browsers_context_as_each_release_takes_it() {
        let theirs = || -> Vec<String> {
            [
                "--referer",
                "https://x.test/",
                "--user-agent",
                "Firefox/150",
                "--cookies",
                "/c.txt",
            ]
            .map(String::from)
            .to_vec()
        };
        let new = Accepts::of("1.30.0");
        let old = Accepts::of("1.20.5");
        assert_eq!(
            browser_args(theirs(), new),
            ["--cookies", "/c.txt", "--user-agent", "Firefox/150"]
        );
        assert_eq!(
            browser_args(theirs(), old),
            ["--cookies", "/c.txt", "-o", "user-agent=Firefox/150"]
        );
        assert_eq!(browser_args(Vec::new(), new), ["--user-agent", USER_AGENT]);
        assert_eq!(
            browser_args(Vec::new(), old),
            ["-o".to_owned(), format!("user-agent={USER_AGENT}")]
        );
    }

    /// A gallery-dl chosen before #236 checked, too old for MDM: MDM's own copy instead.
    #[test]
    fn a_chosen_release_too_old_gives_way_to_mdms_own() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("old-gallery-dl");
        std::fs::write(&exe, b"x").unwrap();
        let json = serde_json::json!({ "gallery-dl": { "version": "1.17.0", "exe": exe } });
        std::fs::write(dir.path().join("programs.json"), json.to_string()).unwrap();
        // Not the old one: on Linux and macOS a gallery-dl the system has may stand in (#571).
        assert!(
            installed(dir.path()).is_none_or(|i| i.version != "1.17.0"),
            "not the old one"
        );
        assert_eq!(
            chosen_too_old(dir.path()).map(|i| i.version).as_deref(),
            Some("1.17.0")
        );
        let own = crate::put(
            dir.path(),
            "gallery-dl",
            "1.32.19",
            asset().unwrap_or("gallery-dl"),
            b"x",
            "https://x/",
        )
        .unwrap();
        assert_eq!(installed(dir.path()), Some(own));
    }

    #[test]
    fn its_lines_are_files_saved_or_already_there() {
        let saved = if cfg!(windows) {
            r"C:\Users\x\Pictures\wikimediacommons\Birds\Rhea (de11b288).jpg"
        } else {
            "/home/x/Pictures/wikimediacommons/Birds/Rhea (de11b288).jpg"
        };
        assert_eq!(
            parse_line(&format!("{saved}\r\n")),
            Some(Line::Saved(PathBuf::from(saved)))
        );
        assert_eq!(
            parse_line(&format!("# {saved}")),
            Some(Line::There(PathBuf::from(saved)))
        );
        assert_eq!(parse_line("[wikimedia][info] something"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn a_long_output_is_counted_without_holding_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, vec![0u8; 1000]).unwrap();
        let mut text = String::new();
        for _ in 0..5000 {
            text.push_str(&format!("{}\n# {}\n", file.display(), file.display()));
        }
        text.push_str(&"x".repeat(200_000));
        text.push('\n');
        let shared = Mutex::new(Shared::default());
        read(text.as_bytes(), &shared);
        let sh = shared.lock().unwrap();
        assert_eq!((sh.saved, sh.there), (5000, 5000));
        assert_eq!(sh.progress.downloaded, 5_000_000);
    }

    #[test]
    fn a_gallery_is_named_by_its_address() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(
            name_of(&u(
                "https://commons.wikimedia.org/wiki/Category:Featured_pictures_of_birds"
            )),
            "commons.wikimedia.org - Category_Featured pictures of birds"
        );
        assert_eq!(
            name_of(&u(
                "https://www.flickr.com/photos/nasawebbtelescope/albums/72177720313923911/"
            )),
            "flickr.com - 72177720313923911"
        );
        assert_eq!(name_of(&u("https://imgur.com/")), "imgur.com");
    }

    #[test]
    fn installs_go_in_their_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let name = asset().unwrap_or("gallery-dl");
        let now = crate::put(
            dir.path(),
            "gallery-dl",
            "1.32.19",
            name,
            b"x",
            "https://x/",
        )
        .unwrap();
        assert_eq!(installed(dir.path()), Some(now));
        assert!(crate::own_ytdlp(dir.path()).is_none(), "not yt-dlp's");
    }
}
