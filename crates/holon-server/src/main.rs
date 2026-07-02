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
//! **Inch 7** (this one, `clock-deploy-L4.md`): the other half — `Clock`-bound deployment.
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
//! scoped to `localhost`/`127.0.0.1` origins only — never `*`. No auth: there is no "who" to
//! gatekeep on a single local process, and content-addressing already answers "can I trust
//! what I got back," which is the only question a stranger-facing boundary would have needed
//! auth *for*.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use holon_core::{
    capabilities, child_refs, decode_preimage, encode_preimage, hash_unit, parse_hash,
    Capability, CacheEntry, Edge, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody,
    Value, BOUNDARY,
};
use holon_host::{
    fire, fire_sourced, CacheIngestError, FileStore, ForkError, Host, MemStore, PublishError,
    Registry, SharedCache, Store, StoreError,
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
    /// uses for the store itself. In-memory only, same as `Registry`'s published set and
    /// `SharedCache` — restarting the server forgets every deployment.
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
}

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
/// `Clone` so a `Deployment`'s cached `TickEntry` history can be cloned out for a response
/// without holding the deployments lock across the JSON serialization.
#[derive(Serialize, Clone)]
struct PerformedEntry {
    capability: String,
    payload: SValue,
}

#[derive(Serialize)]
struct FireResponseBody {
    output: Option<SValue>,
    performed: Vec<PerformedEntry>,
}

/// `POST /deploy`'s request body. `unit` must already be resolvable in the store (staged via
/// `/unit`, or published — see the L4's Fork 1: deploy deliberately does not require
/// publication, a separate, marketplace-facing concept). `interval_seconds` is **only**
/// consulted (and required) if the unit's own derived capabilities include `Clock` — nothing
/// about *which* trigger(s) apply is asserted by the caller; that's read off the unit itself.
#[derive(Deserialize)]
struct DeployRequestBody {
    slug: String,
    unit: String,
    interval_seconds: Option<u64>,
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
}

/// One clock tick's result, kept in a deployment's bounded history.
#[derive(Serialize, Clone)]
struct TickEntry {
    at_epoch_seconds: u64,
    output: Option<SValue>,
    performed: Vec<PerformedEntry>,
}

#[derive(Serialize)]
struct HistoryBody {
    slug: String,
    ticks: Vec<TickEntry>,
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
    origin.starts_with("http://localhost:")
        || origin == "http://localhost"
        || origin.starts_with("http://127.0.0.1:")
        || origin == "http://127.0.0.1"
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
                .with_additional_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
                .with_additional_header("Access-Control-Allow-Headers", "Content-Type")
                .with_additional_header("Vary", "Origin");
        }
    }
    response
}

fn read_body(request: &Request) -> Result<Vec<u8>, Response> {
    let Some(mut body) = request.data() else {
        return Err(err(400, "body already consumed"));
    };
    let mut bytes = Vec::new();
    if body.read_to_end(&mut bytes).is_err() {
        return Err(err(400, "failed to read request body"));
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
    performed: Vec<(String, Value)>,
}

impl Host for RecordingHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        // `{capability:?}` (Debug), not a hand-written name table — one fewer place a new
        // Capability variant could be added upstream and silently never reach this mirror.
        self.performed.push((format!("{capability:?}"), payload.clone()));
    }
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
    let output = fire(root_unit, &inputs, &resolver, &mut host);

    Response::json(&FireResponseBody {
        output: output.as_ref().map(SValue::from_core),
        performed: host
            .performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability, payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// A slug must be non-empty ASCII alphanumeric/`-`/`_` only — keeps the simple prefix-based
/// router in `route()` unambiguous (no `/` inside a slug could be confused with a path
/// separator) and avoids any path-traversal-shaped weirdness reaching `strip_prefix`.
fn is_valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `POST /deploy` — register `slug → unit`. The trigger(s) are **derived**, not asserted:
/// `capabilities(unit, resolver)` (the same closure-union already used for capability-consent
/// elsewhere in this project) tells us whether the unit needs `Ingress`, `Clock`, both, or
/// neither — the caller only supplies what genuinely can't be read off the graph
/// (`interval_seconds`, only when `Clock` is present; nothing tells you "every 3600 seconds"
/// from structure alone). A unit deriving neither capability is refused — nothing would ever
/// trigger it. Overwrites silently on a repeat slug (the L4's Fork 2 — a redeploy, not an
/// error, matching `Store::put`'s and `SharedCache::ingest`'s own "dumb, idempotent" precedent).
fn handle_post_deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
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

    deployments.lock().unwrap().insert(
        body.slug.clone(),
        Deployment { unit: unit_hash, triggers: triggers.clone(), interval_seconds, next_due, history: Vec::new() },
    );

    Response::json(&DeployResponseBody { slug: body.slug, unit: to_hex(&unit_hash), triggers, interval_seconds })
}

/// `GET /deployments` — every deployment, its derived triggers, and its interval if clock-bound.
fn handle_get_deployments(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Response {
    let deployments = deployments.lock().unwrap();
    let list = deployments
        .iter()
        .map(|(slug, dep)| DeploymentEntry {
            slug: slug.clone(),
            unit: to_hex(&dep.unit),
            triggers: dep.triggers.clone(),
            interval_seconds: dep.interval_seconds,
        })
        .collect();
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
fn handle_post_trigger(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    request: &Request,
) -> Response {
    let unit_hash = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(dep) => dep.unit,
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
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        return err(500, "deployed unit no longer resolves in the store");
    };

    let sourced = [(Capability::NetworkIngress, Value::Text(body_text))];
    let mut host = RecordingHost { performed: Vec::new() };
    let output = fire_sourced(root_unit, &sourced, &resolver, &mut host);

    Response::json(&FireResponseBody {
        output: output.as_ref().map(SValue::from_core),
        performed: host
            .performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability, payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// The clock scheduler — the one genuinely new runtime shape in this whole project: a thread
/// that acts on its own, on a timer, with nobody asking. Spawned once at startup with its own
/// `Arc` handle onto `deployments` (see the L4 §2 — no async runtime needed for this workload;
/// a plain sleep loop is enough). Every second, checks which `Clock`-triggered deployments are
/// due, fires each via `fire_sourced` with the real wall-clock time injected, logs the result
/// to stdout, and appends it to that deployment's bounded history (the L4's Fork 2). Single,
/// sequential loop — a slow tick simply delays the next check for everyone, never overlaps
/// itself (the L4 §4's accepted, named limitation, not a general solution).
/// The tick-firing logic, pulled out of `spawn_clock_scheduler`'s loop specifically so it's
/// callable directly with a controlled `now` — the same "extract the testable logic out of the
/// untestable wrapper" move the editor's own `diagnostics.ts` extraction already used (see that
/// module's own doc, elsewhere in this project) for the identical reason: a `thread::spawn` +
/// real-time sleep loop can't be exercised by a fast, deterministic test, but the actual
/// decision ("what's due, what does firing it produce") has nothing to do with sleeping and
/// everything to do with comparing `now` against `next_due` — so that part is its own function.
fn run_due_clock_ticks(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, now: SystemTime) {
    let due: Vec<(String, Hash, u64)> = {
        let deployments = deployments.lock().unwrap();
        deployments
            .iter()
            .filter_map(|(slug, dep)| match (dep.interval_seconds, dep.next_due) {
                (Some(interval), Some(next_due)) if now >= next_due => {
                    Some((slug.clone(), dep.unit, interval))
                }
                _ => None,
            })
            .collect()
    };

    for (slug, unit_hash, interval) in due {
        let resolver = match materialize_closure(store, unit_hash) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("clock tick [{slug}]: store error while materializing closure: {e:?}");
                continue;
            }
        };
        let Some(root_unit) = resolver.resolve(&unit_hash) else {
            eprintln!("clock tick [{slug}]: deployed unit no longer resolves in the store");
            continue;
        };

        let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let sourced = [(Capability::Clock, Value::Number(epoch_seconds as f64))];
        let mut host = RecordingHost { performed: Vec::new() };
        let output = fire_sourced(root_unit, &sourced, &resolver, &mut host);

        println!(
            "clock tick [{slug}] at {epoch_seconds}: output={output:?}, {} effect(s) performed",
            host.performed.len()
        );

        let tick = TickEntry {
            at_epoch_seconds: epoch_seconds,
            output: output.as_ref().map(SValue::from_core),
            performed: host
                .performed
                .into_iter()
                .map(|(capability, payload)| PerformedEntry { capability, payload: SValue::from_core(&payload) })
                .collect(),
        };

        let mut deployments = deployments.lock().unwrap();
        if let Some(dep) = deployments.get_mut(&slug) {
            dep.next_due = Some(now + Duration::from_secs(interval));
            dep.history.push(tick);
            if dep.history.len() > 20 {
                dep.history.remove(0);
            }
        }
    }
}

fn spawn_clock_scheduler(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        run_due_clock_ticks(&store, &deployments, SystemTime::now());
    });
}

fn route(state: &AppState, request: &Request) -> Response {
    // `request.url()` returns an owned `String` (not a borrow of `request`) — bound once here
    // rather than called again per branch, both so `strip_prefix` below has something with a
    // long enough lifetime to borrow from, and so the routing table has one clear read of "what
    // path is this."
    let url = request.url();
    let method = request.method();

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
        return handle_post_deploy(&state.store, &state.deployments, request);
    }

    if method == "GET" && url == "/deployments" {
        return handle_get_deployments(&state.deployments);
    }

    if let Some(rest) = url.strip_prefix("/deployments/") {
        if let Some(slug) = rest.strip_suffix("/history") {
            if method == "GET" {
                return handle_get_history(&state.deployments, slug);
            }
        }
    }

    if let Some(slug) = url.strip_prefix("/trigger/") {
        if method == "POST" {
            return handle_post_trigger(&state.store, &state.deployments, slug, request);
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
    while let Some(a) = args.next() {
        if a == "--store" {
            if let Some(v) = args.next() {
                store_arg = v;
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
    let deployments: Arc<Mutex<BTreeMap<String, Deployment>>> = Arc::new(Mutex::new(BTreeMap::new()));
    spawn_clock_scheduler(store.clone(), Arc::clone(&deployments));
    let state = AppState { store, registry, cache, deployments };

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
        AppState { store, registry, cache, deployments }
    }

    fn req(method: &str, url: &str, body: Vec<u8>) -> Request {
        Request::fake_http(method, url, vec![], body)
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
        assert_eq!(v["output"]["value"], 42.0);
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

        let deploy_body =
            serde_json::to_vec(&serde_json::json!({"slug": "echo", "unit": to_hex(&flow_hash)})).unwrap();
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

        let no_interval =
            serde_json::to_vec(&serde_json::json!({"slug": "tick", "unit": to_hex(&flow_hash)})).unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", no_interval)).status_code, 400);

        let zero_interval = serde_json::to_vec(
            &serde_json::json!({"slug": "tick", "unit": to_hex(&flow_hash), "interval_seconds": 0}),
        )
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", zero_interval)).status_code, 400);

        let ok = serde_json::to_vec(
            &serde_json::json!({"slug": "tick", "unit": to_hex(&flow_hash), "interval_seconds": 5}),
        )
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

        let deploy_body = serde_json::to_vec(
            &serde_json::json!({"slug": "tick", "unit": to_hex(&flow_hash), "interval_seconds": 5}),
        )
        .unwrap();
        route(&state, &req("POST", "/deploy", deploy_body));

        let t0 = SystemTime::now();

        // Due immediately — next_due was set to "now" at deploy time.
        run_due_clock_ticks(&state.store, &state.deployments, t0);
        let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

        // Not due yet — only 1s of a 5s interval has passed.
        run_due_clock_ticks(&state.store, &state.deployments, t0 + Duration::from_secs(1));
        let h2 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
        assert_eq!(body_json(h2)["ticks"].as_array().unwrap().len(), 1);

        // Due again.
        run_due_clock_ticks(&state.store, &state.deployments, t0 + Duration::from_secs(6));
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

    // ── CORS ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn cors_allows_localhost_origin_and_omits_the_header_for_others() {
        let state = test_state();

        let allowed =
            Request::fake_http("GET", "/health", vec![("Origin".to_string(), "http://localhost:5173".to_string())], vec![]);
        let r1 = with_cors(&allowed, route(&state, &allowed));
        assert!(r1
            .headers
            .iter()
            .any(|(k, v)| k.as_ref() == "Access-Control-Allow-Origin" && v.as_ref() == "http://localhost:5173"));

        let disallowed =
            Request::fake_http("GET", "/health", vec![("Origin".to_string(), "http://evil.example".to_string())], vec![]);
        let r2 = with_cors(&disallowed, route(&state, &disallowed));
        assert!(!r2.headers.iter().any(|(k, _)| k.as_ref() == "Access-Control-Allow-Origin"));
    }
}