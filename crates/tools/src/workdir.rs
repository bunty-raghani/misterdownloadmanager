//! Where the helpers run (#834): every one starts in a folder it can write, with TEMP, TMP and
//! TMPDIR naming that folder.
//!
//! A helper used to inherit MDM's own working folder, which is the install folder when MDM is
//! opened from the Start menu (`C:\Program Files\Mister Download Manager`). yt-dlp writes there:
//! before it chooses a format a site marks as needing a check (YouTube's streams in pieces of a
//! video put online in the last two days), it tries the format with a small file of its own in
//! its working folder (its `temp` path, which MDM doesn't set), and Windows refused it: "[Errno
//! 13] Permission denied: 'C:\\Program Files\\Mister Download Manager\\tmp….tmp'".
//!
//! The folder is the person's temporary folder when a file can be made in it, else a `work`
//! folder in MDM's data folder (said once in the log): Python, yt-dlp's and gallery-dl's
//! language, falls back to the working folder when TEMP can't be written, and so would have
//! landed in the install folder again. MDM's own files for the helpers (a page's cookies, a
//! looked-up page) go in the same folder ([`temp_dir`]).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder the helpers run in and are given as their temporary folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folders {
    /// Their working folder and TEMP, TMP and TMPDIR; `None` when no folder could be written at
    /// all (they then get what MDM has, as before).
    pub dir: Option<PathBuf>,
    /// The person's temporary folder, when it couldn't be written and `dir` replaced it.
    pub replaced: Option<PathBuf>,
}

/// The variables a temporary folder is read from: Python reads all three, in this order, on
/// every system; Windows' programs read TMP and TEMP, Linux's and macOS's TMPDIR.
pub const TEMP_VARS: [&str; 3] = ["TMPDIR", "TEMP", "TMP"];

/// `p` as a full path (against MDM's working folder; unchanged if that can't be read): a helper
/// runs elsewhere, so a relative path MDM gives it would name another file.
pub fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_owned())
}

/// Can a file be made in `dir`? A file is made there and removed: a folder's permissions say
/// little on Windows (a deny rule, a controlled folder, a full or read-only drive).
pub fn writable(dir: &Path) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(
        ".mdm-probe-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);
    match made {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The folder for the helpers: the person's temporary folder `user` when a file can be made in
/// it, else `<data>/work` (made, private to the user on Linux and macOS).
pub fn choose(user: &Path, data: Option<&Path>) -> Folders {
    let user = absolute(user);
    if writable(&user) {
        return Folders {
            dir: Some(user),
            replaced: None,
        };
    }
    let own = data
        .map(|d| absolute(&d.join("work")))
        .filter(|w| mdm_ipc::private_dir(w).is_ok() && writable(w));
    Folders {
        replaced: own.as_ref().map(|_| user),
        dir: own,
    }
}

/// The helpers' folder, chosen once per run (the system's temporary folder, MDM's data folder).
/// A replaced temporary folder is said in the log the first time.
pub fn folders() -> &'static Folders {
    static FOLDERS: std::sync::OnceLock<Folders> = std::sync::OnceLock::new();
    FOLDERS.get_or_init(|| {
        let chosen = choose(&std::env::temp_dir(), mdm_ipc::data_dir().as_deref());
        match (&chosen.dir, &chosen.replaced) {
            (Some(dir), Some(user)) => tracing::warn!(
                "helpers: the temporary folder {} can't be written; yt-dlp, gallery-dl and \
                 ffmpeg use {} instead",
                user.display(),
                dir.display()
            ),
            (None, _) => tracing::warn!(
                "helpers: neither the temporary folder {} nor MDM's data folder can be \
                 written; the helpers run where MDM does",
                std::env::temp_dir().display()
            ),
            _ => {}
        }
        chosen
    })
}

/// Where MDM puts its own files for a helper (a page's cookies, a looked-up page): the helpers'
/// folder, else the system's temporary folder.
pub fn temp_dir() -> PathBuf {
    folders().dir.clone().unwrap_or_else(std::env::temp_dir)
}

/// Start `cmd` in `folders`' folder with it as TEMP, TMP and TMPDIR. A working folder or a
/// variable `cmd` was already given stays.
pub fn apply(cmd: &mut Command, folders: &Folders) {
    let Some(dir) = &folders.dir else { return };
    if cmd.get_current_dir().is_none() {
        cmd.current_dir(dir);
    }
    for var in TEMP_VARS {
        let given = cmd.get_envs().any(|(k, _)| {
            if cfg!(windows) {
                k.eq_ignore_ascii_case(var)
            } else {
                k == var
            }
        });
        if !given {
            cmd.env(var, dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_persons_temporary_folder_when_it_can_be_written() {
        let user = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let f = choose(user.path(), Some(data.path()));
        assert_eq!(f.dir.as_deref(), Some(user.path()));
        assert_eq!(f.replaced, None);
        // Nothing made in the data folder, and no probe left behind.
        assert!(!data.path().join("work").exists());
        assert_eq!(std::fs::read_dir(user.path()).unwrap().count(), 0);
    }

    #[test]
    fn mdms_own_folder_when_the_temporary_folder_is_missing() {
        let data = tempfile::tempdir().unwrap();
        let missing = data.path().join("no-such-temp");
        let f = choose(&missing, Some(data.path()));
        let work = data.path().join("work");
        assert_eq!(f.dir.as_deref(), Some(work.as_path()));
        assert_eq!(f.replaced.as_deref(), Some(missing.as_path()));
        assert!(work.is_dir());
    }

    #[test]
    fn mdms_own_folder_when_the_temporary_folder_is_a_file() {
        let data = tempfile::tempdir().unwrap();
        let file = data.path().join("temp.txt");
        std::fs::write(&file, b"x").unwrap();
        let f = choose(&file, Some(data.path()));
        assert_eq!(f.dir, Some(data.path().join("work")));
        assert_eq!(f.replaced, Some(file));
    }

    /// A folder this user may read but not write (Windows: a deny rule, as `C:\Program Files`
    /// refuses a user; elsewhere: mode 0555).
    fn read_only(dir: &Path) {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap();
            let ok = Command::new("icacls")
                .arg(dir)
                .args(["/deny", &format!("{user}:(OI)(CI)(WD,AD,WEA,WA)")])
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
    }

    fn writable_again(dir: &Path) {
        #[cfg(windows)]
        {
            let _ = Command::new("icacls")
                .arg(dir)
                .arg("/reset")
                .stdout(std::process::Stdio::null())
                .status();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn mdms_own_folder_when_the_temporary_folder_cant_be_written() {
        let data = tempfile::tempdir().unwrap();
        let locked = data.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        read_only(&locked);
        // root writes anywhere (a container's test run): nothing to check then.
        if writable(&locked) {
            writable_again(&locked);
            return;
        }
        let f = choose(&locked, Some(data.path()));
        writable_again(&locked);
        assert_eq!(f.dir, Some(data.path().join("work")));
        assert_eq!(f.replaced, Some(locked));
    }

    #[test]
    fn nothing_when_no_folder_can_be_written() {
        let data = tempfile::tempdir().unwrap();
        let missing = data.path().join("no-such-temp");
        // A data folder under a file can't be made.
        let file = data.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let f = choose(&missing, Some(&file.join("data")));
        assert_eq!(
            f,
            Folders {
                dir: None,
                replaced: None
            }
        );
        let f = choose(&missing, None);
        assert_eq!(f.dir, None);
        // The helper is then left as it was.
        let mut cmd = Command::new("x");
        apply(&mut cmd, &f);
        assert!(cmd.get_current_dir().is_none());
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn a_relative_temporary_folder_is_made_full() {
        let f = choose(Path::new("."), None);
        let dir = f.dir.unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
    }

    #[test]
    fn the_folder_and_variables_a_helper_gets() {
        let f = Folders {
            dir: Some(PathBuf::from("/mdm/work")),
            replaced: None,
        };
        let mut cmd = Command::new("x");
        apply(&mut cmd, &f);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/mdm/work")));
        let mut envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
            .collect();
        envs.sort();
        assert_eq!(
            envs,
            ["TEMP", "TMP", "TMPDIR"].map(|k| (k.into(), Some("/mdm/work".into())))
        );
        // What a caller gave stays.
        let mut cmd = Command::new("x");
        cmd.current_dir("/own").env("TMP", "/own-tmp");
        apply(&mut cmd, &f);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/own")));
        let tmp = cmd.get_envs().find(|(k, _)| *k == "TMP").unwrap().1;
        assert_eq!(tmp, Some(std::ffi::OsStr::new("/own-tmp")));
    }
}
