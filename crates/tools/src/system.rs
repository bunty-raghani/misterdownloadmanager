//! Helpers installed by a package manager, an installer or the person (#452), on every system:
//! yt-dlp from pip, winget, scoop, choco or custom folders; ffmpeg from winget, choco or system;
//! gallery-dl from pip or package managers; JS runtimes (deno, bun, node); bgutil-pot provider.
//!
//! Windows keeps no copy of its own to find: whatever an installer, package manager (winget, scoop,
//! choco) or pip put on the PATH or standard directories is found here, as a Unix package manager's
//! or Homebrew's copy is.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::Installed;

/// The usual package-manager folders (Homebrew on Apple silicon and on Intel, then the system's):
/// an app opened from the Dock or Finder may not inherit the shell's PATH, so helpers are looked
/// for there too, and a helper found there gets them on its PATH for its own tools (#659).
#[cfg(unix)]
pub(crate) const PACKAGE_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

/// Folders beyond the PATH to look in, and to give a helper for its own tools:
///
/// - On Unix: the package managers' ([`PACKAGE_DIRS`]), Flatpak (`~/.local/share/flatpak/exports/bin`,
///   `/var/lib/flatpak/exports/bin`), Nix (`~/.nix-profile/bin`, `/nix/var/nix/profiles/default/bin`),
///   and user tool directories (`~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin`, `~/.deno/bin`).
/// - On Windows: winget links (`%LOCALAPPDATA%\Microsoft\WinGet\Links`), the person's own Scoop
///   shims (`%USERPROFILE%\scoop\shims`), tool managers (`%USERPROFILE%\.cargo\bin`,
///   `%USERPROFILE%\.bun\bin`, `%USERPROFILE%\.deno\bin`), pip Scripts folders
///   (`%LOCALAPPDATA%\Python\*\Scripts`, `%APPDATA%\Python\Python*\Scripts`), and Chocolatey's
///   and global Scoop's own places, read from `%ChocolateyInstall%` and `%SCOOP_GLOBAL%` (set
///   by their installers, so they say the program is really there). No fixed folder anyone can
///   create (`C:\yt-dlp`, the default `C:\ProgramData\chocolatey\bin`, `%ALLUSERSPROFILE%\scoop\shims`):
///   a program MDM runs just because it exists there is too easy to plant.
pub(crate) fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(unix)]
    {
        for p in PACKAGE_DIRS {
            dirs.push(PathBuf::from(p));
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".cargo/bin"));
            dirs.push(home.join(".bun/bin"));
            dirs.push(home.join(".deno/bin"));
            dirs.push(home.join(".local/share/flatpak/exports/bin"));
            dirs.push(home.join(".nix-profile/bin"));
        }
        dirs.push(PathBuf::from("/var/lib/flatpak/exports/bin"));
        dirs.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
        dirs.push(PathBuf::from("/bin"));
    }
    #[cfg(windows)]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);

        // WinGet links (ffmpeg, yt-dlp, etc.)
        if let Some(lad) = &local_app_data {
            dirs.push(lad.join(r"Microsoft\WinGet\Links"));
        }

        // The person's own Scoop shims and developer runtimes (.cargo, .bun, .deno)
        if let Some(up) = &user_profile {
            dirs.push(up.join(r"scoop\shims"));
            dirs.push(up.join(r".cargo\bin"));
            dirs.push(up.join(r".bun\bin"));
            dirs.push(up.join(r".deno\bin"));
        }
        // Chocolatey and global Scoop: only the places their own installers name. Their default
        // folders under ProgramData are left out: on a machine without them, any user can create
        // those (ProgramData lets ordinary users make folders), the same plant-it problem as a
        // fixed folder at a drive's root; a real install puts its bin on the machine PATH anyway.
        if let Some(choco) = std::env::var_os("ChocolateyInstall") {
            dirs.push(Path::new(&choco).join("bin"));
        }
        if let Some(scoop_global) = std::env::var_os("SCOOP_GLOBAL") {
            dirs.push(Path::new(&scoop_global).join("shims"));
        }

        // Python Scripts (pip install --user yt-dlp, gallery-dl)
        if let Some(lad) = &local_app_data {
            if let Ok(entries) = std::fs::read_dir(lad.join("Python")) {
                for entry in entries.flatten() {
                    let scripts = entry.path().join("Scripts");
                    if scripts.is_dir() {
                        dirs.push(scripts);
                    }
                }
            }
            if let Ok(entries) = std::fs::read_dir(lad.join(r"Programs\Python")) {
                for entry in entries.flatten() {
                    let scripts = entry.path().join("Scripts");
                    if scripts.is_dir() {
                        dirs.push(scripts);
                    }
                }
            }
        }
        if let Some(appdata) = &std::env::var_os("APPDATA").map(PathBuf::from)
            && let Ok(entries) = std::fs::read_dir(appdata.join("Python"))
        {
            for entry in entries.flatten() {
                let scripts = entry.path().join("Scripts");
                if scripts.is_dir() {
                    dirs.push(scripts);
                }
            }
        }
    }
    dirs
}

/// PATH first, then [`extra_dirs`]. Discovery only reads metadata, never starts a helper on
/// the UI thread.
pub(crate) fn installed(name: &str) -> Option<Installed> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_in(name, std::env::split_paths(&path).chain(extra_dirs())).map(|exe| Installed {
        version: crate::SYSTEM.into(),
        exe,
    })
}

fn find_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    dirs.into_iter()
        // Only absolute folders count: an empty or relative PATH entry names no fixed place.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program_name(name)))
        .find(|exe| runnable(exe))
}

/// `yt-dlp.exe` on Windows, `yt-dlp` elsewhere.
pub(crate) fn program_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

#[cfg(windows)]
fn runnable(exe: &Path) -> bool {
    // metadata follows the links winget's Links folder is made of, and rejects a broken one.
    exe.metadata().is_ok_and(|m| m.is_file())
}

#[cfg(unix)]
fn runnable(exe: &Path) -> bool {
    // metadata follows package-manager symlinks and rejects broken ones.
    exe.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe_name(name: &str) -> String {
        program_name(name)
    }

    /// On Unix a file without the execute bits is no helper; on Windows a plain file runs, so
    /// the non-helper there is a folder of the program's name.
    #[test]
    fn missing_or_non_executable_files_and_directories_are_not_helpers() {
        let root = tempfile::tempdir().unwrap();
        let name = exe_name("yt-dlp");
        assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
        let exe = root.path().join(&name);
        std::fs::write(&exe, b"not a program").unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
            std::fs::remove_file(&exe).unwrap();
        }
        #[cfg(windows)]
        {
            assert_eq!(
                find_in("yt-dlp", [root.path().to_owned()]),
                Some(exe.clone()),
                "a plain file runs on Windows"
            );
            std::fs::remove_file(&exe).unwrap();
        }
        std::fs::create_dir(&exe).unwrap();
        assert_eq!(find_in("yt-dlp", [root.path().to_owned()]), None);
    }

    #[test]
    fn only_absolute_folders_count() {
        // Beside the working directory, so that the relative path really reaches a helper.
        let root = tempfile::tempdir_in(".").unwrap();
        let name = exe_name("yt-dlp");
        let exe = root.path().join(&name);
        std::fs::write(&exe, b"not a program").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let relative = PathBuf::from(root.path().file_name().unwrap());
        assert!(relative.is_relative() && runnable(&relative.join(&name)));
        assert_eq!(find_in("yt-dlp", [relative.clone()]), None);
        assert_eq!(find_in("yt-dlp", [PathBuf::new()]), None, "an empty entry");

        let absolute = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            find_in("yt-dlp", [PathBuf::new(), relative, absolute.clone()]),
            Some(absolute.join(name))
        );
    }

    #[test]
    fn the_first_runnable_helper_wins() {
        let root = tempfile::tempdir().unwrap();
        let dirs = [root.path().join("first"), root.path().join("second")];
        for dir in &dirs {
            std::fs::create_dir(dir).unwrap();
            let exe = dir.join(exe_name("yt-dlp"));
            std::fs::write(&exe, b"not a program").unwrap();
            #[cfg(unix)]
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            find_in("yt-dlp", dirs),
            Some(root.path().join("first").join(exe_name("yt-dlp")))
        );
    }

    #[test]
    fn the_name_gets_the_platforms_executable_suffix() {
        assert_eq!(
            program_name("yt-dlp"),
            format!("yt-dlp{}", std::env::consts::EXE_SUFFIX)
        );
    }
}
