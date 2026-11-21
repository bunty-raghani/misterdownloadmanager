//! App-authorized file handles. Storage never reopens a torrent path for writing.
use librqbit::storage::TorrentStorage;
use librqbit::{ManagedTorrentShared, TorrentMetadata};
use std::{
    collections::HashMap,
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

type Reopen = Arc<dyn Fn() -> io::Result<Vec<PreparedFile>> + Send + Sync>;
struct Grant {
    folder: PathBuf,
    first: Mutex<Option<Vec<PreparedFile>>>,
    reopen: Reopen,
}
type Key = (PathBuf, String);
fn grants() -> &'static Mutex<HashMap<Key, Arc<Grant>>> {
    static GRANTS: OnceLock<Mutex<HashMap<Key, Arc<Grant>>>> = OnceLock::new();
    GRANTS.get_or_init(Default::default)
}

/// The first storage initialization consumes exactly the handles checked by the app.
/// A subsequent initialization (file selection or session restart) must authorize again.
pub fn authorize(
    session: &Path,
    hash: &str,
    folder: &Path,
    files: Vec<PreparedFile>,
    reopen: Reopen,
) {
    grants().lock().unwrap().insert(
        (session.to_owned(), hash.to_owned()),
        Arc::new(Grant {
            folder: folder.to_owned(),
            first: Mutex::new(Some(files)),
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

pub(crate) struct Storage {
    session: PathBuf,
    hash: String,
    // One lock per file: positioned writes to different pieces remain concurrent.
    files: Vec<Option<RwLock<Option<File>>>>,
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
        let Some(slot) = slot else {
            return Ok(padding());
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
        let grant = grants()
            .lock()
            .unwrap()
            .get(&(self.session.clone(), self.hash.clone()))
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
        let first = grant.first.lock().unwrap().take();
        let prepared = match first {
            Some(files) => files,
            None => (grant.reopen)()?,
        };
        let mut prepared: HashMap<_, _> = prepared.into_iter().map(|f| (f.index, f)).collect();
        let mut opened = Vec::with_capacity(metadata.file_infos.len());
        for (index, info) in metadata.file_infos.iter().enumerate() {
            if info.attrs.padding {
                opened.push(None);
                continue;
            }
            let file = prepared
                .remove(&index)
                .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
            if file.path != grant.folder.join(&info.relative_filename) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
            }
            opened.push(Some(RwLock::new(Some(file.file))));
        }
        if !prepared.is_empty() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
        }
        self.files = opened;
        Ok(())
    }

    fn pread_exact(&self, file_id: usize, mut offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        // Handle padding separately because both closures cannot borrow the output mutably.
        if self.files.get(file_id).is_some_and(Option::is_none) {
            buf.fill(0);
            return Ok(());
        }
        self.with_file(
            file_id,
            || (),
            |file| {
                let mut rest = &mut *buf;
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
            },
        )
    }

    fn pwrite_all(&self, file_id: usize, mut offset: u64, mut buf: &[u8]) -> anyhow::Result<()> {
        self.with_file(
            file_id,
            || (),
            |file| {
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
            },
        )
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
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
                None => None,
                Some(slot) => Some(RwLock::new(
                    slot.write()
                        .map_err(|_| io::Error::from(io::ErrorKind::Other))?
                        .take(),
                )),
            });
        }
        Ok(Box::new(Self {
            session: self.session.clone(),
            hash: self.hash.clone(),
            files,
        }))
    }
}
