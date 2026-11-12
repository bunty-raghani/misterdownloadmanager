//! TorBox's API, as documented at <https://api-docs.torbox.app/> and
//! <https://api.torbox.app/openapi.json> (read 2026-11-10). Only what MDM uses: the account, the
//! hosters list, and two kinds of its downloads (web downloads for file-host links, and
//! torrents), which go the same way: created, fetched on TorBox's servers (`mylist` until
//! finished), then each file's link asked (`requestdl`).
//!
//! - Every call carries `Authorization: Bearer <API key>`, except `requestdl`, whose documented
//!   sign-in is the `token` query parameter: MDM never logs or shows that address, and asks it
//!   with `redirect=false` so the API key never reaches the download's own address.
//! - Every answer is `{"success", "error", "detail", "data"}`; `error` is a code
//!   (`BAD_TOKEN`, `UNSUPPORTED_SITE`...). Note `ITEM_NOT_FOUND` comes with `success: true`.
//! - Sign-in by code (`user/auth/device/start`, then `user/auth/device/token` until allowed): the
//!   user types the code at TorBox's page (its short address is `tor.box/link`), and MDM gets the
//!   account's API key. Or the key is pasted from the account's settings page.

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::torrents::{RemoteFile, Status};
use crate::{Fault, Http, ServiceKind};

/// Where TorBox answers (tests: the lab server's `/tb/v1/api/`).
pub const BASE: &str = "https://api.torbox.app/v1/api/";

/// TorBox's kinds of downloads MDM uses: the same calls under their own path and id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Web,
    Torrent,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Web => "webdl",
            Kind::Torrent => "torrents",
        }
    }
    fn create(self) -> &'static str {
        match self {
            Kind::Web => "webdl/createwebdownload",
            Kind::Torrent => "torrents/createtorrent",
        }
    }
    /// The created download's id in `create`'s answer.
    fn created_id(self) -> &'static str {
        match self {
            Kind::Web => "webdownload_id",
            Kind::Torrent => "torrent_id",
        }
    }
    /// The id's name for `control…` (JSON) and for `requestdl` (query).
    fn control(self) -> (&'static str, &'static str) {
        match self {
            Kind::Web => ("webdl/controlwebdownload", "webdl_id"),
            Kind::Torrent => ("torrents/controltorrent", "torrent_id"),
        }
    }
    fn request_id(self) -> &'static str {
        match self {
            Kind::Web => "web_id",
            Kind::Torrent => "torrent_id",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    data: Value,
}

/// What an error code means for MDM (the codes the documentation's examples show).
pub(crate) fn fault_of(code: &str, detail: &str) -> Fault {
    use mdm_engine::ServiceFault as F;
    Fault::Service(match code {
        "NO_AUTH" | "BAD_TOKEN" | "AUTH_ERROR" => F::SignIn,
        "UNSUPPORTED_SITE" => return Fault::Unsupported,
        "TEMPORARILY_DISABLED" => F::HostDown,
        "PLAN_RESTRICTED_FEATURE" => F::NotPremium,
        "ACTIVE_LIMIT" => F::TooManyActive,
        "MONTHLY_LIMIT" | "COOLDOWN_LIMIT" | "DOWNLOAD_TOO_LARGE" => F::LimitReached,
        "ITEM_NOT_FOUND" | "LINK_OFFLINE" | "INVALID_LINK" => F::FileUnavailable,
        "BOZO_TORRENT" | "INVALID_HASH" => F::TorrentUnavailable,
        "DATABASE_ERROR" | "UNKNOWN_ERROR" | "DOWNLOAD_SERVER_ERROR" => F::ServiceDown,
        _ if !detail.is_empty() => F::Other(detail.to_owned()),
        _ => F::Other(code.to_owned()),
    })
}

/// An answer's `data`, or its error.
fn read(status: u16, body: &[u8], retry: Option<u64>) -> Result<Value, Fault> {
    use mdm_engine::ServiceFault as F;
    if status == 429 {
        return Err(Fault::Service(F::TooManyRequests(
            retry.map(std::time::Duration::from_secs),
        )));
    }
    let Ok(e) = serde_json::from_slice::<Envelope>(body) else {
        return Err(Fault::Service(if status >= 500 {
            F::ServiceDown
        } else {
            F::Other("an answer MDM can't read".into())
        }));
    };
    match e.error.filter(|c| !c.is_empty()) {
        Some(code) => Err(fault_of(&code, &e.detail)),
        None if !e.success => Err(Fault::Service(F::Other(e.detail))),
        None => Ok(e.data),
    }
}

fn data<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, Fault> {
    serde_json::from_value(v).map_err(|_| {
        Fault::Service(mdm_engine::ServiceFault::Other(
            "an answer MDM can't read".into(),
        ))
    })
}

async fn call(http: &Http, req: reqwest::RequestBuilder) -> Result<Value, Fault> {
    let (status, body, retry) = http.send(ServiceKind::TorBox, req).await?;
    read(status, &body, retry)
}

fn url(http: &Http, path: &str) -> Url {
    http.services.tb.join(path).expect("relative")
}

/// `GET user/me`.
#[derive(Debug, Deserialize)]
pub(crate) struct User {
    #[serde(default)]
    pub email: String,
    /// 0 is the free plan.
    #[serde(default)]
    pub plan: i64,
    /// RFC 3339.
    pub premium_expires_at: Option<String>,
}

pub(crate) async fn user(http: &Http, key: &str) -> Result<User, Fault> {
    data(call(http, http.get(&url(http, "user/me")).bearer_auth(key)).await?)
}

/// `GET user/auth/device/start?app=` (no sign-in): the code to show and where.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    /// What the user types (6 digits).
    pub code: String,
    /// The page that shows the app's name (on torbox.app; `tor.box/link`, TorBox's short
    /// address for it, is another domain and isn't opened).
    #[serde(default)]
    pub verification_url: String,
    #[serde(default)]
    pub interval: u64,
    /// RFC 3339 (the docs: 10 minutes).
    pub expires_at: Option<String>,
}

pub(crate) async fn device_start(http: &Http) -> Result<DeviceCode, Fault> {
    let mut u = url(http, "user/auth/device/start");
    // Letters and digits only, as the docs ask: shown on TorBox's page.
    u.query_pairs_mut()
        .append_pair("app", "MisterDownloadManager");
    data(call(http, http.get(&u)).await?)
}

/// `POST user/auth/device/token`: the API key once the user allowed MDM, None while not yet
/// (`DEVICE_CODE_NOT_USED`).
pub(crate) async fn device_token(http: &Http, device_code: &str) -> Result<Option<String>, Fault> {
    #[derive(Deserialize)]
    struct Token {
        access_token: String,
    }
    let req = http.post_json(
        &url(http, "user/auth/device/token"),
        &serde_json::json!({ "device_code": device_code }),
    );
    let (status, body, retry) = http.send(ServiceKind::TorBox, req).await?;
    let code = serde_json::from_slice::<Envelope>(&body)
        .ok()
        .and_then(|e| e.error);
    match code.as_deref() {
        Some("DEVICE_CODE_NOT_USED") => Ok(None),
        Some("ITEM_NOT_FOUND") => Err(Fault::Service(mdm_engine::ServiceFault::Other(
            "the code expired: sign in again".into(),
        ))),
        _ => data::<Token>(read(status, &body, retry)?).map(|t| Some(t.access_token)),
    }
}

/// Seconds since 1970 of an RFC 3339 time, as TorBox writes them.
pub(crate) fn unix_of(t: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.timestamp())
}

#[derive(Debug, Deserialize)]
struct Hoster {
    #[serde(default)]
    domains: Vec<String>,
}

/// `GET webdl/hosters` (public): every domain of every host and site TorBox takes, those it
/// marks down for now included (their links then fail with TorBox's reason, as with the others).
pub(crate) async fn domains(http: &Http) -> Result<Vec<String>, Fault> {
    let hosters: Vec<Hoster> = data(call(http, http.get(&url(http, "webdl/hosters"))).await?)?;
    Ok(hosters.into_iter().flat_map(|h| h.domains).collect())
}

/// `POST <kind>/create…` with `field` (`link`, `magnet`): its id at TorBox.
pub(crate) async fn create(
    http: &Http,
    key: &str,
    kind: Kind,
    field: &str,
    value: &str,
) -> Result<u64, Fault> {
    let d = call(
        http,
        http.post_form(&url(http, kind.create()), &[(field, value)])
            .bearer_auth(key),
    )
    .await?;
    d.get(kind.created_id())
        .and_then(Value::as_u64)
        .ok_or(Fault::Service(mdm_engine::ServiceFault::ServiceDown))
}

/// A `mylist` entry: the same for both kinds.
#[derive(Debug, Deserialize)]
pub(crate) struct Item {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    /// The link it was made from (a web download's file-host link).
    #[serde(default)]
    pub original_url: String,
    #[serde(default)]
    pub download_state: String,
    /// 0 to 1.
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub download_finished: bool,
    #[serde(default)]
    pub download_present: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub files: Vec<File>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct File {
    pub id: u64,
    /// Its path in the download, `/`-separated.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub short_name: String,
    #[serde(default)]
    pub size: u64,
}

/// `GET <kind>/mylist?id=` (fresh, not TorBox's cached list).
pub(crate) async fn item(http: &Http, key: &str, kind: Kind, id: u64) -> Result<Item, Fault> {
    let mut u = url(http, &format!("{}/mylist", kind.path()));
    u.query_pairs_mut()
        .append_pair("id", &id.to_string())
        .append_pair("bypass_cache", "true");
    let d = call(http, http.get(&u).bearer_auth(key)).await?;
    // One item for an id; a list of one when TorBox answers with its list form.
    match d {
        Value::Array(mut a) if !a.is_empty() => data(a.swap_remove(0)),
        Value::Object(_) => data(d),
        _ => Err(Fault::Service(mdm_engine::ServiceFault::FileUnavailable)),
    }
}

/// A download of `link` ended (done, or taken off MDM's list): its web downloads leave the
/// account's TorBox list, as a torrent does once downloaded. Found by the link they were made
/// from, so those made before a restart, or by a refreshed link, go too.
pub(crate) async fn release(http: &Http, key: &str, link: &Url) -> Result<(), Fault> {
    // shortcut: only the first 1000 entries (TorBox's default limit), page through them if
    // accounts hold more.
    let mut u = url(http, "webdl/mylist");
    u.query_pairs_mut().append_pair("bypass_cache", "true");
    let list: Vec<Item> = match call(http, http.get(&u).bearer_auth(key)).await {
        Ok(d) => data(d)?,
        // An empty list: nothing to remove.
        Err(Fault::Service(mdm_engine::ServiceFault::FileUnavailable)) => return Ok(()),
        Err(e) => return Err(e),
    };
    for i in list {
        if Url::parse(&i.original_url).is_ok_and(|o| o == *link) {
            delete(http, key, Kind::Web, i.id).await?;
        }
    }
    Ok(())
}

/// How TorBox is doing with `i`: still at it, failed, or finished (its files, not yet linked).
pub(crate) fn state(i: &Item) -> Option<Status> {
    let what = i.download_state.to_lowercase();
    if let Some(e) = i.error.as_deref().filter(|e| !e.trim().is_empty()) {
        return Some(Status::Failed(mdm_engine::ServiceFault::Other(
            e.to_owned(),
        )));
    }
    if what.contains("fail") || what == "error" {
        return Some(Status::Failed(mdm_engine::ServiceFault::Other(
            i.download_state.clone(),
        )));
    }
    if i.download_finished && i.download_present {
        return None;
    }
    Some(Status::Working {
        progress: Some(i.progress.clamp(0.0, 1.0) as f32),
        what: if what.is_empty() {
            "queued".into()
        } else {
            what
        },
    })
}

/// `GET <kind>/requestdl`: a file's direct link (good for 3 hours to start).
pub(crate) async fn link(
    http: &Http,
    key: &str,
    kind: Kind,
    id: u64,
    file: u64,
) -> Result<Url, Fault> {
    let mut u = url(http, &format!("{}/requestdl", kind.path()));
    u.query_pairs_mut()
        .append_pair("token", key)
        .append_pair(kind.request_id(), &id.to_string())
        .append_pair("file_id", &file.to_string())
        .append_pair("redirect", "false");
    let d = call(http, http.get(&u)).await?;
    d.as_str().and_then(|l| Url::parse(l).ok()).ok_or_else(|| {
        Fault::Service(mdm_engine::ServiceFault::Other(
            "the service gave a link MDM can't read".into(),
        ))
    })
}

/// A finished download's files, each with its direct link.
pub(crate) async fn files(
    http: &Http,
    key: &str,
    kind: Kind,
    id: u64,
    i: &Item,
) -> Result<Vec<RemoteFile>, Fault> {
    let mut out = Vec::with_capacity(i.files.len());
    for f in &i.files {
        out.push(RemoteFile {
            path: f.name.clone(),
            size: f.size,
            link: link(http, key, kind, id, f.id).await?.into(),
        });
    }
    Ok(out)
}

/// `POST <kind>/control…` `delete`: gone from the account's list.
pub(crate) async fn delete(http: &Http, key: &str, kind: Kind, id: u64) -> Result<(), Fault> {
    let (path, name) = kind.control();
    call(
        http,
        http.post_json(
            &url(http, path),
            &serde_json::json!({ name: id, "operation": "delete" }),
        )
        .bearer_auth(key),
    )
    .await
    .map(|_| ())
}

/// A torrent's id as the app keeps it (`Order::transfer`): its number.
pub(crate) fn torrent_id(t: &str) -> Result<u64, Fault> {
    t.parse().map_err(|_| {
        Fault::Service(mdm_engine::ServiceFault::Other(
            "the service gave an id MDM can't use".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_engine::ServiceFault as F;

    #[test]
    fn the_documented_answers_read() {
        let ok = br#"{"success":true,"error":null,"detail":"ok","data":{"hash":"h","webdownload_id":42,"auth_id":"a"}}"#;
        assert_eq!(read(200, ok, None).unwrap()["webdownload_id"], 42);
        let bad = br#"{"success":false,"error":"BAD_TOKEN","detail":"Your token is invalid","data":null}"#;
        assert_eq!(read(403, bad, None).unwrap_err(), Fault::Service(F::SignIn));
        let un =
            br#"{"success":false,"error":"UNSUPPORTED_SITE","detail":"not supported","data":null}"#;
        assert_eq!(read(400, un, None).unwrap_err(), Fault::Unsupported);
        // Documented with success: true, still an error.
        let none =
            br#"{"success":true,"error":"ITEM_NOT_FOUND","detail":"No web downloads","data":null}"#;
        assert_eq!(
            read(404, none, None).unwrap_err(),
            Fault::Service(F::FileUnavailable)
        );
        assert_eq!(
            fault_of("NEW_CODE", "Its words"),
            Fault::Service(F::Other("Its words".into()))
        );
        let h: Vec<Hoster> = serde_json::from_str(
            r#"[{"id":1,"name":"Rapidgator","domains":["rapidgator.net","rg.to"],"status":false,"type":"hoster"}]"#,
        )
        .unwrap();
        assert_eq!(h[0].domains, ["rapidgator.net", "rg.to"]);
        assert_eq!(unix_of("2051-02-18T04:08:43Z"), Some(2560306123));
    }

    #[test]
    fn list_states_mean_what_they_say() {
        let i = |state: &str, done: bool, error: Option<&str>| Item {
            id: 1,
            name: "x".into(),
            original_url: String::new(),
            download_state: state.into(),
            progress: 0.25,
            download_finished: done,
            download_present: done,
            error: error.map(Into::into),
            files: vec![],
        };
        assert_eq!(
            state(&i("downloading", false, None)),
            Some(Status::Working {
                progress: Some(0.25),
                what: "downloading".into()
            })
        );
        assert_eq!(state(&i("completed", true, None)), None);
        assert!(matches!(
            state(&i("failed (link dead)", false, None)),
            Some(Status::Failed(_))
        ));
        assert!(matches!(
            state(&i("downloading", false, Some("Host error"))),
            Some(Status::Failed(F::Other(_)))
        ));
        assert_eq!(torrent_id("12").unwrap(), 12);
        assert!(torrent_id("x/../1").is_err());
    }
}
