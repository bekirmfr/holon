//! OpenPGP with a public key (RFC 4880, RFC 6637, and RFC 9580's version 4 forms): a message
//! encrypted to a key, a detached signature checked, and what a key says read
//! (host-capabilities L4 §5.1.11). Version 4 keys of the kinds GnuPG makes, RSA and Curve25519;
//! anything else gives nothing. Written by hand over `rsa`, `ed25519-dalek`, `curve25519-dalek`,
//! `aes`, `sha1` and `sha2`, so that every random byte [`encrypt`] uses is drawn here, from the
//! run's: `holon verify` recomputes a message byte for byte.

use std::collections::BTreeMap;

use aes::cipher::{BlockEncrypt, KeyInit};
use curve25519_dalek::montgomery::MontgomeryPoint;
use rsa::traits::PublicKeyParts as _;
use rsa::{BigUint, Pkcs1v15Sign, RsaPublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

use crate::crypto::{base64_decode, base64_encode, hex_encode};
use crate::Value;

/// `armor`'s values: the message as its bytes, or as ASCII armor's text (in bytes).
pub(crate) const ARMORS: &[&str] = &["binary", "ascii"];

/// An RSA key's least size, as for JWTs; `rsa` refuses one past 4,096 bits.
const RSA_MIN_BITS: usize = 2048;

/// The most `encrypt` takes: a literal packet's length is four bytes.
const MOST: usize = 1 << 31;

// Curve OIDs as a key packet writes them (RFC 9580 §9.2): Ed25519 (legacy EdDSA) and Curve25519
// (legacy ECDH).
const ED25519: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];
const CV25519: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01];

// Public-key algorithms (RFC 9580 §9.1).
const RSA_BOTH: u8 = 1;
const RSA_ENCRYPT: u8 = 2;
const RSA_SIGN: u8 = 3;
const ECDH: u8 = 18;
const EDDSA: u8 = 22;

// Symmetric algorithms (§9.3) and hashes (§9.5).
const AES128: u8 = 7;
const AES192: u8 = 8;
const AES256: u8 = 9;
const SHA1: u8 = 2;
const SHA256: u8 = 8;
const SHA384: u8 = 9;
const SHA512: u8 = 10;
const SHA224: u8 = 11;

// Key flags (§5.2.3.29).
const SIGN: u8 = 0x02;
const ENCRYPT: u8 = 0x0C;

// ── packets ──

struct Packet {
    tag: u8,
    body: Vec<u8>,
}

/// The packets of `b`, in either header format (§4.2), partial body lengths joined; `None` for
/// bytes that aren't whole packets.
fn packets(mut b: &[u8]) -> Option<Vec<Packet>> {
    let mut out = vec![];
    while let Some((&first, rest)) = b.split_first() {
        if first & 0x80 == 0 {
            return None;
        }
        b = rest;
        if first & 0x40 != 0 {
            let mut body = vec![];
            loop {
                let (len, partial, rest) = length(b, true)?;
                body.extend_from_slice(rest.get(..len)?);
                b = &rest[len..];
                if !partial {
                    break;
                }
            }
            out.push(Packet { tag: first & 0x3F, body });
        } else {
            let (len, rest) = match first & 3 {
                0 => (*b.first()? as usize, &b[1..]),
                1 => (u16::from_be_bytes(b.get(..2)?.try_into().ok()?) as usize, &b[2..]),
                2 => (u32::from_be_bytes(b.get(..4)?.try_into().ok()?) as usize, &b[4..]),
                _ => return None,
            };
            out.push(Packet { tag: first >> 2 & 0x0F, body: rest.get(..len)?.to_vec() });
            b = &rest[len..];
        }
    }
    Some(out)
}

/// A new-format packet's length, or a subpacket's (§4.2.1, §5.2.3.7): its value, whether it is a
/// partial body's (only a packet's may be), and what follows it.
fn length(b: &[u8], packet: bool) -> Option<(usize, bool, &[u8])> {
    let first = *b.first()? as usize;
    Some(match first {
        0..=191 => (first, false, &b[1..]),
        192..=223 => (((first - 192) << 8) + *b.get(1)? as usize + 192, false, b.get(2..)?),
        224..=254 if packet => (1 << (first & 0x1F), true, &b[1..]),
        224..=254 => (((first - 192) << 8) + *b.get(1)? as usize + 192, false, b.get(2..)?),
        _ => (u32::from_be_bytes(b.get(1..5)?.try_into().ok()?) as usize, false, b.get(5..)?),
    })
}

/// A packet with a new-format header and a definite length.
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xC0 | tag];
    match body.len() {
        n @ 0..=191 => out.push(n as u8),
        n @ 192..=8383 => out.extend_from_slice(&[((n - 192) >> 8) as u8 + 192, (n - 192) as u8]),
        n => {
            out.push(0xFF);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(body);
    out
}

/// A multiprecision integer (§3.2) of `bytes`, big-endian, its leading zeros left out.
fn mpi(bytes: &[u8]) -> Vec<u8> {
    let bytes = &bytes[bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len())..];
    let bits = bytes.first().map_or(0, |&b| (bytes.len() - 1) * 8 + (8 - b.leading_zeros() as usize));
    [&(bits as u16).to_be_bytes()[..], bytes].concat()
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (taken, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(taken)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    /// A multiprecision integer's bytes, as written.
    fn mpi(&mut self) -> Option<&'a [u8]> {
        let bits = self.u16()? as usize;
        self.take(bits.div_ceil(8))
    }
    /// A length octet, then that many bytes (a curve's OID, ECDH's KDF parameters).
    fn short(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()? as usize;
        self.take(n)
    }
}

// ── armor (§6) ──

/// CRC-24, armor's checksum (§6.1).
fn crc24(bytes: &[u8]) -> u32 {
    let mut crc = 0xB7_04CE_u32;
    for &b in bytes {
        crc ^= (b as u32) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x100_0000 != 0 {
                crc ^= 0x186_4CFB;
            }
        }
    }
    crc & 0xFF_FFFF
}

/// The bytes of each armored block of `kind` in `text` ("PUBLIC KEY BLOCK", "SIGNATURE",
/// "MESSAGE"), joined; a checksum, when given, must match. `None` without such a block.
fn dearmor(text: &str, kind: &str) -> Option<Vec<u8>> {
    let (begin, end) = (format!("-----BEGIN PGP {kind}-----"), format!("-----END PGP {kind}-----"));
    let standard = crate::crypto::alphabet("standard")?;
    let mut lines = text.lines().map(str::trim);
    let (mut out, mut found) = (vec![], false);
    while let Some(line) = lines.next() {
        if line != begin {
            continue;
        }
        found = true;
        // Its headers (`Comment: …`), to the blank line.
        while !lines.next()?.is_empty() {}
        let (mut body, mut sum) = (String::new(), None);
        loop {
            let line = lines.next()?;
            if line == end {
                break;
            }
            match line.strip_prefix('=') {
                Some(s) if s.len() == 4 => sum = Some(s),
                _ => body.push_str(line),
            }
        }
        let bytes = base64_decode(&body, standard)?;
        if let Some(sum) = sum {
            let sum = base64_decode(sum, standard)?;
            if sum.len() != 3 || u32::from_be_bytes([0, sum[0], sum[1], sum[2]]) != crc24(&bytes) {
                return None;
            }
        }
        out.extend(bytes);
    }
    found.then_some(out)
}

/// `bytes` as an armored block of `kind`: lines of 64, its checksum after them, as GnuPG writes.
fn armor(kind: &str, bytes: &[u8]) -> String {
    let standard = crate::crypto::alphabet("standard").expect("the standard alphabet");
    let encoded = base64_encode(bytes, standard, true);
    let mut s = format!("-----BEGIN PGP {kind}-----\n\n");
    for line in encoded.as_bytes().chunks(64) {
        s.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        s.push('\n');
    }
    s.push('=');
    s.push_str(&base64_encode(&crc24(bytes).to_be_bytes()[1..], standard, true));
    s.push_str(&format!("\n-----END PGP {kind}-----\n"));
    s
}

/// The bytes of `value`: a text is read as armor of `kind`, and so are bytes that begin as armor
/// does; other bytes are taken as they are.
fn unarmored(value: &Value, kind: &str) -> Option<Vec<u8>> {
    match value {
        Value::Text(t) => dearmor(t, kind),
        Value::Bytes(b) if b.starts_with(b"-----BEGIN PGP ") => dearmor(std::str::from_utf8(b).ok()?, kind),
        Value::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

/// What a leaf takes as data: a text's UTF-8, or bytes.
fn data_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Text(t) => Some(t.as_bytes().to_vec()),
        Value::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

/// `now` as OpenPGP's time: whole seconds that fit four bytes.
fn seconds(now: f64) -> Option<u32> {
    (now.is_finite() && (0.0..4_294_967_296.0).contains(&now)).then(|| now.floor() as u32)
}

// ── keys ──

enum Material {
    Rsa(RsaPublicKey),
    Ed25519(ed25519_dalek::VerifyingKey),
    /// Its point, and the hash and key wrap its KDF names (RFC 6637 §9).
    Cv25519 { point: [u8; 32], hash: u8, wrap: u8 },
    /// A kind this doesn't use: read past, never chosen.
    Other,
}

struct Key {
    /// The packet's public part, as fingerprints and signatures hash it.
    body: Vec<u8>,
    created: u32,
    algorithm: u8,
    material: Material,
    fingerprint: [u8; 20],
}

impl Key {
    fn key_id(&self) -> &[u8] {
        &self.fingerprint[12..]
    }
    /// The key as signatures over it frame it (§5.2.4): 0x99, its length, its body.
    fn framed(&self) -> Vec<u8> {
        [&[0x99][..], &(self.body.len() as u16).to_be_bytes(), &self.body].concat()
    }
    fn encrypts(&self) -> bool {
        matches!((&self.material, self.algorithm), (Material::Rsa(_), RSA_BOTH | RSA_ENCRYPT) | (Material::Cv25519 { .. }, ECDH))
    }
    fn signs(&self) -> bool {
        matches!((&self.material, self.algorithm), (Material::Rsa(_), RSA_BOTH | RSA_SIGN) | (Material::Ed25519(_), EDDSA))
    }
}

/// A version 4 key packet's public part (§5.5.2), and what follows it (a secret key's part).
fn key(body: &[u8]) -> Option<(Key, &[u8])> {
    let mut r = Reader(body);
    if r.u8()? != 4 {
        return None;
    }
    let created = r.u32()?;
    let algorithm = r.u8()?;
    let material = match algorithm {
        RSA_BOTH | RSA_ENCRYPT | RSA_SIGN => {
            let (n, e) = (BigUint::from_bytes_be(r.mpi()?), BigUint::from_bytes_be(r.mpi()?));
            match RsaPublicKey::new(n, e) {
                Ok(k) if k.n().bits() >= RSA_MIN_BITS => Material::Rsa(k),
                _ => Material::Other,
            }
        }
        EDDSA => {
            let (oid, point) = (r.short()?, r.mpi()?);
            match (oid == ED25519, point) {
                (true, [0x40, p @ ..]) => p.try_into().ok().and_then(|p| ed25519_dalek::VerifyingKey::from_bytes(p).ok()).map_or(Material::Other, Material::Ed25519),
                _ => Material::Other,
            }
        }
        ECDH => {
            let (oid, point, kdf) = (r.short()?, r.mpi()?, r.short()?);
            match (oid == CV25519, point, kdf) {
                (true, [0x40, p @ ..], &[1, hash, wrap]) if matches!(hash, SHA256 | SHA384 | SHA512) && matches!(wrap, AES128 | AES192 | AES256) => {
                    p.try_into().map_or(Material::Other, |point| Material::Cv25519 { point, hash, wrap })
                }
                _ => Material::Other,
            }
        }
        _ => {
            // Its fields aren't known: the whole body is its public part.
            r.0 = &[];
            Material::Other
        }
    };
    let public = &body[..body.len() - r.0.len()];
    let framed = [&[0x99][..], &u16::try_from(public.len()).ok()?.to_be_bytes(), public].concat();
    let fingerprint = Sha1::digest(&framed).into();
    Some((Key { body: public.to_vec(), created, algorithm, material, fingerprint }, r.0))
}

// ── signatures ──

struct Signature {
    typ: u8,
    algorithm: u8,
    hash: u8,
    /// Its version through its hashed subpackets: what it signs after the data.
    hashed: Vec<u8>,
    left: [u8; 2],
    mpis: Vec<Vec<u8>>,
    created: Option<u32>,
    /// Seconds after `created` it stops being valid (§5.2.3.18); 0 or absent, never.
    expires: Option<u32>,
    /// Seconds after the key's creation the key stops being valid (§5.2.3.13).
    key_expires: Option<u32>,
    flags: Option<u8>,
    /// The issuer's fingerprint, or its key ID: which key to try.
    issuer: Option<Vec<u8>>,
    /// An embedded signature's body: a signing subkey's back-signature.
    embedded: Option<Vec<u8>>,
}

/// A version 4 signature packet (§5.2.3); `None` for another version, or one whose hashed area
/// holds a critical subpacket this doesn't know.
fn signature(body: &[u8]) -> Option<Signature> {
    let mut r = Reader(body);
    if r.u8()? != 4 {
        return None;
    }
    let (typ, algorithm, hash) = (r.u8()?, r.u8()?, r.u8()?);
    let n = r.u16()? as usize;
    let hashed_area = r.take(n)?;
    let n = r.u16()? as usize;
    let unhashed_area = r.take(n)?;
    let left = r.take(2)?.try_into().ok()?;
    let mpis = match algorithm {
        RSA_BOTH | RSA_SIGN => vec![r.mpi()?.to_vec()],
        EDDSA => vec![r.mpi()?.to_vec(), r.mpi()?.to_vec()],
        _ => vec![],
    };
    let mut s = Signature {
        typ,
        algorithm,
        hash,
        hashed: body[..6 + hashed_area.len()].to_vec(),
        left,
        mpis,
        created: None,
        expires: None,
        key_expires: None,
        flags: None,
        issuer: None,
        embedded: None,
    };
    let (mut fingerprint, mut key_id) = (None, None);
    for (area, hashed) in [(hashed_area, true), (unhashed_area, false)] {
        let mut r = Reader(area);
        while !r.0.is_empty() {
            let (len, _, rest) = length(r.0, false)?;
            r.0 = rest;
            let (&kind, data) = r.take(len)?.split_first()?;
            let time = || Some(u32::from_be_bytes(data.try_into().ok()?));
            match (kind & 0x7F, hashed) {
                (2, true) => s.created = time(),
                (3, true) => s.expires = time(),
                (9, true) => s.key_expires = time(),
                (27, true) => s.flags = data.first().copied(),
                (33, _) if data.len() == 21 && data[0] == 4 => fingerprint = Some(data[1..].to_vec()),
                (16, _) if data.len() == 8 => key_id = Some(data.to_vec()),
                (32, _) => s.embedded = Some(data.to_vec()),
                // What a key prefers and says of itself, which signs nothing this reads.
                (4..=7 | 11 | 12 | 20..=26 | 28..=31 | 34 | 35 | 37 | 39, _) => {}
                _ if hashed && kind & 0x80 != 0 => return None,
                _ => {}
            }
        }
    }
    s.issuer = fingerprint.or(key_id);
    Some(s)
}

fn digest(hash: u8, parts: &[&[u8]]) -> Option<Vec<u8>> {
    fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut d = D::new();
        for p in parts {
            d.update(p);
        }
        d.finalize().to_vec()
    }
    Some(match hash {
        SHA1 => run::<Sha1>(parts),
        SHA224 => run::<Sha224>(parts),
        SHA256 => run::<Sha256>(parts),
        SHA384 => run::<Sha384>(parts),
        SHA512 => run::<Sha512>(parts),
        _ => return None,
    })
}

/// Whether `sig` over `signed` (what precedes its own hashed part, §5.2.4) verifies with `key`.
fn holds(key: &Key, sig: &Signature, signed: &[&[u8]]) -> bool {
    let trailer = [&[4, 0xFF][..], &(sig.hashed.len() as u32).to_be_bytes()].concat();
    let Some(d) = digest(sig.hash, &[signed, &[&sig.hashed[..], &trailer]].concat()) else { return false };
    if d[..2] != sig.left {
        return false;
    }
    match (&key.material, sig.algorithm, sig.mpis.as_slice()) {
        (Material::Rsa(k), RSA_BOTH | RSA_SIGN, [s]) if s.len() <= k.size() => {
            let s = [vec![0; k.size() - s.len()], s.clone()].concat();
            let scheme = match sig.hash {
                SHA1 => Pkcs1v15Sign::new::<Sha1>(),
                SHA224 => Pkcs1v15Sign::new::<Sha224>(),
                SHA256 => Pkcs1v15Sign::new::<Sha256>(),
                SHA384 => Pkcs1v15Sign::new::<Sha384>(),
                _ => Pkcs1v15Sign::new::<Sha512>(),
            };
            k.verify(scheme, &d, &s).is_ok()
        }
        // EdDSA signs the digest (RFC 9580 §5.2.3.3), of 256 bits at least; r and s are 32 bytes.
        (Material::Ed25519(k), EDDSA, [r, s]) if matches!(sig.hash, SHA256 | SHA384 | SHA512) && r.len() <= 32 && s.len() <= 32 => {
            let mut rs = [0u8; 64];
            rs[32 - r.len()..32].copy_from_slice(r);
            rs[64 - s.len()..].copy_from_slice(s);
            k.verify_strict(&d, &ed25519_dalek::Signature::from_bytes(&rs)).is_ok()
        }
        _ => false,
    }
}

/// Whether `sig` names `key` as its issuer, or names none.
fn names(sig: &Signature, key: &Key) -> bool {
    sig.issuer.as_deref().is_none_or(|i| i == key.fingerprint || i == key.key_id())
}

// ── certificates ──

/// A transferable public key as written (§10.1): the primary key, its own signatures, its user
/// IDs and subkeys with theirs.
struct Cert {
    primary: Key,
    direct: Vec<Signature>,
    users: Vec<(Vec<u8>, Vec<Signature>)>,
    subkeys: Vec<(Key, Vec<Signature>)>,
}

/// The keys `bytes` holds, each a primary key and what follows it; `None` for what isn't, or for
/// a secret key.
fn certs(bytes: &[u8]) -> Option<Vec<Cert>> {
    enum At {
        Direct,
        User,
        Subkey,
        Skipped,
    }
    let (mut out, mut at): (Vec<Cert>, At) = (vec![], At::Skipped);
    for p in packets(bytes)? {
        match p.tag {
            6 => {
                let (primary, _) = key(&p.body)?;
                out.push(Cert { primary, direct: vec![], users: vec![], subkeys: vec![] });
                at = At::Direct;
            }
            13 => {
                out.last_mut()?.users.push((p.body, vec![]));
                at = At::User;
            }
            14 => match key(&p.body) {
                Some((k, _)) => {
                    out.last_mut()?.subkeys.push((k, vec![]));
                    at = At::Subkey;
                }
                None => at = At::Skipped,
            },
            2 => {
                let (Some(c), Some(sig)) = (out.last_mut(), signature(&p.body)) else { continue };
                match at {
                    At::Direct => c.direct.push(sig),
                    At::User => c.users.last_mut()?.1.push(sig),
                    At::Subkey => c.subkeys.last_mut()?.1.push(sig),
                    At::Skipped => {}
                }
            }
            5 | 7 => return None,
            // A user attribute (a photo): its signatures are its own.
            17 => at = At::Skipped,
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The keys `keys` holds: one key, armored or bytes, or a list of them.
fn all_certs(keys: &Value) -> Option<Vec<Cert>> {
    let one = |k: &Value| certs(&unarmored(k, "PUBLIC KEY BLOCK")?);
    match keys {
        Value::List(items) if !items.is_empty() => items.iter().map(one).collect::<Option<Vec<_>>>().map(|c| c.into_iter().flatten().collect()),
        Value::List(_) => None,
        k => one(k),
    }
}

/// A key or subkey as its signatures make it: what its flags let it do, until when, and whether
/// its primary revoked it.
struct Usable<'a> {
    key: &'a Key,
    flags: u8,
    expires: Option<u64>,
    revoked: bool,
}

impl Usable<'_> {
    fn valid_at(&self, t: u64) -> bool {
        !self.revoked && self.key.created as u64 <= t && self.expires.is_none_or(|e| t < e)
    }
}

struct Checked<'a> {
    primary: Usable<'a>,
    user_ids: Vec<String>,
    subkeys: Vec<Usable<'a>>,
}

impl Checked<'_> {
    /// The primary and each subkey valid at `t`, a subkey only while its primary is.
    fn valid_at(&self, t: u64) -> impl Iterator<Item = &Usable<'_>> {
        let primary = self.primary.valid_at(t);
        std::iter::once(&self.primary).chain(&self.subkeys).filter(move |u| primary && u.valid_at(t))
    }
}

/// The flags a key's algorithm gives when its signature names none.
fn flags_of(key: &Key) -> u8 {
    (if key.signs() { SIGN } else { 0 }) | (if key.encrypts() { ENCRYPT } else { 0 })
}

fn expiry(key: &Key, sig: &Signature) -> Option<u64> {
    sig.key_expires.filter(|&e| e != 0).map(|e| key.created as u64 + e as u64)
}

/// `cert` as its own signatures make it (§5.2.1, §10.1): the primary as its newest self-signature
/// says, the user IDs it certified, each subkey it bound (a signing one only with the subkey's
/// back-signature), less what it revoked. SHA-1 counts here, as GnuPG lets it (fork 48). `None`
/// when no self-signature holds.
fn checked(cert: &Cert) -> Option<Checked<'_>> {
    let p = &cert.primary;
    let pk = p.framed();
    let by_primary = |sig: &&Signature, signed: &[&[u8]]| names(sig, p) && holds(p, sig, signed);
    let mut newest: Option<&Signature> = None;
    let mut user_ids = vec![];
    let newer = |a: Option<&'_ Signature>, b: &'_ Signature| -> bool { a.is_none_or(|a| b.created >= a.created) };
    for (uid, sigs) in &cert.users {
        let framed = [&[0xB4][..], &(uid.len() as u32).to_be_bytes(), uid].concat();
        let signed: &[&[u8]] = &[&pk, &framed];
        let certified = sigs.iter().filter(|s| matches!(s.typ, 0x10..=0x13) && by_primary(s, signed)).max_by_key(|s| s.created);
        let revoked = sigs.iter().any(|s| s.typ == 0x30 && by_primary(&s, signed));
        if let (Some(c), false) = (certified, revoked) {
            user_ids.push(String::from_utf8_lossy(uid).into_owned());
            if newer(newest, c) {
                newest = Some(c);
            }
        }
    }
    for s in cert.direct.iter().filter(|s| s.typ == 0x1F && by_primary(s, &[&pk])) {
        if newer(newest, s) {
            newest = Some(s);
        }
    }
    let newest = newest?;
    let primary = Usable {
        key: p,
        flags: newest.flags.unwrap_or_else(|| flags_of(p)),
        expires: expiry(p, newest),
        revoked: cert.direct.iter().any(|s| s.typ == 0x20 && by_primary(&s, &[&pk])),
    };
    let subkeys = cert
        .subkeys
        .iter()
        .filter_map(|(k, sigs)| {
            let sk = k.framed();
            let signed: &[&[u8]] = &[&pk, &sk];
            let binding = sigs.iter().filter(|s| s.typ == 0x18 && by_primary(s, signed)).max_by_key(|s| s.created)?;
            let mut flags = binding.flags.unwrap_or_else(|| flags_of(k));
            // A subkey signs only if it says it belongs to this primary (§5.2.1, 0x19): else
            // another key's owner could bind it and claim its signatures.
            let backed = binding.embedded.as_deref().and_then(signature).is_some_and(|b| b.typ == 0x19 && holds(k, &b, signed));
            if !backed {
                flags &= !SIGN;
            }
            let revoked = sigs.iter().any(|s| s.typ == 0x28 && by_primary(&s, signed));
            Some(Usable { key: k, flags, expires: expiry(k, binding), revoked })
        })
        .collect();
    Some(Checked { primary, user_ids, subkeys })
}

fn fingerprint(key: &Key) -> Value {
    Value::Text(hex_encode(&key.fingerprint).to_uppercase())
}

// ── the leaves ──

/// `pgp_key`: what one key says of itself, as its own signatures bear out:
/// `{fingerprint, user_ids, created, expires, revoked}`, `expires` absent when it doesn't.
pub(crate) fn key_info(key: &Value) -> Option<Value> {
    let certs = all_certs(key)?;
    let [cert] = certs.as_slice() else { return None };
    let c = checked(cert)?;
    let mut out = BTreeMap::from([
        ("fingerprint".to_string(), fingerprint(c.primary.key)),
        ("user_ids".to_string(), Value::List(c.user_ids.iter().cloned().map(Value::Text).collect())),
        ("created".to_string(), Value::Number(c.primary.key.created as f64)),
        ("revoked".to_string(), Value::Boolean(c.primary.revoked)),
    ]);
    if let Some(e) = c.primary.expires {
        out.insert("expires".into(), Value::Number(e as f64));
    }
    Some(Value::Record(out))
}

/// `pgp_verify`: `{signer, key, created}` for the first of `signature`'s signatures over `data`
/// that verifies with a key among `keys` able to sign and valid when it signed, made no later
/// than `now` and not expired at it; `None` when none does.
pub(crate) fn verify(data: &Value, signature_value: &Value, keys: &Value, now: f64) -> Option<Value> {
    let (data, now) = (data_bytes(data)?, seconds(now)? as u64);
    let certs = all_certs(keys)?;
    let checked: Vec<Checked> = certs.iter().filter_map(checked).collect();
    let mut text = None;
    for p in packets(&unarmored(signature_value, "SIGNATURE")?)? {
        if p.tag != 2 {
            return None;
        }
        let Some(sig) = signature(&p.body) else { continue };
        let Some(created) = sig.created.map(u64::from) else { continue };
        let fresh = created <= now && sig.expires.filter(|&e| e != 0).is_none_or(|e| now < created + e as u64);
        if !fresh || !matches!(sig.hash, SHA224 | SHA256 | SHA384 | SHA512) {
            continue;
        }
        let signed = match sig.typ {
            0x00 => &data,
            0x01 => text.get_or_insert_with(|| crlf(&data)),
            _ => continue,
        };
        for c in &checked {
            if let Some(u) = c.valid_at(created).find(|u| u.flags & SIGN != 0 && u.key.signs() && names(&sig, u.key) && holds(u.key, &sig, &[signed])) {
                return Some(Value::Record(BTreeMap::from([
                    ("signer".to_string(), fingerprint(c.primary.key)),
                    ("key".to_string(), fingerprint(u.key)),
                    ("created".to_string(), Value::Number(created as f64)),
                ])));
            }
        }
    }
    None
}

/// A text signature's data (§5.2.1.2): each line ending made CR LF.
fn crlf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 32);
    for (i, &b) in data.iter().enumerate() {
        if b == b'\n' && (i == 0 || data[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// The randomness `encrypt` draws on: BLAKE3, keyed with the run's random bytes, over the leaf's
/// inputs, so another message, or another run, never shares a session key.
struct Stream(blake3::OutputReader);

impl Stream {
    fn new(random: &[u8], parts: &[&[u8]]) -> Stream {
        let key = blake3::derive_key("holon pgp_encrypt 2026-11-08 randomness", random);
        let mut h = blake3::Hasher::new_keyed(&key);
        for p in parts {
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p);
        }
        Stream(h.finalize_xof())
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0; N];
        self.0.fill(&mut b);
        b
    }
    /// `n` bytes none of which is zero, as PKCS #1 v1.5's padding is.
    fn nonzero(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let [b] = self.bytes::<1>();
            if b != 0 {
                out.push(b);
            }
        }
        out
    }
}

/// `pgp_encrypt`: `data` encrypted to each of `keys` (its newest encryption key valid at `now`),
/// as GnuPG writes a message: a session key packet for each, then AES-256 with the modification
/// detection code over a literal data packet dated `now`. `random` (32 bytes at least) decides
/// every random byte. `None` when a key has no encryption key valid at `now`.
pub(crate) fn encrypt(data: &Value, keys: &Value, random: &[u8], now: f64, armored: bool) -> Option<Vec<u8>> {
    let (data, now) = (data_bytes(data)?, seconds(now)?);
    if random.len() < 32 || data.len() > MOST {
        return None;
    }
    let certs = all_certs(keys)?;
    let checked: Vec<Checked> = certs.iter().map(checked).collect::<Option<_>>()?;
    let recipients: Vec<&Key> = checked
        .iter()
        .map(|c| c.valid_at(now as u64).filter(|u| u.flags & ENCRYPT != 0 && u.key.encrypts()).max_by_key(|u| u.key.created).map(|u| u.key))
        .collect::<Option<_>>()?;
    let fingerprints: Vec<&[u8]> = recipients.iter().map(|k| &k.fingerprint[..]).collect();
    let mut rng = Stream::new(random, &[&data, &fingerprints.concat(), &now.to_be_bytes(), &[armored as u8]]);
    let session: [u8; 32] = rng.bytes();
    let mut message = vec![];
    for k in recipients {
        message.extend(session_packet(k, &session, &mut rng)?);
    }
    // The literal data (§5.9): binary, no file name (fork 49), dated now.
    let literal = packet(11, &[&[b'b', 0][..], &now.to_be_bytes(), &data].concat());
    let prefix: [u8; 16] = rng.bytes();
    let mut plain = [&prefix[..], &prefix[14..], &literal, &[0xD3, 0x14]].concat();
    let mdc = Sha1::digest(&plain);
    plain.extend_from_slice(&mdc);
    cfb(&session, &mut plain);
    message.extend(packet(18, &[&[1][..], &plain].concat()));
    Some(if armored { armor("MESSAGE", &message).into_bytes() } else { message })
}

/// A public-key encrypted session key packet, version 3 (§5.1): the session key, its algorithm
/// and checksum, encrypted to `key`.
fn session_packet(key: &Key, session: &[u8; 32], rng: &mut Stream) -> Option<Vec<u8>> {
    let checksum = session.iter().fold(0u16, |s, &b| s.wrapping_add(b as u16));
    let m = [&[AES256][..], session, &checksum.to_be_bytes()].concat();
    let encrypted = match &key.material {
        // PKCS #1 v1.5 (RFC 8017 §7.2.1): 00 02, nonzero padding, 00, the message.
        Material::Rsa(k) => {
            let em = [&[0, 2][..], &rng.nonzero(k.size() - m.len() - 3), &[0], &m].concat();
            mpi(&BigUint::from_bytes_be(&em).modpow(k.e(), k.n()).to_bytes_be())
        }
        // ECDH (RFC 6637 §8): an ephemeral X25519 key, the shared point through the KDF, the
        // message padded to 8 bytes and wrapped.
        Material::Cv25519 { point, hash, wrap } => {
            let secret: [u8; 32] = rng.bytes();
            let shared = MontgomeryPoint(*point).mul_clamped(secret);
            if shared.0 == [0; 32] {
                return None;
            }
            let ephemeral = MontgomeryPoint::mul_base_clamped(secret);
            let kek = kdf(&shared.0, *hash, *wrap, &key.fingerprint)?;
            let pad = 8 - m.len() % 8;
            let wrapped = key_wrap(&kek, &[&m[..], &vec![pad as u8; pad]].concat())?;
            [mpi(&[&[0x40][..], &ephemeral.0].concat()), vec![wrapped.len() as u8], wrapped].concat()
        }
        _ => return None,
    };
    Some(packet(1, &[&[3][..], key.key_id(), &[key.algorithm], &encrypted].concat()))
}

/// ECDH's key-encryption key (RFC 6637 §7): the shared point hashed with the recipient's
/// parameters and fingerprint.
fn kdf(shared: &[u8], hash: u8, wrap: u8, fingerprint: &[u8; 20]) -> Option<Vec<u8>> {
    let param = [&[CV25519.len() as u8][..], CV25519, &[ECDH, 3, 1, hash, wrap], b"Anonymous Sender    ", fingerprint].concat();
    let d = digest(hash, &[&[0, 0, 0, 1], shared, &param])?;
    Some(d[..aes_len(wrap)?].to_vec())
}

fn aes_len(algorithm: u8) -> Option<usize> {
    match algorithm {
        AES128 => Some(16),
        AES192 => Some(24),
        AES256 => Some(32),
        _ => None,
    }
}

/// One AES block encrypted in place, with a key of 16, 24 or 32 bytes.
fn aes_block(key: &[u8], block: &mut [u8; 16]) -> Option<()> {
    let b = aes::Block::from_mut_slice(block);
    match key.len() {
        16 => aes::Aes128::new_from_slice(key).ok()?.encrypt_block(b),
        24 => aes::Aes192::new_from_slice(key).ok()?.encrypt_block(b),
        32 => aes::Aes256::new_from_slice(key).ok()?.encrypt_block(b),
        _ => return None,
    }
    Some(())
}

/// AES key wrap (RFC 3394 §2.2.1), of whole 8-byte blocks.
fn key_wrap(kek: &[u8], plain: &[u8]) -> Option<Vec<u8>> {
    let n = plain.len() / 8;
    let mut a = [0xA6u8; 8];
    let mut r: Vec<[u8; 8]> = plain.chunks_exact(8).map(|c| c.try_into().expect("8 bytes")).collect();
    for j in 0..6 {
        for (i, ri) in r.iter_mut().enumerate() {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&a);
            block[8..].copy_from_slice(ri);
            aes_block(kek, &mut block)?;
            let t = (n * j + i + 1) as u64;
            a = (u64::from_be_bytes(block[..8].try_into().expect("8 bytes")) ^ t).to_be_bytes();
            ri.copy_from_slice(&block[8..]);
        }
    }
    Some([&a[..], &r.concat()].concat())
}

/// AES-256 in CFB mode with a zero IV, in place: the integrity protected packet's (§5.13.1).
fn cfb(key: &[u8; 32], data: &mut [u8]) {
    let cipher = aes::Aes256::new(key.into());
    let mut register = [0u8; 16];
    for chunk in data.chunks_mut(16) {
        let mut stream = aes::Block::from(register);
        cipher.encrypt_block(&mut stream);
        for (b, s) in chunk.iter_mut().zip(stream) {
            *b ^= s;
        }
        register[..chunk.len()].copy_from_slice(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockDecrypt;

    /// Keys, signatures and messages GnuPG made: `node testdata/pgp-vectors.mjs` writes them again.
    fn vectors() -> BTreeMap<String, Value> {
        match crate::json::parse(include_str!("testdata/pgp-vectors.json")).unwrap() {
            Value::Record(r) => r,
            _ => unreachable!(),
        }
    }

    fn at<'a>(v: &'a BTreeMap<String, Value>, path: &[&str]) -> &'a Value {
        let mut here = &v[path[0]];
        for k in &path[1..] {
            let Value::Record(r) = here else { panic!("{path:?}") };
            here = &r[*k];
        }
        here
    }

    fn text(v: &BTreeMap<String, Value>, path: &[&str]) -> String {
        match at(v, path) {
            Value::Text(t) => t.clone(),
            other => panic!("{path:?}: {other:?}"),
        }
    }

    /// A vector GnuPG wrote as bytes (base64 in the JSON), or as armor.
    fn written(v: &BTreeMap<String, Value>, path: &[&str]) -> Value {
        let t = text(v, path);
        if t.starts_with("-----BEGIN") {
            Value::Text(t)
        } else {
            Value::Bytes(base64_decode(&t, crate::crypto::alphabet("standard").unwrap()).unwrap())
        }
    }

    fn fpr(v: &BTreeMap<String, Value>, key: &str, i: usize) -> Value {
        let Value::List(l) = at(v, &["fingerprints", key]) else { panic!() };
        l[i].clone()
    }

    /// 2026-11-08: after the signatures (2026-06-01), before the one dated 2027.
    const NOW: f64 = 1_794_150_000.0;
    const JAN_2026: f64 = 1_767_225_600.0;
    const JUNE_2026: f64 = 1_780_272_000.0;

    fn signed(signer: &Value, key: &Value, created: f64) -> Option<Value> {
        Some(Value::Record(BTreeMap::from([
            ("signer".to_string(), signer.clone()),
            ("key".to_string(), key.clone()),
            ("created".to_string(), Value::Number(created)),
        ])))
    }

    #[test]
    fn a_key_says_what_gnupg_lists() {
        let v = vectors();
        let info = |k: &str| key_info(&Value::Text(text(&v, &["keys", k]))).unwrap();
        let field = |i: &Value, f: &str| match i {
            Value::Record(r) => r.get(f).cloned(),
            _ => None,
        };
        let rsa = info("rsa");
        assert_eq!(field(&rsa, "fingerprint"), Some(fpr(&v, "rsa", 0)));
        assert_eq!(field(&rsa, "user_ids"), Some(Value::List(vec![Value::Text("Bank RSA <rsa@bank.example>".into())])));
        assert_eq!((field(&rsa, "created"), field(&rsa, "expires"), field(&rsa, "revoked")), (Some(Value::Number(JAN_2026)), None, Some(Value::Boolean(false))));
        // Made 2025-01-01, for 30 days.
        assert_eq!(field(&info("old"), "expires"), Some(Value::Number(1_735_689_600.0 + 30.0 * 86_400.0)));
        assert_eq!(field(&info("gone"), "revoked"), Some(Value::Boolean(true)));
        // Two keys at once, a secret key, and what isn't a key give nothing.
        let both = Value::Text(format!("{}{}", text(&v, &["keys", "rsa"]), text(&v, &["keys", "c25519"])));
        assert_eq!(key_info(&both), None);
        assert_eq!(key_info(&Value::Text(text(&v, &["secret", "rsa"]))), None);
        assert_eq!(key_info(&Value::Text("not a key".into())), None);
    }

    #[test]
    fn gnupgs_signatures_verify_with_the_key_that_made_them() {
        let v = vectors();
        let data = Value::Text(text(&v, &["data"]));
        let keys = Value::List(["rsa", "c25519", "old"].iter().map(|k| Value::Text(text(&v, &["keys", k]))).collect());
        let check = |sig: &str| verify(&data, &written(&v, &["signatures", sig]), &keys, NOW);
        let (rsa, ed) = (fpr(&v, "rsa", 0), fpr(&v, "c25519", 0));
        assert_eq!(check("rsa_sha256"), signed(&rsa, &rsa, JUNE_2026));
        assert_eq!(check("rsa_sha512_armored"), signed(&rsa, &rsa, JUNE_2026));
        assert_eq!(check("ed_primary"), signed(&ed, &ed, JUNE_2026));
        // By the signing subkey, which its back-signature lets sign.
        assert_eq!(check("ed_subkey_armored"), signed(&ed, &fpr(&v, "c25519", 2), JUNE_2026));
        // A text signature: over the text with CR LF line endings, so over it either way.
        assert_eq!(check("ed_text"), signed(&ed, &ed, JUNE_2026));
        let crlf_data = Value::Text(text(&v, &["data"]).replace('\n', "\r\n"));
        assert!(verify(&crlf_data, &written(&v, &["signatures", "ed_text"]), &keys, NOW).is_some());
        assert_eq!(verify(&crlf_data, &written(&v, &["signatures", "ed_primary"]), &keys, NOW), None, "a binary signature is over the bytes");
        // Made while the old key was valid, which it is no longer: the signature still holds.
        let old = fpr(&v, "old", 0);
        assert_eq!(check("old_while_valid"), signed(&old, &old, 1_736_467_200.0));
        // Of two signatures, the one a given key made.
        assert_eq!(check("other_then_rsa"), signed(&rsa, &rsa, JUNE_2026));
        // Data as bytes is the same data.
        let bytes = Value::Bytes(text(&v, &["data"]).into_bytes());
        assert!(verify(&bytes, &written(&v, &["signatures", "rsa_sha256"]), &keys, NOW).is_some());
    }

    #[test]
    fn what_doesnt_bear_out_its_signature_is_nothing() {
        let v = vectors();
        let data = Value::Text(text(&v, &["data"]));
        let all = Value::List(["rsa", "c25519", "old", "gone"].iter().map(|k| Value::Text(text(&v, &["keys", k]))).collect());
        let check = |sig: &str, d: &Value, now: f64| verify(d, &written(&v, &["signatures", sig]), &all, now);
        assert_eq!(check("rsa_sha1", &data, NOW), None, "SHA-1 over a document");
        assert_eq!(check("gone_before_revoked", &data, NOW), None, "its key is revoked");
        assert_eq!(check("other", &data, NOW), None, "a key not given");
        assert_eq!(check("future", &data, NOW), None, "made after now");
        assert!(check("future", &data, 1_798_761_600.0).is_some(), "but not after 2027-01-01");
        assert_eq!(check("rsa_sha256", &Value::Text(text(&v, &["data"]).replace("ACCP", "RJCT")), NOW), None, "another document");
        assert_eq!(check("rsa_sha256", &data, JAN_2026), None, "before it was made");
        // Without its back-signature, the subkey can't sign: one key's owner can bind another's
        // subkey, never take its signatures.
        let stripped = Value::Bytes(without_back_signature(&unarmored(&Value::Text(text(&v, &["keys", "c25519"])), "PUBLIC KEY BLOCK").unwrap()));
        assert!(verify(&data, &written(&v, &["signatures", "ed_primary"]), &stripped, NOW).is_some());
        assert_eq!(verify(&data, &written(&v, &["signatures", "ed_subkey_armored"]), &stripped, NOW), None);
        // A changed byte anywhere in the signature, and a broken armor checksum.
        let Value::Bytes(mut sig) = written(&v, &["signatures", "rsa_sha256"]) else { panic!() };
        let last = sig.len() - 1;
        sig[last] ^= 1;
        assert_eq!(verify(&data, &Value::Bytes(sig), &all, NOW), None);
        let armored = text(&v, &["signatures", "rsa_sha512_armored"]);
        let sum = armored.lines().find(|l| l.starts_with('=') && l.len() == 5).unwrap();
        let broken = armored.replace(sum, if sum == "=AAAA" { "=AAAB" } else { "=AAAA" });
        assert_eq!(verify(&data, &Value::Text(broken), &all, NOW), None);
    }

    /// The c25519 key with its signing subkey's back-signature (in the binding's unhashed area)
    /// taken out: the binding still verifies, the subkey no longer signs.
    fn without_back_signature(key: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        for p in packets(key).unwrap() {
            let mut body = p.body.clone();
            if p.tag == 2 && signature(&body).is_some_and(|s| s.typ == 0x18 && s.embedded.is_some()) {
                let n = 4 + 2 + u16::from_be_bytes([body[4], body[5]]) as usize;
                let unhashed = u16::from_be_bytes([body[n], body[n + 1]]) as usize;
                body.splice(n..n + 2 + unhashed, [0, 0]);
            }
            out.extend(packet(p.tag, &body));
        }
        out
    }

    // ── a decryptor, for these tests: GnuPG's messages prove it, then it reads encrypt's ──

    /// The secret parts of an unprotected secret key's packets, by key ID.
    fn secrets(armored: &str) -> Vec<(Key, Vec<Vec<u8>>)> {
        packets(&dearmor(armored, "PRIVATE KEY BLOCK").unwrap())
            .unwrap()
            .into_iter()
            .filter(|p| p.tag == 5 || p.tag == 7)
            .map(|p| {
                let (k, rest) = key(&p.body).unwrap();
                let mut r = Reader(rest);
                assert_eq!(r.u8(), Some(0), "unprotected");
                let n = if matches!(k.material, Material::Rsa(_)) { 4 } else { 1 };
                let mpis = (0..n).map(|_| r.mpi().unwrap().to_vec()).collect();
                (k, mpis)
            })
            .collect()
    }

    fn aes_unblock(key: &[u8], block: &mut [u8; 16]) {
        let b = aes::Block::from_mut_slice(block);
        match key.len() {
            16 => aes::Aes128::new_from_slice(key).unwrap().decrypt_block(b),
            24 => aes::Aes192::new_from_slice(key).unwrap().decrypt_block(b),
            _ => aes::Aes256::new_from_slice(key).unwrap().decrypt_block(b),
        }
    }

    fn key_unwrap(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
        let n = wrapped.len() / 8 - 1;
        let mut a: [u8; 8] = wrapped[..8].try_into().unwrap();
        let mut r: Vec<[u8; 8]> = wrapped[8..].chunks_exact(8).map(|c| c.try_into().unwrap()).collect();
        for j in (0..6).rev() {
            for i in (0..n).rev() {
                let t = (n * j + i + 1) as u64;
                let mut block = [0u8; 16];
                block[..8].copy_from_slice(&(u64::from_be_bytes(a) ^ t).to_be_bytes());
                block[8..].copy_from_slice(&r[i]);
                aes_unblock(kek, &mut block);
                a.copy_from_slice(&block[..8]);
                r[i].copy_from_slice(&block[8..]);
            }
        }
        (a == [0xA6; 8]).then(|| r.concat())
    }

    /// The literal data of `message`, its date with it, decrypted with one of `secrets`.
    fn decrypt(message: &Value, secrets: &[(Key, Vec<Vec<u8>>)]) -> Option<(Vec<u8>, u32)> {
        let mut session = None;
        let mut sealed = None;
        for p in packets(&unarmored(message, "MESSAGE")?)? {
            match p.tag {
                1 => {
                    let mut r = Reader(&p.body);
                    assert_eq!(r.u8(), Some(3));
                    let id = r.take(8)?;
                    let Some((k, s)) = secrets.iter().find(|(k, _)| k.key_id() == id) else { continue };
                    r.u8()?;
                    let m = match &k.material {
                        Material::Rsa(pk) => {
                            let big = |b: &[u8]| BigUint::from_bytes_be(b);
                            let sk = rsa::RsaPrivateKey::from_components(pk.n().clone(), pk.e().clone(), big(&s[0]), vec![big(&s[1]), big(&s[2])]).ok()?;
                            sk.decrypt(rsa::Pkcs1v15Encrypt, r.mpi()?).ok()?
                        }
                        Material::Cv25519 { hash, wrap, .. } => {
                            let ephemeral: [u8; 32] = r.mpi()?.strip_prefix(&[0x40])?.try_into().ok()?;
                            let wrapped = r.short()?;
                            // A legacy Curve25519 secret is written big-endian (RFC 6637).
                            let mut secret = [0u8; 32];
                            secret[32 - s[0].len()..].copy_from_slice(&s[0]);
                            secret.reverse();
                            let shared = MontgomeryPoint(ephemeral).mul_clamped(secret);
                            let m = key_unwrap(&kdf(&shared.0, *hash, *wrap, &k.fingerprint)?, wrapped)?;
                            m[..m.len() - *m.last()? as usize].to_vec()
                        }
                        _ => return None,
                    };
                    let (key, sum) = (&m[1..m.len() - 2], u16::from_be_bytes(m[m.len() - 2..].try_into().ok()?));
                    assert_eq!((m[0], key.iter().fold(0u16, |s, &b| s.wrapping_add(b as u16))), (AES256, sum));
                    session = Some(<[u8; 32]>::try_from(key).ok()?);
                }
                18 => sealed = Some(p.body),
                _ => return None,
            }
        }
        let (session, sealed) = (session?, sealed?);
        assert_eq!(sealed[0], 1);
        let mut plain = sealed[1..].to_vec();
        let cipher = aes::Aes256::new((&session).into());
        let mut register = [0u8; 16];
        for chunk in plain.chunks_mut(16) {
            let mut stream = aes::Block::from(register);
            cipher.encrypt_block(&mut stream);
            register[..chunk.len()].copy_from_slice(chunk);
            for (b, s) in chunk.iter_mut().zip(stream) {
                *b ^= s;
            }
        }
        assert_eq!(plain[14..16], plain[16..18], "the prefix's check");
        let (body, mdc) = plain.split_at(plain.len() - 20);
        assert_eq!(&body[body.len() - 2..], &[0xD3, 0x14]);
        assert_eq!(Sha1::digest(body).as_slice(), mdc, "the modification detection code");
        let [literal] = packets(&body[18..body.len() - 2])?.try_into().ok()?;
        assert_eq!(literal.tag, 11);
        let name = literal.body[1] as usize;
        let date = u32::from_be_bytes(literal.body[2 + name..6 + name].try_into().ok()?);
        Some((literal.body[6 + name..].to_vec(), date))
    }

    #[test]
    fn the_decryptor_reads_what_gnupg_encrypted() {
        let v = vectors();
        let data = text(&v, &["data"]).into_bytes();
        let (rsa, c25519) = (secrets(&text(&v, &["secret", "rsa"])), secrets(&text(&v, &["secret", "c25519"])));
        assert_eq!(decrypt(&written(&v, &["messages", "to_rsa"]), &rsa).unwrap().0, data);
        assert_eq!(decrypt(&written(&v, &["messages", "to_c25519_armored"]), &c25519).unwrap().0, data);
        for secret in [&rsa, &c25519] {
            assert_eq!(decrypt(&written(&v, &["messages", "to_both"]), secret).unwrap().0, data);
        }
        assert_eq!(decrypt(&written(&v, &["messages", "to_rsa"]), &c25519), None, "not to its key");
    }

    #[test]
    fn encrypt_writes_what_each_key_decrypts_and_the_same_again_from_the_same_inputs() {
        let v = vectors();
        let key = |k: &str| Value::Text(text(&v, &["keys", k]));
        let (rsa, c25519) = (secrets(&text(&v, &["secret", "rsa"])), secrets(&text(&v, &["secret", "c25519"])));
        let data = Value::Text("Ntry 1: 120.00 EUR to FR14 2004 1010 0505 0001 3M02 606\n".into());
        let random = [7u8; 32];
        let both = Value::List(vec![key("rsa"), key("c25519")]);
        let message = encrypt(&data, &both, &random, NOW, false).unwrap();
        for secret in [&rsa, &c25519] {
            assert_eq!(decrypt(&Value::Bytes(message.clone()), secret), Some((data_bytes(&data).unwrap(), NOW as u32)));
        }
        // Armored, it reads the same; GnuPG's armor, with its checksum.
        let armored = String::from_utf8(encrypt(&data, &key("c25519"), &random, NOW, true).unwrap()).unwrap();
        assert!(armored.starts_with("-----BEGIN PGP MESSAGE-----\n\n") && armored.ends_with("-----END PGP MESSAGE-----\n"), "{armored}");
        assert_eq!(decrypt(&Value::Text(armored), &c25519).map(|d| d.0), data_bytes(&data));
        // The same inputs give the same bytes, pinned: what decides them is all here, so a change
        // to how they are drawn fails this before it changes what a logged run gives. (New
        // vectors, new keys: the pin moves with them.)
        assert_eq!(encrypt(&data, &both, &random, NOW, false).unwrap(), message);
        assert_eq!(crate::crypto::sha256(&message, "hex").unwrap(), "32f79de4ef97a21e75deaeab6f18f5467f4c2bd32822d0beb21cbb6b10752cd0");
        // Another run's bytes, or other data, give another session key.
        let other = encrypt(&data, &both, &[8u8; 32], NOW, false).unwrap();
        assert_ne!(other[..40], message[..40]);
        assert_eq!(decrypt(&Value::Bytes(other), &rsa).map(|d| d.0), data_bytes(&data));
    }

    #[test]
    fn encrypt_gives_nothing_for_a_key_it_cant_use() {
        let v = vectors();
        let key = |k: &str| Value::Text(text(&v, &["keys", k]));
        let data = Value::Text("x".into());
        let random = [1u8; 32];
        assert_eq!(encrypt(&data, &key("old"), &random, NOW, false), None, "expired");
        assert!(encrypt(&data, &key("old"), &random, 1_735_700_000.0, false).is_some(), "but not before");
        assert_eq!(encrypt(&data, &key("gone"), &random, NOW, false), None, "revoked");
        assert_eq!(encrypt(&data, &key("other"), &random, NOW, false), None, "no encryption key");
        assert_eq!(encrypt(&data, &Value::List(vec![key("rsa"), key("old")]), &random, NOW, false), None, "one of them can't be used");
        assert_eq!(encrypt(&data, &key("rsa"), &[1u8; 31], NOW, false), None, "too little randomness");
        assert_eq!(encrypt(&data, &key("rsa"), &random, -1.0, false), None);
        assert_eq!(encrypt(&data, &Value::List(vec![]), &random, NOW, false), None);
    }

    #[test]
    fn crc24_is_rfc_9580s() {
        // RFC 9580 §6.1's initial value over nothing, and armor that round-trips through dearmor.
        assert_eq!(crc24(b""), 0xB704CE);
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(dearmor(&armor("MESSAGE", &bytes), "MESSAGE"), Some(bytes));
        assert_eq!(dearmor(&armor("MESSAGE", b"x"), "SIGNATURE"), None);
    }
}
