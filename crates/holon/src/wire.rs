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

/// A JSON mirror of `core::Value`, for this HTTP/JSON boundary — the same reasoning as
/// `WValue` at the wasm boundary (`core::Value` deliberately carries no serde, keeping
/// `holon-core` dependency-minimal). Grounded exactly against the real `Value` enum (seven
/// variants — there is no `Product` at the *value* level, only at the `Type` level, so this
/// mirror doesn't need one either) rather than assumed by symmetry with `Type`. Bytes are padded
/// base64, so a log line holding them stays JSON.
///
/// **A real, named limitation, not silently inherited:** `serde_json` cannot represent `NaN`/
/// `Infinity` (JSON has no token for them) — a value whose `Number` is non-finite (an
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
    /// Bytes, as padded base64: `{"type": "Bytes", "value": "aGk="}`.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
}

/// Bytes in JSON as padded base64 ([`SValue::Bytes`]).
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&holon_core::bytes_text(bytes, "base64").expect("base64 writes any bytes"))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        holon_core::text_bytes(&text, "base64").ok_or_else(|| serde::de::Error::custom("bytes that aren't base64"))
    }
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
            Value::Bytes(b) => SValue::Bytes(b.clone()),
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
            SValue::Bytes(b) => Value::Bytes(b),
        }
    }
}

/// How deep JSON holding values may nest: a value as deep as one may be
/// ([`holon_core::MAX_VALUE_DEPTH`]) takes up to three levels for each of its own as an [`SValue`]
/// (a record's `{"type": "Record", "value": [[key, value]]}`), inside the few of what holds it.
pub(crate) const MAX_NESTING: usize = 3 * holon_core::MAX_VALUE_DEPTH + 16;

/// The stack JSON nested past serde_json's own stop is read on: a level of an `SValue` takes about
/// 2 KB of it in a debug build (600 bytes in release), so [`MAX_NESTING`] fits five times over.
const DEEP_READ_STACK: usize = 8 << 20;

/// JSON holding values, read back: a log line, a waiting run's part, the deployments file, an
/// answer listing runs. serde_json stops at 128 levels of nesting, which a value 42 records deep
/// passes as an [`SValue`]: what Holon wrote, it couldn't read (a log line, and every line after it
/// holding the state the value was stored in). JSON past that stop is read without it once its
/// brackets are counted and found no deeper than [`MAX_NESTING`], on a thread of its own with room
/// for that much, whatever thread asked; deeper JSON isn't read.
pub(crate) fn read_json<T: serde::de::DeserializeOwned + Send>(text: &[u8]) -> serde_json::Result<T> {
    use serde::de::Error;
    let nesting = nesting(text);
    // serde_json reads up to 127 levels itself.
    if nesting < 128 {
        return serde_json::from_slice(text);
    }
    if nesting > MAX_NESTING {
        return Err(serde_json::Error::custom(format!("it nests {nesting} levels deep, past the {MAX_NESTING} a value as deep as one may be takes")));
    }
    std::thread::scope(|s| {
        let read = std::thread::Builder::new().stack_size(DEEP_READ_STACK).spawn_scoped(s, || {
            let mut de = serde_json::Deserializer::from_slice(text);
            de.disable_recursion_limit();
            let read = T::deserialize(&mut de)?;
            de.end()?;
            Ok(read)
        });
        match read {
            Ok(reading) => reading.join().unwrap_or_else(|_| Err(serde_json::Error::custom("reading it panicked"))),
            Err(e) => Err(serde_json::Error::custom(format!("no thread to read it on: {e}"))),
        }
    })
}

/// How deep `text`'s brackets nest, those in strings aside (JSON or not: reading it says).
fn nesting(text: &[u8]) -> usize {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0, false, false);
    for &b in text {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    deepest
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
/// publication, a separate, marketplace-facing concept). Nothing about *which* triggers apply
/// is asserted by the caller; that's read off the unit itself (its handlers). `acknowledge`
/// **is** something the caller must assert — not the capability *set* (still fully derived),
/// but that the caller has actually reviewed it: every capability
/// `capabilities()` derives for this unit — its whole manifest, not just the trigger-relevant
/// ones — must appear in `acknowledge` (by its `{:?}` name, e.g. `"NetworkEgress"`) or the
/// deploy is refused, naming exactly what's missing. `#[serde(default)]` so an omitted field
/// becomes an empty list rather than a JSON parse error — the missing-capability message is a
/// clearer failure than "missing field."
#[derive(Deserialize)]
pub(crate) struct DeployRequestBody {
    pub(crate) slug: String,
    pub(crate) unit: String,
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
    /// The web pages it answers from other sites (CORS): `*` for any, else origins
    /// (`https://example.com`); none by default, as a browser then refuses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) browsers: Vec<String>,
    /// An agent's, when it needs a person's approval and is kept as a proposal: what it changes,
    /// in a line, and more to say (proposals.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// The on-disk, persisted shape of one deployment. `triggers` is still not persisted — it's
/// re-derived from `unit`'s own capabilities on load, the same way a fresh `/deploy` call
/// derives it, rather than storing a fact that's already computable. Tick history still isn't
/// either — diagnostic, not load-bearing; fine to lose on restart. `state` **is** persisted
/// now, though — surviving a restart is the whole reason a deployment gets its own durable
/// state instead of an in-memory-only scratch value. Every new field is `#[serde(default)]`
/// so a deployments file written before this inch still loads without error.
/// What a deployment's log doesn't hold of an unrecorded handler's runs (docs/L4s/request-path-L4.md,
/// C5): how many ran, how many failed, how long they took in all, when the last one started, and
/// what the last failure was. An unrecorded run is never logged, failed or not: its handler was
/// declared so (resilience-audit-L4.md, §2.8). Kept per handler, by its label.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Tally {
    pub(crate) runs: u64,
    pub(crate) failed: u64,
    pub(crate) total_ms: f64,
    pub(crate) last_at_ms: u64,
    /// The newest failure: when, its answer's status, and its error, redacted and cut short. Never
    /// its request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_failure: Option<Failure>,
}

/// A run that failed, as a tally keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Failure {
    pub(crate) at_ms: u64,
    pub(crate) status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

impl Tally {
    /// One more run, started at `at_ms`, taking `ms`; `failure` when it failed.
    pub(crate) fn add(&mut self, at_ms: u64, ms: f64, failure: Option<Failure>) {
        self.runs += 1;
        self.total_ms += ms;
        self.last_at_ms = self.last_at_ms.max(at_ms);
        if let Some(failure) = failure {
            self.failed += 1;
            if self.last_failure.as_ref().is_none_or(|f| f.at_ms <= failure.at_ms) {
                self.last_failure = Some(failure);
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct DeploymentRecord {
    pub(crate) slug: String,
    pub(crate) unit: String,
    #[serde(default)]
    pub(crate) deployed_at_epoch_seconds: Option<u64>,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployed_by: Option<String>,
    #[serde(default)]
    pub(crate) trigger_count: u64,
    #[serde(default)]
    pub(crate) error_count: u64,
    /// Its unrecorded handlers' runs, counted, by label.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tallies: BTreeMap<String, Tally>,
    /// Absent in files written before the lifecycle existed: such a deployment was running.
    #[serde(default)]
    pub(crate) status: Option<Status>,
    #[serde(default)]
    pub(crate) sessions: Vec<Session>,
    #[serde(default)]
    pub(crate) document: Option<serde_json::Value>,
    /// The documents of earlier versions, as files before rc.30 kept them (`{unit, document}`, the
    /// last 10). Read once at load, numbered and completed into `versions_kept`; never written,
    /// so an older release reading a newer file finds no versions rather than failing on them.
    #[serde(default, skip_serializing)]
    pub(crate) versions: Vec<Version>,
    /// Every version deployed, oldest first, the current one last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) versions_kept: Vec<Version>,
    /// What the host granted it. Absent in files written before grants: such a deployment's
    /// capabilities were acknowledged when it was deployed, and it is granted them at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grant: Option<Grant>,
    /// Its runs waiting for an event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) parked: Vec<ParkedRun>,
    /// The web pages it answers from other sites (CORS): `*` for any, else origins
    /// (`https://example.com`); none by default, as a browser then refuses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) browsers: Vec<String>,
    /// The saved flows its version places as nodes (`FlowRecord::uses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
    /// A person turned off agents proposing its versions (proposals.rs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) agents_propose_off: bool,
}

#[derive(Serialize)]
pub(crate) struct DeployResponseBody {
    pub(crate) slug: String,
    pub(crate) unit: String,
    /// What starts its runs, as its server declares them: `Http :8080`, `every 5s`, `action reset`.
    pub(crate) triggers: Vec<String>,
    /// The version this deploy made (`GET /deployments/<slug>/versions`).
    pub(crate) version: u32,
}

/// One route of a server deployment, in its listing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RouteEntry {
    pub(crate) method: String,
    /// As declared, e.g. `/todos/:id`.
    pub(crate) path: String,
    /// `strict`, `lazy` or `unrecorded`.
    pub(crate) log: String,
}

#[derive(Serialize)]
pub(crate) struct DeploymentEntry {
    pub(crate) slug: String,
    pub(crate) unit: String,
    pub(crate) triggers: Vec<String>,
    /// Whether this deployment's own derived capabilities include a state axis — `true` means
    /// it carries a real, persisted `state` slot; `false` means it's stateless and `state`
    /// below is always `null`. The actual state *values* aren't exposed here yet (a real,
    /// named, not-yet-built follow-on, the same way tick history got its own endpoint rather
    /// than riding the deployment listing).
    pub(crate) has_state: bool,
    /// The port a server deployment answers on; `None` for any other deployment.
    pub(crate) port: Option<u16>,
    /// A server's routes, as declared, each with how its runs are recorded as granted (an
    /// unrecorded route its grant doesn't keep out of the log is strict): none for any other
    /// deployment. What `holon load` asks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) routes: Vec<RouteEntry>,
    pub(crate) status: Status,
    /// The current session's number, or the last one's when stopped.
    pub(crate) session: u32,
    pub(crate) sessions: usize,
    pub(crate) trigger_count: u64,
    pub(crate) error_count: u64,
    /// Its unrecorded handlers' runs, counted, by label: none while it has none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tallies: BTreeMap<String, Tally>,
    pub(crate) deployed_at_epoch_seconds: u64,
    /// Who did it (access.rs's caller: a named user, `admin`), when the server knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployed_by: Option<String>,
    /// The saved flows its version places as nodes (`FlowRecord::uses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
    /// The web pages it answers from other sites (CORS): `*` for any, else origins
    /// (`https://example.com`); none by default, as a browser then refuses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) browsers: Vec<String>,
    /// Why it can't run until deployed again, if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) attention: Option<String>,
    /// Why it isn't answering on its port (it couldn't listen there), if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) listen_error: Option<String>,
    /// How many of its runs are waiting for an event.
    pub(crate) waiting: usize,
    /// Whether agents may propose its versions (proposals.rs).
    pub(crate) agents_propose: bool,
    /// The number of the version deployed now (`v3`), as `/versions` counts them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<u32>,
}

/// One clock tick's result, kept in a deployment's bounded history. `outputs` is every settled
/// slot the flow declared, not just one.
#[derive(Serialize, Clone)]
pub(crate) struct TickEntry {
    pub(crate) at_epoch_seconds: u64,
    /// Which of its server's clocks ticked (`every 2s`); absent for a flow on an interval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
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
    holon_core::hex(h)
}

/// One deploy of a deployment, kept for good (docs/L4s/history-L4.md): what it deployed, when,
/// by whom, and what its deploy asked, so its runs show on their own canvas and the version can be
/// deployed again (`POST /deployments/<slug>/restore`). The last is the current one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Version {
    /// 1 for its first deploy, then counting up; never reused, a restore included.
    #[serde(default)]
    pub(crate) number: u32,
    /// Its unit (hex), as its log entries name it.
    pub(crate) unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deployed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<String>,
    /// The version it deployed again, when it was a restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) restored_from: Option<u32>,
    /// What its deploy asked beside the unit, so restoring it asks the same. Absent for versions
    /// deployed before Holon kept it (rc.29 and earlier): they open, but don't restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request: Option<VersionRequest>,
    /// Its flow document (canvas), by the hash of a text value holding its JSON in the content
    /// store, so the deployments file stays small however many versions it keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) document_ref: Option<String>,
    /// The state it carried when deployed, as a snapshot's hash (`restore::save_snapshot`): the
    /// state the version before it left, which restoring that one's state puts back. Absent for a
    /// deployment without state, and for versions deployed before rc.30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state_at_deploy: Option<String>,
    /// The seq its deploy's next run took in the log: where the runs since it start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_seq: Option<u64>,
    /// The document inline, as files kept it before `document_ref`. Read once at load, when it
    /// moves into the store; never written.
    #[serde(default, skip_serializing)]
    pub(crate) document: Option<serde_json::Value>,
}

/// What a deploy asked beside its unit and document: what deploying it again asks.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct VersionRequest {
    #[serde(default)]
    pub(crate) requirements: Requirements,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<HolonUse>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) browsers: Vec<String>,
}

/// `POST /deployments/<slug>/restore`'s body: one point to go back to, a version or a run, and
/// what of it to restore.
#[derive(Deserialize)]
pub(crate) struct RestoreRequestBody {
    /// A version, by its number: its code, and the state it left (the next version's
    /// `state_at_deploy`).
    #[serde(default)]
    pub(crate) version: Option<u32>,
    /// A run, by its seq in the log: the state it left (`state_after`), and the code it ran on.
    #[serde(default)]
    pub(crate) run: Option<u64>,
    /// Its code, deployed again as a new version.
    #[serde(default)]
    pub(crate) code: bool,
    /// Its state, put back as the live state, in a new session.
    #[serde(default)]
    pub(crate) state: bool,
    /// What the version needs that today's grant doesn't cover, approved, as a deploy's.
    #[serde(default)]
    pub(crate) acknowledge: Vec<String>,
    /// What it would ask, without deploying anything.
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value as deep as one may be, as an `SValue` (a record three levels of JSON for each of
    /// its own), reads back past serde_json's own stop. JSON nested deeper isn't read: its brackets
    /// are counted before anything recurses (100,000 deep would overflow the reader's stack).
    /// Brackets in a string don't count.
    /// Bytes in the log's JSON are padded base64, and read back as they went in; text that isn't
    /// base64 isn't read.
    #[test]
    fn bytes_are_base64_in_json_and_read_back() {
        let v = Value::Bytes(vec![0xfb, 0xff, 0x00]);
        let text = serde_json::to_string(&SValue::from_core(&v)).unwrap();
        assert_eq!(text, r#"{"type":"Bytes","value":"+/8A"}"#);
        assert_eq!(read_json::<SValue>(text.as_bytes()).unwrap().into_core(), v);
        assert!(read_json::<SValue>(br#"{"type":"Bytes","value":"not base64!"}"#).is_err());
    }

    #[test]
    fn json_holding_a_value_as_deep_as_one_may_be_reads_back() {
        let nested = |depth: usize, record: bool| {
            (0..depth).fold(Value::Number(1.0), |v, _| if record { Value::Record([("k".to_string(), v)].into_iter().collect()) } else { Value::List(vec![v]) })
        };
        for record in [false, true] {
            for depth in [0, 41, 42, 64, holon_core::MAX_VALUE_DEPTH] {
                let v = nested(depth, record);
                let text = serde_json::to_vec(&vec![SValue::from_core(&v)]).unwrap();
                let back: Vec<SValue> = read_json(&text).unwrap_or_else(|e| panic!("{depth} deep: {e}"));
                assert_eq!(back.into_iter().next().map(SValue::into_core), Some(v), "{depth} deep");
            }
        }
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let e = read_json::<serde_json::Value>(deep.as_bytes()).unwrap_err();
        assert!(e.to_string().contains("100000 levels deep"), "{e}");
        let quoted = format!("[\"\\\"{}\"]", "[".repeat(1000));
        assert_eq!(read_json::<serde_json::Value>(quoted.as_bytes()).unwrap()[0].as_str().map(str::len), Some(1001));
    }
}
