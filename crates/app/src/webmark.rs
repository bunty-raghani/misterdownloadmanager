//! The mark a downloaded file carries to say it came from the internet (#813): Windows' Mark of
//! the Web (the `Zone.Identifier` stream SmartScreen and Office's Protected View read), macOS'
//! quarantine attribute (Gatekeeper's), Linux's origin attributes (shown by file managers,
//! enforced by nothing). Every download MDM makes is marked here (the engine's files as they are
//! created, extracted files, videos, galleries, torrents), as Options → Downloads → "Mark
//! downloaded files as coming from the internet" says and, on Windows, as Windows' own policy
//! for attachments says: browsers save their downloads through Windows' attachment services,
//! which leave the mark out under "Do not preserve zone information in file attachments".

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(true);

/// Options → Downloads → "Mark downloaded files as coming from the internet", as it is now
/// (MDM's start, Options → OK).
pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// Whether the system keeps the mark on downloads, from Windows' Group Policy "Do not preserve
/// zone information in file attachments" (`SaveZoneInformation`, `win32::zone_policy`): the
/// computer's value, where it is set, wins over the user's; 1 keeps none, 2 or none set keeps
/// it.
// Asked on Windows only (its tests run everywhere).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn keeps_zones(machine: Option<u32>, user: Option<u32>) -> bool {
    machine.or(user) != Some(1)
}

/// Whether a downloaded file is marked: MDM's option, and the system's policy.
pub fn wanted(option: bool, system_keeps: bool) -> bool {
    option && system_keeps
}

/// Whether the system keeps the mark on downloads, asked now (a registry read on Windows: for a
/// thread other than the window's).
fn system_keeps() -> bool {
    #[cfg(windows)]
    {
        let (machine, user) = crate::win32::zone_policy();
        keeps_zones(machine, user)
    }
    // macOS has no such policy for its quarantine attribute (Gatekeeper's, which only
    // MDM's option leaves out), nor Linux for its origin attributes, which nothing enforces.
    #[cfg(not(windows))]
    {
        true
    }
}

/// Mark `path`, a file being downloaded (or just downloaded) from `url` (`referrer`'s page), as
/// coming from the internet, unless MDM's option or the system's policy says not to.
pub fn mark(path: &Path, url: &str, referrer: Option<&str>) {
    if !wanted(ON.load(Ordering::Relaxed), system_keeps()) {
        return;
    }
    #[cfg(windows)]
    crate::win32::mark_downloaded(path, url, referrer);
    #[cfg(unix)]
    crate::unix::mark_downloaded(path, url, referrer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computers_policy_wins_and_only_1_drops_the_mark() {
        // Not set anywhere: kept, as Windows keeps it.
        assert!(keeps_zones(None, None));
        // The user's own policy.
        assert!(!keeps_zones(None, Some(1)));
        assert!(keeps_zones(None, Some(2)));
        // The computer's, over the user's either way.
        assert!(!keeps_zones(Some(1), None));
        assert!(!keeps_zones(Some(1), Some(2)));
        assert!(keeps_zones(Some(2), Some(1)));
        // A value Windows doesn't define keeps it.
        assert!(keeps_zones(Some(0), None));
    }

    #[test]
    fn a_file_is_marked_only_with_the_option_on_and_the_policy_keeping_it() {
        for (option, policy, marked) in [
            (true, None, true),
            (true, Some(2), true),
            (true, Some(1), false),
            (false, None, false),
            (false, Some(2), false),
            (false, Some(1), false),
        ] {
            assert_eq!(
                wanted(option, keeps_zones(None, policy)),
                marked,
                "option {option}, policy {policy:?}"
            );
        }
    }
}
