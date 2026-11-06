# Concepts

The ideas a person changing Holon has to hold, each named as the code names it and placed where it
lives. The user-facing version is the manual's [Concepts](../../manual/concepts.md); this one says
what each idea is in the code, and which doc goes deeper. The invariants they serve (I1–I9) are in
[COMPOSABLE_VM_DESIGN §3](../../COMPOSABLE_VM_DESIGN.md).

## 1. What it is

Holon runs flows. A flow is a graph of **units**, content-addressed and pure, evaluated by one
core ([holon-core](../components/holon-core.md)), compiled native for the server
([holon](../components/holon.md)) and to wasm for the editor
([holon-wasm](../components/holon-wasm.md)). Everything a flow does outside its inputs is a
**capability** read off the graph, granted by a person under the host's **policy**, and performed
by the host ([holon-host](../components/holon-host.md)) only after the run settles. Each run is
kept in a hash-chained **log** that anyone can check offline by running it again through the same
core. The words below are the pieces of that sentence.

## 2. The concepts

### The graph

**Unit.** `holon_core::Unit` (`hash.rs`): a body, params, input ports and output ports. The body
is `UnitBody::Primitive { name }`, `Composite { children, edges }` or `Module { bytes }` (a
WebAssembly binary with no imports, [holon-module](../components/holon-module.md)). A leaf, a
holon, a whole flow and the js engine are all units. A unit never changes: a change is a new unit,
and changing one deep in a flow re-pins the spine above it (`refork`, `fork.rs`). A `Param` is
frozen, or an open hole that makes the unit a template until `instantiate` fills it.

**Primitive (leaf).** A body named from the catalog (below). Most are pure: given the same inputs
they give the same output. Some build a request the host answers (`http_request`,
`collection_get`, `await`); some are **sourced**, their value supplied by the host before the run
(`clock`, `random`, `secret`, `file_arrival`, `store_read`; `Capability::is_sourced`);
the **sinks** (`TRIGGERED`, `catalog.rs`) build an effect.

**Holon (composite).** `UnitBody::Composite`: children referenced by hash only, never inlined
(I3), wired by edges. Its interface is derived from the wiring once, when it is grouped
(`derive_and_freeze`, `encapsulate.rs`), and frozen into its hash (I4). A primitive and a composite
are evaluated through the same `eval` (I1), so a holon placed in a flow is one node like any leaf.
A flow is a composite too: a server declaration, children its handlers, carrying
`declare = "server"` (`read_server`, `declare.rs`).

**Port.** `Port { name, ty }`. Names are part of the contract and of the hash. Types are `Type`
(`Number`, `Boolean`, `Text`, `List<T>`, `Product`, `Sum`, `Record`, `Effect`, `Any`), checked when
wired by `compatible`. Every primitive has one output, `out`; a composite may have several (P4.7).

**Edge (wire).** `Edge { from_child, from_port, to_child, to_port }`, by child index in the
composite's canonical order. `BOUNDARY` (`u32::MAX`) as a child is the composite itself: its inputs
come in through it, its outputs leave through it. An input port takes at most one edge (`validate`).

**Value.** `holon_core::Value` (`value.rs`): `Number` (one `f64`), `Boolean`, `Text`, `List`,
`Record` (fields in key order), `Sum` (`some`, `none`) and `Effect`. There are no exceptions: what
can't be computed is **absent** (`None`, `Slot::Absent`), and absence propagates. Within a round, an
answer the host hasn't given yet is **pending** (`Slot::Pending`), not absent
([lazy-choices-L4 §4](../../L4s/lazy-choices-L4.md)).

**Resolver.** The `Resolver` trait turns a hash into a unit; it is how a run's world is put in
without core reading it. The store's resolver, `MapResolver`, holon-host's `SourcedResolver`
(stands a sourced leaf in as a constant), design mode's `Simulated` (mocks drivers), and
`WithEngine` (runs modules) all wrap the same trait.

### Identity and storage

**Hash and content addressing.** `hash_unit` is BLAKE3 of `encode_preimage`: a versioned
(`ENCODING_VERSION`), tagged, length-prefixed encoding of everything that changes a result, child
hashes included, so a composite's hash pins its whole closure (I5, I6). Positions, labels and the
order children or edges were written in are left out. A unit's hash is its version. Values hash
the same way (`hash_value`), a state by its keys and value hashes (`state_root`), a run by its
deployment and what it started from (`run_id`), each under its own domain tag so no two kinds
collide (`value_hash.rs`). Changing the encoding changes every hash, run id and state root.

**The store.** `holon_host::Store`: `put` and `get` for units, `put_value` and `get_value` for
values, every read checked by re-hashing (`StoreError::Corrupt`). `FileStore` keeps one file per
unit named by its hex hash and values under `values/`, long-lived ones packed (`packs.rs`);
`MemStore` is the same in memory. The server shares one (`SharedStore`) with a flow cache and a
value cache in front (`units.rs`), and sweeps values nothing kept names (`sweep.rs`). See
[Storage](../capabilities/storage.md).

### Deploying and running

**Deployment.** A unit running on a server under a slug: the server's `Deployment`
(`crates/holon/src/main.rs`) holds its unit hash, its triggers, its routes (`Server`, from
`read_server`), its state, its `grant` and `permit`, its secrets, its parked runs, its `log`
(`LogCell`) and its versions. Its lifecycle (`Lifecycle`) is `Running`, `Paused` or `Stopped`
(`lifecycle::Status`), and each start begins a `Session`. Redeploying is a new unit hash, a new
version; the state carries over.

**Trigger.** Two things in the code. What starts a deployment's runs: a route on its port
(`Route`, answered by `dataplane.rs` and `listeners.rs`), a clock (`Clock`, `scheduler.rs`), a
file handler (`FileHandler`, `arrivals.rs`) or an action run by name (`Action`); `manifest::trigger`
names the capability that says so. And `TRIGGER`, the input every sink takes last (`catalog.rs`):
present but never read, it holds a sink back until what starts it has arrived. See
[Triggers and clocks](../capabilities/triggers-and-clocks.md).

**Run.** One evaluation of a deployment's unit for one request, tick, file or action
(`firing::fire_with_state`). It reads everything at its start (its sourced values and a snapshot of
the state it uses), and evaluates in **rounds** (`holon_host::run_rounds`): requests a round asks
for are performed, then the flow is evaluated again with their answers, until a round asks for
nothing new. That round's outputs and effects are the run's; its writes commit together at the end,
so a run never sees its own writes. A run whose collection read changed meanwhile is run again
(`claims.rs`). A run held at an `await` parks and is resumed by running it again from the start,
its earlier answers taken from its record (`durable.rs`). Its limits are `holon_host::Limits`. See
[Outbound requests](../capabilities/outbound-requests.md) and
[Durable runs](../capabilities/durable-runs.md).

**Run record and the log.** Each run's `LogEntry` (`recorder.rs`) holds what it read (request,
`sourced`, `state_before`, each round's exchanges), what it gave (`outputs`, `performed`, the
`timeline` of each node's outputs) and what it ran under (`unit`, `grant`, `run_id`, `nonce`). One
JSON line per run in the deployment's `<slug>.jsonl`, each line's `prev` the hash of the line
before: a hash chain whose newest hash is the head, sent to a witness when one is set
(`heads.rs`). How a handler is recorded is its `LogLevel` (`strict`, `lazy`, `unrecorded`). Secrets
are redacted from every line, and what is computed from them withheld (`taint.rs`). `holon verify`
(`verify.rs`) checks the chain and runs every run again through the same core. See
[Run records and verify](../capabilities/run-records-and-verify.md),
[Performance](performance.md) and [Resilience](resilience.md).

**State cell.** `store_read` (sourced) and `store_write` (a sink), each with a frozen key, giving
`StateRead(key)` and `StateWrite(key)`. The host's `State` (`holon-host/src/state.rs`) is a Merkle
map, key to value and its hash, its root `state_root`; the server shares it between a deployment's
runs as `claims::StateCell`, a key a run read before calling out held for that run until it
commits ([key-claims-L4](../../L4s/key-claims-L4.md)). Persisted by reference: each value
in the store, the deployment holding the key → hash map
([P2.6.1-state-cell-L4](../../L4s/P2.6.1-state-cell-L4.md)). See
[State and collections](../capabilities/state-and-collections.md).

**Collection.** A keyed, ordered set of records: `collection_get` and `collection_list` are requests
answered from the collection as the run found it; `collection_put` and `collection_delete` are
effects committed when it ends. Kept in the store as a radix trie whose shape follows from its
keys alone (`holon_host::collection`), so a collection is its root hash and every version stays
readable. The server's side is `collections.rs`.

### What a flow may do

**Capability.** `holon_core::Capability` (`kind.rs`): what a leaf touches outside its inputs, most
named by what they touch (`HttpRequest("hooks.slack.com")`, `CollectionWrite("notes")`,
`SecretRead("stripe")`). `capabilities` derives a unit's from its closure, following children and
`map`/`fold` functions; nothing declares them (I8). An unknown leaf or a child that doesn't resolve
gives every capability. `Kind` (`Pure`, `Driver`, `State`, `Sink`) is their coarse projection. A
capability's consent name is its `Debug` form (`consent_name`, `manifest.rs`).

**Manifest.** What a deployment asks for: its capabilities, then the limits and exceptions its
author asks (`manifest_items` in core; `permissions::Manifest` in the server, content-addressed).
`holon check` prints it; the editor shows it through the bridge, by the same rules.

**Policy.** `permissions::Policy`, the host operator's, from `holon.toml`: each limit's default and
ceiling, what is granted without asking (`auto`), whether plain HTTP, private addresses or
unrecorded handlers may ever be granted, and how storage is kept.

**Grant.** `permissions::Grant`: what a person approved for one deployment, for one manifest hash,
persisted with it, with every later change (`revoked`, `lowered`, `changes`). A redeploy asks only
for what it doesn't cover; one that no longer covers its manifest pauses the deployment unless
`keep_running`.

**Permit.** `permissions::Permit`: the grant applied to the policy in force, what every run is held
to: its items, limits, outbound hosts and the handlers it keeps unrecorded. An effect outside it is
refused when it is performed, whatever the graph says. See [Secrets](../capabilities/secrets.md),
[Folders](../capabilities/folders.md) and [Databases](../capabilities/databases.md) for the
capabilities with names of their own.

**Effect.** `Value::Effect { sink, payload }`: what a sink computes, a request as data. Its type,
`Type::Effect`, fits no sink's payload, so effects flow out and never back in. Core builds and
carries effects; the host performs them (`Host::perform`, `perform_effects`) once the last round
settles, under the permit ([sink-as-pure-request-L4](../../L4s/sink-as-pure-request-L4.md)).

**The catalog of primitives.** `GENESIS` (`catalog.rs`) names the 95 primitives. Each name has its
facts in one place each, all in core: its interface (`interface_of`), its capabilities
(`capabilities_of`), its words (`describe`, `docs.rs`), whether it grows inputs (`VARIADIC`), and
its computation (`compute`, by an `Op` from the `ops!` list) or the request it builds. The editor
reads them through the bridge (`primitive_catalog`, `primitive_docs`, `variadic_primitives`), never
a copy. A new primitive goes through the admission gate (design §13) and ships with a sample
([`CLAUDE.md`](../../../CLAUDE.md)); code the vocabulary can't express is a module, the
[js node](../capabilities/js-node.md).

## 3. Why it is this way

- **Pure in the middle, effects at the edges** (I2, I7). Sourced values enter at the top, effects
  leave at the bottom, state is two fenced leaves. So a run is a function of what its record holds,
  and replay and `holon verify` are exact.
- **Identity is structure** (I5). A hash names what a unit computes, not how it was written, so
  two flows that compile alike are one version, and a stored unit is trusted because it re-hashes.
- **What a flow may do is derived** (I8). The manifest is read off the graph, so it can't be
  understated; the grant is checked when an effect is performed, so the graph can't exceed it.
- **One core everywhere** (I1, I5). The server, the command line, the editor and `holon verify` run
  the same evaluator and encoder; nothing is reimplemented in TypeScript.

## 4. Reach and limits

Where the words mislead:

- **"Trigger"** is both what starts a deployment's runs and the last input of a sink.
- **"Holon"** is the manual's word for a composite; "flow" is a composite that declares a server.
  In the code they are all `UnitBody::Composite`.
- **"Kind"** is the effect bucket (`holon_core::Kind`); the evaluator's `NodeKind` is unrelated, and
  a declaration (`declare = "endpoint"`) is a param, not a kind.
- **"Store"** is the content store (`holon_host::Store`); `store_read` and `store_write` are the state
  cell's leaves.
- **"Recorded"** isn't "on disk": a `lazy` run's line reaches the disk within a second, and an
  `unrecorded` run has no line at all, only a count ([Performance](performance.md#three-levels-of-record)).
- **Pending, rounds, resolvers and sourced values** have no name in the manual; they are how the
  code keeps the run pure while the world answers it.
- **A module is opaque**: it is a unit, pure and pinned, but a replay sees only its inputs and
  outputs.
