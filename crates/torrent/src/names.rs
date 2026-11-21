//! Whether a torrent's file names are safe to write (#40). MDM checks every name itself, before a
//! torrent is listed or added, so only plain names reach the disk. librqbit writes the files under
//! the torrent's own names, so MDM can't rename them: an unsafe name refuses the torrent, saying
//! which.
//!
//! Everywhere: no empty part, `.`, `..`, separator or control character. On Windows also none of
//! `:<>"|?*`, no trailing dot or space (Windows drops them, so two names become one), and no device name
//! (`CON`, `NUL.txt`, `COM1`…). Where names ignore letter case (Windows, macOS), no two files
//! whose paths differ only by case, and nowhere a file and a folder of the same name (`a` and
//! `a/b`): the first written would stand where the other needs a folder.

/// The first unsafe name: its path (parts joined by `/`) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsafeName {
    pub path: String,
    pub why: &'static str,
}

/// Check every file's path, given as its parts, for this system.
pub fn check<I, P>(files: I) -> Result<(), UnsafeName>
where
    I: IntoIterator<Item = P>,
    P: AsRef<[String]>,
{
    check_for(
        files,
        cfg!(windows),
        cfg!(any(windows, target_os = "macos")),
    )
}

/// `check` for Windows' rules (`windows`) and a file system that ignores letter case
/// (`case_blind`), so the tests can try every system's rules on any of them.
fn check_for<I, P>(files: I, windows: bool, case_blind: bool) -> Result<(), UnsafeName>
where
    I: IntoIterator<Item = P>,
    P: AsRef<[String]>,
{
    let mut seen = std::collections::HashSet::new();
    // Every folder the files go through: a torrent can list `a` and `a/b` (a file and a folder
    // of the same name), which no disk can hold; it is refused before anything is written.
    let mut folders = std::collections::HashSet::new();
    for parts in files {
        let parts = parts.as_ref();
        let path = parts.join("/");
        let refuse = |why| UnsafeName {
            path: path.clone(),
            why,
        };
        if parts.is_empty() {
            return Err(refuse("a file without a name"));
        }
        for part in parts {
            if let Some(why) = unsafe_part(part, windows) {
                return Err(refuse(why));
            }
        }
        let fold = |s: &str| {
            if case_blind {
                s.to_lowercase()
            } else {
                s.to_owned()
            }
        };
        let mut prefix = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                prefix.push('/');
            }
            prefix.push_str(part);
            if i + 1 == parts.len() {
                break;
            }
            let key = fold(&prefix);
            if seen.contains(&key) {
                return Err(refuse("a file and a folder with the same name"));
            }
            folders.insert(key);
        }
        let key = fold(&path);
        if folders.contains(&key) {
            return Err(refuse("a file and a folder with the same name"));
        }
        if !seen.insert(key) {
            return Err(refuse(if case_blind {
                "two files whose names differ only by letter case"
            } else {
                "two files with the same name"
            }));
        }
    }
    Ok(())
}

/// Why `part` (one folder or file name) can't be written, if it can't.
fn unsafe_part(part: &str, windows: bool) -> Option<&'static str> {
    if part.is_empty() || part == "." || part == ".." {
        return Some("an empty, `.` or `..` name");
    }
    if part
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Some("a separator or control character in a name");
    }
    if !windows {
        return None;
    }
    if part.chars().any(|c| r#":<>"|?*"#.contains(c)) {
        return Some("a character Windows doesn't allow in a name");
    }
    if part.ends_with(['.', ' ']) {
        return Some("a name ending with a dot or a space");
    }
    // A device name, with or without an extension (`NUL.txt`, `com1.tar.gz`; superscript digits
    // count too).
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.chars().count() == 4
        && stem
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')));
    device.then_some("a Windows device name (CON, NUL, COM1…)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&[&str]]) -> Vec<Vec<String>> {
        list.iter()
            .map(|p| p.iter().map(|s| (*s).to_owned()).collect())
            .collect()
    }

    fn windows(list: &[&[&str]]) -> Result<(), UnsafeName> {
        check_for(paths(list), true, true)
    }

    fn linux(list: &[&[&str]]) -> Result<(), UnsafeName> {
        check_for(paths(list), false, false)
    }

    #[test]
    fn ordinary_torrents_pass() {
        let ok: &[&[&str]] = &[
            &["Big Buck Bunny", "bunny.mp4"],
            &["Big Buck Bunny", "Subs", "en.srt"],
            &["readme.txt"],
            &["Ünïcødé", "файл.txt"],
        ];
        assert_eq!(windows(ok), Ok(()));
        assert_eq!(linux(ok), Ok(()));
    }

    /// #40: a `:` anywhere in a name is refused on Windows.
    #[test]
    fn colons_are_refused_on_windows() {
        for bad in ["a:b.bin", "Q:", "q:x.txt", "x.bin:hidden"] {
            let err = windows(&[&["folder", bad]]).unwrap_err();
            assert_eq!(err.path, format!("folder/{bad}"));
            assert!(err.why.contains("Windows"), "{bad}: {}", err.why);
        }
        // Elsewhere a `:` is an ordinary character (Linux).
        assert_eq!(linux(&[&["Movie: Part 2", "a.mkv"]]), Ok(()));
    }

    #[test]
    fn windows_names_it_cannot_write_are_refused() {
        for bad in [
            "CON",
            "nul.txt",
            "Com1",
            "LPT9.log",
            "COM¹",
            "CONIN$",
            "a?.txt",
            "a<b",
            "x\"y",
            "trailing.",
            "trailing ",
            "con .txt",
        ] {
            assert!(windows(&[&[bad]]).is_err(), "{bad}");
        }
        // Not devices: longer names, or the number missing.
        for good in ["CONSOLE.txt", "COM", "COM10", "nullable", "auxiliary.c"] {
            assert_eq!(windows(&[&[good]]), Ok(()), "{good}");
        }
    }

    #[test]
    fn traversal_separators_and_control_characters_everywhere() {
        for bad in ["", ".", "..", "a/b", "a\\b", "tab\there", "nul\0byte"] {
            assert!(windows(&[&["dir", bad]]).is_err(), "{bad:?}");
            assert!(linux(&[&["dir", bad]]).is_err(), "{bad:?}");
        }
        assert!(windows(&[&[]]).is_err());
    }

    #[test]
    fn case_twins_collide_where_case_is_ignored() {
        let twins: &[&[&str]] = &[&["Dir", "A.txt"], &["dir", "a.txt"]];
        assert!(windows(twins).is_err());
        assert_eq!(linux(twins), Ok(()));
        assert!(linux(&[&["a.txt"], &["a.txt"]]).is_err(), "exact twins");
    }

    /// A torrent may list a file and another file inside a folder of the same name (`a` and
    /// `a/b`): one would be written where the other needs a folder, so it is refused first.
    #[test]
    fn a_file_cannot_be_a_folder_too() {
        let file_then_folder: &[&[&str]] = &[&["a"], &["a", "b"]];
        assert!(windows(file_then_folder).is_err());
        assert!(
            linux(file_then_folder).is_err(),
            "a file and a folder clash on every disk"
        );
        let folder_then_file: &[&[&str]] = &[&["a", "b"], &["a"]];
        assert!(windows(folder_then_file).is_err());
        let shared: &[&[&str]] = &[&["a", "b"], &["a", "c"]];
        assert_eq!(windows(shared), Ok(()), "a folder two files use is fine");
        let twins: &[&[&str]] = &[&["A"], &["a", "b"]];
        assert!(windows(twins).is_err(), "one folder where case is ignored");
        assert_eq!(linux(twins), Ok(()));
    }
}
