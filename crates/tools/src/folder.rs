//! yt-dlp's folder build on Windows and macOS, unpacked once by MDM.
//!
//! yt-dlp publishes each of these systems' programs twice. The one-file builds (`yt-dlp.exe`,
//! `yt-dlp_macos`) unpack their Python into a new temporary folder at every start:
//! - on Windows that is most of each run's start (#1053): about 1.3 s before it answers
//!   `--version`, against 0.5 s for the folder build, and as much more on every look-up of the
//!   browser's video button;
//! - on macOS (#330, #339), an app still marked as downloaded by the browser passes that mark to
//!   what it and its children create, so the Python just unpacked carries it, and macOS refuses to
//!   load the library ("Python.framework is damaged", PYI-…): every look-up failed. MDM has no
//!   hand in that temporary folder.
//!
//! The folder builds (`yt-dlp_win.zip`, `yt-dlp_macos.zip`) unpack nothing when they run: MDM
//! unpacks one once, after checking the archive against yt-dlp's signed checksum list, into its
//! own tools folder. On macOS the mark is taken off those files and no others: never the app,
//! never a download (they are marked on purpose: `mark_downloaded`), never a program the person
//! chose; on Windows the program is marked as downloaded, as the one-file build was. Linux keeps
//! the single program for now (its start there wasn't measured for #1053, and a system's own
//! yt-dlp, unpacked by its package manager, is used there when MDM has none) and doesn't compile
//! the install (`#[cfg]` below); the unpacking is portable and tested everywhere.

use std::path::{Component, Path};

#[cfg(test)]
use crate::make_current;
use crate::{Error, Installed};

/// Whether this system installs the folder build.
pub(crate) const BUILD: bool = cfg!(any(windows, target_os = "macos"));
/// The folder build's program, beside its `_internal` folder: each Windows archive names it for
/// its processor (`yt-dlp_win_arm64.zip` holds `yt-dlp_arm64.exe`, `yt-dlp_win_x86.zip`
/// `yt-dlp_x86.exe`; checked against yt-dlp 2026.10.26's archives).
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) const PROGRAM: &str = "yt-dlp.exe";
#[cfg(all(windows, target_arch = "aarch64"))]
pub(crate) const PROGRAM: &str = "yt-dlp_arm64.exe";
#[cfg(all(windows, target_arch = "x86"))]
pub(crate) const PROGRAM: &str = "yt-dlp_x86.exe";
#[cfg(not(windows))]
pub(crate) const PROGRAM: &str = "yt-dlp_macos";
/// What the folder build keeps beside its program; the one-file build has none.
const INTERNAL: &str = "_internal";

fn bad(e: impl std::fmt::Display) -> Error {
    Error::BadRelease(e.to_string())
}

/// Whether `exe` is the one-file build, which earlier versions of MDM installed: nothing beside
/// it. MDM gets the folder build in its place, the same release too (`install_latest`); on
/// macOS, where it can't run, it is treated as not installed meanwhile.
pub(crate) fn one_file_build(exe: &Path) -> bool {
    !exe.with_file_name(INTERNAL).is_dir()
}

/// Install the verified archive `zip`, downloaded `from` there, as `version` of yt-dlp
/// (`dir/yt-dlp/<version>/`; beside a folder of that name already there, the one-file build it
/// replaces), macOS's mark taken off the files just unpacked, and make it current. Unpacked to
/// one side first, so a failure leaves what was installed.
#[cfg(test)]
pub(crate) fn put_folder(
    dir: &Path,
    version: &str,
    zip: &[u8],
    from: &str,
) -> Result<Installed, Error> {
    let placed = place_folder(dir, version, zip, from)?;
    make_current(&dir.join("yt-dlp"), placed)
}

/// [`put_folder`] without making it current: `autoupdate::settle` does, now or once the runs
/// using yt-dlp end.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn place_folder(
    dir: &Path,
    version: &str,
    zip: &[u8],
    from: &str,
) -> Result<Installed, Error> {
    if version.is_empty() || version.contains(['/', '\\', ':']) || version.starts_with('.') {
        return Err(Error::BadRelease(format!("version {version:?}")));
    }
    let root = dir.join("yt-dlp");
    let unpacking = root.join(format!(".unpacking-{version}"));
    let _ = std::fs::remove_dir_all(&unpacking);
    std::fs::create_dir_all(&unpacking)?;
    let placed = unpack(zip, &unpacking)
        .and_then(|()| {
            if unpacking.join(PROGRAM).is_file() && unpacking.join(INTERNAL).is_dir() {
                Ok(())
            } else {
                Err(Error::Missing(format!(
                    "{PROGRAM} in the release's archive"
                )))
            }
        })
        .and_then(|()| {
            clear_mark(&unpacking);
            crate::mark_downloaded(&unpacking.join(PROGRAM), from);
            // The same version again: beside the one-file build it replaces (`one_file_build`),
            // which may be running a look-up now (Windows can't remove a running program); it
            // stays as the previous version, and goes with the next one (`make_current`).
            let folder = (1..=20)
                .map(|n| match n {
                    1 => root.join(version),
                    n => root.join(format!("{version}-{n}")),
                })
                .find(|f| !f.exists())
                .ok_or_else(|| bad(format!("no free folder for {version}")))?;
            std::fs::rename(&unpacking, &folder)?;
            Ok(folder)
        });
    let folder = match placed {
        Ok(folder) => folder,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&unpacking);
            return Err(e);
        }
    };
    Ok(Installed {
        version: version.to_owned(),
        exe: folder.join(PROGRAM),
    })
}

/// Unpack `zip` into `dest`, keeping the files' permissions. An entry that would land outside
/// `dest`, by its name or by a link, refuses the whole archive.
fn unpack(zip: &[u8], dest: &Path) -> Result<(), Error> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(bad)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(bad)?;
        let Some(name) = entry.enclosed_name() else {
            return Err(bad("unsafe name in the archive"));
        };
        let path = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        if entry.is_symlink() {
            use std::io::Read;
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            if !link_stays_inside(&name, Path::new(&target)) {
                return Err(bad(format!("unsafe link in the archive: {target}")));
            }
            std::os::unix::fs::symlink(&target, &path)?;
            continue;
        }
        let mut out = std::fs::File::create(&path)?;
        std::io::copy(&mut entry, &mut out)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// Whether a link at `name` (relative to the archive's root) pointing at `target` stays within
/// the archive.
#[cfg_attr(not(unix), allow(dead_code))]
fn link_stays_inside(name: &Path, target: &Path) -> bool {
    let mut depth = name.components().count().saturating_sub(1);
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return false,
        }
    }
    true
}

/// macOS: take the mark off the verified files just unpacked in `folder` (recursively, links not
/// followed). The program is the system's own, not found through `PATH`. Whatever it says, the
/// install goes on: a file without the mark is fine, and a mark left behind makes the run fail
/// with its own message.
#[cfg(target_os = "macos")]
fn clear_mark(folder: &Path) {
    let _ = std::process::Command::new("/usr/bin/xattr")
        .args(["-drs", "com.apple.quarantine"])
        .arg(folder)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Elsewhere no such mark exists on these files.
#[cfg(not(target_os = "macos"))]
fn clear_mark(_folder: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// An archive with the folder build's layout: `files` are (name, contents, mode).
    fn archive(files: &[(&str, &[u8], u32)], links: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data, mode) in files {
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        for (name, target) in links {
            zip.add_symlink(*name, *target, zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn good() -> Vec<u8> {
        archive(
            &[
                (PROGRAM, b"program", 0o755),
                ("_internal/Python", b"library", 0o644),
                ("_internal/lib/data.txt", b"data", 0o644),
            ],
            &[],
        )
    }

    #[test]
    fn the_folder_build_is_unpacked_as_one_and_made_current() {
        let dir = tempfile::tempdir().unwrap();
        let now = put_folder(dir.path(), "2026.10.26", &good(), "https://x/").unwrap();
        assert_eq!(now.version, "2026.10.26");
        assert_eq!(
            now.exe,
            dir.path().join("yt-dlp/2026.10.26").join(PROGRAM),
            "beside its folder, under its version"
        );
        assert_eq!(std::fs::read(&now.exe).unwrap(), b"program");
        assert_eq!(
            std::fs::read(now.exe.with_file_name("_internal/Python")).unwrap(),
            b"library"
        );
        assert!(!one_file_build(&now.exe));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&now.exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the program runs");
        }
        // Nothing left to one side.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("yt-dlp"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".unpacking"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn the_same_version_replaces_the_one_file_build() {
        let dir = tempfile::tempdir().unwrap();
        // Earlier versions of MDM: a single program, nothing beside it.
        let old = crate::put(
            dir.path(),
            "yt-dlp",
            "2026.10.26",
            PROGRAM,
            b"old one-file program",
            "https://x/",
        )
        .unwrap();
        assert!(one_file_build(&old.exe));
        let now = put_folder(dir.path(), "2026.10.26", &good(), "https://x/").unwrap();
        // Beside it (Windows can't remove it while a look-up runs it), and current.
        let root = dir.path().join("yt-dlp");
        assert_eq!(now.exe, root.join("2026.10.26-2").join(PROGRAM));
        assert_eq!(now.version, "2026.10.26");
        assert_eq!(std::fs::read(&now.exe).unwrap(), b"program");
        assert!(!one_file_build(&now.exe));
        assert_eq!(crate::installed(dir.path()).unwrap(), now);
        assert_eq!(
            std::fs::read(&old.exe).unwrap(),
            b"old one-file program",
            "the one-file build stays as the previous version"
        );
        // The next release: in its own folder; the folder build stays as the previous one, and
        // the one-file build goes.
        let next = put_folder(dir.path(), "2026.11.02", &good(), "https://x/").unwrap();
        assert_eq!(next.exe, root.join("2026.11.02").join(PROGRAM));
        assert!(now.exe.is_file(), "the previous version stays");
        assert!(!root.join("2026.10.26").exists(), "older ones go");
    }

    #[test]
    fn an_install_s_folder_is_its_own_when_its_version_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("yt-dlp");
        for taken in ["2026.10.26", "2026.10.26-2"] {
            std::fs::create_dir_all(root.join(taken)).unwrap();
            std::fs::write(root.join(taken).join("kept"), b"x").unwrap();
        }
        let now = put_folder(dir.path(), "2026.10.26", &good(), "https://x/").unwrap();
        assert_eq!(now.exe, root.join("2026.10.26-3").join(PROGRAM));
        // Neither the new nor a previous version: gone with the install.
        assert!(!root.join("2026.10.26").exists());
        assert!(!root.join("2026.10.26-2").exists());
    }

    #[test]
    fn an_archive_without_the_program_installs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let zip = archive(&[("_internal/Python", b"x", 0o644)], &[]);
        assert!(put_folder(dir.path(), "2026.10.26", &zip, "https://x/").is_err());
        assert!(!dir.path().join("yt-dlp/current.json").exists());
        assert!(!dir.path().join("yt-dlp/2026.10.26").exists());
        assert!(!dir.path().join("yt-dlp/.unpacking-2026.10.26").exists());
    }

    #[test]
    fn nothing_lands_outside_the_install() {
        let dir = tempfile::tempdir().unwrap();
        let zip = archive(
            &[
                (PROGRAM, b"p", 0o755),
                ("_internal/ok", b"y", 0o644),
                ("../escaped", b"x", 0o644),
            ],
            &[],
        );
        assert!(put_folder(dir.path(), "2026.10.26", &zip, "https://x/").is_err());
        assert!(!dir.path().join("yt-dlp/escaped").exists());
        assert!(!dir.path().join("escaped").exists());
        assert!(put_folder(dir.path(), "../evil", &good(), "https://x/").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_are_kept_and_links_out_refuse_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let inside = archive(
            &[
                (PROGRAM, b"p", 0o755),
                ("_internal/Versions/1/Python", b"lib", 0o644),
            ],
            &[("_internal/Python", "Versions/1/Python")],
        );
        let now = put_folder(dir.path(), "1", &inside, "https://x/").unwrap();
        assert_eq!(
            std::fs::read(now.exe.with_file_name("_internal/Python")).unwrap(),
            b"lib"
        );
        for target in ["/etc/passwd", "../../outside", "../../../outside"] {
            let out = archive(
                &[(PROGRAM, b"p", 0o755), ("_internal/x", b"x", 0o644)],
                &[("_internal/link", target)],
            );
            assert!(
                put_folder(dir.path(), "2", &out, "https://x/").is_err(),
                "{target} was accepted"
            );
        }
        assert!(link_stays_inside(
            Path::new("a/b/link"),
            Path::new("../c/file")
        ));
        assert!(!link_stays_inside(
            Path::new("link"),
            Path::new("../outside")
        ));
    }

    /// The mark comes off the unpacked files only (macOS, where `xattr` exists).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mark_comes_off_what_was_unpacked_and_nothing_else() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let mark = |p: &Path| {
            let status = Command::new("/usr/bin/xattr")
                .args(["-w", "com.apple.quarantine", "0083;00000000;Test;"])
                .arg(p)
                .status()
                .unwrap();
            assert!(status.success());
        };
        let marked = |p: &Path| {
            Command::new("/usr/bin/xattr")
                .args(["-p", "com.apple.quarantine"])
                .arg(p)
                .output()
                .unwrap()
                .status
                .success()
        };
        // What a marked app leaves on what it creates, and a file that must keep its mark.
        let folder = dir.path().join("unpacked");
        std::fs::create_dir_all(folder.join("_internal")).unwrap();
        let (program, lib) = (folder.join(PROGRAM), folder.join("_internal/Python"));
        std::fs::write(&program, b"p").unwrap();
        std::fs::write(&lib, b"l").unwrap();
        let elsewhere = dir.path().join("a download");
        std::fs::write(&elsewhere, b"d").unwrap();
        for p in [&program, &lib, &elsewhere] {
            mark(p);
            assert!(marked(p));
        }
        clear_mark(&folder);
        assert!(!marked(&program));
        assert!(!marked(&lib));
        assert!(marked(&elsewhere), "only the helper's own folder");
    }
}
