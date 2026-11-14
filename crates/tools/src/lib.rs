//! Verified, pinned installs of helper tools (D-011). yt-dlp is never updated by itself
//! (`yt-dlp -U`): we fetch a release from its GitHub page, verify its `SHA2-256SUMS` against
//! yt-dlp's OpenPGP signing key (pinned in `keys/yt-dlp.asc`), verify the program against that
//! list, install it next to earlier versions (`tools/yt-dlp/<version>/`), switch to it
//! atomically, never downgrade, and keep the previous version for rollback.

pub mod chosen;
pub mod ffmpeg;
pub mod gallery;
mod macos;
pub mod manifest;
mod openpgp;
pub mod pages;
pub mod pieces;
pub mod playlist;
pub mod proc;
pub mod relay;
pub mod resolve;
pub mod servers;
pub mod streams;
// System helper discovery is for Linux and macOS; Windows keeps its verified managed copies.
#[cfg(unix)]
mod system;
pub mod update;
pub mod video;
pub mod workdir;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// yt-dlp's release signing key, from its repository (`public.key`).
const YTDLP_KEY: &str = include_str!("../keys/yt-dlp.asc");
/// Mister Download Manager's release signing key (Ed25519, fingerprint
/// 5826D399249DE856EA087D802E293C28E8A69D74): release.yml signs each release's SHA256SUMS.txt
/// with it (the MDM_RELEASE_KEY secret), and MDM updates itself only from a list it signed.
const MDM_KEY: &str = include_str!("../keys/mdm-release.asc");
/// Where releases come from.
const RELEASES: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("disk: {0}")]
    Io(#[from] std::io::Error),
    #[error("the release's checksum list is not signed by its publisher's key")]
    BadSignature,
    #[error("the downloaded program doesn't match its signed checksum")]
    BadChecksum,
    #[error("the release has no {0}")]
    Missing(String),
    #[error("unexpected release data: {0}")]
    BadRelease(String),
    /// A program chosen as yt-dlp or ffmpeg (`chosen`) didn't answer as one.
    #[error("this program isn't {0}")]
    NotTheProgram(&'static str),
    /// A program chosen as a helper is a release MDM can't drive (#236: gallery-dl before 1.20).
    #[error("{name} {version} is too old: it needs {oldest} or newer")]
    TooOld {
        name: &'static str,
        version: String,
        oldest: &'static str,
    },
    /// MDM's engine could not download it from `host` (#816: which server, and why).
    #[error("{host}: {error}")]
    Download {
        host: String,
        error: mdm_engine::Error,
    },
    /// The chosen network adapter is down or can't reach the server (#35).
    #[error(transparent)]
    Adapter(#[from] mdm_engine::NetError),
}

impl Error {
    /// The server a network failure was with, and its kind (#816); None for any other error.
    pub fn network(&self) -> Option<(String, mdm_engine::fault::Fault)> {
        match self {
            Error::Http(e) => {
                let host = e.url()?.host_str()?.to_owned();
                Some((host, mdm_engine::fault::of_http(e)))
            }
            Error::Download { host, error } => {
                Some((host.clone(), mdm_engine::fault::of_engine(error)?))
            }
            _ => None,
        }
    }
}

/// A client for MDM's own requests to `url`'s server (a release list, its checksums): through
/// `net`'s proxy (Options → Proxy / Socks), DNS and network adapter, as the engine's downloads
/// go (#816: the release list went past the proxy the download itself took).
pub(crate) fn client(net: &mdm_engine::Options, url: &str) -> Result<reqwest::Client, Error> {
    let parsed = url::Url::parse(url).map_err(|e| Error::BadRelease(e.to_string()))?;
    let host = parsed.host_str().unwrap_or_default().to_owned();
    mdm_engine::http_client(net, &parsed).map_err(|e| match e {
        mdm_engine::Error::Adapter(a) => Error::Adapter(a),
        error => Error::Download { host, error },
    })
}

/// An installed tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub version: String,
    pub exe: PathBuf,
}

/// `Installed::version` of a program found on the system: its package manager's, not asked (#452).
const SYSTEM: &str = "system";

impl Installed {
    /// Whether this is the system's copy (Linux and macOS), kept current by its package manager.
    pub fn is_system(&self) -> bool {
        self.version == SYSTEM
    }
}

/// `<data dir>/tools`.
pub fn tools_dir() -> Option<PathBuf> {
    mdm_ipc::data_dir().map(|d| d.join("tools"))
}

/// yt-dlp's file name for this platform in its releases. On macOS, its folder build: why, in
/// `macos`.
pub fn ytdlp_asset() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "yt-dlp_macos.zip"
    } else {
        "yt-dlp_linux"
    }
}

/// Whether `sums` carries a valid signature `sig` (binary OpenPGP) by yt-dlp's key or one of its
/// subkeys.
pub fn signed_by_ytdlp(sums: &[u8], sig: &[u8]) -> bool {
    signed_by(YTDLP_KEY, sums, sig)
}

/// Whether `sums` (a release's SHA256SUMS.txt) carries a valid signature `sig` by Mister
/// Download Manager's release key.
pub fn signed_by_mdm(sums: &[u8], sig: &[u8]) -> bool {
    if signed_by(MDM_KEY, sums, sig) {
        return true;
    }
    // Update now's end-to-end tests (feature `test-update`, never in a release): a fake
    // release on a lab server, signed with a test key of their own.
    #[cfg(feature = "test-update")]
    if let Some(key) =
        std::env::var_os("MDM_TEST_UPDATE_KEY").and_then(|p| std::fs::read_to_string(p).ok())
    {
        return signed_by(&key, sums, sig);
    }
    false
}

/// Whether `data` carries a valid detached signature `sig` (binary OpenPGP) by the armored
/// public `key` or one of its subkeys.
pub fn signed_by(key: &str, data: &[u8], sig: &[u8]) -> bool {
    openpgp::verify(key.as_bytes(), data, sig)
}

/// `url`'s body.
pub(crate) async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, Error> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
}

/// A release's checksum list (`hash  name` lines), fetched with its detached signature and
/// accepted only when `signed` (the publisher's key) says so.
pub(crate) async fn signed_list(
    client: &reqwest::Client,
    sums_url: &str,
    sig_url: &str,
    signed: fn(&[u8], &[u8]) -> bool,
) -> Result<String, Error> {
    let sums = get_bytes(client, sums_url).await?;
    let sig = get_bytes(client, sig_url).await?;
    if !signed(&sums, &sig) {
        return Err(Error::BadSignature);
    }
    Ok(String::from_utf8_lossy(&sums).into_owned())
}

/// MDM's own name, sent when it fetches its helpers and its updates.
pub const USER_AGENT: &str = concat!("MisterDownloadManager/", env!("CARGO_PKG_VERSION"));

/// How MDM downloads its helpers and its updates: through its own engine (several connections,
/// resumable), as MDM itself, through `proxy` (Options → Proxy / Socks).
pub fn net(proxy: mdm_engine::Proxy) -> mdm_engine::Options {
    mdm_engine::Options {
        user_agent: USER_AGENT.to_owned(),
        automatic_agent: false,
        proxy,
        ..Default::default()
    }
}

/// Download `url` into `path` with MDM's own engine (`net`: its connections, the user's proxy),
/// counting into `progress`, and accept it only when its SHA-256 is `expected` (the engine checks
/// it, and so do we, on the finished file). An earlier attempt cut off is resumed from where it
/// stopped. A file that doesn't match is removed.
pub(crate) async fn fetch_verified(
    url: &str,
    path: &Path,
    expected: [u8; 32],
    net: &mdm_engine::Options,
    progress: Option<&update::Progress>,
) -> Result<(), Error> {
    use mdm_engine::verify::{Algo, Hash};
    let url = url::Url::parse(url).map_err(|e| Error::BadRelease(e.to_string()))?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let opts = mdm_engine::Options {
        checksum: Some(Hash {
            algo: Algo::Sha256,
            value: expected.to_vec(),
        }),
        ..net.clone()
    };
    let got = mdm_engine::fetch(url, path, opts, |done, total| {
        if let Some(p) = progress {
            use std::sync::atomic::Ordering::Relaxed;
            p.got.store(done, Relaxed);
            p.total.store(total.unwrap_or(0), Relaxed);
        }
    })
    .await;
    let state = mdm_engine::state::state_path(path);
    let forget = || {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(mdm_engine::naming::part_path(path));
        let _ = std::fs::remove_file(&state);
    };
    match got {
        Ok(_) if sha256_of(path)? == expected => Ok(()),
        Ok(_) | Err(mdm_engine::Error::ChecksumMismatch(_)) => {
            forget();
            Err(Error::BadChecksum)
        }
        Err(error) => Err(Error::Download { host, error }),
    }
}

/// The SHA-256 of the file at `path`.
fn sha256_of(path: &Path) -> Result<[u8; 32], Error> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hash.finalize().into());
        }
        hash.update(&buf[..n]);
    }
}

/// The SHA-256 a `hash  name` list gives for `name`.
pub fn sum_for(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        if file.trim().trim_start_matches('*') != name {
            return None;
        }
        hex::decode(hash).ok()?.try_into().ok()
    })
}

/// The yt-dlp to use: the person's own (`chosen`), else MDM's install in `dir`
/// (`dir/yt-dlp/current.json`), if any and still there; on Linux and macOS, else the system's.
pub fn installed(dir: &Path) -> Option<Installed> {
    let own = chosen::chosen(dir, chosen::Helper::YtDlp).or_else(|| own_ytdlp(dir));
    #[cfg(unix)]
    return own.or_else(|| system::installed("yt-dlp"));
    #[cfg(not(unix))]
    own
}

fn own_ytdlp(dir: &Path) -> Option<Installed> {
    let text = std::fs::read(dir.join("yt-dlp").join("current.json")).ok()?;
    let i: Installed = serde_json::from_slice(&text).ok()?;
    // macOS: the one-file build earlier versions installed can't run from a downloaded app (`macos`).
    if cfg!(target_os = "macos") && macos::one_file_build(&i.exe) {
        return None;
    }
    i.exe.is_file().then_some(i)
}

/// Versions are release dates (`2026.10.26`, maybe `.1`): numeric parts compare in order.
pub(crate) fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').filter_map(|p| p.parse().ok()).collect() };
    parts(a) > parts(b)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Whether the once-per-session update has a yt-dlp to refresh: not the system's, which is its
/// package manager's to update (#452), and which only an explicit [`install_latest`] replaces.
pub fn refreshable(dir: &Path) -> bool {
    installed(dir).is_some_and(|i| !i.is_system())
}

/// Install the latest yt-dlp release into `dir` unless the installed one is as new, the program
/// downloaded with MDM's engine (`net`), counted into `progress`. Returns what is installed
/// afterwards. A system copy doesn't count as installed here: this is the way to get MDM's own
/// instead (Helpers), and MDM's own then wins ([`installed`]).
pub async fn install_latest(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&update::Progress>,
) -> Result<Installed, Error> {
    // The person's own copy: theirs to update (#51).
    if let Some(mine) = chosen::chosen(dir, chosen::Helper::YtDlp) {
        return Ok(mine);
    }
    let client = client(net, RELEASES)?;
    let body = client
        .get(RELEASES)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let release: Release =
        serde_json::from_slice(&body).map_err(|e| Error::BadRelease(e.to_string()))?;
    if let Some(current) = own_ytdlp(dir)
        && !newer(&release.tag_name, &current.version)
    {
        return Ok(current); // never the same again, never older
    }
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| Error::Missing(name.to_owned()))
    };
    let sums = signed_list(
        &client,
        &url("SHA2-256SUMS")?,
        &url("SHA2-256SUMS.sig")?,
        signed_by_ytdlp,
    )
    .await?;
    let asset = ytdlp_asset();
    let expected =
        sum_for(&sums, asset).ok_or_else(|| Error::Missing(format!("checksum for {asset}")))?;
    let exe_url = url(asset)?;
    let root = dir.join("yt-dlp");
    std::fs::create_dir_all(&root)?;
    // Named by its checksum: one release's cut-off download resumes, never another's.
    let download = root.join(format!("yt-dlp-{}.download", hex::encode(&expected[..8])));
    fetch_verified(&exe_url, &download, expected, net, progress).await?;
    let program = std::fs::read(&download)?;
    let _ = std::fs::remove_file(&download);
    if Sha256::digest(&program).as_slice() != expected {
        return Err(Error::BadChecksum);
    }
    #[cfg(target_os = "macos")]
    return macos::put_folder(dir, &release.tag_name, &program);
    #[cfg(not(target_os = "macos"))]
    put(dir, "yt-dlp", &release.tag_name, asset, &program, &exe_url)
}

/// Install verified `program` as `version` of `tool` (`dir/<tool>/<version>/<asset>`) and make it
/// current (atomically), keeping the previous version for rollback and removing older ones.
pub(crate) fn put(
    dir: &Path,
    tool: &str,
    version: &str,
    asset: &str,
    program: &[u8],
    from: &str,
) -> Result<Installed, Error> {
    if version.is_empty() || version.contains(['/', '\\', ':']) || version.starts_with('.') {
        return Err(Error::BadRelease(format!("version {version:?}")));
    }
    let root = dir.join(tool);
    let folder = root.join(version);
    std::fs::create_dir_all(&folder)?;
    let exe = folder.join(asset);
    craft_atomic::atomic_write(&exe, program)?;
    mark_downloaded(&exe, from);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
    }
    make_current(
        &root,
        Installed {
            version: version.to_owned(),
            exe,
        },
    )
}

/// Make `now` (installed in `root/<version>/`) the current version of a tool: `current.json`
/// switched atomically, the previous version kept for rollback, older ones removed.
pub(crate) fn make_current(root: &Path, now: Installed) -> Result<Installed, Error> {
    let previous = std::fs::read(root.join("current.json"))
        .ok()
        .and_then(|t| serde_json::from_slice::<Installed>(&t).ok());
    let json = serde_json::to_vec_pretty(&now).map_err(std::io::Error::other)?;
    craft_atomic::atomic_write(&root.join("current.json"), &json)?;
    // Keep the new and the previous version; older ones go.
    let keep: Vec<String> = [Some(now.version.clone()), previous.map(|p| p.version)]
        .into_iter()
        .flatten()
        .collect();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !keep.contains(&name) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(now)
}

/// Mark-of-the-Web: the program came from the internet (Windows checks it like a browser
/// download).
pub(crate) fn mark_downloaded(path: &Path, from: &str) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(
            stream,
            format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={from}\r\n"),
        );
    }
    #[cfg(not(windows))]
    let _ = (path, from);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS");
    const SIG: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS.sig");

    #[test]
    fn a_real_release_checksum_list_is_signed_by_the_pinned_key() {
        assert!(signed_by_ytdlp(SUMS, SIG));
    }

    #[test]
    fn mdm_releases_are_checked_against_mdms_own_key() {
        let sums = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt");
        let sig = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt.sig");
        assert!(signed_by_mdm(sums, sig));
        let mut forged = sums.to_vec();
        forged[0] = if forged[0] == b'0' { b'1' } else { b'0' };
        assert!(!signed_by_mdm(&forged, sig));
        // Each key signs for itself only.
        assert!(!signed_by_ytdlp(sums, sig));
        assert!(!signed_by_mdm(SUMS, SIG));
    }

    #[test]
    fn a_changed_list_fails_the_signature() {
        let mut forged = SUMS.to_vec();
        forged[0] = if forged[0] == b'0' { b'1' } else { b'0' };
        assert!(!signed_by_ytdlp(&forged, SIG));
        assert!(!signed_by_ytdlp(SUMS, b"not a signature"));
    }

    #[test]
    fn checksums_by_name() {
        let sums = String::from_utf8_lossy(SUMS);
        let exe = sum_for(&sums, "yt-dlp.exe").unwrap();
        assert_eq!(
            hex::encode(exe),
            "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d"
        );
        assert!(sum_for(&sums, "yt-dlp.ex").is_none());
    }

    #[test]
    fn versions_compare_as_dates() {
        assert!(newer("2026.10.26", "2026.9.30"));
        assert!(newer("2026.10.26.1", "2026.10.26"));
        assert!(!newer("2026.10.26", "2026.10.26"));
    }

    #[test]
    fn installs_switch_and_keep_one_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        let name = "yt-dlp-test-program";
        put(
            dir.path(),
            "yt-dlp",
            "2026.01.01",
            name,
            b"one",
            "https://x/",
        )
        .unwrap();
        put(
            dir.path(),
            "yt-dlp",
            "2026.02.01",
            name,
            b"two",
            "https://x/",
        )
        .unwrap();
        let now = put(
            dir.path(),
            "yt-dlp",
            "2026.03.01",
            name,
            b"three",
            "https://x/",
        )
        .unwrap();
        // macOS installs the folder build (`macos`), with a folder beside the program.
        #[cfg(target_os = "macos")]
        std::fs::create_dir_all(now.exe.with_file_name("_internal")).unwrap();
        assert_eq!(installed(dir.path()).unwrap(), now);
        assert_eq!(std::fs::read(&now.exe).unwrap(), b"three");
        let root = dir.path().join("yt-dlp");
        assert!(
            root.join("2026.02.01").is_dir(),
            "the previous version stays"
        );
        assert!(!root.join("2026.01.01").exists(), "older ones go");
        assert!(put(dir.path(), "yt-dlp", "../evil", name, b"x", "https://x/").is_err());
    }
}
