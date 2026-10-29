# History: runs and deploys as one chain, a diff before approval, and reverts as new links (L4)

**Status:** design, before any code. Written 2026-10-29, against rc.29 (6bc5c68). Step 2.5 of
the [host capabilities](host-capabilities-L4.md) plan (§7, Tier 2). Builds on
[host resources](host-resources-L4.md) (resources, addresses, a timeline per resource),
[databases](databases-L4.md) (one transaction per run, claims, locks) and
[triggers](triggers-L4.md) (actions).

A run is already much like a git commit: it is named by the version it ran on and the Merkle
root of what it started from, it is checked against what it read before it lands, it goes round
again when that changed, and its log line points at the line before. This note makes the
likeness exact, and takes the two things it is good for:

- **a diff before approval:** a new version is run against the recorded runs of the old one, and
  the approver sees what it would have done differently, on real traffic;
- **reverts:** undoing a run's writes, a deploy, or both back to a point, **always as a new link
  in the chain**, never by rewriting it. Like `git revert`, not `git reset`.

```
$ holon history payouts --last 6
#4131  run      POST /payouts         v 3f2a…  root 9c1e… → 7d40…  wrote payouts/881
#4132  deploy   v 3f2a… → v b81c…     by agent:codex, approved by maria
#4133  run      POST /payouts         v b81c…  root 7d40… → 12aa…  wrote payouts/882  sent 1
#4134  run      POST /payouts         v b81c…  root 12aa… → e0f3…  wrote payouts/883  sent 1
#4135  revert   restore to #4131      by maria  "b81c doubles the fee"
         code v b81c… → v 3f2a…; state e0f3… → 9c5b… (payouts/882, payouts/883 as they were)
         compensated: 2 runs (refund_payout #4136, #4137)
```

---

## 1. What is already there

- **A run's id** is `run_id(deployment, input_root)` (`holon-core/src/value_hash.rs`): the hash of
  the unit it ran on and the Merkle root of its inputs, the state it read, its sourced values and
  a nonce. **A state's root** is `state_root`: one hash for a key → value map, from each value's
  hash, so a state whose values are hashed once when written gets its root without reading them.
- **A run's log line** (`recorder.rs`, `LogEntry`) holds `prev` (the hash of the line before: a
  hash chain per deployment, checked when the log is opened), `seq`, `unit`, `grant` (its hash),
  `state_before` (**the whole state, as values**), `reads`, `rounds`, `sourced`, `outputs`,
  `performed` (the effects, a run's writes among them: a collection write with its value, a file
  write by its hash), `retries`, and for durable runs `parked` and `events`. There is **no state
  after**: what the run left is `state_before` with `performed` applied.
- **Deploys are elsewhere.** The activity log (`activity.rs`) keeps `{at, action, target, by}`
  for each save, deploy, grant and delete: no hashes, no chain, no unit. A deployment keeps the
  documents of its last 10 versions (`deploy.rs`, `KEPT_VERSIONS`); every unit itself stays in
  the store by its hash.
- **History is short.** A deployment's log keeps its newest 500 runs (`DEFAULT_KEEP`, or
  `--keep-runs`). File texts a run read are kept in `blobs.rs` and swept when no kept run refers
  to them; their log lines keep the hash, so such a run "stays provable, not re-runnable".
- **`holon verify`** (`verify.rs`) checks an export offline: the chain, then each run again from
  its line (same unit, state, clock, random, outbound answers, reads), comparing run id,
  outputs, effects and status. It checks runs one by one; it does not check that one run's state
  follows from the last's.
- **Replay** (the editor's view mode) runs a logged run again from its line, on the version it
  ran on.
- **Optimistic commits** (`firing.rs`): a run whose reads changed before it could land runs again
  from the state as it now is, its outbound requests answered from its own record (`prior`),
  four times, then once more holding the lock.

## 2. One chain per deployment

A deployment's history is one hash chain of **links**, in the file its run log is today. A run
log line becomes the commonest link; deploys, grant changes and reverts join it.

| Link | Changes | Holds |
|---|---|---|
| `run` | state | today's line, with `root_before`, `root_after` and its `writes` (§3) |
| `deploy` | code | the unit before and after, the grant's hash, who deployed it, who approved it, and its diff (§5) |
| `grant` | permissions | what was granted, lowered or revoked, by whom |
| `revert` | state, code or both | the links it undoes, by whom, why, what it couldn't undo (§6) |

Every link has `prev`, `seq` and `at`, as a run's line has now. The activity log keeps its own
entries as today (it is the server's, across deployments); a deploy or grant is written to both.

**The rule that makes it a chain of states:** each link's `root_before` is the last link's
`root_after`, except where runs committed side by side, which §3 orders.

## 3. A run names its states

- **`root_before`** replaces `state_before`. Replay needs only what the run read, and `reads`
  and the state cells it read are kept with their values; the rest of the state is named by the
  root, not copied. A busy deployment's log gets smaller.
- **`writes`**: what it changed, key by key, each the value's hash (and the value, where the log
  keeps values today).
- **`root_after`**: `root_before` with `writes` applied, computed by `state_root` without
  re-reading anything.
- **Order.** Runs commit one at a time under the deployment's state lock (`firing.rs`), so their
  commits already have an order: the link is written in that order, and each `root_before` is
  the root it committed on. A run that started on an older root and committed anyway (its reads
  unchanged) names the root it committed on; its line also keeps the root it started from, so
  replay starts there.
- **Older lines** (without roots) stay as they are; the chain continues from them, and verify
  checks roots from the first line that has them.

## 4. Checking a whole chain

`holon verify` gains a second check, beside running each run again: **each link's states follow
from the one before.** Starting from a **checkpoint** (a link whose whole state the export
carries, by default the oldest kept), it applies each link's writes and checks the root it
reaches against the link's `root_after`. A run whose state doesn't follow is reported, whatever
its own run said.

**Keeping history long.** State values are kept once each, by hash (as `blobs.rs` keeps file
texts), so keeping every link's state costs only what each link changed. A link past the
deployment's retention keeps its **header** (roots, writes' hashes, `prev`) and drops its
**body** (values, reads, rounds): the chain still checks, the link is no longer re-runnable, and
the report says which. Retention becomes a real setting, because it is also how far back a
revert can reach (§6).

## 5. A diff before approval

When a new version waits for approval (any version an agent deploys that needs one, or one a
person asks to see), the server replays recorded runs of the current version **on the new one**,
and shows what would have been different:

```
v b81c… against the last 500 runs of v 3f2a…:
  463  the same
   34  answer differently        POST /payouts: 34 (fee: 2.00 → 4.00)
    2  would write differently   payouts/…: fee
    1  can't tell                POST /payouts: asks https://fx.example.com, which v 3f2a never did
```

- **Each run is replayed from its own line**, independently: its request, the state it read, its
  clock and random, its outbound answers. Nothing compounds from one replayed run to the next, so
  they replay in parallel, within a time budget.
- **Compared:** status, outputs (the response first), writes, and effects (what it would send,
  and where).
- **"Can't tell"** is a run where the new version asks something the record doesn't hold (a call
  to a new host or path, a read of a key the old run never read, a new secret). It is never
  guessed; it is counted and shown.
- **Runs that touched secrets** compare as verify does today (§1): what a secret-computed node
  produced is taken from the log.
- **Where it shows:** the deploy approval (the editor's deployment card, `holon deploy` waiting),
  with each differing run openable side by side in view mode. The deploy link keeps the summary.
- **Which runs:** the last *N* (500 by default), or those a person picks (a route, a time range).

This is the evidence an approver of an agent's change lacks today: not only what it may touch,
but what it does differently with the traffic it will get.

## 6. Reverts

A revert is a link. It never removes or changes one.

**Three forms:**

- **`holon revert <slug> --run <seq>`** (or a range, `--runs 4133..4134`): the runs' writes are
  undone, newest first.
- **`holon revert <slug> --deploy <seq>`:** the version before that deploy is deployed again.
- **`holon restore <slug> --to <seq>`:** code and state as they were after that link, in one
  link.

**State.** For each key a reverted run wrote, the key gets back the value the run found, **if it
still holds what the run left**. If a later link changed it, that is a **conflict**: the revert
names the later links that touched the key (the resource timeline's blame, host-resources §5),
and either reverts them too (if asked: `--with-later`) or leaves the key and says so. The revert
is applied as a run's commit is: under the state lock, each key checked as expected, so it
never races live runs.

- **State and collections** revert from the chain itself.
- **Files in bound folders** revert when the text they had is still kept (`blobs.rs`); a file
  whose earlier text was swept is left and listed.
- **External databases** revert only with **before-images**: when a run's transaction commits,
  the rows its writes changed are read first in the same transaction and logged (or kept by hash),
  as a database's own undo log does. That costs a read per write, so it is an admin's setting per
  database (`--revertible`, §11, fork 4). Without it, a database's writes are listed, not undone.

**Code.** Reverting a deploy is a deploy link of the earlier unit, **checked against today's
policy and grant**, not those it had then: a secret revoked since stays revoked, and a permission
the earlier version needs that isn't granted now waits for approval like any other.

**Effects outside: compensation, not undo.** A sent mail or a made payout can't be unwritten.
A flow may name an **action that compensates** a route's effects:

```holon
route POST /payouts (req) {
  …
}

action refund_payout (original) compensates POST /payouts {
  http.post("api.bank.example", "/refunds", body: { payout: original.outputs.id })
}
```

The action receives the reverted run's record (its request, outputs and effects). A revert runs it
once per reverted run, each a new run link, with the revert link naming them. Effects with no
compensating action are listed in the revert link for a person to handle. Compensations are runs:
logged, granted, verifiable.

**Who may revert.** Reverting writes to a deployment, so it needs the role a deploy needs, and
where the policy says so (payments), an approval by someone else (approvals-L4, step 2.1). An
agent may revert as its grant allows; its revert waits for approval like its deploys.

**A preview first.** Every revert can be asked with `--dry-run` (the editor shows it before the
button acts): which keys change, which conflict, which files and rows can't be restored, which
compensations would run, which effects nobody compensates.

## 7. Erasure

An append-only chain and a person's right to have their data deleted meet here. Rewriting history
would break every later hash, so it isn't how Holon erases:

- **Values an admin marks personal** (a collection, or fields of one) are kept **encrypted**, each
  subject under its own key, sealed as waiting runs already are (`sealing.rs`). The chain hashes
  what it holds, so it never depends on the clear value.
- **Erasing a subject deletes the key.** The chain still checks; the values are gone; replay, the
  diff and revert treat such a value as erased and say so, as they treat a swept file's text.
- **Card data** (PCI) should never enter a run's log at all: the existing secret rules (`taint.rs`)
  are the place for it, not erasure after the fact.

## 8. What it doesn't do

- **No merges.** Runs that conflict run again; they are never merged. Merging two decisions would
  give a result neither run computed.
- **No rewriting.** Erasure is by deleted keys (§7), retention by dropped bodies (§4); every hash
  stays.
- **No undo of the outside world.** Effects are compensated by actions or listed for a person
  (§6), never claimed undone.
- **No time travel of external state.** A database, a bucket or a mailbox is seen as the runs saw
  it; only before-images (§6) let Holon put rows back.

## 9. Seeing it

- **`holon history <slug>`**: the chain, link by link, filtered by kind, key or time.
- **The editor:** the deployment's history as a timeline of runs, deploys, grants and reverts; a
  key's history (who wrote it, when; host-resources' timeline); a version's diff (§5); a revert's
  preview and its button.
- **The MCP tools** gain `history` and `revert`, the latter waiting for approval when an agent
  asks.

## 10. Samples

- **`payout-revert`:** a payouts flow with a compensating action. Deploy v1, send payouts, deploy
  a v2 that doubles the fee (its diff shows it before approval), approve it anyway, send more,
  then restore to before v2: code back, state back, the payouts v2 made refunded by the action.
- **`notes-history`:** a collection flow whose history, blame and single-run revert show in the
  editor, with a conflict when a later run touched the same key.

Each deployed by the e2e tests, with a local stand-in for the bank.

## 11. Slices

1. **Roots in run links:** `root_before`, `root_after`, `writes`; `state_before` reduced to what
   was read. Verify checks roots where lines have them.
2. **Deploy and grant links** in the deployment's chain.
3. **Whole-chain verify** with checkpoints; headers and bodies, retention.
4. **The diff before approval** (§5), in the deploy approval and `holon deploy`.
5. **Reverting state:** runs and ranges, conflicts, files from kept texts, `--dry-run`.
6. **Reverting code and restoring to a link.**
7. **Compensating actions.**
8. **Before-images** for databases marked revertible.
9. **Erasure** by sealed personal values.

## 12. Forks, surfaced

1. **Diff runs independently, or in sequence on a branch of state.** In sequence, each replayed
   run would read what the previous replayed runs wrote, closer to "what if v2 had been live", but
   one difference changes every later run, and it can't run in parallel. *Lean:* independently.
2. **Whether a deploy needs its diff.** *Lean:* shown whenever a version waits for approval; the
   policy may require it (an agent's version can't be approved without one).
3. **Conflicts in a range revert:** stop at the first, or undo what can be undone and list the
   rest. *Lean:* the dry run shows both; the revert does what can be done and lists the rest.
4. **Before-images per database or per table.** *Lean:* per database, an admin's setting.
5. **What a compensating action receives:** the reverted run's whole record, or only its outputs
   and effects. *Lean:* outputs and effects, plus the request; never its secrets.
6. **Retention:** by count of links, by age, or by bytes. *Lean:* by age (what an audit asks
   for), with a count as a floor.
7. **A revert of a revert.** It's a link like any other, so it can be reverted. *Lean:* allowed.

## 13. What stays true

- History is appended to, never rewritten: every link's hash stays valid for as long as it's
  kept.
- A revert is a link, with who, why and what it couldn't undo.
- Code comes back checked against today's grant, never yesterday's.
- What happened outside is compensated or listed, never claimed undone.
- What can't be checked or restored is said, never guessed.
