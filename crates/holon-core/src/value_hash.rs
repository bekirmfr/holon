//! The canonical encoding and content hash of a `Value`.
//!
//! `Value` derives only `PartialEq` (not `Eq`/`Hash`) because it carries an `f64`, so "are these
//! two values the same?" is a canonicalization decision, made here once. The encoding is
//! **tagged + `u32`-LE length-prefixed** and collision-free, mirroring the unit preimage
//! encoder's discipline, and deterministic across machines (I2 at the value layer).
//!
//! The one hazardous case is `f64`. `NaN`/`inf` can't *enter* as literals (`const` rejects
//! non-finite values) but are reachable as *computed* values (`1e308*1e308 → inf`, `inf-inf →
//! NaN`), so they must encode deterministically. Two collapses, each sound because eval treats
//! the collapsed values identically:
//! - every `NaN` bit pattern → one canonical encoding (IEEE ops and comparisons are
//!   payload-agnostic, so all `NaN` inputs produce the same output);
//! - `-0.0` and `+0.0` → the same encoding (`-0.0 == +0.0`, and eval agrees; `to_text` and
//!   `json_stringify` print both as `0`, so no output tells them apart either).
//!
//! Two consumers:
//! - [`hash_value`]: a value's content hash, e.g. a state cell's, so a whole state hashes from
//!   its values' hashes (a Merkle map) and a run can be named by what it started from;
//! - the memo (`memo.rs`): its cache keys use the same encoding of each input.

use blake3::Hasher;

use crate::{DecodeError, EncodeError, Hash, Value, ENCODING_VERSION};

// Value type tags — distinct, so a `Number 0`, a `Boolean false`, and an empty `Text` can never
// collide once length prefixes are in play.
const TAG_NUMBER: u8 = 1;
const TAG_BOOLEAN: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_LIST: u8 = 4;
const TAG_EFFECT: u8 = 5;
const TAG_SUM: u8 = 6;
const TAG_RECORD: u8 = 7;

// Input-slot tags: an absent input must never collide with a present one.
const SLOT_ABSENT: u8 = 0;
const SLOT_PRESENT: u8 = 1;

/// Domain tag of a standalone value's encoding, after the version byte. A unit preimage starts
/// `[version, 0x01 | 0x02]` (`hash.rs`), so a value hash can never equal a unit hash.
const DOMAIN_VALUE: u8 = 0x20;

// The single canonical quiet-NaN bit pattern every `NaN` collapses to. Hard-coded (not
// `f64::NAN.to_bits()`) so it is unambiguous and platform-independent.
const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// A value's canonical bytes: the encoding version, the value domain tag, then the value.
/// Fails only if a text, list or record is too large to length-prefix (`u32`).
pub fn encode_value(v: &Value) -> Result<Vec<u8>, EncodeError> {
    let mut buf = vec![ENCODING_VERSION, DOMAIN_VALUE];
    encode_value_into(&mut buf, v)?;
    Ok(buf)
}

/// A value's content hash: BLAKE3 of [`encode_value`]. Equal values (with every `NaN` and both
/// zeros folded together) hash equal, on every machine.
pub fn hash_value(v: &Value) -> Result<Hash, EncodeError> {
    let mut h = Hasher::new();
    h.update(&encode_value(v)?);
    Ok(*h.finalize().as_bytes())
}

/// Domain tag of a state root's preimage (see [`state_root`]).
const DOMAIN_STATE: u8 = 0x21;

/// How deep a decoded value may nest. Far beyond anything `json_parse` (depth 64) or a flow
/// builds, and a hard stop for a hostile stream.
const MAX_VALUE_DEPTH: usize = 256;

/// The value [`encode_value`] encoded. Every read is bounds-checked and nothing is
/// pre-allocated from a length the stream claims, so a malformed or hostile blob is an error,
/// never a panic or a huge allocation. Trailing bytes are an error too: one encoding, one value.
pub fn decode_value(bytes: &[u8]) -> Result<Value, DecodeError> {
    let mut r = Reader { bytes, at: 0 };
    let version = r.byte()?;
    if version != ENCODING_VERSION {
        return Err(DecodeError::WrongVersion { found: version, expected: ENCODING_VERSION });
    }
    let domain = r.byte()?;
    if domain != DOMAIN_VALUE {
        return Err(DecodeError::BadTag(domain));
    }
    let v = r.value(0)?;
    if r.at != bytes.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(v)
}

/// The Merkle root of a state: one hash for a whole key → value map, computed from each key and
/// its value's hash ([`hash_value`]) rather than from the values themselves, so a state whose
/// values are already hashed (once, when written) gets its root without re-reading them.
/// `cells` must come in key order, as a `BTreeMap` iterates; two states are equal iff their
/// roots are.
pub fn state_root<'a>(cells: impl ExactSizeIterator<Item = (&'a str, &'a Hash)>) -> Result<Hash, EncodeError> {
    let mut buf = vec![ENCODING_VERSION, DOMAIN_STATE];
    put_len(&mut buf, cells.len())?;
    for (key, hash) in cells {
        put_bytes(&mut buf, key.as_bytes())?;
        buf.extend_from_slice(hash);
    }
    Ok(*blake3::hash(&buf).as_bytes())
}

/// Domain tag of a run id's preimage (see [`run_id`]).
const DOMAIN_RUN: u8 = 0x22;

/// A run's id: the hash of the deployment it ran on and the Merkle root of everything it started
/// from (its inputs, the state it read, its sourced values and a host nonce, as the caller lays
/// them out with [`state_root`]). Anyone holding the run's recorded input can recompute it; the
/// nonce makes two runs from the same deployment and input distinct.
pub fn run_id(deployment: &Hash, input_root: &Hash) -> Hash {
    let mut buf = vec![ENCODING_VERSION, DOMAIN_RUN];
    buf.extend_from_slice(deployment);
    buf.extend_from_slice(input_root);
    *blake3::hash(&buf).as_bytes()
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let b = *self.bytes.get(self.at).ok_or(DecodeError::UnexpectedEof)?;
        self.at += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        let s = self.bytes.get(self.at..end).ok_or(DecodeError::UnexpectedEof)?;
        self.at = end;
        Ok(s)
    }
    fn len(&mut self) -> Result<usize, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn text(&mut self) -> Result<String, DecodeError> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
    fn value(&mut self, depth: usize) -> Result<Value, DecodeError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(DecodeError::TypeTooDeep);
        }
        Ok(match self.byte()? {
            TAG_NUMBER => {
                let b = self.take(8)?;
                Value::Number(f64::from_bits(u64::from_le_bytes(b.try_into().expect("8 bytes"))))
            }
            TAG_BOOLEAN => match self.byte()? {
                0 => Value::Boolean(false),
                1 => Value::Boolean(true),
                b => return Err(DecodeError::BadTag(b)),
            },
            TAG_TEXT => Value::Text(self.text()?),
            TAG_LIST => {
                let n = self.len()?;
                let mut xs = Vec::new();
                for _ in 0..n {
                    xs.push(self.value(depth + 1)?);
                }
                Value::List(xs)
            }
            TAG_EFFECT => {
                let sink = self.text()?;
                Value::Effect { sink, payload: Box::new(self.value(depth + 1)?) }
            }
            TAG_SUM => {
                let arm = self.len()?;
                Value::Sum { arm, payload: Box::new(self.value(depth + 1)?) }
            }
            TAG_RECORD => {
                let n = self.len()?;
                let mut fields = std::collections::BTreeMap::new();
                for _ in 0..n {
                    let k = self.text()?;
                    let v = self.value(depth + 1)?;
                    // Keys must arrive in order and once each, or two encodings would decode
                    // to the same record and the encoding would stop being canonical.
                    if fields.keys().next_back().is_some_and(|last: &String| *last >= k) {
                        return Err(DecodeError::BadTag(TAG_RECORD));
                    }
                    fields.insert(k, v);
                }
                Value::Record(fields)
            }
            t => return Err(DecodeError::BadTag(t)),
        })
    }
}

/// Canonical, collision-free encoding of a `Value` appended to `buf`.
pub(crate) fn encode_value_into(buf: &mut Vec<u8>, v: &Value) -> Result<(), EncodeError> {
    match v {
        Value::Number(n) => {
            buf.push(TAG_NUMBER);
            let bits = if n.is_nan() {
                CANONICAL_NAN
            } else if *n == 0.0 {
                0 // collapses -0.0 and +0.0 (which are ==, and which eval treats equally)
            } else {
                n.to_bits()
            };
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        Value::Boolean(b) => {
            buf.push(TAG_BOOLEAN);
            buf.push(*b as u8);
        }
        Value::Text(s) => {
            buf.push(TAG_TEXT);
            put_bytes(buf, s.as_bytes())?; // byte-exact: Text is opaque, no Unicode normalization
        }
        Value::List(xs) => {
            buf.push(TAG_LIST);
            put_len(buf, xs.len())?;
            for x in xs {
                encode_value_into(buf, x)?; // order significant
            }
        }
        Value::Effect { sink, payload } => {
            buf.push(TAG_EFFECT);
            put_bytes(buf, sink.as_bytes())?;
            encode_value_into(buf, payload)?;
        }
        Value::Sum { arm, payload } => {
            buf.push(TAG_SUM);
            // `arm` is a discriminant, not a length, but it's the same u32-LE shape `put_len`
            // already writes. The arm participates in the encoding, so the same payload at a
            // different arm never collides.
            put_len(buf, *arm)?;
            encode_value_into(buf, payload)?;
        }
        Value::Record(fields) => {
            buf.push(TAG_RECORD);
            put_len(buf, fields.len())?;
            for (k, v) in fields {
                put_bytes(buf, k.as_bytes())?; // key order is the BTreeMap's: canonical
                encode_value_into(buf, v)?;
            }
        }
    }
    Ok(())
}

/// A slot vector (a unit's inputs or outputs) appended to `buf`: one tag per slot, absent or
/// present, then the value. An absent slot never collides with a present one.
pub(crate) fn encode_slots(buf: &mut Vec<u8>, slots: &[Option<Value>]) -> Result<(), EncodeError> {
    for slot in slots {
        match slot {
            None => buf.push(SLOT_ABSENT),
            Some(v) => {
                buf.push(SLOT_PRESENT);
                encode_value_into(buf, v)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn put_len(buf: &mut Vec<u8>, n: usize) -> Result<(), EncodeError> {
    let len: u32 = n.try_into().map_err(|_| EncodeError::FieldTooLong)?;
    buf.extend_from_slice(&len.to_le_bytes());
    Ok(())
}

fn put_bytes(buf: &mut Vec<u8>, b: &[u8]) -> Result<(), EncodeError> {
    put_len(buf, b.len())?;
    buf.extend_from_slice(b);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn num(n: f64) -> Value {
        Value::Number(n)
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn hex(h: Hash) -> String {
        h.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The format, byte for byte. A change here changes every value hash ever recorded (state
    /// roots, run ids), so it must come with an `ENCODING_VERSION` bump.
    #[test]
    fn the_encoding_is_pinned() {
        let v = Value::Record(BTreeMap::from([
            ("n".to_string(), num(1.5)),
            ("t".to_string(), Value::List(vec![txt("a"), Value::Boolean(true)])),
        ]));
        let mut expected = vec![ENCODING_VERSION, 0x20, TAG_RECORD, 2, 0, 0, 0];
        expected.extend([1, 0, 0, 0, b'n', TAG_NUMBER]);
        expected.extend(1.5f64.to_bits().to_le_bytes());
        expected.extend([1, 0, 0, 0, b't', TAG_LIST, 2, 0, 0, 0, TAG_TEXT, 1, 0, 0, 0, b'a', TAG_BOOLEAN, 1]);
        assert_eq!(encode_value(&v).unwrap(), expected);
        assert_eq!(
            hex(hash_value(&txt("buy milk")).unwrap()),
            hex(*blake3::hash(&[ENCODING_VERSION, 0x20, TAG_TEXT, 8, 0, 0, 0, b'b', b'u', b'y', b' ', b'm', b'i', b'l', b'k']).as_bytes())
        );
    }

    #[test]
    fn both_zeros_and_every_nan_hash_alike() {
        assert_eq!(hash_value(&num(0.0)).unwrap(), hash_value(&num(-0.0)).unwrap());
        let other_nan = f64::from_bits(0x7ff8_0000_0000_0001);
        assert!(other_nan.is_nan());
        assert_eq!(hash_value(&num(f64::NAN)).unwrap(), hash_value(&num(other_nan)).unwrap());
        assert_ne!(hash_value(&num(f64::NAN)).unwrap(), hash_value(&num(0.0)).unwrap());
        assert_ne!(hash_value(&num(f64::INFINITY)).unwrap(), hash_value(&num(f64::NEG_INFINITY)).unwrap());
    }

    #[test]
    fn different_values_hash_differently() {
        let values = [
            num(0.0),
            Value::Boolean(false),
            txt(""),
            Value::List(vec![]),
            Value::Record(BTreeMap::new()),
            Value::Sum { arm: 0, payload: Box::new(num(0.0)) },
            Value::Sum { arm: 1, payload: Box::new(num(0.0)) },
            Value::Effect { sink: "egress".into(), payload: Box::new(num(0.0)) },
            Value::List(vec![txt("ab")]),
            Value::List(vec![txt("a"), txt("b")]),
        ];
        let hashes: Vec<Hash> = values.iter().map(|v| hash_value(v).unwrap()).collect();
        for i in 0..hashes.len() {
            for j in i + 1..hashes.len() {
                assert_ne!(hashes[i], hashes[j], "{:?} vs {:?}", values[i], values[j]);
            }
        }
    }

    #[test]
    fn decoding_inverts_encoding_and_refuses_anything_else() {
        let v = Value::List(vec![
            num(-2.5),
            txt("ünïcode"),
            Value::Record(BTreeMap::from([("a".to_string(), Value::Boolean(false)), ("b".to_string(), num(1.0))])),
            Value::Sum { arm: 3, payload: Box::new(Value::List(vec![])) },
            Value::Effect { sink: "store_write:k".into(), payload: Box::new(txt("x")) },
        ]);
        let bytes = encode_value(&v).unwrap();
        assert_eq!(decode_value(&bytes).unwrap(), v);
        // Every truncation fails cleanly; so do trailing bytes and a wrong version or domain.
        for n in 0..bytes.len() {
            assert!(decode_value(&bytes[..n]).is_err(), "truncated at {n}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode_value(&longer), Err(DecodeError::TrailingBytes));
        assert!(matches!(decode_value(&[ENCODING_VERSION + 1, 0x20, TAG_BOOLEAN, 1]), Err(DecodeError::WrongVersion { .. })));
        assert_eq!(decode_value(&[ENCODING_VERSION, 0x01, TAG_BOOLEAN, 1]), Err(DecodeError::BadTag(0x01)));
        // A list that claims four billion items, with none there, is an error, not an allocation.
        assert_eq!(decode_value(&[ENCODING_VERSION, 0x20, TAG_LIST, 0xff, 0xff, 0xff, 0xff]), Err(DecodeError::UnexpectedEof));
        // Record keys out of order would be a second encoding of the same record: refused.
        let mut swapped = vec![ENCODING_VERSION, 0x20, TAG_RECORD, 2, 0, 0, 0];
        swapped.extend([1, 0, 0, 0, b'b', TAG_BOOLEAN, 1, 1, 0, 0, 0, b'a', TAG_BOOLEAN, 1]);
        assert!(decode_value(&swapped).is_err());
        // Nesting has a floor.
        let mut deep = num(1.0);
        for _ in 0..300 {
            deep = Value::List(vec![deep]);
        }
        assert_eq!(decode_value(&encode_value(&deep).unwrap()), Err(DecodeError::TypeTooDeep));
    }

    #[test]
    fn a_state_root_is_the_hash_of_its_keys_and_value_hashes() {
        let h = |v: &Value| hash_value(v).unwrap();
        let state = |pairs: &[(&str, Value)]| -> BTreeMap<String, Hash> {
            pairs.iter().map(|(k, v)| (k.to_string(), h(v))).collect()
        };
        let root = |s: &BTreeMap<String, Hash>| state_root(s.iter().map(|(k, h)| (k.as_str(), h))).unwrap();
        let a = state(&[("todos", Value::List(vec![txt("milk")])), ("count", num(1.0))]);
        let b = state(&[("count", num(1.0)), ("todos", Value::List(vec![txt("milk")]))]);
        assert_eq!(root(&a), root(&b), "insertion order doesn't matter");
        assert_ne!(root(&a), root(&state(&[("todos", Value::List(vec![txt("milk")]))])), "a missing key changes it");
        assert_ne!(root(&a), root(&state(&[("todos", Value::List(vec![])), ("count", num(1.0))])), "a changed value too");
        assert_ne!(root(&a), root(&state(&[("todo", Value::List(vec![txt("milk")])), ("count", num(1.0))])), "and a renamed key");
        assert_ne!(root(&BTreeMap::new()), h(&Value::Record(BTreeMap::new())), "a state root is never a value hash");
    }

    #[test]
    fn a_value_hash_never_equals_a_unit_hash() {
        // Same version byte, different domain tag: the streams differ from the second byte.
        let unit = crate::interface_of("add").unwrap();
        let pre = crate::encode_preimage(&unit).unwrap();
        assert_eq!(pre[0], ENCODING_VERSION);
        assert_ne!(pre[1], 0x20);
    }
}
