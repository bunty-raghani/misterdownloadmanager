//! Internet Archive (archive.org) links and petabox storage cluster nodes (#1298).
//!
//! Archive.org's download links (`archive.org/download/<item_id>/<file>`) respond with a
//! 302 Found redirect to a specific storage node in its server cluster (e.g., `ia600100.us.archive.org`,
//! `ia800102.us.archive.org`, `dn720303.ca.archive.org`, etc.).
//!
//! Account-restricted items (such as borrowed library books or restricted collections) require
//! the user's session cookies (`logged-in-user` and `logged-in-sig` set for domain `.archive.org`).
//! Because the petabox servers reside on subdomains of `archive.org`, they are one party:
//! preserving the cookies across the redirect allows authorized downloads to succeed rather
//! than failing with HTTP 403 Forbidden.

use url::Url;

/// One of Internet Archive's hosts (`archive.org` or any `*.archive.org` subdomain),
/// over http(s).
pub fn is_archive_host(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|h| {
            h.eq_ignore_ascii_case("archive.org")
                || h.to_ascii_lowercase().ends_with(".archive.org")
        })
}

/// Two of Internet Archive's hosts on the same port: one party.
///
/// The browser sets domain cookies for `.archive.org` (`logged-in-user` and `logged-in-sig`),
/// which are required by the petabox storage servers when downloading account-required files.
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    is_archive_host(a)
        && is_archive_host(b)
        && a.port_or_known_default() == b.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn recognizes_archive_org_and_storage_subdomains() {
        assert!(is_archive_host(&url(
            "https://archive.org/download/item/file.pdf"
        )));
        assert!(is_archive_host(&url("http://ARCHIVE.ORG/details/item")));
        assert!(is_archive_host(&url(
            "https://ia600100.us.archive.org/24/items/item/file.pdf"
        )));
        assert!(is_archive_host(&url(
            "https://ia800102.us.archive.org/items/item/file.zip"
        )));
        assert!(is_archive_host(&url(
            "https://dn720303.ca.archive.org/0/items/item/file.epub"
        )));
        assert!(is_archive_host(&url("https://services.archive.org/api/")));

        // Look-alikes and unrelated hosts are not archive hosts.
        assert!(!is_archive_host(&url("https://notarchive.org/download")));
        assert!(!is_archive_host(&url("https://fake-archive.org/")));
        assert!(!is_archive_host(&url("https://archive.org.attacker.com/")));
        assert!(!is_archive_host(&url("ftp://archive.org/download/item")));
    }

    #[test]
    fn archive_org_and_petabox_storage_are_same_party() {
        let frontend = url("https://archive.org/download/item/file.pdf");
        let storage1 = url("https://ia600100.us.archive.org/24/items/item/file.pdf");
        let storage2 = url("https://ia800102.us.archive.org/items/item/file.pdf");
        let foreign = url("https://example.com/file.pdf");

        assert!(same_party(&frontend, &storage1));
        assert!(same_party(&storage1, &storage2));
        assert!(same_party(&frontend, &storage2));

        assert!(!same_party(&frontend, &foreign));
        assert!(!same_party(&storage1, &foreign));

        // Different ports are not the same party.
        let different_port = url("https://ia600100.us.archive.org:8443/items/item/file.pdf");
        assert!(!same_party(&frontend, &different_port));
    }
}
