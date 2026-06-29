# L4 · `drive` — source the world (the sourced resolver, generalized)

> **Status:** design, pre-build. Short — `drive` is mostly *recognition*: the machine already
> exists. **Host-only; no core change.** Surfaces one keying choice (per-capability, anonymous).
> **Pre:** the state cell (`P2.6.1` — `StateResolver`, the round), `is_sourced`.

---

## 1 · The move is already built

The state cell's `StateResolver` substitutes every `store_read` leaf with a constant of the
host's storage — a leaf that has no pure `compute` (it's *sourced*) gets its value injected at
resolve time. `drive` is the **same move with a wider key**.

Grounded: `is_sourced()` is `Clock | NetworkIngress | FileRead | SecretAccess | Random |
StateRead`. So the sourced set is exactly **drivers ∪ {StateRead}**. `StateResolver` keys on
`StateRead`; a driver resolver would key on `Clock` (etc.). They are not two mechanisms — they
are one resolver that injects a host-supplied value at *any* sourced leaf. The only difference
is **where the value comes from**: `StateRead` from the host's *internal* storage (threaded
across rounds), a driver from the *external* world (sampled fresh each round).

So `drive` doesn't add a mechanism. It **generalizes `StateResolver` into a `SourcedResolver`**
and lets the host supply a value for each sourced capability.

---

## 2 · The shape

**`SourcedResolver`** — `StateResolver` with a map instead of a single value:

```rust
struct SourcedResolver<'a, R: Resolver> {
    sourced: Vec<(Capability, Unit)>, // each sourced cap → a constant of its supplied value
    real: &'a R,
}
// resolve(h): if real.resolve(h) is a leaf whose capabilities include a sourced cap we hold a
// value for → return that constant; else delegate. (One sourced cap per leaf in the minimal
// set — timer→Clock, store_read→StateRead; multi-sourced leaves defer.)
```

**`fire_sourced`** — the general "drive a graph with sourced values" entry, the thing every
mode funnels through:

```rust
pub fn fire_sourced(root, sourced: &[(Capability, Value)], real, host) {
    let resolver = SourcedResolver::new(sourced, real); // builds constants via constant_unit
    fire(root, &[], &resolver, host);
}
```

**`StateHost::round` refactors to use it** — DRY, one substituting mechanism. The round assembles
the sourced set from both origins and applies the write at the end:

```rust
pub fn round(&mut self, graph, drivers: &[(Capability, Value)], real) {
    let mut sourced = vec![(StateRead, self.storage.clone())]; // internal: threaded
    sourced.extend_from_slice(drivers);                        // external: this event's inputs
    fire_sourced(graph, &sourced, real, self);
    // apply StateWrite → storage, as before
}
```

`StateRead` is sourced from storage (threaded); drivers are sourced from whatever the caller
hands in for *this* round. The resolver treats them uniformly (sourced ⇒ inject); the round's
storage-threading (`StateWrite → storage`) stays separate, where it belongs — the resolver
injects reads, the round commits writes, no overlap.

---

## 3 · The one open choice: keying

**Per-capability, anonymous (recommended).** The host supplies *one* value per sourced
capability — `Clock → now`, `NetworkIngress → the event` — and the resolver matches a leaf by
its capability. This is the exact parallel of the state cell's *one anonymous cell*. Two timers
in a graph would read the same `Clock` value; keyed/multiple drivers (per-resource capability,
`Clock(id)`) defer precisely as keyed cells did. KISS, and consistent with the donor cell.

The alternative — key by leaf identity (each driver leaf its own value) — is the keyed version;
it's the same deferral the state cell already booked. No reason to pull it forward now.

---

## 4 · Why this is the half that was missing

With sinks (`Cmd`) and state (`Model`) already in, `drive` is **`Sub`** — the world's inputs
flowing *into* the graph. That completes the Elm shape:

| Elm | Holon |
|---|---|
| `Model` | the host's storage (threaded `StateRead`/`StateWrite`) |
| `update` | the graph (pure `eval`) |
| `Cmd` | sinks → `Value::Effect`, performed by the host |
| `Sub` | drivers → sourced values, injected by the host |

And it changes what a `round` *is*. Today the test calls `round` in a loop. Once a driver value
comes **from an event** — a timer tick, an arriving request — that event *is* the round trigger:
`event → sample drivers → round → (effects fire, state updates) → wait`. `round` stops being a
test convenience and becomes the **event loop's body**. The pure interior reacts; the host is
the only thing that touches the clock or the socket.

---

## 5 · Scope

**Build (host-only):** rename/generalize `StateResolver` → `SourcedResolver` (map-keyed);
add `fire_sourced`; refactor `StateHost::round` to thread storage *and* accept per-round
drivers; one test — a `timer`-driven graph fired with a supplied `Clock` value evaluates and
(if it feeds a sink) performs. The 7 host tests stay green (`round` gains a `drivers` arg;
existing calls pass `&[]`).

**Does not touch:** `holon-core` (classification, `is_sourced`, the driver table, `constant_unit`
all already exist); `eval`; the `Host` trait; content addresses.

**Deferred:** *real* world-reading — `SystemTime::now`, an actual socket — is a **platform host**
(the same deferral as real `email`/file IO). `drive` supplies the *values*; whether they come
from a test or a real clock is the host's business, behind the same `Host` seam. Also deferred:
keyed/multiple drivers, multi-sourced leaves, quiescence/event scheduling (`P2.4.2`).

---

## 6 · The one-line shape

> **`drive` = recognize that `store_read` was a driver all along.** Generalize the
> read-substituting resolver to inject *any* sourced value — storage for `StateRead`, the world
> for `Clock`/ingress — and the round becomes an event loop's body. `Sub` joins `Cmd` and
> `Model`; the Elm runtime is complete, on a pure core.
