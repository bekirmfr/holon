# Request path: a run pays for its own flow, not for the server (L4)

**Status:** proposed (2026-11-02). Nothing here is built yet. Every claim about the code is cited
by file and line at `aa2704a` (`main`), and every number was measured on a release build of that
commit (Appendix A). §3 is measured; §4's "Results" say for each figure whether it is measured or
estimated. Appendix B lists problems found along the way that this note doesn't set out to fix.

A deployed flow answers one request in **0.5–0.8 ms**. Under load, its throughput **stops at about
1,000–1,700 requests a second however many connections there are**. A stateful flow, on a server
with 33 deployments, manages **53–65 a second**, with a median answer near **one second**. The
flow's own work is not the reason: a run computes in 0.1–0.4 ms. The rest is work the server does
around each run:

1. It **reads the whole flow from disk again** on every request, one file per unit, through a
   lock the whole store shares (§2.1).
2. It **writes the run record while holding the lock on the table of all deployments**. Every 500
   runs it also re-reads, re-parses and rewrites the whole log file there (§2.2).
3. For a stateful flow, it **rewrites and fsyncs the whole deployments file** (every deployment,
   with its document and kept versions) on every run, even one that only read its state (§2.3).

This note makes a request's cost depend only on its own flow and its own deployment:

> **Requests to different deployments never wait for each other, and no request does work that
> grows with the number of deployments or with the runs before it.** A run costs its evaluation,
> one appended log line, and, when it changed state, the writes that make that change durable.

It doesn't change what a run records, the order of a record and its answer, the file formats a
person reads, or the evaluator, and it weakens nothing that survives a crash. One part, C2 (d),
closes a gap found on the way. Today the run log is never fsynced, so a power loss can lose run
records whose state changes did survive, and can leave a witness holding heads the disk lost.

## Contents

1. Where Holon stands
2. What a request does today
3. Measured: what each cost is worth
4. The changes (C1–C4), each with what it touches, results and side effects
5. Lock order, before and after
6. Build order
7. Samples
8. Open questions
9. Invariant check
- Appendix A. How it was measured
- Appendix B. Found along the way

## 1. Where Holon stands

This is a reference point, not a target. The same two endpoints run as Holon samples and as a
.NET 8 minimal API (Kestrel, `CreateSlimBuilder`, logging off), on the same 4-vCPU machine,
loaded with `wrk` over loopback:

| | .NET 8 | Holon | |
|---|---|---|---|
| `GET /hello/:name` (`hello-api`), 64 connections | 96,980 req/s, p50 0.53 ms | 1,670 req/s, p50 37 ms | 58× |
| `POST /calc` (`calc`, JSON), 64 connections | 80,580 req/s, p50 0.67 ms | 1,020 req/s, p50 61 ms | 79× |
| `GET /hello/:name`, 1 connection | p50 0.105 ms | p50 0.556 ms | 5× |

The comparison favours .NET. Every Holon run also checks the deployment's permit, redacts
secrets, and writes a hash-chained record that can be replayed node by node. The telling number is
Holon's own: **1,480 req/s at 1 connection and 1,670 at 64**. More concurrency buys almost
nothing, so something on the request path runs one request at a time.

## 2. What a request does today

The data plane is `hyper` on one shared tokio runtime (`dataplane.rs:68`), with the default
blocking pool (up to 512 threads). Each request runs `serve_data_forwarding` on that pool
(`dataplane.rs:295`), which reaches `serve_flow` (`listeners.rs:76`). In order:

| # | Step | Code | Lock held | Measured cost |
|---|---|---|---|---|
| 1 | Check the caller's origin (clones the deployment's `browsers`) | `allowed_origin`, `listeners.rs:26`, from `dataplane.rs:291` | table, briefly | small |
| 2 | Look up the deployment, clone its handles | `listeners.rs:84–89` | table, briefly | small |
| 3 | **Load the flow's closure from the store** | `materialize_closure`, `units.rs:159`, from `listeners.rs:95` | **store mutex, once per unit** (`main.rs:300`) | 199 µs at 1 connection; **34 ms** at 64, mostly waiting for the lock |
| 4 | Evaluate the run: rounds, effects, and for state the optimistic commit | `data::handle` → `fire_with_state`, `firing.rs:120`, from `listeners.rs:148–153` | the state cell at commit (`firing.rs:203`), or for the whole of a last attempt (`firing.rs:270`) | 0.13 ms (`calc`) to 0.36 ms (`hello-api`) median, from the log's `duration_ms` |
| 5 | Count the trigger | `record_trigger`, `deploy.rs:867`, from `listeners.rs:207` | table, briefly | small |
| 6 | **Redact, number, chain and append the run record** | `log_run`, `firing.rs:786` → `Log::push`, `recorder.rs:497` | **table, for all of it** | about **150 µs** per run (§2.2) |
| 7 | **Persist the deployments table** (only if the deployment has state, or a run parked) | `persist_deployments`, `persist.rs:22`, from `listeners.rs:227–229` | `PERSIST` (global), then the table while it snapshots, then each cell, then the store | an fsync'd rewrite of the whole file: **80 KB with 3 deployments, 1.1 MB with 33** |
| 8 | Answer | back in `answer`, `dataplane.rs:263` | none | |

The other run kinds follow the same shape. Each loads its closure, runs, calls `log_run`, and (for
state or parked runs) calls `persist_deployments`:

| Run kind | Load closure | `log_run` | `persist_deployments` |
|---|---|---|---|
| Request | `listeners.rs:95` | `listeners.rs:226` | `listeners.rs:228` |
| Clock tick | `scheduler.rs:125` | `scheduler.rs:160` | `scheduler.rs:197` |
| File arrival | `arrivals.rs:373` | `arrivals.rs:407` | `arrivals.rs:412` |
| `/trigger` | `deploy.rs:924` | `deploy.rs:955` | `deploy.rs:958` |
| Action, and flow calls (`calls.rs:86` → `run_action`) | `deploy.rs:1079` | `deploy.rs:1119` | `deploy.rs:1121` |
| Resumed (durable) run | `durable.rs:637` | `durable.rs:651` | `durable.rs:684` (always) |

### 2.1 The closure is loaded on every request

`materialize_closure` (`units.rs:159–184`) walks the units from the root hash. It fetches each one
with `store.get` (`units.rs:168`), and `SharedStore::get` takes the single `Mutex` around the
whole store (`main.rs:300–302`). `FileStore::get` (`holon-host/src/lib.rs:665–672`) reads the
unit's file and checks it against its hash. `hello-api` has 19 units, and `strace` of one request
shows exactly those 19 opens.

The result can never change for a given hash. A unit's hash is its content, and **units are never
deleted**: the sweep removes values only ("Only values are swept, never the units (code) beside
them", `sweep.rs:21–22`). The work is repeated for nothing, and because it runs under the store's
global lock it also serializes requests across every deployment.

### 2.2 The run record is written under the table lock

`log_run` (`firing.rs:786–799`) takes the table lock (`firing.rs:787`) and, while holding it:

- builds a `Redactor` from the deployment's secrets (`Redactor::of`, `secrets.rs:852`, which
  locks the vault) and walks the whole entry with it, including every node's outputs in the
  timeline;
- computes channel packs (`Channels::packs_of`, `channels.rs:233`), using
  `dep.log.next_seq()`;
- calls `Log::push` (`recorder.rs:497–516`), which serializes the entry (4–6 KB here), hashes the
  line (`line_hash`), queues the head for the witness (`Log::export`, `recorder.rs:596` →
  `heads::note`, `heads.rs:76`), trims the in-memory entries (`prune`, `recorder.rs:470`) and
  writes the line (`keep_line`, `recorder.rs:521–549`).

`keep_line` calls `create_dir_all`, `path.exists()`, `OpenOptions::open` and `write_all` **on every
line**. Every `keep` lines (500 by default) it calls `cut_back` instead (`recorder.rs:554–593`).
That reads the whole file (`read_to_string`), **parses every line** into a `LogEntry` to get its
`at_ms`, joins the kept lines, and writes them with `write_private` (`sealing.rs:274–299`: temp
file, `sync_all`, rename).

Time inside the table lock per `hello-api` run, from timers in a throwaway build:

| Part | Average |
|---|---|
| Redaction and packs | 6–11 µs |
| Serialization | 13–23 µs |
| Append (`create_dir_all`, `exists`, `open`, `write`) | 20 µs |
| **Cut-back**: one every 500 runs, each taking **31–36 ms** (3.5 MB file); spread over the runs, about | 66 µs |
| Total held | **about 150 µs** (130–190 µs) |

So the table lock alone caps the whole server near 6,000 runs a second. Every cut-back also stops
**every deployment's** requests for about 33 ms.

This is a regression. `PLATFORM_LAYOUT.md` §7a recorded "disk I/O and per-deployment locks happen
under the global table lock, so one slow fire stalls every endpoint", and M0 marked it fixed ("no
disk I/O or per-deployment locks under the table lock ✅", `PLATFORM_LAYOUT.md:711`). The state
cell still follows that rule (`main.rs:420–432`; `JOURNAL.md:3951`, "Correction #2"). The run
log, added later, brought disk I/O back under the lock.

### 2.3 A stateful run rewrites every deployment

`persist_deployments` (`persist.rs:22–92`) runs after every run of a deployment with state, through
the per-run call sites in §2's second table, including runs that only read. It:

- takes a process-wide `PERSIST` mutex (`persist.rs:23,27`), so every stateful run of every
  deployment passes through it one at a time;
- under the table lock (`persist.rs:28–64`), clones **every** deployment's record: document,
  `versions_kept`, grant, sessions, and parked runs (sealed, which takes each deployment's secrets
  lock at `persist.rs:58`);
- for **every** deployment with state, calls `State::save` (`persist.rs:71`; `holon-host/src/state.rs:103`).
  That is a `put_value` per key, and each `put_value` at least stats the value's file under the
  store mutex (`holon-host/src/lib.rs:673–681`);
- serializes the table as pretty JSON and writes it with `write_private` (fsync, rename,
  `persist.rs:83–89`).

The cost grows with the server, not with the run:

| `counter` sample, 64 connections | 3 deployments (80 KB file) | 33 deployments (1.1 MB file) |
|---|---|---|
| `GET /last` (reads state) | 479 req/s, p50 127 ms | **66 req/s, p50 936 ms** |
| `GET /hit` (writes state) | 396 req/s, p50 153 ms | 65 req/s, p50 933 ms |

### 2.4 The state write path

A run that writes state commits under its cell's lock (`firing.rs:203–237`). It applies its writes
(`commit`, `firing.rs:468`), then calls `snapshot_after` (`firing.rs:230`; on a last attempt,
`firing.rs:298`) → `save_snapshot` (`restore.rs:23–32`). That puts each changed value and a
snapshot record into the store. `FileStore::put_value` writes a new file with `write_atomically`
(`holon-host/src/lib.rs:642–658`: temp, `sync_all`, rename) **while holding the store mutex**
(`main.rs:303–308`). So one deployment's state write stops every other deployment's store access
for the length of an fsync.

### 2.5 Two queues in a row

The table in §3 shows that removing either of §2.1 or §2.2 **alone** helps little. Requests pass
through the store lock and then the table lock, so throughput is set by whichever is slower, and
both have to go before the server scales with its cores.

## 3. Measured: what each cost is worth

The throwaway build gave `serve_flow` three switches, read from `HOLON_X`:

- `cache`: keep each closure in memory by root hash;
- `nolog`: skip `log_run`. This is an ablation to price the log, **not** a proposal; C2 keeps
  every record;
- `nopersist`: skip the per-run `persist_deployments`.

All runs used one server with 33 deployments, 64 connections, 6 s each, after a 3 s settle
(req/s · p50):

| Switches | `hello-api` | `calc` | `counter` `/hit` (writes) | `counter` `/last` (reads) |
|---|---|---|---|---|
| none (today) | 1,575 · 39 ms | 1,014 · 60 ms | 65 · 933 ms | 53 · 1.00 s |
| `cache` | 3,820 · 12.6 ms | 3,076 · 15.2 ms | 67 | 61 |
| `nolog` | 1,778 · 35 ms | 1,077 · 57 ms | 63 | 53 |
| `nopersist` | 1,418 · 43 ms | 1,009 · 61 ms | 863 · 69 ms | 879 · 72 ms |
| `cache` + `nolog` | **21,044 · 2.8 ms** | **16,285 · 3.6 ms** | 65 | 52 |
| all three | 19,279 · 3.0 ms | 16,314 · 3.6 ms | **1,453 · 40 ms** | **14,691 · 4.0 ms** |

At 1 connection, `hello-api`'s median goes from 0.82 ms to 0.42 ms with `cache` + `nolog`.

With all three off, what's left for `/hit` is §2.4. 64 connections write the same key, so runs
retry and then serialize on the cell (`firing.rs:167–300`, by design), and each commit makes two
fsyncs under the cell lock and the store lock.

## 4. The changes

Each change is laid out the same way: **what**, **today**, **after**, **touches** (every site,
with what happens there), **results**, **side effects**, **tests** and **docs**.

### C1. Closures by hash, in memory

**What.** Each closure stays in memory after it is first loaded, keyed by its root hash, so a run
looks its flow up instead of reading it from disk.

**Today.** `materialize_closure` returns an owned `MapResolver` built from scratch
(`units.rs:159`). Every caller builds one per run, per request, or per HTTP call. A hash that isn't
in the store is **left out without an error** (`units.rs:168–170`: "A hash that doesn't resolve in
the store is left out of the resolver rather than erroring", `units.rs:151–154`).

**After.**

```rust
// units.rs
pub(crate) struct Closures { /* Mutex<HashMap<Hash, (Arc<MapResolver>, u64 /* last use */, usize /* units */)>>, total units */ }

/// The closure of `root`, from memory when it was loaded whole before.
pub(crate) fn closure(store: &SharedStore, root: Hash) -> Result<Arc<MapResolver>, Closure> {
    if let Some(hit) = store.closures().get(&root) { return Ok(hit); }  // lock: lookup only
    let (resolver, whole) = load_closure(store, root)?;                 // no cache lock held
    let resolver = Arc::new(resolver);
    if whole { store.closures().insert(root, Arc::clone(&resolver)); }  // lock: insert + evict
    Ok(resolver)
}
```

- `materialize_closure` becomes a wrapper over `load_closure`, which also reports **whether every
  reference resolved** (`whole`). Only a whole closure is cached. A partial one might be completed
  by a later `POST /unit`, and caching it would keep it partial for good.
- **Errors aren't cached.** `TooBig` would fail the same way again, and `Store` errors are
  transient.
- **No invalidation is needed.** A redeploy is a new hash. A rollback (`handle_post_restore` with
  `code`) is a hash already seen. A resumed run evaluates **the unit it started on**, which can be
  older than the deployment's (`durable.rs:633–637`: `record.resume(...)` yields `unit`), and that
  is found by its own hash. Units are never deleted (§2.1).
- **Where it lives.** The cache is a fourth field of `SharedStore` (`main.rs:281`), beside `calls::Hub`
  and `sweep::Puts`, so every caller already holds it ("kept on the store handle every run already
  has", `calls.rs:23–24`). A closure is valid for the store it came from, so a per-store cache is
  the correct scope.
- **Bound.** It evicts the least recently used closures past a total unit count (§8 Q2). One
  closure may hold up to 20,000 units (`MAX_CLOSURE_UNITS`, `units.rs:187`). A closure bigger than
  the whole bound is used once and not kept.

**Touches.**

| Site | Function | Today | After |
|---|---|---|---|
| `main.rs:281–310` | `SharedStore` | `(Arc<Mutex<DynStore>>, Arc<calls::Hub>, Arc<sweep::Puts>)` | adds `Arc<units::Closures>`; `new`/`on_disk` (`main.rs:286–295`) build it |
| `units.rs:159–184` | `materialize_closure` | walks and builds | split into `load_closure` (also returns `whole`) and `closure` (the cache); `materialize_closure` keeps its signature for tests (`tests.rs:2945, 7197, 7216, 7223`) |
| `listeners.rs:95` | `serve_flow` | per request | `closure(...)`; `&resolver` becomes `&*resolver` at `WithEngine::new` (`listeners.rs:147`) |
| `scheduler.rs:125` | `fire_one_clock_tick` | per tick | `closure(...)` |
| `arrivals.rs:373` | `fire_one_arrival` | per file event | `closure(...)` |
| `deploy.rs:924` | `handle_post_trigger` | per `/trigger` | `closure(...)` |
| `deploy.rs:1079` | `run_action` (also flow calls, `calls.rs:86`) | per action or call | `closure(...)` |
| `durable.rs:637` | `resume_parked` | per resume, the run's own unit | `closure(...)` |
| `deploy.rs:162` | `deploy` | per deploy | `closure(...)`, which warms the cache for the first request |
| `persist.rs:132` | `load_deployments` | per deployment at start | `closure(...)`, which warms the cache |
| `deploy.rs:507` | `preview_value` | per deploy preview | `closure(...)` |
| `deploy.rs:749` | `handle_get_permission_view` | per view | `closure(...)` |
| `grants.rs:19` | `derived_capabilities` | per grant change or reload | `closure(...)` |
| `firing.rs:761` | `handle_post_fire` (`/fire`) | per call | `closure(...)` |
| `units.rs:256` | `handle_get_cache` | per call | `closure(...)` |

`MapResolver` (`holon-core/src/resolve.rs:34–58`) needs no change. `Resolver::resolve` takes `&self`,
and `WithEngine` borrows `&R` (`holon-module`, `WithEngine::new(real: &'a R, ...)`). Nothing on the
run path inserts into a resolver after it is built, so an `Arc<MapResolver>` shared between
concurrent runs is sound: `MapResolver` is `Send + Sync`, a `BTreeMap<Hash, Unit>`.

**Results.** *Measured* (the `cache` switch, which differs from C1 only in having no bound):
`hello-api` 1,575 → 3,820 req/s alone, and 21,044 together with the log off the table lock.
Store-lock waits leave the request path, since no unit is read per run.

**Side effects.**
- **Memory.** It holds the closures of everything deployed and recently run, which is small for
  flows of today's size (`hello-api` is 19 units) and bounded by the cap.
- **Integrity.** Each unit is checked against its hash when it is loaded, as now, and the cached
  closure is exactly what the hash names (I-d). A store file that changes on disk afterwards is
  never read again. Today such a file fails its hash check and the run fails with a 500.
- **A unit added to complete a partial closure** is picked up, because partial closures aren't
  cached.
- **Behaviour.** No answer, log entry or run id changes, since evaluation reads the same units.

**Tests.** New, in `units.rs` or `tests.rs`: the same `Arc` for one hash twice; a partial closure
isn't cached (put a missing unit, then the next call sees it); an error isn't cached; the bound
evicts least recently used; a redeploy answers with the new version on its next request; a resumed
run of a superseded version resolves its own unit. Unchanged and must pass: every existing test,
in particular the closure-bound tests (`tests.rs:7190–7225`).

**Docs.** None user-facing.

### C2. The run log off the table lock

**What.** Each deployment's log gets its own lock, the line is appended through a file kept open,
and a cut-back copies bytes instead of parsing. A line still reaches the OS before its run
answers. It reaches the disk within a second, and always before a witness hears its head.

**Today.** `Deployment.log: Log` (`main.rs:483`) is reached only through the table lock. `Log`
(`recorder.rs:354–375`) holds `next_seq`, `entries`, `keep`, `max_age_ms`, `file_oldest_ms`,
`file`, `written` (lines in the file), `cut`, `head`, `breaks` and `slug`. §2.2 describes what a
push does.

**No line is ever fsynced.** `keep_line` (`recorder.rs:521–549`) appends with a plain `write_all`,
and only a cut-back's `write_private` syncs (`sealing.rs:291`). The events log is the same: its
`keep_line` (`events.rs:150–170`) appends one event at a time and never syncs. Each head is queued
for the witness **before** its line is written: `Log::push` calls `self.export("run", …)`
(`recorder.rs:509`) and then `keep_line`, and `Events::push` calls `heads::note` (`events.rs:142`)
and then `keep_line` (`events.rs:145`). The exporter sends what's queued every 5 s (`main.rs:990`,
`Exporter::run`, `heads.rs:181–186`).

A process crash loses no line, because the OS still holds it. **A power loss or OS crash can
lose the newest lines**, and that has two consequences:

- **State can be ahead of the log.** Every state change is fsynced before its run answers (§2.4),
  but its line isn't. After a power loss, the state can reflect runs the log no longer has, and
  `holon verify`'s state check (`check_states`, `verify.rs:469`) reports "it began from a state the
  run before it didn't leave".
- **A witness can hold heads the disk lost.** A head sent in the 5 s before the power loss may name
  a line that never reached the disk. To the witness, the log then looks cut short, which is the
  tampering a witness exists to catch.

**After.**

- **(a) Its own lock.** `Deployment.log: Arc<LogCell>`, where
  `LogCell { log: Mutex<Log>, next_seq: AtomicU64 }`. `log_run` holds the table lock only to clone
  `Arc<LogCell>`, the secrets `Arc`, the channels `Arc` and the session number. It builds the
  `Redactor` and redacts the entry with **no lock held**. Then it locks that deployment's log to
  set `seq` and `prev`, compute packs, push and append, and it **returns the `seq` it assigned**.
  `next_seq` is mirrored into the atomic under the log lock, so a reader that only needs it never
  takes the log lock while holding the table (`events.rs:205`, `deploy.rs:1161`, `deploy.rs:1451`).
- **(b) A file kept open.** `Log` keeps `Option<File>` opened in append mode on the first write.
  `keep_line` becomes one `write_all`, with no `create_dir_all`, `exists` or `open` per line. The
  directory is created when the file is first opened. The line still reaches the OS before the
  answer (I-a), and nothing is buffered across runs.
- **(c) Cut-back without parsing.** `Log` keeps `lines: VecDeque<(u64 offset, u32 len, u64 at_ms)>`
  for each line in the file that parsed. `open_retaining` (`recorder.rs:409–448`) fills it from the
  read it already does, and `keep_line` appends to it. `cut_back` picks the kept lines from it,
  using the same rule as today (`recorder.rs:565–572`: the newest `keep`, none older than the age,
  always the newest line). It reads only those byte ranges from the file, writes them to the temp
  file, and renames that over the log (`write_private`). Lines that didn't parse (a line a crash cut
  short) are dropped at cut-back, as today, because only parsed lines are indexed. The handle is
  **closed before the rename and reopened after**. That's required on Unix, where the old handle
  would keep the replaced file, and it avoids renaming over an open file on Windows.
- **(d) Synced lazily, within a bound.** The log stays off the request's critical path for
  durability, as it is today. It is fsynced in the background instead of never:
  - **Dirty flags.** `Log` and `Events` each gain `dirty: bool`, set by `keep_line` when it appends
    and cleared when a sync takes it.
  - **`recorder::sync_logs(deployments) -> Result<(), String>`.** Under the table lock, it clones
    each deployment's `Arc<LogCell>`, and, for each deployment whose events log is dirty, takes that
    flag and file path (events are pushed under the table, `events.rs:203–206`). Then it releases
    the table. For each log it locks the log, takes the flag and path, and releases it. With no lock
    held, it opens each taken file and calls `sync_data`. On Linux an fsync through any descriptor
    flushes the file's data, so the append handle isn't shared. A failed sync puts its flag back
    and returns the error.
  - **Why taking the lock is enough.** A head is queued inside `push` while the push holds the log
    lock (or the table lock, for an event). So by the time `sync_logs` has taken a log's lock, every
    head already queued for that log has its line written, and the sync after covers it.
  - **Against a cut-back.** A sync that opens the path after a cut-back's rename flushes the new
    file, which `write_private` already synced, plus any lines appended since. One that opened the
    old file just before the rename flushes the old inode. The lines that file held were copied
    into the new file and synced there, and lines appended afterwards set the flag again.
  - **A syncer thread** calls `sync_logs` every `SYNC_EVERY` (1 s, §8 Q7), started beside the
    clock scheduler and the file-text sweeper (`main.rs:1075–1080`).
  - **Before every witness send.** `Exporter::flush` (`heads.rs:130–160`) calls a hook just before
    each batch's POST (`heads.rs:141`), after the batch is taken from `pending`. If the hook fails,
    flush returns `Err` and sends nothing, and the exporter's existing back-off retries
    (`heads.rs:181–200`). `heads::start` runs before the deployments table exists (`main.rs:990`
    against `main.rs:1074`), so the hook is installed afterwards with a new
    `heads::before_send(Box<dyn Fn() -> Result<(), String> + Send + Sync>)` that calls `sync_logs`.

  *Considered and rejected: writing the log in the background,* answering first and appending later
  from a queue:
  - **It gains little.** After (a)–(c), the log's 35–45 µs per run is mostly CPU (serializing and
    hashing), which a background thread still spends. On a CPU-bound host, throughput wouldn't
    move, and latency would drop by under 10% (the 35–45 µs out of about 0.42 ms at one
    connection, §3).
  - **It breaks I-a.** A process crash after the answer and before the write would leave runs that
    answered, called out and changed state with no record. Today a process crash loses nothing.
  - **Readers lose read-your-writes.** An agent reading its run right after the request finds no
    entry. `run_action` names an entry not yet written ("its run log says why (entry {seq})",
    `deploy.rs:1125`). Every reader would have to flush the queue first.
  - **Channel packs lose their timing.** They go out "as it is logged… what a subscriber gets is
    what the log holds" (`firing.rs:791–792`), so a queue would put them ahead of their record or
    delay them.
  - **The queue is no help under load.** An unbounded queue grows without limit when the writer
    falls behind, and a bounded one blocks, which makes logging synchronous exactly under load.
  - **A failed write surfaces too late,** after the caller has its answer.

  The latency it would buy is available without those costs. Redact and serialize the entry body
  before taking the log lock, and splice `prev` in under it, so only the hash and the `write` are
  serialized. Batched appends, where each run waits for its batch's `write`, are the fallback if
  the `write` still shows in a profile.

**Touches.** Every use of a deployment's `log`:

| Site | Function | Today | After |
|---|---|---|---|
| `main.rs:483` | `Deployment` | `log: Log` | `log: Arc<LogCell>` |
| `firing.rs:786–799` | `log_run` | everything under the table lock | clone the `Arc`s, redact unlocked, push under the log lock; returns `seq` |
| `recorder.rs:497–516` | `Log::push` | serialize, hash, export, prune, `keep_line` | unchanged, except it records the line's offset |
| `recorder.rs:521–549` | `keep_line` | open per line | write through the open handle; record `(offset, len, at_ms)` |
| `recorder.rs:554–593` | `cut_back` | read, parse all, rewrite | copy the kept ranges, reopen the handle |
| `recorder.rs:409–448` | `open_retaining` | parses each line | also records offsets |
| `recorder.rs:452–468` | `retain` | reopens when keeping more | unchanged (reopening rebuilds the index) |
| `recorder.rs:611–616` | `lines` | reads the file | unchanged, called under the log lock |
| `recorder.rs:625–629` | `delete_file` | removes the file | closes the handle first |
| `deploy.rs:338–341` | `deploy` (redeploy) | `mem::take(&mut old.log)` then `retain` | clone the old `Arc<LogCell>`, lock it to `retain` |
| `deploy.rs:339`, `persist.rs:221–222` | new deployment, load | `Log::for_deployment` | wrapped in `LogCell` |
| `deploy.rs:841` | `handle_delete_deployment` | `delete_file` under the table lock | the same, through the log lock |
| `deploy.rs:1118–1119` | `run_action` | reads `next_seq`, then `log_run` in a second lock (Appendix B.1) | uses the `seq` `log_run` returns |
| `deploy.rs:1155–1165` | `handle_get_log` | `after`, `next_seq`, `head`, `breaks` under the table lock | clone the `Arc`, release the table, read under the log lock (one lock, so they agree) |
| `deploy.rs:1419`, `deploy.rs:1451` | `restore_plan` | clones every entry under the table lock | the same, under the log lock |
| `deploy.rs:1526` | `state_change` | the same | the same, under the log lock |
| `changes.rs:64` | `handle_get_changes` | the same | the same, under the log lock |
| `collections.rs:148` | `handle_get_collection` | `log.shows` under the table lock | under the log lock |
| `events.rs:205` | `Deployment::note` | `log.next_seq()` | the atomic |
| `events.rs:226` | `handle_get_events` | `log.next_seq()` | the atomic |
| `grants.rs:47` | `recheck` | `log.retain` | lock the log, then `retain` (`recheck` is called with `&mut Deployment` under the table; the log lock is taken inside, which is the allowed order, §5) |
| `verify.rs:100` | `handle_get_export` | `lines` and `head` under the table lock | both under one log lock, so the export's lines and head agree |
| `sweep.rs:137` | `named` | `log.lines()` for every deployment under the table lock | clone each `Arc` under the table, release it, read each log under its own lock |
| `folders.rs:1702` | `folders::sweep` | the same | the same |
| `tests.rs` | 3 direct uses of `.log.` | | updated to lock |
| `recorder.rs:354–375`, `521–549` | `Log`, `keep_line` (d) | no sync | `dirty` set on append |
| new, `recorder.rs` | `sync_logs` (d) | | takes the dirty flags and paths, then syncs with no lock held |
| `events.rs:150–170` | `Events::keep_line` (d) | no sync | `dirty` set on append |
| `heads.rs:50–58`, `130–160` | `Exporter`, `flush` (d) | sends what's queued | calls the `before_send` hook before each POST, and sends nothing if it fails |
| new, `heads.rs` | `before_send` (d) | | installs the hook once |
| `main.rs:1074–1090` | server start (d) | starts the scheduler, the arrival watcher and the text sweeper | also starts the syncer thread, and installs the hook when `--export-heads` is set |

**Results.** *Measured*: removing the log's work from the table lock is worth 3,820 → 21,044 req/s
together with C1 (the `cache` + `nolog` row is the upper bound). *Estimated*: C2 keeps the log, so
about 35–45 µs per run remains under **that deployment's** lock: serialize 13–23, hash and
export a few, write about 5, and the cut-back's share once it copies instead of parsing (a 2–4 MB
copy every 500 runs, under 10 µs a run). That bounds one deployment near 20,000 runs a second,
with no bound across deployments. A cut-back stalls only its own deployment, for a few ms instead
of 33.

*(d), measured and estimated.* An fsync inside the server took 113 µs on average here, plus
31 µs for the rename (`strace -c` during `/hit`; 1,766 fsyncs for 882 requests). Syncing costs
one fsync per **dirty** log per second, whatever the request rate: about 0.015% of one core per
active deployment, on the syncer thread and off every request. Before a witness send, it adds at
most one fsync per log dirty since the last tick, about one per active deployment every 5 s. The
only thing a request does differently is set a flag. A sync holds a log's lock only to take the
flag and the path, not during the fsync. *Estimated*: no measurable change to §3's figures.

**Side effects.**
- **Lock order** gains one lock (§5). The log lock is never held while taking the table. A reader
  that holds the table clones the `Arc` and releases the table before locking the log, so a
  cut-back never stalls the table.
- **Run order within a deployment** is the order in which runs take its log lock, as today, where
  the table lock decides it. Runs of different deployments are no longer ordered against each
  other, and nothing depends on that: seqs and chains are per deployment.
- **One more open file per deployment** with a log, held for the server's life. 33 deployments
  is 33 descriptors.
- **Readers** (log page, export, changes) no longer block runs of other deployments while they
  clone a whole log (Appendix B.3).
- **File format, location and chain** are unchanged (I-b). `holon verify` and an operator's tail
  see the same bytes.
- **Witness export** still goes through `heads::note` (`heads.rs:106–120`), which opens and appends
  `pending.jsonl` under a server-wide lock on every run when `--export-heads` is set. That is
  unmeasured (Appendix B.2) and outside C2.
- **(d) bounds what a power loss loses.** Run and event records written at most about one
  `SYNC_EVERY` (plus an fsync) before the loss can still be lost. Nothing older than that can be.
  A head a witness holds always names a line on disk (I-f), because a head is only sent after
  its line is synced.
- **(d) doesn't close the state-ahead-of-log gap, only bounds it.** After a power loss, the state
  can still reflect up to about a second of runs the log lost, and `holon verify` reports that as a
  break at the end of the log. Closing it means syncing the record with the state head before the
  answer (§8 Q6).
- **(d) adds steady fsync traffic:** one fsync per active log per second. On a slow disk with 33
  busy deployments, that is up to about 66 ms of device time a second at 2 ms each, on the syncer
  thread. It competes for the device with the runs' own state fsyncs (C3), which could slow them
  slightly. Requests never wait for it.
- **(d) makes a witness send wait** for the sync, up to a few ms. A sync that fails (disk full, an
  I/O error) holds the witness back until a sync succeeds, which shows in the exporter's waiting
  count, and the server prints why.
- **(d) doesn't sync the witness's own queue file.** `pending.jsonl` (`heads.rs:114–120`) is still
  not synced, so a power loss can drop heads not yet sent. The witness then holds fewer heads, which
  is not a false alarm (§8 Q7).
- **(d) doesn't sync the directory** after a cut-back's rename, so that gap from §8 Q5 remains.

**Tests.** Unchanged and must pass, in `recorder.rs`:
`numbers_entries_reads_past_a_seq_and_keeps_the_newest` (710),
`each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back` (733),
`a_line_changed_removed_or_inserted_outside_holon_is_a_break_at_the_entry_after_it` (765),
`entries_from_before_the_chain_read_back_and_the_chain_starts_after_them` (798),
`a_log_keeps_runs_by_age_and_reads_back_more_when_it_keeps_more` (835) and
`a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded` (877). In `tests.rs`:
`a_flow_log_survives_a_restart_and_goes_with_its_deployment` (3785),
`a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break` (3813),
`exported_runs_verify_and_a_rewritten_log_doesnt` (4174),
`a_flow_runs_another_flows_action_and_both_logs_say_so` (7419) and the secret-redaction tests
(4308, 4398, 4570, 4722, 6079).

New:
- a cut-back's file is byte-identical to today's for the same lines, including a cut line before
  it and a line with an unknown field;
- appends after a cut-back land in the new file;
- an action's reported `seq` is its own entry under concurrent runs (B.1);
- while a test holds deployment A's log lock, deployment B answers;
- (d) an append sets the dirty flag, `sync_logs` clears it, and a failing sync (a path that can't be
  opened) puts the flag back and returns the error;
- (d) a witness sees the hook run before every POST. Use the local witness server the `heads.rs`
  tests already start (`heads.rs:234`, `heads.rs:286`) and a hook that counts calls. A failing hook
  sends nothing, and the heads stay queued;
- (d) a line appended while a cut-back runs is synced by the next tick.

Losing power can't be simulated in a test. The tests pin the order the guarantee rests on
(written, then synced, then sent) and leave the disk's part to the OS.

**Docs.** For (d), `docs/manual/reference/server.md`'s run-log section (`server.md:163–176`) gains
"A run's entry is written before its answer and on disk within a second; a power loss can lose
the last second of entries." The witness paragraph (`server.md:180–182`) gains "A head is sent
only once its entry is on disk."

### C3. Persist what changed, and only that deployment

**What.** A run persists only when it changed something durable. A state change persists as a small
per-deployment pointer, not as the whole table.

**Today.** §2.3. The deployments file holds each deployment's state as `state_refs`, a key → value
hash map (`wire.rs:224–226`), and load rebuilds the state from it (`persist.rs:172–188` →
`State::load`, `holon-host/src/state.rs:111`). A committed run's snapshot is in the store already:
`save_snapshot` puts each changed value and a record of key → value hash, and the run logs that
record's hash as `state_after` (`firing.rs:103`, `firing.rs:230/298`).

**After.**

- **(a) Persist on change.** The per-run calls (`listeners.rs:228`, `scheduler.rs:197`,
  `arrivals.rs:412`, `deploy.rs:958`, `deploy.rs:1121`) persist only when **the run changed its
  state** or **parked**. "Changed" means `state_after` differs from the snapshot hash of
  `start_state` (`firing.rs:89`, hashed with `restore::snapshot_hash`, `restore.rs:36`, in memory).
  A run that wrote a value equal to the old one changed nothing and persists nothing. A resumed run
  always changes the parked set, so `durable.rs:684` stays unconditional.
- **(b) A state head per deployment.** `deployments.state/<slug>` (beside `deployments.logs/`,
  derived from the deployments path as `recorder::file_for` does, `recorder.rs:384`) holds the hex
  hash of the deployment's current snapshot. It is written with `write_private` (temp, fsync,
  rename):
  - by a run, right after `snapshot_after`, **still under the cell lock** (`firing.rs:230`,
    `firing.rs:298`). The cell gives the order: two commits write their heads in commit order, so a
    later head never gets overwritten by an earlier one. The values and snapshot record are durable
    (`write_atomically`) before the head names them;
  - by the two places that replace state outside a run, both under the cell lock:
    `put_state_back` (`deploy.rs:1613–1625`, restore) and the lifecycle's run-from-session
    (`deploy.rs:1709`). Both already save a snapshot of what they replace;
  - by deploy, for a new stateful deployment's empty state (`deploy.rs:382`).

  The cell needs to know where its head goes, so `StateCell` (`claims.rs:26–35`) gains
  `head: Option<PathBuf>`, set where cells are made (`deploy.rs:382`, `persist.rs:182–186`), and a
  method `write_head(&self, live: &State, snapshot: &str)`.
- **(c) The deployments file holds definitions.** `persist_deployments` is still called on every
  definition change: deploy (`deploy.rs:420`), delete (`845`), restore (`1361`), lifecycle (`1735`),
  grant (`grants.rs:126`), policy reload (`grants.rs:167`), and parked-set changes (a)/(b). It still
  writes `state_refs` (§8 Q3), but load prefers the head.
- **(d) Load.** In `load_deployments`, when `deployments.state/<slug>` exists, the state comes from
  `restore::snapshot_refs(head)` (`restore.rs:46–57`) → `State::load`. Otherwise it comes from
  `state_refs` (an older file) and the head is written. A head naming a snapshot the store lacks is
  an error, as a bad `state_refs` is now (`persist.rs:180`).
- **(e) Delete.** `handle_delete_deployment` (`deploy.rs:841–845`) removes the head file, as it
  removes the log (`dep.log.delete_file()`).

**Touches.**

| Site | Function | Today | After |
|---|---|---|---|
| `listeners.rs:227–229` | `serve_flow` | `state.is_some() \|\| parked_any` | `changed \|\| parked_any` |
| `scheduler.rs:196–198` | `fire_one_clock_tick` | `is_state \|\| parked_any` | `changed \|\| parked_any` |
| `arrivals.rs:411–413` | `fire_one_arrival` | `state.is_some() \|\| parked_any` | `changed \|\| parked_any` |
| `deploy.rs:957–959` | `handle_post_trigger` | the same | the same |
| `deploy.rs:1120–1122` | `run_action` | the same | the same |
| `durable.rs:684` | `resume_parked` | always | always (the parked set changed) |
| `firing.rs:67–104` | `FiredRun` | `state_after`, `start_state` | a `changed()` helper |
| `firing.rs:230`, `firing.rs:298` | `fire_with_state` (commit) | `snapshot_after` | then `cell.write_head(...)`, under the same lock |
| `firing.rs:458–462` | `snapshot_after` | | unchanged |
| `claims.rs:26–35` | `StateCell` | `state`, `claims` | adds `head` and `write_head` |
| `deploy.rs:382` | `deploy` | makes the cell | gives it its head path, writes the empty head |
| `deploy.rs:1613–1625` | `put_state_back` | replaces live state | also writes the head |
| `deploy.rs:1707–1710` | `handle_lifecycle` (run from a session) | replaces live state | also writes the head |
| `deploy.rs:841–845` | `handle_delete_deployment` | deletes the log | also deletes the head |
| `persist.rs:172–188` | `load_deployments` | state from `state_refs` | from the head when present |
| `persist.rs:22–92` | `persist_deployments` | | unchanged; called less often |
| `docs/manual/reference/server.md:22–33` | data-folder table | | adds `deployments.state/`, "each stateful deployment's current state, as a snapshot hash" |

`sweep.rs` needs nothing. The sweep keeps whatever the **in-memory** live state names
(`sweep.rs:148–153`), not what the file names. A snapshot a head names is the live state's, so
it's kept.

**Results.** *Measured* (the `nopersist` switch, an upper bound for (a)+(b)): `/last` 53 → 879
req/s alone and 14,691 with C1 and the log off the table lock; `/hit` 65 → 863 alone and 1,453
with both. *Estimated*: a reading run makes no write at all, so `/last` should match the
measured 14,691. A writing run adds one small fsync'd file under its cell lock, so `/hit` should be
somewhat below 1,453. Neither depends on how many deployments the server has any more.

**Side effects.**
- **Counters persist less often.** `trigger_count`, `error_count` and the session tallies
  (`record_trigger`, `deploy.rs:867–875`) are no longer written on a read-only stateful run. They
  persist with the next write or definition change, as they already do for stateless deployments.
  A crash loses the counts since then and nothing else (§8 Q1).
- **`deployments.json`'s `state_refs` can be behind** the head between definition changes. Load
  prefers the head, so this matters only to a person reading the file, or a downgrade (§8 Q3).
- **One more fsync under the cell lock** per writing run. Today the whole-table fsync happens
  outside the cell lock but under the global `PERSIST` lock. Runs of one deployment were already
  serialized by both, and runs of different deployments no longer wait for each other.
- **A new folder**, `deployments.state/`, with one small file per stateful deployment. Moving a
  server by copying the data folder (`server.md:38–39`) carries it. `holon migrate`
  (`migrate.rs:31–42`) concerns pre-rc.17 names only and needs no entry.
- **Crash between the snapshot and the head:** the head names the previous snapshot, and the run
  never answered, so nothing acknowledged is lost (I-c). **Crash after the head:** the new state
  loads.

**Tests.**

| Test | Effect |
|---|---|
| `state_is_kept_by_reference_and_its_root_survives_a_restart` (`tests.rs:2059`) | asserts `file[0]["state_refs"]["k"]` right after a write (`tests.rs:2081–2084`). It changes to assert the head file names a snapshot with that reference. The restart half is unchanged |
| `trigger_with_state_persists_across_calls` (2242) | unchanged |
| `clock_tick_with_state_persists_across_ticks` (3169) | unchanged |
| `state_persists_across_a_simulated_restart` (3285) | unchanged; it now exercises the head |
| `unloadable_records_are_an_error_and_state_always_survives_a_restart` (2018) | unchanged |
| `persist_writes_whole_files_and_leaves_no_temp_files` (2173) | unchanged; extend it to `deployments.state/` |
| `a_collection_keeps_items_by_key_across_runs_and_restarts` (2474) | unchanged |
| `deployments_persist_across_a_simulated_restart` (1800) | unchanged |

New:
- a stateful run that only reads leaves `deployments.json` byte for byte unchanged;
- a write survives a restart through its head;
- an old file with `state_refs` and no heads loads with its state and writes the heads;
- a restore and a run-from-session survive a restart;
- deleting a deployment removes its head;
- concurrent writing runs leave the head naming the last commit.

**Docs.** `docs/manual/reference/server.md`'s data-folder table.

### C4. The store lock (after C1–C3 are measured)

**What.** On-disk store reads and writes stop serializing on one mutex, while the sweep keeps its
guarantee.

**Today.** `SharedStore` is `Arc<Mutex<DynStore>>`, with `DynStore = Box<dyn Store + Send>`
(`main.rs:272–310`). Every `get`, `put`, `get_value` and `put_value` takes the mutex, including
`FileStore::put_value`'s fsync. The sweep **depends** on that mutex. It holds `store.0` while it
checks whether a value was put lately and removes it (`sweep.rs:198–212`), and `Puts::put` is
"called with the store held, so a sweep, which holds it too while it removes a value, sees every
put before it or none of it" (`sweep.rs:64–66`). Without it, a run could find a value's file
present and skip writing it (`holon-host/src/lib.rs:677`) just before the sweep removes it.
`folders::kept_text` also locks `store.0` directly (`folders.rs:1683`).

**After.**
- `SharedStore` holds `Arc<RwLock<()>>`, a **sweep gate**, beside the backend. Puts and gets take
  it shared. The sweep's check-and-remove (`sweep.rs:199`) takes it exclusively, which keeps the
  guarantee above.
- The backend is called without an outer mutex when it is a `FileStore`, whose operations are safe
  side by side: names are content hashes, temp names are unique per call
  (`holon-host/src/lib.rs:643–646`), the rename is atomic, and two puts of one value write the same
  bytes. `Store::put` and `put_value` take `&mut self` (`holon-host/src/lib.rs:512–523`), so the
  backend becomes an enum: `Disk(FileStore)` called through `&self` helpers, and
  `Locked(Mutex<DynStore>)` for `MemStore` and the tests' own backends (`tests.rs:1807`,
  `tests.rs:3289`).
- `folders.rs:1683` goes through `SharedStore::get_value` instead of `store.0`.

**Touches.** `main.rs:272–310` (`SharedStore`, its `Store` impl), `main.rs:1014–1020` (where it is
built), `sweep.rs:164–212` (`sweep`, `store.2`), `folders.rs:1683` (`kept_text`), and
`holon-host/src/lib.rs:614–690` (`FileStore`: `&self` variants of `put` and `put_value`).

**Results.** *Not measured.* It removes the last server-wide lock from a writing run (§2.4). Its
worth shows on `/hit` with several stateful deployments written at once, which §3 didn't measure.
That's why it comes after C1–C3 are re-measured.

**Side effects.** The sweep waits for in-flight puts to finish, and puts wait for one removal at a
time. That ordering is the same as today's. Reading `sweep.rs` in full (`named`, `Puts` forgetting
at `FORGET_AT`) is part of this step, so the guarantee is checked rather than assumed.

**Tests.** Unchanged and must pass: the sweep tests in `sweep.rs` and `tests.rs`. New: a put racing
a sweep never loses a value a committed run names.

**Not proposed: group commit.** A writing run makes three small fsyncs (the value, the snapshot and
the head), one after another per deployment. On a slow disk (0.5–2 ms per fsync is common) that
caps one deployment at a few hundred writes a second, still several times today's 65. Moving those
writes out of the cell lock is subtle. Run B commits after run A and names A's values, so B's head
must not become durable before A's values are, and B's `save_snapshot` writes only what changed
since **B's** start (`restore.rs:25–28`). A per-server journal with one fsync per batch is the known
answer. It gets its own note if measurements on real disks ask for it (§8 Q4).

## 5. Lock order, before and after

Today, at `aa2704a`:

| Path | Order taken |
|---|---|
| `log_run` | table → secrets (`Redactor::of`) → channels' `sent_waiting` → heads' `pending` (with `--export-heads`); **file I/O under the table** |
| `persist_deployments` | `PERSIST` → table → each secrets lock; then (table released) each cell → store |
| a run's commit | cell → store (fsync inside) |
| a last attempt that calls another flow | cell (caller) → table (`run_action`, `deploy.rs:1055`) → … |
| `put_state_back` | table → cell (`deploy.rs:1614, 1617`) |

The last two rows invert each other (Appendix B.4).

After C1–C3:

| Path | Order taken |
|---|---|
| `log_run` | table (clone `Arc`s, release) → secrets (release) → **log** → channels' `sent_waiting` → heads' `pending`; file I/O under the log only |
| log readers | table (clone `Arc`, release) → **log** |
| a run's commit | cell → store → head file I/O (no lock) |
| `closure` | closures cache (lookup or insert only); loading takes the store, never while holding the cache |
| `sync_logs` (C2 d) | table (clone `Arc`s, take the events' flags, release) → each **log** in turn (take the flag, release); the fsync with no lock held |
| a witness send (C2 d) | `pending` (take the batch, release) → `sync_logs` → POST with no lock held |

The rules: **the log lock is never held while taking the table or a cell**, and **the closure
cache's lock is a leaf**. A witness send holds `pending` only to take its batch, never while it
syncs, so the existing order (log → `pending`, inside `push`) can't invert. C2 and C3 add no edge
that could close a cycle. B.4 is untouched by them.

## 6. Build order

One commit per step, per `CLAUDE.md`. Each commit gives its before and after numbers from the
throughput check (step 1) and says the step was run live.

1. **The throughput check.** An `#[ignore]`d test in `tests.rs` that starts the server in process,
   deploys `hello-api`, `calc` and `counter` plus 30 filler deployments, fires requests from N
   threads, and prints req/s, p50 and p99 per route. It runs with
   `cargo test --release -p holon -- --ignored request_throughput`. It is not a CI gate, since CI
   timing is too noisy. The commit records today's numbers.
2. **C1**, closures by hash.
3. **C2 (a)(b)**, the log's own lock and the open handle, with B.1 fixed by the returned `seq`.
   Steps 2 and 3 together are where the server starts to scale (§2.5).
4. **C2 (c)**, the cut-back without parsing.
5. **C2 (d)**, the logs synced lazily and before every witness send, with the manual's two lines.
   It's a durability fix rather than a speed one, and the commit says so. Its throughput numbers
   should match step 4's.
6. **C3 (a)**, persist on change.
7. **C3 (b)–(e)**, the state head, load, migration, delete, and the manual's line.
8. **C4**, after re-measuring with several stateful deployments written at once, and reading
   `sweep.rs` in full.

Steps 2–6 change no file format. Step 7 adds `deployments.state/`.

## 7. Samples

This changes no feature, so per `CLAUDE.md` it adds no sample and changes none. `hello-api`,
`calc` and `counter` are what the throughput check deploys and loads. Each step is run live before
its commit (deployed, then sent load), and the commit says so. The end-to-end tests (`editor/e2e`)
and `editor/src/samples.test.ts` must stay green.

## 8. Open questions

1. **Counters on read-only runs.** Should `trigger_count`, `error_count` and the session tallies
   persist on every run? This note says no (C3 side effects): they are statistics and already
   behave that way for stateless flows. If they must survive a crash exactly, they belong beside the
   head, not in a rewrite of the whole table.
2. **The closure cache's bound.** A total of 100,000 units would hold every deployed flow on a
   server of today's size. Does an operator need a setting?
3. **`state_refs` after C3.** Keep writing it, which is stale between definition changes but lets a
   downgrade load, or drop it? Holon is on release candidates, so dropping it is simpler, and a
   downgrade would then start stateful deployments empty.
4. **Fsync on real disks.** Measure C3's per-write fsyncs on the hardware of the first outside job
   before deciding on group commit.
5. **Directory fsync after rename.** `write_atomically` (`holon-host/src/lib.rs:642`) and
   `write_private` (`sealing.rs:274`) fsync the file but not its directory, so on some filesystems a
   crash can lose an acknowledged rename. This note neither introduces that nor closes it.
6. **A strict log, per deployment.** Should a deployment be able to ask that an answered run's
   record survive a power loss too? That would close the state-ahead-of-log gap C2 (d) only bounds.
   The cost is one fsync of the log before every answer, about 150 µs per run here and more on a
   slow disk. A writing run could share the C3 head's moment under the cell lock, but a reading run
   would pay it alone. If yes, it belongs in the deploy settings' `keep` limits beside `keep_runs`,
   off by default.
7. **The sync interval, and the witness's queue.** Should `SYNC_EVERY` be 1 s and fixed, or a
   setting? And should `pending.jsonl` be synced on the same tick? That would cost one more fsync a
   second while heads are queued, and a power loss would then drop no queued heads.

## 9. Invariant check

- **I-a, the record before the answer.** Every run's entry reaches the OS before its answer.
  `serve_flow` logs at `listeners.rs:226` and answers after it, and C2 keeps that write synchronous,
  changing only the lock it holds.
- **I-b, the chain.** One deployment's entries are numbered and chained in one order, under one
  lock, as byte-exact lines. A C2 cut-back copies bytes, so a copy still checks. The file's place,
  name and format are unchanged (`server.md:165`).
- **I-c, an answered write survives a crash.** A state change is durable before its run answers:
  values and snapshot (fsync'd), then the head (fsync'd), under the cell lock (C3). A run that
  changes nothing has nothing to make durable.
- **I-d, a flow runs what its hash names.** Closures are checked against their hashes when loaded,
  cached by that hash, and only whole (C1).
- **I-e, redaction before anything is written.** The entry is redacted before it is serialized. The
  only change is that C2 redacts with no lock held, using the secrets cloned out under the brief
  table lock.
- **I-f, a witness's head is on disk (new).** A head is sent only after the line it names has been
  synced (C2 d). Today this doesn't hold (Appendix B.5).
- **A power loss loses at most about a second of records (new).** That is the bound C2 (d) sets.
  Today there is no bound.
- **M0 / §7a.** No disk I/O under the table lock on any run path: the log leaves it (C2), and the
  whole-table persist leaves the per-run path except for definition changes (C3). C2 (d)'s sync
  takes no lock while it fsyncs.

## Appendix A. How it was measured

- **Machine:** a 4-vCPU Linux container (6.18 kernel), ext4 on a virtio disk. `wrk` ran on the same
  machine over loopback, so absolute numbers are low and the ratios are what matter.
- **Holon:** `cargo build --release -p holon` at `aa2704a`, then `holon serve` with a fresh data
  folder, and the samples deployed with `holon deploy <file> --approve`. The 33-deployment runs add
  30 copies of `calc` under other names and ports.
- **.NET:** .NET SDK 8.0.133 from Ubuntu 24.04's archive, `dotnet new web`, with the two endpoints
  as minimal-API handlers on `WebApplication.CreateSlimBuilder`, logging cleared, built in Release.
- **Load:** `wrk -t2 -c64 -d6s --latency`, or `-t1 -c1` for one connection. `calc`'s body is
  `{"op":"mul","a":6,"b":7}`.
- **Attribution.** In a throwaway build, reverted afterwards:
  - timers in `log_run` (lock wait, redaction and packs, serialization, push) and in
    `keep_line`/`cut_back`, plus a timer around `materialize_closure`, each printed as an average
    every 5,000 runs;
  - `strace -c -f` on the server under load: 84% of syscall time in `futex`, about 20 `openat` per
    request;
  - the `HOLON_X=cache,nolog,nopersist` switches of §3. A first pass taken right after each restart
    read low, because the server was still loading; the table uses a 3 s settle.
- **Evaluator:** `cargo bench -p holon-core --bench native_vs_composite` gave
  `traversal/native_sum/4096` 5.2 µs, `traversal/fold_sum/4096` 280 µs and
  `wrapping_tax/composite_add` 0.64 µs.

## Appendix B. Found along the way

These are recorded here so they aren't lost. B.1 and B.5 are fixed by this note (C2); the others
are not.

1. **An action can report another run's entry.** `run_action` reads `next_seq` under one table
   lock (`deploy.rs:1118`) and logs in another (`deploy.rs:1119`, `log_run`). A run of the same
   deployment that logs in between makes the action report its entry (`seq`, used in
   `deploy.rs:1123–1125` and in "its run log says why (entry {seq})"). C2's `log_run` returns the
   `seq` it assigned.
2. **The witness queue opens a file per run under a server-wide lock.** With `--export-heads`,
   `Exporter::note` (`heads.rs:106–112`) holds `pending` while `append` (`heads.rs:114–120`) calls
   `create_dir_all`, `open` and `write`, for every run of every deployment. Unmeasured. The same
   remedy as C2 (b) applies.
3. **Log readers clone whole logs under the table lock.** `handle_get_changes` (`changes.rs:64`),
   `restore_plan` (`deploy.rs:1419`) and `state_change` (`deploy.rs:1526`) call
   `log.after(None, usize::MAX)`, which clones every kept entry (500 by default, each with its
   timeline), while every run of every deployment waits. C2 moves them to the log lock.
4. **A possible lock-order inversion.** A last attempt holds its cell (`firing.rs:270`) for the
   whole run, flow calls included (`calls.rs:86` → `run_action` → table, `deploy.rs:1055`).
   `put_state_back` holds the table (`deploy.rs:1614`) and then takes a cell (`deploy.rs:1617`),
   and `handle_lifecycle` does the same (`deploy.rs:1709`). A restore of deployment X while X's
   last attempt is calling another flow would deadlock. It hasn't been reproduced. The fix belongs in
   `put_state_back` and `handle_lifecycle`: clone the cell's `Arc` and release the table before
   locking the cell.
5. **The run and events logs are never fsynced, and heads reach the witness regardless.**
   `keep_line` in `recorder.rs:521–549` and `events.rs:150–170` never syncs, and heads are queued
   before their lines are written (`recorder.rs:509`, `events.rs:142`) and sent every 5 s. A power
   loss can lose recent lines while their state changes (fsynced, §2.4) survive, and while a witness
   holds their heads, which looks like a log cut short. C2 (d) bounds the first and closes the
   second.
