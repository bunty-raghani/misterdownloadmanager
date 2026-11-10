//! Fault injection settings. One representation for both the server default (`--faults`) and
//! per-URL overrides (query string): `rate=100000&max_conns=2&drop_after=1M`.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Faults {
    /// Per-response speed cap, bytes/s.
    #[serde(deserialize_with = "size_opt")]
    pub rate: Option<u64>,
    /// Every Nth request is throttled to `slow_rate` (simulates one slow connection).
    pub slow_every: Option<u64>,
    #[serde(deserialize_with = "size_opt")]
    pub slow_rate: Option<u64>,
    /// Max concurrent responses per client IP; extra requests get 503.
    pub max_conns: Option<u32>,
    /// Abort the connection after sending this many body bytes.
    #[serde(deserialize_with = "size_opt")]
    pub drop_after: Option<u64>,
    /// Apply `drop_after` only to every Nth request (default: every request).
    pub drop_every: Option<u64>,
    /// Every Nth request fails with 503.
    pub fail_every: Option<u64>,
    /// The first N requests fail with 503 (a server busy when the download starts).
    pub fail_first: Option<u64>,
    /// Ignore `Range` and always send the whole file with 200.
    #[serde(deserialize_with = "flag_opt")]
    pub no_range: Option<bool>,
    /// Ignore `Range` on every Nth request only (a CDN edge without range support); the file
    /// and its validators stay the same.
    pub no_range_every: Option<u64>,
    /// Answer with a small web page (a login wall) instead of the file.
    #[serde(deserialize_with = "flag_opt")]
    pub html: Option<bool>,
    /// ETag / Last-Modified change every N requests (resume validation).
    pub etag_change_every: Option<u64>,
    /// ETag style: `weak` (W/"…"), `per-request` (a new strong ETag on every answer, same file,
    /// same Last-Modified — CDN nodes), `none`. Default: one strong ETag.
    pub etag: Option<String>,
    /// Delay before sending response headers, ms.
    pub latency_ms: Option<u64>,
    /// Delay before answering a request that asks for a range, ms (on top of `latency_ms`): the
    /// probe's plain GET runs ahead of the range attempts, as on a busy machine.
    pub range_latency_ms: Option<u64>,
    /// Per-response speed cap for the answers to requests that ask for a range (instead of
    /// `rate`): a range attempt a server without ranges answers in full can't fill the sockets
    /// of a busy machine before the client drops it.
    #[serde(deserialize_with = "size_opt")]
    pub range_rate: Option<u64>,
    /// 403 unless the request's `Cookie` holds this pair (`sid=1`), or a cookie of this name
    /// (`sid`): a file behind a session.
    pub require_cookie: Option<String>,
    /// 403 unless the request's `Referer` starts with this (hotlink protection).
    pub require_referer: Option<String>,
    /// 403 when the request's `User-Agent` contains this (a bot check that refuses a
    /// browser's User-Agent on a connection that isn't a browser's, #372).
    pub refuse_agent: Option<String>,
    /// A one-time link: the first request for this path and query is served, every later one
    /// (HEAD and ranges included) gets 410 Gone.
    #[serde(deserialize_with = "flag_opt")]
    pub once: Option<bool>,
    /// Never send the file's byte at this offset: a response that reaches it stops there and
    /// waits until the server's faults no longer say so ([`crate::LabServer::set_faults`]; a
    /// URL's own value is never lifted). A download can't finish meanwhile, only be stopped.
    #[serde(deserialize_with = "size_opt")]
    pub stall_at: Option<u64>,
}

impl Faults {
    pub fn parse(query: &str) -> Result<Self, serde_urlencoded::de::Error> {
        serde_urlencoded::from_str(query)
    }

    /// `self` with every field that `over` sets replaced.
    pub fn overridden_by(&self, over: &Faults) -> Faults {
        Faults {
            rate: over.rate.or(self.rate),
            slow_every: over.slow_every.or(self.slow_every),
            slow_rate: over.slow_rate.or(self.slow_rate),
            max_conns: over.max_conns.or(self.max_conns),
            drop_after: over.drop_after.or(self.drop_after),
            drop_every: over.drop_every.or(self.drop_every),
            fail_every: over.fail_every.or(self.fail_every),
            fail_first: over.fail_first.or(self.fail_first),
            no_range: over.no_range.or(self.no_range),
            no_range_every: over.no_range_every.or(self.no_range_every),
            html: over.html.or(self.html),
            etag_change_every: over.etag_change_every.or(self.etag_change_every),
            etag: over.etag.clone().or(self.etag.clone()),
            latency_ms: over.latency_ms.or(self.latency_ms),
            range_latency_ms: over.range_latency_ms.or(self.range_latency_ms),
            range_rate: over.range_rate.or(self.range_rate),
            require_cookie: over.require_cookie.clone().or(self.require_cookie.clone()),
            refuse_agent: over.refuse_agent.clone().or(self.refuse_agent.clone()),
            require_referer: over
                .require_referer
                .clone()
                .or(self.require_referer.clone()),
            once: over.once.or(self.once),
            stall_at: over.stall_at.or(self.stall_at),
        }
    }
}

/// True when request number `n` (1-based) is selected by "every Nth".
pub fn every(n: u64, nth: Option<u64>) -> bool {
    matches!(nth, Some(k) if k > 0 && n.is_multiple_of(k))
}

fn size_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let s = String::deserialize(d)?;
    crate::content::parse_size(&s)
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom(format!("invalid size `{s}`")))
}

/// `true`/`1`/`yes` or `false`/`0`/`no`.
fn flag_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    let s = String::deserialize(d)?;
    parse_flag(&s)
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom(format!("invalid flag `{s}` (1 or 0)")))
}

pub(crate) fn parse_flag(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// True when the `Cookie` header `cookie` satisfies `require_cookie` (`name=value` or `name`).
pub fn has_cookie(cookie: Option<&str>, want: &str) -> bool {
    let (name, value) = match want.split_once('=') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (want.trim(), None),
    };
    cookie.unwrap_or("").split(';').any(|pair| {
        let (n, v) = pair.split_once('=').unwrap_or((pair, ""));
        n.trim() == name && value.is_none_or(|want| v.trim() == want)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_override() {
        let base = Faults::parse("rate=1M&max_conns=2").unwrap();
        assert_eq!(base.rate, Some(1 << 20));
        assert_eq!(base.max_conns, Some(2));
        let over = Faults::parse("max_conns=4&no_range=true").unwrap();
        let f = base.overridden_by(&over);
        assert_eq!(f.rate, Some(1 << 20));
        assert_eq!(f.max_conns, Some(4));
        assert_eq!(f.no_range, Some(true));
    }

    #[test]
    fn rejects_unknown_and_bad_sizes() {
        assert!(Faults::parse("speed=1").is_err());
        assert!(Faults::parse("rate=fast").is_err());
    }

    #[test]
    fn flags_and_refusals() {
        let f = Faults::parse("once=1&html=0&no_range=true&require_cookie=sid=1").unwrap();
        assert_eq!(f.once, Some(true));
        assert_eq!(f.html, Some(false));
        assert_eq!(f.no_range, Some(true));
        assert_eq!(f.require_cookie.as_deref(), Some("sid=1"));
        assert!(Faults::parse("once=maybe").is_err());
        assert!(has_cookie(Some("a=2; sid=1"), "sid=1"));
        assert!(has_cookie(Some("sid=7"), "sid"));
        assert!(!has_cookie(Some("sid=11"), "sid=1"));
        assert!(!has_cookie(Some("xsid=1"), "sid=1"));
        assert!(!has_cookie(None, "sid"));
    }

    #[test]
    fn every_nth() {
        assert!(every(4, Some(2)));
        assert!(!every(3, Some(2)));
        assert!(!every(3, None));
        assert!(!every(3, Some(0)));
    }
}
