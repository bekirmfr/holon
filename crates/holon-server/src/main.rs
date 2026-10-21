//! holon-server — the browser↔host transport (P4.5.1 / P4.6.1's blocker), local-first.
//!
//! Grounded and scoped in `docs/L4s/P4.5-P4.6-transport-scoping-L4.md`. The load-bearing
//! finding from that note, worth repeating here since it's *why* this file is this thin: a
//! network boundary's security is already fully provided by content-addressing
//! (`decode_verified` inside every `Store::get`, `entry_key`/`ingest`, `verify_entry`). This
//! server adds no trust logic of its own — every endpoint is a direct call into `holon-host`
//! code that already exists and is already tested. It is plumbing, not a second decider.
//!
//! **Inch 1** (prior commit): the crate scaffold, CORS, `GET`/`POST /unit` wrapping `Store`.
//! **Inch 2** (prior commit): `POST /registry/publish` + `GET /registry`, wrapping `Registry`.
//! **Inch 3** (prior commit): `POST /registry/fork`, wrapping `Registry::fork`.
//! **Inch 4** (prior commit): `GET /cache/<key>` + `POST /cache/ingest`, wrapping `SharedCache`.
//! **Inch 5** (prior commit): `POST /fire`, wrapping `fire` — settles an already-published unit
//! **once**, on request, via a `RecordingHost` that reports what it would perform rather than
//! actually performing it (see the handler doc for why). This closed this crate's initial
//! buildout: `/unit`, `/registry/*`, `/cache/*`, `/fire`.
//!
//! **Inch 6** (prior commit, `ingress-deploy-L4.md`): the first slice of the standing-deploy
//! pillar — `POST /deploy`, `GET /deployments`, `POST /trigger/<slug>`. Deliberately the small
//! half: nothing there needed a single new mechanism from `holon-core`/`holon-host`
//! (`fire_sourced` and `SourcedResolver` already existed, already tested) — the only genuinely
//! new thing was a `slug → unit hash` table and one new route.
//!
//! **Inch 7** (prior commit, `clock-deploy-L4.md`): the other half — `Clock`-bound deployment.
//! `spawn_clock_scheduler` is the first genuinely new runtime shape anywhere in this project: a
//! background thread that fires on its own, on a schedule, with nobody asking. Reworked
//! `/deploy` along the way (worth naming plainly, not burying): rather than the caller
//! asserting a trigger *type*, `/deploy` now **derives** it from the unit's own
//! `capabilities()` — the same closure-union already used for capability-consent elsewhere in
//! this project. A caller only supplies what genuinely can't be read off the graph
//! (`interval_seconds`, only when the derived capabilities include `Clock`). This correction
//! came from a real design conversation mid-session, not a solo decision — worth recording
//! that the first draft (an explicit `trigger` field the caller asserted) was the wrong shape,
//! caught before it shipped.
//!
//! **A design question worth recording, not just the answer:** should `ingress` let a unit
//! declare the specific format it expects? No — `ingress`'s own genesis interface is
//! deliberately `Any`-typed, and a per-instance declared format would be a **self-asserted**
//! claim on a primitive, cutting against this project's own "derived, not asserted" invariant
//! (kind, capabilities, and interfaces are all computed from structure, never declared by a
//! unit about itself). The tool for "this might not be the expected shape" already exists one
//! tier out — `Sum`/`some`/`none` (this project's `Option<T>`) plus `select`, composed *inside*
//! the flow right after `ingress`, not baked into the primitive every `ingress`-using flow
//! would then have to accommodate.
//!
//! **Inch 8** (prior commit): real integration tests — 17, in-process, via
//! `rouille::Request::fake_http` calling `route()`/`with_cors()` directly, replacing the manual
//! `curl` walkthroughs used to verify inches 1-7. Prompted by a direct, fair challenge partway
//! through this session, not a self-initiated cleanup — worth recording that plainly.
//! `run_due_clock_ticks` (see inch 7) exists specifically so the scheduler could be tested with
//! a controlled `SystemTime` rather than real sleeping.
//!
//! **Inch 9** (prior commit): `DELETE /deployments/<slug>` — closes a gap named plainly at
//! inch 7 (`clock-deploy-L4.md`'s Fork 4) and left open on purpose: until now, the only way to
//! stop a `Clock`-triggered deployment was killing the whole server. Removing the slug from
//! the table is sufficient — the scheduler only ever acts on what's currently there.
//!
//! **Inch 10** (prior commit): three things at once, each real but each contained —
//! **persistence** (`--deployments <path>`, write-through, mirroring `--store`'s own
//! `:memory:` convention; only the deployment *definition* persists, not the derived
//! `triggers` or the diagnostic tick history — re-derived/reset on load, the same discipline
//! as everywhere else a fact stays computed rather than cached to disk); **capability-consent
//! before deploy** (`/deploy`'s `acknowledge` field — the caller asserts *review*, never the
//! capability *set itself*, which stays fully derived via `capabilities()`; covers the whole
//! manifest, not just the trigger-relevant capabilities); and **concurrent per-tick firing**
//! (`run_due_clock_ticks` now fans out via `thread::scope`, so two deployments due in the same
//! moment no longer block each other — deliberately not a bounded worker pool, a real,
//! consciously-left-open limitation for genuinely large simultaneous fan-out, not solved here).
//!
//! **Inch 11** (this one): a deployment carrying `StateRead`/`StateWrite` (see the
//! keyed-state-capability inch, `holon-core`/`holon-host`) now genuinely remembers across
//! fires — both `/trigger` and clock ticks. A real correction on the way here, worth keeping
//! on record: the first plan routed this through `holon-host`'s own `StateHost::round`, and a
//! widening of its return type was even proposed. Tracing `round()`'s actual body found that
//! plan wrong, not just incomplete — it only ever keeps `StateWrite` from what gets performed
//! and silently discards everything else, which would mean a state-bearing deployment that
//! also sends an egress loses that egress with no error. The real fix needed nothing from
//! `holon-core`/`holon-host` at all: `fire_with_state` does the same three-step round
//! (snapshot → fire → apply) directly in this crate, using `RecordingHost` (which already
//! captures everything) instead of `StateHost` (which deliberately doesn't).
//!
//! A second real correctness question surfaced and got solved properly, not deferred: holding
//! the whole deployments-table lock across an entire fire would reintroduce the "one slow flow
//! blocks every other deployment" problem inch 10's concurrent tick-firing was built to avoid
//! — but *not* holding a lock across the whole round reopens the lost-update race between two
//! near-simultaneous triggers of the *same* deployment, flagged and resolved earlier in this
//! project's own history. `Deployment.state` is `Arc<Mutex<...>>`, not a bare map: the table
//! lock is only ever held briefly (to clone the `Arc` out), and the *inner* mutex is what's
//! held for the whole round — so two different deployments never contend, and two triggers of
//! the same deployment correctly serialize instead of racing.
//!
//! Also added, unconditionally on every deployment (not gated on state at all):
//! `trigger_count`/`error_count`/`deployed_at`, all persisted, all visible on
//! `GET /deployments`. `error_count` counts infrastructure failures (the closure didn't
//! resolve) — a settle that honestly evaluates to nothing is not an error, the same "this is a
//! normal, expected outcome" stance this whole project takes everywhere else.
//!
//! **Inch 12**: saved flows — `POST/GET /flows`, `GET/DELETE /flows/<name>`. A flow is a named
//! *workspace document*: the editor's canvas exactly as it was left (node positions, viewport,
//! UI settings), saved and re-opened by name. Deliberately a separate concept from a published
//! unit, not a variant of one: a unit's identity is content-addressed and presentation-free by
//! design (`child_order_is_cosmetic` et al.), while a flow document is *all* presentation —
//! merging them would put pixels into hashes. The server treats the document as opaque JSON
//! (name validated by the same `is_valid_slug` rule deployments use; body validated only as
//! JSON) and persists write-through to `--flows <path>` (default `.holon-flows.json`,
//! `":memory:"` to opt out) — the identical discipline `deployments` already follows.
//!
//! **A second real wrinkle, same family as inch 2's:** `SharedCache::get_verified` needs a
//! `Resolver` (it re-executes the cached computation to check the claimed output), but `Store`
//! only hands back owned units, not a `Resolver`. `Registry::fork` faces the identical problem
//! internally (its own doc: "materialize the root's bounded closure into one [resolver]") — so
//! this inch reuses that shape rather than inventing a second one: `materialize_closure` walks
//! a unit's closure via `child_refs` (the same primitive `Registry::publish`'s own dangling-
//! reference check already uses) and populates a fresh `MapResolver`, once per request. `SharedCache`
//! itself is also in-memory only, same as `Registry`'s published set — restarting the server
//! forgets ingested/cached entries too, worth restating rather than assuming obvious.
//!
//! **A real wrinkle this inch, resolved without touching `holon-host`:** `Registry::new` takes
//! ownership of its `Store` and only exposes it back read-only (`Registry::store() -> &S`), but
//! `POST /unit` still needs direct write access for staging children bottom-up before a
//! parent's `publish`. Rather than adding a `store_mut()` to `Registry` (a change to the spine
//! crate, not this transport crate), both the raw `/unit` handlers and the `Registry` hold
//! their own cheap clone of one shared handle (`SharedStore`) onto the same physical store —
//! so staging and publishing act on the same data without `holon-host` changing at all.
//!
//! **`Registry`'s own documented scope, worth restating plainly, not silently inherited:** the
//! published-hash bookkeeping is in-memory only (`Registry`'s own doc: "persisting the record
//! is a short follow-on," not yet built). Restarting this server forgets which hashes were
//! published — the units themselves stay on disk in `FileStore`, only the "is it published"
//! flag is lost. Not a bug in this inch; a pre-existing, named scope boundary of `Registry`.
//!
//! Localhost-only by design (the L4's Fork 1 resolution): binds `127.0.0.1`, and CORS is
//! scoped to `localhost`/`127.0.0.1` origins only — never `*`. CORS alone never stopped a
//! request from *running*, though, so every request first passes `reject_untrusted` (Host,
//! Origin, Content-Type) and bodies are capped at `MAX_BODY_BYTES`. No auth: there is no "who" to
//! gatekeep on a single local process, and content-addressing already answers "can I trust
//! what I got back," which is the only question a stranger-facing boundary would have needed
//! auth *for*.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use holon_core::{
    capabilities, child_refs, decode_preimage, encode_preimage, hash_unit, parse_hash,
    Capability, CacheEntry, Edge, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody,
    Value, BOUNDARY,
};
use holon_host::{
    fire, fire_sourced, is_storable, write_atomically, CacheIngestError, FileStore, ForkError, Host, MemStore,
    PublishError, Registry, SharedCache, Store, StoreError,
};
use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

/// Any concrete `Store` backend, boxed so the server picks one at startup without the rest of
/// the file branching on which. `Store`'s own trait shape (no generics, no `Self`-returning
/// methods) is already object-safe — this isn't a new abstraction, just `dyn` over the one
/// that exists.
type DynStore = Box<dyn Store + Send>;

/// A cheap-to-clone handle onto one shared `DynStore`. Implements `Store` itself by locking and
/// delegating, so it can be handed to `Registry::new` (which wants an owned `S: Store`) while a
/// second clone stays with the server for direct `/unit` reads/writes — both clones lock the
/// same underlying `Mutex`, so they see the same data. See the module doc for why this exists
/// instead of a `holon-host` change.
#[derive(Clone)]
struct SharedStore(Arc<Mutex<DynStore>>);

impl Store for SharedStore {
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError> {
        self.0.lock().unwrap().put(unit)
    }
    fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError> {
        self.0.lock().unwrap().get(key)
    }
}

/// Everything a request handler needs. `registry`, `cache`, and `deployments` are each their
/// own `Mutex` (not folded into `SharedStore`'s locking) since each needs its own lock scope
/// distinct from a single store put/get.
struct AppState {
    store: SharedStore,
    registry: Mutex<Registry<SharedStore>>,
    cache: Mutex<SharedCache>,
    /// slug → deployment. `Arc` (not just `Mutex`) because the clock scheduler thread
    /// (`spawn_clock_scheduler`) needs its own independent handle onto the same table, cloned
    /// before `AppState` is built — the same `Send`-shared-handle shape `SharedStore` already
    /// uses for the store itself. `Registry`'s published set and `SharedCache` stay in-memory
    /// only; `deployments` itself is now persisted write-through to `deployments_path` (see
    /// `persist_deployments`/`load_deployments`) — a deployment definition surviving a restart
    /// is the whole point of "standing," unlike a cache entry or a publish record.
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    /// Where `deployments` is persisted, or the literal `":memory:"` sentinel (matching
    /// `--store`'s own convention) to opt out entirely.
    deployments_path: String,
    /// name → saved flow document. A flow is a *workspace file* — the editor's canvas exactly
    /// as it was left (node positions, viewport, UI settings), saved and re-opened by name —
    /// deliberately NOT a content-addressed unit: positions and zoom are presentation, and the
    /// whole hashing design exists to keep presentation out of identity (child_order /
    /// param_order are cosmetic). The two stay separate: flows are mutable named documents;
    /// projecting/publishing/deploying stays the existing content-addressed path, untouched.
    /// The document itself is opaque `serde_json::Value` — the server validates the name and
    /// that the body parses, nothing more; its internal shape is the editor's contract with
    /// itself, and teaching the server that shape would couple it to a presentation format for
    /// zero gain. Plain `Mutex`, no `Arc` — deployments needed a second handle for the clock
    /// scheduler thread; nothing but HTTP handlers ever touches flows.
    flows: Mutex<BTreeMap<String, FlowRecord>>,
    /// Where `flows` is persisted (write-through, same discipline as `deployments_path`), or
    /// `":memory:"` to opt out.
    flows_path: String,
    /// The port this server listens on — the one a request's `Host` must name
    /// (`reject_untrusted`).
    port: u16,
}

/// One deployment's keyed state, shared between its fires and persistence (see `Deployment::state`).
type DeploymentState = Arc<Mutex<BTreeMap<String, Value>>>;

/// A registered deployment. `triggers` is derived **once**, at deploy time, from the unit's
/// own capability closure (`capabilities()`) — safe to cache rather than re-derive on every
/// `GET /deployments`, since a unit's hash is its whole identity: the same hash can never later
/// have different capabilities. `next_due`/`history` are `Clock`-only; `None`/empty for a
/// purely `Ingress`-triggered deployment.
struct Deployment {
    unit: Hash,
    triggers: Vec<String>,
    interval_seconds: Option<u64>,
    next_due: Option<SystemTime>,
    /// A bounded ring buffer (last 20) of clock ticks — see the L4's Fork 2: logged to stdout
    /// *and* kept here, retrievable via `GET /deployments/<slug>/history`, so a standing
    /// deployment is checkable after the fact, not just while someone happens to be watching
    /// the console.
    history: Vec<TickEntry>,
    /// This deployment's own keyed state, present iff its derived capabilities include
    /// `StateRead`/`StateWrite` — `None` for a capability-free deployment, nothing to
    /// allocate or thread. `Arc<Mutex<...>>`, not a bare map: a fire needs to lock this
    /// specific deployment's state for the *whole* round (read snapshot → fire → apply
    /// write) to stay correct under two near-simultaneous triggers of the *same* slug —
    /// clone-then-overwrite would lose whichever update finished last. Cloning the `Arc` out
    /// under a brief table lock, then locking just this mutex for the round itself, means the
    /// table lock is never held during a fire at all, so one deployment's slow state round
    /// still never blocks any other deployment — only two triggers of the *same* deployment
    /// ever wait on each other, which is the actual granularity of the contention. A freshly
    /// deployed slug starts with an empty map; a redeploy keeps the existing one (see
    /// `handle_post_deploy`). A `store_read` for a key that's never been
    /// written simply evaluates to nothing, the same honest-absence behavior any other
    /// unsupplied sourced capability already has (see `fire_with_state`) — no special-casing
    /// needed for "first ever fire." Persisted write-through alongside the rest of
    /// `Deployment`, since surviving a restart is the whole point of giving a flow its own
    /// durable state.
    state: Option<DeploymentState>,
    /// Every trigger attempt for this deployment, success or failure — ingress calls and
    /// clock ticks alike.
    trigger_count: u64,
    /// Trigger attempts that failed at the infrastructure level (the unit's closure didn't
    /// resolve). A settle that legitimately evaluates to nothing (an unmet guard, an
    /// unsupplied capability) is *not* an error — it's the same honest, expected outcome
    /// this whole project treats as normal everywhere else, not a fault to count.
    error_count: u64,
    /// When this deployment was first created. Persisted, so a restart doesn't reset it to
    /// "now" and quietly lie about how long it's actually been running.
    deployed_at: SystemTime,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Serialize)]
struct HashBody {
    hash: String,
}

#[derive(Serialize)]
struct RegistryBody {
    published: Vec<String>,
}

/// A JSON mirror of `core::Value`, for this HTTP/JSON boundary — the same reasoning as
/// `WValue` at the wasm boundary (`core::Value` deliberately carries no serde, keeping
/// `holon-core` dependency-minimal). Grounded exactly against the real `Value` enum (six
/// variants — there is no `Product` at the *value* level, only at the `Type` level, so this
/// mirror doesn't need one either) rather than assumed by symmetry with `Type`.
///
/// **A real, named limitation, not silently inherited:** `serde_json` cannot represent `NaN`/
/// `Infinity` (JSON has no token for them) — a cache entry whose `Number` is non-finite (an
/// overflow result) will fail to serialize back to the client. The wasm bridge sidesteps this
/// entirely by using `serde-wasm-bindgen` instead of JSON, since JS numbers carry non-finite
/// values natively; a plain-JSON HTTP boundary doesn't have that option. Not fixed here —
/// naming it is the honest move until it's a real, hit problem, not a hypothetical one.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", content = "value")]
enum SValue {
    Number(f64),
    Boolean(bool),
    Text(String),
    List(Vec<SValue>),
    Effect { sink: String, payload: Box<SValue> },
    Sum { arm: usize, payload: Box<SValue> },
}

impl SValue {
    fn from_core(v: &Value) -> SValue {
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
        }
    }
    fn into_core(self) -> Value {
        match self {
            SValue::Number(n) => Value::Number(n),
            SValue::Boolean(b) => Value::Boolean(b),
            SValue::Text(s) => Value::Text(s),
            SValue::List(items) => Value::List(items.into_iter().map(SValue::into_core).collect()),
            SValue::Effect { sink, payload } => {
                Value::Effect { sink, payload: Box::new(payload.into_core()) }
            }
            SValue::Sum { arm, payload } => Value::Sum { arm, payload: Box::new(payload.into_core()) },
        }
    }
}

/// Wire shape of a `CacheEntry` for `POST /cache/ingest` — `unit_hash` as hex (the one
/// convention every hash already crosses this boundary as), `inputs`/`outputs` as `SValue`.
#[derive(Deserialize)]
struct SCacheEntry {
    unit_hash: String,
    inputs: Vec<Option<SValue>>,
    outputs: Vec<Option<SValue>>,
}

#[derive(Serialize)]
struct CacheOutputsBody {
    outputs: Vec<Option<SValue>>,
}

/// `POST /fire`'s request body — just the inputs, positional, same convention `eval` already
/// uses everywhere (index = input-port index, `null` = absent). The root itself is named by
/// the `?root=<hex>` query param (it must already be published/staged — see the handler doc),
/// not re-sent in the body.
#[derive(Deserialize)]
struct FireRequestBody {
    inputs: Vec<Option<SValue>>,
}

/// One `(capability, payload)` pair `RecordingHost` recorded instead of actually performing.
/// `capability` is stringified via `{c:?}` (`Debug`) at the point a response is built, not
/// when `RecordingHost` records it — `RecordingHost.performed` keeps the real `Capability`
/// (needed by `fire_with_state`, which has to tell `StateWrite(key)` apart from everything
/// else that got performed) and only becomes a plain name here, at the wire boundary. One
/// fewer place a new `Capability` variant could be added upstream and silently never reach
/// this mirror. `Clone` so a `Deployment`'s cached `TickEntry` history can be cloned out for a
/// response without holding the deployments lock across the JSON serialization.
#[derive(Serialize, Clone)]
struct PerformedEntry {
    capability: String,
    payload: SValue,
}

/// `POST /fire` and `POST /trigger/<slug>`'s response shape. `outputs` is every settled slot
/// the fired unit declared — not just one. A single-output unit still gets a one-element
/// array; nothing about the common case changes shape awkwardly. A two-output unit (a text
/// response and a state-write effect, left as two separate exposed outputs rather than
/// combined into one artificial `list(...)`) now shows both, cleanly, instead of needing to
/// be merged into something that echoed the effect twice.
#[derive(Serialize)]
struct FireResponseBody {
    outputs: Vec<Option<SValue>>,
    performed: Vec<PerformedEntry>,
}

/// One saved flow — a named workspace document (see `AppState::flows` for why this is
/// deliberately not a content-addressed unit). `document` is opaque JSON: node positions,
/// viewport, whatever the editor chooses to keep — the server never reads inside it.
/// `Serialize`+`Deserialize` because this exact shape is also the on-disk persistence format
/// (one JSON array of these, same discipline as `DeploymentRecord`).
#[derive(Serialize, Deserialize, Clone)]
struct FlowRecord {
    name: String,
    document: serde_json::Value,
    saved_at_epoch_seconds: u64,
}

/// `POST /flows`' request body: upsert by name. The same name saves over the previous
/// version — a flow is a mutable workspace file, not an immutable identity; "save" means
/// exactly what it means in any editor.
#[derive(Deserialize)]
struct SaveFlowBody {
    name: String,
    document: serde_json::Value,
}

/// `GET /flows`' per-entry listing shape — name and freshness only, never the (possibly
/// large) document itself; that's what `GET /flows/<name>` is for.
#[derive(Serialize)]
struct FlowListEntry {
    name: String,
    saved_at_epoch_seconds: u64,
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
struct DeployRequestBody {
    slug: String,
    unit: String,
    interval_seconds: Option<u64>,
    #[serde(default)]
    acknowledge: Vec<String>,
}

/// The on-disk, persisted shape of one deployment. `triggers` is still not persisted — it's
/// re-derived from `unit`'s own capabilities on load, the same way a fresh `/deploy` call
/// derives it, rather than storing a fact that's already computable. Tick history still isn't
/// either — diagnostic, not load-bearing; fine to lose on restart. `state` **is** persisted
/// now, though — surviving a restart is the whole reason a deployment gets its own durable
/// state instead of an in-memory-only scratch value. Every new field is `#[serde(default)]`
/// so a deployments file written before this inch still loads without error.
#[derive(Serialize, Deserialize)]
struct DeploymentRecord {
    slug: String,
    unit: String,
    interval_seconds: Option<u64>,
    #[serde(default)]
    state: Option<BTreeMap<String, SValue>>,
    #[serde(default)]
    deployed_at_epoch_seconds: Option<u64>,
    #[serde(default)]
    trigger_count: u64,
    #[serde(default)]
    error_count: u64,
}

#[derive(Serialize)]
struct DeployResponseBody {
    slug: String,
    unit: String,
    /// What was actually derived from the unit's own capabilities — `"Ingress"`, `"Clock"`,
    /// both, or (refused before reaching this response) neither.
    triggers: Vec<String>,
    interval_seconds: Option<u64>,
}

#[derive(Serialize)]
struct DeploymentEntry {
    slug: String,
    unit: String,
    triggers: Vec<String>,
    interval_seconds: Option<u64>,
    /// Whether this deployment's own derived capabilities include a state axis — `true` means
    /// it carries a real, persisted `state` slot; `false` means it's stateless and `state`
    /// below is always `null`. The actual state *values* aren't exposed here yet (a real,
    /// named, not-yet-built follow-on, the same way tick history got its own endpoint rather
    /// than riding the deployment listing).
    has_state: bool,
    trigger_count: u64,
    error_count: u64,
    deployed_at_epoch_seconds: u64,
}

/// One clock tick's result, kept in a deployment's bounded history. `outputs`: same shape
/// change as `FireResponseBody`, same reason — every settled slot, not just one.
#[derive(Serialize, Clone)]
struct TickEntry {
    at_epoch_seconds: u64,
    outputs: Vec<Option<SValue>>,
    performed: Vec<PerformedEntry>,
}

#[derive(Serialize)]
struct HistoryBody {
    slug: String,
    ticks: Vec<TickEntry>,
}

/// `GET /deployments/<slug>/state`'s response — the deployment's current keyed state, in
/// full. Real observability, not just a debugging convenience: without this, there is no way
/// to confirm a write actually persisted between two fires except trusting a flow's own
/// self-report, which a `store_read → add → store_write` bootstrap can't even provide on its
/// first fire (see `test_stateful_writer`'s own doc). Named, not built speculatively — a
/// genuine testing and real-world need surfaced this, not spec-writing ahead of a use case.
#[derive(Serialize)]
struct StateBody {
    slug: String,
    state: BTreeMap<String, SValue>,
}

#[derive(Serialize)]
struct DeploymentsBody {
    deployments: Vec<DeploymentEntry>,
}

fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn err(status: u16, message: impl Into<String>) -> Response {
    Response::json(&ErrorBody { error: message.into() }).with_status_code(status)
}

/// Whether `origin` is a localhost origin this server will answer CORS for. Scoped
/// deliberately narrow — see the L4's Fork 1: this server is not meant to be reached from
/// anywhere but the editor's own dev server on the same machine.
fn is_allowed_origin(origin: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|base| match origin.strip_prefix(base) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    })
}

/// The largest request body any endpoint accepts. Units and flow documents are kilobytes; this
/// only exists so one oversized POST can't exhaust memory.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The control plane's request gate, checked before any routing. CORS headers only decide
/// whether a page may *read* a response, never whether the request runs, so without this any
/// web page the user visits could drive the API:
///
/// - **Host** must name this server on loopback (`localhost`, `127.0.0.1` or `[::1]` at the
///   listening port). A DNS-rebinding page reaches 127.0.0.1 under its own hostname, and would
///   otherwise be same-origin with full access.
/// - **Origin**, when a browser sends one, must be an allowed localhost origin. Browsers attach
///   it to every cross-origin POST/DELETE, including "simple" requests that skip preflight.
/// - **Content-Type** of a POST must be `application/json` or `application/octet-stream`,
///   neither of which is CORS-safelisted, so a cross-origin POST always needs a preflight this
///   server never grants. Defense in depth behind the Origin check.
///
/// Clients without a browser (curl, tests) send no Origin and pass by setting the two headers.
fn reject_untrusted(request: &Request, port: u16) -> Option<Response> {
    let host = request.header("Host").unwrap_or("");
    let host_ok = ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|name| host == format!("{name}:{port}") || (port == 80 && host == *name));
    if !host_ok {
        return Some(err(403, format!("Host {host:?} is not this server on loopback")));
    }
    if let Some(origin) = request.header("Origin")
        && !is_allowed_origin(origin)
    {
        return Some(err(403, format!("Origin {origin:?} is not allowed")));
    }
    if request.method() == "POST" {
        let media_type = request.header("Content-Type").unwrap_or("").split(';').next().unwrap_or("").trim();
        if !["application/json", "application/octet-stream"].iter().any(|t| media_type.eq_ignore_ascii_case(t)) {
            return Some(err(415, "POST bodies must be sent as application/json or application/octet-stream"));
        }
    }
    None
}

/// Add CORS headers to every response, in one place — so a new endpoint can never forget them
/// (the class of bug the `pub use` re-export list and the `WEncapError` variant drift are both
/// named for elsewhere in this project: a thing that must be repeated per call site eventually
/// isn't repeated somewhere).
fn with_cors(request: &Request, mut response: Response) -> Response {
    if let Some(origin) = request.header("Origin") {
        if is_allowed_origin(origin) {
            response = response
                .with_additional_header("Access-Control-Allow-Origin", origin.to_string())
                .with_additional_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
                .with_additional_header("Access-Control-Allow-Headers", "Content-Type")
                .with_additional_header("Vary", "Origin");
        }
    }
    response
}

fn read_body(request: &Request) -> Result<Vec<u8>, Response> {
    let Some(body) = request.data() else {
        return Err(err(400, "body already consumed"));
    };
    let mut bytes = Vec::new();
    // One byte past the cap is enough to tell "too large" apart from "exactly at the cap".
    if body.take(MAX_BODY_BYTES + 1).read_to_end(&mut bytes).is_err() {
        return Err(err(400, "failed to read request body"));
    }
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(err(413, format!("request body exceeds {MAX_BODY_BYTES} bytes")));
    }
    Ok(bytes)
}

fn handle_get_unit(store: &SharedStore, hash_hex: &str) -> Response {
    let Some(key) = parse_hash(hash_hex) else {
        return err(400, "not a 64-char hex hash");
    };
    match store.get(&key) {
        // Re-encode through the ONE core encoder rather than caching raw bytes separately —
        // decode∘encode is proven byte-identical (hash.rs's own round-trip tests), so this is
        // not a second encoding path, just the one encoder used twice.
        Ok(Some(unit)) => match encode_preimage(&unit) {
            Ok(bytes) => Response::from_data("application/octet-stream", bytes),
            Err(e) => err(500, format!("re-encode failed: {e:?}")),
        },
        Ok(None) => Response::empty_404(),
        Err(StoreError::Corrupt { key, actual }) => err(
            500,
            format!(
                "stored blob failed verify-on-read: filed under {}, hashes to {}",
                to_hex(&key),
                to_hex(&actual)
            ),
        ),
        Err(e) => err(500, format!("store error: {e:?}")),
    }
}

fn handle_post_unit(store: &SharedStore, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let unit = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };
    let mut store = store.clone();
    match store.put(&unit) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        Err(e) => err(500, format!("store error: {e:?}")),
    }
}

/// `POST /registry/publish` — body is the root unit's own preimage bytes (same convention as
/// `/unit`, deliberately: one "how a unit crosses the wire" shape everywhere, not two).
/// `Registry::publish` itself does the real work: verify the whole closure (every child, every
/// `fn`-param reference) is already present in the store, fail closed on anything dangling,
/// else commit the root and record it published. This handler adds nothing beyond decoding the
/// body and translating `PublishError` to an HTTP status.
fn handle_post_publish(registry: &Mutex<Registry<SharedStore>>, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let unit = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };
    let mut reg = registry.lock().unwrap();
    match reg.publish(&unit) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        // Fails closed exactly as Registry::publish already does — this handler doesn't loosen
        // or re-check anything, just names the missing hash back to the caller so the editor
        // can say "stage this child first" instead of a bare 400.
        Err(PublishError::DanglingReference(h)) => err(
            400,
            format!(
                "publish refused: {} is referenced but not yet in the store — POST it via /unit first",
                to_hex(&h)
            ),
        ),
        Err(PublishError::Store(e)) => err(500, format!("store error: {e:?}")),
    }
}

/// `GET /registry` — the published set, in hash order (`Registry::published()`'s own order,
/// unchanged). See the module doc: this list is in-memory only and resets on restart, by
/// `Registry`'s own existing, documented scope — not something this endpoint adds or hides.
fn handle_get_registry(registry: &Mutex<Registry<SharedStore>>) -> Response {
    let reg = registry.lock().unwrap();
    let published: Vec<String> = reg.published().map(to_hex).collect();
    Response::json(&RegistryBody { published })
}

/// `POST /registry/fork?root=<hex>&target=<hex>` — body is the replacement unit's own preimage
/// bytes (same "unit crosses as its own preimage" convention as `/unit` and
/// `/registry/publish`). `root`/`target` are hashes, so they ride the query string as hex —
/// consistent with how `/unit/<hash>` already puts a hash in the URL rather than the body.
///
/// `Registry::fork` does the real work: re-pin every reference to `target` inside `root`'s
/// closure to the newly-stored replacement, then publish the re-pinned spine (reusing
/// `publish`'s own dangling-reference gate verbatim). This handler only decodes the wire shapes
/// and translates `ForkError` to a status — the same "thin wrapper" discipline as every other
/// endpoint here.
fn handle_post_fork(registry: &Mutex<Registry<SharedStore>>, request: &Request) -> Response {
    let Some(root_hex) = request.get_param("root") else {
        return err(400, "missing ?root=<hex> query param");
    };
    let Some(target_hex) = request.get_param("target") else {
        return err(400, "missing ?target=<hex> query param");
    };
    let Some(root_hash) = parse_hash(&root_hex) else {
        return err(400, "root is not a 64-char hex hash");
    };
    let Some(target_hash) = parse_hash(&target_hex) else {
        return err(400, "target is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let replacement = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };

    let mut reg = registry.lock().unwrap();
    match reg.fork(&root_hash, &target_hash, &replacement) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        Err(ForkError::RootNotFound(h)) => {
            err(404, format!("no unit stored at root {}", to_hex(&h)))
        }
        Err(ForkError::TargetNotInClosure(h)) => err(
            400,
            format!("target {} is not referenced anywhere in root's closure", to_hex(&h)),
        ),
        Err(ForkError::DanglingInClosure(h)) => err(
            400,
            format!("root's closure is incomplete in the store: {} does not resolve", to_hex(&h)),
        ),
        Err(ForkError::Publish(PublishError::DanglingReference(h))) => err(
            400,
            format!("re-pinned root refused: {} is referenced but not in the store", to_hex(&h)),
        ),
        Err(ForkError::Publish(PublishError::Store(e))) => {
            err(500, format!("store error during publish: {e:?}"))
        }
        Err(ForkError::Store(e)) => err(500, format!("store error: {e:?}")),
        Err(ForkError::Encode(e)) => err(500, format!("encode error: {e:?}")),
    }
}

/// Build a `Resolver` over `root`'s full closure by walking `child_refs` (the same closure
/// definition `Registry::publish`'s own dangling-reference check already uses — one definition
/// of "what a unit references," not a second) and fetching each referenced unit from the store.
/// This is the same shape `Registry::fork` already needs internally for the identical reason
/// (`Store` yields owned units; `Resolver` wants borrowed ones) — reused, not reinvented.
///
/// A hash that doesn't resolve in the store is left out of the resolver rather than erroring —
/// `eval`/`verify_entry` already treat an unresolved reference as "nothing evaluable" and fail
/// closed on it, so this doesn't need its own dangling-reference error path; it just doesn't
/// promise more than the store actually has.
fn materialize_closure(store: &SharedStore, root: Hash) -> Result<MapResolver, StoreError> {
    let mut resolver = MapResolver::new();
    let mut seen = BTreeSet::new();
    let mut queue = vec![root];
    while let Some(h) = queue.pop() {
        if !seen.insert(h) {
            continue;
        }
        let Some(unit) = store.get(&h)? else {
            continue;
        };
        queue.extend(child_refs(&unit));
        resolver.insert(unit).map_err(StoreError::Encode)?;
    }
    Ok(resolver)
}

/// `GET /cache/<key>` — the trustless read: re-executes the cached computation against a
/// resolver materialized from the store and returns the outputs **only if they check out**
/// (`SharedCache::get_verified`). A syntactically-present entry whose claimed output is wrong
/// is refused (422), not served — the whole point of this endpoint over the cache's raw,
/// unverified `get`, which this server deliberately does not expose at all.
fn handle_get_cache(cache: &Mutex<SharedCache>, store: &SharedStore, key_hex: &str) -> Response {
    let Some(key) = parse_hash(key_hex) else {
        return err(400, "not a 64-char hex hash");
    };
    let unit_hash = {
        let cache = cache.lock().unwrap();
        match cache.get(&key) {
            Some(entry) => entry.unit_hash,
            None => return Response::empty_404(),
        }
    };
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let cache = cache.lock().unwrap();
    match cache.get_verified(&key, &resolver) {
        Some(outputs) => Response::json(&CacheOutputsBody {
            outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        }),
        // 422, not 404: the entry exists (we already found it above) but failed the semantic
        // check — a different failure than "nothing here."
        None => err(422, "cache entry failed verification — refused, not served"),
    }
}

/// `POST /cache/ingest?key=<hex>` — the trustless *write*: admits a peer-supplied entry only if
/// it actually hashes to the key it's claimed under (`SharedCache::ingest`, P3.8.2's own gap,
/// exposed here). Cheap structural check only — it does **not** re-execute the computation
/// (that's `GET /cache/<key>`'s job on read); a correctly-keyed entry with a wrong output is
/// admitted here and caught there, exactly as `SharedCache`'s own two-tier design intends.
fn handle_post_cache_ingest(cache: &Mutex<SharedCache>, request: &Request) -> Response {
    let Some(key_hex) = request.get_param("key") else {
        return err(400, "missing ?key=<hex> query param — the claimed cache key");
    };
    let Some(claimed_key) = parse_hash(&key_hex) else {
        return err(400, "key is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let sentry: SCacheEntry = match serde_json::from_slice(&bytes) {
        Ok(e) => e,
        Err(e) => return err(400, format!("not a valid cache entry JSON: {e}")),
    };
    let Some(unit_hash) = parse_hash(&sentry.unit_hash) else {
        return err(400, "entry.unit_hash is not a 64-char hex hash");
    };
    let entry = CacheEntry {
        unit_hash,
        inputs: sentry.inputs.into_iter().map(|o| o.map(SValue::into_core)).collect(),
        outputs: sentry.outputs.into_iter().map(|o| o.map(SValue::into_core)).collect(),
    };
    let mut cache = cache.lock().unwrap();
    match cache.ingest(claimed_key, entry) {
        Ok(()) => Response::text("ingested"),
        Err(CacheIngestError::KeyMismatch { claimed, actual }) => err(
            400,
            format!(
                "key mismatch: claimed {}, entry actually hashes to {} — refused",
                to_hex(&claimed),
                to_hex(&actual)
            ),
        ),
        Err(CacheIngestError::Encode(e)) => err(500, format!("encode error: {e:?}")),
    }
}

/// The `Host` this endpoint runs `fire` against. **Deliberately does not perform anything
/// real** — no network call, no file write, no email. It records each `(capability, payload)`
/// `fire` hands it and returns them to the caller as a receipt instead. This is a scoped
/// decision, not an oversight: the project's own plan already names real platform hosts (real
/// email/socket/file IO behind the `Host` seam) as a still-deferred, separate piece of work,
/// not something this transport crate was ever asked to build. Swapping in a real `Host` later
/// is exactly what the `Host` trait already exists to make a clean, isolated change — nothing
/// about this endpoint's wire shape would need to move.
struct RecordingHost {
    performed: Vec<(Capability, Value)>,
}

impl Host for RecordingHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.performed.push((capability, payload.clone()));
    }
}

/// Fire `root_unit` with one extra sourced value (whatever actually triggered this fire —
/// `NetworkIngress`'s body, or `Clock`'s current time) plus, if this deployment carries state,
/// every key currently in `state` sourced as `StateRead(key)` — then, after firing, apply any
/// `StateWrite(key)` that was performed back into `state`. The same three steps
/// `holon-host`'s own `StateHost::round` uses internally (snapshot → fire → apply) —
/// deliberately **not** routed through `StateHost` itself, though: `round()` only ever keeps
/// `StateWrite` from what gets performed and quietly discards everything else, which is wrong
/// here — a state-bearing deployment that also sends an egress needs that egress reported back
/// to the caller, not silently dropped. `RecordingHost` already captures everything performed;
/// this just adds the one extra step `round()` would otherwise hide.
///
/// Locks `state` itself, for the whole round, if this deployment is stateful — the caller
/// hands in the deployment's own `Arc<Mutex<...>>` dereferenced to `&Mutex<...>`, already
/// cloned out from under the deployments-table lock, which is never held here. That's what
/// keeps two triggers of two *different* deployments from ever contending, while two triggers
/// of the *same* deployment correctly serialize instead of racing.
///
/// A `store_read` for a key that's never been written simply isn't in `sourced` at all, so it
/// evaluates to nothing — the same honest-absence behavior any other unsupplied sourced
/// capability already has (`fire_sourced`'s own doc). No special-casing needed for a
/// deployment's first ever fire.
/// Fire `root_unit` with whatever sourced values this fire can honestly supply — always
/// including every `StateRead(key)` this deployment's own state currently holds, plus
/// `extra_sourced`: the driver capabilities the caller can actually answer for this fire.
/// A slice, not one pair: `Clock` (the wall clock is always knowable) and `Random` (a fresh
/// draw is always makeable) are cheap and honest to supply on *every* fire regardless of what
/// specifically triggered it — a unit combining `ingress` and `random`, say, needs both
/// available at once. `FileRead`/`SecretAccess` are deliberately never in this list: there is
/// no addressing mechanism in the wire surface today (a leaf declaring `FileRead` doesn't say
/// *which* file), so there is nothing honest to supply — a real, separate design question, not
/// something to paper over with a placeholder value.
///
/// Returns every settled output slot, not just one — `fire_sourced` itself now carries
/// `eval`'s real `Vec<Option<Value>>` end to end, and this just passes it through. A flow that
/// produces a genuine data response *and* performs a genuine side effect are two separate
/// things; forcing them into one artificial `list(...)` to satisfy an old single-output
/// requirement doesn't just add unrelated wiring, it makes the settled value redundantly
/// echo the effect inside what's supposed to be the plain answer, since nothing ever removes
/// it from there. `perform_effects` already walks every non-`None` slot, so a state-write and
/// a text response left as two separate exposed outputs are both correctly handled now,
/// without combining them into anything.
fn fire_with_state(
    root_unit: &Unit,
    resolver: &impl Resolver,
    state: Option<&Mutex<BTreeMap<String, Value>>>,
    extra_sourced: &[(Capability, Value)],
) -> (Vec<Option<Value>>, Vec<(Capability, Value)>) {
    let mut guard = state.map(|m| m.lock().unwrap());

    let mut sourced: Vec<(Capability, Value)> = match guard.as_deref() {
        Some(storage) => storage
            .iter()
            .map(|(key, value)| (Capability::StateRead(key.clone()), value.clone()))
            .collect(),
        None => Vec::new(),
    };
    sourced.extend_from_slice(extra_sourced);

    let mut host = RecordingHost { performed: Vec::new() };
    let outputs = fire_sourced(root_unit, &sourced, resolver, &mut host);

    if let Some(storage) = guard.as_deref_mut() {
        for (cap, payload) in &host.performed {
            if let Capability::StateWrite(key) = cap {
                if is_storable(payload) {
                    storage.insert(key.clone(), payload.clone());
                } else {
                    eprintln!("holon-server: refused to store an effect request in state key {key:?}");
                }
            }
        }
    }

    (outputs, host.performed)
}

/// `POST /fire?root=<hex>` — settle a published unit **once**, on request. Not "deploy": there
/// is no standing process left running afterward, no listener bound to a driver — this is a
/// single call-and-return, the same shape `eval`/`design_eval` already are, just via `fire`
/// (deploy mode) instead. The genuine "keep a flow alive, bound to `ingress`/`timer`" pillar is
/// real and different work, deliberately not this endpoint — see the L4.
///
/// `root` must already be resolvable from the store (staged via `/unit`, or published) — this
/// reuses `materialize_closure` exactly as the cache endpoints do, rather than accepting the
/// unit fresh in the body, since firing something implies it's a real, referenceable unit, not
/// a throwaway one.
fn handle_post_fire(store: &SharedStore, request: &Request) -> Response {
    let Some(root_hex) = request.get_param("root") else {
        return err(400, "missing ?root=<hex> query param");
    };
    let Some(root_hash) = parse_hash(&root_hex) else {
        return err(400, "root is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let fire_body: FireRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid fire request JSON: {e}")),
    };
    let resolver = match materialize_closure(store, root_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let Some(root_unit) = resolver.resolve(&root_hash) else {
        return err(404, "no unit stored at that hash — stage or publish it first");
    };
    let inputs: Vec<Option<Value>> =
        fire_body.inputs.into_iter().map(|o| o.map(SValue::into_core)).collect();

    let mut host = RecordingHost { performed: Vec::new() };
    let outputs = fire(root_unit, &inputs, &resolver, &mut host);

    Response::json(&FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: host
            .performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// A slug must be non-empty ASCII alphanumeric/`-`/`_` only — keeps the simple prefix-based
/// router in `route()` unambiguous (no `/` inside a slug could be confused with a path
/// separator) and avoids any path-traversal-shaped weirdness reaching `strip_prefix`.
/// Write the whole current deployments table to `path` as a JSON array of `DeploymentRecord`
/// (write-through — called after every successful deploy/delete, not batched or scheduled).
/// A no-op for the `":memory:"` sentinel. Failures are logged, not fatal — a person's server
/// keeps working even if the disk write failed; they just won't survive a restart until it
/// succeeds again, which is the honest degradation for something that was opt-in persistence
/// to begin with.
///
/// Takes the table's mutex rather than a locked table: the table lock is held only long enough
/// to copy each deployment's fields and clone its state `Arc`. Each state map is then locked on
/// its own, and the disk write happens with no lock held but `PERSIST`. Before, the table lock
/// was held throughout, so one deployment mid-fire (holding its state lock) or one slow disk
/// write stalled every endpoint and the clock scheduler. `PERSIST` serializes whole
/// snapshot-and-write passes, so an older snapshot can never land on disk after a newer one.
fn persist_deployments(path: &str, deployments: &Mutex<BTreeMap<String, Deployment>>) {
    static PERSIST: Mutex<()> = Mutex::new(());
    if path == ":memory:" {
        return;
    }
    let _persist = PERSIST.lock().unwrap();
    let snapshot: Vec<(DeploymentRecord, Option<DeploymentState>)> = deployments
        .lock()
        .unwrap()
        .iter()
        .map(|(slug, dep)| {
            let record = DeploymentRecord {
                slug: slug.clone(),
                unit: to_hex(&dep.unit),
                interval_seconds: dep.interval_seconds,
                state: None,
                deployed_at_epoch_seconds: dep
                    .deployed_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs()),
                trigger_count: dep.trigger_count,
                error_count: dep.error_count,
            };
            (record, dep.state.clone())
        })
        .collect();
    let records: Vec<DeploymentRecord> = snapshot
        .into_iter()
        .map(|(mut record, state)| {
            record.state = state.map(|arc| {
                let storage = arc.lock().unwrap();
                storage.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect()
            });
            record
        })
        .collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            if let Err(e) = write_atomically(Path::new(path), &bytes) {
                eprintln!("warning: failed to persist deployments to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize deployments: {e}"),
    }
}

/// Load deployments from `path` at startup — the inverse of `persist_deployments`. Returns an
/// empty table for the `":memory:"` sentinel or a missing file (first run). A file that exists
/// but can't be read or parsed is an `Err`, and the server refuses to start: starting empty
/// would let the first deploy overwrite the file — every deployment and all its state — with
/// the empty table. Each record is re-materialized against the
/// **current** store and re-derives its `triggers`/`state`-eligibility fresh via
/// `capabilities()` — exactly what a live `/deploy` call would compute, not a cached copy that
/// could go stale. A record whose unit no longer resolves in the store (e.g. the store
/// directory was touched separately) is skipped with a warning, not a crash — the rest of the
/// file still loads. `state` in the record is only trusted if the unit still genuinely derives
/// a state capability (defensive: a hand-edited or foreign file claiming state for a
/// state-free unit is ignored, not silently restored); a state-capable unit with no persisted
/// state (an old file predating this feature) starts from an empty map, same as a fresh
/// `/deploy`.
fn load_deployments(path: &str, store: &SharedStore) -> Result<BTreeMap<String, Deployment>, String> {
    let mut loaded = BTreeMap::new();
    if path == ":memory:" {
        return Ok(loaded);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(loaded),
        Err(e) => return Err(format!("could not read {path:?}: {e}")),
    };
    let records: Vec<DeploymentRecord> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{path:?} is not valid deployment JSON: {e}"))?;

    for record in records {
        let Some(unit_hash) = parse_hash(&record.unit) else {
            eprintln!("warning: skipping deployment {:?} — {:?} is not a valid hash", record.slug, record.unit);
            continue;
        };
        let resolver = match materialize_closure(store, unit_hash) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: skipping deployment {:?} — store error: {e:?}", record.slug);
                continue;
            }
        };
        let Some(root_unit) = resolver.resolve(&unit_hash) else {
            eprintln!("warning: skipping deployment {:?} — unit no longer resolves in the store", record.slug);
            continue;
        };
        let caps = capabilities(root_unit, &resolver);
        let mut triggers = Vec::new();
        if caps.contains(Capability::NetworkIngress) {
            triggers.push("Ingress".to_string());
        }
        if caps.contains(Capability::Clock) {
            triggers.push("Clock".to_string());
        }
        let next_due = record.interval_seconds.map(|_| SystemTime::now());

        let state = if derives_state(caps.iter()) {
            let map: BTreeMap<String, Value> = match record.state {
                Some(persisted) => {
                    persisted.into_iter().map(|(k, v)| (k, SValue::into_core(v))).collect()
                }
                None => BTreeMap::new(),
            };
            Some(Arc::new(Mutex::new(map)))
        } else {
            None
        };
        let deployed_at = record
            .deployed_at_epoch_seconds
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap_or_else(SystemTime::now);

        println!("loaded deployment {:?} from {path:?} (triggers: {triggers:?})", record.slug);
        loaded.insert(
            record.slug,
            Deployment {
                unit: unit_hash,
                triggers,
                interval_seconds: record.interval_seconds,
                next_due,
                history: Vec::new(),
                state,
                trigger_count: record.trigger_count,
                error_count: record.error_count,
                deployed_at,
            },
        );
    }
    Ok(loaded)
}

/// Persist the flows table — same write-through discipline and `":memory:"` opt-out as
/// `persist_deployments`, but simpler: a `FlowRecord` is already its own on-disk shape (no
/// store cross-references, no re-derivation needed on the way back in).
fn persist_flows(path: &str, flows: &BTreeMap<String, FlowRecord>) {
    if path == ":memory:" {
        return;
    }
    let records: Vec<&FlowRecord> = flows.values().collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            if let Err(e) = write_atomically(Path::new(path), &bytes) {
                eprintln!("warning: failed to persist flows to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize flows: {e}"),
    }
}

/// Load flows at startup — the inverse of `persist_flows`. A missing file (first run) or
/// `":memory:"` yields an empty table; an unreadable or unparseable file is an `Err` and the
/// server refuses to start, for the same reason as `load_deployments`: the next save would
/// overwrite every flow with the empty table. A record whose name fails `is_valid_slug` is
/// skipped with a warning rather than trusted — same defensive stance `load_deployments`
/// takes toward hand-edited files.
fn load_flows(path: &str) -> Result<BTreeMap<String, FlowRecord>, String> {
    if path == ":memory:" {
        return Ok(BTreeMap::new());
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("could not read {path:?}: {e}")),
    };
    let records: Vec<FlowRecord> =
        serde_json::from_slice(&bytes).map_err(|e| format!("{path:?} is not valid flows JSON: {e}"))?;
    let mut loaded = BTreeMap::new();
    for record in records {
        if !is_valid_slug(&record.name) {
            eprintln!("warning: skipping flow with invalid name {:?}", record.name);
            continue;
        }
        loaded.insert(record.name.clone(), record);
    }
    Ok(loaded)
}

/// `POST /flows` — save (upsert) a flow document by name. The name follows the same rule as
/// deployment slugs (`is_valid_slug`) — one naming convention across the API, not two. The
/// document is stored as-is; the server never inspects inside it (see `AppState::flows`).
fn handle_post_flows(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &str,
    request: &Request,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: SaveFlowBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("invalid save-flow body: {e}")),
    };
    if !is_valid_slug(&body.name) {
        return err(400, "flow name must be non-empty ASCII alphanumeric/'-'/'_' only");
    }
    let saved_at_epoch_seconds =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let record = FlowRecord { name: body.name.clone(), document: body.document, saved_at_epoch_seconds };

    let mut table = flows.lock().unwrap();
    table.insert(body.name.clone(), record);
    persist_flows(flows_path, &table);

    Response::json(&FlowListEntry { name: body.name, saved_at_epoch_seconds })
}

/// `GET /flows` — every saved flow's name + freshness, never the documents themselves.
fn handle_get_flows(flows: &Mutex<BTreeMap<String, FlowRecord>>) -> Response {
    let table = flows.lock().unwrap();
    let entries: Vec<FlowListEntry> = table
        .values()
        .map(|f| FlowListEntry { name: f.name.clone(), saved_at_epoch_seconds: f.saved_at_epoch_seconds })
        .collect();
    Response::json(&entries)
}

/// `GET /flows/<name>` — the full saved record, document included. 404 for an unknown name,
/// same convention as every other by-name lookup on this API.
fn handle_get_flow(flows: &Mutex<BTreeMap<String, FlowRecord>>, name: &str) -> Response {
    let table = flows.lock().unwrap();
    match table.get(name) {
        Some(record) => Response::json(record),
        None => err(404, "no flow saved under that name"),
    }
}

/// `DELETE /flows/<name>` — remove a saved flow. 404 if it was never there.
fn handle_delete_flow(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &str,
    name: &str,
) -> Response {
    let mut table = flows.lock().unwrap();
    if table.remove(name).is_none() {
        return err(404, "no flow saved under that name");
    }
    persist_flows(flows_path, &table);
    Response::json(&serde_json::json!({ "deleted": name }))
}

/// Whether a unit's derived capabilities include either state axis — the gate for whether a
/// deployment gets its own `state` slot at all. Takes the iterator `Capabilities::iter()`
/// already gives, rather than the `Capabilities` type itself, so this needs no new import.
fn derives_state(mut caps: impl Iterator<Item = Capability>) -> bool {
    caps.any(|c| matches!(c, Capability::StateRead(_) | Capability::StateWrite(_)))
}

fn is_valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `POST /deploy` — register `slug → unit`. The trigger(s) are **derived**, not asserted:
/// `capabilities(unit, resolver)` (the same closure-union already used for capability-consent
/// elsewhere in this project) tells us whether the unit needs `Ingress`, `Clock`, both, or
/// neither — the caller only supplies what genuinely can't be read off the graph
/// (`interval_seconds`, only when `Clock` is present; nothing tells you "every 3600 seconds"
/// from structure alone). A unit deriving neither capability is refused — nothing would ever
/// trigger it. A repeat slug is a redeploy, not an error (the L4's Fork 2): the unit, triggers
/// and schedule are replaced, but the slug's **state is kept** (see the insert below).
fn handle_post_deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    request: &Request,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DeployRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid deploy request JSON: {e}")),
    };
    if !is_valid_slug(&body.slug) {
        return err(400, "slug must be non-empty ASCII alphanumeric, '-', or '_' only");
    }
    let Some(unit_hash) = parse_hash(&body.unit) else {
        return err(400, "unit is not a 64-char hex hash");
    };

    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        return err(404, "no unit stored at that hash — stage or publish it first");
    };

    let caps = capabilities(root_unit, &resolver);
    let is_ingress = caps.contains(Capability::NetworkIngress);
    let is_clock = caps.contains(Capability::Clock);
    if !is_ingress && !is_clock {
        return err(
            400,
            "this unit declares no driver capability (NetworkIngress or Clock) — nothing would ever trigger it",
        );
    }

    // Capability-consent, before this unit ever gets to run on a schedule or answer a request.
    // The set itself is still fully derived (`caps`, above) — only "has the caller actually
    // reviewed it" is asserted, which is the one part that genuinely can't come from the graph.
    let missing: Vec<String> = caps
        .iter()
        .map(|c| format!("{c:?}"))
        .filter(|name| !body.acknowledge.contains(name))
        .collect();
    if !missing.is_empty() {
        return err(
            400,
            format!(
                "this unit's derived capabilities include {} — list them in \"acknowledge\" to confirm you've reviewed what it may do",
                missing.join(", ")
            ),
        );
    }

    let interval_seconds = if is_clock {
        match body.interval_seconds {
            Some(n) if n >= 1 => Some(n),
            Some(_) => return err(400, "interval_seconds must be at least 1 (the L4's Fork 3 floor)"),
            None => {
                return err(400, "this unit's derived capabilities include Clock — interval_seconds is required")
            }
        }
    } else {
        None
    };
    // Fires on the scheduler's very next pass rather than waiting a full interval the first
    // time — a person deploying a clock flow wants to see it work now, not in an hour.
    let next_due = interval_seconds.map(|_| SystemTime::now());

    let mut triggers = Vec::new();
    if is_ingress {
        triggers.push("Ingress".to_string());
    }
    if is_clock {
        triggers.push("Clock".to_string());
    }
    let is_state = derives_state(caps.iter());

    {
        let mut deployments = deployments.lock().unwrap();
        // A redeploy keeps the slug's state: shipping a fix must never wipe the data the
        // previous version wrote. The *same* `Arc` is carried over (not a copy), so a fire still
        // in flight against the old version commits into the map the new version reads. Kept
        // even when the new unit is stateless, so redeploying a stateful version later finds
        // its data again; `DELETE /deployments/<slug>` is the one way to discard it.
        let carried = deployments.get(&body.slug).and_then(|old| old.state.clone());
        let state = match carried {
            Some(arc) => Some(arc),
            None if is_state => Some(Arc::new(Mutex::new(BTreeMap::new()))),
            None => None,
        };
        deployments.insert(
            body.slug.clone(),
            Deployment {
                unit: unit_hash,
                triggers: triggers.clone(),
                interval_seconds,
                next_due,
                history: Vec::new(),
                state,
                trigger_count: 0,
                error_count: 0,
                deployed_at: SystemTime::now(),
            },
        );
    }
    persist_deployments(deployments_path, deployments);

    Response::json(&DeployResponseBody { slug: body.slug, unit: to_hex(&unit_hash), triggers, interval_seconds })
}

/// Build the wire-facing summary of one deployment — shared by `GET /deployments` and
/// `DELETE /deployments/<slug>`'s response, so the two never drift apart on what a
/// deployment "is" from the outside.
fn deployment_entry(slug: &str, dep: &Deployment) -> DeploymentEntry {
    DeploymentEntry {
        slug: slug.to_string(),
        unit: to_hex(&dep.unit),
        triggers: dep.triggers.clone(),
        interval_seconds: dep.interval_seconds,
        has_state: dep.state.is_some(),
        trigger_count: dep.trigger_count,
        error_count: dep.error_count,
        deployed_at_epoch_seconds: dep.deployed_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
    }
}

/// `GET /deployments` — every deployment, its derived triggers, and its interval if clock-bound.
fn handle_get_deployments(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Response {
    let deployments = deployments.lock().unwrap();
    let list = deployments.iter().map(|(slug, dep)| deployment_entry(slug, dep)).collect();
    Response::json(&DeploymentsBody { deployments: list })
}

/// `GET /deployments/<slug>/history` — the bounded (last 20) clock-tick history for a
/// `Clock`-triggered deployment. Empty for a purely `Ingress`-triggered one — there's nothing
/// to show; each `/trigger/<slug>` call already returns its own result directly.
fn handle_get_history(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let deployments = deployments.lock().unwrap();
    match deployments.get(slug) {
        Some(dep) => Response::json(&HistoryBody { slug: slug.to_string(), ticks: dep.history.clone() }),
        None => Response::empty_404(),
    }
}

/// `GET /deployments/<slug>/state` — a stateful deployment's current keyed state, in full.
/// 404 for an unknown slug (same convention as `/history`); 400 for a slug that exists but
/// isn't stateful (its derived capabilities never included `StateRead`/`StateWrite`) — a
/// different failure from "not found," so a different status, naming what's actually true
/// rather than pretending an empty map would mean the same thing as "this was never a stateful
/// deployment at all."
fn handle_get_state(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let deployments = deployments.lock().unwrap();
    match deployments.get(slug) {
        Some(dep) => match &dep.state {
            Some(arc) => {
                let storage = arc.lock().unwrap();
                let state = storage.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
                Response::json(&StateBody { slug: slug.to_string(), state })
            }
            None => err(
                400,
                "this deployment has no state — its derived capabilities don't include StateRead/StateWrite",
            ),
        },
        None => Response::empty_404(),
    }
}

/// `DELETE /deployments/<slug>` — closes the named gap from the clock-deploy L4's Fork 4: the
/// only way to stop a `Clock`-triggered deployment used to be killing the whole server. Removing
/// the slug from the table is sufficient — `run_due_clock_ticks` only ever acts on what's
/// currently in the table, so there's no separate "cancel" signal to build; the deployment
/// simply stops being something the scheduler can find. For an `Ingress`-triggered deployment,
/// `POST /trigger/<slug>` starts 404ing immediately afterward, the same 404 it already gives a
/// slug that was never deployed. Returns the removed deployment's last known info, or 404 if
/// the slug didn't exist.
fn handle_delete_deployment(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
) -> Response {
    let removed = deployments.lock().unwrap().remove(slug);
    match removed {
        Some(dep) => {
            persist_deployments(deployments_path, deployments);
            Response::json(&deployment_entry(slug, &dep))
        }
        None => Response::empty_404(),
    }
}

/// `POST /trigger/<slug>` — the ingress trigger. Looks up the deployed unit, materializes its
/// closure exactly as `/fire` and the cache endpoints do, and settles it via `fire_sourced`
/// with the raw request body injected as `Capability::NetworkIngress` — wrapped as
/// `Value::Text`, the L4's Fork 3 default (`ingress`'s declared output type is `Any`; nothing
/// forces this choice, but Text matches how every other text-shaped payload already crosses
/// this boundary). A flow wanting structured data parses that Text itself, downstream, using
/// this project's own `Sum`/`select` machinery — not a format this endpoint imposes. Works
/// against any deployed unit regardless of its derived triggers (a `Clock`-only unit just won't
/// have an `ingress` leaf for the resolver to substitute, so this is harmless, not unsafe, to
/// allow uniformly rather than gating on which triggers were derived at deploy time).
/// Bump a deployment's trigger/error counters after a fire attempt — a small, shared tail
/// step for both `handle_post_trigger` and `fire_one_clock_tick`. Locks the table only
/// briefly; if the deployment was deleted concurrently between the earlier lookup and this
/// call, this quietly no-ops rather than panicking, the same defensive shape
/// `fire_one_clock_tick`'s own history write-back already uses.
fn record_trigger(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, is_error: bool) {
    if let Some(dep) = deployments.lock().unwrap().get_mut(slug) {
        dep.trigger_count += 1;
        if is_error {
            dep.error_count += 1;
        }
    }
}

/// `POST /trigger/<slug>` — the ingress trigger. Looks up the deployed unit, materializes its
/// closure exactly as `/fire` and the cache endpoints do, and settles it via `fire_with_state`
/// with the raw request body injected as `Capability::NetworkIngress` — wrapped as
/// `Value::Text`, the L4's Fork 3 default (`ingress`'s declared output type is `Any`; nothing
/// forces this choice, but Text matches how every other text-shaped payload already crosses
/// this boundary). A flow wanting structured data parses that Text itself, downstream, using
/// this project's own `Sum`/`select` machinery — not a format this endpoint imposes. Works
/// against any deployed unit regardless of its derived triggers (a `Clock`-only unit just won't
/// have an `ingress` leaf for the resolver to substitute, so this is harmless, not unsafe, to
/// allow uniformly rather than gating on which triggers were derived at deploy time).
///
/// Only ever holds the deployments-table lock briefly — once to look up the unit and clone out
/// this deployment's own state handle (an `Arc` clone, not the state itself), and once at the
/// end to bump counters (and, for a stateful deployment, persist). The fire itself, and any
/// state locking it needs, happens with the table lock released, so one slow trigger never
/// blocks any other deployment.
fn handle_post_trigger(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    request: &Request,
) -> Response {
    let (unit_hash, state) = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(dep) => (dep.unit, dep.state.clone()),
            None => return Response::empty_404(),
        }
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body_text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return err(400, "request body is not valid UTF-8 text"),
    };
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            record_trigger(deployments, slug, true);
            return err(500, format!("store error while materializing closure: {e:?}"));
        }
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        record_trigger(deployments, slug, true);
        return err(500, "deployed unit no longer resolves in the store");
    };

    let now = SystemTime::now();
    let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::NetworkIngress, Value::Text(body_text)),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let (outputs, performed) = fire_with_state(root_unit, &resolver, state.as_deref(), &extra_sourced);

    record_trigger(deployments, slug, false);
    if state.is_some() {
        persist_deployments(deployments_path, deployments);
    }

    Response::json(&FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// The tick-firing logic for one scheduler pass, callable directly with a controlled `now` —
/// pulled out of `spawn_clock_scheduler`'s sleep loop specifically so it's testable without real
/// sleeping (the same "extract the testable logic out of the untestable wrapper" move the
/// editor's own `diagnostics.ts` extraction already used, for the identical reason). Every
/// deployment **due** at `now` fires **concurrently** with every other deployment due at the
/// same moment — see `fire_one_clock_tick` and the module doc for why, and for the honest limit
/// of this approach (unbounded per-tick fan-out, not a worker pool).
fn run_due_clock_ticks(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    now: SystemTime,
) {
    let due: Vec<(String, Hash, Option<Arc<Mutex<BTreeMap<String, Value>>>>, u64)> = {
        let deployments = deployments.lock().unwrap();
        deployments
            .iter()
            .filter_map(|(slug, dep)| match (dep.interval_seconds, dep.next_due) {
                (Some(interval), Some(next_due)) if now >= next_due => {
                    Some((slug.clone(), dep.unit, dep.state.clone(), interval))
                }
                _ => None,
            })
            .collect()
    };

    // `thread::scope` (stable since Rust 1.63): spawned threads may borrow `store`/
    // `deployments` directly — no `Arc` clone needed per thread — and every thread is
    // guaranteed joined before `scope()` returns, so this function is still synchronous from
    // its caller's point of view (the existing tests, written before this change, still hold:
    // by the time `run_due_clock_ticks` returns, every due tick from this call has completed).
    // What changes is internal: two deployments due in the same tick no longer block each
    // other — a slow flow only delays itself, not its neighbors.
    //
    // Deliberately NOT a bounded worker pool: a genuinely large number of simultaneously-due
    // deployments spawns that many OS threads at once, unbounded. Fine, and simpler, at the
    // scale this is actually for — a handful of local flows on one machine. Named honestly as
    // a real, open limitation rather than solved here: if per-tick fan-out ever gets large
    // enough to matter, the fix is a bounded worker pool (or a semaphore capping concurrent
    // fires), not more threads. Left open on purpose.
    thread::scope(|scope| {
        for (slug, unit_hash, state, interval) in due {
            scope.spawn(move || {
                fire_one_clock_tick(store, deployments, deployments_path, slug, unit_hash, state, interval, now)
            });
        }
    });
}

/// One deployment's single tick — settle it via `fire_sourced` with the real wall-clock time
/// injected, log the result, append it to that deployment's bounded history. Runs on its own
/// scoped thread when called from `run_due_clock_ticks`'s per-tick fan-out (see there), but has
/// no threading concerns of its own: it only ever touches `store` (internally locked,
/// `Sync`-safe) and this one deployment's own map entry (via the shared `deployments` lock,
/// held only briefly at the end to write the result back).
fn fire_one_clock_tick(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: String,
    unit_hash: Hash,
    state: Option<Arc<Mutex<BTreeMap<String, Value>>>>,
    interval: u64,
    now: SystemTime,
) {
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("clock tick [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, &slug, true);
            return;
        }
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("clock tick [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, &slug, true);
        return;
    };

    let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let is_state = state.is_some();
    let (outputs, performed) = fire_with_state(root_unit, &resolver, state.as_deref(), &extra_sourced);

    println!(
        "clock tick [{slug}] at {epoch_seconds}: outputs={outputs:?}, {} effect(s) performed",
        performed.len()
    );

    let tick = TickEntry {
        at_epoch_seconds: epoch_seconds,
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    };

    {
        let mut deployments = deployments.lock().unwrap();
        if let Some(dep) = deployments.get_mut(&slug) {
            dep.next_due = Some(now + Duration::from_secs(interval));
            dep.trigger_count += 1;
            dep.history.push(tick);
            if dep.history.len() > 20 {
                dep.history.remove(0);
            }
        }
    }
    if is_state {
        persist_deployments(deployments_path, deployments);
    }
}


/// The clock scheduler — the one genuinely new runtime shape in this whole project: a thread
/// that acts on its own, on a timer, with nobody asking. Spawned once at startup with its own
/// `Arc` handle onto `deployments` (no async runtime needed for this workload; a plain sleep
/// loop is enough). Every second, hands off to `run_due_clock_ticks`, which fires whatever's
/// due — concurrently within that one pass, see there for why and for the honest limit.
fn spawn_clock_scheduler(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: String) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        run_due_clock_ticks(&store, &deployments, &deployments_path, SystemTime::now());
    });
}

fn route(state: &AppState, request: &Request) -> Response {
    // `request.url()` returns an owned `String` (not a borrow of `request`) — bound once here
    // rather than called again per branch, both so `strip_prefix` below has something with a
    // long enough lifetime to borrow from, and so the routing table has one clear read of "what
    // path is this."
    let url = request.url();
    let method = request.method();

    if let Some(rejection) = reject_untrusted(request, state.port) {
        return rejection;
    }

    // Preflight — answered before touching any endpoint logic.
    if method == "OPTIONS" {
        return Response::empty_204();
    }

    if method == "GET" && url == "/health" {
        return Response::text("ok");
    }

    if let Some(hash_hex) = url.strip_prefix("/unit/") {
        if method == "GET" {
            return handle_get_unit(&state.store, hash_hex);
        }
    }

    if method == "POST" && url == "/unit" {
        return handle_post_unit(&state.store, request);
    }

    if method == "POST" && url == "/registry/publish" {
        return handle_post_publish(&state.registry, request);
    }

    if method == "GET" && url == "/registry" {
        return handle_get_registry(&state.registry);
    }

    if method == "POST" && url == "/registry/fork" {
        return handle_post_fork(&state.registry, request);
    }

    if method == "POST" && url == "/cache/ingest" {
        return handle_post_cache_ingest(&state.cache, request);
    }

    if let Some(key_hex) = url.strip_prefix("/cache/") {
        if method == "GET" {
            return handle_get_cache(&state.cache, &state.store, key_hex);
        }
    }

    if method == "POST" && url == "/fire" {
        return handle_post_fire(&state.store, request);
    }

    if method == "POST" && url == "/deploy" {
        return handle_post_deploy(&state.store, &state.deployments, &state.deployments_path, request);
    }

    if url == "/flows" {
        if method == "POST" {
            return handle_post_flows(&state.flows, &state.flows_path, request);
        }
        if method == "GET" {
            return handle_get_flows(&state.flows);
        }
    }

    if let Some(name) = url.strip_prefix("/flows/") {
        if method == "GET" {
            return handle_get_flow(&state.flows, name);
        }
        if method == "DELETE" {
            return handle_delete_flow(&state.flows, &state.flows_path, name);
        }
    }

    if method == "GET" && url == "/deployments" {
        return handle_get_deployments(&state.deployments);
    }

    if let Some(rest) = url.strip_prefix("/deployments/") {
        if let Some(slug) = rest.strip_suffix("/history") {
            if method == "GET" {
                return handle_get_history(&state.deployments, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/state") {
            if method == "GET" {
                return handle_get_state(&state.deployments, slug);
            }
        } else if method == "DELETE" {
            return handle_delete_deployment(&state.deployments, &state.deployments_path, rest);
        }
    }

    if let Some(slug) = url.strip_prefix("/trigger/") {
        if method == "POST" {
            return handle_post_trigger(&state.store, &state.deployments, &state.deployments_path, slug, request);
        }
    }

    Response::empty_404()
}

/// A trivial, valid unit for round-trip testing only — the same `const` shape already used as
/// a test helper elsewhere in this project (e.g. `holon-host`'s own test module), not a new
/// invention. It doesn't need to be evaluable for this purpose, just a real, canonically
/// encodable `Unit`.
fn test_unit() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", "Number"), Param::frozen("value", "42")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// A one-child passthrough composite referencing `leaf_hash` — the same `pass_through` shape
/// used as a test helper in `holon-core`'s own `simulate.rs` tests, reused rather than
/// reinvented. Exists only to exercise `Registry::publish`'s `DanglingReference` path: publish
/// this *before* its leaf has been staged via `POST /unit`, and publish should refuse, naming
/// exactly `leaf_hash`.
fn test_composite(leaf_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![leaf_hash],
            edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// A second, distinct leaf (same shape as `test_unit()`, a different frozen value so its hash
/// differs) — for exercising `POST /registry/fork`: re-pin `test_composite`'s child from
/// `test_unit()`'s hash to this one's.
fn test_replacement() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", "Number"), Param::frozen("value", "99")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// The `egress` sink primitive — the same shape used as a test helper throughout `holon-core`
/// and `holon-host`'s own tests, reused rather than reinvented. Under Fork B a sink is a pure
/// request-constructor (`eval` settles it to `Value::Effect{sink:"egress",payload}` without any
/// IO), and `fire` is what actually performs whatever the settled output carries — so firing
/// this unit with a real message input is what finally exercises `RecordingHost::perform` and
/// gives `POST /fire`'s `performed` list something real to report.
fn test_sink() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "egress".into() },
        params: vec![],
        inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// The `ingress` driver primitive — zero inputs, output `Any` (its real genesis interface,
/// confirmed against `catalog.rs`, not assumed). For testing `POST /trigger/<slug>`: nothing
/// this session has fired a real driver leaf before, so this is a genuinely new shape, not a
/// variant of `test_sink`/`test_unit`.
fn test_ingress() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "ingress".into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Any }],
    }
}

/// `ingress → egress`: whatever `/trigger/<slug>` injects for `NetworkIngress` flows straight
/// through to `egress`'s payload. Exercises both halves of `POST /trigger/<slug>` at once —
/// the *sourced* substitution (this composite's own settled output should equal whatever was
/// sent) and `performed` (the `egress` sink should show up, same as `test_sink` alone proved
/// for `/fire`, but now reached via a real driver instead of a positional input).
fn test_ingress_flow(ingress_hash: Hash, egress_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![ingress_hash, egress_hash],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// The `clock` driver primitive — zero inputs, output `Number` (its real genesis interface;
/// unlike `ingress`, this one is concretely typed, not `Any`).
fn test_clock() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "clock".into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// `clock → egress`: wire-compatible because `egress`'s `payload` port is `Any` (`compatible`'s
/// rule: `dst==Any` accepts anything). For exercising `POST /deploy`'s derived-`Clock` path and
/// `spawn_clock_scheduler` end to end — every tick should record a `NetworkEgress` carrying
/// that tick's epoch-seconds value.
fn test_clock_flow(clock_hash: Hash, egress_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![clock_hash, egress_hash],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

fn main() {
    let addr = std::env::var("HOLON_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:7420".to_string());

    // `--store <dir>` for an on-disk FileStore; `--store :memory:` for an in-process MemStore
    // (nothing persisted across restarts — useful for a quick local session or a test run).
    // Defaults to a `.holon-store` directory next to wherever the binary is run from.
    //
    // `--deployments <path>` for where the deployments table is persisted (write-through, on
    // every deploy/delete); `--deployments :memory:` opts out entirely, same convention as
    // `--store`. Defaults to `.holon-deployments.json` next to wherever the binary is run from.
    //
    // `--emit-test-unit <path>` is a dev-only escape hatch, not a server flag: it writes a
    // trivial unit's canonical preimage bytes to `path` and exits *without* starting the
    // server, so the `/unit`/`/registry` round trips can be checked with plain `curl` + a file
    // diff — no HTTP client dependency needed just to generate a valid test payload.
    //
    // `--emit-test-pair <leaf-path> <composite-path>` is the same idea, extended to exercise
    // `Registry::publish`'s `DanglingReference` refusal: a leaf plus a composite that
    // references the leaf's real hash, written as two files. Publishing the composite before
    // staging the leaf should 400; staging the leaf via `/unit` first, then publishing the
    // composite, should succeed.
    //
    // `--emit-test-replacement <path>` writes a second, distinct leaf (same shape, a different
    // frozen value, so a different hash) — the replacement unit for exercising
    // `POST /registry/fork` against the composite from `--emit-test-pair`.
    //
    // `--emit-test-sink <path>` writes an `egress` unit (one Text input, no params) — stage it
    // via `/unit`, then `POST /fire?root=<its hash>` with a real message input to see
    // `RecordingHost` actually record something, unlike the pure `--emit-test-unit` leaf, which
    // triggers nothing.
    let mut args = std::env::args().skip(1);
    let mut store_arg = ".holon-store".to_string();
    let mut deployments_arg = ".holon-deployments.json".to_string();
    let mut flows_arg = ".holon-flows.json".to_string();
    while let Some(a) = args.next() {
        if a == "--store" {
            if let Some(v) = args.next() {
                store_arg = v;
            }
        } else if a == "--deployments" {
            if let Some(v) = args.next() {
                deployments_arg = v;
            }
        } else if a == "--flows" {
            if let Some(v) = args.next() {
                flows_arg = v;
            }
        } else if a == "--emit-test-unit" {
            let Some(path) = args.next() else {
                eprintln!("--emit-test-unit needs a path");
                std::process::exit(1);
            };
            let bytes = encode_preimage(&test_unit()).expect("test_unit() is a fixed, valid unit");
            std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write {path}: {e}"));
            println!("wrote {} bytes to {path}", bytes.len());
            return;
        } else if a == "--emit-test-pair" {
            let (Some(leaf_path), Some(composite_path)) = (args.next(), args.next()) else {
                eprintln!("--emit-test-pair needs <leaf-path> <composite-path>");
                std::process::exit(1);
            };
            let leaf = test_unit();
            let leaf_hash = hash_unit(&leaf).expect("test_unit() is a fixed, valid unit");
            let leaf_bytes = encode_preimage(&leaf).expect("test_unit() is a fixed, valid unit");
            let composite_bytes = encode_preimage(&test_composite(leaf_hash))
                .expect("test_composite() is a fixed, valid unit given any leaf hash");
            std::fs::write(&leaf_path, &leaf_bytes)
                .unwrap_or_else(|e| panic!("write {leaf_path}: {e}"));
            std::fs::write(&composite_path, &composite_bytes)
                .unwrap_or_else(|e| panic!("write {composite_path}: {e}"));
            println!(
                "wrote leaf ({} bytes, hash {}) to {leaf_path}",
                leaf_bytes.len(),
                to_hex(&leaf_hash)
            );
            println!(
                "wrote composite referencing it ({} bytes) to {composite_path}",
                composite_bytes.len()
            );
            println!("try publishing the composite BEFORE staging the leaf — it should 400.");
            return;
        } else if a == "--emit-test-replacement" {
            let Some(path) = args.next() else {
                eprintln!("--emit-test-replacement needs a path");
                std::process::exit(1);
            };
            let bytes = encode_preimage(&test_replacement())
                .expect("test_replacement() is a fixed, valid unit");
            std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write {path}: {e}"));
            println!("wrote {} bytes to {path} — a distinct unit for /registry/fork's replacement", bytes.len());
            return;
        } else if a == "--emit-test-sink" {
            let Some(path) = args.next() else {
                eprintln!("--emit-test-sink needs a path");
                std::process::exit(1);
            };
            let bytes = encode_preimage(&test_sink()).expect("test_sink() is a fixed, valid unit");
            std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write {path}: {e}"));
            println!(
                "wrote {} bytes to {path} — an egress sink; stage it via /unit, then POST /fire \
                 with {{\"inputs\":[{{\"type\":\"Text\",\"value\":\"hello\"}}]}} to see it in performed",
                bytes.len()
            );
            return;
        } else if a == "--emit-test-ingress-flow" {
            let (Some(ingress_path), Some(egress_path), Some(composite_path)) =
                (args.next(), args.next(), args.next())
            else {
                eprintln!("--emit-test-ingress-flow needs <ingress-path> <egress-path> <composite-path>");
                std::process::exit(1);
            };
            let ingress = test_ingress();
            let ingress_hash = hash_unit(&ingress).expect("test_ingress() is a fixed, valid unit");
            let ingress_bytes = encode_preimage(&ingress).expect("test_ingress() is a fixed, valid unit");
            let egress = test_sink();
            let egress_hash = hash_unit(&egress).expect("test_sink() is a fixed, valid unit");
            let egress_bytes = encode_preimage(&egress).expect("test_sink() is a fixed, valid unit");
            let composite = test_ingress_flow(ingress_hash, egress_hash);
            let composite_hash =
                hash_unit(&composite).expect("test_ingress_flow() is a fixed, valid unit");
            let composite_bytes = encode_preimage(&composite)
                .expect("test_ingress_flow() is a fixed, valid unit given any two leaf hashes");
            std::fs::write(&ingress_path, &ingress_bytes)
                .unwrap_or_else(|e| panic!("write {ingress_path}: {e}"));
            std::fs::write(&egress_path, &egress_bytes)
                .unwrap_or_else(|e| panic!("write {egress_path}: {e}"));
            std::fs::write(&composite_path, &composite_bytes)
                .unwrap_or_else(|e| panic!("write {composite_path}: {e}"));
            println!("wrote ingress ({} bytes) to {ingress_path}", ingress_bytes.len());
            println!("wrote egress ({} bytes) to {egress_path}", egress_bytes.len());
            println!(
                "wrote composite ingress->egress ({} bytes, hash {}) to {composite_path}",
                composite_bytes.len(),
                to_hex(&composite_hash)
            );
            println!("stage all three via /unit (order doesn't matter), then /deploy the composite's hash, then POST /trigger/<slug>.");
            return;
        } else if a == "--emit-test-clock-flow" {
            let (Some(clock_path), Some(egress_path), Some(composite_path)) =
                (args.next(), args.next(), args.next())
            else {
                eprintln!("--emit-test-clock-flow needs <clock-path> <egress-path> <composite-path>");
                std::process::exit(1);
            };
            let clock = test_clock();
            let clock_hash = hash_unit(&clock).expect("test_clock() is a fixed, valid unit");
            let clock_bytes = encode_preimage(&clock).expect("test_clock() is a fixed, valid unit");
            let egress = test_sink();
            let egress_hash = hash_unit(&egress).expect("test_sink() is a fixed, valid unit");
            let egress_bytes = encode_preimage(&egress).expect("test_sink() is a fixed, valid unit");
            let composite = test_clock_flow(clock_hash, egress_hash);
            let composite_hash =
                hash_unit(&composite).expect("test_clock_flow() is a fixed, valid unit");
            let composite_bytes = encode_preimage(&composite)
                .expect("test_clock_flow() is a fixed, valid unit given any two leaf hashes");
            std::fs::write(&clock_path, &clock_bytes)
                .unwrap_or_else(|e| panic!("write {clock_path}: {e}"));
            std::fs::write(&egress_path, &egress_bytes)
                .unwrap_or_else(|e| panic!("write {egress_path}: {e}"));
            std::fs::write(&composite_path, &composite_bytes)
                .unwrap_or_else(|e| panic!("write {composite_path}: {e}"));
            println!("wrote clock ({} bytes) to {clock_path}", clock_bytes.len());
            println!("wrote egress ({} bytes) to {egress_path}", egress_bytes.len());
            println!(
                "wrote composite clock->egress ({} bytes, hash {}) to {composite_path}",
                composite_bytes.len(),
                to_hex(&composite_hash)
            );
            println!(
                "stage all three via /unit, then POST /deploy with {{\"slug\":\"...\",\"unit\":\"<hash>\",\"interval_seconds\":5}}."
            );
            return;
        }
    }

    let backend: DynStore = if store_arg == ":memory:" {
        println!("holon-server: in-memory store (nothing persists past this run)");
        Box::new(MemStore::new())
    } else {
        let store = FileStore::new(&store_arg)
            .unwrap_or_else(|e| panic!("could not open store at {store_arg:?}: {e}"));
        println!("holon-server: file store at {store_arg:?}");
        Box::new(store)
    };

    let store = SharedStore(Arc::new(Mutex::new(backend)));
    let registry = Mutex::new(Registry::new(store.clone()));
    let cache = Mutex::new(SharedCache::new());
    let refuse_to_start = |e: String| -> ! {
        eprintln!("holon-server: {e}");
        eprintln!("holon-server: refusing to start rather than overwrite it — fix the file or move it aside");
        std::process::exit(1);
    };
    let initial_deployments = load_deployments(&deployments_arg, &store).unwrap_or_else(|e| refuse_to_start(e));
    let flows = Mutex::new(load_flows(&flows_arg).unwrap_or_else(|e| refuse_to_start(e)));
    let deployments: Arc<Mutex<BTreeMap<String, Deployment>>> = Arc::new(Mutex::new(initial_deployments));
    spawn_clock_scheduler(store.clone(), Arc::clone(&deployments), deployments_arg.clone());
    let port = addr
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|e| panic!("HOLON_SERVER_ADDR {addr:?} is not host:port: {e}"))
        .port();
    let state =
        AppState { store, registry, cache, deployments, deployments_path: deployments_arg, flows, flows_path: flows_arg, port };

    println!("holon-server: listening on http://{addr} (localhost-only, no auth — see the L4)");
    rouille::start_server(addr, move |request| with_cors(request, route(&state, request)));
}

/// In-process integration tests. No real server, no network, no port — `route()`/`with_cors()`
/// are plain functions over `Request`/`Response`, and `Request::fake_http` builds a real
/// `Request` value without a socket, so every endpoint is exercised exactly as the real server
/// would run it, just called directly. This replaces the manual `curl` walkthroughs used to
/// verify each inch while building this crate — a real gap, named plainly rather than left
/// implicit, once the surface grew past the first couple of endpoints. `run_due_clock_ticks`
/// (pulled out of `spawn_clock_scheduler` for exactly this reason) means the scheduler is
/// tested with a controlled `now`, not real sleeping.
#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let backend: DynStore = Box::new(MemStore::new());
        let store = SharedStore(Arc::new(Mutex::new(backend)));
        let registry = Mutex::new(Registry::new(store.clone()));
        let cache = Mutex::new(SharedCache::new());
        let deployments = Arc::new(Mutex::new(BTreeMap::new()));
        AppState {
            store,
            registry,
            cache,
            deployments,
            deployments_path: ":memory:".to_string(),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: ":memory:".to_string(),
            port: TEST_PORT,
        }
    }

    const TEST_PORT: u16 = 7420;

    fn host_header() -> (String, String) {
        ("Host".to_string(), format!("127.0.0.1:{TEST_PORT}"))
    }

    /// A request as the editor sends it: our Host, and for a POST the Content-Type its body is
    /// (a JSON body starts with `{` or `[`; preimages and trigger text go as raw bytes).
    fn req(method: &str, url: &str, body: Vec<u8>) -> Request {
        let mut headers = vec![host_header()];
        if method == "POST" {
            let json = matches!(body.first(), Some(b'{' | b'['));
            let ty = if json { "application/json" } else { "application/octet-stream" };
            headers.push(("Content-Type".to_string(), ty.to_string()));
        }
        Request::fake_http(method, url, headers, body)
    }

    fn body_bytes(response: Response) -> Vec<u8> {
        let (mut reader, _) = response.data.into_reader_and_size();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        buf
    }

    fn body_json(response: Response) -> serde_json::Value {
        serde_json::from_slice(&body_bytes(response)).expect("response body was not valid JSON")
    }

    /// A unique path in the OS temp dir, for the one test that needs real disk IO
    /// (persistence) rather than the `:memory:` isolation every other test uses. Not a
    /// directory or file by itself — `FileStore::new` creates the directory it needs;
    /// `std::fs::write` creates the file it needs.
    fn unique_temp_path(label: &str) -> String {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("holon-server-test-{label}-{}-{nanos}", std::process::id()));
        path.to_string_lossy().to_string()
    }

    /// Stages a full `test_stateful_counter` graph (a driver leaf — `test_ingress()` or
    /// `test_clock()` — plus `store_read(key)`/`one`/`add`/`store_write(key)`) and returns the
    /// composite's hash, ready to deploy.
    /// `store_read(key)` — reads this deployment's own state at `key`.
    fn test_store_read(key: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "store_read".into() },
            params: vec![Param::frozen("key", key)],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    /// `store_write(key)` — writes this deployment's own state at `key`.
    fn test_store_write(key: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "store_write".into() },
            params: vec![Param::frozen("key", key)],
            inputs: vec![Port { name: "value".into(), ty: Type::Any }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        }
    }

    /// `driver -> store_write(key)`: writes whatever the trigger supplies (the ingress body as
    /// text, or the clock's own epoch number) directly to this deployment's state at `key` —
    /// no read involved. Deliberately write-only, not a read-and-increment counter: a
    /// `store_read → add → store_write` graph cannot bootstrap its own first value with only
    /// this primitive set — an absent key's read fails the *whole* single-output settle
    /// (`fire_sourced`'s own honest-absence rule), and there's no coalesce-with-default
    /// primitive to route around that. A real, named limitation of the primitive set itself,
    /// not something to fight with a contrived test. This unit tests exactly what this inch
    /// actually delivers instead: a write from one fire is still there for the *next* fire to
    /// see — checked via `GET /deployments/<slug>/state`, not by asking the flow to read back
    /// its own prior write.
    fn test_stateful_writer(driver_hash: Hash, write_hash: Hash) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children: vec![driver_hash, write_hash],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // driver → store_write.value
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // store_write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        }
    }

    /// The `random` driver primitive — zero inputs, output `Number`, same shape as `clock`.
    fn test_random() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "random".into() },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }

    /// `ingress` (unwired, present only so the composite derives `NetworkIngress` and is
    /// therefore deployable at all — same convention `stage_stateful_reader` already uses)
    /// alongside `driver` wired straight to the boundary output. For proving a *second*
    /// driver's own capability is honestly sourced even when `NetworkIngress` is what actually
    /// triggered the fire — the exact gap `Random` (and `Clock`-via-`/trigger`) had.
    fn stage_ingress_plus_driver(state: &AppState, driver: Unit) -> Hash {
        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));

        let driver_hash = hash_unit(&driver).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&driver).unwrap()));

        let composite = Unit {
            body: UnitBody::Composite {
                children: vec![ingress_hash, driver_hash],
                edges: vec![Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let composite_hash = hash_unit(&composite).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
        composite_hash
    }

    fn stage_stateful_writer(state: &AppState, driver: Unit, key: &str) -> Hash {
        let driver_hash = hash_unit(&driver).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&driver).unwrap()));

        let write = test_store_write(key);
        let write_hash = hash_unit(&write).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&write).unwrap()));

        let composite = test_stateful_writer(driver_hash, write_hash);
        let composite_hash = hash_unit(&composite).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));

        composite_hash
    }

    /// `ingress, store_read(key) -> egress`: for testing the honest-absence side directly —
    /// a fresh deployment's never-written key evaluates to nothing, so the whole single-output
    /// settle fails, proven end to end through `holon-server`, not just at the core level.
    fn stage_stateful_reader(state: &AppState, key: &str) -> Hash {
        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));

        let read = test_store_read(key);
        let read_hash = hash_unit(&read).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&read).unwrap()));

        let egress = test_sink();
        let egress_hash = hash_unit(&egress).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&egress).unwrap()));

        let composite = Unit {
            body: UnitBody::Composite {
                // ingress is an unwired child, present only so this composite derives
                // NetworkIngress and is therefore deployable/triggerable at all.
                children: vec![ingress_hash, read_hash, egress_hash],
                edges: vec![
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 }, // store_read → egress
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // egress → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        let composite_hash = hash_unit(&composite).unwrap();
        route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
        composite_hash
    }

    // ── /health ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn health_check() {
        let state = test_state();
        let r = route(&state, &req("GET", "/health", vec![]));
        assert_eq!(r.status_code, 200);
        assert_eq!(body_bytes(r), b"ok");
    }

    // ── /unit ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn unit_round_trips() {
        let state = test_state();
        let bytes = encode_preimage(&test_unit()).unwrap();
        let expected_hash = to_hex(&hash_unit(&test_unit()).unwrap());

        let post = route(&state, &req("POST", "/unit", bytes.clone()));
        assert_eq!(post.status_code, 200);
        assert_eq!(body_json(post)["hash"], expected_hash);

        let get = route(&state, &req("GET", &format!("/unit/{expected_hash}"), vec![]));
        assert_eq!(get.status_code, 200);
        assert_eq!(body_bytes(get), bytes);
    }

    #[test]
    fn unit_get_rejects_bad_hash_and_404s_a_missing_one() {
        let state = test_state();
        assert_eq!(route(&state, &req("GET", "/unit/deadbeef", vec![])).status_code, 400);

        let zeros = "0".repeat(64);
        assert_eq!(route(&state, &req("GET", &format!("/unit/{zeros}"), vec![])).status_code, 404);
    }

    // ── /registry ────────────────────────────────────────────────────────────────────────

    #[test]
    fn publish_refuses_dangling_reference_then_succeeds_once_staged() {
        let state = test_state();
        let leaf_hash = hash_unit(&test_unit()).unwrap();
        let composite_bytes = encode_preimage(&test_composite(leaf_hash)).unwrap();

        let refused = route(&state, &req("POST", "/registry/publish", composite_bytes.clone()));
        assert_eq!(refused.status_code, 400);

        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));

        let published = route(&state, &req("POST", "/registry/publish", composite_bytes));
        assert_eq!(published.status_code, 200);

        let list = route(&state, &req("GET", "/registry", vec![]));
        assert_eq!(body_json(list)["published"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn fork_replaces_target_and_keeps_original_published() {
        let state = test_state();
        let leaf_hash = hash_unit(&test_unit()).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let composite = test_composite(leaf_hash);
        let composite_hash = hash_unit(&composite).unwrap();
        route(&state, &req("POST", "/registry/publish", encode_preimage(&composite).unwrap()));

        let replacement_bytes = encode_preimage(&test_replacement()).unwrap();
        let url =
            format!("/registry/fork?root={}&target={}", to_hex(&composite_hash), to_hex(&leaf_hash));
        let forked = route(&state, &req("POST", &url, replacement_bytes));
        assert_eq!(forked.status_code, 200);
        let new_hash = body_json(forked)["hash"].as_str().unwrap().to_string();
        assert_ne!(new_hash, to_hex(&composite_hash));

        let list = route(&state, &req("GET", "/registry", vec![]));
        let published: Vec<String> = body_json(list)["published"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(published.contains(&to_hex(&composite_hash)));
        assert!(published.contains(&new_hash));
    }

    #[test]
    fn fork_rejects_unknown_root_and_target_outside_closure() {
        let state = test_state();
        let leaf_hash = hash_unit(&test_unit()).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let composite = test_composite(leaf_hash);
        let composite_hash = hash_unit(&composite).unwrap();
        route(&state, &req("POST", "/registry/publish", encode_preimage(&composite).unwrap()));
        let replacement_bytes = encode_preimage(&test_replacement()).unwrap();

        let bad_root = "de".repeat(32);
        let url1 = format!("/registry/fork?root={bad_root}&target={}", to_hex(&leaf_hash));
        assert_eq!(route(&state, &req("POST", &url1, replacement_bytes.clone())).status_code, 404);

        let zero_target = "0".repeat(64);
        let url2 = format!("/registry/fork?root={}&target={zero_target}", to_hex(&composite_hash));
        assert_eq!(route(&state, &req("POST", &url2, replacement_bytes)).status_code, 400);
    }

    // ── /cache ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn cache_ingest_rejects_key_mismatch_then_succeeds_under_the_real_key() {
        use holon_core::entry_key;

        let state = test_state();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let leaf_hash = hash_unit(&test_unit()).unwrap();

        let entry =
            CacheEntry { unit_hash: leaf_hash, inputs: vec![], outputs: vec![Some(Value::Number(42.0))] };
        let real_key = entry_key(&entry).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "unit_hash": to_hex(&leaf_hash),
            "inputs": [],
            "outputs": [{"type": "Number", "value": 42.0}],
        }))
        .unwrap();

        let zero_key = "0".repeat(64);
        let mismatch = route(&state, &req("POST", &format!("/cache/ingest?key={zero_key}"), body.clone()));
        assert_eq!(mismatch.status_code, 400);

        let ok = route(&state, &req("POST", &format!("/cache/ingest?key={}", to_hex(&real_key)), body));
        assert_eq!(ok.status_code, 200);

        let get = route(&state, &req("GET", &format!("/cache/{}", to_hex(&real_key)), vec![]));
        assert_eq!(get.status_code, 200);
        assert_eq!(body_json(get)["outputs"][0]["value"], 42.0);
    }

    #[test]
    fn cache_verified_read_refuses_a_lying_entry() {
        use holon_core::entry_key;

        let state = test_state();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let leaf_hash = hash_unit(&test_unit()).unwrap();

        // leaf really evaluates to 42, but this entry claims 999.
        let lie =
            CacheEntry { unit_hash: leaf_hash, inputs: vec![], outputs: vec![Some(Value::Number(999.0))] };
        let key = entry_key(&lie).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "unit_hash": to_hex(&leaf_hash),
            "inputs": [],
            "outputs": [{"type": "Number", "value": 999.0}],
        }))
        .unwrap();
        let ingested = route(&state, &req("POST", &format!("/cache/ingest?key={}", to_hex(&key)), body));
        assert_eq!(ingested.status_code, 200); // structurally valid — ingest doesn't re-execute

        let get = route(&state, &req("GET", &format!("/cache/{}", to_hex(&key)), vec![]));
        assert_eq!(get.status_code, 422); // but the verified read catches the lie
    }

    // ── /fire ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn fire_pure_unit_settles_with_no_performed_effects() {
        let state = test_state();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let leaf_hash = hash_unit(&test_unit()).unwrap();

        let body = serde_json::to_vec(&serde_json::json!({"inputs": []})).unwrap();
        let r = route(&state, &req("POST", &format!("/fire?root={}", to_hex(&leaf_hash)), body));
        assert_eq!(r.status_code, 200);
        let v = body_json(r);
        assert_eq!(v["outputs"][0]["value"], 42.0);
        assert_eq!(v["performed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn fire_sink_unit_records_a_performed_effect() {
        let state = test_state();
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let sink_hash = hash_unit(&test_sink()).unwrap();

        let body =
            serde_json::to_vec(&serde_json::json!({"inputs": [{"type": "Text", "value": "hi"}]})).unwrap();
        let r = route(&state, &req("POST", &format!("/fire?root={}", to_hex(&sink_hash)), body));
        assert_eq!(r.status_code, 200);
        let v = body_json(r);
        assert_eq!(v["performed"][0]["capability"], "NetworkEgress");
        assert_eq!(v["performed"][0]["payload"]["value"], "hi");
    }

    // ── /deploy + /trigger ───────────────────────────────────────────────────────────────

    #[test]
    fn deploy_derives_ingress_trigger_and_the_trigger_endpoint_fires_it() {
        let state = test_state();
        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "echo",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        let d = route(&state, &req("POST", "/deploy", deploy_body));
        assert_eq!(d.status_code, 200);
        assert_eq!(body_json(d)["triggers"].as_array().unwrap(), &vec![serde_json::json!("Ingress")]);

        let list = route(&state, &req("GET", "/deployments", vec![]));
        assert_eq!(body_json(list)["deployments"].as_array().unwrap().len(), 1);

        let triggered = route(&state, &req("POST", "/trigger/echo", b"hello from test".to_vec()));
        assert_eq!(triggered.status_code, 200);
        let tv = body_json(triggered);
        assert_eq!(tv["performed"][0]["capability"], "NetworkEgress");
        assert_eq!(tv["performed"][0]["payload"]["value"], "hello from test");
    }

    #[test]
    fn trigger_unknown_slug_is_404() {
        let state = test_state();
        assert_eq!(route(&state, &req("POST", "/trigger/nope", b"x".to_vec())).status_code, 404);
    }

    #[test]
    fn deploy_rejects_a_unit_with_no_driver_capability() {
        let state = test_state();
        route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
        let leaf_hash = hash_unit(&test_unit()).unwrap();
        let body =
            serde_json::to_vec(&serde_json::json!({"slug": "nope", "unit": to_hex(&leaf_hash)})).unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 400);
    }

    #[test]
    fn deploy_clock_unit_requires_a_valid_interval() {
        let state = test_state();
        let clock = test_clock();
        let clock_hash = hash_unit(&clock).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_clock_flow(clock_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

        let no_interval = serde_json::to_vec(&serde_json::json!({
            "slug": "tick",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", no_interval)).status_code, 400);

        let zero_interval = serde_json::to_vec(&serde_json::json!({
            "slug": "tick",
            "unit": to_hex(&flow_hash),
            "interval_seconds": 0,
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", zero_interval)).status_code, 400);

        let ok = serde_json::to_vec(&serde_json::json!({
            "slug": "tick",
            "unit": to_hex(&flow_hash),
            "interval_seconds": 5,
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        let r = route(&state, &req("POST", "/deploy", ok));
        assert_eq!(r.status_code, 200);
        assert_eq!(body_json(r)["triggers"].as_array().unwrap(), &vec![serde_json::json!("Clock")]);
    }

    // ── the clock scheduler, with a controlled clock — no real sleeping ────────────────────

    #[test]
    fn clock_ticks_fire_when_due_and_not_before() {
        let state = test_state();
        let clock = test_clock();
        let clock_hash = hash_unit(&clock).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_clock_flow(clock_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "tick",
            "unit": to_hex(&flow_hash),
            "interval_seconds": 5,
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_body));

        let t0 = SystemTime::now();

        // Due immediately — next_due was set to "now" at deploy time.
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
        let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

        // Not due yet — only 1s of a 5s interval has passed.
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(1));
        let h2 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        assert_eq!(body_json(h2)["ticks"].as_array().unwrap().len(), 1);

        // Due again.
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(6));
        let h3 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        let ticks = body_json(h3)["ticks"].as_array().unwrap().clone();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0]["performed"][0]["capability"], "NetworkEgress");
    }

    #[test]
    fn deployments_history_for_unknown_slug_is_404() {
        let state = test_state();
        assert_eq!(route(&state, &req("GET", "/deployments/nope/history", vec![])).status_code, 404);
    }

    #[test]
    fn delete_deployment_stops_ingress_trigger_and_removes_it_from_the_list() {
        let state = test_state();
        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "echo",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_body));

        // Works before deletion.
        assert_eq!(route(&state, &req("POST", "/trigger/echo", b"x".to_vec())).status_code, 200);

        let deleted = route(&state, &req("DELETE", "/deployments/echo", vec![]));
        assert_eq!(deleted.status_code, 200);
        assert_eq!(body_json(deleted)["unit"], to_hex(&flow_hash));

        // 404s after deletion — same as a slug that was never deployed.
        assert_eq!(route(&state, &req("POST", "/trigger/echo", b"x".to_vec())).status_code, 404);
        let list = route(&state, &req("GET", "/deployments", vec![]));
        assert_eq!(body_json(list)["deployments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn delete_deployment_stops_future_clock_ticks() {
        let state = test_state();
        let clock = test_clock();
        let clock_hash = hash_unit(&clock).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_clock_flow(clock_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "tick",
            "unit": to_hex(&flow_hash),
            "interval_seconds": 5,
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_body));

        let t0 = SystemTime::now();
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
        let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

        assert_eq!(route(&state, &req("DELETE", "/deployments/tick", vec![])).status_code, 200);

        // The scheduler now has nothing to find for this slug — a tick "due" by the old
        // schedule simply doesn't fire, because the deployment isn't in the table anymore.
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(6));
        assert_eq!(route(&state, &req("GET", "/deployments/tick/history", vec![])).status_code, 404);
    }

    #[test]
    fn delete_unknown_deployment_is_404() {
        let state = test_state();
        assert_eq!(route(&state, &req("DELETE", "/deployments/nope", vec![])).status_code, 404);
    }

    // ── capability-consent ───────────────────────────────────────────────────────────────

    #[test]
    fn deploy_refuses_without_full_capability_acknowledgment() {
        let state = test_state();
        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

        // No acknowledgment at all.
        let no_ack =
            serde_json::to_vec(&serde_json::json!({"slug": "echo", "unit": to_hex(&flow_hash)})).unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", no_ack)).status_code, 400);

        // Only half the real manifest acknowledged — still refused, not partially accepted.
        let partial_ack = serde_json::to_vec(&serde_json::json!({
            "slug": "echo",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", partial_ack)).status_code, 400);

        // The whole derived manifest acknowledged — succeeds.
        let full_ack = serde_json::to_vec(&serde_json::json!({
            "slug": "echo",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", full_ack)).status_code, 200);
    }

    // ── persistence ──────────────────────────────────────────────────────────────────────

    #[test]
    fn deployments_persist_across_a_simulated_restart() {
        let deployments_path = unique_temp_path("deployments");
        let store_dir = unique_temp_path("store");

        // A real on-disk store, not MemStore — the deployment definition surviving only means
        // something if the unit it points at is still there too.
        let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
        let store = SharedStore(Arc::new(Mutex::new(backend)));

        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        let mut writer = store.clone();
        writer.put(&ingress).unwrap();
        writer.put(&test_sink()).unwrap();
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        writer.put(&flow).unwrap();

        // "First run": nothing to load yet, deploy, which persists write-through.
        {
            let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store).unwrap()));
            assert_eq!(deployments.lock().unwrap().len(), 0);

            let state = AppState {
                store: store.clone(),
                registry: Mutex::new(Registry::new(store.clone())),
                cache: Mutex::new(SharedCache::new()),
                deployments,
                deployments_path: deployments_path.clone(),
                flows: Mutex::new(BTreeMap::new()),
                flows_path: ":memory:".to_string(),
                port: TEST_PORT,
            };
            let body = serde_json::to_vec(&serde_json::json!({
                "slug": "echo",
                "unit": to_hex(&flow_hash),
                "acknowledge": ["NetworkIngress", "NetworkEgress"],
            }))
            .unwrap();
            assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
        }

        // "Restart": a fresh load from the same path/store reconstructs the deployment, with
        // triggers re-derived (not read back from disk), and it actually works.
        let reloaded = load_deployments(&deployments_path, &store).unwrap();
        assert_eq!(reloaded.len(), 1);
        let dep = reloaded.get("echo").unwrap();
        assert_eq!(dep.unit, flow_hash);
        assert_eq!(dep.triggers, vec!["Ingress".to_string()]);

        let state2 = AppState {
            store: store.clone(),
            registry: Mutex::new(Registry::new(store.clone())),
            cache: Mutex::new(SharedCache::new()),
            deployments: Arc::new(Mutex::new(reloaded)),
            deployments_path: deployments_path.clone(),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: ":memory:".to_string(),
            port: TEST_PORT,
        };
        let triggered = route(&state2, &req("POST", "/trigger/echo", b"restarted".to_vec()));
        assert_eq!(triggered.status_code, 200);
        assert_eq!(body_json(triggered)["performed"][0]["payload"]["value"], "restarted");

        let _ = std::fs::remove_file(&deployments_path);
        let _ = std::fs::remove_dir_all(&store_dir);
    }

    // ── multi-flow overlap ───────────────────────────────────────────────────────────────

    #[test]
    fn multiple_due_clock_deployments_all_fire_in_one_pass() {
        let state = test_state();
        let clock = test_clock();
        let clock_hash = hash_unit(&clock).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_clock_flow(clock_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

        // The same unit, deployed twice under different slugs — a legitimate real scenario,
        // and enough to exercise the per-tick fan-out (two due deployments in one pass).
        for slug in ["tick-a", "tick-b"] {
            let body = serde_json::to_vec(&serde_json::json!({
                "slug": slug,
                "unit": to_hex(&flow_hash),
                "interval_seconds": 5,
                "acknowledge": ["Clock", "NetworkEgress"],
            }))
            .unwrap();
            assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
        }

        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now());

        for slug in ["tick-a", "tick-b"] {
            let h = route(&state, &req("GET", &format!("/deployments/{slug}/history"), vec![]));
            let ticks = body_json(h)["ticks"].as_array().unwrap().clone();
            assert_eq!(ticks.len(), 1, "slug {slug} should have exactly one tick recorded");
            assert_eq!(ticks[0]["performed"][0]["capability"], "NetworkEgress");
        }
    }

    // ── flows: named workspace documents (save / list / load / delete) ────────────────────

    #[test]
    fn flow_saves_lists_loads_and_deletes() {
        let state = test_state();

        // Save — the document is opaque; positions/viewport are exactly the kind of thing it
        // carries, and the server must hand back byte-identical structure without touching it.
        let doc = serde_json::json!({
            "nodes": [{ "id": "n1", "position": { "x": 120.5, "y": -40.0 }, "data": { "name": "const" } }],
            "edges": [],
            "viewport": { "x": 12.0, "y": 34.0, "zoom": 1.5 },
        });
        let save = serde_json::to_vec(&serde_json::json!({ "name": "greet", "document": doc })).unwrap();
        let r = route(&state, &req("POST", "/flows", save));
        assert_eq!(r.status_code, 200);
        assert_eq!(body_json(r)["name"], "greet");

        // List — name + freshness only, never the document.
        let list = body_json(route(&state, &req("GET", "/flows", vec![])));
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["name"], "greet");
        assert!(list[0].get("document").is_none());

        // Load — the document comes back exactly as saved, viewport and positions intact.
        let loaded = body_json(route(&state, &req("GET", "/flows/greet", vec![])));
        assert_eq!(loaded["document"], doc);

        // Delete, then both lookups agree it's gone.
        assert_eq!(route(&state, &req("DELETE", "/flows/greet", vec![])).status_code, 200);
        assert_eq!(route(&state, &req("GET", "/flows/greet", vec![])).status_code, 404);
        let empty = body_json(route(&state, &req("GET", "/flows", vec![])));
        assert_eq!(empty.as_array().unwrap().len(), 0);
    }

    #[test]
    fn saving_the_same_name_overwrites_it() {
        // A flow is a mutable workspace file — save means save, the same name replaces.
        let state = test_state();
        let v1 = serde_json::to_vec(&serde_json::json!({ "name": "wip", "document": { "rev": 1 } })).unwrap();
        let v2 = serde_json::to_vec(&serde_json::json!({ "name": "wip", "document": { "rev": 2 } })).unwrap();
        assert_eq!(route(&state, &req("POST", "/flows", v1)).status_code, 200);
        assert_eq!(route(&state, &req("POST", "/flows", v2)).status_code, 200);

        let list = body_json(route(&state, &req("GET", "/flows", vec![])));
        assert_eq!(list.as_array().unwrap().len(), 1); // still one flow, not two
        let loaded = body_json(route(&state, &req("GET", "/flows/wip", vec![])));
        assert_eq!(loaded["document"]["rev"], 2);
    }

    #[test]
    fn flow_save_rejects_a_bad_name_and_a_malformed_body() {
        let state = test_state();
        let bad_name =
            serde_json::to_vec(&serde_json::json!({ "name": "no spaces!", "document": {} })).unwrap();
        assert_eq!(route(&state, &req("POST", "/flows", bad_name)).status_code, 400);
        assert_eq!(route(&state, &req("POST", "/flows", b"not json".to_vec())).status_code, 400);
        // Neither attempt saved anything.
        let list = body_json(route(&state, &req("GET", "/flows", vec![])));
        assert_eq!(list.as_array().unwrap().len(), 0);
    }

    #[test]
    fn deleting_an_unknown_flow_is_404() {
        let state = test_state();
        assert_eq!(route(&state, &req("DELETE", "/flows/nope", vec![])).status_code, 404);
    }

    #[test]
    fn flows_persist_across_a_simulated_restart() {
        let flows_path = unique_temp_path("flows");
        let doc = serde_json::json!({ "nodes": [], "viewport": { "zoom": 2.0 } });

        // "First run": save one flow against a real on-disk path, then drop the whole state.
        {
            let mut state = test_state();
            state.flows_path = flows_path.clone();
            let save =
                serde_json::to_vec(&serde_json::json!({ "name": "keeper", "document": doc })).unwrap();
            assert_eq!(route(&state, &req("POST", "/flows", save)).status_code, 200);
        }

        // "Restart": a fresh state loads the table the way main() does, from the same path.
        let mut state2 = test_state();
        state2.flows = Mutex::new(load_flows(&flows_path).unwrap());
        state2.flows_path = flows_path.clone();
        let loaded = body_json(route(&state2, &req("GET", "/flows/keeper", vec![])));
        assert_eq!(loaded["document"], doc);

        let _ = std::fs::remove_file(&flows_path);
    }

    /// Regression: a persistence file that failed to parse used to load as an empty table, so
    /// the first deploy or save after startup overwrote every deployment, its state, or every
    /// flow. Loading is now an `Err` (main() refuses to start) and the file is left untouched.
    #[test]
    fn corrupt_persistence_file_is_an_error_not_an_empty_table() {
        let state = test_state();
        for (label, contents) in [("truncated", &b"[{\"slug\": \"wri"[..]), ("foreign", &b"{\"not\": \"a list\"}"[..])] {
            let path = unique_temp_path(&format!("corrupt-{label}"));
            std::fs::write(&path, contents).unwrap();
            assert!(load_deployments(&path, &state.store).is_err(), "{label} deployments file");
            assert!(load_flows(&path).is_err(), "{label} flows file");
            assert_eq!(std::fs::read(&path).unwrap(), contents, "the bad file must be left as it was");
            let _ = std::fs::remove_file(&path);
        }
        // A missing file is still a normal first run, not an error.
        let missing = unique_temp_path("missing");
        assert!(load_deployments(&missing, &state.store).unwrap().is_empty());
        assert!(load_flows(&missing).unwrap().is_empty());
    }

    /// Persisting goes through `write_atomically`: the file always holds one whole, parseable
    /// table, and no temp file is left next to it.
    #[test]
    fn persist_writes_whole_files_and_leaves_no_temp_files() {
        let dir = unique_temp_path("persist-dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = format!("{dir}/flows.json");
        let mut flows = BTreeMap::new();
        for name in ["a", "b"] {
            flows.insert(
                name.to_string(),
                FlowRecord { name: name.to_string(), saved_at_epoch_seconds: 0, document: serde_json::json!({}) },
            );
            persist_flows(&path, &flows);
        }
        flows.remove("a");
        persist_flows(&path, &flows); // shrinking: no stale tail from the longer write
        assert_eq!(load_flows(&path).unwrap().keys().collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── every honestly-answerable driver is actually sourced, regardless of which one
    // triggered this fire (the Random / Clock-via-/trigger gap) ────────────────────────────

    #[test]
    fn trigger_sources_random_even_though_ingress_is_what_actually_fired_it() {
        // Real bug, not a hypothetical: Random was never supplied by any fire path at all — a
        // random leaf always evaluated to nothing, silently, with no error anywhere in the
        // chain (an absent slot is dropped before compute runs, so a downstream primitive like
        // concat just quietly produces shorter text, never a crash). This is the direct
        // regression test for that.
        let state = test_state();
        let composite_hash = stage_ingress_plus_driver(&state, test_random());
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "rnd",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "Random"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        let r = route(&state, &req("POST", "/trigger/rnd", b"x".to_vec()));
        assert_eq!(r.status_code, 200);
        let v = body_json(r);
        assert_eq!(v["outputs"][0]["type"], "Number"); // a real number, not null
    }

    #[test]
    fn trigger_sources_clock_even_though_ingress_is_what_actually_fired_it() {
        // The same gap, found by auditing rather than reported: Clock was only ever sourced on
        // the scheduler's own tick path, never on a manual /trigger of an Ingress+Clock unit.
        let state = test_state();
        let composite_hash = stage_ingress_plus_driver(&state, test_clock());
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "clk",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "Clock"],
            "interval_seconds": 60,
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        let r = route(&state, &req("POST", "/trigger/clk", b"x".to_vec()));
        assert_eq!(r.status_code, 200);
        let v = body_json(r);
        assert_eq!(v["outputs"][0]["type"], "Number");
    }

    // ── per-deployment state ─────────────────────────────────────────────────────────────

    #[test]
    fn trigger_with_state_persists_across_calls() {
        let state = test_state();
        let composite_hash = stage_stateful_writer(&state, test_ingress(), "counter");
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "writer",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "StateWrite(\"counter\")"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        // First fire: writes "hello".
        let r1 = route(&state, &req("POST", "/trigger/writer", b"hello".to_vec()));
        assert_eq!(r1.status_code, 200);
        let v1 = body_json(r1);
        assert_eq!(v1["performed"][0]["capability"], "StateWrite(\"counter\")");
        assert_eq!(v1["performed"][0]["payload"]["value"], "hello");

        let s1 = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s1["state"]["counter"]["value"], "hello");

        // Second fire, a real *different* call: overwrites — proving this is the same state
        // slot persisting across two separate fires, not something reset or freshly allocated
        // each time.
        let r2 = route(&state, &req("POST", "/trigger/writer", b"world".to_vec()));
        assert_eq!(r2.status_code, 200);
        let s2 = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s2["state"]["counter"]["value"], "world");
    }

    /// Regression: `/deploy` on an existing slug used to install a fresh, empty state map, so
    /// shipping a fix silently erased everything the previous version had written — and a fire
    /// in flight at the time committed into the orphaned old map instead.
    #[test]
    fn redeploy_keeps_state() {
        let state = test_state();
        let deploy = |unit: Hash, acknowledge: serde_json::Value| {
            let body = serde_json::to_vec(&serde_json::json!({
                "slug": "writer",
                "unit": to_hex(&unit),
                "acknowledge": acknowledge,
            }))
            .unwrap();
            assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
        };
        let state_arc = || state.deployments.lock().unwrap()["writer"].state.clone().unwrap();

        let v1 = stage_stateful_writer(&state, test_ingress(), "counter");
        deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
        assert_eq!(route(&state, &req("POST", "/trigger/writer", b"hello".to_vec())).status_code, 200);
        let before = state_arc();

        // Same unit redeployed: state survives, and it is the very same map, not a copy.
        deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
        assert!(Arc::ptr_eq(&before, &state_arc()));
        let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s["state"]["counter"]["value"], "hello");

        // A different stateful version writing another key: the old key is still there.
        let v2 = stage_stateful_writer(&state, test_ingress(), "other");
        deploy(v2, serde_json::json!(["NetworkIngress", "StateWrite(\"other\")"]));
        assert_eq!(route(&state, &req("POST", "/trigger/writer", b"world".to_vec())).status_code, 200);
        let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s["state"]["counter"]["value"], "hello");
        assert_eq!(s["state"]["other"]["value"], "world");

        // A stateless version in between doesn't discard it either.
        let stateless = stage_ingress_plus_driver(&state, test_random());
        deploy(stateless, serde_json::json!(["NetworkIngress", "Random"]));
        deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
        let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s["state"]["counter"]["value"], "hello");
        assert!(Arc::ptr_eq(&before, &state_arc()));
    }

    /// Regression: `persist_deployments` ran with the table lock held, and inside it locked
    /// every deployment's state. So while one deployment was mid-fire (holding its own state
    /// lock), any persist parked on that lock *while holding the table*, and every endpoint
    /// and the clock scheduler waited behind it.
    #[test]
    fn a_fire_in_progress_does_not_stall_the_table() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut state = test_state();
        let path = unique_temp_path("stall");
        state.deployments_path = path.clone();
        let deploy = |state: &AppState, slug: &str, unit: Hash, ack: serde_json::Value| {
            let body = serde_json::to_vec(&serde_json::json!({ "slug": slug, "unit": to_hex(&unit), "acknowledge": ack }))
                .unwrap();
            route(state, &req("POST", "/deploy", body)).status_code
        };
        let a = stage_stateful_writer(&state, test_ingress(), "k");
        assert_eq!(deploy(&state, "a", a, serde_json::json!(["NetworkIngress", "StateWrite(\"k\")"])), 200);

        // Simulate "a" mid-fire: hold its state lock, as fire_with_state does for a whole round.
        let a_state = state.deployments.lock().unwrap()["a"].state.clone().unwrap();
        let mid_fire = a_state.lock().unwrap();

        std::thread::scope(|s| {
            // Deploying "b" persists, and persisting needs "a"'s state, so this waits for "a".
            let deploying = s.spawn(|| deploy(&state, "b", a, serde_json::json!(["NetworkIngress", "StateWrite(\"k\")"])));
            std::thread::sleep(Duration::from_millis(200));

            // ...but the table itself must stay usable meanwhile.
            let (tx, rx) = mpsc::channel();
            let shared = &state;
            s.spawn(move || tx.send(route(shared, &req("GET", "/deployments", vec![])).status_code).unwrap());
            let listed = rx.recv_timeout(Duration::from_secs(5));

            // Let "a" finish before asserting, so a failure reports instead of hanging the scope.
            drop(mid_fire);
            assert_eq!(listed, Ok(200), "GET /deployments stalled behind a fire");
            assert_eq!(deploying.join().unwrap(), 200);
        });

        let reloaded = load_deployments(&path, &state.store).unwrap();
        assert_eq!(reloaded.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        let _ = std::fs::remove_file(&path);
    }

    /// Regression: `ingress → egress → store_write` stored the `Effect` itself, the next fire
    /// panicked in holon-host's `constant_unit` while holding the state lock, and the poisoned
    /// mutex then took down every later `/deploy`, `/deployments` and `/trigger`.
    #[test]
    fn effect_written_to_state_does_not_take_the_server_down() {
        let state = test_state();
        let ingress = test_ingress();
        let sink = test_sink();
        let write = test_store_write("cell");
        let read = test_store_read("cell");
        let hashes: Vec<Hash> = [&ingress, &sink, &write, &read]
            .iter()
            .map(|u| {
                route(&state, &req("POST", "/unit", encode_preimage(u).unwrap()));
                hash_unit(u).unwrap()
            })
            .collect();
        let composite = Unit {
            body: UnitBody::Composite {
                children: hashes,
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // ingress → egress
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 }, // egress → store_write
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 1 }, // store_read
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![
                Port { name: "write".into(), ty: Type::Effect },
                Port { name: "read".into(), ty: Type::Any },
            ],
        };
        let composite_hash = hash_unit(&composite).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "bad-writer",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress", "StateRead(\"cell\")", "StateWrite(\"cell\")"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        for _ in 0..2 {
            assert_eq!(route(&state, &req("POST", "/trigger/bad-writer", b"x".to_vec())).status_code, 200);
        }
        let s = body_json(route(&state, &req("GET", "/deployments/bad-writer/state", vec![])));
        assert!(s["state"].get("cell").is_none(), "an effect request is not data: {s}");
        assert_eq!(route(&state, &req("GET", "/deployments", vec![])).status_code, 200);
    }

    #[test]
    fn clock_tick_with_state_persists_across_ticks() {
        let state = test_state();
        let composite_hash = stage_stateful_writer(&state, test_clock(), "last_tick");
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "clock-writer",
            "unit": to_hex(&composite_hash),
            "interval_seconds": 5,
            "acknowledge": ["Clock", "StateWrite(\"last_tick\")"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        let t0 = SystemTime::now();
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
        let epoch0 = t0.duration_since(UNIX_EPOCH).unwrap().as_secs() as f64;
        let s1 = body_json(route(&state, &req("GET", "/deployments/clock-writer/state", vec![])));
        assert_eq!(s1["state"]["last_tick"]["value"], epoch0);

        let t1 = t0 + Duration::from_secs(6);
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t1);
        let epoch1 = t1.duration_since(UNIX_EPOCH).unwrap().as_secs() as f64;
        let s2 = body_json(route(&state, &req("GET", "/deployments/clock-writer/state", vec![])));
        assert_eq!(s2["state"]["last_tick"]["value"], epoch1);
        assert_ne!(epoch0, epoch1); // genuinely a new value, not a stale re-read
    }

    #[test]
    fn reading_a_never_written_key_evaluates_to_nothing() {
        // The honest-absence side, proven end to end: a fresh deployment's key that's never
        // been written makes the whole single-output settle fail — the same rule
        // `holon-core`'s own tests already prove in isolation, now confirmed through the real
        // trigger path.
        let state = test_state();
        let composite_hash = stage_stateful_reader(&state, "never_written");
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "reader",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "StateRead(\"never_written\")", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

        let r = route(&state, &req("POST", "/trigger/reader", b"x".to_vec()));
        assert_eq!(r.status_code, 200);
        let v = body_json(r);
        assert_eq!(v["outputs"][0], serde_json::Value::Null);
        assert_eq!(v["performed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn deployments_listing_shows_state_and_counters() {
        let state = test_state();
        let stateful_hash = stage_stateful_writer(&state, test_ingress(), "k");
        let deploy_stateful = serde_json::to_vec(&serde_json::json!({
            "slug": "stateful",
            "unit": to_hex(&stateful_hash),
            "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_stateful));

        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
        let deploy_stateless = serde_json::to_vec(&serde_json::json!({
            "slug": "stateless",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_stateless));

        route(&state, &req("POST", "/trigger/stateful", b"x".to_vec()));
        route(&state, &req("POST", "/trigger/stateful", b"y".to_vec()));

        let list = body_json(route(&state, &req("GET", "/deployments", vec![])));
        let entries = list["deployments"].as_array().unwrap();
        let stateful = entries.iter().find(|e| e["slug"] == "stateful").unwrap();
        assert_eq!(stateful["has_state"], true);
        assert_eq!(stateful["trigger_count"], 2);
        assert_eq!(stateful["error_count"], 0);
        let stateless = entries.iter().find(|e| e["slug"] == "stateless").unwrap();
        assert_eq!(stateless["has_state"], false);
        assert_eq!(stateless["trigger_count"], 0);
    }

    #[test]
    fn get_state_404s_for_unknown_slug_and_400s_for_stateless_deployment() {
        let state = test_state();
        assert_eq!(route(&state, &req("GET", "/deployments/nope/state", vec![])).status_code, 404);

        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
        route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
        let egress_hash = hash_unit(&test_sink()).unwrap();
        let flow = test_ingress_flow(ingress_hash, egress_hash);
        let flow_hash = hash_unit(&flow).unwrap();
        route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "stateless",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_body));

        assert_eq!(route(&state, &req("GET", "/deployments/stateless/state", vec![])).status_code, 400);
    }

    #[test]
    fn state_persists_across_a_simulated_restart() {
        let deployments_path = unique_temp_path("state-deployments");
        let store_dir = unique_temp_path("state-store");
        let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
        let store = SharedStore(Arc::new(Mutex::new(backend)));

        let ingress = test_ingress();
        let ingress_hash = hash_unit(&ingress).unwrap();
        let mut writer = store.clone();
        writer.put(&ingress).unwrap();
        let write_leaf = test_store_write("counter");
        let write_hash = hash_unit(&write_leaf).unwrap();
        writer.put(&write_leaf).unwrap();
        let composite = test_stateful_writer(ingress_hash, write_hash);
        let composite_hash = hash_unit(&composite).unwrap();
        writer.put(&composite).unwrap();

        // "First run": deploy and trigger once — writes "before-restart".
        {
            let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store).unwrap()));
            let state = AppState {
                store: store.clone(),
                registry: Mutex::new(Registry::new(store.clone())),
                cache: Mutex::new(SharedCache::new()),
                deployments,
                deployments_path: deployments_path.clone(),
                flows: Mutex::new(BTreeMap::new()),
                flows_path: ":memory:".to_string(),
                port: TEST_PORT,
            };
            let deploy_body = serde_json::to_vec(&serde_json::json!({
                "slug": "writer",
                "unit": to_hex(&composite_hash),
                "acknowledge": ["NetworkIngress", "StateWrite(\"counter\")"],
            }))
            .unwrap();
            assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);
            let triggered = route(&state, &req("POST", "/trigger/writer", b"before-restart".to_vec()));
            assert_eq!(triggered.status_code, 200);
        }

        // "Restart": a fresh load from the same path/store — the state survives, and the
        // deployment can still be triggered again afterward.
        let reloaded = load_deployments(&deployments_path, &store).unwrap();
        let dep = reloaded.get("writer").unwrap();
        let restored_state = dep.state.as_ref().unwrap().lock().unwrap();
        assert_eq!(restored_state.get("counter"), Some(&Value::Text("before-restart".to_string())));
        drop(restored_state);

        let state2 = AppState {
            store: store.clone(),
            registry: Mutex::new(Registry::new(store.clone())),
            cache: Mutex::new(SharedCache::new()),
            deployments: Arc::new(Mutex::new(reloaded)),
            deployments_path: deployments_path.clone(),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: ":memory:".to_string(),
            port: TEST_PORT,
        };
        let triggered2 = route(&state2, &req("POST", "/trigger/writer", b"after-restart".to_vec()));
        assert_eq!(triggered2.status_code, 200);
        let s = body_json(route(&state2, &req("GET", "/deployments/writer/state", vec![])));
        assert_eq!(s["state"]["counter"]["value"], "after-restart");

        let _ = std::fs::remove_file(&deployments_path);
        let _ = std::fs::remove_dir_all(&store_dir);
    }

    // ── CORS ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn cors_allows_localhost_origin_and_omits_the_header_for_others() {
        let state = test_state();

        let allowed =
            Request::fake_http("GET", "/health", vec![host_header(), ("Origin".to_string(), "http://localhost:5173".to_string())], vec![]);
        let r1 = with_cors(&allowed, route(&state, &allowed));
        assert!(r1
            .headers
            .iter()
            .any(|(k, v)| k.as_ref() == "Access-Control-Allow-Origin" && v.as_ref() == "http://localhost:5173"));

        let disallowed =
            Request::fake_http("GET", "/health", vec![host_header(), ("Origin".to_string(), "http://evil.example".to_string())], vec![]);
        let r2 = with_cors(&disallowed, route(&state, &disallowed));
        assert!(!r2.headers.iter().any(|(k, _)| k.as_ref() == "Access-Control-Allow-Origin"));
    }

    #[test]
    fn cors_allows_every_method_this_api_actually_uses() {
        // A real, shipped bug, not a hypothetical: Access-Control-Allow-Methods once listed
        // "GET, POST, OPTIONS" — DELETE was missing. The server-side OPTIONS handling
        // (`route`'s own preflight branch) still answered 204 every time regardless, so nothing
        // about the SERVER looked broken in isolation — a browser reads the *header's own
        // contents*, not just the preflight's status code, and silently refuses to ever send
        // the real DELETE if its method isn't listed. `assert!(..headers..)` alone (the test
        // above) would never have caught this — it only checks the header is PRESENT, never
        // what it actually says. This checks the one thing that matters: every method a real
        // endpoint in this file uses (GET, POST, DELETE) is actually named in the value.
        let state = test_state();
        let req = Request::fake_http(
            "OPTIONS",
            "/deployments/whatever",
            vec![host_header(), ("Origin".to_string(), "http://localhost:5173".to_string())],
            vec![],
        );
        let res = with_cors(&req, route(&state, &req));
        let methods = res
            .headers
            .iter()
            .find(|(k, _)| k.as_ref() == "Access-Control-Allow-Methods")
            .map(|(_, v)| v.as_ref().to_string())
            .unwrap_or_default();
        for method in ["GET", "POST", "DELETE"] {
            assert!(methods.contains(method), "Access-Control-Allow-Methods {methods:?} is missing {method}");
        }
    }

    // ── control-plane request gate ───────────────────────────────────────────────────────

    fn raw(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Request::fake_http(method, url, headers, body.to_vec())
    }

    fn saved_flow_names(state: &AppState) -> Vec<String> {
        state.flows.lock().unwrap().keys().cloned().collect()
    }

    /// Regression: CORS only hid responses, so any web page could still *send* a simple
    /// `text/plain` POST and overwrite a saved flow, deploy, or trigger. And with no Host check,
    /// a DNS-rebinding page was same-origin with full access.
    #[test]
    fn cross_site_and_rebinding_requests_are_refused_before_routing() {
        let state = test_state();
        let host = ("Host", "127.0.0.1:7420");
        let save = br#"{"name": "victim", "document": {}}"#;
        let attacks: [(&str, Vec<(&str, &str)>, u16); 7] = [
            ("simple text/plain POST from another site", vec![host, ("Origin", "http://evil.example"), ("Content-Type", "text/plain")], 403),
            ("JSON POST from another site", vec![host, ("Origin", "http://evil.example"), ("Content-Type", "application/json")], 403),
            ("sandboxed iframe (Origin: null)", vec![host, ("Origin", "null"), ("Content-Type", "application/json")], 403),
            ("look-alike origin", vec![host, ("Origin", "http://localhost.evil.example"), ("Content-Type", "application/json")], 403),
            ("DNS rebinding (attacker's hostname)", vec![("Host", "rebind.evil.example:7420"), ("Content-Type", "application/json")], 403),
            ("wrong port in Host", vec![("Host", "127.0.0.1:9999"), ("Content-Type", "application/json")], 403),
            ("form POST with no Origin", vec![host, ("Content-Type", "application/x-www-form-urlencoded")], 415),
        ];
        for (label, headers, status) in attacks {
            let r = route(&state, &raw("POST", "/flows", &headers, save));
            assert_eq!(r.status_code, status, "{label}");
        }
        assert!(saved_flow_names(&state).is_empty(), "no attack may have saved a flow");

        // A missing Host is refused too, even for a GET.
        assert_eq!(route(&state, &raw("GET", "/health", &[], b"")).status_code, 403);

        // The editor (allowed origin) and curl (no Origin) both still work.
        let editor = [host, ("Origin", "http://localhost:5173"), ("Content-Type", "application/json; charset=utf-8")];
        assert_eq!(route(&state, &raw("POST", "/flows", &editor, save)).status_code, 200);
        let curl = [("Host", "localhost:7420"), ("Content-Type", "application/json")];
        assert_eq!(route(&state, &raw("POST", "/flows", &curl, save)).status_code, 200);
        assert_eq!(saved_flow_names(&state), vec!["victim"]);
    }

    /// Regression: `read_body` had no cap, so one huge POST could exhaust memory.
    #[test]
    fn oversized_body_is_refused() {
        let state = test_state();
        let at_cap = vec![b' '; MAX_BODY_BYTES as usize];
        let r = route(&state, &req("POST", "/flows", at_cap));
        assert_ne!(r.status_code, 413, "a body exactly at the cap is read (and then fails to parse)");
        let over = vec![b' '; MAX_BODY_BYTES as usize + 1];
        assert_eq!(route(&state, &req("POST", "/flows", over)).status_code, 413);
    }
}