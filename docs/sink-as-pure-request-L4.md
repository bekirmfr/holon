# Sinks as pure request-constructors — Level-4 reconciliation (Fork B)

> *crate:* pure `holon-core` (the host shell is a later, thinner inch) · *touches:*
> `P2.1.1` (kind semantics), `P2.7.1` (mocks), and sets up `P2.4.1`-deploy / `P2.7.2`
> · *inv:* `I7` / `§7` · *effort:* **M**
> *Decision pinned:* **Fork B** — a sink is a *pure request-constructor*; the host is
> the only impurity. Chosen because it maximizes the pure surface, and the pure
> surface **is** the product (trustless, content-addressed, universally memoized).

---

## 0. The correction (this is a reconciliation, not a new design)

The design doc commits to **one** place purity ends: *"the state cell — the one
explicit place purity ends. A subgraph containing one forfeits caching and replay,
visibly, at that boundary."* Drivers are the graph's **inputs** (the host supplies
them; given them, the graph is a pure function), sinks are the graph's **outputs**
(consequences exit at the bottom). That is Functional-Core/Imperative-Shell — the
model the doc names as the one to adopt.

The **code drifted**: `Kind::is_effectful` is `self != Pure`, and `email` has no pure
`compute` (returns `None`) — so today a sink is treated as a *second* place purity
ends. That quietly contradicts the doc and, worse, carves the entire class of useful
graphs (anything that ultimately *does* something) out of the cacheable/shareable
fabric. Fork B does not change the design; it **reconciles the implementation back to
design intent that was already correct**.

---

## 1. One coarse notion splits into two honest ones

`is_effectful = (self != Pure)` conflates two properties the rest of the system needs
to keep apart:

| notion | question | broken by | used for |
|---|---|---|---|
| **effect-bearing** | does running this *describe* an effect the host must perform? | Driver, State, Sink (any capability) | the security manifest (P2.3), consent before import |
| **replayable / cacheable** | is the output a deterministic function of the inputs? | **State only** | the memo market (P3.3), replay, "is this trustlessly shareable?" |

The capability manifest (`capabilities` / `is_effectful`) already *is* the
effect-bearing notion — **it stays exactly as it is.** What is missing is the second
predicate, and it is the one the design doc actually ties caching to:

```
replayable(unit) ≔ no State capability anywhere in its closure
```

Mapped onto the four kinds:

| kind | effect-bearing | replayable / cacheable | why |
|---|---|---|---|
| Pure | no | **yes** | nothing touched |
| Driver | yes (Clock, …) | **yes** | it is an *input*; the graph is pure *given* it |
| Sink | yes (NetworkEgress, …) | **yes** ← the change | it purely *constructs a request*; sending is the host's act |
| State | yes (StateRead/Write) | **no** | memory across firings — the one place purity ends |

So `is_effectful` is not renamed or removed; a **second** predicate
(`is_replayable` / no-state) joins it, and the two stop being the same question.
Crucially, a sink-bearing but state-free graph is now **effect-bearing *and*
replayable** — which is exactly the combination Fork A could not express.

---

## 2. What a sink *is* now — and why no new `Value`

A sink leaf becomes **identity on its payload, carrying a structural capability**:

- its pure `compute` returns the settled payload it was given — `compute("email",
  [payload]) = payload`. Eval can now run it; it is pure.
- *which* effect that payload describes is **derived, not stored** — it is the leaf's
  capability (`email → NetworkEgress`), already in the manifest, computed from
  structure. The value carries the *what*; the capability carries the *which*.

This is the careful answer to the representation question, and it is: **a request is
not a new kind of `Value`.** A request is the pair *(capability, payload)* where the
**payload is ordinary data** (`Number`/`Boolean`/`Text`/`List`, composed in the pure
interior — a multi-field message is a `List` the interior builds) and the
**capability is structural** (derived from the leaf, never a value). The
"command-ness" lives in the structural manifest and the host's effect channel, **not**
in the value type — so `Value` stays the clean pure-arithmetic value it is, and
nothing in the hash preimage changes (runtime `Value` was never hashed). Reaching for
a `Value::Request` variant would push effect-semantics into the data model that pure
units share; the structural-capability framing keeps the two separated, which is the
whole point of Fork B.

*(Deferred, not needed to prove the model: per-sink **typed** payload schemas. For now
every sink is identity-on-payload + a capability; the interior composes the payload's
shape.)*

---

## 3. Consequence: the sink mock retires

A pure sink needs no pure stand-in. So:

- `mock_of`'s `Sink` arm becomes `None` (nothing to substitute — `eval` runs the real,
  now-pure sink), and `sink_mock` is deleted.
- **Only drivers keep a design mock** (they are genuinely external input the host
  supplies; design mode scripts a stimulus). **Only state is host-threaded.** Sinks
  evaluate directly in both modes.
- `design_eval` over a sink-bearing graph now *succeeds* under the real resolver —
  there is nothing to mock. The honest test is no longer "the mock echoes the input"
  but "the sink evaluates purely, the manifest still surfaces the egress, and the graph
  reports replayable."

This retires an assumption I baked into the P2.4.1 L4 (rev. 2), which treated sinks as
impure leaves needing mocks. That was Fork A leaking into the design; it is now
corrected.

---

## 4. The payoff (why this was the right fork)

A `Timer → compose-message → Email` graph is, after this, **eval-pure, replayable,
cacheable, and trustlessly shareable** — it carries a `NetworkEgress` capability in its
manifest (you see, before import, that it will send), yet computing *what* it will send
is pure data and memoizable by `(closure-hash, input-hash)`. The marketplace's keyed
surface expands from "arithmetic" to "anything whose only impurity is the final send."
Only a *state* cell pulls a graph out of that surface — exactly one place, exactly as
the doc always said. This is the end-state argument made concrete: **the pure surface
grew to cover the useful graphs.**

---

## 5. The buildable inch (pure core)

1. **Sink `compute`** *(primitives.rs)* — `"email" => Some(inputs[0].clone())`
   (identity on payload). The lone sink exemplar today; the pattern is "a sink passes
   its settled payload through, the capability says what it is."
2. **`is_replayable`** *(kind.rs)* — on `Capabilities`: `!self.iter().any(|c| c.kind()
   == Kind::State)`. The cacheability predicate the memo market will gate on; the
   inverse of P2.6.3's "cell-bearing ⇒ non-replayable," landed early.
3. **Retire the sink mock** *(simulate.rs)* — `mock_of` `Sink ⇒ None`; delete
   `sink_mock`; rewrite `sink_mock_echoes_settled_input` into the §3 honest test.
4. **Leave `is_effectful` and the whole capability layer untouched** — the manifest is
   correct as-is; only the missing second predicate is added.

**Tests.** `sink_is_eval_pure` (a sink-bearing graph evaluates under the real resolver
— no design mode needed); `sink_graph_is_effect_bearing_and_replayable` (manifest =
`{NetworkEgress}`, `is_replayable` = true — the combination Fork A couldn't express);
`state_graph_is_not_replayable` (a `Store` closure reports non-replayable);
`driver_still_needs_a_mock` (drivers remain the design-mode exception); the existing
guardrail (`classification_ignores_mocks`) and capability suite stay green unchanged.

---

## 6. What this sets up (deferred to the host inch)

Deploy firing becomes a **thin performer of requests**: source driver values (input
substitution with real values), `eval` the now-fully-pure graph, then for each sink
collect *(capability, settled payload)* and perform it. The collection is a **pure
"pending effects" computation** (`graph, inputs → Vec<(Capability, Value)>`) —
deterministic, inspectable before running, the I9 trust story made literal — and only
*performing* the collected effects is impure (the host). The round's output is Elm's
`(Model, Cmd)`: *(state writes, effects to perform)*. Batching, buried-sink
composition, and the round shape all live there, with real IO. None of it is needed to
land §5.

---

## 7. Invariant check

`I7` ("effects at the edges, purity in the middle") is *better* served, not bent:
effects are at the edges as the **capability manifest** (structural, derived); the
middle — including sinks' payload construction — is genuinely **pure**. The reading
that made sinks impure leaves was the looser one; Fork B is the tighter fit. `§7`
("one explicit place purity ends — state") is now honored by the code, not just the
prose.
