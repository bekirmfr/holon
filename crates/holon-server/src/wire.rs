//! The control plane's JSON: request and response bodies, and `SValue`, a `Value` as JSON.

use crate::*;

#[derive(Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) error: String,
}

#[derive(Serialize)]
pub(crate) struct HashBody {
    pub(crate) hash: String,
}

#[derive(Serialize)]
pub(crate) struct RegistryBody {
    pub(crate) published: Vec<String>,
}

/// A JSON mirror of `core::Value`, for this HTTP/JSON boundary — the same reasoning as
/// `WValue` at the wasm boundary (`core::Value` deliberately carries no serde, keeping
/// `holon-core` dependency-minimal). Grounded exactly against the real `Value` enum (seven
/// variants — there is no `Product` at the *value* level, only at the `Type` level, so this
/// mirror doesn't need one either) rather than assumed by symmetry with `Type`.
///
/// **A real, named limitation, not silently inherited:** `serde_json` cannot represent `NaN`/
/// `Infinity` (JSON has no token for them) — a cache entry whose `Number` is non-finite (an
/// overflow result) will fail to serialize back to the client. The wasm bridge sidesteps this
/// entirely by using `serde-wasm-bindgen` instead of JSON, since JS numbers carry non-finite
/// values natively; a plain-JSON HTTP boundary doesn't have that option. Not fixed here —
/// naming it is the honest move until it's a real, hit problem, not a hypothetical one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "value")]
pub(crate) enum SValue {
    Number(f64),
    Boolean(bool),
    Text(String),
    List(Vec<SValue>),
    Effect { sink: String, payload: Box<SValue> },
    Sum { arm: usize, payload: Box<SValue> },
    /// `[key, value]` pairs in key order — the same shape as the wasm bridge's `WValue::Record`,
    /// so the editor reads both boundaries with one type.
    Record(Vec<(String, SValue)>),
}

impl SValue {
    pub(crate) fn from_core(v: &Value) -> SValue {
        match v {
            Value::Number(n) => SValue::Number(*n),
            Value::Boolean(b) => SValue::Boolean(*b),
            Value::Text(s) => SValue::Text(s.clone()),
            Value::List(items) => SValue::List(items.iter().map(SValue::from_core).collect()),
            Value::Effect { sink, payload } => {
                SValue::Effect { sink: sink.clone(), payload: Box::new(SValue::from_core(payload)) }
            }
            Value::Sum { arm, payload } => {
                SValue::Sum { arm: *arm, payload: Box::new(SValue::from_core(payload)) }
            }
            Value::Record(fields) => {
                SValue::Record(fields.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect())
            }
        }
    }
    pub(crate) fn into_core(self) -> Value {
        match self {
            SValue::Number(n) => Value::Number(n),
            SValue::Boolean(b) => Value::Boolean(b),
            SValue::Text(s) => Value::Text(s),
            SValue::List(items) => Value::List(items.into_iter().map(SValue::into_core).collect()),
            SValue::Effect { sink, payload } => {
                Value::Effect { sink, payload: Box::new(payload.into_core()) }
            }
            SValue::Sum { arm, payload } => Value::Sum { arm, payload: Box::new(payload.into_core()) },
            SValue::Record(fields) => Value::Record(fields.into_iter().map(|(k, v)| (k, v.into_core())).collect()),
        }
    }
}

/// Wire shape of a `CacheEntry` for `POST /cache/ingest` — `unit_hash` as hex (the one
/// convention every hash already crosses this boundary as), `inputs`/`outputs` as `SValue`.
#[derive(Deserialize)]
pub(crate) struct SCacheEntry {
    pub(crate) unit_hash: String,
    pub(crate) inputs: Vec<Option<SValue>>,
    pub(crate) outputs: Vec<Option<SValue>>,
}

#[derive(Serialize)]
pub(crate) struct CacheOutputsBody {
    pub(crate) outputs: Vec<Option<SValue>>,
}

/// `POST /fire`'s request body — just the inputs, positional, same convention `eval` already
/// uses everywhere (index = input-port index, `null` = absent). The root itself is named by
/// the `?root=<hex>` query param (it must already be published/staged — see the handler doc),
/// not re-sent in the body.
#[derive(Deserialize)]
pub(crate) struct FireRequestBody {
    pub(crate) inputs: Vec<Option<SValue>>,
}

/// One `(capability, payload)` pair `RecordingHost` recorded instead of actually performing.
/// `capability` is stringified via `{c:?}` (`Debug`) at the point a response is built, not
/// when `RecordingHost` records it — `RecordingHost.performed` keeps the real `Capability`
/// (needed by `fire_with_state`, which has to tell `StateWrite(key)` apart from everything
/// else that got performed) and only becomes a plain name here, at the wire boundary. One
/// fewer place a new `Capability` variant could be added upstream and silently never reach
/// this mirror. `Clone` so a `Deployment`'s cached `TickEntry` history can be cloned out for a
/// response without holding the deployments lock across the JSON serialization.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct PerformedEntry {
    pub(crate) capability: String,
    pub(crate) payload: SValue,
}

/// `POST /fire` and `POST /trigger/<slug>`'s response shape. `outputs` is every settled slot
/// the fired unit declared — not just one. A single-output unit still gets a one-element
/// array; nothing about the common case changes shape awkwardly. A two-output unit (a text
/// response and a state-write effect, left as two separate exposed outputs rather than
/// combined into one artificial `list(...)`) now shows both, cleanly, instead of needing to
/// be merged into something that echoed the effect twice.
#[derive(Serialize)]
pub(crate) struct FireResponseBody {
    pub(crate) outputs: Vec<Option<SValue>>,
    pub(crate) performed: Vec<PerformedEntry>,
}

/// One saved flow — a named workspace document (see `AppState::flows` for why this is
/// deliberately not a content-addressed unit). `document` is opaque JSON: node positions,
/// viewport, whatever the editor chooses to keep — the server never reads inside it.
/// `Serialize`+`Deserialize` because this exact shape is also the on-disk persistence format
/// (one JSON array of these, same discipline as `DeploymentRecord`).
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct FlowRecord {
    pub(crate) name: String,
    pub(crate) document: serde_json::Value,
    pub(crate) saved_at_epoch_seconds: u64,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) saved_by: Option<String>,
    /// The saved flows it places as nodes, and which save of each (the editor says so beside
    /// the document, which stays opaque). Absent when saved before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
}

/// A saved flow placed as a node in another flow or deployment: its name, and which save of it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct HolonUse {
    pub(crate) flow: String,
    pub(crate) saved_at: u64,
}

/// `POST /flows`' request body: upsert by name. The same name saves over the previous
/// version — a flow is a mutable workspace file, not an immutable identity; "save" means
/// exactly what it means in any editor.
#[derive(Deserialize)]
pub(crate) struct SaveFlowBody {
    pub(crate) name: String,
    pub(crate) document: serde_json::Value,
    #[serde(default)]
    pub(crate) uses: Option<Vec<HolonUse>>,
}

/// `GET /flows`' per-entry listing shape — name and freshness only, never the (possibly
/// large) document itself; that's what `GET /flows/<name>` is for.
#[derive(Serialize)]
pub(crate) struct FlowListEntry {
    pub(crate) name: String,
    pub(crate) saved_at_epoch_seconds: u64,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) saved_by: Option<String>,
    /// The saved flows it places (`FlowRecord::uses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
}

/// `POST /deploy`'s request body. `unit` must already be resolvable in the store (staged via
/// `/unit`, or published — see the L4's Fork 1: deploy deliberately does not require
/// publication, a separate, marketplace-facing concept). `interval_seconds` is **only**
/// consulted (and required) if the unit's own derived capabilities include `Clock` — nothing
/// about *which* trigger(s) apply is asserted by the caller; that's read off the unit itself.
/// `acknowledge` **is** something the caller must assert — not the capability *set* (still
/// fully derived), but that the caller has actually reviewed it: every capability
/// `capabilities()` derives for this unit — its whole manifest, not just the trigger-relevant
/// ones — must appear in `acknowledge` (by its `{:?}` name, e.g. `"NetworkEgress"`) or the
/// deploy is refused, naming exactly what's missing. `#[serde(default)]` so an omitted field
/// becomes an empty list rather than a JSON parse error — the missing-capability message is a
/// clearer failure than "missing field."
#[derive(Deserialize)]
pub(crate) struct DeployRequestBody {
    pub(crate) slug: String,
    pub(crate) unit: String,
    pub(crate) interval_seconds: Option<u64>,
    #[serde(default)]
    pub(crate) acknowledge: Vec<String>,
    /// The flow document (canvas) being deployed, for view mode. Optional.
    #[serde(default)]
    pub(crate) document: Option<serde_json::Value>,
    /// What it needs beyond its graph: limits, exceptions (permissions.rs).
    #[serde(default)]
    pub(crate) requirements: Requirements,
    #[serde(default)]
    pub(crate) uses: Option<Vec<HolonUse>>,
}

/// The on-disk, persisted shape of one deployment. `triggers` is still not persisted — it's
/// re-derived from `unit`'s own capabilities on load, the same way a fresh `/deploy` call
/// derives it, rather than storing a fact that's already computable. Tick history still isn't
/// either — diagnostic, not load-bearing; fine to lose on restart. `state` **is** persisted
/// now, though — surviving a restart is the whole reason a deployment gets its own durable
/// state instead of an in-memory-only scratch value. Every new field is `#[serde(default)]`
/// so a deployments file written before this inch still loads without error.
#[derive(Serialize, Deserialize)]
pub(crate) struct DeploymentRecord {
    pub(crate) slug: String,
    pub(crate) unit: String,
    pub(crate) interval_seconds: Option<u64>,
    /// The state by reference: each key's value hash (hex), the values in the content store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state_refs: Option<BTreeMap<String, String>>,
    /// The state as files before `state_refs` held it, values inline. Read once at load, when
    /// the values move into the store; never written.
    #[serde(default, skip_serializing)]
    pub(crate) state: Option<BTreeMap<String, SValue>>,
    #[serde(default)]
    pub(crate) deployed_at_epoch_seconds: Option<u64>,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployed_by: Option<String>,
    #[serde(default)]
    pub(crate) trigger_count: u64,
    #[serde(default)]
    pub(crate) error_count: u64,
    /// Absent in files written before the lifecycle existed: such a deployment was running.
    #[serde(default)]
    pub(crate) status: Option<Status>,
    #[serde(default)]
    pub(crate) sessions: Vec<Session>,
    #[serde(default)]
    pub(crate) document: Option<serde_json::Value>,
    /// The documents of earlier versions, by unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) versions: Vec<KeptDocument>,
    /// What the host granted it. Absent in files written before grants: such a deployment's
    /// capabilities were acknowledged when it was deployed, and it is granted them at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grant: Option<Grant>,
    /// Its runs waiting for an event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) parked: Vec<ParkedRun>,
    /// The saved flows its version places as nodes (`FlowRecord::uses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
}

#[derive(Serialize)]
pub(crate) struct DeployResponseBody {
    pub(crate) slug: String,
    pub(crate) unit: String,
    /// What was actually derived from the unit's own capabilities — `"Ingress"`, `"Clock"`,
    /// both, or (refused before reaching this response) neither.
    pub(crate) triggers: Vec<String>,
    pub(crate) interval_seconds: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct DeploymentEntry {
    pub(crate) slug: String,
    pub(crate) unit: String,
    pub(crate) triggers: Vec<String>,
    pub(crate) interval_seconds: Option<u64>,
    /// Whether this deployment's own derived capabilities include a state axis — `true` means
    /// it carries a real, persisted `state` slot; `false` means it's stateless and `state`
    /// below is always `null`. The actual state *values* aren't exposed here yet (a real,
    /// named, not-yet-built follow-on, the same way tick history got its own endpoint rather
    /// than riding the deployment listing).
    pub(crate) has_state: bool,
    /// The port a server deployment answers on; `None` for any other deployment.
    pub(crate) port: Option<u16>,
    pub(crate) status: Status,
    /// The current session's number, or the last one's when stopped.
    pub(crate) session: u32,
    pub(crate) sessions: usize,
    pub(crate) trigger_count: u64,
    pub(crate) error_count: u64,
    pub(crate) deployed_at_epoch_seconds: u64,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployed_by: Option<String>,
    /// The saved flows its version places as nodes (`FlowRecord::uses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
    /// Why it can't run until deployed again, if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) attention: Option<String>,
    /// How many of its runs are waiting for an event.
    pub(crate) waiting: usize,
}

/// One clock tick's result, kept in a deployment's bounded history. `outputs`: same shape
/// change as `FireResponseBody`, same reason — every settled slot, not just one.
#[derive(Serialize, Clone)]
pub(crate) struct TickEntry {
    pub(crate) at_epoch_seconds: u64,
    pub(crate) outputs: Vec<Option<SValue>>,
    pub(crate) performed: Vec<PerformedEntry>,
}

#[derive(Serialize)]
pub(crate) struct HistoryBody {
    pub(crate) slug: String,
    pub(crate) ticks: Vec<TickEntry>,
}

/// `GET /deployments/<slug>/state`'s response — the deployment's current keyed state, in
/// full. Real observability, not just a debugging convenience: without this, there is no way
/// to confirm a write actually persisted between two fires except trusting a flow's own
/// self-report, which a `store_read → add → store_write` bootstrap can't even provide on its
/// first fire (see `test_stateful_writer`'s own doc). Named, not built speculatively — a
/// genuine testing and real-world need surfaced this, not spec-writing ahead of a use case.
#[derive(Serialize)]
pub(crate) struct StateBody {
    pub(crate) slug: String,
    pub(crate) state: BTreeMap<String, SValue>,
    /// The state's Merkle root (hex): equal for equal states.
    pub(crate) root: String,
}

#[derive(Serialize)]
pub(crate) struct DeploymentsBody {
    pub(crate) deployments: Vec<DeploymentEntry>,
}

pub(crate) fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// An earlier version's flow document, kept so its runs show on their own canvas.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct KeptDocument {
    /// The version's unit (hex), as its log entries name it.
    pub(crate) unit: String,
    pub(crate) document: serde_json::Value,
}
