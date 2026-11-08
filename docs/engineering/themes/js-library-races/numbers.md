# Numbers: how each gap closes

2026-11-06, base commit `c6d350e`. The times are from the last of three full runs, on a machine
shared with other agents' builds. Between it and the run before, call rows (n = 1) moved by up to
30% and mapped rows mostly by under 10%, with a few outliers up to 2×. Every composite is at least
10× its candidate, so noise decides nothing here; the one race near a threshold, `-x` placed in its
caller's frame, held at 1.21–1.22.

This page applies the decision rule of [README.md](README.md) to `Math`, `Number`, the numeric
operators and the number globals. The bench is `crates/holon-author/benches/races_numbers.rs`:
`cargo bench -p holon-author --bench races_numbers` runs every check and race in under two minutes.

**What the races found, before the table.** Under today's evaluator a composite can't pass the rule
at any size. A function that places one leaf already costs 13.2× a leaf as a call (517 ns against
39 ns) and 17.7× mapped over 64 items, so every composite here is over 11× its candidate at every
size, and most are between 20× and 10,000×. Each verdict below is therefore *leaf*, apart from the
rows that are the reader's (an existing leaf already computes them) or refused. The small
composites are the ones the fixed cost of a call dominates: `x * -1` (2 leaves), `Number.isFinite`
(4), `abs` (8) and `max` (10). Those are the rows to watch when the races are re-run after the evaluator change.

The class test (the ratio at the largest size at most 1.5× the ratio at the smallest) doesn't
separate anything here. Every scalar composite is O(1) per item, like its candidate. In the mapped
races the n = 1 row is mostly `map`'s own cost, paid by both sides, so the ratio rises from n = 1
to n = 64 and then holds flat to n = 4096 within noise. For `Math.max(...xs)` the ratio keeps
rising past n = 64 only because the candidate's one dispatch amortizes: per item, the composite
costs 1.1 µs at both 64 and 4096 items, and the candidate 0.67 ns.

## Verdicts

Measured is the final run: the composite's time over the candidate's for one call, then the ratio
mapped over 64 items. A *projection* puts a one-leaf stand-in where a proposed leaf would go, and
is timed only. *In frame* is a composite's time over the time of a function that places one leaf.
It prices a lowering the reader places inline in its caller (no call is paid), not a library
composite, which pays a call each time.

| JavaScript | Verdict | How (leaf name, or composite name) | Measured (call: composite / candidate = ratio; mapped n=64: ratio) | Why (one line, citing the rule) |
|---|---|---|---|---|
| `Math.abs(x)` | leaf | `math {fn: "abs"}` | call 1.06 µs / 41 ns = 26.2; n=64: 29.6 | expressible (`x <= 0 ? 0 - x : x`), but hot and over 2× (in frame 2.05) |
| `Math.acos(x)` | leaf | `math {fn: "acos"}`, `libm::acos` | | not expressible: JavaScript leaves it implementation-approximated; matching libm's bits means transcribing libm's kernels into leaves |
| `Math.acosh(x)` | leaf | `math {fn: "acosh"}`, `libm::acosh` | | not expressible (as `acos`) |
| `Math.asin(x)` | leaf | `math {fn: "asin"}`, `libm::asin` | | not expressible (as `acos`) |
| `Math.asinh(x)` | leaf | `math {fn: "asinh"}`, `libm::asinh` | | not expressible (as `acos`) |
| `Math.atan(x)` | leaf | `math {fn: "atan"}`, `libm::atan` | | not expressible (as `acos`) |
| `Math.atan2(y, x)` | leaf | `atan2`, `libm::atan2` | | not expressible (as `acos`) |
| `Math.atanh(x)` | leaf | `math {fn: "atanh"}`, `libm::atanh` | | not expressible (as `acos`) |
| `Math.cbrt(x)` | leaf | `math {fn: "cbrt"}`, `libm::cbrt` | | not expressible (as `acos`) |
| `Math.ceil(x)` | leaf | `math {fn: "ceil"}` | call 3.00 µs / 41 ns = 73.9; n=64: 87.6 | expressible by IEEE addition (2^52), but hot and over 2×; over a `floor` leaf (projection) still 20.2 / 23.7 |
| `Math.clz32(x)` | leaf | `math {fn: "clz32"}` | call 9.91 µs / 43 ns = 232.4; n=64: 238.8 | expressible (ToUint32, then a five-step ladder), over 3× |
| `Math.cos(x)` | leaf | `math {fn: "cos"}`, `libm::cos` | | not expressible (as `acos`) |
| `Math.cosh(x)` | leaf | `math {fn: "cosh"}`, `libm::cosh` | | not expressible (as `acos`) |
| `Math.exp(x)` | leaf | `math {fn: "exp"}`, `libm::exp` | | not expressible (as `acos`) |
| `Math.expm1(x)` | leaf | `math {fn: "expm1"}`, `libm::expm1` | | not expressible (as `acos`) |
| `Math.floor(x)` | leaf | `math {fn: "floor"}` | call 2.51 µs / 40 ns = 62.6; n=64: 64.0 | expressible by IEEE addition (javascript-library.md §11 had "no atom drops a fraction"), but hot and over 2× |
| `Math.fround(x)` | leaf | `math {fn: "fround"}` | call 2.58 µs / 40 ns = 64.0; n=64: 73.7 | expressible (Veltkamp's split on 24 bits), over 3× |
| `Math.hypot(a, b, …)` | leaf | `aggregate {fn: "hypot"}`, V8's algorithm | lower bound: call 2.60 µs / 41 ns = 63.1; n=64: 75.0 | composite over a `sqrt` leaf; raced without its last `sqrt(sum) * max` (no leaf takes a root), already over 3× |
| `Math.imul(a, b)` | leaf | `bitwise {op: "imul"}` | call 20.91 µs / 45 ns = 469.3; n=64: 523.6 | expressible (16-bit halves, each product exact), over 3× |
| `Math.log(x)` | leaf | `math {fn: "log"}`, `libm::log` | | not expressible (as `acos`) |
| `Math.log10(x)` | leaf | `math {fn: "log10"}`, `libm::log10` | | not expressible (as `acos`) |
| `Math.log1p(x)` | leaf | `math {fn: "log1p"}`, `libm::log1p` | | not expressible (as `acos`) |
| `Math.log2(x)` | leaf | `math {fn: "log2"}`, `libm::log2` | | not expressible (as `acos`) |
| `Math.max(a, b, …)` | leaf | `max` (variadic) | call 1.07 µs / 41 ns = 26.2; n=64: 32.3 | expressible (`a < b ? b : a - b * -0`, `+0` over `-0`), but hot and over 2× (in frame 2.07) |
| `Math.max(...xs)` | leaf | `aggregate {fn: "max"}` | fold of the composite: n=64: 843.6; n=4096: 1660.6; over a `max` leaf (projection): n=64: 71.0 | O(n) like the candidate, but even a fold of a one-leaf step is over 3× |
| `Math.min(a, b, …)` | leaf | `min` (variadic) | call 1.59 µs / 41 ns = 38.4; n=64: 69.1 | expressible (`-max(-a, -b)`), but hot and over 2× |
| `Math.min(...xs)` | leaf | `aggregate {fn: "min"}` | as `Math.max(...xs)` | the same fold, the same cost |
| `Math.pow(a, b)` | leaf | `power`, `libm::pow` | | not expressible (as `acos`) |
| `Math.random()` | refused | `import { random } from "holon:random"` | | javascript-library.md §12: randomness is imported, one number for the run, recorded |
| `Math.round(x)` | leaf | `math {fn: "round"}` | call 3.24 µs / 42 ns = 78.0; n=64: 91.0 | expressible (nearest-even by addition, a tie moved up, `-0` kept), but hot and over 2× |
| `Math.sign(x)` | leaf | `math {fn: "sign"}` | call 1.42 µs / 60 ns = 23.4; n=64: 37.3 | expressible (`x > 0 ? 1 : x < 0 ? -1 : x`), but hot and over 2× (in frame 2.75) |
| `Math.sin(x)` | leaf | `math {fn: "sin"}`, `libm::sin` | | not expressible (as `acos`) |
| `Math.sinh(x)` | leaf | `math {fn: "sinh"}`, `libm::sinh` | | not expressible (as `acos`) |
| `Math.sqrt(x)` | leaf | `math {fn: "sqrt"}`, IEEE | | correctly rounded by IEEE; a composite would need the exponent found by a ladder (as `%`'s, 646×) and a Dekker residual for the last bit, far more than `floor` (62.6×): not built |
| `Math.tan(x)` | leaf | `math {fn: "tan"}`, `libm::tan` | | not expressible (as `acos`) |
| `Math.tanh(x)` | leaf | `math {fn: "tanh"}`, `libm::tanh` | | not expressible (as `acos`) |
| `Math.trunc(x)` | leaf | `math {fn: "trunc"}` | call 3.54 µs / 41 ns = 86.7; n=64: 101.1 | expressible by IEEE addition, but hot and over 2× |
| `Math.f16round(x)` | leaf | `math {fn: "f16round"}` | call 2.55 µs / 41 ns = 61.4; n=64: 71.0 | expressible (Veltkamp's split on 11 bits), over 3× |
| `Math.sumPrecise(xs)` | leaf | `aggregate {fn: "sum_precise"}` | lower bound: a fold of a one-leaf step, n=64: 71.0 | expressible as Shewchuk's partials in a nested `fold`, each step a TwoSum of six leaves per partial; its fold step alone costs more than the projection's, over 3× |
| `Math.E` | reader | `const` `2.718281828459045` | bits `4005bf0a8b145769` | the literal JavaScript prints reads to the constant's exact double (checked) |
| `Math.LN10` | reader | `const` `2.302585092994046` | bits `40026bb1bbb55516` | as `Math.E` |
| `Math.LN2` | reader | `const` `0.6931471805599453` | bits `3fe62e42fefa39ef` | as `Math.E` |
| `Math.LOG10E` | reader | `const` `0.4342944819032518` | bits `3fdbcb7b1526e50e` | as `Math.E` |
| `Math.LOG2E` | reader | `const` `1.4426950408889634` | bits `3ff71547652b82fe` | as `Math.E` |
| `Math.PI` | reader | `const` `3.141592653589793` | bits `400921fb54442d18` | as `Math.E` |
| `Math.SQRT1_2` | reader | `const` `0.7071067811865476` | bits `3fe6a09e667f3bcd` | as `Math.E` |
| `Math.SQRT2` | reader | `const` `1.4142135623730951` | bits `3ff6a09e667f3bcd` | as `Math.E` |
| `Number(x)` | leaf | `to_number {grammar: "number"}` | | not expressible: a `0x`, `0o` or `0b` literal past 2^53 is rounded once by JavaScript and at each digit by a digit loop (`Number("0x200000000000018")` is 144115188075855904; the loop gives …872) |
| `Number.isFinite(x)` | leaf | `number_test {test: "finite", coerce: "none"}` | call 764 ns / 40 ns = 19.2; n=64: 22.8 | expressible (`(x <= x) ?? false`), over 3× (in frame 1.48) |
| `Number.isInteger(x)` | leaf | `number_test {test: "integer", coerce: "none"}` | call 2.12 µs / 41 ns = 51.2; n=64: 60.2; over a `trunc` leaf (projection): 21.5 / 24.9 | expressible by IEEE addition, over 3× even over a `trunc` leaf |
| `Number.isNaN(x)` | reader | `const` `false` | `x !== x` for the record: call 769 ns / 39 ns = 19.7; n=64: 25.8 | Holon has no NaN, so `false` is exact on every value it has |
| `Number.isSafeInteger(x)` | leaf | `number_test {test: "safe_integer", coerce: "none"}` | call 2.65 µs / 43 ns = 62.2; n=64: 72.3 | expressible, over 3× |
| `Number.parseFloat(s)` | leaf | `to_number {grammar: "float"}` | | the same function as `parseFloat` |
| `Number.parseInt(s, r)` | leaf | `to_number {grammar: "int"}` | | the same function as `parseInt` |
| `Number.EPSILON` | reader | `const` `2.220446049250313e-16` | bits `3cb0000000000000` | as `Math.E` |
| `Number.MAX_SAFE_INTEGER` | reader | `const` `9007199254740991` | bits `433fffffffffffff` | as `Math.E` |
| `Number.MIN_SAFE_INTEGER` | reader | `const` `-9007199254740991` | bits `c33fffffffffffff` | as `Math.E` |
| `Number.MAX_VALUE` | reader | `const` `1.7976931348623157e308` | bits `7fefffffffffffff` | as `Math.E` |
| `Number.MIN_VALUE` | reader | `const` `5e-324` | bits `0000000000000001` | as `Math.E` |
| `Number.NaN` | refused | a computation that fails gives nothing; test with `??` | | §12: Holon has no NaN |
| `Number.POSITIVE_INFINITY`, `NEGATIVE_INFINITY` | refused | as `NaN` | | §12: Holon has no Infinity |
| `x.toFixed(d)` | leaf | `number_format {style: "fixed"}` | | not expressible: the digits come from x's exact binary value (`(1.005).toFixed(2)` is `"1.00"`), which needs arbitrary-precision arithmetic, and no leaf writes digits |
| `x.toPrecision(p)` | leaf | `number_format {style: "precision"}` | | not expressible (as `toFixed`) |
| `x.toExponential(d)` | leaf | `number_format {style: "exponential"}` | | not expressible (as `toFixed`) |
| `x.toString(radix)` | leaf | `number_format {style: "string"}` | | not expressible: a fraction's digits in another radix carry back through the digits written (V8's algorithm) |
| `x.toLocaleString()` | refused | `toFixed`, or a template literal | | §12: no locale; mapping it to `toString` would compile and differ from what it means (`"1,234.5"`) |
| `parseInt(s, r)` | leaf | `to_number {grammar: "int"}` | | not expressible: the radix-16 case of `Number(x)`, and a longest-prefix scan over characters |
| `parseFloat(s)` | leaf | `to_number {grammar: "float"}` | | expressible only as a per-character `fold` (a state machine over `slice`) feeding `json_parse`; the fold step alone is over 3× (the `Math.max(...xs)` projection, 71×) |
| `isNaN(x)` | leaf | `number_test {test: "nan", coerce: "number"}` | | not expressible over `to_number`: `"Infinity"` and `"1e999"` are numbers to JavaScript and nothing to Holon, as `"abc"` is |
| `isFinite(x)` | leaf | `number_test {test: "finite", coerce: "number"}` | over a `to_number` leaf (projection): call 711 ns / 50 ns = 14.2 | a composite over `to_number`, `(n <= n) ?? false`, is exact; over 3× (in frame 1.38) |
| `a % b` | leaf | `remainder` | call 26.66 µs / 41 ns = 646.6; n=64: 416.9 | exactly expressible (a ladder to the top multiple, then exact Sterbenz subtractions in a `fold`); javascript-library.md §11 called it inexact, which holds only for `a - b * Math.trunc(a / b)`; hot and over 2× |
| `a ** b` | leaf | `power` (as `Math.pow`) | | not expressible (as `acos`) |
| `-x` | reader | `multiply(x, -1)` | as a function: call 624 ns / 41 ns = 15.3; in frame: call 604 ns / 498 ns = 1.21; n=64: 1.23 | an existing leaf computes it exactly; placed inline its two nodes cost 1.21× one. `0 - x` gives `0` for `0`, where `-0` is right (checked) |
| `+x` | leaf | `to_number {grammar: "number"}` | | `+x` is `Number(x)` |
| `x++`, `x--` | reader | `add`, `subtract` with 1, after `x ?? 0` (`default`) | | read as a statement and as a value (`x++` the old value, `++x` the new), the change made before the statement it is in; an absent value counts from 0, where JavaScript gives `NaN` |
| `a & b` | leaf | `bitwise {op: "and"}` | call 343.09 µs / 44 ns = 7808; n=64: 9462 | expressible bit by bit (a 32-step `fold`), over 3× |
| `a \| b` | leaf | `bitwise {op: "or"}` | call 333.14 µs / 44 ns = 7601; n=64: 10167 | expressible as `a + b - (a & b)`, over 3× |
| `a ^ b` | leaf | `bitwise {op: "xor"}` | call 340.14 µs / 44 ns = 7768; n=64: 9660 | expressible as `a + b - 2 * (a & b)`, over 3× |
| `~x` | reader | `bitwise(x, -1, {op: "xor"})`, once `bitwise` lands | as `-1 - int32(x)`: call 9.23 µs / 47 ns = 195.5; n=64: 199.9 | `~x` is `x ^ -1` exactly, so `bitwise` computes it |
| `a << b` | leaf | `bitwise {op: "shl"}` | call 26.71 µs / 47 ns = 567.0; n=64: 700.7 | expressible (an exact product, wrapped to int32), over 3× |
| `a >> b` | leaf | `bitwise {op: "shr"}` | call 27.77 µs / 48 ns = 579.7; n=64: 674.4 | expressible (a floored exact quotient), over 3× |
| `a >>> b` | leaf | `bitwise {op: "ushr"}` | call 21.19 µs / 48 ns = 441.6; n=64: 466.7 | expressible (the uint32's floored quotient), over 3× |
| `+=`, `-=`, `*=`, `/=`, `??=`, `\|\|=`, `&&=`; `%=`, `**=`, `&=`, `\|=`, `^=`, `<<=`, `>>=`, `>>>=` | reader | the operator's leaf, as a statement | | statement work, not a leaf: the first seven are mapped today (`ts.rs`, `assignment`); the rest join its table once their operator lowers |

**In counts:** 83 rows. 61 are *leaf*, 18 *reader*, 4 *refused*, and none *composite*. Of the 61
leaves, 23 have an exact composite that was raced. 5 more were priced by a lower bound or a
sibling: `hypot`, `sumPrecise`, the global `isFinite`, `parseFloat` and `Math.min(...xs)`. The
other 33 weren't raced: they aren't expressible, are another row's function under a second name
(`Number.parseFloat`, `Number.parseInt`, `+x`), or, as `sqrt`, would cost more than composites
already far past the threshold.

## Leaf specs

**Family or one leaf each.** Recommended: one leaf per shape, with an `fn` or `op` parameter, as
`compare {op}` already is. The 61 leaf rows then need 11 names: `math`, `max`, `min`, `aggregate`,
`atan2`, `power`, `remainder`, `bitwise`, `number_test`, `to_number`, `number_format`. One leaf
each would need 57, which more than doubles the computing part of the catalog, the editor's
palette, the manual's leaves page and the cross-target parity list. The reasons, by the three
measures:

- **Hashing:** a parameter is part of a unit's hash, so `math {fn: "sin"}` and `math {fn: "cos"}`
  never collide, and adding a function to a family later is append-only, as adding a leaf is: no
  existing unit re-hashes. What a family gives up is when a misspelt name fails. An unknown leaf
  name is refused when the flow is wired; an unknown `fn` gives nothing when it runs (as an
  unknown `compare` `op` does today). So the family's validation must check `fn` against its list
  when the flow is wired, and `compare`'s `op` should get the same check.
- **Catalog size:** 11 names against 57. The trusted surface is the same either way (each `fn`
  value is a meaning core must keep), but a family keeps the list of names an author scans short.
  COMPOSABLE_VM_DESIGN.md §13's point is that the trusted set mustn't balloon unnoticed, and a
  family's `fn` list is the same list in one place.
- **How the reader maps:** `Math.sin(x)` becomes `math` with `fn: "sin"` from one table, the same
  work as mapping it to a leaf `sin`. Calling a leaf by name reads worse as a family
  (`math(x, { fn: "sin" })` instead of `sin(x)`), but with `Math.*` read, a call by name is rare.
- **Dispatch, measured:** `floor` behind a family's parameter lookup and a match over 29 names
  costs 43 ns against `floor`'s 40 ns (1.07), and 1.02 mapped over 64 items. The evaluator could
  also resolve `fn` once, when it resolves the node's leaf.

Shared by every numeric leaf below: a number input is a finite double (`-0` included). An input
that isn't a number gives nothing, where JavaScript coerces (`Math.max("5", 3)` is 5). A result
JavaScript would give as `NaN`, `Infinity` or `-Infinity` is nothing. Each leaf joins
`check_fraction_parity` in `holon-wasm`, which runs random fractions through the native and the
wasm core and pins each result's hash.

### `math {fn}`: one number to a number

- **Inputs:** `x`. **Param:** `fn`, one of the 29 below. **JavaScript forms:** `Math.<fn>(x)`;
  `f16round` and `clz32` by those names.
- **`abs`:** |x|; `abs(-0)` is `0`. **`sign`:** 1, -1, or x itself for `0` and `-0`.
- **`floor`, `ceil`, `trunc`:** IEEE roundToIntegral toward -∞, +∞ and 0. A zero keeps its sign,
  and so does a result rounded to zero: `ceil(-0.5)` and `trunc(-0.5)` are `-0`. IEEE-exact
  (`f64::floor`, `ceil`, `trunc`: an instruction natively, `f64.floor` and so on in wasm).
- **`round`:** JavaScript's: the closest integer, a tie toward +∞ (`round(-2.5)` is -2, `round(2.5)`
  is 3), and `-0` for x in [-0.5, -0]; `round(0.49999999999999994)` is 0. Not `f64::round`, which
  rounds ties away from zero (-3). Exact as `r = floor(x); y = x - r >= 0.5 ? r + 1 : r`, then
  `copysign(0, x)` when y is 0, because `x - floor(x)` is exact for every double.
- **`sqrt`:** correctly rounded by IEEE (`f64::sqrt`; `f64.sqrt` in wasm). `sqrt(-0)` is `-0`;
  x < 0 gives nothing.
- **`cbrt`, `exp`, `expm1`, `log`, `log2`, `log10`, `log1p`, `sin`, `cos`, `tan`, `asin`, `acos`,
  `atan`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`:** `libm` 0.2.16's function of that name
  (pinned in Cargo.lock), never `std`'s. Each gives nothing outside its domain or past overflow:
  `log`, `log2`, `log10` at x <= 0 (`log(0)` is `-Infinity`); `log1p` at x <= -1; `asin`, `acos`
  at |x| > 1; `acosh` at x < 1; `atanh` at |x| >= 1; `exp`, `expm1`, `cosh`, `sinh` past the
  largest double. Signed zeros come through as JavaScript gives them (`sin(-0)`, `tan(-0)`,
  `asinh(-0)`, `expm1(-0)`, `log1p(-0)`, `cbrt(-0)` are `-0`). libm is musl's port of fdlibm,
  the family V8's `base::ieee754` also ports, so most results agree with Node's to the bit.
  JavaScript doesn't require that, and the leaf's meaning is libm's bits. With its default `arch`
  feature, libm uses hardware `sqrt` and `fma` where the target has them; both are correctly
  rounded, so the bits are the software path's.
- **`fround`:** `(x as f32) as f64`: IEEE conversion to binary32, nearest, ties to even. Gives
  nothing for |x| >= 2^128 - 2^103 (3.4028235677973366e38), which rounds to Infinity. `-0` is kept,
  and a negative x that rounds to zero gives `-0`.
- **`f16round`:** binary16, nearest, ties to even; nothing for |x| >= 65520. Without Rust's
  unstable `f16`: x's quantum is 2^(e-10) in binary16's normal range and 2^-24 below it; x is
  divided by it, rounded with `round_ties_even`, and multiplied back. Each step is exact.
- **`clz32`:** the leading zero bits of ToUint32(x): 32 for 0, 0 for any x whose ToUint32 is
  2^31 or more. ToUint32 is `x.trunc().rem_euclid(2^32)`, exact.

### `max`, `min`: numbers to a number, variadic

- **Inputs:** `a`, `b`, … (variadic as `add` is, in `VARIADIC`). **JavaScript forms:**
  `Math.max(a, b, …)`, `Math.min(a, b, …)`.
- **Meaning:** the largest or smallest input; `max` takes `0` over `-0` and `min` takes `-0` under
  `0`, whatever the order (`f64::max` may give either). One input gives itself; no input gives
  nothing (JavaScript's `-Infinity` and `Infinity`). IEEE comparisons only, so exact.

### `aggregate {fn}`: a list of numbers to a number

- **Inputs:** `xs`, a list. **Param:** `fn`: `max`, `min`, `hypot`, `sum_precise`. **JavaScript
  forms:** `Math.max(...xs)`, `Math.min(...xs)`, `Math.hypot(a, b, …)` (the reader builds the list)
  and `Math.hypot(...xs)`, `Math.sumPrecise(xs)`. An item that isn't a number gives nothing.
- **`max`, `min`:** as the leaves above, over the items; an empty list gives nothing.
- **`hypot`:** V8's algorithm (`builtins/math.tq`): the largest magnitude m; 0 if m is 0 or the
  list is empty; else each magnitude divided by m, squared, and summed in list order with Kahan's
  compensation, then `sqrt(sum) * m`. Nothing on overflow. It uses IEEE operations only, `sqrt`
  correctly rounded, so it is bit-identical everywhere without libm and matches V8 to the bit.
  JavaScript leaves hypot implementation-approximated: SpiderMonkey and JavaScriptCore may differ
  in the last bit, and Boa folds `f64::hypot` pairwise.
- **`sum_precise`:** the exact sum, rounded once (Shewchuk's partials, as Python's `math.fsum`, or
  an exact accumulator). `-0` for an empty list and for a list of only `-0`s. Nothing when the
  exact sum's rounding overflows, even if partial sums did and came back. IEEE operations only.

### `atan2`, `power`, `remainder`: two numbers to a number

- **`atan2 (y, x)`:** `libm::atan2`. `atan2(±0, -0)` is ±π and `atan2(±0, 0)` is ±0. Finite inputs
  never give NaN. **Form:** `Math.atan2(y, x)`.
- **`power (a, b)`:** `libm::pow`. `x ** 0` is 1 for every x and `1 ** y` is 1. `(-0) ** 0.5` is
  `0`: libm takes its `sqrt` shortcut only for a positive x, as JavaScript requires. Nothing for
  `0 ** -1` (Infinity), `(-8) ** (1/3)` (NaN) and overflow. **Forms:** `a ** b`, `Math.pow(a, b)`,
  `a **= b`. The reader may lower `x ** 2` to `x * x`, since libm returns `x * x` for b = 2 and the
  bits agree.
- **`remainder (a, b)`:** fmod: `a - b * trunc(a / b)` computed exactly, with a's sign. Nothing for
  b = ±0; a = ±0 gives a, and so does |a| < |b|. fmod's result is always representable, so every
  correct implementation agrees; pin `libm::fmod` anyway (Rust's `%` on `f64` is the platform's
  fmod natively and compiler-builtins' in wasm). **Forms:** `a % b`, `a %= b`.

### `bitwise {op}`: two numbers to an int32 or uint32

- **Inputs:** `a`, `b`. **Param:** `op`: `and`, `or`, `xor`, `shl`, `shr`, `ushr`, `imul`.
- **Meaning:** ToInt32 and ToUint32 as `clz32` above. `and`, `or`, `xor`: ToInt32(a) op
  ToInt32(b). `shl`: ToInt32(a) << (ToUint32(b) & 31), wrapping in int32. `shr`: the same with an
  arithmetic shift. `ushr`: ToUint32(a) >>> (ToUint32(b) & 31), a uint32 from 0 to 2^32 - 1.
  `imul`: ToInt32(a) × ToInt32(b) modulo 2^32, as an int32 (`wrapping_mul`). The result is never
  `-0`. Integer operations, so exact.
- **Forms:** `a & b`, `a | b`, `a ^ b`, `a << b`, `a >> b`, `a >>> b`, `Math.imul(a, b)`, their
  compound assignments, and `~x` as `bitwise(x, -1, {op: "xor"})`.

### `number_test {test, coerce}`: a value to a boolean

- **Inputs:** `x`, any value. **Params:** `test`: `finite`, `integer`, `safe_integer`, `nan`;
  `coerce`: `none` or `number`.
- **`coerce: "none"`** (the `Number.is…` functions): a value that isn't a number is false. For a
  number: `finite` is true, `integer` is `x === trunc(x)`, `safe_integer` is an integer with
  |x| <= 2^53 - 1, and `nan` is false.
- **`coerce: "number"`** (the globals): x through ToNumber first (`to_number`'s meaning below,
  keeping NaN and ±Infinity inside the leaf), then the test. `isNaN("abc")` is true and
  `isNaN("Infinity")` false; `isFinite("5")` is true and `isFinite("1e999")` false.
- **Forms:** `Number.isFinite`, `Number.isInteger`, `Number.isSafeInteger`, `isNaN`, `isFinite`.
  `Number.isNaN(x)` the reader lowers to `false`. Comparisons and `trunc` only, so exact.

### `to_number {grammar}`: a value to a number

- **Inputs:** `x`, any value, and `radix`, optional, for `int`. **Param:** `grammar`: `number`,
  `float`, `int`.
- **`number`** (ToNumber): a number is itself; `true` is 1 and `false` 0. A text goes through
  StringToNumber. It is trimmed of JavaScript's white space and line terminators (U+FEFF is one,
  U+0085 isn't, unlike Rust's `char::is_whitespace`), and empty is 0. `Infinity`, `+Infinity` and
  `-Infinity` give nothing. `0x`, `0o` and `0b` integers take no sign and are rounded once. A
  decimal literal takes an optional sign, `.5`, `5.` and an exponent, and is rounded correctly.
  `"1_000"`, `"0x"` and `"abc"` give nothing, `"-0"` gives `-0`, and `"1e999"` gives nothing. A
  list is ToNumber of its `join(",")`: `[]` is 0, `[7]` is 7, and two items give nothing. A record
  gives nothing.
- **`float`** (parseFloat): x as text, a number written as `number_format {style: "string"}` writes
  it. Leading white space is trimmed, then the longest prefix that is a decimal literal is read.
  `Infinity` gives nothing, as does no prefix at all. `"1e"` is 1, `"1.2.3"` is 1.2, `"0x1F"` is 0
  and `"-0"` is `-0`.
- **`int`** (parseInt): x as text, trimmed, with an optional sign. The radix: absent or 0 is 10,
  or 16 when a `0x` or `0X` prefix follows; 16 drops that prefix; outside 2 to 36 gives nothing.
  Then the longest prefix of the radix's digits, none giving nothing. The value is rounded once,
  in every radix: JavaScript lets radix 10 drop digits past the 20th and other radices
  approximate, and once-rounding is within that. `"-0"` is `-0`. `parseInt(0.0000005)` is 5,
  because the number is written `"5e-7"` first.
- **Bit identity:** decimal texts through Rust's `f64::from_str` (correctly rounded, the same on
  every target). A radix integer accumulates in a `u128` with a sticky bit past 120 bits and is
  converted once (`as f64` rounds to nearest, ties to even). The bench's `string_to_number` is
  this leaf's `number` grammar.
- **Forms:** `Number(x)`, `+x`, `parseFloat(s)`, `Number.parseFloat(s)`, `parseInt(s)`,
  `parseInt(s, r)`, `Number.parseInt(s, r)`.

### `number_format {style}`: a number to a text

- **Inputs:** `x`, and `digits`, optional (a radix for `string`). **Param:** `style`: `string`,
  `fixed`, `precision`, `exponential`. A digits value outside its range gives nothing, where
  JavaScript throws a RangeError.
- **`string`** (`x.toString(radix)`): with radix absent or 10, Number::toString. That is the
  shortest round-trip digits, in exponent form at 1e21 and above (`"1e+21"`) and below 1e-6
  (`"1e-7"`), and `-0` is `"0"`. A radix of 2 to 36 writes integers exactly, and fractions with
  V8's `DoubleToRadixCString` (IEEE arithmetic: fraction digits until the remaining delta, rounded
  up with a carry back through the digits), so the text matches V8's.
- **`fixed`** (`toFixed(d)`, d from 0 to 100, absent 0): at |x| >= 1e21, as `string`. Otherwise
  the integer n for which n / 10^d - x is closest to zero, from x's exact binary value, taking the
  larger n on a tie. `(1.005).toFixed(2)` is `"1.00"`, `(2.5).toFixed(0)` is `"3"`,
  `(-2.5).toFixed(0)` is `"-3"`, `(-0).toFixed(2)` is `"0.00"` and `(-0.0001).toFixed(2)` is
  `"-0.00"`. Not Rust's `format!("{:.0}", 2.5)`, which rounds an exact tie to even (`"2"`).
- **`precision`** (`toPrecision(p)`, p from 1 to 100, absent as `string`): p significant digits by
  the same exact rule, in exponent form when the exponent is below -6 or at least p.
- **`exponential`** (`toExponential(d)`, d from 0 to 100): d digits after the point by the same
  rule. Absent, as many digits as the shortest round trip needs.
- **Bit identity:** digits from x's exact value by integer arithmetic: Rust's exact formatting at
  a precision past 1074 digits, then a half-up rounding written by hand. The same on every
  target.
- **Forms:** `x.toString()`, `x.toString(r)`, `x.toFixed(d)`, `x.toPrecision(p)`,
  `x.toExponential(d)`, on a value the reader knows is a number.

## Composites

Each as raced. Every one passed its check against its candidate by bits: the edge cases in
`edges()` (signed zeros, halves, `0.49999999999999994`, 2^52 and its neighbours, integers above
2^53, `5e-324`, `±MAX`, the int32 and uint32 boundaries, binary32 and binary16 boundaries and ties)
and 3000 fixed pseudo-random doubles. The binary composites ran on every pair of a small set plus
600 to 1000 random pairs. Rounding by IEEE addition relies on one fact: below 2^52,
`x + 2^52 - 2^52` (`x - 2^52 + 2^52` for x < 0) is x rounded to the nearest integer, ties to even,
exactly.

### `ABS`

Math.abs: `0 - x` at and below zero keeps `-0` out (`x * -1` would give `-0` for `0`).

```ts
function abs(x) {
  return x <= 0 ? 0 - x : x
}
```

### `FLOOR`

Rounding by IEEE addition: below 2^52, `x + 2^52 - 2^52` (or `x - 2^52 + 2^52` for x < 0) is x rounded to the nearest integer, ties to even, exactly; one comparison corrects it to a floor or a ceiling. At and above 2^52 every double is an integer.

```ts
function floor(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return r > x ? r - 1 : r
}
```

### `CEIL`

`x * 0` is `-0` for a negative x: the sign a ceiling of (-1, 0) keeps.

```ts
function ceil(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  const c = r < x ? r + 1 : r
  return c === 0 ? x * 0 : c
}
```

### `TRUNC`

```ts
function trunc(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496 || x === 0) return x
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  const t = x > 0 ? (r > x ? r - 1 : r) : (r < x ? r + 1 : r)
  return t === 0 ? x * 0 : t
}
```

### `ROUND`

The nearest integer, ties to even, then a tie that went down (x - r is exactly 0.5) goes up; [-0.5, 0] keeps x's sign of zero.

```ts
function round(x) {
  if (x >= 4503599627370496 || x <= -4503599627370496) return x
  if (x <= 0 && x >= -0.5) return x * 0
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return x - r === 0.5 ? r + 1 : r
}
```

### `SIGN`

```ts
function sign(x) {
  return x > 0 ? 1 : x < 0 ? -1 : x
}
```

### `MAX`

`a - b * -0`: when a isn't below b it is a, except two zeros, where it is their max (`-0` only when both are). Subtraction, not `a + b * 0`: `add` folds from `+0`, so it gives `0` for `-0 + -0`, where IEEE and JavaScript give `-0`.

```ts
function max(a, b) {
  return a < b ? b : a - b * -0
}
```

### `MIN`

`-max(-a, -b)`, negating by `* -1` (exact).

```ts
function min(a, b) {
  return a > b ? b : (a * -1 - b * 0) * -1
}
```

### `MAX_OF`

```ts
import { head, tail } from "holon"
function max(a, b) {
  return a < b ? b : a - b * -0
}
function max_of(xs) {
  return tail(xs).reduce((m, x) => max(m, x), head(xs))
}
```

### `MAX_OF_LEAF`

Projection: the fold of a one-leaf step, where a `max` leaf would be.

```ts
import { head, tail } from "holon"
function max_of(xs) {
  return tail(xs).reduce((m, x) => m + x, head(xs))
}
```

### `INT32`

ToUint32 and ToInt32: |x| modulo 2^32 (the quotient floored exactly, the product exact, the difference exact by Sterbenz), truncated, and the sign put back modulo 2^32.

```ts
function pfloor(y) {
  const r = y + 4503599627370496 - 4503599627370496
  return r > y ? r - 1 : r
}
function uint32(x) {
  const a = x < 0 ? 0 - x : x
  const q = a / 4294967296
  const m = a - (q >= 4503599627370496 ? q : pfloor(q)) * 4294967296
  const t = pfloor(m)
  return x < 0 && t > 0 ? 4294967296 - t : t
}
function int32(x) {
  const u = uint32(x)
  return u >= 2147483648 ? u - 4294967296 : u
}
```

### `CLZ32`

Raced together with `INT32`.

```ts
function clz32(x) {
  let v = uint32(x)
  if (v === 0) return 32
  let n = 0
  if (v < 65536) {
    n = n + 16
    v = v * 65536
  }
  if (v < 16777216) {
    n = n + 8
    v = v * 256
  }
  if (v < 268435456) {
    n = n + 4
    v = v * 16
  }
  if (v < 1073741824) {
    n = n + 2
    v = v * 4
  }
  if (v < 2147483648) {
    n = n + 1
  }
  return n
}
```

### `IMUL`

The product modulo 2^32 from 16-bit halves, each partial product below 2^53 so exact. Raced together with `INT32`.

```ts
function imul(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  const xh = pfloor(x / 65536)
  const xl = x - xh * 65536
  const yh = pfloor(y / 65536)
  const yl = y - yh * 65536
  const mid = xh * yl + xl * yh
  const lo = mid - pfloor(mid / 65536) * 65536
  const p = lo * 65536 + xl * yl
  const u = p - pfloor(p / 4294967296) * 4294967296
  return u >= 2147483648 ? u - 4294967296 : u
}
```

### `FROUND`

Veltkamp's split, `c - (c - x)` with c = x * (2^29 + 1), is x rounded to nearest on 24 bits, ties to even; below float32's normal range, adding 1.5 * 2^-97 rounds x to a multiple of 2^-149.

```ts
function fround(x) {
  if (x < 1.1754943508222875e-38 && x > -1.1754943508222875e-38) {
    const r = x + 9.466330862652142e-30 - 9.466330862652142e-30
    return r === 0 ? x * 0 : r
  }
  const c = x * 536870913
  return x < 3.4028235677973366e38 && x > -3.4028235677973366e38 ? c - (c - x) : undefined
}
```

### `F16ROUND`

The same on 11 bits (2^42 + 1), and multiples of 2^-24 below float16's normal range.

```ts
function f16round(x) {
  if (x < 0.00006103515625 && x > -0.00006103515625) {
    const r = x + 402653184 - 402653184
    return r === 0 ? x * 0 : r
  }
  const c = x * 4398046511105
  return x < 65520 && x > -65520 ? c - (c - x) : undefined
}
```

### `HYPOT_SUM`

V8's hypot for two arguments, without its last `sqrt(sum) * max` (no leaf takes a square root): a lower bound on the composite once `sqrt` is a leaf. For two values Kahan's compensation stays 0.

```ts
function hypot_sum(a, b) {
  const x = a <= 0 ? 0 - a : a
  const y = b <= 0 ? 0 - b : b
  const m = x < y ? y : x
  if (m === 0) return 0
  const p = x / m
  const q = y / m
  return p * p + q * q
}
```

### `NEG`

```ts
function neg(x) {
  return x * -1
}
```

### `NEG_SUBTRACT`

Not raced: the bench asserts it gives `0` for `0`, where `-x` is `-0`.

```ts
function neg(x) {
  return 0 - x
}
```

### `ONE`

One `default` leaf, the identity on a present value: the least a function can place. The baseline: what a function costs when it places one leaf.

```ts
function one(x) {
  return x ?? 0
}
```

### `IS_FINITE`

A comparison answers nothing for a value that isn't a number: `?? false` makes that false.

```ts
function is_finite(x) {
  return (x <= x) ?? false
}
```

### `IS_NAN`

Holon has no NaN, so this is false for every value; `x !== x` is also JavaScript's own test, true only for NaN, should one ever be made.

```ts
function is_nan(x) {
  return x !== x
}
```

### `IS_INTEGER`

```ts
function is_integer(x) {
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return (x >= 4503599627370496 || x <= -4503599627370496 || r === x) ?? false
}
```

### `IS_SAFE_INTEGER`

```ts
function is_safe_integer(x) {
  const r = x > 0 ? x + 4503599627370496 - 4503599627370496 : x - 4503599627370496 + 4503599627370496
  return ((x >= 4503599627370496 || x <= -4503599627370496 || r === x) && x <= 9007199254740991 && x >= -9007199254740991) ?? false
}
```

### `IS_INTEGER_LEAF`

Projection: `Number.isInteger(x)` as `trunc(x) === x` once `trunc` is a leaf; `x ?? 0` (one `default` leaf, the identity on a present x) stands where `trunc(x)` would.

```ts
function is_integer(x) {
  return ((x ?? 0) === x) ?? false
}
```

### `CEIL_LEAF`

Projection: `Math.ceil(x)` as `-floor(-x)` once `floor` is a leaf, `?? 0` standing for it.

```ts
function ceil(x) {
  return ((x * -1) ?? 0) * -1
}
```

### `IS_FINITE_TEXT`

The global `isFinite(s)` once `to_number` is a leaf, `json_parse` standing for it: checked on texts both read alike.

```ts
import { json_parse } from "holon"
function is_finite_text(s) {
  const n = json_parse(s)
  return (n <= n) ?? false
}
```

### `REM`

`a % b` exactly: |b| scaled by a ladder of powers of two to the largest d = |b| * 2^k not above |a|, then k + 1 steps of `r >= d ? r - d : r` (exact by Sterbenz, as d <= r < 2d) with d halved (exact: every d is |b| times a power of two). The dividend's sign goes back on by `* -1`. The step is written in the lambda: a function returning a record literal is lowered with one output per field, so a fold through it would carry only its first field. The reader refuses a fold item the step doesn't use, so the step keeps the count as a field (one `with_field` a step).

```ts
import { range } from "holon"
function rem_pos(a, y) {
  const x = a < 0 ? 0 - a : a
  if (x < y) return a
  let d = y
  let k = 0
  if (d * 1.0715086071862673e301 <= x) {
    d = d * 1.0715086071862673e301
    k = k + 1000
  }
  if (d * 1.0715086071862673e301 <= x) {
    d = d * 1.0715086071862673e301
    k = k + 1000
  }
  if (d * 1.3407807929942597e154 <= x) {
    d = d * 1.3407807929942597e154
    k = k + 512
  }
  if (d * 1.157920892373162e77 <= x) {
    d = d * 1.157920892373162e77
    k = k + 256
  }
  if (d * 3.402823669209385e38 <= x) {
    d = d * 3.402823669209385e38
    k = k + 128
  }
  if (d * 18446744073709551616 <= x) {
    d = d * 18446744073709551616
    k = k + 64
  }
  if (d * 4294967296 <= x) {
    d = d * 4294967296
    k = k + 32
  }
  if (d * 65536 <= x) {
    d = d * 65536
    k = k + 16
  }
  if (d * 256 <= x) {
    d = d * 256
    k = k + 8
  }
  if (d * 16 <= x) {
    d = d * 16
    k = k + 4
  }
  if (d * 4 <= x) {
    d = d * 4
    k = k + 2
  }
  if (d * 2 <= x) {
    d = d * 2
    k = k + 1
  }
  const s = range(k + 1).reduce((acc, step) => ({ r: acc.r >= acc.d ? acc.r - acc.d : acc.r, d: acc.d / 2, n: step }), { r: x, d: d, n: 0 })
  return a < 0 ? s.r * -1 : s.r
}
function rem(a, b) {
  const y = b < 0 ? 0 - b : b
  return y > 0 ? rem_pos(a, y) : undefined
}
```

### `BITS`

The bitwise operators bit by bit: a `fold` over the 32 bits' values, each step halving both operands exactly and adding the bit's value where both bits are 1; then `a | b` is `a + b - (a & b)` and `a ^ b` is `a + b - 2 * (a & b)` on the uint32s, exactly. A lambda is one expression, so each step floors each half twice (`half` and `low`). Raced together with `INT32`.

```ts
function bit_values() {
  return [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728, 268435456, 536870912, 1073741824, 2147483648]
}
function half(x) {
  return pfloor(x / 2)
}
function low(x) {
  return x - pfloor(x / 2) * 2
}
function and_u(x, y) {
  return bit_values().reduce((acc, p) => ({ x: half(acc.x), y: half(acc.y), r: acc.r + low(acc.x) * low(acc.y) * p }), { x: x, y: y, r: 0 }).r
}
function signed(u) {
  return u >= 2147483648 ? u - 4294967296 : u
}
function and32(a, b) {
  return signed(and_u(uint32(a), uint32(b)))
}
function or32(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  return signed(x + y - and_u(x, y))
}
function xor32(a, b) {
  const x = uint32(a)
  const y = uint32(b)
  return signed(x + y - 2 * and_u(x, y))
}
```

### `NOT32`

Raced together with `INT32`.

```ts
function not32(x) {
  return -1 - int32(x)
}
```

### `SHIFTS`

The shifts: 2^(count & 31) by a ladder of five choices; `<<` wraps the exact product to int32, `>>` floors an exact quotient, `>>>` floors the uint32's. Raced together with `FLOOR` and `INT32`.

```ts
function pow2(s) {
  let p = 1
  let k = s
  if (k >= 16) {
    p = p * 65536
    k = k - 16
  }
  if (k >= 8) {
    p = p * 256
    k = k - 8
  }
  if (k >= 4) {
    p = p * 16
    k = k - 4
  }
  if (k >= 2) {
    p = p * 4
    k = k - 2
  }
  if (k >= 1) {
    p = p * 2
  }
  return p
}
function count(b) {
  const s = uint32(b)
  return s - pfloor(s / 32) * 32
}
function shl(a, b) {
  return int32(int32(a) * pow2(count(b)))
}
function shr(a, b) {
  return floor(int32(a) / pow2(count(b)))
}
function ushr(a, b) {
  return pfloor(uint32(a) / pow2(count(b)))
}
```

## Differences from JavaScript that stay

- **No NaN, no Infinity.** Every leaf gives nothing where JavaScript gives `NaN` or `±Infinity`:
  `Math.log(0)`, `0 ** -1`, `Math.max()` with no arguments, `parseInt("x")`, `Number("abc")`. A
  flow tests for presence (`?? fallback`, `x !== undefined`) where JavaScript would test `isNaN`.
- **No coercion into numeric leaves.** `Math.max("5", 3)`, `"5" & 3`, `"7" % 2` coerce their texts
  in JavaScript and give nothing in Holon, as `add` on texts does today (javascript-library.md
  §9). A flow converts with `Number(s)` first. The reader could insert it where it knows a value
  is a text.
- **Transcendental bits are libm's.** JavaScript leaves `sin`, `exp`, `pow` and the rest
  implementation-approximated; Holon pins libm 0.2.16, so a result may differ in the last bit
  from some engine's. The `js()` oracle, Boa 0.22, computes these with Rust's `std`
  (`f64::sin`, and `Math.hypot` as `f64::hypot` folded pairwise), which calls the platform's
  maths library. So these leaves' oracle tests compare within an ulp or against fixed vectors,
  not by bits.
- **`hypot` is V8's.** Other engines may differ from it in the last bit.
- **`Math.random()`** is `import { random } from "holon:random"`: one number for the run,
  recorded, not a new one each call.
- **`toLocaleString`** stays refused: Holon has no locale.

Three differences found on the way, which shouldn't stay. They are core and reader changes, not
leaves of this page:

- **`add` gives `0` for `-0 + -0`.** `fold_numbers` starts its sum from `+0`
  (`primitives.rs`, `Op::Add`), and `+0 + -0` is `+0`, where IEEE and JavaScript give `-0`.
  Starting from `-0`, which is also an identity for addition and the one IEEE's addition has,
  fixes it without changing any other result. The `max` composite here uses `a - b * -0` to step
  around it.
- **`add`, `subtract`, `multiply` overflow to ±Infinity.** `1e308 * 10` gives `Infinity`, and
  `Infinity - Infinity` gives NaN, values Holon says it hasn't (`determinism.rs` admits finite
  numbers only). Giving nothing on a non-finite result, as this page's leaves do, makes
  `Number.isNaN(x) → false` exact everywhere. Until then, `x !== x` is the exact form.
- **`as_text` writes numbers in Rust's form.** `` `${1e21}` `` gives `"1000000000000000000000"`
  and `` `${1e-7}` `` gives `"0.0000001"`, where JavaScript writes `"1e+21"` and `"1e-7"`. So
  `x.toString()` composed as `` `${x}` `` (javascript-library.md §5) is exact only between 1e-6
  and 1e21. `as_text` and `to_text` should write a number as `number_format {style: "string"}`
  does (texts.md).

## Reader changes

- **`-x`:** `multiply(x, -1)`, exact, `-0` included. Not `0 - x`, which gives `0` for `0` (the
  bench asserts it). The reader's mistake for `-x` today, "to negate a value, write 0 - x" (quoted
  in javascript-library.md §1), should name `x * -1` until `-x` is read. A literal keeps its own form (`-0`, `-3`).
- **`+x`:** `to_number`; the reader may drop it on a value it knows is a number.
- **`Math`:** each member to its leaf (`math {fn}`, `max`, `min`, `atan2`, `power`, `aggregate`,
  `bitwise {op: "imul"}`), and each constant to a `const` of the literal in the table above.
  `Math` is in `GLOBALS` today ("a flow has no globals"); it becomes a namespace the reader knows,
  as it knows `JSON` (`JSON.parse`).
- **`Number`:** the five constants to `const`; `Number.isNaN(x)` to `false`; the rest to their
  leaves. `NaN` and `Infinity` stay refused.
- **Globals:** `parseInt`, `parseFloat`, `isNaN`, `isFinite` are unknown names today; each maps to
  its leaf.
- **Operators:** `%` to `remainder` and `**` to `power`; `&`, `|`, `^`, `<<`, `>>`, `>>>` to
  `bitwise`; `~x` to `bitwise(x, -1, {op: "xor"})`. Today the reader's binary table
  (`lang/ts.rs`, `expr`, around line 2682) knows `+ - * /` and the comparisons, and the language's
  `BinOp` (`lang/ast.rs`) has no others, so either `BinOp` gains these operators or the reader
  lowers them to calls. The statement table in `assignment` gains `%=`, `**=`, `&=`, `|=`, `^=`,
  `<<=`, `>>=`, `>>>=`.
- **Methods on a number:** `toFixed`, `toPrecision`, `toExponential` and `toString(radix)` go to
  `number_format`. `toString` is also a text's and a list's method, so the reader lowers it by the
  value's type where it knows the type, and refuses it, naming the leaf, where it doesn't.
  `.length` needs the same kind of work and is refused today.

What the reader refused or did while these composites were written. None of it blocked a
composite, but each changed how one had to be written:

- **An unused parameter is refused** ("the input "x" is never used"), so a function can't ignore
  its argument (`function isNaN(x) { return false }` doesn't compile). A fold's item must also be
  used: the `%` composite keeps its step count as a record field, one `with_field` a step.
- **`reduce`'s function takes both parameters** (""fold" gives its function 2 inputs (the result
  so far and the item); this one takes 1"), and **a function written in place is one
  expression**. A step that needs a value
  twice computes it twice: the bitwise steps floor each half twice.
- **A function that returns a record literal is lowered with one output per field, and a call
  reads the first.** `function h(acc) { return { r: acc.r + 1, d: acc.d } }` makes `h({ r: 1,
  d: 1 })` answer `2`, not the record. A `fold` stepping through such a function carries a number
  after its first step, and every later `acc.r` is nothing. A record field named as one of the
  function's inputs is refused ("the port "step" is declared twice"), and so is a field that is an
  input passed straight through ("an output can't be an input passed straight through"). In
  JavaScript `h(x)` is the record: this is a "differs" for javascript-library.md §9, not a
  limitation of numbers. Built inside the lambda itself, the record works.
- **An input used twice passes through a `guard(true, x)` node** (`lower.rs`, `connect`), one more
  node and a `const` for each composite that reads its argument twice: most of them.
- **`c ? a : b` fell to `b` when `a` was absent**, since it was `coalesce(guard(c, a), b)`. No
  composite here has a branch that can be absent, so none relied on it. Fixed 2026-11-07: it is
  `guard(c, a) ?? guard(!(c ?? false), b)`, so a chosen `a` that is absent makes it absent, as in
  JavaScript (texts.md, "Reader changes").

## Race output

The bench's printed tables from the final run, verbatim.

### constants

| JavaScript | literal | bits |
|---|---|---|
| `Math.E` | `2.718281828459045` | `4005bf0a8b145769` |
| `Math.LN10` | `2.302585092994046` | `40026bb1bbb55516` |
| `Math.LN2` | `0.6931471805599453` | `3fe62e42fefa39ef` |
| `Math.LOG10E` | `0.4342944819032518` | `3fdbcb7b1526e50e` |
| `Math.LOG2E` | `1.4426950408889634` | `3ff71547652b82fe` |
| `Math.PI` | `3.141592653589793` | `400921fb54442d18` |
| `Math.SQRT1_2` | `0.7071067811865476` | `3fe6a09e667f3bcd` |
| `Math.SQRT2` | `1.4142135623730951` | `3ff6a09e667f3bcd` |
| `Number.EPSILON` | `2.220446049250313e-16` | `3cb0000000000000` |
| `Number.MAX_SAFE_INTEGER` | `9007199254740991` | `433fffffffffffff` |
| `Number.MIN_SAFE_INTEGER` | `-9007199254740991` | `c33fffffffffffff` |
| `Number.MAX_VALUE` | `1.7976931348623157e308` | `7fefffffffffffff` |
| `Number.MIN_VALUE` | `5e-324` | `0000000000000001` |

### abs

| n | composite abs (8 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.06 µs | 41 ns | 26.22 |

### abs, mapped

| n | composite xs.map(abs) (9 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.42 µs | 166 ns | 14.59 |
| 64 | 67.83 µs | 2.29 µs | 29.57 |
| 4096 | 4.21 ms | 141.67 µs | 29.71 |

### floor

| n | composite floor (28 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.51 µs | 40 ns | 62.57 |

### floor, mapped

| n | composite xs.map(floor) (29 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 3.96 µs | 162 ns | 24.43 |
| 64 | 162.11 µs | 2.53 µs | 64.00 |
| 4096 | 15.42 ms | 145.00 µs | 106.36 |

### ceil

| n | composite ceil (34 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 3.00 µs | 41 ns | 73.93 |

### ceil, mapped

| n | composite xs.map(ceil) (35 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 4.56 µs | 165 ns | 27.57 |
| 64 | 203.09 µs | 2.32 µs | 87.55 |
| 4096 | 12.88 ms | 144.88 µs | 88.91 |

### trunc

| n | composite trunc (43 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 3.54 µs | 41 ns | 86.72 |

### trunc, mapped

| n | composite xs.map(trunc) (44 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 5.10 µs | 166 ns | 30.73 |
| 64 | 230.67 µs | 2.28 µs | 101.11 |
| 4096 | 14.68 ms | 142.73 µs | 102.83 |

### round

| n | composite round (37 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 3.24 µs | 42 ns | 78.01 |

### round, mapped

| n | composite xs.map(round) (38 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 4.76 µs | 174 ns | 27.28 |
| 64 | 207.25 µs | 2.28 µs | 91.04 |
| 4096 | 13.15 ms | 141.86 µs | 92.70 |

### sign

| n | composite sign (12 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.42 µs | 60 ns | 23.42 |

### sign, mapped

| n | composite xs.map(sign) (13 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.77 µs | 175 ns | 15.81 |
| 64 | 85.09 µs | 2.28 µs | 37.25 |
| 4096 | 5.23 ms | 142.79 µs | 36.64 |

### fround

| n | composite fround (29 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.58 µs | 40 ns | 64.04 |

### fround, mapped

| n | composite xs.map(fround) (30 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 3.98 µs | 168 ns | 23.67 |
| 64 | 174.92 µs | 2.37 µs | 73.69 |
| 4096 | 11.03 ms | 143.74 µs | 76.76 |

### f16round

| n | composite f16round (29 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.55 µs | 41 ns | 61.39 |

### f16round, mapped

| n | composite xs.map(f16round) (30 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 3.89 µs | 162 ns | 23.99 |
| 64 | 160.79 µs | 2.27 µs | 70.98 |
| 4096 | 9.92 ms | 141.22 µs | 70.21 |

### math family dispatch

| n | native candidate | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 43 ns | 40 ns | 1.07 |

### math family dispatch, mapped

| n | native candidate, mapped | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 164 ns | 160 ns | 1.02 |
| 64 | 2.42 µs | 2.37 µs | 1.02 |
| 4096 | 157.86 µs | 146.44 µs | 1.08 |

### max

| n | composite max (10 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.07 µs | 41 ns | 26.23 |

### max, mapped

| n | composite xs.map((x) => max(x, 200)) (12 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.56 µs | 164 ns | 15.58 |
| 64 | 74.60 µs | 2.31 µs | 32.31 |
| 4096 | 5.04 ms | 142.59 µs | 35.33 |

### min

| n | composite min (14 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.59 µs | 41 ns | 38.41 |

### min, mapped

| n | composite xs.map((x) => min(x, 200)) (16 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 4.94 µs | 171 ns | 28.87 |
| 64 | 160.98 µs | 2.33 µs | 69.13 |
| 4096 | 6.67 ms | 139.87 µs | 47.65 |

### max_of

| n | composite max_of (15 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.55 µs | 41 ns | 37.63 |
| 64 | 71.60 µs | 85 ns | 843.60 |
| 4096 | 4.61 ms | 2.77 µs | 1660.56 |

### max_of over a max leaf (projection)

| n | composite max_of (6 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.54 µs | 42 ns | 36.92 |
| 64 | 6.09 µs | 86 ns | 71.03 |
| 4096 | 311.47 µs | 2.79 µs | 111.69 |

### clz32

| n | composite clz32 (100 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 9.91 µs | 43 ns | 232.42 |

### clz32, mapped

| n | composite xs.map(clz32) (101 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 9.96 µs | 167 ns | 59.61 |
| 64 | 627.83 µs | 2.63 µs | 238.84 |
| 4096 | 40.06 ms | 150.56 µs | 266.05 |

### imul

| n | composite imul (173 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 20.91 µs | 45 ns | 469.29 |

### imul, mapped

| n | composite xs.map((x) => imul(x, 2654435761)) (175 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 20.75 µs | 173 ns | 120.05 |
| 64 | 1.29 ms | 2.47 µs | 523.56 |
| 4096 | 82.73 ms | 146.59 µs | 564.35 |

### hypot without its sqrt (lower bound)

| n | composite hypot_sum (29 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.60 µs | 41 ns | 63.10 |

### hypot without its sqrt (lower bound), mapped

| n | composite xs.map((x) => hypot_sum(x, 3)) (31 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 4.89 µs | 169 ns | 28.99 |
| 64 | 176.58 µs | 2.35 µs | 75.02 |
| 4096 | 11.49 ms | 138.51 µs | 82.99 |

### is_finite

| n | composite is_finite (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 764 ns | 40 ns | 19.15 |

### is_finite, mapped

| n | composite xs.map(is_finite) (5 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.04 µs | 159 ns | 12.79 |
| 64 | 50.21 µs | 2.20 µs | 22.79 |
| 4096 | 3.13 ms | 130.50 µs | 23.95 |

### is_nan

| n | composite is_nan (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 769 ns | 39 ns | 19.70 |

### is_nan, mapped

| n | composite xs.map(is_nan) (5 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.07 µs | 164 ns | 12.57 |
| 64 | 56.16 µs | 2.18 µs | 25.82 |
| 4096 | 3.22 ms | 135.73 µs | 23.72 |

### is_integer

| n | composite is_integer (21 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.12 µs | 41 ns | 51.21 |

### is_integer, mapped

| n | composite xs.map(is_integer) (22 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 3.36 µs | 164 ns | 20.56 |
| 64 | 133.78 µs | 2.22 µs | 60.20 |
| 4096 | 8.70 ms | 139.04 µs | 62.56 |

### is_safe_integer

| n | composite is_safe_integer (26 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 2.65 µs | 43 ns | 62.16 |

### is_safe_integer, mapped

| n | composite xs.map(is_safe_integer) (27 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 3.95 µs | 171 ns | 23.13 |
| 64 | 164.18 µs | 2.27 µs | 72.25 |
| 4096 | 10.79 ms | 136.17 µs | 79.24 |

### is_integer over a trunc leaf (projection)

| n | composite is_integer (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 870 ns | 40 ns | 21.48 |

### is_integer over a trunc leaf (projection), mapped

| n | composite xs.map(is_integer) (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.26 µs | 188 ns | 12.05 |
| 64 | 55.36 µs | 2.22 µs | 24.92 |
| 4096 | 3.46 ms | 152.12 µs | 22.72 |

### ceil over a floor leaf (projection)

| n | composite ceil (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 864 ns | 43 ns | 20.17 |

### ceil over a floor leaf (projection), mapped

| n | composite xs.map(ceil) (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.21 µs | 171 ns | 12.88 |
| 64 | 60.53 µs | 2.56 µs | 23.67 |
| 4096 | 3.63 ms | 134.75 µs | 26.97 |

### isFinite(s) over a to_number leaf (projection)

| n | composite is_finite_text (3 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 711 ns | 50 ns | 14.20 |

### rem

| n | composite rem (192 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 26.66 µs | 41 ns | 646.57 |

### rem, mapped

| n | composite xs.map((x) => rem(x, 7)) (194 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 12.38 µs | 177 ns | 70.10 |
| 64 | 963.51 µs | 2.31 µs | 416.92 |
| 4096 | 97.36 ms | 139.15 µs | 699.70 |

### neg

| n | composite neg (2 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 624 ns | 41 ns | 15.27 |

### neg, mapped

| n | composite xs.map(neg) (3 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 1.91 µs | 162 ns | 11.74 |
| 64 | 40.80 µs | 2.20 µs | 18.52 |
| 4096 | 2.59 ms | 134.60 µs | 19.22 |

### a one-leaf function (baseline)

| n | composite one (1 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 517 ns | 39 ns | 13.17 |

### a one-leaf function (baseline), mapped

| n | composite xs.map(one) (2 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 1.76 µs | 155 ns | 11.37 |
| 64 | 34.32 µs | 1.94 µs | 17.68 |
| 4096 | 2.09 ms | 114.75 µs | 18.25 |

### neg in frame: x * -1 against one leaf

| n | composite neg (2 leaves) | composite one (1 leaves) | ratio |
|---:|---:|---:|---:|
| 1 | 604 ns | 498 ns | 1.21 |

### neg in frame: x * -1 against one leaf, mapped

| n | composite xs.map(neg) (3 leaves) | composite xs.map(one) (2 leaves) | ratio |
|---:|---:|---:|---:|
| 1 | 1.86 µs | 1.74 µs | 1.07 |
| 64 | 41.31 µs | 33.58 µs | 1.23 |
| 4096 | 2.55 ms | 2.08 ms | 1.23 |

### and32

| n | composite and32 (219 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 343.09 µs | 44 ns | 7808.00 |

### and32, mapped

| n | composite xs.map((x) => and32(x, 1431655765)) (221 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 342.94 µs | 164 ns | 2096.42 |
| 64 | 21.88 ms | 2.31 µs | 9462.17 |
| 4096 | 1448.58 ms | 137.18 µs | 10559.66 |

### or32

| n | composite or32 (221 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 333.14 µs | 44 ns | 7601.12 |

### or32, mapped

| n | composite xs.map((x) => or32(x, 1431655765)) (223 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 405.14 µs | 175 ns | 2308.80 |
| 64 | 23.60 ms | 2.32 µs | 10166.74 |
| 4096 | 1546.29 ms | 139.68 µs | 11070.47 |

### xor32

| n | composite xor32 (223 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 340.14 µs | 44 ns | 7768.35 |

### xor32, mapped

| n | composite xs.map((x) => xor32(x, 1431655765)) (225 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 355.31 µs | 171 ns | 2075.16 |
| 64 | 22.15 ms | 2.29 µs | 9660.49 |
| 4096 | 1697.09 ms | 212.65 µs | 7980.51 |

### not32

| n | composite not32 (56 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 9.23 µs | 47 ns | 195.51 |

### not32, mapped

| n | composite xs.map(not32) (57 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 9.45 µs | 181 ns | 52.14 |
| 64 | 479.20 µs | 2.40 µs | 199.94 |
| 4096 | 31.24 ms | 151.01 µs | 206.87 |

### shl

| n | composite shl (222 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 26.71 µs | 47 ns | 566.95 |

### shl, mapped

| n | composite xs.map((x) => shl(x, 3)) (224 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 31.79 µs | 262 ns | 121.43 |
| 64 | 1.93 ms | 2.75 µs | 700.72 |
| 4096 | 138.18 ms | 222.62 µs | 620.67 |

### shr

| n | composite shr (196 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 27.77 µs | 48 ns | 579.74 |

### shr, mapped

| n | composite xs.map((x) => shr(x, 3)) (198 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 25.20 µs | 183 ns | 137.79 |
| 64 | 1.71 ms | 2.53 µs | 674.38 |
| 4096 | 105.09 ms | 154.61 µs | 679.73 |

### ushr

| n | composite ushr (173 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 21.19 µs | 48 ns | 441.57 |

### ushr, mapped

| n | composite xs.map((x) => ushr(x, 3)) (175 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 22.39 µs | 186 ns | 120.51 |
| 64 | 1.24 ms | 2.65 µs | 466.68 |
| 4096 | 84.57 ms | 144.55 µs | 585.07 |
