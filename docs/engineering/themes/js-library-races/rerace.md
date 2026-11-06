# The races, re-raced after the evaluator change

Every race of [numbers.md](numbers.md), [texts.md](texts.md), [lists.md](lists.md) and
[records.md](records.md) was run again on 2026-11-06 once the evaluator change of
[leaves.md](leaves.md) was merged, one bench after another with no other race
running, and every check passed again: the change kept every value the composites give. They ran
on the change as it was before its audit; what the audit changed before the merge (#43) changed
no value, and its own benches measured what it did to time ([leaves.md](leaves.md) §6 and §9).

Each row is a race's ratio (the first side's time over the second's) at its representative size:
n = 64 where it was raced there, else its smallest size. *Before* is the race page's own output,
*after* this run; *growth after* is the ratio at the largest size over the ratio at the smallest
(the rule's class test, at most 1.5). A race whose first side isn't a composite (a projection, a
family's dispatch, a code-unit leaf against a scalar one) is a measurement, not a verdict.

## What changed a verdict

- **`Object.hasOwn`, `k in r`, `r.hasOwnProperty(k)`: composite.** Mapped over 64 keys, the
  composite `has` (6 leaves) is 2.57 times a `lookup` standing in for a leaf (10.44 before), the
  same at every size. One call of it is still 12.6 times, which is the call's preparation
  ([leaves.md](leaves.md) §6, proposal 1), not its work.
- **`s.replaceAll(t, u)` with a text: the leaf stays, the reader may lower the literal case.**
  `split` then `join` is 2.81 times the leaf (5.18 before), but it is exact only when `t` isn't
  empty and `u` has no `$`; a reader that sees both as literals may lower to it.
- **`xs.keys()`: the leaf stays.** Its composite over `range` is 2.81 times at 64 and 1.03 at
  4096, but gives nothing past `range`'s 10000 items, where `indices` doesn't.

Nothing else moved across a threshold. The change roughly halved what a composite pays (abs mapped
29.6 to 14.2, max 32.3 to 14.8, a one-leaf function 13.2 to 6.2) and made every accumulating fold
linear (`Object.groupBy` 61.6 to 7.6 at 64, growth 0.71; `{ ...a, ...b }` 29.2 to 6.5), but a
composite still pays about 30 ns a node where the leaf it stands for pays its whole body in 5 to
40: a composite of k nodes over a scalar costs about k times its leaf, and wins only where it
wraps a leaf that does O(n) work.

## Every race

### numbers: 65 races after, 65 before

| Race | Before | After | Growth after | Within 3 after |
|---|---:|---:|---:|---|
| abs | 26.22 | 18.99 | 1.00 |  |
| abs, mapped | 29.57 | 14.16 | 1.15 |  |
| floor | 62.57 | 46.78 | 1.00 |  |
| floor, mapped | 64.00 | 41.69 | 1.74 |  |
| ceil | 73.93 | 62.67 | 1.00 |  |
| ceil, mapped | 87.55 | 47.32 | 1.86 |  |
| trunc | 86.72 | 70.58 | 1.00 |  |
| trunc, mapped | 101.11 | 52.13 | 2.07 |  |
| round | 78.01 | 62.25 | 1.00 |  |
| round, mapped | 91.04 | 48.47 | 2.31 |  |
| sign | 23.42 | 24.92 | 1.00 |  |
| sign, mapped | 37.25 | 17.08 | 1.30 |  |
| fround | 64.04 | 50.14 | 1.00 |  |
| fround, mapped | 73.69 | 35.68 | 1.67 |  |
| f16round | 61.39 | 46.14 | 1.00 |  |
| f16round, mapped | 70.98 | 34.07 | 1.89 |  |
| math family dispatch | 1.07 | 1.07 | 1.00 | **yes** |
| math family dispatch, mapped | 1.02 | 1.07 | 0.93 | **yes** |
| max | 26.23 | 17.74 | 1.00 |  |
| max, mapped | 32.31 | 14.77 | 1.27 |  |
| min | 38.41 | 29.88 | 1.00 |  |
| min, mapped | 69.13 | 25.10 | 1.50 |  |
| max_of | 843.60 | 326.32 | 24.20 |  |
| max_of over a max leaf (projection) | 71.03 | 92.49 | 6.14 |  |
| clz32 | 232.42 | 177.84 | 1.00 |  |
| clz32, mapped | 238.84 | 136.14 | 2.03 |  |
| imul | 469.29 | 313.21 | 1.00 |  |
| imul, mapped | 523.56 | 266.70 | 2.40 |  |
| hypot without its sqrt (lower bound) | 63.10 | 49.59 | 1.00 |  |
| hypot without its sqrt (lower bound), mapped | 75.02 | 43.54 | 2.12 |  |
| is_finite | 19.15 | 11.70 | 1.00 |  |
| is_finite, mapped | 22.79 | 9.32 | 0.70 |  |
| is_nan | 19.70 | 12.09 | 1.00 |  |
| is_nan, mapped | 25.82 | 9.63 | 0.86 |  |
| is_integer | 51.21 | 37.57 | 1.00 |  |
| is_integer, mapped | 60.20 | 32.19 | 1.83 |  |
| is_safe_integer | 62.16 | 51.82 | 1.00 |  |
| is_safe_integer, mapped | 72.25 | 39.21 | 2.12 |  |
| is_integer over a trunc leaf (projection) | 21.48 | 13.26 | 1.00 |  |
| is_integer over a trunc leaf (projection), mapped | 24.92 | 9.68 | 0.77 |  |
| ceil over a floor leaf (projection) | 20.17 | 14.29 | 1.00 |  |
| ceil over a floor leaf (projection), mapped | 23.67 | 9.90 | 0.81 |  |
| isFinite(s) over a to_number leaf (projection) | 14.20 | 8.42 | 1.00 |  |
| rem | 646.57 | 806.31 | 1.00 |  |
| rem, mapped | 416.92 | 311.79 | 13.48 |  |
| neg | 15.27 | 8.62 | 1.00 |  |
| neg, mapped | 18.52 | 6.86 | 0.72 |  |
| a one-leaf function (baseline) | 13.17 | 6.22 | 1.00 |  |
| a one-leaf function (baseline), mapped | 17.68 | 6.65 | 0.78 |  |
| neg in frame: x * -1 against one leaf | 1.21 | 1.46 | 1.00 | **yes** |
| neg in frame: x * -1 against one leaf, mapped | 1.23 | 1.28 | 1.14 | **yes** |
| and32 | 7808.00 | 3610.70 | 1.00 |  |
| and32, mapped | 9462.17 | 5492.73 | 5.57 |  |
| or32 | 7601.12 | 3825.66 | 1.00 |  |
| or32, mapped | 10166.74 | 5024.92 | 5.30 |  |
| xor32 | 7768.35 | 3993.14 | 1.00 |  |
| xor32, mapped | 9660.49 | 5444.70 | 6.39 |  |
| not32 | 195.51 | 121.51 | 1.00 |  |
| not32, mapped | 199.94 | 83.08 | 2.09 |  |
| shl | 566.95 | 457.17 | 1.00 |  |
| shl, mapped | 700.72 | 326.02 | 2.19 |  |
| shr | 579.74 | 368.05 | 1.00 |  |
| shr, mapped | 674.38 | 268.50 | 1.90 |  |
| ushr | 441.57 | 363.70 | 1.00 |  |
| ushr, mapped | 466.68 | 261.96 | 2.13 |  |

### texts: 34 races after, 34 before

| Race | Before | After | Growth after | Within 3 after |
|---|---:|---:|---:|---|
| text_length, code units (left) against scalars (right) | 1.09 | 1.01 | 1.72 | **yes** |
| s.slice(1, -1), code units (left) against scalars (right) | 1.38 | 1.12 | 1.60 | **yes** |
| s.indexOf(t), t absent, code units (left) against scalars (right) | 1.01 | 1.00 | 1.02 | **yes** |
| s.at(i) | 23.04 | 21.52 | 0.14 |  |
| s.at(3), mapped over 16 texts | 10.94 | 13.31 | 1.22 |  |
| s.charAt(i) | 18.33 | 10.91 | 0.11 |  |
| s.charAt(3), mapped over 16 texts | 7.46 | 8.18 | 0.87 |  |
| s.slice(a, b) | 28.94 | 18.92 | 0.09 |  |
| s.slice(1, -1), mapped over 16 texts | 12.11 | 14.39 | 0.13 |  |
| s.substring(a, b) | 29.84 | 21.89 | 0.11 |  |
| s.includes(t), t absent | 12.94 | 8.85 | 0.12 |  |
| s.includes("zq"), mapped over 16 texts | 5.06 | 6.06 | 0.21 |  |
| s.startsWith(t) | 27.67 | 19.53 | 1.04 |  |
| s.startsWith("The "), mapped over 16 texts | 9.06 | 9.79 | 0.97 |  |
| s.endsWith(t), t of 4 | 41.22 | 27.74 | 28.13 |  |
| s.indexOf(t, p), t absent | 36.22 | 31.91 | 0.07 |  |
| s.lastIndexOf(t) | 246.19 | 329.69 | 7.60 |  |
| s.trimStart(), 4 spaces first | 764.92 | 924.29 | 57.72 |  |
| s.repeat(k), s of 2 | 92.01 | 102.11 | 171.33 |  |
| s.padStart(len, "0"), half filled | 35.97 | 31.94 | 9.91 |  |
| s.padStart(len, "0"), repeat as a leaf (stand-in) | 13.06 | 9.64 | 0.22 |  |
| s.split("") | 5.90 | 6.37 | 1.14 |  |
| s.split(" ", 2) | 20.42 | 18.61 | 38.06 |  |
| s.replaceAll(" ", "_") | 5.18 | 2.81 | 0.71 | **yes** |
| s.replace("e", "E") | 24.51 | 17.21 | 1.91 |  |
| s.isWellFormed() | 21.91 | 14.79 | 0.97 |  |
| s.isWellFormed(), mapped over 16 texts | 7.29 | 8.53 | 1.00 |  |
| s.isWellFormed() as s === s, the leaf equals | 1.24 | 1.26 | 2.68 | **yes** |
| s.toWellFormed() | 8.57 | 5.58 | 0.89 |  |
| s.toWellFormed(), mapped over 16 texts | 2.92 | 4.61 | 0.87 |  |
| String(x), JavaScript's layout: the composite's test alone (a lower bound), mapped | 4.87 | 5.29 | 0.23 |  |
| a + b, two texts of n/2 | 41.82 | 31.10 | 2.08 |  |
| encodeURIComponent(s) | 369.88 | 200.98 | 1.93 |  |
| decodeURIComponent(s), n characters encoded | 93.21 | 114.98 | 2.11 |  |

### lists: 50 races after, 50 before

| Race | Before | After | Growth after | Within 3 after |
|---|---:|---:|---:|---|
| a list used twice, against once | 1.64 | 1.54 | 1.00 | **yes** |
| at(-2), mapped over 64 lists of n | 73.02 | 42.01 | 0.96 |  |
| at(-1), mapped over 64 lists of n | 43.64 | 19.44 | 0.93 |  |
| keys() | 4.63 | 2.81 | 0.22 | **yes** |
| entries() | 6.21 | 7.93 | 0.35 |  |
| some, match first | 249.78 | 262.21 | 265.36 |  |
| some, no match | 141.73 | 142.70 | 6.17 |  |
| some, against map | 1.20 | 1.28 | 1.01 | **yes** |
| every, first fails | 208.16 | 298.02 | 259.04 |  |
| every, all pass | 162.50 | 162.35 | 7.05 |  |
| every, against map | 1.34 | 1.28 | 0.99 | **yes** |
| find, match first | 694.60 | 837.16 | 297.17 |  |
| find, no match | 405.93 | 460.29 | 6.53 |  |
| find, against map | 3.56 | 3.78 | 1.11 |  |
| findIndex, match first | 423.71 | 525.97 | 265.44 |  |
| findIndex, no match | 423.91 | 513.20 | 8.54 |  |
| findIndex, against map | 4.10 | 4.25 | 1.42 |  |
| findLast, match last | 497.32 | 521.45 | 214.22 |  |
| findLast, no match | 225.44 | 340.24 | 4.99 |  |
| findLast, against map | 2.18 | 2.33 | 1.05 | **yes** |
| findLastIndex, match last | 235.76 | 813.58 | 363.23 |  |
| findLastIndex, no match | 145.24 | 453.79 | 7.69 |  |
| findLastIndex, against map | 2.43 | 3.24 | 1.39 |  |
| filter, all kept | 86.23 | 68.00 | 2.31 |  |
| filter, against map | 9.33 | 5.93 | 2.17 |  |
| flatMap | 129.87 | 87.07 | 0.98 |  |
| flatMap, against map | 4.79 | 2.69 | 0.90 | **yes** |
| map((x, i) => x + i) | 19.68 | 42.87 | 3.52 |  |
| map((x, i) => x + i), against map | 1.63 | 2.44 | 1.19 | **yes** |
| reduce, no start | 156.40 | 240.52 | 4.19 |  |
| reduce, no start, against reduce with one | 1.16 | 1.00 | 0.84 | **yes** |
| reduceRight | 106.74 | 251.92 | 2.23 |  |
| reduceRight, against reduce | 2.28 | 2.44 | 1.12 | **yes** |
| includes, match first | 143.20 | 232.19 | 286.30 |  |
| includes, no match | 49.86 | 63.11 | 1.97 |  |
| indexOf, match first | 501.28 | 471.62 | 279.65 |  |
| indexOf, no match | 187.51 | 219.71 | 2.48 |  |
| lastIndexOf, match last | 244.92 | 577.13 | 342.97 |  |
| lastIndexOf, no match | 99.99 | 169.65 | 2.46 |  |
| toReversed | 47.52 | 76.09 | 1.80 |  |
| slice(1, -1) | 45.44 | 66.13 | 0.86 |  |
| toSpliced(n/2, 2, 3 items) | 156.04 | 234.10 | 1.65 |  |
| with(n/2, 7) | 60.79 | 97.40 | 1.93 |  |
| fill(7, n/4, 3n/4) | 71.95 | 83.60 | 1.02 |  |
| copyWithin(0, n/2, n) | 151.40 | 127.41 | 1.25 |  |
| flat() | 135.56 | 75.84 | 2.01 |  |
| flat(2) | 442.46 | 155.81 | 1.64 |  |
| join() of numbers | 2.24 | 2.16 | 0.58 | **yes** |
| Array.isArray, mapped | 14.59 | 10.99 | 0.96 |  |
| [...new Set(xs)] | 41.63 | 64.12 | 20.66 |  |

### records: 35 races after, 35 before

| Race | Before | After | Growth after | Within 3 after |
|---|---:|---:|---:|---|
| Object.values | 60.95 | 55.29 | 2.44 |  |
| Object.entries | 5.87 | 8.37 | 0.83 |  |
| Object.fromEntries | 24.80 | 10.66 | 0.34 |  |
| { ...a, ...b } | 29.19 | 6.49 | 0.63 |  |
| Object.hasOwn | 18.88 | 12.60 | 0.72 |  |
| Object.hasOwn, mapped | 10.44 | 2.57 | 0.93 | **yes** |
| Object.groupBy | 61.58 | 7.61 | 0.71 |  |
| typeof, a number | 36.98 | 36.62 | 1.00 |  |
| typeof, a number, mapped | 41.91 | 26.51 | 1.00 |  |
| typeof, a record | 153.75 | 145.55 | 25.72 |  |
| new Date(ms) | 33.24 | 26.27 | 1.00 |  |
| new Date(ms), mapped | 33.30 | 21.07 | 1.00 |  |
| getUTCMilliseconds | 28.38 | 16.61 | 1.00 |  |
| getUTCMilliseconds, mapped | 30.29 | 10.95 | 1.00 |  |
| getUTCSeconds | 24.78 | 18.61 | 1.00 |  |
| getUTCSeconds, mapped | 28.75 | 11.67 | 1.00 |  |
| getUTCMinutes | 24.62 | 19.26 | 1.00 |  |
| getUTCMinutes, mapped | 27.89 | 12.00 | 1.00 |  |
| getUTCHours | 24.62 | 19.16 | 1.00 |  |
| getUTCHours, mapped | 29.27 | 11.40 | 1.00 |  |
| getUTCDay | 28.84 | 22.60 | 1.00 |  |
| getUTCDay, mapped | 32.30 | 14.15 | 1.00 |  |
| getUTCFullYear | 117.45 | 94.47 | 1.00 |  |
| getUTCFullYear, mapped | 122.10 | 73.58 | 1.00 |  |
| getUTCMonth | 102.65 | 88.58 | 1.00 |  |
| getUTCMonth, mapped | 113.25 | 72.77 | 1.00 |  |
| getUTCDate | 102.62 | 92.98 | 1.00 |  |
| getUTCDate, mapped | 120.17 | 51.18 | 1.00 |  |
| utc_time | 168.11 | 156.79 | 1.00 |  |
| Date.UTC | 24.75 | 20.97 | 1.00 |  |
| setUTCFullYear | 14.81 | 11.73 | 1.00 |  |
| setUTCHours | 14.69 | 11.61 | 1.00 |  |
| setUTCHours, mapped | 22.40 | 7.16 | 1.00 |  |
| toISOString | 57.66 | 48.92 | 1.00 |  |
| toISOString, mapped | 65.23 | 26.20 | 1.00 |  |
