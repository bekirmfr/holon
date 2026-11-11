//! `holon`: one command for Holon. `holon serve` runs the server (below); `new`/`check`/`show`/
//! `fmt`/`types` handle flow files (flowcmd.rs); `deploy`, `call`, `grant` and the rest talk to a
//! running server (the `*cmd.rs` files, cmdline.rs, local.rs); `editor` is the terminal editor
//! (workbench/), `mcp` the agent's tools (mcp.rs), `lsp` the language server (lsp.rs); and
//! `version`, `update`, `uninstall` and `migrate` look after the installation (selfmanage.rs,
//! migrate.rs). `holon-server`, the name releases before 0.1.0-rc.4 installed, is an alias kept
//! for scripts (src/bin/holon-server.rs).
//!
//! The server has two planes. The control plane is the API the editor and the commands use, on
//! `rouille` at `--at` (routed by http.rs); the data plane is one listener per deployed server
//! flow, on its own port, on `hyper` over `tokio` (dataplane.rs, listeners.rs). Every run goes
//! through the same path (firing.rs) and is recorded (recorder.rs). It adds no trust of its own
//! to what content addressing gives: every unit and value read from the store is checked against
//! its hash (`Store::get`, holon-host; docs/L4s/P4.5-P4.6-transport-scoping-L4.md). What it adds
//! is who may ask (access.rs, can.rs) and what a run may do (permissions.rs).
//! docs/engineering/components/holon.md maps the modules, what `serve` builds and the locks; the
//! steps that built it are in docs/JOURNAL.md.
//!
//! Binds `127.0.0.1` unless told another address (`--at`), and answers only a `Host` and an
//! `Origin` it knows (localhost, or a name given with `--allow-host` / `--allow-origin`): CORS
//! never `*`. CORS alone never stopped a request from *running*, so every request first passes
//! `reject_untrusted` (Host, Origin, Content-Type), bodies are capped at `MAX_BODY_BYTES`, and
//! no more than `MAX_IN_FLIGHT` are answered at once. Everyone signs in (access.rs): accounts
//! with roles, API tokens, an identity provider, or this machine's own user through the run file
//! (local.rs); docs/engineering/themes/security.md says what each guards and what it doesn't.

mod access;
mod accesscmd;
mod accounts;
mod accountscmd;
mod decidecmd;
mod waitcmd;
mod deploycmd;
mod replaycmd;
mod activity;
mod arrivals;
mod auth;
mod bindings;
mod blobs;
mod calls;
mod can;
mod changes;
mod channels;
mod claims;
mod cmdline;
mod oidc;
mod opscmd;
mod collections;
mod data;
mod databases;
mod dataplane;
mod mails;
mod smtp;
mod imap;
mod oauth;
mod inbox;
mod sse;
mod ws;
mod feeds;
mod listen;
mod subscriptions;
mod sigv4;
mod sqs;
mod queues;
mod dequeue;
mod deploy;
mod editor;
mod durable;
mod engines;
mod events;
mod mongo;
mod firing;
mod fixtures;
mod grantcmd;
mod grants;
mod heads;
mod history;
mod jsonedit;
mod flowcmd;
mod flows;
mod flowscmd;
mod folders;
mod http;
mod lifecycle;
mod listeners;
mod logs;
mod logscmd;
mod load;
mod locks;
mod local;
mod lsp;
mod migrate;
mod mcp;
mod mcphosts;
mod outbound;
mod paths;
mod permissions;
mod persist;
mod proposals;
mod pick;
mod ranges;
mod recorder;
mod restore;
mod scheduler;
mod storage;
mod summary;
mod sweep;
mod sealing;
mod stopping;
mod sync;
mod secret_sources;
mod secretcmd;
mod secrets;
mod selfmanage;
mod taint;
mod touches;
#[cfg(test)]
mod tests;
mod units;
mod verify;
mod websockets;
mod wire;
mod workbench;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use holon_core::{
    capabilities, child_refs, decode_preimage, encode_preimage, hash_unit, hash_value, parse_hash, run_id,
    state_root,
    read_server, Capability, Server, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody,
    Value,
};
use holon_host::{
    is_storable, write_atomically, FileStore, Host, InsertError, MemStore,
    run_rounds, Waiting, Round, State, Store, StoreError,
};
use lifecycle::{Lifecycle, RunFrom, Session, Status};
use recorder::{Log, LogCell, LogEntry};
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
use paths::{HostPath, Place};
use permissions::{Grant, Manifest, Permit, Policy, Requirements};
use secrets::{Redactor, SecretStore, Vault};
use durable::{ParkedRun, Resumed, RunWaits};
use persist::*;
use scheduler::*;
use units::*;
use wire::*;

/// What the server keeps beside its deployments file: the flow logs (and their sealed segments
/// inside), the state heads, the arrivals it noted, the heads it published, its activity log.
const BESIDE_DEPLOYMENTS: [&str; 6] = ["logs", "state", "arrivals", "mail-seen", "heads", "activity.jsonl"];

/// The server's own files, which no folder may be or hold: those it was given (`files`), what it
/// keeps beside its deployments file (`BESIDE_DEPLOYMENTS`) and beside its secrets file (their
/// audit), the `.env` it reads secrets from, and the secrets key's file.
fn server_files(files: &[&String], deployments: &str, secrets: &str, dotenv: Option<&str>, key: &sealing::KeySource) -> Vec<PathBuf> {
    let mut own: Vec<PathBuf> = files.iter().filter(|p| p.as_str() != ":memory:").map(PathBuf::from).collect();
    if deployments != ":memory:" {
        own.extend(BESIDE_DEPLOYMENTS.map(|beside| Path::new(deployments).with_extension(beside)));
    }
    if secrets != ":memory:" {
        own.push(Path::new(secrets).with_extension("audit.jsonl"));
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
/// delegating, so each clone (a run's, the server's for `/unit`) locks the same underlying
/// `Mutex` and sees the same data.
/// The second part is the hub through which flows call each other (calls.rs); the third notes
/// the values put lately, for a sweep to spare (sweep.rs); the fourth keeps
/// the closures runs have loaded (units.rs's `FlowCache`); the fifth the values runs have read
/// from disk (units.rs's `ValueCache`).
#[derive(Clone)]
struct SharedStore(Arc<Mutex<DynStore>>, Arc<calls::Hub>, Arc<sweep::Puts>, Arc<units::FlowCache>, Arc<units::ValueCache>);

impl SharedStore {
    /// A handle onto `backend`, which no run can yet reach the other deployments through: the
    /// server connects that once its table of deployments exists (`calls::Hub::connect`).
    fn new(backend: DynStore) -> SharedStore {
        // In memory already: nothing to keep beside it.
        SharedStore(Arc::new(Mutex::new(backend)), Arc::default(), Arc::default(), Arc::default(), Arc::new(units::ValueCache::with_budget(0)))
    }

    /// The same onto a `FileStore` keeping its values in `values`, which a sweep may clear.
    fn on_disk(backend: DynStore, values: PathBuf) -> SharedStore {
        SharedStore(Arc::new(Mutex::new(backend)), Arc::default(), Arc::new(sweep::Puts::at(values)), Arc::default(), Arc::default())
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
        let mut store = self.0.lock().unwrap();
        let hash = store.put_value(value)?;
        self.2.put(hash);
        Ok(hash)
    }
    fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError> {
        // A value is its hash's forever, so one read from disk once is the same one kept.
        if let Some(value) = self.4.get(key) {
            return Ok(Some(value));
        }
        let value = self.0.lock().unwrap().get_value(key)?;
        if let Some(v) = &value {
            self.4.keep(*key, v);
        }
        Ok(value)
    }
}

/// Everything a request handler needs. `deployments` is its own `Mutex` (not folded into
/// `SharedStore`'s locking) since it needs its own lock scope distinct from a single store
/// put/get.
struct AppState {
    store: SharedStore,
    /// slug → deployment. `Arc` (not just `Mutex`) because the clock scheduler thread
    /// (`spawn_clock_scheduler`) needs its own independent handle onto the same table, cloned
    /// before `AppState` is built — the same `Send`-shared-handle shape `SharedStore` already
    /// uses for the store itself. It is persisted write-through to `deployments_path` (see
    /// `persist_deployments`/`load_deployments`): a deployment definition surviving a restart
    /// is the whole point of "standing."
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    /// Where `deployments` is persisted, or nowhere (`--deployments :memory:`, matching
    /// `--store`'s own convention) to opt out entirely.
    deployments_path: Place,
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
    /// nowhere to opt out.
    flows_path: Place,
    /// The versions proposed for a person to deploy, and what became of each (proposals.rs): kept
    /// beside `flows`.
    proposals: proposals::Proposals,
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
    policy: crate::sync::RwLock<Arc<Policy>>,
    /// Where the policy is read from: nowhere, the defaults (the tests).
    policy_path: Place,
    /// Every deployment's secrets (`--secrets`, secrets.rs): written over the control plane,
    /// never returned.
    secrets: Arc<SecretStore>,
    /// The folders an admin bound (`--folders`, folders.rs): flows read and write them by name.
    folders: Arc<folders::Folders>,
    /// What was done on the control plane, by whom (activity.rs).
    activity: activity::Activity,
    /// The editor, served at `/` (editor.rs): built in, or a folder; none without either.
    editor: Option<editor::Source>,
    /// The server's own sweeps, on `[storage] sweep_every`'s schedule (storage.rs).
    sweeps: storage::Sweeps,
    /// What the last check of every run log found, since the server started (summary.rs).
    checked: Mutex<Option<summary::Check>>,
}

impl AppState {
    /// The policy in force.
    fn policy(&self) -> Arc<Policy> {
        Arc::clone(&self.policy.read().unwrap())
    }
}

impl Deployment {
    /// Fire each of its clocks on the scheduler's next pass.
    fn arm_clocks(&mut self, now: SystemTime) {
        self.clock_due = self.server.as_ref().map(|s| vec![now; s.clocks.len()]).unwrap_or_default();
    }

    /// Whether a trigger may start a run of it now: it runs, and its server (`store`'s) isn't
    /// stopping (stopping.rs). Every trigger asks before it starts one.
    fn takes_runs(&self, store: &SharedStore) -> bool {
        self.life.status == Status::Running && !store.1.stopping()
    }
}

/// slug → (port, stop signal) of each running data-plane listener.
type Listeners = BTreeMap<String, (u16, dataplane::Stop)>;

/// One deployment's keyed state, shared between its fires and persistence (see `Deployment::state`).
type DeploymentState = Arc<claims::StateCell>;

/// A registered deployment. `triggers` is derived **once**, at deploy time, from the unit's
/// own capability closure (`capabilities()`) — safe to cache rather than re-derive on every
/// `GET /deployments`, since a unit's hash is its whole identity: the same hash can never later
/// have different capabilities. `clock_due`/`history` are its clocks'; empty for a deployment
/// with none.
struct Deployment {
    unit: Hash,
    triggers: Vec<String>,
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
    /// The runs of its unrecorded handlers, counted, by label: what the log doesn't hold of them.
    /// Persisted with the deployments file, as the counts above are.
    tallies: BTreeMap<String, wire::Tally>,
    /// Its tallies changed since the deployments file was last written: the logs' periodic sync
    /// writes it, so a restart keeps them, the last second's at most lost (an unrecorded handler's
    /// failures are in its tally alone, resilience-audit-L4.md §2.8). Never persisted.
    tallies_changed: bool,
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
    /// the deployments file so a restart reads them back. Behind a lock of its own, so a run's
    /// entry is written without holding the table (docs/L4s/request-path-L4.md, C2).
    log: Arc<LogCell>,
    /// Its channels, with their kept packs and subscribers (channels.rs): kept across redeploys,
    /// rebuilt from the log at load.
    channels: Arc<channels::Channels>,
    /// Its open WebSocket connections (websockets.rs): kept across redeploys, closed when it is
    /// paused, stopped or removed. Never persisted: a restart ends them.
    sockets: Arc<websockets::Sockets>,
    /// The flow document (canvas) this deployment was deployed from, as the editor sent it, so
    /// view mode shows exactly what runs even after the flow is edited. Persisted.
    document: Option<serde_json::Value>,
    /// A person turned off agents proposing its versions (proposals.rs). Persisted.
    agents_propose_off: bool,
    /// Every version deployed, oldest first, the current one last (`wire::Version`): a run
    /// logged on an earlier version is shown on its own canvas, and any version with its request
    /// kept can be deployed again (`holon restore`). Kept for good. Persisted.
    versions: Vec<wire::Version>,
    /// What was done to it, by whom (events.rs): deploys, restores, grant changes, secrets,
    /// bindings, pausing and running. Kept for good beside the log.
    events: events::Events,
}

const USAGE: &str = "usage: holon <command>

  serve [options]                    run the server, and the editor at its address
  new <file> [--sample <name>]       write a starter flow (or a sample: holon new --help)
  check <file>                       compile a flow: its routes, what it may do, or each mistake
  show <file>                        an outline: routes, nodes and what feeds them
  fmt <file>                         rewrite a flow in the canonical form
  types [dir]                        write holon.d.ts, for an editor's TypeScript
  deploy <file> [--server PORT|URL]  compile and deploy a flow to a running server (--approve)
  replay <file> [--on SLUG]          a new version run offline on a deployment's recent runs:
                                     which it answers the same, and how the others differ
  editor [file|slug] [--server …]    the editor in this terminal: flows, runs, approvals, and
                                     flows' text in tabs, checked as you type
  deployments [<slug>]               what is deployed; one's details and its waiting runs
  run|pause|stop|remove <slug>       start (--fresh, --session N), hold, end or delete one
  call <slug> METHOD PATH            send a deployed flow a request (-H, --body)
  load <slug> [--for SECONDS]        its routes under load, one after another: requests a second,
                                     latency, and the runs the server recorded, synced, counted
  action <slug> <name>               run one of a deployment's actions (--input JSON)
  tick <slug> CLOCK                  fire one of a flow's clocks now
  state <slug>                       its live state (--collection NAME, --sessions)
  decide <slug> [<run> <name>]       approve, or decline with --reason, an approval a run waits
                                     at (--approve|--decline); the slug alone lists what waits
  logs <slug>                        what its flow wrote with console.log, apart from its runs
                                     (--level, --run N, --until N, --limit N, --follow)
  wait <slug> [--timeout SECONDS]    hold until a person decides the flow's proposal: how, and why
                                     (for an agent to run in the background after it proposes)
  grant <slug>                       what it is granted; --revoke, --grant, --lower LIMIT=N
  grants | policy [--reload]         who holds what (admin); the host's policy
  secret list|history|set|rollback|remove|env
                                     a deployment's secrets (a value is piped in, never shown)
  flows [list|show|save|remove]      the flows saved on the server
  activity [--last N]                what was done on the server, by whom
  people | oidc                      people, agents and services, and their tokens; signing in
                                     through a provider (admin)
  roles | groups                     the roles principals are bound, and groups of them (admin)
  bind | unbind                      a role to a principal or a group, on the host, a flow, a tag
                                     or an approval point, until a time if you say
  access [versions|show]             who may do what, every version of it (for an audit)
  export <slug> [--to DIR] [--as N]  write a deployment's runs to a file, for holon verify
  verify <export> [--head HEX]       check a deployment's exported runs, offline: run each again
                                     (--files DIR: files they read that the log keeps by hash)
  history <slug> [--last N]          a deployment's timeline: its runs, and what was done to it
                                     (--cell NAME: the runs that changed one value)
  restore <slug> [--to N --code]     a deployment's versions; an earlier one's code or state back
  folder add|list|remove             bind a name flows use to a folder on this machine (admin)
  database add|list|remove|tables    bind a name flows use to a database on this machine (admin)
  mail add|list|remove               bind a name flows send mail through to an SMTP server (admin)
  feed add|list|remove               bind a name flows listen to to a feed of server-sent events (admin)
  queue add|list|remove              bind a name flows send to and listen to to an SQS queue (admin)
  store sweep [--dry-run]            clear the stored states no kept run, version or state names
  host                               what the host holds: memory, its disks, what it keeps (admin)
  mcp [--server PORT|URL]            tools for an agent (MCP, on stdin and stdout): check, deploy,
                                     call and explain flows; it never approves what they may do
  mcp setup [HOST…|--all]            add holon mcp to an agent host's settings (hosts: which)
  lsp                                a language server for editors (LSP, on stdin and stdout):
                                     mistakes as you type, hover, formatting
  version                            this build's version
  update [--check] [--to VERSION]    install the newest release (or see if there is one)
  migrate [--yes] [--check]          move what an older release left elsewhere into ~/.holon
  uninstall [--purge] [--yes]        remove holon (and with --purge, all of ~/.holon)

  Each command says more, with examples: holon <command> --help.";

const SERVE_USAGE: &str = "usage: holon serve [options]

  Runs the server: the editor at its address, the API the other commands use, and each deployed
  flow's own port. A new server prints a one-time setup code for its first admin (or takes
  HOLON_SETUP_CODE). An option's environment variable, where it has one, is in brackets.

  --at [IP:]PORT           where the server and editor listen (default 127.0.0.1:7420; a port
                           alone means 127.0.0.1) [HOLON_SERVER_ADDR]
  --data DIR               keep everything below in DIR (default ~/.holon/data, or
                           HOLON_HOME/data) [HOLON_DATA]
  --data-host IP           where deployed flows' own ports listen (default 127.0.0.1)
  --allow-host HOST        a host name it answers to besides localhost, on its own port and each
                           flow's (repeatable): one a proxy in front forwards
  --allow-origin ORIGIN    an editor origin it answers besides localhost's (repeatable)
  --keep-runs N            runs each deployment's log keeps (default 500) [HOLON_KEEP_RUNS]
  --log-stdout             also print each line a flow writes with console.log, as it is kept
                           [HOLON_LOG_STDOUT]
  --log-size SIZE          each file of a deployment's log lines: 64K, 2M, 1G (default 2M)
                           [HOLON_LOG_SIZE]
  --log-files N            files kept of them, 1 to 10 (default 3) [HOLON_LOG_FILES]
  --log-rate N             lines a second a deployment writes, the rest dropped and counted
                           (default 1000) [HOLON_LOG_RATE]
  --flow-cache SIZE        memory kept for loaded flows, such as 64M or 1G; 0 keeps none
                           (default 256M) [HOLON_FLOW_CACHE]
  --value-cache SIZE       memory kept for values runs read from disk, such as 64M; 0 keeps
                           none (default 64M) [HOLON_VALUE_CACHE]
  --export-heads URL       send each run's head to a witness [HOLON_EXPORT_HEADS; its token in
                           HOLON_EXPORT_HEADS_TOKEN]
  --secrets-key SOURCE     where the secrets file's key is kept: os, the OS key store (the
                           default); env:VAR; or file:PATH
  --new-secrets-key        print a fresh key for --secrets-key env:VAR, and exit
  --dotenv FILE            a .env file HOLON_SECRET_* values are also read from
  --editor DIR             serve the editor from DIR instead of the built-in one

  Where each part is kept, if not in the data folder:
  --store PATH|:memory:    the content store: units, values, packs (:memory: keeps it only
                           while the server runs)
  --deployments PATH|:memory:
                           the deployments file (:memory: too)
  --flows PATH             the saved flows
  --secrets PATH           the secrets file
  --folders PATH           the folders an admin bound (holon folder --help)
  --databases PATH         the databases an admin bound (holon database --help)
  --mails PATH             the mail resources an admin bound (holon mail --help); the mail on
                           its way waits in mail-spool beside it
  --feeds PATH             the feeds an admin bound (holon feed --help)
  --queues PATH            the queues an admin bound (holon queue --help); the messages on
                           their way wait in queue-spool beside it
  --accounts PATH          the accounts: who may sign in, and as what
  --policy PATH            the host's policy file
  --host-key PATH          the key the tokens it hands out are signed with

examples:
  holon serve
  holon serve --at 0.0.0.0:7420 --allow-host holon.example.com
  holon serve --data /srv/holon --keep-runs 5000 --flow-cache 1G
  holon serve --secrets-key env:HOLON_SECRETS_KEY --dotenv .env";

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
        Some(cmd @ ("new" | "check" | "show" | "fmt" | "types")) => std::process::exit(flowcmd::run(cmd, &rest)),
        Some("deploy") => std::process::exit(deploycmd::run(&rest)),
        Some("replay") => std::process::exit(replaycmd::run(&rest)),
        Some("editor") => std::process::exit(workbench::run(&rest)),
        Some("verify") => std::process::exit(verify::run(&rest)),
        Some("export") => std::process::exit(verify::export(&rest)),
        Some("restore") => std::process::exit(restore::cli(&rest)),
        Some("history") => std::process::exit(history::cli(&rest)),
        Some("deployments") => std::process::exit(opscmd::deployments(&rest)),
        Some("runs") => std::process::exit(opscmd::runs(&rest)),
        Some(cmd @ ("run" | "pause" | "stop" | "remove")) => std::process::exit(opscmd::lifecycle(cmd, &rest)),
        Some("call") => std::process::exit(opscmd::call(&rest)),
        Some("load") => std::process::exit(load::cli(&rest)),
        Some("action") => std::process::exit(opscmd::action(&rest)),
        Some("tick") => std::process::exit(opscmd::tick(&rest)),
        Some("state") => std::process::exit(opscmd::state(&rest)),
        Some("decide") => std::process::exit(decidecmd::run(&rest)),
        Some("logs") => std::process::exit(logscmd::run(&rest)),
        Some("wait") => std::process::exit(waitcmd::run(&rest)),
        Some("grant") => std::process::exit(grantcmd::grant(&rest)),
        Some("grants") => std::process::exit(grantcmd::grants(&rest)),
        Some("policy") => std::process::exit(grantcmd::policy(&rest)),
        Some("secret") => std::process::exit(secretcmd::cli(&rest)),
        Some("flows") => std::process::exit(flowscmd::flows(&rest)),
        Some("activity") => std::process::exit(flowscmd::activity(&rest)),
        Some("people") => std::process::exit(accountscmd::people(&rest)),
        Some("oidc") => std::process::exit(accountscmd::oidc(&rest)),
        Some("roles") => std::process::exit(accesscmd::roles(&rest)),
        Some("groups") => std::process::exit(accesscmd::groups(&rest)),
        Some("bind") => std::process::exit(accesscmd::bind(&rest)),
        Some("unbind") => std::process::exit(accesscmd::unbind(&rest)),
        Some("access") => std::process::exit(accesscmd::access(&rest)),
        Some("folder") => std::process::exit(folders::cli(&rest)),
        Some("store") => std::process::exit(sweep::cli(&rest)),
        Some("host") => std::process::exit(storage::cli(&rest)),
        Some("database") => std::process::exit(databases::cli(&rest)),
        Some("mail") => std::process::exit(mails::cli(&rest)),
        Some("feed") => std::process::exit(feeds::cli(&rest)),
        Some("queue") => std::process::exit(queues::cli(&rest)),
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
    // server, so the `/unit` round trip can be checked with plain `curl` + a file diff — no
    // HTTP client dependency needed just to generate a valid test payload.
    let mut args = args.into_iter();
    let mut store_arg = ".holon-store".to_string();
    let mut deployments_arg = ".holon-deployments.json".to_string();
    let mut flows_arg = ".holon-flows.json".to_string();
    let mut policy_arg = "holon.toml".to_string();
    let mut secrets_arg = ".holon-secrets.json".to_string();
    let mut folders_arg = ".holon-folders.json".to_string();
    let mut databases_arg = ".holon-databases.json".to_string();
    let mut mails_arg = ".holon-mails.json".to_string();
    let mut feeds_arg = ".holon-feeds.json".to_string();
    let mut queues_arg = ".holon-queues.json".to_string();
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
    // What the flows' log lines are kept and said as (`--log-stdout`, `--log-size`, `--log-files`,
    // `--log-rate`, or HOLON_LOG_*; logs.rs).
    let mut log_stdout = std::env::var("HOLON_LOG_STDOUT").is_ok_and(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"));
    let mut log_size: Option<String> = std::env::var("HOLON_LOG_SIZE").ok().filter(|v| !v.trim().is_empty());
    let mut log_files: Option<String> = std::env::var("HOLON_LOG_FILES").ok().filter(|v| !v.trim().is_empty());
    let mut log_rate: Option<String> = std::env::var("HOLON_LOG_RATE").ok().filter(|v| !v.trim().is_empty());
    // How much memory loaded flows may keep (`--flow-cache SIZE`, or HOLON_FLOW_CACHE; units.rs).
    let mut flow_cache: Option<String> = std::env::var("HOLON_FLOW_CACHE").ok().filter(|v| !v.trim().is_empty());
    // How much memory values read from disk may keep (`--value-cache SIZE`, or HOLON_VALUE_CACHE;
    // units.rs's `ValueCache`).
    let mut value_cache: Option<String> = std::env::var("HOLON_VALUE_CACHE").ok().filter(|v| !v.trim().is_empty());
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
        } else if a == "--log-stdout" {
            log_stdout = true;
        } else if a == "--log-size" || a == "--log-files" || a == "--log-rate" {
            let Some(v) = args.next() else {
                eprintln!("{a} needs a value: a size (64K, 2M, 1G), a number of files (1 to 10), or lines a second");
                std::process::exit(2);
            };
            match a.as_str() {
                "--log-size" => log_size = Some(v),
                "--log-files" => log_files = Some(v),
                _ => log_rate = Some(v),
            }
        } else if a == "--flow-cache" {
            flow_cache = args.next();
            if flow_cache.is_none() {
                eprintln!("--flow-cache needs a size: 64M, 1G, or 0 to keep none");
                std::process::exit(2);
            }
        } else if a == "--value-cache" {
            value_cache = args.next();
            if value_cache.is_none() {
                eprintln!("--value-cache needs a size: 64M, 1G, or 0 to keep none");
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
        } else if a == "--mails" {
            if let Some(v) = args.next() {
                mails_arg = v;
            }
        } else if a == "--feeds" {
            if let Some(v) = args.next() {
                feeds_arg = v;
            }
        } else if a == "--queues" {
            if let Some(v) = args.next() {
                queues_arg = v;
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

    let mut log_caps = logs::Caps { stdout: log_stdout, ..logs::Caps::default() };
    if let Some(v) = log_size {
        match verify::budget_of(&v) {
            Some(bytes) if bytes >= logs::LEAST_FILE_BYTES => log_caps.file_bytes = bytes,
            _ => {
                eprintln!("holon: --log-size (or HOLON_LOG_SIZE) needs a size of at least 64K: 65536, 64K, 2M or 1G, not {v:?}");
                std::process::exit(2);
            }
        }
    }
    if let Some(v) = log_files {
        match v.trim().parse::<usize>() {
            Ok(n) if (1..=logs::MOST_FILES).contains(&n) => log_caps.files = n,
            _ => {
                eprintln!("holon: --log-files (or HOLON_LOG_FILES) needs a number of files, 1 to {}, not {v:?}", logs::MOST_FILES);
                std::process::exit(2);
            }
        }
    }
    if let Some(v) = log_rate {
        match v.trim().parse::<u64>() {
            Ok(n) if n >= 1 => log_caps.per_second = n,
            _ => {
                eprintln!("holon: --log-rate (or HOLON_LOG_RATE) needs a number of lines a second, at least 1, not {v:?}");
                std::process::exit(2);
            }
        }
    }

    // Whatever wasn't named on its own goes in the data folder: `--data`, else HOLON_DATA, else
    // ~/.holon/data (docs/L4s/holon-home-L4.md). The folder it was started from doesn't matter.
    let dir = match data_dir.map(PathBuf::from).or_else(|| local::holon_home().map(|h| h.join("data"))).map(paths::normalize) {
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
    inside(&mut mails_arg, ".holon-mails.json", "mails.json");
    inside(&mut feeds_arg, ".holon-feeds.json", "feeds.json");
    inside(&mut queues_arg, ".holon-queues.json", "queues.json");
    inside(&mut accounts_arg, ".holon-accounts.json", "accounts.json");
    inside(&mut host_key_arg, ".holon-host-key", "host.key");
    // Each in its platform's form, whole (paths.rs): as it is kept, joined, shown and compared.
    for arg in [&mut store_arg, &mut deployments_arg, &mut flows_arg, &mut policy_arg, &mut secrets_arg, &mut folders_arg, &mut databases_arg, &mut mails_arg, &mut feeds_arg, &mut queues_arg, &mut accounts_arg, &mut host_key_arg] {
        if arg.as_str() != ":memory:" {
            *arg = paths::normalize(&*arg).to_string_lossy().into_owned();
        }
    }
    let (deployments_at, flows_at, policy_at) = (Place::parse(&deployments_arg), Place::parse(&flows_arg), Place::parse(&policy_arg));
    if landed {
        // Its owner's only when it is made here: it holds every flow's state, logs and secrets.
        if let Err(e) = holon_host::create_private_dir(&dir) {
            eprintln!("holon: the data folder {}: {e}", dir.display());
            std::process::exit(1);
        }
        println!("holon: data in {}", dir.display());
        if let Some(warning) = open_to_others(&dir) {
            eprintln!("holon: warning: {warning}");
        }
    }
    if let Some(note) = migrate::left_here() {
        println!("holon: {note}");
    }
    if let Some(url) = &export_heads {
        // The token stays out of the command line: HOLON_EXPORT_HEADS_TOKEN.
        let token = std::env::var("HOLON_EXPORT_HEADS_TOKEN").ok();
        if let Err(e) = heads::start(url, token, heads::dir_for(&deployments_at), Duration::from_secs(5)) {
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
            admitted(cell.get().expect("the server runs once its state is set"), request, &IN_FLIGHT, MAX_IN_FLIGHT)
        })
    }
    .unwrap_or_else(|e| {
        eprintln!("holon: can't listen on {addr}: {e}");
        eprintln!("holon: is another holon running there? Stop it, or pick another address with --at");
        std::process::exit(1);
    });
    let store = if store_arg == ":memory:" {
        println!("holon: in-memory store (nothing persists past this run)");
        SharedStore::new(Box::new(MemStore::new()))
    } else {
        let store = FileStore::new(&store_arg)
            .unwrap_or_else(|e| panic!("could not open store at {store_arg:?}: {e}"));
        println!("holon: file store at {store_arg:?}");
        let values = store.values_dir();
        SharedStore::on_disk(Box::new(store), values)
    };
    if let Some(v) = flow_cache {
        match verify::budget_of(&v) {
            Some(bytes) => store.3.set_budget(bytes),
            None => {
                eprintln!("holon: --flow-cache (or HOLON_FLOW_CACHE) needs a size: 1048576, 64K, 64M or 1G, or 0 to keep none, not {v:?}");
                std::process::exit(2);
            }
        }
    }
    if let Some(v) = value_cache {
        match verify::budget_of(&v) {
            Some(bytes) => store.4.set_budget(bytes),
            None => {
                eprintln!("holon: --value-cache (or HOLON_VALUE_CACHE) needs a size: 1048576, 64K, 64M or 1G, or 0 to keep none, not {v:?}");
                std::process::exit(2);
            }
        }
    }
    let refuse_to_start = |e: String| -> ! {
        eprintln!("holon: {e}");
        eprintln!("holon: refusing to start rather than overwrite it — fix the file or move it aside");
        std::process::exit(1);
    };
    let policy = Arc::new(Policy::load(&policy_at).unwrap_or_else(|e| refuse_to_start(e)));
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
        &[&store_arg, &deployments_arg, &flows_arg, &policy_arg, &secrets_arg, &folders_arg, &databases_arg, &mails_arg, &feeds_arg, &queues_arg, &accounts_arg, &host_key_arg],
        &deployments_arg,
        &secrets_arg,
        dotenv_arg.as_deref(),
        &secrets_key,
    );
    own.extend(local::holon_home());
    // The access document beside the accounts (bindings.rs), and its versions.
    if accounts_arg != ":memory:" {
        let access = bindings::beside(Path::new(&accounts_arg));
        own.extend([bindings::history(&access), access]);
    }
    let folders =folders::Folders::open(&folders_arg, &own).unwrap_or_else(|e| refuse_to_start(e));
    // The databases an admin bound (databases.rs), beside them: a Postgres or MySQL one's
    // connection is fetched from the environment and dotenv file a secret's source is.
    let db_sources = secret_sources::Sources::new(dotenv_path.clone()).unwrap_or_else(|e| refuse_to_start(e));
    let folders = folders.with_databases(databases::Databases::open(&databases_arg, &own).unwrap_or_else(|e| refuse_to_start(e)).with_sources(db_sources));
    // And the mail resources (mails.rs): a server's URL with its login is read as a database's is.
    let mail_sources = secret_sources::Sources::new(dotenv_path.clone()).unwrap_or_else(|e| refuse_to_start(e));
    let folders = folders.with_mails(mails::Mails::open(&mails_arg).unwrap_or_else(|e| refuse_to_start(e)).with_sources(mail_sources));
    // And the feeds (feeds.rs): a feed's URL and headers are read as a mail server's are.
    let feed_sources = secret_sources::Sources::new(dotenv_path.clone()).unwrap_or_else(|e| refuse_to_start(e));
    let folders = folders.with_feeds(feeds::Feeds::open(&feeds_arg).unwrap_or_else(|e| refuse_to_start(e)).with_sources(feed_sources));
    // And the queues (queues.rs): a queue's URL with its key is read as a feed's is.
    let queue_sources = secret_sources::Sources::new(dotenv_path).unwrap_or_else(|e| refuse_to_start(e));
    let folders = folders.with_queues(queues::Queues::open(&queues_arg).unwrap_or_else(|e| refuse_to_start(e)).with_sources(queue_sources));
    // The lines flows write to their logs, in a folder beside the deployments file (logs.rs).
    let folders = folders.with_logs(logs::Logs::beside(&deployments_at, log_caps));
    // The texts of files runs read are kept beside the content store (blobs.rs).
    let folders = Arc::new(if store_arg == ":memory:" { folders } else { folders.with_blobs(blobs::Blobs::at(Path::new(&store_arg).join("blobs"))) });
    durable::load_host_key(&host_key_arg).unwrap_or_else(|e| refuse_to_start(e));
    // Whether the server before stopped with every line on disk: after a crash, a run may have
    // been cut off between its commit and its line, and each deployment it left a state says so.
    let stopped_cleanly = stopping::stopped_cleanly(&deployments_at);
    let mut initial_deployments = load_deployments(&deployments_at, &store, &policy).unwrap_or_else(|e| refuse_to_start(e));
    if !stopped_cleanly {
        persist::note_cut_off_runs(&deployments_at, &mut initial_deployments);
    }
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
    // The mail its runs sent that wasn't delivered when the server stopped, sent now: what its
    // deployment's log holds of it (mails.rs).
    folders.mails.recover(|slug| initial_deployments.get(slug).map(|dep| dep.log.after(None, usize::MAX)));
    // And the messages to its queues (queues.rs).
    folders.queues.recover(|slug| initial_deployments.get(slug).map(|dep| dep.log.after(None, usize::MAX)));
    let flows = Mutex::new(load_flows(&flows_at).unwrap_or_else(|e| refuse_to_start(e)));
    let proposals = proposals::Proposals::beside(&flows_at).unwrap_or_else(|e| refuse_to_start(e));
    let deployments: Arc<Mutex<BTreeMap<String, Deployment>>> = Arc::new(Mutex::new(initial_deployments));
    folders.mails.serve(Arc::clone(&deployments));
    folders.queues.serve(Arc::clone(&deployments));
    spawn_clock_scheduler(store.clone(), Arc::clone(&deployments), deployments_at.clone());
    arrivals::spawn_watcher(store.clone(), Arc::clone(&deployments), deployments_at.clone());
    inbox::spawn_watcher(store.clone(), Arc::clone(&deployments), deployments_at.clone());
    subscriptions::spawn(store.clone(), Arc::clone(&deployments), deployments_at.clone());
    dequeue::spawn(store.clone(), Arc::clone(&deployments), deployments_at.clone());
    {
        // The run logs and event logs are synced to disk every second, so a power loss loses at most
        // the last second of them (recorder.rs's `sync_logs`), the witness's queue with them; and
        // before each batch of heads goes to a witness, so a witness never holds a head whose line
        // the disk lacks. A log a run found past what it keeps is cut back here too, so no run
        // waits for it.
        let syncing = Arc::clone(&deployments);
        let kept_at = deployments_at.clone();
        std::thread::spawn(move || {
            let mut failing = false;
            loop {
                std::thread::sleep(recorder::SYNC_EVERY);
                match sync_to_disk(&syncing, &kept_at) {
                    Ok(()) if failing => {
                        println!("holon: the run logs are synced to disk again");
                        failing = false;
                    }
                    Ok(()) => {}
                    Err(e) if !failing => {
                        eprintln!("holon: {e} (tried again every second)");
                        failing = true;
                    }
                    Err(_) => {}
                }
            }
        });
        let before = Arc::clone(&deployments);
        heads::before_send(Box::new(move || recorder::sync_logs(&before)));
    }
    let port = bound.port();
    // Everyone signs in (access.rs); a new server first needs its admin, set up with the code
    // it prints here.
    let accounts = Arc::new(accounts::Accounts::open(&accounts_arg).unwrap_or_else(|e| refuse_to_start(e)));
    // The tags each flow is deployed with, for a binding on a tag to cover it (bindings.rs).
    for (slug, dep) in deployments.lock().unwrap().iter() {
        accounts.set_tags(slug, dep.server.as_ref().map(|s| s.tags.as_slice()).unwrap_or_default());
    }
    if let Some(code) = accounts.setup_code() {
        println!("holon: no accounts yet. Open the editor and set up the first admin with this code: {code}");
    }
    // The names a flow's port answers to as well (dataplane.rs `host_allowed`).
    data_limits.names = allow_hosts.clone();
    let access = access::Access::new(Arc::clone(&accounts), allow_hosts, allow_origins);
    let activity = activity::Activity::beside(&deployments_at);
    store.1.connect(Arc::clone(&deployments), deployments_at.clone());
    let state = AppState {
        store,
        deployments,
        deployments_path: deployments_at,
        flows,
        proposals,
        flows_path: flows_at,
        port,
        listeners: Some(Mutex::new(BTreeMap::new())),
        data_limits,
        policy: crate::sync::RwLock::new(policy),
        policy_path: policy_at,
        secrets,
        folders,
        activity,
        access,
        editor: editor::Source::pick(editor_dir.map(paths::normalize)),
        sweeps: storage::Sweeps::default(),
        checked: Default::default(),
    };
    sync_listeners(&state);

    let gate = match accounts.principals().len() {
        0 => "waiting to be set up".to_string(),
        n => format!("{n} principal(s); signing in required"),
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
    // The store's values and the folders' texts no kept run names any more are swept (storage.rs).
    storage::start(Arc::clone(&state_cell));
    // Told to stop, it finishes the runs it started and syncs them before it exits (stopping.rs).
    stopping::install();
    std::thread::spawn(move || server.run());
    stopping::wait_until_told();
    let state = state_cell.get().expect("set above");
    state.store.1.stop();
    println!("holon: stopping: no new runs, {} going on (ctrl+c again stops at once)", stopping::DOOR.runs());
    stopping::drain(&stopping::DOOR, Duration::from_secs(5), |left| println!("holon: stopping: {left} run(s) still going on"));
    match sync_to_disk(&state.deployments, &state.deployments_path) {
        Ok(()) => {
            // Every line on disk: the next start takes this for a stop, not a crash.
            if let Err(e) = stopping::mark_stopped(&state.deployments_path) {
                eprintln!("holon: the mark of this stop wasn't left ({e}): the next start takes it for a crash");
            }
            println!("holon: stopped");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("holon: stopped, but {e}: the last second of runs may not be on disk");
            std::process::exit(1);
        }
    }
}

/// The tallies counted since the deployments file was last written, written (every flag cleared,
/// not only up to the first one set), then the run logs and event logs synced to disk, and the
/// witness's queue: every second, and as the server stops.
fn sync_to_disk(deployments: &Mutex<BTreeMap<String, Deployment>>, kept_at: &Place) -> Result<(), String> {
    let mut counted = false;
    for d in deployments.lock().unwrap().values_mut() {
        counted |= std::mem::take(&mut d.tallies_changed);
    }
    if counted {
        persist_deployments(kept_at, deployments);
    }
    recorder::sync_logs(deployments).and_then(|()| heads::sync())
}

/// Why the data folder `dir` is a warning, if it is: other users of this machine may read it
/// (its mode lets the group or others in). A folder the server makes is its owner's only
/// (`create_private_dir`), and so is every file it writes; one made before, or by someone else,
/// is left as it is and said, since what it already holds (the store's values) isn't written
/// again. None where the system has no modes.
fn open_to_others(dir: &std::path::Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir).ok()?.permissions().mode() & 0o777;
        (mode & 0o077 != 0).then(|| format!("the data folder {} can be read by other users of this machine (mode {mode:o}): it holds every flow's state and logs; make it yours alone: chmod 700 {}", dir.display(), dir.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// The control plane's requests being answered now, and how many may be at once: a request past
/// it is answered 503 at once (`Admitted::take`), so a flood takes threads that end at once, not
/// work (a run, a sign-in's hash). A stream's answer, once its handler has returned, isn't
/// counted.
static IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const MAX_IN_FLIGHT: usize = 512;

/// A control-plane request, answered if it is admitted. Each has a thread of its own (rouille's):
/// past `most` being answered at once, one more is told to try again rather than given work,
/// signed in or not. Its body is read by a deadline (`http::BODY_DEADLINE`), so a slow one gives
/// its place back.
pub(crate) fn admitted(state: &AppState, request: &Request, in_flight: &std::sync::atomic::AtomicUsize, most: usize) -> Response {
    // A server stopping (stopping.rs) takes no more work: a run, a deploy, a decision.
    if state.store.1.stopping() {
        return with_cors(&state.access, request, rouille::Response::json(&serde_json::json!({ "error": stopping::REFUSED })).with_status_code(503).with_additional_header("Retry-After", "5"));
    }
    let Some(_admitted) = Admitted::take(in_flight, most) else {
        return with_cors(&state.access, request, rouille::Response::json(&serde_json::json!({ "error": "the server is answering too many requests at once: try again in a moment" })).with_status_code(503).with_additional_header("Retry-After", "1"));
    };
    with_cors(&state.access, request, route(state, request))
}

/// One request counted in `IN_FLIGHT` while it is answered.
pub(crate) struct Admitted<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> Admitted<'a> {
    /// Count one more in `in_flight`, unless `most` are there already.
    pub(crate) fn take(in_flight: &'a std::sync::atomic::AtomicUsize, most: usize) -> Option<Admitted<'a>> {
        use std::sync::atomic::Ordering;
        in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < most).then_some(n + 1)).ok().map(|_| Admitted(in_flight))
    }
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
