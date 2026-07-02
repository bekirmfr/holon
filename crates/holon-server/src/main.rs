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
//! **Inch 6** (this one, `ingress-deploy-L4.md`): the first slice of the standing-deploy
//! pillar — `POST /deploy`, `GET /deployments`, `POST /trigger/<slug>`. Deliberately the small
//! half: nothing here needed a single new mechanism from `holon-core`/`holon-host`
//! (`fire_sourced` and `SourcedResolver` already existed, already tested) — the only genuinely
//! new thing is a `slug → unit hash` table and one new route. A `Clock`-bound deployment is
//! explicitly **not** this inch — it needs a standing background scheduler, the first
//! genuinely new runtime shape anywhere in this project, and stays its own, later, separately
//! grounded pillar.
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

use holon_core::{
    child_refs, decode_preimage, encode_preimage, hash_unit, parse_hash, Capability, CacheEntry,
    Edge, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody, Value, BOUNDARY,
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
    /// slug → unit hash. In-memory only, same as `Registry`'s published set and `SharedCache` —
    /// restarting the server forgets every deployment, consistent with everything else here.
    deployments: Mutex<BTreeMap<String, Hash>>,
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
#[derive(Serialize)]
struct PerformedEntry {
    capability: String,
    payload: SValue,
}

#[derive(Serialize)]
struct FireResponseBody {
    output: Option<SValue>,
    performed: Vec<PerformedEntry>,
}

/// `POST /deploy`'s request body: a chosen `slug` plus the unit's hash. The unit itself is not
/// re-sent — it must already be resolvable in the store (staged via `/unit`, or published; see
/// the L4's Fork 1 — deploy deliberately does not require publication, a separate,
/// marketplace-facing concept).
#[derive(Deserialize)]
struct DeployRequestBody {
    slug: String,
    unit: String,
}

#[derive(Serialize)]
struct DeployResponseBody {
    slug: String,
    unit: String,
}

#[derive(Serialize)]
struct DeploymentEntry {
    slug: String,
    unit: String,
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

/// `POST /deploy` — register `slug → unit`. Confirms the unit actually resolves in the store
/// (reuses `Store::get`, not a new check) before committing the mapping. Overwrites silently on
/// a repeat slug (the L4's Fork 2 — a redeploy, not an error; matches `Store::put`'s and
/// `SharedCache::ingest`'s own "dumb, idempotent" precedent elsewhere in this crate).
fn handle_post_deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Hash>>,
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
    match store.get(&unit_hash) {
        Ok(Some(_)) => {}
        Ok(None) => return err(404, "no unit stored at that hash — stage or publish it first"),
        Err(e) => return err(500, format!("store error: {e:?}")),
    }
    deployments.lock().unwrap().insert(body.slug.clone(), unit_hash);
    Response::json(&DeployResponseBody { slug: body.slug, unit: to_hex(&unit_hash) })
}

/// `GET /deployments` — the current slug → hash table, same shape/convention as `GET /registry`.
fn handle_get_deployments(deployments: &Mutex<BTreeMap<String, Hash>>) -> Response {
    let deployments = deployments.lock().unwrap();
    let list = deployments
        .iter()
        .map(|(slug, hash)| DeploymentEntry { slug: slug.clone(), unit: to_hex(hash) })
        .collect();
    Response::json(&DeploymentsBody { deployments: list })
}

/// `POST /trigger/<slug>` — the actual ingress trigger. Looks up the deployed unit, materializes
/// its closure exactly as `/fire` and the cache endpoints do, and settles it via `fire_sourced`
/// with the raw request body injected as `Capability::NetworkIngress` — wrapped as
/// `Value::Text`, the L4's Fork 3 default (`ingress`'s declared output type is `Any`; nothing
/// forces this choice, but Text matches how every other text-shaped payload already crosses
/// this boundary). A flow wanting structured data parses that Text itself, downstream, using
/// this project's own `Sum`/`select` machinery — not a format this endpoint imposes.
fn handle_post_trigger(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Hash>>,
    slug: &str,
    request: &Request,
) -> Response {
    let unit_hash = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(h) => *h,
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
    let deployments = Mutex::new(BTreeMap::new());
    let state = AppState { store, registry, cache, deployments };

    println!("holon-server: listening on http://{addr} (localhost-only, no auth — see the L4)");
    rouille::start_server(addr, move |request| with_cors(request, route(&state, request)));
}