# What the throughput work assumes: an audit for resilience

The throughput work, from `aa2704a` to `f47c854` (45 commits: the request path's C1–C5 and build
step 12, then the interpreter path's steps 1–5 and the per-request bookkeeping), made Holon several
times faster without changing an answer, a record or a hash, as its tests and golden records hold.
What tests of today's behaviour can't hold is what the faster code assumes of code elsewhere, which
can stop being true without failing anything. This note lists every such assumption the work
added, how each fails if it stops holding, and what to do about it.

## 1. How it was done

- Every line the work added outside the tests (4,187 lines over 66 files) was scanned for new ways
  to stop: `unreachable!`, `panic!`, `expect`, `unwrap`; new state: thread-locals, `RefCell`,
  `OnceCell`, atomics, addresses used as keys; and new fixed sizes and budgets.
- Each optimization was then read for what it takes for granted: knowledge now kept in two places
  that must agree; invariants of other code it relies on; what it does when they don't hold.
- Each finding is one of: **a resilience cost** (the server behaves worse when something goes
  wrong), **a drift risk** (correct now, wrong silently after an unrelated change), or **sound**
  (an assumption that holds by construction, or that fails safe).

The fixes proposed are of three kinds: **one source of truth** (the knowledge in one place),
**fail safe** (when the assumption doesn't hold, do the slower thing that was always right), and
**a tripwire** (a test that fails the moment the assumption stops holding).

## 2. Findings

### 2.1 Resilience costs

| # | Where | What | Fix |
|---|---|---|---|
| R1 | dataplane.rs, the inline path (12.1) | A run on the worker that read its request has no panic guard. On the blocking pool a panicking run is answered `500 the request's handler failed`; inline, the panic unwinds through the connection's task: the caller's connection drops with no answer and the run leaves no log entry. | Fail safe: catch the unwind on the inline path and answer as the pool does. |
| R2 | dataplane.rs, `READS_HERE` | A route reading a collection runs inline, but a collection read goes through the content store: the value cache, then on a miss the on-disk store behind its global lock. A cold read, a slow disk or a contended store lock then blocks a tokio worker that every other connection on it shares. (State, the clock, random and secrets are read from memory: those are fine.) | Fail safe: take `CollectionRead` out of `READS_HERE`, so such routes take the hop (30–40 µs), as they did before 12.1. |
| R3 | eval.rs, `eval_as` and `run_planned` | Two new panics in core where the walk answered absent: `eval_as`'s arm for a primitive whose kind isn't a primitive's, and the plan's "a child's sources are evaluated before it". Both are unreachable by construction, but a bug there now aborts the run (R1 makes that worse inline) instead of failing closed. | Fail safe: the kind mismatch answers absent; the plan hands the run to the walk, as a failed check does. |

#### Measured (throwaway builds with an injected panic and the fixes, not committed)

- **R1.** A panic injected into a run: inline, the caller gets an empty reply (the connection
  dropped); on the pool, `500 the request's handler failed`. With the guard, both answer 500. The
  server survives either way (tokio keeps a panic to its task: after 8 in a row, more than its 4
  workers, the next 50 requests were all answered), and neither path logs an entry for a run that
  panicked. The guard costs nothing measurable: unrecorded `hello` 65,691–78,004 req/s without it,
  75,749–79,300 with it.
- **R2, and why.** An unrelated unrecorded `hello` deployment, probed at 2 connections, while
  `notes-api` takes a load at 64:

  | `hello`'s probe | alone | + `notes` listing reads | + reads and strict writes |
  |---|---|---|---|
  | as built | 24,759–24,886 req/s, p50 76–77 µs | **66–403 req/s, p50 4.4–23.5 ms** | **30 req/s, p50 60–61 ms** |
  | collection reads on the pool | 25,087–25,211, p50 75 µs | 10,347–10,847, p50 85–87 µs | 18,684–18,787, p50 81–82 µs |
  | inline, `block_in_place` on a store miss | 24,641–26,263, p50 72–77 µs | 68–69, p50 23 ms | 32–33, p50 55 ms |

  A listing run blocks inline, holding a worker with the CPU idle: on the store's lock while a write
  syncs (`put_value` syncs each new value under it), and on the log's lock while it is cut back
  (R4). `block_in_place` on store misses alone doesn't help: most of the waiting is on the log's
  lock. On the pool the waits block a pool thread, and every other route's runs go on.
- **Not R2: a CPU-bound load.** Under 64 connections of `crunch` (1,000 items), `hello`'s probe falls
  to 70–83 req/s at a p50 of 26–28 ms whether every route runs inline or every route on the pool:
  the 4 cores are saturated and the probe gets its share. That is overload, not the inline path.
- **What the pool costs each kind of route** (the same build, inline and every route pooled, two
  rounds): unrecorded `hello` 76,667–76,804 → 54,541–55,032 req/s (−29%; p50 at one connection 57
  → 117–118 µs), unrecorded `calc` −29%, lazy `hello` −8% (88–90 → 184–216 µs), `crunch` at 100
  items within noise, at 1,000 −20%. The `notes` listing, which waits as it runs, does better on the
  pool at 64 connections: 1,905–1,918 → 2,135–2,153 req/s (+11%), its p50 at one connection 665–714
  → 746–803 µs. So pooling every route would give back most of 12.1; pooling the routes that can
  wait costs them 60–90 µs at one connection and gains them throughput under load.

| # | Where | What | Fix |
|---|---|---|---|
| R4 | recorder.rs, `keep_line` → `cut_back` | **Found by measuring, not by reading.** Once a log's file holds twice what it keeps, the next run reads the whole file, writes the lines it keeps and syncs it, under the log's lock: about 25 MB in one write (84–92 ms) and several 20 ms syncs for `notes-api` here. Every run of the deployment, strict or lazy, waits 100–150 ms for it (a p99 of 134–151 ms at one connection, once a second at 1,100 req/s), and since 12.1 an inline lazy run waits holding a shared worker. The cut-back predates the throughput work; the inline path made it reach other deployments. | Cut back outside the log's lock: the kept lines written to a new file beside it, then, under the lock, the lines that came meanwhile appended and the file swapped in. |

### 2.2 Drift risks: knowledge in two places

| # | Where | What | Fix |
|---|---|---|---|
| D1 | primitives.rs, `Op::of` and `compute_op` | A primitive's name is listed in `Op::of`, its computation in `compute_op`'s arm. The match on `Op` is exhaustive, but a name left out of `Op::of` makes the primitive compute nothing, silently: it would read as a node that is always absent. Before, one `match name` held both. | One source: generate the enum and `Op::of` from one list of names and variants, with a tripwire that every genesis primitive `compute` evaluates has an opcode. |
| D2 | firing.rs, `log_or_count` and `push_run` | The rule for counting an unrecorded run that didn't fail (the level from the grant at log time, the tally's label, what failed means) is written twice. | One source: one function both call. |
| D3 | firing.rs, `redacted_line` | The shortcut holds because, with nothing to mark, redaction can only act on text holding `‹`, and an entry's records are already in key order (every one comes from `SValue::from_core` over a `BTreeMap`). A record built any other way, or redaction acting on another character, would make the shortcut keep a line redaction would have changed. | Tripwires: a property test that a redactor with nothing to mark leaves every text without `‹` as it is; a debug assertion, on in every test, that each record in an entry taking the shortcut is in key order. |
| D4 | recorder.rs, the timer's size bound | The bound's frames (64 bytes a value, 84 a `Sum`'s, 6 an escaped byte, 16 a field) mirror `SValue`'s JSON shape. If that shape grows (a longer tag, a new field), outputs past 16 KB could be kept. A new kind of value fails to compile (the match is exhaustive); a changed shape isn't caught for values the test doesn't try. | Tripwire: the existing test, over random values of every kind as well as the fixed ones. |
| D5 | eval.rs, `passes` | The plan takes a `guard` on a `const` `true` because that is how the language shares an input today. If the lowering changes, the plan isn't used (the walk is): correct, only slower, and silently so. | Tripwire: a test that the functions of the compute-heavy flow get a plan. |

### 2.3 Smaller costs

| # | Where | What | Fix |
|---|---|---|---|
| S1 | units.rs, the flow and value caches' budgets | Both count encoded bytes, not memory: a unit decoded takes several times its preimage, and a closure now also holds its composites' shapes. `--flow-cache 256M` holds more than 256 MB. | Measure the factor and say it where the setting is documented; count the shapes table in the closure's size. |
| S2 | dataplane.rs, `here` | The data plane's memo of which routes run inline is keyed by endpoint hash and never lets go: it grows by a few dozen bytes with every version a listener serves. | Keep only the current version's endpoints (clear it when the deployment's unit changes). |
| S3 | listeners.rs, `serve_flow` | `let [Some(request)] = inputs else { unreachable!(…) }` on an array made two lines above. | Make it irrefutable. |
| S4 | eval.rs and value_hash.rs, the reused scratch and buffer | Each falls back to a fresh one when already in use. The fallbacks are believed unreachable and aren't tested. | A test for each. |
| S5 | units.rs, `VALUE_CACHE_BYTES` | Fixed at 64 MiB, where the flow cache's budget is a server setting. | Make it one, as the flow cache's is. |

### 2.4 Sound

- **Shapes found by a unit's address** (step 1b): the table is built over the units a closure holds
  behind `Arc`s and never changed; an address it holds is that very unit's while the closure lives,
  and a unit from anywhere else can't be at it. Every other resolver keeps none.
- **A collection read held when its root didn't move** (step 2): a root is the hash of the
  collection's contents. The one behaviour change, in a failure mode only, is tested
  (`a_store_failing_at_the_commit_check_fails_only_a_run_whose_collection_moved`).
- **Whether a run changed its state, from its cells' hashes** (step 2): a snapshot is those hashes
  and nothing else, and a test holds the two answers equal over every pair of a set of states. If
  the snapshot ever holds more, the test fails.
- **The line made outside the log's lock** (C2): a line that doesn't start as the splice expects is
  made again under the lock.
- **The plan's hand-over to the walk** (step 5, level 2): tested, and the test fails without it.
- **Sourced constants made when asked** (bookkeeping): each value's pieces under keys of their own,
  which never leave the resolver; tested with two list-valued values in one run.
- **Thread-local buffers** (bookkeeping): bounded (the hashing buffer shrinks past 64 KiB; the
  line's room is the last line's, capped at 1 MiB), and holding no data between uses.
- **Unrecorded runs counted without an entry** (bookkeeping): an unrecorded handler is limited at
  deploy to reading (`may_record_lightly`), so running a failed one again with its timeline kept,
  and logging that, changes nothing outside it; the rule is D2's to keep in one place.
- **Unknown capabilities go to the blocking pool** (12.1): `READS_HERE` lists what may run inline;
  anything else, a new capability included, takes the hop.

### 2.5 Decided, and kept as decided

These weaken what is recorded by default, by the decisions taken in request-path-L4.md §8, and are
listed so they are seen as policy, not as optimizations:
- `allow_unrecorded` is on by default (Q9): a handler may be declared `unrecorded` and approved.
- A `lazy` run's record reaches the disk within a second (`SYNC_EVERY`), not before its answer.
- A request no route took is logged lazily (Q12).
- A failed unrecorded run is run again to be logged: the run logged is the second, which may read
  a newer state than the first did.

## 2.6 Fixed: R1–R4, and what it cost

- **R1:** a run on the inline path that panics is answered 500, as on the pool
  (`catch_unwind`); a test asks a run to panic, inline and pooled, and checks both are answered
  500 and the next requests answered.
- **R2:** a run goes inline only if it can never wait: its route is unrecorded (no log line, no
  log lock), it reads only its request, the clock and random numbers (`READS_HERE`: no state, no
  collection, no secret, each behind a lock a writer holds through a sync), and its flow is in the
  flow cache already (`FlowCache::holds`: nothing loaded from the store on the worker). The memo of
  which routes run inline holds only the live version's endpoints (S2).
- **R3:** where the evaluator's new paths stopped the run, they fail closed: a node whose kind
  isn't its body's computes nothing; a plan that finds an input not yet evaluated, or a
  pass-through whose condition or value isn't in, hands the run to the walk. A test turns 1,000
  plans' order around and checks the outputs are the walk's; writing it found the pass-through's
  own step answering absent where it should have handed over, fixed with it.
- **R4:** a run that finds its log's file past what it keeps only says so; the periodic sync
  (`sync_logs`, every second) makes the cut: the kept lines written beside the file and synced
  with the lock released, what runs wrote meanwhile caught up the same way while it is more than
  64 KiB, and only the last of it copied, synced and swapped in under the lock, then the directory
  synced. A cut the file moved under (cut holding the lock, the log told to keep more or fewer,
  the log deleted) is dropped and made as it always was, or not at all for a deleted log. Tests:
  the file a cut made so is byte for byte the file a cut made holding the lock leaves; 8 threads
  writing 2,000 entries while cuts are made leave every one in order, chained; a cut overtaken by
  the file changing is made afresh; a deleted log stays deleted. Two designs measured before this
  one did worse: the run that found the file full making the cut itself (no run waited on its
  lock, but that run did: p99 at one connection 151–163 ms), and the plan taken when the file was
  found full but made up to a second later (everything written meanwhile copied under the lock:
  371–408 ms).
- **S3:** the request is kept as the `Option` it is, not unpacked with `unreachable!`.

Measured, the build before against the build with them (`wrk`, two rounds alternating, the
`notes-api`, `hello` and `counter` deployments of request-path-L4.md §1 and `crunch`):

| | before | after |
|---|---|---|
| an unrelated unrecorded `hello`, probed at 2 connections, under `notes` listing reads | 63–64 req/s, p50 25 ms | 3,429–4,189 req/s, p50 252–344 µs |
| the same, under listing reads and strict writes | 27–29 req/s, p50 63–68 ms | 18,801–19,183 req/s, p50 80–81 µs |
| the `notes` listing at one connection | 1,052–1,073 req/s, p50 723–734 µs, **p99 130–157 ms** | 1,181–1,200 req/s, p50 753–758 µs, **p99 29–30 ms** |
| the `notes` listing at 64 connections (pooled now) | 1,720–1,821 req/s | 2,518–2,526 (+39–46%) |
| strict `hello` | 9,510–9,786 | 10,509–11,526 (+10–18%: no run waits behind a cut-back) |
| unrecorded `hello`, `calc`; `crunch` at 100 items (inline still) | 74,465–78,269; 69,157–75,126; 13,450–13,790 | 76,169–81,403; 71,066–77,051; 14,415–15,240 (unchanged) |
| lazy `hello` (pooled now) | 18,598–18,818 req/s, p50 at one connection 93–96 µs | 19,473–19,601, 228–241 µs |
| unrecorded `counter /last`, reading state (pooled now) | 48,859–50,448, 62 µs | 40,048–40,523 (−18–20%), 128–135 µs |

So the price is the pool's hop on the routes that may wait: 65–145 µs more at one connection, and
a fifth of the throughput of an unrecorded route that reads state, where it is the run's largest
cost. Under load they do as well or better; and no deployment's waiting reaches another's routes.
A one-connection p99 of about 30 ms remains on a lazy route with a busy log, likely the
once-a-second sync of its file holding up the next append to it; left as it is.

## 2.7 Fixed: D1–D3

- **D1:** one list (`ops!`, primitives.rs) makes both the `Op` enum and `Op::of`, each primitive's
  name beside its variant, so a name can't be in one and not the other; `compute_op`'s match on
  `Op` is exhaustive, so a variant can't be listed without an arm. A test holds every genesis
  primitive to being computed somewhere (an opcode, a request the host answers, `map` or `fold`,
  or a source the host stands a constant in for) and every opcode to being a genesis primitive's.
- **D2:** one function, `count_if_unrecorded`, decides the level a run is logged at and counts an
  unrecorded one; `push_run` and `log_or_count` both call it.
- **D3:** in a debug build, which every test runs, each line the redaction shortcut makes is
  checked against redacting first, the line and the entry the same: every run the server's tests
  log checks the shortcut's assumptions on a real entry (with the look-alike check taken out, two
  tests fail on it). And a test holds a redactor with nothing to mark to leaving 5,000 texts of
  the awkward kinds (quotes, backslashes, `›`, `secret:`, `token:`, multi-byte characters) as they
  are.

Their cost, counted (callgrind, the difference between two run counts, the build before them
against the build with them): `hello` +0.04%, `notes` −0.02%, `crunch` +0.6% (the opcode table is
used once a composite and the check is debug-only: the compiler's inlining moved). R3's checks of a
pass-through's condition and value on every item cost `crunch` 1.4% before them (22.24M → 22.56M).

## 2.8 Changed: an unrecorded run is never logged, failed or not

The audit's list kept the request-path note's decision Q10 (an unrecorded handler's failures are
recorded, each failed run run again with its timeline kept and logged). Weighed again for
resilience, it went the other way:
- **A declaration is a reason.** A handler may be declared unrecorded to keep its requests out of
  the log (redaction takes out secrets, not personal data); a failed run's entry kept them.
- **An amplifier.** A failure can be forced from outside (a malformed body, a bad parameter), and
  each one cost a second run and a log line of several kilobytes, with its share of cut-backs: a
  flood of bad requests to a route declared cheap made it dear.
- **Surprise,** and an answer that was the second run's, which may have read a newer state.

Now an unrecorded run is counted and never logged, failed or not, and never run again. Its tally
keeps the last failure: when, its status (at least 500 for an error), and its error, redacted of
the deployment's secrets and cut to 300 characters, never its request. The views say it ("2
failed, the last 503: why"); the approval says "keep runs of GET /health out of its log, failed or
not (they are counted)"; to see why one fails, its handler is made `lazy` for a while. A failed
unrecorded action answers that its runs aren't logged.

Found while running it: tallies were written to disk only when the deployments file was written
for something else (a deploy, a parked run), so a restart lost the counts since. With failures in
the tally alone, that lost them too: now a count marks the deployment (`tallies_changed`) and the
logs' periodic sync writes the deployments file when one is marked, so a restart keeps them, the
last second's at most lost, as a lazy record's are.

Tests: an unrecorded run leaves no entry, failed or not, and is counted, the last failure's time and
status kept; a failure's error is kept redacted of a secret and cut short, an error counting as a
500; an unrecorded action is counted and never logged. Run live: `status-board` and a route
answering 503 deployed, requested, the server restarted, and `holon history` says "unrecorded GET
/down 3 runs counted, not logged, 3 failed, the last 503".

## 2.9 Fixed: D4, D5, S1, S4, S5

- **D4:** the timer's size bound is held over 20,000 random values of every kind, nested, from tiny
  to past the limit (awkward texts and keys, numbers of every size and none): never below the
  JSON, and the timer keeps exactly what writing the JSON would have kept, on both sides.
- **D5:** a test compiles the compute-heavy flow and holds every function its `map`s and `fold`s
  apply to running down a plan (`holon_core::would_plan`, for tests); with the pass-through taken
  out of the plan, it fails.
- **S4:** a run whose scratch is taken runs in one of its own, the outputs the same, walked and
  planned; a hash taken while the thread's buffer is in use is the same hash.
- **S5:** the value cache's size is a server setting, `--value-cache SIZE` (or `HOLON_VALUE_CACHE`),
  as the flow cache's is; set, it lets go of the least used down to it.
- **S1, measured.** A test run by hand (`measure_cache_memory`) loads every sample's flow through
  the flow cache and keeps 20,000 values of the shapes runs read (notes, lists of numbers, long
  text), and compares the allocator's bytes in use (`mallinfo2`) with what each cache counts.
  Counted by encoding, the value cache held **2.8×** its budget and the flow cache 1.2× with the js
  engine's module among its units but **7.8×** for flows without one: a server of ordinary flows
  with the default 256 MiB would have held about 2 GB. Both now count what they hold in memory: a
  value's footprint (its own size, its texts, a record's tree nodes, which hold 11 entries whatever
  they keep, the cache's own map); a unit's (its fields' heap, each short text the allocator's
  smallest chunk, its `Arc`, the map's entry), a closure's own tree of units and its shapes table,
  rounded up a quarter for what the estimate can't see, so a cache keeps a little less than its
  budget, never more. Measured again: the value cache **1.0×**, the flow cache **0.8–1.0×**.

## 2.10 The soak, and what it found

A mixed load on one server for 25 minutes (`9969f8b`, on a copy of the measurements' data folder:
thousands of notes, logs of up to 60 MB): unrecorded, lazy and strict `hello` and the `notes`
listing at 8 connections each, `crunch` and unrecorded `calc` at 4, strict `notes` writes of random
ids at 1, an unrecorded route that always fails at 2. Sampled every 10 seconds, a one-connection
probe of unrecorded `hello` every minute.

- **Served:** 23.8 million requests, none failed but the failing route's (all 503, as it says), no
  socket error, nothing in the server's log. Throughput per route steady to the end.
- **Steady:** threads (76 at most: the blocking pool's), open files (72–75), the logs' folder
  (160–217 MB, cut back as it fills), the deployments file (422 KB, written every second by the
  changing tallies). The probe held its p50 at 1.41–1.76 ms and p99 at 5.3–8.2 ms throughout.
- **Memory** rose from 311 MB to 660 MB: 530 MB in the first 4 minutes, then flat, but for two steps
  of about 45 MB, at 10 and 21 minutes. A second run with the allocator's adaptive behaviour pinned
  (`MALLOC_MMAP_THRESHOLD_=131072`, `MALLOC_ARENA_MAX=2`, 15 minutes) stayed flat at 453 MB for its
  first 10 minutes: the early rise is the allocator's, not a leak. It stepped up 44 MB at 10
  minutes as the first had: R5.

| # | Where | What | Fix |
|---|---|---|---|
| R5 | folders.rs, `sweep` (every `SWEEP_EVERY`, 10 minutes) | **Found by the soak.** The sweep of folders' kept file texts reads every deployment's whole log (`Log::lines`, which reads the file, under the log's lock: up to 60 MB here, every run of the deployment waiting) and parses every entry in full, every 10 minutes, whether or not any deployment has a folder (none here did). About 200 MB of lines parsed each time; what it frees the allocator keeps: the 45 MB steps. | Skip it when no folder keeps a file's text; read each log outside its lock (as a cut-back does); parse only the lines that name a kept text. **Done** (`42a75f9`, §2.11). |
| R6 | the content store (holon-host's `FileStore`), `sweep.rs` | **Found by the soak; predates the throughput work.** Each state or collection write puts its changed values and a snapshot as files of their own, synced: 77,220 note writes added 594,619 files and 2.35 GB (7.7 files, about 30 KB of disk each: files of 1.8 KB on average, each a 4 KB block). Values nothing names any more are removed only by `holon store sweep` (or `POST /admin/store/sweep`), an admin's: at this soak's 51 writes a second the disk fills by about 5.4 GB an hour until someone sweeps. | The store swept on its own on a schedule (what a sweep spares, a value put in the last hour, is already safe for it): **done** (`6d764f5`, paced since, §2.11). Fewer, larger files per write (packed small values): to decide. |

## 2.11 Fixed: R5 and R6, and what the second soak found

- **R5** (`42a75f9`): the folders' sweep is skipped when the blob store holds nothing put before
  what it spares (`Blobs::any_put_before`); otherwise it reads each log's file a line at a time
  with the lock released (`LogCell::each_line`: a file is only ever replaced by a rename, so what
  is read is one file's lines, in order) and parses only the lines holding `"blob":`. The store's
  sweep reads the logs the same way, scanning each line for hashes.
- **R6** (`6d764f5`): the server sweeps its store every `sweep::EVERY` (10 minutes), sparing what
  was put in the last hour as an admin's sweep does, and skips a sweep when nothing was put since
  one that spared nothing (`sweep::Scheduled`); the two take turns (`Puts::sweeping`).

**The second soak** (22 minutes at `6d764f5`, set up as the first, on a fresh copy of the data
folder with its files' times kept): 19.2 million requests, none failing but the failing route's,
nothing in the server's log; threads 72, open files 72–75, the logs' folder 164–224 MB, the probe's
p50 1.48–1.86 ms and p99 5.2–8.2 ms throughout. No memory step at 10 minutes from the folders'
sweep (R5): none of the copy's deployments keeps file texts, and the sweep was skipped. The
store's sweep ran twice: at 10 minutes it removed the copy's old values, **137,962 in 50.5 s**; at
20, 271 in 8.3 s (everything newer spared). The data folder ended at 1,899 MB and 432,808 files,
against the first soak's 3,105 MB and 734,795 after 25 minutes.

**It found the sweep's own cost.** Memory stepped up about 110 MB as the first sweep ran (607 →
717 MB) and stayed (the allocator's, as before). And the `notes` routes ran 17% below the first
soak (listing 347 → 287 requests a second, writes 51 → 43, their p99 doubled), the other routes
3–8% (this machine's noise). Measured on its own, an admin's sweep on a fresh copy, the `notes`
listing at 8 connections with one writer, two rounds:

| While | Requests a second (before → during → after) |
|---|---|
| no sweep (the same wait instead) | 419 → 466 → 452; 362 → 373 → 375 |
| a dry run (reads everything kept, removes nothing) | 352 → 401 → 362; 378 → 392 → 354 |
| a sweep removing 134,000 values (27 s) | 384 → **123** → 352; 375 → **125** → 356 |
| the same, paced (61 s) | 381 → 337 → 346; 387 → 356 → 345 |

Reading and checking cost nothing; removing does: each removal holds the store's lock, back to
back, and ext4 journals each unlink. Read past the value cache, and listing the store by hash
only, measured the same (two rounds each), and were left out. **Fixed:** the server's own sweep
pauses 100 ms after every 256 values it removes (`PACE`, `PAUSE`), about 2,500 a second at most:
the listing loses 7–12% while it runs instead of 67%, and a store writing 50 states a second
leaves about 400 values a second to sweep, so it still keeps up six times over. An admin's sweep
is not paced: asked for now, it removes as fast as it can (paced, a large one would outlast the
command's 60-second wait), and holds up the server's runs while it does.

**Still bounded, not small:** a value is swept 60 to 70 minutes after it was put; at 51 writes a
second that is about 5.5–6.4 GB (1.4–1.65 million files) of unnamed values while writes go on.
Fewer, larger files per write would lower it.

## 2.12 Found by the host's view: R8

| # | Where | What | Fix |
|---|---|---|---|
| R8 | recorder.rs, `Log::entries` | **Found by storage step 3** (storage-L4.md). A log keeps every run it keeps in memory, whole, as parsed entries, beside its file: 1.1 to 3.6 times the lines they were written as (`measure_log_memory`, on the soaks' logs). The soaks' `notes` listing holds about 86 MB, `crunch` about 52 MB, most of the server's memory beyond its caches; and nothing bounds it: no cache's size counts it, and a deployment approved to keep 20,000 runs (the ceiling) of a 50 KB line would hold about 3.6 GB. | A log on file keeps only its newest few runs in memory (what the runs list shows first) and reads older ones from its file by the index it keeps already (`lines`), as a cut-back does. **Done** (below). |

**Fixed.** A log on file keeps its newest `IN_MEMORY` (32) runs in memory, whole, and of the others
it keeps only their seqs and times (`kept`, 16 bytes a run); a read of older runs is planned under
the log's lock (what is in memory, cloned; where the others are in the file, the file opened) and
read and parsed with it released (`Wanted::read`): a file is only ever replaced by a rename, so the
one opened holds those lines whatever is written or cut meanwhile. A log in memory only keeps them
all, as before. The readers that held the deployments table's lock while reading every run (the
cell history, a restore's dry run, a collection's old root) now read them with it let go. Opening
a log parses its file a line at a time instead of all at once.

Measured on copies of the soaks' data folder, the old build and the new in alternating rounds:

| | Before | After |
|---|---|---|
| The server's memory once started | 319–320 MB | **39 MB** |
| After 18 s of load (lazy and strict `hello`, the `notes` listing at 64 connections) | 482–505 MB | **148–155 MB** |
| Lazy `hello` | 14,393–16,426 | 16,453–17,958 |
| The `notes` listing | 1,258–1,934 | 2,368–2,390 |
| Strict `hello` | 5,453–6,073 | 6,766–6,855 |
| A page of the newest 200 runs, `notes` (50 KB lines) / `crunch` (95 KB) | 527–574 / 117–131 ms | 468–471 / 122–126 ms |
| The newest 32, `crunch`, a warm server | 18–46 ms | 21–26 ms |

Throughput rose: each run let go of the entry it pushed out as before, but the server no longer
moves hundreds of MB of entries through the allocator. Tests: a log on file gives what a log in
memory gives, run for run, as it cuts back, opens again and keeps more; a read planned before a
cut-back reads what it planned; reads while four threads log and cut back are whole and in order;
`shows` finds a state only the file holds; the end-to-end suite (72 passed).

## 3. Order

1. **R1–R4**: done (§2.6).
2. **D1–D3**: done (§2.7).
3. **D4, D5, S1–S5**: done (§2.6, §2.9).
4. Then the rest of the check:
   - **the end-to-end suite** (`editor/e2e`, the real editor and server, every sample deployed):
     run at `9969f8b`, **72 passed**, 1 skipped (`atlas-notes`, which runs only against a MongoDB
     replica set named by `HOLON_TEST_MONGODB`), in 7.1 minutes;
   - a soak under load watching memory: run (§2.10), found R5 and R6, fixed, run again (§2.11);
   - a review of `aa2704a..HEAD` by a reader given the code, not this note.
