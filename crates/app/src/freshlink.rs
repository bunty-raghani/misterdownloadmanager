//! A fresh address from the browser for a download that waits for one (`Item::waits_for_address`:
//! its signed link expired, or the user chose Refresh download address). As IDM does while it
//! waits for a new address, the browser's next download of the same file isn't a new row starting
//! over: it gives the waiting download its address, and that one goes on from what it has
//! (`App::take_fresh_address`). The engine checks, as the download resumes, that the new address
//! gives the same file (its size, its validators, the bytes before each part), so a wrong match
//! fails to resume and keeps the data, never mixes two files.
//!
//! "The same file": from the same site (the addresses' registrable domains, or the pages the
//! browser started them from), under the same name (the size, when both are known, must agree too), and never in or from a
//! private window.

use url::{Host, Url};

use crate::model::{Item, Status};

/// Second-level labels under which a country's registry hands out names (`example.co.uk`,
/// `example.com.au`, `example.ne.jp`): such a name's site is three labels long.
const COUNTRY_SECOND_LEVEL: [&str; 13] = [
    "co", "com", "net", "org", "gov", "edu", "ac", "or", "ne", "go", "gob", "nic", "mil",
];

/// The site `url` is of, as near as MDM tells without the public suffix list: its host's
/// registrable domain (`s118.rapidgator.net` and `rapidgator.net` are `rapidgator.net`'s,
/// `dl.example.co.uk` is `example.co.uk`'s); an address by number as it is.
pub fn site(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            let labels: Vec<&str> = d.split('.').collect();
            let n = labels.len();
            let country =
                n >= 3 && labels[n - 1].len() == 2 && COUNTRY_SECOND_LEVEL.contains(&labels[n - 2]);
            let keep = if country { 3 } else { 2 };
            Some(labels[n.saturating_sub(keep)..].join("."))
        }
        ip => Some(ip.to_string()),
    }
}

/// What the browser hands over: its address, the name it gives the file, the size its own
/// download announced, the page it was started from.
pub struct Offer<'a> {
    pub url: &'a Url,
    pub name: &'a str,
    pub size: Option<u64>,
    pub page: Option<&'a str>,
    /// From a private window: never takes a row's place.
    pub private: bool,
}

/// The sites whose addresses expire and whose failures wait for a fresh one on their own
/// (Rapidgator: `rapidgator.net`, `rg.to`; as the engine's `sites::expiring`). Another site's
/// download waits only when the user chose Refresh download address.
pub fn expiring_site(url: &Url) -> bool {
    matches!(site(url).as_deref(), Some("rapidgator.net" | "rg.to"))
}

/// How well `item` matches the offer: None when it can't be the same file (another name, another
/// size, a private window's); else 2 for the same name and size, 1 for the same name with a size
/// not known on one side.
fn matches(item: &Item, offer: &Offer) -> Option<u8> {
    let waiting = item.waits_for_address
        && item.live.is_none()
        && item.status != Status::Complete
        && item.video.is_none()
        && item.gallery.is_none()
        && item.torrent.is_none()
        && item.remote.is_none()
        && !item.is_private_window()
        && !offer.private
        && item.url != *offer.url;
    if !waiting {
        return None;
    }
    let page_site = |p: Option<&str>| p.and_then(|p| Url::parse(p).ok()).and_then(|p| site(&p));
    let same_site = site(&item.url).is_some_and(|s| Some(s) == site(offer.url))
        || page_site(item.page.as_deref()).is_some_and(|s| Some(s) == page_site(offer.page));
    if !same_site {
        return None;
    }
    let size = match (item.size, offer.size) {
        (Some(a), Some(b)) if a != b => return None,
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let name = !offer.name.is_empty() && item.file_name.eq_ignore_ascii_case(offer.name);
    // The name decides; a size alone never does (a multi-part archive's parts have one).
    match (name, size) {
        (true, true) => Some(2),
        (true, false) => Some(1),
        (false, _) => None,
    }
}

/// The download waiting for a new address that `offer` is for: the best match (same name and
/// size before the name alone), the newest of equals. None: the offer is a download of its own.
pub fn waiting_for(items: &[Item], offer: &Offer) -> Option<u64> {
    items
        .iter()
        .filter_map(|i| matches(i, offer).map(|m| (m, i.id)))
        .max()
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Category;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    const OLD: &str = "https://s118.rapidgator.net/download/0f3c9a2e-6b1d-4c8e-9a7f-2d5b8e1c4a60";
    const NEW: &str = "https://s93.rapidgator.net/download/7a1e4b9c-2f3d-4e5a-8b6c-9d0e1f2a3b4c";
    const PAGE: &str = "https://rapidgator.net/file/6e2d0c8a3a7b4f1e9d5c2b1a0f9e8d7c/a.rar.html";
    const SIZE: u64 = 3_270_000_000;

    /// A download stopped at 90 % with its link expired, waiting for a new address.
    fn waiting(id: u64) -> Item {
        let mut i = Item::new(
            id,
            url(OLD),
            "/downloads/Lab.Archive.part1.rar".into(),
            Category::Compressed,
        );
        i.size = Some(SIZE);
        i.downloaded = SIZE / 10 * 9;
        i.status = Status::Error;
        i.page = Some(PAGE.into());
        i.waits_for_address = true;
        i
    }

    fn offer<'a>(u: &'a Url, name: &'a str, size: Option<u64>) -> Offer<'a> {
        Offer {
            url: u,
            name,
            size,
            page: Some(PAGE),
            private: false,
        }
    }

    #[test]
    fn sites_are_registrable_domains() {
        for (u, want) in [
            (OLD, "rapidgator.net"),
            ("https://rapidgator.net/file/1", "rapidgator.net"),
            ("https://dl.example.co.uk/a", "example.co.uk"),
            ("https://a.b.example.com.au/a", "example.com.au"),
            ("https://cdn.example.io/a", "example.io"),
            ("https://localhost:8080/a", "localhost"),
            ("http://192.0.2.7/a", "192.0.2.7"),
            ("https://EXAMPLE.org./a", "example.org"),
        ] {
            assert_eq!(site(&url(u)).as_deref(), Some(want), "{u}");
        }
    }

    /// The user's case: the browser's next download of the file (another file host, the same
    /// name and size) is the waiting download's new address.
    #[test]
    fn the_same_file_from_the_same_site_is_the_waiting_download() {
        let items = vec![waiting(1)];
        let new = url(NEW);
        for (name, size) in [
            ("Lab.Archive.part1.rar", Some(SIZE)),
            ("lab.archive.PART1.rar", None),
        ] {
            assert_eq!(
                waiting_for(&items, &offer(&new, name, size)),
                Some(1),
                "{name} {size:?}"
            );
        }
        // Another site, its page on the same site: the same site still.
        let cdn = url("https://cdn.rapidgator-files.example/x/Lab.Archive.part1.rar");
        assert_eq!(
            waiting_for(&items, &offer(&cdn, "Lab.Archive.part1.rar", Some(SIZE))),
            Some(1)
        );
    }

    #[test]
    fn another_file_or_site_is_a_download_of_its_own() {
        let items = vec![waiting(1)];
        let new = url(NEW);
        // Another size, whatever the name; neither the name nor the size.
        for (name, size) in [
            ("Lab.Archive.part1.rar", Some(SIZE - 1)),
            ("Lab.Archive.part2.rar", None),
            ("Lab.Archive.part2.rar", Some(SIZE + 1)),
            // The same size under another name is another file, not a name's guess either.
            ("Lab.Archive.part2.rar", Some(SIZE)),
            ("7a1e4b9c-2f3d-4e5a-8b6c-9d0e1f2a3b4c", Some(SIZE)),
            ("", None),
        ] {
            assert_eq!(
                waiting_for(&items, &offer(&new, name, size)),
                None,
                "{name} {size:?}"
            );
        }
        // Another site, its page elsewhere too.
        let other = url("https://files.example.com/Lab.Archive.part1.rar");
        let o = Offer {
            url: &other,
            name: "Lab.Archive.part1.rar",
            size: Some(SIZE),
            page: Some("https://example.com/"),
            private: false,
        };
        assert_eq!(waiting_for(&items, &o), None);
        // Its very address: the duplicate check's, not a new address.
        let same = url(OLD);
        assert_eq!(
            waiting_for(&items, &offer(&same, "Lab.Archive.part1.rar", Some(SIZE))),
            None
        );
    }

    /// Only a download that waits for an address, isn't finished, and is a file's (not a gallery's).
    #[test]
    fn only_a_waiting_download_takes_it() {
        let new = url(NEW);
        let o = offer(&new, "Lab.Archive.part1.rar", Some(SIZE));
        let mut not_waiting = waiting(1);
        not_waiting.waits_for_address = false;
        let mut complete = waiting(2);
        complete.status = Status::Complete;
        let mut gallery = waiting(3);
        gallery.gallery = Some(Default::default());
        assert_eq!(waiting_for(&[not_waiting, complete, gallery], &o), None);
    }

    /// Several waiting: the same name and size before one of them, then the newest.
    #[test]
    fn the_best_match_takes_it() {
        let new = url(NEW);
        let mut by_size = waiting(5);
        by_size.file_name = "renamed.rar".into();
        let items = vec![waiting(1), waiting(2), by_size];
        assert_eq!(
            waiting_for(&items, &offer(&new, "Lab.Archive.part1.rar", Some(SIZE))),
            Some(2)
        );
        assert_eq!(
            waiting_for(&items, &offer(&new, "renamed.rar", Some(SIZE))),
            Some(5)
        );
    }

    /// Multi-part archives: part 2 (the same size) is not part 1's new address.
    #[test]
    fn another_part_of_the_same_size_is_not_taken() {
        let new = url(NEW);
        assert_eq!(
            waiting_for(
                &[waiting(1)],
                &offer(&new, "Lab.Archive.part2.rar", Some(SIZE))
            ),
            None
        );
    }

    /// A private window's row neither takes a normal window's download nor is taken.
    #[test]
    fn private_rows_and_offers_stay_apart() {
        let new = url(NEW);
        let mut private_row = waiting(1);
        private_row.context.private = true;
        let normal = offer(&new, "Lab.Archive.part1.rar", Some(SIZE));
        assert_eq!(waiting_for(&[private_row], &normal), None);
        // Read back from the list after a restart: no longer private in memory, still marked.
        let mut reloaded = waiting(1);
        reloaded.private_window = true;
        assert_eq!(waiting_for(&[reloaded], &normal), None);
        let private = Offer {
            private: true,
            ..offer(&new, "Lab.Archive.part1.rar", Some(SIZE))
        };
        assert_eq!(waiting_for(&[waiting(1)], &private), None);
    }

    #[test]
    fn only_expiring_sites_wait_on_their_own() {
        assert!(expiring_site(&url(OLD)));
        assert!(expiring_site(&url(PAGE)));
        assert!(!expiring_site(&url("https://files.example.com/a.rar")));
        assert!(!expiring_site(&url("https://notrapidgator.net/a")));
    }
}
