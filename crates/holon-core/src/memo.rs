//! Memoization key derivation (P3.3.1) — the pure half of the cache thesis.
//!
//! A pure, content-pinned unit is a function from inputs to output that never changes, so its
//! result is cacheable, keyed by `(unit-hash, inputs)`. This module derives that key:
//!
//! `memo_key = BLAKE3( unit_hash ‖ canonical_encode(inputs) )`
//!
//! The key is itself a **content hash** — same derivation on every machine, so the local cache
//! and a future shared/trustless cache (P3.3.2) key identically, and a result one party computed
//! another can look up and verify by re-hashing. The resolver never enters the key: a pure
//! unit's `unit_hash` already captures its entire transitive closure (I6), so `(unit_hash,
//! inputs)` fully determines the output.
//!
//! ## The canonical `Value` encoder
//! `Value` derives only `PartialEq` (not `Eq`/`Hash`) because it carries an `f64`. So "are these
//! two inputs the same, for caching?" is a **canonicalization decision**, not a free derive. The
//! encoding is **tagged + `u32`-LE length-prefixed**, collision-free, mirroring the preimage
//! encoder's discipline — deterministic across machines, the I2 guarantee at the value layer.
//!
//! The one hazardous case is `f64`. `NaN`/`inf` can't *enter* as literals (`const` rejects
//! non-finite values) but are reachable as *computed* values (`1e308*1e308 → inf`, `inf-inf →
//! NaN`), so they must encode deterministically. Two collapses, each sound because eval treats
//! the collapsed values identically:
//! - every `NaN` bit pattern → one canonical encoding (IEEE ops and comparisons are
//!   payload-agnostic, so all `NaN` inputs produce the same output);
//! - `-0.0` and `+0.0` → the same encoding (`-0.0 == +0.0`, and eval agrees).

use crate::{EncodeError, Hash, Value, ENCODING_VERSION};
use blake3::Hasher;

// Value type tags — distinct, so a `Number 0`, a `Boolean false`, and an empty `Text` can never
// collide once length prefixes are in play.
const TAG_NUMBER: u8 = 1;
const TAG_BOOLEAN: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_LIST: u8 = 4;
const TAG_EFFECT: u8 = 5;

// Input-slot tags: an absent input must never collide with a present one.
const SLOT_ABSENT: u8 = 0;
const SLOT_PRESENT: u8 = 1;

// The single canonical quiet-NaN bit pattern every `NaN` collapses to. Hard-coded (not
// `f64::NAN.to_bits()`) so it is unambiguous and platform-independent.
const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// The memoization cache key for evaluating the unit identified by `unit_hash` on `inputs`:
/// a content hash of the unit's identity and a canonical encoding of its inputs. Fails only if
/// an input is too large to length-prefix (`u32` budget), matching `hash_unit`'s discipline.
pub fn memo_key(unit_hash: &Hash, inputs: &[Option<Value>]) -> Result<Hash, EncodeError> {
    // Version the stream up front, like the preimage encoder, so the format can evolve.
    let mut buf = vec![ENCODING_VERSION];
    put_len(&mut buf, inputs.len())?;
    for slot in inputs {
        match slot {
            None => buf.push(SLOT_ABSENT),
            Some(v) => {
                buf.push(SLOT_PRESENT);
                encode_value(&mut buf, v)?;
            }
        }
    }
    let mut h = Hasher::new();
    h.update(unit_hash); // domain: the unit (and, transitively via its hash, its whole closure)
    h.update(&buf); // the inputs
    Ok(*h.finalize().as_bytes())
}

/// Canonical, collision-free encoding of a `Value` appended to `buf`.
fn encode_value(buf: &mut Vec<u8>, v: &Value) -> Result<(), EncodeError> {
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
                encode_value(buf, x)?; // order significant
            }
        }
        Value::Effect { sink, payload } => {
            buf.push(TAG_EFFECT);
            put_bytes(buf, sink.as_bytes())?;
            encode_value(buf, payload)?;
        }
    }
    Ok(())
}

fn put_len(buf: &mut Vec<u8>, n: usize) -> Result<(), EncodeError> {
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

    const H: Hash = [7u8; 32]; // a fixed unit hash to key against

    fn key(inputs: &[Option<Value>]) -> Hash {
        memo_key(&H, inputs).unwrap()
    }

    #[test]
    fn encoding_is_deterministic() {
        let v = Value::List(vec![
            Value::Number(1.5),
            Value::Text("hi".into()),
            Value::Boolean(true),
        ]);
        assert_eq!(key(&[Some(v.clone())]), key(&[Some(v)]));
    }

    #[test]
    fn signed_zero_canonicalizes() {
        assert_eq!(key(&[Some(Value::Number(-0.0))]), key(&[Some(Value::Number(0.0))]));
    }

    #[test]
    fn all_nans_canonicalize() {
        let nan_a = f64::NAN;
        let nan_b = f64::from_bits(0xfff8_0000_0000_0001); // a different NaN bit pattern
        assert!(nan_a.is_nan() && nan_b.is_nan());
        assert_eq!(key(&[Some(Value::Number(nan_a))]), key(&[Some(Value::Number(nan_b))]));
        // ...and distinct from a finite number.
        assert_ne!(key(&[Some(Value::Number(nan_a))]), key(&[Some(Value::Number(0.0))]));
    }

    #[test]
    fn types_do_not_collide() {
        let mut keys = vec![
            key(&[Some(Value::Number(0.0))]),
            key(&[Some(Value::Boolean(false))]),
            key(&[Some(Value::Text(String::new()))]),
            key(&[Some(Value::List(vec![]))]),
        ];
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 4); // all four encode distinctly
    }

    #[test]
    fn order_and_presence_matter() {
        let a = key(&[Some(Value::List(vec![Value::Number(1.0), Value::Number(2.0)]))]);
        let b = key(&[Some(Value::List(vec![Value::Number(2.0), Value::Number(1.0)]))]);
        assert_ne!(a, b); // [1,2] != [2,1]
        assert_ne!(key(&[None]), key(&[Some(Value::Number(0.0))])); // absent != present
    }
}