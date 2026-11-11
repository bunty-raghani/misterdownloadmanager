//! Helpers installed by a Unix package manager or Homebrew (#452).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::Installed;

/// The usual package-manager folders (Homebrew on Apple silicon and on Intel, then the system's):
/// an app opened from the Dock or Finder may not inherit the shell's PATH, so helpers are looked
/// for there too, and a helper found there gets them on its PATH for its own tools (#659).
pub(crate) const PACKAGE_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

/// PATH first, then [`PACKAGE_DIRS`]. Discovery only reads metadata, never starts a helper on
/// the UI thread.
pub(crate) fn installed(name: &str) -> Option<Installed> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_in(
        name,
        std::env::split_paths(&path).chain(PACKAGE_DIRS.map(PathBuf::from)),
    )
    .map(|exe| Installed {
        version: crate::SYSTEM.into(),
        exe,
    })
}

fn find_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    dirs.into_iter()
        // Only absolute folders count: an empty or relative PATH entry names no fixed place.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|exe| runnable(exe))
}

fn runnable(exe: &Path) -> bool {
    // metadata follows package-manager symlinks and rejects broken ones.
    exe.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_non_executable_files_and_directories_are_not_helpers() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
        let exe = root.path().join("yt-dlp");
        std::fs::write(&exe, b"not executable").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
        std::fs::remove_file(&exe).unwrap();
        std::fs::create_dir(&exe).unwrap();
        assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
    }

    #[test]
    fn only_absolute_folders_count() {
        // Beside the working directory, so that the relative path really reaches a helper.
        let root = tempfile::tempdir_in(".").unwrap();
        let exe = root.path().join("yt-dlp");
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let relative = PathBuf::from(root.path().file_name().unwrap());
        assert!(relative.is_relative() && runnable(&relative.join("yt-dlp")));
        assert_eq!(find_in("yt-dlp", [relative.clone()]), None);
        assert_eq!(find_in("yt-dlp", [PathBuf::new()]), None, "an empty entry");

        let absolute = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            find_in("yt-dlp", [PathBuf::new(), relative, absolute.clone()]),
            Some(absolute.join("yt-dlp"))
        );
    }

    #[test]
    fn the_first_runnable_helper_wins() {
        let root = tempfile::tempdir().unwrap();
        let dirs = [root.path().join("first"), root.path().join("second")];
        for dir in &dirs {
            std::fs::create_dir(dir).unwrap();
            let exe = dir.join("yt-dlp");
            std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            find_in("yt-dlp", dirs),
            Some(root.path().join("first/yt-dlp"))
        );
    }
}
