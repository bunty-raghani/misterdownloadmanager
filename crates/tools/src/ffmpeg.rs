//! ffmpeg for merging video and audio (D-011, user 2026-10-30): BtbN's LGPL "shared" Windows
//! build of the newest stable line (`ffmpeg-nX.Y-latest-win64-lgpl-shared-X.Y.zip`), downloaded
//! on first need, checked against the release's `checksums.sha256` and GitHub's own digest of the
//! asset, its `bin/` unpacked into `tools/ffmpeg/<version>/`, switched to atomically, the previous
//! version kept.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Installed, make_current, mark_downloaded, newer, sum_for};

/// BtbN's newest releases. Not `releases/latest`: BtbN publishes a dated build each day
/// (`autobuild-…`) and then remakes its `latest` release with simpler names, and meanwhile
/// GitHub's "latest" is the dated one, or a release still uploading its files.
const RELEASES: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases?per_page=5";

#[derive(Deserialize)]
struct Release {
    published_at: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>` (GitHub's digest of the upload).
    digest: Option<String>,
}

/// The stable line of an LGPL shared win64 asset, named as BtbN's `latest` release names it
/// (`ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`) or as a dated build does
/// (`ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip`) → `9.0`. Never the development
/// (`master`, `ffmpeg-N-…`) builds.
fn stable_line(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("ffmpeg-n")?.strip_suffix(".zip")?;
    let (head, line) = rest.rsplit_once("-win64-lgpl-shared-")?;
    let numeric = |v: &str| !v.is_empty() && v.split('.').all(|p| p.parse::<u32>().is_ok());
    let (version, build) = head.split_once('-')?;
    let of_line = version == line || version.starts_with(&format!("{line}."));
    let build_ok = build == "latest"
        || build.split_once("-g").is_some_and(|(n, hash)| {
            n.parse::<u32>().is_ok() && hash.chars().all(|c| c.is_ascii_hexdigit())
        });
    (numeric(line) && numeric(version) && of_line && build_ok).then_some(line)
}

/// The release's LGPL shared win64 build of the newest stable line.
fn best_build(release: &Release) -> Option<&Asset> {
    let parts = |v: &str| -> Vec<u32> { v.split('.').filter_map(|p| p.parse().ok()).collect() };
    release
        .assets
        .iter()
        .filter_map(|a| stable_line(&a.name).map(|l| (parts(l), a)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, a)| a)
}

/// The ffmpeg to use: the person's own (`chosen`, #51), else MDM's install in `dir`
/// (`dir/ffmpeg/current.json`), if any and still there; on Linux and macOS, else the system's
/// own. Its `exe` is `ffmpeg(.exe)`; yt-dlp is given its folder.
pub fn installed(dir: &Path) -> Option<Installed> {
    let own = crate::chosen::chosen(dir, crate::chosen::Helper::Ffmpeg).or_else(|| own(dir));
    #[cfg(unix)]
    return own.or_else(|| crate::system::installed("ffmpeg"));
    #[cfg(not(unix))]
    own
}

/// MDM's own install in `dir`.
fn own(dir: &Path) -> Option<Installed> {
    std::fs::read(dir.join("ffmpeg").join("current.json"))
        .ok()
        .and_then(|text| serde_json::from_slice::<Installed>(&text).ok())
        .filter(|i| i.exe.is_file())
}

/// Install the newest stable LGPL build into `dir` unless the installed one is as new, the zip
/// downloaded with MDM's engine (`net`), counted into `progress`.
pub async fn install_latest(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
) -> Result<Installed, Error> {
    // The person's own copy: theirs to update (#51).
    if let Some(mine) = crate::chosen::chosen(dir, crate::chosen::Helper::Ffmpeg) {
        return Ok(mine);
    }
    if !cfg!(windows) {
        return Err(Error::Missing(
            "ffmpeg build for this system (install ffmpeg)".into(),
        ));
    }
    let client = mdm_engine::net::client_builder(&net.dns, &net.route, RELEASES)?
        .user_agent(crate::USER_AGENT)
        .build()?;
    let body = client
        .get(RELEASES)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let mut releases: Vec<Release> =
        serde_json::from_slice(&body).map_err(|e| Error::BadRelease(e.to_string()))?;
    // Newest first (RFC 3339 times sort as text); the first with a build and its checksums.
    releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    let (release, asset) = releases
        .iter()
        .find_map(|r| {
            let has_sums = r.assets.iter().any(|a| a.name == "checksums.sha256");
            best_build(r).filter(|_| has_sums).map(|a| (r, a))
        })
        .ok_or_else(|| Error::Missing("LGPL Windows build".into()))?;
    let line = stable_line(&asset.name).unwrap_or_default();
    // `9.0.20261028`: the line, then the build date (the release is rebuilt daily).
    let date: String = release
        .published_at
        .chars()
        .take(10)
        .filter(char::is_ascii_digit)
        .collect();
    let version = format!("{line}.{date}");
    if let Some(current) = own(dir)
        && !newer(&version, &current.version)
    {
        return Ok(current);
    }
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == "checksums.sha256")
        .ok_or_else(|| Error::Missing("checksums.sha256".into()))?;
    let sums = client
        .get(&sums.browser_download_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let expected = sum_for(&sums, &asset.name)
        .ok_or_else(|| Error::Missing(format!("checksum for {}", asset.name)))?;
    if let Some(d) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        && hex::decode(d).ok().as_deref() != Some(&expected[..])
    {
        return Err(Error::BadChecksum);
    }

    // Download to a file beside the install (about 80 MB), checked against the checksum.
    let root = dir.join("ffmpeg");
    std::fs::create_dir_all(&root)?;
    // Named by its checksum: one release's cut-off download resumes, never another's.
    let zip_path = root.join(format!("ffmpeg-{}.zip", hex::encode(&expected[..8])));
    crate::fetch_verified(
        &asset.browser_download_url,
        &zip_path,
        expected,
        net,
        progress,
    )
    .await?;
    let res = unpack(&zip_path, &root, &version, &asset.browser_download_url);
    let _ = std::fs::remove_file(&zip_path);
    let exe = res?;
    make_current(&root, Installed { version, exe })
}

/// Unpack the zip's `bin/` (ffmpeg.exe, ffprobe.exe and their DLLs) into `root/<version>/`.
fn unpack(zip_path: &Path, root: &Path, version: &str, from: &str) -> Result<PathBuf, Error> {
    if version.is_empty() || version.contains(['/', '\\', ':']) || version.starts_with('.') {
        return Err(Error::BadRelease(format!("version {version:?}")));
    }
    let folder = root.join(version);
    std::fs::create_dir_all(&folder)?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?)
        .map_err(|e| Error::BadRelease(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| Error::BadRelease(e.to_string()))?;
        // Only plain files directly in a `bin` folder, by their own name (nothing can climb out).
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let in_bin = path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|p| p == "bin");
        let Some(name) = path.file_name().map(|n| n.to_owned()) else {
            continue;
        };
        if !entry.is_file() || !in_bin {
            continue;
        }
        let dest = folder.join(&name);
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
        if Path::new(&name).extension().is_some_and(|e| e == "exe") {
            mark_downloaded(&dest, from);
        }
    }
    let exe = folder.join("ffmpeg.exe");
    if !exe.is_file() {
        return Err(Error::Missing("ffmpeg.exe in the build".into()));
    }
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_stable_lgpl_shared_builds_only() {
        assert_eq!(
            stable_line("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"),
            Some("9.0")
        );
        assert_eq!(stable_line("ffmpeg-n9.0-latest-win64-lgpl-9.0.zip"), None);
        assert_eq!(
            stable_line("ffmpeg-master-latest-win64-lgpl-shared.zip"),
            None
        );
        assert_eq!(
            stable_line("ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip"),
            None
        );
        assert_eq!(
            stable_line("ffmpeg-n8.1-latest-win64-lgpl-shared-9.0.zip"),
            None
        );
        // A dated build's names (what GitHub calls "latest" while BtbN remakes its own).
        assert_eq!(
            stable_line("ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip"),
            Some("9.0")
        );
        assert_eq!(
            stable_line("ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-8.1.zip"),
            Some("8.1")
        );
        for other in [
            "ffmpeg-N-128406-g1fe6854fd9-win64-lgpl-shared.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-9.0.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-win64-gpl-shared-9.0.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-linux64-lgpl-shared-9.0.tar.xz",
            "ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-9.0.zip",
            "ffmpeg-n9.0.2-x-gzz-win64-lgpl-shared-9.0.zip",
        ] {
            assert_eq!(stable_line(other), None, "{other}");
        }
    }

    /// Today's case: the newest release is a dated build; and a release still uploading (no
    /// checksums yet) is passed over for the one before.
    #[test]
    fn the_newest_release_with_a_build_and_its_checksums() {
        let asset = |name: &str| Asset {
            name: name.into(),
            browser_download_url: format!("https://x/{name}"),
            digest: None,
        };
        let dated = Release {
            published_at: "2026-11-04T12:53:43Z".into(),
            assets: vec![
                asset("ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-8.1.zip"),
                asset("ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip"),
                asset("checksums.sha256"),
            ],
        };
        assert_eq!(
            best_build(&dated).map(|a| a.name.as_str()),
            Some("ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip")
        );
        let uploading = Release {
            published_at: "2026-11-05T12:50:00Z".into(),
            assets: vec![asset("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip")],
        };
        let mut releases = [dated, uploading];
        releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        let pick = releases.iter().find_map(|r| {
            let has_sums = r.assets.iter().any(|a| a.name == "checksums.sha256");
            best_build(r).filter(|_| has_sums).map(|a| a.name.clone())
        });
        assert_eq!(
            pick.as_deref(),
            Some("ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip")
        );
    }

    #[test]
    fn unpacks_only_bin_files() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("b.zip");
        {
            use std::io::Write;
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in [
                ("ffmpeg-n9.0/bin/ffmpeg.exe", "exe"),
                ("ffmpeg-n9.0/bin/avcodec-62.dll", "dll"),
                ("ffmpeg-n9.0/doc/readme.txt", "doc"),
                ("ffmpeg-n9.0/LICENSE.txt", "lic"),
            ] {
                w.start_file(name, opts).unwrap();
                w.write_all(data.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let exe = unpack(&zip_path, dir.path(), "9.0.20261028", "https://x/").unwrap();
        let folder = exe.parent().unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "exe");
        assert!(folder.join("avcodec-62.dll").is_file());
        assert!(!folder.join("readme.txt").exists());
        assert!(!folder.join("LICENSE.txt").exists());
    }
}
