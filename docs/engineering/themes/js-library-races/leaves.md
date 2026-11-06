# Leaves and the evaluator: what composites pay

Measured on 2026-11-06, Windows 11, one machine, the base being `c6d350e` (the races' harness) and
the after this change. Every number below is a measurement of the evaluator through `eval`, the
way a flow is run; none is an estimate unless it says so.

The question was why a composite costs so much more than the leaf it could replace, and how much
of that the evaluator could take back without changing any value, unit, port, parameter or hash.
The answer: most of it was copying and preparing, not computing. A composite paid to prepare each
nested function again for every item of a `map`, to copy the value a `fold` grew at every step, and
to copy every value it used twice. Those three are gone; what a composite pays now is mostly the
walk itself, about 30 to 40 ns a node.

## How to run it

```sh
cargo bench -p holon-author --bench races_evaluator              # every race below
cargo bench -p holon-author --bench races_evaluator -- filter    # the races whose name has "filter"
cargo bench -p holon-author --bench races_evaluator -- "as it was"   # each changed leaf against its old body
cargo bench -p holon-author --bench races_evaluator -- arc       # the Arc estimate
cargo bench -p holon-core --bench native_vs_composite            # criterion: fold, the wrapping tax
cargo bench -p holon-author --bench perf_audit                   # one call as the server makes it; maps and folds
cargo bench -p holon-author --bench host_request                 # samples' requests, in-process, as the server runs them
```

`perf_audit` and `host_request` use only the public API, so the same file builds against the
evaluator before a change and after it, and the two binaries are raced in turn (§9).

This machine's noise is large: the same binary run twice apart gave times 20 to 40% apart, and
other sessions were building at the same time. So "before" and "after" were raced as two copies
of the bench binary, the base one and this one, alternating race by race over four rounds; the
tables give the median of the four. A ratio under 0.8 or over 1.25 held in every round; one
between did not always.

## 1. Where the time went

| Cost | Before | What it came from |
|---|---:|---|
| Preparing a composite for a run: resolving its children | 109 ns (9 children), 437 ns (25) | a `MapResolver` lookup per child (a `BTreeMap` keyed by a 32-byte hash) |
| Preparing it: working out its shape | 450 ns (9 children), 789 ns (25) | a dozen small allocations, for a resolver that keeps no shapes |
| A nested function called from a `map`'s function | 600 to 770 ns an item | the nested composite was prepared again at every application |
| A `const` | 22 ns a call | its literal parsed from its parameter at every call, where `add` takes 8.6 ns and `compare` 11 ns |
| A `fold` that grows its accumulator (`[...acc, x]`, `insert(acc, k, v)`) | O(n) a step, O(n²) the fold | the accumulator was lent to each step, so the leaf copied it to grow it |
| A value used twice (`get(xs, length(xs) - 1)`) | O(n) a call | it goes through a pass-through `guard(true, xs)`, which copied it |
| A list captured by a `map`'s function and read twice an item | O(n) an item, O(n²) the map | the same copy, in every application |
| `join`, `concat`, `slice` | | `join` copied every item into a piece, then the pieces again; `concat` grew its text; `slice` collected it character by character |

The first two were measured by a probe in `eval.rs` (removed): `abs` (9 children) prepared in
643 ns, `score` (25) in 1368 ns. The deployed server never paid the shape step: its resolver keeps
every unit's shape (`crates/holon/src/units.rs`, `Loaded::shape`). The races, the tests and any
resolver that keeps none did; the nested preparation and the copies were paid everywhere.

## 2. What changed, each with its effect

All in `crates/holon-core/src/eval.rs` and `primitives.rs`. Effects are from the races of §3 and
the criterion runs of §4; what the audit changed before the merge is in §9.

| Change | What it does | Measured |
|---|---|---|
| **Nested functions prepared once** (`Many::nested`) | A preparation run many times (a `map`'s or `fold`'s function) keeps the preparation of each composite inside it, made at the first application, run down its plan after. | The call of a 5-leaf function from inside a `map`'s function: 600 to 770 ns an item before, 40 to 90 ns after. Per item, `xs.map((x) => lin(x))` ×0.32 and `xs.map((x) => abs(x))` ×0.23 |
| **Shapes kept per thread** (`kept_shape`) | For a resolver that keeps no shapes, the last 64 shapes made, by the unit's address, used only for a unit with the same child count and the same edges (a shape is a function of those alone, so whatever unit lives at an address now, the shape found is its own). | The shape step: 417 to 789 ns before, about 25 ns once kept. Criterion's composite wrapping one `add`: 552 → 278 ns |
| **`const` computed once** (`Many::consts`) | In a preparation run many times, a `const` is computed when the preparation is made, and each run copies the value (a number, a truth value or a short text). | 22 ns a `const` a run |
| **Giving** (`Prepared::fire_giving`, `primitives::compute_given`) | In a preparation run many times, a value moves to the last node that reads it, when that node builds on it (`append`'s list, `insert`'s record, a `fold`'s accumulator), and the leaf builds in place instead of copying. A `fold` gives its accumulator to each step; a `map` or `fold` given its list gives each item to its application. Which ports may be given their value is worked out once for the preparation (`Gives`), and a value is moved only when every other reader has read it or never will. | filter by `fold` at 4096: ×0.013; a record by `fold` of `insert`: ×0.004 |
| **Lending** (`Prepared::chosen`, `Prepared::output`) | A choice (`guard`, `coalesce`) that passes on a value it can't move (one its composite was lent, or one read elsewhere too) keeps no copy: its readers read the value where it is. Only under an observer that doesn't look at produced outputs (`Observer::reads_produced`: plain `eval`, and every `map`/`fold` application); others see the copy as before. | `get(xs, length(xs) - 1)` at 4096: ×0.02, and flat in n; a captured list read twice an item: ×0.02 at 4096 |
| **`extend` puts a short list in front of a given long one** | `[x, ...acc]` with the accumulator given: the one item goes in front of it where it is, instead of the accumulator being moved behind a copy of the item. Still O(n) a step: a `Vec` has no room in front. | reverse by `fold`: ×0.82 at 64, ×0.15 at 4096 (giving without this: ×1.17 and ×0.21) |
| **Leaf functions read by reference** | A `map` or `fold` whose function is a leaf, with nothing worth giving it (a sum's numbers), reads its inputs by reference as before; a `map` of a composite over a lent list runs it directly. | criterion's fold of `add`: 55 ns an item with the change's arguments, 42 ns with this (the same as before); the composite run directly: about 11 ns an item |
| **`join`** | Measured first, then written once into one text. | ×0.10 at 64 items, ×0.05 at 4096 (against its old body, same inputs) |
| **`concat`** | Lengths measured first, one text made at its size; a given first text grows in place. | ×0.76 at 64 bytes, ×0.46 to 0.62 at 10 KB |
| **`slice`** | The start and the end found as byte offsets, the piece copied at once. | ×0.62 at 64 bytes, ×0.78 to 0.79 at 10 KB |
| **`append`, `extend`** | Lent: the list made at its final size. Given: grown in place. | `append` lent ×0.78 to 0.94; given, O(1) amortized where it was O(n) |

Three things hold for all of them, and the tests check each:

- **Outputs are the same for every input, absent and pending included.** A leaf given an input
  computes what `compute_op` computes on the same values; when it produces nothing, it hands back
  what it was given, and the value goes back where it was, so a reader passed over on the
  assumption that it would produce a value (a `coalesce`'s fallback) still finds it.
  `given_inputs_compute_what_lent_ones_do` checks it for every leaf that builds on a given input,
  on every mix of given and lent inputs over values of every kind, about 400,000 cases. A choice
  still runs only what it needs.
- **Observers see the same steps.** `enter`, `produced` and `leave` come in the same order with the
  same paths; a choice that lends is reported `produced` with no outputs, and only to an observer
  that said it doesn't look.
- **The eager oracle stays the reference.** `giving_and_lending_agree_with_eager_on_generated_graphs`
  builds random composites of the leaves that give or take (`guard`, `coalesce`, `append`,
  `extend`, `cons`, `tail`, `head`, `get`, `length`, `concat`, `insert`, `lookup`, `remove`,
  `with_field`, `field`, `select`, `pair`, `is_truthy`, `equals`), with one or two outputs, the
  second sometimes an input, and functions joining the pool every 50 rounds (called, folded and
  mapped). It checks over 12,000 runs against `eval_eager`, as one call, as a `fold`'s function and
  as a `map`'s, and checks that an observer that reads and one that doesn't see the same steps.

## 3. The races, before and after

Each composite is TypeScript compiled by the reader, raced through `eval` against a native
candidate (races_evaluator.rs). The ratio columns are the composite over the native candidate.

### (a) abs, mapped: `xs.map((x) => abs(x))`, a function with a choice

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 1 | 2.8 µs | 2.3 µs | 0.81 | 16 | 11 |
| 64 | 95.9 µs | 24.9 µs | 0.26 | 42 | 13 |
| 4096 | 6.22 ms | 1.43 ms | 0.23 | 42 | 12 |

Per item, (t(4096) − t(64)) / 4032: 1.5 µs before, 347 ns after (×0.23).

### (b) lin, mapped: `xs.map((x) => lin(x))`, a function with no choice, called from the map's function

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 1 | 2.1 µs | 2.1 µs | 1.00 | 12 | 11 |
| 64 | 54.2 µs | 18.6 µs | 0.34 | 24 | 9.9 |
| 4096 | 3.28 ms | 1.06 ms | 0.33 | 25 | 9.3 |

Per item: 799 ns before, 259 ns after (×0.32).

### lin in place: `xs.map((x) => x * 2 + 1)`, the same function written in the map

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 1 | 1.9 µs | 1.3 µs | 0.67 | 11 | 6.7 |
| 64 | 12.4 µs | 13.6 µs | 1.10 | 5.2 | 7.0 |
| 4096 | 686 µs | 832 µs | 1.21 | 4.2 | 6.6 |

Per item: 167 ns before, 203 ns after (×1.21; §6).

### last, list used twice: `get(xs, length(xs) - 1)`

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 1.3 µs | 681 ns | 0.52 | 30 | 16 |
| 512 | 3.1 µs | 693 ns | 0.23 | 75 | 16 |
| 4096 | 29.6 µs | 660 ns | 0.02 | 687 | 16 |

### last, mapped over 64 lists of n

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 80.9 µs | 33.0 µs | 0.41 | 40 | 19 |
| 512 | 181 µs | 30.8 µs | 0.17 | 86 | 19 |
| 4096 | 1.29 ms | 30.8 µs | 0.02 | 611 | 18 |

### mirror: `idx.map((i) => get(ys, length(ys) - 1 - i))`, a captured list read twice an item

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 36.0 µs | 25.2 µs | 0.70 | 41 | 30 |
| 512 | 1.26 ms | 193 µs | 0.15 | 220 | 33 |
| 4096 | 86.1 ms | 1.51 ms | 0.02 | 1328 | 28 |

### (c) filter, by fold: `xs.reduce((acc, x) => x > 10 ? [...acc, x] : acc, [])`

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 76.8 µs | 35.7 µs | 0.46 | 176 | 82 |
| 512 | 2.38 ms | 276 µs | 0.12 | 892 | 101 |
| 4096 | 170 ms | 2.26 ms | 0.01 | 8467 | 119 |

### (d) reverse, by fold: `xs.reduce((acc, x) => [x, ...acc], [])`

Three rounds, after the `extend` change of §2 (the four rounds of the other races ran before it).

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 38.1 µs | 31.3 µs | 0.82 | 136 | 113 |
| 512 | 675 µs | 257 µs | 0.38 | 390 | 134 |
| 4096 | 35.0 ms | 5.19 ms | 0.15 | 2049 | 258 |

### (e) record, by fold: `xs.reduce((acc, x) => insert(acc, x, x), {})`

| n | before | after | after / before | before ÷ native | after ÷ native |
|---:|---:|---:|---:|---:|---:|
| 64 | 164 µs | 29.5 µs | 0.18 | 24 | 4.3 |
| 512 | 9.67 ms | 232 µs | 0.02 | 132 | 3.1 |
| 4096 | 599 ms | 2.21 ms | 0.004 | 774 | 2.4 |

### (f) score: about twenty nodes, three comparisons and two choices

| | before | after | after / before | before ÷ native | after ÷ native |
|---|---:|---:|---:|---:|---:|
| one call | 2.2 µs | 1.6 µs | 0.71 | 56 | 39 |
| `xs.map((x) => score(x, 7))`, n = 1 | 3.9 µs | 3.2 µs | 0.82 | 48 | 39 |
| the same, n = 64 | 159 µs | 57.1 µs | 0.36 | 382 | 142 |

### How each grows

t(4096) / t(512); 8 is linear, 64 quadratic, and the native candidates are about 7 to 10 (the
record's is n log n).

| Race | Before | After |
|---|---:|---:|
| last, list used twice | 9.7 | 1.0 |
| last, mapped over 64 lists | 7.5 | 1.0 |
| mirror | 68.6 | 7.8 |
| (c) filter | 71.3 | 8.2 |
| (d) reverse | 50.3 | 19.4 |
| (e) record | 62.7 | 9.5 |

### The two findings from the lists races

**Shared values are copied.** A list a function uses twice passes through a pass-through
`guard(true, xs)`, which copied it: `at(-1)` written as `get(xs, length(xs) - 1)` was O(n), and a
list captured by a `map`'s function and read twice per item was O(n²). Now the guard lends the list
(plain `eval`, and inside any `map` or `fold`): "last" costs the same at 64 and at 4096 items, and
"mirror" grows as the native does (t(4096)/t(512) about 7.8). Under an observer that reads what
each node produced (the host's, which records the run), the top-level guard still copies; inside a
`map` or `fold` it lends there too.

**Each nested function call costs about 1 µs.** It cost 600 to 770 ns an item on this machine: the
difference between `xs.map((x) => lin(x))` and the same function written in place,
`xs.map((x) => x * 2 + 1)`. The nested composite was prepared at every application. It is now
prepared at the first and kept with the function; what is left of the call is 40 to 90 ns an item,
the cost of running a composite node.

## 4. Per node and per call

| Cost | Before | After | From |
|---|---:|---:|---|
| A node in a `map`'s function, 27 nodes (`score` mapped), per item | 2.46 µs (91 ns a node) | 0.86 µs (32 ns a node) | score mapped, (t(64) − t(1)) / 63 |
| A 5-leaf function written in place, mapped, per item | 167 ns | 203 ns | lin in place, (t(4096) − t(64)) / 4032 |
| The call of that function from inside a `map`'s function, per item | 600 to 770 ns | 40 to 90 ns | lin mapped minus lin in place, each round |
| A 9-leaf function with a choice, called from a `map`'s function, per item | 1.5 µs | 347 ns | abs mapped |
| A `fold` of the leaf `add`, per item | 42 ns | 42 ns | criterion `fold_sum`, 4096 |
| One call of a composite wrapping one `add` | 552 ns | 278 ns | criterion `composite_add` |
| One call of `score` (25 nodes) | 2.2 µs | 1.6 µs | score, one call |
| Preparing `score` for a call | 1368 ns | about 616 ns | the probe; after, an earlier build of this change |
| A `const` | 22 ns | none in a function run many times | the probe |

Criterion (`cargo bench -p holon-core --bench native_vs_composite`), the base's saved run and this
change's:

| Bench | Before | After |
|---|---:|---:|
| `fold_sum/1` | 151 ns | 179 ns |
| `fold_sum/8` | 434 ns | 448 ns |
| `fold_sum/64` | 3.13 µs | 2.60 µs |
| `fold_sum/512` | 26.1 µs | 20.0 µs |
| `fold_sum/4096` | 172 µs | 170 µs |
| `wrapping_tax/composite_add` | 552 ns | 278 ns |
| `wrapping_tax/native_add` | 55 ns | 59 ns |

The native benches (`head`, `length`, `is_empty`, `sum`), whose path didn't change, moved by up to
23% either way: this machine's noise. A `fold` of one item costs about 28 ns more than before (its
arguments are set up so that it can give); per item it costs what it did. The first criterion run after the change had a `fold` of `add` at
55 ns an item; that is what led to reading a leaf function's inputs by reference again (§2).

## 5. Every pure leaf, and `map` and `fold`

n is the size of the input that matters: a list's items, a text's bytes, a record's fields (k).
"Lent" is an input the leaf reads where it is kept; "given" is one moved to it because nothing
reads it after (§2, giving). A copy is a deep one: a list's items, a record's fields, all the way
down. The composites are those of [javascript-library.md](../javascript-library.md) §11 and the
races here.

| Leaf | Time | Copies a call | Composites leaning on it | Change |
|---|---|---|---|---|
| `const` | O(1); parses its literal | its value | every literal: `abs`'s `0`, `lin`'s `2` and `1` | computed once in a function run many times |
| `default` | O(1) | lent: the input; given: none | `x ?? literal` | given: moved |
| `equals` | O(n) | none | `===`, `!==`; `includes` by `fold` | |
| `add`, `multiply` | O(inputs) | none | every sum; `-x` as `x * -1` | |
| `subtract`, `divide` | O(1) | none | `abs`, `score` | |
| `compare` | O(1) | none | `abs`, filter's `x > 10`, `score` | |
| `and`, `or`, `not` | O(inputs) | none | `some`, `every` by `fold`; `!==` | |
| `select` | O(1) | lent: the value chosen; given: none | by name only (the ternary lowers to `guard` and a fallback) | given: moved |
| `concat` | O(total length) | one text | template texts; the `replace`, `padStart` composites | sized once; given: the first grows |
| `uppercase`, `lowercase` | O(n) | one text | | |
| `text_length` | O(n): counts characters | none | `s.slice(a, b)`, `charAt`, `at`, `padStart` | |
| `to_text` | O(size) | one text | | |
| `as_text` | O(size) | a text input copied | `` `${x}` `` | |
| `coalesce` | O(1) | inside a composite, a choice: before, its value copied; now given to its last reader or lent | `??`, the ternary's fallback | giving, lending |
| `is_truthy` | O(1) | none | `Boolean(x)` | |
| `guard` | O(1) | a choice: before, its value copied, at every pass-through `guard(true, x)` too; now given or lent | the ternary; every value used twice | giving, lending |
| `slice` | O(start + len) characters | the piece | `substring`, `charAt`, `at` on texts | byte offsets, one copy |
| `split` | O(n) | a text a piece | `replaceAll` as `split(a).join(b)` | |
| `index_of` | O(n + m), then the characters before the match counted | none | `includes(t)` on texts, `replace` | |
| `list` | O(inputs) | lent: each input; given: none | `[a, b]`; `[x, ...xs]` (a one-item list, extended) | given: moved |
| `get` | O(1) | lent: the item; given: none (moved out) | `xs[i]` by name, last (`at(-1)`), mirror | given: moved out |
| `append` | lent O(n); given O(1) amortized | lent: the list; given: none | `[...acc, x]`: filter, `flat` | sized once; given: grows |
| `extend` | lent O(n + m); the first given O(m); the second given and longer, O(n + m) moved in place | lent: both lists; given: only the one lent | `[...xs, ...ys]`, `[x, ...acc]`: reverse | sized once; given: the longer grows, at either end |
| `length` | O(1) | none | last, mirror | |
| `range` | O(n) | n numbers made | `Array.from({ length: n }, …)` | |
| `sum` | O(n) | none | | |
| `join` | O(total length) | one text | `replaceAll`, `join()` | measured first, written once |
| `cons` | O(n) lent or given | lent: the list; given: shifted in place | reverse by `cons`, by name | given: in place, still O(n) |
| `head` | O(1) | lent: the item; given: none | `xs[0]` as `shift()`'s answer | given: moved out |
| `tail` | O(n) | lent: the rest; given: shifted in place | `slice(1)` | given: in place |
| `is_empty` | O(1) | none | `xs.length === 0` | |
| `record` | O(1) | none | `{}` | |
| `field` | O(log k) | lent: the field; given: none | `r.k` | given: moved out |
| `with_field` | lent O(k); given O(log k) | lent: the record; given: none | `{ ...r, k: v }` | given: in place |
| `without_field` | lent O(k); given O(log k) | lent: the record; given: none | `delete r.k`, `const { k, ...rest } = r` | given: in place |
| `keys` | O(k) | each key | `Object.keys`; `Object.values`, `entries` as `map` over `keys` | |
| `lookup` | O(log k) | lent: the value; given: none | `r[k]`, `k in r` | given: moved out |
| `insert` | lent O(k); given O(log k) | lent: the record; given: none | `{ ...r, [k]: v }`: `fromEntries`, `groupBy`, the record race | given: in place |
| `remove` | lent O(k); given O(log k) | lent: the record; given: none | `delete r[k]` | given: in place |
| `find` | O(n) | the record found | `xs.find((x) => x.id === v)` | |
| `json_parse` | O(n) | the value made | `JSON.parse` | |
| `json_stringify` | O(size) | one text | `JSON.stringify` | |
| `sha256`, `hmac_sha256` | O(n) | one text | | |
| `hex`, `base64` | O(n) | one text | | |
| `secure_equals` | O(n) | none | | |
| `pair` | O(1) | lent: both inputs; given: none | the lowering's own | given: moved |
| `some` | O(1) | lent: the input; given: none | the lowering's optional values | given: moved |
| `none` | O(1) | none | | |
| `map` | n applications | none of its own: each application copies what it keeps; a given list gives each item | `xs.map(f)`, `Object.values`, `flatMap` | nested functions prepared once; items given when the list is; a leaf or composite function over a lent list run directly |
| `fold` | n applications | before: the accumulator lent, so a step that grew it copied it; now given | `reduce`: filter, reverse, record, sum, `some`, `every`, `flat`, `fromEntries` | the accumulator given |

The sinks (`egress`, `store_write`, `collection_put`, `collection_delete`, the `file_*`, `db_*`
and `channel_*` leaves) build an effect around their payload and copy it once; they aren't in inner
loops, and nothing changed there.

## 6. What isn't better

- **A function written in place in a `map` cost about a fifth more an item, until the audit
  (§9).** `xs.map((x) => x * 2 + 1)` went from 167 to 203 ns an item (lin in place, §3). The
  performance audit found where: every run of a plan looked at every node for a value to give, and
  worked out a place for each output. The plan now marks the children that build on an input
  (`Step::Build`), a node goes through `fire_giving` only when the value can be moved, and one
  output takes a short path: lin in place 180 → 168 ns an item against the base, and as merged
  ×0.88 to ×0.98 by size (`perf_audit`).
- **What is slower as merged** (#43, `perf_audit` and `host_request` against the base, medians of
  interleaved rounds). One call through a resolver that keeps shapes, the deployed server's:
  ×1.16 to ×1.23 (lin 395 → 472 ns), ×1.08 to ×1.15 under an observer that reads outputs; 10 to
  15 ns a node neither audit could place, a third of it come with the audit's fixes and the
  smaller frames. `${s}!` mapped over texts ×1.35 (likely `concat` given `as_text`'s fresh short
  text and growing it, not measured). Folds of a number over a composite ×1.09 to ×1.20. A filter
  by fold over 16 items ×1.16 (×0.21 and below from 256). `last` of 4096 under an observer that
  reads outputs, without kept shapes, ×2.0, cause not found. Whole requests (hello, calc, quote,
  basket, fetch-json, slugs) all within noise, ×0.87 to ×1.05.
- **One call of `score` is ×0.71, not halved** (×0.70 to 0.80 by round). What it pays now is
  mostly its preparation (about 616 ns of 1.6 µs): resolving 25 children through the resolver
  (437 ns on a `MapResolver`), deciding each child's kind, and the lists that hold them. Keeping
  the whole preparation across calls is what would halve it (its run alone is about 0.9 µs), and
  the evaluator can't keep it alone: a preparation borrows the units the resolver holds, so it can
  live only as long as the resolver, and `eval` gets the resolver by reference for one call. It
  needs the resolver to keep it (`Resolver` in `resolve.rs` gaining a place for it, as it has one
  for shapes), which each resolver that forwards shapes today (five) would forward too. Mapped,
  `score` is ×0.36, where the preparation is paid once.
- **reverse by `fold` is still O(n²).** `[x, ...acc]` is `extend` of a one-item list with the
  accumulator. With the accumulator given, the item now goes in front of it where it is (§2),
  allocating only when the list outgrows its room, but that still moves its n items a step (t(4096)/t(512) about 19). A `Vec` can't
  put an item in front in O(1); javascript-library.md §11 already sorts `reverse` into a leaf.
- **The host's top level still copies at a pass-through.** The host's observers (holon-host,
  `rounds.rs`) look at what each node produced, so a choice at a flow's top level keeps a copy for
  them. Inside a `map` or `fold`, the observer passes on only what is asked, and lends.

## 7. Arc: an estimate

The question was whether `Value::List`, `Text` and `Record` should hold their contents behind an
`Arc`, so a copy is a reference count.

| Value | Deep copy | `Arc` copy | Ratio |
|---|---:|---:|---:|
| 64 records of 4 fields | 22.5 to 23.8 µs | 12 to 13 ns | 1700 to 2000 |
| a 10 KB text | 93 to 98 ns | 12 ns | 8 |

The sites it would touch (a count of `Value::List(`, `Value::Text(`, `Value::Record(` that build a
value, and of patterns that take one apart):

| Where | List (build / pattern) | Text | Record |
|---|---:|---:|---:|
| all `src` | 59 / 52 | 218 / 109 | 118 / 112 |
| all tests | 110 / 11 | 122 / 8 | 77 / 25 |
| `holon-core/src` | 30 / 22 | 66 / 37 | 46 / 25 |
| `holon/src` | 14 / 22 | 133 / 64 | 57 / 76 |

`Value` derives only `Clone`, `Debug` and `PartialEq`, all of which an `Arc` keeps; a leaf that
builds on a given value would call `Arc::make_mut`, which copies only when the value is shared,
so giving keeps working.

**Recommendation: not now.** After giving and lending, no race here copies anything large in its
inner loop. The copies left are: a part read out of a container that stays where it is (`get`,
`field`, `lookup`, `head` of a lent list or record: the part is copied deep); an item a `map`'s
function keeps (`xs.map((x) => [x, 1])` copies each item); a value two builders read
(`append(xs, a)` and `append(xs, b)`: one copy, which `make_mut` would make too); and the host's
top-level choices. If a race shows the first two dominating, which is what a list of JSON rows
read by field would do, `Record` first: its copy is the deep one (1700 to 2000 times an `Arc`'s),
and reading a record out of a list or a field out of a record are the copies that remain. A text's
copy is one `memcpy` (8 times an `Arc`'s at 10 KB), not worth its 327 sites.

## 8. Proposals

1. **Keep a composite's preparation in the resolver**, as its shape is kept, so one call pays its
   run and not its preparation: one call of `score` from 1.6 to about 0.9 µs.
2. **Place what one call pays with kept shapes** (§6, 10 to 15 ns a node), and `${s}!`, with a
   profiler, on a machine with one.
3. **`reverse` as a leaf**, as §11 says; `cons` and `[x, ...acc]` stay O(n) a step whatever the
   evaluator does.
4. **The host's observers that only record say so**, if a recorded run's outputs can be read where
   they are kept rather than copied for the record: then the top-level pass-through lends in a
   deployed flow too.
5. **A faster child lookup for `MapResolver`** (a hash map, or children resolved once per unit),
   which is most of a preparation once its shape is kept.
6. **`Record` behind an `Arc`**, only when a race shows a part copied out of a lent container
   dominating (§7).

## 9. Reviewed

An independent review of the change, run against the eager oracle with its own generator (map and
fold leaves as functions, constants, every output of multi-output children: about 70,000 checks a
seed over five seeds), found one output the change got wrong, fixed since:

- **A `map` whose function is a bare `fold` lost its extra after the first item.** The map hands
  its extras to each application from one shared argument list, and `eval_fold` moved its list
  out of it, so from the second item the fold had no list and the map gave nothing. `eval_fold`
  now moves an input only when it was given, and lends a lent one again (`take_given`). The
  language never places such a map (a function is always a composite), but the core's API can;
  `a_map_of_a_bare_fold_keeps_its_extra_for_every_item` holds it, and the generated-graph test now
  folds and maps the bare `map`s and `fold`s too.

And three it couldn't make fail, noted: an edge from an output port a child doesn't have, or into
an input port it doesn't declare, could mislead the giving; the check that a value is never read
again had no memo; and `join` sized its text before checking its items, which could overflow on a
32-bit target (it saturates now). The audit made the first fail and fixed the second.

### The audit, before it was merged

Three audits ran before the change went to main (#43), for correctness, security and performance.

**Correctness.** A differential harness (`crates/holon-core/tests/differential.rs`, with
`differential.sh` to run it against any earlier commit) generates worlds of leaves, composites
nested four deep and maps and folds of them, with valid edges and with edges only decoding would
take, and runs every case under each observer a host uses, comparing outputs and every observer
call between two versions. It mattered that the unvalidated edges were in: a host never calls
`validate` on a unit it stores (the server's `POST /unit` only decodes), so those edges can reach
the evaluator. It found, besides the bare `fold` above, the two the review noted:

- a choice that lends was read as its value at an output port it doesn't have (a choice unit
  declaring a second output passes `validate`): it now reads through only at port 0;
- a choice fed at a port it doesn't declare wasn't counted among the value's readers, so the
  value could be given away under it, and a lookup could run past its child's entries (a panic):
  a choice's two ports count whatever it declares, and an entry is looked up within its child's.

With them fixed: no divergence from the base in 1,000,000 cases (5,246,846 evaluations), run again
on the merged code, and the eager oracle disagreeing with both in the same 611 cases, all odd
interfaces or unvalidated edges.

**Security and resilience** (`tests/audit_security.rs`, `examples/audit_stack_probe.rs`). No panic
on any unit that decodes; no shape kept for one unit handed to another; no limit skipped on the
giving path (every leaf with a limit isn't one that builds on a given value); memory flat over
2,000,000 calls on one thread. Two found: `holon verify` ran a record's units without the bounds
the server holds a closure to (64 deep, 20,000 units), so a crafted record nested deeper could
overflow its stack, and it now refuses a unit past them; and the change's frames were larger, so
maps nested in maps evaluated only 200 levels deep on a 1 MiB thread, where the base reached 500.
What only a leaf needs is now out of line: 400 levels on 1 MiB and 850 on 2 MiB, for maps in maps
and composites alike, against the base's 500 and 1000 and a host's 64. The never-read check (`unneeded`) now
looks at each child once a question, so its work is the composite's size whatever its shape.

**Performance** (`benches/perf_audit.rs`, `benches/host_request.rs`). Speculative giving, a
`coalesce`'s fallback given ahead, stays: without it four folds go O(n²) again, 31 to 232 times
slower at 4096. The in-place regression was placed and fixed (§6). The never-read check's cap of 8
kept a fold's accumulator from being given behind four or more untaken branches (4096 items: 44
ms); with the memo the cap is 64 (3.5 to 3.8 ms). What is slower as merged is in §6.
