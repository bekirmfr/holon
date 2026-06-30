//! holon-wasm — the WASM bridge (P4.2).
//!
//! Exposes `holon-core`'s pure operations to JavaScript so the editor runs the **same** core,
//! compiled to wasm32 — one evaluator, never a second (I5/I2). Depends on `holon-core` **only**
//! (never `holon-host`): the host's native I/O has no place in a browser, and the dependency edge
//! keeps it out, Cargo-enforced.
//!
//! - **P4.2a** bridged bytes→bytes hashing (`hash_unit_bytes`, `canonicalize_preimage`) and proved
//!   content-addressing is bit-identical native vs wasm.
//! - **P4.2b** (this) bridges **evaluation**: a stateful [`Evaluator`] (insert units → a resolver,
//!   then eval against it) and the JS `Value` boundary ([`WValue`]). `Value` crosses via
//!   `serde-wasm-bindgen` (not JSON — eval can yield `NaN`/`Infinity`, which JSON cannot represent
//!   but JS numbers can), and eval is proven bit-identical native vs wasm (the NaN case via
//!   `is_nan`, the canonical view — which bits a NaN carries is target-dependent and irrelevant).

use holon_core::{decode_preimage, encode_preimage, hash_unit, MapResolver, Value};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Hash a unit from its canonical preimage bytes: decode → re-hash → its 32-byte content address.
#[wasm_bindgen]
pub fn hash_unit_bytes(preimage: &[u8]) -> Result<Vec<u8>, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    let hash = hash_unit(&unit).map_err(|e| JsError::new(&format!("hash: {e:?}")))?;
    Ok(hash.to_vec())
}

/// Decode then re-encode a unit's preimage — a canonical-form check (output equals input iff the
/// input was already canonical), so a round-trip mismatch flags a malformed encoding early.
#[wasm_bindgen]
pub fn canonicalize_preimage(preimage: &[u8]) -> Result<Vec<u8>, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    encode_preimage(&unit).map_err(|e| JsError::new(&format!("encode: {e:?}")))
}

// ── The JS Value boundary ────────────────────────────────────────────────────────────────────
// `WValue` is a bridge-local serde mirror of `core::Value` (which has no serde, keeping core
// blake3-only). Adjacently tagged: every value is `{ type, value }` in JS — uniform and
// recursion-friendly. f64 (incl. NaN/Inf) survives because serde-wasm-bindgen maps it to a JS
// number, not to JSON.

/// The JavaScript-facing form of a value: `{ "type": "Number", "value": 3 }`,
/// `{ "type": "List", "value": [ … ] }`, `{ "type": "Effect", "value": { "sink", "payload" } }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum WValue {
    Number(f64),
    Boolean(bool),
    Text(String),
    List(Vec<WValue>),
    Effect { sink: String, payload: Box<WValue> },
}

impl WValue {
    fn from_core(v: &Value) -> WValue {
        match v {
            Value::Number(n) => WValue::Number(*n),
            Value::Boolean(b) => WValue::Boolean(*b),
            Value::Text(s) => WValue::Text(s.clone()),
            Value::List(xs) => WValue::List(xs.iter().map(WValue::from_core).collect()),
            Value::Effect { sink, payload } => WValue::Effect {
                sink: sink.clone(),
                payload: Box::new(WValue::from_core(payload)),
            },
        }
    }

    fn into_core(self) -> Value {
        match self {
            WValue::Number(n) => Value::Number(n),
            WValue::Boolean(b) => Value::Boolean(b),
            WValue::Text(s) => Value::Text(s),
            WValue::List(xs) => Value::List(xs.into_iter().map(WValue::into_core).collect()),
            WValue::Effect { sink, payload } => Value::Effect {
                sink,
                payload: Box::new(payload.into_core()),
            },
        }
    }
}

// ── The evaluator ────────────────────────────────────────────────────────────────────────────
// Stateful on purpose: the editor maintains a workspace of units; the bridge mirrors it as a
// populated resolver. You `insert` units (children of what you'll evaluate), then `eval` a unit
// against that workspace — so a live editor doesn't re-ship the whole closure on every keystroke.

/// A workspace-backed evaluator: insert units, then evaluate against them. Wraps the *same*
/// `MapResolver` and `eval` the native host uses — there is no second evaluator.
#[wasm_bindgen]
pub struct Evaluator {
    resolver: MapResolver,
}

#[wasm_bindgen]
impl Evaluator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Evaluator {
        Evaluator { resolver: MapResolver::new() }
    }

    /// Add a unit (from its canonical preimage) to the workspace; returns its 32-byte hash so JS
    /// can reference it as a child.
    pub fn insert(&mut self, preimage: &[u8]) -> Result<Vec<u8>, JsError> {
        let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
        let hash = self
            .resolver
            .insert(unit)
            .map_err(|e| JsError::new(&format!("insert: {e:?}")))?;
        Ok(hash.to_vec())
    }

    /// Evaluate the unit (from its preimage) on `inputs` against the inserted workspace. `inputs`
    /// is a JS array of `WValue | null` (null = an absent input slot). Returns the output `WValue`,
    /// or `null` if the unit did not evaluate (e.g. an unresolved child or a missing input) —
    /// `null` is a normal "not ready" outcome for a live editor, not an error.
    pub fn eval(&self, preimage: &[u8], inputs: JsValue) -> Result<JsValue, JsError> {
        let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
        let winputs: Vec<Option<WValue>> = serde_wasm_bindgen::from_value(inputs)
            .map_err(|e| JsError::new(&format!("inputs: {e}")))?;
        let core_inputs: Vec<Option<Value>> =
            winputs.into_iter().map(|o| o.map(WValue::into_core)).collect();
        let out = holon_core::eval(&unit, &core_inputs, &self.resolver);
        let wout = out.as_ref().map(WValue::from_core);
        serde_wasm_bindgen::to_value(&wout).map_err(|e| JsError::new(&format!("output: {e}")))
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Evaluator::new()
    }
}

// ── Parity & boundary tests: one body, run on BOTH targets ─────────────────────────────────────

#[cfg(test)]
use holon_core::{Edge, Param, Type, Unit, UnitBody, BOUNDARY};

#[cfg(test)]
fn prim(name: &str) -> Unit {
    Unit { body: UnitBody::Primitive { name: name.into() }, params: vec![], inputs: vec![], output: Some(Type::Number) }
}

#[cfg(test)]
fn konst(v: &str) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("value", v), Param::frozen("type", "Number")],
        inputs: vec![],
        output: Some(Type::Number),
    }
}

#[cfg(test)]
fn check_hash_parity() {
    for u in [prim("add"), prim("subtract"), konst("0"), konst("42")] {
        let bytes = encode_preimage(&u).unwrap();
        assert_eq!(hash_unit_bytes(&bytes).unwrap(), hash_unit(&u).unwrap().to_vec());
        assert_eq!(canonicalize_preimage(&bytes).unwrap(), bytes);
    }
}

#[cfg(test)]
fn check_eval_parity() {
    let num = |n: f64| Value::Number(n);
    let r = MapResolver::new();
    // finite arithmetic — pinned, exact
    assert_eq!(holon_core::eval(&prim("add"), &[Some(num(2.0)), Some(num(3.0))], &r), Some(num(5.0)));
    assert_eq!(holon_core::eval(&prim("multiply"), &[Some(num(4.0)), Some(num(5.0))], &r), Some(num(20.0)));
    // NaN (inf - inf): assert *is_nan*, not bits — the canonical view, and what is target-stable
    match holon_core::eval(&prim("subtract"), &[Some(num(f64::INFINITY)), Some(num(f64::INFINITY))], &r) {
        Some(Value::Number(x)) => assert!(x.is_nan()),
        other => panic!("expected NaN, got {other:?}"),
    }
    // composite through the resolver: const(7) wired to the boundary ⇒ 7
    let mut r2 = MapResolver::new();
    let kh = r2.insert(konst("7")).unwrap();
    let comp = Unit {
        body: UnitBody::Composite {
            children: vec![kh],
            edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![],
        inputs: vec![],
        output: Some(Type::Number),
    };
    assert_eq!(holon_core::eval(&comp, &[], &r2), Some(num(7.0)));
}

#[cfg(test)]
fn check_wvalue_roundtrip() {
    // WValue mirrors Value losslessly (NaN compared by is_nan; everything else by ==).
    let cases = [
        Value::Number(3.5),
        Value::Boolean(true),
        Value::Text("hi".into()),
        Value::List(vec![Value::Number(1.0), Value::Text("x".into())]),
        Value::Effect { sink: "egress".into(), payload: Box::new(Value::Number(2.0)) },
    ];
    for v in cases {
        assert_eq!(WValue::from_core(&v).into_core(), v);
    }
    match WValue::from_core(&Value::Number(f64::NAN)).into_core() {
        Value::Number(n) => assert!(n.is_nan()),
        other => panic!("expected NaN, got {other:?}"),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::*;
    #[test]
    fn hash_parity_native() { check_hash_parity(); }
    #[test]
    fn eval_parity_native() { check_eval_parity(); }
    #[test]
    fn wvalue_roundtrip_native() { check_wvalue_roundtrip(); }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn hash_parity_wasm() { check_hash_parity(); }

    #[wasm_bindgen_test]
    fn eval_parity_wasm() { check_eval_parity(); }

    #[wasm_bindgen_test]
    fn wvalue_roundtrip_wasm() { check_wvalue_roundtrip(); }

    // End-to-end through the real JS boundary: JS inputs → Evaluator::eval → core eval → JS output.
    #[wasm_bindgen_test]
    fn evaluator_evals_through_js_boundary() {
        let ev = Evaluator::new();
        let add_bytes = encode_preimage(&prim("add")).unwrap();
        let inputs = serde_wasm_bindgen::to_value(&vec![
            Some(WValue::Number(2.0)),
            Some(WValue::Number(3.0)),
        ])
        .unwrap();
        let out = ev.eval(&add_bytes, inputs).unwrap();
        let wout: Option<WValue> = serde_wasm_bindgen::from_value(out).unwrap();
        assert_eq!(wout, Some(WValue::Number(5.0)));
    }
}