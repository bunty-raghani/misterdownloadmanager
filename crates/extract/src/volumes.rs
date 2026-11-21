//! Archives in several files (#37): which file is which volume of what, and reading a byte-split
//! archive (`x.7z.001`, `.002`…) as one file.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// `name` without the ASCII suffix that starts at `at` in `lower` (`name`'s lowercase): the
/// suffix ends `name` the same number of bytes from its end. Indexing `name` with an offset
/// measured in `lower` is wrong wherever lowercasing changes a character's byte length (`İ` is
/// 2 bytes, its lowercase `i̇` is 3): it slices into the wrong place or panics.
fn without_lower_suffix<'a>(name: &'a str, lower: &str, at: usize) -> &'a str {
    let suffix = lower.len() - at;
    name.get(..name.len() - suffix).unwrap_or(name)
}

/// How an archive is split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    /// `x.part1.rar`, `x.part2.rar`… (RAR 3 and later).
    RarPart,
    /// `x.rar`, `x.r00`, `x.r01`… (older RAR): `x.rar` is volume 1.
    RarOld,
    /// `x.7z.001`, `x.zip.001`…: the archive cut in pieces, byte for byte.
    Split,
}

/// A file's place in a multi-volume archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The name without its volume suffix and archive extension (`movie` for
    /// `movie.part2.rar`): volumes of one archive share it.
    pub base: String,
    pub kind: VolumeKind,
    /// 1-based.
    pub index: u32,
}

impl Volume {
    /// The volume extraction starts from.
    pub fn is_first(&self) -> bool {
        self.index == 1
    }
}

/// The volume `name` is, if it's one (case doesn't matter). A lone `x.rar` is volume 1 of an
/// old-style set, which may have no other volume.
pub fn volume_key(name: &str) -> Option<Volume> {
    let lower = name.to_lowercase();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    // x.partN.rar
    if let Some(stem) = lower.strip_suffix(".rar")
        && let Some(i) = stem.rfind(".part")
        && digits(&stem[i + 5..])
    {
        return Some(Volume {
            base: without_lower_suffix(name, &lower, i).to_owned(),
            kind: VolumeKind::RarPart,
            index: stem[i + 5..].parse().ok()?,
        });
    }
    // x.rar, x.r00 → 1, 2
    if lower.ends_with(".rar") {
        return Some(Volume {
            base: without_lower_suffix(name, &lower, lower.len() - 4).to_owned(),
            kind: VolumeKind::RarOld,
            index: 1,
        });
    }
    if let Some(i) = lower.rfind(".r")
        && lower.len() - i == 4
        && digits(&lower[i + 2..])
    {
        return Some(Volume {
            base: without_lower_suffix(name, &lower, i).to_owned(),
            kind: VolumeKind::RarOld,
            index: lower[i + 2..].parse::<u32>().ok()? + 2,
        });
    }
    // x.7z.001, x.zip.001
    for ext in [".7z.", ".zip."] {
        if let Some(i) = lower.rfind(ext) {
            let num = &lower[i + ext.len()..];
            if num.len() == 3 && digits(num) {
                return Some(Volume {
                    base: without_lower_suffix(name, &lower, i).to_owned(),
                    kind: VolumeKind::Split,
                    index: num.parse().ok()?,
                });
            }
        }
    }
    None
}

/// A byte-split archive's parts, read as one file: `first` is `x.7z.001`; `.002`, `.003`… follow
/// as long as they exist.
pub struct MultiFile {
    parts: Vec<(File, u64)>,
    total: u64,
    pos: u64,
}

impl MultiFile {
    pub fn open(first: &Path) -> io::Result<Result<MultiFile, u32>> {
        let parts_paths = split_parts(first);
        let mut parts = Vec::new();
        let mut total = 0;
        for (n, p) in parts_paths.iter().enumerate() {
            match File::open(p) {
                Ok(f) => {
                    let len = f.metadata()?.len();
                    total += len;
                    parts.push((f, len));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Ok(Err(n as u32 + 1));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Ok(MultiFile {
            parts,
            total,
            pos: 0,
        }))
    }
}

/// `x.7z.001`'s parts on disk: `.001`, `.002`… while they exist, and any later one found after a
/// gap (so a missing middle volume is reported, not taken for the end).
pub fn split_parts(first: &Path) -> Vec<PathBuf> {
    let name = first
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(stem) = name.get(..name.len().saturating_sub(3)) else {
        return vec![first.to_path_buf()];
    };
    let dir = first.parent().unwrap_or(Path::new(""));
    let path = |n: u32| dir.join(format!("{stem}{n:03}"));
    let mut last = 1;
    // The highest part there (up to 999), a gap or not.
    for n in 2..=999 {
        if path(n).exists() {
            last = n;
        } else if n > last + 10 {
            break;
        }
    }
    (1..=last).map(path).collect()
}

impl Read for MultiFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut start = 0;
        for (f, len) in &mut self.parts {
            if self.pos < start + *len {
                f.seek(SeekFrom::Start(self.pos - start))?;
                let room = (*len - (self.pos - start)) as usize;
                let take = buf.len().min(room);
                let n = f.read(&mut buf[..take])?;
                self.pos += n as u64;
                return Ok(n);
            }
            start += *len;
        }
        Ok(0)
    }
}

impl Seek for MultiFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => self.total as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}
