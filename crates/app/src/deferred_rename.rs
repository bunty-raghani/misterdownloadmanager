//! Safe deferred rename (`Item::rename_on_complete`) from a download's progress window.
//!
//! Typing a new name while a download runs never touches the live HTTP stream, signed URL,
//! temporary `.mdm.part` state, or torrent session in flight. Instead, the chosen main name is
//! stored on the item and applied in place once the download reaches `Status::Complete`:
//! - Single-file downloads replace only the finalized file's main stem, keeping its real
//!   extension (including compound archive suffixes such as `.tar.gz`).
//! - Folder downloads that belong exclusively to one download replace the folder's name,
//!   leaving every file and extension inside untouched.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use mdm_extract::{VolumeKind, volume_key};

use crate::model::Item;

/// Idle delay after typing in the progress window's Rename box before saving automatically.
pub const AUTOSAVE_DELAY: Duration = Duration::from_secs(3);

const COMPOUND_EXTS: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz4", ".tar.lz", ".tar.br",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameError {
    TrailingDot,
    TooLong,
    InvalidChar,
    ReservedDevice,
}

/// Validate the text typed in the Rename box (and re-validate before touching disk).
///
/// - `Ok(None)`: blank / whitespace-only (clears any pending deferred rename).
/// - `Ok(Some(clean))`: safe single path component (trimmed) ready to save.
/// - `Err(reason)`: forbidden path characters, traversal dots, or reserved device name.
pub fn validate_name(raw: &str) -> Result<Option<String>, RenameError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed == "." || trimmed == ".." || trimmed.ends_with('.') {
        return Err(RenameError::TrailingDot);
    }
    if trimmed.encode_utf16().count() > 200 {
        return Err(RenameError::TooLong);
    }
    if trimmed.chars().any(|c| {
        c.is_control() || mdm_engine::naming::is_format_char(c) || r#"<>:"/\|?*"#.contains(c)
    }) {
        return Err(RenameError::InvalidChar);
    }
    if mdm_engine::naming::is_windows_reserved(trimmed) {
        return Err(RenameError::ReservedDevice);
    }
    if !is_plain_single_component(OsStr::new(trimmed)) {
        return Err(RenameError::InvalidChar);
    }
    Ok(Some(trimmed.to_owned()))
}

/// True when `name` is one plain `Component::Normal` without path separators, drive colons, or NUL.
pub fn is_plain_single_component(name: &OsStr) -> bool {
    if name.is_empty() {
        return false;
    }
    let lossy = name.to_string_lossy();
    if lossy.contains(['/', '\\', ':', '\0']) || lossy == "." || lossy == ".." {
        return false;
    }
    let mut comps = Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(c)), None) if c == name
    )
}

/// True when `target` sits directly inside `parent` and its file name is one plain component.
pub fn is_safe_sibling_target(parent: &Path, target: &Path) -> bool {
    target.parent() == Some(parent) && target.file_name().is_some_and(is_plain_single_component)
}

/// Split a finalized file name into `(stem, ext_with_dot)`.
///
/// Preserves multi-volume archive suffixes (`.part1.rar`, `.7z.001`, `.zip.001`, `.r00`),
/// compound archive suffixes (`.tar.gz`, `.tar.xz`, …), and treats leading-dot names without
/// another dot (`.gitignore`) as extensionless.
pub fn split_stem_and_ext(file_name: &str) -> (&str, &str) {
    if let Some(v) = volume_key(file_name) {
        let at = v.base.len();
        if at > 0 && at < file_name.len() && &file_name[..at] != "." {
            return (&file_name[..at], &file_name[at..]);
        }
    }
    let lower = file_name.to_ascii_lowercase();
    for &cext in COMPOUND_EXTS {
        if lower.len() > cext.len() && lower.ends_with(cext) {
            let at = file_name.len() - cext.len();
            if at > 0 && &file_name[..at] != "." {
                return (&file_name[..at], &file_name[at..]);
            }
        }
    }
    match file_name.rfind('.') {
        Some(i) if i > 0 && i + 1 < file_name.len() => (&file_name[..i], &file_name[i..]),
        _ => (file_name, ""),
    }
}

/// Whether `name` in `dir` (among `items` for download `id`) is a volume of a multi-volume
/// split archive (`#37`), whose volumes must share one base name so extraction finds every part.
pub fn is_split_archive_name(id: u64, name: &str, dir: Option<&Path>, items: &[Item]) -> bool {
    let Some(v) = volume_key(name) else {
        return false;
    };
    match v.kind {
        VolumeKind::RarPart | VolumeKind::Split => true,
        VolumeKind::RarOld if v.index >= 2 => true,
        VolumeKind::RarOld => {
            let base_low = v.base.to_lowercase();
            let sibling_in_list = items.iter().any(|other| {
                other.id != id
                    && other.dest.parent() == dir
                    && volume_key(&other.file_name).is_some_and(|w| {
                        w.kind == VolumeKind::RarOld
                            && w.index >= 2
                            && w.base.to_lowercase() == base_low
                    })
            });
            if sibling_in_list {
                return true;
            }
            let r00 = format!("{base_low}.r00");
            dir.and_then(|d| std::fs::read_dir(d).ok())
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().to_lowercase() == r00)
        }
    }
}

/// Whether `item` is a volume of a multi-volume split archive (`#37`).
pub fn is_split_archive_volume(item: &Item, items: &[Item]) -> bool {
    let dir = item.dest.parent();
    if is_split_archive_name(item.id, &item.file_name, dir, items) {
        return true;
    }
    item.dest
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n != item.file_name && is_split_archive_name(item.id, n, dir, items))
}

/// Strip a duplicate trailing extension when the user typed it themselves (e.g. typing
/// `"Movie.mp4"` for a file whose finalized extension is already `.mp4`).
fn strip_duplicate_ext<'a>(desired: &'a str, real_ext: &str) -> &'a str {
    if real_ext.is_empty() || desired.len() <= real_ext.len() {
        return desired;
    }
    let split_at = desired.len() - real_ext.len();
    if !desired.is_char_boundary(split_at) {
        return desired;
    }
    let (prefix, tail) = desired.split_at(split_at);
    let trimmed = prefix.trim_end_matches(['.', ' ']);
    if tail.eq_ignore_ascii_case(real_ext) && !trimmed.is_empty() {
        trimmed
    } else {
        desired
    }
}

/// Build the target entry name inside the download's parent folder.
pub fn build_renamed_file_name(current_name: &str, desired_stem: &str, is_dir: bool) -> String {
    if is_dir {
        return desired_stem.to_owned();
    }
    let (_, real_ext) = split_stem_and_ext(current_name);
    let base = strip_duplicate_ext(desired_stem, real_ext);
    format!("{base}{real_ext}")
}

/// Whether `item`'s destination stands for a folder (multi-file torrent, gallery, multi-file
/// debrid transfer) rather than a single file.
pub fn is_folder_target(item: &Item) -> bool {
    item.gallery.is_some()
        || item.torrent.as_ref().is_some_and(|t| t.folder == item.dest)
        || item.dest.is_dir()
}

/// Whether two paths refer to the same existing file or folder (e.g. a case-only rename on a
/// case-insensitive APFS or NTFS volume, where `canonicalize` resolves both to the stored entry).
pub fn same_existing_entry(a: &Path, b: &Path) -> bool {
    let (Ok(ca), Ok(cb)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) else {
        return false;
    };
    ca == cb
}

fn trim_trailing_sep(p: &Path) -> PathBuf {
    p.components().collect()
}

/// Whether `a` and `b` refer to the same folder path (by normalized components or canonical path).
pub fn paths_refer_to_same_dir(a: &Path, b: &Path) -> bool {
    let (na, nb) = (trim_trailing_sep(a), trim_trailing_sep(b));
    if na.as_os_str().is_empty() || nb.as_os_str().is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    #[cfg(windows)]
    if na
        .to_string_lossy()
        .eq_ignore_ascii_case(&nb.to_string_lossy())
    {
        return true;
    }
    same_existing_entry(&na, &nb)
}

/// Whether `candidate` is `dir` itself or sits inside `dir`.
pub fn path_is_at_or_inside_dir(candidate: &Path, dir: &Path) -> bool {
    let (nc, nd) = (trim_trailing_sep(candidate), trim_trailing_sep(dir));
    if nc.as_os_str().is_empty() || nd.as_os_str().is_empty() {
        return false;
    }
    if nc.starts_with(&nd) || paths_refer_to_same_dir(&nc, &nd) {
        return true;
    }
    #[cfg(windows)]
    {
        let cl = nc.to_string_lossy().to_lowercase();
        let dl = nd.to_string_lossy().to_lowercase();
        if cl == dl
            || cl
                .strip_prefix(&dl)
                .is_some_and(|rest| rest.starts_with(['\\', '/']))
        {
            return true;
        }
    }
    if let Ok(canon_dir) = std::fs::canonicalize(&nd) {
        let mut probe = nc.as_path();
        let mut tail = Vec::new();
        while !probe.as_os_str().is_empty() {
            if let Ok(mut canon_probe) = std::fs::canonicalize(probe) {
                for part in tail.into_iter().rev() {
                    canon_probe.push(part);
                }
                return canon_probe.starts_with(&canon_dir);
            }
            let Some(name) = probe.file_name() else {
                break;
            };
            tail.push(name.to_os_string());
            let Some(parent) = probe.parent() else {
                break;
            };
            probe = parent;
        }
    }
    false
}

/// Whether `dir` already exists on disk and contains at least one entry.
pub fn folder_has_any_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut rd| rd.next().is_some())
}

/// Whether `dir` holds any file or subfolder outside `allowed_files` (files this download wrote
/// or that belong to its torrent manifest).
pub fn folder_has_untracked_files(dir: &Path, allowed_files: &[PathBuf]) -> bool {
    use std::collections::HashSet;
    if !dir.exists() {
        return false;
    }
    let root = std::fs::canonicalize(dir).unwrap_or_else(|_| trim_trailing_sep(dir));
    let mut allowed = HashSet::with_capacity(allowed_files.len());
    let mut allowed_dirs = HashSet::new();
    allowed_dirs.insert(root.clone());
    for f in allowed_files {
        let norm = std::fs::canonicalize(f).unwrap_or_else(|_| trim_trailing_sep(f));
        let mut anc = norm.parent();
        while let Some(d) = anc {
            if d == root || !d.starts_with(&root) {
                break;
            }
            if !allowed_dirs.insert(d.to_path_buf()) {
                break;
            }
            anc = d.parent();
        }
        allowed.insert(norm);
    }

    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&cur) else {
            return cur.exists();
        };
        for entry in rd {
            let Ok(entry) = entry else {
                return true;
            };
            let Ok(ft) = entry.file_type() else {
                return true;
            };
            let path = entry.path();
            let norm = std::fs::canonicalize(&path).unwrap_or_else(|_| trim_trailing_sep(&path));
            if ft.is_dir() {
                if !allowed_dirs.contains(&norm) {
                    return true;
                }
                stack.push(path);
            } else if !allowed.contains(&norm) {
                return true;
            }
        }
    }
    false
}

fn entry_taken(p: &Path) -> bool {
    p.exists() || mdm_engine::naming::part_path(p).exists()
}

fn os_rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return crate::win32::rename_no_replace(from, to);
    #[cfg(unix)]
    return crate::unix::rename_no_replace(from, to);
}

/// Re-validate `desired_stem` and resolve a collision-safe sibling path in `current_dest`'s
/// parent folder, plus an in-process `naming::Reservation` when the target is a distinct entry.
/// Returns `None` when `desired_stem` is invalid, blank, or yields the current name unchanged.
pub fn reserve_target_path(
    current_dest: &Path,
    desired_stem: &str,
    is_dir: bool,
) -> Option<(PathBuf, Option<mdm_engine::naming::Reservation>)> {
    let clean = validate_name(desired_stem).ok()??;
    let parent = current_dest.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    let cur_name = current_dest.file_name()?.to_string_lossy();
    let raw_wanted = build_renamed_file_name(&cur_name, &clean, is_dir);
    let wanted = mdm_engine::naming::fit_name(parent, &raw_wanted)
        .unwrap_or(raw_wanted)
        .trim_end_matches(['.', ' '])
        .to_owned();
    if !is_plain_single_component(OsStr::new(&wanted)) || wanted == cur_name.as_ref() {
        return None;
    }
    let first = parent.join(&wanted);
    if !is_safe_sibling_target(parent, &first) {
        return None;
    }
    if same_existing_entry(current_dest, &first) {
        return Some((first, None));
    }
    if !entry_taken(&first)
        && let Some(res) = mdm_engine::naming::reserve(&first)
    {
        return Some((first, Some(res)));
    }
    let (base, ext) = if is_dir {
        (wanted.as_str(), "")
    } else {
        split_stem_and_ext(&wanted)
    };
    for n in 1u32.. {
        let candidate = format!("{base} ({n}){ext}");
        let fitted = mdm_engine::naming::fit_name(parent, &candidate).unwrap_or(candidate);
        if !is_plain_single_component(OsStr::new(&fitted)) {
            return None;
        }
        let path = parent.join(fitted);
        if !is_safe_sibling_target(parent, &path) {
            return None;
        }
        if !entry_taken(&path)
            && let Some(res) = mdm_engine::naming::reserve(&path)
        {
            return Some((path, Some(res)));
        }
    }
    None
}

/// Resolve the collision-safe destination path in `current_dest`'s parent folder, or `None`
/// when `desired_stem` fails validation or `current_dest` already has the desired name.
#[cfg(test)]
pub fn resolved_target_path(
    current_dest: &Path,
    desired_stem: &str,
    is_dir: bool,
) -> Option<PathBuf> {
    reserve_target_path(current_dest, desired_stem, is_dir).map(|(p, _)| p)
}

/// Atomically rename `from` to `to` within the same parent folder without ever replacing an
/// existing file or folder, stepping through a non-replacing temporary sibling if a
/// case-insensitive filesystem rejects a direct case-only rename.
pub fn rename_entry_io(from: &Path, to: &Path) -> std::io::Result<()> {
    let Some(parent) = from.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing parent folder",
        ));
    };
    if !is_safe_sibling_target(parent, to) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target must be a single name in the same folder",
        ));
    }
    if from == to {
        return Ok(());
    }
    let case_only = from.file_name().zip(to.file_name()).is_some_and(|(a, b)| {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }) && same_existing_entry(from, to);

    if !case_only {
        return os_rename_no_replace(from, to);
    }

    if os_rename_no_replace(from, to).is_ok() {
        return Ok(());
    }
    let stamp = std::process::id();
    for k in 0..16u32 {
        let hop = parent.join(format!(".mdm-rename-{stamp}-{k}.tmp"));
        if !hop.exists() && os_rename_no_replace(from, &hop).is_ok() {
            return match os_rename_no_replace(&hop, to) {
                Ok(()) => Ok(()),
                Err(e) => match os_rename_no_replace(&hop, from) {
                    Ok(()) => Err(e),
                    Err(back_err) => Err(std::io::Error::new(
                        e.kind(),
                        crate::i18n::fmt(
                            // i18n: appended to an OS error when a case-only rename could not move the temporary file back to its original name; {error} = first OS error, {path} = temporary path left on disk, {rollback_error} = second OS error
                            crate::i18n::t("{error} (left as {path}: {rollback_error})"),
                            &[
                                ("error", &e.to_string()),
                                ("path", &hop.display().to_string()),
                                ("rollback_error", &back_err.to_string()),
                            ],
                        ),
                    )),
                },
            };
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate temporary hop for case-only rename",
    ))
}

/// Rename `from` to `to` without ever replacing an existing entry.
#[cfg(test)]
pub fn rename_entry(from: &Path, to: &Path) -> Result<(), String> {
    rename_entry_io(from, to).map_err(|e| e.to_string())
}

/// Rename `from` to `initial_to` (holding `hold`), automatically stepping to `base (N).ext` if
/// another process created `initial_to` before the atomic non-replacing rename ran.
pub fn rename_reserved_or_number(
    from: &Path,
    initial_to: PathBuf,
    hold: Option<mdm_engine::naming::Reservation>,
    is_dir: bool,
) -> std::io::Result<PathBuf> {
    match rename_entry_io(from, &initial_to) {
        Ok(()) => {
            drop(hold);
            return Ok(initial_to);
        }
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            drop(hold);
            return Err(e);
        }
        Err(_) => {
            drop(hold);
        }
    }
    let Some(parent) = from.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing parent folder",
        ));
    };
    let initial_name = initial_to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (base, ext) = if is_dir {
        (initial_name.as_str(), "")
    } else {
        split_stem_and_ext(&initial_name)
    };
    for n in 1u32..10_000 {
        let candidate = format!("{base} ({n}){ext}");
        let fitted = mdm_engine::naming::fit_name(parent, &candidate).unwrap_or(candidate);
        let path = parent.join(&fitted);
        if entry_taken(&path) {
            continue;
        }
        let Some(_res) = mdm_engine::naming::reserve(&path) else {
            continue;
        };
        match rename_entry_io(from, &path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free numbered destination",
    ))
}

/// Rename `yt-dlp` subtitle side files (`<old_stem>.<lang>.<srt|vtt…>`) alongside a renamed
/// video (`old_video` → `new_video`), returning the final list of side-file paths (renamed or,
/// if a side file could not move, its original path).
pub fn rename_video_side_files(
    old_video: &Path,
    new_video: &Path,
    side_files: &[PathBuf],
) -> Vec<PathBuf> {
    let (Some(dir), Some(old_stem), Some(new_stem)) = (
        old_video.parent(),
        old_video.file_stem().and_then(|s| s.to_str()),
        new_video.file_stem().and_then(|s| s.to_str()),
    ) else {
        return side_files.to_vec();
    };
    if old_stem.is_empty() || new_stem.is_empty() || old_stem == new_stem {
        return side_files.to_vec();
    }
    let prefix = format!("{old_stem}.");
    side_files
        .iter()
        .map(|sf| {
            if sf.parent() != Some(dir) || !sf.is_file() {
                return sf.clone();
            }
            let Some(name) = sf.file_name().and_then(|n| n.to_str()) else {
                return sf.clone();
            };
            let Some(rest) = name.strip_prefix(&prefix) else {
                return sf.clone();
            };
            let raw_target = format!("{new_stem}.{rest}");
            let fitted = mdm_engine::naming::fit_name(dir, &raw_target).unwrap_or(raw_target);
            if !is_plain_single_component(OsStr::new(&fitted)) {
                return sf.clone();
            }
            let target = dir.join(&fitted);
            let hold = mdm_engine::naming::reserve(&target);
            rename_reserved_or_number(sf, target, hold, false).unwrap_or_else(|_| sf.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_safe_and_unsafe_names() {
        assert_eq!(validate_name(""), Ok(None));
        assert_eq!(validate_name("   "), Ok(None));
        assert_eq!(
            validate_name("  My Movie (2026)  "),
            Ok(Some("My Movie (2026)".into()))
        );
        assert_eq!(validate_name("归来_第01集"), Ok(Some("归来_第01集".into())));
        for bad in [
            ".",
            "..",
            "trailing.",
            "sub/file",
            r"sub\file",
            r"..\..\x",
            "../../x",
            "/tmp/evil",
            r"C:\evil",
            "a:b",
            "a*b",
            "a?b",
            "CON",
            "CON .txt",
            "aux",
            "lpt1",
            "COM¹",
            "nul.part",
            "bad\u{202E}name",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn revalidates_tampered_names_right_before_renaming() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("video.mp4");
        std::fs::write(&src, b"video").unwrap();

        for tampered in [
            r"..\..\escaped",
            "../../escaped",
            "/tmp/escaped",
            r"C:\escaped",
            "CON",
            "trailing.",
            "bad\u{202E}exe",
            "   ",
        ] {
            assert_eq!(
                resolved_target_path(&src, tampered, false),
                None,
                "tampered value {tampered:?} from downloads.json must be refused"
            );
        }

        // Direct rename_entry also refuses leaving the parent folder.
        let outside = dir.path().parent().unwrap().join("outside.mp4");
        assert!(rename_entry(&src, &outside).is_err());
        assert!(src.exists());
    }

    #[test]
    fn splits_single_compound_and_split_volume_extensions() {
        assert_eq!(split_stem_and_ext("89af12.mp4"), ("89af12", ".mp4"));
        assert_eq!(
            split_stem_and_ext("archive.v1.tar.gz"),
            ("archive.v1", ".tar.gz")
        );
        assert_eq!(split_stem_and_ext("ARCHIVE.TAR.XZ"), ("ARCHIVE", ".TAR.XZ"));
        assert_eq!(
            split_stem_and_ext("Movie.2026.part1.rar"),
            ("Movie.2026", ".part1.rar")
        );
        assert_eq!(split_stem_and_ext("set.7z.001"), ("set", ".7z.001"));
        assert_eq!(split_stem_and_ext("set.zip.002"), ("set", ".zip.002"));
        assert_eq!(split_stem_and_ext("old.r00"), ("old", ".r00"));
        assert_eq!(split_stem_and_ext("single.rar"), ("single", ".rar"));
        assert_eq!(split_stem_and_ext("README"), ("README", ""));
        assert_eq!(split_stem_and_ext(".gitignore"), (".gitignore", ""));
    }

    #[test]
    fn detects_multi_volume_split_archives_without_blocking_single_rar() {
        let dir = tempfile::tempdir().unwrap();
        let items: Vec<Item> = Vec::new();
        assert!(is_split_archive_name(
            1,
            "x.part1.rar",
            Some(dir.path()),
            &items
        ));
        assert!(is_split_archive_name(
            1,
            "x.7z.001",
            Some(dir.path()),
            &items
        ));
        assert!(is_split_archive_name(1, "x.r00", Some(dir.path()), &items));
        assert!(
            !is_split_archive_name(1, "lone.rar", Some(dir.path()), &items),
            "standalone .rar is renameable"
        );

        // Once lone.r00 exists alongside lone.rar, lone.rar is part of an old-style volume set.
        std::fs::write(dir.path().join("lone.r00"), b"part2").unwrap();
        assert!(is_split_archive_name(
            1,
            "lone.rar",
            Some(dir.path()),
            &items
        ));
    }

    #[test]
    fn replaces_stem_keeping_real_extension_or_folder_name() {
        assert_eq!(
            build_renamed_file_name("9f82a10b3c.mp4", "Lecture 01", false),
            "Lecture 01.mp4"
        );
        assert_eq!(
            build_renamed_file_name("snapshot-20261111.tar.gz", "clean-backup", false),
            "clean-backup.tar.gz"
        );
        assert_eq!(
            build_renamed_file_name("x.part1.rar", "Name", false),
            "Name.part1.rar"
        );
        assert_eq!(
            build_renamed_file_name("x.7z.001", "Name", false),
            "Name.7z.001"
        );
        assert_eq!(
            build_renamed_file_name("9f82a10b3c.mp4", "Lecture 01.MP4", false),
            "Lecture 01.mp4"
        );
        assert_eq!(
            build_renamed_file_name("installer.dmg", "App v2.4", false),
            "App v2.4.dmg"
        );
        assert_eq!(
            build_renamed_file_name("Ubuntu-24.04-Torrent", "Linux Mint 22.1", true),
            "Linux Mint 22.1"
        );
    }

    #[test]
    fn non_replacing_rename_never_overwrites_existing_file_and_numbers_on_race() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("unknown_hash.tar.gz");
        std::fs::write(&src, b"payload").unwrap();

        let existing = dir.path().join("backup.tar.gz");
        std::fs::write(&existing, b"precious").unwrap();

        // Direct atomic rename refuses to replace an existing target.
        let err = rename_entry_io(&src, &existing).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&existing).unwrap(), b"precious");
        assert_eq!(std::fs::read(&src).unwrap(), b"payload");

        // Race simulation: `initial_to` was free when chosen, then created before rename ran.
        let race_src = dir.path().join("late_race.mp4");
        std::fs::write(&race_src, b"mine").unwrap();
        let race_target = dir.path().join("Report.mp4");
        std::fs::write(&race_target, b"theirs").unwrap();
        let actual =
            rename_reserved_or_number(&race_src, race_target.clone(), None, false).unwrap();
        assert_eq!(actual, dir.path().join("Report (1).mp4"));
        assert_eq!(std::fs::read(&race_target).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&actual).unwrap(), b"mine");

        // Folder collision with a dot in its name numbers at the end ("Season 1.5 (1)").
        let fsrc = dir.path().join("raw_pack");
        std::fs::create_dir_all(&fsrc).unwrap();
        std::fs::write(fsrc.join("ep01.mkv"), b"ep1").unwrap();
        std::fs::create_dir_all(dir.path().join("Season 1.5")).unwrap();
        let fto = resolved_target_path(&fsrc, "Season 1.5", true).unwrap();
        assert_eq!(fto, dir.path().join("Season 1.5 (1)"));
        rename_entry(&fsrc, &fto).unwrap();
        assert_eq!(std::fs::read(fto.join("ep01.mkv")).unwrap(), b"ep1");

        // Case-only rename never appends " (1)".
        let lower = dir.path().join("movie.mp4");
        std::fs::write(&lower, b"video").unwrap();
        let upper = resolved_target_path(&lower, "Movie", false).unwrap();
        assert_eq!(upper, dir.path().join("Movie.mp4"));
        rename_entry(&lower, &upper).unwrap();
        assert_eq!(std::fs::read(&upper).unwrap(), b"video");
    }

    #[test]
    fn renames_yt_dlp_side_files_alongside_video() {
        let dir = tempfile::tempdir().unwrap();
        let old_vid = dir.path().join("RawClip.mp4");
        let new_vid = dir.path().join("Keynote 2026.mp4");
        let sub_en = dir.path().join("RawClip.en.srt");
        let sub_zh = dir.path().join("RawClip.zh-Hans.vtt");
        std::fs::write(&new_vid, b"v").unwrap();
        std::fs::write(&sub_en, b"en").unwrap();
        std::fs::write(&sub_zh, b"zh").unwrap();

        let moved = rename_video_side_files(&old_vid, &new_vid, &[sub_en, sub_zh]);
        assert_eq!(
            moved,
            vec![
                dir.path().join("Keynote 2026.en.srt"),
                dir.path().join("Keynote 2026.zh-Hans.vtt"),
            ]
        );
        assert_eq!(std::fs::read(&moved[0]).unwrap(), b"en");
        assert_eq!(std::fs::read(&moved[1]).unwrap(), b"zh");
    }

    #[test]
    fn detects_untracked_files_and_subfolders_inside_download_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("gallery_pack");
        let sub = folder.join("part1");
        std::fs::create_dir_all(&sub).unwrap();
        let a = folder.join("01.jpg");
        let b = sub.join("02.jpg");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();

        let allowed = vec![a.clone(), b.clone()];
        assert!(!folder_has_untracked_files(&folder, &allowed));

        let extra_dir = folder.join("unrelated_empty_dir");
        std::fs::create_dir(&extra_dir).unwrap();
        assert!(folder_has_untracked_files(&folder, &allowed));
        std::fs::remove_dir(&extra_dir).unwrap();

        let foreign = folder.join("leftover_from_removed_download.png");
        std::fs::write(&foreign, b"x").unwrap();
        assert!(folder_has_untracked_files(&folder, &allowed));
    }

    #[cfg(unix)]
    #[test]
    fn unix_fallback_never_replaces_existing_empty_folder_or_file() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src_folder");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("inside.txt"), b"mine").unwrap();

        let empty_target = dir.path().join("empty_folder");
        std::fs::create_dir(&empty_target).unwrap();
        let err = crate::unix::fallback_rename_no_replace(&src_dir, &empty_target).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(src_dir.join("inside.txt").exists());
        assert!(
            std::fs::read_dir(&empty_target).unwrap().next().is_none(),
            "existing empty folder must remain untouched"
        );

        let moved_dir = dir.path().join("fresh_folder");
        crate::unix::fallback_rename_no_replace(&src_dir, &moved_dir).unwrap();
        assert_eq!(
            std::fs::read(moved_dir.join("inside.txt")).unwrap(),
            b"mine"
        );
    }
}
