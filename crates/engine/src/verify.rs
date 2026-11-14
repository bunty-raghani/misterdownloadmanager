//! Whole-file checksums (beyond IDM, D-015): verify a finished download when its hash is known —
//! given by the user, or announced by the server (`Digest`, `Repr-Digest`, `Content-MD5`,
//! `x-goog-hash`). The overlap check catches a *changed* file; this catches a *corrupt* one.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algo {
    Crc32,
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl Algo {
    pub fn name(self) -> &'static str {
        match self {
            Algo::Crc32 => "CRC32",
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA-1",
            Algo::Sha256 => "SHA-256",
            Algo::Sha512 => "SHA-512",
        }
    }
}

/// An expected hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hash {
    pub algo: Algo,
    #[serde(with = "hex_bytes")]
    pub value: Vec<u8>,
}

impl Hash {
    /// A hash typed or pasted by the user: hex, optionally `algo:` / `algo=` first
    /// (`sha256:…`, `md5=…`); without a name the length decides (8 CRC32, 32 MD5, 40 SHA-1,
    /// 64 SHA-256, 128 SHA-512). Spaces and a trailing file name (`… *file.iso`) are ignored.
    pub fn parse(text: &str) -> Option<Hash> {
        let text = text.trim();
        let (name, rest) = match text.split_once([':', '=']) {
            Some((n, r)) if n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
                (Some(n.to_ascii_lowercase().replace('-', "")), r)
            }
            _ => (None, text),
        };
        let digits: String = rest
            .split_whitespace()
            .next()?
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let value = hex::decode(&digits).ok()?;
        let algo = match name.as_deref() {
            Some("crc32") => Algo::Crc32,
            Some("md5") => Algo::Md5,
            Some("sha1") => Algo::Sha1,
            Some("sha256") => Algo::Sha256,
            Some("sha512") => Algo::Sha512,
            Some(_) => return None,
            None => match value.len() {
                4 => Algo::Crc32,
                16 => Algo::Md5,
                20 => Algo::Sha1,
                32 => Algo::Sha256,
                64 => Algo::Sha512,
                _ => return None,
            },
        };
        (value.len() == digest_len(algo)).then_some(Hash { algo, value })
    }

    /// The strongest hash a full (200) response announces for its content, if any.
    pub fn from_headers(h: &HeaderMap) -> Option<Hash> {
        let b64 = |s: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(s.trim().trim_matches(':'))
                .ok()
        };
        let mut found: Vec<Hash> = Vec::new();
        // RFC 9530 `Repr-Digest: sha-256=:…:`, RFC 3230 `Digest: SHA-256=…`.
        for name in ["repr-digest", "digest"] {
            for v in h.get_all(name).iter().filter_map(|v| v.to_str().ok()) {
                for part in v.split(',') {
                    let Some((algo, value)) = part.trim().split_once('=') else {
                        continue;
                    };
                    let algo = match algo.trim().to_ascii_lowercase().as_str() {
                        "sha-512" => Algo::Sha512,
                        "sha-256" => Algo::Sha256,
                        "sha" | "sha-1" => Algo::Sha1,
                        "md5" => Algo::Md5,
                        _ => continue,
                    };
                    if let Some(value) = b64(value).filter(|v| v.len() == digest_len(algo)) {
                        found.push(Hash { algo, value });
                    }
                }
            }
        }
        if let Some(value) = h
            .get("content-md5")
            .and_then(|v| v.to_str().ok())
            .and_then(b64)
            .filter(|v| v.len() == 16)
        {
            found.push(Hash {
                algo: Algo::Md5,
                value,
            });
        }
        // Google Cloud Storage: `x-goog-hash: crc32c=…, md5=…` (crc32c is not CRC32).
        for v in h
            .get_all("x-goog-hash")
            .iter()
            .filter_map(|v| v.to_str().ok())
        {
            for part in v.split(',') {
                if let Some(value) = part.trim().strip_prefix("md5=").and_then(b64)
                    && value.len() == 16
                {
                    found.push(Hash {
                        algo: Algo::Md5,
                        value,
                    });
                }
            }
        }
        found.into_iter().max_by_key(|h| strength(h.algo))
    }

    /// The hash as text (`sha256:…`).
    pub fn display(&self) -> String {
        format!(
            "{}:{}",
            self.algo.name().to_ascii_lowercase().replace('-', ""),
            hex::encode(&self.value)
        )
    }
}

fn digest_len(a: Algo) -> usize {
    match a {
        Algo::Crc32 => 4,
        Algo::Md5 => 16,
        Algo::Sha1 => 20,
        Algo::Sha256 => 32,
        Algo::Sha512 => 64,
    }
}

fn strength(a: Algo) -> u8 {
    match a {
        Algo::Crc32 => 0,
        Algo::Md5 => 1,
        Algo::Sha1 => 2,
        Algo::Sha256 => 3,
        Algo::Sha512 => 4,
    }
}

/// The file's hash with `algo` (reads the whole file; call off the async threads).
pub fn digest(path: &Path, algo: Algo) -> std::io::Result<Vec<u8>> {
    digest_counted(path, algo, &AtomicU64::new(0))
}

/// [`digest`], adding each part read to `read` as it goes (#736: how far a check has come).
pub fn digest_counted(path: &Path, algo: Algo, read: &AtomicU64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    macro_rules! run {
        ($h:expr) => {{
            let mut h = $h;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                read.fetch_add(n as u64, Ordering::Relaxed);
            }
            h.finalize().to_vec()
        }};
    }
    Ok(match algo {
        Algo::Crc32 => {
            let mut h = crc32fast::Hasher::new();
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                read.fetch_add(n as u64, Ordering::Relaxed);
            }
            h.finalize().to_be_bytes().to_vec()
        }
        Algo::Md5 => run!(md5::Md5::new()),
        Algo::Sha1 => run!(sha1::Sha1::new()),
        Algo::Sha256 => run!(sha2::Sha256::new()),
        Algo::Sha512 => run!(sha2::Sha512::new()),
    })
}

/// Whether the file matches `hash`.
pub fn matches(path: &Path, hash: &Hash) -> std::io::Result<bool> {
    matches_counted(path, hash, &AtomicU64::new(0))
}

/// [`matches`], counting the bytes checked into `read` as it goes.
pub fn matches_counted(path: &Path, hash: &Hash, read: &AtomicU64) -> std::io::Result<bool> {
    Ok(digest_counted(path, hash.algo, read)? == hash.value)
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_hashes_by_length_or_name() {
        let md5 = Hash::parse("9e107d9d372bb6826bd81d3542a419d6").unwrap();
        assert_eq!(md5.algo, Algo::Md5);
        let sha = Hash::parse(
            "SHA256: d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592  *x.iso",
        )
        .unwrap();
        assert_eq!(sha.algo, Algo::Sha256);
        assert!(Hash::parse("md5:abc").is_none());
        assert!(Hash::parse("not a hash").is_none());
    }

    #[test]
    fn server_announced_hashes() {
        let mut h = HeaderMap::new();
        h.insert("content-md5", "nhB9nTcrtoJr2B01QqQZ1g==".parse().unwrap());
        h.insert(
            "repr-digest",
            "sha-256=:16j7swfXgJRpypq8sAguT41WUeRtPNt2LQLQvzfJ5ZI=:"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            Hash::from_headers(&h).unwrap().algo,
            Algo::Sha256,
            "the strongest"
        );
        h.remove("repr-digest");
        assert_eq!(Hash::from_headers(&h).unwrap().algo, Algo::Md5);
    }

    #[test]
    fn digests_of_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fox.txt");
        std::fs::write(&p, b"The quick brown fox jumps over the lazy dog").unwrap();
        let sha = Hash::parse("d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592")
            .unwrap();
        assert!(matches(&p, &sha).unwrap());
        let md5 = Hash::parse("9e107d9d372bb6826bd81d3542a419d6").unwrap();
        assert!(matches(&p, &md5).unwrap());
        let crc = Hash::parse("crc32:414fa339").unwrap();
        assert!(matches(&p, &crc).unwrap());
        let wrong = Hash::parse("00000000000000000000000000000000").unwrap();
        assert!(!matches(&p, &wrong).unwrap());
        // #736: every byte checked is counted.
        let read = AtomicU64::new(0);
        assert!(matches_counted(&p, &sha, &read).unwrap());
        assert_eq!(read.load(Ordering::Relaxed), 43);
    }
}
