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
4. **The closure** (the flow's units) from the flow cache, keyed by the root's hash; each
   composite's shape and its children, resolved, are kept beside them (`Shapes::kept`). Values read
   come from the value cache.
5. **Firing** (`firing.rs`, `fire_with_state`): the nonce, run locks, the run id, the state it
   starts from (a copy of the keys, the values shared), its sourced values lent to the leaves that
   read them, rounds of outbound calls.
6. **Evaluation** (`holon-core/src/eval.rs`): a walk of the graph with inputs lent, not copied,
   and a value given to the node that builds on it once nothing else will read it; each node's
   kind decided once, kept with the closure; a function given to `map`/`fold` run down a compiled
   `Plan` of opcodes.
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

### The evaluator: steps 1 to 6, and after

Counted in instructions per run under callgrind ([Measuring](measuring.md#instructions)), but for
the rows that say otherwise.

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
| #43 `e988258` | Giving and lending: a value moved to the node that builds on it once nothing else reads it, a function's composite children prepared once | A `fold` copied what it grew at every step, O(n²) | through release servers, a filter by fold ×5.0 at 1,000 items, ×21 at 5,000; `crunch` ×0.86, from its small records, not its numbers ([below](#small-values-are-copied-2026-11-07)) |
| `5f29d1d` | Each composite's children resolved once and kept with the closure, with their kinds | Every call resolved each child by hash and decided its kind | in-process, one call of a 25-node function 1.61 → 0.93 µs, `hello` ×0.86, `calc` ×0.82 ([leaves §10](js-library-races/leaves.md#10-a-composites-preparation-kept-81)) |
| small values | A number, a boolean, a text up to 32 bytes, and a list or record of up to 8 of those copied, not given (`heavy`); arithmetic's finite check on the `f64` in place | #43 gave every record: about 1,770 instructions a step, where copying `{ v, dev, sq }` costs less; the check was a call a node | `crunch` 25.76 → 23.37 M at 1,000 items (×0.907), its records part ×0.873; a filter by fold +1.0% ([below](#small-values-are-copied-2026-11-07)); through a server, `crunch` ×1.06 to ×1.09 the build before #43 ([below](#through-a-server-again-after-the-kept-children-and-small-values-490d03e-2026-11-07)) |
| `befee65` | The ternary a guard on each side: `c ? a : b` answers `a`, absent or not, when `c` holds, as in JavaScript | A guard and a fallback: a true side that was absent fell through to the false one | three nodes more a ternary; through a server a filter by fold ×0.85 the build before it, still ×18 the one before #43 ([below](#through-a-server-again-after-the-kept-children-and-small-values-490d03e-2026-11-07)) |
| values as they are | A leaf that reads state, the clock, a random number, a secret or the file that started the run asks the resolver for its value (`Resolver::sourced`), which it produces as it is; every composite keeps its children whatever the run reads | The host built each value as constant units in the run, and let no composite with a capability leaf keep its children, the clock being sourced in every run | a field of a 20-field state record ×0.25, the length of a 100-record state list ×0.16, 80 operations from a state number ×0.61; `notes` −0.8% ([below](#a-runs-values-handed-as-they-are-2026-11-07)) |
| `48eb0fe` | A state cell's value held in an `Arc`; a run lent the values it was granted; its entry made from the state it started from | The host copied the deployment's whole state four times a run, read or not | the length of a 100-record state list ×0.26, a field of a 20-field state record ×0.73; `notes` −0.6% ([below](#a-runs-state-shared-2026-11-07)) |
| `241b594` | A state holds data only: a write and a load refuse an effect, so a run is lent its values unchecked | Each run checked each state value it read was data, though a write refused an effect already | the length of a 100-record state list ×0.95 (−21.8 K); the rest within 0.4% ([below](#a-state-holds-data-only-2026-11-07)) |

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

### The evaluator's change through a server (#43, 2026-11-06)

The evaluator's audit ([leaves](js-library-races/leaves.md)) measured #43 through `eval` and an
in-process bench. Raced through release servers on the machine above: `f4f269f` (before #43),
`e988258` (after it) and `main` at `78f8f81` (after the TypeScript switch, #40, and #44). One flow,
written in `.holon` for the first two and `.holon.ts` for `main`, compiles to the same unit in all
three (`70a3d6c0…`), every route `unrecorded`, so what is raced is the evaluator and the host:

```ts
route.get("/hello", () => "ok", { log: "unrecorded" })
route.post("/calc", …)                          // the calc sample's switch
route.post("/crunch", …)                        // golden.rs's crunch, its rows not answered
route.post("/filter", (req) => {
  const kept = req.json.xs.reduce((acc, x) => x > 10 ? [...acc, x] : acc, [])
  return length(kept)
}, { log: "unrecorded" })
```

`holon load` (one client binary, `main`'s) at 16 connections, 5 s a workload, three rounds, each
build started fresh in a turning order; `xs` is `i % 20` for `i` below n. Medians of the rounds,
requests a second, with the server's CPU a request from `/proc/<pid>/stat`; no errors:

| Workload | before | after | main | after ÷ before | main ÷ before | CPU a request (before / after / main) |
|---|---:|---:|---:|---:|---:|---|
| `hello` | 59,632 | 58,422 | 58,993 | 0.98 | 0.99 | 46 / 48 / 47 µs |
| `calc` | 49,106 | 47,271 | 49,436 | 0.96 | 1.01 | 59 / 62 / 59 µs |
| `crunch`, 100 | 12,536 | 10,840 | 11,517 | 0.86 | 0.92 | 294 / 340 / 321 µs |
| `crunch`, 1,000 | 1,718 | 1,493 | 1,604 | 0.87 | 0.93 | 2.28 / 2.62 / 2.44 ms |
| `crunch`, 5,000 | 361 | 306 | 326 | 0.85 | 0.90 | 11.0 / 13.0 / 12.2 ms |
| `filter`, 1,000 | 1,045 | 5,225 | 5,070 | 5.00 | 4.85 | 3.75 / 0.74 / 0.76 ms |
| `filter`, 5,000 | 53 | 1,132 | 1,131 | 21.3 | 21.3 | 78.3 / 3.46 / 3.47 ms |

A fold that builds a list is linear now, where it was quadratic: its accumulator is given to each
step, not copied (the audit's "giving"). `crunch` is about 14% slower after #43 at every size, the
rounds' ranges apart (12,383–12,800 before, 10,768–10,907 after, at 100). `main`'s ×0.92 and
`calc`'s recovery were the rounds' spread: counted in instructions, `main` is 0.5% above #43
([below](#small-values-are-copied-2026-11-07), which places the 14%). `hello` is the same in all
three.

### Small values are copied (2026-11-07)

Profiled under callgrind ([Measuring](measuring.md#instructions)) in WSL's Ubuntu on a Windows
desktop, Rust 1.96.0, every build alike; `crunch` split in two: its three folds and one map of
numbers, and its map to a record of three numbers (`{ v, dev, sq }`) with the fold over it, each
written in its build's language. Instructions a run, 1,000 items:

| | before #43 `f4f269f` | #43 `e988258` | `main` `c865f3a` | small values, before | after |
|---|---:|---:|---:|---:|---:|
| `crunch` | 22.73 M | 25.21 M | 25.34 M | 25.76 M | **23.37 M** |
| its numbers | 8.65 M | 7.76 M | | 8.08 M | 8.03 M |
| its records | 15.99 M | 18.25 M | | 18.49 M | 16.14 M |
| a filter by fold, 1,000 / 4,000 | | | | 9.77 / 38.77 M | 9.87 / 39.08 M |

#43 made the numbers ×0.90, not slower; the records ×1.14 were the 14%. Its test of what is worth
giving, `heavy`, said yes to every record and list, and giving pays about 1,770 instructions a step
(`fire_giving`, `may_give`, `movable`, `take`, `give_to_leaf`, `compute_given`): more than copying
a record of three numbers, whose fields cost about 420 each (each key a text of its own). `heavy`
looks one level down now: a list or record of up to 8 numbers, booleans or texts up to 32 bytes is
copied like a number, anything longer given, so a list of 1,000 still is and the filter stays
linear (×3.96 for four times the items). Looking costs the filter 1%. The finite check arithmetic
gained (`8bf01b6`, no infinity) went through a call a node, about 25 instructions; on the `f64` in
place it takes 0.18 M off `crunch`. `crunch` is 2.8% above where it was before #43.

A record of 5 to 8 fields built a field at a time is about even (each copy grows with it); a state
value of 20 fields read into a run, which the host then built as constant units a field at a time,
cost 4% more (628 → 655 K a run). Copying records only up to 4 fields changed neither (664 K). The
value is handed to the run as it is now, not built ([below](#a-runs-values-handed-as-they-are-2026-11-07)).

### A run's values handed as they are (2026-11-07)

Profiled first, then built. For each leaf that reads state, the clock, a random number or a
secret, the host stood a constant unit in under that leaf's hash (`SourcedResolver`, holon-host):
a record became `record()` and a `with_field` a field, a list a `list` of constants, all made,
prepared and evaluated in the run. Since a hash named another unit in each run, it let no
composite with a leaf that has any capability keep its children (§8.1 of
[leaves](js-library-races/leaves.md#10-a-composites-preparation-kept-81)), and the clock and a
random number are sourced in every run: each run of a route with a read, a write or a call resolved
its composites' children again, about 1,100 instructions a node.

Now such a leaf ([`SOURCED`](../../../crates/holon-core/src/catalog.rs): `clock`, `random`,
`secret`, `file_arrival`, `store_read`) asks the resolver for its value (`Resolver::sourced`) and
produces a copy of it; the host's resolver hands every unit on as the real one has it, kept
children included. A value that isn't data (an effect left in old storage) still reads as nothing.

Callgrind as [above](#small-values-are-copied-2026-11-07), `79c9c8f` against this change; the state
routes are `GET`s, unrecorded, their state seeded before. Instructions a run:

| Route | before | after | ratio |
|---|---:|---:|---:|
| answers `1` | 103.7 K | 104.0 K | 1.00 |
| reads a state number | 112.0 K | 106.2 K | 0.95 |
| reads a field of a 20-field state record | 648.5 K | 159.8 K | 0.25 |
| the length of a 100-record state list | 10.79 M | 1.74 M | 0.16 |
| 80 operations from a constant | 279.1 K | 277.3 K | 0.99 |
| the same 80 from a state number | 458.8 K | 281.0 K | 0.61 |
| `hello` | 88.7 K | 88.6 K | 1.00 |
| `notes` (its listing, a collection read) | 354.0 K | 351.2 K | 0.99 |
| `crunch`, 1,000 items | 23.38 M | 23.36 M | 1.00 |
| a filter by fold, 1,000 / 4,000 | 9.87 / 39.03 M | 9.87 / 39.13 M | 1.00 |

The 80 operations from a state number now cost 3.7 K more than from a constant; `notes` gained from
its composites' children, kept now that a write beside a read no longer stops them. The two values
gained less than the profile's estimate (×0.18 and ×0.03): the run's own copy of the list is 171 K,
and most of the rest was the host's copies of the whole state around the run, which the next
change shares ([below](#a-runs-state-shared-2026-11-07)).

### A run's state shared (2026-11-07)

The host copied a deployment's whole state four times a run, whether its route read it or not
(`firing.rs`): the state it starts from, under the lock; its values again, for its log entry; each
value it was granted, for the run; and the start once more, kept on the run for the commit's check.
A state cell now holds its value in an `Arc` (`State`, holon-host), so a copy of the state is its
keys and hashes; `run_rounds` is lent the values the run was granted (`&[(Capability, &Value)]`);
and the entry is made from the state the run started from, which it keeps anyway. The entry is the
same, key for key.

Callgrind as [above](#small-values-are-copied-2026-11-07), the change before (`2c0f4d8`) against
this one (`48eb0fe`), the state routes as in the last section. Instructions a run:

| Route | before | after | ratio |
|---|---:|---:|---:|
| answers `1` | 104.0 K | 102.9 K | 0.99 |
| reads a state number | 106.2 K | 104.8 K | 0.99 |
| reads a field of a 20-field state record | 159.8 K | 117.3 K | 0.73 |
| the length of a 100-record state list | 1.74 M | 445.4 K | 0.26 |
| 80 operations from a constant | 277.3 K | 276.6 K | 1.00 |
| the same 80 from a state number | 281.0 K | 278.4 K | 0.99 |
| `hello` | 88.6 K | 88.6 K | 1.00 |
| `notes` (its listing, a collection read) | 351.2 K | 349.0 K | 0.99 |
| `crunch`, 1,000 items | 23.36 M | 23.36 M | 1.00 |
| a filter by fold, 1,000 / 4,000 | 9.87 / 39.13 M | 9.88 / 39.08 M | 1.00 |

The estimate was about 0.4 M for the list. What it pays over a route that answers `1` (343 K) is
now the run's own copy of the list, where it is read, its free, and the check that it is data,
which the next change moves to where a value is written ([below](#a-state-holds-data-only-2026-11-07)).

### A state holds data only (2026-11-07)

A run checked each state value it read was data (`is_storable`), every run, though a write refused
an effect already: the check stood for storage older than that rule. `State` now holds the rule,
as it holds each value's hash: `insert` refuses a value that isn't data, and `load` a store holding
one, so `run_rounds` is lent values it never looks at. One read back from storage outside a state
(a resumed run's clock, verify's from the log) is checked by what reads it.

Callgrind as [above](#small-values-are-copied-2026-11-07), `48eb0fe` against this change
(`241b594`). Instructions a run:

| Route | before | after | ratio |
|---|---:|---:|---:|
| answers `1` | 102.9 K | 102.2 K | 0.99 |
| reads a state number | 104.8 K | 104.7 K | 1.00 |
| reads a field of a 20-field state record | 117.3 K | 116.7 K | 1.00 |
| the length of a 100-record state list | 445.4 K | 423.6 K | 0.95 |
| 80 operations from a constant | 276.6 K | 276.3 K | 1.00 |
| the same 80 from a state number | 278.4 K | 279.5 K | 1.00 |
| `hello` | 88.6 K | 88.5 K | 1.00 |
| `notes` (its listing, a collection read) | 349.0 K | 348.8 K | 1.00 |
| `crunch`, 1,000 items | 23.36 M | 23.39 M | 1.00 |
| a filter by fold, 1,000 / 4,000 | 9.88 / 39.08 M | 9.87 / 39.07 M | 1.00 |

The list gained 21.8 K, the check's own instructions (`is_storable`, 21.5 K), not the 45 K the last
section's profile gave: that was callgrind's inclusive cost, which counts a recursive function's
inner calls again in the outer one's. The rest moved by 0.4% at most. What the list pays over the
route that answers `1` is now 321 K, its copy and the copy's free: `malloc`, `free` and the C
library's functions under them (unnamed in this profile) about 235 K, the copy's own code about
41 K and the free's about 40 K ([Limits](#limits)).

### Through a server again, after the kept children and small values (`490d03e`, 2026-11-07)

**Net, against the build before #43:** no workload is slower. `crunch` is ×1.06 to ×1.09, a filter
by fold ×4.3 at 1,000 items and ×18 at 5,000, `hello` and `calc` the same. #43 made list-building
folds linear and numeric work about 14% slower; #48 took that back and more. Against the last
race's `main`, the filter gave back ×0.85 to the ternary's correctness (below).

The race [above](#the-evaluators-change-through-a-server-43-2026-11-06) run again on `main` at
`490d03e` (#48 and #49: kept children, small values copied, values handed as they are, a run's
state shared, the new leaves and their bounds), against `f4f269f` (before #43) and `78f8f81` (the
last race's `main`), the same three rounds of `holon load`, 16 connections, 5 s a workload, the
same client binary (`78f8f81`'s). The container was a faster one than the first race's (`hello`
about 67 K against 59 K), so only ratios within a race compare. The flow's text is the same, its
unit is not (`ae89406c…`): the number leaves changed (`8bf01b6`, no infinity), so every unit
holding one hashes anew, and the ternary lowers differently (`befee65`, below). Medians of the
rounds, requests a second, with the server's CPU a request; no errors:

| Workload | before #43 | `78f8f81` | `490d03e` | ÷ before | ÷ `78f8f81` | CPU a request (before / `78f8f81` / `490d03e`) |
|---|---:|---:|---:|---:|---:|---|
| `hello` | 67,039 | 64,479 | 65,305 | 0.97 | 1.01 | 42 / 43 / 42 µs |
| `calc` | 57,243 | 55,972 | 57,454 | 1.00 | 1.03 | 51 / 52 / 50 µs |
| `crunch`, 100 | 14,120 | 13,135 | 15,052 | 1.07 | 1.15 | 261 / 282 / 244 µs |
| `crunch`, 1,000 | 1,999 | 1,815 | 2,128 | 1.06 | 1.17 | 1.97 / 2.17 / 1.84 ms |
| `crunch`, 5,000 | 401 | 388 | 439 | 1.09 | 1.13 | 9.9 / 10.3 / 9.1 ms |
| a filter by fold, 1,000 | 1,207 | 6,045 | 5,167 | 4.28 | 0.85 | 3.26 / 0.64 / 0.75 ms |
| a filter by fold, 5,000 | 60 | 1,280 | 1,096 | 18.3 | 0.86 | 69.3 / 3.09 / 3.62 ms |

- **`crunch` is faster than before #43 now**, ×1.06 to ×1.09, the rounds' ranges apart
  (13,862–14,445 before, 15,011–15,254 now, at 100): the small records copied, not given
  ([above](#small-values-are-copied-2026-11-07)). Counted in instructions it is 2.8% above
  `f4f269f`; in time it is ahead, so its instructions likely cost fewer cycles (fewer
  allocations), not measured. `78f8f81` ran at ×0.93 of `f4f269f` again, as in the first race.
- **A filter by fold lost ×0.85 to `78f8f81`**, the ranges apart (5,815–6,127 against 4,983–5,514
  at 1,000), and is still ×18 the filter before #43. The cost is the ternary's: `befee65` lowers
  `c ? a : b` with a guard on each side, so a chosen side that is absent is the answer, as in
  JavaScript; it is 14 nodes where it was 11 (`default`, `not` and the second `guard` added), in
  every step of the fold. The CPU it adds, about 110 µs a request at 1,000 items, is about 110 ns
  a step, three nodes at 30 to 40 ns. The price of the answer being right; a lowering that skips
  the second guard where the false side can't be absent might take it back (not tried).
- **`hello` and `calc` within noise.** The kept children made one call of a 25-node function ×0.58
  in process (`5f29d1d`); whole requests at this load are bound by the HTTP and the host.

### The step budget (2026-11-12)

A round asks for a step before each node and each application of a `map`'s or `fold`'s function,
against `max_steps` (B5, [permissions](../../manual/reference/permissions.md)). Inside a function
the observer is `Asks`, a `dyn Observer`, so each step was a virtual call through it: about 25
instructions a step, `crunch` ×1.025. `Asks` now takes 256 steps at a time from the round's
budget (`Observer::take_steps`), counts them down itself and gives back what it didn't use, so the
count stays exact and the call is made once a batch. Callgrind in WSL as
[above](#small-values-are-copied-2026-11-07), `0650d3d` against the change; instructions, so
WSL's slower sync ([below](#limits)) doesn't enter them. Instructions a run:

| Route | before | a call a step | a batch at a time |
|---|---:|---:|---:|
| `crunch`, 1,000 items | 25.00 M | 25.63 M (×1.025) | 25.13 M (×1.005) |
| `hello` | 94.2 K | 93.8 K | 94.5 K (×1.00) |

In time, a release build took 10 to 20 million plain steps a second (a nested `range` loop, three
steps an item) and about 3 million that each build a record, so the default of 10 million is a
second or a few of computing.

In time, the editor's design mode (holon-wasm's `Evaluator.eval`, built as the editor ships it:
release, no wasm-opt; run in Node) and the same workloads natively (eval under `Budgeted`, as the
design mode evaluates), on a laptop's i7-1360P, each pinned to one performance core at high
priority. Before and after in turn, the order turning each round: wasm in one Node process, 15
rounds, twice (both shown); native one process a run, 15 rounds. Medians of the rounds:

| Workload | wasm, before | a call a step | a batch at a time | native, before | a batch at a time |
|---|---:|---:|---:|---:|---:|
| `hello` | 13.5 / 12.4 µs | ×0.99 / ×0.99 | ×1.01 / ×0.99 | 4.3 µs | ×1.03 |
| `crunch`, 100 items | 237 / 235 µs | ×1.07 / ×1.06 | ×1.04 / ×1.01 | 208 µs | ×1.01 |
| `crunch`, 1,000 items | 1.90 / 2.00 ms | ×1.10 / ×1.04 | ×1.04 / ×1.00 | 1.87 ms | ×1.02 |
| `crunch`, 5,000 items | 10.4 / 9.6 ms | ×1.09 / ×1.07 | ×1.08 / ×1.04 | 9.73 ms | ×1.04 |
| nested loop, 1,000 × 1,000 | 164 / 169 ms | ×1.16 / ×1.16 | ×1.10 / ×1.08 | 140 ms | ×1.01 |

A round's ratio still ranges ±10–20% pinned, so a median is good to a few percent. `crunch` costs
0–4% in time either way. A loop of bare additions, three steps an item and little else, costs 8–10%
in wasm and about 1% natively: V8 makes the count down a step dearer than the native build does.
Unpinned, the scheduler moved the process between the laptop's performance and efficiency cores,
and the same workload's time varied 1.6× from one run to the next.

### Limits

- **Strict is the disk's pace.** About 60% of a strict request is the fsync; any service keeping
  such a record pays it (IP §5). An fsync of an appended file costs about 0.6 ms on ext4, against
  113 µs for a small new file: the size change goes through the journal.
- **On a desktop drive, the drive's own flush is the cost.** Measured on a Windows desktop's SSD
  (NTFS, rc.33, `holon load stress-test`, 8 connections, 10 s a route, 2026-11-05):

  | | strict req/s | p50 | runs a sync | a sync |
  |---|---|---|---|---|
  | Windows | 1,649 | 4.64 ms | 16,491 ÷ 4,133 ≈ 4.0 | ≈ 2.4 ms |
  | WSL2, same machine | 803 | 9.66 ms | 8,025 ÷ 1,992 ≈ 4.0 | ≈ 5.0 ms |

  Lazy (22,388 and 26,774) and unrecorded (65,467 and 60,673) don't wait for the disk and are
  about even. A probe timing 3,000 syncs each way on that drive found the durable ones at about
  2.0 ms and a flush without the drive's cache flush at 32 µs: the drive emptying its own cache
  is about 98% of a strict sync, and no way of syncing removes it without breaking strict's
  promise. Strict's throughput is about the syncs a second times the runs each covers; more
  connections share each sync, and p50 stays near two syncs (the one under way, then its own).
  WSL2's sync passes through Hyper-V to a `.vhdx` on NTFS and, on ext4, likely commits the new
  size through the journal as well: about twice Windows'. A drive with power-loss protection
  (most server SSDs) answers a flush in microseconds, on any system.
- **Writes sync three times** (value, snapshot, head) under the cell's lock. A group commit across
  writes (C4, Q4) is not done; the store's global lock is still taken on a miss and every put.
- **`crunch` is 2.8% above where it was before #43** in instructions, after small values are
  copied ([above](#small-values-are-copied-2026-11-07)), and ×1.06 to ×1.09 ahead of it in time
  through a server; folds that build a list gained ×5 to ×21.
- **A ternary costs three nodes more since `befee65`** (a guard on each side, as JavaScript
  chooses): a filter by fold ×0.85 through a server against `78f8f81`, still ×18 the filter before
  #43 ([above](#through-a-server-again-after-the-kept-children-and-small-values-490d03e-2026-11-07)).
- **Compute-heavy flows** are 3–14 times slower than C#, measured before #43 (`crunch` about 3%
  more instructions since, though faster in time). What is left is allocating and freeing values
  (about 26%), copying fields (6.7%) and firing each node (about 20%). Shared values (step 4) are
  still deferred: after #43's giving and lending no race copies anything large in its inner loop,
  and what an `Arc` would save, and where (`Record` first), is estimated in [leaves
  §7](js-library-races/leaves.md#7-arc-an-estimate). Typed slots or native code (level 3) would
  reverse the core's dependency limit and the no-JIT choice.
- **A run that reads a big state value copies it.** The leaf that reads it produces a copy
  (`Body::Sourced`), and frees it. With a 100-record list in state, the copy and its free are the
  321 K instructions a run that reads the list's length pays over one that answers `1`, about
  three quarters of them in the allocator; they go only with values shared inside the core
  ([leaves §7](js-library-races/leaves.md#7-arc-an-estimate)).
  The samples keep small values in state and big ones in collections, which are read another way.
- **A lazy route on a busy log** keeps a one-connection p99 of about 30 ms, likely the per-second
  sync holding up the next append (RA §2.6); left as it is.
- **The log's lock** still covers the append (about 17 µs) and the line's hash (about 12 µs): a
  strict run must never count a line not written (PL 12.4c).
- **The timeline is 63% of a line** and stays: the log holds what each step gave (Q16, below).
- **The throughput check leaves out hyper and the hop**; at 64 connections hyper and the client
  bound `/health`.
- **The witness export** costs 4–13%; its `pending.jsonl` is synced with the logs, once a second.

### Tried and left

- **Answering on the worker by `block_in_place`** (12.1, R2): gained nothing, cost `/notes` 12%;
  the waiting is on the log's lock, not the store.
- **Dropping each step's output from recorded runs** (Q16, `49c12ea`): lazy `hello` 20,652 →
  28,814, the outputs recomputed matched for every run. Not done: the log would stop holding what
  each step gave.
- **Writing the log in the background** (RP C2): little gain for CPU-bound work, and it would lose
  "answered means recorded", read-your-writes and the packs' timing.
- **Retention per handler** (RP C5): would cut lines from the middle of the chain.
- **Syncing through a handle kept open** (`8e5eb6f`): no change measured. Tried again on Windows
  (2026-11-05), where opening a file passes the file-system filters (Defender's among them):
  the open and close cost about 87 µs of a 2 ms sync, and two runs of 3,000 syncs each put the
  kept handle 1–5% ahead, within their rounds' noise (one run's better p99 didn't repeat). Not
  built: a kept handle not renewed when the log's file is replaced (a cut, a seal) would sync
  the old file, and a strict run would answer without its record on disk, saying nothing.
- **Flushing less metadata** (Windows, 2026-11-05): `NtFlushBuffersFileEx` with
  `FLUSH_FLAGS_FILE_DATA_SYNC_ONLY`, and a log written over zeros so a sync never records a new
  size, each saved nothing measurable on Windows: every durable way's rounds fell within
  1.98–2.06 ms, the kept handle's among them. On Linux
  (ext4, virtio) the zeroed file synced about 30% faster, but that would need reading and
  recovery to stop where the written lines end, for syncs already about 0.3 ms there.
  `FLUSH_FLAGS_FILE_DATA_ONLY` (32 µs) leaves out the drive's cache flush: not durable.
- **Shared values in `Value`** (IP step 4): deferred, as above.
- **Composites evaluated by reference** (JOURNAL, 2026-06-30): every primitive about 24 ns slower
  for a 5–10% gain; reverted. Inputs by reference for primitives was kept (`native_sum` −91%).

## 5. What the tests hold

See [Testing](testing.md). For speed in particular:

- the golden records (`the_evaluator_gives_what_its_golden_record_holds`,
  `the_server_records_what_its_golden_record_holds`) hold every answer, record and hash, so a
  faster path that changes one fails, and the evaluator's is run again with every shape and child
  kept (`keeping_each_composites_shape_changes_nothing_the_evaluator_gives`);
- the differential harness (`holon-core/tests/differential.rs`, `differential.sh`) compares the
  evaluator with an earlier version of itself case by case, outputs and every observer call;
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
