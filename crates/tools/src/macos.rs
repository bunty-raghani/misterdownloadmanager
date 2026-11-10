//! yt-dlp on macOS (#330, #339): its folder build, unpacked once by MDM.
//!
//! yt-dlp publishes two macOS programs. The one-file `yt-dlp_macos` unpacks its Python into a new
//! temporary folder at every start. An app still marked as downloaded by the browser passes that
//! mark to what it and its children create, so the Python just unpacked carries it, and macOS
//! refuses to load the library ("Python.framework is damaged", PYI-…): every
//! look-up failed. MDM has no hand in that temporary folder.
//!
//! The folder build (`yt-dlp_macos.zip`) unpacks nothing when it runs: MDM unpacks it once, after
//! checking the archive against yt-dlp's signed checksum list, into its own tools folder. The
//! mark is taken off those files and no others: never the app, never a download (they are marked
//! on purpose: `mark_downloaded`), never a program the person chose. Linux and Windows use the
//! single-program builds and don't compile the install (`#[cfg]` below); the unpacking is
//! portable and tested everywhere.

use std::path::{Component, Path};

use crate::{Error, Installed, make_current};

/// The folder build's program, beside its `_internal` folder.
pub(crate) const PROGRAM: &str = "yt-dlp_macos";
/// What the folder build keeps beside its program; the one-file build has none.
const INTERNAL: &str = "_internal";

fn bad(e: impl std::fmt::Display) -> Error {
    Error::BadRelease(e.to_string())
}

/// Whether `exe` is the one-file build, which earlier versions of MDM installed: nothing beside
/// it. It is treated as not installed, so MDM gets the folder build in its place.
pub(crate) fn one_file_build(exe: &Path) -> bool {
    !exe.with_file_name(INTERNAL).is_dir()
}

/// Install the verified archive `zip` as `version` of yt-dlp (`dir/yt-dlp/<version>/`), the mark
/// taken off the files just unpacked, and make it current. Unpacked to one side first, so a
/// failure leaves what was installed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn put_folder(dir: &Path, version: &str, zip: &[u8]) -> Result<Installed, Error> {
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
            let folder = root.join(version);
            // The same version again: the one-file build it replaces (`one_file_build`).
            if folder.exists() {
                std::fs::remove_dir_all(&folder)?;
            }
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
    make_current(
        &root,
        Installed {
            version: version.to_owned(),
            exe: folder.join(PROGRAM),
        },
    )
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
        let now = put_folder(dir.path(), "2026.10.26", &good()).unwrap();
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
        let now = put_folder(dir.path(), "2026.10.26", &good()).unwrap();
        assert_eq!(now.exe, old.exe, "the same place");
        assert_eq!(std::fs::read(&now.exe).unwrap(), b"program");
        assert!(!one_file_build(&now.exe));
    }

    #[test]
    fn an_archive_without_the_program_installs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let zip = archive(&[("_internal/Python", b"x", 0o644)], &[]);
        assert!(put_folder(dir.path(), "2026.10.26", &zip).is_err());
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
        assert!(put_folder(dir.path(), "2026.10.26", &zip).is_err());
        assert!(!dir.path().join("yt-dlp/escaped").exists());
        assert!(!dir.path().join("escaped").exists());
        assert!(put_folder(dir.path(), "../evil", &good()).is_err());
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
        let now = put_folder(dir.path(), "1", &inside).unwrap();
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
                put_folder(dir.path(), "2", &out).is_err(),
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
