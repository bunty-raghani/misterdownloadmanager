//! Deletion authority is a receipt for an object we created, not a torrent path.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub index: usize,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ownership {
    pub token: String,
    pub files: Vec<Claim>,
}

/// The ledger's format. A newer one than this build knows is read as "keep everything".
const LEDGER_VERSION: u32 = 2;

/// Volume or device, file index or inode, and creation time (see `win32.rs`, `unix.rs`).
pub type Identity = [u64; 5];

/// Which object each handed-over file is, for the files whose filesystem tells.
pub fn identities(
    files: &[mdm_torrent::authorized_storage::PreparedFile],
) -> Vec<(usize, Identity)> {
    files
        .iter()
        .filter_map(|f| identity(&f.file).ok().map(|id| (f.index, id)))
        .collect()
}

/// Whether a file now at a torrent's path is another object than the one the engine holds:
/// only files both sides can identify are compared.
pub fn swapped(held: &[(usize, Identity)], now: &[(usize, Identity)]) -> bool {
    let was: HashMap<usize, Identity> = held.iter().copied().collect();
    now.iter()
        .any(|(i, id)| was.get(i).is_some_and(|w| w != id))
}

/// The object shown in the existing-files question, captured before consent.
#[derive(Clone, Debug)]
pub struct Consent {
    path: PathBuf,
    identity: Option<Identity>,
}

pub fn consent_for(paths: &[PathBuf]) -> Vec<Consent> {
    paths
        .iter()
        .map(|path| Consent {
            path: key(path),
            identity: identity_at(path).ok(),
        })
        .collect()
}

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    token: String,
    /// Files MDM never deletes: present when the torrent was added (the person agreed to use
    /// them), or whose identity could not be had.
    #[serde(default)]
    protected: Vec<usize>,
    /// Permission to reuse a particular pre-existing object, independently of deletion.
    #[serde(default)]
    consented: Vec<Receipt>,
    #[serde(default)]
    files: Vec<Receipt>,
    #[serde(default)]
    directories: Vec<Receipt>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    index: usize,
    path: PathBuf,
    identity: Identity,
}

#[cfg(unix)]
use crate::unix::{
    open_torrent_file, remove_owned_torrent_file, torrent_file_identity as identity,
    torrent_file_identity_at as identity_at, torrent_file_path,
};
#[cfg(windows)]
use crate::win32::{
    open_torrent_file, remove_owned_torrent_file, torrent_file_identity as identity,
    torrent_file_identity_at as identity_at, torrent_file_path,
};

/// Resolve existing ancestors too, so aliases and case differences cannot evade a claim.
pub fn key(path: &Path) -> PathBuf {
    let mut ancestor = path.to_owned();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name().map(|s| s.to_owned()) else {
            break;
        };
        tail.push(name);
        if !ancestor.pop() {
            break;
        }
    }
    let mut resolved = ancestor.canonicalize().unwrap_or(ancestor);
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    fold_case(resolved)
}

/// The folder `path` should really be in, resolved (once per folder). The name is not
/// compared: a link as the name is refused when opening, and some systems store names in
/// another Unicode form than the one asked for.
fn expected_path(homes: &mut HashMap<PathBuf, PathBuf>, path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(path);
    homes
        .entry(parent.to_owned())
        .or_insert_with(|| key(parent))
        .clone()
}

/// Case-insensitive systems compare paths in lower case.
fn fold_case(path: PathBuf) -> PathBuf {
    #[cfg(any(windows, target_os = "macos"))]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        path
    }
}

fn outside_error() -> io::Error {
    io::Error::other(crate::i18n::t(
        // i18n: a torrent's folder is a link or junction leading outside its destination
        "A torrent file's folder points outside its destination",
    ))
}

/// Open or make the file at `path` without following a link, and check the handle itself is
/// where `expected` says (the folder it is in, resolved before): a folder swapped
/// for a junction or a link after the checks leads elsewhere, and what this made there is taken
/// out again. The names are checked before; this is what the system actually opened.
fn open_inside(path: &Path, create: bool, expected: &Path) -> io::Result<std::fs::File> {
    let file = open_torrent_file(path, create)?;
    // What this made is taken out again where the handle really is: the name may lead
    // elsewhere by now (a link put back), so removing it by name would miss it.
    let refuse = |file: &std::fs::File, real: Option<&Path>, e: io::Error| {
        if create && let Ok(id) = identity(file) {
            let _ = remove_owned_torrent_file(real.unwrap_or(path), id);
        }
        e
    };
    match torrent_file_path(&file) {
        Ok(real)
            if real
                .parent()
                .is_some_and(|p| fold_case(p.to_owned()) == expected) =>
        {
            Ok(file)
        }
        Ok(real) => Err(refuse(&file, Some(&real), outside_error())),
        // A system that can't say where a handle is keeps the checks before the open.
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Ok(file),
        Err(e) => Err(refuse(&file, None, e)),
    }
}

fn claimed(set: &BTreeSet<PathBuf>, path: &Path) -> bool {
    path.ancestors().any(|p| set.contains(p))
        || set
            .range(path.to_owned()..)
            .next()
            .is_some_and(|p| p.starts_with(path))
}

/// The drive or mount a path is on, for comparing without touching the disk. On Windows the
/// drive; elsewhere every path starts at `/`, so the mount's own folder stands for it: the one
/// under `/mnt`, `/media/<user>`, `/run/media/<user>` or `/Volumes`, where disks that sleep or
/// leave are mounted. Anything else is the main disk.
fn root_of(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    #[cfg(windows)]
    {
        parts.into_iter().next()
    }
    #[cfg(not(windows))]
    {
        let at = |i: usize| parts.get(i).map(String::as_str);
        let depth = match (at(1), at(2)) {
            (Some("mnt" | "volumes"), _) => 3,
            (Some("media"), _) => 4,
            (Some("run"), Some("media")) => 5,
            _ => 1,
        };
        Some(parts.into_iter().take(depth).collect::<Vec<_>>().join("/"))
    }
}

/// What looking at other downloads' files cost once, kept for a batch of checks: a start of
/// MDM checks every finished torrent against every other one's files, and each look is an open
/// and a canonicalize.
#[derive(Default)]
pub struct ClaimCache(Mutex<HashMap<PathBuf, (PathBuf, Option<Identity>)>>);

impl ClaimCache {
    fn look(&self, path: &Path) -> (PathBuf, Option<Identity>) {
        if let Some(found) = self.0.lock().unwrap().get(path) {
            return found.clone();
        }
        let found = (key(path), identity_at(path).ok());
        self.0
            .lock()
            .unwrap()
            .insert(path.to_owned(), found.clone());
        found
    }
}

pub fn conflicts(paths: &[PathBuf], claims: &[PathBuf], canonical: bool) -> Vec<PathBuf> {
    conflicts_cached(paths, claims, canonical, None)
}

/// [`conflicts`], looking at each claim once for all the checks that share `cache`.
pub fn conflicts_cached(
    paths: &[PathBuf],
    claims: &[PathBuf],
    canonical: bool,
    cache: Option<&ClaimCache>,
) -> Vec<PathBuf> {
    let normalize = |path: &PathBuf| {
        if canonical {
            return cache.map_or_else(|| key(path), |c| c.look(path).0);
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            PathBuf::from(path.to_string_lossy().to_lowercase())
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            path.clone()
        }
    };
    // Looking at a claim means opening it: leave alone those on another drive than any of the
    // paths asked about, so a sleeping USB stick or a network share is not woken for nothing.
    let roots: HashSet<_> = paths.iter().filter_map(|p| root_of(p)).collect();
    let near: Vec<&PathBuf> = claims
        .iter()
        .filter(|c| !canonical || root_of(c).is_some_and(|r| roots.contains(&r)))
        .collect();
    let set: BTreeSet<_> = near.iter().map(|p| normalize(p)).collect();
    let identities: HashSet<_> = if canonical {
        near.iter()
            .filter_map(|p| match cache {
                Some(c) => c.look(p).1,
                None => identity_at(p).ok(),
            })
            .collect()
    } else {
        Default::default()
    };
    paths
        .iter()
        .filter(|p| {
            claimed(&set, &normalize(p))
                || (canonical && identity_at(p).is_ok_and(|id| identities.contains(&id)))
        })
        .cloned()
        .collect()
}

impl Ownership {
    pub fn validate(&self, bytes: &[u8], hash: &str) -> io::Result<()> {
        let meta = mdm_torrent::librqbit::torrent_from_bytes(bytes).map_err(io::Error::other)?;
        let files = mdm_torrent::files_from_bytes(bytes).map_err(io::Error::other)?;
        let count = files.iter().filter(|f| !f.padding).count();
        if !meta.info_hash.as_string().eq_ignore_ascii_case(hash)
            || count != self.files.len()
            || self.files.iter().any(|claim| {
                files
                    .get(claim.index)
                    .is_none_or(|f| f.padding || (count != 1 && f.path != claim.path))
            })
        {
            return Err(io::Error::other(crate::i18n::t(
                // i18n: error when the saved record of a torrent's files differs from the .torrent file
                "The torrent ownership record does not match its file list",
            )));
        }
        Ok(())
    }
    pub fn new(meta: &mdm_torrent::Meta) -> Self {
        Self {
            token: mdm_ipc::random_token(),
            files: meta
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| !f.padding)
                .map(|(index, f)| Claim {
                    index,
                    path: f.path.clone(),
                })
                .collect(),
        }
    }

    /// An older row or lost ledger has no deletion authority. A stable recovery
    /// token permits fresh user consent to survive another restart without guessing ownership.
    pub fn recover(bytes: &[u8], hash: &str) -> io::Result<Self> {
        if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self {
            token: format!("{hash:0<64}").to_lowercase(),
            files: mdm_torrent::files_from_bytes(bytes)
                .map_err(io::Error::other)?
                .into_iter()
                .enumerate()
                .filter(|(_, f)| !f.padding)
                .map(|(index, f)| Claim {
                    index,
                    path: f.path,
                })
                .collect(),
        })
    }

    pub fn paths(&self, folder: &Path, dest: &Path) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|f| {
                if self.files.len() == 1 {
                    dest.to_owned()
                } else {
                    folder.join(&f.path)
                }
            })
            .collect()
    }

    /// One lock per torrent for everything that reads and writes its record: a start, a move, a
    /// delete and a forget on different threads must not interleave their read, change and
    /// write, or one writes over the other.
    fn lock(&self) -> std::sync::Arc<Mutex<()>> {
        static LOCKS: std::sync::LazyLock<Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>> =
            std::sync::LazyLock::new(Default::default);
        LOCKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(self.token.clone())
            .or_default()
            .clone()
    }

    fn ledger_path(&self, data: &Path) -> PathBuf {
        data.join("ownership").join(format!("{}.json", self.token))
    }

    pub fn forget(&self, data: &Path) -> io::Result<()> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.check_token()?;
        match std::fs::remove_file(self.ledger_path(data)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn initialize(
        &self,
        data: &Path,
        folder: &Path,
        dest: &Path,
        existing: &[Consent],
    ) -> io::Result<()> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let paths = self.paths(folder, dest);
        let mut protected = Vec::new();
        let mut consented = Vec::new();
        // One look at each path and one lookup of what was agreed to: a torrent of thousands of
        // files re-added onto files already there must not compare every pair.
        let agreed: HashMap<&Path, &Consent> =
            existing.iter().map(|c| (c.path.as_path(), c)).collect();
        for (file, path) in self.files.iter().zip(paths) {
            let consent = if agreed.is_empty() {
                None
            } else {
                agreed.get(key(&path).as_path()).copied()
            };
            if path.symlink_metadata().is_ok() || consent.is_some() {
                protected.push(file.index);
                if let Some(id) = consent.and_then(|c| c.identity) {
                    consented.push(Receipt {
                        index: file.index,
                        path: file.path.clone(),
                        identity: id,
                    });
                }
            }
        }
        self.save(
            data,
            &Ledger {
                version: LEDGER_VERSION,
                token: self.token.clone(),
                protected,
                consented,
                ..Default::default()
            },
        )
    }

    /// A torrent added before ownership records, whose folder MDM made for it (or its one
    /// file): everything already there was written by MDM's own engine, so it is taken as agreed
    /// to, once, and never deleted through this record (such a torrent is deleted as it always
    /// was, by name). Without it, resuming a half-downloaded torrent or sharing a finished one
    /// after an update would ask about MDM's own files. A record that exists is left alone.
    pub fn adopt_existing(&self, data: &Path, folder: &Path, dest: &Path) -> io::Result<()> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.check_token()?;
        if self.ledger_path(data).exists() {
            return Ok(());
        }
        let mut protected = Vec::new();
        let mut consented = Vec::new();
        for (claim, path) in self.files.iter().zip(self.paths(folder, dest)) {
            let regular = path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file());
            if !regular || mdm_torrent::check_relative_path(&claim.path).is_err() {
                continue;
            }
            protected.push(claim.index);
            if let Ok(identity) = identity_at(&path) {
                consented.push(Receipt {
                    index: claim.index,
                    path: claim.path.clone(),
                    identity,
                });
            }
        }
        self.save(
            data,
            &Ledger {
                version: LEDGER_VERSION,
                token: self.token.clone(),
                protected,
                consented,
                ..Default::default()
            },
        )
    }

    /// What the ledger says. A ledger that is missing (a crash between saving the list and
    /// writing it, a list restored without its `ownership` folder), unreadable, from a newer
    /// version or for another token proves nothing, so every file is kept and the download
    /// still starts and finishes.
    fn load(&self, data: &Path) -> io::Result<Ledger> {
        self.check_token()?;
        let keep_everything = || Ledger {
            version: LEDGER_VERSION,
            token: self.token.clone(),
            protected: self.files.iter().map(|f| f.index).collect(),
            ..Default::default()
        };
        let bytes = match std::fs::read(self.ledger_path(data)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(keep_everything()),
            Err(e) => return Err(e),
        };
        Ok(match serde_json::from_slice::<Ledger>(&bytes) {
            Ok(ledger) if ledger.token == self.token && ledger.version == LEDGER_VERSION => ledger,
            _ => keep_everything(),
        })
    }

    fn save(&self, data: &Path, ledger: &Ledger) -> io::Result<()> {
        self.check_token()?;
        let path = self.ledger_path(data);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let bytes = serde_json::to_vec(ledger).map_err(io::Error::other)?;
        // Nothing changed: no rewrite and no flush (a start of a big torrent saves its record
        // each time).
        if std::fs::read(&path).is_ok_and(|old| old == bytes) {
            return Ok(());
        }
        let temp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(temp, path)
    }

    fn check_token(&self) -> io::Result<()> {
        if self.token.len() != 64 || !self.token.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(io::Error::other(crate::i18n::t(
                // i18n: error when the saved record of a torrent's files cannot be read as one
                "The torrent ownership record is invalid",
            )));
        }
        Ok(())
    }

    /// Make every file of the torrent ours before the engine gets the storage: each one is
    /// created here, empty, with `create_new`, and its identity recorded. The engine opens
    /// whatever is at a path for writing and sets its length, so a file that is neither ours
    /// nor agreed to at Add (one saved there since, or replaced while the torrent was stopped)
    /// stops the start: it would be cut and filled with torrent data. Unticked files count:
    /// the engine opens them too and writes the parts of pieces they share.
    pub fn prepare(
        &self,
        data: &Path,
        folder: &Path,
        dest: &Path,
    ) -> io::Result<Vec<mdm_torrent::authorized_storage::PreparedFile>> {
        self.prepare_consented(data, folder, dest, Vec::new())
    }

    /// Fresh consent transfers the exact open objects shown in the question. The engine
    /// receives these handles, not permission to reopen whatever later occupies their names.
    pub fn prepare_consented(
        &self,
        data: &Path,
        folder: &Path,
        dest: &Path,
        fresh: Vec<mdm_torrent::authorized_storage::PreparedFile>,
    ) -> io::Result<Vec<mdm_torrent::authorized_storage::PreparedFile>> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        use mdm_torrent::authorized_storage::PreparedFile;
        use std::collections::{HashMap, HashSet};
        let mut ledger = self.load(data)?;
        let mut opened = Vec::new();
        // What the ledger already says, looked up by file, and what this start adds to it:
        // a torrent of 50 000 files must not cost a scan of the ledger for each of them.
        let mut owned: HashMap<usize, Vec<Identity>> = HashMap::new();
        for r in ledger.files.iter().chain(&ledger.consented) {
            owned.entry(r.index).or_default().push(r.identity);
        }
        let mut protected: HashSet<usize> = ledger.protected.iter().copied().collect();
        let mut new_files: Vec<Receipt> = Vec::new();
        let mut new_consented: Vec<Receipt> = Vec::new();
        let mut new_protected: Vec<usize> = Vec::new();
        let mut new_directories: Vec<Receipt> = Vec::new();
        // Files whose earlier receipts a fresh agreement or a new object replaces.
        let mut reset: HashSet<usize> = HashSet::new();
        // Where each folder really is, found once, before its files are opened.
        let mut homes: HashMap<PathBuf, PathBuf> = HashMap::new();
        // Where the torrent's folder is, worked out once: every file's folder is checked against
        // this same answer, and the same `expected` the handle is then compared with, so a
        // folder swapped for a junction between the checks cannot make both agree on outside.
        let folder_key = key(folder);
        let multi = self.files.len() != 1;
        let result = (|| {
            let mut paths = Vec::new();
            // Validate every path before opening or allocating anything, even unticked and
            // deletion-protected entries. The engine opens every non-padding file.
            for claim in &self.files {
                mdm_torrent::check_relative_path(&claim.path).map_err(|_| {
                    io::Error::other(crate::i18n::t(
                        // i18n: unsafe path found in a saved torrent ownership record
                        "The torrent ownership record contains an unsafe path",
                    ))
                })?;
                let path = if self.files.len() == 1 {
                    dest.to_owned()
                } else {
                    folder.join(&claim.path)
                };
                if multi && !key(path.parent().unwrap()).starts_with(&folder_key) {
                    return Err(outside_error());
                }
                paths.push(path);
            }
            let mut fresh: HashMap<_, _> = fresh.into_iter().map(|f| (f.index, f)).collect();
            let mut missing = Vec::new();
            let mut unknown = Vec::new();
            for (claim, path) in self.files.iter().zip(paths) {
                if let Some(file) = fresh.remove(&claim.index) {
                    if file.path != path {
                        return Err(io::ErrorKind::InvalidInput.into());
                    }
                    if protected.insert(claim.index) {
                        new_protected.push(claim.index);
                    }
                    // The object the person agreed to, replacing what was recorded for it.
                    owned.remove(&claim.index);
                    reset.insert(claim.index);
                    if let Ok(id) = identity(&file.file) {
                        owned.entry(claim.index).or_default().push(id);
                        new_consented.push(Receipt {
                            index: claim.index,
                            path: claim.path.clone(),
                            identity: id,
                        });
                    }
                    opened.push(file);
                    continue;
                }
                match path.symlink_metadata() {
                    Ok(metadata) => {
                        if !metadata.is_file() || metadata.file_type().is_symlink() {
                            return Err(io::ErrorKind::InvalidInput.into());
                        }
                        // Open without create/truncate/set_len, then identify this handle.
                        // A name replacement before open cannot borrow the old object's grant.
                        let expected = expected_path(&mut homes, &path);
                        if multi && !expected.starts_with(&folder_key) {
                            return Err(outside_error());
                        }
                        let file = open_inside(&path, false, &expected)?;
                        let id = identity(&file).ok();
                        let allowed = id.is_some_and(|id| {
                            owned.get(&claim.index).is_some_and(|ids| ids.contains(&id))
                        });
                        let file = PreparedFile {
                            index: claim.index,
                            path,
                            file,
                        };
                        if allowed {
                            opened.push(file);
                        } else {
                            unknown.push(file);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push((claim, path)),
                    Err(e) => return Err(e),
                }
            }
            if !fresh.is_empty() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            if !unknown.is_empty() {
                return Err(consent_error(unknown));
            }
            // No foreign file remains unchecked. Allocate only with create_new; an object
            // winning this race needs its own consent, never a silent overwrite.
            for (claim, path) in missing {
                if multi && !key(path.parent().unwrap()).starts_with(&folder_key) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Self::mkdirs(folder, path.parent().unwrap(), &mut new_directories)?;
                // Resolved after the folders are made, and checked against the folder as it was
                // before: this answer is the one the handle is held to.
                let expected = expected_path(&mut homes, &path);
                if multi && !expected.starts_with(&folder_key) {
                    return Err(outside_error());
                }
                match open_inside(&path, true, &expected) {
                    Ok(file) => {
                        match identity(&file) {
                            Ok(id) => {
                                owned.entry(claim.index).or_default().push(id);
                                reset.insert(claim.index);
                                new_files.push(Receipt {
                                    index: claim.index,
                                    path: claim.path.clone(),
                                    identity: id,
                                });
                            }
                            Err(_) => {
                                // No identity on this filesystem: the file is never deleted
                                // by MDM, and each start asks before writing to it.
                                if protected.insert(claim.index) {
                                    new_protected.push(claim.index);
                                }
                            }
                        }
                        opened.push(PreparedFile {
                            index: claim.index,
                            path,
                            file,
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        // Whatever won the race must be a plain file, not a link that
                        // appeared meanwhile: it is opened without following one.
                        let file = open_inside(&path, false, &expected)?;
                        return Err(consent_error(vec![PreparedFile {
                            index: claim.index,
                            path,
                            file,
                        }]));
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })();
        // What was made or agreed to is recorded even when the start stops after it, so that
        // deleting the torrent finds it.
        ledger.files.retain(|r| !reset.contains(&r.index));
        ledger.consented.retain(|r| !reset.contains(&r.index));
        ledger.files.extend(new_files);
        ledger.consented.extend(new_consented);
        ledger.protected.extend(new_protected);
        ledger.directories.extend(new_directories);
        self.save(data, &ledger)?;
        result.map(|()| opened)
    }

    /// Make `path` and what is missing above it. Folders at and below `root` (the torrent's
    /// own) are recorded as made here; those above it (a Save to that doesn't exist yet, or its
    /// parents) are made as librqbit made them, with no record: they are the person's choice.
    fn mkdirs(root: &Path, path: &Path, made: &mut Vec<Receipt>) -> io::Result<()> {
        if path.exists() {
            return Ok(());
        }
        if !path.starts_with(root) {
            return std::fs::create_dir_all(path);
        }
        if path != root {
            let parent = path.parent().ok_or_else(|| {
                io::Error::other(crate::i18n::t(
                    // i18n: error when a torrent's output folder path cannot be used
                    "The output folder is invalid",
                ))
            })?;
            Self::mkdirs(root, parent, made)?;
        } else if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::create_dir(path) {
            Ok(()) => {
                if let Ok(id) = identity_at(path) {
                    made.push(Receipt {
                        index: usize::MAX,
                        path: path
                            .strip_prefix(root)
                            .map_err(io::Error::other)?
                            .to_owned(),
                        identity: id,
                    });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Before MDM moves the files itself: which of them are still the objects it made or the
    /// person agreed to use (their identity matches the record). Only those may be re-recorded
    /// afterwards ([`Self::rebind`]): a file someone put in their place is not. Each comes with
    /// its length, which a move keeps, so that a different file put there between the check and
    /// the move is told apart afterwards.
    pub fn verified(
        &self,
        data: &Path,
        folder: &Path,
        dest: &Path,
    ) -> io::Result<Vec<(usize, u64)>> {
        let ledger = self.load(data)?;
        let mut recorded: HashMap<usize, Vec<Identity>> = HashMap::new();
        for r in ledger.files.iter().chain(&ledger.consented) {
            recorded.entry(r.index).or_default().push(r.identity);
        }
        let mut ours = Vec::new();
        for (claim, path) in self.files.iter().zip(self.paths(folder, dest)) {
            let Some(ids) = recorded.get(&claim.index) else {
                continue;
            };
            if let Ok(metadata) = path.symlink_metadata()
                && metadata.file_type().is_file()
                && identity_at(&path).is_ok_and(|id| ids.contains(&id))
            {
                ours.push((claim.index, metadata.len()));
            }
        }
        Ok(ours)
    }

    /// MDM has moved the files itself. A copy to another drive and the original deleted give a
    /// file another volume and index, so the record would call MDM's own file a stranger's:
    /// record the identities the `verified` files have now, at their new place.
    pub fn rebind(
        &self,
        data: &Path,
        verified: &[(usize, u64)],
        folder: &Path,
        dest: &Path,
    ) -> io::Result<()> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut ledger = self.load(data)?;
        let mut changed = false;
        let verified: HashMap<usize, u64> = verified.iter().copied().collect();
        let mut now: HashMap<usize, Identity> = HashMap::new();
        for (claim, path) in self.files.iter().zip(self.paths(folder, dest)) {
            let Some(length) = verified.get(&claim.index) else {
                continue;
            };
            // Still a plain file of the length it had: not a link or another file put there.
            if !path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file() && m.len() == *length)
            {
                continue;
            }
            if let Ok(id) = identity_at(&path) {
                now.insert(claim.index, id);
            }
        }
        for receipt in ledger.files.iter_mut().chain(ledger.consented.iter_mut()) {
            if let Some(id) = now.get(&receipt.index)
                && receipt.identity != *id
            {
                receipt.identity = *id;
                changed = true;
            }
        }
        if changed {
            self.save(data, &ledger)?;
        }
        Ok(())
    }

    /// Delete what this torrent made: the files whose identity still matches their receipt
    /// (all of the torrent's files were made here, ticked or not), then the folders it made,
    /// when empty. Anything else, and anything another download claims, stays; the files of
    /// the torrent that stay are returned, for the person to be told.
    pub fn delete(
        &self,
        data: &Path,
        folder: &Path,
        dest: &Path,
        keep_root: bool,
        protected: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let lock = self.lock();
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let ledger = self.load(data)?;
        // Other downloads' files on another drive can't be this torrent's: leave their drives
        // asleep.
        let roots: HashSet<_> = self
            .paths(folder, dest)
            .iter()
            .chain([&folder.to_owned(), &dest.to_owned()])
            .filter_map(|p| root_of(p))
            .collect();
        let near: Vec<&PathBuf> = protected
            .iter()
            .filter(|p| root_of(p).is_some_and(|r| roots.contains(&r)))
            .collect();
        let reserved: BTreeSet<_> = near.iter().map(|p| key(p)).collect();
        let identities: HashSet<_> = near.iter().filter_map(|p| identity_at(p).ok()).collect();
        let mut failure = None;
        let protected_files: HashSet<usize> = ledger.protected.iter().copied().collect();
        for receipt in &ledger.files {
            if protected_files.contains(&receipt.index) {
                continue;
            }
            mdm_torrent::check_relative_path(&receipt.path).map_err(|_| {
                io::Error::other(crate::i18n::t(
                    // i18n: error when the saved record of a torrent's files holds a path that leaves its folder
                    "The torrent ownership record contains an unsafe path",
                ))
            })?;
            let path = if self.files.len() == 1 {
                dest.to_owned()
            } else {
                folder.join(&receipt.path)
            };
            if path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            if claimed(&reserved, &key(&path)) || identities.contains(&receipt.identity) {
                continue;
            }
            let Some(parent) = path.parent() else {
                continue;
            };
            if self.files.len() != 1 && !key(parent).starts_with(key(folder)) {
                continue;
            }
            match identity_at(&path) {
                Ok(id) if id == receipt.identity => {
                    if let Err(e) = remove_owned_torrent_file(&path, receipt.identity) {
                        failure.get_or_insert(e);
                    }
                }
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    failure.get_or_insert(e);
                }
                _ => {}
            }
        }
        let mut dirs: Vec<_> = ledger.directories.iter().collect();
        dirs.sort_by_key(|r| std::cmp::Reverse(r.path.components().count()));
        for receipt in dirs {
            if receipt.path.as_os_str().is_empty() && (keep_root || self.files.len() == 1) {
                continue;
            }
            if !receipt.path.as_os_str().is_empty()
                && mdm_torrent::check_relative_path(&receipt.path).is_err()
            {
                continue;
            }
            let path = folder.join(&receipt.path);
            if path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            if claimed(&reserved, &key(&path)) {
                continue;
            }
            if identity_at(&path).ok() == Some(receipt.identity) {
                match std::fs::remove_dir(path) {
                    Err(e)
                        if !matches!(
                            e.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                        ) =>
                    {
                        failure.get_or_insert(e);
                    }
                    _ => {}
                }
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        // What is still at the torrent's paths was not deleted: protected, not MDM's, or
        // another download's.
        Ok(self
            .paths(folder, dest)
            .into_iter()
            .filter(|p| p.symlink_metadata().is_ok_and(|m| m.file_type().is_file()))
            .collect())
    }
}

#[derive(Debug)]
struct ConsentRequired {
    files: Vec<mdm_torrent::authorized_storage::PreparedFile>,
}
impl std::fmt::Display for ConsentRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = self
            .files
            .first()
            .map(|file| file.path.display().to_string())
            .unwrap_or_default();
        f.write_str(&crate::i18n::fmt(
            crate::i18n::t(
                // i18n: torrent start needs explicit write permission; {file} is the first existing file without a matching grant
                "This torrent needs permission to use {file} before it can start.",
            ),
            &[("file", &crate::i18n::isolate(&path))],
        ))
    }
}
impl std::error::Error for ConsentRequired {}
fn consent_error(files: Vec<mdm_torrent::authorized_storage::PreparedFile>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, ConsentRequired { files })
}
pub fn take_consent(
    error: io::Error,
) -> Result<Vec<mdm_torrent::authorized_storage::PreparedFile>, io::Error> {
    if error.get_ref().is_some_and(|e| e.is::<ConsentRequired>()) {
        Ok(error
            .into_inner()
            .unwrap()
            .downcast::<ConsentRequired>()
            .unwrap()
            .files)
    } else {
        Err(error)
    }
}

/// Delete a one-file torrent added before ownership records, as deleting a torrent always
/// did: its file, by name, when it is a plain file (not a link) inside the folder it was saved
/// to and no other download claims it. Nothing else is touched.
pub fn delete_single_file(folder: &Path, dest: &Path, protected: &[PathBuf]) -> io::Result<()> {
    let regular = dest
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_file());
    let inside = dest
        .parent()
        .is_some_and(|p| key(p).starts_with(key(folder)));
    let reserved: BTreeSet<_> = protected.iter().map(|p| key(p)).collect();
    if !regular || !inside || claimed(&reserved, &key(dest)) {
        return Ok(());
    }
    match std::fs::remove_file(dest) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Delete a torrent added before ownership records, which has its own outer folder (the one
/// MDM made for it): its files that are regular files in that folder, and the folders
/// that empty, as deleting a torrent always did. Links, other downloads' files and anything
/// that is not one of the torrent's stay.
pub fn delete_outer_folder(bytes: &[u8], folder: &Path, protected: &[PathBuf]) -> io::Result<()> {
    let files = mdm_torrent::files_from_bytes(bytes).map_err(io::Error::other)?;
    let reserved: BTreeSet<_> = protected.iter().map(|p| key(p)).collect();
    let root = key(folder);
    let mut dirs = BTreeSet::new();
    let mut failure = None;
    for file in files.iter().filter(|f| !f.padding) {
        if mdm_torrent::check_relative_path(&file.path).is_err() {
            continue;
        }
        let path = folder.join(&file.path);
        let regular = path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_file());
        let inside = path.parent().is_some_and(|p| key(p).starts_with(&root));
        if !regular || !inside || claimed(&reserved, &key(&path)) {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            failure.get_or_insert(e);
        }
        for dir in path.ancestors().skip(1) {
            dirs.insert(dir.to_owned());
            if dir == folder {
                break;
            }
        }
    }
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let linked = dir
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink());
        if linked || claimed(&reserved, &key(&dir)) {
            continue;
        }
        match std::fs::remove_dir(&dir) {
            Err(e)
                if !matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                failure.get_or_insert(e);
            }
            _ => {}
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(paths: &[&str]) -> Ownership {
        Ownership {
            token: mdm_ipc::random_token(),
            files: paths
                .iter()
                .enumerate()
                .map(|(index, path)| Claim {
                    index,
                    path: PathBuf::from(path),
                })
                .collect(),
        }
    }

    #[test]
    fn direct_preexisting_files_replaced_files_and_preexisting_directories_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(root.join("existing")).unwrap();
        std::fs::write(
            root.join("README.txt"),
            b"already here; user consented to reuse",
        )
        .unwrap();
        let own = owner(&["README.txt", "existing/a.bin", "unticked.txt", "new/b.bin"]);
        own.initialize(
            &data,
            &root,
            &root,
            &consent_for(&[root.join("README.txt")]),
        )
        .unwrap();
        own.prepare(&data, &root, &root).unwrap();
        // The person saves their own file over one of the torrent's (a new object).
        std::fs::rename(root.join("unticked.txt"), tmp.path().join("moved-away")).unwrap();
        std::fs::write(root.join("unticked.txt"), b"user put this here after Add").unwrap();
        let kept = own.delete(&data, &root, &root, true, &[]).unwrap();
        // What stayed is reported, to be told to the person: the file they agreed to use and
        // the one that took a torrent file's name.
        assert_eq!(
            kept,
            vec![root.join("README.txt"), root.join("unticked.txt")]
        );
        assert_eq!(
            std::fs::read(root.join("README.txt")).unwrap(),
            b"already here; user consented to reuse"
        );
        assert_eq!(
            std::fs::read(root.join("unticked.txt")).unwrap(),
            b"user put this here after Add"
        );
        assert!(root.join("existing").is_dir());
        assert!(!root.join("existing/a.bin").exists());
        assert!(!root.join("new").exists());
    }

    #[test]
    fn direct_owned_objects_claimed_by_another_download_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["README.txt", "Sample/a.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        let protected = vec![root.join("README.txt"), root.join("Sample")];
        own.delete(tmp.path(), &root, &root, true, &protected)
            .unwrap();
        assert!(root.join("README.txt").is_file());
        assert!(root.join("Sample/a.bin").is_file());
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert!(!root.join("README.txt").exists());
        assert!(!root.join("Sample").exists());
        assert!(root.is_dir());
    }

    #[test]
    fn direct_replacement_file_is_not_the_object_the_torrent_created() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["README.txt", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        std::fs::rename(root.join("README.txt"), root.join("old-object.txt")).unwrap();
        std::fs::write(root.join("README.txt"), b"user replacement").unwrap();
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert_eq!(
            std::fs::read(root.join("README.txt")).unwrap(),
            b"user replacement"
        );
        assert!(!root.join("b.bin").exists());
    }

    #[test]
    fn direct_single_file_delete_uses_current_dest_after_move_and_rename() {
        for move_to_another_folder in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("output");
            std::fs::create_dir(&root).unwrap();
            let own = owner(&["original.bin"]);
            let original = root.join("original.bin");
            own.initialize(tmp.path(), &root, &original, &[]).unwrap();
            own.prepare(tmp.path(), &root, &original).unwrap();
            let parent = if move_to_another_folder {
                tmp.path().join("moved")
            } else {
                root.clone()
            };
            std::fs::create_dir_all(&parent).unwrap();
            let current = parent.join("completion-name.bin");
            std::fs::rename(&original, &current).unwrap();
            std::fs::write(&original, b"not the torrent's file").unwrap();
            let reloaded: Ownership =
                serde_json::from_str(&serde_json::to_string(&own).unwrap()).unwrap();
            reloaded
                .delete(tmp.path(), &root, &current, false, &[])
                .unwrap();
            assert!(!current.exists());
            assert_eq!(std::fs::read(&original).unwrap(), b"not the torrent's file");
            assert!(root.is_dir());
            assert!(parent.is_dir());
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn direct_a_mount_is_told_from_the_main_disk_without_opening_anything() {
        let on = |p: &str| root_of(Path::new(p));
        assert_eq!(on("/home/a/Downloads/x"), on("/home/b/y"));
        assert_ne!(on("/mnt/usb/x"), on("/home/a/x"));
        assert_ne!(on("/mnt/usb/x"), on("/mnt/nas/x"));
        assert_eq!(on("/mnt/usb/a/x"), on("/mnt/usb/b/y"));
        assert_ne!(on("/media/me/disk1/x"), on("/media/me/disk2/x"));
        assert_ne!(on("/run/media/me/disk1/x"), on("/home/me/x"));
        assert_ne!(on("/Volumes/Backup/x"), on("/Users/me/x"));
    }

    #[test]
    fn direct_a_move_to_another_drive_is_recorded_so_mdms_own_file_is_not_a_stranger() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin"]);
        let there = root.join("a.bin");
        own.initialize(tmp.path(), &root, &there, &[]).unwrap();
        own.prepare(tmp.path(), &root, &there).unwrap();
        std::fs::write(&there, b"the finished download").unwrap();
        let ours = own.verified(tmp.path(), &root, &there).unwrap();
        assert_eq!(ours.iter().map(|o| o.0).collect::<Vec<_>>(), vec![0]);
        // Move to another drive: a copy, then the original deleted. A new object.
        let moved = tmp.path().join("elsewhere");
        std::fs::create_dir(&moved).unwrap();
        let here = moved.join("a.bin");
        std::fs::copy(&there, &here).unwrap();
        std::fs::remove_file(&there).unwrap();
        // Unrecorded, the next start would ask about its own file (and delete would keep it).
        let error = own.prepare(tmp.path(), &moved, &here).unwrap_err();
        assert!(
            take_consent(error).is_ok(),
            "an unrecorded file is asked about"
        );
        own.rebind(tmp.path(), &ours, &moved, &here).unwrap();
        own.prepare(tmp.path(), &moved, &here)
            .expect("MDM's own moved file needs no question");
        own.delete(tmp.path(), &moved, &here, true, &[]).unwrap();
        assert!(!here.exists(), "and it is still the torrent's to delete");
    }

    #[test]
    fn direct_a_file_the_person_replaced_is_not_recorded_by_a_move() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        // One of them is replaced by the person's own file before the move.
        std::fs::rename(root.join("a.bin"), tmp.path().join("original")).unwrap();
        std::fs::write(root.join("a.bin"), b"the person's").unwrap();
        let ours = own.verified(tmp.path(), &root, &root).unwrap();
        assert_eq!(
            ours.iter().map(|o| o.0).collect::<Vec<_>>(),
            vec![1],
            "only the file that is still MDM's"
        );
        own.rebind(tmp.path(), &ours, &root, &root).unwrap();
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert!(root.join("a.bin").exists(), "the person's file stays");
        assert!(!root.join("b.bin").exists());
    }

    #[test]
    fn direct_a_second_start_does_not_rewrite_an_unchanged_record() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "b.bin", "c.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        let ledger = own.ledger_path(tmp.path());
        let first = std::fs::metadata(&ledger).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        own.prepare(tmp.path(), &root, &root).unwrap();
        assert_eq!(
            std::fs::metadata(&ledger).unwrap().modified().unwrap(),
            first
        );
    }

    #[test]
    fn direct_an_older_one_file_download_goes_by_name_inside_its_folder_only() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Downloads");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("movie.mkv");
        // Inside its folder, a plain file, claimed by no one: gone.
        std::fs::write(&file, b"movie").unwrap();
        delete_single_file(&folder, &file, &[]).unwrap();
        assert!(!file.exists());
        // Claimed by another download: stays.
        std::fs::write(&file, b"movie").unwrap();
        delete_single_file(&folder, &file, std::slice::from_ref(&file)).unwrap();
        assert!(file.exists());
        // A path that leads out of the folder it was saved to: stays.
        let outside = tmp.path().join("elsewhere.mkv");
        std::fs::write(&outside, b"not in the folder").unwrap();
        delete_single_file(&folder, &outside, &[]).unwrap();
        assert!(outside.exists());
        // A folder with that name is no file: stays.
        let dir = folder.join("a-folder.mkv");
        std::fs::create_dir(&dir).unwrap();
        delete_single_file(&folder, &dir, &[]).unwrap();
        assert!(dir.is_dir());
        // Already gone is not a failure.
        std::fs::remove_file(&file).unwrap();
        delete_single_file(&folder, &file, &[]).unwrap();
    }

    #[test]
    fn direct_file_appearing_while_queued_is_never_written_to() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["README.txt", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        std::fs::write(root.join("README.txt"), b"arrived after Later").unwrap();
        // The engine would open it, cut it to the torrent's size and fill it: the start stops.
        let error = own.prepare(tmp.path(), &root, &root).unwrap_err();
        assert!(error.to_string().contains("README.txt"), "{error}");
        assert!(!root.join("b.bin").exists(), "nothing else is set up");
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert_eq!(
            std::fs::read(root.join("README.txt")).unwrap(),
            b"arrived after Later"
        );
    }

    #[test]
    fn direct_add_time_consent_does_not_follow_a_replacement_filename() {
        for before_initialization in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("output");
            std::fs::create_dir(&root).unwrap();
            let path = root.join("README.txt");
            std::fs::write(&path, b"the file shown in the consent question").unwrap();
            let consent = consent_for(std::slice::from_ref(&path));
            let own = owner(&["README.txt", "another.bin"]);
            if !before_initialization {
                own.initialize(tmp.path(), &root, &root, &consent).unwrap();
            }
            std::fs::rename(&path, root.join("original-object")).unwrap();
            let replacement = b"a different user file, never consented to";
            std::fs::write(&path, replacement).unwrap();
            if before_initialization {
                own.initialize(tmp.path(), &root, &root, &consent).unwrap();
            }
            assert!(own.prepare(tmp.path(), &root, &root).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), replacement);
            assert!(!root.join("another.bin").exists());
            own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
            assert_eq!(std::fs::read(path).unwrap(), replacement);
        }
    }

    #[test]
    fn direct_ownership_and_created_directory_receipts_survive_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("owned-root");
        let own = owner(&["a.bin", "nested/b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        let reloaded: Ownership =
            serde_json::from_str(&serde_json::to_string(&own).unwrap()).unwrap();
        reloaded.prepare(tmp.path(), &root, &root).unwrap();
        reloaded
            .delete(tmp.path(), &root, &root, false, &[])
            .unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn direct_queued_claims_cover_internal_paths_and_file_directory_prefixes() {
        let tmp = tempfile::tempdir().unwrap();
        let claimed_paths = vec![
            tmp.path().join("README.txt"),
            tmp.path().join("Sample/a.bin"),
        ];
        let candidates = vec![
            tmp.path().join("README.txt"),
            tmp.path().join("Sample"),
            tmp.path().join("free.bin"),
        ];
        assert_eq!(
            conflicts(&candidates, &claimed_paths, true),
            candidates[..2]
        );
        #[cfg(windows)]
        assert_eq!(
            conflicts(&[tmp.path().join("readme.TXT")], &claimed_paths, false).len(),
            1
        );
    }

    #[test]
    fn direct_ownership_rejects_escaping_paths_and_invalid_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["../outside.bin", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        assert!(own.prepare(tmp.path(), &root, &root).is_err());
        assert!(!tmp.path().join("outside.bin").exists());
        let mut invalid = owner(&["a.bin"]);
        invalid.token = "../../escape".into();
        assert!(invalid.initialize(tmp.path(), &root, &root, &[]).is_err());
    }

    #[tokio::test]
    async fn direct_ownership_is_checked_against_the_saved_torrent_file_list_and_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.bin"), b"ownership a").unwrap();
        std::fs::write(source.join("nested/b.bin"), b"ownership b").unwrap();
        let spawner = mdm_torrent::librqbit::spawn_utils::BlockingSpawner::new(1);
        let made = mdm_torrent::librqbit::create_torrent(
            &source,
            mdm_torrent::librqbit::CreateTorrentOptions {
                name: Some("ownership-validation"),
                trackers: Vec::new(),
                piece_length: Some(16384),
            },
            &spawner,
        )
        .await
        .unwrap();
        let bytes = made.as_bytes().unwrap().to_vec();
        let hash = mdm_torrent::librqbit::torrent_from_bytes(&bytes)
            .unwrap()
            .info_hash
            .as_string();
        let mut own = Ownership {
            token: mdm_ipc::random_token(),
            files: mdm_torrent::files_from_bytes(&bytes)
                .unwrap()
                .into_iter()
                .enumerate()
                .map(|(index, f)| Claim {
                    index,
                    path: f.path,
                })
                .collect(),
        };
        own.validate(&bytes, &hash).unwrap();
        assert!(own.validate(&bytes, &"0".repeat(40)).is_err());
        own.files[0].path = "not-the-torrent.txt".into();
        assert!(own.validate(&bytes, &hash).is_err());
    }

    #[test]
    fn direct_refused_preparation_allocates_nothing_before_write_permission() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "later.txt"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        std::fs::write(root.join("later.txt"), b"user file").unwrap();
        assert!(own.prepare(tmp.path(), &root, &root).is_err());
        assert!(!root.join("a.bin").exists());
        own.delete(tmp.path(), &root, &root, false, &[]).unwrap();
        assert!(!root.join("a.bin").exists());
        assert_eq!(std::fs::read(root.join("later.txt")).unwrap(), b"user file");
        assert!(root.is_dir(), "the pre-existing empty root is not owned");
    }

    #[test]
    fn direct_unticked_files_are_made_here_first_so_ticking_one_later_needs_no_guess() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["selected.bin", "unticked.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        assert!(root.join("unticked.bin").is_file());
        // The engine writes into what is there, without replacing it.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(root.join("unticked.bin"))
                .unwrap();
            f.write_all(b"part of a shared piece").unwrap();
        }
        let reloaded: Ownership =
            serde_json::from_str(&serde_json::to_string(&own).unwrap()).unwrap();
        reloaded.prepare(tmp.path(), &root, &root).unwrap();
        reloaded.prepare(tmp.path(), &root, &root).unwrap();
        reloaded
            .delete(tmp.path(), &root, &root, true, &[])
            .unwrap();
        assert!(!root.join("selected.bin").exists());
        assert!(!root.join("unticked.bin").exists(), "no empty leftovers");
    }

    #[test]
    fn direct_a_replaced_file_stops_the_start_and_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["edited.bin", "other.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        own.prepare(tmp.path(), &root, &root).unwrap();
        let path = root.join("edited.bin");
        std::fs::rename(&path, tmp.path().join("original-object")).unwrap();
        std::fs::write(&path, b"saved by an editor").unwrap();
        // A resume would let the engine check its pieces against this file and rewrite it.
        assert!(own.prepare(tmp.path(), &root, &root).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"saved by an editor");
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert!(path.exists());
        assert!(!root.join("other.bin").exists());
    }

    #[test]
    fn direct_an_empty_file_is_asked_about_like_any_other_and_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        // An empty file (a `.gitkeep`, a link, one another program just made) and one with
        // content: both are the person's until they say otherwise.
        std::fs::write(root.join("a.bin"), b"").unwrap();
        std::fs::write(root.join("b.bin"), b"theirs").unwrap();
        let error = own.prepare(tmp.path(), &root, &root).unwrap_err();
        let asked = take_consent(error).unwrap();
        assert_eq!(asked.len(), 2);
        // Yes: they are used, and the answer is remembered.
        assert_eq!(
            own.prepare_consented(tmp.path(), &root, &root, asked)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(own.prepare(tmp.path(), &root, &root).unwrap().len(), 2);
        // They were the person's: deleting the torrent leaves them.
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert!(root.join("a.bin").exists());
        assert!(root.join("b.bin").exists());
    }

    #[test]
    fn direct_a_swapped_file_is_told_from_the_one_the_engine_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        let held = identities(&own.prepare(tmp.path(), &root, &root).unwrap());
        assert_eq!(held.len(), 2);
        // The same file again: not swapped.
        assert!(!swapped(
            &held,
            &identities(&own.prepare(tmp.path(), &root, &root).unwrap())
        ));
        // Replaced by another object, which the person agrees to use.
        let path = root.join("a.bin");
        std::fs::rename(&path, tmp.path().join("old")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"").unwrap();
        let asked = take_consent(own.prepare(tmp.path(), &root, &root).unwrap_err()).unwrap();
        let now = identities(
            &own.prepare_consented(tmp.path(), &root, &root, asked)
                .unwrap(),
        );
        assert!(swapped(&held, &now));
    }

    #[test]
    fn direct_damaged_ledger_recovers_without_granting_implicit_write_permission() {
        for damage in ["missing", "garbage", "newer", "other token"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("output");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("a.bin"), b"torrent data so far").unwrap();
            let own = owner(&["a.bin", "b.bin"]);
            own.initialize(tmp.path(), &root, &root, &[]).unwrap();
            let ledger = own.ledger_path(tmp.path());
            match damage {
                "missing" => std::fs::remove_file(&ledger).unwrap(),
                "garbage" => std::fs::write(&ledger, b"{not json").unwrap(),
                "newer" => std::fs::write(
                    &ledger,
                    format!(r#"{{"version":99,"token":"{}"}}"#, own.token),
                )
                .unwrap(),
                _ => std::fs::write(
                    &ledger,
                    format!(r#"{{"version":1,"token":"{}"}}"#, "0".repeat(64)),
                )
                .unwrap(),
            }
            let error = own.prepare(tmp.path(), &root, &root).unwrap_err();
            assert_eq!(
                std::fs::read(root.join("a.bin")).unwrap(),
                b"torrent data so far"
            );
            assert!(
                !root.join("b.bin").exists(),
                "refusal allocates nothing: {damage}"
            );
            let fresh = take_consent(error).unwrap();
            drop(
                own.prepare_consented(tmp.path(), &root, &root, fresh)
                    .unwrap(),
            );
            own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
            assert!(root.join("a.bin").exists(), "{damage}");
        }
    }

    #[test]
    fn direct_weak_ledger_migrates_to_protection_without_write_consent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("output");
        std::fs::create_dir(&root).unwrap();
        let own = owner(&["a.bin", "b.bin"]);
        std::fs::write(root.join("b.bin"), b"user's data").unwrap();
        std::fs::create_dir_all(tmp.path().join("ownership")).unwrap();
        std::fs::write(
            own.ledger_path(tmp.path()),
            format!(r#"{{"token":"{}","protected":[1]}}"#, own.token),
        )
        .unwrap();
        let error = own.prepare(tmp.path(), &root, &root).unwrap_err();
        assert!(!root.join("a.bin").exists());
        assert_eq!(std::fs::read(root.join("b.bin")).unwrap(), b"user's data");
        drop(
            own.prepare_consented(tmp.path(), &root, &root, take_consent(error).unwrap())
                .unwrap(),
        );
        own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert!(root.join("a.bin").exists());
        assert_eq!(std::fs::read(root.join("b.bin")).unwrap(), b"user's data");
    }

    #[test]
    fn direct_identity_has_a_creation_time_so_a_file_that_took_the_name_later_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cover.jpg");
        std::fs::write(&path, b"one").unwrap();
        let first = identity_at(&path).expect("this filesystem gives an identity");
        assert_ne!(first[3], 0, "a creation time is part of it");
        assert_eq!(identity_at(&path).unwrap(), first);
    }

    #[tokio::test]
    async fn direct_an_older_download_with_its_own_folder_still_deletes_its_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.bin"), b"ownership a").unwrap();
        std::fs::write(source.join("nested/b.bin"), b"ownership b").unwrap();
        let spawner = mdm_torrent::librqbit::spawn_utils::BlockingSpawner::new(1);
        let made = mdm_torrent::librqbit::create_torrent(
            &source,
            mdm_torrent::librqbit::CreateTorrentOptions {
                name: Some("Set"),
                trackers: Vec::new(),
                piece_length: Some(16384),
            },
            &spawner,
        )
        .await
        .unwrap();
        let bytes = made.as_bytes().unwrap().to_vec();
        let folder = tmp.path().join("Downloads/Set");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("a.bin"), b"torrent").unwrap();
        std::fs::write(folder.join("nested/b.bin"), b"torrent").unwrap();
        std::fs::write(folder.join("notes.txt"), b"the person's own note").unwrap();
        delete_outer_folder(&bytes, &folder, &[]).unwrap();
        assert!(!folder.join("a.bin").exists());
        assert!(!folder.join("nested").exists());
        assert_eq!(
            std::fs::read(folder.join("notes.txt")).unwrap(),
            b"the person's own note"
        );
        assert!(folder.is_dir(), "a folder with something else in it stays");
        // Another download's file stays too.
        std::fs::write(folder.join("a.bin"), b"claimed").unwrap();
        delete_outer_folder(&bytes, &folder, &[folder.join("a.bin")]).unwrap();
        assert!(folder.join("a.bin").exists());
    }

    #[test]
    fn direct_a_save_to_that_does_not_exist_yet_is_made_and_only_the_torrents_own_folder_is_recorded()
     {
        let tmp = tempfile::tempdir().unwrap();
        // RSS and link grabber adds go to a category folder that may not exist: neither the
        // folder nor its parents (nor a drive's folder that is not plugged in yet).
        let save_to = tmp.path().join("Downloads").join("Torrents");
        let folder = save_to.join("Album");
        let own = owner(&["a.bin", "sub/b.bin"]);
        own.initialize(tmp.path(), &folder, &folder, &[]).unwrap();
        own.prepare(tmp.path(), &folder, &folder).unwrap();
        assert!(folder.join("a.bin").is_file());
        assert!(folder.join("sub/b.bin").is_file());
        // Deleting removes what was made for the torrent, not the person's Downloads.
        own.delete(tmp.path(), &folder, &folder, false, &[])
            .unwrap();
        assert!(!folder.exists());
        assert!(save_to.is_dir(), "Save to was not recorded as made here");
        // A one-file torrent: its file's folder is made, and stays when the file goes.
        let direct = tmp.path().join("Other").join("Place");
        let file = direct.join("x.bin");
        let own = owner(&["x.bin"]);
        own.initialize(tmp.path(), &direct, &file, &[]).unwrap();
        own.prepare(tmp.path(), &direct, &file).unwrap();
        assert!(file.is_file());
        own.delete(tmp.path(), &direct, &file, true, &[]).unwrap();
        assert!(!file.exists());
        assert!(direct.is_dir());
        assert!(tmp.path().join("Other").is_dir());
    }

    #[test]
    fn direct_a_torrent_added_before_records_takes_the_files_its_engine_wrote_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Album");
        std::fs::create_dir_all(&root).unwrap();
        // A half-downloaded torrent of an older MDM: the files are there, sized, non-empty.
        std::fs::write(root.join("a.bin"), vec![1u8; 4096]).unwrap();
        std::fs::write(root.join("b.bin"), vec![2u8; 4096]).unwrap();
        let own = owner(&["a.bin", "b.bin"]);
        // With no record they would be asked about (another record, so this one stays new).
        let probe = owner(&["a.bin", "b.bin"]);
        assert!(take_consent(probe.prepare(tmp.path(), &root, &root).unwrap_err()).is_ok());
        own.adopt_existing(tmp.path(), &root, &root).unwrap();
        assert_eq!(own.prepare(tmp.path(), &root, &root).unwrap().len(), 2);
        // Their bytes are untouched, and they are never deleted through this record.
        assert_eq!(std::fs::read(root.join("a.bin")).unwrap().len(), 4096);
        let kept = own.delete(tmp.path(), &root, &root, true, &[]).unwrap();
        assert_eq!(kept.len(), 2);
        assert!(root.join("a.bin").exists() && root.join("b.bin").exists());
        // Once: a record that exists is not widened. A file that takes a name later is a
        // stranger's, and is asked about.
        std::fs::rename(root.join("a.bin"), tmp.path().join("old")).unwrap();
        std::fs::write(root.join("a.bin"), b"another program's").unwrap();
        own.adopt_existing(tmp.path(), &root, &root).unwrap();
        assert!(own.prepare(tmp.path(), &root, &root).is_err());
    }

    #[test]
    fn direct_files_agreed_to_at_add_are_looked_up_not_compared_pair_by_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Big");
        std::fs::create_dir_all(&root).unwrap();
        let names: Vec<String> = (0..1500).map(|i| format!("f{i}.bin")).collect();
        for name in &names {
            std::fs::write(root.join(name), b"already here").unwrap();
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let own = owner(&refs);
        let paths: Vec<PathBuf> = names.iter().map(|n| root.join(n)).collect();
        let existing = consent_for(&paths);
        let started = std::time::Instant::now();
        own.initialize(tmp.path(), &root, &root, &existing).unwrap();
        // Comparing every pair (each with its own look at the disk) took about 8 s at this
        // size; looking each path up once takes a fraction of a second.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(own.prepare(tmp.path(), &root, &root).unwrap().len(), 1500);
    }

    #[test]
    fn direct_other_downloads_files_are_looked_at_once_for_a_batch_and_give_the_same_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Shared");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.bin"), b"x").unwrap();
        let mine = vec![root.join("a.bin"), root.join("b.bin")];
        let claims = vec![root.join("A.BIN"), root.join("c.bin")];
        let cache = ClaimCache::default();
        let plain = conflicts(&mine, &claims, true);
        // The same cache serves every check of the batch.
        for _ in 0..3 {
            assert_eq!(conflicts_cached(&mine, &claims, true, Some(&cache)), plain);
        }
        assert!(!plain.contains(&root.join("b.bin")));
    }

    #[test]
    fn direct_swapped_verified_and_rebind_agree_with_what_they_did_before_on_a_big_torrent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Big");
        std::fs::create_dir_all(&root).unwrap();
        let names: Vec<String> = (0..1000).map(|i| format!("f{i}.bin")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let own = owner(&refs);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        let held = identities(&own.prepare(tmp.path(), &root, &root).unwrap());
        assert!(!swapped(&held, &held));
        let verified = own.verified(tmp.path(), &root, &root).unwrap();
        assert_eq!(verified.len(), 1000);
        own.rebind(tmp.path(), &verified, &root, &root).unwrap();
        // One file replaced: it is no longer verified.
        std::fs::rename(root.join("f7.bin"), tmp.path().join("old")).unwrap();
        std::fs::write(root.join("f7.bin"), b"other").unwrap();
        assert_eq!(own.verified(tmp.path(), &root, &root).unwrap().len(), 999);
    }

    /// A pipe at a claimed path must not be opened (it would wait for a writer for ever, and
    /// the torrent would stay busy until MDM restarted); a folder MDM made has an identity.
    #[cfg(unix)]
    #[test]
    fn direct_a_pipe_at_a_path_is_not_opened_and_a_folder_has_an_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            assert!(identity_at(&fifo).is_err());
            // Nor does removing by identity wait on it or follow it.
            assert!(remove_owned_torrent_file(&fifo, [0; 5]).is_err());
        }
        assert!(identity_at(tmp.path()).is_ok());
        assert!(identity_at(&tmp.path().join("missing")).is_err());
    }

    /// Deleting a torrent while a program still has its files open (the engine on its way out,
    /// an antivirus scanning files just made) must remove them, as it always did: on Windows
    /// the delete went through a handle that refused to share, and about one file in twenty of
    /// a big torrent was left behind with "Access is denied".
    #[test]
    fn direct_files_another_program_still_has_open_are_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        std::fs::create_dir_all(&root).unwrap();
        let names: Vec<String> = (0..40).map(|i| format!("d{}/f{i}.bin", i % 4)).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let own = owner(&refs);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        let held = own.prepare(tmp.path(), &root, &root).unwrap();
        for f in &held {
            use std::io::Write;
            (&f.file).write_all(b"data").unwrap();
        }
        let kept = own.delete(tmp.path(), &root, &root, false, &[]).unwrap();
        assert!(kept.is_empty(), "{kept:?}");
        drop(held);
        // Every file and the folders MDM made went; the folder the person made stays, empty.
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    /// A folder link (a junction on Windows) from `link` to `target`.
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(made.status.success(), "{made:?}");
        }
    }

    /// The folders are checked first, then the file is made: a folder swapped for a link in
    /// between must not let the file be made outside the destination.
    #[test]
    fn direct_a_folder_swapped_for_a_link_after_the_check_gets_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        let sub = root.join("sub");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let mut homes = HashMap::new();
        let file = sub.join("f.bin");
        let expected = expected_path(&mut homes, &file);
        std::fs::remove_dir(&sub).unwrap();
        link_dir(&outside, &sub);
        let refused = open_inside(&file, true, &expected);
        assert!(refused.is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // The same folder, unswapped, is fine.
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let expected = expected_path(&mut homes, &plain.join("g.bin"));
        assert!(open_inside(&plain.join("g.bin"), true, &expected).is_ok());
    }

    /// A link as the file's own name is never opened through, whether it is there already or
    /// appears between the check and the open.
    #[cfg(unix)]
    #[test]
    fn direct_a_link_at_the_file_name_is_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        std::fs::create_dir_all(&root).unwrap();
        let elsewhere = tmp.path().join("elsewhere.txt");
        std::fs::write(&elsewhere, b"mine").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("f.bin")).unwrap();
        let mut homes = HashMap::new();
        let expected = expected_path(&mut homes, &root.join("f.bin"));
        assert!(open_inside(&root.join("f.bin"), false, &expected).is_err());
        assert!(open_inside(&root.join("f.bin"), true, &expected).is_err());
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"mine");
    }

    /// Save to is itself a link to another place (a junction to another drive, a symlinked
    /// Documents): the torrent starts, writes where the link leads, and is deleted again.
    #[test]
    fn direct_a_save_to_that_is_a_link_works_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let linked = tmp.path().join("linked");
        link_dir(&real, &linked);
        let root = linked.join("Set");
        let own = owner(&["a.bin", "d/b.bin", "d/e/c.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        let held = own.prepare(tmp.path(), &root, &root).unwrap();
        assert_eq!(held.len(), 3);
        assert!(real.join("Set/d/e/c.bin").is_file());
        drop(held);
        // A second start with the objects it made recorded: no question.
        drop(own.prepare(tmp.path(), &root, &root).unwrap());
        let kept = own.delete(tmp.path(), &root, &root, false, &[]).unwrap();
        assert!(kept.is_empty(), "{kept:?}");
        assert!(!real.join("Set/a.bin").exists());
    }

    /// The folder written differently from how the system names it (case on Windows and
    /// macOS, a trailing separator, `.` and `..` parts) is still the same folder.
    #[test]
    fn direct_a_folder_named_another_way_is_not_taken_for_a_different_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        std::fs::create_dir_all(root.join("Sub")).unwrap();
        let odd = tmp.path().join("x").join("..").join("Set");
        std::fs::create_dir_all(tmp.path().join("x")).unwrap();
        let own = owner(&["Sub/a.bin", "Sub/b.bin"]);
        own.initialize(tmp.path(), &odd, &odd, &[]).unwrap();
        assert_eq!(own.prepare(tmp.path(), &odd, &odd).unwrap().len(), 2);
        #[cfg(any(windows, target_os = "macos"))]
        {
            let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
            let own = owner(&["Sub/c.bin", "Sub/d.bin"]);
            own.initialize(tmp.path(), &upper, &upper, &[]).unwrap();
            assert_eq!(own.prepare(tmp.path(), &upper, &upper).unwrap().len(), 2);
            assert!(root.join("Sub/c.bin").is_file());
        }
    }

    /// Another program swaps a folder for a link and back as fast as it can while files are
    /// made in it: whatever is made is where it was asked for, and what could not be made
    /// there is not left anywhere.
    #[test]
    fn direct_a_folder_flipping_between_real_and_link_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        let sub = root.join("sub");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let lnk = root.join("lnk");
        link_dir(&outside, &lnk);
        let parked = root.join("parked");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flipper = {
            let (stop, sub, lnk, parked) = (stop.clone(), sub.clone(), lnk.clone(), parked.clone());
            std::thread::spawn(move || {
                let mut flips = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    // The real folder out of the way and the link in its place, then back.
                    if std::fs::rename(&sub, &parked).is_ok() {
                        if std::fs::rename(&lnk, &sub).is_ok() {
                            flips += 1;
                            std::thread::sleep(std::time::Duration::from_micros(300));
                            let _ = std::fs::rename(&sub, &lnk);
                        }
                        let _ = std::fs::rename(&parked, &sub);
                    }
                }
                flips
            })
        };
        let mut refused = 0;
        let mut kinds: std::collections::BTreeMap<String, u32> = Default::default();
        let mut made = std::collections::BTreeSet::new();
        for i in 0..150 {
            let file = sub.join(format!("f{i}.bin"));
            let mut homes = HashMap::new();
            let expected = expected_path(&mut homes, &file);
            match open_inside(&file, true, &expected) {
                Ok(f) => {
                    made.insert(format!("f{i}.bin"));
                    // open_inside has just checked where it is; asking again here would see
                    // a folder that has been flipped since.
                    drop(f);
                }
                Err(e) => {
                    refused += 1;
                    *kinds.entry(format!("{:?}", e.kind())).or_insert(0) += 1;
                }
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let flips = flipper.join().unwrap();
        // Nothing made while the folder was not the one asked for is left in the other place.
        let left: Vec<_> = std::fs::read_dir(&outside)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        eprintln!(
            "{flips} flips; refused {refused} of 150 {kinds:?}, {} files in outside",
            left.len()
        );
        // A file in `outside` is allowed only when it was made and kept as asked: the link was
        // in place when its folder was resolved. Any other was made through the link after the
        // check and not taken out again.
        let leaked: Vec<_> = left
            .iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !made.contains(n))
            .collect();
        assert!(leaked.is_empty(), "leaked through the link: {leaked:?}");
    }

    /// A delete and a start of the same torrent on two threads must neither deadlock nor
    /// lose the record.
    #[test]
    fn direct_a_start_and_a_delete_on_two_threads_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        std::fs::create_dir_all(&root).unwrap();
        let names: Vec<String> = (0..200).map(|i| format!("d{}/f{i}.bin", i % 5)).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let own = std::sync::Arc::new(owner(&refs));
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        drop(own.prepare(tmp.path(), &root, &root).unwrap());
        let data = tmp.path().to_owned();
        let threads: Vec<_> = (0..4)
            .map(|n| {
                let (own, data, root) = (own.clone(), data.clone(), root.clone());
                std::thread::spawn(move || {
                    for _ in 0..5 {
                        if n % 2 == 0 {
                            let _ = own.prepare(&data, &root, &root);
                        } else {
                            let _ = own.delete(&data, &root, &root, false, &[]);
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // Whatever order they ran in, one more delete leaves nothing of the torrent.
        let kept = own.delete(tmp.path(), &root, &root, false, &[]).unwrap();
        assert!(kept.is_empty(), "{kept:?}");
    }

    /// A folder of the torrent that leads outside it (a link) stops the start with nothing made
    /// there. (The swap between the checks and the open cannot be timed; the flipping test above
    /// makes it happen as often as it can.)
    #[test]
    fn direct_a_folder_that_resolves_outside_the_torrents_stops_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Set");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        link_dir(&outside, &root.join("d"));
        let own = owner(&["d/a.bin", "d/b.bin"]);
        own.initialize(tmp.path(), &root, &root, &[]).unwrap();
        assert!(own.prepare(tmp.path(), &root, &root).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }
}
