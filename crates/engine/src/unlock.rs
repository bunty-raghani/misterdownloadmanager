//! Downloads through a service account (#44, debrid services such as Real-Debrid): a file host's
//! link is turned, through the service's API and the user's own account, into a direct link the
//! engine then downloads as any file (every connection, resume). The engine knows nothing of
//! accounts: the app gives a download an [`Unlocker`] (`Options::unlocker`), asked before the
//! first request, and again when the direct link stops working (it lives minutes or hours).
//!
//! The direct link is never shown and never leaves the download: the list, "Copy address" and
//! the event log keep the host's link (the event log names only the direct link's server).

use std::time::Duration;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;
use url::Url;

/// A service that turns a link into the address the file actually comes from, with the user's
/// account.
pub trait Unlocker: Send + Sync + std::fmt::Debug {
    /// Who unlocks, for the event log and errors: "Real-Debrid".
    fn name(&self) -> &str;

    /// Today's direct address for `url`. `Ok(None)`: not for this service after all (it doesn't
    /// support the host): the engine downloads `url` itself. `note` reports waits to the event log,
    /// with how far the service is (0 to 1) when it says: the list shows it while the link waits.
    fn unlock<'a>(
        &'a self,
        url: &'a Url,
        note: &'a (dyn Fn(String, Option<f32>) + Send + Sync),
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Unlocked>, ServiceFault>>;

    /// The event log's line for a link it gave (never the link itself).
    fn noted(&self, u: &Unlocked) -> String {
        note_unlocked(self.name(), u)
    }
}

/// A service's direct link for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unlocked {
    /// The direct link: never shown, never stored outside the resume state.
    pub url: Url,
    /// The service's name for the file (used when the direct link names none).
    pub file_name: Option<String>,
    /// Its size, when the service knows it.
    pub size: Option<u64>,
    /// The most connections the service allows on it (Real-Debrid's `chunks`).
    pub max_connections: Option<u32>,
    /// Headers the direct link wants on every request (a helper's: yt-dlp's `http_headers` and
    /// cookies for a stream, #769). A service's links want none: the host's cookies and logins
    /// never go to them.
    pub headers: reqwest::header::HeaderMap,
}

/// Why a service didn't give the file (`Refusal::Service`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceFault {
    /// The account's sign-in no longer works: sign in again.
    SignIn,
    /// The service wants the user to allow this device first (an email).
    NeedsApproval,
    /// The account is locked, banned or not activated.
    AccountBlocked,
    /// This needs a premium account (or the premium time is over).
    NotPremium,
    /// The service can't reach the file host now.
    HostDown,
    /// The account's limit for this host (or its traffic) is used up for now.
    LimitReached,
    /// Too many downloads at once through the account.
    TooManyActive,
    /// The service refuses this computer's address (a VPN or a server).
    IpRefused,
    /// The file is gone or unavailable on the host.
    FileUnavailable,
    /// The service won't give this file (infringing, not allowed).
    FileRefused,
    /// The file is password-protected.
    PasswordNeeded,
    /// Too many requests: slow down (for this long, when the service says).
    TooManyRequests(Option<Duration>),
    /// The service isn't answering or fails.
    ServiceDown,
    /// A torrent the service can't get (#44): nobody shares it any more, or its magnet link
    /// can't be read.
    TorrentUnavailable,
    /// A torrent's file whose name can't be written safely in its folder (it climbs out of it,
    /// `../`): nothing is written.
    UnsafeName(String),
    /// Anything else, in the service's own words.
    Other(String),
}

impl ServiceFault {
    /// Waiting may help: how long to wait before trying again.
    pub fn retry_wait(&self) -> Option<Duration> {
        match self {
            ServiceFault::TooManyRequests(d) => Some(d.unwrap_or(Duration::from_secs(60))),
            ServiceFault::HostDown | ServiceFault::ServiceDown => Some(Duration::from_secs(300)),
            ServiceFault::TooManyActive => Some(Duration::from_secs(120)),
            _ => None,
        }
    }
}

impl std::fmt::Display for ServiceFault {
    /// English, for the command line and the event log (the app words it in its language).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ServiceFault::SignIn => "sign in again",
            ServiceFault::NeedsApproval => "allow this device first (the service sent an email)",
            ServiceFault::AccountBlocked => "the account is locked or not activated",
            ServiceFault::NotPremium => "this needs a premium account",
            ServiceFault::HostDown => "the file host can't be reached through it right now",
            ServiceFault::LimitReached => "the limit for this file host is used up for now",
            ServiceFault::TooManyActive => "too many downloads at once",
            ServiceFault::IpRefused => "it refuses this connection (a VPN or server address?)",
            ServiceFault::FileUnavailable => "the file is gone or unavailable on the file host",
            ServiceFault::FileRefused => "it won't give this file",
            ServiceFault::PasswordNeeded => "the file is password-protected",
            ServiceFault::TooManyRequests(_) => "too many requests; slowing down",
            ServiceFault::ServiceDown => "it isn't responding",
            ServiceFault::TorrentUnavailable => {
                "the torrent can't be downloaded (nobody shares it, or its link can't be read)"
            }
            ServiceFault::UnsafeName(n) => {
                return write!(f, "a file's name can't be written safely: {n}");
            }
            ServiceFault::Other(m) => return f.write_str(m),
        };
        f.write_str(s)
    }
}

/// The event log's line for an unlock: the service, the name, the size; never the link.
pub(crate) fn note_unlocked(service: &str, u: &Unlocked) -> String {
    format!(
        "Unlocked through {service}: {} ({}), from {}",
        u.file_name.as_deref().unwrap_or("?"),
        u.size
            .map_or_else(|| "size unknown".to_owned(), crate::disk::human),
        u.url.host_str().unwrap_or("?"),
    )
}
