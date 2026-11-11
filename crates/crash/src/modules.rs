//! A Linux dump's module list mended before it is read (#661).
//!
//! The Linux dump writer (minidump-writer) makes one module of a program's mappings only when
//! they follow each other in memory. A program linked by lld can leave an unmapped page between
//! its first, read-only mapping (the ELF header, at file offset 0) and its code, whenever the
//! code segment happens to start on a page of its own (MDM 0.5.0's did). The writer then lists
//! the code as a module of its own, based where the code starts instead of where the program
//! starts. Every address in the code was then read 0xd96000 too low: the release's symbols named
//! the wrong functions, unwinding found no unwind tables, and the report's own offsets
//! (`misterdownloadmanager + 0x…`) couldn't be looked up either.
//!
//! The dump also holds the process's `/proc/self/maps`, which says which part of the file each
//! mapping holds. A module whose mapping doesn't start the file is joined here to the mapping of
//! the same file that does (offset 0, just below it): one module, based where the program
//! starts, as the symbols (and every other reader) expect.

/// The module list stream (MINIDUMP_STREAM_TYPE ModuleListStream).
const MODULE_LIST: u32 = 4;
/// Breakpad's `/proc/self/maps` stream (MD_LINUX_MAPS).
const LINUX_MAPS: u32 = 0x4767_0009;
/// MINIDUMP_MODULE's size: base u64, size u32, checksum u32, time stamp u32, name RVA u32,
/// VS_FIXEDFILEINFO (52), CodeView and misc locations (8 each), two reserved u64.
const MODULE_SIZE: usize = 108;

/// One line of `/proc/self/maps`: where, and which part of which file.
struct Mapping<'a> {
    start: u64,
    offset: u64,
    path: &'a str,
}

/// `dump` with every module split from its program joined back to it. Unchanged when there is
/// nothing to mend, or the dump isn't one this understands (a Windows or macOS dump has no
/// maps stream).
pub fn mend(mut dump: Vec<u8>) -> Vec<u8> {
    if let Some(fixed) = mended(&dump) {
        for (at, bytes) in fixed {
            dump[at..at + bytes.len()].copy_from_slice(&bytes);
        }
    }
    dump
}

/// The bytes to write over (where, what), or None.
fn mended(dump: &[u8]) -> Option<Vec<(usize, Vec<u8>)>> {
    if dump.get(..4)? != b"MDMP" {
        return None;
    }
    let streams = u32_at(dump, 8)? as usize;
    let dir = u32_at(dump, 12)? as usize;
    let (mut list, mut maps) = (None, None);
    for i in 0..streams {
        let e = dir + i * 12;
        let (kind, size, rva) = (u32_at(dump, e)?, u32_at(dump, e + 4)?, u32_at(dump, e + 8)?);
        match kind {
            MODULE_LIST => list = Some((e, rva as usize)),
            LINUX_MAPS => {
                maps = Some(dump.get(rva as usize..(rva as usize).checked_add(size as usize)?)?)
            }
            _ => {}
        }
    }
    let ((entry, list), maps) = (list?, std::str::from_utf8(maps?).ok()?);
    let maps: Vec<Mapping> = maps.lines().filter_map(mapping).collect();
    let count = u32_at(dump, list)? as usize;
    let mut modules: Vec<(u64, u64)> = (0..count)
        .map(|i| {
            let at = list + 4 + i * MODULE_SIZE;
            Some((u64_at(dump, at)?, u64::from(u32_at(dump, at + 8)?)))
        })
        .collect::<Option<_>>()?;
    let mut keep = vec![true; count];
    let mut changed = false;
    for i in 0..count {
        let (base, size) = modules[i];
        let Some(m) = maps.iter().find(|m| m.start == base) else {
            continue;
        };
        if m.offset == 0 || !m.path.starts_with('/') {
            continue;
        }
        // The same file's mapping from its start, the closest below.
        let Some(start) = maps
            .iter()
            .filter(|s| s.path == m.path && s.offset == 0 && s.start < base)
            .map(|s| s.start)
            .max()
        else {
            continue;
        };
        let end = base + size;
        match (0..count).find(|&j| keep[j] && j != i && modules[j].0 == start) {
            // Listed already (the writer lists the read-only start too): it grows to cover the
            // code, and the code's own entry goes.
            Some(j) => {
                let (b, s) = modules[j];
                modules[j].1 = end.max(b + s) - b;
                keep[i] = false;
            }
            // Not listed: the code's entry starts where the program does.
            None => modules[i] = (start, end - start),
        }
        changed = true;
    }
    if !changed {
        return None;
    }
    // The list rewritten: the entries kept, in their order, each based and sized anew; the
    // stream's size follows its new count (what is left after it is no longer read).
    let mut out = Vec::new();
    let mut bytes = Vec::with_capacity(4 + count * MODULE_SIZE);
    let kept: Vec<usize> = (0..count).filter(|&i| keep[i]).collect();
    bytes.extend_from_slice(&(kept.len() as u32).to_le_bytes());
    for &i in &kept {
        let at = list + 4 + i * MODULE_SIZE;
        let mut raw = dump.get(at..at + MODULE_SIZE)?.to_vec();
        let (base, size) = modules[i];
        raw[..8].copy_from_slice(&base.to_le_bytes());
        raw[8..12].copy_from_slice(&u32::try_from(size).ok()?.to_le_bytes());
        bytes.extend_from_slice(&raw);
    }
    let new_size = bytes.len() as u32;
    out.push((list, bytes));
    out.push((entry + 4, new_size.to_le_bytes().to_vec()));
    Some(out)
}

/// `start-end perms offset dev inode [path]`.
fn mapping(line: &str) -> Option<Mapping<'_>> {
    let mut parts = line.splitn(6, ' ');
    let (start, _) = parts.next()?.split_once('-')?;
    let _perms = parts.next()?;
    let offset = parts.next()?;
    let (_dev, _inode) = (parts.next()?, parts.next()?);
    Some(Mapping {
        start: u64::from_str_radix(start, 16).ok()?,
        offset: u64::from_str_radix(offset, 16).ok()?,
        path: parts.next().unwrap_or("").trim(),
    })
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minidump::Module;

    /// A dump with a module list and a maps stream, as minidump-writer writes them.
    fn dump(modules: &[(u64, u32, &str)], maps: &str) -> Vec<u8> {
        let mut d = vec![0u8; 32];
        d[..4].copy_from_slice(b"MDMP");
        d[4..8].copy_from_slice(&0xa793u32.to_le_bytes());
        d[8..12].copy_from_slice(&2u32.to_le_bytes());
        d[12..16].copy_from_slice(&32u32.to_le_bytes());
        // The directory (2 entries), filled in below.
        d.extend_from_slice(&[0; 24]);
        // The modules' names (MINIDUMP_STRING: byte length, UTF-16).
        let mut names = Vec::new();
        for (_, _, name) in modules {
            names.push(d.len() as u32);
            let utf16: Vec<u16> = name.encode_utf16().collect();
            d.extend_from_slice(&((utf16.len() * 2) as u32).to_le_bytes());
            for c in utf16 {
                d.extend_from_slice(&c.to_le_bytes());
            }
            d.extend_from_slice(&[0, 0]);
        }
        while !d.len().is_multiple_of(4) {
            d.push(0);
        }
        let list = d.len() as u32;
        d.extend_from_slice(&(modules.len() as u32).to_le_bytes());
        for (i, (base, size, _)) in modules.iter().enumerate() {
            let mut raw = [0u8; MODULE_SIZE];
            raw[..8].copy_from_slice(&base.to_le_bytes());
            raw[8..12].copy_from_slice(&size.to_le_bytes());
            raw[20..24].copy_from_slice(&names[i].to_le_bytes());
            d.extend_from_slice(&raw);
        }
        let list_size = d.len() as u32 - list;
        let maps_at = d.len() as u32;
        d.extend_from_slice(maps.as_bytes());
        let dir = [
            (MODULE_LIST, list_size, list),
            (LINUX_MAPS, maps.len() as u32, maps_at),
        ];
        for (i, (kind, size, rva)) in dir.iter().enumerate() {
            let e = 32 + i * 12;
            d[e..e + 4].copy_from_slice(&kind.to_le_bytes());
            d[e + 4..e + 8].copy_from_slice(&size.to_le_bytes());
            d[e + 8..e + 12].copy_from_slice(&rva.to_le_bytes());
        }
        d
    }

    fn modules(d: Vec<u8>) -> Vec<(u64, u64, String)> {
        let dump = minidump::Minidump::read(d).unwrap();
        let list: minidump::MinidumpModuleList = dump.get_stream().unwrap();
        list.iter()
            .map(|m| (m.base_address(), m.size(), m.code_file().into_owned()))
            .collect()
    }

    const EXE: &str = "/usr/bin/misterdownloadmanager";
    const LIBC: &str = "/usr/lib64/libc.so.6";

    /// MDM 0.5.0's layout (#661): the header's mapping, a page unmapped, then the code with the
    /// rest of the program after it.
    fn split_maps() -> String {
        format!(
            "5c0000000000-5c0000d95000 r--p 00000000 08:02 7 {EXE}\n\
             5c0000d96000-5c0002670000 r-xp 00d95000 08:02 7 {EXE}\n\
             7f0000000000-7f0000028000 r--p 00000000 08:02 9 {LIBC}\n\
             7f0000028000-7f00001b0000 r-xp 00028000 08:02 9 {LIBC}\n"
        )
    }

    #[test]
    fn a_program_split_by_a_gap_is_one_module_again() {
        let d = dump(
            &[
                (0x5c00_0000_0000, 0xd95000, EXE),
                (0x5c00_00d9_6000, 0x18da000, EXE),
                (0x7f00_0000_0000, 0x1b0000, LIBC),
            ],
            &split_maps(),
        );
        assert_eq!(
            modules(mend(d)),
            vec![
                (0x5c00_0000_0000, 0x2670000, EXE.to_owned()),
                (0x7f00_0000_0000, 0x1b0000, LIBC.to_owned()),
            ]
        );
    }

    #[test]
    fn code_listed_without_its_start_is_based_at_the_start() {
        let d = dump(&[(0x5c00_00d9_6000, 0x18da000, EXE)], &split_maps());
        assert_eq!(
            modules(mend(d)),
            vec![(0x5c00_0000_0000, 0x2670000, EXE.to_owned())]
        );
    }

    #[test]
    fn whole_modules_and_other_dumps_are_left_alone() {
        // Merged by the writer already (no gap): listed from the file's start.
        let d = dump(&[(0x7f00_0000_0000, 0x1b0000, LIBC)], &split_maps());
        assert_eq!(mend(d.clone()), d);
        // No maps stream (Windows, macOS), or not a dump at all.
        let mut no_maps = d.clone();
        no_maps[32 + 12..32 + 16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(mend(no_maps.clone()), no_maps);
        assert_eq!(mend(b"not a dump".to_vec()), b"not a dump".to_vec());
    }
}
