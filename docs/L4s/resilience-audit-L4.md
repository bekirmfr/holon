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

## 3. Order

1. **R1–R3**, which change how the server behaves when something goes wrong.
2. **D1–D3**, the knowledge kept twice and the redaction shortcut's assumptions.
3. **D4, D5, S1–S5**.
4. Then the rest of the check: the end-to-end suite (`editor/e2e`), a soak under load watching
   memory, and a review of `aa2704a..HEAD` by a reader given the code, not this note.
