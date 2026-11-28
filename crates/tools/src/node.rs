//! Node.js, the JavaScript runtime MDM installs for yt-dlp when the computer has none (#1570):
//! YouTube makes yt-dlp solve a JavaScript challenge, and without a runtime a look-up is slow,
//! lists few qualities, or fails ("Requested format is not available"). Which runtime is used,
//! the computer's own first, is `jsruntime`'s choice; this is MDM's own copy.
//!
//! - **Which release:** the newest long-term support (LTS) release in nodejs.org's own list
//!   (`dist/index.json`) that has a build for this system and processor: 64-bit Windows (x64,
//!   ARM64), macOS (Intel, Apple silicon) and Linux (x64, ARM64) take the newest LTS; 32-bit
//!   Windows the newest LTS line that still builds for it (Node.js 22).
//! - **How it is checked:** the release's `SHASUMS256.txt` must carry a valid signature
//!   (`SHASUMS256.txt.sig`) by one of Node.js's releasers, whose keys are pinned in
//!   `keys/node/` (the active releasers of github.com/nodejs/release-keys); the archive must then
//!   match the SHA-256 that list gives for it, as yt-dlp's program matches its signed list.
//! - **Where it goes:** only the program (`node.exe`, `bin/node`) is taken out of the archive,
//!   into `tools/node/<version>/`, and switched to atomically beside the previous version
//!   (`autoupdate`), which is kept to go back to. Never an older version.

use std::path::{Path, PathBuf};

use crate::{Error, Installed, mark_downloaded, newer, sum_for};

/// nodejs.org's list of every release.
const INDEX: &str = "https://nodejs.org/dist/index.json";
/// Where a release's files are: `<DIST>v26.13.1/<file>`.
const DIST: &str = "https://nodejs.org/dist/";

/// The keys Node.js's active releasers sign `SHASUMS256.txt` with, from
/// github.com/nodejs/release-keys (README's "Active releasers keys", at its commit 481637f8 of
/// 2026-09-01), each named by its fingerprint. A release signed by a key not here is refused
/// until MDM pins it: a new releaser's key comes with an MDM release.
const KEYS: [&str; 8] = [
    // Antoine du Hamel (Ed25519)
    include_str!("../keys/node/5BE8A3F6C8A5C01D106C0AD820B1A390B168D356.asc"),
    // Juan José Arboleda (Ed25519)
    include_str!("../keys/node/DD792F5973C6DE52C432CBDAC77ABFA00DDBF2B7.asc"),
    // Marco Ippolito (RSA)
    include_str!("../keys/node/CC68F5A3106FF448322E48ED27F5E38D5B0A215F.asc"),
    // Rafael Gonzaga (RSA)
    include_str!("../keys/node/890C08DB8579162FEE0DF9DB8BEAB4DFCF555EF4.asc"),
    // Richard Lau (RSA)
    include_str!("../keys/node/C82FA3AE1CBEDC6BE46B9360C43CEC45C17AB93C.asc"),
    // Ruy Adorno (RSA)
    include_str!("../keys/node/108F52B48DB57BB0CC439B2997B01419BD92F80A.asc"),
    // Stewart X Addison (Ed25519)
    include_str!("../keys/node/655F3B5C1FB3FA8D1A0CA6BDE4A7D232B936D2FD.asc"),
    // Ulises Gascón (RSA)
    include_str!("../keys/node/A363A499291CBBC940DD62E41F10027AF002F8B0.asc"),
];

/// Whether `sums` (a release's `SHASUMS256.txt`) carries a valid signature `sig` (binary
/// OpenPGP) by one of Node.js's releasers ([`KEYS`]).
pub fn signed_by_node(sums: &[u8], sig: &[u8]) -> bool {
    KEYS.iter().any(|key| crate::signed_by(key, sums, sig))
}

/// A Node.js build for one system and processor: the name nodejs.org's list gives it in a
/// release's `files`, and its archive's platform part (`node-v<version>-<platform>.<ext>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Build {
    /// In `index.json`'s `files`.
    listed: &'static str,
    /// In the archive's name.
    platform: &'static str,
    /// The archive's extension.
    ext: &'static str,
    /// The oldest release taken for it ([`OLDEST`]).
    oldest: &'static str,
}

/// The oldest Node.js release MDM installs, whatever nodejs.org's list says (the list itself isn't
/// signed; each release's checksums are, older ones too): the newest LTS line when this MDM was
/// made (26, "Lithium"). 32-bit Windows: 22, the newest line still built for it (supported until
/// April 2027).
const OLDEST: &str = "26.0.0";
const OLDEST_WIN_X86: &str = "22.0.0";

/// Node.js's build for this system and processor; None where nodejs.org has none MDM takes
/// (other processors; Linux with musl isn't told apart yet, and gets the glibc build).
pub fn build() -> Option<Build> {
    build_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether MDM downloads a Node.js of its own on this system ([`build`]).
pub fn available() -> bool {
    build().is_some()
}

/// [`build`] for a system and a processor, as Rust names them (`std::env::consts`).
fn build_for(os: &str, arch: &str) -> Option<Build> {
    let (listed, platform, ext) = match (os, arch) {
        ("windows", "x86_64") => ("win-x64-zip", "win-x64", "zip"),
        ("windows", "aarch64") => ("win-arm64-zip", "win-arm64", "zip"),
        ("windows", "x86") => ("win-x86-zip", "win-x86", "zip"),
        ("macos", "x86_64") => ("osx-x64-tar", "darwin-x64", "tar.gz"),
        ("macos", "aarch64") => ("osx-arm64-tar", "darwin-arm64", "tar.gz"),
        ("linux", "x86_64") => ("linux-x64", "linux-x64", "tar.gz"),
        ("linux", "aarch64") => ("linux-arm64", "linux-arm64", "tar.gz"),
        _ => return None,
    };
    let oldest = if platform == "win-x86" {
        OLDEST_WIN_X86
    } else {
        OLDEST
    };
    Some(Build {
        listed,
        platform,
        ext,
        oldest,
    })
}

impl Build {
    /// The archive's name for `version` (`26.13.1`): `node-v26.13.1-win-x64.zip`.
    fn archive(&self, version: &str) -> String {
        format!("node-v{version}-{}.{}", self.platform, self.ext)
    }

    /// The program's path inside that archive.
    fn program_in(&self, version: &str) -> String {
        let top = format!("node-v{version}-{}", self.platform);
        if self.ext == "zip" {
            format!("{top}/node.exe")
        } else {
            format!("{top}/bin/node")
        }
    }
}

/// One release in nodejs.org's list.
#[derive(serde::Deserialize)]
struct Listed {
    version: String,
    #[serde(default)]
    files: Vec<String>,
    /// The LTS line's name ("Krypton"), or `false`.
    #[serde(default)]
    lts: serde_json::Value,
}

/// The newest LTS release in `index` (nodejs.org's `index.json`, newest first) with a `build`:
/// its version, without the `v` (`26.13.1`). A version that isn't plain numbers is passed over,
/// and so is one older than the build's oldest ([`OLDEST`]): the list isn't signed, so it can't
/// steer MDM to an old (signed) release.
fn newest_lts(index: &[u8], build: &Build) -> Result<String, Error> {
    let list: Vec<Listed> =
        serde_json::from_slice(index).map_err(|e| Error::BadRelease(format!("index.json: {e}")))?;
    let mut best: Option<String> = None;
    for r in list {
        let lts = !matches!(
            r.lts,
            serde_json::Value::Bool(false) | serde_json::Value::Null
        );
        let Some(v) = r.version.strip_prefix('v') else {
            continue;
        };
        let recent = v == build.oldest || newer(v, build.oldest);
        if !lts || !plain_version(v) || !recent || !r.files.iter().any(|f| f == build.listed) {
            continue;
        }
        if best.as_deref().is_none_or(|b| newer(v, b)) {
            best = Some(v.to_owned());
        }
    }
    best.ok_or_else(|| Error::Missing(format!("Node.js LTS build {}", build.listed)))
}

/// `26.13.1`: three numbers, nothing else (no path can come of it).
fn plain_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `node.exe` on Windows, `node` elsewhere.
fn program() -> String {
    format!("node{}", std::env::consts::EXE_SUFFIX)
}

/// MDM's own Node.js in the tools folder `dir` (`dir/node/current.json`), if any and still there.
pub fn installed(dir: &Path) -> Option<Installed> {
    crate::autoupdate::current_in(&dir.join("node")).filter(|i| i.exe.starts_with(dir.join("node")))
}

/// Install Node.js's newest LTS release into `dir` unless the installed one is as new, the archive
/// downloaded with MDM's engine (`net`), counted into `progress`. Returns what is installed
/// afterwards.
pub async fn install_latest(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
) -> Result<Installed, Error> {
    install_with(dir, net, progress, None).await?;
    installed(dir).ok_or_else(|| Error::Missing("Node.js after its install".into()))
}

/// Where Node.js's releases come from and the check of their checksum list: nodejs.org's and
/// its releasers' keys, outside this module's tests.
pub(crate) struct Source<'a> {
    pub index: &'a str,
    pub dist: &'a str,
    pub signed: fn(&[u8], &[u8]) -> bool,
}

const NODEJS_ORG: Source<'static> = Source {
    index: INDEX,
    dist: DIST,
    signed: signed_by_node,
};

/// [`install_latest`] for `autoupdate`: the newest LTS release, installed when it is newer than
/// the version in use and than `floor` (a version the person went back from), switched to now or
/// once the runs using Node.js end.
pub(crate) async fn install_with(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
    floor: Option<&str>,
) -> Result<crate::autoupdate::Got, Error> {
    install_from(dir, net, progress, floor, &NODEJS_ORG).await
}

/// [`install_with`] from `source`.
pub(crate) async fn install_from(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
    floor: Option<&str>,
    source: &Source<'_>,
) -> Result<crate::autoupdate::Got, Error> {
    use crate::autoupdate::{Got, kept, settle};
    let build = build().ok_or_else(|| Error::Missing("Node.js build for this system".into()))?;
    let root = dir.join("node");
    let _installing = crate::autoupdate::installing(&root, "node")?;
    let client = crate::client(net, source.index)?;
    let index = crate::get_bytes(&client, source.index).await?;
    let version = newest_lts(&index, &build)?;
    let none = || Got {
        latest: version.clone(),
        settled: None,
    };
    if installed(dir).is_some_and(|current| !newer(&version, &current.version))
        || floor.is_some_and(|f| !newer(&version, f))
    {
        return Ok(none()); // never the same again, never older
    }
    // Already here (staged, or kept after going back): switched to, not downloaded again.
    if let Some(here) = kept(&root, &version) {
        let settled = Some(settle(&root, here)?);
        return Ok(Got {
            latest: version,
            settled,
        });
    }
    let folder = format!("{}v{version}/", source.dist);
    let client = crate::client(net, &folder)?;
    let sums = crate::signed_list(
        &client,
        &format!("{folder}SHASUMS256.txt"),
        &format!("{folder}SHASUMS256.txt.sig"),
        source.signed,
    )
    .await?;
    let archive = build.archive(&version);
    let expected = sum_for(&sums, &archive)
        .ok_or_else(|| Error::Missing(format!("checksum for {archive}")))?;
    let url = format!("{folder}{archive}");
    std::fs::create_dir_all(&root)?;
    // Named by its checksum: one release's cut-off download resumes, never another's.
    let path = root.join(format!(
        "node-{}.{}",
        hex::encode(&expected[..8]),
        build.ext
    ));
    crate::fetch_verified(&url, &path, expected, net, progress).await?;
    let res = unpack(&path, &root, &version, &build, &url);
    let _ = std::fs::remove_file(&path);
    let exe = res?;
    tracing::info!(
        "Node.js {version} verified and installed: {}",
        exe.display()
    );
    let settled = Some(settle(
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

/// Take the archive's one program (`build.program_in`, matched by its exact name: no other entry
/// is ever written) into `root/<version>/`.
fn unpack(
    archive: &Path,
    root: &Path,
    version: &str,
    build: &Build,
    from: &str,
) -> Result<PathBuf, Error> {
    if !plain_version(version) {
        return Err(Error::BadRelease(format!("version {version:?}")));
    }
    let inside = build.program_in(version);
    let folder = root.join(version);
    std::fs::create_dir_all(&folder)?;
    let exe = folder.join(program());
    // Written beside, then put in place: a cut-off unpack never leaves half a program.
    let part = folder.join(format!("{}.part", program()));
    let written = copy_program(archive, &inside, build, &part);
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written?;
    std::fs::rename(&part, &exe)?;
    mark_downloaded(&exe, from);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(exe)
}

/// [`unpack`]'s copy of the archive's entry `inside` (its exact name) into `part`.
fn copy_program(archive: &Path, inside: &str, build: &Build, part: &Path) -> Result<(), Error> {
    let missing = || Error::Missing(format!("{inside} in the release"));
    let mut out = std::fs::File::create(part)?;
    if build.ext == "zip" {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
            .map_err(|e| Error::BadRelease(e.to_string()))?;
        let mut entry = zip.by_name(inside).map_err(|_| missing())?;
        if !entry.is_file() {
            return Err(missing());
        }
        std::io::copy(&mut entry, &mut out)?;
    } else {
        let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
        let mut tar = tar::Archive::new(gz);
        let mut found = false;
        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.header().entry_type().is_file()
                && entry.path().ok().is_some_and(|p| p == Path::new(inside))
            {
                std::io::copy(&mut entry, &mut out)?;
                found = true;
                break;
            }
        }
        if !found {
            return Err(missing());
        }
    }
    out.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX_SAMPLE: &[u8] = include_bytes!("../tests/fixtures/node/index.json");
    const SUMS_ED25519: &[u8] = include_bytes!("../tests/fixtures/node/v26.13.1-SHASUMS256.txt");
    const SIG_ED25519: &[u8] = include_bytes!("../tests/fixtures/node/v26.13.1-SHASUMS256.txt.sig");
    const SUMS_RSA: &[u8] = include_bytes!("../tests/fixtures/node/v26.13.0-SHASUMS256.txt");
    const SIG_RSA: &[u8] = include_bytes!("../tests/fixtures/node/v26.13.0-SHASUMS256.txt.sig");

    #[test]
    fn each_system_takes_its_own_build() {
        for (os, arch, listed, archive, inside) in [
            (
                "windows",
                "x86_64",
                "win-x64-zip",
                "node-v26.13.1-win-x64.zip",
                "node-v26.13.1-win-x64/node.exe",
            ),
            (
                "windows",
                "aarch64",
                "win-arm64-zip",
                "node-v26.13.1-win-arm64.zip",
                "node-v26.13.1-win-arm64/node.exe",
            ),
            (
                "windows",
                "x86",
                "win-x86-zip",
                "node-v26.13.1-win-x86.zip",
                "node-v26.13.1-win-x86/node.exe",
            ),
            (
                "macos",
                "aarch64",
                "osx-arm64-tar",
                "node-v26.13.1-darwin-arm64.tar.gz",
                "node-v26.13.1-darwin-arm64/bin/node",
            ),
            (
                "macos",
                "x86_64",
                "osx-x64-tar",
                "node-v26.13.1-darwin-x64.tar.gz",
                "node-v26.13.1-darwin-x64/bin/node",
            ),
            (
                "linux",
                "x86_64",
                "linux-x64",
                "node-v26.13.1-linux-x64.tar.gz",
                "node-v26.13.1-linux-x64/bin/node",
            ),
            (
                "linux",
                "aarch64",
                "linux-arm64",
                "node-v26.13.1-linux-arm64.tar.gz",
                "node-v26.13.1-linux-arm64/bin/node",
            ),
        ] {
            let b = build_for(os, arch).unwrap();
            assert_eq!(b.listed, listed, "{os} {arch}");
            assert_eq!(b.archive("26.13.1"), archive);
            assert_eq!(b.program_in("26.13.1"), inside);
        }
        assert_eq!(build_for("linux", "riscv64"), None);
        assert_eq!(build_for("freebsd", "x86_64"), None);
        assert_eq!(available(), build().is_some());
    }

    /// The real checksum lists of two releases appear in the release's real archive names, and
    /// the newest LTS with the build is picked from nodejs.org's list: never a Current or
    /// pre-release, 32-bit Windows the newest LTS line still built for it.
    #[test]
    fn the_newest_lts_with_a_build_is_picked() {
        let win = build_for("windows", "x86_64").unwrap();
        assert_eq!(newest_lts(INDEX_SAMPLE, &win).unwrap(), "26.13.1");
        let x86 = build_for("windows", "x86").unwrap();
        assert_eq!(newest_lts(INDEX_SAMPLE, &x86).unwrap(), "22.23.5");
        for (os, arch) in [("linux", "aarch64"), ("macos", "aarch64")] {
            let b = build_for(os, arch).unwrap();
            assert_eq!(newest_lts(INDEX_SAMPLE, &b).unwrap(), "26.13.1");
            let sums = String::from_utf8_lossy(SUMS_ED25519);
            assert!(sum_for(&sums, &b.archive("26.13.1")).is_some(), "{os}");
        }
        assert!(
            sum_for(
                &String::from_utf8_lossy(SUMS_ED25519),
                &win.archive("26.13.1")
            )
            .is_some()
        );
        // An old release, however listed, is never picked: a list naming only 18 or 24 for
        // 64-bit Windows offers nothing; 32-bit Windows takes nothing older than 22.
        let old =
            br#"[{"version":"v18.20.8","lts":"Hydrogen","files":["win-x64-zip","win-x86-zip"]},
                       {"version":"v24.22.3","lts":"Krypton","files":["win-x64-zip"]}]"#;
        assert!(newest_lts(old, &win).is_err());
        assert!(newest_lts(old, &x86).is_err());
        // Not a list, or none with this build.
        assert!(newest_lts(b"<html>", &win).is_err());
        assert!(
            newest_lts(
                br#"[{"version":"v99.0.0","lts":false,"files":["win-x64-zip"]}]"#,
                &win
            )
            .is_err()
        );
        // A version that could be a path is never picked.
        let odd = br#"[{"version":"v99.2.3/../x","lts":"A","files":["win-x64-zip"]},
                       {"version":"v26.1.0","lts":"Lithium","files":["win-x64-zip"]}]"#;
        assert_eq!(newest_lts(odd, &win).unwrap(), "26.1.0");
    }

    #[test]
    fn versions_are_three_numbers() {
        assert!(plain_version("26.13.1"));
        for v in [
            "",
            "26.13",
            "26.13.1.2",
            "26.13.x",
            "../1.2",
            "1..2",
            "26.13.1-rc",
        ] {
            assert!(!plain_version(v), "{v}");
        }
        assert!(newer("26.13.1", "24.22.3"));
        assert!(!newer("22.23.5", "24.0.0"));
    }

    /// Node.js's real `SHASUMS256.txt` of two releases, one signed with an Ed25519 key and one
    /// with an RSA key, verify against the pinned keys; a changed byte, or another release's
    /// signature, doesn't.
    #[test]
    fn the_checksum_lists_are_signed_by_nodes_releasers() {
        assert!(signed_by_node(SUMS_ED25519, SIG_ED25519));
        assert!(signed_by_node(SUMS_RSA, SIG_RSA));
        assert!(!signed_by_node(SUMS_RSA, SIG_ED25519));
        assert!(!signed_by_node(SUMS_ED25519, SIG_RSA));
        let mut changed = SUMS_ED25519.to_vec();
        changed[10] ^= 1;
        assert!(!signed_by_node(&changed, SIG_ED25519));
        // yt-dlp's key doesn't sign Node.js's lists.
        assert!(!crate::signed_by_ytdlp(SUMS_ED25519, SIG_ED25519));
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::new();
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        for (n, data) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        out
    }

    fn tar_gz_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut t = tar::Builder::new(gz);
        for (n, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            t.append_data(&mut h, n, *data).unwrap();
        }
        t.into_inner().unwrap().finish().unwrap()
    }

    /// Only the program comes out, from either kind of archive; an archive without it is
    /// refused and leaves nothing.
    #[test]
    fn unpacks_the_program_alone() {
        let dir = tempfile::tempdir().unwrap();
        for (b, archive) in [
            (
                build_for("windows", "x86_64").unwrap(),
                zip_with(&[
                    ("node-v26.13.1-win-x64/node.exe", b"node"),
                    ("node-v26.13.1-win-x64/npm.cmd", b"npm"),
                ]),
            ),
            (
                build_for("linux", "x86_64").unwrap(),
                tar_gz_with(&[
                    ("node-v26.13.1-linux-x64/bin/npm", b"npm"),
                    ("node-v26.13.1-linux-x64/bin/node", b"node"),
                ]),
            ),
        ] {
            let root = dir.path().join(b.platform);
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("a");
            std::fs::write(&path, &archive).unwrap();
            let exe = unpack(&path, &root, "26.13.1", &b, "https://x/").unwrap();
            assert_eq!(exe, root.join("26.13.1").join(program()));
            assert_eq!(std::fs::read(&exe).unwrap(), b"node");
            let left: Vec<_> = std::fs::read_dir(exe.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(left.len(), 1, "{left:?}");
            assert!(unpack(&path, &root, "../x", &b, "https://x/").is_err());
            // Another version's archive: its program isn't where this one's is.
            assert!(matches!(
                unpack(&path, &root, "26.13.2", &b, "https://x/"),
                Err(Error::Missing(_))
            ));
            assert!(!root.join("26.13.2").join(program()).exists());
        }
    }

    /// The whole install against a release served on a lab server as nodejs.org serves it: the
    /// list, the signed checksum list (a test's "signature" here), the archive checked against
    /// it, unpacked, made current; the same release again is not downloaded twice; an unsigned
    /// list, or an archive that isn't the one listed, is refused and nothing changes.
    #[tokio::test]
    async fn installs_from_a_release_checked_against_its_signed_sums() {
        let Some(b) = build() else {
            return; // no Node.js build for this system
        };
        let version = "26.13.1";
        let inside = b.program_in(version);
        let archive = if b.ext == "zip" {
            zip_with(&[(&inside, b"a Node.js, as far as this test goes")])
        } else {
            tar_gz_with(&[(&inside, b"a Node.js, as far as this test goes")])
        };
        let name = b.archive(version);
        let sum = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&archive));
        let files = tempfile::tempdir().unwrap();
        let rel = files.path().join(format!("v{version}"));
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::write(rel.join(&name), &archive).unwrap();
        std::fs::write(rel.join("SHASUMS256.txt"), format!("{sum}  {name}\n")).unwrap();
        std::fs::write(rel.join("SHASUMS256.txt.sig"), b"good").unwrap();
        std::fs::write(
            files.path().join("index.json"),
            format!(
                r#"[{{"version":"v{version}","lts":"Lithium","files":["{}"]}}]"#,
                b.listed
            ),
        )
        .unwrap();
        let lab = mdm_labserver::LabServer::start(mdm_labserver::Config {
            files: Some(files.path().to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
        let net = crate::net(mdm_engine::Proxy::None);
        let (index, dist) = (lab.url("/files/index.json"), lab.url("/files/"));
        let source = |signed: fn(&[u8], &[u8]) -> bool| Source {
            index: &index,
            dist: &dist,
            signed,
        };
        let dir = tempfile::tempdir().unwrap();
        let unsigned = install_from(dir.path(), &net, None, None, &source(|_, _| false)).await;
        assert!(matches!(unsigned, Err(Error::BadSignature)), "{unsigned:?}");
        assert_eq!(installed(dir.path()), None);
        let good = |_: &[u8], sig: &[u8]| sig == b"good";
        let got = match install_from(dir.path(), &net, None, None, &source(good))
            .await
            .unwrap()
            .settled
        {
            Some(crate::autoupdate::Settled::Switched(i)) => i,
            other => panic!("{other:?}"),
        };
        assert_eq!(got.version, version);
        assert_eq!(
            got.exe,
            dir.path().join("node").join(version).join(program())
        );
        assert_eq!(
            std::fs::read(&got.exe).unwrap(),
            b"a Node.js, as far as this test goes"
        );
        assert_eq!(installed(dir.path()), Some(got.clone()));
        // The same again: nothing downloaded, nothing changed; the archive itself isn't kept.
        let again = install_from(dir.path(), &net, None, None, &source(good))
            .await
            .unwrap();
        assert!(again.settled.is_none());
        let left: Vec<_> = std::fs::read_dir(dir.path().join("node"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|n| !n.starts_with("node-")), "{left:?}");
        // A newer release whose archive isn't the one its list names: refused, the installed one
        // stays.
        let newer_v = "26.14.0";
        let rel2 = files.path().join(format!("v{newer_v}"));
        std::fs::create_dir_all(&rel2).unwrap();
        std::fs::write(rel2.join(b.archive(newer_v)), b"not the listed archive").unwrap();
        std::fs::write(
            rel2.join("SHASUMS256.txt"),
            format!("{sum}  {}\n", b.archive(newer_v)),
        )
        .unwrap();
        std::fs::write(rel2.join("SHASUMS256.txt.sig"), b"good").unwrap();
        std::fs::write(
            files.path().join("index.json"),
            format!(
                r#"[{{"version":"v{newer_v}","lts":"Lithium","files":["{}"]}}]"#,
                b.listed
            ),
        )
        .unwrap();
        let bad = install_from(dir.path(), &net, None, None, &source(good)).await;
        assert!(matches!(bad, Err(Error::BadChecksum)), "{bad:?}");
        assert_eq!(installed(dir.path()), Some(got.clone()));
        // An older LTS listed, below the oldest MDM takes on every build (20 is older than 32-bit
        // Windows' own oldest, 22, which 24 isn't): nothing is offered at all, never a downgrade.
        std::fs::write(
            files.path().join("index.json"),
            format!(
                r#"[{{"version":"v20.19.0","lts":"Iron","files":["{}"]}}]"#,
                b.listed
            ),
        )
        .unwrap();
        let older = install_from(dir.path(), &net, None, None, &source(good)).await;
        assert!(matches!(older, Err(Error::Missing(_))), "{older:?}");
        assert_eq!(installed(dir.path()), Some(got));
    }
}
