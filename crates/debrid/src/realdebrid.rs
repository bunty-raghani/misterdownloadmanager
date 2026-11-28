//! Real-Debrid's REST API 1.0 and its OAuth v2 device flow, as documented at
//! <https://api.real-debrid.com/> (read 2026-11-04). Only what MDM uses: the account, the
//! supported hosts, unrestricting a link, and signing in.
//!
//! - Every call carries `Authorization: Bearer <token>` (never `?auth_token=`, which ends up in
//!   logs).
//! - Errors are HTTP 4xx/5xx with `{"error": "...", "error_code": N}` (`fault`).
//! - `remote` is always 0: MDM is a desktop program, and remote traffic is for servers.

use serde::Deserialize;
use url::Url;

use crate::{Fault, Http};

/// The client id Real-Debrid gives open-source programs for the device flow ("Opensource Apps").
pub const OPEN_SOURCE_CLIENT_ID: &str = "X245A4XAIBGVM";
/// The grant the device flow and refreshes use.
const DEVICE_GRANT: &str = "http://oauth.net/grant_type/device/1.0";

/// Where Real-Debrid answers (tests: the lab server's fake, `Endpoints::at`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub rest: Url,
    pub oauth: Url,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            rest: Url::parse("https://api.real-debrid.com/rest/1.0/").expect("constant"),
            oauth: Url::parse("https://api.real-debrid.com/oauth/v2/").expect("constant"),
        }
    }
}

impl Endpoints {
    /// The lab server's fake Real-Debrid at `base` (`http://127.0.0.1:PORT/`).
    pub fn at(base: &Url) -> Self {
        Endpoints {
            rest: base.join("rd/rest/1.0/").expect("relative"),
            oauth: base.join("rd/oauth/v2/").expect("relative"),
        }
    }
}

/// `GET /user`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct User {
    pub username: String,
    /// "premium" or "free".
    #[serde(rename = "type")]
    pub kind: String,
    /// Premium seconds left.
    #[serde(default)]
    pub premium: i64,
}

/// `POST /unrestrict/link`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Unrestricted {
    pub filename: Option<String>,
    /// 0: unknown.
    #[serde(default)]
    pub filesize: i64,
    /// "Max Chunks allowed".
    #[serde(default)]
    pub chunks: i64,
    /// The direct link.
    pub download: Option<String>,
}

/// `GET oauth/v2/device/code`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub interval: u64,
    pub expires_in: u64,
    pub verification_url: String,
}

/// `GET oauth/v2/device/credentials`: the user-bound credentials, once the user allowed MDM.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DeviceCredentials {
    pub client_id: String,
    pub client_secret: String,
}

/// `POST oauth/v2/token`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Token {
    pub access_token: String,
    pub expires_in: u64,
    pub refresh_token: String,
}

/// An error answer: `{"error": "...", "error_code": N}` (the code is optional).
#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    error_code: Option<i64>,
}

/// What an error answer means for MDM (the API documentation's list of codes).
pub(crate) fn fault(status: u16, body: &[u8], retry_after: Option<u64>) -> Fault {
    use mdm_engine::ServiceFault as F;
    let e: ErrorBody = serde_json::from_slice(body).unwrap_or(ErrorBody {
        error: String::new(),
        error_code: None,
    });
    let fault = match (e.error_code, status) {
        (Some(8), _) | (None, 401) => F::SignIn,
        (Some(9 | 14 | 15), _) => F::AccountBlocked,
        (Some(20), _) => F::NotPremium,
        (Some(16), _) => return Fault::Unsupported,
        (Some(17 | 19), _) => F::HostDown,
        (Some(18 | 23 | 36), _) => F::LimitReached,
        (Some(21), _) => F::TooManyActive,
        (Some(22), _) => F::IpRefused,
        (Some(24), _) | (None, 503) => F::FileUnavailable,
        (Some(28 | 35), _) => F::FileRefused,
        (Some(5 | 34), _) | (None, 429) => {
            F::TooManyRequests(retry_after.map(std::time::Duration::from_secs))
        }
        (Some(25 | -1), _) => F::ServiceDown,
        (None, 403) => F::AccountBlocked,
        (None, s) if s >= 500 => F::ServiceDown,
        _ if !e.error.is_empty() => F::Other(e.error),
        _ => F::Other(format!("HTTP {status}")),
    };
    Fault::Service(fault)
}

/// The account: name, premium or not.
pub(crate) async fn user(http: &Http, token: &str) -> Result<User, Fault> {
    let url = http.services.rd.rest.join("user").expect("relative");
    http.json(http.get(&url).bearer_auth(token), fault).await
}

/// The domains Real-Debrid supports (no sign-in needed).
pub(crate) async fn domains(http: &Http) -> Result<Vec<String>, Fault> {
    let url = http
        .services
        .rd
        .rest
        .join("hosts/domains")
        .expect("relative");
    http.json(http.get(&url), fault).await
}

/// Today's direct link for `link`.
pub(crate) async fn unrestrict(
    http: &Http,
    token: &str,
    link: &Url,
) -> Result<Unrestricted, Fault> {
    let url = http
        .services
        .rd
        .rest
        .join("unrestrict/link")
        .expect("relative");
    let form = [("link", link.as_str()), ("remote", "0")];
    http.json(http.post_form(&url, &form).bearer_auth(token), fault)
        .await
}

/// Sign-in by code, step 1: the code the user types at Real-Debrid.
pub async fn device_code(http: &Http) -> Result<DeviceCode, Fault> {
    let mut url = http
        .services
        .rd
        .oauth
        .join("device/code")
        .expect("relative");
    url.query_pairs_mut()
        .append_pair("client_id", OPEN_SOURCE_CLIENT_ID)
        .append_pair("new_credentials", "yes");
    http.json(http.get(&url), fault).await
}

/// Step 2, asked every `interval` seconds: the user-bound credentials once the user allowed
/// MDM, None while waiting.
pub(crate) async fn device_credentials(
    http: &Http,
    device_code: &str,
) -> Result<Option<DeviceCredentials>, Fault> {
    let mut url = http
        .services
        .rd
        .oauth
        .join("device/credentials")
        .expect("relative");
    url.query_pairs_mut()
        .append_pair("client_id", OPEN_SOURCE_CLIENT_ID)
        .append_pair("code", device_code);
    match http.json(http.get(&url), fault).await {
        Ok(c) => Ok(Some(c)),
        // Not allowed yet: Real-Debrid answers with an error until then (its wording isn't
        // documented); a service that is down or asks to slow down is said as such.
        Err(Fault::Service(
            mdm_engine::ServiceFault::ServiceDown | mdm_engine::ServiceFault::TooManyRequests(_),
        )) => Err(Fault::Service(mdm_engine::ServiceFault::ServiceDown)),
        Err(Fault::Service(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Step 3, and every refresh: an access token for the user-bound client. `code` is the device
/// code the first time, then the refresh token.
pub(crate) async fn token(
    http: &Http,
    client_id: &str,
    client_secret: &str,
    code: &str,
) -> Result<Token, Fault> {
    let url = http.services.rd.oauth.join("token").expect("relative");
    let form = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", DEVICE_GRANT),
    ];
    http.json(http.post_form(&url, &form), fault).await
}

/// `POST torrents/addMagnet` (201): the torrent's id.
pub(crate) async fn add_magnet(http: &Http, token: &str, magnet: &str) -> Result<String, Fault> {
    #[derive(Deserialize)]
    struct Added {
        id: String,
    }
    let url = http
        .services
        .rd
        .rest
        .join("torrents/addMagnet")
        .expect("relative");
    let a: Added = http
        .json(
            http.post_form(&url, &[("magnet", magnet)])
                .bearer_auth(token),
            fault,
        )
        .await?;
    Ok(a.id)
}

/// `GET torrents/info/{id}`; a torrent waiting for its files to be chosen gets the ones ticked in
/// Add torrent, `chosen` (`POST torrents/selectFiles/{id}`, `files` their ids comma-separated;
/// None: `all`). A ticked file it doesn't list: nothing is selected.
pub(crate) async fn torrent_status(
    http: &Http,
    token: &str,
    id: &str,
    chosen: Option<&[String]>,
) -> Result<crate::torrents::Status, Fault> {
    let id = crate::plain_id(id)?;
    let rest = &http.services.rd.rest;
    let url = rest.join(&format!("torrents/info/{id}")).expect("relative");
    let info: crate::torrents::RdInfo = http.json(http.get(&url).bearer_auth(token), fault).await?;
    if info.status == "waiting_files_selection" {
        let files = crate::torrents::rd_selection(&info, chosen)
            .map_err(|p| Fault::Service(mdm_engine::ServiceFault::ChosenFileMissing(p)))?;
        let url = rest
            .join(&format!("torrents/selectFiles/{id}"))
            .expect("relative");
        http.empty(
            http.post_form(&url, &[("files", files.as_str())])
                .bearer_auth(token),
            fault,
        )
        .await?;
        return Ok(crate::torrents::Status::Working {
            progress: Some(0.0),
            what: "files chosen".into(),
        });
    }
    Ok(crate::torrents::rd_status(info))
}

/// `DELETE torrents/delete/{id}` (204).
pub(crate) async fn torrent_delete(http: &Http, token: &str, id: &str) -> Result<(), Fault> {
    let id = crate::plain_id(id)?;
    let url = http
        .services
        .rd
        .rest
        .join(&format!("torrents/delete/{id}"))
        .expect("relative");
    http.empty(http.delete(&url).bearer_auth(token), fault)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_engine::ServiceFault as F;

    #[test]
    fn documented_errors_mean_what_they_say() {
        let f = |status, body: &str| fault(status, body.as_bytes(), None);
        assert_eq!(
            f(401, r#"{"error":"bad_token","error_code":8}"#),
            Fault::Service(F::SignIn)
        );
        assert_eq!(
            f(403, r#"{"error":"x","error_code":20}"#),
            Fault::Service(F::NotPremium)
        );
        assert_eq!(
            f(503, r#"{"error":"hoster_unsupported","error_code":16}"#),
            Fault::Unsupported
        );
        assert_eq!(
            f(503, r#"{"error":"unavailable_file","error_code":24}"#),
            Fault::Service(F::FileUnavailable)
        );
        assert_eq!(
            f(503, r#"{"error":"hoster_unavailable","error_code":19}"#),
            Fault::Service(F::HostDown)
        );
        assert_eq!(
            f(403, r#"{"error":"traffic_exhausted","error_code":23}"#),
            Fault::Service(F::LimitReached)
        );
        assert_eq!(
            f(403, r#"{"error":"ip_not_allowed","error_code":22}"#),
            Fault::Service(F::IpRefused)
        );
        assert_eq!(
            fault(
                429,
                br#"{"error":"too_many_requests","error_code":34}"#,
                Some(30)
            ),
            Fault::Service(F::TooManyRequests(Some(std::time::Duration::from_secs(30))))
        );
        assert_eq!(f(500, "not json"), Fault::Service(F::ServiceDown));
        assert_eq!(
            f(400, r#"{"error":"something new"}"#),
            Fault::Service(F::Other("something new".into()))
        );
    }

    #[test]
    fn the_documented_unrestrict_answer_reads() {
        // The shape of the documentation's example.
        let body = r#"{"id":"ABC","filename":"x.rar","mimeType":"application/x-rar","filesize":1048576,
            "link":"https://host/file/x","host":"host.com","chunks":16,"crc":1,
            "download":"https://abc.download.real-debrid.com/d/ABC/x.rar","streamable":0}"#;
        let u: Unrestricted = serde_json::from_str(body).unwrap();
        assert_eq!(u.filename.as_deref(), Some("x.rar"));
        assert_eq!((u.filesize, u.chunks), (1048576, 16));
        assert!(u.download.unwrap().starts_with("https://abc.download"));
    }
}
