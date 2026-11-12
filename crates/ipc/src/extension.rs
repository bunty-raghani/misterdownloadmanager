//! The browser extension's identity and whether a browser has it, shared by the native messaging
//! host (which serves it and notes each browser that starts it) and the app (which offers to
//! install it until one has).

/// The extension's ID loaded unpacked (the public key in extension/manifest.json).
pub const UNPACKED_ID: &str = "oogbclbbninliongijffeeggalppliok";
/// Its ID in the Chrome Web Store, which signs it with the store's own key
/// (tools/extension/pack.ps1 leaves the manifest's key out of the store's copy). [`store_id`] is
/// `None` while this is not a valid ID (a placeholder), and then nothing names it.
pub const STORE_ID: &str = "ejoleppdfgjdndddcjoehopknmlglojp";
/// Its IDs in Microsoft Edge Add-ons and Opera add-ons, which give their own; placeholders until
/// listed there, never served then (`valid_id`).
pub const OTHER_STORE_IDS: [&str; 2] = ["EDGE_ADDONS_ID_PENDING", "OPERA_ADDONS_ID_PENDING"];
/// The Firefox add-on's page on addons.mozilla.org (listed: Firefox installs and updates it from
/// there).
pub const FIREFOX_PAGE: &str = "https://addons.mozilla.org/firefox/addon/mister-download-manager/";
/// The Firefox add-on's ID (extension/manifest.firefox.json).
pub const FIREFOX_ID: &str = "integration@misterdownloadmanager.com";
/// Chrome's update service: an external-install entry naming it makes Chrome offer the store's
/// extension at its next start (developer.chrome.com, "Alternative extension distribution
/// options").
pub const STORE_UPDATE_URL: &str = "https://clients2.google.com/service/update2/crx";
/// Helium's name for the Chrome Web Store's update service (its proxy-extension-downloads.patch,
/// `kChromeWebstoreUpdateURL`): an entry naming it goes through Helium's own services, while
/// Google's address, asked directly without the browser's version, answers "no update" (#685).
pub const HELIUM_STORE_UPDATE_URL: &str = "https://update-url-to-be-replaced.qjz9zk";

/// A Chromium extension ID: 32 letters from a to p.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The store's ID, once it is a real one.
pub fn store_id() -> Option<&'static str> {
    valid_id(STORE_ID).then_some(STORE_ID)
}

/// The Chromium extensions the native host serves: unpacked, and each store's once listed.
pub fn chromium_ids() -> Vec<&'static str> {
    std::iter::once(UNPACKED_ID)
        .chain(store_id())
        .chain(OTHER_STORE_IDS.into_iter().filter(|id| valid_id(id)))
        .collect()
}

/// The extension's page in the Chrome Web Store (Chrome, Edge, Brave, Opera and Vivaldi all
/// install from it), once it is listed: the listing's own address, its name then its ID.
pub fn store_page() -> Option<String> {
    store_id()
        .map(|id| format!("https://chromewebstore.google.com/detail/mister-download-manager/{id}"))
}

/// The tutorial that adds the extension: from the stores, and by hand (a build of its own, or
/// a browser that can't use its store). Its file in docs/: the app opens it on the website, in
/// the interface language (the app's help::page_url).
pub const GUIDE_DOC: &str = "guide-catch-browser-downloads.md";

/// The help page for a browser that doesn't reach Mister Download Manager, at the section for
/// `why` (the native host's reasons, and the app's own checks): its file in docs/ and the
/// section, as GUIDE_DOC.
pub fn help_doc(why: &str) -> String {
    format!("browser-help.md#{why}")
}

/// Marks the data folder once a browser has started the host for the extension.
const SEEN: &str = "extension-seen";

/// The host, started by a browser for the extension: the extension is installed there. The
/// file holds one line per browser (`capture::browser_named`), with the extension's version.
pub fn mark_seen(browser: &str, version: &str) {
    let Some(dir) = crate::data_dir() else {
        return;
    };
    let file = dir.join(SEEN);
    let known = std::fs::read_to_string(&file).ok();
    let mut lines: Vec<(String, String)> = known.as_deref().map(parse_seen).unwrap_or_default();
    let entry = (browser.to_owned(), version.to_owned());
    if known.is_some() && (browser.is_empty() || lines.contains(&entry)) {
        return;
    }
    lines.retain(|(b, _)| b != browser);
    lines.push(entry);
    let text: String = lines.iter().map(|(b, v)| format!("{b}\t{v}\n")).collect();
    let _ = crate::private_dir(&dir);
    let _ = std::fs::write(file, text);
}

/// The lines of `extension-seen`: (browser, extension version). An older file holds only a
/// browser's name.
fn parse_seen(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once('\t') {
            Some((b, v)) => (b.to_owned(), v.to_owned()),
            None => (l.to_owned(), String::new()),
        })
        .collect()
}

/// Has any browser's extension reached the app?
pub fn seen() -> bool {
    crate::data_dir().is_some_and(|d| d.join(SEEN).exists())
}

/// The browsers whose extension has reached the app, with its version (empty when unknown).
pub fn seen_browsers() -> Vec<(String, String)> {
    crate::data_dir()
        .and_then(|d| std::fs::read_to_string(d.join(SEEN)).ok())
        .as_deref()
        .map(parse_seen)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_chromium_ids() {
        assert!(valid_id(UNPACKED_ID));
        assert!(!valid_id("STORE_ID_PENDING") && !valid_id("abc"));
        // A placeholder store ID is never served, registered or linked.
        assert_eq!(store_id().is_some(), valid_id(STORE_ID));
        assert!(chromium_ids().iter().all(|id| valid_id(id)));
        assert_eq!(store_page().is_some(), store_id().is_some());
    }

    /// The store's copy and the app folder's (unpacked) both reach the host: the manifests'
    /// allowed_origins and the host's own caller check are this list.
    #[test]
    fn store_and_unpacked_both_served() {
        assert_eq!(STORE_ID, "ejoleppdfgjdndddcjoehopknmlglojp");
        assert_eq!(UNPACKED_ID, "oogbclbbninliongijffeeggalppliok");
        let ids = chromium_ids();
        assert!(
            ids.contains(&UNPACKED_ID) && ids.contains(&STORE_ID),
            "{ids:?}"
        );
        assert_eq!(
            store_page().as_deref(),
            Some(
                "https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp"
            )
        );
    }

    #[test]
    fn seen_lines_old_and_new() {
        assert_eq!(
            parse_seen("Google Chrome\n"),
            vec![("Google Chrome".into(), String::new())]
        );
        assert_eq!(
            parse_seen("Google Chrome\t0.2.5\nMozilla Firefox\t0.2.5\n").len(),
            2
        );
    }
}
