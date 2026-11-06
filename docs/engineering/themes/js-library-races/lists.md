# Lists: how each gap closes

As of 2026-11-06, on c6d350e. The races are `crates/holon-author/benches/races_lists.rs`
(`cargo bench -p holon-author --bench races_lists`, about 100 s). Times come from a Windows desktop
while four other agents built in the same target directory, so single ratios move by about ±30%
from run to run, and a single size can be off by 2× (one run reads `some` against `map` at 0.74 at
n = 512). Every verdict within reach of a threshold was re-raced twice (the last tables below), and
a verdict quotes the range across runs.

Three facts of the evaluator decide most rows, and two of them are copies:

- **The fold's copy.** `[...acc, x]` and `[...acc, ...ys]` in a `fold` copy the accumulator each
  step (`append`, `extend`), so `filter`, `flat` and `flatMap` written over `fold` are O(n²). This is
  the copy the evaluator change removes.
- **The share's copy.** A list that a composite, or a function written in place, uses twice
  passes through a shared `guard(true, xs)`. Both walks keep a choice's value as the guard's own
  output, a copy (`eval.rs`: `run_planned`'s `cloned()`, `run_in`'s `slot.owned()`), so every run
  copies the list. The same holds for every `guard` and `coalesce`, so `c ? acc : …` copies `acc`
  too. So `get(xs, length(xs) - 1)` is O(n), where it should be O(1). Inside a function written in
  place, a captured list used twice is copied for every item. The first draft of `toSpliced` and `copyWithin` read the list twice per item and took
  79.7 ms at n = 4096, against 18 µs and 23 µs native (4451× and 3532×). Written to read it once,
  they take 2.5 ms and 2.1 ms. The first race below shows the copy alone: 30 µs against 0.9 µs at
  4096. The fold change doesn't touch this copy, and `filter` pays it as well unless written to use
  its accumulator once (as it is raced here).
- **A function costs an application per item, and a function node costs about 1 µs a call.** A
  composite that applies a function to each item pays about 100–150 ns per item, where a native
  loop pays 2–5 ns. So a list operation that takes no function from its caller loses 20–200× to a
  leaf, at every size: the rule makes it a leaf. Only higher-order methods, whose leaf would pay
  the same applications, can be composites. Against those, the races compare the composite with
  `map` of the same function, the least a leaf applying it to every item pays. A function called by
  name inside `map` (how `mapped` times a scalar) is a nested composite. `eval_composite` prepares it
  again at each call and walks it without a plan, about 1 µs, which is most of what `at` and
  `Array.isArray` cost below.

`range(length(xs))` stands in for `indices(xs)`, the leaf proposed for a list's positions.
`range` gives nothing past MAX_RANGE (10000) items, and a list can be longer, so a composite over
`range` is inexact there. The bench asserts it for `keys`, `entries`, `map((x, i) => …)` and
`findLastIndex`, and `indices` is what makes them exact.

## Verdicts

The rule is the README's. "Map" in the Measured column is the composite's time over `map` (or
`reduce`) of the same function, given for higher-order rows. The sizes are n = 64 and the largest
raced.

| JavaScript | Verdict | How | Measured (sizes: composite / candidate = ratio) | Why (one line, citing the rule) |
|---|---|---|---|---|
| `xs.length` | reader | `length` | checked equal | already there: read `.length` on a list as `length` |
| `xs[i]` | reader | `get` | checked equal (`-1`, `1.5`, `-0`, `2**32`) | already there: a negative or fractional index is no array index in JavaScript either; read `xs[i]` on a list as `get` |
| `xs.at(-1)` | leaf | `at` | mapped over 64 lists: 64: 102 µs / 2.3 µs = 44; 4096: 675 | grows with n (the share's copy) and > 3 at 64 (a function node per call) |
| `at(i)` | leaf | `at` | mapped over 64 lists, `at(-2)`: 64: 154 µs / 2.1 µs = 73; 4096: 527 | as `at(-1)`; a fractional `i` would also need `Math.trunc` |
| `concat` | reader | `extend` (`append` for an item that isn't a list) | checked equal on lists | already there, as `[...a, ...b]`; which leaf depends on the argument's type |
| `copyWithin` | leaf | `copy_within` | 64: 65.4 µs / 432 ns = 151; 4096: 58 | > 3: a function applied per item where the leaf copies once. Value form only |
| `entries` | leaf | `entries` | 64: 14.4 µs / 2.32 µs = 6.2; 4096: 6.8 | same class, > 3; over `range`, also inexact past MAX_RANGE |
| `every` | leaf, re-race once a fold can stop | `all {fn}` | first fails: 64: 208, 4096: 16390; all pass: 64: 163; map: 1.3–2.1 | no early exit: the ratio grows with n where JavaScript stops at the first false |
| `fill` | leaf | `fill` | 64: 27.1 µs / 376 ns = 72; 4096: 51 | > 3: a function per item where the leaf copies once. Value form only |
| `filter` | leaf (re-raced after the evaluator change) | `filter {fn}` | re-raced on f764335: all kept: 64: 51.4 µs / 476 ns = 108 (108–111); 512: 151–156; map: 8: 4.3–4.9, 64: 5.9–7.7, 512: 6.2–7.8 (before the change: 64: 8.1–9.3, 512: 26–38) | same class as `map` now (the fold's copy is gone: 5.9 → 6.2 from 64 to 512), but > 3 at 64: a `fold` step and its `extend` per item cost about 6× `map`'s application |
| `find` | leaf | `find {fn}` | first: 64: 695, 4096: 38848; none: 64: 406; map: 3.5–3.6 | > 3 against `map` even when nothing matches, and no early exit |
| `findIndex` | leaf | `find_index {fn}` | first: 64: 424, 4096: 23911; none: 64: 424; map: 3.6–4.1 | > 3 against `map`, and no early exit |
| `findLast` | leaf, re-race once a fold can stop | `find_last {fn}` | match last: 64: 497, 4096: 27709; none: 64: 225; map: 2.2–2.6 | no early exit: JavaScript stops at the last match, scanning from the end |
| `findLastIndex` | leaf, re-race once a fold can stop | `find_last_index {fn}` | match last: 64: 236, 4096: 34747; none: 64: 145; map: 2.2–2.5 | no early exit, as `findLast`; the composite reads `indices` |
| `flat()` | leaf | `flat {depth}` | 64: 117 µs / 862 ns = 136; 512: 408 | > 3 at 8 already (74), then the fold's copy makes it grow |
| `flat(depth)` | leaf | `flat {depth}` | `flat(2)`: 64: 464 µs / 1.05 µs = 442; 512: 1724 | as `flat()`, once per level; `flat(Infinity)` has no composite (no bound on the depth) |
| `flatMap` | composite, once `flat` is a leaf | `flat(xs.map(f))` | as a fold: 64: 130, 512: 569; map: 8: 3.3–3.8, 64: 4.7–5.0, 512: 12 | the fold form loses to the copy; over the `flat` leaf it pays `map` plus one native pass (re-race then) |
| `forEach` | refused | `xs.map(f)` | | a statement per item has no Holon meaning; `map` computes the values (§12) |
| `includes` | leaf | `includes` | first: 64: 143, 4096: 11319; none: 64: 50, 4096: 85 | > 3 even when nothing matches (an equality per item through a function), and no early exit |
| `indexOf` | leaf | `index_of` on lists | first: 64: 501, 4096: 30677; none: 64: 188 | > 3, and no early exit. The reader maps `.indexOf` to `index_of` today, which answers nothing on a list |
| `join(sep)` | reader | `join` | checked equal on texts | already there (mapped) for texts. Other items need `String(x)`, as `join()` says |
| `join()` | reader | `join(xs, ",")`; non-texts: `xs.map((x) => String(x)).join(sep)` | of numbers: 64: 11.1 µs / 4.88 µs = 2.3 (2.2–2.3); 4096: 2.0 | `","` is a literal. Over non-texts the composite is the same class and ≤ 3, exact once `String(x)` is JavaScript's ToString (`as_text`, raced here, differs at `1e21`, on lists and records) |
| `keys` | leaf | `indices` | 64: 791 ns / 171 ns = 4.6; 4096: 1.1 | not expressible exactly: `range(length(xs))` gives nothing past MAX_RANGE. In class otherwise, and > 3 at 64 only from the composite's fixed cost |
| `lastIndexOf` | leaf | `last_index_of` | match last: 64: 245, 4096: 13450; none: 64: 100 | > 3, and no early exit from the end |
| `map(f)` | reader | `map` | | already mapped |
| `map((x, i) => …)` | composite | `indices(xs).map((i) => f(get(xs, i), i))` | map: 64: 1.60–1.66; 4096: 1.3–1.7 (native, `x + i`: 64: 20) | same class as `map`, ≤ 3: an index costs `get` and a capture per item |
| `pop` | refused | `xs.at(-1)`, `xs.slice(0, -1)` | | mutation, answering the item: no in-place change (§12) |
| `push` | refused | `[...xs, x]` (`append`) | | mutation, answering the new length: the value form is the new list |
| `reduce(f, init)` | reader | `fold` | | already mapped |
| `reduce(f)` | composite | `tail(xs).reduce(f, head(xs))` | against `reduce` with a start: 64: 1.1–1.2 (one noisy run 0.64); 4096: 1.05–1.10 | same class, ≤ 3. An empty list throws in JavaScript; here `head` gives nothing |
| `reduceRight` | composite | `xs.toReversed().reduce(f, init)` over `reverse`; raced over `indices` | over `indices`, against `reduce`: 64: 2.0–2.3 (one noisy run 3.1); 4096: 1.8–2.1 | same class, ≤ 3 over `indices`. Over the `reverse` leaf it pays one native pass more than `reduce` |
| `reverse` | leaf | `reverse` | as `toReversed` | as `toReversed`; read as its value form (Reader changes) |
| `shift` | refused | `head(xs)`, `tail(xs)` | | mutation, answering the item |
| `slice` | leaf | `sublist` | `slice(1, -1)`: 64: 12.3 µs / 271 ns = 45; 4096: 29 | > 3: a function per item where the leaf copies once |
| `some` | leaf, re-race once a fold can stop | `any {fn}` | first: 64: 250, 4096: 13975; none: 64: 142; map: 1.2–2.0 | no early exit: the ratio grows with n where JavaScript stops at the first true |
| `sort()` | leaf | `sort` | | not expressible: no composite sorts in O(n log n) (`fold` inserts in O(n²)) |
| `sort(cmp)` | leaf | `sort_by {fn}` | | not expressible, as `sort()` |
| `splice` | refused | `xs.toSpliced(…)`, `xs.slice(s, s + k)` | | mutation, answering the removed items |
| `toReversed` | leaf | `reverse` | 64: 12.5 µs / 262 ns = 48; 4096: 44 | > 3: a function per item where the leaf copies once |
| `toSorted` | reader | `sort`, `sort_by {fn}` | | the `sort` rows' leaves |
| `toSpliced` | leaf | `to_spliced` | 64: 45.1 µs / 289 ns = 156; 4096: 97 | > 3: a function per item where the leaf copies once |
| `unshift` | refused | `[x, ...xs]` | | mutation, answering the new length |
| `values` | reader | the list itself | | `[...xs.values()]` is `xs`: no leaf |
| `with` | leaf | `with_item` | 64: 16.2 µs / 267 ns = 61; 4096: 117 | > 3: a function per item where the leaf copies once |
| `Array.from(xs)`, `Array.from(xs, f)` | reader | the list itself; `map` | | already there |
| `Array.from({ length: n }, (_, i) => f(i))` | reader | `range(n).map((i) => f(i))` | | already there (`range`, n ≤ MAX_RANGE by design). `() => v` needs the reader to take an unused parameter |
| `Array.of(a, b)` | reader | `list` (`[a, b]`) | | already there |
| `Array.isArray(x)` | leaf | `is_list` (or `type_of`, records.md) | mapped: 64: 56.4 µs / 3.86 µs = 14.6; 4096: 19.8 | the composite `(length(x) ?? -1) >= 0` is exact but > 3 (a function node per call) |
| `[...a, ...b]` | reader | `extend` | checked equal | already mapped |
| `new Set(xs)` | leaf | `unique` | `[...new Set(xs)]`: 64: 488 µs / 11.7 µs = 42; 512: 368 | worse class: each item is compared with every kept one, O(n²) without any copy |

**In numbers:** 50 rows. 13 reader, 4 composite (`map((x, i) => …)`, `reduce(f)`, `reduceRight`,
`flatMap` over `flat`), 23 leaf (`filter` among them, re-raced after the evaluator change), 4 leaf
pending a re-race (`some`, `every`, `findLast`, `findLastIndex` once a fold can stop), 6 refused.

With `sublist` and `to_spliced` as leaves, `with`, `copyWithin` and `pop`'s rest could be
composites of one or two leaf calls (`xs.with(i, v)` as `to_spliced(xs, j, 1, [v])` behind a
range check). That would be three fewer leaves, at one more copy each, and should be raced once
those two exist.

## Leaf specs

Each leaf gives nothing for an input of the wrong kind, and for a number that isn't one where an
index is expected. Indices are truncated toward zero (JavaScript's ToIntegerOrInfinity; `-0` is
`0`), and "relative" means: from the end when negative (`len + k`, at least 0), at most `len`.
None computes a float beyond comparing and counting, so each gives the same bits natively and in
wasm. A higher-order leaf takes its function as `map` and `fold` do, by a `fn` param, with the
inputs after the list passed after the item as captures. Its function's answer is read as
JavaScript's ToBoolean (`false`, `0`, `-0` and `""` are false; anything else is true, an empty list
or record included, unlike `is_truthy`). An application that gives nothing, or waits, makes the
leaf give nothing, or wait, as `map`'s do. A leaf that stops early applies nothing after the item
it stops at.

| Leaf | Inputs, params | Meaning and edges | Gives nothing | JavaScript forms |
|---|---|---|---|---|
| `at` | `list`, `index` | the item at the truncated index, from the end when negative | out of range | `xs.at(i)`, `xs.at(-1)` (and `s.at(i)` if texts.md takes the same name) |
| `indices` | `list` | `[0, 1, …, len - 1]`, with no MAX_RANGE cap (the list bounds it) | | `[...xs.keys()]`, `Array.from(xs.keys())`; the reader's `map((x, i) => …)` |
| `entries` | `list` | `[[0, x0], [1, x1], …]` | | `[...xs.entries()]`, `Array.from(xs.entries())` |
| `any` | `list`, extras; `fn` | true at the first item whose answer is true, stopping there; false otherwise (`[]`: false) | | `xs.some(f)` (`some` is the optional value's leaf) |
| `all` | `list`, extras; `fn` | false at the first item whose answer is false, stopping there; true otherwise (`[]`: true) | | `xs.every(f)` |
| `filter` | `list`, extras; `fn` | the items whose answer is true, in order | | `xs.filter(f)` |
| `find` | `list`, extras; `fn` | the first item whose answer is true, stopping there. Replaces today's `find {name}`: `xs.find((x) => x.id === v)` is this, `v` a capture | none matches | `xs.find(f)` |
| `find_index` | `list`, extras; `fn` | the first such item's position, stopping there; `-1` if none | | `xs.findIndex(f)` |
| `find_last` | `list`, extras; `fn` | the last such item, applying from the end and stopping at the first match | none matches | `xs.findLast(f)` |
| `find_last_index` | `list`, extras; `fn` | its position, from the end; `-1` if none | | `xs.findLastIndex(f)` |
| `includes` | `list`, `item` | whether an item `equals` `item`, stopping at the first: SameValueZero on numbers, texts and booleans; records and lists by value | | `xs.includes(x)`; a start position: refused for now |
| `index_of` | `list` or `text`, `item` | on a list, the first position whose item `equals` `item` (strict equality is SameValueZero on Holon's values), stopping there, else `-1`; on a text, as today | | `xs.indexOf(x)` (already read; the leaf takes a list too, so the reader needs no type) |
| `last_index_of` | `list`, `item` | the last such position, scanning from the end, else `-1` | | `xs.lastIndexOf(x)` |
| `reverse` | `list` | the items in reverse order | | `xs.toReversed()`; `xs.reverse()` as a value |
| `sublist` | `list`, `start`, `end` | the items from relative `start` up to relative `end` (exclusive); `[]` when `end ≤ start` | | `xs.slice(s, e)`; `slice(s)` with `end` the length; `slice()` is `xs`. Named so, since the text `slice` takes a length |
| `to_spliced` | `list`, `start`, `count`, `items` | the list with `count` (truncated, clamped to `0..len - start`) items from relative `start` replaced by `items` | | `xs.toSpliced(s, c, ...items)`; `toSpliced(s)` with `count` the rest |
| `with_item` | `list`, `index`, `value` | the list with the item at the index (from the end when negative) replaced | out of range (JavaScript throws a RangeError) | `xs.with(i, v)` (`with` is a reserved word) |
| `fill` | `list`, `value`, `start`, `end` | the list with the positions from relative `start` to relative `end` set to `value` | | `xs.fill(v, s, e)` as a value; `fill(v)` over the whole list |
| `copy_within` | `list`, `target`, `start`, `end` | the list with the items of relative `start..end` copied from relative `target`, as from the list before (`count` = min(end − start, len − target)) | | `xs.copyWithin(t, s, e)` as a value |
| `sort` | `list` | stable, by JavaScript's default order: each item's `String(x)` (numbers as JavaScript writes them, `-0` as `"0"`, lists as `join(",")`, records as `"[object Object]"`), compared by UTF-16 code units, so an astral character sorts before U+E000–U+FFFF, not by code point as Rust's `str` order is | | `xs.toSorted()`, `xs.sort()` as a value |
| `sort_by` | `list`, extras; `fn` | stable; `fn(a, b, extras…)` answers a number: below 0 puts `a` first, above 0 `b`, `0` and `-0` keep their order. One pinned merge sort, written in Holon, never `slice::sort_by`: Rust's algorithm can change between versions and panics on a comparator that isn't a total order, where JavaScript gives an implementation-defined order. So an inconsistent comparator gives one deterministic order | an answer that isn't a number | `xs.toSorted((a, b) => …)`, `xs.sort(cmp)` as a value |
| `flat` | `list`; `depth` (a whole number, default `1`, or `all`) | FlattenIntoArray: an item that is a list, at a depth above 0, is spread one level less deep; any other item is kept | | `xs.flat()`, `xs.flat(2)` (a literal), `xs.flat(Infinity)` as `depth: all` |
| `is_list` | `value` | whether it is a list | | `Array.isArray(x)`; unneeded if `type_of` (records.md) lands, read as `type_of(x) === "list"` |
| `unique` | `list` | each item's first occurrence, in order, by `equals` (records and lists by value). O(n) by hashing, `-0` hashing as `0` | | `[...new Set(xs)]`, `Array.from(new Set(xs))` |

`index_of`, `last_index_of`, `includes`, `at` and `sublist`/`slice` have text forms in texts.md.
One leaf per name that takes either kind would spare the reader the value's type, which it doesn't
know today. That is why `index_of` takes a list here. The names should be settled with texts.md.

## Composites

As raced. `indices(xs)` replaces `range(length(xs))` in the verdicts.

```ts
// map((x, i) => x + i): an index costs a get and a capture per item.
function js_map_indexed(xs) {
  return range(length(xs)).map((i) => get(xs, i) + i)
}

// reduce((a, b) => a + b), no start: the first item is the start.
function js_reduce_no_init(xs) {
  return tail(xs).reduce((a, b) => a + b, head(xs))
}

// reduceRight((acc, x) => acc * 0.5 + x, init); the verdict's form is xs.toReversed().reduce(f, init).
function js_reduce_right(xs, init) {
  const n = length(xs)
  return range(n).reduce((acc, k) => acc * 0.5 + get(xs, n - 1 - k), init)
}

// join() of items that aren't texts; String(x) in the verdict.
function js_join_any(xs) {
  return xs.map((x) => as_text(x)).join(",")
}

// flatMap((x) => (x > 10 ? [x, x * 2] : x)), as a fold; the verdict's form is flat(xs.map(f)).
function is_list(y) {
  return (length(y) ?? -1) >= 0
}
function flat1(xs) {
  return xs.reduce((acc, y) => [...acc, ...(is_list(y) ? y : [y])], [])
}
function js_flat_map(xs) {
  return flat1(xs.map((x) => (x > 10 ? [x, x * 2] : x)))
}
```

The rows pending a re-race, as they will be re-raced:

```ts
function js_some(xs) {
  return xs.reduce((acc, x) => acc || x > 10, false)
}
function js_every(xs) {
  return xs.reduce((acc, x) => acc && x > 10, true)
}
function js_find_last(xs) {
  return head(xs.reduce((acc, x) => (x > 10 ? [x] : acc), []))
}
function js_find_last_index(xs) {
  return range(length(xs)).reduce((r, k) => (get(xs, k) > 10 ? k : r), -1)
}
// The accumulator used once: only extend's copy is paid, not the share's as well (twice as fast at 512).
function js_filter(xs) {
  return xs.reduce((acc, x) => [...acc, ...(x > 10 ? [x] : [])], [])
}
```

The composites of the leaf rows are in the bench, each checked equal to its candidate. A few are
worth knowing:

- `Array.isArray(x)` is exact as `(length(x) ?? -1) >= 0`: `length` gives nothing for anything but
  a list, and `??` sees that. The same test makes `flat` keep an item that isn't a list.
- `findIndex` and `indexOf` keep one number as their accumulator: the index once found, else
  `-(items seen) - 1`. That is cheaper than a record `{ i, r }` (7.7× `map`, against 3.6× for the
  number), and cheaper than reading `indices` (4.3×).
- `lastIndexOf` and `findLastIndex` read `indices`: `(r, k) => (get(xs, k) === x ? k : r)` is 2.5×
  `map`, where a record accumulator was 7.7×.

## Differences from JavaScript that stay

- **Equality on records and lists.** `includes`, `indexOf`, `lastIndexOf` and `unique` compare by
  `equals`, by value: `[[1]].includes([1])` is true, and `new Set([[1], [1]])` keeps one item, where
  JavaScript compares objects by identity (false, two items). Holon's values have no identity.
  On numbers, texts and booleans nothing differs: Holon has no `NaN`, and `-0` equals `0`, which is
  SameValueZero and strict equality alike.
- **No holes, no `undefined` items.** `[1, , 3]` is refused, and no item is `undefined`, so `find`
  giving nothing always means that nothing matched.
- **What throws gives nothing.** `[].reduce(f)` (TypeError) and `xs.with(9, v)` out of range
  (RangeError) give nothing.
- **Counting is bounded.** `Array.from({ length: n }, f)` past 10000 gives nothing (`range`'s
  MAX_RANGE, by design). `indices` has no cap, since a list it reads already exists.
- **The function sees the item.** JavaScript passes `(item, index, array)`; a leaf's function gets
  the item and its captures. The reader turns an index parameter into `indices` (as `map` does
  above); the array is the list itself, a capture.
- **An inconsistent comparator.** JavaScript's order is implementation-defined; `sort_by` pins one
  merge sort, so it is deterministic, and may differ from V8's on such a comparator.
- **No change in place.** `push`, `pop`, `shift`, `unshift` and `splice` are refused, naming their
  value forms. `sort`, `reverse`, `fill` and `copyWithin` are read as the list they return.
- **`join` of numbers**, until `String(x)` is JavaScript's ToString (numbers.md, texts.md):
  `as_text` writes `1e21` as `1000000000000000000000` and `1e-7` as `0.0000001` (JavaScript: `1e+21`,
  `1e-7`), a list as JSON (`[1,2]`, JavaScript: `1,2`) and a record as JSON (JavaScript:
  `[object Object]`).

## Reader changes

- **By the value's type:** `.length` on a list as `length`, and `xs[i]` as `get`, where the reader
  knows the type (L4 §7). Elsewhere it refuses the form, naming the leaf, as it does today. The
  leaves that take a list or a text (`index_of`, `includes`, `at`, `last_index_of`) need no type.
- **Methods that take a function written in place:** `filter`, `some`, `every`, `find`,
  `findIndex`, `findLast`, `findLastIndex`, `flatMap`, `toSorted`/`sort`, as `map` and `reduce` do
  today ("a function written in place goes to map or reduce").
- **Lowerings:** `map((x, i) => …)` to `indices(xs).map((i) => …)` with `x` read as `get(xs, i)`;
  `reduce(f)` to `tail(xs).reduce(f, head(xs))`; `reduceRight(f, init)` to
  `xs.toReversed().reduce(f, init)`; `flatMap(f)` to `flat(xs.map(f))`; `join()` to `join(xs, ",")`;
  `concat(ys, …)` to `extend` per list (`append` per item that isn't one); `xs.values()`,
  `Array.from(xs)` and `[...xs]` to `xs`; `Array.of(a, b)` to `[a, b]`; `Array.from(xs, f)` to `map`;
  `[...new Set(xs)]` and `Array.from(new Set(xs))` to `unique`; `flat(Infinity)` to `flat {depth: all}`.
- **Mutations:** `xs.sort()`, `reverse()`, `fill(…)` and `copyWithin(…)` read as their value where
  that value is used and `xs` isn't read after, and are refused as a statement, naming the value
  form (`toSorted`, `toReversed`). `push`, `pop`, `shift`, `unshift` and `splice` are refused, naming
  theirs (`[...xs, x]`; `xs.at(-1)` and `xs.slice(0, -1)`; `head(xs)` and `tail(xs)`; `[x, ...xs]`;
  `toSpliced` and `slice`).
- **A limitation that blocked composites:** a parameter a function written in place doesn't use
  is refused ("the parameter "k" is never used"), and so is a body that passes a capture straight
  through. So `Array.from({ length: n }, () => v)` (`range(n).map((k) => v)`) can't be written, and
  `flat(depth)` had to guard its step on the unused index (`k >= 0 ? flat1(acc) : acc`). An unused
  parameter should be allowed (left unwired), and `_` read as one.

## Race output

The full run, verbatim:

### a list used twice, against once

| n | composite used_twice (6 leaves) | composite used_once (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 1.18 µs | 837 ns | 1.41 |
| 64 | 1.37 µs | 869 ns | 1.58 |
| 512 | 3.18 µs | 840 ns | 3.78 |
| 4096 | 29.99 µs | 871 ns | 34.43 |

### at(-2), mapped over 64 lists of n

| n | composite xs.map(at_minus_2) (14 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 127.41 µs | 2.12 µs | 60.16 |
| 64 | 153.69 µs | 2.10 µs | 73.02 |
| 512 | 266.53 µs | 2.10 µs | 126.94 |
| 4096 | 2.41 ms | 4.58 µs | 527.21 |

### at(-1), mapped over 64 lists of n

| n | composite xs.map(js_last) (7 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 66.08 µs | 2.00 µs | 33.00 |
| 64 | 101.96 µs | 2.34 µs | 43.64 |
| 512 | 211.76 µs | 1.98 µs | 107.11 |
| 4096 | 1.37 ms | 2.03 µs | 674.88 |

### keys()

| n | composite js_keys (2 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 705 ns | 75 ns | 9.36 |
| 64 | 791 ns | 171 ns | 4.63 |
| 512 | 1.59 µs | 958 ns | 1.66 |
| 4096 | 11.95 µs | 10.79 µs | 1.11 |

### entries()

| n | composite js_entries (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.62 µs | 328 ns | 11.04 |
| 64 | 14.44 µs | 2.32 µs | 6.21 |
| 512 | 141.66 µs | 18.94 µs | 7.48 |
| 4096 | 1.14 ms | 168.09 µs | 6.76 |

### some, match first

| n | composite js_some (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.60 µs | 40 ns | 64.94 |
| 64 | 9.94 µs | 40 ns | 249.78 |
| 512 | 65.66 µs | 48 ns | 1357.64 |
| 4096 | 551.02 µs | 39 ns | 13974.87 |

### some, no match

| n | composite js_some (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.65 µs | 44 ns | 59.60 |
| 64 | 10.44 µs | 74 ns | 141.73 |
| 512 | 65.16 µs | 468 ns | 139.12 |
| 4096 | 984.53 µs | 1.88 µs | 524.53 |

### some, against map

| n | composite js_some (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.99 µs | 3.89 µs | 0.77 |
| 64 | 17.23 µs | 8.44 µs | 2.04 |
| 512 | 69.02 µs | 50.97 µs | 1.35 |
| 4096 | 958.61 µs | 416.25 µs | 2.30 |

### every, first fails

| n | composite js_every (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.64 µs | 48 ns | 55.58 |
| 64 | 13.79 µs | 66 ns | 208.16 |
| 512 | 94.81 µs | 59 ns | 1617.63 |
| 4096 | 661.27 µs | 40 ns | 16389.58 |

### every, all pass

| n | composite js_every (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.60 µs | 62 ns | 41.85 |
| 64 | 9.84 µs | 61 ns | 162.50 |
| 512 | 66.63 µs | 398 ns | 167.30 |
| 4096 | 517.60 µs | 2.28 µs | 226.58 |

### every, against map

| n | composite js_every (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.56 µs | 2.16 µs | 1.18 |
| 64 | 14.85 µs | 7.08 µs | 2.10 |
| 512 | 65.20 µs | 48.62 µs | 1.34 |
| 4096 | 518.77 µs | 378.30 µs | 1.37 |

### find, match first

| n | composite js_find (15 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.40 µs | 41 ns | 133.23 |
| 64 | 43.11 µs | 62 ns | 694.60 |
| 512 | 231.34 µs | 45 ns | 5195.78 |
| 4096 | 3.81 ms | 98 ns | 38848.15 |

### find, no match

| n | composite js_find (15 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 9.07 µs | 64 ns | 141.50 |
| 64 | 61.16 µs | 151 ns | 405.93 |
| 512 | 219.50 µs | 286 ns | 768.08 |
| 4096 | 1.47 ms | 1.78 µs | 830.75 |

### find, against map

| n | composite js_find (15 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.82 µs | 2.14 µs | 2.26 |
| 64 | 25.57 µs | 7.42 µs | 3.45 |
| 512 | 192.18 µs | 57.83 µs | 3.32 |
| 4096 | 1.51 ms | 393.94 µs | 3.84 |

### findIndex, match first

| n | composite js_find_index (22 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.59 µs | 40 ns | 113.90 |
| 64 | 17.19 µs | 41 ns | 423.71 |
| 512 | 122.43 µs | 40 ns | 3073.42 |
| 4096 | 1.02 ms | 43 ns | 23911.16 |

### findIndex, no match

| n | composite js_find_index (22 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.70 µs | 41 ns | 138.27 |
| 64 | 30.95 µs | 73 ns | 423.91 |
| 512 | 212.70 µs | 268 ns | 793.34 |
| 4096 | 1.78 ms | 1.78 µs | 999.23 |

### findIndex, against map

| n | composite js_find_index (22 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 5.74 µs | 2.15 µs | 2.67 |
| 64 | 30.76 µs | 7.94 µs | 3.87 |
| 512 | 296.65 µs | 48.69 µs | 6.09 |
| 4096 | 1.71 ms | 391.25 µs | 4.38 |

### findLast, match last

| n | composite js_find_last (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.10 µs | 60 ns | 101.91 |
| 64 | 27.15 µs | 55 ns | 497.32 |
| 512 | 206.64 µs | 81 ns | 2565.79 |
| 4096 | 1.63 ms | 59 ns | 27708.59 |

### findLast, no match

| n | composite js_find_last (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.37 µs | 43 ns | 149.62 |
| 64 | 16.19 µs | 72 ns | 225.44 |
| 512 | 115.54 µs | 260 ns | 443.61 |
| 4096 | 911.33 µs | 1.91 µs | 476.01 |

### findLast, against map

| n | composite js_find_last (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.44 µs | 2.40 µs | 1.43 |
| 64 | 19.74 µs | 7.54 µs | 2.62 |
| 512 | 115.77 µs | 58.99 µs | 1.96 |
| 4096 | 879.80 µs | 392.17 µs | 2.24 |

### findLastIndex, match last

| n | composite js_find_last_index (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.12 µs | 43 ns | 95.55 |
| 64 | 18.81 µs | 80 ns | 235.76 |
| 512 | 156.83 µs | 58 ns | 2705.96 |
| 4096 | 2.16 ms | 62 ns | 34746.51 |

### findLastIndex, no match

| n | composite js_find_last_index (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 8.36 µs | 79 ns | 105.40 |
| 64 | 26.62 µs | 183 ns | 145.24 |
| 512 | 241.21 µs | 368 ns | 656.36 |
| 4096 | 2.06 ms | 2.04 µs | 1008.93 |

### findLastIndex, against map

| n | composite js_find_last_index (13 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 7.16 µs | 3.73 µs | 1.92 |
| 64 | 21.97 µs | 9.96 µs | 2.21 |
| 512 | 142.46 µs | 53.04 µs | 2.69 |
| 4096 | 1.16 ms | 440.12 µs | 2.64 |

### filter, all kept

| n | composite js_filter (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 8.29 µs | 141 ns | 58.64 |
| 64 | 78.73 µs | 913 ns | 86.23 |
| 512 | 1.40 ms | 2.89 µs | 484.35 |

### filter, against map

| n | composite js_filter (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 7.78 µs | 2.72 µs | 2.86 |
| 64 | 93.68 µs | 11.61 µs | 8.07 |
| 512 | 2.53 ms | 88.68 µs | 28.49 |

### flatMap

| n | composite js_flat_map (23 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 25.45 µs | 242 ns | 105.07 |
| 64 | 165.61 µs | 1.28 µs | 129.87 |
| 512 | 4.80 ms | 8.44 µs | 568.65 |

### flatMap, against map

| n | composite js_flat_map (23 leaves) | composite xs.map((x) => (x > 10 ? [x, x * 2] : x)) (10 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 33.60 µs | 9.32 µs | 3.61 |
| 64 | 156.32 µs | 33.41 µs | 4.68 |
| 512 | 3.90 ms | 308.93 µs | 12.62 |

### map((x, i) => x + i)

| n | composite js_map_indexed (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.55 µs | 130 ns | 42.85 |
| 64 | 16.05 µs | 816 ns | 19.68 |
| 512 | 143.53 µs | 2.77 µs | 51.79 |
| 4096 | 709.92 µs | 21.55 µs | 32.94 |

### map((x, i) => x + i), against map

| n | composite js_map_indexed (9 leaves) | composite xs.map((x) => x + 1) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 6.09 µs | 2.47 µs | 2.46 |
| 64 | 13.64 µs | 8.54 µs | 1.60 |
| 512 | 130.99 µs | 75.97 µs | 1.72 |
| 4096 | 678.75 µs | 530.61 µs | 1.28 |

### reduce, no start

| n | composite js_reduce_no_init (6 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.60 µs | 46 ns | 55.91 |
| 64 | 9.14 µs | 58 ns | 156.40 |
| 512 | 36.70 µs | 282 ns | 130.23 |
| 4096 | 477.15 µs | 1.98 µs | 241.59 |

### reduce, no start, against reduce with one

| n | composite js_reduce_no_init (6 leaves) | composite xs.reduce((a, b) => a + b, 0) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.52 µs | 3.50 µs | 1.01 |
| 64 | 6.20 µs | 9.62 µs | 0.64 |
| 512 | 44.80 µs | 34.90 µs | 1.28 |
| 4096 | 291.26 µs | 278.12 µs | 1.05 |

### reduceRight

| n | composite js_reduce_right (12 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 7.49 µs | 66 ns | 114.06 |
| 64 | 20.89 µs | 196 ns | 106.74 |
| 512 | 248.57 µs | 841 ns | 295.46 |
| 4096 | 1.57 ms | 6.52 µs | 240.54 |

### reduceRight, against reduce

| n | composite js_reduce_right (12 leaves) | composite xs.reduce((acc, x) => acc * 0.5 + x, init) (4 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 8.61 µs | 2.69 µs | 3.21 |
| 64 | 33.43 µs | 10.73 µs | 3.11 |
| 512 | 153.29 µs | 72.32 µs | 2.12 |
| 4096 | 1.24 ms | 691.51 µs | 1.79 |

### includes, match first

| n | composite js_includes (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.50 µs | 48 ns | 51.82 |
| 64 | 8.53 µs | 60 ns | 143.20 |
| 512 | 52.67 µs | 45 ns | 1172.63 |
| 4096 | 478.51 µs | 42 ns | 11318.97 |

### includes, no match

| n | composite js_includes (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.26 µs | 89 ns | 47.80 |
| 64 | 8.00 µs | 161 ns | 49.86 |
| 512 | 51.90 µs | 1.55 µs | 33.54 |
| 4096 | 641.29 µs | 7.52 µs | 85.24 |

### indexOf, match first

| n | composite js_index_of (21 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 8.44 µs | 45 ns | 188.89 |
| 64 | 22.42 µs | 45 ns | 501.28 |
| 512 | 249.22 µs | 45 ns | 5561.39 |
| 4096 | 1.74 ms | 57 ns | 30677.43 |

### indexOf, no match

| n | composite js_index_of (21 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.40 µs | 51 ns | 105.41 |
| 64 | 28.09 µs | 150 ns | 187.51 |
| 512 | 192.23 µs | 1.02 µs | 188.31 |
| 4096 | 1.82 ms | 7.05 µs | 257.61 |

### lastIndexOf, match last

| n | composite js_last_index_of (12 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.77 µs | 42 ns | 88.99 |
| 64 | 15.90 µs | 65 ns | 244.92 |
| 512 | 110.92 µs | 41 ns | 2683.88 |
| 4096 | 860.32 µs | 64 ns | 13449.99 |

### lastIndexOf, no match

| n | composite js_last_index_of (12 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.40 µs | 50 ns | 128.13 |
| 64 | 15.18 µs | 152 ns | 99.99 |
| 512 | 109.24 µs | 902 ns | 121.15 |
| 4096 | 850.19 µs | 7.24 µs | 117.49 |

### toReversed

| n | composite js_to_reversed (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.48 µs | 89 ns | 38.97 |
| 64 | 12.46 µs | 262 ns | 47.52 |
| 512 | 86.66 µs | 1.71 µs | 50.68 |
| 4096 | 693.92 µs | 15.82 µs | 43.86 |

### slice(1, -1)

| n | composite js_slice (47 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 16.30 µs | 83 ns | 196.77 |
| 64 | 12.32 µs | 271 ns | 45.44 |
| 512 | 56.47 µs | 1.61 µs | 35.07 |
| 4096 | 418.30 µs | 14.51 µs | 28.83 |

### toSpliced(n/2, 2, 3 items)

| n | composite js_to_spliced (60 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 10.70 µs | 97 ns | 110.14 |
| 64 | 45.07 µs | 289 ns | 156.04 |
| 512 | 317.03 µs | 1.79 µs | 176.83 |
| 4096 | 2.50 ms | 25.81 µs | 96.99 |

### with(n/2, 7)

| n | composite js_with (23 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.42 µs | 96 ns | 46.21 |
| 64 | 16.24 µs | 267 ns | 60.79 |
| 512 | 112.43 µs | 1.82 µs | 61.86 |
| 4096 | 1.63 ms | 13.90 µs | 117.48 |

### fill(7, n/4, 3n/4)

| n | composite js_fill (49 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 7.84 µs | 145 ns | 54.13 |
| 64 | 27.07 µs | 376 ns | 71.95 |
| 512 | 221.17 µs | 3.23 µs | 68.51 |
| 4096 | 1.19 ms | 23.15 µs | 51.44 |

### copyWithin(0, n/2, n)

| n | composite js_copy_within (79 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 21.34 µs | 226 ns | 94.63 |
| 64 | 65.42 µs | 432 ns | 151.40 |
| 512 | 455.59 µs | 6.30 µs | 72.27 |
| 4096 | 2.12 ms | 36.38 µs | 58.24 |

### flat()

| n | composite js_flat (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 15.17 µs | 204 ns | 74.41 |
| 64 | 116.85 µs | 862 ns | 135.56 |
| 512 | 1.97 ms | 4.83 µs | 407.98 |

### flat(2)

| n | composite js_flat_depth (27 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 42.19 µs | 231 ns | 182.59 |
| 64 | 463.67 µs | 1.05 µs | 442.46 |
| 512 | 12.00 ms | 6.96 µs | 1723.66 |

### join() of numbers

| n | composite js_join_any (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.69 µs | 626 ns | 4.29 |
| 64 | 11.09 µs | 4.88 µs | 2.27 |
| 512 | 79.81 µs | 42.32 µs | 1.89 |
| 4096 | 618.88 µs | 310.53 µs | 1.99 |

### Array.isArray, mapped

| n | composite xs.map(js_is_array) (5 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 7.66 µs | 390 ns | 19.64 |
| 64 | 56.38 µs | 3.86 µs | 14.59 |
| 4096 | 4.69 ms | 237.02 µs | 19.80 |

### [...new Set(xs)]

| n | composite js_unique (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 17.59 µs | 756 ns | 23.28 |
| 64 | 487.86 µs | 11.72 µs | 41.63 |
| 512 | 32.30 ms | 87.70 µs | 368.29 |

The rows near a threshold, re-raced (`-- against`, twice, then `-- join`):

### a list used twice, against once

| n | composite used_twice (6 leaves) | composite used_once (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 1.17 µs | 848 ns | 1.38 |
| 64 | 1.38 µs | 878 ns | 1.57 |
| 512 | 3.04 µs | 843 ns | 3.61 |
| 4096 | 24.88 µs | 857 ns | 29.04 |

### some, against map

| n | composite js_some (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.55 µs | 2.17 µs | 1.17 |
| 64 | 9.84 µs | 7.53 µs | 1.31 |
| 512 | 69.46 µs | 93.70 µs | 0.74 |
| 4096 | 1.05 ms | 682.62 µs | 1.53 |

### every, against map

| n | composite js_every (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.69 µs | 2.17 µs | 1.24 |
| 64 | 9.25 µs | 6.93 µs | 1.33 |
| 512 | 63.00 µs | 49.60 µs | 1.27 |
| 4096 | 563.18 µs | 381.96 µs | 1.47 |

### find, against map

| n | composite js_find (15 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.64 µs | 2.08 µs | 2.23 |
| 64 | 24.24 µs | 6.96 µs | 3.48 |
| 512 | 181.43 µs | 47.25 µs | 3.84 |
| 4096 | 1.43 ms | 377.00 µs | 3.80 |

### findIndex, against map

| n | composite js_find_index (22 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 5.56 µs | 2.13 µs | 2.61 |
| 64 | 28.81 µs | 8.02 µs | 3.59 |
| 512 | 215.67 µs | 49.08 µs | 4.39 |
| 4096 | 1.78 ms | 382.79 µs | 4.64 |

### findLast, against map

| n | composite js_find_last (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.49 µs | 2.28 µs | 1.53 |
| 64 | 15.36 µs | 7.04 µs | 2.18 |
| 512 | 111.74 µs | 47.81 µs | 2.34 |
| 4096 | 864.59 µs | 392.02 µs | 2.21 |

### findLastIndex, against map

| n | composite js_find_last_index (13 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.46 µs | 2.29 µs | 1.95 |
| 64 | 18.82 µs | 7.65 µs | 2.46 |
| 512 | 278.82 µs | 94.43 µs | 2.95 |
| 4096 | 1.64 ms | 605.60 µs | 2.70 |

### filter, against map

| n | composite js_filter (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 6.63 µs | 2.10 µs | 3.16 |
| 64 | 77.61 µs | 8.77 µs | 8.85 |
| 512 | 1.79 ms | 47.65 µs | 37.61 |

### flatMap, against map

| n | composite js_flat_map (23 leaves) | composite xs.map((x) => (x > 10 ? [x, x * 2] : x)) (10 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 16.67 µs | 5.06 µs | 3.29 |
| 64 | 147.85 µs | 29.52 µs | 5.01 |
| 512 | 2.62 ms | 226.43 µs | 11.58 |

### map((x, i) => x + i), against map

| n | composite js_map_indexed (9 leaves) | composite xs.map((x) => x + 1) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.33 µs | 2.08 µs | 1.60 |
| 64 | 11.64 µs | 7.01 µs | 1.66 |
| 512 | 79.15 µs | 46.44 µs | 1.70 |
| 4096 | 612.51 µs | 366.35 µs | 1.67 |

### reduce, no start, against reduce with one

| n | composite js_reduce_no_init (6 leaves) | composite xs.reduce((a, b) => a + b, 0) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.13 µs | 1.79 µs | 1.19 |
| 64 | 5.66 µs | 5.06 µs | 1.12 |
| 512 | 35.76 µs | 31.45 µs | 1.14 |
| 4096 | 268.81 µs | 245.32 µs | 1.10 |

### reduceRight, against reduce

| n | composite js_reduce_right (12 leaves) | composite xs.reduce((acc, x) => acc * 0.5 + x, init) (4 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.58 µs | 2.48 µs | 1.85 |
| 64 | 19.40 µs | 9.56 µs | 2.03 |
| 512 | 139.70 µs | 66.92 µs | 2.09 |
| 4096 | 1.10 ms | 523.18 µs | 2.11 |

### a list used twice, against once

| n | composite used_twice (6 leaves) | composite used_once (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 1.15 µs | 832 ns | 1.38 |
| 64 | 1.37 µs | 834 ns | 1.64 |
| 512 | 2.88 µs | 837 ns | 3.44 |
| 4096 | 24.23 µs | 857 ns | 28.27 |

### some, against map

| n | composite js_some (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.57 µs | 2.26 µs | 1.14 |
| 64 | 9.70 µs | 8.08 µs | 1.20 |
| 512 | 71.47 µs | 89.17 µs | 0.80 |
| 4096 | 1.04 ms | 685.80 µs | 1.51 |

### every, against map

| n | composite js_every (5 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.55 µs | 2.24 µs | 1.58 |
| 64 | 9.31 µs | 6.97 µs | 1.34 |
| 512 | 63.53 µs | 48.99 µs | 1.30 |
| 4096 | 650.46 µs | 392.47 µs | 1.66 |

### find, against map

| n | composite js_find (15 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.68 µs | 2.12 µs | 2.21 |
| 64 | 25.26 µs | 7.10 µs | 3.56 |
| 512 | 184.83 µs | 47.76 µs | 3.87 |
| 4096 | 1.45 ms | 382.74 µs | 3.80 |

### findIndex, against map

| n | composite js_find_index (22 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 5.53 µs | 2.11 µs | 2.62 |
| 64 | 28.97 µs | 7.06 µs | 4.10 |
| 512 | 221.03 µs | 59.23 µs | 3.73 |
| 4096 | 1.71 ms | 397.47 µs | 4.31 |

### findLast, against map

| n | composite js_find_last (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.51 µs | 2.35 µs | 1.49 |
| 64 | 15.32 µs | 7.02 µs | 2.18 |
| 512 | 123.53 µs | 47.22 µs | 2.62 |
| 4096 | 864.10 µs | 374.25 µs | 2.31 |

### findLastIndex, against map

| n | composite js_find_last_index (13 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.91 µs | 2.11 µs | 1.86 |
| 64 | 17.70 µs | 7.27 µs | 2.43 |
| 512 | 141.92 µs | 47.64 µs | 2.98 |
| 4096 | 1.08 ms | 384.96 µs | 2.80 |

### filter, against map

| n | composite js_filter (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 6.15 µs | 2.11 µs | 2.92 |
| 64 | 66.11 µs | 7.09 µs | 9.33 |
| 512 | 1.25 ms | 47.82 µs | 26.08 |

### flatMap, against map

| n | composite js_flat_map (23 leaves) | composite xs.map((x) => (x > 10 ? [x, x * 2] : x)) (10 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 19.47 µs | 5.10 µs | 3.82 |
| 64 | 146.54 µs | 30.58 µs | 4.79 |
| 512 | 2.79 ms | 226.74 µs | 12.29 |

### map((x, i) => x + i), against map

| n | composite js_map_indexed (9 leaves) | composite xs.map((x) => x + 1) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 3.25 µs | 2.13 µs | 1.53 |
| 64 | 11.49 µs | 7.07 µs | 1.63 |
| 512 | 78.77 µs | 47.37 µs | 1.66 |
| 4096 | 616.33 µs | 369.49 µs | 1.67 |

### reduce, no start, against reduce with one

| n | composite js_reduce_no_init (6 leaves) | composite xs.reduce((a, b) => a + b, 0) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 2.14 µs | 1.81 µs | 1.18 |
| 64 | 5.80 µs | 5.01 µs | 1.16 |
| 512 | 35.30 µs | 30.50 µs | 1.16 |
| 4096 | 276.80 µs | 262.74 µs | 1.05 |

### reduceRight, against reduce

| n | composite js_reduce_right (12 leaves) | composite xs.reduce((acc, x) => acc * 0.5 + x, init) (4 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 4.40 µs | 2.53 µs | 1.74 |
| 64 | 26.74 µs | 11.72 µs | 2.28 |
| 512 | 174.76 µs | 130.02 µs | 1.34 |
| 4096 | 2.08 ms | 546.02 µs | 3.80 |

    Blocking waiting for file lock on build directory

### join() of numbers

| n | composite js_join_any (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.71 µs | 618 ns | 4.38 |
| 64 | 11.13 µs | 4.96 µs | 2.24 |
| 512 | 157.83 µs | 40.43 µs | 3.90 |
| 4096 | 716.62 µs | 316.11 µs | 2.27 |

`filter`, re-raced after the evaluator change (#43) on f764335, twice
(`cargo bench -p holon-author --bench races_lists -- filter`):

### filter, all kept (first run)

| n | composite js_filter (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 8.54 µs | 136 ns | 63.01 |
| 64 | 51.43 µs | 476 ns | 108.14 |
| 512 | 399.10 µs | 2.64 µs | 151.22 |

### filter, against map (first run)

| n | composite js_filter (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 8.23 µs | 1.91 µs | 4.32 |
| 64 | 51.26 µs | 8.69 µs | 5.90 |
| 512 | 392.95 µs | 63.66 µs | 6.17 |

### filter, all kept (second run)

| n | composite js_filter (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 8.37 µs | 133 ns | 62.93 |
| 64 | 52.45 µs | 471 ns | 111.29 |
| 512 | 397.81 µs | 2.55 µs | 155.74 |

### filter, against map (second run)

| n | composite js_filter (11 leaves) | composite xs.map((x) => x > 10) (3 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 8.53 µs | 1.73 µs | 4.93 |
| 64 | 50.98 µs | 6.66 µs | 7.66 |
| 512 | 376.70 µs | 48.38 µs | 7.79 |
