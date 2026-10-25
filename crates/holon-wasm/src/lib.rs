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

use holon_author::builder::FlowDocument;
use holon_author::canvas::{Node, Position, Wire};
use holon_author::composites::{self, BoundaryPort, Names};
use holon_author::group::{self, GroupError};
use holon_author::lang;
use holon_author::wire::{WEdge, WParam, WType, WUnit};
use holon_author::{flow, routes, templates, to_hex};
use holon_core::{
    decode_preimage, default_spec, derive_and_freeze, encode_preimage, genesis_units, hash_unit,
    parse_hash, Capability, Edge, EncapError, EncapSpec, Expose, Hash, Kind, MapResolver,
    Promote, Resolver, Unit, Value, ValidationError,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// The JS forms of core's units, ports, params and types (`WUnit`, `WPort`, `WParam`, `WType`,
// `WEdge`) are holon-author's (wire.rs): the same ones the flow documents hold and the command
// line reads.

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
    /// Mirrors `core::Value::Record` as `[key, value]` pairs in key order:
    /// `{ "type": "Record", "value": [["id", {...}], ...] }`. Pairs rather than a map because
    /// serde-wasm-bindgen turns a map into a JS `Map`, not a plain object.
    Record(Vec<(String, WValue)>),
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
            Value::Record(fields) => {
                WValue::Record(fields.iter().map(|(k, v)| (k.clone(), WValue::from_core(v))).collect())
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
            // A repeated key keeps the last value, as a JS object literal would.
            WValue::Record(fields) => {
                Value::Record(fields.into_iter().map(|(k, v)| (k, v.into_core())).collect())
            }
        }
    }
}

// ── The JS Unit boundary (construction) ────────────────────────────────────────────────────────
// `WUnit` mirrors `core::Unit` so the editor describes a node as a plain JS object and the bridge
// encodes it with the ONE core encoder. Children cross as hex hashes; types as plain strings.

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
    SecretRead(String),
    Await(String),
    Random,
    StateRead(String),
    StateWrite(String),
    CollectionRead(String),
    CollectionWrite(String),
    NetworkEgress,
    HttpRequest(String),
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
            Capability::SecretRead(name) => WCapability::SecretRead(name),
            Capability::Await(name) => WCapability::Await(name),
            Capability::Random => WCapability::Random,
            Capability::StateRead(key) => WCapability::StateRead(key),
            Capability::StateWrite(key) => WCapability::StateWrite(key),
            Capability::CollectionRead(name) => WCapability::CollectionRead(name),
            Capability::CollectionWrite(name) => WCapability::CollectionWrite(name),
            Capability::NetworkEgress => WCapability::NetworkEgress,
            Capability::HttpRequest(host) => WCapability::HttpRequest(host),
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

/// Build a unit's canonical preimage from a JS [`WUnit`] — the editor's "compile this node to
/// canonical bytes", through the ONE core encoder. The bytes feed `insert`, `eval`, or `hash`.
#[wasm_bindgen]
pub fn build_preimage(unit: JsValue) -> Result<Vec<u8>, JsError> {
    let wunit: WUnit =
        serde_wasm_bindgen::from_value(unit).map_err(|e| JsError::new(&format!("unit: {e}")))?;
    let core = wunit.into_core().map_err(|e| JsError::new(&e))?;
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

/// The variadic leaves whose extra inputs are any type, passed to their function after the item
/// (`map`, `fold`: core's `catalog::PASSES_EXTRA`), rather than more of the same port.
#[wasm_bindgen]
pub fn passes_extra_primitives() -> Vec<String> {
    holon_core::PASSES_EXTRA.iter().map(|s| s.to_string()).collect()
}

/// What each genesis primitive does, by name (core's `describe`): the editor shows it on each
/// node and in the palette, read from core like the catalog, never a TS copy that could drift.
#[wasm_bindgen]
pub fn primitive_docs() -> Result<JsValue, JsError> {
    let docs: std::collections::BTreeMap<&str, &str> =
        holon_core::GENESIS.iter().filter_map(|n| holon_core::describe(n).map(|d| (*n, d))).collect();
    serde_wasm_bindgen::to_value(&docs).map_err(|e| JsError::new(&format!("docs: {e}")))
}

/// What a flow's deploy settings ask beyond the host's defaults, as the editor keeps them.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct AskedFor {
    limits: std::collections::BTreeMap<String, u64>,
    plain_http: std::collections::BTreeSet<String>,
    private_addresses: bool,
}

/// A manifest's items (core's `manifest_items`, the rule the server grants by): `capabilities`
/// (consent names), then what `requirements` (`{limits, plain_http, private_addresses}`) ask.
#[wasm_bindgen]
pub fn manifest_items(capabilities: Vec<String>, requirements: JsValue) -> Result<Vec<String>, JsError> {
    let asked: AskedFor = if requirements.is_undefined() || requirements.is_null() {
        AskedFor::default()
    } else {
        serde_wasm_bindgen::from_value(requirements).map_err(|e| JsError::new(&format!("requirements: {e}")))?
    };
    Ok(holon_core::manifest_items(&capabilities, &asked.limits, &asked.plain_http, asked.private_addresses).into_iter().collect())
}

/// What starts the runs of a unit with these capabilities (core's `trigger`).
#[wasm_bindgen]
pub fn manifest_trigger(capabilities: Vec<String>) -> Option<String> {
    holon_core::trigger(&capabilities).map(String::from)
}

/// `"required"` or `"optional"` (core's `permission_kind`): optional only when marked so, never
/// the trigger.
#[wasm_bindgen]
pub fn permission_kind(item: &str, optional: Vec<String>, trigger: Option<String>) -> String {
    let optional: std::collections::BTreeSet<String> = optional.into_iter().collect();
    match holon_core::permission_kind(item, &optional, trigger.as_deref()) {
        holon_core::PermissionKind::Required => "required".into(),
        holon_core::PermissionKind::Optional => "optional".into(),
    }
}

/// Whether a unit with these capabilities reads from outside a run's inputs (core's
/// `reads_from_outside`): then a replay shows what the run recorded it produced.
#[wasm_bindgen]
pub fn reads_from_outside(capabilities: Vec<String>) -> bool {
    holon_core::reads_from_outside(&capabilities)
}

/// One input of a choice as a replay has seen it: `null` not evaluated yet, else `{value}`
/// (`value: null` for absent).
#[derive(Deserialize)]
struct WSeen {
    value: Option<WValue>,
}

/// What a choice needs next: `{need: port}`, or `{done: value}` (`null` for absent).
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum WChoice {
    Need(usize),
    Done(Option<WValue>),
}

/// Core's choice rule (`choice_next`): what a `guard` or `coalesce` needs next, given the inputs
/// seen so far, or its output once decided; `undefined` for a leaf that isn't a choice. The
/// editor's replay walks a run by it, so the nodes it shows as not run are the ones the run
/// didn't run. A replay never has a pending input: what the record lacks is absent.
#[wasm_bindgen]
pub fn choice_next(name: &str, seen: JsValue) -> Result<JsValue, JsError> {
    let seen: Vec<Option<WSeen>> = serde_wasm_bindgen::from_value(seen).map_err(|e| JsError::new(&format!("seen: {e}")))?;
    let seen: Vec<Option<holon_core::Slot>> = seen
        .into_iter()
        .map(|s| s.map(|s| s.value.map_or(holon_core::Slot::Absent, |v| holon_core::Slot::Value(v.into_core()))))
        .collect();
    let choice = holon_core::choice_next(name, &seen).map(|c| match c {
        holon_core::Choice::Need(p) => WChoice::Need(p),
        holon_core::Choice::Done(slot) => WChoice::Done(slot.value().map(WValue::from_core)),
    });
    serde_wasm_bindgen::to_value(&choice).map_err(|e| JsError::new(&format!("choice: {e}")))
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
    let (s, d) = (s.into_core().map_err(|e| JsError::new(&e))?, d.into_core().map_err(|e| JsError::new(&e))?);
    Ok(holon_core::compatible(&s, &d))
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
    /// A module body that isn't one: the message says what (imports, a missing export, a budget).
    Module { message: String },
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
            ValidationError::Module(_) => WValidationError::Module { message },
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

    /// A unit's capabilities by the names consent uses (`StateRead("todos")`, core's
    /// `consent_name`): what a deploy acknowledges and a manifest lists, with no TypeScript
    /// rendering of its own.
    pub fn capability_names(&self, hash: &str) -> Result<Vec<String>, JsError> {
        let unit = self.resolve_hash(hash)?;
        Ok(holon_core::capabilities(unit, &self.resolver).iter().map(|c| holon_core::consent_name(&c)).collect())
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

    /// A placed node's own unit, stored here, by hash (holon-author's `unit_for`): a primitive
    /// rebuilt from its name and ports, a template from its params, a grouped composite by its
    /// hash.
    pub fn unit_for(&mut self, node: JsValue) -> Result<String, JsError> {
        let node: Node = serde_wasm_bindgen::from_value(node).map_err(|e| JsError::new(&format!("node: {e}")))?;
        flow::unit_for(&node, &mut self.resolver).map(|h| to_hex(&h)).map_err(|e| JsError::new(&e))
    }

    /// A group of nodes as core children plus the wires among them, `{children, wedges}`.
    pub fn group_of(&mut self, nodes: JsValue, edges: JsValue) -> Result<JsValue, JsError> {
        #[derive(Serialize)]
        struct Group {
            children: Vec<String>,
            wedges: Vec<WEdge>,
        }
        let (nodes, wires) = canvas(nodes, edges)?;
        let group: Vec<&Node> = nodes.iter().collect();
        let (children, edges) = flow::group_of(&group, &wires, &mut self.resolver).map_err(|e| JsError::new(&e))?;
        to_js(&Group { children: children.iter().map(to_hex).collect(), wedges: edges.iter().map(WEdge::from_core).collect() })
    }

    /// Which node each child of the stored composite `hash` is, in the order it is stored and
    /// run in, `nodes` being the group it was made from, in the order it was given.
    pub fn trace_of(&self, hash: &str, nodes: JsValue) -> Result<Vec<String>, JsError> {
        let nodes: Vec<Node> = serde_wasm_bindgen::from_value(nodes).map_err(|e| JsError::new(&format!("nodes: {e}")))?;
        let group: Vec<&Node> = nodes.iter().collect();
        Ok(flow::trace_of(self.resolve_hash(hash)?, &group))
    }

    /// The server a flow declares, `{endpoints, errors, serverHash, trace}`, its units stored here
    /// (holon-author's `derive_server`). `port` is as the flow's settings hold it.
    pub fn derive_server(&mut self, nodes: JsValue, edges: JsValue, port: &str) -> Result<JsValue, JsError> {
        let (nodes, wires) = canvas(nodes, edges)?;
        to_js(&flow::derive_server(&nodes, &wires, &mut self.resolver, port))
    }

    /// The unit a flow deploys as, stored here: its server when it has routes, else the whole flow
    /// as one composite. Throws why it can't be.
    pub fn flow_unit(&mut self, nodes: JsValue, edges: JsValue, port: &str) -> Result<String, JsError> {
        let (nodes, wires) = canvas(nodes, edges)?;
        flow::flow_unit(&nodes, &wires, &mut self.resolver, port).map(|h| to_hex(&h)).map_err(|e| JsError::new(&e))
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

    /// The reordering core applies to a composite's children when it hashes (and so stores) it:
    /// element `k` is the index, as given here, of the child that is child `k` once stored. The
    /// editor records it at deploy time, to trace the server's per-child timings back to canvas
    /// nodes. Same `children`/`edges` shape as `propose_encapsulation`.
    pub fn canonical_child_order(&self, children: Vec<String>, edges: JsValue) -> Result<Vec<u32>, JsValue> {
        let (hashes, core_edges) = Self::parse_children_and_edges(&children, edges)?;
        holon_core::canonical_child_order(&hashes, &core_edges)
            .map(|order| order.into_iter().map(|i| i as u32).collect())
            .map_err(|e| JsValue::from_str(&format!("order: {e:?}")))
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
        Ok((hashes, wedges.into_iter().map(WEdge::into_core).collect()))
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Evaluator::new()
    }
}

// ── Parity & boundary tests: one body, run on BOTH targets ─────────────────────────────────────

// ── Authoring (holon-author) ─────────────────────────────────────────────────────────────────
// What a flow turns into: its nodes' units, its routes, its server, the unit it deploys as. The
// editor hands its canvas (nodes and wires, as React Flow holds them; only what decides the
// units is read) and gets back what core derives, decided once, in Rust, for the editor and the
// command line alike.

/// Results that hold `null`s and maps cross as plain JSON-shaped values (`null`, objects), not
/// `undefined` and `Map`.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsError::new(&e.to_string()))
}

fn canvas(nodes: JsValue, edges: JsValue) -> Result<(Vec<Node>, Vec<Wire>), JsError> {
    let nodes = serde_wasm_bindgen::from_value(nodes).map_err(|e| JsError::new(&format!("nodes: {e}")))?;
    let wires = serde_wasm_bindgen::from_value(edges).map_err(|e| JsError::new(&format!("wires: {e}")))?;
    Ok((nodes, wires))
}

fn not_a_template(name: &str) -> JsError {
    JsError::new(&format!("\"{name}\" is not a template"))
}

/// The node templates, in palette order (holon-author's templates.rs).
#[wasm_bindgen]
pub fn template_names() -> Vec<String> {
    templates::TEMPLATES.iter().map(|s| s.to_string()).collect()
}

/// What a template does, for the person placing it; `undefined` for a name that isn't one.
#[wasm_bindgen]
pub fn template_doc(name: &str) -> Option<String> {
    templates::describe(name).map(String::from)
}

/// A template node's `{inputs, outputs, hiddenInputs}` for these params.
#[wasm_bindgen]
pub fn template_interface(name: &str, params: JsValue) -> Result<JsValue, JsError> {
    let params: Vec<WParam> = serde_wasm_bindgen::from_value(params).map_err(|e| JsError::new(&format!("params: {e}")))?;
    to_js(&templates::interface(name, &params).ok_or_else(|| not_a_template(name))?)
}

/// The params a freshly placed template node starts with.
#[wasm_bindgen]
pub fn template_defaults(name: &str) -> Result<JsValue, JsError> {
    to_js(&templates::defaults(name).ok_or_else(|| not_a_template(name))?)
}

/// A route path's segments, as core parses it: `[{literal, param}]` (one of the two set), or
/// `null` for a path core refuses.
#[wasm_bindgen]
pub fn route_path(path: &str) -> Result<JsValue, JsError> {
    #[derive(Serialize)]
    struct Seg {
        literal: Option<String>,
        param: Option<String>,
    }
    let segs = holon_core::parse_path(path).map(|segs| {
        segs.into_iter()
            .map(|s| match s {
                holon_core::Segment::Literal(l) => Seg { literal: Some(l), param: None },
                holon_core::Segment::Param(p) => Seg { literal: None, param: Some(p) },
            })
            .collect::<Vec<_>>()
    });
    to_js(&segs)
}

/// The flow's connected groups of node ids (wires either way), in the order `ids` gives.
#[wasm_bindgen]
pub fn canvas_components(ids: Vec<String>, edges: JsValue) -> Result<JsValue, JsError> {
    let wires: Vec<Wire> = serde_wasm_bindgen::from_value(edges).map_err(|e| JsError::new(&format!("wires: {e}")))?;
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    to_js(&routes::components(&ids, &wires))
}

/// The routes a flow declares, `{routes, errors}`: one per connected group holding a request node.
#[wasm_bindgen]
pub fn find_routes(nodes: JsValue, edges: JsValue) -> Result<JsValue, JsError> {
    #[derive(Serialize)]
    struct Found {
        routes: Vec<routes::RouteGroup>,
        errors: Vec<String>,
    }
    let (nodes, wires) = canvas(nodes, edges)?;
    let (routes, errors) = routes::find_routes(&nodes, &wires);
    to_js(&Found { routes, errors })
}

// ── Composites, grouping and the text format (holon-author) ──────────────────────────────────
// Grouping a selection, a holon's inside opened as a canvas, expanded, frozen back; a flow built
// node by node (the samples) and the `.holon` text format. Each answers `{ok: true, …}` or
// `{ok: false, message, nodeIds}`, as the canvas operations do, so a failure points at nodes.

fn from_js<T: serde::de::DeserializeOwned>(value: JsValue, what: &str) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&format!("{what}: {e}")))
}

/// `{ok: true, …fields}` from a value that serializes to an object.
fn ok_with<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let mut v = serde_json::to_value(value).map_err(|e| JsError::new(&e.to_string()))?;
    if let serde_json::Value::Object(m) = &mut v {
        m.insert("ok".into(), serde_json::Value::Bool(true));
    }
    to_js(&v)
}

fn failed(e: &GroupError) -> Result<JsValue, JsError> {
    to_js(&serde_json::json!({ "ok": false, "message": e.message, "nodeIds": e.node_ids }))
}

#[wasm_bindgen]
impl Evaluator {
    /// A selection grouped into one placed composite node, `{ok, node, edges}` (group.rs), its id
    /// `composite-<id>-…`.
    pub fn group_selection(&mut self, nodes: JsValue, edges: JsValue, id: f64) -> Result<JsValue, JsError> {
        let (nodes, wires) = canvas(nodes, edges)?;
        match group::group_selection(&nodes, &wires, &mut self.resolver, id as u64) {
            Ok((node, edges)) => ok_with(&serde_json::json!({ "node": node, "edges": edges })),
            Err(e) => failed(&e),
        }
    }

    /// A saved flow placed in another as one node, `{ok, node}` or `{ok: false, message}`. Its
    /// units must be inserted here first.
    pub fn flow_as_node(&mut self, name: &str, nodes: JsValue, edges: JsValue, id: &str, position: JsValue, saved_at: Option<f64>) -> Result<JsValue, JsError> {
        let (nodes, wires) = canvas(nodes, edges)?;
        let position: Position = from_js(position, "position")?;
        match group::flow_as_node(name, &nodes, &wires, &mut self.resolver, id, position, saved_at) {
            Ok(node) => ok_with(&serde_json::json!({ "node": node })),
            Err(message) => failed(&GroupError::new(message)),
        }
    }

    /// The composite node `target` as a canvas of its own, `{ok, nodes, edges, childIds}`: its
    /// boundary's two nodes, then its children (composites.rs). `names` names nested composites.
    pub fn inner_canvas(&self, target: JsValue, nodes: JsValue, edges: JsValue, names: JsValue) -> Result<JsValue, JsError> {
        let target: Node = from_js(target, "target")?;
        let (nodes, wires) = canvas(nodes, edges)?;
        let names: Names = from_js(names, "names")?;
        match composites::inner_canvas(&target, &nodes, &wires, &self.resolver, &names) {
            Ok(c) => {
                let json = |e: serde_json::Error| JsError::new(&e.to_string());
                let mut all = vec![serde_json::to_value(&c.inputs).map_err(json)?, serde_json::to_value(&c.outputs).map_err(json)?];
                for n in &c.children {
                    all.push(serde_json::to_value(n).map_err(json)?);
                }
                ok_with(&serde_json::json!({ "nodes": all, "edges": c.wires, "childIds": c.child_ids }))
            }
            Err(message) => failed(&GroupError::new(message)),
        }
    }

    /// The composite node `target` expanded in place, `{ok, nodes, rewired, added}`: its children
    /// (ids from `ids`, in order), the flow's wires re-pointed, its inner wires as new ones.
    pub fn expand_composite(&self, target: JsValue, edges: JsValue, ids: Vec<f64>, names: JsValue) -> Result<JsValue, JsError> {
        let target: Node = from_js(target, "target")?;
        let wires: Vec<Wire> = from_js(edges, "wires")?;
        let names: Names = from_js(names, "names")?;
        let ids: Vec<u64> = ids.into_iter().map(|i| i as u64).collect();
        match composites::expand_composite(&target, &wires, &self.resolver, &ids, &names) {
            Ok(expanded) => ok_with(&expanded),
            Err(message) => failed(&GroupError::new(message)),
        }
    }

    /// An inner canvas frozen back into a composite, `{ok, hash, inputs, outputs}`: its boundary's
    /// ports (`ins`, `outs`), its other nodes, its wires.
    pub fn freeze_inner(&mut self, ins: JsValue, outs: JsValue, nodes: JsValue, edges: JsValue) -> Result<JsValue, JsError> {
        let ins: Vec<BoundaryPort> = from_js(ins, "inputs")?;
        let outs: Vec<BoundaryPort> = from_js(outs, "outputs")?;
        let (nodes, wires) = canvas(nodes, edges)?;
        match composites::freeze_inner(&ins, &outs, &nodes, &wires, &mut self.resolver) {
            Ok((hash, inputs, outputs)) => ok_with(&serde_json::json!({ "hash": hash, "inputs": inputs, "outputs": outputs })),
            Err(e) => failed(&e),
        }
    }

    /// A flow document as `.holon` text in the language's expression form (holon-author's
    /// lang/print.rs). Its units are inserted here; `names` names the composites the document
    /// doesn't; `bindings` (optional) names top-level nodes, by id, so a text imported and
    /// exported keeps its names.
    pub fn print_holon(&mut self, name: Option<String>, doc: JsValue, names: JsValue, bindings: JsValue) -> Result<String, JsError> {
        let doc: FlowDocument = from_js(doc, "document")?;
        let names: Names = from_js(names, "names")?;
        let bindings = bindings_from(bindings)?;
        lang::print(name.as_deref(), &doc, &mut self.resolver, &names, &bindings).map_err(|e| JsError::new(&e))
    }

    /// [`Self::print_holon`], with the line each top-level node is written on: `{text, lines}`,
    /// `lines` by node id (1-based). What view mode shows beside a selected node.
    pub fn print_holon_lines(&mut self, name: Option<String>, doc: JsValue, names: JsValue, bindings: JsValue) -> Result<JsValue, JsError> {
        let doc: FlowDocument = from_js(doc, "document")?;
        let names: Names = from_js(names, "names")?;
        let bindings = bindings_from(bindings)?;
        let (text, lines) = lang::print::print_lines(name.as_deref(), &doc, &mut self.resolver, &names, &bindings).map_err(|e| JsError::new(&e))?;
        to_js(&serde_json::json!({ "text": text, "lines": lines }))
    }
}

/// Top-level node names by id, or none when JS passed nothing.
fn bindings_from(v: JsValue) -> Result<std::collections::BTreeMap<String, String>, JsError> {
    if v.is_undefined() || v.is_null() {
        return Ok(Default::default());
    }
    from_js(v, "bindings")
}

/// What a manifest item (a capability's consent name, or a requirement) lets a flow do, in words
/// (holon-author's describe.rs).
#[wasm_bindgen]
pub fn describe_item(item: &str) -> String {
    holon_author::describe::describe_item(item)
}

/// `.holon` text, in either form, as a flow document: `{name, doc, names, lines}` (each
/// top-level node's name in the text, and the line it is written on, by id). Throws the first
/// mistake, as `line:col: message`.
#[wasm_bindgen]
pub fn compile_holon(source: &str) -> Result<JsValue, JsError> {
    let compiled = lang::compile(source).map_err(|e| JsError::new(&e.to_string()))?;
    let lines: std::collections::BTreeMap<&String, u32> = compiled.spans.iter().map(|(id, s)| (id, s.start.line)).collect();
    to_js(&serde_json::json!({ "name": compiled.name, "doc": compiled.doc, "names": compiled.names, "lines": lines }))
}

// ── The text pane (docs/L4s/text-pane-L4.md) ────────────────────────────────────────────────
// What an editor asks of a text (holon-author's editing.rs, the answers `holon lsp` gives), with
// positions as the editor counts them: UTF-16 offsets into the whole text. The language counts
// columns in characters, so each position is converted here against the text.

/// A text's lines, for converting between the language's positions and UTF-16 offsets.
struct Offsets<'a> {
    lines: Vec<&'a str>,
    /// Each line's first offset.
    starts: Vec<usize>,
}

impl<'a> Offsets<'a> {
    fn new(source: &'a str) -> Self {
        let lines: Vec<&str> = source.split('\n').collect();
        let mut starts = Vec::with_capacity(lines.len());
        let mut at = 0;
        for line in &lines {
            starts.push(at);
            at += line.encode_utf16().count() + 1;
        }
        Offsets { lines, starts }
    }

    /// The offset of a 1-based line and column (in characters); past a line's end is its end.
    fn offset(&self, p: lang::ast::Pos) -> usize {
        let l = (p.line.max(1) as usize - 1).min(self.lines.len() - 1);
        let before: usize = self.lines[l].chars().take(p.col.max(1) as usize - 1).map(char::len_utf16).sum();
        self.starts[l] + before
    }

    /// The 1-based line and column (in characters) of an offset.
    fn pos(&self, offset: usize) -> lang::ast::Pos {
        let l = self.starts.partition_point(|s| *s <= offset).saturating_sub(1);
        let mut left = offset - self.starts[l];
        let mut col = 1;
        for c in self.lines[l].chars() {
            if left < c.len_utf16() {
                break;
            }
            left -= c.len_utf16();
            col += 1;
        }
        lang::ast::Pos { line: l as u32 + 1, col }
    }
}

/// The text's mistakes, as `holon check` says them: `[{from, to, message}]`.
#[wasm_bindgen]
pub fn holon_mistakes(source: &str) -> Result<JsValue, JsError> {
    let at = Offsets::new(source);
    let found: Vec<_> = holon_author::editing::mistakes(source)
        .into_iter()
        .map(|m| serde_json::json!({ "from": at.offset(m.span.start), "to": at.offset(m.span.end), "message": m.message }))
        .collect();
    to_js(&found)
}

/// What the word at `offset` is: `{from, to, text}` (markdown), or null.
#[wasm_bindgen]
pub fn holon_hover(source: &str, offset: usize) -> Result<JsValue, JsError> {
    let at = Offsets::new(source);
    let said = holon_author::editing::hover(source, at.pos(offset)).map(|h| serde_json::json!({ "from": at.offset(h.span.start), "to": at.offset(h.span.end), "text": h.text }));
    to_js(&said)
}

/// What may be written at `offset`, starting with the word typed before it: `[{label, kind,
/// detail, doc?}]`, `kind` one of keyword, function, variable, module, member.
#[wasm_bindgen]
pub fn holon_complete(source: &str, offset: usize) -> Result<JsValue, JsError> {
    use holon_author::editing::Kind;
    let at = Offsets::new(source);
    let offered: Vec<_> = holon_author::editing::complete(source, at.pos(offset))
        .into_iter()
        .map(|c| {
            let kind = match c.kind {
                Kind::Keyword => "keyword",
                Kind::Function => "function",
                Kind::Variable => "variable",
                Kind::Module => "module",
                Kind::Member => "member",
            };
            serde_json::json!({ "label": c.label, "kind": kind, "detail": c.detail, "doc": c.doc })
        })
        .collect();
    to_js(&offered)
}

/// The text as `holon fmt` writes it. Throws why it can't be (a mistake).
#[wasm_bindgen]
pub fn holon_format(source: &str) -> Result<String, JsError> {
    holon_author::editing::format(source).map_err(|e| JsError::new(&e))
}

/// The text's colours: `[{from, to, class}]`, in order, `class` the lowercase name of
/// editing.rs's `Class` (keyword, noun, function, definition, …).
#[wasm_bindgen]
pub fn holon_highlight(source: &str) -> Result<JsValue, JsError> {
    let at = Offsets::new(source);
    let coloured: Vec<_> = holon_author::editing::highlight(source)
        .into_iter()
        .map(|h| serde_json::json!({ "from": at.offset(h.span.start), "to": at.offset(h.span.end), "class": format!("{:?}", h.class).to_lowercase() }))
        .collect();
    to_js(&coloured)
}

/// The sample flows, one per capability, in the order the flows page lists them: `[{name, title,
/// description, shows, port?, interval?, secrets?, optional?, browsers?, tryIt}]` (samples.rs).
#[wasm_bindgen]
pub fn samples() -> Result<JsValue, JsError> {
    to_js(&holon_author::samples::samples())
}

/// The sample `name` laid out on a canvas: the flow document opening it loads, its holons' units
/// in it. Throws when there's no such sample, or it doesn't build (`sample "name": …`).
#[wasm_bindgen]
pub fn build_sample(name: &str) -> Result<JsValue, JsError> {
    let sample = holon_author::samples::sample(name).ok_or_else(|| JsError::new(&format!("no sample \"{name}\"")))?;
    to_js(&holon_author::samples::build(sample).map_err(|e| JsError::new(&e))?)
}

#[cfg(test)]
use holon_core::{Param, Port, Type, UnitBody, BOUNDARY};
#[cfg(test)]
use holon_author::wire::WBody;
#[cfg(all(test, target_arch = "wasm32"))]
use holon_core::MAX_TYPE_DEPTH;
#[cfg(all(test, target_arch = "wasm32"))]
use holon_author::wire::WPort;

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

/// Arithmetic on fractions gives the same bits natively and in wasm. Random operands, everyday
/// fractions and raw bit patterns across the whole finite range (subnormals included), go
/// through the core's add, subtract, multiply, divide, sum and compare; each result must be
/// exactly the IEEE operation on this target, and the hash of all of them is pinned, so the
/// native and wasm runs must agree with each other bit for bit.
#[cfg(test)]
fn check_fraction_parity() {
    // splitmix64: a fixed seed, so both targets draw the same operands.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut operand = |i: usize| loop {
        let r = next();
        let x = if i.is_multiple_of(2) {
            (r >> 11) as f64 / (1u64 << 53) as f64 * 2000.0 - 1000.0
        } else {
            f64::from_bits(r)
        };
        if x.is_finite() {
            return x;
        }
    };
    let num = Value::Number;
    let r = MapResolver::new();
    let less = Unit { params: vec![Param::frozen("op", "<")], ..prim("compare") };
    let sum = Unit { inputs: vec![], ..prim("sum") };
    let one = |unit: &Unit, inputs: &[Value]| -> Value {
        let inputs: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
        match holon_core::eval(unit, &inputs, &r).into_vec().as_slice() {
            [Some(v)] => v.clone(),
            other => panic!("expected one value, got {other:?}"),
        }
    };
    let same = |got: &Value, want: f64| match got {
        Value::Number(x) => assert!(x.to_bits() == want.to_bits() || (x.is_nan() && want.is_nan()), "{x} != {want}"),
        other => panic!("expected a number, got {other:?}"),
    };
    let mut results = Vec::new();
    for i in 0..2000 {
        let (a, b) = (operand(i), operand(i));
        let (va, vb) = (num(a), num(b));
        let added = one(&prim("add"), &[va.clone(), vb.clone()]);
        same(&added, a + b);
        let taken = one(&prim("subtract"), &[va.clone(), vb.clone()]);
        same(&taken, a - b);
        let times = one(&prim("multiply"), &[va.clone(), vb.clone()]);
        same(&times, a * b);
        let over = one(&prim("divide"), &[va.clone(), vb.clone()]);
        same(&over, a / b);
        let list: Vec<f64> = (0..5).map(|_| operand(i)).collect();
        let total = one(&sum, &[Value::List(list.iter().copied().map(num).collect())]);
        same(&total, list.iter().fold(0.0, |acc, x| acc + x));
        let below = one(&less, &[va, vb]);
        assert_eq!(below, Value::Boolean(a < b));
        results.extend([added, taken, times, over, total, below]);
    }
    let digest = to_hex(&holon_core::hash_value(&Value::List(results)).unwrap());
    assert_eq!(digest, "7cfce01fd18b2d61697e95827946a049328984738904c64e6af56b9bfb2fb0e2");
}

/// The signature kit computes the same digests natively and in wasm (sha2/hmac are pure Rust, so
/// they should; this is the check), through the catalog's own default-configured units.
#[cfg(test)]
fn check_signature_parity() {
    let txt = |s: &str| Value::Text(s.to_string());
    let r = MapResolver::new();
    let run = |name: &str, inputs: &[Value]| {
        let unit = holon_core::interface_of(name).unwrap();
        let inputs: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
        holon_core::eval(&unit, &inputs, &r).into_vec()
    };
    // RFC 4231 test case 2, FIPS 180-2 "abc", RFC 4648 "foobar".
    let mac = txt("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    assert_eq!(run("hmac_sha256", &[txt("Jefe"), txt("what do ya want for nothing?")]), vec![Some(mac.clone())]);
    assert_eq!(run("sha256", &[txt("abc")]), vec![Some(txt("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"))]);
    assert_eq!(run("base64", &[txt("foobar")]), vec![Some(txt("Zm9vYmFy"))]);
    assert_eq!(run("hex", &[txt("foobar")]), vec![Some(txt("666f6f626172"))]);
    assert_eq!(run("secure_equals", &[mac.clone(), mac]), vec![Some(Value::Boolean(true))]);
    // And split, which takes a signature header apart.
    assert_eq!(run("split", &[txt("t=1,v1=ab"), txt(",")]), vec![Some(Value::List(vec![txt("t=1"), txt("v1=ab")]))]);
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
        Value::Record(
            [("id".to_string(), Value::Number(1.0)), ("tags".to_string(), Value::List(vec![]))].into(),
        ),
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
    assert_eq!(cat.len(), 71);
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
    assert_eq!(sorted, vec!["add", "and", "concat", "fold", "list", "map", "multiply", "or"]);
    // map's and fold's extra inputs are passed to their function: any type, not more of one port.
    let mut extra = passes_extra_primitives();
    extra.sort();
    assert_eq!(extra, vec!["fold", "map"]);
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
    fn fraction_parity_native() { check_fraction_parity(); }
    #[test]
    fn signature_parity_native() { check_signature_parity(); }
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

    /// A line with characters of two UTF-16 units (an emoji) and one unit past the BMP's
    /// two-byte range (`→`): offsets and the language's columns convert both ways.
    #[test]
    fn text_offsets_are_utf16() {
        use lang::ast::Pos;
        let text = "flow f\nx = \"😀→\" + y\r\nz";
        let at = Offsets::new(text);
        let p = |line, col| Pos { line, col };
        assert_eq!(at.offset(p(1, 1)), 0);
        assert_eq!(at.offset(p(2, 1)), 7);
        // `"` is col 5, the emoji col 6 (2 units), `→` col 7, the closing quote col 8.
        assert_eq!(at.offset(p(2, 7)), 7 + 5 + 2);
        assert_eq!(at.offset(p(2, 8)), 7 + 5 + 3);
        assert_eq!(at.offset(p(2, 99)), 7 + "x = \"😀→\" + y\r".encode_utf16().count(), "past the end is the end");
        for (line, col) in [(1, 1), (1, 4), (2, 6), (2, 7), (2, 8), (2, 12), (3, 1), (3, 2)] {
            assert_eq!(at.pos(at.offset(p(line, col))), p(line, col), "{line}:{col}");
        }
    }
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
    fn fraction_parity_wasm() { check_fraction_parity(); }
    #[wasm_bindgen_test]
    fn signature_parity_wasm() { check_signature_parity(); }
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
        assert_eq!(cat.len(), 71);
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