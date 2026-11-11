//! The User-Agent MDM sends for the links it is given by hand (#668): the one the default browser
//! of this computer sends today, at the version installed, so a site sees the browser its user
//! has rather than a program or a browser from years ago. The app finds that browser once, off
//! the interface's thread (win32.rs, unix.rs), and hands it here (`set_automatic`); until then,
//! or when none is found, MDM speaks as a current Chrome on this system (`fallback`).
//!
//! A User-Agent typed in Options, or the one a browser hands over with its download (the
//! extension's), always wins over this one: the download's `Options::automatic_agent` says which.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// MDM's User-Agent before #668: Internet Explorer 11's, as IDM 6.42 sends it (lab S1, D-007).
/// Still asked with once when a site turns a browser's User-Agent away on MDM's connection
/// (#372: a bot check that sees a browser's name on a connection that isn't a browser's lets
/// this one through), and a setting saved with it means "automatic" (it was the default).
pub const IE11_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 6.1; Trident/7.0; rv:11.0) like Gecko";

/// A browser whose User-Agent MDM knows how to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Vivaldi,
    Opera,
    /// Chromium itself and the Chromium browsers that send its User-Agent unchanged.
    Chromium,
    Firefox,
    /// LibreWolf counts as Firefox does; Waterfox, Floorp and Zen count their own versions but
    /// send Firefox's User-Agent (at the Firefox they are built on: MDM's estimate of today's).
    FirefoxFork,
    Safari,
}

impl Browser {
    /// The name people know it by (a product name: not translated).
    pub fn name(self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave",
            Browser::Vivaldi => "Vivaldi",
            Browser::Opera => "Opera",
            Browser::Chromium => "Chromium",
            Browser::Firefox | Browser::FirefoxFork => "Firefox",
            Browser::Safari => "Safari",
        }
    }

    /// The browser `name` is: an executable's file name without its extension (`msedge`), a
    /// `.desktop` file's ID (`com.google.Chrome.desktop`, `firefox_firefox.desktop`), a program
    /// (`google-chrome-stable`) or a macOS bundle ID (`com.brave.Browser`); case ignored. None
    /// for anything else (an unknown browser: MDM then speaks as Chrome).
    pub fn named(name: &str) -> Option<Browser> {
        let n = name.to_ascii_lowercase();
        let has = |w: &str| n.contains(w);
        // The order matters: "chromium" holds "chrom", Opera's ID "operasoftware".
        Some(
            if has("msedge") || has("microsoft-edge") || has("edgemac") || n == "edge" {
                Browser::Edge
            } else if has("brave") {
                Browser::Brave
            } else if has("vivaldi") {
                Browser::Vivaldi
            } else if has("opera") {
                Browser::Opera
            } else if has("chromium") {
                Browser::Chromium
            } else if has("chrome") {
                Browser::Chrome
            } else if has("librewolf") || has("firefox") || has("iceweasel") {
                Browser::Firefox
            } else if has("waterfox")
                || has("floorp")
                || has("zen_browser")
                || has("zen-browser")
                || n == "zen"
                || n == "zen.desktop"
            {
                Browser::FirefoxFork
            } else if has("safari") {
                Browser::Safari
            } else {
                return None;
            },
        )
    }

    fn chromium_based(self) -> bool {
        !matches!(
            self,
            Browser::Firefox | Browser::FirefoxFork | Browser::Safari
        )
    }
}

/// The system a User-Agent names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    /// This computer's.
    pub fn this() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_vendor = "apple") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// This computer's processor as Firefox names it on Linux (`uname -m`'s word).
pub fn this_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "i686",
        a => a,
    }
}

/// The default browser found on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub browser: Browser,
    /// Its version as installed ("157.0.8103.39", Brave's "157.1.99.55", Opera's "122.0.5643.24",
    /// Firefox's "160.0", Safari's "26.1"); None when it couldn't be read (MDM's estimate then).
    pub version: Option<String>,
    /// Firefox built by Ubuntu (its snap) names Ubuntu in its platform: "X11; Ubuntu; Linux …".
    pub ubuntu: bool,
}

impl Found {
    /// "Brave 157", "Firefox 160", "Safari 26.1": the browser as Options shows it.
    pub fn label(&self) -> String {
        match (self.browser, self.version.as_deref()) {
            (Browser::Safari, Some(v)) => format!("Safari {}", safari_version(v)),
            (Browser::FirefoxFork, _) | (_, None) => self.browser.name().to_owned(),
            (b, Some(v)) => match leading_number(v) {
                Some(major) => format!("{} {major}", b.name()),
                None => b.name().to_owned(),
            },
        }
    }
}

/// The day Chrome 157 became stable (2026-11-10), and Chrome's and Firefox's rhythm of a new
/// major version every 4 weeks: `chrome_major` counts from it.
const CHROME_KNOWN: (u32, u64) = (157, 1_794_268_800);
/// Firefox 160, the same day.
const FIREFOX_KNOWN: (u32, u64) = (160, 1_794_268_800);
/// Safari 26 (2025-09-15), and Safari's one major version a year.
const SAFARI_KNOWN: (u32, u64) = (26, 1_757_894_400);
const FOUR_WEEKS: u64 = 28 * 24 * 3600;
const A_YEAR: u64 = 365 * 24 * 3600;

/// A browser's current stable major version at `now` (seconds since 1970), counted from one
/// known release in its rhythm: what MDM says when it can't read the installed version. Never
/// before the known one, never more than two years past it (a clock far off).
fn estimate(known: (u32, u64), every: u64, now: u64) -> u32 {
    let steps = now.saturating_sub(known.1) / every;
    known.0 + steps.min(2 * A_YEAR / every) as u32
}

/// Chrome's current stable major version (`estimate`).
pub fn chrome_major(now: u64) -> u32 {
    estimate(CHROME_KNOWN, FOUR_WEEKS, now)
}

fn firefox_major(now: u64) -> u32 {
    estimate(FIREFOX_KNOWN, FOUR_WEEKS, now)
}

/// The number `s` starts with ("157" of "157.0.8103.39", "140" of "140.4.0esr").
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|n| *n > 0)
}

/// Safari's `Version/` word: its major and minor ("18.6" of "18.6.2", "26.0" of "26").
fn safari_version(v: &str) -> String {
    let mut parts = v.trim().split('.').map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        (Some(major), None) => format!("{major}.0"),
        _ => "26.0".into(),
    }
}

/// The User-Agent `found` sends on `os` (`arch`: this processor, `this_arch`), at `now` (seconds
/// since 1970, for the versions MDM estimates): the reduced User-Agents browsers send today,
/// whose minor version numbers are zeros. Nothing found, or a browser MDM doesn't know: a
/// current Chrome's on this system.
pub fn user_agent(found: Option<&Found>, os: Os, arch: &str, now: u64) -> String {
    let browser = found.map_or(Browser::Chrome, |f| f.browser);
    let version = found.and_then(|f| f.version.as_deref());
    let major = version.and_then(leading_number);
    if browser.chromium_based() {
        let platform = match os {
            Os::Windows => "Windows NT 10.0; Win64; x64",
            Os::Mac => "Macintosh; Intel Mac OS X 10_15_7",
            // Chrome says x86_64 on every Linux since its User-Agent was reduced.
            Os::Linux => "X11; Linux x86_64",
        };
        // Opera and Vivaldi count their own versions, not their Chromium's.
        let chrome = match browser {
            Browser::Opera | Browser::Vivaldi => None,
            _ => major,
        }
        .unwrap_or_else(|| chrome_major(now));
        let base = format!(
            "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/{chrome}.0.0.0 Safari/537.36"
        );
        return match (browser, major) {
            (Browser::Edge, _) => format!("{base} Edg/{chrome}.0.0.0"),
            (Browser::Opera, Some(opera)) => format!("{base} OPR/{opera}.0.0.0"),
            _ => base,
        };
    }
    if browser == Browser::Safari {
        let version = version.map_or_else(
            || format!("{}.0", estimate(SAFARI_KNOWN, A_YEAR, now)),
            safari_version,
        );
        return format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/{version} Safari/605.1.15"
        );
    }
    let firefox = match browser {
        Browser::FirefoxFork => None,
        _ => major,
    }
    .unwrap_or_else(|| firefox_major(now));
    let platform = match os {
        Os::Windows => "Windows NT 10.0; Win64; x64".to_owned(),
        Os::Mac => "Macintosh; Intel Mac OS X 10.15".to_owned(),
        Os::Linux if found.is_some_and(|f| f.ubuntu) => format!("X11; Ubuntu; Linux {arch}"),
        Os::Linux => format!("X11; Linux {arch}"),
    };
    format!("Mozilla/5.0 ({platform}; rv:{firefox}.0) Gecko/20100101 Firefox/{firefox}.0")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What the app found (`set_automatic`): the browser's label and its User-Agent. None: not
/// looked for yet.
static AUTOMATIC: RwLock<Option<(Option<String>, String)>> = RwLock::new(None);

/// The default browser the app found (None: none, or one MDM doesn't know): from now on the
/// automatic User-Agent is the one it sends.
pub fn set_automatic(found: Option<Found>) {
    let agent = user_agent(found.as_ref(), Os::this(), this_arch(), now());
    *AUTOMATIC.write().unwrap() = Some((found.map(|f| f.label()), agent));
}

/// The User-Agent MDM sends for a link given by hand when the user set none: the default
/// browser's (`set_automatic`), else a current Chrome's on this system (`fallback`).
pub fn automatic() -> String {
    match &*AUTOMATIC.read().unwrap() {
        Some((_, agent)) => agent.clone(),
        None => fallback(),
    }
}

/// The browser `automatic` speaks as ("Brave 157"); None until it is found, or when none was.
pub fn automatic_browser() -> Option<String> {
    AUTOMATIC
        .read()
        .unwrap()
        .as_ref()
        .and_then(|(b, _)| b.clone())
}

/// Whether the default browser has been looked for (`set_automatic`).
pub fn looked_for() -> bool {
    AUTOMATIC.read().unwrap().is_some()
}

/// A current Chrome's User-Agent on this system: when no browser is found.
pub fn fallback() -> String {
    user_agent(None, Os::this(), this_arch(), now())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-11, the day these were written.
    const TODAY: u64 = 1_794_355_200;

    fn found(browser: Browser, version: Option<&str>) -> Found {
        Found {
            browser,
            version: version.map(str::to_owned),
            ubuntu: false,
        }
    }

    fn ua(browser: Browser, version: Option<&str>, os: Os) -> String {
        user_agent(Some(&found(browser, version)), os, "x86_64", TODAY)
    }

    #[test]
    fn chromium_browsers_send_the_reduced_user_agent() {
        use Browser::*;
        use Os::*;
        assert_eq!(
            ua(Chrome, Some("157.0.8103.39"), Windows),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/157.0.0.0 Safari/537.36"
        );
        assert_eq!(
            ua(Chrome, Some("157.0.8103.38"), Linux),
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/157.0.0.0 Safari/537.36"
        );
        assert_eq!(
            ua(Chrome, Some("157.0.8103.39"), Mac),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like \
             Gecko) Chrome/157.0.0.0 Safari/537.36"
        );
        // Brave's version is its Chromium's major, then its own; it sends Chrome's agent.
        assert_eq!(
            ua(Brave, Some("157.1.99.55"), Windows),
            ua(Chrome, Some("157.0.8103.39"), Windows)
        );
        assert_eq!(
            ua(Chromium, Some("156.0.7000.1"), Linux),
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/156.0.0.0 Safari/537.36"
        );
        assert_eq!(
            ua(Edge, Some("157.0.4341.42"), Windows),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0"
        );
        assert_eq!(
            ua(Edge, Some("157.0.4341.42"), Mac),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like \
             Gecko) Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0"
        );
        // Opera counts its own version: OPR/ is it, Chrome/ today's Chromium (estimated).
        assert_eq!(
            ua(Opera, Some("122.0.5643.24"), Windows),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/157.0.0.0 Safari/537.36 OPR/122.0.0.0"
        );
        // Opera without a version, and Vivaldi: Chrome's.
        assert_eq!(ua(Opera, None, Linux), ua(Chrome, Some("157.0.1.2"), Linux));
        assert_eq!(
            ua(Vivaldi, Some("7.6.3797.52"), Windows),
            ua(Chrome, Some("157.0.1.2"), Windows)
        );
    }

    #[test]
    fn firefox_and_safari_send_their_own() {
        use Browser::*;
        use Os::*;
        assert_eq!(
            ua(Firefox, Some("160.0"), Windows),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:160.0) Gecko/20100101 Firefox/160.0"
        );
        assert_eq!(
            ua(Firefox, Some("140.4.0esr"), Mac),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:140.0) Gecko/20100101 Firefox/140.0"
        );
        assert_eq!(
            ua(Firefox, Some("155.0.1"), Linux),
            "Mozilla/5.0 (X11; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0"
        );
        // Ubuntu's snap, as it was seen sending it (Ubuntu 24.04, Firefox 155.0.1).
        let snap = Found {
            ubuntu: true,
            ..found(Firefox, Some("155.0.1"))
        };
        assert_eq!(
            user_agent(Some(&snap), Linux, "x86_64", TODAY),
            "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0"
        );
        assert_eq!(
            user_agent(
                Some(&found(Firefox, Some("160.0"))),
                Linux,
                "aarch64",
                TODAY
            ),
            "Mozilla/5.0 (X11; Linux aarch64; rv:160.0) Gecko/20100101 Firefox/160.0"
        );
        // A fork counts its own version: Firefox's of today instead.
        assert_eq!(
            ua(FirefoxFork, Some("6.6.4"), Windows),
            ua(Firefox, Some("160.0"), Windows)
        );
        assert_eq!(
            ua(Safari, Some("26.1"), Mac),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like \
             Gecko) Version/26.1 Safari/605.1.15"
        );
        assert!(ua(Safari, Some("18.6.2"), Mac).contains(" Version/18.6 Safari/"));
        assert!(ua(Safari, None, Mac).contains(" Version/27.0 Safari/"));
    }

    #[test]
    fn nothing_found_is_a_current_chrome_here() {
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            assert_eq!(
                user_agent(None, os, "x86_64", TODAY),
                ua(Browser::Chrome, Some("157.0.0.0"), os)
            );
            // A browser without a version: today's.
            assert_eq!(
                ua(Browser::Edge, None, os),
                ua(Browser::Edge, Some("157.0.0.0"), os)
            );
        }
        assert!(ua(Browser::Firefox, None, Os::Windows).contains("Firefox/160.0"));
        // Chrome's rhythm, never before the known release nor far past it.
        assert_eq!(chrome_major(0), 157);
        assert_eq!(chrome_major(CHROME_KNOWN.1 + 28 * 86_400), 158);
        assert_eq!(chrome_major(CHROME_KNOWN.1 + 100 * A_YEAR), 157 + 26);
    }

    #[test]
    fn browsers_by_their_programs_names() {
        use Browser::*;
        for (name, b) in [
            ("chrome", Chrome),
            ("msedge", Edge),
            ("brave", Brave),
            ("vivaldi", Vivaldi),
            ("opera", Opera),
            ("firefox", Firefox),
            ("librewolf", Firefox),
            ("waterfox", FirefoxFork),
            ("zen", FirefoxFork),
            ("com.google.Chrome.desktop", Chrome),
            ("google-chrome-stable", Chrome),
            ("chromium_chromium.desktop", Chromium),
            ("firefox_firefox.desktop", Firefox),
            ("org.mozilla.firefox.desktop", Firefox),
            ("microsoft-edge.desktop", Edge),
            ("brave-browser.desktop", Brave),
            ("com.google.chrome", Chrome),
            ("com.microsoft.edgemac", Edge),
            ("com.brave.Browser", Brave),
            ("com.operasoftware.Opera", Opera),
            ("org.mozilla.firefox", Firefox),
            ("com.apple.Safari", Safari),
        ] {
            assert_eq!(Browser::named(name), Some(b), "{name}");
        }
        for other in ["iexplore", "thunderbird", "org.gnome.Epiphany.desktop", ""] {
            assert_eq!(Browser::named(other), None, "{other}");
        }
    }

    #[test]
    fn labels_name_the_browser_and_its_major_version() {
        assert_eq!(
            found(Browser::Brave, Some("157.1.99.55")).label(),
            "Brave 157"
        );
        assert_eq!(
            found(Browser::Opera, Some("122.0.1.2")).label(),
            "Opera 122"
        );
        assert_eq!(found(Browser::Safari, Some("26.1")).label(), "Safari 26.1");
        assert_eq!(found(Browser::Firefox, None).label(), "Firefox");
    }
}
