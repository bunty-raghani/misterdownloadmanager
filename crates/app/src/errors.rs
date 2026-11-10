//! Why a download failed, in the interface language (every visible text goes through the
//! catalog; D-019). The engine's own English (`Display`) stays for the command line and the
//! event log. Every engine error is named here, so a new one can't reach the list untranslated.
//! Data inside a message (status codes, paths, sizes) goes in as isolates (crate::i18n::fmt).

use mdm_engine::{Error, NetKind, Refusal};

use crate::i18n::{fmt, ltr, t};

/// The sentence the list, the progress window and Properties show for `e`.
pub fn describe(e: &Error) -> String {
    match e {
        Error::Http(h) => http(h),
        Error::Status(s) => fmt(
            t("the server answered {status}"),
            &[("status", &ltr(&s.to_string()))],
        ),
        // i18n: why a download failed: the site's bot check (Cloudflare) lets browsers through, not programs. Alt+click: the link clicked with the Alt key held, which leaves the download to the browser
        Error::BotCheck => t("this site lets only browsers through; Alt+click the link").to_owned(),
        Error::Throttled(s, _) => fmt(
            t("the server is busy ({status}); try again later"),
            &[("status", &ltr(&s.to_string()))],
        ),
        Error::BadRange(what) => fmt(
            t("the server sent an unexpected part of the file ({detail})"),
            &[("detail", &ltr(what))],
        ),
        Error::ShortBody => t("the connection closed before the part was complete").to_owned(),
        Error::FileChanged => t("the file changed on the server").to_owned(),
        Error::NoRanges => {
            t("the server can't send parts of the file, so it can't be resumed").to_owned()
        }
        Error::BadState(what) => fmt(t("cannot resume: {detail}"), &[("detail", &ltr(what))]),
        Error::Io(io) => fmt(
            t("disk error: {error}"),
            &[("error", &os_text(&io.to_string()))],
        ),
        Error::NoSpace { needed, free } => fmt(
            t("not enough disk space: {needed} needed, {free} free"),
            &[
                ("needed", &crate::model::size(*needed)),
                ("free", &crate::model::size(*free)),
            ],
        ),
        Error::DiskFull => t("the disk is full").to_owned(),
        Error::TooBigForDrive { size, file_system } => fmt(
            t("{size} is too big for this {file_system} drive (files up to 4 GB)"),
            &[
                ("size", &crate::model::size(*size)),
                ("file_system", &ltr(file_system)),
            ],
        ),
        Error::Cancelled => t("cancelled").to_owned(),
        Error::Adapter(e) => adapter(e),
        Error::InUse(path) => fmt(
            t("already being downloaded: {path}"),
            &[("path", &ltr(&path.display().to_string()))],
        ),
        Error::DestinationUnavailable(path) => fmt(
            t("the folder {path} is not available (drive removed or network share offline?)"),
            &[("path", &ltr(&path.display().to_string()))],
        ),
        Error::DriveGone => t("the drive or network share is no longer available").to_owned(),
        Error::PathTooLong(path) => fmt(
            t("the path is too long for Windows: {path}"),
            &[("path", &ltr(&path.display().to_string()))],
        ),
        Error::GotWebPage => t(
            "the server sent a web page instead of the file (sign-in needed or the link expired?)",
        )
        .to_owned(),
        Error::RangesUnavailable => t("the server did not send the requested part").to_owned(),
        Error::LinkExpired => {
            t("the download link has expired (refresh the download address)").to_owned()
        }
        Error::SlowConnection => {
            t("much slower than the other connections; reconnecting").to_owned()
        }
        Error::ChecksumMismatch(algorithm) => fmt(
            t("the file's {algorithm} checksum doesn't match (the data is kept; redownload it)"),
            &[("algorithm", &ltr(algorithm))],
        ),
        Error::FtpReply { code, text } => fmt(
            // i18n: {code} is the FTP server's reply number (such as 550), {reply} the words it sent with it (left as sent)
            t("the FTP server answered {code} {reply}"),
            &[("code", &ltr(&code.to_string())), ("reply", &ltr(text))],
        ),
        Error::Refused(r) => refusal(r),
        Error::NoCertificates => {
            // i18n: a download's error on a computer (a minimal Linux) without the certificates that secure connections check servers with; "ca-certificates" is the package's name, left as is
            t("this computer has no certificates to check the server with: install the ca-certificates package, then try again").to_owned()
        }
        Error::Net { kind, cause } => {
            let what = match kind {
                NetKind::Connect => t("cannot connect to the server"),
                NetKind::Timeout => t("the server did not answer in time"),
                NetKind::Lost => t("the connection was lost while receiving data"),
                // i18n: an FTPS (or other TLS) connection could not be secured
                NetKind::Tls => t("the secure connection failed"),
            };
            let cause = os_text(cause);
            if cause.is_empty() {
                what.to_owned()
            } else {
                fmt(
                    t("{problem} ({cause})"),
                    &[("problem", what), ("cause", &cause)],
                )
            }
        }
    }
}

/// A site's refusal to give the file (Google Drive, #34), with what to do about it.
/// A debrid service's refusal (#44), `{service}` its name ("Real-Debrid").
pub fn service(service: &str, fault: &mdm_engine::ServiceFault) -> String {
    use mdm_engine::ServiceFault as F;
    let text = match fault {
        // i18n: a download's error through a debrid account (Real-Debrid…): its sign-in no longer works
        F::SignIn => t("{service}: sign in again (Options → Accounts)"),
        // i18n: a debrid service's error: it wants the user to allow this computer (it sent an email)
        F::NeedsApproval => t("{service} asks you to allow this computer first: see its email"),
        // i18n: a debrid service's error: the account is locked, banned or not activated
        F::AccountBlocked => t("{service}: the account is locked or not activated"),
        // i18n: a debrid service's error: the account has no premium (or it ended)
        F::NotPremium => t("{service}: this needs a premium account"),
        // i18n: a debrid service's error: it can't reach the file's host now; MDM retries later
        F::HostDown => {
            t("{service} can't reach this file host right now; MDM will try again later")
        }
        // i18n: a debrid service's error: the account's limit (traffic) for this host is used up
        F::LimitReached => t("{service}: your limit for this file host is used up for now"),
        // i18n: a debrid service's error: too many downloads at once through the account
        F::TooManyActive => {
            t("{service}: too many downloads at once through your account; MDM will try again")
        }
        // i18n: a debrid service's error: it refuses this computer's address (a VPN or a server address)
        F::IpRefused => t("{service} refuses this connection (a VPN or server address?)"),
        // i18n: a debrid service's error: the file is gone from its host
        F::FileUnavailable => t("{service}: the file is gone or unavailable on the file host"),
        // i18n: a debrid service's error: it won't give this file (not allowed)
        F::FileRefused => t("{service} won't give this file"),
        // i18n: a debrid service's error: the file has a password
        F::PasswordNeeded => t("{service}: this file is password-protected"),
        // i18n: a debrid service's error: too many requests; MDM waits and retries
        F::TooManyRequests(_) => t("{service} asks to slow down; MDM will try again in a minute"),
        // i18n: a debrid service's error: it isn't answering
        F::ServiceDown => t("{service} is not responding; try again later"),
        // i18n: a debrid service's error: it can't get the torrent sent to it (nobody shares it any more, or its magnet link can't be read)
        F::TorrentUnavailable => {
            t("{service} can't get this torrent: nobody shares it, or its link can't be read")
        }
        F::UnsafeName(name) => {
            return fmt(
                // i18n: a torrent through a debrid service: one of its files has a name that would be written outside its folder; nothing was written. {name} the file's name as the service gave it
                t("A file of this torrent can't be saved safely: {name}"),
                &[("name", &ltr(name))],
            );
        }
        F::Other(message) => {
            return fmt(
                // i18n: a debrid service's error in its own words (English); {service} its name
                t("{service}: {message}"),
                &[("service", service), ("message", &ltr(message))],
            );
        }
    };
    fmt(text, &[("service", service)])
}

fn refusal(r: &Refusal) -> String {
    match r {
        Refusal::Drive { message } => fmt(
            // i18n: a download's error; {message} = Google Drive's own explanation, in the language Google chose (often: too many people downloaded the file lately)
            t(
                "Google Drive won't give this file now: {message} (signing in to Google in your browser often helps; otherwise, try again later)",
            ),
            &[("message", &ltr(message))],
        ),
        Refusal::SignIn => {
            // i18n: a download's error: a Google Drive file that isn't shared with everyone
            t("this Google Drive file needs you to sign in: sign in to Google in your browser, then download it from there")
                .to_owned()
        }
        Refusal::NoAccess => {
            // i18n: a download's error: the signed-in Google account may not download this Drive file
            t("your Google account has no access to this file, or its owner turned off downloading")
                .to_owned()
        }
        Refusal::Folder => {
            // i18n: a download's error: the address is a Google Drive folder, which isn't one file
            t("this is a Google Drive folder, not a file").to_owned()
        }
        Refusal::NeedsBrowser { site } => fmt(
            // i18n: a download's error: the address is a site's web page that needs a browser, not the file; {site} = the site's name (gofile)
            t(
                "this {site} link is a web page, not the file: open it in your browser and download from there",
            ),
            &[("site", &ltr(site))],
        ),
        Refusal::Service { service, fault } => self::service(service, fault),
    }
}

/// Why a torrent couldn't be listed or added, in the interface language. librqbit's own errors
/// (`Engine`) come in its English.
pub fn torrent(e: &mdm_torrent::TorrentError) -> String {
    use mdm_torrent::TorrentError as E;
    match e {
        E::NotATorrent => t("this is not a magnet link or a .torrent file").to_owned(),
        E::MetadataTimeout => t("no peer sent the torrent's file list in time").to_owned(),
        E::Unknown => t("this torrent is not in the list").to_owned(),
        // i18n: error when adding a torrent; {name} = a file's path inside the torrent
        E::UnsafeName(u) => fmt(
            t("this torrent has a file name that isn't safe to write, so it wasn't added: {name}"),
            &[("name", &ltr(&u.path))],
        ),
        E::Engine(text) => text.clone(),
    }
}

/// Why the chosen network adapter (#35) didn't let a connection open, in the interface language.
pub fn adapter(e: &mdm_engine::NetError) -> String {
    use mdm_engine::NetError as N;
    match e {
        N::Down { label } => fmt(
            // i18n: an error: the network adapter chosen in Options (a VPN's, usually) is off or gone; {adapter} its name
            t("the network adapter {adapter} is not connected"),
            &[("adapter", label)],
        ),
        N::Family { label, ip } => fmt(
            // i18n: an error: the server's address is IPv6 (or IPv4) and the chosen network adapter has none of that kind; {ip} the address, {adapter} the adapter's name
            t(
                "{ip} can't be reached through {adapter}: it has no address of that kind (IPv4 or IPv6)",
            ),
            &[("ip", &ltr(&ip.to_string())), ("adapter", label)],
        ),
        N::Unbound { label } => fmt(
            // i18n: an error: a server on this computer redirected the download to the internet, which would not go through the chosen network adapter; {adapter} its name
            t(
                "a server on this computer sent the download elsewhere, which would not go through {adapter}",
            ),
            &[("adapter", label)],
        ),
        N::NotSupported { label, what } => fmt(
            // i18n: an error: {what} (e.g. "ftps://", "Video pages (yt-dlp)") can't be held to the chosen network adapter yet, so it isn't sent; {adapter} its name
            t("{what} can't go through a chosen network adapter ({adapter}) yet"),
            &[("what", &ltr(what)), ("adapter", label)],
        ),
    }
}

/// Why yt-dlp or ffmpeg couldn't be installed, in the interface language (mdm-tools words its
/// errors in English, for the command line). File and build names stay as they are.
pub fn tools(e: &mdm_tools::Error) -> String {
    use mdm_tools::Error as E;
    match e {
        E::Http(e) => http(e),
        // i18n: error installing yt-dlp or ffmpeg; {error} = the system's reason
        E::Io(e) => fmt(
            t("its files could not be written: {error}"),
            &[("error", &e.to_string())],
        ),
        E::BadSignature => {
            t("the release's checksum list is not signed by its publisher's key").to_owned()
        }
        E::BadChecksum => t("the downloaded program doesn't match its signed checksum").to_owned(),
        // i18n: error installing yt-dlp or ffmpeg; {what} = what its release lacks (a file name, or e.g. "LGPL Windows build")
        E::Missing(what) => fmt(t("the release has no {what}"), &[("what", &ltr(what))]),
        // i18n: error installing yt-dlp or ffmpeg; {detail} = the technical detail, in English
        E::BadRelease(detail) => fmt(
            t("its release information is not as expected: {detail}"),
            &[("detail", detail)],
        ),
        E::Download(text) => text.clone(),
        E::Adapter(e) => adapter(e),
        // i18n: error: a program chosen as yt-dlp or ffmpeg didn't answer as one; {name} = yt-dlp or ffmpeg
        E::NotTheProgram(name) => fmt(t("this program isn't {name}"), &[("name", name)]),
        E::TooOld {
            name,
            version,
            oldest,
        } => fmt(
            // i18n: error: a program chosen as a helper is too old; {name} = gallery-dl, {version} = its version, {oldest} = the oldest one MDM can use
            t("{name} {version} is too old: MDM needs {oldest} or newer"),
            &[("name", name), ("version", version), ("oldest", oldest)],
        ),
    }
}

/// A network error in words a user can act on, with Windows' own description of the cause
/// (Windows words it in its display language) but not the address, which the window shows.
fn http(e: &reqwest::Error) -> String {
    let what = if e.is_timeout() {
        t("the server did not answer in time")
    } else if e.is_connect() {
        t("cannot connect to the server")
    } else if e.is_body() || e.is_decode() {
        t("the connection was lost while receiving data")
    } else if e.is_redirect() {
        t("too many redirects")
    } else {
        t("network error")
    };
    let mut cause = None;
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        cause = Some(s.to_string());
        source = s.source();
    }
    match cause.map(|c| os_text(&c)) {
        Some(c) if !c.is_empty() && !c.contains("://") => fmt(
            t("{problem} ({cause})"),
            &[("problem", what), ("cause", &c)],
        ),
        _ => what.to_owned(),
    }
}

/// An OS message without its "(os error N)" tail and final period.
fn os_text(s: &str) -> String {
    const TAIL: &str = " (os error "; // not translated: Rust's own wording, parsed
    let s = match s.rfind(TAIL) {
        Some(i) if s.ends_with(')') => &s[..i],
        _ => s,
    };
    s.trim().trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_their_data() {
        assert!(describe(&Error::Status(reqwest::StatusCode::NOT_FOUND)).contains("404"));
        let quota = Error::Refused(Refusal::Drive {
            message: "Too many users".into(),
        });
        assert!(describe(&quota).contains("Too many users"));
        assert_eq!(
            os_text("Access is denied. (os error 5)"),
            "Access is denied"
        );
    }
}
