//! The Deno earlier versions of MDM installed for yt-dlp (#1448, #1470), in `tools/deno/`. MDM's
//! own JavaScript runtime is Node.js now (#1570, `node`): a Deno MDM installed keeps being used
//! until its Node.js is in place, then it is removed ([`remove_own`], from
//! `jsruntime::retire_deno`). Only that folder, which MDM alone makes: a Deno the person
//! installed (on the PATH, in `~/.deno`, chosen in Helpers) is theirs and never touched.

use std::path::Path;

use crate::Installed;

/// MDM's own Deno in the tools folder `dir` (`dir/deno/current.json`), if any and still there.
pub fn installed(dir: &Path) -> Option<Installed> {
    crate::autoupdate::current_in(&dir.join("deno")).filter(|i| i.exe.starts_with(dir.join("deno")))
}

/// Remove `dir/deno` when nothing uses that Deno (a look-up or a download holds it,
/// `autoupdate::Hold`) and nothing installs it; true once it is gone. A folder without MDM's
/// `current.json` isn't one MDM made: left alone.
pub(crate) fn remove_own(dir: &Path) -> bool {
    let root = dir.join("deno");
    if !root.join("current.json").is_file() {
        return false;
    }
    let Ok(_installing) = crate::autoupdate::installing(&root, "deno") else {
        return false;
    };
    let Some(_alone) = crate::autoupdate::alone(&root) else {
        tracing::info!("MDM's own Deno is in use: removed once its runs end");
        return false;
    };
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {
            tracing::info!("MDM's own Deno removed: its Node.js takes its place");
            true
        }
        Err(e) => {
            tracing::warn!("MDM's own Deno: couldn't remove {}: {e}", root.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installed as `current.json` names it, only while the program is there and in its folder.
    #[test]
    fn the_installed_copy_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(installed(dir.path()), None);
        let root = dir.path().join("deno");
        let exe = root
            .join("2.9.8")
            .join(format!("deno{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"deno").unwrap();
        let now = crate::make_current(
            &root,
            Installed {
                version: "2.9.8".into(),
                exe: exe.clone(),
            },
        )
        .unwrap();
        assert_eq!(installed(dir.path()), Some(now));
        std::fs::remove_file(&exe).unwrap();
        assert_eq!(installed(dir.path()), None);
    }

    /// Only a folder MDM made goes, and not while a run holds it.
    #[test]
    fn only_mdms_own_unused_deno_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("deno");
        // A folder without current.json: not MDM's install.
        std::fs::create_dir_all(root.join("mine")).unwrap();
        std::fs::write(root.join("mine").join("deno"), b"x").unwrap();
        assert!(!remove_own(dir.path()));
        assert!(root.join("mine").join("deno").is_file());
        std::fs::remove_dir_all(&root).unwrap();
        let exe = root
            .join("2.9.8")
            .join(format!("deno{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"deno").unwrap();
        crate::make_current(
            &root,
            Installed {
                version: "2.9.8".into(),
                exe: exe.clone(),
            },
        )
        .unwrap();
        // A look-up uses it: kept.
        let hold = crate::autoupdate::Hold::of([exe.as_path()]);
        assert_eq!(hold.tools().len(), 0, "Deno is no tool MDM updates");
        assert!(hold.holds(&root));
        assert!(!remove_own(dir.path()));
        assert!(exe.is_file());
        drop(hold);
        assert!(remove_own(dir.path()));
        assert!(!root.exists());
    }
}
