//! gofile.io (#346). Its share links (`gofile.io/d/<id>`) are a web page that lists the files
//! with JavaScript, so asking one gets that page: MDM saved it in place of the file. Only its
//! file hosts' direct links are files, and those come from the browser.

use url::Url;

use super::Refusal;

/// A link to gofile's page rather than to a file (`Refusal::NeedsBrowser`).
pub(crate) fn not_a_file(url: &Url) -> Option<Refusal> {
    let host = url.host_str()?;
    if !matches!(url.scheme(), "http" | "https")
        || !(host.eq_ignore_ascii_case("gofile.io") || host.eq_ignore_ascii_case("www.gofile.io"))
    {
        return None;
    }
    let mut parts = url.path_segments()?;
    (parts.next() == Some("d") && parts.next().is_some_and(|id| !id.is_empty())).then(|| {
        Refusal::NeedsBrowser {
            site: "gofile".to_owned(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_share_link_is_a_page_for_the_browser() {
        for link in [
            "https://gofile.io/d/AbC123",
            "https://www.gofile.io/d/AbC123",
            "http://GOFILE.io/d/AbC123?x=1",
        ] {
            assert_eq!(
                not_a_file(&url(link)),
                Some(Refusal::NeedsBrowser {
                    site: "gofile".to_owned()
                }),
                "{link}"
            );
        }
    }

    /// A file host's direct link, the site's other pages, and look-alike names go on as usual.
    #[test]
    fn its_files_and_other_addresses_are_left_alone() {
        for link in [
            "https://store1.gofile.io/download/web/abc/archive.rar",
            "https://gofile.io/",
            "https://gofile.io/d/",
            "https://gofile.io/myFiles",
            "https://gofile.io.example.com/d/AbC123",
            "https://notgofile.io/d/AbC123",
        ] {
            assert_eq!(not_a_file(&url(link)), None, "{link}");
        }
    }
}
