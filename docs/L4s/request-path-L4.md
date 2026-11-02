# Request path: a run pays for its own flow, not for the server (L4)

**Status:** proposed (2026-11-02). Nothing here is built yet. The numbers were measured on `main`
at `aa2704a`, with a release build. §2 and §3 report what was measured, and §5 says which parts
of the expected results are measured and which are estimated. Claims about the code are cited by
file and line at that commit.

A deployed flow answers a request in about **0.5–0.8 ms** when nothing else is running. Under
load, though, its throughput **stops at about 1,000–1,700 requests a second, whatever the number
of connections**. A stateful flow on a server with 33 deployments manages **53–65 a second**, with
a median answer near **one second**. The flow's own work is not the reason. Each run computes in
0.1–0.4 ms. The rest of the time goes to work the server does around every run:

1. It **reads the whole flow from disk again**, one file per unit, through a lock the whole store
   shares.
2. It **writes the run record while holding the lock on the table of all deployments**. Every
   500 runs it also re-reads, re-parses and rewrites the whole log file there.
3. For a stateful flow, it **rewrites and fsyncs the whole deployments file** (every deployment,
   with its document and kept versions) on every run, even one that only read its state.

This note makes the cost of a request depend only on its own flow and its own deployment:

> **Requests to different deployments never wait for each other, and no request does work that
> grows with the number of deployments or with the runs before it.** A run costs its evaluation,
> one appended log line, and (when it changed state) the writes that make that change durable.

The note does not change what a run records, the order in which a record and an answer happen,
what survives a crash, or the evaluator.

## 1. Where Holon stands

This section gives a reference point. It is not a target. Below, the same two endpoints run as
Holon samples and as a .NET 8 minimal API (Kestrel, `CreateSlimBuilder`, logging off), on the
same 4-vCPU machine, loaded with `wrk` over loopback (Appendix A):

| | .NET 8 | Holon | |
|---|---|---|---|
| `GET /hello/:name` (`hello-api`), 64 connections | 96,980 req/s, p50 0.53 ms | 1,670 req/s, p50 37 ms | 58× |
| `POST /calc` (`calc`, JSON), 64 connections | 80,580 req/s, p50 0.67 ms | 1,020 req/s, p50 61 ms | 79× |
| `GET /hello/:name`, 1 connection | p50 0.105 ms | p50 0.556 ms | 5× |

The comparison favours .NET. Every Holon run also checks the deployment's permit, redacts
secrets, and writes a hash-chained record that can be replayed node by node. That record has a
cost, and §5 estimates what is left once it is paid in the right place.

The more telling number is Holon's own: **1,480 req/s at 1 connection and 1,670 at 64**. More
concurrency buys almost nothing, so something on the request path runs one request at a time.

## 2. What a request does today

`serve_flow` (`crates/holon/src/listeners.rs:76`) answers each data-plane request on tokio's
blocking pool (`dataplane.rs:295`). In order, it does this:

| Step | Where | Lock held | Cost measured |
|---|---|---|---|
| Look up the deployment, clone its handles | `listeners.rs:84` | table (brief) | small |
| **Load the flow's closure from the store** | `materialize_closure`, `units.rs:159` | **store, once per unit** | 199 µs at 1 connection, **34 ms** at 64 (queueing on the store lock) |
| Evaluate the run (rounds, effects, commit) | `fire_with_state`, `firing.rs:120` | the state cell, at commit | 0.13 ms (`calc`) to 0.36 ms (`hello-api`) median, per the log's own `duration_ms` |
| Count the trigger | `record_trigger`, `deploy.rs:867` | table (brief) | small |
| **Redact, number, chain and append the run record** | `log_run`, `firing.rs:786` → `Log::push`, `recorder.rs:497` | **table, for all of it** | about **150 µs** held per run (below) |
| **Persist the deployments table** (stateful or parked runs only) | `persist_deployments`, `persist.rs:22` | a global `PERSIST` lock; the table lock while it snapshots every deployment | an fsync'd rewrite of the whole file (80 KB with 3 deployments, **1.1 MB with 33**) |
| Answer | `dataplane.rs:263` | none | |

### 2.1 The closure is loaded on every request

`materialize_closure` walks the flow's units from its root hash and fetches each one with
`store.get`. `SharedStore::get` (`main.rs:300`) takes the one mutex around the whole store.
`FileStore::get` (`holon-host/src/lib.rs:665`) reads the unit's file and checks it against its
hash. The `hello-api` sample has 19 units, so every request opens 19 files. Tracing one request
with `strace` shows exactly those 19 store files, plus the log.

The result can never change. A unit's hash is its content, so the closure of a hash is the same
every time it is loaded. The work is repeated for no gain, and because it runs under the store's
global lock it also serializes requests to every deployment.

### 2.2 The run record is written under the table lock

`log_run` locks the table of all deployments (`firing.rs:787`) and, while holding it:

- redacts the entry (it walks the whole entry, including every node's outputs in the timeline)
  and works out its channel packs;
- serializes it (4–6 KB of JSON for these samples), hashes the line and queues the head for
  the witness (`recorder.rs:497`);
- appends the line (`keep_line`, `recorder.rs:521`). Each append calls `create_dir_all`,
  `path.exists()`, `open` and `write`;
- **every `keep` runs (500 by default), cuts the file back** (`cut_back`, `recorder.rs:554`).
  This reads the whole file (up to 2 × `keep` lines), parses every line into a `LogEntry` to read
  its `at_ms`, writes the kept lines to a temp file, fsyncs it and renames it over the log.

The time spent inside the table lock, measured per run with `hello-api`:

| Part | Average |
|---|---|
| Redaction and packs | 6–11 µs |
| Serialization | 13–23 µs |
| Append (`create_dir_all`, `exists`, `open`, `write`) | 20 µs |
| **Cut-back**: one every 500 runs, each taking **31–36 ms**; spread over the runs, about | 66 µs |
| Total held per run | **about 150 µs** (130–190 µs) |

So with nothing else in the way, the table lock alone caps the whole server at about 6,000 runs
a second. Every cut-back also stops **every deployment's** requests for about 33 ms, which
explains much of the p99 tail.

This is a regression. `PLATFORM_LAYOUT.md` §7a recorded "disk I/O and per-deployment locks happen
under the global table lock, so one slow fire stalls every endpoint", and M0 fixed it ("no disk
I/O or per-deployment locks under the table lock ✅"). The state cell keeps the rule today:
`main.rs:420`, and JOURNAL 2026-07-02 ("real per-deployment state"), "Correction #2" ("the table lock is only ever held briefly, to
clone the `Arc` out"). The run log came later and puts its disk I/O back under that lock.

### 2.3 A stateful run rewrites every deployment

After any run of a deployment that has state, `serve_flow` calls `persist_deployments`
(`listeners.rs:228`). It does so even for a run that only read its state. The same per-run call
appears for clock runs (`scheduler.rs:197`), file arrivals (`arrivals.rs:412`), `/trigger`
(`deploy.rs:958`), actions (`deploy.rs:1121`) and resumed runs (`durable.rs:684`).
`persist_deployments` does the following:

- takes a process-wide `PERSIST` mutex, so every stateful run of every deployment goes through it
  one at a time;
- under the table lock, clones **every** deployment's record: its document, its kept versions,
  its grant, its sessions, and its parked runs (sealed, which takes each deployment's secrets
  lock);
- saves **every** deployment's state into the store. For each key this calls `put_value`, which
  at least stats the value's file, under the store lock;
- serializes the whole table as pretty JSON and writes it with `write_private`: a temp file,
  then `sync_all`, then a rename.

The cost grows with the size of the server, not of the run:

| `counter` sample, 64 connections | 3 deployments (80 KB file) | 33 deployments (1.1 MB file) |
|---|---|---|
| `GET /last` (reads state) | 479 req/s, p50 127 ms | **66 req/s, p50 936 ms** |
| `GET /hit` (writes state) | 396 req/s, p50 153 ms | 65 req/s, p50 933 ms |

A read-only stateful route is the plainest case. It changes nothing, yet it rewrites the server's
whole deployments file with an fsync before it answers.

### 2.4 Two bottlenecks in a row

The measurements below (§3) show something that shapes the build order. Removing either of the
first two costs **alone** helps little, because the other one then becomes the queue. Requests
pass through the store lock and then the table lock, one after the other, so throughput is set
by whichever is slower. Both have to go before the server scales with its cores.

## 3. Measured: what each cost is worth

A throwaway build (not committed) gained three switches in `serve_flow`, each of which removes one
cost:

- `cache`: keep each closure in memory, keyed by its root hash;
- `nolog`: skip `log_run`. This is an ablation to measure the log's share, **not** a proposal. The
  proposal keeps every record (§4.2);
- `nopersist`: skip the per-run `persist_deployments`.

All runs used the same server with 33 deployments, 64 connections, 6 s each, and a 3 s settle
after start (req/s, then p50):

| Switches | `hello-api` | `calc` | `counter` `/hit` (writes) | `counter` `/last` (reads) |
|---|---|---|---|---|
| none (today) | 1,575 · 39 ms | 1,014 · 60 ms | 65 · 933 ms | 53 · 1.00 s |
| `cache` | 3,820 · 12.6 ms | 3,076 · 15.2 ms | 67 | 61 |
| `nolog` | 1,778 · 35 ms | 1,077 · 57 ms | 63 | 53 |
| `nopersist` | 1,418 · 43 ms | 1,009 · 61 ms | 863 · 69 ms | 879 · 72 ms |
| `cache` + `nolog` | **21,044 · 2.8 ms** | **16,285 · 3.6 ms** | 65 | 52 |
| all three | 19,279 · 3.0 ms | 16,314 · 3.6 ms | **1,453 · 40 ms** | **14,691 · 4.0 ms** |

At 1 connection, `hello-api`'s median goes from 0.82 ms to 0.42 ms with `cache` + `nolog`.

What remains for `/hit` once all three are off is the commit itself. 64 connections write the same
key, so runs retry optimistically and then serialize on the cell (`firing.rs:167–300`). That
serialization is correct and stays. Each commit also writes the new value and a snapshot record
with an fsync each (`snapshot_after` → `save_snapshot`, `restore.rs:23`), while holding both the
state cell and the store's global lock. §4.4 deals with that.

## 4. The design

### 4.1 Closures by hash, in memory

Add `units::closure(store, hash) -> Result<Arc<MapResolver>, Closure>`, a cache in front of
`materialize_closure` **keyed by the root hash**:

- **No invalidation is needed.** A hash names exactly one closure, forever. A redeploy is a new
  hash and gets a new entry. A rollback to a kept version is a hash already seen. A parked run
  that resumes the version it started with (`durable.rs:637` resolves *the run's* unit, which may
  be older than the deployment's current one) finds that version by its hash too.
- **Only successes are cached.** A missing unit might be added later, and a closure too big or too
  deep fails again the same way, which costs nothing to recompute.
- **The lock covers lookup and insert, never the load.** Two concurrent misses on one hash may
  both load it. That is harmless, because both produce the same value.
- **Bounded by units, not entries.** One closure may hold up to `MAX_CLOSURE_UNITS` (20,000,
  `units.rs:187`). The cache keeps the most recently used closures up to a total unit count
  (§9 Q2), and evicts the least recently used.
- **Warmed at deploy and load.** Both already load the closure (`deploy.rs:388` builds the
  `Deployment` after loading it; `persist.rs:132` loads every deployment at start). They go
  through the cache instead, so the first request doesn't pay for it.

Integrity is unchanged (I-d, §10). Each unit is checked against its hash when it is read into the
cache, and the cached closure is exactly the units its hash names. A store file that changes on
disk afterwards is never read again. Today such a file would fail its hash check, and the request
would fail with a 500.

Callers to switch: the request and run paths first, `listeners.rs:95`, `scheduler.rs:125`,
`arrivals.rs:373`, `deploy.rs:924` (`/trigger`), `deploy.rs:1079` (actions) and `durable.rs:637`.
The others (`deploy.rs:162/507/749`, `firing.rs:761` `/fire`, `grants.rs:19`, `units.rs:256`,
`persist.rs:132`) can switch in the same step, since nothing in the result differs.

*Considered: a `closure` field on `Deployment`.* It ties the lifetime to the deployment, but it
must be kept in step on every redeploy and rollback, and it still misses resumed runs of an older
version. A cache keyed by what it holds can't go stale. That is the same argument the store
itself rests on.

### 4.2 The run log off the table lock

**(a) Each deployment's log gets its own lock.** `Deployment.log: Log` becomes
`Arc<Mutex<Log>>`, the same shape as the state cell (`main.rs:420`). `log_run` holds the table
lock only to clone what it needs (the log, secrets and channels `Arc`s, and the session number).
It redacts the entry and works out its packs with **no lock held**. Then it locks **that
deployment's** log to number, chain and append the entry.

- Numbering and chaining stay in one order per deployment: the order in which runs take the log's
  lock. Today that order comes from the table lock. Runs of one deployment still append one at a
  time, which the chain requires, and runs of different deployments no longer wait for each other.
- The entry's `prev` is part of the line it hashes, so serializing stays under the log's lock
  (13–23 µs). Splicing `prev` into a line serialized beforehand would save that, but isn't worth
  the cleverness yet.
- The readers lock the deployment's log instead of the table. They are about 25 sites in 12 files:
  `deploy.rs` (10), `permissions.rs` (3), `events.rs`, `firing.rs`, and one each in
  `changes.rs`, `collections.rs`, `folders.rs`, `grants.rs`, `main.rs`, `persist.rs`,
  `sweep.rs` and `verify.rs`. The change is mechanical. A reader that needs the table and the log
  together takes the table lock, clones the `Arc`, releases the table, then locks the log, never
  holding both.

**(b) Append through a handle kept open.** `Log` keeps its `File` open in append mode, creating
the directory once and the file once. `keep_line` becomes a single `write_all` of the line. Every
line is still written to the OS before the answer (I-a), with nothing buffered across runs. A
cut-back reopens the handle.

**(c) Cut back without parsing.** For each line the file holds, `Log` keeps its byte offset and its
`at_ms` (16 bytes per line, at most 2 × `keep` lines). `open_retaining` builds these from the read
it already does, and `keep_line` appends to them. `cut_back` then knows from memory which lines it
keeps. It copies the file's bytes from the first kept offset to a temp file, fsyncs it and renames
it over the log, as `write_private` does today, with no JSON parsing. The lines stay byte for byte
the same, so the chain still checks (I-b). It still runs once every `keep` runs, but it costs about
one 2–4 MB copy instead of parsing 500–1,000 entries, and it holds only that deployment's lock.

*Considered: segment files* (`<slug>.jsonl` plus `<slug>.1.jsonl`, with a cut-back being a rename).
That makes a cut-back O(1), but the manual documents the log as one file,
`deployments.logs/<slug>.jsonl` (`docs/manual/reference/server.md:165`), and an operator may tail
it. Keep the one file unless (c) still shows in a profile.

*Considered and rejected: writing the log in the background.* An answer could go out before its
run's record is written. Today the record is written first (`listeners.rs:226` comes before the
`answer` it returns), and "every request is a run in the log" (the manual's server reference) is a
promise worth keeping exactly (I-a).

### 4.3 Persist what changed, and only that deployment

**(a) Persist only when something durable changed.** The per-run `persist_deployments` call
(`listeners.rs:228` and the five other sites in §2.3) runs only when the run **committed a state or
collection write**, or **parked or resumed** a run. A run that only read state writes nothing.

What a read-only run no longer persists right away are its counters: `trigger_count`,
`error_count` and the session's tallies (`record_trigger`, `deploy.rs:867`). They are statistics.
They are persisted with the next write or deploy, which is already how a stateless deployment's
counters are persisted. A crash loses the counts since that point and nothing else (§9 Q1).

**(b) A state head per deployment.** Split the deployments file's per-run part out of it. Each
committed run already saves its state into the content store (`save_snapshot`, `restore.rs:23`:
each changed value, then a snapshot record) and logs the snapshot's hash as `state_after`.
Persisting the state therefore only needs a pointer per deployment:

- `deployments.state/<slug>` holds the deployment's current snapshot hash (hex). It is written
  with `write_private` (temp, fsync, rename) **after** the value and snapshot files and **before**
  the answer, under the state cell's lock, where `snapshot_after` already runs
  (`firing.rs:230`, `firing.rs:298`). Holding the cell lock keeps head writes in commit order.
  Because the values and the snapshot record are durable before the head points at them, a crash
  leaves the old head or the new one, never one pointing at something missing (I-c).
- `deployments.json` is rewritten only when a deployment's **definition** changes: a deploy,
  grant, lifecycle change or change to its parked runs, plus the counters' piggy-back (a). It
  stops carrying `state_refs` as the source of truth.
- **Load:** a deployment's state comes from its head file when there is one, and otherwise from
  `state_refs` (files written before this change). The first persist after an upgrade writes the
  head files. `deployments.json` keeps writing `state_refs` for one release, so a downgrade still
  finds state that is at most one definition change old. Whether that is worth it before 0.1.0 is
  §9 Q3.

Afterwards, a writing run costs the small fsync'd writes of its own state (changed values, the
snapshot record, the head), whatever else the server holds. A reading run costs no write at all.
The O(all deployments) snapshot under the table lock happens only when a definition changes.

### 4.4 The store lock (after §4.1–§4.3 are measured)

`SharedStore` is one `Mutex` around the whole backend (`main.rs:281`). Once §4.1 is in place, the
request path no longer reads units through it. It still writes state values through it, and
`FileStore::put_value` fsyncs the new file **while holding that lock** (`write_atomically`,
`holon-host/src/lib.rs:642`). A stateful write anywhere therefore stops every other deployment's
store access for the length of an fsync.

`FileStore`'s operations are already safe to run at the same time: names are content hashes, temp
names are unique per call, a rename replaces a file atomically, and two puts of one value write the
same bytes. The lock is only needed for `MemStore`. The proposal is to give `Store` interior
locking where a backend needs it (`MemStore`), and let `SharedStore` call a `FileStore` without
the outer lock. Before this step, `sweep.rs`'s sparing of recent puts (`sweep::Puts`) needs
reading, to be sure a sweep and an unlocked put can't race. That has to be checked, not assumed.

*Group commit* is deliberately not proposed yet. Per writing run there are three small fsyncs (the
value, the snapshot, the head), one after another per deployment. On this machine's disk that is
cheap. On a slower disk (0.5–2 ms per fsync is common) it would cap one deployment at a few
hundred writes a second, still several times today's 65. If that turns out to bind, a per-server
journal with one fsync per batch, acknowledging each run once its batch is down, is the known
answer. It has to keep "a run answers only once what it committed **and every write it read** is
durable", which a run reading another run's not-yet-durable commit makes subtle. It gets its own
note when the numbers ask for it (§9 Q4).

### 4.5 Later: the per-run constant

With §4.1–§4.2 in place, `hello-api` at 1 connection answers in about 0.42 ms, against 0.105 ms for
.NET. What makes up the rest is **not measured**. The candidates are the hop onto the blocking pool,
building the entry's timeline (every node's outputs converted to `SValue`), redacting and
serializing it, and hyper. Profile before changing anything.

The evaluator itself (an interpreted graph; the repo's bench puts a `fold` at about 68 ns an element,
where a native `sum` stays close to native speed) is a separate subject. See
`eval-inputs-by-ref-L4.md`. This note doesn't touch it.

## 5. What to expect

The first three columns of the "after" row are the ablation's measurements (§3). The note's own
mechanisms will differ from the ablation in two places:

- **§4.2 keeps the log.** The ablation skipped it entirely. The log's remaining cost, about 35–45 µs
  per run (serialize, hash, one `write`, and the cut-back's share once it no longer parses), moves
  under the deployment's own lock. That bounds **one** deployment at roughly 20,000+ runs a second,
  above where §3's numbers sit, and puts no bound across deployments. Expect `hello-api` and `calc`
  somewhat below the `cache` + `nolog` row. **Estimated, not measured.**
- **§4.3 keeps durability.** The ablation skipped persisting entirely. A writing run still makes its
  three small fsyncs. A reading run makes none, so `/last` should match the `all three` row and
  `/hit` should fall somewhat below it. **Estimated, not measured.**

| 33 deployments, 64 connections | today | after §4.1–§4.3 (expected) |
|---|---|---|
| `hello-api` | 1,575 req/s · 39 ms | ~15,000–20,000 req/s · ~3 ms |
| `calc` | 1,014 · 60 ms | ~12,000–16,000 · ~4 ms |
| `counter` `/last` | 53 · 1.0 s | ~12,000–15,000 · ~4 ms |
| `counter` `/hit` | 65 · 0.93 s | ~1,000–1,450 · ~40 ms (bounded by its fsyncs and one hot key) |
| any deployment, while another one's log cuts back | stalls ~33 ms | unaffected |
| any stateful request, as deployments are added | slows in proportion | unaffected |

Each step's commit reports what it actually measured (§7).

## 6. Tests

- **Closure cache (`units.rs`).** One hash gives the same `Arc` twice. A failure isn't cached (a
  unit missing, then put, then found). The bound evicts the least recently used. A redeploy answers
  with the new version on its first request (the existing redeploy tests cover this, now through
  the cache). A resumed run of an older version resolves that version.
- **Recorder (`recorder.rs`).** The existing chain and cut-back tests pass unchanged
  (`each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back`,
  `a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded`, and the others). New
  ones check that a cut-back keeps the kept lines byte for byte without parsing them (a line a
  crash cut short and a line with an unknown field both survive as today), that offsets are rebuilt
  at open, that the handle appends after a cut-back, and that the age-based cut still cuts.
- **Server (`holon/src/tests.rs`).**
  - **Independence:** while a test holds deployment A's log lock, deployment B still answers.
  - **No write on read:** a stateful run that only reads leaves `deployments.json` byte for byte as
    it was.
  - **Durability:** a write's state survives a restart through its head file.
  - **Migration:** an old `deployments.json` with `state_refs` and no head files loads with its state.
  - **Order:** a run's entry is in the log file when its answer arrives.
- **End to end and samples:** unchanged, and they must stay green (`editor/e2e`,
  `editor/src/samples.test.ts`).
- **A throughput check without external tools:** an `#[ignore]`d test that starts the server in
  process, deploys `hello-api`, `calc` and `counter` (plus 30 filler deployments) and fires requests
  from N threads, printing req/s and p50/p99. It is run by hand (`cargo test --release -- --ignored
  request_throughput`), not as a CI gate, since CI timing is too noisy to gate on.

## 7. Build order

One commit per step, each with its before and after numbers from the throughput check:

1. **The throughput check** (§6), with today's numbers in the commit.
2. **Closures by hash** (§4.1).
3. **The log behind its own lock, with the file handle kept open** (§4.2 a, b). Steps 2 and 3
   together are where the server starts to scale (§2.4).
4. **Cut back without parsing** (§4.2 c).
5. **Persist only on change** (§4.3 a).
6. **A state head per deployment, and the migration** (§4.3 b).
7. **The store lock** (§4.4), after re-measuring steps 1–6 and reading `sweep.rs`.

Steps 2–5 change no file format. Step 6 adds `deployments.state/` and needs a line in
`docs/manual/reference/server.md`'s list of what the data folder keeps.

## 8. Samples

This changes no feature, so per `CLAUDE.md` it adds no sample and changes none. The `hello-api`,
`calc` and `counter` samples are what the throughput check deploys and loads. Each step is run live
before its commit (deployed, then sent load), and the commit says so.

## 9. Open questions

1. **Counters on read-only runs.** Should `trigger_count`, `error_count` and the session's tallies
   persist on every run? This note says no (§4.3 a): they are statistics and already behave that way
   for stateless flows. If they must survive a crash exactly, they belong in the head file, not in a
   rewrite of the whole table.
2. **The closure cache's bound.** A total unit count (say 100,000) is enough to hold every deployed
   flow on a server of today's size. Does an operator need a setting for it?
3. **Keeping `state_refs` for a downgrade.** Holon is still on release candidates. Dropping it at
   once is simpler, and keeping it costs one release of carrying both.
4. **Fsync on real disks.** Measure §4.3's three fsyncs per writing run on the hardware the first
   outside job uses before deciding on group commit (§4.4).
5. **Directory fsync after rename.** `write_atomically` and `write_private` fsync the file but not
   its directory, so on some filesystems a crash can lose a rename that was already acknowledged.
   This note doesn't introduce that gap, and it doesn't close it either.

## 10. Invariant check

- **I-a, the record before the answer.** Every request's entry is appended (to the OS) before its
  answer is returned. §4.2 keeps the write synchronous and only changes which lock it holds.
- **I-b, the chain.** Entries of one deployment are numbered and chained in one order, under one
  lock, and their lines are byte-exact. A cut-back copies bytes instead of re-serializing, so a
  copy still checks. The file stays where the manual says it is, with the same format.
- **I-c, an answered write survives.** A state write is durable before its run answers: values and
  snapshot first, then the head, all fsync'd (§4.3 b). Nothing a run reads but doesn't write
  becomes less durable.
- **I-d, a flow runs what its hash names.** Closures are checked against their hashes when they are
  loaded into the cache, and the cache is keyed by that hash (§4.1).
- **I-e, redaction before anything is written.** Redaction still happens before the entry is
  serialized. It now runs outside the table lock, with the deployment's secrets cloned out under
  the brief table lock as today.
- **M0 / §7a.** No disk I/O under the table lock on the request path again: the log moves out
  (§4.2), and the whole-table persist leaves the per-run path (§4.3).

## Appendix A. How it was measured

- **Machine:** a 4-vCPU Linux container (6.18 kernel), ext4 on a virtio disk. `wrk` ran on the same
  machine over loopback, so absolute numbers are low and the ratios are what matter.
- **Holon:** `cargo build --release -p holon` at `aa2704a`, `holon serve` with a fresh data folder,
  the samples deployed with `holon deploy <file> --approve`. The 33-deployment runs add 30 copies
  of `calc` under other names and ports.
- **.NET:** .NET SDK 8.0.133 from Ubuntu 24.04's archive, `dotnet new web`, with the two endpoints
  as minimal-API handlers on `WebApplication.CreateSlimBuilder` with logging cleared, built in
  Release.
- **Load:** `wrk -t2 -c64 -d6s --latency` (`-t1 -c1` for one connection). `calc`'s body is
  `{"op":"mul","a":6,"b":7}`.
- **Attribution:** timers inside `log_run`, `keep_line`/`cut_back` and around `materialize_closure`,
  printed as averages every 5,000 runs; `strace -c -f` on the server under load (84% of syscall time
  in `futex`, 20 `openat` per request); and the `HOLON_X=cache,nolog,nopersist` switches of §3. All
  of them were in a throwaway build, reverted afterwards.
- **Evaluator:** `cargo bench -p holon-core --bench native_vs_composite` (`traversal/native_sum/4096`
  at 5.2 µs, `traversal/fold_sum/4096` at 280 µs, `wrapping_tax/composite_add` at 0.64 µs).
