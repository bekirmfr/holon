# L4 · A→B — the effect *value* (`Value::Effect`, computed fan-out)

> **Status:** design, pre-build. Recommends a shape and surfaces **one** open sub-decision
> (`Type::Effect`: add it, or defer with type-looseness). Builds on confirmation.
> **Pre:** Fork B (sink = pure request-constructor), deploy firing (effects output-carried),
> the state cell (`P2.6.1`). **Touches no *existing* content address.**

---

## 1 · What A can't do, and why we need B

Under representation **A**, a sink is **identity on its payload**: `email(msg) → msg`,
`store_write(v) → v`. The graph's single output *is* the one request, and `fire` performs
the manifest's output-side capabilities against that one payload. That ceiling is **one
effect per round**. The counter already brushes it: a round can write state **or** send a
mail, not both — and "a welcome email **per** signup" (N effects from one `map`) can't be
said at all.

The blocker is **ambiguity**, not expressiveness. Suppose `map(email, [a,b,c])` under A:
`map` yields `[a, b, c]` (identity over the list) — a `List` of payloads. Now `fire` holds
output `List[a,b,c]` and manifest `{NetworkEgress}`, and **cannot tell** "three emails"
from "one email whose payload is a list." The output isn't self-describing. **B makes each
effect self-describing**, which dissolves the ambiguity and gives fan-out for free (because
`map` already iterates — grounded: `eval_map` does `out.push(eval(fn,[elem]))`).

---

## 2 · The realization: the effect value carries the **name**, not the capability

Earlier I told you B forces an inversion — `Value` would carry a `Capability`, so `value.rs`
would start depending on `kind.rs`, and the sink's `compute` (in `primitives.rs`) would too.
**That was wrong, and the fix is small.** Let the effect value carry the **sink's name**, a
plain `String`, not its `Capability`:

```rust
Value::Effect { sink: String, payload: Box<Value> }
```

The capability is **derivable** from the name — `capabilities_of(&sink)` already gives it —
so nothing is lost. And both inversions vanish:

- **`value.rs`** gains `Effect { sink: String, payload: Box<Value> }` — `String` + `Box<Value>`,
  **no `kind` import.** (Grounded: `value.rs` imports only `Type`; `kind.rs` does not import
  `Value`, so even the by-capability form wouldn't *cycle* — but by-name needs no import at all.)
- **`primitives.rs`** constructs `Effect { "email".into(), … }` using the name it is *already
  matching on*. **No `kind` import.**
- **`fire`** reads the name off the value and derives the capability (`capabilities_of(sink)`),
  so the **`Host` trait is unchanged** — `perform(capability, payload)` still. `StateHost`'s
  `cap == StateWrite` match is untouched.

This is the same move the whole codebase already makes: **everything keys on the name**
(`compute`, `capabilities_of`, `kind_of`). The effect value joins that convention instead of
smuggling a typed capability into the data layer. The name *is* the structural identity; the
capability is a *projection* of it (`I8`: derived, not asserted).

---

## 3 · The shape

**Sinks construct, they don't pass through.** Two lines flip from identity to construction:

```rust
"email"       => Some(Value::Effect { sink: "email".into(),       payload: Box::new(inputs.first()?.clone()) }),
"store_write" => Some(Value::Effect { sink: "store_write".into(), payload: Box::new(inputs.first()?.clone()) }),
```

(`store_read` is unchanged — it's sourced, not constructed.)

**Fan-out is `map`, already built.** `map(email, [a,b,c])` ⇒ `[Effect{email,a}, Effect{email,b},
Effect{email,c}]` — a `List` of `Effect`s. No new primitive; `map` iterating the sink *is* the
fan-out.

**`fire` walks the output for effects** instead of performing one payload:

```
eval → output
collect(output):                       // an Effect, or a List of Effects/Lists, flattened
    Effect{sink, payload}  → [(sink, payload)]
    List(items)            → items.flat_map(collect)
    _                      → []          // pure data at the root: nothing to perform
for (sink, payload) in collect(output):
    for cap in capabilities_of(sink) where !cap.is_sourced():
        host.perform(cap, payload)
```

The manifest (`capabilities`) stays the **may** (structural disclosure, trust, computed from
the *real* resolver). The collected effects are the **does** (what actually reached the
output this run). Heterogeneous rounds — `[Effect{store_write,v}, Effect{email,msg}]` — work
the same way: each element self-describes, so a `StateWrite` and an egress ride out together
with **no multi-output composite** (that fork stays deferred). `StateHost::round` collects the
same way and routes `store_write` to storage, the rest to real performance.

---

## 4 · The open sub-decision: `Type::Effect` — add it, or defer?

A sink's output is now an `Effect` value. What is its **port type**?

**Option α — add `Type::Effect` (recommended).** Append it to the `Type` enum. Cost is genuinely
small:
- `Type` is `{Number, Boolean, Text, List, Any}`; appending `Effect` **last** keeps every
  existing tag stable, so **no existing unit's hash moves** (must verify the encoder is
  append-stable — a build-time check).
- validate's compat rule is `dst==Any || src==Any || src==dst` — it covers `Effect` with **zero
  code change** (`Effect` matches only `Effect` or `Any`).
- `Value::ty`: one arm, `Effect{..} => Type::Effect`.

And it **buys an invariant for free**: an `Effect` can be wired to an output or into `map`, but
**not into a sink's payload slot** (a `Text`/`Number` input) — `Effect`-vs-`Text` is a type
mismatch validate already rejects. So "a request cannot be re-consumed as data" becomes a
*typing fact*, not a convention. Effects flow **down and out**, never back in.

**Option β — defer (type-looseness).** Leave the sink's output port typed as its payload
(`email: … → Text`), let the *value* be an `Effect`. validate stays untouched, but the port
now **lies** (says `Text`, carries `Effect`), and nothing stops an `Effect` being fed into a
payload slot. Smaller diff, dishonest types, loses the free invariant.

**Recommendation: α.** It's a one-variant change that makes the type system tell the truth and
fences effects structurally — squarely in the project's grain (`I8`, "derivable ⟹ derived").
The only contingency is verifying the hash encoder appends stably; if it somehow doesn't, that's
a five-line fix to make it so, still α.

---

## 5 · Staging (two green-able inches)

**Slice 1 — representation swap (same behavior).** Add `Value::Effect` + `Type::Effect`; flip
the two sink `compute` lines; teach `fire`/`round` to **unwrap a single `Effect`** (and `ty`).
Observable behavior is **identical** — `email("hi")` still performs `(NetworkEgress, "hi")`,
the counter still climbs `0→1→2→3` — only the internal representation changes from
identity-payload to `Effect`-wrapped. The 5 host tests stay green *as-is*; the core change is
the two primitive tests (`email`/`store_write` now return `Effect{…}`) plus `ty`. A clean
refactor-shaped inch: prove the new representation carries the old behavior.

**Slice 2 — fan-out (new capability).** Make `fire`/`round`'s collect **flatten a `List` of
`Effect`s**. Purely additive. New tests: `map(email, [a,b,c])` performs **three** egresses;
a heterogeneous `[store_write, email]` round writes **and** sends. This is the inch that lifts
the one-effect ceiling.

Splitting this way keeps each inch's diff legible and each test green before the next.

---

## 6 · What B does **not** touch

- **No existing content address.** `compute` is keyed by name (outside the hash preimage);
  `Type::Effect` appends last (existing tags stable). New `Effect`-using units get new hashes —
  as new units should.
- **No `eval` rewrite.** `Effect` is a value; `eval` flows it like any other (`map` already
  iterates). `eval` neither knows nor cares it's an effect.
- **No `Host` trait change.** `perform(capability, payload)` stands; `fire` derives the
  capability from the name.
- **No `validate` rewrite.** The existing compat rule absorbs `Effect`.
- **Multi-output composites stay deferred.** Self-describing `List[Effect]` is how heterogeneous
  effects exit a single output, so the multi-output fork isn't needed for this.

---

## 7 · The one-line shape

> **B = give an effect a self-describing value — `Effect { sink: name, payload }` — so the
> output can carry *many*, and let `map` be the fan-out.** The name (not the capability) is what
> the value holds, which is why B costs less than I first quoted: no layer learns anything it
> didn't already key on.
