# History: one timeline per deployment, and restoring code, state or both (L4)

**Status:** design, before any code. Written 2026-10-29, against rc.29 (6bc5c68); narrowed the
same day to observability and restore, leaving out per-run reverts, conflicts and compensations
(§8). Step 2.5 of the [host capabilities](host-capabilities-L4.md) plan (§7, Tier 2), taken
ahead of Tier 1. Builds on [host resources](host-resources-L4.md) (a timeline per resource).
Slice 1, restoring code, is built (§10).

Two things, and only two:

- **See what happened:** one timeline per deployment of its runs, deploys, grant changes and
  restores, each with the code it ran on and the state it left.
- **Go back:** restore a deployment's **code**, its **state**, or **both**, to any earlier point,
  so a version that worked is never lost. A restore is a new entry on the timeline, never a
  rewrite of it; it can itself be restored past.

It is not git: nothing branches, nothing merges, nothing is undone run by run. Going back means
putting the whole deployment, or its code, or its state, where it was at a point in its history.

```
$ holon history prices --last 5
#212  run      POST /quote          v 3f2a…  state 9c1e… → 9c1e…
#213  deploy   v 3f2a… → v b81c…   by agent:codex, approved by maria
#214  run      PUT /prices/eur      v b81c…  state 9c1e… → 7d40…  wrote prices
#215  run      POST /quote          v b81c…  state 7d40… → 7d40…  500
#216  restore  to #212 (code and state)   by maria  "b81c writes prices in cents"
        code v b81c… → v 3f2a…   state 7d40… → 9c1e…
        not restored: 0 effects outside since #212
```

---

## 1. What is already there

Most of what this needs is in the store already; what's missing is keeping it and naming it.

- **Code is kept by hash.** A deployed version is a unit in the content store, and stays there.
  What isn't kept for long is its **document** (the flow as the editor opens it): a deployment
  keeps its last 10 (`deploy.rs`, `KEPT_VERSIONS`).
- **State is a map of cells, and collections are cells too.** A collection's root hash is a state
  cell under `#collection:<name>`, its trie's nodes in the content store (`collections.rs`). So
  a collection is restored by putting its root back, if its nodes are still there; the content
  store isn't swept today.
- **A state has one hash:** `state_root` (`holon-core/src/value_hash.rs`), from each cell's value
  hash. A run's id covers it.
- **The run log** (`recorder.rs`) is a hash chain per deployment (each line's `prev`, checked
  when the log is opened). Each line holds its unit, its grant's hash, `state_before` (the
  whole state map, copied) and `performed` (its writes and other effects). There is no state
  after; the log keeps the newest 500 runs (`--keep-runs`).
- **Deploys and grants** are in the activity log (`activity.rs`): `{at, action, target, by}`, no
  unit hash, no chain, across all deployments.
- **Sessions already keep states** (`lifecycle.rs`): stopping a deployment ends its session and
  keeps the state it ended with, by reference (values in the content store); running it again
  starts from the latest state, from nothing, or from any session's snapshot. That is a restore of
  state at the points where a session ended, which §4 extends to any entry.
- **The editor's view mode** opens a logged run on the version it ran on (the kept document).

## 2. One timeline

A deployment's run log becomes its timeline. Next to runs, it gets **deploy**, **grant** and
**restore** entries, in the same hash chain, so everything that changed the deployment's code,
permissions or state is in one ordered, checkable place.

| Entry | Holds |
|---|---|
| `run` | as today, plus the state root it left (`state_after`) |
| `deploy` | the unit before and after, the grant's hash, who deployed it, who approved it |
| `grant` | what was granted, lowered or revoked, and by whom |
| `restore` | the point restored to, what (code, state or both), by whom, why, and what it didn't touch (§5) |

Every entry has `seq`, `at` and `prev`, as a run's line does. The activity log stays as it is
(the server's own, across deployments); a deploy or a grant change is written to both.

## 3. Restore points

Every entry is a point the deployment can go back to: its code is the unit current then, its
state the root it left.

- **Each state is kept by its root.** A state map is stored in the content store as one record,
  cells by name to value hashes, and its values by hash, so a state shares everything it didn't
  change with the one before. An entry names its state by root; a run line's `state_before`
  stops being a copy.
- **Each document is kept by hash** with its unit, not only the last 10, so any version on the
  timeline opens in the editor and can be deployed again.
- **Collections need nothing new:** their roots are cells, their nodes are in the store.

What a point costs is what changed at it. Reaching it costs nothing: a restore reads a root and
puts it back.

## 4. Restoring

```
holon restore <slug> --to <seq>              code and state, as they were after entry <seq>
holon restore <slug> --to <seq> --code       the code only
holon restore <slug> --to <seq> --state      the state only
holon restore <slug> --to <seq> --dry-run    what it would do, and change nothing
```

`--to` also takes a version (`--to v 3f2a…`: the point just before that version was replaced).

- **Code** is restored as a deploy of the earlier unit, with everything a deploy does today
  (waiting runs, routes, channels), and **checked against today's policy and grant**, not those it
  had: a secret revoked since stays revoked; a permission it needs that isn't granted now waits
  for approval like any new version's.
- **State** is restored by setting the deployment's state to the point's map, collections
  included, under the deployment's state lock, as one commit: runs in flight that read the old
  state run again on the restored one (the optimistic commit already does this, `firing.rs`).
- **Both** is one restore entry, code first, then state, with nothing running in between.
- **Code alone over a newer state** is allowed; the newer state may not suit the older code
  (a cell it doesn't know, a shape it doesn't expect). `--dry-run` shows the cells that differ
  between the two points, so whoever restores sees it.
- **Who may restore:** whoever may deploy to the deployment. Where the policy asks for an approval
  of deploys (approvals-L4, step 2.1), a restore asks for one too. An agent may restore as its
  grant allows; its restore waits for approval like its deploys.

## 5. What a restore doesn't touch

A restore puts back what Holon holds: the code and the deployment's state, collections included.
Everything else belongs to whoever holds it, and isn't put back:

- requests sent, mail sent, messages published on channels;
- rows in external databases, files in bound folders;
- secrets (their own versions, `secrets.rs`) and grants (today's apply, §4).

The restore entry and `--dry-run` list the runs since the point that had such effects, by kind
and target, from their `performed`: so whoever restores knows what the outside world saw, and
Holon never claims to have undone it.

## 6. Seeing it

- **`holon history <slug>`**: the timeline, filterable by kind, route, cell, collection, version
  and time; `--json` for tools.
- **A cell's or a collection's history:** the entries that changed it, by whom and on which
  version, with the values before and after (host-resources' timeline, for the deployment's own
  resources).
- **Two versions side by side:** the documents of any two units on the timeline, as text.
- **The editor:** the deployment's timeline, each entry opening its run in view mode, its
  version, or its state; a restore's preview and its button.
- **MCP:** `history` and `restore`, the latter waiting for approval when an agent asks.
- **`holon verify`** checks, beyond what it checks today, that each entry's state follows from the
  one before it (its writes applied to the root before give the root after), from the oldest
  entry the export carries whole.

## 7. Retention

Points are kept as long as the timeline keeps their entries; the log's 500-run window is what
bounds that today.

- **Deploys and restores are kept for good**, with the state they left: those are the points
  people go back to ("the version that worked, as it was"), and they are few.
- **Runs** are kept by the deployment's retention: the 500 newest, or a setting by age. A run past
  it keeps its header (seq, `prev`, roots, unit), so the chain still checks, and drops its body;
  a state no kept entry names can be swept from the store.

## 8. Not in this design

Left out on purpose; each would need conflict rules, ordering or semantics that aren't Holon's to
decide:

- **Reverting one run or a range** while keeping later ones: a state no run computed.
- **Merging** two states or two versions.
- **Compensating** effects outside (refunds, retractions): a flow's own business, in its own
  routes and actions.
- **Before-images of external databases**, to put their rows back.
- **Rewriting history**, to erase or shorten it: retention drops bodies (§7), never entries.
- **A diff of a new version against recorded runs** before approval: a fit for approvals-L4, if
  it is wanted.

## 9. Samples

- **`price-rollback`:** a prices flow with a collection. Deploy v1, set prices, deploy a v2 that
  writes them wrong, then restore to before v2: code and state back, the timeline showing all of
  it. Deployed by the e2e tests, which restore it (through the server's endpoint until the
  editor's button, slice 3) and check a quote again.

## 10. Slices

Reordered so restoring code, which changes nothing already logged, comes first:

1. **Restoring code** (built). Every deploy is a numbered version kept for good, with its canvas
   (by hash, in the content store) and what its deploy asked; `GET /deployments/<slug>/versions`;
   `POST /deployments/<slug>/restore` and `holon restore <slug> --to N --code`, a deploy like any
   other, checked against today's grant, with a dry run. Restore by version number for now: the
   timeline's entries (`--to <seq>`) come with slice 2. Agents can't restore yet.
2. **Restoring state:** roots in run lines (`state_after`, `state_before` by root), states kept by
   hash; deploy, grant and restore entries in the deployment's chain; `--state`, both together,
   and the dry run's list of effects outside.
3. **Seeing it:** `holon history`, a cell's history, the editor's timeline and restore button,
   MCP's `history` and `restore`.
4. **Retention and verify** of whole states.

## 11. Forks, surfaced

1. **Whether a restore of code alone is allowed when the state's cells don't match the older
   code.** *Lean:* allowed, with the differing cells in the dry run.
2. **Retention:** by count or by age. *Lean:* by age, with deploys and restores kept for good.
3. **Whether `--to` a version means just before it was replaced, or just after it was deployed.**
   *Lean:* just before it was replaced: the version as it had run.

## 12. What stays true

- A version that ran is never lost; going back to it is one entry.
- History is appended to, never rewritten.
- Code comes back checked against today's grant.
- What happened outside Holon is listed, never claimed undone.
