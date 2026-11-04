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
   **Done and measured (step 2):**
   - **The timer** keeps a node's outputs when their JSON fits, as before, without writing the
     JSON: an upper bound on its size, worked out from the value (each value's frame under 64
     bytes, a byte of text at most 6 escaped), settles almost every node, and only one whose
     bound is past the limit is counted exactly, as JSON written nowhere. A number that isn't
     finite has no bound, so its JSON (`null`) is counted. Tests hold the bound above the JSON
     for every kind of value, the worst escapes and the longest numbers included, and the timer
     to keeping exactly what writing the JSON kept, across the limit.
   - **Redaction** leaves a text with nothing to mark (and nothing that reads as a mark) as it
     is, instead of copying it, and sorts a record's keys only when they aren't in order
     already, which a stable sort would have left as they are.
   - **The commit check** (`still_as_read`) asks a collection read again only when the
     collection's root moved since the run read it: the run read it at its start's root, and a
     root names its contents, so the same root gives the same answer. A run that read nothing
     but its cells stops after them. A test holds both to what asking again gives.
     One difference, in a failure mode only: a store read that fails at the check no longer
     sends a run whose collection didn't move round again. A test with a store that fails on demand holds
     it: a run whose collection didn't move commits on what it read; one whose collection moved
     is asked again, and the failed read sends it round again, as before (with the shortcut
     taken out, the test fails on the first).
   - **Whether a run changed its state** (and writes its head) is decided by comparing the two
     states' cells, key by key and hash by hash, instead of hashing the start's snapshot again:
     a snapshot is those hashes and nothing else, and a test holds the two answers equal. The
     snapshot is still put to the store for every run, as the sweep counts on.
   - A withheld output or effect is no longer made loggable before it is withheld.

   Workspace tests and both golden records unchanged. `notes` 629.6k → 537.9k instructions a
   run (**−14.6%**; 73.8–87.0 → 70.4–77.2 µs a run), `hello` and `crunch` unchanged (+0.0%,
   −0.3%). Live, against step 1b (`wrk`, two rounds alternating): the `notes` listing (lazy)
   9,114–9,418 → 9,753–10,232 req/s, 292–297 → 262–270 µs of CPU a request, p50 at one
   connection 189–190 → 168–169 µs; its strict read 7,224–7,451 → 7,554–7,610 req/s;
   `hello` and `calc` within noise.
3. **The walk itself** (§3.2, §3.3 within `run_from`): fetch each input once instead of on every
   revisit, take a node's inputs by reference where the leaf only reads them (`field`, `length`,
   comparisons), move a value to its last consumer.
   *Estimated: `crunch` 20–30%.*
   **Done and measured (step 3):** a node's inputs are lent, never copied to be read. The walk
   reads each input as a reference to where it is kept (the composite's own inputs, or the slot
   of the child that made it), so looking at a node, and looking again at one still waiting,
   copies nothing; `eval_at` takes its inputs so, and hands an ordinary leaf's straight to
   `compute`, which borrowed them already. A `map` lends each item and its extras to its
   function, a `fold` its accumulator too, where each application cloned them. A composite's
   results are moved out of its scratch, which goes as it returns, unless two of its outputs
   read the same one. The choice rule (`choice_next`) reads borrowed slots underneath (it cloned
   each it looked at), with the same answers; `Observer::asserted` is lent the inputs too. The
   few nodes whose builders take inputs owned (a request, a module) are given copies, as before.
   Moving a value to its last consumer, the map's third item, turned out to be needless: lent,
   no consumer copies its input at all.

   Workspace tests, both golden records (every node's outputs, every sample), the eager oracle
   and the deep-nesting test unchanged. Instructions a run: `crunch` 66.35M → 29.42M
   (**−55.7%**; 4,489–4,838 → 2,046–2,079 µs a run), `hello` 205.0k → 135.0k (**−34.2%**; every
   node reading the request copied the whole request to read one field of it; 16.8–19.0 →
   11.6–12.8 µs), `notes` 537.9k → 461.3k (−14.2%; 72–78 → 60–64 µs). Live against step 2
   (`wrk`, two rounds alternating, C# in the same rounds):
   - unrecorded `hello` 55,477–56,808 → 65,536–68,300 req/s (54–55 → 43–46 µs of CPU a
     request), against C#'s 132,607–134,479: **2.0×** from 2.4×;
   - unrecorded `calc` 51,808–53,283 → 62,595–62,872 (59–61 → 48–49 µs);
   - `crunch` (unrecorded), throughput doubled at every size: 100 items 6,295–6,888 →
     12,188–12,272 req/s, 1,000 695–787 → 1,516–1,573, 5,000 139–160 → 312–319; against C#,
     7.2–7.4×, 20× and 24.5× its throughput, and 4.0–4.2×, 16× and 20× its p50 at one
     connection (from 7.4× and 42× after `Prepared`, request-path-L4.md §4.5);
   - lazy `hello` and the `notes` listing within noise: their cost is mostly the record.
4. **Shared values**: `Arc` (or `Rc`) inside `Value` for text, lists and records, so a clone is a
   count, not a copy, and records built by `with_field` share what they don't change. The
   canonical encoding and every hash stay as they are; this is the largest change to core here,
   and the one the copies of §3.3 point to.
   *Estimated: `crunch` 30–50% of what remains.*
   **Deferred, measured first.** After step 3 the copies it would make cheap are gone: what
   copying remains is 5–7% of `crunch` (records copied by `with_field`, about 2,000 a run), 8–9%
   of `hello` (the request's `params` record, copied as a field is read) and about 10% of `notes`.
   Less than that would come off: `with_field` is lent its record, so a shared one would still be
   copied to add a field, unless the last reader of a value could take it, which lending works
   against. For a change to every crate's ~850 uses of `Value::Text`, `List` and `Record`, under
   10%; it comes back with taking a value at its last reader.
5. **A compiled plan** (request-path-L4.md §4.5 b, c): the order fixed, values in slots, leaves as
   direct calls, and in the end generated code, for what steps 1–4 leave.
   Three levels, each harder: (1) a plan per composite, its nodes' kinds and their inputs' sources
   decided once; (2) the walk replaced by instructions over numbered slots, with jumps for the
   lazy choices and every observer event in the walk's order; (3) generated native code.
   **Level 1, done and measured:**
   - Each node's kind (`NodeKind`: a `map`, a `fold`, which request, `coalesce`, a leaf and whether
     variadic, a composite, a module; whether it takes a trigger, is a choice, is held while a
     wired input is absent) is decided once where a composite prepares its children, instead of
     by comparing its name against some twenty at every evaluation. Per run, not kept: a resolver
     may hand a run its own unit for a hash. `eval_at` decides it for a unit evaluated alone.
   - A node's inputs are looked at in one pass: the first not yet evaluated, whether one is
     pending, and the gathered values, together.
   - The edge index is one flat list with each port's source worked out when it is made (an
     input, a child's output, unwired, or a child that doesn't exist), not a list per child; a
     test holds it to a map of the edges on 4,000 random graphs.
   - The walk's scratch is kept for a `map`'s or `fold`'s next application, and a `map` builds
     its function's arguments once, putting each item in place.

   Workspace tests, both golden records, the editor's tests (on a fresh wasm build) and clippy
   pass. Instructions a run: `crunch` 29.42M → 26.18M (−11.0%; 1,999–2,221 → 1,768–1,783 µs a
   run), `hello` and `notes` unchanged (+0.6%, +0.5%): their nodes are each evaluated once a run,
   so deciding a node's kind once a run is what deciding it at its evaluation was; only a
   function applied per item gains. Below the estimate (20–30% and 10%). Live against step 3
   (`wrk`, two rounds alternating), more than the count, from fewer allocations under 64
   connections: `crunch` 11,979–12,021 → 14,133–15,468 req/s at 100 items, 1,580–1,632 →
   1,999–2,074 at 1,000, 320–326 → 416–417 at 5,000; p50 at one connection 0.29 ms, 2.0 ms and
   9.5 ms, against C#'s 0.08 ms, 0.16 ms and 0.60 ms in step 3's rounds (3.5×, 13× and 16×, from
   4.0–4.2×, 16× and 20×); unrecorded `hello` unchanged. What remains in `crunch`'s walk: input
   lookups 9.8%, `compute`'s match by name 5.2%, gathering a leaf's present inputs about 7%,
   allocation about 11%, `with_field`'s copies 6.7%.
   **Level 2, done and measured:**
   - **A plan** (`Plan`) for a composite a `map` or `fold` applies, the walk worked out ahead: the
     order it evaluates the children in, found by its own rule without evaluating anything (from
     the roots, depth first, a child's inputs pulled in port order), and each child's input
     sources as slots. A run goes down it with no stack, no revisit and nothing to look for.
     Firing a child once its inputs are in is one function the walk and the plan share, so only
     the order could differ, and a test holds the two to the same events, in the same order, and
     outputs on 3,000 random graphs, each run twice (the second in the first one's scratch).
   - **Choices.** What a `guard` or `coalesce` evaluates depends on a value, so a composite with
     one is walked, with one exception the profile found: the language shares an input among
     several nodes through a pass-through (a `guard` on a `const` `true`), and nearly every
     function using an input twice has one. Its order is the walk's while its condition reads
     `true`: the plan checks it where the walk's choice reads it, and when it doesn't (only an
     observer standing in for the constant could make it so), the walk carries on from there on
     the same scratch, its next step being a function of what is evaluated. A test makes an
     observer stand `false` in for 1,000-odd such conditions and holds the runs to the walk's.
   - **Leaves called by opcode.** `compute`'s 70 arms match an `Op`, found once for a node (in
     `NodeKind`) instead of a name at every evaluation; `compute(name, …)` stays, for every other
     caller.

   Workspace tests, both golden records, the editor's tests (fresh wasm) and clippy pass.
   Instructions a run: `crunch` 26.18M → 22.24M (−15.1%; 1,756–1,870 → 1,569–1,741 µs a run),
   `hello` and `notes` unchanged (their handlers aren't applied per item, so they are walked).
   Without the pass-through the plan reached almost nothing (−3.6%). Live against level 1 (`wrk`,
   two rounds alternating): `crunch` 14,340–15,455 → 16,937–17,027 req/s at 100 items,
   1,992–2,033 → 2,276–2,357 at 1,000, 399–409 → 469–472 at 5,000; p50 at one connection 0.27 ms,
   1.74 ms and 8.4 ms, against C#'s 0.08, 0.16 and 0.60 ms in step 3's rounds: **3.2×, 11× and
   14×** (from 3.5×, 13× and 16×). What a run now spends is the values themselves: allocating and
   freeing them (~12%) and dropping them (~14%), records copied to add a field (6.7%), and each
   node's own firing (~20%), with `compute`'s match now 5%. The next large step is values that
   don't allocate per node: typed slots for numbers and booleans, and records built in place,
   level 3's ground.

6. **The per-request bookkeeping**, once the graph was no longer the largest part of a request.
   Measured on unrecorded `hello` (136.4k instructions: the graph 41.9k, the run id 24.7k, a log
   entry made and dropped only to be counted ~14k, copies of the request ~6.6k, the sourced
   constants 4.7k) and the lazy `notes` listing (463.8k, of which its record about half: the
   line's JSON 101.5k, the work under the log's lock 40.4k, the entry 49.4k, redaction 19.2k).
   **Done and measured:**
   - **An unrecorded run that didn't fail is counted without an entry.** Its entry is made only
     where it is logged (`log_or_count`): counted under the table's lock with the level the grant
     gives then, exactly as `log_run` counts it; a run that failed, or one whose level isn't
     unrecorded by then, is logged from the run kept for it, as before. The answer's checks read
     the run's error, not its entry.
   - **One copy of the request** is lent to the run, the check for a wait and the entry, where
     each had its own.
   - **Hashing into a kept buffer.** `hash_value` and `state_root` write their encoding into a
     buffer the thread keeps, instead of one grown afresh: the same bytes hashed. A run's id is
     hex-encoded once, not three times.
   - **Sourced constants made when a leaf asks.** A run's sourced values (the clock, a random
     number, the state it reads) become constant units the first time a leaf reads one, not all
     at the start; each value's pieces under keys of their own. A test reads two list- and
     record-valued ones in one run and checks each comes back whole, one no leaf reads is never
     made, and one a constant can't rebuild still reads as nothing.
   - **A request's timeline moved into its entry**, not copied, and **its line written into room
     the thread's last line needed**, not a buffer doubled from nothing: the same bytes.
   - **Redaction looked at, not walked, with nothing to mark.** With no secret, token or answered
     credential to replace, redaction can only escape a look-alike of a mark, text holding a `‹`
     (an entry's records are in key order already, from `BTreeMap`s): the line is made and looked
     at for one, and redacted and made again only if it has one. A test holds the line and the
     entry to redacting first, with and without look-alikes (with the check taken out, it fails).

   Workspace tests, both golden records and clippy pass. Instructions a request: unrecorded
   `hello` 136.4k → 97.0k (**−28.9%**), the lazy `notes` listing 463.8k → 367.7k (**−20.7%**).
   Live against the build before (`wrk`, two rounds alternating, C# in the same rounds):
   - unrecorded `hello` 65,837–66,203 → 73,041–77,022 req/s (45 → 37–39 µs of CPU a request, p50
     at one connection 62–64 → 56–57 µs), against C#'s 127,458–134,524: **1.75×** from 2.0×;
     unrecorded `calc` 57,765–63,092 → 63,736–69,730, against C#'s 114,978–119,162: 1.65–1.87×;
   - lazy `hello` 18,440–20,241 → 20,941–21,591 (127–133 → 110–112 µs), lazy `calc`
     14,255–15,056 → 16,944–17,390 (170–173 → 134–136 µs), the `notes` listing 10,498–11,131 →
     11,498–11,732 (240–251 → 216–219 µs);
   - strict `hello` 9,484–9,532 → 10,513–10,727 (p50 at one connection 584–600 → 521–535 µs),
     above the 9,843–10,268 the C# service keeping the same record gave in request-path-L4.md
     §1.3 (not measured again here).

   What a recorded request still spends on its record is mostly the record itself: its line's
   JSON (escaping its strings, 13.8% of `notes`), the line's hash for the chain under the lock
   (it holds the seq and the line before's hash, which only the lock knows), and the timeline,
   63% of the line, which stays as it is (request-path-L4.md §8, Q16).

## 5. What this doesn't cover

- **Strict runs' disk sync**, which is 60% of a strict request and the same for any service that
  keeps its record before answering (request-path-L4.md §1.3).
- **Runs that call out**, where the network sets the pace.
- **The editor's evaluator** in the browser: the same core in wasm, so steps 1, 3 and 4 help it too,
  and step 2 doesn't reach it.
- **JS nodes**, which run in `wasmi`, an interpreter of its own (wasm-module-L4.md).
