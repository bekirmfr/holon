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
    decode_preimage, default_spec, derive_and_freeze, encode_preimage, genesis_units, hash_unit,
    parse_hash, Capability, Edge, EncapError, EncapSpec, Expose, Hash, Kind, MapResolver, Param,
    Port, Promote, Resolver, Type, Unit, UnitBody, Value, ValidationError, MAX_TYPE_DEPTH,
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
    /// Mirrors `core::Value::Sum` losslessly — the *value* bridge was already a faithful
    /// one-to-one mirror when `Sum` first crossed it (P1.3.2 inch 3, before `WType` itself
    /// became recursive), so a tagged choice crosses intact:
    /// `{ "type": "Sum", "value": { "arm": 0, "payload": {...} } }`.
    Sum { arm: usize, payload: Box<WValue> },
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
            Value::Sum { arm, payload } => {
                WValue::Sum { arm: *arm, payload: Box::new(WValue::from_core(payload)) }
            }
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
            WValue::Sum { arm, payload } => {
                Value::Sum { arm, payload: Box::new(payload.into_core()) }
            }
        }
    }
}

// ── The JS Unit boundary (construction) ────────────────────────────────────────────────────────
// `WUnit` mirrors `core::Unit` so the editor describes a node as a plain JS object and the bridge
// encodes it with the ONE core encoder. Children cross as hex hashes; types as plain strings.

/// JS-facing wire type. Mirrors `core::Type` exactly, including its recursive shape
/// (P1.3.2 editor inch). No `#[serde(tag=...)]` needed: serde's own default (external
/// tagging) already gives the right shape for free — a nullary variant serializes as a
/// bare string (`"Number"`), a variant carrying data serializes as a single-key object
/// (`{"List": "Number"}`, `{"Product": ["Number","Text"]}`). Every existing scalar-only
/// TS call site (`p.ty === "Number"`, `Record<WType,string>` keyed by the flat cases,
/// …) is untouched by this — only `List`/`Product`/`Sum` change shape, from a bare tag
/// to a tagged object, because they are the only variants that now carry a payload.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum WType {
    Number,
    Boolean,
    Text,
    /// A homogeneous list of `T`. `{"List": "Number"}`.
    List(Box<WType>),
    /// A positional n-ary tuple. `Pair<A,B>` = `{"Product": [A, B]}`; `Unit` = `{"Product": []}`.
    Product(Vec<WType>),
    /// A tagged choice. `Option<T>` = `{"Sum": [T, {"Product": []}]}`.
    Sum(Vec<WType>),
    Effect,
    Any,
}

impl WType {
    fn from_core(t: &Type) -> WType {
        match t {
            Type::Number => WType::Number,
            Type::Boolean => WType::Boolean,
            Type::Text => WType::Text,
            Type::List(inner) => WType::List(Box::new(WType::from_core(inner))),
            Type::Product(elems) => WType::Product(elems.iter().map(WType::from_core).collect()),
            Type::Sum(elems) => WType::Sum(elems.iter().map(WType::from_core).collect()),
            Type::Effect => WType::Effect,
            Type::Any => WType::Any,
        }
    }

    /// The public entry point: depth 0.
    fn into_core(self) -> Result<Type, JsError> {
        self.into_core_at(0)
    }

    /// Depth-bounded, mirroring core's own `Decoder::ty_at` and for the identical reason: this is
    /// now a genuinely untrusted-input boundary. `compatible`/`build_preimage` are public
    /// `#[wasm_bindgen]` functions reachable by any JS on the page (not only this editor's own
    /// React code), and `WType` can now nest — so a JS caller can hand this a deeply (or
    /// adversarially) nested object the same way a hostile byte blob could target the decoder.
    /// Reuses `MAX_TYPE_DEPTH` rather than a parallel bound that could drift from core's.
    fn into_core_at(self, depth: u32) -> Result<Type, JsError> {
        if depth > MAX_TYPE_DEPTH {
            return Err(JsError::new("type nested too deep"));
        }
        Ok(match self {
            WType::Number => Type::Number,
            WType::Boolean => Type::Boolean,
            WType::Text => Type::Text,
            WType::List(inner) => Type::List(Box::new(inner.into_core_at(depth + 1)?)),
            WType::Product(elems) => Type::Product(
                elems
                    .into_iter()
                    .map(|e| e.into_core_at(depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            WType::Sum(elems) => Type::Sum(
                elems
                    .into_iter()
                    .map(|e| e.into_core_at(depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            WType::Effect => Type::Effect,
            WType::Any => Type::Any,
        })
    }
}

/// A category of effect a leaf may touch, outside its inputs — mirrors `core::Capability`.
/// Every variant crosses as a bare string (`"Clock"`) via serde's default external tagging,
/// except `StateRead`/`StateWrite`, which now carry their state key and cross as a
/// single-key object (`{"StateRead": "var1"}`) — the identical convention `WType`'s own
/// nullary-vs-payload variants already use (see that type's own doc). No longer `Copy` (a
/// `String` payload can't be).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum WCapability {
    Clock,
    NetworkIngress,
    FileRead,
    SecretAccess,
    Random,
    StateRead(String),
    StateWrite(String),
    NetworkEgress,
    FileWrite,
    Spawn,
}

impl WCapability {
    fn from_core(c: Capability) -> WCapability {
        match c {
            Capability::Clock => WCapability::Clock,
            Capability::NetworkIngress => WCapability::NetworkIngress,
            Capability::FileRead => WCapability::FileRead,
            Capability::SecretAccess => WCapability::SecretAccess,
            Capability::Random => WCapability::Random,
            Capability::StateRead(key) => WCapability::StateRead(key),
            Capability::StateWrite(key) => WCapability::StateWrite(key),
            Capability::NetworkEgress => WCapability::NetworkEgress,
            Capability::FileWrite => WCapability::FileWrite,
            Capability::Spawn => WCapability::Spawn,
        }
    }
}

/// The coarse effect bucket a unit belongs to — mirrors `core::Kind`, the fixed projection of
/// `WCapability` above. Bare string, same convention.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WKind {
    Pure,
    Driver,
    State,
    Sink,
}

impl WKind {
    fn from_core(k: Kind) -> WKind {
        match k {
            Kind::Pure => WKind::Pure,
            Kind::Driver => WKind::Driver,
            Kind::State => WKind::State,
            Kind::Sink => WKind::Sink,
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

/// JS-facing unit. `params` / `inputs` / `outputs` may be omitted in JS (default empty).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WUnit {
    body: WBody,
    #[serde(default)]
    params: Vec<WParam>,
    #[serde(default)]
    inputs: Vec<WPort>,
    #[serde(default)]
    outputs: Vec<WPort>,
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
                .map(|p| WPort { name: p.name.clone(), ty: WType::from_core(&p.ty) })
                .collect(),
            outputs: u
                .outputs
                .iter()
                .map(|p| WPort { name: p.name.clone(), ty: WType::from_core(&p.ty) })
                .collect(),
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
                .map(|p| Ok(Port { name: p.name, ty: p.ty.into_core()? }))
                .collect::<Result<Vec<_>, JsError>>()?,
            outputs: self
                .outputs
                .into_iter()
                .map(|p| Ok(Port { name: p.name, ty: p.ty.into_core()? }))
                .collect::<Result<Vec<_>, JsError>>()?,
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

/// The genesis primitive names the editor may grow a *placed* node's port count past the
/// catalog's declared default (e.g. `add` beyond its usual 2 inputs) — core's 4th catalog fact
/// (`catalog::VARIADIC`), mirrored here so the editor derives this from core instead of
/// hand-listing names in TypeScript that could silently drift from what `compute` actually does.
/// A bare `string[]`, not `WUnit`-shaped data — this is a classification, not an interface.
#[wasm_bindgen]
pub fn variadic_primitives() -> Vec<String> {
    holon_core::VARIADIC.iter().map(|s| s.to_string()).collect()
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
    types_compatible(s, d)
}

/// The rule itself, on `WType` (shared by the bridge fn and its tests): map to core `Type` via the
/// one `into_core` and delegate to `holon_core::compatible`. Fully-qualified so it does not collide
/// with the exported `compatible` above. Fallible now that `into_core` is depth-bounded.
fn types_compatible(s: WType, d: WType) -> Result<bool, JsError> {
    Ok(holon_core::compatible(&s.into_core()?, &d.into_core()?))
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

/// A structural validation failure — mirrors `core::ValidationError`. Every variant carries
/// a `message` (core's own `Display` impl, `.to_string()`'d here — one decider for the
/// wording, not a second copy hand-maintained in the bridge) alongside the structured
/// fields the editor needs to highlight the actual node(s) at fault: `child`/`edge` indices
/// the editor already holds locally (it built the `children`/`edges` arrays it sent to
/// `project`), so it resolves them back to a specific placed node without core needing to
/// know anything about the canvas. `src`/`dst` cross as real `WType`s (not just text) so the
/// editor can render them with its own `typeLabel`, the same as everywhere else.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind")]
pub enum WValidationError {
    ChildOutOfRange { edge: usize, child: u32, message: String },
    UnresolvedChild { child: u32, message: String },
    OutputPortOutOfRange { edge: usize, child: u32, port: u32, message: String },
    InputPortOutOfRange { edge: usize, child: u32, port: u32, message: String },
    BoundaryInputOutOfRange { edge: usize, port: u32, message: String },
    BoundaryOutputOutOfRange { edge: usize, port: u32, message: String },
    DuplicateInputEdge { to_child: u32, to_port: u32, message: String },
    Cycle { message: String },
    TypeMismatch { edge: usize, src: WType, dst: WType, message: String },
}

impl WValidationError {
    fn from_core(e: &ValidationError) -> WValidationError {
        let message = e.to_string();
        match e {
            ValidationError::ChildOutOfRange { edge, child } => {
                WValidationError::ChildOutOfRange { edge: *edge, child: *child, message }
            }
            ValidationError::UnresolvedChild { child } => {
                WValidationError::UnresolvedChild { child: *child, message }
            }
            ValidationError::OutputPortOutOfRange { edge, child, port } => {
                WValidationError::OutputPortOutOfRange { edge: *edge, child: *child, port: *port, message }
            }
            ValidationError::InputPortOutOfRange { edge, child, port } => {
                WValidationError::InputPortOutOfRange { edge: *edge, child: *child, port: *port, message }
            }
            ValidationError::BoundaryInputOutOfRange { edge, port } => {
                WValidationError::BoundaryInputOutOfRange { edge: *edge, port: *port, message }
            }
            ValidationError::BoundaryOutputOutOfRange { edge, port } => {
                WValidationError::BoundaryOutputOutOfRange { edge: *edge, port: *port, message }
            }
            ValidationError::DuplicateInputEdge { to_child, to_port } => {
                WValidationError::DuplicateInputEdge { to_child: *to_child, to_port: *to_port, message }
            }
            ValidationError::Cycle => WValidationError::Cycle { message },
            ValidationError::TypeMismatch { edge, src, dst } => WValidationError::TypeMismatch {
                edge: *edge,
                src: WType::from_core(src),
                dst: WType::from_core(dst),
                message,
            },
        }
    }
}

/// A promotion choice crossing to JS. Mirrors `core::Promote`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WPromote {
    child: u32,
    port: u32,
    name: String,
}

impl WPromote {
    fn from_core(p: &Promote) -> WPromote {
        WPromote { child: p.child, port: p.port, name: p.name.clone() }
    }
    fn into_core(self) -> Promote {
        Promote { child: self.child, port: self.port, name: self.name }
    }
}

/// An exposure choice crossing to JS. Mirrors `core::Expose`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WExpose {
    child: u32,
    port: u32,
    name: String,
}

impl WExpose {
    fn from_core(e: &Expose) -> WExpose {
        WExpose { child: e.child, port: e.port, name: e.name.clone() }
    }
    fn into_core(self) -> Expose {
        Expose { child: self.child, port: self.port, name: self.name }
    }
}

/// The canonical encapsulation proposal, crossing to JS. Mirrors `core::EncapSpec` (P4.3,
/// fork 3). The editor may accept `expose` as-is — `project()` below does exactly that, in one
/// call, unchanged — or edit it first: excluding some of the automatically-proposed free
/// outputs from the composite's final interface, via `propose_encapsulation` +
/// `freeze_encapsulation` below. `promote` crosses too but isn't editable yet on the editor
/// side — no UI exists to rename or reorder promoted inputs either, so giving outputs more
/// editing power than inputs currently have would be asymmetric for no reason.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WEncapSpec {
    promote: Vec<WPromote>,
    expose: Vec<WExpose>,
}

impl WEncapSpec {
    fn from_core(s: &EncapSpec) -> WEncapSpec {
        WEncapSpec {
            promote: s.promote.iter().map(WPromote::from_core).collect(),
            expose: s.expose.iter().map(WExpose::from_core).collect(),
        }
    }
    fn into_core(self) -> EncapSpec {
        EncapSpec {
            promote: self.promote.into_iter().map(WPromote::into_core).collect(),
            expose: self.expose.into_iter().map(WExpose::into_core).collect(),
        }
    }
}

/// An encapsulation failure — mirrors `core::EncapError`. Same shape and reasoning as
/// `WValidationError`: structured fields for highlighting, plus core's own `message`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind")]
pub enum WEncapError {
    UnresolvedChild { child: u32, message: String },
    PromoteChildOutOfRange { child: u32, message: String },
    PromotePortOutOfRange { child: u32, port: u32, message: String },
    PromoteAlreadyBound { child: u32, port: u32, message: String },
    DuplicatePromotion { child: u32, port: u32, message: String },
    ExposeChildOutOfRange { child: u32, message: String },
    ExposePortInvalid { child: u32, port: u32, message: String },
    Invalid { errors: Vec<WValidationError>, message: String },
}

impl WEncapError {
    fn from_core(e: &EncapError) -> WEncapError {
        let message = e.to_string();
        match e {
            EncapError::UnresolvedChild(c) => WEncapError::UnresolvedChild { child: *c, message },
            EncapError::PromoteChildOutOfRange(c) => {
                WEncapError::PromoteChildOutOfRange { child: *c, message }
            }
            EncapError::PromotePortOutOfRange { child, port } => {
                WEncapError::PromotePortOutOfRange { child: *child, port: *port, message }
            }
            EncapError::PromoteAlreadyBound { child, port } => {
                WEncapError::PromoteAlreadyBound { child: *child, port: *port, message }
            }
            EncapError::DuplicatePromotion { child, port } => {
                WEncapError::DuplicatePromotion { child: *child, port: *port, message }
            }
            EncapError::ExposeChildOutOfRange(c) => {
                WEncapError::ExposeChildOutOfRange { child: *c, message }
            }
            EncapError::ExposePortInvalid { child, port } => {
                WEncapError::ExposePortInvalid { child: *child, port: *port, message }
            }
            EncapError::Invalid(errs) => WEncapError::Invalid {
                errors: errs.iter().map(WValidationError::from_core).collect(),
                message,
            },
        }
    }
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

    /// Evaluate the unit (from its preimage) on `inputs` against the inserted workspace. `inputs`
    /// is a JS array of `WValue | null` (null = an absent input slot). Returns an ARRAY of
    /// `WValue | null` — one slot per the unit's declared output, in order, each independently
    /// present or absent — not a single value. A single-output unit still returns a one-element
    /// array; read `[0]`.
    ///
    /// Widened from a single value to the full array (P4.3, fork 2): `default_spec` now proposes
    /// every free output automatically, so a person who leaves two independent primitives
    /// unconsumed on the canvas gets a genuine two-output composite from an ordinary `project()`
    /// call — an everyday case, not an exotic one. This crosses core's real `eval` result now,
    /// not `eval_one`'s narrowed single value: narrowing here would silently show "not ready"
    /// (null) for a completely valid, common composite, which is worse than the missing feature
    /// it used to be a placeholder for.
    pub fn eval(&self, preimage: &[u8], inputs: JsValue) -> Result<JsValue, JsError> {
        let unit = decode_preimage(preimage).map_err(|e| JsError::new(&format!("decode: {e:?}")))?;
        let winputs: Vec<Option<WValue>> = serde_wasm_bindgen::from_value(inputs)
            .map_err(|e| JsError::new(&format!("inputs: {e}")))?;
        let core_inputs: Vec<Option<Value>> =
            winputs.into_iter().map(|o| o.map(WValue::into_core)).collect();
        let out: Vec<Option<WValue>> = holon_core::eval(&unit, &core_inputs, &self.resolver)
            .iter()
            .map(|o| o.as_ref().map(WValue::from_core))
            .collect();
        serde_wasm_bindgen::to_value(&out).map_err(|e| JsError::new(&format!("output: {e}")))
    }

    /// Look up an already-resolved unit by hash, straight from this Evaluator's own resolver —
    /// no preimage round-trip (P4.3.1, expand-to-inspect's actual primitive). A grouped
    /// composite's preimage bytes are computed once inside the editor's own grouping step and
    /// then discarded — only the hash survives, on the placed node itself — so `load_unit`
    /// can't do this job: it decodes bytes handed in from *outside* this session (a file, the
    /// registry), not something already sitting in this resolver's own memory since it was
    /// inserted. `MapResolver::resolve` already does exactly the lookup needed; this is just
    /// that, crossed to JS. Fails as a plain string, not a structured error — there's no
    /// EncapError/ValidationError family a "not found" belongs to — for either a malformed hex
    /// hash or a hash never inserted in this session (a stale reference, or one from a
    /// different `Evaluator` instance).
    pub fn get_unit(&self, hash: &str) -> Result<JsValue, JsError> {
        let unit = self.resolve_hash(hash)?;
        serde_wasm_bindgen::to_value(&WUnit::from_core(unit))
            .map_err(|e| JsError::new(&format!("unit: {e}")))
    }

    /// The capability manifest of a unit's whole closure, by hash — core's `capabilities`,
    /// crossed as an array of `WCapability` (P4.4.1). `capabilities` already recurses
    /// internally over whatever unit it's given, so calling this once per hash encountered
    /// while walking a tree in the editor already gives that subtree's own manifest at that
    /// level — no separate "per-level" mode needed here, and no new recursive bridge
    /// capability beyond `get_unit` (already built) for descending the tree itself.
    pub fn capabilities(&self, hash: &str) -> Result<JsValue, JsError> {
        let unit = self.resolve_hash(hash)?;
        let caps: Vec<WCapability> = holon_core::capabilities(unit, &self.resolver)
            .iter()
            .map(WCapability::from_core)
            .collect();
        serde_wasm_bindgen::to_value(&caps).map_err(|e| JsError::new(&format!("capabilities: {e}")))
    }

    /// The coarse kind-set of a unit's whole closure, by hash — core's `effects_of`, the fixed
    /// projection of `capabilities` above. Exposed as its own method rather than left for the
    /// editor to derive from `capabilities`, so `Capability::kind()`'s projection table stays
    /// core's own, never a second, TypeScript-side copy that could drift from it.
    pub fn effects_of(&self, hash: &str) -> Result<JsValue, JsError> {
        let unit = self.resolve_hash(hash)?;
        let kinds: Vec<WKind> = holon_core::effects_of(unit, &self.resolver)
            .iter()
            .map(WKind::from_core)
            .collect();
        serde_wasm_bindgen::to_value(&kinds).map_err(|e| JsError::new(&format!("effects: {e}")))
    }

    /// Parse a hex hash and resolve it against this Evaluator's own resolver — the identical
    /// "parse then look up" pattern `get_unit`, `capabilities`, and `effects_of` all need.
    /// Factored out once three call sites needed it, not before.
    fn resolve_hash(&self, hash: &str) -> Result<&Unit, JsError> {
        let h = parse_hash(hash).ok_or_else(|| JsError::new(&format!("bad hash: {hash}")))?;
        self.resolver.resolve(&h).ok_or_else(|| JsError::new(&format!("unresolved: {hash}")))
    }

    /// Project a placed graph into a composite (the editor's "compute this graph's unit + hash").
    /// `children` = the inserted nodes' hex hashes; `edges` = inner child→child `WEdge[]`. Core
    /// derives the boundary (`default_spec`) and freezes (`derive_and_freeze`) — never reimplemented
    /// in TS (one decider). Returns the composite `WUnit`; the editor hashes it through the existing
    /// `build_preimage` / `hash_unit_hex` path and later evaluates it. Fails closed: an ambiguous
    /// output, or any validation error (cycle, duplicate source, type mismatch), throws — as a
    /// structured `WEncapError`, not a flattened string, so the editor can highlight the actual
    /// offending node(s) instead of just displaying text. Malformed *input* (a bad hex hash, edges
    /// that don't even deserialize) is a different failure mode — the editor's own marshalling
    /// should never trigger it — and stays a plain string, not part of this structured family.
    ///
    /// Unchanged by P4.3's fork 3, on purpose: this is still `default_spec` then
    /// `derive_and_freeze`, combined, in one call, with zero editing in between — the path for
    /// anyone who never touches the new seam below. `propose_encapsulation` +
    /// `freeze_encapsulation` are the *same two core calls*, just split apart so a person can
    /// look at the proposal and edit `expose` before freezing — not a second implementation.
    pub fn project(&self, children: Vec<String>, edges: JsValue) -> Result<JsValue, JsValue> {
        let (hashes, core_edges) = Self::parse_children_and_edges(&children, edges)?;
        // The structured path: build a WEncapError core's own Display already put into words
        // (`e.to_string()`, inside from_core) — not a second, hand-maintained message here. Falls
        // back to a plain string only if serializing the structured error itself somehow fails.
        let composite = self.project_core(hashes, core_edges).map_err(|e| {
            serde_wasm_bindgen::to_value(&WEncapError::from_core(&e))
                .unwrap_or_else(|_| JsValue::from_str(&format!("project: {e}")))
        })?;
        serde_wasm_bindgen::to_value(&WUnit::from_core(&composite))
            .map_err(|e| JsValue::from_str(&format!("unit: {e}")))
    }

    /// Propose the canonical encapsulation spec for a selection — core's `default_spec`, crossed
    /// intact (P4.3, fork 3). Pair with `freeze_encapsulation` below: propose, let a person edit
    /// `expose` (excluding some of the automatically-proposed free outputs), freeze the result.
    /// Same structured-error behavior as `project` — `default_spec`'s only error is an
    /// unresolvable child, everything else about *this* call is unfailable by construction.
    pub fn propose_encapsulation(&self, children: Vec<String>, edges: JsValue) -> Result<JsValue, JsValue> {
        let (hashes, core_edges) = Self::parse_children_and_edges(&children, edges)?;
        let spec = default_spec(&hashes, &core_edges, &self.resolver).map_err(|e| {
            serde_wasm_bindgen::to_value(&WEncapError::from_core(&e))
                .unwrap_or_else(|_| JsValue::from_str(&format!("propose: {e}")))
        })?;
        serde_wasm_bindgen::to_value(&WEncapSpec::from_core(&spec))
            .map_err(|e| JsValue::from_str(&format!("spec: {e}")))
    }

    /// Freeze a (possibly edited) encapsulation spec into a composite — core's
    /// `derive_and_freeze` directly, no `default_spec` call inside (P4.3, fork 3). The
    /// `children`/`edges` here must be the exact same values passed to
    /// `propose_encapsulation` above — `spec`'s child/port indices are only meaningful against
    /// that same selection. Same structured-error and hashing behavior as `project`, which is
    /// really just `propose_encapsulation` then this, glued together with zero editing allowed
    /// in between.
    pub fn freeze_encapsulation(
        &self,
        children: Vec<String>,
        edges: JsValue,
        spec: JsValue,
    ) -> Result<JsValue, JsValue> {
        let (hashes, core_edges) = Self::parse_children_and_edges(&children, edges)?;
        let wspec: WEncapSpec = serde_wasm_bindgen::from_value(spec)
            .map_err(|e| JsValue::from_str(&format!("spec: {e}")))?;
        let composite = derive_and_freeze(hashes, core_edges, &wspec.into_core(), &self.resolver)
            .map_err(|e| {
                serde_wasm_bindgen::to_value(&WEncapError::from_core(&e))
                    .unwrap_or_else(|_| JsValue::from_str(&format!("freeze: {e}")))
            })?;
        serde_wasm_bindgen::to_value(&WUnit::from_core(&composite))
            .map_err(|e| JsValue::from_str(&format!("unit: {e}")))
    }

    /// The JS-free heart of [`Evaluator::project`], so it runs on both targets: derive the canonical
    /// boundary then freeze, both core's. `children` are inserted child hashes; `edges` are inner
    /// child→child.
    fn project_core(&self, children: Vec<Hash>, edges: Vec<Edge>) -> Result<Unit, EncapError> {
        let spec = default_spec(&children, &edges, &self.resolver)?;
        derive_and_freeze(children, edges, &spec, &self.resolver)
    }

    /// Parse hex child hashes + a JS `WEdge[]` into their core forms — the exact marshalling
    /// `project`, `propose_encapsulation`, and `freeze_encapsulation` all need identically.
    /// Factored out once three call sites needed it, not before.
    fn parse_children_and_edges(
        children: &[String],
        edges: JsValue,
    ) -> Result<(Vec<Hash>, Vec<Edge>), JsValue> {
        let hashes: Vec<Hash> = children
            .iter()
            .map(|c| parse_hash(c).ok_or_else(|| JsValue::from_str(&format!("bad child hash: {c}"))))
            .collect::<Result<_, JsValue>>()?;
        let wedges: Vec<WEdge> = serde_wasm_bindgen::from_value(edges)
            .map_err(|e| JsValue::from_str(&format!("edges: {e}")))?;
        let core_edges: Vec<Edge> = wedges
            .into_iter()
            .map(|e| Edge {
                from_child: e.from_child,
                from_port: e.from_port,
                to_child: e.to_child,
                to_port: e.to_port,
            })
            .collect();
        Ok((hashes, core_edges))
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
    Unit {
        body: UnitBody::Primitive { name: name.into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

#[cfg(test)]
fn konst(v: &str) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("value", v), Param::frozen("type", "Number")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

#[cfg(test)]
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

#[cfg(test)]
fn check_project() {
    // Insert a binary `add`, project the one-node graph (no inner edges): default_spec promotes
    // both inputs (a, b), derive_and_freeze exposes add's output — the composite is valid and runs.
    let mut ev = Evaluator::new();
    let add_pre = encode_preimage(&binop("add")).unwrap();
    let hex = ev.insert(&add_pre).unwrap();
    let h = parse_hash(&hex).unwrap();

    let composite = ev.project_core(vec![h], vec![]).unwrap();
    assert!(matches!(composite.body, UnitBody::Composite { .. }));
    assert_eq!(composite.inputs.len(), 2); // a, b promoted to the boundary
    assert_eq!(composite.outputs[0].ty, Type::Number);
    assert!(holon_core::validate(&composite, &ev.resolver).is_ok());
    assert_eq!(
        holon_core::eval(
            &composite,
            &[Some(Value::Number(2.0)), Some(Value::Number(3.0))],
            &ev.resolver,
        ).into_vec(),
        vec![Some(Value::Number(5.0))],
    );

    // Two independent adds ⇒ both free outputs are now exposed automatically — a valid
    // two-output composite, not an error. (Before this inch, this was the exact case
    // that triggered AmbiguousDefaultOutput; that variant is gone now.)
    let two = ev.project_core(vec![h, h], vec![]).unwrap();
    assert_eq!(two.outputs.len(), 2);
    assert_eq!(
        holon_core::eval(
            &two,
            &[
                Some(Value::Number(2.0)),
                Some(Value::Number(3.0)),
                Some(Value::Number(10.0)),
                Some(Value::Number(20.0)),
            ],
            &ev.resolver,
        ).into_vec(),
        vec![Some(Value::Number(5.0)), Some(Value::Number(30.0))],
    );
}

#[cfg(test)]
fn check_encap_error_structure() {
    // WEncapError::from_core must carry the SAME structured data core's own EncapError does, not
    // just a stringified message — proven directly, no JsValue crossing needed here (the actual
    // serde_wasm_bindgen serialization is the same already-proven mechanism WValue/WType/WUnit use
    // everywhere else in this file).
    //
    // Scenario: a child hash that was never inserted ⇒ UnresolvedChild. (Previously this test used
    // "two independent adds", which triggered AmbiguousDefaultOutput — that variant no longer
    // exists: default_spec now exposes every free output automatically instead of erroring on
    // more than one. UnresolvedChild is still a live, simple error to trigger structurally.)
    let ev = Evaluator::new();
    let never_inserted: Hash = [7u8; 32];

    let err = ev.project_core(vec![never_inserted], vec![]).unwrap_err();
    match WEncapError::from_core(&err) {
        WEncapError::UnresolvedChild { child, message } => {
            assert_eq!(child, 0);
            assert!(!message.is_empty()); // core's own Display, not silently blank
        }
        other => panic!("expected UnresolvedChild, got {other:?}"),
    }
}

#[cfg(test)]
fn check_validation_error_structure() {
    // The nested case: EncapError::Invalid wraps ValidationError — proving TypeMismatch's src/dst
    // cross as real WTypes (so the editor renders them with its own typeLabel), not just text
    // buried in a message string.
    let inner = ValidationError::TypeMismatch { edge: 2, src: Type::Number, dst: Type::Text };
    match WValidationError::from_core(&inner) {
        WValidationError::TypeMismatch { edge, src, dst, message } => {
            assert_eq!(edge, 2);
            assert_eq!(src, WType::Number);
            assert_eq!(dst, WType::Text);
            assert!(!message.is_empty());
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
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
    assert_eq!(holon_core::eval(&prim("add"), &[Some(num(2.0)), Some(num(3.0))], &r).into_vec(), vec![Some(num(5.0))]);
    assert_eq!(holon_core::eval(&prim("multiply"), &[Some(num(4.0)), Some(num(5.0))], &r).into_vec(), vec![Some(num(20.0))]);
    // NaN (inf - inf): assert *is_nan*, not bits — the canonical view, and what is target-stable.
    match holon_core::eval(&prim("subtract"), &[Some(num(f64::INFINITY)), Some(num(f64::INFINITY))], &r).as_slice() {
        [Some(Value::Number(x))] => assert!(x.is_nan()),
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
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    };
    assert_eq!(holon_core::eval(&comp, &[], &r2).into_vec(), vec![Some(num(7.0))]);
}

#[cfg(test)]
fn check_wvalue_roundtrip() {
    let cases = [
        Value::Number(3.5),
        Value::Boolean(true),
        Value::Text("hi".into()),
        Value::List(vec![Value::Number(1.0), Value::Text("x".into())]),
        Value::Effect { sink: "egress".into(), payload: Box::new(Value::Number(2.0)) },
        Value::Sum { arm: 0, payload: Box::new(Value::Number(2.0)) },
        Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }, // none
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
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
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
    assert_eq!(cat.len(), 40);
    // A known interface survives core→WUnit: add = (a:Number, b:Number) -> Number.
    let add = cat
        .iter()
        .find(|w| matches!(&w.body, WBody::Primitive { name } if name == "add"))
        .expect("add in catalog");
    assert_eq!(add.inputs.len(), 2);
    assert!(add.inputs.iter().all(|p| p.ty == WType::Number));
    assert_eq!(add.outputs[0].ty, WType::Number);
    // A higher-order leaf keeps its function param: map carries an open `fn`.
    let map = cat
        .iter()
        .find(|w| matches!(&w.body, WBody::Primitive { name } if name == "map"))
        .expect("map in catalog");
    assert!(map.params.iter().any(|p| matches!(p, WParam::Open { name, .. } if name == "fn")));
}

#[cfg(test)]
fn check_variadic_primitives() {
    let names = variadic_primitives();
    // The exact set — a tripwire against silent drift between core's list and what crosses.
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["add", "and", "concat", "list", "multiply", "or"]);
    // subtract shares add's catalog interface line but must NOT be offered as growable — it's
    // not associative, core gives it no N-ary meaning.
    assert!(!names.contains(&"subtract".to_string()));
}

#[cfg(test)]
fn check_compatible() {
    // Core's §5 rule, reached through the bridge helper: equal types or either `Any`.
    assert!(types_compatible(WType::Number, WType::Number).unwrap());
    assert!(types_compatible(WType::Any, WType::Number).unwrap());
    assert!(types_compatible(WType::Number, WType::Any).unwrap());
    assert!(!types_compatible(WType::Number, WType::Text).unwrap());
    assert!(!types_compatible(WType::Effect, WType::List(Box::new(WType::Any))).unwrap());
    // Recursive now: List<Number> fits List<Any>, but not List<Text> — the whole point of this inch.
    assert!(types_compatible(
        WType::List(Box::new(WType::Number)),
        WType::List(Box::new(WType::Any))
    )
    .unwrap());
    assert!(!types_compatible(
        WType::List(Box::new(WType::Number)),
        WType::List(Box::new(WType::Text))
    )
    .unwrap());
}

#[cfg(test)]
fn check_wtype_roundtrip() {
    // Every shape round-trips through from_core -> into_core, not just List — Product and Sum are
    // the two constructors this inch actually adds precision for.
    let cases = [
        Type::Number,
        Type::list_any(),
        Type::List(Box::new(Type::Number)), // the precise element type now survives the bridge
        Type::Product(vec![Type::Number, Type::Text]),
        Type::Sum(vec![Type::Number, Type::Product(vec![])]), // Option<Number>
        // Nested: Product<List<Number>, Sum<Text, Unit>> — the two n-ary constructors compose,
        // and so does List inside them, exactly mirroring core's own sum_type_round_trips.
        Type::Product(vec![
            Type::List(Box::new(Type::Number)),
            Type::Sum(vec![Type::Text, Type::Product(vec![])]),
        ]),
    ];
    for t in cases {
        let w = WType::from_core(&t);
        assert_eq!(w.into_core().expect("into_core"), t);
    }
}

// wasm32-only, matching its sole caller (wasm_tests, below): its failure path constructs a
// JsError, which only has a real implementation under the wasm target — see the comment at its
// call site in native_tests for why. Gating the definition itself, not just the registration,
// so a native build has no dead, never-callable function sitting in it.
#[cfg(all(test, target_arch = "wasm32"))]
fn check_wtype_depth_bound() {
    // WType::into_core is now a genuinely untrusted-input boundary (compatible/build_preimage are
    // public #[wasm_bindgen] fns) — an adversarially deep WType must be refused, not overflow the
    // stack. Mirrors core's own deep_type_is_rejected.
    let mut deep = WType::Any;
    for _ in 0..(MAX_TYPE_DEPTH + 5) {
        deep = WType::List(Box::new(deep));
    }
    assert!(deep.into_core().is_err());

    // A depth well within the bound still succeeds — this isn't accidentally rejecting everything.
    let mut shallow = WType::Any;
    for _ in 0..3 {
        shallow = WType::List(Box::new(shallow));
    }
    assert!(shallow.into_core().is_ok());
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
    fn wtype_roundtrip_native() { check_wtype_roundtrip(); }
    // wtype_depth_bound is wasm-only (below): its failure path constructs a JsError, a
    // wasm-bindgen FFI stub with no native implementation — invoking it outside wasm32 panics
    // unconditionally ("cannot call wasm-bindgen imported functions on non-wasm targets"), not a
    // property of the depth-bound logic itself. The bound and its rejection are still proven —
    // just where JsError can actually exist.
    #[test]
    fn primitive_catalog_native() { check_primitive_catalog(); }
    #[test]
    fn variadic_primitives_native() { check_variadic_primitives(); }
    #[test]
    fn compatible_native() { check_compatible(); }
    #[test]
    fn project_native() { check_project(); }
    #[test]
    fn encap_error_structure_native() { check_encap_error_structure(); }
    #[test]
    fn validation_error_structure_native() { check_validation_error_structure(); }
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
    fn wtype_roundtrip_wasm() { check_wtype_roundtrip(); }
    #[wasm_bindgen_test]
    fn wtype_depth_bound_wasm() { check_wtype_depth_bound(); }
    #[wasm_bindgen_test]
    fn primitive_catalog_wasm() { check_primitive_catalog(); }
    #[wasm_bindgen_test]
    fn variadic_primitives_wasm() { check_variadic_primitives(); }
    #[wasm_bindgen_test]
    fn compatible_wasm() { check_compatible(); }
    #[wasm_bindgen_test]
    fn project_wasm() { check_project(); }
    #[wasm_bindgen_test]
    fn encap_error_structure_wasm() { check_encap_error_structure(); }
    #[wasm_bindgen_test]
    fn validation_error_structure_wasm() { check_validation_error_structure(); }

    // The full editor path across the JS boundary: insert an add, project the one-node graph, get the
    // composite WUnit back, confirm it equals the JS-free project_core (same canonical bytes) and
    // evaluates to 5.
    #[wasm_bindgen_test]
    fn project_crosses_js_boundary() {
        let mut ev = Evaluator::new();
        let add_pre = encode_preimage(&binop("add")).unwrap();
        let hex = ev.insert(&add_pre).unwrap();

        let edges_js = serde_wasm_bindgen::to_value(&Vec::<WEdge>::new()).unwrap();
        let wcomp_js = ev.project(vec![hex.clone()], edges_js).unwrap();
        let wcomp: WUnit = serde_wasm_bindgen::from_value(wcomp_js).unwrap();
        let bridged = wcomp.into_core().unwrap();

        let h = parse_hash(&hex).unwrap();
        let native = ev.project_core(vec![h], vec![]).unwrap();
        assert_eq!(encode_preimage(&bridged).unwrap(), encode_preimage(&native).unwrap());

        assert_eq!(
            holon_core::eval(
                &bridged,
                &[Some(Value::Number(2.0)), Some(Value::Number(3.0))],
                &ev.resolver,
            ).into_vec(),
            vec![Some(Value::Number(5.0))],
        );
    }

    // get_unit (P4.3.1, expand-to-inspect's actual primitive): a hash already resolved in this
    // session — a primitive AND a composite, since expand specifically needs the latter — comes
    // back as the exact same structure, no preimage involved anywhere in the call. A hash never
    // inserted here fails as a plain string, not a structured error.
    #[wasm_bindgen_test]
    fn get_unit_crosses_js_boundary() {
        let mut ev = Evaluator::new();
        let add_pre = encode_preimage(&binop("add")).unwrap();
        let hex = ev.insert(&add_pre).unwrap();

        let prim_js = ev.get_unit(&hex).unwrap();
        let prim_w: WUnit = serde_wasm_bindgen::from_value(prim_js).unwrap();
        assert!(matches!(prim_w.body, WBody::Primitive { ref name } if name == "add"));
        assert_eq!(prim_w.inputs.len(), 2);

        // A composite hash too — get_unit's own doc names this as the actual reason it exists
        // (a grouped node's preimage is discarded after grouping; only the hash survives).
        let h = parse_hash(&hex).unwrap();
        let composite = ev.project_core(vec![h], vec![]).unwrap();
        let comp_pre = encode_preimage(&composite).unwrap();
        let comp_hex = ev.insert(&comp_pre).unwrap();

        let comp_js = ev.get_unit(&comp_hex).unwrap();
        let comp_w: WUnit = serde_wasm_bindgen::from_value(comp_js).unwrap();
        assert!(matches!(comp_w.body, WBody::Composite { .. }));
        assert_eq!(comp_w.inputs.len(), 2); // a, b promoted

        // A hash never inserted in this session — a plain string failure, not WEncapError or
        // any other structured family; there's nothing structured a "not found" belongs to.
        let never_inserted: Hash = [9u8; 32];
        let never_hex = never_inserted.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(ev.get_unit(&never_hex).is_err());
    }

    // capabilities/effects_of (P4.4.1): a pure primitive declares nothing; an effectful one
    // declares its own capability and the kind it projects to; a capability buried two levels
    // deep still surfaces at the top — mirrors core's own buried_egress_surfaces_before_import,
    // now proven crossing the JS boundary too; an unresolved hash fails as a plain string, same
    // convention as get_unit.
    #[wasm_bindgen_test]
    fn capabilities_and_effects_cross_js_boundary() {
        let mut ev = Evaluator::new();

        let add_hex = ev.insert(&encode_preimage(&prim("add")).unwrap()).unwrap();
        let add_caps: Vec<WCapability> =
            serde_wasm_bindgen::from_value(ev.capabilities(&add_hex).unwrap()).unwrap();
        assert!(add_caps.is_empty());
        let add_effects: Vec<WKind> =
            serde_wasm_bindgen::from_value(ev.effects_of(&add_hex).unwrap()).unwrap();
        assert!(add_effects.is_empty());

        let egress_hex = ev.insert(&encode_preimage(&prim("egress")).unwrap()).unwrap();
        let egress_caps: Vec<WCapability> =
            serde_wasm_bindgen::from_value(ev.capabilities(&egress_hex).unwrap()).unwrap();
        assert_eq!(egress_caps, vec![WCapability::NetworkEgress]);
        let egress_effects: Vec<WKind> =
            serde_wasm_bindgen::from_value(ev.effects_of(&egress_hex).unwrap()).unwrap();
        assert_eq!(egress_effects, vec![WKind::Sink]);

        // Buried two levels down, still surfaces at the top through the JS boundary too.
        let egress_h = parse_hash(&egress_hex).unwrap();
        let inner = ev.project_core(vec![egress_h], vec![]).unwrap();
        let inner_hex = ev.insert(&encode_preimage(&inner).unwrap()).unwrap();
        let inner_h = parse_hash(&inner_hex).unwrap();
        let mid = ev.project_core(vec![inner_h], vec![]).unwrap();
        let mid_hex = ev.insert(&encode_preimage(&mid).unwrap()).unwrap();

        let mid_caps: Vec<WCapability> =
            serde_wasm_bindgen::from_value(ev.capabilities(&mid_hex).unwrap()).unwrap();
        assert_eq!(mid_caps, vec![WCapability::NetworkEgress]);

        let never_inserted: Hash = [11u8; 32];
        let never_hex = never_inserted.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(ev.capabilities(&never_hex).is_err());
        assert!(ev.effects_of(&never_hex).is_err());
    }

    // The split encapsulation seam (P4.3, fork 3): propose across the JS boundary, edit the
    // proposal (exclude one of two free outputs), freeze the edited spec — proving a person can
    // genuinely change what the final composite exposes, not just preview it.
    #[wasm_bindgen_test]
    fn propose_and_freeze_encapsulation_crosses_js_boundary() {
        let mut ev = Evaluator::new();
        let add_pre = encode_preimage(&binop("add")).unwrap();
        let hex = ev.insert(&add_pre).unwrap();
        let children = vec![hex.clone(), hex.clone()]; // two independent adds ⇒ two free outputs
        let no_edges = || serde_wasm_bindgen::to_value(&Vec::<WEdge>::new()).unwrap();

        // Propose: default_spec's full proposal, crossed intact — both children exposed.
        let spec_js = ev.propose_encapsulation(children.clone(), no_edges()).unwrap();
        let mut spec: WEncapSpec = serde_wasm_bindgen::from_value(spec_js).unwrap();
        assert_eq!(spec.expose.len(), 2);
        assert_eq!(spec.promote.len(), 4); // a, b for each of the two adds

        // Edit: exclude the second exposed output — the actual point of this seam.
        spec.expose.truncate(1);

        // Freeze the edited spec: the resulting composite has ONE output, not two.
        let spec_edited_js = serde_wasm_bindgen::to_value(&spec).unwrap();
        let composite_js =
            ev.freeze_encapsulation(children.clone(), no_edges(), spec_edited_js).unwrap();
        let composite: WUnit = serde_wasm_bindgen::from_value(composite_js).unwrap();
        let bridged = composite.into_core().unwrap();
        assert_eq!(bridged.outputs.len(), 1);

        // It still evaluates correctly — the excluded output is gone from the interface, not
        // silently broken.
        assert_eq!(
            holon_core::eval(
                &bridged,
                &[
                    Some(Value::Number(2.0)),
                    Some(Value::Number(3.0)),
                    Some(Value::Number(10.0)),
                    Some(Value::Number(20.0)),
                ],
                &ev.resolver,
            ).into_vec(),
            vec![Some(Value::Number(5.0))],
        );

        // project() itself is genuinely unchanged: called directly on the same two adds, it
        // still produces the full two-output composite — the new seam doesn't alter the
        // existing path.
        let full_js = ev.project(children, no_edges()).unwrap();
        let full: WUnit = serde_wasm_bindgen::from_value(full_js).unwrap();
        assert_eq!(full.outputs.len(), 2);
    }

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
        let wout: Vec<Option<WValue>> = serde_wasm_bindgen::from_value(out).unwrap();
        assert_eq!(wout, vec![Some(WValue::Number(5.0))]);
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
            outputs: vec![WPort { name: "out".into(), ty: WType::Number }],
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
            outputs: vec![WPort { name: "out".into(), ty: WType::Number }],
        };
        let comp_pre = build_preimage(serde_wasm_bindgen::to_value(&comp_w).unwrap()).unwrap();

        let no_inputs = serde_wasm_bindgen::to_value(&Vec::<Option<WValue>>::new()).unwrap();
        let out = ev.eval(&comp_pre, no_inputs).unwrap();
        let wout: Vec<Option<WValue>> = serde_wasm_bindgen::from_value(out).unwrap();
        assert_eq!(wout, vec![Some(WValue::Number(7.0))]);
    }

    // The catalog crosses the JS boundary as an array of WUnit, intact — the new risk the
    // single-WUnit load_unit tests didn't cover.
    #[wasm_bindgen_test]
    fn primitive_catalog_crosses_js_boundary() {
        let js = primitive_catalog().unwrap();
        let cat: Vec<WUnit> = serde_wasm_bindgen::from_value(js).unwrap();
        assert_eq!(cat.len(), 40);
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