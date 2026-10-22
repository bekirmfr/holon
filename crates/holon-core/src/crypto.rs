//! Signatures and encodings: what checking a webhook's signature takes (Stripe, GitHub, Slack all
//! sign with HMAC-SHA256), and can't be built from other nodes, or not safely: a comparison made of
//! ordinary nodes stops at the first differing character, and its timing tells an attacker how much
//! of a guessed signature was right.
//!
//! Text in, text out, like every other value: a hash or HMAC is taken over the text's UTF-8 bytes
//! and given as `hex` (lowercase) or `base64`; `hex` and `base64` encode a text's UTF-8 bytes, and
//! decode back to text, absent when the input is malformed or the bytes aren't UTF-8 (Holon has no
//! bytes value).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// A digest as the `encoding` param asks: lowercase `hex` (the default) or padded `base64`.
fn encode_digest(bytes: &[u8], encoding: &str) -> Option<String> {
    match encoding {
        "hex" => Some(hex_encode(bytes)),
        "base64" => Some(base64_encode(bytes, STANDARD, true)),
        _ => None,
    }
}

pub(crate) fn sha256(text: &str, encoding: &str) -> Option<String> {
    encode_digest(&Sha256::digest(text.as_bytes()), encoding)
}

pub(crate) fn hmac_sha256(key: &str, message: &str, encoding: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(message.as_bytes());
    encode_digest(&mac.finalize().into_bytes(), encoding)
}

/// Whether `a` and `b` are equal, taking the same time wherever they differ: every byte is
/// compared, and the result is only read at the end. Only the lengths can show (as with every
/// constant-time compare), which is why signatures are compared at their fixed length.
pub(crate) fn secure_equals(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

const DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0xf) as usize] as char);
    }
    s
}

/// Upper- or lowercase hex, even length, nothing else.
pub(crate) fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let s = s.as_bytes();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// RFC 4648 §4 (`standard`) and §5 (`url`, URL- and filename-safe) alphabets.
const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(crate) fn alphabet(name: &str) -> Option<&'static [u8; 64]> {
    match name {
        "standard" => Some(STANDARD),
        "url" => Some(URL),
        _ => None,
    }
}

/// Base64 of `bytes`; `pad` adds `=` to a multiple of 4 (standard does, url leaves it off, as JWTs
/// and most URL uses expect).
pub(crate) fn base64_encode(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        let chars = chunk.len() + 1;
        for i in 0..4 {
            if i < chars {
                s.push(alphabet[(n >> (18 - 6 * i) & 0x3f) as usize] as char);
            } else if pad {
                s.push('=');
            }
        }
    }
    s
}

/// The bytes `s` encodes in `alphabet`, padded or not; `None` for any other character, a length
/// no encoding produces, or leftover bits that aren't zero (so one text has one decoding).
pub(crate) fn base64_decode(s: &str, alphabet: &[u8; 64]) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let value = |c: u8| alphabet.iter().position(|&a| a == c).map(|p| p as u32);
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = chunk.len() - 1;
        if bytes[keep..].iter().any(|&b| b != 0) && chunk.len() < 4 {
            return None;
        }
        out.extend_from_slice(&bytes[..keep]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_fips_180_vectors() {
        assert_eq!(sha256("", "hex").unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256("abc", "hex").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256("abc", "nope"), None);
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_and_a_real_webhook() {
        // RFC 4231 §4.2 (test case 1) and §4.3 (test case 2).
        assert_eq!(
            hmac_sha256(&"\u{0b}".repeat(20), "Hi There", "hex").unwrap(),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hmac_sha256("Jefe", "what do ya want for nothing?", "hex").unwrap(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(hmac_sha256("Jefe", "what do ya want for nothing?", "base64").unwrap(), "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=");
        // GitHub's "Validating webhook deliveries" example (X-Hub-Signature-256: sha256=<this>).
        assert_eq!(
            hmac_sha256("It's a Secret to Everybody", "Hello, World!", "hex").unwrap(),
            "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
    }

    #[test]
    fn base64_and_hex_match_rfc_4648_vectors_and_round_trip() {
        let cases = [("", "", ""), ("f", "Zg==", "Zg"), ("fo", "Zm8=", "Zm8"), ("foo", "Zm9v", "Zm9v"), ("foob", "Zm9vYg==", "Zm9vYg"), ("fooba", "Zm9vYmE=", "Zm9vYmE"), ("foobar", "Zm9vYmFy", "Zm9vYmFy")];
        for (text, standard, url) in cases {
            assert_eq!(base64_encode(text.as_bytes(), STANDARD, true), standard);
            assert_eq!(base64_encode(text.as_bytes(), URL, false), url);
            assert_eq!(base64_decode(standard, STANDARD).unwrap(), text.as_bytes());
            assert_eq!(base64_decode(url, URL).unwrap(), text.as_bytes(), "unpadded decodes too");
        }
        assert_eq!(base64_encode("ünï".as_bytes(), STANDARD, true), "w7xuw68=");
        assert_eq!(base64_encode(&[0xfb, 0xff], STANDARD, true), "+/8=");
        assert_eq!(base64_encode(&[0xfb, 0xff], URL, false), "-_8");
        assert_eq!(base64_decode("Zg=", STANDARD), Some(b"f".to_vec()), "padding is optional");
        for bad in ["Z", "Zh==", "Zm9v!", "-_8"] {
            assert_eq!(base64_decode(bad, STANDARD), None, "{bad}");
        }
        assert_eq!(hex_encode(b"foobar"), "666f6f626172");
        assert_eq!(hex_decode("666F6f626172").unwrap(), b"foobar");
        for bad in ["6", "6g", "0x66"] {
            assert_eq!(hex_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn secure_equals_is_equality() {
        assert!(secure_equals("abc", "abc") && secure_equals("", ""));
        assert!(!secure_equals("abc", "abd") && !secure_equals("abc", "ab") && !secure_equals("abc", "Abc"));
    }
}
