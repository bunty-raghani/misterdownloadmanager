//! Sites whose links aren't the file's own address (#34): a Google Drive link leads to Drive's
//! viewer, not to the file. `download::get` asks these hooks on its way to the file; everything a
//! site knows stays in its own module, so the engine stays generic and a later site is one more
//! module here.

pub mod archive;
pub mod gdrive;
mod gofile;
mod lulustream;
mod mediafire;
mod rapidgator;

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
    /// A site kept answering its download form with the form again (`site`, its name): it didn't
    /// give the file this time, and asking again in a moment usually does (LuluStream's).
    NotTaken { site: String },
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
            Refusal::NotTaken { site } => write!(
                f,
                "{site} didn't give the file this time: try again in a moment"
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
    /// Post this form to `action` next, its fields in `body` as a browser encodes them
    /// (`application/x-www-form-urlencoded`): a site whose page gives the file only once its form
    /// is sent back (LuluStream's). Its answer is read as a page; the form again is posted again,
    /// a few times at most (`download::MAX_FORM_POSTS`).
    Form { action: Url, body: String },
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
    gdrive::rewrite(url).or_else(|| lulustream::rewrite(url))
}

/// The refusal when the site at `at` still answers its form with the form after its last post
/// (`PageSays::Form`, `download::MAX_FORM_POSTS`). None: the page is just a web page
/// (`Error::GotWebPage`).
pub(crate) fn form_refused(at: &Url) -> Option<Refusal> {
    lulustream::form_refused(at)
}

/// What the web page `page` (its first `PAGE_PEEK` bytes), answered 200 at `at`, says.
pub(crate) fn read_page(at: &Url, page: &[u8]) -> PageSays {
    if gdrive::is_drive_host(at) {
        gdrive::read_page(at, &String::from_utf8_lossy(page))
    } else if mediafire::is_site(at) {
        mediafire::read_page(at, &String::from_utf8_lossy(page))
    } else if rapidgator::is_site(at) {
        rapidgator::read_page(at, &String::from_utf8_lossy(page))
    } else if lulustream::is_site(at) {
        lulustream::read_page(at, &String::from_utf8_lossy(page))
    } else {
        PageSays::Nothing
    }
}

/// The page that gives `url` again when it stops working, for a site whose file addresses
/// expire (MediaFire's, a few hours after its page gave one, #952; Rapidgator's, whose page is
/// known only from `pages`: the browser's tab the download was started from and its Referer):
/// the download's origin (`download::Ctx::refresh_target`). None: `url` itself is all there is.
pub(crate) fn origin(url: &Url, pages: &[Url]) -> Option<Url> {
    mediafire::origin(url)
        .or_else(|| rapidgator::origin(url, pages))
        .or_else(|| lulustream::origin(url, pages))
}

/// `url` is a site's address for a file that expires (a signed link: Rapidgator's file hosts'):
/// a dead link's answer from it is the link expired (`Error::LinkExpired`), even when no page is
/// known to ask for a fresh one.
pub(crate) fn expiring(url: &Url) -> bool {
    rapidgator::expiring(url)
}

/// An answer that is a site's refusal by where it ended (`at`, asked as `asked`) and its status.
pub(crate) fn refused(asked: &Url, at: &Url, status: u16) -> Option<Refusal> {
    gdrive::refused(asked, at, status)
}

/// `a` and `b` are one party's hosts (Drive's three, Internet Archive's frontend and petabox
/// storage nodes, Rapidgator's pages and file hosts): the user's credentials given for one go to
/// the other, as the browser's own domain cookies do (see `gdrive::same_party`,
/// `archive::same_party` and `rapidgator::same_party`).
pub(crate) fn same_party(a: &Url, b: &Url) -> bool {
    gdrive::same_party(a, b)
        || archive::same_party(a, b)
        || rapidgator::same_party(a, b)
        || lulustream::same_party(a, b)
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
        || mediafire::expects_file(at)
        || rapidgator::expects_file(at)
        || lulustream::expects_file(at)
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
