# The interpreter's path: what a request does, step by step, and what it needn't

**Status:** a map, not a change (2026-11-03, at `4313f0a`). Nothing here is built. Every claim
about the code is cited by file and line at that commit; every share is measured (§1).

Holon's engine is compiled Rust, but a flow is not compiled: at run time the engine walks the
flow's graph as data, and each node is told apart, fed and run through general-purpose code on
general-purpose values. That is the difference to a C# handler, which is the compiled code
itself (request-path-L4.md §1). This note maps that walk, from the socket to the answer, so that
what is redone, repeated or copied without need can be seen and taken out in order, before (and
apart from) compiling flows.

## 1. How it was measured

- **Three runs**, through `serve_data` on an on-disk store (`profile_one_route` in
  `crates/holon/src/tests.rs`, `HOLON_PROFILE=…`), under callgrind: instructions inclusive and
  exclusive, and calls, divided by the runs measured.
  - **`hello`**: an unrecorded `GET /hello/:name`, three nodes. 20.2 µs a run.
  - **`notes`**: a lazy `GET /notes` listing a collection of three, recorded. 79.6 µs a run.
  - **`crunch`**: an unrecorded `POST /stats` over 1,000 numbers, six `map` and `fold` passes.
    4.57 ms a run.
- **The code path** was traced in full, every step read, with the file and line of each claim.
- Shares below are of a run's instructions (callgrind). Live latency adds hyper and the socket,
  which this leaves out.

## 2. The path

Each step: where, how often, what it depends on (the unit or deployment alone, the run, or the
values), and its share where it shows in a profile. "Per run" is per request unless it retries.

| # | Step | Where | How often | Depends on | Share (hello / notes / crunch) |
|---|---|---|---|---|---|
| 1 | Read the request: body, headers, `DataRequest` | dataplane.rs:267-292 | request | values | small |
| 2 | CORS origin, under the table lock | listeners.rs:27-31 | request | deployment | small |
| 3 | On the worker or the pool (`runs_here_for`): `match_route`, the label, the level, a cache | dataplane.rs:351-379 | request | unit (cached) | small |
| 4 | Snapshot of the deployment under the table lock, **`d.server.clone()`** | listeners.rs:87-91 | request | deployment | small |
| 5 | The closure from the flow cache | listeners.rs:98, units.rs:185-196 | request | unit (cached) | small |
| 6 | A second `match_route`, for resumes | listeners.rs:112-127 | request | values | 0.8% (hello, both) |
| 7 | `data::handle`: a third `match_route`, the body copied, `request_record` | data.rs:95-160 | request | values | 2.2% (hello) |
| 8 | The level read again, the request cloned for the run | listeners.rs:145-150 | request | unit | small |
| 9 | **`hash_unit(endpoint)`**, for `park_if_waiting` | listeners.rs:155 | request | unit | **4.5% / 1.6%** |
| 10 | `fire_with_state`: nonce, run locks, the state copied for the attempt | firing.rs:133-260 | attempt | run | |
| 11 | `run_attempt`: `state_before`, **`run_id_of`**, the readables, sourced state and secrets, taint walks, the timer | firing.rs:522-625 | attempt | run, unit | **10.1% / 4.2%** (run id) |
| 12 | **`SourcedResolver::new`**: each sourced value turned into a `const` unit | holon-host/src/lib.rs:326-398 | attempt | values | 2.0% / 1.0% |
| 13 | `run_rounds`: the recorded answers indexed, then rounds | holon-host/src/rounds.rs:223-290 | attempt | run | |
| 14 | `eval_at` for each node: the trigger check, **20 guard arms**, `VARIADIC`, then `compute` | eval.rs:172-250 | node | unit | 2.7% / 1.4% / 7.4% (`compute`) |
| 15 | **`Prepared::of`** for each composite: children resolved, an edge index, **`topo_order`** (only a cycle check), the roots | eval.rs:338, 363-399 | composite evaluation | unit | **12.8% / 4.3% / –** |
| 16 | `SourcedResolver::resolve` for each child: **`capabilities_of`** per leaf, a `Capability` clone per sourced entry | holon-host/src/lib.rs:341-355 | child, per composite evaluation | unit | 3.4% / – |
| 17 | `run_from`: per node, `slot_in` **clones** each input; `seen`, a second Vec, `gathered`; revisits recompute `seen` | eval.rs:407-513 | node | values | – / – / 12.5% self |
| 18 | `compute`: a **67-arm string `match`**; `const` **parses its literal**; `field` clones; `with_field` clones the map | primitives.rs:18-596, 965-979 | leaf | unit, values | parse 0.6% (crunch) |
| 19 | `map`/`fold`: the function prepared once (since `4313f0a`), each item and **each extra cloned** | eval.rs:574-708 | item | values | 88% (crunch, `apply`) |
| 20 | The timer: per node at depth ≤ 3, outputs to `SValue`, then **JSON only to measure** | recorder.rs:336-343 | node | values | – / **9.0%** / off |
| 21 | Stateful commit: `still_as_read`, commit, **a snapshot put even when nothing changed**, its hash recomputed | firing.rs:438-517, restore.rs:23-32 | attempt | run | – / **6.9%** |
| 22 | `run_entry`: every field of the run copied, values converted twice | firing.rs:993-1071 | run | values | 2.4% / 6.7% |
| 23 | Seven `error.starts_with(…)` checks, `record_trigger`, the answer's body copied | listeners.rs:193-233 | request | run | small |
| 24 | `log_run`: the level a third time, **`Redactor::of`**, **`Redactor::entry` reallocating every string** | firing.rs:877-902, secrets.rs:852-1045 | run | vault, values | – / **6.0%** |
| 25 | **`Made::of`**: the entry to JSON, outside the lock | recorder.rs:457-468 | run | values | – / **14.7%** |
| 26 | Under the log lock: the line copied, hashed, `format!("{line}\n")`, written | recorder.rs:738-818 | run | values | – / 3.4% (hash) |
| 27 | `respond`, then hyper | data.rs:163-221 | request | values | 0.5% / 0.4% |

Under all of it:
- **Allocation:** about 470 `malloc` and 475 `free` a `hello` run, 950 a `notes` run, and 110,000 a
  `crunch` run (18 per function application). With `memcpy`, about a third of `hello`'s
  instructions.
- **Locks per request:** the global deployments table is taken five times (steps 2, 3, 4, 23, 24),
  besides the flow cache, the vault (several times when there are secrets) and the log cell.

## 3. What the map shows

### 3.1 Unit-only work redone per request, attempt, node or item

| Item | Where | Measured | Behaviour |
|---|---|---|---|
| Composites prepared on every evaluation, the cycle check included (`Prepared::of`, `topo_order`) | eval.rs:338, 383 | hello 12.8% (topo 5.0%), notes 4.3% | none: a function of the unit; keep it per unit hash |
| `hash_unit(endpoint)` per request, equal to `route.endpoint` | listeners.rs:155 | hello 4.5%, notes 1.6% | none |
| `capabilities_of` per leaf child each time its composite is prepared (SQL parsed for database leaves), and a `Capability` clone per sourced entry | holon-host/src/lib.rs:341-355 | hello 3.4% (resolve) | none: per unit and set of sourced capabilities |
| Each sourced value rebuilt as a `const` unit per attempt; a `const` re-parses its literal per evaluation | holon-host/src/lib.rs:326-398, primitives.rs:25, 965-979 | hello 2.0%, crunch 0.6% | none |
| Leaves told apart by name: 20 guard arms, `TRIGGERED` and `VARIADIC` scans, a 67-arm string `match` | eval.rs:181-236, primitives.rs:19-595 | crunch: `compute` 1.9% self, `takes_trigger` 0.7%, `choice_next` 2.3% | none: an opcode fixed when prepared |
| `d.server.clone()`: the whole route table deep-cloned under the global table lock | listeners.rs:91 | small alone; holds the global lock | none: `Arc<Server>` |
| `match_route` three times, allocating a params map per candidate route; the label built twice | dataplane.rs:362, listeners.rs:114, data.rs:104 | hello 0.8% | none |
| The log level read three times; `Route.log` holds it | dataplane.rs:364, listeners.rs:145, firing.rs:887 | small | none |
| `SecretFed::of` and `sent_secrets` per attempt, `answered_secrets` per request | firing.rs:577-583, listeners.rs:166 | only with secrets | none: per unit and secret names |
| `Permit::allows` formats the capability into a new string for every check; set accessors clone `BTreeSet<String>`s about ten times an attempt | permissions.rs:891-947 | small each, many | none |
| `Redactor::of` built from the vault per run | secrets.rs:852-869 | in notes' 6.0% | none: per vault version |

### 3.2 Work done twice, or for nothing

| Item | Where | Measured | Behaviour |
|---|---|---|---|
| The timer turns each node's outputs into JSON only to measure them; the line serializes them again | recorder.rs:340, 467 | notes: `produced` 9.0% | none: measure without serializing |
| Redaction returns a new string for every text, even when nothing matches; every record re-sorted | secrets.rs:957, 1040 | notes: `logged` 3.7% | none |
| A snapshot put to the store (under its global lock) for a run that changed no state, then its hash recomputed to find nothing changed | restore.rs:30, firing.rs:123-128 | notes 2.8% | none: the same snapshot is already there by hash |
| `still_as_read` builds readables even when nothing was read | firing.rs:438-468 | notes 4.1% (all of it) | none |
| A node waiting for an input recomputes `seen`, cloning every input it already had, on each revisit | eval.rs:439-454 | in crunch's 12.5% | none |
| `without_kept_files` clones the timeline before checking it has anything to do | firing.rs:936-939 | small | none |
| `logged_outputs` builds a loggable copy, converts it and drops it, even when withheld | firing.rs:961-978 | in run_entry | none |
| `SharedReads` built and dropped without a claim; claim records cloned then overwritten; rounds cloned back | firing.rs:591-611, 715/247, 194 | small | none |
| The run id hex-encoded three times; the body hashed twice into it | firing.rs:540, 607, 706, 747-757 | small | none |

### 3.3 Copies

| Item | Where | Measured |
|---|---|---|
| **Values flow by deep clone.** Each input is cloned out of its source (`slot_in`), never moved to its last consumer; `field` clones the whole record to read one field; `with_field`, `insert`, `append` clone the whole container, so building a record or accumulating with `fold` is O(n²); `map`/`fold` clone the extras per item | eval.rs:416-428, primitives.rs:255, 337-373, eval.rs:590, 701 | crunch: `clone` 10.9%, record copies (`clone_subtree`) 8.9%, drops 13.8% |
| **The request body** is copied or converted about ten times (bytes to Vec to String, into the record, into the run twice, into a `const`, into the entry twice, through redaction, into the line) | dataplane.rs:290 … recorder.rs:790 | in each step's share |
| **The state** is deep-copied about six times per attempt before any node reads it | firing.rs:190, 537, 557, 712, 1012, holon-host/src/lib.rs:374 | small for small states; grows with the state |
| **The log line** is built three times as a whole string | recorder.rs:467, 474-483, 790 | notes: part of 14.7% and 3.4% |
| `run_entry` clones every field of a run that is dropped right after | firing.rs:993-1071 | hello 2.4%, notes 6.7% |

### 3.4 Gaps: what isn't there

- **No shared values.** `Value` is an owned tree (value.rs:20-41): a `List` is a `Vec`, a `Record` a
  `BTreeMap<String, Value>`, and every clone is deep. There is no `Arc` anywhere, so a value read
  by two nodes is copied twice, and a list passed to a `map` is copied with each extra.
- **No per-unit plan kept across runs.** The flow cache keeps units; what is worked out from a unit
  to run it (§3.1) is thrown away after each evaluation.
- **No opcodes.** A leaf is its name, compared as text at each evaluation in three places.
- **No last-use analysis.** Nothing knows a value's last reader, so nothing can be moved.

## 4. In what order

Each step is measured as the request-path plan's were (`profile_one_route` for `hello`, `notes`
and `crunch`, the throughput check, live `wrk` against the build before), and changes no answer, no
record and no hash: a test that a run's entry and answer are byte for byte the same before and
after keeps each honest. The gains below are estimated from the shares above.

1. **Keep what a unit needs to run, per unit hash, across runs** (§3.1, the first six rows): the
   prepared composite (children, edge index, roots, the cycle check), each leaf's opcode, its
   capabilities and sourced substitution, a `const`'s parsed literal; `route.endpoint` instead of
   `hash_unit`; `Arc<Server>` and one `match_route` with the level from `Route.log`.
   *Estimated: `hello` 20–25% (about 4–5 µs of 20), `crunch` 3–5%.*
   **Done, in part, and measured (step 1a):** the endpoint's hash and its level taken from the
   route (`data::handle` hands the run its `Route`), the resume route looked for only in a flow
   that has one, and, in `Prepared`, an index of edges by child and port in place of a map, and a
   cycle check of lists (`has_cycle`, a test holds it to `topo_order` on 4,000 random graphs) in
   place of building an order to discard. Both golden records unchanged. `hello` 242.5k → 226.0k
   instructions a run (−6.8%; 20.7–24.3 → 18.5–19.1 µs a run), `notes` −1.9%, `crunch` within
   noise: below the estimate, because preparing a composite is still done on every run, its cycle
   check and index rebuilt (3.8% and 2.1% of `hello`) and its children resolved (3.7%, which has to
   stay: resolution swaps in the run's sourced values).
   **Done (step 1b):** each composite's shape (core's `Shape`: the index, the roots, whether it
   has a cycle) is kept across runs. A resolver may hand one out (`Resolver::shape`, which
   wrappers forward and the default answers with none, so a run works it out as before); the
   server's closure (`Loaded`) builds a table of them (`Shapes`) when it loads a flow and keeps
   it with its units. The table finds a shape by the unit itself, by where the closure holds it,
   so a lookup hashes nothing and a unit a resolver makes up for one run (a sourced value's
   constant) never meets a shape that isn't its own. Children are still resolved on every run.
   The evaluator's golden record is checked a second time with every shape kept (found over a
   thousand times), and gives the same record. `hello` 223.4k → 204.8k instructions a run
   (−8.3%, measured by the difference between two run counts, so setup is left out; 20.6–21.7 →
   17.7–18.3 µs a run), `notes` −2.4%, `crunch` −0.15%. Step 1 as a whole, counted the same way
   against the build before it: `hello` 239.9k → 204.8k (−14.6%; 20.7–24.3 → 17.7–18.3 µs a
   run), `notes` 657.8k → 629.6k (−4.3%), `crunch` 71.50M → 66.47M (−7.0%, step 1a's index and
   cycle check, within the noise of its time). The opcode and the `const` literal aren't kept: the profile puts all of `compute`,
   its dispatch by name included, at 3.0% of `hello` and parsing literals at 0.13%, too little
   to be worth another kept table.
   Live, step 1 as a whole against the build before it (`wrk` at 64 connections, two rounds
   alternating, as request-path-L4.md §1): unrecorded `hello` 49,573–51,141 → 56,537–57,270
   req/s (60–62 → 53–54 µs of CPU a request, p50 at one connection 76–79 → 70–71 µs),
   unrecorded `calc` 45,288–46,095 → 52,556–52,818 (69–70 → 59 µs); lazy `hello` 172–201 →
   163–187 µs of CPU a request, its throughput within the rounds' noise (12,911–15,472 →
   13,945–16,269); strict as before (the disk's pace: `hello` 7,892–8,399 → 8,909–9,067,
   `calc` 7,301–7,394 → 7,540–7,569).
2. **Stop the double work around the record** (§3.2): measure the timer's outputs without
   serializing; redaction that copies only what it changes; no snapshot put or re-hash for an
   unchanged state, no readables for nothing read.
   *Estimated: `notes` 15–20%, lazy and strict routes alike.*
3. **The walk itself** (§3.2, §3.3 within `run_from`): fetch each input once instead of on every
   revisit, take a node's inputs by reference where the leaf only reads them (`field`, `length`,
   comparisons), move a value to its last consumer.
   *Estimated: `crunch` 20–30%.*
4. **Shared values**: `Arc` (or `Rc`) inside `Value` for text, lists and records, so a clone is a
   count, not a copy, and records built by `with_field` share what they don't change. The
   canonical encoding and every hash stay as they are; this is the largest change to core here,
   and the one the copies of §3.3 point to.
   *Estimated: `crunch` 30–50% of what remains.*
5. **A compiled plan** (request-path-L4.md §4.5 b, c): the order fixed, values in slots, leaves as
   direct calls, and in the end generated code, for what steps 1–4 leave.

## 5. What this doesn't cover

- **Strict runs' disk sync**, which is 60% of a strict request and the same for any service that
  keeps its record before answering (request-path-L4.md §1.3).
- **Runs that call out**, where the network sets the pace.
- **The editor's evaluator** in the browser: the same core in wasm, so steps 1, 3 and 4 help it too,
  and step 2 doesn't reach it.
- **JS nodes**, which run in `wasmi`, an interpreter of its own (wasm-module-L4.md).
