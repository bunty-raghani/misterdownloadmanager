//! Disk preflight: will a download fit where it is going? (notes/PLAN.md "Disk preflight";
//! lessons from AB Download Manager, Motrix, JDownloader, aria2.)

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Room kept free beyond the file itself (the OS, our list file, other programs).
pub const MARGIN: u64 = 128 << 20;
/// The longest the volume probe may run before the check is skipped (a stale network drive
/// can hang it), counted from when it starts running: see [`probe`].
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// FAT32's largest file: 4 GiB − 1.
pub const FAT32_MAX: u64 = (4 << 30) - 1;

/// Free space and file system of the volume holding `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The bytes a new file may take, as each system counts them for the user: on Windows the
    /// space this user may use, disk quotas included (GetDiskFreeSpaceExW); on Linux and other
    /// Unix systems the blocks left to programs that aren't root (statvfs's `f_bavail`); on
    /// macOS a local volume's capacity "for important usage", which includes the purgeable space
    /// (caches, local snapshots) the system frees as soon as a program writes. Finder shows the
    /// same number; statvfs leaves the purgeable space out, and would refuse a download that
    /// fits. A network volume on macOS has no purgeable space and answers that capacity with 0:
    /// its statvfs count is used (#623, `free_on_apple`).
    pub free: u64,
    /// As the system names it (`NTFS`, `FAT32`, `exFAT`; `ext4`, `vfat`; `apfs`, `msdos`), or
    /// empty when it can't tell: the space is still checked.
    pub file_system: String,
}

/// The volume `path` is on, or will be once created: a missing folder is looked up through the
/// nearest folder above it that exists. Only that volume is asked (a network drive too). None
/// when no folder of the path exists (a removed drive) or the system can't tell: the check is
/// then skipped.
pub fn volume_of(path: &Path) -> Option<Volume> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let path = std::path::absolute(path).ok()?;
    let dir = path.ancestors().find(|p| p.is_dir())?;
    imp::volume_of(dir)
}

/// [`volume_of`] off the async threads, skipped (None) if it runs for [`PROBE_TIMEOUT`].
///
/// The time counts from when the probe starts, not from when it is asked for: on a starved CPU
/// the blocking thread itself can take seconds to start (#224: 1 in 90 starved runs refused
/// nothing, the check skipped though the probe takes under a millisecond). Waiting for a
/// thread is not a hung drive.
pub async fn probe(path: PathBuf) -> Option<Volume> {
    bounded(move || volume_of(&path), PROBE_TIMEOUT)
        .await
        .flatten()
}

/// `f` on a blocking thread, given up (None) once it has run for `limit`.
async fn bounded<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    limit: Duration,
) -> Option<T> {
    let started = Arc::new(OnceLock::new());
    let mut task = tokio::task::spawn_blocking({
        let started = started.clone();
        move || {
            let _ = started.set(Instant::now());
            f()
        }
    });
    loop {
        let left = started
            .get()
            .map_or(limit, |t: &Instant| limit.saturating_sub(t.elapsed()));
        match tokio::time::timeout(left, &mut task).await {
            Ok(done) => return done.ok(),
            Err(_) if started.get().is_some_and(|t| t.elapsed() >= limit) => return None,
            // Not started yet, or only just: wait on.
            Err(_) => {}
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // Two calls; the engine's other unsafe code is in `crate::sparse`.
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, GetDiskFreeSpaceExW, GetVolumeInformationByHandleW,
    };

    pub fn volume_of(dir: &Path) -> Option<super::Volume> {
        // A folder of the volume; a share's root (`\\server\share\`) needs its backslash.
        let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
        if !matches!(wide.last(), Some(&c) if c == u16::from(b'\\') || c == u16::from(b'/')) {
            wide.push(u16::from(b'\\'));
        }
        wide.push(0);
        let mut free = 0u64;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call; `free` is a
        // valid u64 (the space this user may use, quotas included); the two other outputs are
        // optional and left null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return None;
        }
        Some(super::Volume {
            free,
            file_system: file_system(dir).unwrap_or_default(),
        })
    }

    /// The file system of the volume the folder really is on (through junctions and mounted
    /// folders), from a handle to the folder that asks for no access.
    fn file_system(dir: &Path) -> Option<String> {
        let dir = std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
            .ok()?;
        // MAX_PATH + 1, the size the documentation asks for.
        let mut name = [0u16; 261];
        // SAFETY: `dir` owns the handle and outlives the call; `name` is a writable buffer of
        // the length passed; every other output is optional and left null (with size 0).
        let ok = unsafe {
            GetVolumeInformationByHandleW(
                dir.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..len]))
    }
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub fn volume_of(dir: &Path) -> Option<super::Volume> {
        let st = rustix::fs::statvfs(dir).ok()?;
        let block = if st.f_frsize > 0 {
            st.f_frsize
        } else {
            st.f_bsize
        };
        // The blocks a program may use (without the reserve kept for root).
        let free = st.f_bavail.saturating_mul(block);
        #[cfg(target_vendor = "apple")]
        let free = super::free_on_apple(free, apple::important_capacity(dir), apple::is_local(dir));
        Some(super::Volume {
            free,
            file_system: file_system(dir),
        })
    }

    /// The capacity Apple's systems give "for important usage" (a download the user asked for
    /// is one): the free space plus the purgeable space the system frees on demand, as Finder
    /// shows it. Asked of the folder's volume only. `target_vendor` rather than `target_os =
    /// "macos"`, as for the file system's name below: the key is on iOS (11+) too. It needs macOS
    /// 10.13; the macOS build asks for 11.0 (build-macos.sh).
    #[cfg(target_vendor = "apple")]
    #[allow(unsafe_code)] // One call and its key; the engine's other unsafe code is in `crate::sparse`.
    mod apple {
        use std::path::Path;

        use core_foundation::base::{CFType, TCFType};
        use core_foundation::number::CFNumber;
        use core_foundation::url::CFURL;
        use core_foundation_sys::base::{Boolean, CFTypeRef};
        use core_foundation_sys::error::CFErrorRef;
        use core_foundation_sys::string::CFStringRef;
        use core_foundation_sys::url::CFURLRef;

        // Both are commented out in core-foundation-sys 0.8.
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            static kCFURLVolumeAvailableCapacityForImportantUsageKey: CFStringRef;
            fn CFURLCopyResourcePropertyForKey(
                url: CFURLRef,
                key: CFStringRef,
                value: *mut CFTypeRef,
                error: *mut CFErrorRef,
            ) -> Boolean;
        }

        /// None when the system can't tell (the key unknown to the volume, a failed call): the
        /// statvfs count is used then.
        pub fn important_capacity(dir: &Path) -> Option<u64> {
            let url = CFURL::from_path(dir, true)?;
            let mut value: CFTypeRef = std::ptr::null();
            // SAFETY: `url` is a live CFURL that outlives the call; the key is a constant
            // CFString CoreFoundation exports (macOS 10.13+, iOS 11+); `value` is a writable
            // pointer the call fills with a +1 reference or leaves null; the error output is
            // optional and left null.
            let ok = unsafe {
                CFURLCopyResourcePropertyForKey(
                    url.as_concrete_TypeRef(),
                    kCFURLVolumeAvailableCapacityForImportantUsageKey,
                    &mut value,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || value.is_null() {
                return None;
            }
            // SAFETY: a non-null `value` from a Copy function is ours to release: the CFType
            // takes that reference and releases it when dropped.
            let value = unsafe { CFType::wrap_under_create_rule(value) };
            // A CFNumber (a 64-bit integer); anything else, or a negative count, is no answer.
            let bytes = value.downcast::<CFNumber>()?.to_i64()?;
            u64::try_from(bytes).ok()
        }

        /// Whether the folder's volume is on this computer (`MNT_LOCAL` in statfs's flags), not a
        /// network share (SMB, AFP, NFS, WebDAV). False when the system can't tell.
        pub fn is_local(dir: &Path) -> bool {
            /// `<sys/mount.h>`: "filesystem is stored locally".
            const MNT_LOCAL: u32 = 0x0000_1000;
            rustix::fs::statfs(dir).is_ok_and(|st| st.f_flags & MNT_LOCAL != 0)
        }
    }

    /// Linux tells a file system by a number; the FAT check needs `vfat`'s, the others are
    /// named for messages.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn file_system(dir: &Path) -> String {
        let Ok(st) = rustix::fs::statfs(dir) else {
            return String::new();
        };
        let name = match i128::from(st.f_type) {
            0x4d44 => "vfat",
            0x2011_bab0 => "exfat",
            0xef53 => "ext4",
            0x9123_683e => "btrfs",
            0x5846_5342 => "xfs",
            0x2fc1_2fc1 => "zfs",
            0xf2f5_2010 => "f2fs",
            0x0102_1994 => "tmpfs",
            0x794c_7630 => "overlay",
            0x6969 => "nfs",
            0xff53_4d42 => "cifs",
            0xfe53_4d42 => "smb2",
            0x5346_544e => "ntfs",
            0x7366_746e => "ntfs3",
            0x6573_5546 => "fuse",
            _ => "",
        };
        name.to_owned()
    }

    /// macOS names it (`apfs`, `hfs`, `msdos`, `exfat`, `smbfs`).
    #[cfg(target_vendor = "apple")]
    fn file_system(dir: &Path) -> String {
        let Ok(st) = rustix::fs::statfs(dir) else {
            return String::new();
        };
        let bytes: Vec<u8> = st
            .f_fstypename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Other Unix systems: the space only.
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    fn file_system(_: &Path) -> String {
        String::new()
    }
}

/// The free space a new file may take on Apple's systems, from statvfs's count (`statvfs`), the
/// volume's capacity "for important usage" (`important`, None when it can't tell) and whether the
/// volume is on this computer (`local`).
///
/// The important-usage capacity adds the purgeable space the system frees on demand, a local
/// volume's notion (APFS caches and snapshots). A network volume (SMB, AFP, NFS) answers it with
/// 0, and taking that 0 refused every download to it (#623): there, and whenever it answers 0,
/// statvfs's count is the free space. Portable, so its tests run on every system.
#[cfg(any(target_vendor = "apple", test))]
fn free_on_apple(statvfs: u64, important: Option<u64>, local: bool) -> u64 {
    match important {
        Some(bytes) if local && bytes > 0 => bytes,
        _ => statvfs,
    }
}

/// Neither Windows nor Unix: no probe, the check is skipped.
#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn volume_of(_: &std::path::Path) -> Option<super::Volume> {
        None
    }
}

/// Why a file of `size` bytes can't go on `volume`, if it can't.
pub fn check(volume: &Volume, size: u64) -> Result<(), Refusal> {
    let fs = volume.file_system.to_ascii_uppercase();
    // Windows names it FAT32 (or FAT), Linux vfat, macOS msdos.
    if size > FAT32_MAX && matches!(fs.as_str(), "FAT32" | "FAT" | "VFAT" | "MSDOS") {
        return Err(Refusal::TooBigForFileSystem {
            size,
            file_system: volume.file_system.clone(),
        });
    }
    let needed = size.saturating_add(MARGIN);
    if needed > volume.free {
        return Err(Refusal::NoSpace {
            needed: size,
            free: volume.free.saturating_sub(MARGIN.min(volume.free)),
        });
    }
    Ok(())
}

/// A size for messages: `512 bytes`, `1.5 MB`, `4.00 GB` (binary units, as IDM shows them).
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let decimals = if v < 10.0 {
        2
    } else if v < 100.0 {
        1
    } else {
        0
    };
    format!("{v:.decimals$} {}", UNITS[unit])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoSpace { needed: u64, free: u64 },
    TooBigForFileSystem { size: u64, file_system: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(free: u64, fs: &str) -> Volume {
        Volume {
            free,
            file_system: fs.into(),
        }
    }

    #[test]
    fn needs_the_file_plus_a_margin() {
        assert!(check(&vol(10 << 30, "NTFS"), 1 << 30).is_ok());
        assert!(matches!(
            check(&vol((1 << 30) + MARGIN - 1, "NTFS"), 1 << 30),
            Err(Refusal::NoSpace { .. })
        ));
    }

    #[test]
    fn fat32_refuses_4_gib_files() {
        for fs in ["FAT32", "vfat", "msdos"] {
            assert!(check(&vol(64 << 30, fs), FAT32_MAX).is_ok());
            assert!(
                matches!(
                    check(&vol(64 << 30, fs), 4 << 30),
                    Err(Refusal::TooBigForFileSystem { .. })
                ),
                "{fs}"
            );
        }
        assert!(check(&vol(64 << 30, "exFAT"), 8 << 30).is_ok());
        // Unknown: the space is still checked.
        assert!(check(&vol(64 << 30, ""), 8 << 30).is_ok());
        assert!(check(&vol(1 << 30, ""), 8 << 30).is_err());
    }

    #[test]
    fn a_network_volume_on_apple_counts_statvfs() {
        const TB: u64 = 1 << 40;
        // #623: a share with terabytes free answers 0 for important usage. The probe took that
        // 0, and a 3.28 KB download was refused with "0 bytes free".
        let free = free_on_apple(4 * TB, Some(0), false);
        assert_eq!(free, 4 * TB);
        assert!(check(&vol(free, "smbfs"), 3360).is_ok());
        assert!(matches!(
            check(&vol(0, "smbfs"), 3360),
            Err(Refusal::NoSpace { free: 0, .. })
        ));
        // A share's answer isn't taken even when it isn't 0 (it has no purgeable space).
        for fs_answer in [Some(1), Some(TB), None] {
            assert_eq!(
                free_on_apple(4 * TB, fs_answer, false),
                4 * TB,
                "{fs_answer:?}"
            );
        }
        // A local volume: the important-usage capacity, purgeable space included (#224), unless
        // it says 0 or nothing.
        assert_eq!(free_on_apple(10 << 30, Some(12 << 30), true), 12 << 30);
        assert_eq!(free_on_apple(10 << 30, Some(0), true), 10 << 30);
        assert_eq!(free_on_apple(10 << 30, None, true), 10 << 30);
    }

    #[test]
    fn sizes_read_like_idms() {
        assert_eq!(human(512), "512 bytes");
        assert_eq!(human(1536), "1.50 KB");
        assert_eq!(human(4 << 30), "4.00 GB");
        assert_eq!(human(150 << 20), "150 MB");
    }

    /// The volume as `sysinfo` lists it (the probe before #224, which listed every volume).
    fn listed(path: &Path) -> Option<(u64, String)> {
        let path = std::fs::canonicalize(path).ok()?;
        let p = path.to_string_lossy().to_ascii_lowercase();
        let p = p.strip_prefix(r"\\?\").unwrap_or(&p).to_owned();
        let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
        disks
            .iter()
            .filter(|d| p.starts_with(&d.mount_point().to_string_lossy().to_ascii_lowercase()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|d| {
                (
                    d.available_space(),
                    d.file_system().to_string_lossy().into_owned(),
                )
            })
    }

    #[test]
    fn finds_the_volume_of_a_real_path() {
        let dir = tempfile::tempdir().unwrap();
        let v = volume_of(dir.path()).expect("the temp folder's volume");
        assert!(v.free > 0, "{v:?}");
        // The volume sysinfo lists, give or take what other programs wrote meanwhile. On macOS
        // sysinfo counts the important-usage capacity too (purgeable space included), so the
        // two agree there as well: statvfs alone fell short of it by more than 1 GiB on CI.
        if let Some((free, fs)) = listed(dir.path()) {
            assert!(
                v.file_system.eq_ignore_ascii_case(&fs),
                "{v:?} against {fs}"
            );
            assert!(v.free.abs_diff(free) < 1 << 30, "{v:?} against {free}");
        }
    }

    #[test]
    fn a_missing_folder_is_on_the_volume_of_the_folder_above() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not").join("yet");
        let v = volume_of(&missing).expect("found through its parent");
        let above = volume_of(dir.path()).unwrap();
        assert_eq!(v.file_system, above.file_system);
        assert!(v.free.abs_diff(above.free) < 1 << 30);
        // A file's path is looked up through its folder.
        let file = dir.path().join("f.bin");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(volume_of(&file).unwrap().file_system, above.file_system);
        // A relative path, and the empty one, are in the current folder.
        assert!(volume_of(Path::new("")).is_some());
        assert!(volume_of(Path::new("some/missing/folder")).is_some());
    }

    /// A runtime with one blocking thread, so a test can keep it busy.
    fn one_blocking_thread() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_time()
            .build()
            .unwrap()
    }

    #[test]
    fn a_probe_that_hangs_is_given_up() {
        // Given up before it ends: waited for, it would answer `Some(())`.
        let v = one_blocking_thread().block_on(bounded(
            || std::thread::sleep(Duration::from_secs(1)),
            Duration::from_millis(200),
        ));
        assert_eq!(v, None);
    }

    #[test]
    fn a_probe_that_starts_late_still_answers() {
        // #224: on a starved CPU the probe's thread started after the time the probe is given.
        // Here the only blocking thread is busy for 3 times that first.
        one_blocking_thread().block_on(async {
            let busy =
                tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(600)));
            let t = Instant::now();
            let v = bounded(|| 7, Duration::from_millis(200)).await;
            assert_eq!(v, Some(7));
            assert!(
                t.elapsed() >= Duration::from_millis(500),
                "{:?}",
                t.elapsed()
            );
            busy.await.unwrap();
        });
    }

    #[test]
    fn the_probe_answers_off_the_async_threads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let v = one_blocking_thread().block_on(probe(path.clone()));
        assert_eq!(
            v.map(|v| v.file_system),
            volume_of(&path).map(|v| v.file_system)
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_that_isnt_there_has_no_volume() {
        let free = (b'D'..=b'Z')
            .rev()
            .map(|l| format!("{}:\\", l as char))
            .find(|root| !Path::new(root).exists())
            .expect("a drive letter not in use");
        assert_eq!(volume_of(&Path::new(&free).join("Downloads")), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_share_is_probed_like_a_drive() {
        // `\\?\C:\…`, and C: as the administrative share `\\localhost\C$\…` when it is there.
        let dir = tempfile::tempdir().unwrap();
        let local = volume_of(dir.path()).unwrap();
        let verbatim = volume_of(&std::fs::canonicalize(dir.path()).unwrap()).unwrap();
        assert_eq!(verbatim.file_system, local.file_system);
        let path = dir.path().to_string_lossy().into_owned();
        if let Some(rest) = path.strip_prefix("C:\\") {
            let share = std::path::PathBuf::from(format!(r"\\localhost\C$\{rest}"));
            if share.is_dir() {
                let v = volume_of(&share).expect("the share's volume");
                assert!(
                    v.free.abs_diff(local.free) < 1 << 30,
                    "{v:?} against {local:?}"
                );
                assert!(!v.file_system.is_empty(), "{v:?}");
            }
        }
    }
}
