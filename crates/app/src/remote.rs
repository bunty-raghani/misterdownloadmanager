//! The remote control's side in the app (#141; the server is crates/remote): starting it as
//! Options says, the page's texts from the app's catalogs, and the torrents a paired device
//! sends while their files are looked up. The page's own English texts are listed here, so
//! tools/i18n/extract.py puts them in the catalog; a test keeps the list and the page in step.

use std::sync::{Arc, Mutex};

use crate::model::Category;
use crate::settings::Settings;

/// The remote control for `settings`, started (or not) as they say. None without a data folder.
pub fn start(
    rt: &tokio::runtime::Handle,
    sink: mdm_ipc::Sink,
    settings: &Settings,
) -> Option<mdm_remote::Remote> {
    let dir = mdm_ipc::data_dir()?.join("remote");
    let texts: mdm_remote::Texts = Arc::new(|langs: &str| page_texts(langs));
    let remote = mdm_remote::Remote::new(rt.clone(), dir, sink, texts);
    remote.apply(config(settings));
    Some(remote)
}

/// The page's address as people open it (`MDM_REMOTE_URL`, #157): a container's own address
/// isn't the server's.
pub fn public_url() -> Option<String> {
    let url = std::env::var("MDM_REMOTE_URL").ok()?;
    let url = url.trim();
    url.starts_with("https://").then(|| url.to_owned())
}

/// The names `--hosts` gave a headless run (#157), over `MDM_REMOTE_HOSTS`.
static HOSTS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Use `hosts` (names separated by commas) as [`config`]'s extra names.
pub fn set_hosts(hosts: Option<&str>) {
    if let Some(h) = hosts {
        let _ = HOSTS.set(h.to_owned());
    }
}

/// Where and whether to listen, from `settings`; `MDM_REMOTE_HOSTS` (names separated by commas)
/// adds the names a request may use besides the addresses (a server's, a reverse proxy's).
pub fn config(settings: &Settings) -> mdm_remote::Config {
    let hosts = HOSTS
        .get()
        .cloned()
        .or_else(|| std::env::var("MDM_REMOTE_HOSTS").ok())
        .unwrap_or_default()
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    mdm_remote::Config {
        on: settings.remote_on,
        port: settings.remote_port,
        listen: settings.remote_listen.clone(),
        hosts,
    }
}

/// The page's own texts (crates/remote/web), English as written there.
const PAGE: &[&str] = &[
    // i18n: remote control page (on a phone): its menu's button
    crate::i18n::mark("More"),
    // i18n: remote control page (on a phone): menu command: unpair this phone or computer from MDM
    crate::i18n::mark("Forget this device"),
    // i18n: remote control page (on a phone): title of the pairing screen
    crate::i18n::mark("Pair with Mister Download Manager"),
    // i18n: remote control page (on a phone): how to pair; "Options", "Remote" and "Pair a device" are MDM's dialog, tab and button
    crate::i18n::mark(
        "On the computer, open Options, then Remote, and click Pair a device. Enter the code it shows.",
    ),
    // i18n: remote control page (on a phone): label of the pairing code's box (8 digits)
    crate::i18n::mark("Code"),
    // i18n: remote control page (on a phone): label of the box naming this phone (shown in MDM's list of paired devices)
    crate::i18n::mark("This device's name"),
    // i18n: remote control page (on a phone): button: pair this phone with MDM
    crate::i18n::mark("Pair"),
    // i18n: remote control page (on a phone): label of the box where a download's address or a magnet link is typed
    crate::i18n::mark("Address or magnet link"),
    // i18n: remote control page (on a phone): button that picks a .torrent file on the phone
    crate::i18n::mark(".torrent file"),
    // i18n: remote control page (on a phone): what the ".torrent file" button does (screen readers)
    crate::i18n::mark("Choose a .torrent file"),
    // i18n: remote control page (on a phone): label of the category choice for a new download
    crate::i18n::mark("Category"),
    // i18n: remote control page (on a phone): label of the choice: start the new download now, or let it wait in a queue
    crate::i18n::mark("When"),
    // i18n: remote control page (on a phone): button: add the download
    crate::i18n::mark("Add"),
    // i18n: remote control page (on a phone): tab: every download
    crate::i18n::mark("All"),
    // i18n: remote control page (on a phone): tab: downloads not finished yet
    crate::i18n::mark("Unfinished"),
    // i18n: remote control page (on a phone): tab: downloads finished
    crate::i18n::mark("Finished"),
    // i18n: remote control page (on a phone): the list shown is empty
    crate::i18n::mark("No downloads here."),
    // i18n: remote control page (on a phone): checkbox when removing a download: delete its file from the computer too
    crate::i18n::mark("Delete the file too"),
    crate::i18n::mark("Cancel"),
    // i18n: remote control page (on a phone): button: remove a download from the list
    crate::i18n::mark("Remove"),
    // i18n: remote control page (on a phone): pairing failed; {n} = how many more codes may be tried
    crate::i18n::mark("Wrong code. Tries left: {n}"),
    // i18n: remote control page (on a phone): pairing failed and the code is void now
    crate::i18n::mark("Wrong code. Show a new code in MDM and try again."),
    // i18n: remote control page (on a phone): no pairing code is shown in MDM; "Pair a device" is MDM's button
    crate::i18n::mark("MDM shows no code now. Click Pair a device there first."),
    // i18n: remote control page (on a phone): codes were tried too fast
    crate::i18n::mark("Wait a moment, then try again."),
    // i18n: remote control page (on a phone): MDM can't be reached while pairing
    crate::i18n::mark("MDM does not answer. Check that it is running and on the same network."),
    // i18n: remote control page (on a phone): MDM can't be reached now; the page keeps trying by itself
    crate::i18n::mark("MDM does not answer. Trying again..."),
    // i18n: remote control page (on a phone): free disk space on the computer; {size} e.g. "120 GB"
    crate::i18n::mark("{size} free"),
    // i18n: remote control page (on a phone): category choice: the category follows the file's type
    crate::i18n::mark("Automatic (by file type)"),
    // i18n: remote control page (on a phone): "When" choice: start the download at once
    crate::i18n::mark("Start now"),
    // i18n: remote control page (on a phone): "When" choice: the download waits in a queue; {queue} its name
    crate::i18n::mark("Wait in {queue}"),
    // i18n: remote control page (on a phone): button: stop a queue
    crate::i18n::mark("Stop"),
    // i18n: remote control page (on a phone): button: start a download or a queue
    crate::i18n::mark("Start"),
    // i18n: remote control page (on a phone): button: pause a download
    crate::i18n::mark("Pause"),
    // i18n: remote control page (on a phone): progress; {done} and {size} are sizes ("1.2 MB of 7.5 MB")
    crate::i18n::mark("{done} of {size}"),
    // i18n: remote control page (on a phone): an action failed
    crate::i18n::mark("MDM could not do it."),
    // i18n: remote control page (on a phone): question before removing a download; {name} its file name
    crate::i18n::mark("Remove {name} from the list?"),
    // i18n: remote control page (on a phone): the file chosen is too large for a .torrent file
    crate::i18n::mark("This file is too large to be a torrent."),
    // i18n: remote control page (on a phone): a magnet link or .torrent file was added; MDM reads the torrent's files first
    crate::i18n::mark("Added. MDM is reading the torrent: it joins the list in a moment."),
    // i18n: remote control page (on a phone): a download was added
    crate::i18n::mark("Added."),
    // i18n: remote control page (on a phone): the address typed isn't one MDM downloads (not http, https, ftp or magnet)
    crate::i18n::mark("MDM cannot download this address."),
    // i18n: remote control page (on a phone): the file chosen isn't a .torrent file
    crate::i18n::mark("This is not a torrent file."),
    // i18n: remote control page (on a phone): adding a download failed
    crate::i18n::mark("MDM could not add it."),
    // i18n: remote control page (on a phone): question before unpairing this phone
    crate::i18n::mark("Forget this device? It needs a new code to connect again."),
    // i18n: remote control page (on a phone): a download's state: added, waiting to start
    crate::i18n::mark("Queued"),
    crate::i18n::mark("Downloading"),
    crate::i18n::mark("Stopped"),
    crate::i18n::mark("Complete"),
    crate::i18n::mark("Error"),
];

/// Texts the page shows that the app has already (the built-in categories' and queues' names).
const KNOWN: &[&str] = &[
    "General",
    "Compressed",
    "Documents",
    "Music",
    "Programs",
    "Video",
    "Torrents",
    "Main download queue",
    "Synchronization queue",
];

/// The units the page writes sizes and speeds with (context "unit"; key `unit:<template>`).
const UNITS: &[&str] = &["{n} KB", "{n} MB", "{n} GB", "{n} KB/sec", "{n} MB/sec"];

/// The page's texts in the first of `langs` (the browser's languages, most wanted first) that
/// MDM speaks; else in MDM's own language.
pub fn page_texts(langs: &str) -> mdm_remote::PageTexts {
    let i = crate::i18n::i18n();
    let lang = langs
        .split(',')
        .map(|t| t.split(';').next().unwrap_or("").trim())
        .filter(|t| !t.is_empty())
        .find_map(|t| i.match_tag(t))
        .unwrap_or_else(crate::i18n::lang);
    let mut texts = std::collections::BTreeMap::new();
    for s in PAGE.iter().chain(KNOWN) {
        texts.insert((*s).to_owned(), i.t(lang, s).to_owned());
    }
    for u in UNITS {
        texts.insert(format!("unit:{u}"), i.tc(lang, "unit", u).to_owned());
    }
    mdm_remote::PageTexts {
        lang: i.code(lang).to_owned(),
        rtl: i.rtl(lang),
        texts,
    }
}

/// A QR code of `text` (black on white, whatever the theme: phones' cameras read it so) in the
/// middle of `rect`, with its quiet zone, in whole pixels.
pub fn paint_qr(p: &egui::Painter, rect: egui::Rect, text: &str) {
    use egui::{Color32, Rect, vec2};
    let Ok(qr) = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium) else {
        return;
    };
    let n = qr.size();
    let quiet = 3;
    let cells = (n + 2 * quiet) as f32;
    let cell = (rect.width().min(rect.height()) / cells).floor().max(1.0);
    let side = cell * cells;
    let origin = (rect.center() - vec2(side / 2.0, side / 2.0)).round();
    p.rect_filled(
        Rect::from_min_size(origin, vec2(side, side)),
        0.0,
        Color32::WHITE,
    );
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
                let at = origin + vec2((x + quiet) as f32 * cell, (y + quiet) as f32 * cell);
                p.rect_filled(
                    Rect::from_min_size(at, vec2(cell, cell)),
                    0.0,
                    Color32::BLACK,
                );
            }
        }
    }
}

/// A QR code of `text` for a terminal: two rows of modules per line, with half blocks. Light
/// modules are drawn (full blocks) and dark ones left as the terminal's background, so it reads
/// on a dark terminal, the usual one; its quiet zone is light.
pub fn qr_text(text: &str) -> String {
    let Ok(qr) = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Low) else {
        return String::new();
    };
    let (n, quiet) = (qr.size(), 2);
    let dark = |x: i32, y: i32| x >= 0 && y >= 0 && x < n && y < n && qr.get_module(x, y);
    let mut out = String::new();
    let mut y = -quiet;
    while y < n + quiet {
        for x in -quiet..n + quiet {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (false, false) => '\u{2588}',
                (false, true) => '\u{2580}',
                (true, false) => '\u{2584}',
                (true, true) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}

/// A torrent's files being looked up: filled in on the runtime when they are known.
pub type Slot = Arc<Mutex<Option<Result<mdm_torrent::Meta, String>>>>;

/// A torrent a paired device sent, or a magnet link ticked in Link grabber (#709): its files
/// looked up on the runtime, then added as Add torrent would with everything chosen.
pub struct Lookup {
    pub slot: Slot,
    pub category: Option<Category>,
    /// The queue it waits in (by index), or None to start now.
    pub queue: Option<usize>,
    pub from_magnet: bool,
    /// The folder its own folder goes in (Link grabber's package's), else its category's.
    pub dir: Option<std::path::PathBuf>,
    /// The downloads added with it (#766): it joins their group, for one summary.
    pub group: Option<u64>,
    /// Someone waits for it at the computer: a look-up that fails says so (a paired device's
    /// is only logged).
    pub tell: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text the page shows is one the app translates for it.
    #[test]
    fn the_page_and_its_texts_agree() {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../remote/web");
        let html = std::fs::read_to_string(web.join("index.html")).unwrap();
        let js = std::fs::read_to_string(web.join("app.js")).unwrap();
        let mut used = Vec::new();
        for attr in ["data-t=\"", "data-ta=\"", "data-tp=\""] {
            for part in html.split(attr).skip(1) {
                used.push(part.split('"').next().unwrap().to_owned());
            }
        }
        for call in ["T('", "F('"] {
            for part in js.split(call).skip(1) {
                used.push(part.split('\'').next().unwrap().to_owned());
            }
        }
        // The statuses, looked up by their English name.
        for s in mdm_ipc::api::STATUSES {
            used.push(s.to_owned());
        }
        assert!(used.len() > 40, "the page's texts were found");
        for u in &used {
            assert!(
                PAGE.contains(&u.as_str()) || KNOWN.contains(&u.as_str()),
                "the page shows {u:?}, which remote.rs doesn't list"
            );
        }
        for p in PAGE {
            assert!(
                used.iter().any(|u| u == p),
                "{p:?} is listed but not on the page"
            );
        }
    }

    #[test]
    fn texts_follow_the_browser() {
        let de = page_texts("de-DE,de;q=0.9,en;q=0.8");
        assert_eq!(de.lang, "de");
        assert!(!de.rtl);
        let ar = page_texts("ar-TN");
        assert_eq!(ar.lang, "ar");
        assert!(ar.rtl);
        assert_eq!(page_texts("en-US").texts["Pair"], "Pair");
        assert!(de.texts.contains_key("unit:{n} MB"));
    }
}
