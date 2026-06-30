//! holon-wasm — the WASM bridge (P4.2a).
//!
//! Exposes `holon-core`'s pure operations to JavaScript so the editor runs the **same** core,
//! compiled to wasm32 — one evaluator, never a second (I5/I2). Depends on `holon-core` **only**
//! (never `holon-host`): the host's native I/O has no place in a browser, and the dependency edge
//! keeps it out, Cargo-enforced.
//!
//! This first inch bridges only **bytes→bytes** operations — decode a unit's canonical preimage
//! and hash it — which need no `Value` serialization across the JS boundary and have no float
//! divergence. So they prove the cleanest, most important parity first: **content-addressing is
//! bit-identical native vs wasm.** (If hashes differed across targets, the whole content-addressed
//! model would break.) The `eval` bridge — with its JS `Value` boundary and NaN/±0.0
//! canonicalization — is the next inch (P4.2b).

use holon_core::{decode_preimage, encode_preimage, hash_unit};
use wasm_bindgen::prelude::*;

/// Hash a unit from its canonical preimage bytes: decode → re-hash → its 32-byte content address.
/// Bytes in, bytes out — no `Value` crosses the boundary. The wasm-compiled result MUST equal the
/// native `hash_unit` for the same unit; that equality is the parity this inch exists to prove.
#[wasm_bindgen]
pub fn hash_unit_bytes(preimage: &[u8]) -> Result<Vec<u8>, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    let hash = hash_unit(&unit).map_err(|e| JsError::new(&format!("hash: {e:?}")))?;
    Ok(hash.to_vec())
}

/// Decode then re-encode a unit's preimage — a canonical-form check the editor can use to validate
/// bytes it constructed. The output is the canonical preimage (identical to the input iff the input
/// was already canonical), so a round-trip mismatch flags a malformed/non-canonical encoding early.
#[wasm_bindgen]
pub fn canonicalize_preimage(preimage: &[u8]) -> Result<Vec<u8>, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    encode_preimage(&unit).map_err(|e| JsError::new(&format!("encode: {e:?}")))
}

// ── Parity: one body, run on BOTH targets ───────────────────────────────────────────────────
// The same `check_hash_parity` runs natively (`cargo test`) and under wasm (`wasm-pack test
// --node`). Each asserts the bridge equals the *direct* core call on its own target; since BLAKE3
// is byte-deterministic and the preimage encoder is integer/byte-only (u32 length prefixes — no
// usize-width or float hazard), native-core and wasm-core hashes are identical, so both passes
// together mean the wasm core hashes bit-identically to native.

#[cfg(test)]
fn corpus() -> Vec<holon_core::Unit> {
    use holon_core::{Param, Type, Unit, UnitBody};
    let prim = |name: &str| Unit {
        body: UnitBody::Primitive { name: name.into() },
        params: vec![],
        inputs: vec![],
        output: Some(Type::Number),
    };
    let konst = |v: &str| Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("value", v), Param::frozen("type", "Number")],
        inputs: vec![],
        output: Some(Type::Number),
    };
    vec![prim("add"), prim("subtract"), konst("0"), konst("42")]
}

#[cfg(test)]
fn check_hash_parity() {
    for u in corpus() {
        let bytes = encode_preimage(&u).unwrap();
        // bridge (decode+hash) == direct core hash, on whatever target this runs on
        assert_eq!(hash_unit_bytes(&bytes).unwrap(), hash_unit(&u).unwrap().to_vec());
        // and canonicalizing an already-canonical preimage is a no-op
        assert_eq!(canonicalize_preimage(&bytes).unwrap(), bytes);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::*;
    #[test]
    fn hash_parity_native() {
        check_hash_parity();
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;
    #[wasm_bindgen_test]
    fn hash_parity_wasm() {
        check_hash_parity();
    }
}