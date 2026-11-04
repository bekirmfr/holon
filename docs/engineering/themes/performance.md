# Performance

What a request costs Holon, where the time goes, and why each part of the request path is the way
it is. The evidence is in three L4 notes: [request-path-L4](../../L4s/request-path-L4.md) (RP:
the measurements, the comparisons, the decisions Q1–Q16),
[request-path-plan](../../L4s/request-path-plan.md) (PL: the build steps and their numbers) and
[interpreter-path-L4](../../L4s/interpreter-path-L4.md) (IP: the evaluator's steps); the
[resilience audit](../../L4s/resilience-audit-L4.md) (RA) revised three of their choices.

## 1. What it is

Every request Holon answers runs a flow: a graph of content-addressed units evaluated by the core
(`holon-core`), with its effects performed by the host and its run recorded in a hash-chained log.
A web framework answers a request; Holon answers it **and keeps a record a third party can check**
(each run's inputs, every step's output, the state before and after, chained to the run before).
Most of a request's cost is that record, and most of the work here was making the record cheap
without making it less true.

## 2. How a request goes through

In `crates/holon/src` (IP §2 lists all 27 steps with lines):

1. **hyper** reads the request on a shared tokio runtime (`dataplane.rs`); the CORS origin is
   checked once and passed down (12.3).
2. **Here or on the pool** (`runs_here_for`, `dataplane.rs`): a run stays on the tokio worker that
   read it only when it can never wait: its handler is `unrecorded`, it reads only its request, the
   clock and random numbers (`READS_HERE`), and its flow is already in memory
   (`FlowCache::holds`). Everything else runs on the blocking pool (`spawn_blocking`, up to 512
   threads). See [Resilience](resilience.md#g2-a-deployment-cant-slow-another) for why the rule is this narrow.
3. **A snapshot of the deployment** under the table lock (taken about 5 times a request).
4. **The closure** (the flow's units) from the flow cache, keyed by the root's hash; the
   composites' shapes are kept beside them (`Shapes`). Values read come from the value cache.
5. **Firing** (`firing.rs`, `fire_with_state`): the nonce, run locks, the run id, sourced
   constants (made only when read), rounds of outbound calls.
6. **Evaluation** (`holon-core/src/eval.rs`): a walk of the graph with inputs lent, not copied;
   each node's kind decided once; a function given to `map`/`fold` run down a compiled `Plan` of
   opcodes.
7. **The state's commit**: values, snapshot and head written and synced under the cell's lock; a
   head only when the state changed.
8. **The entry**: redaction skipped when nothing could need it; the log line made outside the
   log's lock (`Made`); under the deployment's own lock (`LogCell`), the sequence number and the
   previous line's hash spliced in, hashed, appended to the open file.
9. **The sync**: a `strict` run waits for its line to be on disk (`sync_through`, shared by every
   run waiting at once); the syncer thread syncs every log each second (`SYNC_EVERY`) and makes
   the cut-backs a full log asks for.

### Three levels of record

A handler says how its runs are recorded (CHANGELOG, RP C5):

| Level | Guarantee | What it costs |
|---|---|---|
| `strict` (the default) | The record is on disk before the answer; a write or sync that fails is a 500. | The fsync: about 60% of a strict `hello` (259 of 436 µs at one connection, RP §1.3). Runs at once share one sync (4–5 per sync at 8 and 64 threads, PL 9a). |
| `lazy` | The record reaches the OS before the answer, the disk within a second. Only for a handler that reads and answers (every capability sourced, none waits). | Making the record; the pool's hop. |
| `unrecorded` | No entry, failed or not; runs are counted (runs, failures, time, the last failure's status and redacted error). Needs approval: `Unrecorded("GET /health")`. | Nothing but the run. |

Requests no route took are recorded lazily (Q12: strict would let anyone cause an fsync per
request). Strict promises *answered means recorded*, not *acted means recorded* (Q8).

## 3. Why it is this way

Each part, the cost it removed, and what it gained. "Check" is the in-process throughput test
(`request_throughput`, 33 deployments, 64 threads); "live" is `wrk` over HTTP. Commits and
numbers from PL, IP and RA.

### The host: from 1,600 to 10,000–80,000 requests a second

| Step | What | Why | Gain |
|---|---|---|---|
| C1 `6d062fe` | Closures cached by root hash, units shared, the cache bounded in bytes (`--flow-cache`, 256 MiB) | Each request read its 19 unit files under the store's global lock: 199 µs at one connection, 34 ms at 64 | check `hello` 2,287 → 6,734; live 1,575–1,670 → 4,987 |
| C2 a, b `8c28340` | A lock per deployment's log, its file kept open | Logging held the whole table's lock for about 150 µs a run: about 6,000 runs a second for the whole server | check `hello` and `calc` together 5,113 → 11,224 |
| C2 c `7c08198` | A cut-back copies the lines it keeps by byte range, from an index | A cut-back parsed the whole log: 31–36 ms every 500 runs | cut-back 14–25 → 4.4–6.0 ms |
| C2 d `e32aecc` | Lines synced every second, and before each witness send | No line was ever synced (a durability fix, not a speed-up) | none lost; about 5 syncs a second |
| C3 a `ff3f8bd` | The deployments file written only when a run changed state | A 1.1 MB file rewritten and synced on every stateful run | check `/last` 79 → 14,150 |
| C3 b–e `43ebafc` | A state head file per deployment | A write rewrote the whole deployments file | live `/hit` 74 → 986 |
| C2 e `b1259e3` | Strict by default, with a group commit | "Answered means recorded" (Q6) | check `hello` 13,583 → 9,531 strict, 14,464 lazy: strict costs its fsync |
| 9b `e40e7b5` | `unrecorded` | A health check needs no record | live 6,649 at one connection, 20,830 at 64 |
| 12.1 `c5e5556` | A short run answered on the worker that read it | The hop to the pool was 41 µs, 26% of an unrecorded request (RP §4.5) | 1 connection 6,555–6,850 → 8,520–8,945; later narrowed (R2) |
| 12.2 `cef16d2` | No timeline for an unrecorded run | The timer was 13 µs, 8% | check 26,521 → 37,531 at one thread |
| 12.4a `b17cbbb` | Hex from a table; a permit's name built once | `format!` per byte was 24–28% of a short run | profile `hello` 25.7 → 19.3 µs; live 64 connections 26,628–26,887 → 35,060–35,935 |
| 12.4b `0fdf913` | A value cache (64 MiB, values up to 1 MiB) | The store's lock and a file read per collection node: 15% | live `/notes` 6,976–7,250 → 10,489–10,936 |
| 12.4c `0e482b4` | The line made before the lock; an entry let go of freed after it | 10–14 µs of the 40 held under the lock was freeing | check lazy `hello` 14,015 → 20,652 |

### The evaluator: steps 1 to 6

Counted in instructions per run under callgrind ([Measuring](measuring.md#instructions)).

| Step | What | Why | Gain |
|---|---|---|---|
| 4.5 `4313f0a` | A function given to `map`/`fold` prepared once, not per item | Its graph was prepared again for every item | `crunch` 888 → 495 µs at 100 items, 45,944 → 23,765 at 5,000 (in µs, by `profile_one_route`, not instructions) |
| 1a `e916977` | Endpoint hash and level from the route; an edge index; `has_cycle` | Work the unit fixes done every run | `hello` −6.8% |
| 1b `440eca0` | Composites' shapes kept with the closure | Rebuilt every run | `hello` −8.3%; step 1 overall `hello` −14.6%, `crunch` −7.0% |
| 2 `6220b50` | The timer's size bound without JSON; redaction without a copy; the commit check skipped when the root hasn't moved | Work done twice around the record | `notes` −14.6% |
| 3 `ece0790` | Inputs lent to a node, not cloned | Deep copies of every input | `crunch` −55.7%, `hello` −34.2%; live `crunch` twice as fast at every size |
| 5, level 1 `7c31de1` | Each node's kind decided once; inputs gathered in one pass; the scratch reused | Dispatch and allocation per node | `crunch` −11.0% |
| 5, level 2 `abc4b11` | A compiled `Plan` for a `map`/`fold` function, primitives by opcode | Walking and dispatching by name per item | `crunch` −15.1% |
| 6 `f47c854` | Per-request bookkeeping: no entry made for an unrecorded run, one copy of the request, a kept hash buffer, sourced constants made only when read | Costs of the request around the run | `hello` −28.9%, `notes` −20.7% |

The plan is an optimisation the walk stands behind: whenever the plan meets a case it was not
built for (an input not yet evaluated, a guard whose condition is missing), it hands the run to
the walk, which answers as it always did. A test runs both on 3,000 random graphs and compares.

### What the safety work changed (RA §2.6–§2.10)

The audit's fixes took some of this back on purpose and gave some more:

- Only a run that can never wait answers on the worker (R2): routes that read state, a collection
  or a secret, and every `lazy` one, pay the pool's hop again: 65–145 µs at one connection, and a
  fifth of the throughput of an unrecorded route reading state (`counter /last` 48,859–50,448 →
  40,048–40,523).
- No run waits for its log to be cut back (R4): `notes` at 64 connections +39–46%, strict `hello`
  +10–18%, the `notes` listing's one-connection p99 130–157 → 29–30 ms.
- Before, a deployment busy reading could take an unrelated route from about 25,000 requests a
  second to 30–400 (R2); now 18,801–19,183 under reads and writes.

## 4. Reach and limits

### Where it stands

On a 4-vCPU Linux container (ext4 on a virtio disk), `wrk` on the same machine, 64 connections,
two rounds (IP §4.6, RA §2.6):

| Route | Requests a second | Against |
|---|---|---|
| `hello`, unrecorded | 76,169–81,403 (37–39 µs CPU) | plain .NET 8: 127,458–134,524 (1.75×) |
| `calc`, unrecorded | 71,066–77,051 | C#: 114,978–119,162 |
| `hello`, lazy | 19,473–19,601 | |
| `hello`, strict | 10,509–11,526 | C# keeping the same record: 9,843–10,268 |
| `notes` listing, lazy | 11,498–11,732; with a large data folder 2,518–2,526 | |
| `counter /hit` (a write) | about 700–1,400 | bound by its own sync |
| `crunch`, unrecorded | 16,937–17,027 / 2,276–2,357 / 469–472 at 100 / 1,000 / 5,000 items | C#: 3.2× / 11× / 14× faster |

Against the other graph runtimes (RP §1.5; Holon at `440eca0`, Node 24, `hello`):

- **Node-RED 5.0.9**: 7,052–7,239. Holon unrecorded answers 6–8 times as many at a third to a half
  of the CPU, and strict, keeping a record Node-RED doesn't, still answers more.
- **n8n 2.46.4**: 32 as shipped, 56–59 in its `legacy` mode with saving on; Holon lazy is 260–275
  times that. n8n spends 43% of its time in its database layer even with saving off; its own
  documentation cites about 220 executions a second.

Against a C# middleware keeping the same record (4.6–4.8 KB, SHA-256 chain, fdatasync group
commit), Holon strict reaches 77–96% of it; at one connection the two are alike (419–427 against
427–437 µs). The gap to plain .NET is the record, not the language.

### Limits

- **Strict is the disk's pace.** About 60% of a strict request is the fsync; any service keeping
  such a record pays it (IP §5). An fsync of an appended file costs about 0.6 ms on ext4, against
  113 µs for a small new file: the size change goes through the journal.
- **Writes sync three times** (value, snapshot, head) under the cell's lock. A group commit across
  writes (C4, Q4) is not done; the store's global lock is still taken on a miss and every put.
- **Compute-heavy flows** are 3–14 times slower than C#. What is left is allocating and freeing
  values (about 26%), copying fields (6.7%) and firing each node (about 20%). Shared values (step
  4) were deferred: copying is 5–10% after step 3, for a change touching about 850 places. Typed
  slots or native code (level 3) would reverse the core's dependency limit and the no-JIT choice.
- **A lazy route on a busy log** keeps a one-connection p99 of about 30 ms, likely the per-second
  sync holding up the next append (RA §2.6); left as it is.
- **The log's lock** still covers the append (about 17 µs) and the line's hash (about 12 µs): a
  strict run must never count a line not written (PL 12.4c).
- **The timeline is 63% of a line** and stays: the log holds what each step gave (Q16, below).
- **The throughput check leaves out hyper and the hop**; at 64 connections hyper and the client
  bound `/health`.
- **The witness export** costs 4–13%; its `pending.jsonl` is not synced (Q7).

### Tried and left

- **Answering on the worker by `block_in_place`** (12.1, R2): gained nothing, cost `/notes` 12%;
  the waiting is on the log's lock, not the store.
- **Dropping each step's output from recorded runs** (Q16, `49c12ea`): lazy `hello` 20,652 →
  28,814, the outputs recomputed matched for every run. Not done: the log would stop holding what
  each step gave.
- **Writing the log in the background** (RP C2): little gain for CPU-bound work, and it would lose
  "answered means recorded", read-your-writes and the packs' timing.
- **Retention per handler** (RP C5): would cut lines from the middle of the chain.
- **Syncing through a handle kept open** (`8e5eb6f`): no change measured.
- **Shared values in `Value`** (IP step 4): deferred, as above.
- **Composites evaluated by reference** (JOURNAL, 2026-06-30): every primitive about 24 ns slower
  for a 5–10% gain; reverted. Inputs by reference for primitives was kept (`native_sum` −91%).

## 5. What the tests hold

See [Testing](testing.md). For speed in particular:

- the golden records (`the_evaluator_gives_what_its_golden_record_holds`,
  `the_server_records_what_its_golden_record_holds`) hold every answer, record and hash, so a
  faster path that changes one fails;
- the plan against the walk on 3,000 random graphs, and the compute-heavy samples' functions all
  run down a plan (`the_compute_heavy_flows_functions_all_run_down_a_plan`: a change that makes
  them fall back fails, though the answers would still be right);
- the caches' sizes in memory against their budgets (`measure_cache_memory`, by hand);
- the throughput check and the profile (`request_throughput`, `profile_one_route`) are run by hand
  ([Measuring](measuring.md)); nothing in CI fails on a slower build.

## 6. History

The work ran from `feb2ef5` (the throughput check) to `f47c854` (per-request bookkeeping), then the
audit `dc21f76`–`6d764f5`. Decisions along the way (RP §8):

- **Q1** counters kept in memory, persisted with the next change: an fsync per read was the cost.
- **Q2** the flow cache bounded in bytes, a server setting: units vary 1,000 times in size.
- **Q6** strict by default: "answered means recorded"; measured at 0.6 ms, not the 150 µs guessed.
- **Q7** `SYNC_EVERY` one second, fixed for now.
- **Q9** `allow_unrecorded` on by default: the handler only reads and is approved one by one.
- **Q10** a failed unrecorded run recorded, then **reversed** (RA §2.8): never logged; counted.
- **Q12** requests no route took recorded lazily.
- **Q16** each step's output kept in the record.
