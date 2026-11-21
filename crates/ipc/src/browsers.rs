//! The browsers the extension runs in, and where each reads native messaging host manifests:
//! one table for the native host (which writes the manifests), and the app (which lists the
//! browsers and checks the manifests are there), #41.
//!
//! Sources: each browser's own code or packaging (Chromium's `launch_context_win.cc` and
//! `chrome_paths*.cc`, Firefox's `nsXREDirProvider.cpp`, Helium's
//! `scan-chrome-native-messaging-hosts.patch` and branding patches, Zen's issue #10622, Brave's
//! `brave_main_delegate.cc`, `chrome_paths_linux.cc` and `chromium_install_modes.h`); folders
//! marked "unverified" in issue #41 come from other projects' installers and cost nothing when
//! absent.
//!
//! Everyone's folders on Linux (`LINUX_SYSTEM_HOST_DIRS`), which the .deb and .rpm ship a manifest
//! in (#266), each browser's documented one: Chrome's and Chromium's from Chrome's "Native
//! messaging" guide (developer.chrome.com/docs/extensions/develop/concepts/native-messaging,
//! "Native messaging host location") and `chrome_paths.cc` (`DIR_NATIVE_MESSAGING`: Chrome's
//! branding reads `/etc/opt/chrome`, every other build `/etc/chromium`); Edge's from Microsoft's
//! (learn.microsoft.com/microsoft-edge/extensions/developer-guide/native-messaging, Linux);
//! Brave's from `brave_main_delegate.cc` (Linux: Chrome's, `/etc/opt/chrome`); Vivaldi, built
//! without Chrome's branding, reads Chromium's (as Vivaldi's forum confirms); Firefox's from
//! MDN's "Native manifests" (Linux, global: `/usr/lib/mozilla` or `/usr/lib64/mozilla`); Helium
//! reads Chromium's, then Chrome's (its patch, above). Opera, Thorium and Firefox's forks
//! document none: they keep their manifest in the home folder.

use std::path::{Path, PathBuf};

/// Which manifest a browser reads: Chromium's (`allowed_origins`) or Firefox's
/// (`allowed_extensions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Gecko,
}

/// A browser MDM knows by name.
#[derive(Debug)]
pub struct Browser {
    /// Its name in the app (the offer window, Options).
    pub name: &'static str,
    pub family: Family,
    /// macOS: its app bundles, in /Applications or ~/Applications.
    pub mac_apps: &'static [&'static str],
    /// Linux: its commands on the PATH.
    pub linux_cmds: &'static [&'static str],
    /// macOS: its data folders under ~/Library/Application Support. A Chromium browser reads
    /// `<folder>/NativeMessagingHosts`; for a Gecko one the folder only says it is there.
    pub mac_data: &'static [&'static str],
    /// Linux: the same, under $XDG_CONFIG_HOME (~/.config).
    pub linux_config: &'static [&'static str],
    /// Linux: Gecko data folders under $HOME (`.zen`).
    pub linux_home: &'static [&'static str],
    /// Linux: the folders for everyone it reads host manifests from as well, after its user
    /// folder (`LINUX_SYSTEM_HOST_DIRS`); none known for some.
    pub linux_system: &'static [&'static str],
    /// macOS: the folder under ~/Library/Application Support whose `NativeMessagingHosts` a
    /// Chromium browser reads when it isn't its own data folder (Brave reads Chrome's).
    pub mac_hosts: Option<&'static str>,
    /// It offers the extensions named in its data folders' `External Extensions` (Chromium's
    /// DIR_USER_EXTERNAL_EXTENSIONS, read for one user) at its next start: on Linux (Chromium's
    /// branding only, not Chrome's) and on macOS ([`external_dirs`]).
    pub linux_user_external: bool,
    pub mac_user_external: bool,
    /// The store's update service as it names it there.
    pub store_update_url: &'static str,
}

const fn chromium(
    name: &'static str,
    mac_apps: &'static [&'static str],
    linux_cmds: &'static [&'static str],
    mac_data: &'static [&'static str],
    linux_config: &'static [&'static str],
) -> Browser {
    Browser {
        name,
        family: Family::Chromium,
        mac_apps,
        linux_cmds,
        mac_data,
        linux_config,
        linux_home: &[],
        linux_system: &[],
        mac_hosts: None,
        linux_user_external: false,
        mac_user_external: false,
        store_update_url: crate::extension::STORE_UPDATE_URL,
    }
}

impl Browser {
    /// On macOS it reads host manifests from `folder`'s `NativeMessagingHosts`, not its own.
    const fn mac_hosts_in(mut self, folder: &'static str) -> Browser {
        self.mac_hosts = Some(folder);
        self
    }

    /// On Linux it reads the manifests for everyone in `folders` too.
    const fn linux_system_in(mut self, folders: &'static [&'static str]) -> Browser {
        self.linux_system = folders;
        self
    }

    /// It reads `External Extensions` in its data folders, for one user, on Linux and macOS.
    const fn user_external(mut self, linux: bool, mac: bool) -> Browser {
        self.linux_user_external = linux;
        self.mac_user_external = mac;
        self
    }

    /// It names the store's update service `url` (Helium).
    const fn store_update_url(mut self, url: &'static str) -> Browser {
        self.store_update_url = url;
        self
    }
}

/// Linux: Chrome's folder for everyone (Google's builds of Chrome, every channel; Brave).
const ETC_CHROME: &str = "/etc/opt/chrome/native-messaging-hosts";
/// Linux: Chromium's (every build without Chrome's branding that keeps Chromium's: Vivaldi).
const ETC_CHROMIUM: &str = "/etc/chromium/native-messaging-hosts";
/// Linux: Edge's.
const ETC_EDGE: &str = "/etc/opt/edge/native-messaging-hosts";
/// Linux: Firefox's, `lib64`'s on 64-bit Fedora and openSUSE (MDN, "Native manifests").
const MOZILLA: [&str; 2] = [
    "/usr/lib/mozilla/native-messaging-hosts",
    "/usr/lib64/mozilla/native-messaging-hosts",
];

/// Linux: every folder for everyone a browser reads host manifests from, with the family whose
/// manifest goes there: where the .deb and .rpm put theirs (`misterdownloadmanager-nmhost
/// --package-manifests`), and `--register --all-users` too.
pub const LINUX_SYSTEM_HOST_DIRS: &[(&str, Family)] = &[
    (ETC_CHROME, Family::Chromium),
    (ETC_CHROMIUM, Family::Chromium),
    (ETC_EDGE, Family::Chromium),
    (MOZILLA[0], Family::Gecko),
    (MOZILLA[1], Family::Gecko),
];

const fn gecko(
    name: &'static str,
    mac_apps: &'static [&'static str],
    linux_cmds: &'static [&'static str],
    mac_data: &'static [&'static str],
    linux_config: &'static [&'static str],
    linux_home: &'static [&'static str],
) -> Browser {
    Browser {
        name,
        family: Family::Gecko,
        mac_apps,
        linux_cmds,
        mac_data,
        linux_config,
        linux_home,
        linux_system: &[],
        mac_hosts: None,
        linux_user_external: false,
        mac_user_external: false,
        store_update_url: crate::extension::STORE_UPDATE_URL,
    }
}

/// Every browser MDM names, Chromium's first.
pub const BROWSERS: &[Browser] = &[
    // Each channel keeps its own folder (Chromium's chrome_paths_linux.cc and chrome_paths_mac.mm).
    chromium(
        "Google Chrome",
        &[
            "Google Chrome.app",
            "Google Chrome Beta.app",
            "Google Chrome Dev.app",
            "Google Chrome Canary.app",
        ],
        &[
            "google-chrome",
            "google-chrome-stable",
            "google-chrome-beta",
            "google-chrome-unstable",
            "google-chrome-canary",
        ],
        &[
            "Google/Chrome",
            "Google/Chrome Beta",
            "Google/Chrome Dev",
            "Google/Chrome Canary",
        ],
        &[
            "google-chrome",
            "google-chrome-beta",
            "google-chrome-unstable",
            "google-chrome-canary",
        ],
    )
    .linux_system_in(&[ETC_CHROME])
    .user_external(false, true),
    chromium(
        "Chromium",
        &["Chromium.app"],
        &["chromium", "chromium-browser"],
        &["Chromium"],
        &["chromium"],
    )
    .linux_system_in(&[ETC_CHROMIUM]),
    chromium(
        "Microsoft Edge",
        &[
            "Microsoft Edge.app",
            "Microsoft Edge Beta.app",
            "Microsoft Edge Dev.app",
            "Microsoft Edge Canary.app",
        ],
        &[
            "microsoft-edge",
            "microsoft-edge-stable",
            "microsoft-edge-beta",
            "microsoft-edge-dev",
        ],
        &[
            "Microsoft Edge",
            "Microsoft Edge Beta",
            "Microsoft Edge Dev",
            "Microsoft Edge Canary",
        ],
        &[
            "microsoft-edge",
            "microsoft-edge-beta",
            "microsoft-edge-dev",
        ],
    )
    .linux_system_in(&[ETC_EDGE]),
    // Brave and Brave Origin (#253), each channel in its own folder (`-Beta`, `-Dev`, `-Nightly`),
    // all named "Brave" to the extension. On Linux each reads its own folder; on macOS all read
    // Chrome's (brave_main_delegate.cc); on Windows, Chromium's and Chrome's keys.
    chromium(
        "Brave",
        &[
            "Brave Browser.app",
            "Brave Browser Beta.app",
            "Brave Browser Nightly.app",
            "Brave Origin.app",
            "Brave Origin Beta.app",
            "Brave Origin Nightly.app",
        ],
        &[
            "brave-browser",
            "brave",
            "brave-browser-beta",
            "brave-browser-nightly",
            "brave-origin",
            "brave-origin-beta",
            "brave-origin-nightly",
        ],
        &[
            "BraveSoftware/Brave-Browser",
            "BraveSoftware/Brave-Browser-Beta",
            "BraveSoftware/Brave-Browser-Nightly",
            "BraveSoftware/Brave-Origin",
            "BraveSoftware/Brave-Origin-Beta",
            "BraveSoftware/Brave-Origin-Nightly",
        ],
        &[
            "BraveSoftware/Brave-Browser",
            "BraveSoftware/Brave-Browser-Beta",
            "BraveSoftware/Brave-Browser-Dev",
            "BraveSoftware/Brave-Browser-Nightly",
            "BraveSoftware/Brave-Origin",
            "BraveSoftware/Brave-Origin-Beta",
            "BraveSoftware/Brave-Origin-Dev",
            "BraveSoftware/Brave-Origin-Nightly",
        ],
    )
    .mac_hosts_in("Google/Chrome")
    .linux_system_in(&[ETC_CHROME]),
    chromium(
        "Vivaldi",
        &["Vivaldi.app"],
        &["vivaldi", "vivaldi-stable", "vivaldi-snapshot"],
        &["Vivaldi"],
        &["vivaldi", "vivaldi-snapshot"],
    )
    .linux_system_in(&[ETC_CHROMIUM]),
    // Opera's folders: unverified.
    chromium(
        "Opera",
        &["Opera.app"],
        &["opera"],
        &["com.operasoftware.Opera"],
        &["opera"],
    ),
    // Helium reads its own folder, then Chrome's (on Linux and macOS); `.dev` is an unofficial
    // build's. On Linux, built with Chromium's branding, it then reads Chromium's folder for
    // everyone, then Chrome's (its scan-chrome-native-messaging-hosts.patch; both paths are in
    // its program, #685), so the .deb's and .rpm's manifest covers it. On Windows it reads
    // Chromium's key, then Chrome's (launch_context_win.cc), as Chromium does. It offers the
    // extensions named in its own folder's `External Extensions` on Linux and macOS, through its
    // services when they name the store by its own address; Chrome's entries (the registry's on
    // Windows) name Google's, which answers it "no update", so it offers nothing from them (#685).
    chromium(
        "Helium",
        &["Helium.app"],
        &["helium", "helium-browser"],
        &["net.imput.helium"],
        &["net.imput.helium", "net.imput.helium.dev"],
    )
    .linux_system_in(&[ETC_CHROMIUM, ETC_CHROME])
    .user_external(true, true)
    .store_update_url(crate::extension::HELIUM_STORE_UPDATE_URL),
    // Thorium's and Arc's folders: unverified.
    chromium(
        "Thorium",
        &["Thorium.app"],
        &["thorium-browser"],
        &["Thorium"],
        &["thorium"],
    ),
    chromium("Arc", &["Arc.app"], &[], &["Arc/User Data"], &[]),
    gecko(
        "Mozilla Firefox",
        &["Firefox.app"],
        &["firefox"],
        &["Mozilla", "Firefox"],
        &["mozilla"],
        // The Snap (Ubuntu's own Firefox) keeps its profile in its snap folder and never makes
        // ~/.mozilla; it reaches the host through the desktop portal, which reads ~/.mozilla's,
        // then Mozilla's folders for everyone (xdg-desktop-portal's WebExtensions portal).
        &[".mozilla", "snap/firefox/common/.mozilla"],
    )
    .linux_system_in(&MOZILLA),
    gecko(
        "Zen Browser",
        &["Zen.app", "Zen Browser.app"],
        &["zen", "zen-browser", "zen-bin"],
        &["Zen"],
        &["zen"],
        &[".zen"],
    ),
    gecko(
        "LibreWolf",
        &["LibreWolf.app"],
        &["librewolf"],
        &["LibreWolf"],
        &[],
        &[".librewolf"],
    ),
    // Floorp's and Waterfox's: unverified (no patch found; they read Firefox's folder).
    gecko(
        "Floorp",
        &["Floorp.app"],
        &["floorp"],
        &["Floorp"],
        &[],
        &[".floorp"],
    ),
    gecko(
        "Waterfox",
        &["Waterfox.app"],
        &["waterfox"],
        &["Waterfox"],
        &[],
        &[".waterfox"],
    ),
];

/// The browser named `name` in the table.
pub fn named(name: &str) -> Option<&'static Browser> {
    BROWSERS.iter().find(|b| b.name == name)
}

/// Windows: the programs of the browsers in the table, with the folder that tells apart those
/// that share a program's name (Chrome, Chromium and Helium all install a `chrome.exe`).
pub const WINDOWS_PROGRAMS: &[(&str, Option<&str>, &str)] = &[
    ("chrome.exe", Some(r"\google\chrome\"), "Google Chrome"),
    ("chrome.exe", Some(r"\imput\helium\"), "Helium"),
    ("chrome.exe", Some(r"\chromium\"), "Chromium"),
    ("msedge.exe", None, "Microsoft Edge"),
    ("brave.exe", None, "Brave"),
    ("vivaldi.exe", None, "Vivaldi"),
    ("opera.exe", None, "Opera"),
    ("thorium.exe", None, "Thorium"),
    ("firefox.exe", None, "Mozilla Firefox"),
    ("zen.exe", None, "Zen Browser"),
    ("librewolf.exe", None, "LibreWolf"),
    ("floorp.exe", None, "Floorp"),
    ("waterfox.exe", None, "Waterfox"),
];

/// Windows: the browser `program` (a full path) is, by its file name and folder; none for a
/// program not in the table (or a `chrome.exe` in another folder).
pub fn windows_named(program: &str) -> Option<&'static str> {
    let lower = program.to_lowercase();
    let file = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
    WINDOWS_PROGRAMS
        .iter()
        .find(|(exe, folder, _)| *exe == file && folder.is_none_or(|f| lower.contains(f)))
        .map(|(_, _, name)| *name)
}

/// Windows: the program a `shell\open\command` value starts (`"C:\…\zen.exe" -osint -url "%1"`
/// or unquoted `C:\…\x.exe %1`).
pub fn command_program(command: &str) -> Option<String> {
    let c = command.trim();
    let program = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        let end = c.to_lowercase().find(".exe").map(|i| i + 4)?;
        &c[..end]
    };
    let program = program.trim();
    (!program.is_empty()).then(|| program.to_owned())
}

/// Windows lists Internet Explorer as a browser; it can't run the extension.
pub fn windows_can_run_extension(program: &str) -> bool {
    !program.to_lowercase().ends_with("iexplore.exe")
}

/// The host manifest's file name, in every folder.
pub const HOST_MANIFEST: &str = "com.misterdownloadmanager.host.json";

/// Linux and macOS (`mac`): the folders to write the host manifest into, each with the family
/// whose manifest goes there, for this user (`home`, `config` = $XDG_CONFIG_HOME) or for
/// everyone. A user's folder is given only for a browser that is there.
///
/// Firefox and its forks (Zen, LibreWolf, Floorp, Waterfox) all read Mozilla's own user folder,
/// `~/.mozilla/native-messaging-hosts` or `~/Library/Application Support/Mozilla/
/// NativeMessagingHosts`, even when they keep their data elsewhere (Firefox's newer
/// `~/.config/mozilla`, Zen's `~/.zen`, the Snap's `~/snap/firefox`): it is given, to be created,
/// when any of them is there.
///
/// `host` (Linux, for one user): the helper being registered. A browser that already reads a
/// manifest for everyone naming it (the .deb's or .rpm's, #266) gets no user folder: the package
/// manager removes that one with the package, and nothing is left in the home folder. Mozilla's
/// user folder is given while one Gecko browser there doesn't read such a manifest.
pub fn host_dirs(
    home: &Path,
    config: &Path,
    mac: bool,
    all_users: bool,
    host: Option<&Path>,
) -> Vec<(PathBuf, Family)> {
    host_dirs_in(Path::new("/"), home, config, mac, all_users, host)
}

/// `host_dirs`, with everyone's folders under `root` (`/`; a test's own folder).
fn host_dirs_in(
    root: &Path,
    home: &Path,
    config: &Path,
    mac: bool,
    all_users: bool,
    host: Option<&Path>,
) -> Vec<(PathBuf, Family)> {
    let system = |d: &str| root.join(d.trim_start_matches('/'));
    let mut dirs: Vec<(PathBuf, Family)> = Vec::new();
    if all_users {
        if mac {
            dirs.extend(
                [
                    "/Library/Google/Chrome/NativeMessagingHosts",
                    "/Library/Application Support/Chromium/NativeMessagingHosts",
                    "/Library/Microsoft/Edge/NativeMessagingHosts",
                ]
                .iter()
                .map(|d| (system(d), Family::Chromium)),
            );
            dirs.push((
                system("/Library/Application Support/Mozilla/NativeMessagingHosts"),
                Family::Gecko,
            ));
        } else {
            dirs.extend(LINUX_SYSTEM_HOST_DIRS.iter().map(|(d, f)| (system(d), *f)));
        }
        // Everyone's: only where the browser's folder is.
        dirs.retain(|(d, _)| d.parent().is_some_and(Path::is_dir));
        return dirs;
    }
    // A browser that reads a manifest for everyone naming this helper.
    let covered = |b: &Browser| {
        !mac && host.is_some_and(|host| {
            b.linux_system
                .iter()
                .any(|d| names_program(&system(d).join(HOST_MANIFEST), host))
        })
    };
    let support = home.join("Library/Application Support");
    let data = |b: &Browser| -> Vec<PathBuf> {
        if mac {
            b.mac_data.iter().map(|d| support.join(d)).collect()
        } else {
            b.linux_config
                .iter()
                .map(|d| config.join(d))
                .chain(b.linux_home.iter().map(|d| home.join(d)))
                .collect()
        }
    };
    let mut push = |d: PathBuf, f: Family| {
        if !dirs.iter().any(|(x, _)| *x == d) {
            dirs.push((d, f));
        }
    };
    let mut gecko_there = false;
    for b in BROWSERS {
        let there: Vec<PathBuf> = data(b).into_iter().filter(|d| d.is_dir()).collect();
        if there.is_empty() || covered(b) {
            continue;
        }
        match (b.family, b.mac_hosts.filter(|_| mac)) {
            // Brave on macOS: Chrome's folder, made if need be, whichever channel is there.
            (Family::Chromium, Some(hosts)) => {
                push(
                    support.join(hosts).join("NativeMessagingHosts"),
                    Family::Chromium,
                );
            }
            (Family::Chromium, None) => {
                for d in there {
                    push(d.join("NativeMessagingHosts"), Family::Chromium);
                }
            }
            (Family::Gecko, _) => gecko_there = true,
        }
    }
    if gecko_there {
        if mac {
            push(support.join("Mozilla/NativeMessagingHosts"), Family::Gecko);
            // LibreWolf reads its own first.
            if support.join("LibreWolf").is_dir() {
                push(
                    support.join("LibreWolf/NativeMessagingHosts"),
                    Family::Gecko,
                );
            }
        } else {
            push(home.join(".mozilla/native-messaging-hosts"), Family::Gecko);
            if home.join(".librewolf").is_dir() {
                push(
                    home.join(".librewolf/native-messaging-hosts"),
                    Family::Gecko,
                );
            }
        }
    }
    dirs
}

/// A browser's folder of external extensions in its data folder (Chromium's
/// DIR_USER_EXTERNAL_EXTENSIONS).
const USER_EXTERNAL: &str = "External Extensions";

/// Linux and macOS: the folders whose `<id>.json`, naming the store's update service as the
/// browser reading it names it (the second of each pair), makes it offer that extension at its
/// next start ("New extension added", the user turning it on there), for this user (`home`,
/// `config` = $XDG_CONFIG_HOME) or for everyone, each given only where its browser is.
///
/// For one user, each browser marked so reads `External Extensions` in its data folder: Chrome on
/// macOS, Helium on both, by its own name for the store (#685); Chrome on Linux reads none in the
/// home folder. For everyone, Chrome's: on macOS `/Library/Application Support/Google/Chrome/
/// External Extensions` (chrome_paths.cc, DIR_EXTERNAL_EXTENSIONS), on Linux
/// `/usr/share/google-chrome/extensions`. Helium reads macOS's too, and an entry there, read
/// before its own, would win with Google's address: none is written there for it. Windows offers
/// the extension through the registry (crates/nmhost).
pub fn external_dirs(
    home: &Path,
    config: &Path,
    mac: bool,
    all_users: bool,
) -> Vec<(PathBuf, &'static str)> {
    external_dirs_in(Path::new("/"), home, config, mac, all_users)
}

/// `external_dirs`, with everyone's folders under `root` (`/`; a test's own folder).
fn external_dirs_in(
    root: &Path,
    home: &Path,
    config: &Path,
    mac: bool,
    all_users: bool,
) -> Vec<(PathBuf, &'static str)> {
    let system = |d: &str| root.join(d.trim_start_matches('/'));
    if all_users {
        // Chrome's folder for everyone, where Chrome is.
        let (dir, chrome) = if mac {
            (
                "/Library/Application Support/Google/Chrome/External Extensions",
                "/Applications/Google Chrome.app",
            )
        } else {
            ("/usr/share/google-chrome/extensions", "/opt/google/chrome")
        };
        return if system(chrome).exists() {
            vec![(system(dir), crate::extension::STORE_UPDATE_URL)]
        } else {
            Vec::new()
        };
    }
    let support = home.join("Library/Application Support");
    let mut dirs: Vec<(PathBuf, &'static str)> = Vec::new();
    for b in BROWSERS {
        let data: Vec<PathBuf> = match mac {
            true if b.mac_user_external => b.mac_data.iter().map(|d| support.join(d)).collect(),
            false if b.linux_user_external => {
                b.linux_config.iter().map(|d| config.join(d)).collect()
            }
            _ => continue,
        };
        for d in data.into_iter().filter(|d| d.is_dir()) {
            let d = d.join(USER_EXTERNAL);
            if !dirs.iter().any(|(x, _)| *x == d) {
                dirs.push((d, b.store_update_url));
            }
        }
    }
    dirs
}

/// Whether the host manifest at `manifest` starts `program` (the same file), and it runs.
pub fn names_program(manifest: &Path, program: &Path) -> bool {
    let Some(named) = manifest_program(manifest) else {
        return false;
    };
    let same = match (
        std::fs::canonicalize(&named),
        std::fs::canonicalize(program),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    same && runs(&named)
}

/// A program file that can be run.
fn runs(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    // Windows reads its manifests through the registry, not these folders.
    #[cfg(not(unix))]
    meta.is_file()
}

/// The program the host manifest at `manifest` starts (its `path`; a relative one, which
/// Windows allows, is beside the manifest), when the manifest can be read.
pub fn manifest_program(manifest: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = PathBuf::from(json.get("path")?.as_str()?.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(match manifest.parent() {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path,
    })
}

/// The native host's name, which its manifests and registry entries carry.
pub const HOST_NAME: &str = "com.misterdownloadmanager.host";

/// Windows: the registry keys the browsers look the native host up under (`<key>\HOST_NAME`'s
/// default value: the manifest's path), under HKCU for one user or HKLM for everyone, each with
/// the family whose manifest it names. Chromium-branded builds (Chromium, Helium, Thorium, Brave
/// and Brave Origin with every channel) read Chromium's first, then Chrome's, as most other forks
/// do (Chromium's `launch_context_win.cc`); Brave's own key is kept for builds that read it.
/// Firefox and its forks read Mozilla's.
pub const WINDOWS_HOST_KEYS: [(&str, Family); 5] = [
    (
        r"Software\Google\Chrome\NativeMessagingHosts",
        Family::Chromium,
    ),
    (r"Software\Chromium\NativeMessagingHosts", Family::Chromium),
    (
        r"Software\Microsoft\Edge\NativeMessagingHosts",
        Family::Chromium,
    ),
    (
        r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        Family::Chromium,
    ),
    (r"Software\Mozilla\NativeMessagingHosts", Family::Gecko),
];

/// Windows: the manifest a registry entry's value names (quotes and spaces around it dropped).
pub fn registered_manifest(value: &str) -> PathBuf {
    PathBuf::from(value.trim().trim_matches('"'))
}

/// What a browser finds where it looks for the native host (#818).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// A manifest naming a helper program that runs (this one).
    Alive(PathBuf),
    /// The manifest isn't there (or can't be read): the copy that wrote it is gone, such as one
    /// run from inside a zip, whose temporary folder the archive tool deleted afterwards.
    ManifestGone(PathBuf),
    /// The manifest names a helper program that isn't there or can't be run, or the AppImage's
    /// copy of the helper whose AppImage is gone (this path).
    ProgramGone(PathBuf),
}

impl Holder {
    /// What the manifest at `manifest` leads to.
    pub fn of(manifest: &Path) -> Holder {
        let Some(program) = manifest_program(manifest) else {
            return Holder::ManifestGone(manifest.to_owned());
        };
        if !runs(&program) {
            return Holder::ProgramGone(program);
        }
        // The AppImage's copy of the helper (crates/app/src/unix.rs) starts the AppImage named
        // in `app-path` beside it when MDM isn't running.
        #[cfg(unix)]
        if let Ok(app) = std::fs::read(program.with_file_name("app-path")) {
            use std::os::unix::ffi::OsStrExt;
            let app = Path::new(std::ffi::OsStr::from_bytes(&app));
            if !app.is_file() {
                return Holder::ProgramGone(app.to_owned());
            }
        }
        Holder::Alive(program)
    }

    /// The helper it leads to is gone: no browser can start it.
    pub fn dead(&self) -> bool {
        !matches!(self, Holder::Alive(_))
    }
}

/// What a copy of MDM does at its start with one browser's registry entries on Windows (#818).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repair {
    /// Nothing: they work (whichever copy they name: two copies are both legitimate, and
    /// neither takes the other's), or there are none (the setup's box was unticked).
    Keep,
    /// Point the user's entry at this copy: the one there leads nowhere, or there is none and
    /// the computer's leads nowhere.
    Take,
    /// Remove the user's entry, which leads nowhere: the computer's works, and is read once the
    /// user's is gone.
    Clear,
}

/// The [`Repair`] for one browser's entries, the user's (HKCU) and the computer's (HKLM). The
/// browsers read the user's first, and the computer's only when there is no user's: a user's
/// that leads nowhere hides a computer's that works (Chromium's `launch_context_win.cc`).
pub fn windows_repair(user: Option<&Holder>, machine: Option<&Holder>) -> Repair {
    match (user, machine) {
        (Some(Holder::Alive(_)), _) => Repair::Keep,
        (Some(_), Some(Holder::Alive(_))) => Repair::Clear,
        (Some(_), _) => Repair::Take,
        (None, Some(m)) if m.dead() => Repair::Take,
        (None, _) => Repair::Keep,
    }
}

/// Linux and macOS, at MDM's start: whether `program` writes its manifest where `existing` is
/// (None: there is none yet). One that works naming another copy is left alone; one naming this
/// program is written again (its allowed extensions may have changed), and one that leads
/// nowhere is taken over (#818).
pub fn takes_over(existing: Option<&Holder>, program: &Path) -> bool {
    match existing {
        Some(Holder::Alive(named)) => same_file(named, program),
        _ => true,
    }
}

/// The same file, by its full path.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether `program` runs from a folder that is deleted soon after it quits (#818), where a
/// registration would lead the browsers nowhere once it is gone: under the system's temporary
/// folder (where WinRAR, 7-Zip, Windows Explorer and Linux's archive managers put a program run
/// from inside an archive, and where macOS moves an app run from its download, App
/// Translocation), or in an archive tool's own extraction folder wherever its temporary folder
/// was set.
pub fn in_temporary_folder(program: &Path) -> bool {
    temporary(program, &temporary_folders())
}

/// The system's temporary folders: this process's, the system's own, and the usual ones.
fn temporary_folders() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::temp_dir()];
    for var in ["TMP", "TEMP", "TMPDIR"] {
        if let Some(d) = std::env::var_os(var).filter(|d| !d.is_empty()) {
            dirs.push(PathBuf::from(d));
        }
    }
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            dirs.push(PathBuf::from(root).join("Temp"));
        }
    } else {
        dirs.extend(["/tmp", "/var/tmp"].map(PathBuf::from));
    }
    dirs.retain(|d| d.is_absolute());
    dirs
}

/// [`in_temporary_folder`] with the temporary folders given.
fn temporary(program: &Path, temps: &[PathBuf]) -> bool {
    let program = full_path(program);
    let under = |dir: &PathBuf| {
        let dir = full_path(dir);
        if cfg!(windows) {
            let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
            lower(&program).starts_with(lower(&dir))
        } else {
            program.starts_with(&dir)
        }
    };
    let extracted = program
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .any(|c| archive_folder(&c.as_os_str().to_string_lossy()));
    extracted || temps.iter().any(under)
}

/// A folder an archive tool extracts a program into to run it from inside the archive, and
/// deletes afterwards: WinRAR's `Rar$EXa…` (`Rar$DIa…` for a drag), 7-Zip's `7zO…` / `7zE…`
/// followed by hex digits, Windows Explorer's `Temp1_<name>.zip`.
fn archive_folder(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let seven = lower.strip_prefix("7z").is_some_and(|rest| {
        rest.len() >= 5
            && rest.starts_with(['o', 'e', 's'])
            && rest[1..].chars().all(|c| c.is_ascii_hexdigit())
    });
    let explorer = lower.strip_prefix("temp").is_some_and(|rest| {
        rest.ends_with(".zip")
            && rest
                .split_once('_')
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    });
    lower.starts_with("rar$") || seven || explorer
}

/// `path` with its links and short names resolved (`C:\Users\MEGAPC~1` is `C:\Users\Mega PC`,
/// macOS' `/tmp` is `/private/tmp`), without Windows' `\\?\` before a drive. A path that isn't
/// there (yet) is resolved as far as it is, the rest kept as given, so it compares with a folder
/// that is; as given when none of it can be read.
fn full_path(path: &Path) -> PathBuf {
    let mut there = path;
    let mut rest = Vec::new();
    let full = loop {
        if let Ok(full) = std::fs::canonicalize(there) {
            break rest
                .iter()
                .rev()
                .fold(full, |p: PathBuf, name| p.join(name));
        }
        match (there.parent(), there.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                there = parent;
            }
            _ => break path.to_owned(),
        }
    };
    let text = full.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(home: &Path, mac: bool) -> Vec<(PathBuf, Family)> {
        host_dirs(home, &home.join(".config"), mac, false, None)
    }

    #[test]
    fn nothing_installed_nothing_written() {
        let h = tempfile::tempdir().unwrap();
        assert!(user(h.path(), false).is_empty());
        assert!(user(h.path(), true).is_empty());
    }

    /// #41: a Zen-only user has no ~/.mozilla; Zen reads it all the same.
    #[test]
    fn zen_alone_gets_mozillas_folder() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".zen")).unwrap();
        assert_eq!(
            user(h.path(), false),
            vec![(
                h.path().join(".mozilla/native-messaging-hosts"),
                Family::Gecko
            )]
        );
        let m = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(m.path().join("Library/Application Support/Zen")).unwrap();
        assert_eq!(
            user(m.path(), true),
            vec![(
                m.path()
                    .join("Library/Application Support/Mozilla/NativeMessagingHosts"),
                Family::Gecko
            )]
        );
    }

    /// Firefox with only the newer ~/.config/mozilla still reads ~/.mozilla.
    #[test]
    fn firefox_xdg_alone_gets_mozillas_folder() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".config/mozilla/firefox")).unwrap();
        assert_eq!(
            user(h.path(), false),
            vec![(
                h.path().join(".mozilla/native-messaging-hosts"),
                Family::Gecko
            )]
        );
    }

    /// Ubuntu's Snap Firefox has no ~/.mozilla; the portal it goes through reads it.
    #[test]
    fn snap_firefox_alone_gets_mozillas_folder() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join("snap/firefox/common/.mozilla/firefox")).unwrap();
        assert_eq!(
            user(h.path(), false),
            vec![(
                h.path().join(".mozilla/native-messaging-hosts"),
                Family::Gecko
            )]
        );
    }

    /// #41: Helium reads its own folder (then Chrome's, which a Helium-only user doesn't have).
    #[test]
    fn helium_gets_its_own_folder() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".config/net.imput.helium")).unwrap();
        assert_eq!(
            user(h.path(), false),
            vec![(
                h.path()
                    .join(".config/net.imput.helium/NativeMessagingHosts"),
                Family::Chromium
            )]
        );
        let m = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            m.path()
                .join("Library/Application Support/net.imput.helium"),
        )
        .unwrap();
        assert_eq!(
            user(m.path(), true),
            vec![(
                m.path()
                    .join("Library/Application Support/net.imput.helium/NativeMessagingHosts"),
                Family::Chromium
            )]
        );
    }

    #[test]
    fn several_browsers_each_once() {
        let h = tempfile::tempdir().unwrap();
        for d in [".config/google-chrome", ".mozilla", ".zen", ".librewolf"] {
            std::fs::create_dir_all(h.path().join(d)).unwrap();
        }
        assert_eq!(
            user(h.path(), false),
            vec![
                (
                    h.path().join(".config/google-chrome/NativeMessagingHosts"),
                    Family::Chromium
                ),
                (
                    h.path().join(".mozilla/native-messaging-hosts"),
                    Family::Gecko
                ),
                (
                    h.path().join(".librewolf/native-messaging-hosts"),
                    Family::Gecko
                ),
            ]
        );
    }

    /// #41: Helium installs a chrome.exe; only Chrome's folder makes it Google Chrome.
    #[test]
    fn windows_programs_by_file_and_folder() {
        let named = |p: &str| windows_named(p);
        assert_eq!(
            named(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            Some("Google Chrome")
        );
        assert_eq!(
            named(r"C:\Users\a\AppData\Local\imput\Helium\Application\chrome.exe"),
            Some("Helium")
        );
        assert_eq!(
            named(r"C:\Users\a\AppData\Local\Chromium\Application\chrome.exe"),
            Some("Chromium")
        );
        assert_eq!(
            named(r"D:\Tools\chrome.exe"),
            None,
            "a chrome.exe elsewhere"
        );
        assert_eq!(
            named(r"C:\Program Files\Zen Browser\ZEN.EXE"),
            Some("Zen Browser")
        );
        assert_eq!(named(r"C:\x\unknown.exe"), None);
        for (_, _, name) in WINDOWS_PROGRAMS {
            assert!(super::named(name).is_some(), "{name} is in the table");
        }
    }

    #[test]
    fn programs_from_open_commands() {
        assert_eq!(
            command_program(r#""C:\Program Files\Zen Browser\zen.exe" -osint -url "%1""#)
                .as_deref(),
            Some(r"C:\Program Files\Zen Browser\zen.exe")
        );
        assert_eq!(
            command_program(r"C:\Browsers\x.EXE %1").as_deref(),
            Some(r"C:\Browsers\x.EXE")
        );
        assert_eq!(command_program(r#""""#), None);
        assert_eq!(command_program("rundll32"), None);
        assert!(!windows_can_run_extension(
            r"C:\Program Files\Internet Explorer\iexplore.exe"
        ));
        assert!(windows_can_run_extension(r"C:\x\zen.exe"));
    }

    #[test]
    fn names_are_unique_and_found() {
        for b in BROWSERS {
            assert_eq!(named(b.name).map(|x| x.name), Some(b.name));
        }
        assert!(named("Netscape").is_none());
    }

    /// #253: Brave Origin keeps its own folder on Linux (`Brave-Origin`, beside Brave's
    /// `Brave-Browser`) and reads its manifest there; a user with both gets both.
    #[test]
    fn brave_origin_gets_its_own_folder() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".config/BraveSoftware/Brave-Origin")).unwrap();
        assert_eq!(
            user(h.path(), false),
            vec![(
                h.path()
                    .join(".config/BraveSoftware/Brave-Origin/NativeMessagingHosts"),
                Family::Chromium
            )]
        );
        std::fs::create_dir_all(h.path().join(".config/BraveSoftware/Brave-Browser")).unwrap();
        assert_eq!(user(h.path(), false).len(), 2);
    }

    /// Brave, Brave Origin and their channels read Chrome's folder on macOS, even with no Chrome.
    #[test]
    fn brave_on_macos_gets_chromes_folder() {
        for folder in ["BraveSoftware/Brave-Browser", "BraveSoftware/Brave-Origin"] {
            let m = tempfile::tempdir().unwrap();
            let support = m.path().join("Library/Application Support");
            std::fs::create_dir_all(support.join(folder)).unwrap();
            assert_eq!(
                user(m.path(), true),
                vec![(
                    support.join("Google/Chrome/NativeMessagingHosts"),
                    Family::Chromium
                )],
                "{folder}"
            );
        }
    }

    /// Every folder the table lists, alone in a home folder, gets the manifest where that
    /// browser reads it: its own `NativeMessagingHosts` (Chromium's family; on macOS the one it
    /// reads instead, when the table says so), or Mozilla's (Firefox's family).
    #[test]
    fn every_listed_folder_is_covered() {
        for b in BROWSERS {
            for mac in [false, true] {
                let folders: Vec<&str> = if mac {
                    b.mac_data.to_vec()
                } else {
                    b.linux_config.iter().chain(b.linux_home).copied().collect()
                };
                for folder in folders {
                    let h = tempfile::tempdir().unwrap();
                    let home = h.path();
                    let support = home.join("Library/Application Support");
                    let data = if mac {
                        support.join(folder)
                    } else if b.linux_home.contains(&folder) {
                        home.join(folder)
                    } else {
                        home.join(".config").join(folder)
                    };
                    std::fs::create_dir_all(&data).unwrap();
                    let want = match (b.family, mac) {
                        (Family::Chromium, true) => match b.mac_hosts {
                            Some(hosts) => support.join(hosts).join("NativeMessagingHosts"),
                            None => data.join("NativeMessagingHosts"),
                        },
                        (Family::Chromium, false) => data.join("NativeMessagingHosts"),
                        (Family::Gecko, true) => support.join("Mozilla/NativeMessagingHosts"),
                        (Family::Gecko, false) => home.join(".mozilla/native-messaging-hosts"),
                    };
                    let got = user(home, mac);
                    assert!(
                        got.contains(&(want.clone(), b.family)),
                        "{} ({folder}, mac {mac}): {got:?} lacks {want:?}",
                        b.name
                    );
                }
            }
        }
    }

    /// #266: a browser that reads the package's manifest for everyone, naming this helper, gets
    /// no user folder; a manifest naming another program, or one that doesn't run, doesn't count.
    /// Mozilla's user folder stays while a fork that reads only it is there.
    #[cfg(unix)]
    #[test]
    fn a_manifest_for_everyone_covers_its_browsers() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let (root, home) = (t.path().join("root"), t.path().join("home"));
        for d in [
            ".config/google-chrome",
            ".config/BraveSoftware/Brave-Browser",
            ".config/opera",
            ".mozilla",
        ] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let helper = root.join("usr/bin/misterdownloadmanager-nmhost");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let system = |d: &str, program: &Path| {
            let dir = root.join(d.trim_start_matches('/'));
            std::fs::create_dir_all(&dir).unwrap();
            let json = serde_json::json!({ "name": "x", "path": program, "type": "stdio" });
            std::fs::write(dir.join(HOST_MANIFEST), json.to_string()).unwrap();
        };
        let dirs = |host: Option<&Path>| -> Vec<PathBuf> {
            host_dirs_in(&root, &home, &home.join(".config"), false, false, host)
                .into_iter()
                .map(|(d, _)| d.strip_prefix(&home).unwrap().to_owned())
                .collect()
        };
        let all = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
        let chrome = ".config/google-chrome/NativeMessagingHosts";
        let brave = ".config/BraveSoftware/Brave-Browser/NativeMessagingHosts";
        let opera = ".config/opera/NativeMessagingHosts";
        let mozilla = ".mozilla/native-messaging-hosts";
        assert_eq!(dirs(Some(&helper)), all(&[chrome, brave, opera, mozilla]));

        // Chrome's for everyone: Chrome and Brave read it, Opera doesn't.
        system(ETC_CHROME, &helper);
        assert_eq!(dirs(Some(&helper)), all(&[opera, mozilla]));
        // Unregistering (no helper given) still sees every user folder.
        assert_eq!(dirs(None), all(&[chrome, brave, opera, mozilla]));
        // Firefox's, in lib64 (Fedora's).
        system(MOZILLA[1], &helper);
        assert_eq!(dirs(Some(&helper)), all(&[opera]));
        // Zen reads only Mozilla's user folder: it stays.
        std::fs::create_dir_all(home.join(".zen")).unwrap();
        assert_eq!(dirs(Some(&helper)), all(&[opera, mozilla]));
        std::fs::remove_dir(home.join(".zen")).unwrap();

        // Another helper (an AppImage's copy) isn't covered by the package's.
        let other = t.path().join("copy/misterdownloadmanager-nmhost");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::copy(&helper, &other).unwrap();
        assert_eq!(dirs(Some(&other)), all(&[chrome, brave, opera, mozilla]));
        // A manifest naming a helper that can't run covers nothing.
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(dirs(Some(&helper)), all(&[chrome, brave, opera, mozilla]));
    }

    /// #685: Helium alone, on Linux, reads the package's manifest for everyone in Chromium's
    /// folder (or Chrome's): no file of MDM's left in its own folder in the home.
    #[cfg(unix)]
    #[test]
    fn a_manifest_for_everyone_covers_helium() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let (root, home) = (t.path().join("root"), t.path().join("home"));
        std::fs::create_dir_all(home.join(".config/net.imput.helium")).unwrap();
        let helper = root.join("usr/bin/misterdownloadmanager-nmhost");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = || -> Vec<PathBuf> {
            host_dirs_in(
                &root,
                &home,
                &home.join(".config"),
                false,
                false,
                Some(&helper),
            )
            .into_iter()
            .map(|(d, _)| d.strip_prefix(&home).unwrap().to_owned())
            .collect()
        };
        let own = PathBuf::from(".config/net.imput.helium/NativeMessagingHosts");
        assert_eq!(dirs(), std::slice::from_ref(&own));
        for etc in [ETC_CHROMIUM, ETC_CHROME] {
            let dir = root.join(etc.trim_start_matches('/'));
            std::fs::create_dir_all(&dir).unwrap();
            let json = serde_json::json!({ "name": "x", "path": helper, "type": "stdio" });
            std::fs::write(dir.join(HOST_MANIFEST), json.to_string()).unwrap();
            assert!(dirs().is_empty(), "{etc}");
            std::fs::remove_file(dir.join(HOST_MANIFEST)).unwrap();
            assert_eq!(dirs(), std::slice::from_ref(&own));
        }
    }

    /// #685: the external extension folders, each with the store's address as its browser names
    /// it. One user: Chrome's only on macOS, Helium's own data folders (each channel's) on Linux
    /// and macOS, only those there, by Helium's own name for the store. Everyone: Chrome's, for
    /// Chrome only (Helium would read macOS's before its own).
    #[test]
    fn external_extension_folders() {
        use crate::extension::{HELIUM_STORE_UPDATE_URL as HELIUM, STORE_UPDATE_URL as GOOGLE};
        let t = tempfile::tempdir().unwrap();
        let (root, home) = (t.path().join("root"), t.path().join("home"));
        let config = home.join(".config");
        let user = |mac| external_dirs_in(&root, &home, &config, mac, false);
        let everyone = |mac| external_dirs_in(&root, &home, &config, mac, true);
        assert!(user(false).is_empty() && user(true).is_empty());
        assert!(everyone(false).is_empty() && everyone(true).is_empty());

        // Linux: Chrome reads none in the home folder; Helium its own.
        std::fs::create_dir_all(config.join("google-chrome")).unwrap();
        assert!(user(false).is_empty());
        std::fs::create_dir_all(config.join("net.imput.helium.dev")).unwrap();
        std::fs::create_dir_all(config.join("net.imput.helium")).unwrap();
        assert_eq!(
            user(false),
            [
                (config.join("net.imput.helium/External Extensions"), HELIUM),
                (
                    config.join("net.imput.helium.dev/External Extensions"),
                    HELIUM
                ),
            ]
        );

        // macOS: Chrome's and Helium's, each by its own name for the store.
        let support = home.join("Library/Application Support");
        std::fs::create_dir_all(support.join("net.imput.helium")).unwrap();
        assert_eq!(
            user(true),
            [(support.join("net.imput.helium/External Extensions"), HELIUM)]
        );
        std::fs::create_dir_all(support.join("Google/Chrome")).unwrap();
        assert_eq!(
            user(true),
            [
                (support.join("Google/Chrome/External Extensions"), GOOGLE),
                (support.join("net.imput.helium/External Extensions"), HELIUM),
            ]
        );

        // Everyone's: Chrome's, only where Chrome is; never for Helium alone.
        std::fs::create_dir_all(root.join("Applications/Helium.app")).unwrap();
        std::fs::create_dir_all(root.join("opt/helium")).unwrap();
        assert!(everyone(true).is_empty() && everyone(false).is_empty());
        std::fs::create_dir_all(root.join("Applications/Google Chrome.app")).unwrap();
        std::fs::create_dir_all(root.join("opt/google/chrome")).unwrap();
        assert_eq!(
            everyone(true),
            [(
                root.join("Library/Application Support/Google/Chrome/External Extensions"),
                GOOGLE
            )]
        );
        assert_eq!(
            everyone(false),
            [(root.join("usr/share/google-chrome/extensions"), GOOGLE)]
        );
    }

    /// The folders for everyone each browser reads are in the list the packages ship.
    #[test]
    fn every_browsers_system_folder_is_shipped() {
        for b in BROWSERS {
            for d in b.linux_system {
                assert!(
                    LINUX_SYSTEM_HOST_DIRS
                        .iter()
                        .any(|(x, f)| x == d && *f == b.family),
                    "{} reads {d}",
                    b.name
                );
            }
        }
    }

    #[test]
    fn a_manifests_program() {
        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("host.json");
        let abs = d.path().join("elsewhere").join("host");
        let json = serde_json::json!({ "name": "x", "path": abs, "type": "stdio" });
        std::fs::write(&m, json.to_string()).unwrap();
        assert_eq!(manifest_program(&m), Some(abs));
        std::fs::write(&m, r#"{"name":"x","path":"host.exe"}"#).unwrap();
        assert_eq!(manifest_program(&m), Some(d.path().join("host.exe")));
        for bad in [r#"{"name":"x"}"#, r#"{"path":""}"#, "not json"] {
            std::fs::write(&m, bad).unwrap();
            assert_eq!(manifest_program(&m), None, "{bad}");
        }
        assert_eq!(manifest_program(&d.path().join("none.json")), None);
    }

    /// #818: a copy run from inside an archive, or from anywhere under the temporary folder, is
    /// in a folder that is deleted later; a copy in a folder of its own isn't.
    #[test]
    fn temporary_folders_are_found() {
        let names = [
            ("Rar$EXa34720.17029.rartemp", true),
            ("Rar$DIa1234.5678", true),
            ("7zO8A3B1C2D", true),
            ("7zE0F12AB34", true),
            ("Temp1_MisterDownloadManager-0.5.2-x64.zip", true),
            ("Temp12_x.zip", true),
            ("7-Zip", false),
            ("7zip", false),
            ("7zOops", false),
            ("Templates", false),
            ("Temp_x.zip", false),
            ("MisterDownloadManager-0.5.2-x64", false),
            ("Rar", false),
        ];
        for (name, archive) in names {
            assert_eq!(archive_folder(name), archive, "{name}");
        }
        let (temp, home) = if cfg!(windows) {
            (r"C:\Users\x\AppData\Local\Temp", r"C:\Users\x")
        } else {
            ("/tmp", "/home/x")
        };
        let temps = [PathBuf::from(temp)];
        let at = |p: &str| Path::new(p).join("misterdownloadmanager.exe");
        let rar = format!(r"{temp}/Rar$EXa34720.17029.rartemp/MisterDownloadManager-0.5.2-x64");
        assert!(temporary(&at(&rar), &temps));
        assert!(temporary(&at(&format!("{temp}/mdm")), &temps));
        assert!(temporary(&at(temp), &temps));
        // An archive tool's folder outside the temporary folder (WinRAR's own setting).
        assert!(temporary(&at(&format!("{home}/rt/Rar$EXa1.2")), &temps));
        assert!(!temporary(&at(&format!("{home}/MDM")), &temps));
        assert!(!temporary(&at(&format!("{home}/Temporary/MDM")), &temps));
        assert!(!temporary(&at(&format!("{temp}x/MDM")), &temps));
        if cfg!(windows) {
            let upper = r"C:\USERS\X\APPDATA\LOCAL\TEMP\mdm";
            assert!(temporary(&at(upper), &temps), "Windows' paths ignore case");
        }
        // The folder itself, read through its links and short names, as the system gives it.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("copy")).unwrap();
        let exe = d.path().join("copy").join("misterdownloadmanager.exe");
        std::fs::write(&exe, b"").unwrap();
        assert!(in_temporary_folder(&exe));
        // A temporary folder reached through a link (macOS' /tmp is /private/tmp), with a path
        // under it that isn't there.
        #[cfg(unix)]
        {
            let real = d.path().join("real");
            std::fs::create_dir(&real).unwrap();
            let link = d.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let gone = |dir: &Path| at(&dir.join("gone").to_string_lossy());
            assert!(temporary(&gone(&link), std::slice::from_ref(&link)));
            assert!(temporary(&gone(&link), std::slice::from_ref(&real)));
            assert!(temporary(&gone(&real), &[link]));
        }
    }

    fn manifest_naming(dir: &Path, program: &Path) -> PathBuf {
        let m = dir.join(HOST_MANIFEST);
        let json = serde_json::json!({ "name": HOST_NAME, "path": program, "type": "stdio" });
        std::fs::write(&m, json.to_string()).unwrap();
        m
    }

    fn program(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// #818: what a registration leads to: a helper that runs, or nothing (the manifest gone
    /// with its temporary folder, or the helper it names gone).
    #[test]
    fn a_registration_leads_to_a_helper_or_nowhere() {
        let d = tempfile::tempdir().unwrap();
        let helper = d.path().join("mdm").join("misterdownloadmanager-nmhost");
        program(&helper);
        let m = manifest_naming(&d.path().join("mdm"), &helper);
        assert_eq!(Holder::of(&m), Holder::Alive(helper.clone()));
        assert!(!Holder::of(&m).dead());
        let gone = d.path().join("Rar$EXa1.2").join(HOST_MANIFEST);
        assert_eq!(Holder::of(&gone), Holder::ManifestGone(gone.clone()));
        let missing = d.path().join("old").join("misterdownloadmanager-nmhost");
        let m = manifest_naming(d.path(), &missing);
        assert_eq!(Holder::of(&m), Holder::ProgramGone(missing));
        assert!(Holder::of(&m).dead());
        // The AppImage's copy of the helper, whose AppImage was deleted.
        #[cfg(unix)]
        {
            let copy = d
                .path()
                .join("browser-host")
                .join("misterdownloadmanager-nmhost");
            program(&copy);
            let image = d.path().join("MDM.AppImage");
            program(&image);
            std::fs::write(
                copy.with_file_name("app-path"),
                image.as_os_str().as_encoded_bytes(),
            )
            .unwrap();
            let m = manifest_naming(&d.path().join("browser-host"), &copy);
            assert_eq!(Holder::of(&m), Holder::Alive(copy.clone()));
            std::fs::remove_file(&image).unwrap();
            assert_eq!(Holder::of(&m), Holder::ProgramGone(image));
        }
    }

    /// #818: Windows, at a start: only an entry that leads nowhere is repaired; one that works is
    /// never taken, whichever copy it names; a user's that leads nowhere is removed when the
    /// computer's works (the browsers read the user's first).
    #[test]
    fn only_a_dead_registration_is_repaired() {
        let alive = Holder::Alive(PathBuf::from(r"C:\Other\misterdownloadmanager-nmhost.exe"));
        let gone = Holder::ManifestGone(PathBuf::from(r"C:\Temp\Rar$EXa1\x.json"));
        let helper = Holder::ProgramGone(PathBuf::from(r"C:\Old\misterdownloadmanager-nmhost.exe"));
        use Repair::*;
        for (user, machine, repair) in [
            (None, None, Keep),
            (Some(&alive), None, Keep),
            (Some(&alive), Some(&gone), Keep),
            (Some(&alive), Some(&alive), Keep),
            (None, Some(&alive), Keep),
            (Some(&gone), None, Take),
            (Some(&helper), None, Take),
            (Some(&gone), Some(&helper), Take),
            (None, Some(&gone), Take),
            (Some(&gone), Some(&alive), Clear),
            (Some(&helper), Some(&alive), Clear),
        ] {
            assert_eq!(
                windows_repair(user, machine),
                repair,
                "{user:?} {machine:?}"
            );
        }
    }

    /// #818: Linux and macOS, at a start: a manifest that works naming another copy stays; this
    /// copy's own is written again, and one that leads nowhere is taken over.
    #[test]
    fn a_start_takes_over_only_what_leads_nowhere() {
        let d = tempfile::tempdir().unwrap();
        let me = d.path().join("me").join("misterdownloadmanager-nmhost");
        let other = d.path().join("other").join("misterdownloadmanager-nmhost");
        program(&me);
        program(&other);
        assert!(takes_over(None, &me));
        assert!(takes_over(Some(&Holder::Alive(me.clone())), &me));
        assert!(!takes_over(Some(&Holder::Alive(other.clone())), &me));
        assert!(takes_over(Some(&Holder::ProgramGone(other)), &me));
        assert!(takes_over(
            Some(&Holder::ManifestGone(d.path().join("x.json"))),
            &me
        ));
    }
}
