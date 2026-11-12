//! Resume state: `<file>.mdm.json` beside the part file (notes/ENGINE.md §3).
//!
//! Request headers (cookies, authorization) are deliberately not stored: whoever resumes the
//! download supplies them again, so no credentials sit in the download folder.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::planner::SavedSegment;

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeState {
    pub version: u32,
    /// The URL the user asked for.
    pub url: Url,
    /// The URL after redirects; segments are fetched from here.
    pub final_url: Url,
    /// The finished file's path (the part file is `naming::part_path(dest)`).
    pub dest: PathBuf,
    pub size: u64,
    /// What identifies the version of the file the data so far was fetched under.
    #[serde(default)]
    pub validators: Validators,
    /// Before `validators`: the one `If-Range` value used (read once, then migrated).
    #[serde(default, rename = "validator", skip_serializing)]
    legacy_validator: Option<String>,
    /// Bytes `[start, cursor)` of each segment are on disk.
    pub segments: Vec<SavedSegment>,
    /// The `Content-Type` the file was first served with (an HTML answer to a file that wasn't
    /// HTML is a sign-in wall, not data).
    #[serde(default)]
    pub content_type: Option<String>,
    /// The hash the finished file must have (the user's or the server's), if known.
    #[serde(default)]
    pub checksum: Option<crate::verify::Hash>,
    /// The first run got past a refusal by asking with `Options::agent_fallback` instead of the
    /// browser's User-Agent (#372): a resume asks the same way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asked_as_mdm: bool,
    /// A POST download (`Options::post`, its body not stored) whose redirects turned it into
    /// a GET (301, 302, 303): `final_url` is asked with GET.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub final_get: bool,
    /// Only the browser-like connection got past the site's bot check (#668): a resume goes on
    /// through it, on one connection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub browser_like: bool,
}

impl ResumeState {
    pub fn new(url: Url, final_url: Url, dest: PathBuf, size: u64, validators: Validators) -> Self {
        ResumeState {
            version: VERSION,
            url,
            final_url,
            dest,
            size,
            validators,
            legacy_validator: None,
            segments: Vec::new(),
            content_type: None,
            checksum: None,
            asked_as_mdm: false,
            final_get: false,
            browser_like: false,
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut s: ResumeState =
            serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(v) = s.legacy_validator.take()
            && s.validators == Validators::default()
        {
            s.validators = Validators::migrate(&v, s.final_url.host_str());
        }
        if s.version != VERSION {
            return Err(format!(
                "{}: unsupported version {}",
                path.display(),
                s.version
            ));
        }
        Ok(s)
    }

    /// Persist with `segments`. `part` is flushed first, so the saved positions never claim
    /// bytes that are not on disk yet — even across a power loss.
    pub fn save(
        &mut self,
        path: &Path,
        segments: Vec<SavedSegment>,
        part: &File,
    ) -> io::Result<()> {
        part.sync_data()?;
        self.segments = segments;
        self.write(path)
    }

    /// Write as is (its segments must already be on disk).
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        craft_atomic::atomic_write(path, &json)
    }

    /// Bytes already on disk.
    pub fn downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.cursor - s.start).sum()
    }
}

/// What identifies a version of the file, for `If-Range` (RFC 9110 §13.1.5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Validators {
    /// A strong ETag, and the host that issued it (CDN nodes may each have their own).
    pub etag: Option<String>,
    pub etag_host: Option<String>,
    /// Last-Modified, kept only when it is a strong validator (`Date` at least 1 s later).
    pub last_modified: Option<String>,
}

impl Validators {
    /// The validators a response offers. A weak or malformed ETag means none at all: the
    /// server says its bytes may differ between equivalent versions, and a date may then not
    /// stand in for the ETag.
    pub fn from_response(
        etag: Option<&str>,
        last_modified: Option<&str>,
        date: Option<&str>,
        host: Option<&str>,
    ) -> Self {
        let etag = match etag.map(str::trim) {
            Some(e) if is_strong_etag(e) => Some(e.to_owned()),
            Some(_) => return Validators::default(),
            None => None,
        };
        let last_modified = last_modified
            .filter(|lm| is_strong_date(lm, date))
            .map(str::to_owned);
        Validators {
            etag_host: etag.as_ref().and(host.map(str::to_owned)),
            etag,
            last_modified,
        }
    }

    /// The `If-Range` value for a request to `host`: the ETag only to the host that issued it,
    /// else the date.
    pub fn if_range(&self, host: Option<&str>) -> Option<&str> {
        self.etag
            .as_deref()
            .filter(|_| self.etag_host.as_deref() == host)
            .or(self.last_modified.as_deref())
    }

    /// From the single value older versions stored.
    fn migrate(v: &str, host: Option<&str>) -> Self {
        if is_strong_etag(v) {
            Validators {
                etag: Some(v.to_owned()),
                etag_host: host.map(str::to_owned),
                last_modified: None,
            }
        } else {
            Validators {
                last_modified: httpdate::parse_http_date(v).is_ok().then(|| v.to_owned()),
                ..Validators::default()
            }
        }
    }
}

/// `"…"` without inner quotes (a weak `W/"…"` or an unquoted value is not).
fn is_strong_etag(e: &str) -> bool {
    e.len() >= 2 && e.starts_with('"') && e.ends_with('"') && !e[1..e.len() - 1].contains('"')
}

/// A Last-Modified is strong only if the response's `Date` is at least a second later (RFC
/// 9110 §8.8.2.2): a file modified within the second it was served may change again unseen.
fn is_strong_date(last_modified: &str, date: Option<&str>) -> bool {
    let (Ok(lm), Some(Ok(date))) = (
        httpdate::parse_http_date(last_modified),
        date.map(httpdate::parse_http_date),
    ) else {
        return false;
    };
    date >= lm + std::time::Duration::from_secs(1)
}

/// `<dest>.mdm.json`.
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".mdm.json");
    PathBuf::from(s)
}

/// Downloads started under the app's former codename keep `<dest>.idmnext.json` and
/// `<dest>.idmnext.part`: rename them to today's names (only if those don't exist yet), so
/// they resume. Returns true when something was renamed.
pub fn migrate_legacy(dest: &Path) -> bool {
    let legacy = |suffix: &str| {
        let mut s = dest.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    };
    let (old_state, old_part) = (legacy(".idmnext.json"), legacy(".idmnext.part"));
    let (new_state, new_part) = (state_path(dest), crate::naming::part_path(dest));
    if !old_state.exists() || new_state.exists() || new_part.exists() {
        return false;
    }
    // Part first: a state file without its part would be rejected on resume anyway.
    if old_part.exists() && std::fs::rename(&old_part, &new_part).is_err() {
        return false;
    }
    std::fs::rename(&old_state, &new_state).is_ok()
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    #[test]
    fn legacy_files_are_renamed_once() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.zip");
        std::fs::write(dir.path().join("a.zip.idmnext.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("a.zip.idmnext.part"), b"data").unwrap();
        assert!(migrate_legacy(&dest));
        assert!(state_path(&dest).exists());
        assert_eq!(
            std::fs::read(crate::naming::part_path(&dest)).unwrap(),
            b"data"
        );
        assert!(!dir.path().join("a.zip.idmnext.json").exists());
        assert!(!migrate_legacy(&dest), "nothing left to migrate");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.bin");
        let part = File::create(dir.path().join("a.bin.part")).unwrap();
        let url: Url = "http://x/a.bin".parse().unwrap();
        let v = Validators::from_response(Some("\"e\""), None, None, Some("x"));
        let mut s = ResumeState::new(url.clone(), url, dest.clone(), 100, v);
        let segs = vec![
            SavedSegment {
                start: 0,
                cursor: 10,
                end: 50,
            },
            SavedSegment {
                start: 50,
                cursor: 60,
                end: 100,
            },
        ];
        s.save(&state_path(&dest), segs, &part).unwrap();
        let loaded = ResumeState::load(&state_path(&dest)).unwrap();
        assert_eq!(loaded, s);
        assert_eq!(loaded.downloaded(), 20);
    }

    const LM: &str = "Wed, 21 Oct 2015 07:28:00 GMT";
    const LATER: &str = "Wed, 21 Oct 2015 07:28:01 GMT";

    #[test]
    fn a_weak_or_malformed_etag_means_no_validator() {
        for etag in ["W/\"x\"", "unquoted", "\"a\"b\""] {
            let v = Validators::from_response(Some(etag), Some(LM), Some(LATER), Some("h"));
            assert_eq!(v, Validators::default(), "{etag}");
        }
    }

    #[test]
    fn a_date_counts_only_when_served_a_second_later() {
        let strong = Validators::from_response(None, Some(LM), Some(LATER), Some("h"));
        assert_eq!(strong.if_range(Some("h")), Some(LM));
        let same_second = Validators::from_response(None, Some(LM), Some(LM), Some("h"));
        assert_eq!(same_second.if_range(Some("h")), None);
        let no_date = Validators::from_response(None, Some(LM), None, Some("h"));
        assert_eq!(no_date.if_range(Some("h")), None);
    }

    #[test]
    fn the_etag_goes_only_to_its_host() {
        let v = Validators::from_response(Some("\"e\""), Some(LM), Some(LATER), Some("cdn1"));
        assert_eq!(v.if_range(Some("cdn1")), Some("\"e\""));
        assert_eq!(v.if_range(Some("cdn2")), Some(LM));
    }

    #[test]
    fn the_old_single_validator_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin.mdm.json");
        let old = r#"{"version":1,"url":"http://x/a","final_url":"http://cdn/a","dest":"a.bin",
            "size":5,"validator":"\"e\"","segments":[]}"#;
        std::fs::write(&p, old).unwrap();
        let s = ResumeState::load(&p).unwrap();
        assert_eq!(s.validators.if_range(Some("cdn")), Some("\"e\""));
    }

    #[test]
    fn rejects_garbage_and_other_versions() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        std::fs::write(&p, b"{not json").unwrap();
        assert!(ResumeState::load(&p).is_err());
        std::fs::write(&p, br#"{"version":99}"#).unwrap();
        assert!(ResumeState::load(&p).is_err());
    }
}
