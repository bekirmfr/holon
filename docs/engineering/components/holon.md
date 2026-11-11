# holon (the server and the command line)

A map of `crates/holon` for someone changing it: what each module does, what `holon serve` builds
and starts, which locks exist and the order they are taken in. How a request goes through, and
what it costs, is in [Performance](../themes/performance.md#2-how-a-request-goes-through); what
the server guarantees under load and failure is in [Resilience](../themes/resilience.md); who may
do what is in [Security](../themes/security.md). The host pieces it builds on (the store, rounds,
the registry) are [holon-host](holon-host.md).

## 1. What it is

One binary, `holon`, with one command per job (`main`'s match on the first argument):
`holon serve` runs the server; `new`, `check`, `show` and `fmt` handle flow files (flowcmd.rs);
`deploy`, `deployments`, `call`, `grant`, `secret`, `export`, `verify`, `history`, `restore` and
the rest talk to a running server (the `*cmd.rs` files, cmdline.rs, local.rs); `editor` is the
terminal editor (workbench/), `mcp` the agent's tools (mcp.rs, mcphosts.rs), `lsp` the language
server (lsp.rs); `version`, `update`, `uninstall` and `migrate` look after the install
(selfmanage.rs, migrate.rs). `holon-server` is an alias kept for scripts (src/bin/holon-server.rs).

The server has two planes. The **control plane** is the API the editor and the commands use, on
`rouille`, at `--at` (default `127.0.0.1:7420`), behind sign-in. The **data plane** is one HTTP
listener per deployed server flow, on its own port, on `hyper` over one `tokio` runtime, facing
callers nobody vetted, with no sign-in. Below both, every run goes through the same path
(firing.rs) and is recorded (recorder.rs).

## 2. How it works

### 2.1 What `serve` builds, in order

`serve` (main.rs) parses its flags, puts every default file under the data folder (`--data`, else
`~/.holon/data`), starts the witness exporter if `--export-heads` is set (`heads::start`), and
binds the control plane's address **first**, so a second server stops there before its clocks
and listeners start beside the first one's. Then: the store (`FileStore`, or `MemStore` for
`:memory:`) wrapped in a `SharedStore`, the cache budgets, the `Registry` and `SharedCache`, the
policy (`Policy::load`), the secrets key and store, the named folders and databases (never the
server's own files: `server_files`), the host key for resume tokens (`durable::load_host_key`),
the deployments (`load_deployments`) and flows (`load_flows`), the background threads (§2.4),
the accounts and their setup code, `access::Access`, the activity log, and `AppState`. It brings
the listeners in line (`sync_listeners`), writes the run file the user's own commands sign in
with (`local::publish`), sets the state, starts the housekeeping (`storage::start`) and runs.

A file that exists but doesn't parse (the policy, the secrets, the deployments, the folders, the
accounts) stops the start (`refuse_to_start`: "refusing to start rather than overwrite it").

### 2.2 The modules

| Area | Modules | Job |
|---|---|---|
| Control plane edge | http.rs, access.rs, can.rs, auth.rs, accounts.rs, oidc.rs, local.rs, activity.rs, editor.rs | `route` dispatches every request after `reject_untrusted` (Host, Origin, Content-Type, `MAX_BODY_BYTES`) and `Access::admit` (who, and whether `can` lets them take the route: can.rs's `ROUTES`, deny by default); `/auth/…` and `/admin/…` are auth.rs's; `with_cors` adds CORS to every answer; the built-in editor is served at `/` before the gate |
| Deployments | deploy.rs, persist.rs, lifecycle.rs, permissions.rs, grants.rs, restore.rs, history.rs, events.rs, flows.rs, wire.rs | deploy, delete, `/trigger`, run, pause, stop; the deployments and flows files; manifests, the policy, grants and permits; versions and restores; each deployment's events |
| Run path | firing.rs, units.rs, claims.rs, locks.rs, taint.rs, secrets.rs | `fire_with_state` and what it is held to; the flow and value caches; key claims and commit locks; redaction |
| Data plane | dataplane.rs, listeners.rs, data.rs, channels.rs, websockets.rs | the listeners and their limits; routing a request to its endpoint (`serve_data`); subscriptions as server-sent events; inbound WebSocket connections, a run for each block's event (`websockets::open`, hooked before routing) |
| Triggers | scheduler.rs, arrivals.rs, durable.rs, calls.rs | clocks, files arriving, runs that wait for an event, flows calling flows |
| Resources | folders.rs, blobs.rs, ranges.rs, databases.rs, engines.rs, mongo.rs, collections.rs, outbound.rs, secret_sources.rs, sealing.rs | what a flow touches outside its run, each bound or granted by the host |
| The record | recorder.rs, heads.rs, verify.rs, changes.rs, touches.rs | the hash-chained run logs, the witness, export and `holon verify`, what a dry run shows |
| Housekeeping | storage.rs, sweep.rs, paths.rs | the server's sweeps, the host's view, warnings, what happens when a disk fills; every path it keeps, whole and in its platform's form (`HostPath`, or a `Place` that may be nowhere, `:memory:`) |
| Commands | flowcmd.rs, deploycmd.rs, opscmd.rs, load.rs, grantcmd.rs, secretcmd.rs, flowscmd.rs, accountscmd.rs, cmdline.rs, workbench/, mcp.rs, mcphosts.rs, lsp.rs, pick.rs, jsonedit.rs, selfmanage.rs, migrate.rs | the CLI, the terminal editor, the agent and editor protocols |

Every module does `use crate::*`, and main.rs brings most modules' items into the crate root
(`use deploy::*` and the rest), so handlers call each other by bare name, as when this was one
file. tests.rs (9,000 lines) holds the in-process server
tests; fixtures.rs the small units they deploy.

### 2.3 `AppState`, `Deployment` and `SharedStore`

`AppState` (main.rs) is everything a handler needs: the `store`, the `registry` and `cache` (each
its own `Mutex`), the `deployments` table (`Arc<Mutex<BTreeMap<String, Deployment>>>`, shared
with the threads), the flows, `access`, the `port`, the running `listeners`, the data plane's
`data_limits`, the `policy` (a `RwLock<Arc<Policy>>`, swapped by `POST /permissions/reload`),
`secrets`, `folders`, `activity`, the `editor` and the `sweeps`.

A `Deployment` holds its unit, its derived triggers and `server` (routes and port), its `grant` and
the `permit` derived from it, its `life` (running, paused, stopped, and its sessions), and its
own shared parts, each behind its own lock: `state` (an `Arc<claims::StateCell>`), `log` (an
`Arc<LogCell>`), `secrets` (a `Vault`), `folders`, `channels`, `arrivals`. A run clones these out
under a brief table lock and works on them with the table released.

`SharedStore` is a cheap clone of five shared parts: the store behind one `Mutex` (`.0`), the
hub through which flows call each other (`calls::Hub`, `.1`), the puts a sweep spares
(`sweep::Puts`, `.2`), the `FlowCache` (`.3`) and the `ValueCache` (`.4`). It implements `Store`
by locking and delegating, so the `Registry` can own one clone while `/unit` writes through
another (`SharedStore`'s doc: no change to holon-host for it). `get_value` answers from the value
cache first. The flow cache keeps closures by root hash (`closure`, `--flow-cache`, default
`FLOW_CACHE_BYTES`, 256 MiB) and refuses a closure past `MAX_CLOSURE_UNITS` units or nested past
`MAX_CLOSURE_DEPTH`; the value cache (`--value-cache`, `VALUE_CACHE_BYTES`, 64 MiB) keeps values
up to `VALUE_CACHE_LARGEST`. Both count memory, not encoded bytes
([Resilience G7](../themes/resilience.md#g7-the-caches-keep-to-their-sizes-in-memory)).

### 2.4 The threads it starts

| Thread | Started by | What it does |
|---|---|---|
| a thread per control-plane request | `rouille::Server::new` (rouille 3.6's default executor, `Executor::Threaded`) | the control plane |
| `holon-data` runtime, and its blocking pool | `dataplane::runtime`, with the first listener | the data plane: each request read on a worker, run there only when `runs_here_for` says it can never wait, otherwise on `spawn_blocking` |
| the clock | `spawn_clock_scheduler` | every second, `run_due_clock_ticks` (the deployments due, fanned out on `thread::scope`), then `durable::expire_waiting` |
| the file watcher | `arrivals::spawn_watcher` | every `POLL` (1 s), new files in watched folders start runs |
| the syncer | `serve` | every `SYNC_EVERY` (1 s): writes the deployments file when tallies changed, then `recorder::sync_logs` (syncs each log, makes the cut-backs runs asked for) |
| the secrets refresh | `serve` | every `secret_sources::REFRESH` (60 s), `SecretStore::refresh` reads references again |
| the witness | `heads::start` | sends queued heads; `heads::before_send` runs `sync_logs` first |
| housekeeping | `storage::start` | every `LOOK_EVERY` (10 s), whether a round of sweeps is due |

A MongoDB binding keeps a lease thread of its own (mongo.rs).

### 2.5 Locks, and the order they are taken in

- **The table** (`AppState::deployments`) is only ever held briefly: to clone a deployment's
  parts out, to count, to persist's copy. A fire never holds it (firing.rs: "cloned out from
  under the table lock, which is never held here"; deploy.rs's `handle_post_trigger`).
- **`PERSIST`**, a static in `persist_deployments`, comes before the table: it serializes whole
  snapshot-and-write passes, so an older snapshot never lands after a newer one, and the disk
  write happens with no lock held but it.
- **Under the table** may be taken a deployment's log (`LogCell::with`, as deploy cuts a log to a
  new keep), its state (deploy saving the carried state's snapshot), or its vault
  (`count_if_unrecorded` redacting a failure, `persist_deployments` sealing waiting runs).
- **Never the table under a log's lock**: "The deployments table is never taken while this lock
  is held" (recorder.rs, `LogCell`). `push_run` takes the table, lets it go, makes the line, then
  takes the log; under the log, the channels send their packs.
- **`LogCell::synced`** is held across a sync, the log's own lock only to read what to sync, so
  runs go on appending while one syncs.
- **A run's state** (`StateCell::state`) is held only to copy it and to commit. Commit takes the
  folders' and databases' commit locks under it (`commit_locks`), then the store's lock (writes).
  Claims (`claims::Hold`) and run locks (`RunLocked`) are taken around it; every one of those waits
  is bounded (`claim_wait`, `commit_wait`), and "a run holding keys never waits for another's"
  (claims.rs), so a collision ends in a timeout and a rerun, not a deadlock.
- **The store's mutex** (`SharedStore.0`) is held across each file read and write.
- **The caches' locks are leaves**: nothing else is locked under them, and the store is never read
  while one is held (units.rs).
- **The listeners' lock** is never held with the table (`sync_listeners` reads the table, lets it
  go, then starts and stops listeners).

### 2.6 Deployments

`POST /deploy` (`handle_post_deploy` → `deploy`) derives the unit's capabilities, builds its
`Manifest`, refuses what the policy never grants, asks for what needs approval (`approval_needed`;
[Security](../themes/security.md#24-what-a-flow-may-do-grants-and-approvals)), and inserts the
`Deployment` under the table lock: a redeploy keeps the same state `Arc`, log, channels, open
WebSocket connections and waiting runs, adds a `Version`, and starts the deployment again. Then `persist_deployments`
writes the whole table (`write_private`), and `sync_listeners` runs. A stateful deployment's state
is kept apart, as a head file per slug (`state_head_for`: `<deployments>.state/<slug>`, written by
`StateCell::write_head` as each change lands). `load_deployments` re-derives what was never
persisted (triggers, `server`, `permit`) and pauses a deployment whose grant no longer covers its
manifest (`attention`). lifecycle.rs holds running, paused and stopped, and the sessions; a
deployment is deleted only once stopped.

### 2.7 The run path, and the record

A request on a listener becomes a `DataRequest`; `serve_data` (listeners.rs) looks the deployment
up for each request (a redeploy takes effect on the next one), routes it, and runs the endpoint
with `fire_with_state`. That runs on a copy of the state with no lock held and commits only if
what it read is unchanged, running again otherwise; after `OPTIMISTIC_ATTEMPTS` (4) it runs once
more holding the state. Effects the permit doesn't allow, and writes carrying a secret, are
dropped before commit (`run_attempt`). The entry goes to `log_or_count` / `log_run`: an
unrecorded run is counted, a strict one waits for `sync_through`. Each step is in
[Performance §2](../themes/performance.md#2-how-a-request-goes-through).

recorder.rs keeps each log as `<deployments>.logs/<slug>.jsonl`, a hash chain (`line_hash`), its
newest `IN_MEMORY` runs in memory, cut back by the syncer and sealed and backed up as storage.rs
says ([Resilience G5, G7b, G9](../themes/resilience.md#g9-the-disk-stays-bounded)). storage.rs and
sweep.rs are the housekeeping: the content store's sweep, its packing, the host's view
(`/admin/host`), and `when_full`.

## 3. Why it is this way

- **Plumbing, not a second decider.** The server adds no trust logic to what content addressing
  gives: `Store::get` verifies every unit and value against its hash (main.rs's module doc;
  [P4.5–P4.6 scoping](../../L4s/P4.5-P4.6-transport-scoping-L4.md)). What it adds is who may ask
  (access.rs, can.rs) and what a run may do (permissions.rs).
- **Two HTTP stacks.** The control plane stayed on `rouille`; the data plane, which faces
  strangers, is `hyper`, so every connection is held to limits from its first byte (dataplane.rs).
- **A short table lock, a lock per part.** Holding the table across a fire would let one slow flow
  block every deployment; a lock per log and per state is what took logging and state writes off
  the table ([request-path L4 C2, C3](../../L4s/request-path-L4.md)).
- **Optimistic runs.** Runs of one deployment proceed side by side and still lose no update
  (`concurrent_runs_lose_no_update`), because each commits only over what it read; a run about to
  call out first claims what it read ([key claims L4](../../L4s/key-claims-L4.md)).
- **Write-through persistence, owner-only.** The deployments file is rewritten whole after each
  change, by `write_private`, so a crash leaves the old file or the new one.

## 4. Reach and limits

- **The control plane takes a thread per request** (rouille's default executor), with no bound
  on how many and no timeout of its own: a slow handler holds one, and many requests at once,
  signed in or not, are as many threads. The data plane's limits (§2.4) are not applied here.
- **A redeploy saves the state it carries with the table held** (deploy.rs, `save_snapshot` inside
  the table's block), and a deploy writes the whole deployments file: deploys are not cheap, and
  every other request waits for the table meanwhile.
- **The deployments file is one file**: it holds every deployment's documents, versions and
  grants, and is rewritten whole on each change (state is not in it: C3).
- **The data plane has no sign-in and no Host check**: a flow guards its own routes; one bound to
  loopback is reachable by anything that reaches loopback
  ([Security](../themes/security.md#4-reach-and-limits)).
- **`Registry`'s published set and `SharedCache` are in memory only**: a restart forgets which
  hashes were published, not the units.
- **One sync thread** syncs every log and makes every cut-back in turn (Resilience G5).
- The rest of the run path's limits are in [Performance §4](../themes/performance.md#4-reach-and-limits).

## 5. What the tests hold

In tests.rs unless named, in-process through `route` and `serve_data` (`test_state`):

- **Isolation**: `a_fire_in_progress_does_not_stall_the_table`,
  `one_deployments_log_doesnt_hold_up_another`,
  `a_light_route_answers_on_the_data_planes_worker_as_any_route_does`,
  `a_panic_ends_its_run_and_nothing_else`.
- **State under concurrency**: `concurrent_runs_lose_no_update`,
  `a_state_is_kept_by_its_head_across_restarts_concurrent_writes_and_a_delete`, `redeploy_keeps_state`.
- **Persistence**: `deployments_persist_across_a_simulated_restart`,
  `flows_persist_across_a_simulated_restart`, `corrupt_persistence_file_is_an_error_not_an_empty_table`,
  `persist_writes_whole_files_and_leaves_no_temp_files`, `load_rechecks_each_grant_and_migrates_older_records`.
- **The data plane over sockets**: `a_server_deployment_answers_on_its_own_port_within_limits`,
  `a_server_deployment_whose_port_is_taken_says_it_is_not_answering`.
- **The caches and the closure bounds**: `the_flow_cache_lets_go_of_the_least_used_past_its_budget`,
  `the_store_keeps_the_values_it_read_within_a_budget`,
  `a_closure_too_big_to_run_is_refused_and_every_sample_is_well_inside`.
- **Nothing changes by accident**: `the_server_records_what_its_golden_record_holds`
  ([Testing](../themes/testing.md#nothing-changes-by-accident)).
- **The gate**: `cross_site_and_rebinding_requests_are_refused_before_routing`,
  `oversized_body_is_refused`, access.rs's and can.rs's own tests, the access table
  (`tests/access_table.rs`: every route, every kind of caller) and
  `the_gate_and_the_router_read_every_path_alike`.
- **By hand**: `request_throughput`, `profile_one_route` ([Measuring](../themes/measuring.md)).

Gaps: nothing checks the lock order itself (code that took the table under a log's lock would
pass until it deadlocked); nothing tests the control plane under many slow requests; no test sends a
data-plane request with a foreign `Host`. The others are in
[Testing §4](../themes/testing.md#4-gaps).

## 6. History

The history in this clone starts at `e49e371` (a shallow copy); the crate is older. Since then:

| Commit | Change |
|---|---|
| `a2861a7`, `ee66b6c` | Every deploy is a version, kept for good; a version's or a run's state restores |
| `64e78a9` | `holon editor`, the editor in a terminal |
| `6d062fe` | Closures kept in memory by hash, bounded in bytes (C1) |
| `8c28340`, `e32aecc` | A lock per deployment's log, an open file; logs synced every second and before each witness send (C2) |
| `ff3f8bd`, `43ebafc` | A run persists only what it changed; each state kept as a head of its own (C3) |
| `b1259e3`, `e40e7b5` | Strict by default, lazy where a handler says; unrecorded handlers (C2 e, C5) |
| `c5e5556` | A short route runs on the worker that read its request (12.1) |
| `0fdf913` | The value cache (12.4 b) |
| `dc21f76` | Resilience fixes R1–R4: the panic guard, only a run that can't wait on the worker |
| `4bb40a8` | An unrecorded run is never logged, failed or not |
| `6d764f5`, `de42b69` | The server sweeps its content store on its own, paced |
| `824db82` | A log keeps only its newest runs in memory (R8) |
| `156a5f0`, `4e3d47f` | What happens when the disk fills; old runs backed up |
| `09f0bdc` | Logs seal their older runs, gzipped |
