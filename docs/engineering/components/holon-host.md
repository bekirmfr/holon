# holon-host

The host side of Holon: where evaluation meets the world. It performs what a settled graph
carries, runs a flow in rounds of outbound requests, and keeps units and values in a
content-addressed store. The server ([holon](holon.md)) builds everything else on it.

## 1. What it is

`crates/holon-host`, a library depending on `holon-core` and nothing else but `flate2` (for
packs). The dependency only goes one way: core carries no I/O, and this crate is "the first and
only impurity" (lib.rs's notes); `Host::perform` is the one impure operation. Only the server
depends on it: the editor's bridge (`holon-wasm`) uses the pure core alone.

| File | Holds |
|---|---|
| lib.rs | `Host`, `fire`, `perform_effects`, `fire_sourced*`, `SourcedResolver`, `is_storable`; the `Store` trait, `MemStore`, `FileStore`, `write_atomically`; `Registry`; `Memo`, `SharedCache`; `StateHost`, `Mode` and `run` |
| rounds.rs | `run_rounds`, `Limits`, `Performer`, `Waits`, `Claim`, `Round`, `Exchange`, `Recorded` |
| state.rs | `State`: a deployment's keyed state as a Merkle map |
| collection.rs | a collection as a canonical radix trie in the store |
| packs.rs | `Packs`: long-lived values deflated together in pack files |

## 2. How it works

### 2.1 Performing: the `Host` seam

`fire` evaluates (core's `eval_observed`) and then hands each output to `perform_effects`, which
walks it for `Effect` values (a list of them is a fan-out, performed in order) and calls
`Host::perform` with a capability derived from the sink's name (`parse_sink`: `store_write:key`,
`collection_*:name`, `file_*:folder`, the database writes). The capability comes from what reached
the boundary, so a graph performs only what a leaf it holds could build. `SourcedResolver`
injects a host-supplied value at every sourced leaf (the clock, the request, `StateRead(key)`),
matched by the full `Capability`; a leaf with nothing supplied evaluates to nothing.
`is_storable` refuses an `Effect` as a state value, nested or not: the one rule both write paths
apply (`StateHost::round` here, firing.rs's `commit` in the server).

### 2.2 Rounds (rounds.rs)

`run_rounds` is how a run makes outbound requests while evaluation stays pure. Each round is a
whole evaluation: an `http_request` (or a call to another flow) whose request is ready asks the
observer for its answer; those with none yet are collected, sent between rounds by a `Performer`
(`perform_all`: hosts in parallel, at most `per_host_concurrency` to each, calls to one flow one at
a time), and the flow is evaluated again with the answers. A round that asks for nothing new is
the last, and only its effects are performed; a run still waiting at an `await` performs nothing.
Before anything is sent, `Waits::claim` lets the host claim the keys the run read
([key claims L4](../../L4s/key-claims-L4.md)); a claim overtaken ends the attempt with nothing
sent. Past `max_requests` or `run_budget` the run fails and performs nothing, and no request
outlasts the run (each round's `request_timeout` is cut to what is left). `prior` answers an
earlier part of the same run from its record (a resumed run), and `Recorded` answers a whole run
from its record (replay and `holon verify`).

`Limits` holds the numbers: `request_run` (a 25 s run budget, for a caller waiting) and
`clock_run` (60 s; 50 requests, 6 per host, 10 MiB each way, 10 redirects, `claim_wait` 10 s,
`commit_wait` 2 s), with `ceilings` no grant exceeds. The server applies the grant to them
(permissions.rs's `Permit`).

### 2.3 The store (lib.rs, packs.rs)

`Store` has four calls: `put` and `get` for units, `put_value` and `get_value` for values, each
filed under its content hash. Every read is checked: `decode_verified` and
`decode_value_verified` hash what they decoded and refuse bytes that don't match their key
(`StoreError::Corrupt`), so the store is trusted for nothing. `MemStore` and `FileStore` share that
path and differ only in where the bytes are.

`FileStore` keeps a unit as `<root>/<hex>` and a value as `<root>/values/<hex>`, each written by
`write_atomically`: a sibling `<name>.tmp-<pid>-<n>`, synced, renamed over, so a crash leaves the
old file or the new one. A `put` of a unit writes again each time; `put_value` writes nothing
when the value is there already, as a file or packed. `get_value` reads the value's own file,
then the packs.

**Packs** (packs.rs) hold the values a sweep keeps once they are old (the server's sweep calls
`Packs::pack` and `Packs::rewrite`): a pack is `values/packs/<id>.pack`, `MAGIC`, raw-deflated
blocks of about `BLOCK` (64 KiB) of values, then an index of `ENTRY` bytes a value, written whole
aside, synced and renamed in, the loose files removed only after. The index is read into memory
when the store opens, one per values folder a process (`Packs::shared`), and up to `KEPT_BYTES`
(8 MiB) of inflated blocks are kept. A pack is written with no lock held and its index taken in
after, so reads go on; a rewrite moves the index to the new packs at once. A pack that can't be
read is said (`unreadable`) and the store opens without its values.

### 2.4 State and collections

`State` (state.rs) is a key → value map that knows each value's hash, computed once when it is
written, so its `root` (core's `state_root`) and a comparison of two states cost a hash per key,
not per byte. Each value is shared (an `Arc`): a copy of the state, which every run takes as it
starts, is its keys and hashes, and the run is lent the values it reads (`run_rounds` takes them
borrowed). `State::save` puts each value into the store and returns the `key → hash` map a
deployment's snapshots keep; `State::load` rebuilds one, each value checked.

A collection (collection.rs) is a radix trie stored as values (`{p, v?, c?}` nodes), canonical (the
shape follows from which keys exist, so equal contents have equal roots) and persistent (a write
copies the nodes on its key's path and shares the rest, so every version stays readable).
`get`, `put`, `delete` and `list` (in key order, by prefix and page) load only the nodes they pass.

### 2.5 The registry and the caches

`Registry::publish` stores a unit only after `verify_closure` finds every reference (children and
a `fn` param, core's `child_refs`) in the store, else `PublishError::DanglingReference`;
`Registry::fork` re-pins a reference in a published root and publishes the result; `adoption`
counts citations fresh from the store. The published set is in memory. `Memo` caches the results
of pure units only. `SharedCache` is trustless: `ingest` checks an entry hashes to the key it is
offered under, and `get_verified` returns outputs only if they compute again the same.

## 3. Why it is this way

- **Purity by dependency.** Keeping every effect behind one trait in a crate core never imports
  makes "the core performs nothing" a fact the compiler holds
  ([sink as a pure request](../../L4s/sink-as-pure-request-L4.md),
  [capability model](../../L4s/P2.3.1-capability-model-L4.md)).
- **Verify on read.** A store that proves each read by its hash can be a disk, a cache or a peer
  without trusting it ([registry](../../L4s/P3.1.1-registry-L4.md),
  [store, resolver and fork](../../L4s/P3-store-resolver-fork-L4.md)).
- **Values by hash, state by reference.** A snapshot holds hashes, so a value shared by many
  snapshots and sessions is stored once, and a write stores only what changed.
- **Rounds, not callbacks.** A request is ordinary output data; the host performs it between
  evaluations, so evaluation stays pure and a run is replayable from its recorded exchanges.
- **Packs.** A small value as a file of its own takes a whole disk block: measured once on the
  soaks' data folder, 2,000 values of 358 B each (717 KB) took about 8 MB on disk, and 128 KB
  packed together and gzipped ([storage L4 §4.1](../../L4s/storage-L4.md)). On a copy of the soaks' data folder, 140,026 files
  (562 MB on disk) became one pack of 1.0 MiB (step 10).

## 4. Reach and limits

- **No locking of its own.** `FileStore` takes `&mut self` to put; the server serializes every
  call behind `SharedStore`'s mutex, held across the file read or write
  ([Performance §4](../themes/performance.md#4-reach-and-limits)).
- **Files take the process's umask.** `write_atomically` creates its temporary file with
  `File::create`, not owner-only as the server's `sealing::write_private` does, and syncs the file
  but not its directory ([Security §4](../themes/security.md#4-reach-and-limits)).
- **A unit put is a file written**, every time, even when it is there.
- **Packed reads cost an inflate.** With no value cache, a deployment's `notes` listing answered
  636–672 requests a second from loose files, 74–82 from a pack keeping 16 inflated blocks, and
  811–825 keeping 8 MiB of them; with the server's default value cache, 2,201–2,355 (storage L4,
  step 10).
- **The registry's published set and both caches live in memory**: a restart forgets them, not the
  units. `Memo` caches the top-level unit only.
- **`StateHost` keeps only `StateWrite`** from what a round performs and drops every other effect;
  the server doesn't use it for that reason (main.rs, inch 11), nor `Mode` and `run`, nor `Memo`:
  those are held by this crate's tests.
- **Rounds are threads.** `perform_all` sends a round on scoped threads: one per host, and under
  each up to `per_host_concurrency` at once; nothing is async.

## 5. What the tests hold

- The store: `store_rejects_a_corrupted_blob` and `a_file_store_keeps_values_apart_and_refuses_tampered_ones`
  (bytes that don't hash to their key are `Corrupt`), `store_is_content_addressed`,
  `file_store_round_trips_on_disk`, `write_atomically_replaces_and_leaves_no_temp_files`.
- Packs: `packed_values_read_back_as_put_and_a_rewrite_leaves_the_rest`;
  `a_rewrite_hides_no_value_it_keeps_and_keeps_one_put_again` (a thread reading kept values
  through a rewrite of 4,000 finds each every time); `a_pack_cut_short_is_said_and_the_others_read`.
- Rounds: `dependent_requests_take_a_round_each_and_only_the_last_round_performs`,
  `a_run_waits_performing_nothing_and_finishes_when_its_event_arrives`,
  `the_request_budget_fails_the_run_and_performs_nothing`, `the_time_budget_fails_the_run`,
  `a_rounds_requests_to_one_host_respect_its_concurrency_limit`,
  `calls_to_one_flow_go_one_at_a_time_in_order`, `a_request_in_a_branch_not_taken_is_not_sent`.
- State and collections: `the_root_follows_the_values_and_nothing_else`,
  `saving_and_loading_round_trips_and_shares_values`,
  `equal_contents_have_equal_roots_whatever_the_order`, `a_write_stores_only_the_nodes_on_its_path`.
- Performing: `effect_is_never_stored_and_never_panics`, `map_fans_out_one_egress_per_element`,
  `pure_graph_performs_nothing`.
- The registry and caches: `dangling_child_is_refused`, `dangling_fn_param_is_refused`,
  `fork_repins_and_republishes`, `impure_unit_is_not_cached`, `get_verified_catches_a_liar`,
  `ingest_rejects_a_mis_keyed_entry`.

Gaps: nothing crashes a write between its sync and its rename, or checks a file's mode; no test
runs two `FileStore`s on one folder at once (the server never does); the packs' read cost is
measured by hand, not held by a test.

## 6. History

The history in this clone starts at `e49e371`; the crate is older (its inches are in lib.rs's
notes and the P2–P3 L4s).

| Commit | Change |
|---|---|
| `a16e2ba` | `FileStore::values_dir`, for the store's sweep |
| `b17cbbb` | Hex from a table, everywhere (request-path 12.4 a) |
| `440eca0` | A composite's shape kept across runs (interpreter path, step 1b) |
| `ece0790` | A node's inputs lent, not copied (interpreter path, step 3) |
| `f47c854` | Sourced values made into constants only when a leaf reads one |
| `5b35b42` | Packs: the values a sweep keeps, deflated together |
| `91cc5e2` | A pack that can't be read is said, and the store still opens |
| `a6babdf` | Packs written with no lock held, the index moved at once |
