//! A downloaded file handed to other programs, from the browser popup's list (#1496): put on
//! the clipboard as a file, to paste into a folder, a chat, an e-mail or a web page's upload
//! box (browsers read pasted files), or offered through the system's share sheet. A popup can't
//! drag a real file out of the browser (docs/browser-integration.md says why), so these do that
//! job, with Show in folder.
//!
//! - Windows: the clipboard gets the file as Explorer's Copy puts it (`CF_HDROP`, and
//!   `Preferred DropEffect` = copy); the share sheet is Windows' own (DataTransferManager),
//!   for a window of MDM's at the pointer (win32.rs).
//! - macOS: the pasteboard gets the file's URL, as Finder's Copy does; the share sheet is
//!   AppKit's picker (unix.rs).
//! - Linux: the clipboard gets `text/uri-list` and `x-special/gnome-copied-files` (which GNOME's
//!   and most other file managers paste), served by MDM over X11 (XWayland in a Wayland session)
//!   while it owns the clipboard (unix.rs). There is no share sheet: no standard one exists
//!   (xdg-desktop-portal has none; KDE's Purpose is KDE's alone), so Share isn't offered.
//!
//! An off-screen run (MDM_OFFSCREEN, the tests) touches neither: it logs what it would do.

use std::path::Path;

/// Why a file wasn't handed over.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// This system can't (no share sheet on Linux; no X11 display to own the clipboard on).
    Unsupported,
    /// The system said no (another program holds the clipboard; the share sheet failed).
    Failed(String),
}

/// MDM can open this system's share sheet ([`share`]).
pub const CAN_SHARE: bool = cfg!(any(windows, target_os = "macos"));

/// Put the file at `path` on the clipboard, as the file manager's Copy does.
pub fn copy_file(path: &Path) -> Result<(), Refused> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    if crate::offscreen() {
        tracing::info!(
            "clipboard (off-screen, not copied): the file {}",
            path.display()
        );
        return Ok(());
    }
    let Some(_lock) = crate::clipboard_lock() else {
        return Err(Refused::Failed("the clipboard is in use in MDM".into()));
    };
    #[cfg(windows)]
    let done = crate::win32::copy_files(&[&path]).map_err(Refused::Failed);
    #[cfg(target_os = "macos")]
    let done = arboard::Clipboard::new()
        .and_then(|mut c| c.set().file_list(&[&path]))
        .map_err(|e| Refused::Failed(e.to_string()));
    #[cfg(target_os = "linux")]
    let done = crate::unix::copy_files(&[&path]);
    if let Err(e) = &done {
        tracing::warn!("clipboard: {} not copied ({e:?})", path.display());
    } else {
        tracing::info!("clipboard: the file {} copied", path.display());
    }
    done
}

/// Offer the file at `path` through the system's share sheet (Windows, macOS), near the
/// pointer. Returns once the sheet is asked for: what the user does there is theirs.
pub fn share(path: &Path) -> Result<(), Refused> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    if !CAN_SHARE {
        return Err(Refused::Unsupported);
    }
    if crate::offscreen() {
        tracing::info!("share (off-screen, not shown): {}", path.display());
        return Ok(());
    }
    #[cfg(windows)]
    let done = crate::win32::share_file(&path).map_err(Refused::Failed);
    #[cfg(target_os = "macos")]
    let done = crate::unix::share_file(&path).map_err(Refused::Failed);
    // Linux: no share sheet (CAN_SHARE is false, see above).
    #[cfg(target_os = "linux")]
    let done: Result<(), Refused> = Err(Refused::Unsupported);
    match &done {
        Ok(()) => tracing::info!("share: the sheet for {} asked for", path.display()),
        Err(e) => tracing::warn!("share: {} not shared ({e:?})", path.display()),
    }
    done
}
