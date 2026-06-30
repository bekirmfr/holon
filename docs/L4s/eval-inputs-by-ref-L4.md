# L4 · Evaluator input-gathering by reference (firing-rule inch)

> **Companion to `holon-speed-findings.md`.** The bench found that the "O(1)" list
> inspectors (`head`/`length`/`is_empty`) cost O(n) *as evaluated*, because `eval`'s
> firing rule deep-clones a primitive's whole input list before `compute` ever looks at
> it. This inch removes that one clone by handing `compute` borrowed values. It changes
> **no behaviour** — the 163 core tests pin that — and it does **not** touch the §13
> admission gate. The gate's claim ("`head` is O(1)") is true of the primitive; the
> evaluator was throwing it away. We stop throwing it away.
>
> **Pre:** P1.4.2 (`compute`), P1.4.3 (`eval`). **Inv:** I2 (purity — unchanged; no IO
> added). **Scope:** the *primitive firing rule* only. The composite input-gather
> (`eval_composite`) is a **separate, deliberately-deferred inch** — see §6. **Effort:** M
> (mechanical, but wide: one signature + every arm audited + test call sites).

---

## 0. The defect, in one line

`eval.rs`, primitive-firing arm:

```rust
UnitBody::Primitive { name } => {
    let present: Vec<Value> = inputs.iter().flatten().cloned().collect(); // ← deep-clones
    primitives::compute(name, &present, &unit.params)
}
```

`inputs: &[Option<Value>]`. `.flatten()` yields `&Value`; `.cloned()` deep-copies each
present value. When an input is a `Value::List`, that copies the **entire list** (every
element) before `compute` runs. `head` then reads one element of the copy. So an O(1)
primitive pays an O(n) copy. The copy is the only O(n) thing `head`/`length`/`is_empty`
do, which is why the bench shows them tracking `sum` within ~10% at every size — they are
all really measuring the same up-front clone.

**The change:** gather *references*, not clones, and let `compute` borrow.

```rust
UnitBody::Primitive { name } => {
    let present: Vec<&Value> = inputs.iter().flatten().collect(); // ← O(k) refs, no deep copy
    primitives::compute(name, &present, &unit.params)
}
```

`k` = number of present ports (≤ a handful), independent of list size. `compute`'s
signature changes from `&[Value]` to `&[&Value]`. That is the whole idea; the rest of this
note is the audit that proves it's safe and minimal.

---

## 1. Grounding: the exact blast radius

`compute` is `pub fn compute(name, inputs, params)` in `primitives.rs`. Searched every
caller in the workspace:

- **`eval.rs`** — the one firing-rule line above. *(The only non-test caller.)*
- **`primitives.rs` test module** — ~16 `#[test]` functions calling `compute(...)`
  directly with owned `&[Value]` literals.
- **`simulate.rs`** — calls `eval`, **not** `compute`. No change.
- **`holon-host`, `holon-wasm`** — call `eval` / `Evaluator` (which calls `eval`).
  Never call `compute`. No change.
- **`benches/native_vs_composite.rs`** — builds `Unit`s and calls `eval`. `eval`'s
  signature is unchanged, so the bench **compiles and runs unchanged**; re-running it is
  the speed proof.

So the edit is confined to `eval.rs` (one line) + `primitives.rs` (signature, four private
helpers, the test module). Nothing in `catalog.rs` (interface axis) or `kind.rs`
(capability axis) moves — this touches only the *behaviour* axis, and only its plumbing,
not any result.

*(Aside, out of scope: `compute` is `pub` but has no caller outside this crate. It could be
`pub(crate)`. That is a separate tidy-up — don't braid it into this inch.)*

---

## 2. The arm-by-arm audit (the heart of it)

With `inputs: &[&Value]`, a slice pattern like `let [l] = inputs else { … }` binds
`l: &&Value`. Two facts make this almost free:

**(a) Reading arms need no edit — deref coercion handles `&&Value`.** Every extractor
(`as_number`/`as_bool`/`as_text`/`as_list`/`as_index`) takes `&Value`. Rust deref-coerces
`&&Value → &Value` at the call site (`&Value: Deref<Target = Value>`), so `as_list(l)` with
`l: &&Value` compiles untouched. Same for `for v in inputs { … as_text(v) … }`.

**(b) Retaining arms need exactly one explicit deref — and the compiler forces it.**
`x.clone()` on `x: &&Value` clones the *reference* (yields `&Value`), not the `Value`, so it
fails to typecheck. The fix is `(*x).clone()`. This fires at precisely the arms that **keep**
an input, and nowhere else — which is the "clone only what you keep" rule made into a
compile error rather than a discipline.

| arm | what it does | change |
|---|---|---|
| `add` `multiply` `subtract` `divide` `compare` `and` `or` | via `bin_numbers`/`fold_numbers`/`fold_bools` | helper sig only (§3) |
| `not` `is_empty` `length` | read one value, return scalar | **none** |
| `head` `get` | `…first()/get().cloned()` — clone the *one* element returned | **none** (and now no up-front list clone) |
| `tail` | `…split_first()…rest.to_vec()` — clones the kept tail | **none** (inherent) |
| `sum` `join` `concat` | iterate a borrowed list/inputs, build a fresh result | **none** |
| `const` | params only, ignores `inputs` | **none** |
| `cons` | keeps `x` + tail | `xs.push((*x).clone())` |
| `select` | keeps `a` or `b` | `(*a).clone()` / `(*b).clone()` |
| `append` | keeps list + `item` | `xs.push((*item).clone())` |
| `list` | keeps every input | `inputs.iter().map(|&v| v.clone()).collect()` (was `inputs.to_vec()`) |
| `egress` `store_write` | via `sink_effect`, keeps payload | helper edit (§3) |

The retaining arms (`cons`, `select`, `append`, `list`, `sink_effect`) clone exactly what
they structurally retain — no more, no less. They were already O(n) where they keep a list
(`append`/`cons` copy the backing `Vec`; that is the nature of an immutable Vec-backed
value, not the evaluator's fault). This inch does not make them worse; it just stops the
*evaluator* from cloning on top of them.

---

## 3. The four private helpers

These take the whole slice, so they cannot ride deref coercion (a `&[&Value]` does not
coerce to `&[Value]`). Each gets its `inputs` parameter retyped `&[Value] → &[&Value]`;
their bodies are then covered by fact (a) above, except `sink_effect`'s retain:

```rust
fn fold_numbers(inputs: &[&Value], init: f64, f: fn(f64,f64)->f64) -> Option<Value> // body unchanged (coercion)
fn fold_bools  (inputs: &[&Value], init: bool, f: fn(bool,bool)->bool) -> Option<Value> // body unchanged
fn bin_numbers (inputs: &[&Value]) -> Option<(f64,f64)>                  // `let [a,b]=inputs` → coercion
fn sink_effect (sink: &str, inputs: &[&Value]) -> Option<Value>         // Box::new((*p).clone())
```

The extractors (`as_number`/`as_bool`/`as_text`/`as_list`/`as_index`) and `param`,
`parse_bool`, `compare` are **unchanged** — they already take `&Value` / params.

---

## 4. Why this is the least the architecture needs

- **No new layer.** Nothing is added — a redundant copy is removed. The property the gate
  already promised (`head` O(1)) becomes true in practice.
- **No control around the symptom.** No cache, no special-case for inspectors, no size
  threshold. A targeted "skip the clone for `head`/`length`/`is_empty`" would be exactly the
  artificial control to avoid; the systemic fix (references for *all* primitives) is both
  simpler and uniform.
- **Aligns to the existing seam.** The firing rule stays split the way it already is: `eval`
  gathers present inputs; `compute` checks arity and computes. `&[&Value]` preserves that
  boundary precisely — it changes how inputs are *handed over*, not who does what.
- **Purity untouched (I2).** No IO, clock, or randomness added; `compute` stays
  deterministic; no value enters any hash preimage. `blake3`-only runtime is unaffected.

---

## 5. Tests, and the expected tripwire

**Behaviour is identical**, so the existing 163 core tests are the gate. But the
`primitives.rs` test module calls `compute` with owned literals, which no longer match
`&[&Value]`. Mechanical rewrap, assertions unchanged:

```rust
// before
assert_eq!(compute("add", &[num(3.0), num(7.0), num(5.0)], &[]), Some(num(15.0)));
let l = compute("list", &[num(3.0), num(7.0), num(5.0)], &[]).unwrap();
assert_eq!(compute("length", &[l.clone()], &[]), Some(num(3.0)));

// after — wrap each owned Value arg in `&`; `l.clone()` becomes `&l` (now only borrowed)
assert_eq!(compute("add", &[&num(3.0), &num(7.0), &num(5.0)], &[]), Some(num(15.0)));
let l = compute("list", &[&num(3.0), &num(7.0), &num(5.0)], &[]).unwrap();
assert_eq!(compute("length", &[&l], &[]), Some(num(3.0)));
```

(The `&num(3.0)` temporaries live to the end of each `assert_eq!` statement, which outlasts
the `compute` call — fine. A bonus: `list_construct_and_query` no longer needs its
`l.clone()` dance, since every use is now a borrow.)

**Expected count: 163 core green, unchanged** (no test added or removed). Host 38, wasm
6 native / 10 target — all untouched.

**The speed proof is the existing bench**, re-run unchanged. Predicted deltas vs the
findings' table (corrected where the findings mis-predicted):

- `head` / `length` / `is_empty` → flatten toward the ~70 ns floor (the O(1) the gate
  promised, finally visible).
- `native_sum` → **drops sharply** (it stops cloning the list, then summing; it just sums
  the borrowed list). *The findings said "barely moves" — that conflates "reads every
  element" with "must copy every element." It reads without copying now.*
- `fold_sum` → barely moves (correct: `fold` is intercepted before the firing-rule line, so
  it never paid this clone).
- `wrapping_add` → **barely moves** (its inputs are two scalars; there is no list to stop
  copying; its 9× is composite machinery — hash resolve, BTreeMap, topo sort, scratch
  alloc — which this inch does not touch). *The findings said "drops somewhat."*

---

## 6. Honest scope: what this inch does NOT fix

The bench times **bare** primitives (e.g. `head` as a top-level unit), so it only exercises
the firing-rule clone. But in a real graph an inspector lives **inside a composite**, and
`eval_composite` gathers a child's inputs with its *own* clones:

```rust
Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(), // clones the promoted input
Some(&(fc, _fp))      => scratch[fc as usize].clone(),               // clones the upstream output
```

So a `list → head` *composite* still deep-copies the list into the scratch-gather before
`head` fires. This inch makes the bench honest and makes bare inspectors (and `native_sum`)
fast; it does **not** make a *composed* inspector O(1). Closing that needs the same
by-reference treatment in `eval_composite`, and that is genuinely harder: a child borrows
from earlier `scratch` slots while we assign later ones, which fights the borrow checker on
a single `Vec` (you cannot hold `&scratch[i]` while doing `scratch[j] = …`). That friction
is plausibly *why* the original just clones everywhere — a KISS-for-the-borrow-checker
trade, not an oversight.

**Decision recorded:** do the firing-rule inch first, on its own, green, with the bench
re-run. Treat `eval_composite` by-reference as a **separate** inch with its own L4 and its
own before/after numbers (a composite-wrapped `head`/`sum` over a large list added to the
bench), so its larger blast radius and borrow-checker work are weighed on their own merits
rather than braided in here. Do not start it until this one is green.

---

## 7. The inch loop

1. Apply: `eval.rs` firing-rule line (`Vec<&Value>`, drop `.cloned()`); `primitives.rs`
   `compute` signature `&[&Value]`; four helper signatures; the five retaining arms'
   `(*x).clone()`; the test-module rewrap.
2. User runs `cargo test --workspace` → **expect 163 core + 38 host + 6 wasm-native green**;
   `wasm-pack test --node crates/holon-wasm` → 10 target green.
3. If green: commit; re-run `cargo bench -p holon-core`; confirm `head`/`length`/`is_empty`
   go flat and `native_sum` drops; JOURNAL + PLAN update.
4. Next: decide whether to take the `eval_composite` by-reference inch (§6).
