//! App-authorized file handles. Storage never reopens a torrent path for writing.
//!
//! An unticked file the app didn't hand over is never made at its path: the parts of the pieces
//! it shares with ticked files (its first and last piece at most) go to a parts file of MDM's
//! own, `<session>/<hash>.parts/<index>`, as other BitTorrent clients do. Ticked later, it is
//! made then, and what the parts file held is copied into it.
use librqbit::storage::TorrentStorage;
use librqbit::{ManagedTorrentShared, TorrentMetadata};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock},
};

#[derive(Debug)]
pub struct PreparedFile {
    pub index: usize,
    pub path: PathBuf,
    pub file: File,
}

/// What the app hands the engine: the files it checked, and the unticked ones it didn't make,
/// whose shared pieces go to a parts file instead (`parted`, by index).
#[derive(Debug, Default)]
pub struct Prepared {
    pub files: Vec<PreparedFile>,
    pub parted: Vec<usize>,
}

impl From<Vec<PreparedFile>> for Prepared {
    fn from(files: Vec<PreparedFile>) -> Self {
        Self {
            files,
            parted: Vec::new(),
        }
    }
}

type Reopen = Arc<dyn Fn() -> io::Result<Prepared> + Send + Sync>;
struct Grant {
    folder: PathBuf,
    first: Mutex<Option<Prepared>>,
    reopen: Reopen,
}
type Key = (PathBuf, String);
fn grants() -> &'static Mutex<HashMap<Key, Arc<Grant>>> {
    static GRANTS: OnceLock<Mutex<HashMap<Key, Arc<Grant>>>> = OnceLock::new();
    GRANTS.get_or_init(Default::default)
}

/// Which files of each torrent are in a parts file now (by index), as its storage last opened.
fn parted() -> &'static Mutex<HashMap<Key, HashSet<usize>>> {
    static PARTED: OnceLock<Mutex<HashMap<Key, HashSet<usize>>>> = OnceLock::new();
    PARTED.get_or_init(Default::default)
}

/// The first storage initialization consumes exactly the handles checked by the app.
/// A subsequent initialization (file selection or session restart) must authorize again.
/// Every file but padding must be among `files`.
pub fn authorize(
    session: &Path,
    hash: &str,
    folder: &Path,
    files: Vec<PreparedFile>,
    reopen: Arc<dyn Fn() -> io::Result<Vec<PreparedFile>> + Send + Sync>,
) {
    authorize_parted(
        session,
        hash,
        folder,
        files.into(),
        Arc::new(move || reopen().map(Prepared::from)),
    );
}

/// [`authorize`], where the files in `prepared.parted` (unticked, not made) are kept in a parts
/// file.
pub fn authorize_parted(
    session: &Path,
    hash: &str,
    folder: &Path,
    prepared: Prepared,
    reopen: Reopen,
) {
    grants().lock().unwrap().insert(
        (session.to_owned(), hash.to_owned()),
        Arc::new(Grant {
            folder: folder.to_owned(),
            first: Mutex::new(Some(prepared)),
            reopen,
        }),
    );
}

pub fn forget(session: &Path, hash: &str) {
    grants()
        .lock()
        .unwrap()
        .remove(&(session.to_owned(), hash.to_owned()));
}

/// Whether file `index` of the torrent is kept in a parts file by its running storage: ticking
/// it needs the torrent opened again, on a file the app makes.
pub fn is_parted(session: &Path, hash: &str, index: usize) -> bool {
    parted()
        .lock()
        .unwrap()
        .get(&(session.to_owned(), hash.to_owned()))
        .is_some_and(|p| p.contains(&index))
}

/// Whether file `index` of the torrent has bytes kept for it in a parts file, or is kept in one
/// by its running storage: ticking it needs the torrent in the session, opened on a file the app
/// makes. Read from the disk too, so it holds for a torrent out of the session (finished, the
/// session stopped while idle) and after MDM starts again.
pub fn has_parts(session: &Path, hash: &str, index: usize) -> bool {
    is_parted(session, hash, index)
        || parts_dir(session, hash).is_some_and(|d| d.join(index.to_string()).is_file())
}

/// The torrent's parts files, under the session's folder (None: not an info hash).
fn parts_dir(session: &Path, hash: &str) -> Option<PathBuf> {
    (hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| session.join(format!("{}.parts", hash.to_ascii_lowercase())))
}

/// Delete the torrent's parts files (it is taken off the list).
pub fn remove_parts(session: &Path, hash: &str) -> io::Result<()> {
    parted()
        .lock()
        .unwrap()
        .remove(&(session.to_owned(), hash.to_owned()));
    let Some(dir) = parts_dir(session, hash) else {
        return Ok(());
    };
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Where an unticked file's bytes are kept. Only the pieces it shares with ticked files are
/// ever written, and those are within its first and its last piece: the parts file holds those
/// two (a file of two pieces or less, all of it at the same offsets).
struct Parts {
    path: PathBuf,
    len: u64,
    piece: u64,
    file: Mutex<Option<Arc<File>>>,
}

impl Parts {
    fn new(path: PathBuf, len: u64, piece: u64) -> Self {
        Self {
            path,
            len,
            piece,
            file: Mutex::new(None),
        }
    }

    /// Where `n` bytes at `offset` of the file are in the parts file (None: in the middle of a
    /// long file, never shared with a ticked one).
    fn map(&self, offset: u64, n: u64) -> Option<u64> {
        map_offset(self.len, self.piece, offset, n)
    }

    /// What the parts file holds at `offset` of the file; zeros for what it doesn't (never
    /// written, or the middle of a long file). A piece read so fails its hash and counts as
    /// missing, and an error here would make librqbit's check skip the rest of the file, the
    /// last piece it shares with the next file among them.
    fn read(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        buf.fill(0);
        let Some(at) = self.map(offset, buf.len() as u64) else {
            return Ok(());
        };
        let file = match self.open(false) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut done = 0;
        while done < buf.len() {
            #[cfg(windows)]
            let n = {
                use std::os::windows::fs::FileExt;
                file.seek_read(&mut buf[done..], at + done as u64)?
            };
            #[cfg(unix)]
            let n = {
                use std::os::unix::fs::FileExt;
                file.read_at(&mut buf[done..], at + done as u64)?
            };
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(())
    }

    fn open(&self, create: bool) -> io::Result<Arc<File>> {
        let mut slot = self.file.lock().map_err(|_| io::Error::other("lock"))?;
        if let Some(f) = slot.as_ref() {
            return Ok(f.clone());
        }
        if create && let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = Arc::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(create)
                .truncate(false)
                .open(&self.path)?,
        );
        *slot = Some(f.clone());
        Ok(f)
    }
}

fn map_offset(len: u64, piece: u64, offset: u64, n: u64) -> Option<u64> {
    let end = offset.checked_add(n)?;
    if end > len {
        return None;
    }
    if len <= piece.saturating_mul(2) || end <= piece {
        return Some(offset);
    }
    let tail = len - piece;
    (offset >= tail).then(|| piece + (offset - tail))
}

/// Copy what an unticked file's parts file held into the file made for it now that it is
/// ticked, at the offsets it has in the file. Only into a file still empty: one with bytes
/// already is the file itself.
fn move_parts_into(parts: &Path, file: &File, len: u64, piece: u64) -> io::Result<()> {
    let held = match std::fs::read(parts) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() == 0 && !held.is_empty() {
        let piece_us = usize::try_from(piece).unwrap_or(usize::MAX);
        if len <= piece.saturating_mul(2) {
            let n = held.len().min(usize::try_from(len).unwrap_or(usize::MAX));
            write_at(file, 0, &held[..n])?;
        } else {
            write_at(file, 0, &held[..held.len().min(piece_us)])?;
            if held.len() > piece_us {
                let rest = &held[piece_us..];
                let rest = &rest[..rest.len().min(piece_us)];
                write_at(file, len - piece, rest)?;
            }
        }
        file.sync_data()?;
    }
    std::fs::remove_file(parts)
}

fn read_at(file: &File, mut offset: u64, buf: &mut [u8]) -> io::Result<()> {
    let mut rest = buf;
    while !rest.is_empty() {
        #[cfg(windows)]
        let n = {
            use std::os::windows::fs::FileExt;
            file.seek_read(rest, offset)?
        };
        #[cfg(unix)]
        let n = {
            use std::os::unix::fs::FileExt;
            file.read_at(rest, offset)?
        };
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        offset += n as u64;
        rest = &mut rest[n..];
    }
    Ok(())
}

fn write_at(file: &File, mut offset: u64, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = {
            use std::os::windows::fs::FileExt;
            file.seek_write(buf, offset)?
        };
        #[cfg(unix)]
        let n = {
            use std::os::unix::fs::FileExt;
            file.write_at(buf, offset)?
        };
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        offset += n as u64;
        buf = &buf[n..];
    }
    Ok(())
}

enum Slot {
    Padding,
    // One lock per file: positioned writes to different pieces remain concurrent.
    File(RwLock<Option<File>>),
    Parts(Parts),
}

pub(crate) struct Storage {
    session: PathBuf,
    hash: String,
    files: Vec<Slot>,
}
impl Storage {
    pub(crate) fn new(session: PathBuf, hash: String) -> Self {
        Self {
            session,
            hash,
            files: Vec::new(),
        }
    }
    fn with_file<T>(
        &self,
        index: usize,
        padding: impl FnOnce() -> T,
        op: impl FnOnce(&File) -> io::Result<T>,
    ) -> anyhow::Result<T> {
        let slot = self
            .files
            .get(index)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let slot = match slot {
            Slot::Padding => return Ok(padding()),
            Slot::Parts(_) => return Err(io::Error::from(io::ErrorKind::InvalidInput).into()),
            Slot::File(slot) => slot,
        };
        let file = slot
            .read()
            .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
        Ok(op(file.as_ref().ok_or_else(|| {
            io::Error::from(io::ErrorKind::NotConnected)
        })?)?)
    }
}

impl TorrentStorage for Storage {
    fn init(
        &mut self,
        _shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        let key = (self.session.clone(), self.hash.clone());
        let grant = grants()
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
        let first = grant.first.lock().unwrap().take();
        let prepared = match first {
            Some(p) => p,
            None => (grant.reopen)()?,
        };
        let parted_ok: HashSet<usize> = prepared.parted.into_iter().collect();
        let mut prepared: HashMap<_, _> =
            prepared.files.into_iter().map(|f| (f.index, f)).collect();
        let dir = parts_dir(&self.session, &self.hash)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let piece = u64::from(metadata.lengths().default_piece_length());
        let mut opened = Vec::with_capacity(metadata.file_infos.len());
        let mut now_parted = HashSet::new();
        for (index, info) in metadata.file_infos.iter().enumerate() {
            if info.attrs.padding {
                opened.push(Slot::Padding);
                continue;
            }
            let parts = dir.join(index.to_string());
            match prepared.remove(&index) {
                Some(file) => {
                    if file.path != grant.folder.join(&info.relative_filename) {
                        return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
                    }
                    // Ticked since it was kept in a parts file: what it held goes in.
                    move_parts_into(&parts, &file.file, info.len, piece)?;
                    opened.push(Slot::File(RwLock::new(Some(file.file))));
                }
                None if parted_ok.contains(&index) => {
                    now_parted.insert(index);
                    opened.push(Slot::Parts(Parts::new(parts, info.len, piece)));
                }
                None => return Err(io::Error::from(io::ErrorKind::PermissionDenied).into()),
            }
        }
        if !prepared.is_empty() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        parted().lock().unwrap().insert(key, now_parted);
        self.files = opened;
        Ok(())
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        match self.files.get(file_id) {
            // Handle padding separately because both closures cannot borrow the output mutably.
            Some(Slot::Padding) => {
                buf.fill(0);
                return Ok(());
            }
            Some(Slot::Parts(p)) => return Ok(p.read(offset, buf)?),
            _ => {}
        }
        self.with_file(file_id, || (), |file| read_at(file, offset, buf))
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        if let Some(Slot::Parts(p)) = self.files.get(file_id) {
            let at = p
                .map(offset, buf.len() as u64)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let file = p.open(true)?;
            return Ok(write_at(&file, at, buf)?);
        }
        self.with_file(file_id, || (), |file| write_at(file, offset, buf))
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        // A parts file holds what was written and no more.
        if let Some(Slot::Parts(_)) = self.files.get(file_id) {
            return Ok(());
        }
        self.with_file(
            file_id,
            || (),
            |file| mdm_engine::disk::resize_download_file(file, length),
        )
    }
    fn remove_file(&self, _: usize, _: &Path) -> anyhow::Result<()> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
    }
    fn remove_directory_if_empty(&self, _: &Path) -> anyhow::Result<()> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
    }
    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        let mut files = Vec::with_capacity(self.files.len());
        for slot in &self.files {
            files.push(match slot {
                Slot::Padding => Slot::Padding,
                Slot::File(slot) => Slot::File(RwLock::new(
                    slot.write()
                        .map_err(|_| io::Error::from(io::ErrorKind::Other))?
                        .take(),
                )),
                Slot::Parts(p) => {
                    let file = p
                        .file
                        .lock()
                        .map_err(|_| io::Error::from(io::ErrorKind::Other))?
                        .take();
                    Slot::Parts(Parts {
                        path: p.path.clone(),
                        len: p.len,
                        piece: p.piece,
                        file: Mutex::new(file),
                    })
                }
            });
        }
        Ok(Box::new(Self {
            session: self.session.clone(),
            hash: self.hash.clone(),
            files,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_keep_the_first_and_last_piece_of_a_long_file() {
        // 10 pieces of 4: the first piece at 0, the last at 4.
        assert_eq!(map_offset(40, 4, 0, 4), Some(0));
        assert_eq!(map_offset(40, 4, 1, 2), Some(1));
        assert_eq!(map_offset(40, 4, 36, 4), Some(4));
        assert_eq!(map_offset(40, 4, 38, 2), Some(6));
        // The middle is never shared with another file: never kept.
        assert_eq!(map_offset(40, 4, 4, 4), None);
        assert_eq!(map_offset(40, 4, 3, 2), None);
        assert_eq!(map_offset(40, 4, 39, 2), None);
        // Two pieces or less: all of it, where it is.
        assert_eq!(map_offset(8, 4, 5, 3), Some(5));
        assert_eq!(map_offset(3, 4, 0, 3), Some(0));
    }

    #[test]
    fn what_parts_dont_hold_reads_as_zeros() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p");
        // A 40-byte file of 4-byte pieces, nothing written yet: no parts file at all.
        let p = Parts::new(path.clone(), 40, 4);
        let mut buf = [9u8; 4];
        p.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0; 4]);
        // Its last piece written: kept at 4.
        std::fs::write(&path, b"\0\0\0\0TAIL").unwrap();
        let p = Parts::new(path, 40, 4);
        p.read(36, &mut buf).unwrap();
        assert_eq!(&buf, b"TAIL");
        // The middle is never kept, and isn't an error: librqbit's check goes on to the end.
        let mut buf = [9u8; 4];
        p.read(20, &mut buf).unwrap();
        assert_eq!(buf, [0; 4]);
    }

    #[test]
    fn ticked_files_get_what_their_parts_file_held() {
        let tmp = tempfile::tempdir().unwrap();
        let parts = tmp.path().join("parts");
        // A 40-byte file of 4-byte pieces: its first piece and its last were written.
        std::fs::write(&parts, b"HEADTAIL").unwrap();
        let path = tmp.path().join("file");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        move_parts_into(&parts, &file, 40, 4).unwrap();
        file.set_len(40).unwrap();
        let got = std::fs::read(&path).unwrap();
        assert_eq!(&got[..4], b"HEAD");
        assert_eq!(&got[36..], b"TAIL");
        assert!(got[4..36].iter().all(|b| *b == 0));
        assert!(!parts.exists());
    }

    #[test]
    fn a_file_with_bytes_is_not_overwritten_from_parts() {
        let tmp = tempfile::tempdir().unwrap();
        let parts = tmp.path().join("parts");
        std::fs::write(&parts, b"zz").unwrap();
        let path = tmp.path().join("file");
        std::fs::write(&path, b"mine").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        move_parts_into(&parts, &file, 4, 4).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
        assert!(!parts.exists());
    }

    #[test]
    fn parts_folders_are_only_for_info_hashes() {
        let s = Path::new("s");
        assert!(parts_dir(s, "../../x").is_none());
        assert_eq!(
            parts_dir(s, &"AB".repeat(20)).unwrap(),
            s.join(format!("{}.parts", "ab".repeat(20)))
        );
    }
}
