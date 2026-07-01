//! Memoization key derivation (P3.3.1) + the shared trustless cache's core (P3.3.2) — the pure
//! half of the cache thesis.
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

use crate::{capabilities, eval, EncodeError, Hash, Resolver, Value, ENCODING_VERSION};
use blake3::Hasher;

// Value type tags — distinct, so a `Number 0`, a `Boolean false`, and an empty `Text` can never
// collide once length prefixes are in play.
const TAG_NUMBER: u8 = 1;
const TAG_BOOLEAN: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_LIST: u8 = 4;
const TAG_EFFECT: u8 = 5;
const TAG_SUM: u8 = 6;

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
    encode_slots(&mut buf, inputs)?;
    let mut h = Hasher::new();
    h.update(unit_hash); // domain: the unit (and, transitively via its hash, its whole closure)
    h.update(&buf); // the inputs
    Ok(*h.finalize().as_bytes())
}

/// A portable, self-describing memo entry (P3.3.2): the claim that evaluating `unit_hash` on
/// `inputs` yields `outputs` — one slot per the unit's declared output (P4.7), each
/// independently present or absent, mirroring `eval`'s own return shape exactly. Self-describing
/// *precisely* so a recipient who did not compute it can verify it — unlike the private local
/// `Memo`'s bare `Value`s, which it trusts because it produced them. This is the unit of exchange
/// in the shared trustless cache.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheEntry {
    pub unit_hash: Hash,
    pub inputs: Vec<Option<Value>>,
    pub outputs: Vec<Option<Value>>,
}

/// The universal key this entry must be filed under: `memo_key(unit_hash, inputs)`. Cheap — one
/// hash over the canonical encoding. The **output is deliberately not in the key**: the key names
/// the *computation*, and a pure computation has exactly one correct output (P4.7: one correct
/// *set* of outputs), so the key is the coordinate and the outputs are what is looked up — and
/// verified — against it. (This cheap check is the cache's analogue of the `Store`'s
/// verify-on-read re-hash; it earns its keep at the network boundary, where a peer supplies the
/// claimed key.)
pub fn entry_key(entry: &CacheEntry) -> Result<Hash, EncodeError> {
    memo_key(&entry.unit_hash, &entry.inputs)
}

/// The trustless verification oracle (P3.3.2): re-execute and confirm. `true` iff `unit_hash`
/// resolves, the unit is **pure** (only a pure unit's output is a stable function of its inputs —
/// I2), and re-evaluating reproduces `outputs` — every slot, independently (P4.7: `eval` itself
/// returns `Vec<Option<Value>>`; a claimed entry with three outputs must match on all three, not
/// just the first). Compared **canonically** (the same NaN/±0.0 folding `memo_key` applies to
/// inputs), so a `NaN`-producing computation verifies across machines despite non-portable NaN
/// bit patterns — and because `Value` is only `PartialEq` (NaN ≠ NaN), canonical comparison is
/// also what lets a legitimate NaN result verify at all.
///
/// The oracle is **decisive** (one correct answer, by determinism) and **always terminates** (the
/// VM is total — no `while`; control is `select` + `fold`); a verifier that could hang would be no
/// verifier. Fail-closed: unresolvable, impure, or unencodable ⇒ `false`. This is the thesis made
/// operational — a memo is checkable by anyone *exactly because* the VM is pure, total, and
/// content-addressed.
pub fn verify_entry(entry: &CacheEntry, resolver: &impl Resolver) -> bool {
    let Some(unit) = resolver.resolve(&entry.unit_hash) else {
        return false; // can't resolve the unit ⇒ can't check the claim ⇒ refuse (fail closed)
    };
    if !capabilities(unit, resolver).is_pure() {
        return false; // impure ⇒ output is not a stable function of inputs ⇒ not cacheable
    }
    let produced = eval(unit, &entry.inputs, resolver); // EvalOutputs derefs to &[Option<Value>]
    match (canonical_slots_bytes(&produced), canonical_slots_bytes(&entry.outputs)) {
        (Ok(a), Ok(b)) => a == b, // canonical equality = "same result", machine-independent
        _ => false,               // unencodable ⇒ cannot compare ⇒ fail closed
    }
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
        Value::Sum { arm, payload } => {
            buf.push(TAG_SUM);
            // `arm` is a discriminant, not a length, but it's the same u32-LE shape `put_len`
            // already writes — reused rather than duplicating the mechanics. The arm participates
            // in the encoding (see `sum_arm_changes_the_key`), so the same payload at a different
            // arm never collides.
            put_len(buf, *arm)?;
            encode_value(buf, payload)?;
        }
    }
    Ok(())
}

/// The per-slot encoding `memo_key` uses for `inputs` — factored out so `verify_entry` can reuse
/// the exact same shape for `outputs` (P4.7), rather than writing a second version of this loop.
/// An absent slot never collides with a present one (`SLOT_ABSENT`/`SLOT_PRESENT` are distinct
/// tags), the same discipline `memo_key`'s own doc already states for inputs.
fn encode_slots(buf: &mut Vec<u8>, slots: &[Option<Value>]) -> Result<(), EncodeError> {
    for slot in slots {
        match slot {
            None => buf.push(SLOT_ABSENT),
            Some(v) => {
                buf.push(SLOT_PRESENT);
                encode_value(buf, v)?;
            }
        }
    }
    Ok(())
}

/// The canonical bytes of a slot vector (a unit's `outputs`, in `verify_entry`) — length-prefixed
/// then `encode_slots`, the same shape `memo_key` builds for `inputs`. Two slot vectors are the
/// "same result" for the cache iff these agree: every NaN folded together, every ±0.0 together,
/// and — since length is prefixed — a 2-output claim can never collide with a 3-output one.
fn canonical_slots_bytes(slots: &[Option<Value>]) -> Result<Vec<u8>, EncodeError> {
    let mut buf = Vec::new();
    put_len(&mut buf, slots.len())?;
    encode_slots(&mut buf, slots)?;
    Ok(buf)
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
            key(&[Some(Value::Sum { arm: 0, payload: Box::new(Value::List(vec![])) })]),
        ];
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 5); // all five encode distinctly
    }

    #[test]
    fn sum_arm_changes_the_key() {
        // The property the cache actually depends on: `some(5)` and a hypothetical "other arm
        // holding 5" must key differently, or two distinct computations could share a cache slot.
        let payload = Some(Value::Number(5.0));
        let some_5 = Value::Sum { arm: 0, payload: Box::new(payload.clone().unwrap()) };
        let other_arm_5 = Value::Sum { arm: 1, payload: Box::new(payload.unwrap()) };
        assert_ne!(key(&[Some(some_5)]), key(&[Some(other_arm_5)]));
    }

    #[test]
    fn order_and_presence_matter() {
        let a = key(&[Some(Value::List(vec![Value::Number(1.0), Value::Number(2.0)]))]);
        let b = key(&[Some(Value::List(vec![Value::Number(2.0), Value::Number(1.0)]))]);
        assert_ne!(a, b); // [1,2] != [2,1]
        assert_ne!(key(&[None]), key(&[Some(Value::Number(0.0))])); // absent != present
    }

    // ── P3.3.2: the entry + the trustless oracle ──
    use crate::{Edge, MapResolver, Param, Port, Type, Unit, UnitBody, BOUNDARY};

    fn const_num(n: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("value", n), Param::frozen("type", "Number")],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn prim(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }

    #[test]
    fn entry_key_is_the_memo_key() {
        let entry = CacheEntry {
            unit_hash: H,
            inputs: vec![Some(Value::Number(3.0))],
            outputs: vec![Some(Value::Number(9.0))],
        };
        assert_eq!(entry_key(&entry).unwrap(), memo_key(&H, &entry.inputs).unwrap());
    }

    #[test]
    fn honest_entry_verifies() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("3")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(3.0))] };
        assert!(verify_entry(&entry, &r));
    }

    #[test]
    fn tampered_output_fails() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("3")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(99.0))] };
        assert!(!verify_entry(&entry, &r)); // re-execution catches the lie
    }

    #[test]
    fn unresolvable_unit_fails() {
        let r = MapResolver::new(); // empty — H resolves to nothing
        let entry = CacheEntry { unit_hash: H, inputs: vec![], outputs: vec![Some(Value::Number(3.0))] };
        assert!(!verify_entry(&entry, &r)); // can't check ⇒ refuse
    }

    #[test]
    fn impure_unit_fails() {
        let mut r = MapResolver::new();
        let h = r.insert(prim("clock")).unwrap(); // a driver (capability Clock) ⇒ not pure
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(0.0))] };
        assert!(!verify_entry(&entry, &r)); // impure outputs aren't stable ⇒ uncacheable
    }

    #[test]
    fn nan_output_verifies_canonically() {
        // inf - inf = NaN. `Value` is only `PartialEq` (NaN ≠ NaN), so this verifies ONLY because
        // the oracle compares canonically — the very property that makes a NaN result checkable
        // across machines (whose NaN bit patterns may differ).
        let mut r = MapResolver::new();
        let h = r.insert(prim("subtract")).unwrap();
        let inf = f64::INFINITY;
        let entry = CacheEntry {
            unit_hash: h,
            inputs: vec![Some(Value::Number(inf)), Some(Value::Number(inf))],
            outputs: vec![Some(Value::Number(f64::NAN))],
        };
        assert!(verify_entry(&entry, &r));
    }

    #[test]
    fn multi_output_entry_verifies_every_slot_independently() {
        // A genuine multi-output composite (P4.7): add and divide off the same two inputs, both
        // exposed. Proves verify_entry compares the WHOLE outputs vector, not just the first
        // slot — an honest entry with a partial (Some/None mixed) claim, matching divide(3,0)'s
        // real failure, must still verify; a tampered claim on either slot must not.
        //
        // Deliberately NOT the zero-input prim() helper: eval_composite derives how many values
        // to gather for a child from *that child's own declared inputs.len()*, not from the
        // edges pointing at it — prim("add") declares zero inputs, which is invisible when it's
        // passed straight to eval() (compute reads the inputs slice directly, ignoring
        // unit.inputs entirely) but would silently break as a composite child here.
        fn binop(name: &str) -> Unit {
            Unit {
                body: UnitBody::Primitive { name: name.into() },
                params: vec![],
                inputs: vec![
                    Port { name: "a".into(), ty: Type::Number },
                    Port { name: "b".into(), ty: Type::Number },
                ],
                outputs: vec![Port { name: "out".into(), ty: Type::Number }],
            }
        }
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add")).unwrap();
        let hdiv = r.insert(binop("divide")).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![hadd, hdiv],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 },
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 1, to_port: 1 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 1 },
                ],
            },
            params: vec![],
            inputs: vec![
                Port { name: "a".into(), ty: Type::Number },
                Port { name: "b".into(), ty: Type::Number },
            ],
            outputs: vec![
                Port { name: "out0".into(), ty: Type::Number },
                Port { name: "out1".into(), ty: Type::Number },
            ],
        };
        let h = r.insert(comp).unwrap();

        // Honest, partial claim: add(3,0)=3 succeeds, divide(3,0) fails — matches real behaviour.
        let honest = CacheEntry {
            unit_hash: h,
            inputs: vec![Some(Value::Number(3.0)), Some(Value::Number(0.0))],
            outputs: vec![Some(Value::Number(3.0)), None],
        };
        assert!(verify_entry(&honest, &r));

        // Tampered on the SECOND slot only (claiming divide didn't fail) — must still be caught,
        // proving the comparison isn't just checking the first output and stopping.
        let lied_second = CacheEntry {
            unit_hash: h,
            inputs: honest.inputs.clone(),
            outputs: vec![Some(Value::Number(3.0)), Some(Value::Number(99.0))],
        };
        assert!(!verify_entry(&lied_second, &r));

        // Tampered on the FIRST slot only (the one that actually succeeded) — also caught.
        let lied_first = CacheEntry {
            unit_hash: h,
            inputs: honest.inputs,
            outputs: vec![Some(Value::Number(4.0)), None],
        };
        assert!(!verify_entry(&lied_first, &r));
    }
}