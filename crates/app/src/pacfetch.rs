//! The proxy auto-config script of Options → Proxy / Socks ("Use automatic configuration
//! script"): fetched from its address (http, https, file or a path) when the settings are
//! loaded or changed, and kept for the engine to run (mdm_engine::pac). Until it has arrived,
//! or when it can't be had, downloads use the system's proxy settings. The system's own script
//! (Windows' "Use setup script") is fetched the same way for "Use system settings"
//! ([`watch_system`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static SCRIPTS: Mutex<Option<HashMap<String, Arc<str>>>> = Mutex::new(None);

/// The script fetched from `address`, if it has arrived.
pub fn script(address: &str) -> Option<Arc<str>> {
    SCRIPTS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(address.trim()).cloned())
}

/// The system's automatic configuration script ("Use setup script" in Windows' proxy settings):
/// its address as last read, when it was read, and when its script was last fetched.
struct System {
    address: Option<String>,
    read: Option<Instant>,
    fetched: Option<Instant>,
}

static SYSTEM: Mutex<System> = Mutex::new(System {
    address: None,
    read: None,
    fetched: None,
});

/// How often the system's script address is read again: a proxy program turning its PAC mode
/// on or off is followed within this.
const READ_EVERY: Duration = Duration::from_secs(5);
/// How often the system's script is fetched again: its proxy program changes it with its rules.
const FETCH_EVERY: Duration = Duration::from_secs(10 * 60);
/// How soon a system script that couldn't be fetched (its program not started yet) is asked
/// for again.
const RETRY_EVERY: Duration = Duration::from_secs(30);

/// "Use system settings" (and the browser's proxy, which is the system's) with the system's
/// automatic configuration script on (#1448: Clash, v2rayN and the like in PAC mode set one):
/// its address read again every few seconds, its script fetched when it changes, now and then
/// besides. Called with the app's clock: the window's thread only looks whether a read is due,
/// the system is asked on a thread of the runtime's (it may be slow to answer).
pub fn watch_system(rt: &tokio::runtime::Handle) {
    {
        let mut system = SYSTEM.lock().unwrap();
        let now = Instant::now();
        if system.read.is_some_and(|at| now < at + READ_EVERY) {
            return;
        }
        system.read = Some(now);
    }
    let handle = rt.clone();
    rt.spawn_blocking(move || read_system(&handle));
}

/// [`watch_system`]'s read of the system's script address, and its fetch when due.
fn read_system(rt: &tokio::runtime::Handle) {
    let address = system_script_address();
    let mut system = SYSTEM.lock().unwrap();
    let now = Instant::now();
    let changed = address != system.address;
    let again = if address.as_deref().is_some_and(|a| script(a).is_none()) {
        RETRY_EVERY
    } else {
        FETCH_EVERY
    };
    let due = changed || system.fetched.is_none_or(|at| now >= at + again);
    if changed {
        tracing::info!(
            "The system's automatic proxy configuration script: {}",
            address.as_deref().unwrap_or("none")
        );
    }
    system.address.clone_from(&address);
    if let Some(address) = address.filter(|_| due) {
        system.fetched = Some(now);
        refresh(&address, rt);
    }
}

/// The system's automatic configuration script, once fetched ([`watch_system`]); None when the
/// system has none on, or it hasn't arrived (the system's proxy settings are used meanwhile).
pub fn system_script() -> Option<Arc<str>> {
    let address = SYSTEM.lock().unwrap().address.clone()?;
    script(&address)
}

/// The address of the system's automatic configuration script, when it is on.
fn system_script_address() -> Option<String> {
    #[cfg(windows)]
    return crate::win32::system_proxy_script();
    // Linux desktops and macOS keep theirs in their own settings (GNOME's, KDE's, the network
    // service's in macOS), each read its own way: not read yet. Their manual proxy reaches MDM
    // through the environment (`HTTPS_PROXY`) and, on macOS, the system's settings.
    #[cfg(not(windows))]
    None
}

/// Fetch the script at `address` now and keep it, waiting for it (20 s at most): for a process
/// that runs one command and has no time to wait for [`refresh`] (the command line, #1556).
/// None when it can't be had (said on standard error): the system's settings are used then,
/// as in the app.
pub fn fetch_now(address: &str) -> Option<Arc<str>> {
    let address = address.trim().to_owned();
    if address.is_empty() {
        return None;
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    match rt.block_on(fetch(&address)) {
        Ok(text) => {
            let script: Arc<str> = Arc::from(text.as_str());
            SCRIPTS
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(address, script.clone());
            Some(script)
        }
        Err(why) => {
            eprintln!(
                "The automatic proxy configuration script {address} couldn't be fetched ({why}): the system's proxy settings are used"
            );
            None
        }
    }
}

/// The system's automatic configuration script read and fetched now ([`fetch_now`]), for "Use
/// system settings" in a process that doesn't watch it ([`watch_system`] is the app's).
pub fn system_now() {
    let address = system_script_address();
    {
        let mut system = SYSTEM.lock().unwrap();
        system.address.clone_from(&address);
        system.read = Some(Instant::now());
    }
    if let Some(address) = address {
        fetch_now(&address);
        SYSTEM.lock().unwrap().fetched = Some(Instant::now());
    }
}

/// Fetch the script at `address` (again), in the background.
pub fn refresh(address: &str, rt: &tokio::runtime::Handle) {
    let address = address.trim().to_owned();
    if address.is_empty() {
        return;
    }
    rt.spawn(async move {
        if let Ok(text) = fetch(&address).await {
            SCRIPTS
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(address, Arc::from(text.as_str()));
        }
    });
}

async fn fetch(address: &str) -> Result<String, String> {
    if let Ok(url) = url::Url::parse(address)
        && matches!(url.scheme(), "http" | "https")
    {
        // Directly: the script says which proxy to use, it isn't fetched through one. Through
        // the chosen network adapter (#35): a script on the local network can't be reached
        // through a VPN's, and the one saved last stays in use.
        let local = mdm_engine::net::is_this_computer(url.host_str().unwrap_or(""));
        let route = crate::settings::own_route();
        mdm_engine::net::check_url(&route, &url).map_err(|e| e.to_string())?;
        // The system's DNS (local network names), only the addresses the adapter reaches.
        let resolver = mdm_engine::dns::client_resolver(&Default::default(), &route);
        let client = mdm_engine::net::bind_client(
            reqwest::Client::builder().no_proxy().dns_resolver(resolver),
            &route,
            local,
        )
        .map_err(|e| e.to_string())?
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
        let r = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status()));
        }
        return r.text().await.map_err(|e| e.to_string());
    }
    let path = url::Url::parse(address)
        .ok()
        .filter(|u| u.scheme() == "file")
        .and_then(|u| u.to_file_path().ok())
        .unwrap_or_else(|| std::path::PathBuf::from(address));
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}
