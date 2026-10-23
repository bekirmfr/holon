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
//! lock is only ever held briefly (to clone the `Arc` out). Runs of one deployment no longer
//! hold the inner mutex either: each runs on a copy and commits only if what it read is
//! unchanged, running again otherwise (firing.rs), so they proceed side by side and still lose
//! no update.
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

mod access;
mod activity;
mod oidc;
mod collections;
mod data;
mod dataplane;
mod deploy;
mod durable;
mod firing;
mod fixtures;
mod grants;
mod flows;
mod http;
mod lifecycle;
mod listeners;
mod outbound;
mod permissions;
mod persist;
mod recorder;
mod scheduler;
mod sealing;
mod secret_sources;
mod secrets;
#[cfg(test)]
mod tests;
mod units;
mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use holon_core::{
    capabilities, child_refs, declaration_of, decode_preimage, encode_preimage, hash_unit, hash_value, parse_hash, run_id,
    state_root,
    read_server, Capability, CacheEntry, Server, Edge, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody,
    Value, BOUNDARY,
};
use holon_host::{
    fire, is_storable, write_atomically, CacheIngestError, FileStore, ForkError, Host, MemStore,
    run_rounds, PublishError, Waiting, Registry, Round, SharedCache, State, Store, StoreError,
};
use lifecycle::{Lifecycle, RunFrom, Session, Status};
use recorder::{Log, LogEntry};
use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

// Every module's items, so the handlers keep calling each other by bare name, as when this was
// one file (each module does `use crate::*` for the same reason).
use deploy::*;
use firing::*;
use fixtures::*;
use flows::*;
use http::*;
use listeners::*;
use permissions::{Grant, Manifest, Permit, Policy, Requirements};
use secrets::{Redactor, SecretStore, Vault};
use durable::{ParkedRun, Resumed, RunWaits};
use persist::*;
use scheduler::*;
use units::*;
use wire::*;

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
    fn put_value(&mut self, value: &Value) -> Result<Hash, StoreError> {
        self.0.lock().unwrap().put_value(value)
    }
    fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError> {
        self.0.lock().unwrap().get_value(key)
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
    /// Who may use the control plane: tokens, and hosts and origins beyond loopback (access.rs).
    access: access::Access,
    /// The port this server listens on — the one a request's `Host` must name
    /// (`reject_untrusted`).
    port: u16,
    /// slug → (port, stop signal) for each running data-plane listener, kept in line with the
    /// deployments table by `sync_listeners`. `None` turns listening off (the tests, which call
    /// `serve_data` directly instead of opening sockets).
    listeners: Option<Mutex<Listeners>>,
    /// What the data plane holds every connection and request to, and the address it binds
    /// (dataplane.rs).
    data_limits: dataplane::DataLimits,
    /// The host operator's policy (`--policy`, default `holon.toml`): limits, ceilings, what
    /// is granted without asking. Read at startup, and again on `POST /permissions/reload`.
    policy: std::sync::RwLock<Arc<Policy>>,
    /// Where the policy is read from.
    policy_path: String,
    /// Every deployment's secrets (`--secrets`, secrets.rs): written over the control plane,
    /// never returned.
    secrets: Arc<SecretStore>,
    /// What was done on the control plane, by whom (activity.rs).
    activity: activity::Activity,
}

impl AppState {
    /// The policy in force.
    fn policy(&self) -> Arc<Policy> {
        Arc::clone(&self.policy.read().unwrap())
    }
}

/// slug → (port, stop signal) of each running data-plane listener.
type Listeners = BTreeMap<String, (u16, dataplane::Stop)>;

/// One deployment's keyed state, shared between its fires and persistence (see `Deployment::state`).
type DeploymentState = Arc<Mutex<State>>;

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
    /// The routes and port, when the unit is a server declaration (`holon_core::read_server`):
    /// such a deployment is answered on its own port by the data plane (`data.rs`), not by
    /// `/trigger` or the clock. Derived from the unit at deploy and load time, never persisted.
    server: Option<Server>,
    /// When this deployment was first created. Persisted, so a restart doesn't reset it to
    /// "now" and quietly lie about how long it's actually been running.
    deployed_at: SystemTime,
    /// Who deployed it, when the server knows (access.rs).
    deployed_by: Option<String>,
    /// The saved flows its version places as nodes (wire.rs's `HolonUse`), when the editor said.
    uses: Option<Vec<HolonUse>>,
    /// The web pages it answers from other sites (wire.rs's `browsers`; listeners.rs).
    browsers: Vec<String>,
    /// Running, paused or stopped, and every run session so far with its end-of-session state
    /// snapshot (lifecycle.rs). Persisted.
    life: Lifecycle,
    /// What the host granted it (permissions.rs). Persisted.
    grant: Grant,
    /// The grant applied to the policy in force: what every run is held to. Derived at deploy
    /// and load, never persisted.
    permit: Arc<Permit>,
    /// Why it can't run until it is deployed again, when its grant no longer covers what it
    /// needs (found at load). It is paused meanwhile.
    attention: Option<String>,
    /// Its secrets (the store's vault for its slug): runs read the ones it was granted, and
    /// everything it logs or answers has their values redacted.
    secrets: Vault,
    /// Its runs waiting for an event, by run id (durable.rs). Persisted; kept across redeploys,
    /// each pinned to the unit it started on.
    parked: BTreeMap<String, ParkedRun>,
    /// Every run, with what it read and produced (recorder.rs): the newest in memory, kept beside
    /// the deployments file so a restart reads them back.
    log: Log,
    /// The flow document (canvas) this deployment was deployed from, as the editor sent it, so
    /// view mode shows exactly what runs even after the flow is edited. Persisted.
    document: Option<serde_json::Value>,
    /// The documents of the versions deployed before this one, oldest first, by unit (hex):
    /// a run logged on an earlier version is shown on its own canvas. The last
    /// [`deploy::KEPT_VERSIONS`] are kept. Persisted.
    versions: Vec<wire::KeptDocument>,
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
    let mut policy_arg = "holon.toml".to_string();
    let mut secrets_arg = ".holon-secrets.json".to_string();
    let mut secrets_key_arg: Option<String> = None;
    let mut dotenv_arg: Option<String> = None;
    let mut admin_token_file: Option<String> = None;
    let mut users_file: Option<String> = None;
    let mut oidc_issuer: Option<String> = std::env::var("HOLON_OIDC_ISSUER").ok().filter(|v| !v.trim().is_empty());
    let mut oidc_client_id: Option<String> = std::env::var("HOLON_OIDC_CLIENT_ID").ok().filter(|v| !v.trim().is_empty());
    let mut secrets_token_file: Option<String> = None;
    let mut allow_hosts: Vec<String> = Vec::new();
    let mut allow_origins: Vec<String> = Vec::new();
    let mut host_key_arg = ".holon-host-key".to_string();
    let mut data_limits = dataplane::DataLimits::default();
    while let Some(a) = args.next() {
        if a == "--data-host" {
            // The address data-plane listeners bind (default 127.0.0.1): `0.0.0.0` opens every
            // deployed server to the network.
            match args.next().map(|v| v.parse()) {
                Some(Ok(host)) => data_limits.host = host,
                _ => {
                    eprintln!("--data-host needs an IP address (e.g. 127.0.0.1 or 0.0.0.0)");
                    std::process::exit(2);
                }
            }
        } else if a == "--host-key" {
            if let Some(v) = args.next() {
                host_key_arg = v;
            }
        } else if a == "--secrets" {
            if let Some(v) = args.next() {
                secrets_arg = v;
            }
        } else if a == "--secrets-key" {
            // Where the secrets file's master key is kept: `os` (the OS key store, the default
            // where there is one), `env:VAR` or `file:PATH` (sealing.rs).
            if let Some(v) = args.next() {
                secrets_key_arg = Some(v);
            }
        } else if a == "--hash-token" {
            // A token's hash for a users file (access.rs): the token is read on stdin, so it
            // stays out of the shell's history.
            let mut token = String::new();
            if std::io::stdin().read_line(&mut token).is_err() || token.trim().is_empty() {
                eprintln!("--hash-token reads a token on stdin");
                std::process::exit(2);
            }
            println!("{}", access::hash_token(&token));
            return;
        } else if a == "--admin-token-file" || a == "--secrets-token-file" || a == "--allow-host" || a == "--allow-origin" || a == "--users-file" || a == "--oidc-issuer" || a == "--oidc-client-id" {
            // Who may use the control plane (access.rs). A token's file holds just the token.
            if let Some(v) = args.next() {
                match a.as_str() {
                    "--admin-token-file" => admin_token_file = Some(v),
                    "--secrets-token-file" => secrets_token_file = Some(v),
                    "--users-file" => users_file = Some(v),
                    // Signing in through an identity provider (oidc.rs).
                    "--oidc-issuer" => oidc_issuer = Some(v),
                    "--oidc-client-id" => oidc_client_id = Some(v),
                    "--allow-host" => allow_hosts.push(v),
                    _ => allow_origins.push(v),
                }
            }
        } else if a == "--dotenv" {
            // A dotenv file `env:HOLON_SECRET_…` sources also read (secret_sources.rs): for
            // development; the real environment wins.
            if let Some(v) = args.next() {
                dotenv_arg = Some(v);
            }
        } else if a == "--new-secrets-key" {
            // A fresh master key, for `--secrets-key env:VAR`: keep it safe, and set the variable.
            println!("{}", sealing::to_hex(sealing::new_key().as_slice()));
            return;
        } else if a == "--policy" {
            if let Some(v) = args.next() {
                policy_arg = v;
            }
        } else if a == "--store" {
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
    let policy = Arc::new(Policy::load(&policy_arg).unwrap_or_else(|e| refuse_to_start(e)));
    let secrets_key = sealing::KeySource::parse(secrets_key_arg.as_deref(), &secrets_arg).unwrap_or_else(|e| refuse_to_start(e));
    let sources = secret_sources::Sources::new(dotenv_arg.map(std::path::PathBuf::from)).unwrap_or_else(|e| refuse_to_start(e));
    let secrets = Arc::new(SecretStore::open(&secrets_arg, &secrets_key, sources).unwrap_or_else(|e| refuse_to_start(e)));
    {
        // References are read again every minute: a value changed or revoked where it is kept
        // reaches the runs without a restart.
        let secrets = Arc::clone(&secrets);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(secret_sources::REFRESH);
                secrets.refresh();
            }
        });
    }
    durable::load_host_key(&host_key_arg).unwrap_or_else(|e| refuse_to_start(e));
    let mut initial_deployments = load_deployments(&deployments_arg, &store, &policy).unwrap_or_else(|e| refuse_to_start(e));
    for (slug, dep) in initial_deployments.iter_mut() {
        dep.secrets = secrets.vault(slug);
    }
    let flows = Mutex::new(load_flows(&flows_arg).unwrap_or_else(|e| refuse_to_start(e)));
    let deployments: Arc<Mutex<BTreeMap<String, Deployment>>> = Arc::new(Mutex::new(initial_deployments));
    spawn_clock_scheduler(store.clone(), Arc::clone(&deployments), deployments_arg.clone());
    let bound = addr
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|e| panic!("HOLON_SERVER_ADDR {addr:?} is not host:port: {e}"));
    let port = bound.port();
    let access = (|| {
        let admin = access::token_from("HOLON_ADMIN_TOKEN", admin_token_file.as_deref())?;
        let secrets = access::token_from("HOLON_SECRETS_TOKEN", secrets_token_file.as_deref())?;
        let users = access::users_from(users_file.as_deref())?;
        let access = access::Access::with_users(admin, secrets, users, allow_hosts, allow_origins)?;
        match (oidc_issuer, oidc_client_id) {
            (None, None) => Ok(access),
            (Some(issuer), Some(client_id)) => Ok(access.signing_in_with(oidc::Oidc::discover(&issuer, &client_id)?)),
            _ => Err("signing in needs both --oidc-issuer and --oidc-client-id (HOLON_OIDC_ISSUER, HOLON_OIDC_CLIENT_ID)".to_string()),
        }
    })()
    .unwrap_or_else(|e| {
        eprintln!("holon-server: {e}");
        std::process::exit(1)
    });
    // Reachable beyond this machine only behind a token.
    if !bound.ip().is_loopback() && !access.is_gated() {
        eprintln!("holon-server: {addr} is reachable beyond this machine: set HOLON_ADMIN_TOKEN (or --admin-token-file), or name users (--users-file), first");
        std::process::exit(1);
    }
    let activity = activity::Activity::beside(&deployments_arg);
    let state = AppState {
        store,
        registry,
        cache,
        deployments,
        deployments_path: deployments_arg,
        flows,
        flows_path: flows_arg,
        port,
        listeners: Some(Mutex::new(BTreeMap::new())),
        data_limits,
        policy: std::sync::RwLock::new(policy),
        policy_path: policy_arg,
        secrets,
        activity,
        access,
    };
    sync_listeners(&state);

    let gate = match (state.access.is_gated(), state.access.user_count(), state.access.oidc()) {
        (false, _, _) => "localhost-only, no token".to_string(),
        (true, n, Some(oidc)) => format!("signing in through {}: {n} named user(s)", oidc.issuer),
        (true, 0, None) => "admin token required".to_string(),
        (true, n, None) => format!("a token required: {n} named user(s)"),
    };
    println!("holon-server: listening on http://{addr} ({gate}; see access.rs)");
    rouille::start_server(addr, move |request| with_cors(&state.access, request, route(&state, request)));
}
