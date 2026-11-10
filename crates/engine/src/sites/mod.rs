//! Sites whose links aren't the file's own address (#34): a Google Drive link leads to Drive's
//! viewer, not to the file. `download::get` asks these hooks on its way to the file; everything a
//! site knows stays in its own module, so the engine stays generic and a later site is one more
//! module here.

pub mod gdrive;
mod gofile;

use url::Url;

/// A site's own refusal to give the file, in its words where it has them: no retry helps now,
/// and the interface says what to do instead (`Error::Refused`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Google Drive's own error page (most often: too many downloads of this file lately),
    /// with Google's words, in the language Google chose.
    Drive { message: String },
    /// The file isn't shared with everyone: sign in to Google (in the browser MDM takes the
    /// cookies from).
    SignIn,
    /// Signed in, but this account has no access (or the owner turned downloading off).
    NoAccess,
    /// A Drive folder: it lists files, it isn't one.
    Folder,
    /// A site's link is a web page that needs a browser (`site`, its name), not the file (#346).
    NeedsBrowser { site: String },
    /// A service account's refusal to unlock the link (#44, `crate::unlock`).
    Service {
        service: String,
        fault: crate::unlock::ServiceFault,
    },
}

impl std::fmt::Display for Refusal {
    /// English, for the command line and the event log (the app words it in its language).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Drive { message } => {
                write!(f, "Google Drive won't give this file now: {message}")
            }
            Refusal::SignIn => write!(
                f,
                "this Google Drive file needs you to sign in: sign in to Google in your browser, \
                 then download it from there"
            ),
            Refusal::NoAccess => write!(
                f,
                "your Google account has no access to this file, or its owner turned off \
                 downloading"
            ),
            Refusal::Folder => write!(f, "this is a Google Drive folder, not a file"),
            Refusal::NeedsBrowser { site } => write!(
                f,
                "this {site} link is a web page, not the file: open it in your browser and download from there"
            ),
            Refusal::Service { service, fault } => write!(f, "{service}: {fault}"),
        }
    }
}

/// What a site's web page says about the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageSays {
    /// Ask this address next (a confirmation form's target).
    Next(Url),
    /// The site refuses (its error page).
    Refused(Refusal),
    /// Nothing the site's rules know: the generic reading (meta refresh, the page itself) goes on.
    Nothing,
}

/// A link that names something that isn't a file (a Drive folder): refused before asking.
pub(crate) fn not_a_file(url: &Url) -> Option<Refusal> {
    gdrive::not_a_file(url).or_else(|| gofile::not_a_file(url))
}

/// The address to ask instead of `url`, when `url` is a site's link to a file rather than the
/// file (a Drive viewer link → its download address). None: ask `url` itself.
pub(crate) fn rewrite(url: &Url) -> Option<Url> {
    gdrive::rewrite(url)
}

/// What the web page `page` (its first `PAGE_PEEK` bytes), answered 200 at `at`, says.
pub(crate) fn read_page(at: &Url, page: &[u8]) -> PageSays {
    if !gdrive::is_drive_host(at) {
        return PageSays::Nothing;
    }
    gdrive::read_page(at, &String::from_utf8_lossy(page))
}

/// An answer that is a site's refusal by where it ended (`at`, asked as `asked`) and its status.
pub(crate) fn refused(asked: &Url, at: &Url, status: u16) -> Option<Refusal> {
    gdrive::refused(asked, at, status)
}

/// `a` and `b` are one party's hosts (Drive's three): the user's credentials given for one go to
/// the other, as the browser's own `.google.com` cookies do (see `gdrive::same_party`).
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    gdrive::same_party(a, b)
}

/// The User-Agent a site's hosts want instead of MDM's default (Drive: a browser's, see
/// `gdrive::USER_AGENT`). None: the download's own.
pub(crate) fn user_agent(url: &Url) -> Option<&'static str> {
    gdrive::user_agent(url)
}

/// Where to ask once more before a site's refusal at `at` is reported (Drive: whether the ID is
/// a Google Doc, which Drive's file address answers with an error page).
pub(crate) fn second_look(at: &Url) -> Option<Url> {
    gdrive::second_look(at)
}

/// `at` is a site's address for a file: a web page from it without a file name is the site
/// talking, never the file (it is an error, not saved).
pub(crate) fn expects_file(at: &Url) -> bool {
    gdrive::expects_file(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #346: the check `download::get` makes before asking refuses gofile's page.
    #[test]
    fn a_gofile_page_is_refused_before_asking() {
        let url = Url::parse("https://gofile.io/d/AbC123").unwrap();
        assert_eq!(
            not_a_file(&url),
            Some(Refusal::NeedsBrowser {
                site: "gofile".to_owned()
            })
        );
    }
}
