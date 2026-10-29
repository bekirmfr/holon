//! `holon`: one command for Holon. `holon serve` runs the server (the browser↔host transport,
//! local-first; below), `new`/`check`/`show`/`fmt` handle flow files (flowcmd.rs), and `version`,
//! `update` and `uninstall` look after the installation (selfmanage.rs). `holon-server`, the name
//! releases before 0.1.0-rc.4 installed, is an alias kept for scripts (src/bin/holon-server.rs).
//!
//! The server: the browser↔host transport (P4.5.1 / P4.6.1's blocker), local-first.
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
mod accounts;
mod activity;
mod arrivals;
mod auth;
mod blobs;
mod calls;
mod channels;
mod claims;
mod oidc;
mod collections;
mod data;
mod databases;
mod dataplane;
mod deploy;
mod editor;
mod durable;
mod engines;
mod mongo;
mod firing;
mod fixtures;
mod grants;
mod heads;
mod jsonedit;
mod flowcmd;
mod flows;
mod folders;
mod http;
mod lifecycle;
mod listeners;
mod locks;
mod local;
mod lsp;
mod migrate;
mod mcp;
mod mcphosts;
mod outbound;
mod permissions;
mod persist;
mod pick;
mod ranges;
mod recorder;
mod restore;
mod scheduler;
mod sealing;
mod secret_sources;
mod secrets;
mod selfmanage;
mod taint;
#[cfg(test)]
mod tests;
mod units;
mod verify;
mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
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

/// The server's own files, which no folder may be or hold: those it was given (`files`), what it
/// keeps beside its deployments file (the flow logs, the arrivals it noted, the heads it
/// published), the `.env` it reads secrets from, and the secrets key's file.
fn server_files(files: &[&String], deployments: &str, dotenv: Option<&str>, key: &sealing::KeySource) -> Vec<PathBuf> {
    let mut own: Vec<PathBuf> = files.iter().filter(|p| p.as_str() != ":memory:").map(PathBuf::from).collect();
    if deployments != ":memory:" {
        own.extend(["logs", "arrivals", "heads"].map(|beside| Path::new(deployments).with_extension(beside)));
    }
    own.extend(dotenv.map(PathBuf::from));
    if let sealing::KeySource::File(path) = key {
        own.push(path.clone());
    }
    own
}

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
struct SharedStore(Arc<Mutex<DynStore>>, Arc<calls::Hub>);

impl SharedStore {
    /// A handle onto `backend`, which no run can yet reach the other deployments through: the
    /// server connects that once its table of deployments exists (`calls::Hub::connect`).
    fn new(backend: DynStore) -> SharedStore {
        SharedStore(Arc::new(Mutex::new(backend)), Arc::default())
    }
}

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
    /// The folders an admin bound (`--folders`, folders.rs): flows read and write them by name.
    folders: Arc<folders::Folders>,
    /// What was done on the control plane, by whom (activity.rs).
    activity: activity::Activity,
    /// The editor, served at `/` (editor.rs): built in, or a folder; none without either.
    editor: Option<editor::Source>,
}

impl AppState {
    /// The policy in force.
    fn policy(&self) -> Arc<Policy> {
        Arc::clone(&self.policy.read().unwrap())
    }
}

impl Deployment {
    /// Fire on the scheduler's next pass: its interval, if it has one, and each of its clocks.
    fn arm_clocks(&mut self, now: SystemTime) {
        if self.interval_seconds.is_some() {
            self.next_due = Some(now);
        }
        self.clock_due = self.server.as_ref().map(|s| vec![now; s.clocks.len()]).unwrap_or_default();
    }
}

/// slug → (port, stop signal) of each running data-plane listener.
type Listeners = BTreeMap<String, (u16, dataplane::Stop)>;

/// One deployment's keyed state, shared between its fires and persistence (see `Deployment::state`).
type DeploymentState = Arc<claims::StateCell>;

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
    /// When each of its server's clocks (`server.clocks`, in order) fires next: due at deploy,
    /// at load and when it runs again, then a period after each tick.
    clock_due: Vec<SystemTime>,
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
    /// Why a server deployment isn't answering on its port (it couldn't listen there, say another
    /// program holds it), until a deploy or run listens there again (listeners.rs). Never persisted.
    listen_error: Option<String>,
    /// Its secrets (the store's vault for its slug): runs read the ones it was granted, and
    /// everything it logs or answers has their values redacted.
    secrets: Vault,
    /// The folders bound on this server, as this deployment sees them (folders.rs): runs read and
    /// write the ones it was granted.
    folders: Arc<folders::Scoped>,
    /// The folders whose new files start its runs, and what it has seen of them (arrivals.rs).
    /// Derived from its unit at deploy and load; what it saw is kept beside its log.
    arrivals: Arc<arrivals::Watch>,
    /// Its runs waiting for an event, by run id (durable.rs). Persisted; kept across redeploys,
    /// each pinned to the unit it started on.
    parked: BTreeMap<String, ParkedRun>,
    /// Every run, with what it read and produced (recorder.rs): the newest in memory, kept beside
    /// the deployments file so a restart reads them back.
    log: Log,
    /// Its channels, with their kept packs and subscribers (channels.rs): kept across redeploys,
    /// rebuilt from the log at load.
    channels: Arc<channels::Channels>,
    /// The flow document (canvas) this deployment was deployed from, as the editor sent it, so
    /// view mode shows exactly what runs even after the flow is edited. Persisted.
    document: Option<serde_json::Value>,
    /// Every version deployed, oldest first, the current one last (`wire::Version`): a run
    /// logged on an earlier version is shown on its own canvas, and any version with its request
    /// kept can be deployed again (`holon restore`). Kept for good. Persisted.
    versions: Vec<wire::Version>,
}

const USAGE: &str = "usage: holon <command>

  serve [options]                    run the server, and the editor at its address
  new <file> [--sample <name>]       write a starter flow (or a sample: holon new --help)
  check <file>                       compile a flow: its routes, what it may do, or each mistake
  show <file>                        an outline: routes, nodes and what feeds them
  fmt <file>                         rewrite a flow in the canonical form
  export <slug> [--to DIR] [--as N]  write a deployment's runs to a file, for holon verify
  verify <export> [--head HEX]       check a deployment's exported runs, offline: run each again
                                     (--files DIR: files they read that the log keeps by hash)
  restore <slug> [--to N --code]     a deployment's versions; deploy an earlier one's code again
  folder add|list|remove             bind a name flows use to a folder on this machine (admin)
  database add|list|remove           bind a name flows use to a database on this machine (admin)
  mcp [--server PORT|URL]            tools for an agent (MCP, on stdin and stdout): check, deploy,
                                     call and explain flows; it never approves what they may do
  mcp setup [HOST…|--all]            add holon mcp to an agent host's settings (mcp hosts: which)
  lsp                                a language server for editors (LSP, on stdin and stdout):
                                     mistakes as you type, hover, formatting
  version                            this build's version
  update [--check] [--to VERSION]    install the newest release (or see if there is one)
  migrate [--yes] [--check]          move what an older release left elsewhere into ~/.holon
  uninstall [--purge] [--yes]        remove holon (and with --purge, all of ~/.holon)";

const SERVE_USAGE: &str = "usage: holon serve [options]

  --at [IP:]PORT             where the server and editor listen (default 127.0.0.1:7420;
                             a port alone means 127.0.0.1; or HOLON_SERVER_ADDR)
  --data DIR                 keep everything below in DIR (or HOLON_DATA; default
                             ~/.holon/data, or HOLON_HOME/data)
  --data-host IP             where deployed flows' own servers listen (default 127.0.0.1)
  --allow-host HOST          a host name it answers to besides localhost (repeatable)
  --allow-origin ORIGIN      an editor origin it answers besides localhost's (repeatable)
  --keep-runs N              runs each deployment's log keeps (default 500; or HOLON_KEEP_RUNS)
  --export-heads URL         send each run's head to a witness (or HOLON_EXPORT_HEADS; its
                             token in HOLON_EXPORT_HEADS_TOKEN)
  --secrets PATH             the secrets file
  --folders PATH             the named folders an admin bound (holon folder --help)
  --databases PATH           the databases an admin bound (holon database --help)
  --secrets-key os|env:VAR|file:PATH
                             where the secrets file's key is kept (default: the OS key store)
  --new-secrets-key          print a fresh key for --secrets-key env:VAR, and exit
  --dotenv FILE              a .env file HOLON_SECRET_* values are also read from
  --editor DIR               serve the editor from DIR instead of the built-in one
  --store PATH|:memory:      --deployments PATH|:memory:   --flows PATH
  --accounts PATH            --policy PATH                 --host-key PATH
                             where each of those is kept

A new server prints a one-time setup code for its first admin (or takes HOLON_SETUP_CODE).";

/// Where `--at` (or HOLON_SERVER_ADDR) says to listen: `IP:PORT`, or a port alone on 127.0.0.1.
fn parse_at(v: &str) -> Result<std::net::SocketAddr, String> {
    let v = v.trim();
    if let Ok(port) = v.parse::<u16>() {
        return Ok(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    }
    v.parse().map_err(|_| format!("{v:?} isn't an address to listen on: give IP:PORT (127.0.0.1:8080, 0.0.0.0:8080, [::1]:8080) or a port"))
}

/// What `serve` warns when it listens beyond this machine: Holon speaks plain HTTP, and TLS ends
/// at the operator's ingress in front of it (the manual's guides/public-https.md). None when
/// both the server and deployed flows' ports stay on loopback.
fn plain_http_warning(server: std::net::SocketAddr, data_host: std::net::IpAddr) -> Option<String> {
    let open: Vec<String> = [
        (!server.ip().is_loopback()).then(|| format!("the server and editor ({server})")),
        (!data_host.is_loopback()).then(|| format!("deployed flows' ports ({data_host})")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if open.is_empty() {
        return None;
    }
    Some(format!(
        "holon: warning: {} take plain HTTP from the network. Holon has no TLS of its own: put it behind your ingress or a TLS proxy (the manual: \"A public HTTPS address\"), and keep the hop from there to Holon on this machine or a network you trust.",
        open.join(" and ")
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default().to_vec();
    match args.first().map(String::as_str) {
        Some("serve") => serve(rest),
        Some(cmd @ ("new" | "check" | "show" | "fmt")) => std::process::exit(flowcmd::run(cmd, &rest)),
        Some("verify") => std::process::exit(verify::run(&rest)),
        Some("export") => std::process::exit(verify::export(&rest)),
        Some("restore") => std::process::exit(restore::cli(&rest)),
        Some("folder") => std::process::exit(folders::cli(&rest)),
        Some("database") => std::process::exit(databases::cli(&rest)),
        // Not in the usage: the server runs it for the admin page's "browse…" (pick.rs).
        Some("pick-folder") => std::process::exit(pick::cli(&rest)),
        Some("mcp") => std::process::exit(mcp::run(&rest)),
        Some("lsp") => std::process::exit(lsp::run(&rest)),
        Some("migrate") => std::process::exit(migrate::run(&rest)),
        // `version`, `update`, `uninstall`: about this installation, not the server.
        _ => match selfmanage::dispatch(&args) {
            Some(code) => std::process::exit(code),
            None => {
                let asked = matches!(args.first().map(String::as_str), None | Some("help" | "--help" | "-h"));
                if asked {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                eprintln!("holon: no command {:?}

{USAGE}", args[0]);
                std::process::exit(2);
            }
        },
    }
}

/// `holon serve`: the server, with `args` its options.
fn serve(args: Vec<String>) {
    // Where it listens (`--at [IP:]PORT`, or HOLON_SERVER_ADDR).
    let mut at: Option<String> = std::env::var("HOLON_SERVER_ADDR").ok().filter(|v| !v.trim().is_empty());

    // `--store <dir>` for an on-disk FileStore; `--store :memory:` for an in-process MemStore
    // (nothing persisted across restarts — useful for a quick local session or a test run).
    // Defaults to `store` in the data folder (`~/.holon/data`; see below).
    //
    // `--deployments <path>` for where the deployments table is persisted (write-through, on
    // every deploy/delete); `--deployments :memory:` opts out entirely, same convention as
    // `--store`. Defaults to `deployments.json` in the data folder.
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
    let mut args = args.into_iter();
    let mut store_arg = ".holon-store".to_string();
    let mut deployments_arg = ".holon-deployments.json".to_string();
    let mut flows_arg = ".holon-flows.json".to_string();
    let mut policy_arg = "holon.toml".to_string();
    let mut secrets_arg = ".holon-secrets.json".to_string();
    let mut folders_arg = ".holon-folders.json".to_string();
    let mut databases_arg = ".holon-databases.json".to_string();
    let mut secrets_key_arg: Option<String> = None;
    let mut dotenv_arg: Option<String> = None;
    let mut accounts_arg = ".holon-accounts.json".to_string();
    // One folder for all of it (`--data DIR`, or HOLON_DATA): the defaults above move inside it.
    let mut data_dir: Option<String> = std::env::var("HOLON_DATA").ok().filter(|v| !v.trim().is_empty());
    // The built editor, served at `/` (`--editor DIR`, or HOLON_EDITOR; editor.rs).
    let mut editor_dir: Option<String> = std::env::var("HOLON_EDITOR").ok().filter(|v| !v.trim().is_empty());
    let mut allow_hosts: Vec<String> = Vec::new();
    let mut allow_origins: Vec<String> = Vec::new();
    let mut host_key_arg = ".holon-host-key".to_string();
    let mut data_limits = dataplane::DataLimits::default();
    // How many runs each deployment's log keeps (`--keep-runs N`, or HOLON_KEEP_RUNS; recorder.rs).
    let mut keep_runs: Option<String> = std::env::var("HOLON_KEEP_RUNS").ok().filter(|v| !v.trim().is_empty());
    // A witness every run's head goes to (`--export-heads URL`, or HOLON_EXPORT_HEADS; heads.rs).
    let mut export_heads: Option<String> = std::env::var("HOLON_EXPORT_HEADS").ok().filter(|v| !v.trim().is_empty());
    while let Some(a) = args.next() {
        if a == "--help" || a == "-h" {
            println!("{SERVE_USAGE}");
            return;
        } else if a == "--at" {
            at = args.next();
            if at.is_none() {
                eprintln!("--at needs where to listen: IP:PORT, or a port");
                std::process::exit(2);
            }
        } else if a == "--export-heads" {
            export_heads = args.next();
            if export_heads.is_none() {
                eprintln!("--export-heads needs the witness's URL");
                std::process::exit(2);
            }
        } else if a == "--keep-runs" {
            keep_runs = args.next();
            if keep_runs.is_none() {
                eprintln!("--keep-runs needs a number of runs (at least 1)");
                std::process::exit(2);
            }
        } else if a == "--data-host" {
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
        } else if a == "--folders" {
            if let Some(v) = args.next() {
                folders_arg = v;
            }
        } else if a == "--databases" {
            if let Some(v) = args.next() {
                databases_arg = v;
            }
        } else if a == "--secrets-key" {
            // Where the secrets file's master key is kept: `os` (the OS key store, the default
            // where there is one), `env:VAR` or `file:PATH` (sealing.rs).
            if let Some(v) = args.next() {
                secrets_key_arg = Some(v);
            }
        } else if a == "--data" || a == "--editor" {
            if let Some(v) = args.next() {
                if a == "--data" { data_dir = Some(v) } else { editor_dir = Some(v) }
            }
        } else if a == "--accounts" {
            // Where the accounts are kept (accounts.rs): who may use the control plane.
            if let Some(v) = args.next() {
                accounts_arg = v;
            }
        } else if a == "--allow-host" || a == "--allow-origin" {
            // Hosts and editor origins it answers as besides loopback's (access.rs).
            if let Some(v) = args.next() {
                match a.as_str() {
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
        } else {
            eprintln!("holon serve: no option {a:?}

{SERVE_USAGE}");
            std::process::exit(2);
        }
    }

    let bound = parse_at(at.as_deref().unwrap_or("127.0.0.1:7420")).unwrap_or_else(|e| {
        eprintln!("holon: --at {e}");
        std::process::exit(2);
    });
    let addr = bound.to_string();

    if let Some(v) = keep_runs {
        match v.trim().parse::<usize>() {
            Ok(n) if n >= 1 => recorder::set_keep(n),
            _ => {
                eprintln!("holon: --keep-runs (or HOLON_KEEP_RUNS) needs a number of runs, at least 1, not {v:?}");
                std::process::exit(2);
            }
        }
    }

    // Whatever wasn't named on its own goes in the data folder: `--data`, else HOLON_DATA, else
    // ~/.holon/data (docs/L4s/holon-home-L4.md). The folder it was started from doesn't matter.
    let dir = match data_dir.map(PathBuf::from).or_else(|| local::holon_home().map(|h| h.join("data"))) {
        Some(d) => d,
        None => {
            eprintln!("holon: no home folder (USERPROFILE or HOME) to keep the server's data in: pass --data DIR");
            std::process::exit(1);
        }
    };
    let mut landed = false;
    let mut inside = |arg: &mut String, default: &str, name: &str| {
        if arg == default {
            *arg = dir.join(name).to_string_lossy().into_owned();
            landed = true;
        }
    };
    inside(&mut store_arg, ".holon-store", "store");
    inside(&mut deployments_arg, ".holon-deployments.json", "deployments.json");
    inside(&mut flows_arg, ".holon-flows.json", "flows.json");
    inside(&mut policy_arg, "holon.toml", "holon.toml");
    inside(&mut secrets_arg, ".holon-secrets.json", "secrets.json");
    inside(&mut folders_arg, ".holon-folders.json", "folders.json");
    inside(&mut databases_arg, ".holon-databases.json", "databases.json");
    inside(&mut accounts_arg, ".holon-accounts.json", "accounts.json");
    inside(&mut host_key_arg, ".holon-host-key", "host.key");
    if landed {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("holon: the data folder {}: {e}", dir.display());
            std::process::exit(1);
        }
        println!("holon: data in {}", dir.display());
    }
    if let Some(note) = migrate::left_here() {
        println!("holon: {note}");
    }
    if let Some(url) = &export_heads {
        // The token stays out of the command line: HOLON_EXPORT_HEADS_TOKEN.
        let token = std::env::var("HOLON_EXPORT_HEADS_TOKEN").ok();
        if let Err(e) = heads::start(url, token, heads::dir_for(&deployments_arg), Duration::from_secs(5)) {
            eprintln!("holon: {e}");
            std::process::exit(2);
        }
        println!("holon: each run's head goes to the witness at {url}");
    }
    // The address first: a second server (one still running from before an update, say) stops
    // here, before its clock and listeners start beside the first one's. Requests wait for the
    // state, which is set before the server runs.
    let state_cell: Arc<std::sync::OnceLock<AppState>> = Arc::new(std::sync::OnceLock::new());
    let server = {
        let cell = Arc::clone(&state_cell);
        rouille::Server::new(&addr, move |request| {
            let state = cell.get().expect("the server runs once its state is set");
            with_cors(&state.access, request, route(state, request))
        })
    }
    .unwrap_or_else(|e| {
        eprintln!("holon: can't listen on {addr}: {e}");
        eprintln!("holon: is another holon running there? Stop it, or pick another address with --at");
        std::process::exit(1);
    });
    let backend: DynStore = if store_arg == ":memory:" {
        println!("holon: in-memory store (nothing persists past this run)");
        Box::new(MemStore::new())
    } else {
        let store = FileStore::new(&store_arg)
            .unwrap_or_else(|e| panic!("could not open store at {store_arg:?}: {e}"));
        println!("holon: file store at {store_arg:?}");
        Box::new(store)
    };

    let store = SharedStore::new(backend);
    let registry = Mutex::new(Registry::new(store.clone()));
    let cache = Mutex::new(SharedCache::new());
    let refuse_to_start = |e: String| -> ! {
        eprintln!("holon: {e}");
        eprintln!("holon: refusing to start rather than overwrite it — fix the file or move it aside");
        std::process::exit(1);
    };
    let policy = Arc::new(Policy::load(&policy_arg).unwrap_or_else(|e| refuse_to_start(e)));
    let secrets_key = sealing::KeySource::parse(secrets_key_arg.as_deref(), &secrets_arg).unwrap_or_else(|e| refuse_to_start(e));
    let dotenv_path = dotenv_arg.clone().map(std::path::PathBuf::from);
    let sources = secret_sources::Sources::new(dotenv_path.clone()).unwrap_or_else(|e| refuse_to_start(e));
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
    // The folders an admin bound (folders.rs): never the server's own files, nor what holds them.
    let mut own = server_files(
        &[&store_arg, &deployments_arg, &flows_arg, &policy_arg, &secrets_arg, &folders_arg, &databases_arg, &accounts_arg, &host_key_arg],
        &deployments_arg,
        dotenv_arg.as_deref(),
        &secrets_key,
    );
    own.extend(local::holon_home());
    let folders = folders::Folders::open(&folders_arg, &own).unwrap_or_else(|e| refuse_to_start(e));
    // The databases an admin bound (databases.rs), beside them: a Postgres or MySQL one's
    // connection is fetched from the environment and dotenv file a secret's source is.
    let db_sources = secret_sources::Sources::new(dotenv_path).unwrap_or_else(|e| refuse_to_start(e));
    let folders = folders.with_databases(databases::Databases::open(&databases_arg, &own).unwrap_or_else(|e| refuse_to_start(e)).with_sources(db_sources));
    // The texts of files runs read are kept beside the content store (blobs.rs).
    let folders = Arc::new(if store_arg == ":memory:" { folders } else { folders.with_blobs(blobs::Blobs::at(Path::new(&store_arg).join("blobs"))) });
    durable::load_host_key(&host_key_arg).unwrap_or_else(|e| refuse_to_start(e));
    let mut initial_deployments = load_deployments(&deployments_arg, &store, &policy).unwrap_or_else(|e| refuse_to_start(e));
    for (slug, dep) in initial_deployments.iter_mut() {
        dep.secrets = secrets.vault(slug);
        // Its waiting runs' sealed parts open under the secrets key.
        let sealer = dep.secrets.lock().unwrap().sealer.clone();
        for run in dep.parked.values_mut() {
            if let Err(e) = run.open(slug, sealer.as_deref()) {
                eprintln!("holon: waiting run {} of {slug:?} can't be opened: {e}", run.run_id);
            }
        }
        dep.folders = Arc::new(folders::Scoped::new(Arc::clone(&folders), slug));
    }
    let flows = Mutex::new(load_flows(&flows_arg).unwrap_or_else(|e| refuse_to_start(e)));
    let deployments: Arc<Mutex<BTreeMap<String, Deployment>>> = Arc::new(Mutex::new(initial_deployments));
    spawn_clock_scheduler(store.clone(), Arc::clone(&deployments), deployments_arg.clone());
    arrivals::spawn_watcher(store.clone(), Arc::clone(&deployments), deployments_arg.clone());
    {
        // The texts of files no kept run refers to any more are swept (folders.rs).
        let (folders, deployments) = (Arc::clone(&folders), Arc::clone(&deployments));
        std::thread::spawn(move || {
            loop {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                match folders::sweep(&folders, &deployments, now, folders::SWEEP_SPARES_MS) {
                    Ok(s) if s.texts > 0 || s.chunks > 0 => {
                        println!("holon: swept {} file texts no kept run refers to ({} freed)", s.texts, folders::size_text(s.bytes));
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("holon: sweeping file texts: {e}"),
                }
                std::thread::sleep(folders::SWEEP_EVERY);
            }
        });
    }
    let port = bound.port();
    // Everyone signs in (access.rs); a new server first needs its admin, set up with the code
    // it prints here.
    let accounts = Arc::new(accounts::Accounts::open(&accounts_arg).unwrap_or_else(|e| refuse_to_start(e)));
    if let Some(code) = accounts.setup_code() {
        println!("holon: no accounts yet. Open the editor and set up the first admin with this code: {code}");
    }
    let access = access::Access::new(Arc::clone(&accounts), allow_hosts, allow_origins);
    let activity = activity::Activity::beside(&deployments_arg);
    store.1.connect(Arc::clone(&deployments), deployments_arg.clone());
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
        folders,
        activity,
        access,
        editor: editor::Source::pick(editor_dir.map(std::path::PathBuf::from)),
    };
    sync_listeners(&state);

    let gate = match accounts.users().len() {
        0 => "waiting to be set up".to_string(),
        n => format!("{n} account(s); signing in required"),
    };
    if let Some(source) = &state.editor {
        println!("holon: the editor ({}) at http://{addr}/", source.describe());
    }
    println!("holon: listening on http://{addr} ({gate}; see access.rs)");
    if let Some(warning) = plain_http_warning(bound, state.data_limits.host) {
        eprintln!("{warning}");
    }
    // Commands this user runs on this machine find it, and sign in, from here (local.rs).
    local::publish(bound, &accounts.new_local(&local::user()));
    if state_cell.set(state).is_err() {
        unreachable!("the state is set once");
    }
    server.run();
}
