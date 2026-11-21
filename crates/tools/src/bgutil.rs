//! Detection of the bgutil YouTube Proof-of-Origin Token (POT) provider on this computer
//! ([Brainicism/bgutil-ytdlp-pot-provider] and its Rust fork [jim60105/bgutil-ytdlp-pot-provider-rs]).
//!
//! YouTube uses proof-of-origin tokens to protect videos against automated / bot traffic
//! ("Sign in to confirm you're not a bot"). yt-dlp delegates token generation to a POT provider
//! plugin, which either calls a local HTTP server on `127.0.0.1:4416` or a standalone CLI binary
//! (`bgutil-pot` or `bgutil`).
//!
//! MDM only reports what is already here — a running provider server, a `bgutil-pot` binary on
//! the system, or an installed yt-dlp plugin. It never downloads or starts one itself: unlike
//! yt-dlp (verified against its developers' signing key) and ffmpeg (against its published
//! checksums), a third-party provider has nothing to verify it against.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Installed;

/// The default port for the bgutil POT HTTP provider server.
pub const DEFAULT_PORT: u16 = 4416;

/// Check if a POT HTTP provider server is listening on `127.0.0.1:4416`.
pub fn server_reachable() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// Find a bgutil POT provider executable on the system (`bgutil-pot`, `bgutil-pot-server`, or `bgutil`).
pub fn find_system_executable() -> Option<Installed> {
    for name in ["bgutil-pot", "bgutil-pot-server", "bgutil"] {
        if let Some(inst) = crate::system::installed(name) {
            return Some(inst);
        }
    }
    None
}

/// Check candidate plugin directories for the bgutil plugin.
pub fn find_plugin() -> Option<PathBuf> {
    for dir in plugin_dirs() {
        if let Some(found) = check_dir_for_bgutil(&dir) {
            return Some(found);
        }
    }
    None
}

/// Standard directories where yt-dlp searches for plugins.
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            dirs.push(appdata.join(r"yt-dlp\plugins"));
            dirs.push(appdata.join(r"yt-dlp-plugins"));
        }
        if let Some(user) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            dirs.push(user.join(r"yt-dlp-plugins"));
            dirs.push(user.join(r".yt-dlp\plugins"));
        }
    }

    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".config/yt-dlp/plugins"));
            dirs.push(home.join(".config/yt-dlp-plugins"));
            dirs.push(home.join(".yt-dlp/plugins"));
            dirs.push(home.join("yt-dlp-plugins"));
        }
        dirs.push(PathBuf::from("/etc/yt-dlp/plugins"));
        dirs.push(PathBuf::from("/etc/yt-dlp-plugins"));
    }

    dirs
}

fn check_dir_for_bgutil(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.contains("bgutil") || name.contains("pot-provider") {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_plugin_in_directory() {
        let temp = tempfile::tempdir().unwrap();
        let plugin = temp.path().join("bgutil-ytdlp-pot-provider");
        std::fs::create_dir(&plugin).unwrap();
        assert_eq!(check_dir_for_bgutil(temp.path()), Some(plugin));
    }

    #[test]
    fn detects_plugin_zip_in_directory() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("bgutil-ytdlp-pot-provider-rs.zip");
        std::fs::write(&zip, b"PK").unwrap();
        assert_eq!(check_dir_for_bgutil(temp.path()), Some(zip));
    }

    #[test]
    fn ignores_unrelated_directories() {
        let temp = tempfile::tempdir().unwrap();
        let other = temp.path().join("some-other-plugin");
        std::fs::create_dir(&other).unwrap();
        assert_eq!(check_dir_for_bgutil(temp.path()), None);
    }
}
