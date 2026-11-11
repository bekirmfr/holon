# The test suite, reviewed under the class-level approach (L4)

2026-11-11. Every test in the repository read against one rule
([Testing](../engineering/themes/testing.md), §2): a test holds a **class** of cases, not the
case that was found. This note is the review's result: what the suite is today, the classes its
tests are examples of, the generic test conceived for each, and the order to take them in.

Nothing was changed in this pass. The work that follows goes one test at a time: conceive the
generic test of a class, check which existing tests it covers, drop those (their cases become its
rows), go on to the next. Each generic test's doc says what it **guards**, **why** it is needed,
and what it means **if it fails**.

## 1. What was read

Thirteen reviewers, one per part of the suite, each read every test body in its part, and the
code under test where a judgment needed it. Each test was put in one of four groups:

- **C**, class-level already: it sweeps, generates, or uses an oracle, a model or a table;
- **E**, an example of a class: one or a few hand-picked cases of a property that holds for a
  whole class;
- **S**, single on purpose: wiring, a pinned constant, a tripwire, a measurement;
- **R**, redundant now: every case it checks is inside a class-level test that exists today.

| Part (its report) | Tests | C | E | S | R | Clusters |
|---|---:|---:|---:|---:|---:|---:|
| `holon/src/tests.rs` 1–3591 (`server-tests-1`, T1) | 82 | 1 | 76 | 5 | 0 | 21 |
| `holon/src/tests.rs` 3592–7461 (`server-tests-2`, T2) | 83 | 3 | 78 | 2 | 0 | 25 |
| `holon/src/tests.rs` 7462–end, `tests/semantics.rs` (`server-tests-3`, T3) | 83 | 5 | 71 | 7 | 0 | 21 |
| `recorder.rs`, `mcp.rs`, `lsp.rs`, `workbench/` (`server-big-modules`, BM) | 113 | 10 | 72 | 31 | 0 | 24 |
| the server's modules a to l (`server-modules-a-l`, AL) | 131 | 4 | 119 | 8 | 0 | 30 |
| the server's modules m to z (`server-modules-m-z`, MZ) | 146 | 5 | 130 | 11 | 0 | 34 |
| holon-core: leaves and values (`core-leaves`, CL) | 190 | 10 | 170 | 10 | 0 | 18 |
| holon-core: the evaluator (`core-engine-a`, CA) | 114 | 13 | 86 | 2 | 13 | 15 |
| holon-core: kinds, hashes, validation, taint (`core-engine-b`, CB) | 187 | 14 | 165 | 4 | 4 | 18 |
| holon-author (`author`, AU) | 218 | 36 | 170 | 11 | 1 | 19 |
| holon-host, holon-module, holon-wasm (`host-module-wasm`, HM) | 115 | 7 | 100 | 8 | 0 | 18 |
| the editor's unit tests (`editor`, ED) | 298 | 17 | 227 | 54 | 0 | 26 |
| end to end (`e2e`, E2E) | 112 | 2 | 90 | 20 | 0 | 14 |
| **All** | **1,872** | **127** | **1,554** | **173** | **18** | **283** |

The editor's 298 blocks run as 667 tests (four of them once per sample). The reports are in
`docs/L4s/test-suite-review/`, one per part, each with its clusters in full: members with their
lines, the generic test proposed, what it guards, why, what a failure means, what it covers, what
no test reaches today, the effort. A cluster is cited below as part and number (`T2§10`). That
folder is kept out of the repository for now: the reports name suspected bugs, some of them in
what keeps secrets, and the repository is public (§7).

## 2. What the suite is today

**Five tests in six are examples.** 1,554 of 1,872 check hand-picked cases of a property that
holds for a class. 127 are class-level, and of those the language crate's 36 include 17 hand
tables that don't fail when a member has no row (read strictly: 110).

**Where class-level tests exist they are strong, and each has holes.** The evaluator against the
eager one on generated graphs, a function's plan against the walk, the edge table against a map,
SQLite's authorizer over generated statements, a handler as a flow and as JavaScript, the two
golden records, the sweeps over the 84 samples. Their holes are listed in §6: most are a
generator that draws a few kinds of the many there are, or a run count in CI too small to reach
a kind it can draw.

**Only 18 tests are redundant today** (§5), all in holon-core and one in holon-author. The reason
is the shape of the server's tests: in the last third of `tests.rs`, 30 of the 71 examples are
scripts of 60 to 150 lines that walk one sample through five to ten properties. No generic test
drops one of those whole. Each
generic test takes one property out of many of them, and a script goes when its last property is
held elsewhere. So for the server, "drop the redundant tests" is mostly "take these lines out of
these tests", until a test is empty.

**The same few classes are examined over and over, a case at a time.** The 283 clusters fold
into the 32 classes of §4. A secret staying out of what a run leaves is tested in 63 tests, in
the server, the core, the language crate, the editor and the browser tests, none of which
searches everything a run leaves; runs at once in 53, twenty of which place the other run with a
sleep; every recorded run checking offline ends 26 tests with the same six lines and is asked of
no golden run.

**What no test holds at all**, most costly first:

- no path through TLS is run by any test: every stand-in server is plain, on loopback (MZ);
- there is no list of the control plane's routes (the router is a chain of `if`s), so nothing is
  held of *every* route: who may call it, that a read changes nothing, that a change names who
  made it (T1, T2, AL, BM);
- nothing holds a staged leaf to the catalog's leaf of that name, and no generated bytes reach
  the unit decoder (CB);
- a run whose requests the host answers has no source of truth: the eager evaluator answers
  nothing, so every oracle test of the evaluator covers only runs whose requests are absent (CA);
- no expression is evaluated against JavaScript (about 130 forms are checked only for the leaf
  they place), and nothing says what `atan2`, `power`, `remainder` or fifteen `math` functions
  answer (AU, CL);
- the one place the agent's role is put on a request is run by no test: the tests use a stand-in
  that adds it (BM);
- nothing holds the committed JavaScript engine to its source or to today's core, and no unit
  hash or module call is pinned across native and wasm32 (HM);
- no restart test opens the store again from disk; they reload through the same object (T2);
- `MAX_CLOSURE_UNITS` has no test in any crate (T3); which file starts which run
  (`arrivals.rs`) has none in its module; no command's `run()` is tested, its printer is, on JSON
  written in the test (AL);
- the editor as shipped (the built bundle inside the binary) is loaded by no browser; nothing
  fails when `editor/wasm` is stale; `signin.ts` has no unit test (E2E, ED).

**Tests that don't test what they seem to.** Twenty concurrency tests schedule by sleeping and
judge by elapsed time. Five engine tests check nothing without `HOLON_TEST_POSTGRES` or
`HOLON_TEST_MYSQL`. Three end-to-end tests call the real httpbin.org. The SQS stand-in checks
signatures with the signer under test. `e2eSweep.test.ts` takes a click on the deploy button for
a deploy, so two samples are deployed by no test.

## 3. Seams to build once

Several classes wait on the same missing piece. Built with the first class that needs it.

1. **A list of the control plane's routes**, each with its method, what it changes and who may
   call it. Classes 8 and 26, and parts of 2, 4 and 28. `http.rs` is being changed by the
   accounts and access work in another branch; this seam follows that.
2. **Every way a run starts, as one list.** The panic table has it (a request on the worker and
   on the pool, an action, a clock, a file, a waiting run); a mail, a feed event, a queue message
   and a WebSocket block are missing from it. Rules that list handler kinds by hand have drifted
   from it. Classes 2, 11, 15.
3. **A server on disk that is closed and opened again**, where today's restart tests reload
   through the same store. Classes 2, 15, 16.
4. **An upstream that holds a call until told**, in place of sleeps. Class 13. *Built: the
   `Gate` of `gated_counting_server` in tests.rs.*
5. **One far side that misbehaves on a script** (slow, cut, too much, again), for every
   connector. Classes 14, 17.
6. **Three generators shared between tests**: sound units (classes 1, 21, 22), the audit's value
   pool with bytes in it (classes 5, 19, 21), and the handler grid of `semantics.rs` (classes 12,
   18).

## 4. The worklist

32 classes in five groups, by what a break costs. Effort: S under an hour, M a few hours, L a
day or more. "From" cites the clusters folded in and counts their member tests; a test with
several properties is counted in each. "Settles" names entries of the suspected-bugs file the
class's test would show or clear.

### Order

1. **Step 0** (§5, §6): drop the 18 redundant tests; close the cheap holes in the class-level
   tests that exist, each of which makes more tests redundant. *§5 done; five of §6's twelve
   holes closed (2026-11-11).*
2. **Class 9, first half**: an hour's work that checks offline the golden record's cases (about
   550), none of which is checked so today.
3. Then the groups in order, A to E. Inside a group, the cheaper first.
4. Class 8 after the accounts and access redesign lands. Class 21 after the decision on code only
   tests call (§8). Class 31 after 9 and 12, which take most of what the browser tests repeat.

### A. What a flow may do, and what is kept secret

**1. A unit a client stages is taken exactly when its wiring is sound and each leaf is the
catalog's, and bytes that aren't a unit's are refused.**
- Guards: the server never loads or runs a unit whose shape it didn't check.
- Why: `validate` is the one gate for units made outside the language; what it doesn't hold, the
  taint walk, the manifest and the evaluator read as declared.
- If it fails: a hand-made unit runs with a promise shown to a person that isn't the one it keeps.
- Test: property. Units sound by construction, one mistake put in (a row per `ValidationError`),
  the catalog as truth for a leaf's ports, and bytes drawn at the decoder.
- From: CB§10, CB§9, CA§6, CA§5, T2§9 (39). Effort M; seam 6.
- Settles: `core-engine-b 1`, `12`; `core-engine-a 2`.

**2. Neither a secret nor what is computed from it is in anything a run leaves or the server
says, but where a person's approval shows it.**
- Guards: the log, the events, every answer of the control plane, every file under the data
  folder, for every leaf a secret can pass through, every place a run puts something and every
  way a run starts.
- Why: redaction is a list of fields walked by hand, and a search for the value's literal forms;
  a new field or a new form is outside both.
- If it fails: a credential in a log that is exported and shared.
- Test: a sweep with an oracle. A canary value; every byte on disk and every read searched; the
  run made twice with two values must read the same; what shows compared with what the taint walk
  says may show. First the pure half (the redactor against a plain model, a canary in every text
  field of a log entry: M), then the whole server on disk (L).
- From: T1§1, T2§1, T3§1, MZ§1, MZ§2, MZ§7, AL§5, CB§1, AU§2, BM§5, E2E§1 (63). Seams 2, 3.
- Settles: `server-modules-m-z 1`, `3`; `core-engine-b 3`.
- Done in part (1c094ab3, 2026-11-11): a flow's tainted keys and the credentials every run taints,
  sent through every place a run has them and looked for in every form in the log, its lines,
  the listing, the export, `verify`'s report and a replay
  (`a_tainted_keys_values_never_reach_what_the_host_keeps_wherever_a_run_has_them`). Left: every
  file under the data folder and every answer of the control plane; the run made twice with two
  values; what shows against what the taint walk says may show.

**3. A secret leaves the server only to a host its approval shows, over the scheme shown, and
through any chain of redirects stays with the first host.**
- Guards: outbound calls, the witness, every connector's "this machine" rule.
- Why: seven modules each read a URL their own way.
- If it fails: a credential sent where nobody approved.
- Test: property against a model of the rules, with hosts that keep what they were sent; the
  connecting library as oracle for which host a URL names.
- From: T2§2, MZ§3, MZ§4, MZ§11, AL§6 (15). Effort M to L.
- Settles: `server-modules-a-l 2`.

**4. A flow's manifest names everything a run of it can do, and a run does no more than the
grant in force allows.** Three tests:
- (a) *the manifest*: for every closure, every request and effect a run makes is one the manifest
  names (a run that records what it asked for, against the manifest); a row per leaf that touches
  something; a row per capability in every table that sorts them. From CB§2, CB§3, CB§5, CB§7,
  HM§2, AU§4 (72). M.
- (b) *the grant*: a table with a row per `Capability` and per limit, failing on one with no row,
  truth being the far side's count. From T1§2, T2§21, T3§19, AL§20, MZ§8, MZ§9, MZ§12 (25). L.
- (c) *the gate*: a deploy goes through exactly when all it needs is granted, the preview says
  what the deploy does, and an agent's version runs with nothing a person didn't approve for that
  code. Property against a plain set model over every sample's plan. From T1§3, T3§15, MZ§10,
  BM§1, BM§2, ED§4, ED§5 (35). M.
- Guards: the first promise, "it shows what a flow may do before it runs".
- Why: the capability list is matched by hand in several tables, and they have drifted.
- If it fails: a flow acts outside what was approved.
- Settles: `core-engine-b 5`, `7`, `9`; `server-tests-1 7`.

**5. A value that isn't plain data crosses no boundary: into a run from outside, between flows,
or into an effect's payload.**
- Test: a table of doors crossed with the value pool `boundary.rs` uses.
- From: HM§1, T3§4, CL§1 (8, and lines of five more). Effort M; seam 6.
- Settles: `core-leaves 2`.

**6. A name from outside (a file name, a slug, a path, a key, a value in a statement) touches
nothing outside what it is for.**
- Test: property, a snapshot of the file system before and after as oracle; SQL values as
  parameters against a model of places.
- From: AL§1, T2§8, T3§2, T3§5, MZ§29, MZ§6, AU§1 (22). Effort M; link and device-name rows need
  a Windows job beside CI's Linux one.
- Settles: `server-tests-3 1`; `core-leaves 5`; `server-modules-a-l 12`.

**7. Every check of who sent something passes exactly what was signed, by that key, now.**
- Guards: `jwt_verify`, `stripe.verify`, the SQS signer, PGP, a mailbox's authentication, the
  Host and Origin gate.
- Test: per scheme, the standard's vectors and one change at a time to each; Node's `crypto` for
  Stripe; AWS's suite for the signer; a plain model for Host and Origin (S).
- From: CL§2, CL§3, CL§5, ED§1, AU§3, MZ§27, T2§3, T3§3 (32). Effort M each.
- Settles: `core-leaves 1`, `4`; `server-tests-2 4`, `5`; `server-tests-3 2`.

**8. Every route answers every kind of caller exactly as its role may; a read changes nothing; a
change names who made it; a mistake is a 4xx that says why.** *In flux: after the accounts and
access redesign.*
- Test: a table that fails on a route with no row. Seam 1.
- From: T1§4, T1§15, T2§4, T2§6, T2§19, AL§2, AL§3, AL§4, BM§3, MZ§13, E2E§2 (46). Effort M.
- Settles: `server-tests-2 2`, `3`; `server-modules-a-l 3`, `11`.

### B. Evidence that checks offline

**9. Every run a sample records checks offline from its export.**
- Guards: the second promise, for every sample and every run, not the thirteen that end with it.
- Test: a sweep, `verify` as oracle. (a) inside the server's golden loop: S, about 550 recorded
  cases; (b) a row per sample that needs a binding: L.
- From: T1§13, T2§15, T3§14, E2E§3 (31, and the same six lines ending 26 more).
- Done in part (dc88fe26, 2026-11-11), for the kinds of run: `verify` finds the handler of a run
  of every kind by its kind and label, a resumed part's included
  (`a_run_of_every_kind_finds_the_handler_it_ran_by_its_kind_and_label`), and each kind's own
  test ends with `replays_as_it_verifies`. Left: (a) and (b).

**10. An export the server wrote always checks; one changed in any way no longer does, but for
what is listed as taken on trust.**
- Test: property by mutation over exports of every sample; every way of checking a chain finds
  the same breaks; `verify` runs again exactly the units the server runs.
- From: T2§5, T3§12, AL§16, MZ§25, MZ§26 (12). Effort M to L.
- Settles: `core-engine-b 11`.

**11. For every schedule of what can happen to a log, it reads as a plain list of its runs, its
chain breaks exactly where it was changed, every run is in the log or the backup once, and a run
is recorded as its level says.**
- Test: one model with failures put in: seven kinds of step (write, sync, cut back, seal,
  restart, a write or sync that fails, a change from outside), six invariants. It stands in for
  16 of `recorder.rs`'s 21 single-scenario tests and widens its four oracle tests.
- From: BM§6, BM§7, T1§7, T1§11, T3§10, MZ§17, AL§14 (37). Effort L; needs a sync that can be
  made to fail.
- Settles: `server-big-modules 1`; `server-modules-m-z 2`.

**12. A replay says of each run what deploying the version would do, and a recorded run is shown
as it was recorded.**
- Test: the handler grid of `semantics.rs` as generator, deploying as oracle; the editor's replay
  over the golden record's runs.
- From: T1§12, T2§16, ED§11, AU§13 (46). Effort M to L; seam 6.
- Settles: `author 1`.
- Done in part (dc88fe26, 2026-11-11), for the kinds of run: a replay finds every kind's handler
  through `verify::handler_of`, and `replays_as_it_verifies` is the oracle each kind's test ends
  with. Left: the handler grid as generator, and the editor's replay over the golden record.

### C. Done once, nothing lost

**13. Runs at once over what they share leave what some one-at-a-time order leaves, each call
out is made once, and nothing waits for a key it doesn't touch.**
- Guards: state, files, rows on each engine, documents, claims, the three places the lock rule is
  written.
- Test: model with drawn schedules and faults; truth is a plain map in commit order, the far
  side's counts and `verify`. Seam 4.
- From: T1§9, T2§10, T3§7, AL§11, AL§12, AL§13, HM§8, HM§10, HM§12, HM§17, CB§8 (53). Effort L.
- Settles: `core-engine-b 4`; `server-big-modules 6`.
- Done in part (2026-11-11), seam 4 for the tests that said "nothing waits" by the clock: a
  `Gate` (`gated_counting_server`, tests.rs) holds `/slow` calls until the test opens it, and the
  four tests that judged by elapsed time judge by what was out at once. One of them failed on CI
  (998 ms, its bound 780) with the `semantics.rs` sweep on every core beside it. The model, and
  the tests still ordered by a sleep, are open.

**14. For every trigger with a far side, each item starts one run, in order, whatever is sent
again, dropped or cut; and what a run sends after it goes once its record is on disk, once.**
- Guards: files, mailboxes, feeds, WebSockets, channels, queues, the witness.
- Test: model with failures over the stand-ins already there. Seam 5.
- From: T3§8, T3§11, MZ§14, MZ§23, AL§8, AL§9, AL§10, AL§18, AL§23 (40). Effort L.
- Settles: `server-modules-m-z 5`, `6`, `7`; `server-modules-a-l 7`.

**15. A deployment is what a plain model of its history says (deploys, runs, ticks, pauses,
stops, restores, restarts), runs exactly when nobody holds it, and a dry run says what the real
one then does.**
- Test: model, with restart as the fault, checked for every way a run starts. Seams 2, 3.
- From: T1§5, T1§6, T1§10, T1§14, T2§11, T2§12, T3§9, AL§17, BM§11, E2E§4, E2E§8 (48). Effort L.
- Settles: `server-tests-1 1` to `6`.

**16. Every file the server keeps reads back as last written whole, or is said; never taken as
empty, never written over, and its owner's alone.**
- Test: a table made a sweep (every file found in a used data folder must have a row), each row
  damaged every way; a model with failures for the value store and its packs, and for the secret
  store.
- From: T1§8, T1§20, T3§6, AL§7, AL§29, MZ§15, MZ§16, MZ§18, HM§4, HM§7, HM§9 (52). Effort M to L.
- Settles: `server-modules-a-l 4`, `5`, `13`, `14`; `host-module-wasm 5`.

**17. Whatever the far side does, a connector ends within its time, says which failure it was,
holds no more than its bound, and never says its login.**
- Test: a table of connector × fault on one scripted far side. Seam 5; needs timeouts a test can
  set, and a TLS stand-in.
- From: AL§26, MZ§5, MZ§20, MZ§22, MZ§30, T1§18, T2§7, T2§22, T2§23, T3§18 (37). Effort L.
- Settles: `server-modules-a-l 6`.

### D. The answer is right

**18. A handler does and answers what JavaScript does with its text, for every kind of handler,
and every surface treats each kind as a handler.**
- Test: `a_handler_does_what_its_text_says` widened: the answer and each effect's payload
  compared, a handler axis (action, WebSocket block, clock, file, mail, feed, queue), more effect
  rows, `throw` and `finally`; and a table with a row per kind of handler across lowering,
  printing, `check`, `show`, the graph, the trace and `explain`.
- From: T2§13, T3§16, AU§6, AU§7, HM§11 (44). Effort M to L.
- Settles: `author 4`.

**19. Every leaf and expression form that mirrors JavaScript gives what JavaScript gives, but
for the differences written down once; on both targets.**
- Test: a sweep over the catalog with an oracle per row (the JavaScript engine Holon ships; Node's
  frozen answers for dates), failing when a leaf has no source of truth; one digest pinned across
  native and wasm32.
- From: CL§14, CL§15, CL§16, CL§17, AU§5, HM§13, HM§14, CB§12, T3§13 (125). Effort L in all; by
  family, texts and conversions S, the rest M. Seam 6.
- Settles: `host-module-wasm 1`, `6`, `7`; `core-leaves 8`.

**20. For every graph and every way the host answers, a run asks for exactly what it needs and
gives what evaluating every node gives; an absent input gives nothing; depth and sharing cost
what they should.**
- Test: the eager evaluator given the same table of answers, with a small model of need; a sweep
  of all 195 leaves × absent inputs (it joins `audit_leaves.rs`); every closure walk against a
  plain walk, with a count of visits.
- From: CA§1, CA§2, CA§3, CA§7 to §11, CA§13, CA§14, CB§6, CB§17 (50). Effort L.
- Settles: `core-engine-a 1`, `4`, `5`; `core-engine-b 2`; `host-module-wasm 3`, `4`.

**21. A cache gives back only what running again gives, under a key shared exactly by the same
unit and inputs; a fork is the same graph with one reference swapped.** *After the decision on
code only tests call.*
- From: CA§4, CA§12, HM§5, HM§6, HM§18, T1§16, T3§17, CB§18 (65). Effort M. Seam 6.
- Settles: `core-engine-a 3`.
- T1§16 (the registry and the cache over HTTP) goes with `/registry*` and `/cache*` once they are
  removed (§8, 2).

**22. A unit's hash moves when what it computes or promises does, and at no other time; its
bytes read back as written.**
- Test: property. Rewrites of presentation keep the hash and the evaluation; a row per field
  changes it; grouping, templates and the editor's documents on the same generator.
- From: CB§4, CB§13, CB§14, CB§16, CL§11, ED§6, ED§7, ED§9, ED§15 (69). Effort M. Seam 6.
- Settles: `host-module-wasm 2`.

**23. Each format reads back what it writes and takes exactly its standard.**
- Guards: decimals, CSV, XML, gzip and zip, the encodings and digests, mail out, document
  templates, sizes and spans of time, dotenv, positional parameters.
- Test: per format, an inverse and the standard's vectors or a whole-number model.
- From: CL§4, CL§6 to §10, CL§12, CL§13, CL§18, AL§28, MZ§19, MZ§28, MZ§33, AU§8, CB§15 (71).
  Effort M each.
- Settles: `core-leaves 3`, `6`, `7`; `server-modules-m-z 4`; `core-engine-b 10`; `author 2`.

**24. Every program the language takes prints back as itself, formats to one text, and is
refused, when it is, at the place and in words that say what to write.**
- Test: text to graph to text to graph on generated programs, by unit hash; `fmt` a fixed point
  whatever the layout; every place the reader refuses reached by a row (a table holds 50 today;
  the reader refuses in about 324 places and nothing says which are reached); every form listed
  once and held on every surface.
- From: AU§9 to §12, AU§14 to §19, BM§8, BM§9, BM§15, HM§15, HM§16, ED§2, ED§18, ED§19 (123).
  Effort M to L.
- Settles: `author 3`, `5`, `6`, `7`.

**25. A flow on any engine answers as it does on SQLite, but for what is listed.**
- From: T2§14, AL§19 (6). Effort M; the engines are beside CI.
- Settles: `server-modules-a-l 8`; `core-engine-b 8`.

**26. A request reaches the route a plain matcher picks, with its record what the request said,
and is answered through the control plane as on the port.**
- From: T2§17, T2§18, AL§21, AL§22, CB§11, ED§13, ED§16 (25). Effort S to M.
- Settles: `server-tests-2 1`; `core-engine-b 6`; `editor 1`, `2`, `3`.

### E. Tools and screens

**27. Every tool an agent calls that says it only reads sends only reads; every failure is a
failed tool in a sentence.** From BM§4, BM§10, BM§20, MZ§24 (9). Effort S, then M. Settles
`server-big-modules 4`, `5`.

**28. Every command reads its arguments and prints what the server really answers; no argv and
no answer makes one panic.** From AL§30, MZ§34, E2E§10, T2§24, BM§19 (48). Effort M; the
commands' `run` taking its server as an argument.

**29. The terminal workbench: its text is a plain text edited so, every screen draws at every
size in every state, any keys leave it standing.** From BM§12 to §14, BM§16 to §18, BM§21 to §24
(27). Effort M to L. Settles `server-big-modules 2`, `3`.

**30. The editor says what the server says: one shared table per rule written in both
languages; a review shows exactly what changed; the timeline holds each thing once; tidy places
every node.** From ED§3, ED§8, ED§10, ED§12, ED§14, ED§17, ED§20 to §26 (about 110). Effort L in
all, S to M each. Settles `editor 4` to `9`.

**31. End to end holds what needs a browser, and no more.** The sweep over every sample sends a
request and checks its runs offline; every page opens at its address, survives a reload and fits
at every width; what a person typed stays. Of the 53 per-sample tests, 37 check only what the
server does after the deploy and 29 have a Rust twin: about 30 go or leave the browser once
classes 9 and 12 are in. From E2E§5 to §7, §9, §11 to §14 (about 60). Effort M each.

**32. Bindings are made or refused by one written rule; every limit holds at its edge; log lines
stay within their caps; what the host says it holds is what is there.** From AL§15, AL§24, AL§25,
AL§27, MZ§21, MZ§31, MZ§32, T1§17, T1§19, T1§21, T2§20, T2§25, T3§20, T3§21 (39). Effort M.
Done in part (ee5a768c, 2026-11-11), for the control plane's bodies: every one is read by the
deadline (`every_body_the_control_plane_reads_is_read_by_the_deadline`), and one that stops or
trickles gives its place back
(`a_body_that_stops_or_trickles_gives_its_place_back_by_the_deadline`).

## 5. Redundant now

Dropped in step 0. Where the last column names a literal outside what the covering test draws, it
becomes a fixed row of that test first. *Done (41910d98, 6f7065ad): all 18 dropped.*

| Test | Covered by | Outside it |
|---|---|---|
| `eval_composite_add` (eval.rs) | `lazy_and_eager_agree_on_generated_graphs` | inputs 3, 4; one child |
| `eval_nested_composite` | same | literals |
| `none_propagates` | same | `subtract` where it has `add` |
| `eval_composite_multi_output` | same | three outputs |
| `multi_output_partial_success` | same | `divide` by zero as the reason |
| `promoted_input_routes_straight_to_output` | `giving_and_lending_agree_with_eager_on_generated_graphs` | a composite with no child |
| `map_doubles_list`, `map_none_propagates` | same | the function `x * 2` |
| `fold_sums_with_add`, `fold_on_empty_is_init`, `fold_none_propagates` | same | `add` as the function |
| `deterministic` | `deterministic_over_random_graphs`, `differential_record` | literals |
| `golden_nested_arithmetic` (determinism.rs) | `deterministic_over_random_graphs` | the constant 10 |
| `map_is_pure`, `pure_leaf_declares_empty_manifest` (kind.rs) | `pure_leaves_are_pure` | nothing |
| `store_read_write_have_a_valid_default_key` (catalog.rs) | `the_two_catalogs_agree` | the default key's text |
| `audit_64_deep_composite_evaluates_on_a_2mib_thread` (audit_security.rs) | `a_composite_nested_as_deep_as_a_host_runs_fits_half_a_threads_stack` | nothing: the same unit on a looser bound |
| `every_sample_has_no_mistakes` (editing.rs) | `fmt_is_stable_and_every_sample_round_trips_through_new` | nothing |

## 6. Holes in the class-level tests that exist

Each is small, and each closed makes tests beside it redundant. Step 0.

- **The differential test** runs 300 cases in CI, worlds 0 and 1 only, so unvalidated edges never
  run there; it never asserts that the eager oracle ran (it is behind a feature); mismatches in
  odd-interface cases are listed, not failed. Its three generators hand-list 9, 35 and 55 of the
  195 leaves.
- **Plan against walk** draws numbers only: no giving, no requests, no nested functions.
  `giving_and_lending…` draws no `const`, so never a pass-through guard.
- **The edge-table test**, drawing 8 children and 16 edges, covers
  `the_flat_index_finds_what_a_map_of_the_edges_finds`. *Closed (7909066d), and that test went.*
- **`audit_leaves.rs`**: its pool has no bytes, so the leaves that take them are never swept on
  hostile ones. *Closed (e21ceebf).*
- **`audit_security.rs`** builds units in memory: it reaches neither the decoder nor nested
  composites, higher-order leaves, modules, sinks or requests.
- **`a_staged_unit_loads_or_is_refused_whatever_its_wiring`** draws no leaf with an effect port;
  four leaves make `effect_written_to_state_does_not_take_the_server_down` a row. *Closed
  (597c6bf8), and that test went.*
- **The panic table** has no row for a feed event, a WebSocket block, a queue message or a mail,
  and both its faults come before the run holds anything: none under a state lock, a claim, or
  mid-commit.
- **The log's model** draws no failed sync; with it,
  `sync_logs_syncs_what_was_written_and_keeps_a_log_that_fails_dirty` is covered. *Closed
  (cd8a89a5), and that test went.*
- **`a_handler_does_what_its_text_says`** compares what a handler did, not what it answered, for
  routes only (class 18).
- **The server's golden record** drives 40 of 84 samples, routes and actions only; 235 of its 552
  cases are refusals.
- **The end-to-end sweep** deploys every sample and never sends one a request; sending one makes
  eight per-sample tests redundant. `e2eSweep.test.ts` should ask for a deploy, not a click.
  *Closed (de6a3962, b991fa30): the sweep deploys every sample from its text through the command
  line and sends each a request; the per-sample tests the server holds went.*
- **The language crate's hand tables** (17) should each fail when a form the reader takes has no
  row.

## 7. Suspected bugs

Reading every test against its code turned up 101 suspected bugs, none run, none fixed. They are
in `docs/L4s/test-suite-review/suspected-bugs.md`, by part, as each reviewer wrote them. Eleven
were read again against the code afterwards, and the code is as each says; that file lists them.

By what a break would cost, roughly: about 20 touch what is kept secret or who may do what; about
16 lose or double something (a run, a mail, a file's contents); about 14 can stall or stop a
server or one of its threads; the other half are wrong answers, wrong words, the editor and the
server disagreeing, or defects in tests themselves.

They are not a second worklist. Each is a case of a class in §4, cited there under "Settles", and
is fixed where every case of its class passes: the class's test first, failing; then the fix.

The file, and the reports that cite it, stay out of the repository while it is public and those
entries are open.

## 8. For the owner to decide

1. **Compatibility held by tests.** About 13 tests hold formats "from before": a grant's
   migration, stored tokens, five old formats in `tests.rs`, the log's pre-chain entries,
   `migrate.rs`, two in `secrets.rs`, the editor's `withTriggers`. The dev-phase rule says a
   design replaces outright: drop the tests with the code?
2. **Code only tests call.** `Memo`, `StateHost`, `Mode`/`run`, `adoption` and `Recorded` in
   holon-host (11 to 13 tests), and the routes `/registry*`, `/cache*` and `/fire` that no client
   calls (8 tests). Keep, or delete before class 21? *The routes decided (2026-11-11): removed,
   in the principals work, and their tests with them. The holon-host code is still open.*
3. **`-0`.** The value encoding folds it into `0`; `same_value` and `atan2` tell them apart.
4. **A waiting run goes on while its deployment is paused or stopped.** Meant?
5. **`a_resumed_run_resumed_again_after_its_commit_applies_once` pins an outbound call going
   twice**, where `approved-payout` says nothing is paid twice.
6. ~~**A write that fails still advances the chain's head.** Meant?~~ *Decided and done
   (2026-11-11): the chain stays on the last line written, its head goes to the witness only
   then, and the next line says from which run on the lines before it were lost (`lost_since`):
   `verify` reads them as lost, not as a break. Lines lost just before a restart are said by
   nothing, as a crash's are.*
7. **An agent's version that drops an `await approval(…)`** appears to deploy at once.
8. **Tests that need what CI has and a laptop hasn't**: engine tests that pass having checked
   nothing; end-to-end tests that call httpbin.org.
9. **How much stays in a browser** (class 31).
10. ~~Still open from the first batch of fixes: a guard that rejects on a condition lets the
    request through when the value is missing.~~ *Decided and done (948ee310): an if on a
    condition with no value runs neither branch, and nothing after an if that may return runs.*

## 9. How a step goes

1. Take the next class. Read its clusters in the reports.
2. Conceive the generic test: its domain, how it is listed or generated, and what says the right
   answer (an oracle, a model, an inverse, an invariant; a table when there is none).
3. Write it, with a doc that says **Guards**, **Why**, **If it fails**. Its name is the property,
   as a sentence.
4. Run it. What it finds is fixed where every case passes, not where the found case does. Show
   that it fails on the code before each fix.
5. Check which existing tests it covers. Their cases become its rows; the tests are dropped, or
   lose the lines it now holds.
6. Record it in [Testing](../engineering/themes/testing.md) §2 under what it holds, with what it
   replaced and what it found. One commit.
