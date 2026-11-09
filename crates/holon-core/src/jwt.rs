//! JWTs (RFC 7519), verified as jose verifies them: a compact JWS (RFC 7515) whose signature its key
//! checks, and whose claims hold at the time given.
//!
//! The key decides the algorithm, never the token: a shared secret (a text that isn't PEM, or bytes)
//! allows HS256, HS384 and HS512; an RSA key RS256 to PS512; a P-256 key ES256; an Ed25519 key EdDSA.
//! So a token naming `none`, or HS256 over an RSA public key's text, is refused by construction. A
//! public key is a PEM text or a JWK record, and a JWKS gives the key the token's `kid` names.
//! Anything that isn't one of these gives nothing, as the other leaves do.

use std::collections::BTreeMap;

use ed25519_dalek::pkcs8::DecodePublicKey as _;
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::Verifier as _;
use rsa::pkcs1::DecodeRsaPublicKey as _;
use rsa::traits::PublicKeyParts as _;
use sha2::{Sha256, Sha384, Sha512};

use crate::Value;

/// The seconds `leeway` may widen `exp` and `nbf` by: none (the default, as jose's), or a clock's
/// usual skew.
pub(crate) const LEEWAYS: &[&str] = &["0", "30", "60", "120", "300"];

/// An RSA key's least size, as jose and RFC 7518 §3.3 ask; `rsa` refuses one past 4,096 bits.
const RSA_MIN_BITS: usize = 2048;

/// A key a token's signature is checked with.
enum Key {
    /// A shared secret.
    Hmac(Vec<u8>),
    Rsa(rsa::RsaPublicKey),
    P256(p256::ecdsa::VerifyingKey),
    Ed25519(ed25519_dalek::VerifyingKey),
}

/// A key, and the one algorithm a JWK's `alg` pins it to, if it has one.
struct Usable {
    key: Key,
    alg: Option<String>,
}

/// A token taken apart: what its signature covers, its header and payload, and the signature.
struct Parts<'a> {
    signed: &'a str,
    header: BTreeMap<String, Value>,
    payload: BTreeMap<String, Value>,
    signature: Vec<u8>,
}

/// `{header, payload}`, verifying nothing: what a token says before it is trusted.
pub(crate) fn decode(token: &str) -> Option<Value> {
    let parts = parts(token)?;
    Some(Value::Record(BTreeMap::from([("header".to_string(), Value::Record(parts.header)), ("payload".to_string(), Value::Record(parts.payload))])))
}

/// The token's claims, when its signature verifies with `key` and they hold at `now` (seconds since
/// the epoch, as `exp` is): `exp` after `now`, `nbf` not after it, each widened by `leeway`; `iss`
/// equal to `issuer` and `aud` holding `audience`, each only when given (not empty).
pub(crate) fn verify(token: &str, key: &Value, now: f64, issuer: &str, audience: &str, leeway: f64) -> Option<Value> {
    let parts = parts(token)?;
    let Some(Value::Text(alg)) = parts.header.get("alg") else { return None };
    let usable = chosen(key, &parts.header, alg)?;
    (signature_holds(&usable.key, alg, parts.signed.as_bytes(), &parts.signature) && claims_hold(&parts.payload, now, issuer, audience, leeway)).then_some(Value::Record(parts.payload))
}

/// Three base64url parts without padding, the header a JSON object naming its `alg` and requiring
/// no extension (`crit`), the payload a JSON object.
fn parts(token: &str) -> Option<Parts<'_>> {
    let mut pieces = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (pieces.next(), pieces.next(), pieces.next(), pieces.next()) else { return None };
    let object = |piece: &str| match crate::json::parse(std::str::from_utf8(&unpadded(piece)?).ok()?)? {
        Value::Record(r) => Some(r),
        _ => None,
    };
    let header = object(h)?;
    if !matches!(header.get("alg"), Some(Value::Text(_))) || header.contains_key("crit") {
        return None;
    }
    Some(Parts { signed: &token[..h.len() + 1 + p.len()], header, payload: object(p)?, signature: unpadded(s)? })
}

/// Base64url as a JWS writes it: no padding, one text for one decoding.
fn unpadded(s: &str) -> Option<Vec<u8>> {
    if s.contains('=') {
        return None;
    }
    crate::crypto::base64_decode(s, crate::crypto::alphabet("url")?)
}

/// The key that checks a token with this header: `key` itself, or a JWKS's key by `kid`, which
/// must allow the token's algorithm.
fn chosen(key: &Value, header: &BTreeMap<String, Value>, alg: &str) -> Option<Usable> {
    let usable = match key {
        Value::Record(r) if r.contains_key("keys") => {
            let Some(Value::List(keys)) = r.get("keys") else { return None };
            let kid = match header.get("kid") {
                None => None,
                Some(Value::Text(k)) => Some(k),
                Some(_) => return None,
            };
            // A key the set holds but this host can't use (another curve, another kind) is passed
            // over, as an issuer may publish several kinds.
            let mut fit = keys
                .iter()
                .filter_map(|k| match k {
                    Value::Record(jwk) if kid.is_none_or(|kid| jwk.get("kid") == Some(&Value::Text(kid.clone()))) => from_jwk(jwk),
                    _ => None,
                })
                .filter(|u| allows(u, alg));
            let one = fit.next()?;
            if fit.next().is_some() {
                return None;
            }
            one
        }
        Value::Record(jwk) => from_jwk(jwk)?,
        Value::Text(t) if t.trim_start().starts_with("-----BEGIN") => from_pem(t)?,
        Value::Text(t) => Usable { key: Key::Hmac(t.as_bytes().to_vec()), alg: None },
        Value::Bytes(b) => Usable { key: Key::Hmac(b.clone()), alg: None },
        _ => return None,
    };
    allows(&usable, alg).then_some(usable)
}

/// Whether `alg` is one the key allows: by its kind, and by a JWK's own `alg` when it names one.
fn allows(usable: &Usable, alg: &str) -> bool {
    let kind = match &usable.key {
        // RFC 7518 §3.2: a key at least as long as the hash.
        Key::Hmac(k) => matches!((alg, k.len()), ("HS256", 32..) | ("HS384", 48..) | ("HS512", 64..)),
        Key::Rsa(_) => matches!(alg, "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512"),
        Key::P256(_) => alg == "ES256",
        Key::Ed25519(_) => matches!(alg, "EdDSA" | "Ed25519"),
    };
    kind && usable.alg.as_deref().is_none_or(|pinned| pinned == alg)
}

/// A PEM public key: SubjectPublicKeyInfo (`PUBLIC KEY`) of RSA, P-256 or Ed25519, or PKCS #1
/// (`RSA PUBLIC KEY`).
fn from_pem(text: &str) -> Option<Usable> {
    let key = if let Ok(k) = <rsa::RsaPublicKey as rsa::pkcs8::DecodePublicKey>::from_public_key_pem(text) {
        rsa_key(k)?
    } else if let Ok(k) = rsa::RsaPublicKey::from_pkcs1_pem(text) {
        rsa_key(k)?
    } else if let Ok(k) = <p256::ecdsa::VerifyingKey as p256::pkcs8::DecodePublicKey>::from_public_key_pem(text) {
        Key::P256(k)
    } else {
        Key::Ed25519(ed25519_dalek::VerifyingKey::from_public_key_pem(text).ok()?)
    };
    Some(Usable { key, alg: None })
}

fn rsa_key(k: rsa::RsaPublicKey) -> Option<Key> {
    (k.n().bits() >= RSA_MIN_BITS).then_some(Key::Rsa(k))
}

/// A JWK (RFC 7517) of kind `RSA`, `EC` on P-256, `OKP` on Ed25519 or `oct`: none for another, or
/// one whose `use` isn't `sig` or whose `key_ops` lack `verify`.
fn from_jwk(jwk: &BTreeMap<String, Value>) -> Option<Usable> {
    let text = |k: &str| match jwk.get(k) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    let bytes = |k: &str| unpadded(text(k)?);
    if jwk.get("use").is_some_and(|u| *u != Value::Text("sig".into())) {
        return None;
    }
    if let Some(ops) = jwk.get("key_ops") {
        let Value::List(ops) = ops else { return None };
        if !ops.contains(&Value::Text("verify".into())) {
            return None;
        }
    }
    let alg = match jwk.get("alg") {
        None => None,
        Some(Value::Text(a)) => Some(a.clone()),
        Some(_) => return None,
    };
    let key = match (text("kty")?, text("crv")) {
        ("RSA", _) => {
            let (n, e) = (bytes("n")?, bytes("e")?);
            rsa_key(rsa::RsaPublicKey::new(rsa::BigUint::from_bytes_be(&n), rsa::BigUint::from_bytes_be(&e)).ok()?)?
        }
        ("EC", Some("P-256")) => {
            let (x, y) = (bytes("x")?, bytes("y")?);
            if x.len() != 32 || y.len() != 32 {
                return None;
            }
            let point = p256::EncodedPoint::from_affine_coordinates(x.as_slice().into(), y.as_slice().into(), false);
            Key::P256(p256::ecdsa::VerifyingKey::from_encoded_point(&point).ok()?)
        }
        ("OKP", Some("Ed25519")) => Key::Ed25519(ed25519_dalek::VerifyingKey::from_bytes(&bytes("x")?.try_into().ok()?).ok()?),
        ("oct", _) => Key::Hmac(bytes("k")?),
        _ => return None,
    };
    Some(Usable { key, alg })
}

/// Whether `signature` is `key`'s, by `alg`, over `signed` (the header and payload as sent). An
/// HMAC is compared in constant time.
fn signature_holds(key: &Key, alg: &str, signed: &[u8], signature: &[u8]) -> bool {
    use rsa::{pkcs1v15, pss};
    fn mac<M: Mac + hmac::digest::KeyInit>(key: &[u8], signed: &[u8], signature: &[u8]) -> bool {
        let mut m = <M as Mac>::new_from_slice(key).expect("HMAC takes a key of any length");
        m.update(signed);
        m.verify_slice(signature).is_ok()
    }
    match (key, alg) {
        (Key::Hmac(k), "HS256") => mac::<Hmac<Sha256>>(k, signed, signature),
        (Key::Hmac(k), "HS384") => mac::<Hmac<Sha384>>(k, signed, signature),
        (Key::Hmac(k), "HS512") => mac::<Hmac<Sha512>>(k, signed, signature),
        (Key::Rsa(k), _) => {
            let Ok(s) = pkcs1v15::Signature::try_from(signature) else { return false };
            let Ok(p) = pss::Signature::try_from(signature) else { return false };
            let k = k.clone();
            match alg {
                "RS256" => pkcs1v15::VerifyingKey::<Sha256>::new(k).verify(signed, &s).is_ok(),
                "RS384" => pkcs1v15::VerifyingKey::<Sha384>::new(k).verify(signed, &s).is_ok(),
                "RS512" => pkcs1v15::VerifyingKey::<Sha512>::new(k).verify(signed, &s).is_ok(),
                "PS256" => pss::VerifyingKey::<Sha256>::new(k).verify(signed, &p).is_ok(),
                "PS384" => pss::VerifyingKey::<Sha384>::new(k).verify(signed, &p).is_ok(),
                "PS512" => pss::VerifyingKey::<Sha512>::new(k).verify(signed, &p).is_ok(),
                _ => false,
            }
        }
        // JWS writes an ECDSA signature as r and s, 32 bytes each (RFC 7518 §3.4), not as DER.
        (Key::P256(k), "ES256") => p256::ecdsa::Signature::from_slice(signature).is_ok_and(|s| k.verify(signed, &s).is_ok()),
        (Key::Ed25519(k), "EdDSA" | "Ed25519") => ed25519_dalek::Signature::from_slice(signature).is_ok_and(|s| k.verify_strict(signed, &s).is_ok()),
        _ => false,
    }
}

/// Whether the claims hold at `now`: `exp` required and after it, `nbf` not after it, each within
/// `leeway`; `exp`, `nbf` and `iat` numbers; `iss` and `aud` as given.
fn claims_hold(payload: &BTreeMap<String, Value>, now: f64, issuer: &str, audience: &str, leeway: f64) -> bool {
    let number = |k: &str| match payload.get(k) {
        None => Some(None),
        Some(Value::Number(n)) if n.is_finite() => Some(Some(*n)),
        Some(_) => None,
    };
    let (Some(Some(exp)), Some(nbf), Some(_)) = (number("exp"), number("nbf"), number("iat")) else { return false };
    let on_time = now.is_finite() && now < exp + leeway && nbf.is_none_or(|nbf| nbf <= now + leeway);
    let from = issuer.is_empty() || payload.get("iss") == Some(&Value::Text(issuer.into()));
    let to = audience.is_empty()
        || match payload.get("aud") {
            Some(Value::Text(a)) => a == audience,
            Some(Value::List(auds)) => auds.contains(&Value::Text(audience.into())),
            _ => false,
        };
    on_time && from && to
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys and tokens Node's crypto (OpenSSL) made: `node testdata/jwt-vectors.mjs` writes them again.
    fn vectors() -> BTreeMap<String, Value> {
        match crate::json::parse(include_str!("testdata/jwt-vectors.json")).unwrap() {
            Value::Record(r) => r,
            _ => unreachable!(),
        }
    }

    fn text<'a>(v: &'a BTreeMap<String, Value>, k: &str) -> &'a str {
        match &v[k] {
            Value::Text(t) => t,
            other => panic!("{k}: {other:?}"),
        }
    }

    fn token<'a>(v: &'a BTreeMap<String, Value>, name: &str) -> &'a str {
        let Value::Record(tokens) = &v["tokens"] else { unreachable!() };
        let Value::Text(t) = &tokens[name] else { unreachable!() };
        t
    }

    /// In the claims' hour: after `nbf` (1794150000), before `exp` (1794153600).
    const NOW: f64 = 1_794_151_000.0;

    #[test]
    fn each_algorithm_verifies_with_its_key_as_pem_and_as_jwk() {
        let v = vectors();
        let claims = v["claims"].clone();
        let t = |s: &str| Value::Text(text(&v, s).to_string());
        let rsa = [t("rsa_pem"), t("rsa_pkcs1_pem"), v["rsa_jwk"].clone()];
        for alg in ["RS256", "RS384", "RS512", "PS256", "PS384", "PS512"] {
            for key in &rsa {
                assert_eq!(verify(token(&v, alg), key, NOW, "", "", 0.0), Some(claims.clone()), "{alg} with {key:?}");
            }
        }
        for key in [t("ec_pem"), v["ec_jwk"].clone()] {
            assert_eq!(verify(token(&v, "ES256"), &key, NOW, "", "", 0.0), Some(claims.clone()), "ES256");
        }
        for key in [t("ed_pem"), v["ed_jwk"].clone()] {
            for alg in ["EdDSA", "Ed25519"] {
                assert_eq!(verify(token(&v, alg), &key, NOW, "", "", 0.0), Some(claims.clone()), "{alg}");
            }
        }
        assert_eq!(verify(token(&v, "HS256"), &t("secret"), NOW, "", "", 0.0), Some(claims.clone()));
        assert_eq!(verify(token(&v, "HS256"), &Value::Bytes(text(&v, "secret").as_bytes().to_vec()), NOW, "", "", 0.0), Some(claims.clone()), "the secret as bytes");
        for alg in ["HS384", "HS512"] {
            assert_eq!(verify(token(&v, alg), &t("long"), NOW, "", "", 0.0), Some(claims.clone()), "{alg}");
        }
        // A key of the wrong kind, or another key of the right one, gives nothing.
        assert_eq!(verify(token(&v, "RS256"), &t("ec_pem"), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "ES256"), &t("rsa_pem"), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "HS256"), &t("long"), NOW, "", "", 0.0), None);
    }

    #[test]
    fn a_jwks_gives_the_key_its_kid_names() {
        let v = vectors();
        let keys = Value::Record(BTreeMap::from([("keys".to_string(), Value::List(vec![v["ec_jwk"].clone(), v["rsa_jwk"].clone(), v["ed_jwk"].clone()]))]));
        for alg in ["RS256", "PS512", "ES256", "EdDSA"] {
            assert!(verify(token(&v, alg), &keys, NOW, "", "", 0.0).is_some(), "{alg}");
        }
        // A kid the set doesn't hold; no kid, and one key for RS256 in the set; no kid, and two.
        assert_eq!(verify(token(&v, "other_kid"), &keys, NOW, "", "", 0.0), None);
        assert!(verify(token(&v, "no_kid"), &keys, NOW, "", "", 0.0).is_some());
        let mut other = match v["rsa_jwk"].clone() {
            Value::Record(r) => r,
            _ => unreachable!(),
        };
        other.insert("kid".into(), Value::Text("rsa-2".into()));
        let two = Value::Record(BTreeMap::from([("keys".to_string(), Value::List(vec![v["rsa_jwk"].clone(), Value::Record(other)]))]));
        assert_eq!(verify(token(&v, "no_kid"), &two, NOW, "", "", 0.0), None, "two keys could be meant");
        assert!(verify(token(&v, "other_kid"), &two, NOW, "", "", 0.0).is_some(), "rsa-2 is the same key");
    }

    #[test]
    fn a_jwks_alg_use_and_key_ops_narrow_it() {
        let v = vectors();
        let with = |k: &str, value: Value| {
            let Value::Record(mut r) = v["rsa_jwk"].clone() else { unreachable!() };
            r.insert(k.into(), value);
            Value::Record(r)
        };
        assert!(verify(token(&v, "RS256"), &with("alg", Value::Text("RS256".into())), NOW, "", "", 0.0).is_some());
        assert_eq!(verify(token(&v, "PS256"), &with("alg", Value::Text("RS256".into())), NOW, "", "", 0.0), None, "pinned to RS256");
        assert_eq!(verify(token(&v, "RS256"), &with("use", Value::Text("enc".into())), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "RS256"), &with("key_ops", Value::List(vec![Value::Text("encrypt".into())])), NOW, "", "", 0.0), None);
        assert!(verify(token(&v, "RS256"), &with("key_ops", Value::List(vec![Value::Text("verify".into())])), NOW, "", "", 0.0).is_some());
    }

    #[test]
    fn what_a_token_names_never_chooses_the_algorithm() {
        let v = vectors();
        let t = |s: &str| Value::Text(text(&v, s).to_string());
        // alg none, signed by nothing; HS256 over the RSA key's own PEM text, the classic confusion.
        assert_eq!(verify(token(&v, "none"), &t("rsa_pem"), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "none"), &t("secret"), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "confused"), &t("rsa_pem"), NOW, "", "", 0.0), None);
        // A 1,024-bit RSA key, and an HS256 secret shorter than its hash.
        assert_eq!(verify(token(&v, "weak"), &t("weak_pem"), NOW, "", "", 0.0), None);
        assert_eq!(verify(token(&v, "short_secret"), &Value::Text("too short".into()), NOW, "", "", 0.0), None);
    }

    #[test]
    fn a_token_that_isnt_one_as_written_is_nothing() {
        let v = vectors();
        let key = Value::Text(text(&v, "rsa_pem").to_string());
        for bad in ["crit", "no_exp", "exp_text", "padded", "array_payload"] {
            assert_eq!(verify(token(&v, bad), &key, NOW, "", "", 0.0), None, "{bad}");
        }
        let rs = token(&v, "RS256");
        for bad in [&format!("{rs}.x"), &rs.replacen('.', "..", 1), &format!(" {rs}"), "", "a.b", "a.b.c"] {
            assert_eq!(verify(bad, &key, NOW, "", "", 0.0), None, "{bad:?}");
            assert_eq!(decode(bad), None, "{bad:?}");
        }
        // One flipped bit of the signature.
        let (head, sig) = rs.rsplit_once('.').unwrap();
        let mut bytes = unpadded(sig).unwrap();
        bytes[10] ^= 1;
        let flipped = format!("{head}.{}", crate::crypto::base64_encode(&bytes, crate::crypto::alphabet("url").unwrap(), false));
        assert_eq!(verify(&flipped, &key, NOW, "", "", 0.0), None);
    }

    #[test]
    fn the_claims_hold_at_now_for_the_issuer_and_audience_given() {
        let v = vectors();
        let key = Value::Text(text(&v, "rsa_pem").to_string());
        let rs = token(&v, "RS256");
        let at = |now: f64, leeway: f64| verify(rs, &key, now, "", "", leeway).is_some();
        // exp 1794153600: verified just before it, not at it; nbf 1794150000: at it, not before.
        assert!(at(1_794_153_599.5, 0.0) && !at(1_794_153_600.0, 0.0));
        assert!(at(1_794_150_000.0, 0.0) && !at(1_794_149_999.0, 0.0));
        assert!(at(1_794_153_629.0, 30.0) && !at(1_794_153_630.0, 30.0), "leeway past exp");
        assert!(at(1_794_149_970.0, 30.0) && !at(1_794_149_969.0, 30.0), "leeway before nbf");
        assert!(!at(f64::NAN, 300.0));
        assert!(verify(rs, &key, NOW, "partner-a", "payouts-api", 0.0).is_some());
        assert_eq!(verify(rs, &key, NOW, "partner-b", "", 0.0), None);
        assert_eq!(verify(rs, &key, NOW, "", "other-api", 0.0), None);
        let listed = token(&v, "aud_list");
        assert!(verify(listed, &key, NOW, "", "payouts-api", 0.0).is_some() && verify(listed, &key, NOW, "", "other", 0.0).is_some());
        assert_eq!(verify(listed, &key, NOW, "", "third", 0.0), None);
    }

    #[test]
    fn decode_reads_a_token_without_trusting_it() {
        let v = vectors();
        let Some(Value::Record(d)) = decode(token(&v, "confused")) else { panic!() };
        assert_eq!(d["payload"], v["claims"]);
        assert_eq!(d["header"], Value::Record(BTreeMap::from([("alg".to_string(), Value::Text("HS256".into()))])));
        assert!(decode(token(&v, "none")).is_some(), "an unsigned one reads too");
    }

    /// RFC 7515 Appendix A.1's HS256 example, its key a JWK of kind `oct`.
    #[test]
    fn rfc_7515s_hs256_example_verifies() {
        let token = "eyJ0eXAiOiJKV1QiLA0KICJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let jwk = Value::Record(BTreeMap::from([
            ("kty".to_string(), Value::Text("oct".into())),
            ("k".to_string(), Value::Text("AyM1SysPpbyDfgZld3umj1qzKObwVMkoqQ-EstJQLr_T-1qS0gZH75aKtMN3Yj0iPS4hcgUuTwjAzZr1Z9CAow".into())),
        ]));
        let Some(Value::Record(claims)) = verify(token, &jwk, 1_300_819_379.0, "joe", "", 0.0) else { panic!() };
        assert_eq!(claims["http://example.com/is_root"], Value::Boolean(true));
        assert_eq!(verify(token, &jwk, 1_300_819_380.0, "joe", "", 0.0), None, "at exp");
    }
}
