# The races: closing JavaScript's library gaps, by measurement

[javascript-library.md](../javascript-library.md) maps JavaScript's library onto Holon's leaves;
its §0 sets the rule and its §11 sorts each gap into a composite or a native leaf on reasons. These
pages decide each gap **by measurement**: for every gap a composite can close, the composite is
written in TypeScript, compiled by the reader, checked against what JavaScript gives, and raced
through the real `eval` against a native candidate.

| Page | Covers |
|---|---|
| [leaves.md](leaves.md) | every pure leaf and `map`/`fold`: what each costs, what composites over it pay, and the evaluator and leaf changes that make them cheaper without changing a value or a hash |
| [numbers.md](numbers.md) | `Math`, `Number`, the numeric operators, `parseInt`/`parseFloat`/`isNaN`/`isFinite` |
| [texts.md](texts.md) | `String.prototype`, `String.*`, text operators and comparison, regular expressions, URI functions |
| [lists.md](lists.md) | `Array.prototype`, `Array.*`, list access and `.length` |
| [records.md](records.md) | `Object.*`, `JSON` options, `in`/`delete`/spread, `typeof`, `structuredClone`, dates |
| [rerace.md](rerace.md) | every race run again once the evaluator change of leaves.md was merged, and the verdicts it moved |

## The decision rule

For each gap, in this order:

1. **Already there:** an existing leaf computes exactly JavaScript's result; the gap is the
   reader's (it doesn't read the form yet). Verdict: *reader*, naming the leaf.
2. **Not expressible:** no composite of existing leaves gives JavaScript's result on every input
   Holon has (numbers are finite IEEE doubles, `-0` included; texts are Unicode scalar values; no
   `null`, `NaN` or `Infinity`). Verdict: *leaf*, with the reason a composite can't (the atom it
   lacks, or the input it gets wrong). No race.
3. **Expressible:** the composite is written, checked equal to the native candidate on edge cases
   (`-0`, empty, one item, a match first and last, astral characters for texts), and raced at
   sizes. Verdict:
   - *composite* when it is in the same complexity class (its ratio to the candidate doesn't grow
     with `n`: the ratio at the largest size is at most 1.5× the ratio at the smallest) **and** its
     ratio at the representative size (n = 64, or the scalar) is **at most 3**;
   - a scalar's representative measure is mapped over 64 items (`race::mapped`), where a library
     call is hot; one call also pays the call's preparation, which [leaves.md](leaves.md) §8
     proposes to keep in the resolver;
   - *leaf* when the ratio grows with `n` (a worse class), or exceeds 3; a scalar operation paid in
     inner loops (arithmetic, comparison, rounding: COMPOSABLE_VM_DESIGN.md §13's "hot and
     pervasive") is a *leaf* above **2**.
   - A composite that loses only because its `fold` copies its accumulator (`append`, `extend`,
     `insert`, `with_field` grow a copy) or can't stop early is marked *leaf, re-race after the
     evaluator change* ([leaves.md](leaves.md)), and re-raced once that lands.
4. **Refused by design:** no Holon meaning (javascript-library.md §12). Verdict: *refused*, naming
   Holon's form.

A *leaf* verdict comes with its spec: name (catalog style, `snake_case`), inputs and params, the
exact JavaScript meaning including edge cases, what it gives where JavaScript would throw or give
`NaN`/`Infinity`/`null` (Holon gives nothing), how it stays bit-identical natively and in wasm
(IEEE-exact, or `libm` 0.2.16 pinned, never `f64::powf`/`sin`/…), and the JavaScript form the
reader maps to it. A *composite* verdict comes with its TypeScript text, as raced.

## The harness

`crates/holon-author/benches/race/mod.rs`, shared by the five bench files
`crates/holon-author/benches/races_{evaluator,numbers,texts,lists,records}.rs`:

- `function(functions, name, arity) -> Composite`: compiles TypeScript `function` declarations
  (with any `import { … } from "holon"` lines first) through the reader and returns the named
  function's unit and store. A leaf with no JavaScript form is called by name:
  `import { text_length, slice } from "holon"`.
- `leaf(name, params) -> Unit`: an existing catalog leaf.
- `Side::{Composite(&c), Leaf(&unit), Native(fn)}`: a contender. A native candidate is a
  `fn(&[&Value]) -> Option<Value>`, timed behind one leaf's dispatch through `eval`, as a real leaf
  would be reached.
- `check(name, &a, &b, &cases)`: panics unless both sides give the same output on every case,
  numbers compared by bits (`same`).
- `race(name, &a, &b, &sizes, |n| inputs)`: prints a markdown table, one row per size: each side's
  median time per call and `a` over `b`.
- `v(json)`, `num`, `text`, `numbers(n)`: values.

Run one bench, or the races whose name contains a filter:

```sh
cargo bench -p holon-author --bench races_numbers
cargo bench -p holon-author --bench races_numbers -- abs
```

Times are medians of seven samples of about 20 ms each; other builds running at the same time add
noise, so a verdict near a threshold is re-raced before it stands.
