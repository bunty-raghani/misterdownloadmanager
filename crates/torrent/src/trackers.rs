//! Which of a torrent's trackers MDM lets librqbit contact (#40). A torrent names its trackers,
//! and a tracker on this computer or the local network (`127.0.0.1`, `192.168.1.1`, `localhost`)
//! lets a torrent make MDM send requests into the network it sits in: those are dropped. With a
//! proxy, UDP trackers are dropped too: librqbit sends them around the proxy.
//!
//! The list is changed in the torrent itself (`announce`, `announce-list`, outside the info
//! dictionary, so its hash stays the same) and in a magnet link (`tr=`).

use std::net::{IpAddr, Ipv4Addr};

/// Whether MDM may contact `tracker`.
pub fn allowed(tracker: &str, proxied: bool) -> bool {
    let Ok(url) = url::Url::parse(tracker.trim()) else {
        return false;
    };
    match url.scheme() {
        "http" | "https" => {}
        "udp" if !proxied => {}
        _ => return false,
    }
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !(d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local"))
        }
        Some(url::Host::Ipv4(a)) => public(IpAddr::V4(a)),
        Some(url::Host::Ipv6(a)) => public(IpAddr::V6(a)),
        None => false,
    }
}

/// An address on the internet (not this computer, the local network or a reserved range).
pub(crate) fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                // 0.0.0.0/8, "this network": 0.0.0.1 reaches the machine's own stack.
                || o[0] == 0
                // 100.64.0.0/10, carrier-grade NAT.
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // 198.18.0.0/15, the benchmark ranges.
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                // 240.0.0.0/4, reserved (255.255.255.255 is the broadcast, above).
                || o[0] >= 240)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            // IPv4 inside IPv6 that isn't the mapped form: compatible (::a.b.c.d) and NAT64
            // (64:ff9b::a.b.c.d). Whichever it is, its IPv4 rules decide.
            let embedded = (s[..6] == [0; 6] && !a.is_unspecified() && !a.is_loopback())
                .then_some([s[6], s[7]])
                .or_else(|| {
                    (s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4]).then_some([s[6], s[7]])
                });
            if let Some([hi, lo]) = embedded {
                let v4 = Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
                return public(IpAddr::V4(v4));
            }
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || a.to_ipv4_mapped().is_some_and(|v4| !public(IpAddr::V4(v4))))
        }
    }
}

/// A magnet link's trackers (`tr=`), each once, in its order.
pub fn in_magnet(magnet: &str) -> Vec<String> {
    let Some((_, query)) = magnet.split_once('?') else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        let v = v.trim();
        if k.eq_ignore_ascii_case("tr") && !v.is_empty() && !out.iter().any(|t| t == v) {
            out.push(v.to_owned());
        }
    }
    out
}

/// A magnet link with only the trackers MDM may contact.
pub fn filter_magnet(magnet: &str, proxied: bool) -> String {
    let Some((head, query)) = magnet.split_once('?') else {
        return magnet.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| match kv.split_once('=') {
            Some((k, v)) if k.eq_ignore_ascii_case("tr") => {
                let v = url::form_urlencoded::parse(format!("x={v}").as_bytes())
                    .next()
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                allowed(&v, proxied)
            }
            _ => true,
        })
        .collect();
    format!("{head}?{}", kept.join("&"))
}

/// A .torrent with only the trackers MDM may contact; everything else byte for byte, so its
/// info hash is the same. None when it isn't a bencoded dictionary.
pub fn filter_torrent(bytes: &[u8], proxied: bool) -> Option<Vec<u8>> {
    let entries = top_level(bytes)?;
    // An `announce-list` replaces `announce` ([`listed`]): when none of its trackers is kept,
    // `announce` goes too, or the torrent would ask a tracker it never named for its peers.
    let list = entries
        .iter()
        .find(|(k, _)| *k == b"announce-list")
        .map(|e| e.1);
    let replaced = list.and_then(list_of_lists).is_some_and(|t| !t.is_empty());
    let none_kept = match list {
        Some(v) => kept(v, proxied)?.is_empty(),
        None => false,
    };
    let mut out = b"d".to_vec();
    for (key, value) in entries {
        match key {
            b"announce" => {
                let url = string_at(value)?;
                if allowed(&String::from_utf8_lossy(url), proxied) && !(replaced && none_kept) {
                    push_string(&mut out, key);
                    out.extend_from_slice(value);
                }
            }
            b"announce-list" => {
                let tiers = kept(value, proxied)?;
                if !tiers.is_empty() {
                    push_string(&mut out, key);
                    out.push(b'l');
                    for tier in tiers {
                        out.push(b'l');
                        for u in tier {
                            push_string(&mut out, u);
                        }
                        out.push(b'e');
                    }
                    out.push(b'e');
                }
            }
            _ => {
                push_string(&mut out, key);
                out.extend_from_slice(value);
            }
        }
    }
    out.push(b'e');
    Some(out)
}

/// An `announce-list`'s tiers with only the trackers MDM may contact, and only the tiers left
/// with one. None when it isn't a list of lists.
fn kept(value: &[u8], proxied: bool) -> Option<Vec<Vec<&[u8]>>> {
    Some(
        list_of_lists(value)?
            .into_iter()
            .map(|tier| {
                tier.into_iter()
                    .filter(|u| allowed(&String::from_utf8_lossy(u), proxied))
                    .collect::<Vec<_>>()
            })
            .filter(|tier| !tier.is_empty())
            .collect(),
    )
}

/// A .torrent's trackers, each once, in its order: `announce-list`'s tiers, or `announce` when
/// it has no list (BEP 12: a list replaces it).
pub fn listed(bytes: &[u8]) -> Vec<String> {
    let Some(entries) = top_level(bytes) else {
        return Vec::new();
    };
    let value = |k: &[u8]| entries.iter().find(|(key, _)| *key == k).map(|e| e.1);
    let urls: Vec<&[u8]> = match value(b"announce-list").and_then(list_of_lists) {
        Some(tiers) if !tiers.is_empty() => tiers.into_iter().flatten().collect(),
        _ => value(b"announce").and_then(string_at).into_iter().collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for u in urls {
        let u = String::from_utf8_lossy(u).trim().to_owned();
        if !u.is_empty() && !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// A .torrent with `more` trackers (each a tier of its own, after the ones it has; those it
/// has already are skipped); everything else byte for byte, so its info hash is the same. One
/// with only `announce` gets a list holding it first; one with none gets `announce` too. None
/// when it isn't a bencoded dictionary.
pub fn add(bytes: &[u8], more: &[String]) -> Option<Vec<u8>> {
    let entries = top_level(bytes)?;
    let have = listed(bytes);
    let mut fresh: Vec<&str> = Vec::new();
    for u in more.iter().map(|u| u.trim()) {
        if !u.is_empty() && !have.iter().any(|h| h == u) && !fresh.contains(&u) {
            fresh.push(u);
        }
    }
    if fresh.is_empty() {
        return Some(bytes.to_vec());
    }
    let mut tiers: Vec<Vec<&[u8]>> = match entries
        .iter()
        .find(|(k, _)| *k == b"announce-list")
        .map(|e| list_of_lists(e.1))
    {
        Some(None) => return None,
        Some(Some(list)) if !list.is_empty() => list,
        _ => have.iter().map(|u| vec![u.as_bytes()]).collect(),
    };
    tiers.extend(fresh.iter().map(|u| vec![u.as_bytes()]));
    let mut list = b"l".to_vec();
    for tier in &tiers {
        list.push(b'l');
        for u in tier {
            push_string(&mut list, u);
        }
        list.push(b'e');
    }
    list.push(b'e');
    let mut out: Vec<(&[u8], Vec<u8>)> = entries
        .into_iter()
        .filter(|(k, _)| *k != b"announce-list")
        .map(|(k, v)| (k, v.to_vec()))
        .collect();
    out.push((b"announce-list", list));
    if !out.iter().any(|(k, _)| *k == b"announce") {
        let mut first = Vec::new();
        push_string(&mut first, fresh[0].as_bytes());
        out.push((b"announce", first));
    }
    // A dictionary's keys are sorted (bencode); the info dictionary's bytes are untouched.
    out.sort_by(|a, b| a.0.cmp(b.0));
    let mut done = b"d".to_vec();
    for (k, v) in out {
        push_string(&mut done, k);
        done.extend_from_slice(&v);
    }
    done.push(b'e');
    Some(done)
}

fn push_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(s);
}

/// The top-level dictionary's entries: each key and its value's bytes.
fn top_level(b: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    if b.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    let mut out = Vec::new();
    while *b.get(at)? != b'e' {
        let (key, next) = string(b, at)?;
        let end = skip(b, next, 0)?;
        out.push((key, &b[next..end]));
        at = end;
    }
    (at + 1 == b.len()).then_some(out)
}

/// A string at `at`: its bytes, and where it ends.
fn string(b: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let colon = at + b.get(at..)?.iter().position(|&c| c == b':')?;
    let len: usize = std::str::from_utf8(&b[at..colon]).ok()?.parse().ok()?;
    let start = colon + 1;
    let end = start.checked_add(len)?;
    Some((b.get(start..end)?, end))
}

fn string_at(value: &[u8]) -> Option<&[u8]> {
    let (s, end) = string(value, 0)?;
    (end == value.len()).then_some(s)
}

fn list_of_lists(value: &[u8]) -> Option<Vec<Vec<&[u8]>>> {
    if value.first() != Some(&b'l') {
        return None;
    }
    let mut at = 1;
    let mut tiers = Vec::new();
    while *value.get(at)? != b'e' {
        if value[at] != b'l' {
            return None;
        }
        at += 1;
        let mut tier = Vec::new();
        while *value.get(at)? != b'e' {
            let (s, next) = string(value, at)?;
            tier.push(s);
            at = next;
        }
        at += 1;
        tiers.push(tier);
    }
    Some(tiers)
}

/// Where the value at `at` ends; nesting is limited (librqbit 9.0.1's crash was unbounded depth).
fn skip(b: &[u8], at: usize, depth: usize) -> Option<usize> {
    if depth > 64 {
        return None;
    }
    match *b.get(at)? {
        b'i' => Some(at + b.get(at..)?.iter().position(|&c| c == b'e')? + 1),
        b'l' | b'd' => {
            let mut at = at + 1;
            while *b.get(at)? != b'e' {
                at = skip(b, at, depth + 1)?;
            }
            Some(at + 1)
        }
        b'0'..=b'9' => string(b, at).map(|(_, end)| end),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_odd_trackers_are_dropped() {
        for ok in [
            "http://tracker.example.org:80/announce",
            "https://t.example/announce",
            "udp://tracker.opentrackr.org:1337/announce",
            "http://8.8.8.8/announce",
        ] {
            assert!(allowed(ok, false), "{ok}");
        }
        for bad in [
            "http://127.0.0.1:8080/announce",
            "http://localhost/announce",
            "http://router.local/announce",
            "http://192.168.1.1/announce",
            "http://10.0.0.5/a",
            "http://169.254.1.1/a",
            "http://100.64.0.1/a",
            "http://[::1]/a",
            "http://[fd00::1]/a",
            "http://[::ffff:192.168.0.1]/a",
            "file:///etc/passwd",
            "wss://t.example/a",
            "not a url",
        ] {
            assert!(!allowed(bad, false), "{bad}");
        }
        assert!(
            !allowed("udp://tracker.example:1337/a", true),
            "UDP goes around the proxy"
        );
        assert!(allowed("https://t.example/a", true));
    }

    /// "This" network, the reserved ranges and IPv4 addresses written as IPv6 reach this
    /// machine or go nowhere: no tracker there gets a request.
    #[test]
    fn this_networks_and_reserved_addresses_are_dropped() {
        for bad in [
            "http://0.0.0.1/announce",
            "http://0.255.255.254/announce",
            "http://240.0.0.1/announce",
            "http://192.0.0.170/announce",
            "http://198.18.0.1/announce",
            "http://198.19.255.254/announce",
            "http://[::7f00:1]/announce",
            "http://[64:ff9b::a00:1]/announce",
        ] {
            assert!(!allowed(bad, false), "{bad}");
        }
        // Their public neighbours still pass.
        assert!(allowed("http://192.0.1.1/announce", false));
        assert!(allowed("http://198.20.0.1/announce", false));
        assert!(allowed("http://[2001:4860:4860::8888]/announce", false));
    }

    #[test]
    fn magnets_lose_only_the_trackers_dropped() {
        let m = "magnet:?xt=urn:btih:abc&dn=x&tr=udp%3A%2F%2Ft.example%3A1337&tr=http%3A%2F%2F127.0.0.1%2Fa";
        assert_eq!(
            filter_magnet(m, false),
            "magnet:?xt=urn:btih:abc&dn=x&tr=udp%3A%2F%2Ft.example%3A1337"
        );
        assert_eq!(filter_magnet(m, true), "magnet:?xt=urn:btih:abc&dn=x");
        assert_eq!(
            in_magnet(&format!("{m}&tr=udp%3A%2F%2Ft.example%3A1337&tr=")),
            ["udp://t.example:1337", "http://127.0.0.1/a"]
        );
        assert!(in_magnet("magnet:?xt=urn:btih:abc").is_empty());
    }

    /// Add trackers (#39): new ones after the ones there, each once, keys still sorted, the info
    /// dictionary byte for byte.
    #[test]
    fn trackers_are_added_after_the_ones_there() {
        let info = b"d6:lengthi5e4:name1:a12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaae";
        let s = |v: &str| format!("{}:{v}", v.len());
        let with = |head: &str| {
            let mut t = format!("d{head}4:info").into_bytes();
            t.extend(info);
            t.push(b'e');
            t
        };
        let (a, b, c) = (
            "http://a.example/announce",
            "udp://b.example:1337/announce",
            "https://c.example/announce",
        );
        let more = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Only `announce`: a list with it first, then the new ones, each a tier.
        let one = with(&format!("8:announce{}", s(a)));
        let out = add(&one, &more(&[b, a, b, " ", c])).unwrap();
        assert_eq!(
            out,
            with(&format!(
                "8:announce{}13:announce-listll{}el{}el{}ee",
                s(a),
                s(a),
                s(b),
                s(c)
            ))
        );
        assert_eq!(listed(&out), [a, b, c]);

        // A list already: kept as it is, the new one after; a comment key stays in its place.
        let listed_one = with(&format!(
            "8:announce{}13:announce-listll{}{}ee7:comment2:hi",
            s(a),
            s(a),
            s(b)
        ));
        let out = add(&listed_one, &more(&[c])).unwrap();
        assert_eq!(
            out,
            with(&format!(
                "8:announce{}13:announce-listll{}{}el{}ee7:comment2:hi",
                s(a),
                s(a),
                s(b),
                s(c)
            ))
        );

        // None at all (a magnet's): `announce` and a list.
        let bare = with("");
        let out = add(&bare, &more(&[b])).unwrap();
        assert_eq!(
            out,
            with(&format!("8:announce{}13:announce-listll{}ee", s(b), s(b)))
        );
        // Nothing new: the same bytes.
        assert_eq!(add(&out, &more(&[b])).unwrap(), out);
        assert_eq!(add(b"junk", &more(&[a])), None);

        // The hash is the same: the info bytes are untouched.
        let at = |t: &[u8]| t.windows(info.len()).position(|w| w == info).is_some();
        assert!(at(&add(&one, &more(&[c])).unwrap()));
    }

    /// The trackers change; the info dictionary, and so the hash, doesn't.
    #[test]
    fn torrents_keep_their_info_byte_for_byte() {
        let info = b"d6:lengthi5e4:name1:a12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaae";
        let s = |v: &str| format!("{}:{v}", v.len());
        let (local, udp) = ("http://127.0.0.1/announce", "udp://t.example:1337/a");
        let mut t = format!(
            "d8:announce{}13:announce-listll{}el{}ee4:info",
            s(local),
            s(local),
            s(udp)
        )
        .into_bytes();
        t.extend(info);
        t.push(b'e');
        let out = filter_torrent(&t, false).unwrap();
        let mut want = format!("d13:announce-listll{}ee4:info", s(udp)).into_bytes();
        want.extend(info);
        want.push(b'e');
        assert_eq!(out, want);
        // With a proxy nothing is left: no tracker keys at all.
        let mut bare = b"d4:info".to_vec();
        bare.extend(info);
        bare.push(b'e');
        assert_eq!(filter_torrent(&t, true).unwrap(), bare);
        assert_eq!(filter_torrent(b"not bencode", false), None);
        assert_eq!(filter_torrent(b"d4:infoi1e", false), None, "unterminated");
    }

    /// Filtering never brings a tracker in: an `announce` that a list replaced stays unused.
    #[test]
    fn filtering_only_takes_trackers_away() {
        let s = |v: &str| format!("{}:{v}", v.len());
        let (public, local) = ("http://t.example/announce", "http://127.0.0.1/announce");
        // The list names only trackers MDM won't ask: `announce` (not used) doesn't come back.
        let t = format!("d8:announce{}13:announce-listll{}eee", s(public), s(local));
        let out = filter_torrent(t.as_bytes(), false).unwrap();
        assert_eq!(listed(t.as_bytes()), [local]);
        assert!(listed(&out).is_empty(), "{}", String::from_utf8_lossy(&out));
        assert_eq!(out, b"de");
        // An empty tier is a list all the same.
        let t = format!("d8:announce{}13:announce-listlleee", s(public));
        assert!(listed(&filter_torrent(t.as_bytes(), false).unwrap()).is_empty());
        // With a tracker of the list kept, or no list at all, `announce` stays as it was.
        let t = format!(
            "d8:announce{}13:announce-listll{}{}eee",
            s(public),
            s(local),
            s(public)
        );
        assert_eq!(
            listed(&filter_torrent(t.as_bytes(), false).unwrap()),
            [public]
        );
        let t = format!("d8:announce{}e", s(public));
        assert_eq!(filter_torrent(t.as_bytes(), false).unwrap(), t.as_bytes());
    }
}
