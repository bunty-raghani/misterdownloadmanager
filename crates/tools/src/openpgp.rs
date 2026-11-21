//! Just enough OpenPGP (RFC 9580, and RFC 4880 before it) to check a detached signature by one
//! of the keys MDM pins: yt-dlp's (RSA) and MDM's own release key (Ed25519, in its legacy
//! EdDSA form, algorithm 22). Both sign their checksum lists as binary documents with v4
//! signatures; that, with SHA-256 or SHA-512, is all this understands, and anything else fails.
//! The keys are built into the program, so their self-signatures aren't checked cryptographically,
//! but what they say is honoured: a key or subkey signs only when its newest self-signature (or
//! subkey binding signature) gives it the "sign data" key flag (a primary key without key flags
//! at all, as older keys are made, may sign), only signatures made before the key expired count,
//! and a revoked key signs nothing. The cryptography is aws-lc-rs's, the library rustls already
//! brings.
//!
//! The input is untrusted: every read is bounds-checked, and a malformed byte only makes a
//! check fail.

use aws_lc_rs::digest::{self, Digest};
use aws_lc_rs::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use base64::Engine;

// Packet tags (RFC 9580 §5).
const TAG_SIGNATURE: u8 = 2;
const TAG_PUBLIC_KEY: u8 = 6;
const TAG_PUBLIC_SUBKEY: u8 = 14;

// Public-key algorithms (§9.1).
const RSA: u8 = 1;
const EDDSA_LEGACY: u8 = 22;

// Hash algorithms (§9.5).
const SHA256: u8 = 8;
const SHA512: u8 = 10;

// Signature subpackets (§5.2.3.7).
const SUB_CREATED: u8 = 2;
const SUB_EXPIRES: u8 = 3;
const SUB_KEY_EXPIRES: u8 = 9;
const SUB_KEY_FLAGS: u8 = 27;

// Key flags (§5.2.3.29): may sign data.
const FLAG_SIGN: u8 = 0x02;

// Signature types (§5.2.1) of self-signatures.
const SIG_CERTIFICATIONS: std::ops::RangeInclusive<u8> = 0x10..=0x13;
const SIG_SUBKEY_BINDING: u8 = 0x18;
const SIG_DIRECT_KEY: u8 = 0x1f;
const SIG_KEY_REVOCATION: u8 = 0x20;
const SIG_SUBKEY_REVOCATION: u8 = 0x28;
const SUB_ISSUER: u8 = 16;
const SUB_ISSUER_FINGERPRINT: u8 = 33;

/// Ed25519's curve OID as legacy EdDSA keys carry it (1.3.6.1.4.1.11591.15.1, §9.2).
const ED25519_OID: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xda, 0x47, 0x0f, 0x01];

/// Whether `sig` (one signature packet, binary or ASCII-armored) is a valid signature of `data`
/// by the public key `key` (binary or ASCII-armored) or one of its subkeys.
pub fn verify(key: &[u8], data: &[u8], sig: &[u8]) -> bool {
    let (Some(key), Some(sig)) = (unarmor(key, "PUBLIC KEY BLOCK"), unarmor(sig, "SIGNATURE"))
    else {
        return false;
    };
    let (Some(keys), Some(sig)) = (public_keys(&key), Signature::parse(&sig)) else {
        return false;
    };
    keys.iter().any(|key| sig.verifies(key, data))
}

/// The packets of `armored` if it's an ASCII-armored block of `kind` ("SIGNATURE", ...), or
/// `armored` itself if it's binary.
fn unarmor(armored: &[u8], kind: &str) -> Option<Vec<u8>> {
    // A binary packet starts with a byte whose top bit is set, which no armor does.
    if armored.first().is_some_and(|b| b & 0x80 != 0) {
        return Some(armored.to_vec());
    }
    let text = std::str::from_utf8(armored).ok()?;
    let mut lines = text.lines().map(str::trim_end);
    let begin = format!("-----BEGIN PGP {kind}-----");
    let end = format!("-----END PGP {kind}-----");
    lines.find(|line| *line == begin)?;
    // Headers ("Version: ..."), up to the blank line before the data.
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if !line.contains(": ") {
            return None;
        }
    }
    let mut body = String::new();
    let mut crc = None;
    loop {
        let line = lines.next()?;
        if line == end {
            break;
        }
        if let Some(sum) = line.strip_prefix('=') {
            // The optional checksum comes last, just before the end line.
            let sum = base64::engine::general_purpose::STANDARD.decode(sum).ok()?;
            let [a, b, c] = sum[..] else {
                return None;
            };
            crc = Some(u32::from_be_bytes([0, a, b, c]));
            if lines.next()? != end {
                return None;
            }
            break;
        }
        body.push_str(line);
    }
    let packets = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    if crc.is_some_and(|crc| crc != crc24(&packets)) {
        return None;
    }
    Some(packets)
}

/// The armor's checksum (RFC 9580 §6.1.1).
fn crc24(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xb7_04ce;
    for &byte in bytes {
        crc ^= u32::from(byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x100_0000 != 0 {
                crc ^= 0x186_4cfb;
            }
        }
    }
    crc & 0xff_ffff
}

/// Reads through a packet or subpacket, failing at its end instead of panicking.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A multiprecision integer (§3.2): big-endian, no leading zeros, its length in bits
    /// counted from its first set bit. Any other length fails.
    fn mpi(&mut self) -> Option<&'a [u8]> {
        let bits = usize::from(self.u16()?);
        let bytes = self.take(bits.div_ceil(8))?;
        let top = bytes.first().map_or(0, |&b| 8 - b.leading_zeros() as usize);
        (bits == (bytes.len().saturating_sub(1)) * 8 + top).then_some(bytes)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `bytes`, split into its packets as (tag, body); none if any packet is malformed.
fn packets(bytes: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut r = Reader(bytes);
    let mut packets = Vec::new();
    while !r.is_empty() {
        let first = r.u8()?;
        if first & 0x80 == 0 {
            return None;
        }
        let (tag, len) = if first & 0x40 != 0 {
            // The current format (§4.2.1). Partial lengths are only for data packets.
            let len = match r.u8()? {
                n @ 0..192 => usize::from(n),
                n @ 192..224 => (usize::from(n - 192) << 8) + usize::from(r.u8()?) + 192,
                255 => usize::try_from(r.u32()?).ok()?,
                _ => return None,
            };
            (first & 0x3f, len)
        } else {
            // The legacy format (§4.2.2), which gpg still writes; never of indeterminate length.
            let len = match first & 0x03 {
                0 => usize::from(r.u8()?),
                1 => usize::from(r.u16()?),
                2 => usize::try_from(r.u32()?).ok()?,
                _ => return None,
            };
            ((first >> 2) & 0x0f, len)
        };
        packets.push((tag, r.take(len)?));
    }
    Some(packets)
}

/// A v4 public key or subkey this can check signatures with.
struct PublicKey {
    /// SHA-1 over the key packet (§5.5.4.2); its last 8 bytes are the key ID.
    fingerprint: [u8; 20],
    material: KeyMaterial,
    /// When it was made (seconds since 1970).
    created: u32,
    /// What its self-signatures say: whether it may sign data, when it expires (seconds since
    /// 1970; none: never) and whether it was revoked.
    can_sign: bool,
    expires: Option<u64>,
    revoked: bool,
}

enum KeyMaterial {
    /// The modulus and the public exponent, big-endian, no leading zeros.
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    Ed25519([u8; 32]),
}

impl PublicKey {
    fn algorithm(&self) -> u8 {
        match self.material {
            KeyMaterial::Rsa { .. } => RSA,
            KeyMaterial::Ed25519(_) => EDDSA_LEGACY,
        }
    }

    fn key_id(&self) -> &[u8] {
        &self.fingerprint[12..]
    }

    /// The key in a public key or subkey packet's `body`; none if it's of another version or
    /// algorithm.
    fn parse(body: &[u8]) -> Option<Self> {
        let mut r = Reader(body);
        if r.u8()? != 4 {
            return None;
        }
        let created = r.u32()?;
        let material = match r.u8()? {
            RSA => {
                let n = r.mpi()?.to_vec();
                let e = r.mpi()?.to_vec();
                KeyMaterial::Rsa { n, e }
            }
            EDDSA_LEGACY => {
                let oid_len = usize::from(r.u8()?);
                if r.take(oid_len)? != ED25519_OID {
                    return None;
                }
                // The point, prefixed with 0x40 (native encoding, §11.2.2).
                let point = r.mpi()?;
                let [0x40, point @ ..] = point else {
                    return None;
                };
                KeyMaterial::Ed25519(point.try_into().ok()?)
            }
            _ => return None,
        };
        if !r.is_empty() {
            return None;
        }
        let len = u16::try_from(body.len()).ok()?.to_be_bytes();
        let mut sha1 = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
        sha1.update(&[0x99, len[0], len[1]]);
        sha1.update(body);
        let fingerprint = sha1.finish().as_ref().try_into().ok()?;
        Some(Self {
            fingerprint,
            material,
            created,
            can_sign: false,
            expires: None,
            revoked: false,
        })
    }

    /// Whether it may have made a signature at `when` (seconds since 1970): it signs, it was
    /// made by then, it hadn't expired, it isn't revoked.
    fn signs_at(&self, when: u32) -> bool {
        self.can_sign
            && !self.revoked
            && when >= self.created
            && self.expires.is_none_or(|e| u64::from(when) < e)
    }
}

/// What a self-signature says of its key ([`self_signature`]).
struct SelfSignature {
    kind: u8,
    created: u32,
    /// The key flags' first byte, if it has them.
    flags: Option<u8>,
    /// Seconds after the key's creation it expires (0: never), if it says.
    key_expires: Option<u32>,
    /// The key ID of the key that made it, if it says (others certify user IDs too).
    issuer: Option<[u8; 8]>,
}

/// The type, creation time, key flags and key expiry of a v4 signature packet's `body`, from its
/// hashed subpackets, and its issuer (hashed or not: it only names a key); none for another
/// version or a malformed one.
fn self_signature(body: &[u8]) -> Option<SelfSignature> {
    let mut r = Reader(body);
    let [4, kind, _algorithm, _hash] = *r.take(4)? else {
        return None;
    };
    let mut sig = SelfSignature {
        kind,
        created: 0,
        flags: None,
        key_expires: None,
        issuer: None,
    };
    for hashed in [true, false] {
        let len = usize::from(r.u16()?);
        let mut area = Reader(r.take(len)?);
        while !area.is_empty() {
            let len = match area.u8()? {
                n @ 0..192 => usize::from(n),
                n @ 192..255 => (usize::from(n - 192) << 8) + usize::from(area.u8()?) + 192,
                255 => usize::try_from(area.u32()?).ok()?,
            };
            let mut sub = Reader(area.take(len)?);
            match sub.u8()? & 0x7f {
                SUB_CREATED if hashed => sig.created = sub.u32()?,
                SUB_KEY_EXPIRES if hashed => sig.key_expires = Some(sub.u32()?),
                SUB_KEY_FLAGS if hashed => sig.flags = Some(sub.u8().unwrap_or(0)),
                SUB_ISSUER => sig.issuer = Some(sub.take(8)?.try_into().ok()?),
                SUB_ISSUER_FINGERPRINT if sub.u8()? == 4 => {
                    sig.issuer = Some(sub.take(20)?.get(12..)?.try_into().ok()?);
                }
                _ => {}
            }
        }
    }
    Some(sig)
}

/// Apply the newest of `sigs` (the signatures after one key's packet) made by the primary key
/// (`primary_id`) to `key`; a revocation among them revokes it. Without key flags, a primary key
/// may sign (as keys were made before key flags); a subkey may not. Signatures by other keys
/// (certifications of a user ID) say nothing of it.
fn apply_self_signatures(
    key: &mut PublicKey,
    sigs: &[SelfSignature],
    primary: bool,
    primary_id: &[u8],
) {
    let sigs: Vec<&SelfSignature> = sigs
        .iter()
        .filter(|s| s.issuer.is_none_or(|id| id == primary_id))
        .collect();
    key.revoked = sigs
        .iter()
        .any(|s| matches!(s.kind, SIG_KEY_REVOCATION | SIG_SUBKEY_REVOCATION));
    let binding = |s: &&&SelfSignature| {
        if primary {
            SIG_CERTIFICATIONS.contains(&s.kind) || s.kind == SIG_DIRECT_KEY
        } else {
            s.kind == SIG_SUBKEY_BINDING
        }
    };
    let Some(newest) = sigs.iter().filter(binding).max_by_key(|s| s.created) else {
        // Not bound to the key: a subkey without a binding signature isn't part of it.
        key.can_sign = primary;
        return;
    };
    key.can_sign = match newest.flags {
        Some(flags) => flags & FLAG_SIGN != 0,
        None => primary,
    };
    key.expires = newest
        .key_expires
        .filter(|&e| e != 0)
        .map(|e| u64::from(key.created) + u64::from(e));
}

/// The keys in a transferable public key (§10.1) this can check signatures with: the primary key
/// and its subkeys, each with what its self-signatures say ([`apply_self_signatures`]). A key of
/// an algorithm this doesn't know is left out; the user IDs only carry the primary key's
/// self-signatures.
fn public_keys(bytes: &[u8]) -> Option<Vec<PublicKey>> {
    let packets = packets(bytes)?;
    if packets.first()?.0 != TAG_PUBLIC_KEY {
        return None;
    }
    // Each key packet with the self-signatures after it, up to the next key packet.
    let mut keys: Vec<(Option<PublicKey>, bool, Vec<SelfSignature>)> = Vec::new();
    for (tag, body) in &packets {
        match *tag {
            TAG_PUBLIC_KEY if keys.is_empty() => {
                keys.push((PublicKey::parse(body), true, Vec::new()))
            }
            TAG_PUBLIC_KEY => return None,
            TAG_PUBLIC_SUBKEY => keys.push((PublicKey::parse(body), false, Vec::new())),
            TAG_SIGNATURE => {
                if let (Some(last), Some(sig)) = (keys.last_mut(), self_signature(body)) {
                    last.2.push(sig);
                }
            }
            _ => {}
        }
    }
    let primary_id: [u8; 8] = keys.first()?.0.as_ref()?.key_id().try_into().ok()?;
    Some(
        keys.into_iter()
            .filter_map(|(key, primary, sigs)| {
                let mut key = key?;
                apply_self_signatures(&mut key, &sigs, primary, &primary_id);
                Some(key)
            })
            .collect(),
    )
}

/// A v4 signature of a binary document (type 0x00).
struct Signature<'a> {
    algorithm: u8,
    hash: u8,
    /// The signature's hashed part: version, type, algorithms and hashed subpackets.
    hashed: &'a [u8],
    left16: [u8; 2],
    /// Seconds since 1970 when made, and for how long it holds (0: forever).
    created: u32,
    expires: u32,
    issuer: Option<[u8; 8]>,
    issuer_fingerprint: Option<[u8; 20]>,
    /// RSA: the signature; Ed25519: R and S.
    mpis: Vec<&'a [u8]>,
}

impl<'a> Signature<'a> {
    /// The only packet in `bytes`, a signature this understands.
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        let [(TAG_SIGNATURE, body)] = packets(bytes)?[..] else {
            return None;
        };
        let mut r = Reader(body);
        let [4, 0x00, algorithm, hash] = *r.take(4)? else {
            return None;
        };
        let hashed_len = usize::from(r.u16()?);
        let hashed_area = r.take(hashed_len)?;
        let unhashed_len = usize::from(r.u16()?);
        let unhashed_area = r.take(unhashed_len)?;
        let left16 = r.take(2)?.try_into().ok()?;
        let mut sig = Signature {
            algorithm,
            hash,
            hashed: body.get(..6 + hashed_len)?,
            left16,
            created: 0,
            expires: 0,
            issuer: None,
            issuer_fingerprint: None,
            mpis: Vec::new(),
        };
        let mut created = None;
        sig.subpackets(hashed_area, true, &mut created)?;
        sig.subpackets(unhashed_area, false, &mut created)?;
        // The creation time must be there, and signed (§5.2.3.11).
        sig.created = created?;
        let count = match algorithm {
            RSA => 1,
            EDDSA_LEGACY => 2,
            _ => return None,
        };
        for _ in 0..count {
            sig.mpis.push(r.mpi()?);
        }
        if !r.is_empty() {
            return None;
        }
        Some(sig)
    }

    /// Reads the subpackets in `area` (§5.2.3.7). A critical one this doesn't know, or one
    /// that is malformed, fails the signature; times only count from the hashed area.
    fn subpackets(&mut self, area: &[u8], hashed: bool, created: &mut Option<u32>) -> Option<()> {
        let mut r = Reader(area);
        while !r.is_empty() {
            let len = match r.u8()? {
                n @ 0..192 => usize::from(n),
                n @ 192..255 => (usize::from(n - 192) << 8) + usize::from(r.u8()?) + 192,
                255 => usize::try_from(r.u32()?).ok()?,
            };
            let mut sub = Reader(r.take(len)?);
            let kind = sub.u8()?;
            let critical = kind & 0x80 != 0;
            match kind & 0x7f {
                SUB_CREATED if hashed => {
                    if created.replace(sub.u32()?).is_some() {
                        return None;
                    }
                }
                SUB_EXPIRES if hashed => self.expires = sub.u32()?,
                SUB_ISSUER => self.issuer = Some(sub.take(8)?.try_into().ok()?),
                SUB_ISSUER_FINGERPRINT => {
                    // A v4 key's fingerprint; any other version can't be one of ours.
                    if sub.u8()? != 4 {
                        return None;
                    }
                    self.issuer_fingerprint = Some(sub.take(20)?.try_into().ok()?);
                }
                _ if critical => return None,
                _ => continue,
            }
            if !sub.is_empty() {
                return None;
            }
        }
        Some(())
    }

    /// Whether this signs `data` with `key`.
    fn verifies(&self, key: &PublicKey, data: &[u8]) -> bool {
        if !key.signs_at(self.created)
            || self.algorithm != key.algorithm()
            || self.issuer.is_some_and(|id| id != key.key_id())
            || self
                .issuer_fingerprint
                .is_some_and(|fp| fp != key.fingerprint)
            || self.expired()
        {
            return false;
        }
        let Some(digest) = self.digest(data) else {
            return false;
        };
        // A quick check first (§5.2.4): the hash's first two bytes are in the packet.
        if digest.as_ref().get(..2) != Some(&self.left16[..]) {
            return false;
        }
        match (&key.material, &self.mpis[..]) {
            (KeyMaterial::Rsa { n, e }, [sig]) => {
                let params = match self.hash {
                    SHA256 => &signature::RSA_PKCS1_2048_8192_SHA256,
                    SHA512 => &signature::RSA_PKCS1_2048_8192_SHA512,
                    _ => return false,
                };
                // As long as the modulus (the MPI dropped the leading zeros).
                let Some(sig) = left_pad(sig, n.len()) else {
                    return false;
                };
                RsaPublicKeyComponents { n, e }
                    .to_parsed_public_key(params)
                    .is_ok_and(|key| key.verify_digest_sig(&digest, &sig).is_ok())
            }
            (KeyMaterial::Ed25519(point), [r, s]) => {
                // Legacy EdDSA signs the hash; R and S are 32 bytes each.
                let (Some(mut rs), Some(s)) = (left_pad(r, 32), left_pad(s, 32)) else {
                    return false;
                };
                rs.extend_from_slice(&s);
                UnparsedPublicKey::new(&signature::ED25519, point)
                    .verify(digest.as_ref(), &rs)
                    .is_ok()
            }
            _ => false,
        }
    }

    /// The hash the signature is over: the data, the signature's hashed part and the v4 trailer
    /// (§5.2.4).
    fn digest(&self, data: &[u8]) -> Option<Digest> {
        let algorithm = match self.hash {
            SHA256 => &digest::SHA256,
            SHA512 => &digest::SHA512,
            _ => return None,
        };
        let mut hash = digest::Context::new(algorithm);
        hash.update(data);
        hash.update(self.hashed);
        hash.update(&[4, 0xff]);
        hash.update(&u32::try_from(self.hashed.len()).ok()?.to_be_bytes());
        Some(hash.finish())
    }

    /// Whether the signature has run out. Most never do (no expiry); one made "in the future"
    /// by a clock ahead of this one still counts.
    fn expired(&self) -> bool {
        if self.expires == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        now >= u64::from(self.created) + u64::from(self.expires)
    }
}

/// `bytes` with zeros in front up to `len` bytes; none if it's longer.
fn left_pad(bytes: &[u8], len: usize) -> Option<Vec<u8>> {
    let mut padded = vec![0; len.checked_sub(bytes.len())?];
    padded.extend_from_slice(bytes);
    Some(padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MDM_KEY: &[u8] = include_bytes!("../keys/mdm-release.asc");
    const YTDLP_KEY: &[u8] = include_bytes!("../keys/yt-dlp.asc");
    const MDM_SUMS: &[u8] = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt");
    const MDM_SIG: &[u8] = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt.sig");
    const YTDLP_SUMS: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS");
    const YTDLP_SIG: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS.sig");
    const BOTH: [(&[u8], &[u8], &[u8]); 2] = [
        (MDM_KEY, MDM_SUMS, MDM_SIG),
        (YTDLP_KEY, YTDLP_SUMS, YTDLP_SIG),
    ];

    /// `sig` in ASCII armor with the checksum `crc`.
    fn armor(sig: &[u8], crc: u32) -> String {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let body = b64(sig);
        let lines: Vec<&str> = (0..body.len())
            .step_by(64)
            .map(|at| &body[at..(at + 64).min(body.len())])
            .collect();
        format!(
            "-----BEGIN PGP SIGNATURE-----\r\n\r\n{}\r\n={}\r\n-----END PGP SIGNATURE-----\r\n",
            lines.join("\r\n"),
            b64(&crc.to_be_bytes()[1..])
        )
    }

    /// Where the unhashed subpackets of the signature packet `sig` are: the one part no
    /// signature covers.
    fn unhashed(sig: &[u8]) -> std::ops::Range<usize> {
        let header = 1 + [1, 2, 4][usize::from(sig[0] & 0x03)];
        let hashed = usize::from(u16::from_be_bytes([sig[header + 4], sig[header + 5]]));
        let at = header + 6 + hashed;
        let len = usize::from(u16::from_be_bytes([sig[at], sig[at + 1]]));
        at + 2..at + 2 + len
    }

    /// The fingerprints of the keys in `armored` that may sign at `when`.
    fn signers(armored: &[u8], when: u32) -> Vec<String> {
        public_keys(&unarmor(armored, "PUBLIC KEY BLOCK").unwrap())
            .unwrap()
            .iter()
            .filter(|k| k.signs_at(when))
            .map(|k| hex::encode_upper(k.fingerprint))
            .collect()
    }

    /// 2026-11-01, when the pinned keys were checked.
    const NOV_2026: u32 = 1_793_491_200;

    #[test]
    fn the_pinned_keys_have_the_fingerprints_we_expect() {
        let fingerprint = |armored| {
            let keys = signers(armored, NOV_2026);
            assert_eq!(keys.len(), 1);
            keys[0].clone()
        };
        assert_eq!(
            fingerprint(MDM_KEY),
            "5826D399249DE856EA087D802E293C28E8A69D74"
        );
        assert_eq!(
            fingerprint(YTDLP_KEY),
            "AC0CBBE6848D6A873464AF4E57CF65933B5A7581"
        );
    }

    /// Node.js's releasers' keys (crates/tools/keys/node): only their signing keys sign. Their
    /// encryption subkeys (cv25519, RSA "E") never do, nor Ulises Gascón's old "SEA" subkey after
    /// it expired on 2025-06-01; his newer signing subkey does.
    #[test]
    fn node_keys_sign_with_their_signing_keys_only() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("keys/node");
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let primary = name.trim_end_matches(".asc").to_owned();
            let key = std::fs::read(e.path()).unwrap();
            let mut want = vec![primary.clone()];
            if primary == "A363A499291CBBC940DD62E41F10027AF002F8B0" {
                want.push("C2B25D9B4272DB29565BA87F3C7824F39A895758".into());
            }
            let mut got = signers(&key, NOV_2026);
            got.sort();
            want.sort();
            assert_eq!(got, want, "{name}");
            seen += 1;
        }
        assert_eq!(seen, 8);
        // Before it expired, the old subkey signed too.
        let ulises =
            std::fs::read(dir.join("A363A499291CBBC940DD62E41F10027AF002F8B0.asc")).unwrap();
        assert!(
            signers(&ulises, 1_735_689_600) // 2025-01-01
                .contains(&"A6023530FC53461FEC91F99C04CD3F2FDE079578".to_owned())
        );
    }

    /// A signing key's self-signature decides: without the "sign" flag, or expired at the
    /// signature's time, or revoked, the real signature fails.
    #[test]
    fn key_flags_expiry_and_revocation_are_honoured() {
        let raw = unarmor(MDM_KEY, "PUBLIC KEY BLOCK").unwrap();
        let mut keys = public_keys(&raw).unwrap();
        assert_eq!(keys.len(), 1);
        let sig = Signature::parse(MDM_SIG).unwrap();
        assert!(sig.verifies(&keys[0], MDM_SUMS));
        keys[0].can_sign = false;
        assert!(!sig.verifies(&keys[0], MDM_SUMS));
        keys[0].can_sign = true;
        keys[0].expires = Some(u64::from(sig.created));
        assert!(!sig.verifies(&keys[0], MDM_SUMS), "expired as it signed");
        keys[0].expires = Some(u64::from(sig.created) + 1);
        assert!(sig.verifies(&keys[0], MDM_SUMS));
        keys[0].revoked = true;
        assert!(!sig.verifies(&keys[0], MDM_SUMS));
        // What a self-signature's subpackets say, read back.
        let flags_only = [
            4,
            SIG_SUBKEY_BINDING,
            22,
            8,
            0,
            9, //
            5,
            SUB_CREATED,
            0,
            0,
            0,
            100, //
            2,
            SUB_KEY_FLAGS,
            0x0c, // encrypt only
            0,
            0,
        ];
        let s = self_signature(&flags_only).unwrap();
        assert_eq!(
            (s.kind, s.created, s.flags),
            (SIG_SUBKEY_BINDING, 100, Some(0x0c))
        );
        let mut k = public_keys(&raw).unwrap().remove(0);
        let id: [u8; 8] = k.key_id().try_into().unwrap();
        apply_self_signatures(&mut k, &[s], false, &id);
        assert!(!k.can_sign, "an encryption subkey doesn't sign");
        // A subkey without a binding signature isn't the key's.
        apply_self_signatures(&mut k, &[], false, &id);
        assert!(!k.can_sign);
        // Another key's signature binds nothing.
        let mut other = flags_only;
        other[14] = FLAG_SIGN;
        let mut s = self_signature(&other).unwrap();
        s.issuer = Some([1; 8]);
        apply_self_signatures(&mut k, &[s], false, &id);
        assert!(!k.can_sign);
    }

    #[test]
    fn real_signatures_verify_and_any_change_fails_them() {
        for (key, sums, sig) in BOTH {
            assert!(verify(key, sums, sig));
            for at in [0, sums.len() / 2, sums.len() - 1] {
                let mut data = sums.to_vec();
                data[at] ^= 1;
                assert!(!verify(key, &data, sig), "data byte {at} changed");
            }
            // Every byte of the signature counts but the unhashed subpackets (the issuer, which
            // only names the key).
            let unhashed = unhashed(sig);
            for at in (0..sig.len()).filter(|at| !unhashed.contains(at)) {
                for flip in [0x01, 0x10, 0x80] {
                    let mut forged = sig.to_vec();
                    forged[at] ^= flip;
                    assert!(!verify(key, sums, &forged), "signature byte {at} changed");
                }
            }
            for len in 0..sig.len() {
                assert!(!verify(key, sums, &sig[..len]), "cut to {len} bytes");
            }
            assert!(!verify(key, sums, &[sig, &[0]].concat()));
            assert!(!verify(key, sums, &[sig, sig].concat()), "two signatures");
        }
    }

    #[test]
    fn an_issuer_that_names_another_key_fails() {
        for (key, sums, sig) in BOTH {
            let mut forged = sig.to_vec();
            forged[unhashed(sig).end - 1] ^= 1;
            assert!(!verify(key, sums, &forged));
        }
    }

    #[test]
    fn each_key_signs_for_itself_only() {
        assert!(!verify(YTDLP_KEY, MDM_SUMS, MDM_SIG));
        assert!(!verify(MDM_KEY, YTDLP_SUMS, YTDLP_SIG));
        assert!(!verify(MDM_KEY, MDM_SUMS, MDM_KEY));
        assert!(!verify(MDM_SIG, MDM_SUMS, MDM_SIG));
    }

    #[test]
    fn armor_is_checked_by_its_checksum() {
        let armored = armor(MDM_SIG, crc24(MDM_SIG));
        assert!(verify(MDM_KEY, MDM_SUMS, armored.as_bytes()));
        let wrong = armor(MDM_SIG, crc24(MDM_SIG) ^ 1);
        assert!(!verify(MDM_KEY, MDM_SUMS, wrong.as_bytes()));
        // A damaged key: "mDME" begins its data.
        let key = String::from_utf8(MDM_KEY.to_vec())
            .unwrap()
            .replacen("mDME", "mDMF", 1);
        assert!(!verify(key.as_bytes(), MDM_SUMS, MDM_SIG));
    }

    #[test]
    fn garbage_fails_without_panicking() {
        // xorshift64: the same "random" bytes on every run.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..20_000 {
            let len = (next() % 600) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Half begin like the real signature, to get past the first checks.
            if round % 2 == 0 && len >= 6 {
                bytes[..6].copy_from_slice(&YTDLP_SIG[..6]);
            }
            assert!(!verify(YTDLP_KEY, YTDLP_SUMS, &bytes));
            assert!(!verify(&bytes, YTDLP_SUMS, YTDLP_SIG));
            let text: String = bytes.iter().map(|&b| char::from(b'+' + b % 80)).collect();
            let armored = format!("-----BEGIN PGP SIGNATURE-----\n\n{text}\n=");
            assert!(!verify(MDM_KEY, MDM_SUMS, armored.as_bytes()));
        }
        // The real keys and signatures, each byte damaged in turn, and cut short.
        for (key, sums, sig) in BOTH {
            let key = unarmor(key, "PUBLIC KEY BLOCK").unwrap();
            for at in 0..key.len() {
                let mut bad = key.clone();
                bad[at] = next() as u8;
                let _ = verify(&bad, sums, sig);
                let _ = verify(&key[..at], sums, sig);
            }
            for at in 0..sig.len() {
                let mut bad = sig.to_vec();
                bad[at] = 0xff;
                let _ = verify(&key, sums, &bad);
            }
        }
    }
}
