//! A downloaded archive extracted in process (`mdm_extract::extract`: zip, tar, tar.gz, tar.xz,
//! a single .gz or .xz), within small limits, into a folder of its own: nothing may land outside
//! the destination, and no link may come out of it.
//!
//! The input's first byte picks the archive's name (its extension decides between a compressed
//! tar and a single compressed file); the rest is the archive.
#![no_main]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use mdm_extract::{Limits, Policy, Subfolder};

/// One folder for the whole run, emptied after each input.
fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mdm-fuzz-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    })
}

/// Every path under `dir`, links not followed; panics on a link.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        assert!(!meta.file_type().is_symlink(), "a link came out: {path:?}");
        out.push(path.clone());
        if meta.is_dir() {
            walk(&path, out);
        }
    }
}

const NAMES: [&str; 6] = ["a.zip", "a.tar", "a.tar.gz", "a.tar.xz", "a.gz", "a.xz"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, bytes)) = data.split_first() else {
        return;
    };
    let root = root();
    let boxed = root.join("box");
    let _ = fs::remove_dir_all(&boxed);
    fs::create_dir_all(&boxed).unwrap();
    let archive = boxed.join(NAMES[usize::from(pick) % NAMES.len()]);
    fs::write(&archive, bytes).unwrap();
    let dest = boxed.join("out");

    let mut on_file = |p: &Path| assert!(p.starts_with(&dest), "{p:?} put outside");
    let mut policy = Policy {
        dest: dest.clone(),
        subfolder: Subfolder::Smart,
        password: Some("password".into()),
        limits: Limits {
            total: 4 << 20,
            entries: 200,
            depth: 16,
        },
        tool: None,
        hatchzip: None,
        in_place: false,
        runs: None,
        on_file: &mut on_file,
        cancel: None,
    };
    let result = mdm_extract::extract(&archive, &mut policy);

    // Only the archive and the destination in the box, and only the box in the root.
    let mut top: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    top.sort();
    assert_eq!(top, ["box"], "something was written beside the box");
    for e in fs::read_dir(&boxed).unwrap() {
        let path = e.unwrap().path();
        assert!(
            path == archive || path == dest,
            "{path:?} written outside the destination"
        );
    }
    if dest.exists() {
        let mut all = Vec::new();
        walk(&dest, &mut all);
        // The staging folder (`dest/.mdm-extract-<pid>-<n>`) is gone; an entry of such a name
        // deeper down is the archive's own.
        let staging = format!(".mdm-extract-{}-", std::process::id());
        for p in all.iter().filter(|p| p.parent() == Some(dest.as_path())) {
            let name = p.file_name().unwrap().to_string_lossy();
            assert!(
                !name.starts_with(&staging),
                "the staging folder was left: {p:?}"
            );
        }
        if let Ok(report) = &result {
            assert!(report.folder.starts_with(&dest));
        }
    }
    let _ = fs::remove_dir_all(&boxed);
});
