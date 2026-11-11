# State and collections

A flow's own data: the `state` cells it uses and the collections it imports, read as a run
begins and written when it finishes. The design is in the
[state cell L4](../../L4s/P2.6.1-state-cell-L4.md) (the two leaves), the
[request path L4](../../L4s/request-path-L4.md) C3 (how a write is kept), the
[key claims L4](../../L4s/key-claims-L4.md) and [PLATFORM_LAYOUT](../../PLATFORM_LAYOUT.md) §4.5 and §4.5.1
(Merkle state, collections, runs side by side).

## 1. What it is

A deployment keeps one keyed state: a key → value map whose values are each hashed once, when
written (`State`, `holon-host/src/state.rs`), so the whole hashes from those (`state_root`) and two
states compare by root. A **cell** is one key. A **collection** is a keyed, ordered set of
records kept as a radix trie in the content store (`holon-host/src/collection.rs`); its root hash
is itself a cell, under the reserved key `#collection:<name>` (`collections::cell_key`), so a
collection is persisted, snapshotted, part of a run's id and in the log's `state_before` exactly
as a cell is. The server's side is `crates/holon/src/collections.rs` (reads, writes, the editor's
view), `changes.rs` (a cell's history), `claims.rs` (`StateCell`: the live state, its claims and
its head) and `firing.rs` (the commit).

In the language ([LANGUAGE.md](../../LANGUAGE.md) §5.1, §5.4, §6.6):

```ts
import { flow, route } from "holon"
import state from "state"
import visits from "collection:visits"
import { now } from "holon:clock"

flow("counter", { port: 8155 })

route.get("/hit", () => {
  const hits = (state.hits ?? 0) + 1
  state.hits = hits // tied to the request
  return `hit ${hits}`
})

route.post("/visits/:id", (req) => {
  const id = req.params.id
  const saved = visits.put(id, now)
  if (saved !== undefined) state.last = id
  return `visited ${id}`
})
```

`state.k` reads `store_read{key: k}` (`StateRead(k)`), `state.k = e` is `store_write{key: k}`
(`StateWrite(k)`); `notes[k]`, `notes[k] = v`, `delete notes[k]` and `notes.list(…)` are
`collection_get`, `collection_put`, `collection_delete` and `collection_list`
(`CollectionRead`/`CollectionWrite(name)`). The state is imported once,
`import state from "state"`, and each key the text uses is one of its state keys; a collection is
imported by its name, and one not imported is refused at the use. The capabilities are derived
from the leaves, never from the imports.

## 2. How it works

- **Read at the start.** A run copies the live state under the cell's lock and lets go
  (`fire_with_state`); every cell it was granted is sourced as `StateRead(key)`. A collection
  read is answered on the spot from the roots as the run found them (`collections::Readable`,
  only for granted collections) and kept with its node's path (`collections::Read`): the log
  shows it, replay answers from it.
- **Writes are effects.** `store_write` is an identity sink (Fork B,
  [sink-as-pure-request L4](../../L4s/sink-as-pure-request-L4.md)); every sink takes the block's
  trigger as its last input, so a write is its route's even when it reads nothing of the request,
  and `if (x !== undefined)` ties it to `x` instead (LANGUAGE.md §5.11). A key may be written once
  per run: `state.k is written twice (first on line n)`.
- **The commit.** Optimistic: a run runs on its copy, then, holding the cell's lock, commits only
  if `still_as_read` holds: each granted cell by its hash, each collection read asked again
  (skipped when that collection's root hasn't moved). Otherwise it runs again from the live state,
  its outbound requests answered from its own record; after `OPTIMISTIC_ATTEMPTS` (4) it runs
  once more holding the lock. `commit` applies each `StateWrite` (`State::insert` refuses a value
  carrying an effect, `is_storable`, as `State::load` does), then each collection put or delete (`collections::apply`, which stores
  only the changed trie nodes), then the run's files.
- **What is kept.** `snapshot_after` saves the changed values and a snapshot record (key → value
  hash) in the content store (`restore::save_snapshot`); its hash is the entry's `state_after`.
  `keep_if_changed` then writes the **state head**, `deployments.state/<slug>` (`state_head_for`),
  holding that hash, whole and synced (`StateCell::write_head`, `write_private`), still under the
  cell's lock so heads land in commit order. A run that changed nothing (`changed_from`: same
  keys, same hashes) writes no head. Load reads the head (`load_deployments`); no head is an empty
  state. (`heads.rs` is another thing: the run logs' heads sent to a witness.)
- **The deployments file** (`persist_deployments`) holds definitions only: a run rewrites it
  only when it parked (`listeners.rs`); a run's counts stay in memory until the next definition
  change writes them.
- **Seen.** `GET /deployments/<slug>/state` (`handle_get_state`), `…/collections/<name>`
  (`handle_get_collection`: by key, or a page of up to 1,000; `?root=` only for a root the log
  shows, `LogCell::shows`), `…/changes` (`handle_get_changes`: the runs that changed a cell, a
  collection or one item, from each entry's `state_before` and `state_after`, `SCAN` 2,000 entries
  back), `holon state`, and the editor's Flow state pane. All redacted (`Redactor::of`).

## 3. Why it is this way

- **Two leaves, no cycle.** A counter looks like a loop (read → +1 → write); with read and write
  separate leaves, storage between rounds breaks it, and `eval` and `validate` are untouched
  ([state cell L4](../../L4s/P2.6.1-state-cell-L4.md) §0).
- **Keyed cells, and collections beside them.** A growing set kept in one cell is read and
  rewritten whole by every run. The trie is canonical (the same items give the same root), and a
  write stores only the nodes on its key's path (PLATFORM_LAYOUT §4.5.1).
- **Runs side by side, not a lock per deployment.** A run waiting on an outbound call held up
  every other run of its deployment; now only conflicting runs run again. A run about to call
  out claims the collection keys it read and may write, so two runs over one key call out once
  ([key claims](../../L4s/key-claims-L4.md) §2). Never a state cell: claiming one would serialize
  every calling run of the deployment, which the first draft did and a test caught (§2 there).
- **A head per deployment, written only on change** (C3). The whole 1.1 MB deployments file was
  rewritten and synced on every stateful run: check `/last` 79 → 14,150 req/s for (a), live
  `/hit` 74 → 986 for (b)–(e) ([Performance](../themes/performance.md), RP C3). A head names a
  snapshot already durable, so a crash between the two loses nothing answered (RP I-c).
  Collapsing the head into the log line (C3 f) needs a commit ticket and is not built.
- **No migration.** A data folder from before heads loads its deployments empty (RP C3 (d)).
- **Reads that can wait run on the pool** (RA R2): a route reading state or a collection pays
  the hop, 65–145 µs at one connection ([Resilience](../themes/resilience.md#g2-a-deployment-cant-slow-another)).

## 4. Reach and limits

- **A write is three syncs** (value, snapshot, head) under the cell's lock; `counter /hit` is
  about 700–1,400 requests a second, bound by them ([Performance](../themes/performance.md),
  limits). No group commit across writes (C4).
- **One state per deployment.** No store shared between deployments; a shared one is a
  [folder](folders.md) or a [database](databases.md).
- **Contention costs re-runs.** Runs over one cell or key run again, up to four times, then one
  holds the lock; a state cell is never claimed, so a read-decide-call over a cell keeps one call
  per run, not one per key.
- **Collections are key order only**: no secondary index, no query beyond `prefix`/`after`/`limit`.
- **Disk.** Each write leaves its changed values and a snapshot as files until the sweep, an hour
  later at the earliest: at the soak's 51 writes a second, 7.7 files and about 30 KB each, a floor
  of 5.5–6.4 GB while writes go on (RA, [Resilience](../themes/resilience.md#g9-the-disk-stays-bounded)).
- **Restores** put a state back from a run or version ([history L4](../../L4s/history-L4.md)); a
  history scan reads at most `SCAN` entries a request.
- **A value is data**: no effect, no secret and nothing computed from one is stored
  ([secrets](secrets.md)).

## 5. What the tests hold

- `concurrent_runs_lose_no_update`: 24 runs at once, the count ends at 24.
- `a_slow_run_holds_up_no_other_and_retries_without_resending`: a run waiting on a call holds up
  none, then runs again without sending twice.
- `a_run_writes_only_its_state_head_and_only_when_it_changed_the_state`: reads write no head and
  no run rewrites the deployments file; writing back the same value writes nothing.
- `a_state_is_kept_by_its_head_across_restarts_concurrent_writes_and_a_delete`: 16 threads of
  writes, the head names the live state, every write back after a reload, the head gone on delete.
- `a_collection_keeps_items_by_key_across_runs_and_restarts`, `state_persists_across_a_simulated_restart`.
- `a_state_changed_exactly_when_its_snapshot_hash_moved`, `a_collection_read_is_asked_again_only_when_its_root_moved`
  (firing.rs): the shortcuts stand in for the full comparison.
- In `holon-host`: `equal_contents_have_equal_roots_whatever_the_order`,
  `listing_is_in_key_order_by_page_and_prefix`, `a_write_stores_only_the_nodes_on_its_path` (at
  most 8 new values for a write in 5,000 items), `the_root_follows_the_values_and_nothing_else`.
- `a_cells_history_is_the_runs_that_changed_it_and_a_restore_between_them`;
  `names_collections_and_state_errors` and
  `sinks_are_tied_to_their_block_or_to_what_after_names` (lower.rs).
- **Samples**: `counter` (a sink's trigger, `if (saved !== undefined)`), `likes` (concurrent
  runs), `notes-api`, `heartbeat`, `idempotent` (claims), `basket` (a read inside `map`),
  `balance-trail` (cell history), `price-rollback` (state restore). Every sample deploys and
  answers in `editor/e2e/sweep.e2e.ts`; `balance-trail`, `price-rollback`, `basket` and
  `rolling-log` have tests of their own in `samples.e2e.ts`. What `counter` and `likes` answer is
  the server's golden record.
- **Gaps**: nothing in CI measures a write's cost; the soak's disk floor is checked by hand.

## 6. History

Collections and runs side by side predate this repository's history (CHANGELOG: there by rc.15);
it starts at the rc.29 import, `6bc5c68`.

| Commit | Change |
|---|---|
| `a2861a7`, `ee66b6c` | Every deploy a version; a version's or a run's state restores |
| `1431a3b` | A cell's history names the runs that changed it (`changes.rs`) |
| `1ed17fe` | `holon mcp` reads state |
| `9c5751d` | The header declares the state keys |
| `ff3f8bd` | C3 a: a run persists only what it changed |
| `43ebafc` | C3 b–e: a state head per deployment |
| `6220b50` | The commit check skipped when a collection's root hasn't moved |
| `dc21f76` | R2: a route reading state runs on the pool |
| `6d764f5` | R6: the server sweeps the state values no kept run names |
