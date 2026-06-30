//! holon-wasm — the WASM bridge (P4.2).
//!
//! Exposes `holon-core`'s pure operations to JavaScript so the editor runs the **same** core,
//! compiled to wasm32 — one evaluator, one encoder, never a second (I5/I2). Depends on `holon-core`
//! **only** (never `holon-host`): the host's native I/O has no place in a browser, and the
//! dependency edge keeps it out, Cargo-enforced.
//!
//! - **P4.2a** bridged bytes→bytes hashing and proved content-addressing is bit-identical native vs
//!   wasm.
//! - **P4.2b** bridged **evaluation**: a stateful [`Evaluator`] and the JS `Value` boundary
//!   ([`WValue`]). `Value` crosses via `serde-wasm-bindgen` (not JSON — eval can yield `NaN`/`Inf`,
//!   which JSON cannot represent but JS numbers can).
//! - **P4.2c** (this) bridges **construction**: a serde mirror of `Unit` ([`WUnit`]) plus
//!   [`build_preimage`] / [`load_unit`], so the editor builds units *through the one core encoder*
//!   rather than reimplementing canonical encoding in TypeScript (which would be a second encoder,
//!   free to drift). Hashes now cross the boundary as **lowercase hex strings** — usable as JS map
//!   keys and `===`-comparable, which `Uint8Array` is not.

use holon_core::{
    decode_preimage, encode_preimage, genesis_units, hash_unit, parse_hash, Edge, Hash, MapResolver,
    Param, Port, Type, Unit, UnitBody, Value,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// A 32-byte hash as 64 lowercase hex chars — the boundary form of a content address.
fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash a unit from its canonical preimage bytes: decode → re-hash → its content address (hex).
#[wasm_bindgen]
pub fn hash_unit_hex(preimage: &[u8]) -> Result<String, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    let hash = hash_unit(&unit).map_err(|e| JsError::new(&format!("hash: {e:?}")))?;
    Ok(to_hex(&hash))
}

/// Decode then re-encode a unit's preimage — a canonical-form check (output equals input iff the
/// input was already canonical), so a round-trip mismatch flags a malformed encoding early.
#[wasm_bindgen]
pub fn canonicalize_preimage(preimage: &[u8]) -> Result<Vec<u8>, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    encode_preimage(&unit).map_err(|e| JsError::new(&format!("encode: {e:?}")))
}

// ── The JS Value boundary ────────────────────────────────────────────────────────────────────
// `WValue` mirrors `core::Value` (which has no serde, keeping core blake3-only). Adjacently tagged:
// every value is `{ type, value }` in JS. f64 (incl. NaN/Inf) survives because serde-wasm-bindgen
// maps it to a JS number, not to JSON.

/// The JS-facing form of a value: `{ "type": "Number", "value": 3 }`, etc.
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

// ── The JS Unit boundary (construction) ────────────────────────────────────────────────────────
// `WUnit` mirrors `core::Unit` so the editor describes a node as a plain JS object and the bridge
// encodes it with the ONE core encoder. Children cross as hex hashes; types as plain strings.

/// JS-facing wire type — a bare string in JS: `"Number"`, `"Any"`, …
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum WType {
    Number,
    Boolean,
    Text,
    List,
    Effect,
    Any,
}

impl WType {
    fn from_core(t: Type) -> WType {
        match t {
            Type::Number => WType::Number,
            Type::Boolean => WType::Boolean,
            Type::Text => WType::Text,
            Type::List => WType::List,
            Type::Effect => WType::Effect,
            Type::Any => WType::Any,
        }
    }
    fn into_core(self) -> Type {
        match self {
            WType::Number => Type::Number,
            WType::Boolean => Type::Boolean,
            WType::Text => Type::Text,
            WType::List => Type::List,
            WType::Effect => Type::Effect,
            WType::Any => Type::Any,
        }
    }
}

/// JS-facing param: `{ kind: "Frozen", name, value }` or `{ kind: "Open", name, bind }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum WParam {
    Frozen { name: String, value: String },
    Open { name: String, bind: String },
}

impl WParam {
    fn from_core(p: &Param) -> WParam {
        match p {
            Param::Frozen { name, value } => {
                WParam::Frozen { name: name.clone(), value: value.clone() }
            }
            Param::Open { name, bind } => WParam::Open { name: name.clone(), bind: bind.clone() },
        }
    }
    fn into_core(self) -> Param {
        match self {
            WParam::Frozen { name, value } => Param::Frozen { name, value },
            WParam::Open { name, bind } => Param::Open { name, bind },
        }
    }
}

/// JS-facing port: `{ name, ty: "Number" }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WPort {
    name: String,
    ty: WType,
}

/// JS-facing internal wire: child *indices* + port indices. `to_child`/`from_child` == 4294967295
/// (u32::MAX) is the composite's own boundary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WEdge {
    from_child: u32,
    from_port: u32,
    to_child: u32,
    to_port: u32,
}

/// JS-facing unit body: `{ kind: "Primitive", name }` or
/// `{ kind: "Composite", children: [hex…], edges: [...] }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum WBody {
    Primitive { name: String },
    Composite { children: Vec<String>, edges: Vec<WEdge> },
}

/// JS-facing unit. `params` / `inputs` / `output` may be omitted in JS (default empty / null).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WUnit {
    body: WBody,
    #[serde(default)]
    params: Vec<WParam>,
    #[serde(default)]
    inputs: Vec<WPort>,
    #[serde(default)]
    output: Option<WType>,
}

impl WUnit {
    fn from_core(u: &Unit) -> WUnit {
        let body = match &u.body {
            UnitBody::Primitive { name } => WBody::Primitive { name: name.clone() },
            UnitBody::Composite { children, edges } => WBody::Composite {
                children: children.iter().map(to_hex).collect(),
                edges: edges
                    .iter()
                    .map(|e| WEdge {
                        from_child: e.from_child,
                        from_port: e.from_port,
                        to_child: e.to_child,
                        to_port: e.to_port,
                    })
                    .collect(),
            },
        };
        WUnit {
            body,
            params: u.params.iter().map(WParam::from_core).collect(),
            inputs: u
                .inputs
                .iter()
                .map(|p| WPort { name: p.name.clone(), ty: WType::from_core(p.ty) })
                .collect(),
            output: u.output.map(WType::from_core),
        }
    }

    fn into_core(self) -> Result<Unit, JsError> {
        let body = match self.body {
            WBody::Primitive { name } => UnitBody::Primitive { name },
            WBody::Composite { children, edges } => {
                let mut hs = Vec::with_capacity(children.len());
                for c in &children {
                    let h = parse_hash(c)
                        .ok_or_else(|| JsError::new(&format!("bad child hash: {c}")))?;
                    hs.push(h);
                }
                UnitBody::Composite {
                    children: hs,
                    edges: edges
                        .into_iter()
                        .map(|e| Edge {
                            from_child: e.from_child,
                            from_port: e.from_port,
                            to_child: e.to_child,
                            to_port: e.to_port,
                        })
                        .collect(),
                }
            }
        };
        Ok(Unit {
            body,
            params: self.params.into_iter().map(WParam::into_core).collect(),
            inputs: self
                .inputs
                .into_iter()
                .map(|p| Port { name: p.name, ty: p.ty.into_core() })
                .collect(),
            output: self.output.map(WType::into_core),
        })
    }
}

/// Build a unit's canonical preimage from a JS [`WUnit`] — the editor's "compile this node to
/// canonical bytes", through the ONE core encoder. The bytes feed `insert`, `eval`, or `hash`.
#[wasm_bindgen]
pub fn build_preimage(unit: JsValue) -> Result<Vec<u8>, JsError> {
    let wunit: WUnit =
        serde_wasm_bindgen::from_value(unit).map_err(|e| JsError::new(&format!("unit: {e}")))?;
    let core = wunit.into_core()?;
    encode_preimage(&core).map_err(|e| JsError::new(&format!("encode: {e:?}")))
}

/// Inverse of [`build_preimage`]: decode a preimage back to a JS [`WUnit`] — load a unit's structure
/// onto the canvas (and, later, descend into a composite to inspect it — I9).
#[wasm_bindgen]
pub fn load_unit(preimage: &[u8]) -> Result<JsValue, JsError> {
    let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
    serde_wasm_bindgen::to_value(&WUnit::from_core(&unit))
        .map_err(|e| JsError::new(&format!("unit: {e}")))
}

/// The genesis primitive catalog as JS [`WUnit`]s — the authoritative typed interface of every §13a
/// leaf (names, ports, output types, default params), the single source the editor's palette is
/// *derived* from (never a hand-written TS copy that could drift, I1). Returned whole, in one
/// crossing; the editor builds its palette and its name→interface map from this array.
#[wasm_bindgen]
pub fn primitive_catalog() -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(&catalog_wunits())
        .map_err(|e| JsError::new(&format!("catalog: {e}")))
}

/// Core's genesis interfaces mirrored to `WUnit` via the *same* `from_core` `load_unit` uses — no
/// second core→JS converter. Shared by the bridge fn and its tests.
fn catalog_wunits() -> Vec<WUnit> {
    genesis_units().iter().map(WUnit::from_core).collect()
}

/// Wire-time type compatibility (§5), straight from core's rule — the editor's `isValidConnection`
/// asks THIS, never a TS reimplementation, so the canvas and `validate` can't disagree (I1). `src` is
/// the upstream output type, `dst` the downstream input type, each a bare type string ("Number").
#[wasm_bindgen]
pub fn compatible(src: JsValue, dst: JsValue) -> Result<bool, JsError> {
    let s: WType =
        serde_wasm_bindgen::from_value(src).map_err(|e| JsError::new(&format!("src: {e}")))?;
    let d: WType =
        serde_wasm_bindgen::from_value(dst).map_err(|e| JsError::new(&format!("dst: {e}")))?;
    Ok(types_compatible(s, d))
}

/// The rule itself, on `WType` (shared by the bridge fn and its tests): map to core `Type` via the
/// one `into_core` and delegate to `holon_core::compatible`. Fully-qualified so it does not collide
/// with the exported `compatible` above.
fn types_compatible(s: WType, d: WType) -> bool {
    holon_core::compatible(s.into_core(), d.into_core())
}

// ── The evaluator ────────────────────────────────────────────────────────────────────────────
// Stateful on purpose: the editor maintains a workspace of units; the bridge mirrors it as a
// populated resolver. Insert units (children), then eval a unit against them — a live editor needn't
// re-ship the whole closure on every keystroke.

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

    /// Add a unit (from its canonical preimage) to the workspace; returns its hash (hex) so JS can
    /// reference it as a child in a composite.
    pub fn insert(&mut self, preimage: &[u8]) -> Result<String, JsError> {
        let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
        let hash =
            self.resolver.insert(unit).map_err(|e| JsError::new(&format!("insert: {e:?}")))?;
        Ok(to_hex(&hash))
    }

    /// Evaluate the unit (from its preimage) on `inputs` against the inserted workspace. `inputs` is
    /// a JS array of `WValue | null` (null = an absent input slot). Returns the output `WValue`, or
    /// `null` if the unit did not evaluate — a normal "not ready" outcome for a live editor.
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
use holon_core::BOUNDARY;

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
        assert_eq!(hash_unit_hex(&bytes).unwrap(), to_hex(&hash_unit(&u).unwrap()));
        assert_eq!(canonicalize_preimage(&bytes).unwrap(), bytes);
    }
}

#[cfg(test)]
fn check_eval_parity() {
    let num = |n: f64| Value::Number(n);
    let r = MapResolver::new();
    assert_eq!(holon_core::eval(&prim("add"), &[Some(num(2.0)), Some(num(3.0))], &r), Some(num(5.0)));
    assert_eq!(holon_core::eval(&prim("multiply"), &[Some(num(4.0)), Some(num(5.0))], &r), Some(num(20.0)));
    // NaN (inf - inf): assert *is_nan*, not bits — the canonical view, and what is target-stable.
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

#[cfg(test)]
fn check_wunit_roundtrip() {
    // A composite with a real child hash exercises the hex round-trip; an open param + interface
    // exercise the rest. WUnit must mirror Unit losslessly — same value AND same canonical bytes.
    let mut r = MapResolver::new();
    let kh = r.insert(konst("7")).unwrap();
    let comp = Unit {
        body: UnitBody::Composite {
            children: vec![kh],
            edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![Param::open("x", "x0")],
        inputs: vec![Port { name: "in".into(), ty: Type::Any }],
        output: Some(Type::Number),
    };
    for u in [prim("add"), konst("7"), comp] {
        let back = WUnit::from_core(&u).into_core().expect("into_core");
        assert_eq!(back, u);
        assert_eq!(encode_preimage(&back).unwrap(), encode_preimage(&u).unwrap());
    }
}

#[cfg(test)]
fn check_primitive_catalog() {
    let cat = catalog_wunits();
    // The §13a count crosses the mirror intact.
    assert_eq!(cat.len(), 34);
    // A known interface survives core→WUnit: add = (a:Number, b:Number) -> Number.
    let add = cat
        .iter()
        .find(|w| matches!(&w.body, WBody::Primitive { name } if name == "add"))
        .expect("add in catalog");
    assert_eq!(add.inputs.len(), 2);
    assert!(add.inputs.iter().all(|p| p.ty == WType::Number));
    assert_eq!(add.output, Some(WType::Number));
    // A higher-order leaf keeps its function param: map carries an open `fn`.
    let map = cat
        .iter()
        .find(|w| matches!(&w.body, WBody::Primitive { name } if name == "map"))
        .expect("map in catalog");
    assert!(map.params.iter().any(|p| matches!(p, WParam::Open { name, .. } if name == "fn")));
}

#[cfg(test)]
fn check_compatible() {
    // Core's §5 rule, reached through the bridge helper: equal types or either `Any`.
    assert!(types_compatible(WType::Number, WType::Number));
    assert!(types_compatible(WType::Any, WType::Number));
    assert!(types_compatible(WType::Number, WType::Any));
    assert!(!types_compatible(WType::Number, WType::Text));
    assert!(!types_compatible(WType::Effect, WType::List));
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
    #[test]
    fn wunit_roundtrip_native() { check_wunit_roundtrip(); }
    #[test]
    fn primitive_catalog_native() { check_primitive_catalog(); }
    #[test]
    fn compatible_native() { check_compatible(); }
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
    #[wasm_bindgen_test]
    fn wunit_roundtrip_wasm() { check_wunit_roundtrip(); }
    #[wasm_bindgen_test]
    fn primitive_catalog_wasm() { check_primitive_catalog(); }
    #[wasm_bindgen_test]
    fn compatible_wasm() { check_compatible(); }

    // Eval through the JS boundary: JS inputs → Evaluator::eval → core eval → JS output.
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

    // The FULL editor path: build a const(7) and a composite from plain JS objects (through the one
    // core encoder), insert, wire to the boundary, eval ⇒ 7. This is exactly what P4.1 will do.
    #[wasm_bindgen_test]
    fn construction_through_js_boundary() {
        let const_w = WUnit {
            body: WBody::Primitive { name: "const".into() },
            params: vec![
                WParam::Frozen { name: "value".into(), value: "7".into() },
                WParam::Frozen { name: "type".into(), value: "Number".into() },
            ],
            inputs: vec![],
            output: Some(WType::Number),
        };
        let const_pre = build_preimage(serde_wasm_bindgen::to_value(&const_w).unwrap()).unwrap();

        let mut ev = Evaluator::new();
        let child_hex = ev.insert(&const_pre).unwrap();

        let comp_w = WUnit {
            body: WBody::Composite {
                children: vec![child_hex],
                edges: vec![WEdge { from_child: 0, from_port: 0, to_child: u32::MAX, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            output: Some(WType::Number),
        };
        let comp_pre = build_preimage(serde_wasm_bindgen::to_value(&comp_w).unwrap()).unwrap();

        let no_inputs = serde_wasm_bindgen::to_value(&Vec::<Option<WValue>>::new()).unwrap();
        let out = ev.eval(&comp_pre, no_inputs).unwrap();
        let wout: Option<WValue> = serde_wasm_bindgen::from_value(out).unwrap();
        assert_eq!(wout, Some(WValue::Number(7.0)));
    }

    // The catalog crosses the JS boundary as an array of WUnit, intact — the new risk the
    // single-WUnit load_unit tests didn't cover.
    #[wasm_bindgen_test]
    fn primitive_catalog_crosses_js_boundary() {
        let js = primitive_catalog().unwrap();
        let cat: Vec<WUnit> = serde_wasm_bindgen::from_value(js).unwrap();
        assert_eq!(cat.len(), 34);
        assert!(cat
            .iter()
            .any(|w| matches!(&w.body, WBody::Primitive { name } if name == "add")));
    }

    // The compatibility rule crosses as (type string, type string) → bool: the editor's wire-time
    // check calls exactly this.
    #[wasm_bindgen_test]
    fn compatible_crosses_js_boundary() {
        let s = |t: &str| serde_wasm_bindgen::to_value(t).unwrap();
        assert!(compatible(s("Number"), s("Number")).unwrap());
        assert!(compatible(s("Number"), s("Any")).unwrap());
        assert!(!compatible(s("Number"), s("Text")).unwrap());
    }
}