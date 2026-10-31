# History: one timeline per deployment, and restoring code, state or both (L4)

**Status:** design, before any code. Written 2026-10-29, against rc.29 (6bc5c68); narrowed the
same day to observability and restore, leaving out per-run reverts, conflicts and compensations
(§8). Step 2.5 of the [host capabilities](host-capabilities-L4.md) plan (§7, Tier 2), taken
ahead of Tier 1. Builds on [host resources](host-resources-L4.md) (a timeline per resource).
Slices 1 and 2, restoring code and restoring state, and slice 3b, events, are built (§10).

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
  after; the log keeps the newest 500 runs (`--keep-runs`; since slice 4 the deployment's
  retention, §7).
- **Deploys and grants** are in the activity log (`activity.rs`): `{at, action, target, by}`, no
  unit hash, no chain, across all deployments.
- **Sessions already keep states** (`lifecycle.rs`): stopping a deployment ends its session and
  keeps the state it ended with, by reference (values in the content store); running it again
  starts from the latest state, from nothing, or from any session's snapshot. That is a restore of
  state at the points where a session ended, which §4 extends to any entry.
- **The editor's view mode** opens a logged run on the version it ran on (the kept document).

## 2. One timeline

A deployment's timeline is its runs and its **events**: everything a person, an agent or the host
did to the deployment, each with who and when. So the timeline answers why a run behaved as it
did (a secret replaced, a database rebound, a limit lowered) and who changed what the flow may do,
in a record `holon verify` checks offline.

| Entry | Holds |
|---|---|
| `run` | as today, plus the state it left (`state_after`) |
| `deploy` | the version it made, and what it restored, if it was a restore of code |
| `restore` | the state put back: the point (`run 41`, `version 3`) and the session it started |
| `grant` | what was revoked, granted back or lowered, and keep-running set or cleared |
| `secret` | a secret set, replaced, removed or rolled back: its name, version and source kind (value, env, vault), never a value or a fingerprint |
| `binding` | a folder or database bound or unbound for this deployment (`--for SLUG`) |
| `lifecycle` | paused, resumed, stopped, run (and from which state) |

Left out: deploy settings (interval, port, limits asked, browsers, which actions agents may run),
which each version's deploy already holds; saved drafts and accounts, which aren't the deployment
and stay in the activity log; bindings for every deployment, which stay there too (the runs
record what they read through them). The activity log stays as it is, across deployments; an
event is written to both.

**Recorded, never restored.** A restore puts back code and state only. Grants, secrets and
bindings are always as they are today: restoring a version from before a revoke must not grant
back what a person took away. The restore's dry run says what differs.

**Events have a log of their own,** `<slug>.events.jsonl` beside the run log, hash chained the same
way (`seq`, `prev`), and kept for good: events are few, a person's actions, while runs are many
and the run log keeps the newest 500. One chain for both would mean dropping runs from the middle
of it, which needs headers kept for every run trimmed (§7), and would put non-runs in front of
everything that reads runs today (replay, verify's re-run, the editor's runs, MCP's `runs`). Each
event names the run log's next seq when it happened (`run_seq`), so the two merge into one
order: event 7 came after run 40 and before run 41. The witness gets each event log head too, and
an export carries both logs.

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
  of deploys (approvals-L4, step 2.1), a restore asks for one too. An agent sees a restore only as
  its dry run (built: the server refuses it more), and tells the person, who restores it. *Later:*
  an agent's restore saved for a person to approve, as its deploys are.

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
- **MCP:** `history` and `restore` (built), the latter a dry run only.
- **`holon verify`** checks that each run's state follows from the one before (built): it began
  from the state the run before it left, and its writes applied to the state it began with give
  the state it left, each state by its snapshot's hash, made again from `state_before`. A
  collection by its root (a run that wrote one is checked in a later run's state), and runs that
  ran beside another as far as their commit shows (firing.rs).

## 7. Retention

Points are kept as long as the timeline keeps their entries.

- **Deploys and restores are kept for good**, with the state they left: those are the points
  people go back to ("the version that worked, as it was"), and they are few. They are events
  (events.rs), which no retention cuts.
- **Runs** are kept by the deployment's retention (built): two limits, `keep_runs` (the newest
  that many; the host's `--keep-runs`, 500 without it) and `keep_run_days` (none older; any age by
  default), set in the deploy settings and held to the host's policy, as every limit is: fewer than
  the default is the author's to choose, more asks for approval (the host's disk), past the
  ceiling (20000 runs, ten years) is refused. A grant lowered later holds the log at once. A run
  past it is cut from the front of the file, which keeps its newest line so numbering and the
  chain go on; the chain checks from the oldest kept, and a witness hears a `trim` (heads.rs).
- **Later:** a run past retention keeping its header (seq, `prev`, roots, unit) so the chain checks
  across it, which needs a line's hash to cover its header and its body's hash rather than the
  line; and sweeping a state no kept entry names from the store.
- **Retention from the text** (built): `keep 5 runs for 1d`, a header line for the two limits, so
  `holon mcp` and `holon new --sample` carry it; the deploy settings' two fields follow it.

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
   other, checked against today's grant, with a dry run. Agents can't restore yet.
2. **Restoring state** (built), narrower than first written:
   - **Snapshots by hash, not by root.** A run's line names the state it left (`state_after`): the
     hash of a record, cell by name to value hash, in the content store, with only the values it
     changed put there. A version keeps the state it was deployed with (`state_at_deploy`), and
     the log's seq at its deploy (`first_seq`). `state_before` stays a copy, so export, verify and
     replay are unchanged; a Merkle root per entry, and verify checking one from the next, wait
     for slice 4.
   - **A state restore is a new session, not a new kind of entry.** The session ends as a stop
     would, keeping the state it replaces, and a new one starts from the point (`from: "run 41"`,
     `"version 3"`). Sessions already were the record of where a state came from; deploy, grant
     and restore entries in the chain move to slice 3, with the timeline that shows them.
   - **Points are a version (`--to N`) or a run (`--to-run SEQ`).** A version's state is the state
     it left, just before it was replaced (fork 3): the next version's `state_at_deploy`. A run's
     code is the version it ran on, skipped when that is the code deployed now.
   - **Both** is the code (a deploy), then the state. A run that lands between the two is replaced
     with the rest; the dry run counts the runs since and their effects outside (§5) by
     capability.
   - **Running or paused only:** a stopped deployment runs from a session's state instead.
3. **Seeing it**, in two parts:
   - **3b, events** (built; first, so the timeline is built once with every kind): the event log of §2,
     `GET /deployments/<slug>/events`, `holon history <slug>` merging events and runs, exports
     carrying events and `holon verify` checking their chain.
   - **3a, the editor's timeline** and restore button, MCP's `history` and `restore`, agents
     restoring, a cell's history, in three steps:
     - **the editor** (built): view mode's history button beside the runs' filter: versions
       and events (runs when asked) merged newest first, filtered by kind and by words; any row picked shows its details,
       and restores its code, its state or both (a dry run, then a dialog that says what changes
       and approves what the code needs, as a deploy's does): a run or a version itself, anything
       else how it was right after (the version then; the state the run before it left, or the
       restore before it put back, which its event names as `from`); ↺ on a row restores what it
       can; a version's canvas shows. The price-rollback e2e restores through it;
     - **MCP and agents** (built): a `history` tool, the timeline as `holon history` prints it,
       and a `restore` tool that is the dry run only: the code's text beside today's (a line
       diff, names on both sides or neither), the permissions, the state's keys and collection
       items now → then, the runs since and the files they wrote. The server answers an agent's
       restore no more than that; a person restores it. An agent's restore waiting for a
       person's approval, as its deploys do, is later;
     - **a cell's history** (built): the runs that changed a cell, a collection or one item of
       it, with values before and after (`GET …/changes`, changes.rs): walking the log in order,
       a run whose snapshot holds another value than the last one changed it, so a run that only
       read it while another wrote isn't named; a value moved between runs where an event (a
       restore, a deploy) came between is a row of its own. `holon history --cell`, MCP's
       `history` with a cell, and ⧗ in view mode's flow state. The sample balance-trail.
4. **Retention and verify** of whole states.
     - **retention** (built): `keep_runs` and `keep_run_days`, limits in the deploy settings (§7),
       and the text's `keep` line (`keep 5 runs for 1d`). The sample rolling-log keeps 5 runs, a
       day at most.
     - **verify of whole states** (built): each run began from the state the one before left, and
       left what its writes make of it (§6). The sample ledger checks it.

## 11. Forks, surfaced

1. **Whether a restore of code alone is allowed when the state's cells don't match the older
   code.** *Lean:* allowed, with the differing cells in the dry run.
2. **Retention:** by count or by age. *Taken:* both, each a limit, with deploys and restores kept
   for good.
3. **Whether `--to` a version means just before it was replaced, or just after it was deployed.**
   *Lean:* just before it was replaced: the version as it had run.

## 12. What stays true

- A version that ran is never lost; going back to it is one entry.
- History is appended to, never rewritten.
- Code comes back checked against today's grant.
- What happened outside Holon is listed, never claimed undone.
