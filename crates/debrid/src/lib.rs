//! Debrid services (#44): the user's own account at a service such as Real-Debrid turns a file
//! host's link into a direct link, which MDM's engine then downloads as any file (every
//! connection, resume). Only the services' official, documented APIs, with the user's own
//! account; MDM ships no host lists or link sources, and never shows or copies the direct links
//! (the services forbid sharing them).
//!
//! The engine knows nothing of accounts: the app asks [`Debrid::unlocker_for`] for each download
//! (`mdm_engine::Options::unlocker`). Secrets (tokens) are kept by the app, in the system's
//! keychain; here they are only in memory, and never written to a log.
//!
//! The services: Real-Debrid (`realdebrid`), AllDebrid (`alldebrid`), Premiumize (`premiumize`)
//! and TorBox (`torbox`), each from its own public API documentation, on one interface
//! (`ServiceKind`).

pub mod alldebrid;
pub mod premiumize;
pub mod realdebrid;
pub mod torbox;
pub mod torrents;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use mdm_engine::{ServiceFault, Unlocked};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

/// The services MDM knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceKind {
    #[default]
    RealDebrid,
    AllDebrid,
    Premiumize,
    TorBox,
}

impl ServiceKind {
    /// Every service, in the order Options lists them: the most asked for first. Add an account
    /// starts on the first.
    pub const ALL: [ServiceKind; 4] = [
        ServiceKind::TorBox,
        ServiceKind::RealDebrid,
        ServiceKind::AllDebrid,
        ServiceKind::Premiumize,
    ];

    /// The service's name, as it writes it (not translated).
    pub fn name(self) -> &'static str {
        match self {
            ServiceKind::RealDebrid => "Real-Debrid",
            ServiceKind::AllDebrid => "AllDebrid",
            ServiceKind::Premiumize => "Premiumize",
            ServiceKind::TorBox => "TorBox",
        }
    }

    /// The service's page where a user finds their API token (key).
    pub fn token_page(self) -> &'static str {
        match self {
            ServiceKind::RealDebrid => "https://real-debrid.com/apitoken",
            ServiceKind::AllDebrid => "https://alldebrid.com/apikeys",
            ServiceKind::Premiumize => "https://www.premiumize.me/account",
            ServiceKind::TorBox => "https://torbox.app/settings",
        }
    }

    /// Whether it signs in by code in the browser (Real-Debrid's device flow, AllDebrid's PIN,
    /// TorBox's device code). Premiumize's needs a client registered by MDM's maintainer: its API
    /// key is pasted.
    pub fn signs_in_by_code(self) -> bool {
        !matches!(self, ServiceKind::Premiumize)
    }

    /// The requests a minute MDM allows itself (each service's published limit, halved or more:
    /// Real-Debrid 250 a minute, AllDebrid 600; Premiumize publishes none). TorBox: 300 a minute
    /// for each endpoint, per API key, and 60 new web downloads (or uncached torrents) an hour
    /// (https://support.torbox.app/en/articles/13726368-api-rate-limits); a refused request
    /// pauses it as any "too many requests" does.
    fn per_minute(self) -> usize {
        match self {
            ServiceKind::RealDebrid => 120,
            ServiceKind::AllDebrid => 300,
            ServiceKind::Premiumize => 60,
            ServiceKind::TorBox => 120,
        }
    }
}

/// Where the services answer (tests: the lab server's fakes, [`Services::at`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Services {
    pub rd: realdebrid::Endpoints,
    pub ad: Url,
    pub pm: Url,
    pub tb: Url,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            rd: realdebrid::Endpoints::default(),
            ad: Url::parse(alldebrid::BASE).expect("constant"),
            pm: Url::parse(premiumize::BASE).expect("constant"),
            tb: Url::parse(torbox::BASE).expect("constant"),
        }
    }
}

impl Services {
    /// The lab server's fakes at `base` (`http://127.0.0.1:PORT/`).
    pub fn at(base: &Url) -> Self {
        Services {
            rd: realdebrid::Endpoints::at(base),
            ad: base.join("ad/").expect("relative"),
            pm: base.join("pm/api/").expect("relative"),
            tb: base.join("tb/v1/api/").expect("relative"),
        }
    }
}

/// How an account signs in: what the app keeps sealed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Credential {
    /// A token the user pasted (Real-Debrid's personal API token).
    Token { token: String },
    /// The device flow's user-bound client and its refresh token (Real-Debrid).
    Oauth {
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
}

/// Never print a secret, even in a debug dump.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Token { .. } => f.write_str("Token(…)"),
            Credential::Oauth { .. } => f.write_str("Oauth(…)"),
        }
    }
}

/// An account as the app gives it: its id (stable), service and credential.
#[derive(Clone, Debug)]
pub struct Account {
    pub id: u64,
    pub service: ServiceKind,
    pub credential: Credential,
}

/// What the service says about the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    pub user: String,
    pub premium: bool,
    /// Premium time left.
    pub premium_left: Option<Duration>,
}

/// Why a call failed: the service's word, or "not a host this service takes".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Service(ServiceFault),
    /// The service doesn't support this file host: download the link directly.
    Unsupported,
}

impl From<ServiceFault> for Fault {
    fn from(f: ServiceFault) -> Self {
        Fault::Service(f)
    }
}

impl Fault {
    fn service(self) -> ServiceFault {
        match self {
            Fault::Service(f) => f,
            Fault::Unsupported => ServiceFault::Other("this file host isn't supported".into()),
        }
    }
}

/// Requests to the services: through the user's proxy, DNS and network adapter (`net`, the
/// engine's own client), each service at most its `per_minute` budget, and none for a while
/// after it said "too many requests" (refused requests count, and abuse gets a block).
pub struct Http {
    client: reqwest::Client,
    pub(crate) services: Services,
    budgets: Mutex<HashMap<ServiceKind, Budget>>,
}

#[derive(Default)]
struct Budget {
    sent: std::collections::VecDeque<Instant>,
    paused_until: Option<Instant>,
}

impl Http {
    /// `net`: the downloads' options (proxy, DNS, adapter); `services`: where they answer.
    pub fn new(net: &mdm_engine::Options, services: Services) -> Result<Self, String> {
        let client = mdm_engine::http_client(net, &services.rd.rest).map_err(|e| e.to_string())?;
        Ok(Http {
            client,
            services,
            budgets: Mutex::default(),
        })
    }

    /// Wait for `kind`'s budget to allow one more request.
    async fn slot(&self, kind: ServiceKind) -> Result<(), Fault> {
        loop {
            let wait = {
                let mut all = self.budgets.lock().unwrap();
                let b = all.entry(kind).or_default();
                let now = Instant::now();
                if let Some(until) = b.paused_until.filter(|u| *u > now) {
                    // Asked to slow down: no request until then.
                    return Err(Fault::Service(ServiceFault::TooManyRequests(Some(
                        until - now,
                    ))));
                }
                while b
                    .sent
                    .front()
                    .is_some_and(|t| now - *t >= Duration::from_secs(60))
                {
                    b.sent.pop_front();
                }
                if b.sent.len() < kind.per_minute() {
                    b.sent.push_back(now);
                    return Ok(());
                }
                Duration::from_secs(60) - (now - b.sent[0])
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// `kind` said "too many requests": nothing more to it for that long (a minute if unsaid).
    pub(crate) fn pause(&self, kind: ServiceKind, f: &Fault) {
        if let Fault::Service(ServiceFault::TooManyRequests(d)) = f {
            self.budgets
                .lock()
                .unwrap()
                .entry(kind)
                .or_default()
                .paused_until = Some(Instant::now() + d.unwrap_or(Duration::from_secs(60)));
        }
    }

    fn get(&self, url: &Url) -> reqwest::RequestBuilder {
        self.client
            .get(url.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
    }

    fn delete(&self, url: &Url) -> reqwest::RequestBuilder {
        self.client
            .delete(url.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
    }

    /// Real-Debrid: send `req`, whose answer has no body (204); an error answer through `fault`.
    async fn empty(
        &self,
        req: reqwest::RequestBuilder,
        fault: fn(u16, &[u8], Option<u64>) -> Fault,
    ) -> Result<(), Fault> {
        let (status, body, retry_after) = self.send(ServiceKind::RealDebrid, req).await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        let f = fault(status, &body, retry_after);
        self.pause(ServiceKind::RealDebrid, &f);
        Err(f)
    }

    fn post_json(&self, url: &Url, body: &serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .post(url.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
    }

    fn post_form(&self, url: &Url, form: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        self.client
            .post(url.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
    }

    /// Send `req` to `kind` within its budget: the HTTP status, the body and `Retry-After`.
    async fn send(
        &self,
        kind: ServiceKind,
        req: reqwest::RequestBuilder,
    ) -> Result<(u16, bytes::Bytes, Option<u64>), Fault> {
        self.slot(kind).await?;
        let resp = req
            .send()
            .await
            .map_err(|_| Fault::Service(ServiceFault::ServiceDown))?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let body = resp
            .bytes()
            .await
            .map_err(|_| Fault::Service(ServiceFault::ServiceDown))?;
        if status == 429 {
            self.pause(
                kind,
                &Fault::Service(ServiceFault::TooManyRequests(
                    retry_after.map(Duration::from_secs),
                )),
            );
        }
        Ok((status, body, retry_after))
    }

    /// Real-Debrid: send `req` and read its JSON answer; an error answer through `fault`.
    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        fault: fn(u16, &[u8], Option<u64>) -> Fault,
    ) -> Result<T, Fault> {
        let (status, body, retry_after) = self.send(ServiceKind::RealDebrid, req).await?;
        if (200..300).contains(&status) {
            return serde_json::from_slice(&body).map_err(|_| {
                Fault::Service(ServiceFault::Other("an answer MDM can't read".into()))
            });
        }
        let f = fault(status, &body, retry_after);
        self.pause(ServiceKind::RealDebrid, &f);
        Err(f)
    }
}

const USER_AGENT: &str = concat!("MisterDownloadManager/", env!("CARGO_PKG_VERSION"));

/// One account, signed in: its access token in memory (Real-Debrid's device flow gives tokens
/// that expire; a pasted token doesn't).
struct Session {
    account: Account,
    http: Arc<Http>,
    access: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl Session {
    /// A token to call with: the pasted one, or the device flow's access token, refreshed five
    /// minutes before it expires (or when `stale`, after a sign-in error).
    async fn token(&self, stale: bool) -> Result<String, Fault> {
        match &self.account.credential {
            Credential::Token { token } => Ok(token.clone()),
            Credential::Oauth {
                client_id,
                client_secret,
                refresh_token,
            } => {
                let mut access = self.access.lock().await;
                if let Some((t, until)) = access.as_ref()
                    && !stale
                    && *until > Instant::now()
                {
                    return Ok(t.clone());
                }
                let t =
                    realdebrid::token(&self.http, client_id, client_secret, refresh_token).await?;
                let until =
                    Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(300).max(60));
                *access = Some((t.access_token.clone(), until));
                Ok(t.access_token)
            }
        }
    }

    /// `call` with a token; once more with a fresh one after a sign-in error.
    async fn with_token<T, F, Fut>(&self, call: F) -> Result<T, Fault>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, Fault>>,
    {
        match call(self.token(false).await?).await {
            Err(Fault::Service(ServiceFault::SignIn))
                if matches!(self.account.credential, Credential::Oauth { .. }) =>
            {
                call(self.token(true).await?).await
            }
            r => r,
        }
    }

    async fn unlock(
        &self,
        link: &Url,
        note: &(dyn Fn(String, Option<f32>) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Option<Unlocked>, Fault> {
        let not_a_file = || {
            Fault::Service(ServiceFault::Other(
                "the service gave no file for this link".into(),
            ))
        };
        match self.account.service {
            ServiceKind::AllDebrid => {
                let key = self.token(false).await?;
                let u = match alldebrid::unlock(&self.http, &key, link).await {
                    Ok(u) => u,
                    Err(Fault::Unsupported) => return Ok(None),
                    Err(e) => return Err(e),
                };
                // Some links take time to generate: ask every 5 s or more, up to 10 minutes,
                // saying so in the event log, and stop as soon as the download is stopped.
                let mut direct = u.link.clone();
                if let Some(id) = u.delayed {
                    let until = Instant::now() + Duration::from_secs(600);
                    loop {
                        let d = alldebrid::delayed(&self.http, &key, id).await?;
                        match d.status {
                            2 => {
                                direct = d.link.unwrap_or_default();
                                break;
                            }
                            3 => return Err(Fault::Service(ServiceFault::FileUnavailable)),
                            _ => {}
                        }
                        if Instant::now() >= until {
                            return Err(Fault::Service(ServiceFault::ServiceDown));
                        }
                        note(
                            format!(
                                "AllDebrid is preparing the link (about {} s)",
                                d.time_left.max(0)
                            ),
                            None,
                        );
                        let wait = Duration::from_secs((d.time_left.max(0) as u64 / 4).max(5));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = cancel.cancelled() => {
                                return Err(Fault::Service(ServiceFault::Other("cancelled".into())));
                            }
                        }
                    }
                }
                let url = Url::parse(&direct).map_err(|_| not_a_file())?;
                Ok(Some(Unlocked {
                    url,
                    file_name: u.filename.filter(|n| !n.trim().is_empty()),
                    size: (u.filesize > 0).then_some(u.filesize as u64),
                    max_connections: None,
                    headers: Default::default(),
                }))
            }
            ServiceKind::Premiumize => {
                let key = self.token(false).await?;
                let d = match premiumize::direct(&self.http, &key, link).await {
                    Ok(d) => d,
                    Err(Fault::Unsupported) => return Ok(None),
                    Err(e) => return Err(e),
                };
                // One file per host link; several are a folder (a later phase: the grabber).
                let [entry] = d.content.as_slice() else {
                    return Err(not_a_file());
                };
                let url = Url::parse(&entry.link).map_err(|_| not_a_file())?;
                let name = entry.path.rsplit('/').next().unwrap_or("").to_owned();
                Ok(Some(Unlocked {
                    url,
                    file_name: Some(name).filter(|n| !n.trim().is_empty()),
                    size: (entry.size > 0).then_some(entry.size as u64),
                    max_connections: None,
                    headers: Default::default(),
                }))
            }
            ServiceKind::TorBox => {
                use torbox::Kind;
                let key = self.token(false).await?;
                let id = match torbox::create(&self.http, &key, Kind::Web, "link", link.as_str())
                    .await
                {
                    Ok(id) => id,
                    Err(Fault::Unsupported) => return Ok(None),
                    Err(e) => return Err(e),
                };
                // TorBox fetches the file to its servers first (at once when it has it already):
                // asked every 5 s, up to an hour, said in the event log, stopped with the download.
                let until = Instant::now() + Duration::from_secs(3600);
                let mut said = String::new();
                let item = loop {
                    let i = torbox::item(&self.http, &key, Kind::Web, id).await?;
                    match torbox::state(&i) {
                        None => break i,
                        Some(torrents::Status::Failed(f)) => return Err(Fault::Service(f)),
                        Some(torrents::Status::Working { progress, what }) => {
                            let line = format!(
                                "TorBox is fetching the file: {what}, {}%",
                                (progress.unwrap_or(0.0) * 100.0) as u32 / 10 * 10
                            );
                            if line != said {
                                note(line.clone(), progress);
                                said = line;
                            }
                        }
                        Some(torrents::Status::Ready { .. }) => unreachable!("state() never says"),
                    }
                    if Instant::now() >= until {
                        return Err(Fault::Service(ServiceFault::ServiceDown));
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                        _ = cancel.cancelled() => {
                            return Err(Fault::Service(ServiceFault::Other("cancelled".into())));
                        }
                    }
                };
                // One file per host link; several are a folder (as Premiumize: not a file).
                let [file] = item.files.as_slice() else {
                    return Err(not_a_file());
                };
                let url = torbox::link(&self.http, &key, Kind::Web, id, file.id).await?;
                let name = if file.short_name.trim().is_empty() {
                    file.name.rsplit('/').next().unwrap_or("").to_owned()
                } else {
                    file.short_name.clone()
                };
                Ok(Some(Unlocked {
                    url,
                    file_name: Some(name).filter(|n| !n.trim().is_empty()),
                    size: (file.size > 0).then_some(file.size),
                    max_connections: None,
                    headers: Default::default(),
                }))
            }
            ServiceKind::RealDebrid => {
                let u = match self
                    .with_token(
                        |t| async move { realdebrid::unrestrict(&self.http, &t, link).await },
                    )
                    .await
                {
                    Ok(u) => u,
                    Err(Fault::Unsupported) => return Ok(None),
                    Err(e) => return Err(e),
                };
                // A multi-link answer (a video site) has no single `download`: not a file.
                let Some(url) = u.download.as_deref().and_then(|d| Url::parse(d).ok()) else {
                    return Err(Fault::Service(ServiceFault::Other(
                        "the service gave no file for this link".into(),
                    )));
                };
                Ok(Some(Unlocked {
                    url,
                    file_name: u.filename.filter(|n| !n.trim().is_empty()),
                    size: (u.filesize > 0).then_some(u.filesize as u64),
                    max_connections: (u.chunks >= 1).then_some(u.chunks as u32),
                    headers: Default::default(),
                }))
            }
        }
    }
}

/// A service's id for a torrent, checked before it goes into an address: letters, digits, `-`
/// and `_` only.
pub(crate) fn plain_id(id: &str) -> Result<&str, Fault> {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(id)
    } else {
        Err(Fault::Service(ServiceFault::Other(
            "the service gave an id MDM can't use".into(),
        )))
    }
}

/// Does `host` belong to one of `domains` (the domain itself or a subdomain of it; `www.` and
/// case ignored)? `notexample.com` is not `example.com`'s.
pub fn host_matches(host: &str, domains: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    domains.iter().any(|d| {
        let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
        let d = d.strip_prefix("www.").unwrap_or(&d);
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}

/// The app's handle: the accounts in priority order, each with its service's supported domains.
#[derive(Clone, Default)]
pub struct Debrid {
    sessions: Arc<RwLock<Vec<Arc<Session>>>>,
    domains: Arc<RwLock<HashMap<u64, Vec<String>>>>,
}

impl std::fmt::Debug for Debrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Debrid")
            .field("accounts", &self.sessions.read().unwrap().len())
            .finish()
    }
}

impl Debrid {
    /// Use these accounts (in this order), through `http`. Their host lists are kept for the
    /// accounts still there; `refresh_hosts` reads them again.
    pub fn set_accounts(&self, accounts: Vec<Account>, http: Arc<Http>) {
        let mut sessions = self.sessions.write().unwrap();
        let kept: Vec<Arc<Session>> = accounts
            .into_iter()
            .map(|account| {
                // The same account (service and credential) keeps its access token.
                sessions
                    .iter()
                    .find(|s| {
                        s.account.id == account.id
                            && s.account.credential == account.credential
                            && Arc::ptr_eq(&s.http, &http)
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        Arc::new(Session {
                            account,
                            http: http.clone(),
                            access: tokio::sync::Mutex::new(None),
                        })
                    })
            })
            .collect();
        let ids: Vec<u64> = kept.iter().map(|s| s.account.id).collect();
        self.domains
            .write()
            .unwrap()
            .retain(|id, _| ids.contains(id));
        *sessions = kept;
    }

    /// Read each account's service's supported domains. A service that doesn't answer leaves
    /// the others' lists read: the first failure is returned, with its service, once every
    /// account was asked.
    pub async fn refresh_hosts(&self) -> Result<(), (ServiceKind, Fault)> {
        let sessions = self.sessions.read().unwrap().clone();
        let mut first = None;
        for s in sessions {
            let list = match s.account.service {
                ServiceKind::RealDebrid => realdebrid::domains(&s.http).await,
                ServiceKind::AllDebrid => alldebrid::domains(&s.http).await,
                // Premiumize lists its services to signed-in accounts.
                ServiceKind::Premiumize => match s.token(false).await {
                    Ok(t) => premiumize::domains(&s.http, &t).await,
                    Err(e) => Err(e),
                },
                ServiceKind::TorBox => torbox::domains(&s.http).await,
            };
            match list {
                Ok(list) => {
                    self.domains.write().unwrap().insert(s.account.id, list);
                }
                Err(f) => {
                    first.get_or_insert((s.account.service, f));
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Whether the host lists are there for every account.
    pub fn hosts_known(&self) -> bool {
        let d = self.domains.read().unwrap();
        self.sessions
            .read()
            .unwrap()
            .iter()
            .all(|s| d.contains_key(&s.account.id))
    }

    /// The account that would take `url` (the first whose service supports its host).
    pub fn account_for(&self, url: &Url) -> Option<(u64, ServiceKind)> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str()?;
        let d = self.domains.read().unwrap();
        self.sessions.read().unwrap().iter().find_map(|s| {
            d.get(&s.account.id)
                .filter(|list| host_matches(host, list))
                .map(|_| (s.account.id, s.account.service))
        })
    }

    /// The unlocker for `url` (`mdm_engine::Options::unlocker`), when an account takes its host.
    pub fn unlocker_for(&self, url: &Url) -> Option<Arc<dyn mdm_engine::Unlocker>> {
        let (id, _) = self.account_for(url)?;
        let session = self
            .sessions
            .read()
            .unwrap()
            .iter()
            .find(|s| s.account.id == id)?
            .clone();
        Some(Arc::new(AccountUnlocker { session }))
    }

    /// A download of `url` through an account ended (done, or taken off MDM's list): on TorBox,
    /// its web downloads leave the account's list, as a torrent does once downloaded. The other
    /// services keep no list of unlocked links.
    pub async fn release(&self, url: &Url) -> Result<(), Fault> {
        let Some((id, ServiceKind::TorBox)) = self.account_for(url) else {
            return Ok(());
        };
        let Some(s) = self.session(id) else {
            return Ok(());
        };
        torbox::release(&s.http, &s.token(false).await?, url).await
    }

    /// Ask the service about account `id` (Options, after signing in).
    pub async fn account_info(&self, id: u64) -> Result<AccountInfo, Fault> {
        let s = self
            .sessions
            .read()
            .unwrap()
            .iter()
            .find(|s| s.account.id == id)
            .cloned()
            .ok_or(Fault::Service(ServiceFault::SignIn))?;
        account_info(&s).await
    }
}

async fn account_info(s: &Session) -> Result<AccountInfo, Fault> {
    match s.account.service {
        ServiceKind::RealDebrid => {
            let u = s
                .with_token(|t| async move { realdebrid::user(&s.http, &t).await })
                .await?;
            Ok(AccountInfo {
                premium: u.kind == "premium" && u.premium > 0,
                premium_left: (u.premium > 0).then(|| Duration::from_secs(u.premium as u64)),
                user: u.username,
            })
        }
        ServiceKind::AllDebrid => {
            let u = alldebrid::user(&s.http, &s.token(false).await?).await?;
            let left = u.premium_until - now_unix();
            Ok(AccountInfo {
                premium: u.is_premium,
                premium_left: (u.is_premium && left > 0).then(|| Duration::from_secs(left as u64)),
                user: u.username,
            })
        }
        ServiceKind::Premiumize => {
            let a = premiumize::account(&s.http, &s.token(false).await?).await?;
            let left = a.premium_until.map(|t| t - now_unix()).filter(|l| *l > 0);
            Ok(AccountInfo {
                premium: left.is_some(),
                premium_left: left.map(|l| Duration::from_secs(l as u64)),
                // Premiumize gives no user name: its customer ID stands for it.
                user: match a.customer_id {
                    serde_json::Value::String(c) => c,
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => String::new(),
                },
            })
        }
        ServiceKind::TorBox => {
            let u = torbox::user(&s.http, &s.token(false).await?).await?;
            let left = u
                .premium_expires_at
                .as_deref()
                .and_then(torbox::unix_of)
                .map(|t| t - now_unix())
                .filter(|l| *l > 0);
            Ok(AccountInfo {
                // Plan 0 is the free one.
                premium: u.plan > 0 && left.is_some(),
                premium_left: left.map(|l| Duration::from_secs(l as u64)),
                // TorBox gives no user name: the account's email stands for it.
                user: u.email,
            })
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Check a credential before keeping it: the account it signs in to.
pub async fn check(
    http: &Http,
    service: ServiceKind,
    credential: Credential,
) -> Result<AccountInfo, Fault> {
    let s = Session {
        account: Account {
            id: 0,
            service,
            credential,
        },
        http: Arc::new(Http {
            client: http.client.clone(),
            services: http.services.clone(),
            budgets: Mutex::default(),
        }),
        access: tokio::sync::Mutex::new(None),
    };
    account_info(&s).await
}

/// One account's unlocker for the engine.
#[derive(Clone)]
struct AccountUnlocker {
    session: Arc<Session>,
}

impl std::fmt::Debug for AccountUnlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AccountUnlocker({})",
            self.session.account.service.name()
        )
    }
}

impl mdm_engine::Unlocker for AccountUnlocker {
    fn name(&self) -> &str {
        self.session.account.service.name()
    }

    fn unlock<'a>(
        &'a self,
        url: &'a Url,
        note: &'a (dyn Fn(String, Option<f32>) + Send + Sync),
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Unlocked>, ServiceFault>> {
        Box::pin(async move {
            tokio::select! {
                r = self.session.unlock(url, note, cancel) => r.map_err(Fault::service),
                _ = cancel.cancelled() => Err(ServiceFault::Other("cancelled".into())),
            }
        })
    }
}

/// Sign-in by code: Real-Debrid's device flow, AllDebrid's PIN, TorBox's device code. The code to
/// show and the page to type it at, then a poll until the user allowed MDM there.
pub mod signin {
    use super::*;

    /// A code waiting for the user.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Code {
        /// What the user types.
        pub shown: String,
        /// Where (the page's address; AllDebrid's has the PIN in it already).
        pub page: String,
        /// Ask every this many seconds, until this many have gone by.
        pub interval: u64,
        pub expires_in: u64,
        inner: Inner,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Inner {
        Rd(realdebrid::DeviceCode),
        Ad(alldebrid::Pin),
        Tb(torbox::DeviceCode),
    }

    /// Step 1: a code from `service` (one that signs in by code).
    pub async fn start(http: &Http, service: ServiceKind) -> Result<Code, Fault> {
        match service {
            ServiceKind::RealDebrid => {
                let c = realdebrid::device_code(http).await?;
                own_page(service, &c.verification_url)?;
                Ok(Code {
                    shown: c.user_code.clone(),
                    page: c.verification_url.clone(),
                    interval: c.interval.max(1),
                    expires_in: c.expires_in,
                    inner: Inner::Rd(c),
                })
            }
            ServiceKind::AllDebrid => {
                let p = alldebrid::pin(http).await?;
                own_page(service, &p.user_url)?;
                Ok(Code {
                    shown: p.pin.clone(),
                    page: p.user_url.clone(),
                    // The docs: ask every 5 seconds or more.
                    interval: 5,
                    expires_in: p.expires_in,
                    inner: Inner::Ad(p),
                })
            }
            ServiceKind::TorBox => {
                let c = torbox::device_start(http).await?;
                let left = c
                    .expires_at
                    .as_deref()
                    .and_then(torbox::unix_of)
                    .map(|t| t - now_unix())
                    .filter(|l| *l > 0);
                own_page(service, &c.verification_url)?;
                Ok(Code {
                    shown: c.code.clone(),
                    page: c.verification_url.clone(),
                    // The docs: ask every `interval` seconds; 5 or more, as the others.
                    interval: c.interval.max(5),
                    expires_in: left.map_or(600, |l| l as u64),
                    inner: Inner::Tb(c),
                })
            }
            ServiceKind::Premiumize => Err(Fault::Service(ServiceFault::Other(
                "Premiumize signs in with its API key".into(),
            ))),
        }
    }

    /// The page MDM opens in the browser for the user to type the code at comes from the
    /// service's answer: it is opened only on the service's own site, over https. Anything else
    /// is a service that isn't answering as it should.
    fn own_page(service: ServiceKind, page: &str) -> Result<(), Fault> {
        let site = match service {
            ServiceKind::RealDebrid => "real-debrid.com",
            ServiceKind::AllDebrid => "alldebrid.com",
            ServiceKind::Premiumize => "premiumize.me",
            ServiceKind::TorBox => "torbox.app",
        };
        let ok = url::Url::parse(page).is_ok_and(|u| {
            u.scheme() == "https"
                && u.username().is_empty()
                && u.password().is_none()
                && u.port().is_none()
                && u.host_str()
                    .is_some_and(|h| h == site || h.ends_with(&format!(".{site}")))
        });
        if ok {
            Ok(())
        } else {
            Err(ServiceFault::ServiceDown.into())
        }
    }

    /// Step 2, every `interval` seconds until `expires_in`: the credential once allowed.
    pub async fn poll(http: &Http, code: &Code) -> Result<Option<Credential>, Fault> {
        match &code.inner {
            Inner::Rd(c) => {
                let Some(cr) = realdebrid::device_credentials(http, &c.device_code).await? else {
                    return Ok(None);
                };
                let t = realdebrid::token(http, &cr.client_id, &cr.client_secret, &c.device_code)
                    .await?;
                Ok(Some(Credential::Oauth {
                    client_id: cr.client_id,
                    client_secret: cr.client_secret,
                    refresh_token: t.refresh_token,
                }))
            }
            Inner::Ad(p) => Ok(alldebrid::pin_check(http, p)
                .await?
                .map(|token| Credential::Token { token })),
            Inner::Tb(c) => Ok(torbox::device_token(http, &c.device_code)
                .await?
                .map(|token| Credential::Token { token })),
        }
    }

    #[cfg(test)]
    mod page_tests {
        use super::{ServiceKind, own_page};

        #[test]
        fn only_the_services_own_site_over_https() {
            let rd = ServiceKind::RealDebrid;
            let ad = ServiceKind::AllDebrid;
            assert!(own_page(rd, "https://real-debrid.com/device").is_ok());
            assert!(own_page(rd, "https://www.real-debrid.com/device").is_ok());
            assert!(own_page(ad, "https://alldebrid.com/pin/?pin=ABCD").is_ok());
            for bad in [
                "http://real-debrid.com/device",
                "https://real-debrid.com.example.net/device",
                "https://notreal-debrid.com/device",
                "https://example.com/real-debrid.com",
                "https://user@real-debrid.com/device",
                "https://real-debrid.com:8443/device",
                "file:///C:/Windows/system32/calc.exe",
                "javascript:alert(1)",
                "real-debrid.com/device",
                "",
            ] {
                assert!(own_page(rd, bad).is_err(), "{bad}");
            }
            assert!(own_page(ad, "https://real-debrid.com/device").is_err());
            let tb = ServiceKind::TorBox;
            assert!(
                own_page(
                    tb,
                    "https://torbox.app/oauth/device?app=MisterDownloadManager"
                )
                .is_ok()
            );
            assert!(own_page(tb, "https://real-debrid.com/device").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_their_domains_and_subdomains_only() {
        let d = vec!["rapidgator.net".to_owned(), "WWW.1fichier.com".to_owned()];
        for yes in [
            "rapidgator.net",
            "www.rapidgator.net",
            "RG.rapidgator.NET",
            "1fichier.com",
            "a.1fichier.com",
        ] {
            assert!(host_matches(yes, &d), "{yes}");
        }
        for no in [
            "notrapidgator.net",
            "rapidgator.net.evil.com",
            "fichier.com",
            "",
        ] {
            assert!(!host_matches(no, &d), "{no}");
        }
    }

    #[test]
    fn a_credential_never_prints_its_secret() {
        let c = Credential::Oauth {
            client_id: "cid".into(),
            client_secret: "SECRET".into(),
            refresh_token: "REFRESH".into(),
        };
        let shown = format!(
            "{c:?} {:?}",
            Credential::Token {
                token: "TOKEN".into()
            }
        );
        assert!(
            !shown.contains("SECRET") && !shown.contains("REFRESH") && !shown.contains("TOKEN")
        );
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""kind":"oauth""#));
        assert_eq!(serde_json::from_str::<Credential>(&json).unwrap(), c);
    }
}
