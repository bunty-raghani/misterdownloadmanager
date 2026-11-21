//! ffmpeg for merging video and audio (D-011, user 2026-10-30): BtbN's LGPL "shared" Windows
//! build of the newest stable line for this processor
//! (`ffmpeg-nX.Y-latest-win64-lgpl-shared-X.Y.zip`, `…-winarm64-…` on Windows on ARM; BtbN builds
//! none for 32-bit Windows), downloaded on first need, checked against the release's `checksums.sha256` and GitHub's own digest of the
//! asset, its `bin/` unpacked into `tools/ffmpeg/<version>/`, switched to atomically, the previous
//! version kept. When GitHub's API refuses for its rate limit (#1337), BtbN's `latest` release on
//! github.com's pages instead: its checksum list names the builds, and checks the one taken.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::github::{self, Api};
use crate::{Error, Installed, mark_downloaded, newer, sum_for};

/// BtbN's newest releases. Not `releases/latest`: BtbN publishes a dated build each day
/// (`autobuild-…`) and then remakes its `latest` release with simpler names, and meanwhile
/// GitHub's "latest" is the dated one, or a release still uploading its files.
const RELEASES: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases?per_page=5";
/// BtbN's repository on GitHub.
const REPO: &str = "BtbN/FFmpeg-Builds";
/// The release BtbN remakes with the newest builds under names that stay: the one read from
/// github.com's pages, which list no releases without the API.
const LATEST_TAG: &str = "latest";
/// Each release's checksum list.
const SUMS: &str = "checksums.sha256";

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

/// BtbN's name for its Windows builds for this processor (`win64`, `winarm64`); None where it
/// publishes none (32-bit Windows), and off Windows, where the system's ffmpeg is used.
pub fn build() -> Option<&'static str> {
    build_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether MDM downloads an ffmpeg of its own on this system ([`build`]). Where it doesn't, the
/// person's own (chosen in Helpers) or the system's is used.
pub fn available() -> bool {
    build().is_some()
}

/// [`build`] for a system and a processor, as Rust names them (`std::env::consts`).
fn build_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("win64"),
        ("windows", "aarch64") => Some("winarm64"),
        _ => None,
    }
}

/// The stable line of an LGPL shared asset for `build` (`win64`, `winarm64`), named as BtbN's
/// `latest` release names it (`ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`) or as a dated build
/// does (`ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip`) → `9.0`. Never the development
/// (`master`, `ffmpeg-N-…`) builds, nor another processor's.
fn stable_line<'a>(name: &'a str, build: &str) -> Option<&'a str> {
    let rest = name.strip_prefix("ffmpeg-n")?.strip_suffix(".zip")?;
    let (head, line) = rest.rsplit_once(&format!("-{build}-lgpl-shared-"))?;
    let numeric = |v: &str| !v.is_empty() && v.split('.').all(|p| p.parse::<u32>().is_ok());
    let (version, build) = head.split_once('-')?;
    let of_line = version == line || version.starts_with(&format!("{line}."));
    let build_ok = build == "latest"
        || build.split_once("-g").is_some_and(|(n, hash)| {
            n.parse::<u32>().is_ok() && hash.chars().all(|c| c.is_ascii_hexdigit())
        });
    (numeric(line) && numeric(version) && of_line && build_ok).then_some(line)
}

/// The release's LGPL shared build for `build` of the newest stable line.
fn best_build<'a>(release: &'a Release, build: &str) -> Option<&'a Asset> {
    let name = best_name(release.assets.iter().map(|a| a.name.as_str()), build)?;
    release.assets.iter().find(|a| a.name == name)
}

/// Of the file `names`, the LGPL shared build for `build` of the newest stable line.
fn best_name<'a>(names: impl Iterator<Item = &'a str>, build: &str) -> Option<&'a str> {
    let parts = |v: &str| -> Vec<u32> { v.split('.').filter_map(|p| p.parse().ok()).collect() };
    names
        .filter_map(|n| stable_line(n, build).map(|l| (parts(l), n)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, n)| n)
}

/// The build found on github.com's pages ([`from_pages`]).
struct Paged {
    name: String,
    url: String,
    version: String,
    /// The release's checksum list.
    sums: String,
}

/// BtbN's `latest` release on GitHub's pages at `web`, without its API (#1337): its checksum list
/// (`releases/download/latest/checksums.sha256`) names its files, the newest stable line's build
/// for `build` is taken from them, and its version is that line and the list's date (when the
/// release was made again; `.0` when the server gives none, which never replaces a dated
/// install).
async fn from_pages(net: &mdm_engine::Options, web: &str, build: &str) -> Result<Paged, Error> {
    let sums_url = github::download_url(web, REPO, LATEST_TAG, SUMS);
    let client = crate::client(net, &sums_url)?;
    let resp = client.get(&sums_url).send().await?.error_for_status()?;
    let date = resp
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|d| d.to_str().ok())
        .and_then(date_digits)
        .unwrap_or_else(|| "0".into());
    let sums = resp.text().await?;
    let names = sums.lines().filter_map(|l| {
        let (_, name) = l.trim().split_once(char::is_whitespace)?;
        Some(name.trim().trim_start_matches('*'))
    });
    let name = best_name(names, build)
        .ok_or_else(|| Error::Missing(format!("LGPL Windows build ({build})")))?
        .to_owned();
    let line = stable_line(&name, build).unwrap_or_default();
    Ok(Paged {
        url: github::download_url(web, REPO, LATEST_TAG, &name),
        version: format!("{line}.{date}"),
        name,
        sums,
    })
}

/// An HTTP date (`Mon, 16 Nov 2026 15:49:35 GMT`) as `20261116`, as the API's dates give it.
fn date_digits(http: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = http.split_whitespace().skip(1);
    let day: u32 = parts.next()?.parse().ok()?;
    let month = parts.next()?;
    let month = MONTHS.iter().position(|m| *m == month)? + 1;
    let year: u32 = parts.next()?.parse().ok()?;
    ((1..=31).contains(&day) && (1000..=9999).contains(&year))
        .then(|| format!("{year:04}{month:02}{day:02}"))
}

/// The ffmpeg to use: the person's own (`chosen`, #51), else MDM's install in `dir`
/// (`dir/ffmpeg/current.json`), if any and still there; else the system's own. Its `exe` is
/// `ffmpeg(.exe)`; yt-dlp is given its folder.
pub fn installed(dir: &Path) -> Option<Installed> {
    let own = crate::chosen::chosen(dir, crate::chosen::Helper::Ffmpeg).or_else(|| own(dir));
    own.or_else(|| crate::system::installed("ffmpeg"))
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
    install_with(dir, net, progress, None).await?;
    own(dir).ok_or_else(|| Error::Missing("ffmpeg after its install".into()))
}

/// [`install_latest`] for `autoupdate`: the newest build, installed when it is newer than the
/// version in use and than `floor` (a version the person went back from), switched to now or once
/// the runs using ffmpeg end.
pub(crate) async fn install_with(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
    floor: Option<&str>,
) -> Result<crate::autoupdate::Got, Error> {
    use crate::autoupdate::Got;
    let Some(build) = build() else {
        return Err(Error::Missing(if cfg!(windows) {
            "ffmpeg build for this processor (choose your own ffmpeg in Helpers)".into()
        } else {
            "ffmpeg build for this system (install ffmpeg)".into()
        }));
    };
    let root = dir.join("ffmpeg");
    let _installing = crate::autoupdate::installing(&root, "ffmpeg")?;
    // GitHub's API, else (its rate limit) the `latest` release's files on github.com (#1337).
    let client = crate::client(net, RELEASES)?;
    // Never the same again, never older, never at or below `floor`.
    let up_to_date = |version: &str| {
        (own(dir).is_some_and(|c| !newer(version, &c.version))
            || floor.is_some_and(|f| !newer(version, f)))
        .then(|| Got {
            latest: version.to_owned(),
            settled: None,
        })
    };
    let (name, url, version, sums, digest) = match github::api_get(&client, RELEASES).await? {
        Api::Answer(body) => {
            let mut releases: Vec<Release> =
                serde_json::from_slice(&body).map_err(|e| Error::BadRelease(e.to_string()))?;
            // Newest first (RFC 3339 times sort as text); the first with a build and its
            // checksums.
            releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
            let (release, asset) = releases
                .iter()
                .find_map(|r| {
                    let has_sums = r.assets.iter().any(|a| a.name == SUMS);
                    best_build(r, build).filter(|_| has_sums).map(|a| (r, a))
                })
                .ok_or_else(|| Error::Missing(format!("LGPL Windows build ({build})")))?;
            let line = stable_line(&asset.name, build).unwrap_or_default();
            // `9.0.20261028`: the line, then the build date (the release is rebuilt daily).
            let date: String = release
                .published_at
                .chars()
                .take(10)
                .filter(char::is_ascii_digit)
                .collect();
            let version = format!("{line}.{date}");
            if let Some(none) = up_to_date(&version) {
                return Ok(none);
            }
            let sums = release
                .assets
                .iter()
                .find(|a| a.name == SUMS)
                .ok_or_else(|| Error::Missing(SUMS.into()))?;
            let sums = client
                .get(&sums.browser_download_url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            (
                asset.name.clone(),
                asset.browser_download_url.clone(),
                version,
                sums,
                asset.digest.clone(),
            )
        }
        Api::RateLimited(refusal) => match from_pages(net, github::WEB, build).await {
            Ok(Paged {
                name,
                url,
                version,
                sums,
            }) => {
                tracing::info!("ffmpeg: {name} ({version}), from github.com's pages");
                if let Some(none) = up_to_date(&version) {
                    return Ok(none);
                }
                // GitHub's own digest of the file is the API's only: the checksum list checks it.
                (name, url, version, sums, None)
            }
            Err(e) => {
                tracing::warn!("ffmpeg: github.com's pages: {e}");
                return Err(refusal);
            }
        },
    };
    let expected =
        sum_for(&sums, &name).ok_or_else(|| Error::Missing(format!("checksum for {name}")))?;
    if let Some(d) = digest.as_deref().and_then(|d| d.strip_prefix("sha256:"))
        && hex::decode(d).ok().as_deref() != Some(&expected[..])
    {
        return Err(Error::BadChecksum);
    }

    // Already here (staged, or kept after going back): switched to, not downloaded again.
    if let Some(here) = crate::autoupdate::kept(&root, &version) {
        let settled = Some(crate::autoupdate::settle(&root, here)?);
        return Ok(Got {
            latest: version,
            settled,
        });
    }
    // Download to a file beside the install (about 80 MB), checked against the checksum.
    std::fs::create_dir_all(&root)?;
    // Named by its checksum: one release's cut-off download resumes, never another's.
    let zip_path = root.join(format!("ffmpeg-{}.zip", hex::encode(&expected[..8])));
    crate::fetch_verified(&url, &zip_path, expected, net, progress).await?;
    let res = unpack(&zip_path, &root, &version, &url);
    let _ = std::fs::remove_file(&zip_path);
    let exe = res?;
    tracing::info!("ffmpeg {version} verified and installed");
    let settled = Some(crate::autoupdate::settle(
        &root,
        Installed {
            version: version.clone(),
            exe,
        },
    )?);
    Ok(Got {
        latest: version,
        settled,
    })
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

    fn line(name: &str) -> Option<&str> {
        stable_line(name, "win64")
    }

    #[test]
    fn picks_stable_lgpl_shared_builds_only() {
        assert_eq!(
            line("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"),
            Some("9.0")
        );
        assert_eq!(line("ffmpeg-n9.0-latest-win64-lgpl-9.0.zip"), None);
        assert_eq!(line("ffmpeg-master-latest-win64-lgpl-shared.zip"), None);
        assert_eq!(line("ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip"), None);
        assert_eq!(line("ffmpeg-n8.1-latest-win64-lgpl-shared-9.0.zip"), None);
        // A dated build's names (what GitHub calls "latest" while BtbN remakes its own).
        assert_eq!(
            line("ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-shared-9.0.zip"),
            Some("9.0")
        );
        assert_eq!(
            line("ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-8.1.zip"),
            Some("8.1")
        );
        for other in [
            "ffmpeg-N-128406-g1fe6854fd9-win64-lgpl-shared.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-win64-lgpl-9.0.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-win64-gpl-shared-9.0.zip",
            "ffmpeg-n9.0.2-47-gb204e94e90-linux64-lgpl-shared-9.0.tar.xz",
            "ffmpeg-n8.1.3-34-ga35157abb9-win64-lgpl-shared-9.0.zip",
            "ffmpeg-n9.0.2-x-gzz-win64-lgpl-shared-9.0.zip",
            // Windows on ARM's: not x64's.
            "ffmpeg-n9.0-latest-winarm64-lgpl-shared-9.0.zip",
        ] {
            assert_eq!(line(other), None, "{other}");
        }
    }

    /// Each processor takes BtbN's build for it, never another's: x64's on x64, ARM64's on
    /// Windows on ARM; 32-bit Windows (BtbN builds none for it) and other systems none.
    #[test]
    fn each_processor_takes_its_own_build() {
        assert_eq!(build_for("windows", "x86_64"), Some("win64"));
        assert_eq!(build_for("windows", "aarch64"), Some("winarm64"));
        assert_eq!(build_for("windows", "x86"), None);
        assert_eq!(build_for("linux", "x86_64"), None);
        assert_eq!(build_for("macos", "aarch64"), None);
        assert_eq!(available(), build().is_some());
        // Today's `latest` release (2026-11-13), as BtbN names its files.
        let release = Release {
            published_at: "2026-11-13T13:02:35Z".into(),
            assets: [
                "ffmpeg-n8.1-latest-winarm64-lgpl-shared-8.1.zip",
                "ffmpeg-n9.0-latest-linuxarm64-lgpl-shared-9.0.tar.xz",
                "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip",
                "ffmpeg-n9.0-latest-winarm64-lgpl-shared-9.0.zip",
                "checksums.sha256",
            ]
            .iter()
            .map(|n| Asset {
                name: (*n).into(),
                browser_download_url: format!("https://x/{n}"),
                digest: None,
            })
            .collect(),
        };
        assert_eq!(
            best_build(&release, "win64").map(|a| a.name.as_str()),
            Some("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip")
        );
        assert_eq!(
            best_build(&release, "winarm64").map(|a| a.name.as_str()),
            Some("ffmpeg-n9.0-latest-winarm64-lgpl-shared-9.0.zip")
        );
        assert_eq!(
            stable_line(
                "ffmpeg-n9.0.2-57-gd838f9f9ae-winarm64-lgpl-shared-9.0.zip",
                "winarm64"
            ),
            Some("9.0")
        );
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
            best_build(&dated, "win64").map(|a| a.name.as_str()),
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
            best_build(r, "win64")
                .filter(|_| has_sums)
                .map(|a| a.name.clone())
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

    #[test]
    fn http_dates_become_build_dates() {
        assert_eq!(
            date_digits("Mon, 16 Nov 2026 15:49:35 GMT").as_deref(),
            Some("20261116")
        );
        assert_eq!(
            date_digits("Thu, 01 Jan 2026 00:00:00 GMT").as_deref(),
            Some("20260101")
        );
        for bad in [
            "",
            "Mon, 16 November 2026",
            "Mon, 32 Nov 2026 00:00:00 GMT",
            "16 Nov",
        ] {
            assert_eq!(date_digits(bad), None, "{bad}");
        }
    }

    /// #1337: with GitHub's API refusing for its rate limit, BtbN's `latest` release on the
    /// pages: its checksum list names the builds, the newest stable line's for this processor is
    /// taken (never another processor's, nor a development build), dated by the list, and it
    /// downloads and checks against that list.
    #[tokio::test]
    async fn the_latest_build_from_the_release_pages() {
        let build = b"a build of ffmpeg, as far as this test goes";
        let sum = |data: &[u8]| hex::encode(<sha2::Sha256 as sha2::Digest>::digest(data));
        let names = [
            "ffmpeg-master-latest-win64-lgpl-shared.zip",
            "ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip",
            "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip",
            "ffmpeg-n9.0-latest-winarm64-lgpl-shared-9.0.zip",
            "ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip",
        ];
        let list: String = names
            .iter()
            .map(|n| {
                let data: &[u8] = if n.contains("n9.0-latest-win64-lgpl") {
                    build
                } else {
                    b"another"
                };
                format!("{}  {n}\n", sum(data))
            })
            .collect();
        let files = tempfile::tempdir().unwrap();
        let release = files.path().join("gh").join(REPO).join(LATEST_TAG);
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(release.join(SUMS), &list).unwrap();
        std::fs::write(release.join(names[2]), build).unwrap();
        // Made again on 2026-11-16 15:49:35 UTC, as the server says.
        let made = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_794_844_175);
        std::fs::File::options()
            .write(true)
            .open(release.join(SUMS))
            .unwrap()
            .set_modified(made)
            .unwrap();
        let lab = mdm_labserver::LabServer::start(mdm_labserver::Config {
            files: Some(files.path().to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
        let net = crate::net(mdm_engine::Proxy::None);
        let web = lab.url("/gh/web");
        let found = from_pages(&net, &web, "win64").await.unwrap();
        assert_eq!(found.name, names[2]);
        assert_eq!(found.version, "9.0.20261116");
        assert_eq!(
            found.url,
            format!("{web}/{REPO}/releases/download/latest/{}", names[2])
        );
        assert_eq!(
            from_pages(&net, &web, "winarm64").await.unwrap().name,
            names[3]
        );
        // Downloaded through GitHub's redirect and checked against the list.
        let expected = sum_for(&found.sums, &found.name).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("ffmpeg.zip");
        crate::fetch_verified(&found.url, &zip, expected, &net, None)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&zip).unwrap(), build);
        // A file that isn't the build the list names is refused.
        std::fs::write(release.join(names[3]), b"not the listed build").unwrap();
        let arm = from_pages(&net, &web, "winarm64").await.unwrap();
        let bad = crate::fetch_verified(
            &arm.url,
            &dir.path().join("arm.zip"),
            sum_for(&arm.sums, &arm.name).unwrap(),
            &net,
            None,
        )
        .await;
        assert!(matches!(bad, Err(Error::BadChecksum)), "{bad:?}");
    }
}
