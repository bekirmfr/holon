# Key claims: a key read before a call is held until the run commits (L4)

> *crate:* **`holon`** (`firing.rs`: the claim table beside the deployment's state, the wait, the
> re-validation, the commit's respect for claims; `recorder.rs`, `mcp.rs`: the log and `explain`)
> **+ `holon-host`** (`rounds.rs`: one hook before the first round's requests are sent)
> **+ `holon-author`** (`check`'s derived note; the sample) · *pre:* `PLATFORM_LAYOUT.md`
> §4.5.1 (runs side by side, the optimistic commit and its retry) · *inv:* `I7` / `I8` / one
> evaluator · *effort:* **M** (three inches, with the sample)
> *Status:* **designed 2026-10-25**, from a race run live against the `charges` flow (§1); **inch 1
> built the same day** (the hook, the table, the claim, the commit's wait, `claim_wait_ms`, tests 1
> to 7). One change from the design: a claim never names a **state cell** (§2, §3).
> *Pinned:* a claim is **derived, never declared**: nothing in the text, nothing in the manifest,
> nothing to approve · taken **only when a run is about to send its first outbound request**,
> never at a read · **per key**, per deployment · **released** at commit, discard or park · a
> run that would wait longer than `claim_wait_ms` fails as busy rather than piling up.

---

## 1. The bug, as it ran

`charges` (the idempotency sample, §9) reads `charges[key]`, calls out to a processor when there
is no prior record, and writes the result. Two requests with the same key, sent in the same
millisecond, against the deployed flow:

| Run | Read at start | Called out | Commit |
|---|---|---|---|
| 4 | nothing under `k2` | yes, 678 ms | written |
| 5, attempt 1 | nothing under `k2` | **yes**, ~690 ms | discarded: `k2` changed under it |
| 5, attempt 2 | run 4's record | no (`prior ?? fresh` never asks for `fresh`) | written, the same record |

The store ended right: one record, one id, the loser answered `replayed: true`. The log entry for
run 5 says `retries: 1`. But two requests reached the processor. §4.5.1's retry keeps its promise,
*one call per run*: the discarded attempt's call is answered from the record on the retry, not
sent again. What a flow like this needs is *one call per key*, and nothing in the host says that
today. The language can't say it either: a run reads at its start and writes at its end, so no
run can mark a key as taken before it does the work that the mark is meant to protect.

The retry fixes the store. The claim fixes the wire.

## 2. The rule

**A run that is about to send an outbound request first claims every key it has read that it may
write. A claim is held until the run commits, is discarded, or parks. While a key is claimed, no
other run calls out over a read of that key, and no other run commits a write to it.**

Spelled out:

- **Which keys.** The attempt's collection reads so far, as `waits.reads` holds them (`{collection,
  get: key}`), for each collection the grant lets the flow write (`CollectionWrite(name)` in the
  permit). A `list` read of a writable collection claims the collection as a whole (§8, F1). A
  read of a collection the flow never writes claims nothing: the run can't be the writer that
  overtakes anyone. **Never a state cell.** The first draft claimed each state key the flow both
  reads and writes, and the existing test `a_slow_run_holds_up_no_other_and_retries_without_resending`
  failed at once: a flow that counts its runs in `state.count` and calls out would claim `count`
  on every run, and every calling run of the deployment would serialize on it, which is the
  problem runs side by side was built to end. A cell is one per deployment; a collection key is
  one per thing (an idempotency key, an order, a post), and serializing on it is the point. A
  read-decide-call over a state cell (`state.token ?? http.get(…)`) keeps today's behaviour: one
  call per run, the retry keeping one value. The way to bring cells in, if a flow ever needs it,
  is §8 F5.
- **When.** In `run_rounds`, after a round has settled with requests pending and before
  `perform_all` sends them: the first time, and again after any later round that made new reads.
  Never at a read, and never for a run that sends nothing: the `likes` path (read, add, write,
  no call) stays optimistic and lock-free, exactly as §4.5.1 built it.
- **What a claim does, in order.**
  1. Takes the keys in the deployment's claim table, in sorted order. A key held by another run is
     waited for, up to `claim_wait_ms` in all; over that the run fails as busy (§6).
  2. Re-validates the reads it is claiming against the live state (the same check
     `still_as_read` makes at commit, over this subset). If any has changed, it releases every
     key and the attempt ends *overtaken*: `fire_with_state` runs it again from the live state,
     as a retry, with nothing sent. This is the step that closes the window: the run that waited
     for run 4's claim reads run 4's record before it can call anyone.
  3. Holds until the run's outcome: commit, discard, or park.
- **Commit respects claims.** A run that commits a write to a key another run holds waits for that
  claim first (bounded the same way; past the bound it commits anyway and the host logs that it
  did, since the claimer's own commit check still protects the store), without the state mutex,
  then takes the mutex, checks its reads, and commits. So a claimed key is not changed under its claimer by a lock-free run either:
  the claimer's own commit check is a safety net, not the mechanism. Two runs that both call out
  on one key therefore serialize on that key and nowhere else.
- **No deadlock.** A run waits only while holding nothing: when a later round needs a key that
  is held, the run releases all its claims, waits for that key, and restarts the attempt
  (*wait-die*). Its earlier requests are answered from `prior` on the restart, so the release
  costs no second call. Keys are taken in sorted order within one claim, so two runs claiming the
  same set never hold halves of each other's.
- **The locked last attempt.** §4.5.1's fifth attempt, run holding the deployment's state mutex so
  a busy key can't starve it, first drains the deployment's claims (waits for every one, bounded),
  then runs claiming nothing: it owns everything already, and nothing may commit into it.
- **Park and resume.** A run that settles waiting performs nothing and its part ends: it releases
  its claims. A resumed part is pinned to the state it first read and commits unchecked (§4.5.1):
  claims change nothing there, and that pin stays a named hole, not this note's (§8, F2).
- **Clock flows** run one tick at a time on the single scheduler: they never contend, and a claim
  on a clock run is free.

## 3. Derived, not declared (the manifest question)

Whether the flow's text or its manifest should say which keys are claimed was the first question
asked of this design. The answer is **neither**, for three reasons that each stand alone:

1. **A claim is not something the flow does to the outside.** The manifest lists what a person
   approves: hosts it calls, collections it writes, secrets it reads, the budget its code runs
   under (`I8`). A claim adds no reach. It is how the host keeps a promise it already makes, one
   call per run, across two runs on one key. There is nothing to approve, so it is not in the
   manifest, for the reason `Limit(module_fuel)` is and a `js` node's purity is not: one is a
   budget a person grants, the other a property the host derives.
2. **The host already knows everything a declaration would tell it**, and knows it better. The
   grant says which collections and state keys the flow writes. The attempt's reads say which
   keys it read, resolved, after the request's key arrived on a wire. A declaration in the text
   (`lock charges`, say) could name the collection but never the key, and could be forgotten,
   which is the bug in a different place: correctness would be opt-in, and the flow that forgot
   would double-charge with a clean manifest. The language's principle is that what a flow may
   do is read from what it is; this extends it to how the host must run it.
3. **The default is already narrow.** A claim is taken only for a key the run read *and* the flow
   writes, and only when the run is about to call out. A flow that reads and writes a key without
   calling out never claims. A flow that calls out without writing what it read never claims. The
   one shape that claims is the one shape that needs it: read, decide, call, write.

What *is* shown, because an author should see it: `check` (and `show`) print a derived line under
the manifest, in a part headed *while it runs* rather than *it may*:

```
while it runs:
  holds the keys it read in "charges" from its first outbound call until it commits
```

The line is derived from the same two facts (a `http_request` reachable from a `collection_get`
of a written collection, or from a read of a written state key); a flow with no such path prints
nothing. It is information, not a permission, and the deploy dialog does not ask about it.

Refused along the way: a `lock`/`claim` header or leaf (opt-in correctness; and a leaf would be an
effect performed mid-run, the one thing `I7` forbids); a manifest item (nothing to approve;
and policy would have to be able to refuse it, which would mean refusing correctness); a
per-deployment lock for every run that calls out (every key contends with every other, the
"one slow flow blocks every other" problem §4.5.1 was built to end); and claiming at the first
read (every read-modify-write run serializes, and the fast path pays for the slow one).

## 4. Where it lives

```
Deployment.state:  Arc<Mutex<State>>                      as today
Deployment.claims: Arc<(Mutex<BTreeMap<ClaimKey, RunId>>, Condvar)>   new, beside it

ClaimKey = Item { collection, key } | Collection(name)
```

- The table is cloned out of the deployments map like `state` is, and never held across a run.
  Two deployments never share a table.
- `holon-host`'s `Waits` trait (the host's per-run hook that already answers reads and tokens)
  gains `fn claim(&mut self, round: usize) -> Claim { Held, Overtaken, Busy(key) }`, called by
  `run_rounds` after a round settles with pending requests and before `perform_all`. `NoWaits`
  answers `Held`. The pure crate learns nothing about keys or locks: it asks, and acts on the
  answer (`Overtaken` ends the attempt with `RunError::Overtaken`; `Busy` with
  `RunError::Busy { key }`).
- `firing.rs`'s `RunWaits` implements it: it has the reads (`waits.reads`), the permit (for the
  written collections and state keys), the live cell (to re-validate), and the table.
  `fire_with_state`'s loop treats `Overtaken` as a retry that sent nothing (it still counts in
  `retries`, since the run did run again) and `Busy` as the run's error. `commit` is preceded by
  the wait on foreign claims for the keys it writes.
- Release is a guard dropped at the end of the attempt, whatever ended it, so a panic or a budget
  failure can't leave a key claimed. The table holds run ids, so a stale entry is recognisable;
  a restart of the host empties it (nothing was in flight).

## 5. Seeing it: the log, `explain` and `runs`

A log entry gains `claims`, absent when none were taken:

```json
"claims": [{ "key": "charges[k2]", "waited_ms": 681, "held_ms": 2, "overtaken": true }]
```

`explain` prints it with the statement that made the read it protects, and the retry it caused:

```
 8    stored = charges[key]
        ⇒ { id: "4ef256af9fcb", … }
        claimed charges[k2] before calling out: waited 681 ms for run 4, then found it
        written and ran again from there (retry 1 of 1); sent nothing on the first try
```

and on the run that held it: `claimed charges[k2] before calling out: held 678 ms`.

`runs` adds `retried ×1` and `waited 681 ms` to a line that has them, which is also what the race
in §1 needed and the log alone gave: a reader of two same-key runs can tell from the list that
one waited for the other.

Replay and `holon verify` are untouched: a claim changes when a run runs, never what it computes,
and an entry without `claims` verifies as before.

## 6. Limits and policy

- `claim_wait_ms`, the 15th name in `LIMIT_NAMES`: how long one run may wait, in all, for keys
  other runs hold. Default 10 000 ms, ceiling 60 000, askable per deployment like
  `request_timeout_ms`. A run over it fails with `busy: "charges[k2]" is held by another run`,
  answered 503 on a route (the request can be retried by its client, and the flow's own
  idempotency makes that safe), and logged with the key.
- The wait is bounded by the run that holds the key, which is bounded by `run_budget_ms`; so the
  default need only exceed one run's budget to make `busy` mean contention, not slowness.
- No new policy knob switches claims off. A host that wants the old behaviour has it for every
  flow that doesn't call out over a written key, which is every flow that didn't have the bug.

## 7. What it costs

- A run that sends nothing: nothing. No claim is taken, no table is touched.
- A run that calls out on a cold key: one table insert in sorted order, one re-ask of its reads so
  far (the same work the commit does, done once more), one removal.
- Two runs on one key: the second waits for the first to commit, then reads its record and, in the
  idempotent shape, sends nothing. The serialization is per key and lasts one run.
- A later round reading a held key: a restart of the attempt, with its requests answered from
  record. Rare (a key chosen from a response), and never a second call.

## 8. Open forks

- **F1. A `list` read claims the collection.** Coarse, and correct: a listing's answer changes
  when any key in its range does. A flow that lists, calls out and writes serializes with every
  writer of that collection. Narrowing it to the range (`prefix`, `after`, the last key answered)
  is mechanical once a flow needs it; none of the samples does.
- **F2. The resumed part's pin.** A resumed durable run commits onto the live state what it
  computed from the state it first read, unchecked (§4.5.1). A claim can't help: the run parked,
  and nothing may hold a key for a day. The fix is the resumed part re-validating its reads and
  running again from the live state when they changed, the same retry, which `durable.rs`'s
  `Resumption` already carries enough to do. Named here, built when a durable sample needs it.
- **F3. The claim as a value.** `explain` says a run waited; the flow can't. A flow that wants to
  answer *"someone else is doing this, try again"* instead of waiting would need the claim's
  outcome as a value, which is a leaf that performs something mid-run. Refused for now (`I7`);
  the busy error and the 503 are the answer a client sees.
- **F4. Shared stores across deployments** (§4.5.1's "left for later") would need the table to
  live with the store, not the deployment. Nothing here precludes it; the key already names the
  collection.
- **F5. State cells, by dependency.** A cell could be claimed only when a pending request
  *depends* on its read: a data path from `store_read` to the request's inputs, or the request
  inside a branch (`guard`, `coalesce`) whose deciding input does. That is a static property of
  the unit, computable once, and it would leave the counter-and-call shape unclaimed while
  claiming `state.token ?? http.get(…)`. Not built: no sample needs it, and the analysis has to be
  right through holons and the functions of `map` and `fold` before it can be trusted.

## 9. The sample, and what it must show

`charges` becomes the sample **`idempotent`** (`crates/holon-author/samples/idempotent.holon`,
its entry in `samples.rs`): the flow as written in §1's session, with its processor call to
`httpbin.org`. Its entry says what it shows: *an Idempotency-Key answered once; the same key with a
different body refused; a key expired after a day; and two identical requests at once, where the
second waits for the first's claim, reads what it wrote, and sends nothing*. The try-it line is
the two-curl race, and the commit says the race was run and the log showed one call.

## 10. Tests

In `crates/holon/src/tests.rs`, beside `concurrent_runs_lose_no_update` and
`a_slow_run_holds_up_no_other_and_retries_without_resending`, against a local slow server that
counts hits:

1. **`a_racing_pair_on_one_key_calls_out_once`**: two triggers of a read-call-write flow with the
   same key, 10 ms apart; both answer 200 with the first's value; `hits == 1`; the second's entry
   has `retries == 1`, a `claims` entry with `waited_ms > 0` and `overtaken: true`.
2. **`different_keys_never_wait`**: the same with two keys; both call out; neither entry has
   `waited_ms > 0`; the second finishes before the first's slow call does.
3. **`a_run_that_sends_nothing_claims_nothing`**: the `likes` shape under 24 concurrent triggers,
   as today, and no entry has `claims`.
4. **`a_lock_free_writer_waits_for_a_claim`**: a claimer mid-call and a pure read-modify-write
   run on the same key; the writer's commit lands after the claimer's; the claimer's entry has no
   retry; `hits == 1`.
5. **`a_later_round_needing_a_held_key_restarts_without_resending`**: a flow whose second-round
   read is of a key held by another run; the restart's first request is answered from record;
   `hits` counts one.
6. **`busy_after_claim_wait`**: `claim_wait_ms = 50` and a holder that takes 400 ms; the second
   run fails with `Busy`, answers 503, and its entry names the key.
7. **`a_parked_run_releases_its_claims`**: a flow that reads a key, calls out, then awaits; after
   it parks, another run claims the key at once.
8. **`check_names_the_held_keys`** (`holon-author`): `check` on the sample prints the *while it
   runs* line; on `likes`, nothing.
9. **`the_sample_races_live`** (`editor/e2e`): the two-curl race through the deployed sample, one
   call in the log.

## 11. Build order

| Inch | What | Done when |
|---|---|---|
| 1 ✅ | `holon-host`: the `claim` hook in `Waits`, `RunError::Overtaken` / `Busy`, `NoWaits` answers `Held`; `holon`: the table (`claims.rs`, beside the state in `StateCell`), the claim, re-validation, release; `fire_with_state` retries on `Overtaken`; `commit` waits for foreign claims; `claim_wait_ms`; the log's `claims` | tests 1 to 7 (in `tests.rs`, from the flow text through `holon_author::agent::plan`, against a local server that counts hits) |
| 2 | The log's `claims`, `explain`'s lines, `runs`' columns; `check`'s derived line | test 8; the race in §1, rerun, reads as §5 shows |
| 3 | The sample `idempotent` with its entry, run live (the race), deployed by e2e | test 9; `samples.test.ts` builds it; `fmt` leaves it |

## 12. Invariant check

- **I7, the effect boundary:** a claim is performed by the host around a run, never by a node;
  nothing new crosses the boundary in either direction. ✓
- **I8, derived labels:** the *while it runs* line is read from the unit (a request reachable
  from a read of a written key) and the grant, never declared. The manifest is unchanged. ✓
- **One evaluator:** `run_rounds` asks one question at one point and acts on three answers; the
  evaluator computes the same values with or without claims, and a logged run replays and
  verifies the same. ✓
- **§4.5.1's promises hold:** runs of one deployment still proceed side by side except on a key
  one of them is calling out over; a slow call still holds up no run that doesn't need its key;
  no update is lost; a retry still sends nothing twice. ✓
