# JavaScript's library, in Holon's leaves

A flow is written in TypeScript's syntax (docs/L4s/typescript-syntax-L4.md), but it isn't run by a
JavaScript engine: the reader (`crates/holon-author/src/lang/ts.rs`) turns each expression into
leaves of the catalog (`crates/holon-core/src/catalog.rs`), which the evaluator computes
(`crates/holon-core/src/primitives.rs`). So each piece of JavaScript's standard library is one of
these:

| Status | Means |
|---|---|
| **mapped** | the JavaScript form lowers to a leaf that computes the same thing |
| **differs** | it compiles, but computes something else than JavaScript does (each is listed in [§9](#9-what-compiles-but-differs)) |
| **by name** | a leaf does it, but the JavaScript form isn't read: call the leaf by its name, imported from `"holon"` (`import { length } from "holon"`, `length(xs)`) |
| **compose** | no leaf of its own, but mapped forms build it (`s.indexOf(t) !== -1` for `includes`) |
| **import** | the outside world: what JavaScript reaches through a global, a flow imports, and it is a line of what the deploy asks for |
| **js()** | not covered yet: refused, saying "do it in a js() function". Under the rule below this is a gap, not coverage: [§11](#11-closing-the-gaps-by-the-gate) says whether each closes by a composite or a leaf |
| **refused** | refused with Holon's own form named in the mistake; [§12](#12-what-stays-refused) lists what stays so by design |

## 0. The rule

**Every expression over Holon's values lowers to pure leaves.** A library call doesn't fall back
to a `js()` function: `js()` is for a body someone chooses to carry as JavaScript, not the answer
to a missing leaf (a `js()` call costs a fresh realm, about 15M instructions, js-node-L4.md §3,
and the deploy asks for its budget). Each form lowers one of two ways:

- **A composite** of existing leaves (the "blessed standard-library composite" of
  COMPOSABLE_VM_DESIGN.md §13): one node on the canvas, named for what it does (`max`), pinned by
  its hash. Chosen when it costs about what a leaf would: **the same complexity class, counting
  the copies each step makes and where JavaScript stops early, and within a small constant factor
  measured** (`crates/holon-core/benches/native_vs_composite.rs` races a leaf against its
  composite through the real `eval`).
- **A native leaf**, when the composite would cost drastically more or can't be built:
  §13's gate, which already decides every leaf this way. A leaf is native iff it is in a better
  complexity class than any composite, hot and pervasive (paid in inner loops: arithmetic,
  comparison), an effect, or not expressible from the atoms at all. A numeric leaf must also give
  the same bits natively and in wasm: correctly rounded by IEEE (`sqrt`, `floor`, `%`), or one
  pinned pure-Rust implementation compiled into every target (`exp`, `log`, `sin`: `libm` is
  already in the build).

**Either way it means exactly what JavaScript means**, edge cases included: a composite that
gives `-0` where JavaScript gives `0` is a "differs", not coverage (`x < 0 ? 0 - x : x` is not
`Math.abs` at `-0`). Each form ships with a test of its JavaScript meaning, run against the
engine inside `js()` as the oracle.

As of 2026-11-06 (slice 4 of the L4). The mapping is in `ts.rs`: `expr` (operators and literals,
around line 2587), `method_call` (namespaces and methods, around line 2968), `static_member`
(`.length`, around line 2818); the lowering of each operator to its leaf is `lower.rs`'s `binary`
and `expr`. Each mistake quoted below is the reader's own text.

**In numbers:** of JavaScript's library functions, these are read: `JSON.parse` (with a
reviver), `JSON.stringify` (with a replacer, keys and an indent), `Object`'s record functions
(§8), `Date` in UTC (§8), `structuredClone`, a text's methods but `localeCompare` and those that
take a regular expression, `String`, `String.fromCharCode`, `String.fromCodePoint`, `String.raw`,
the URI functions, `btoa` and `atob`, as the text leaves (§3, §6), a list's methods but `forEach` and those that change the list and answer
something else (`push` as a value, `pop`, `shift`, `unshift`, `splice`), as the list leaves (§7), `Array.isArray`,
`Array.of` and `Array.from`, every one of `Math`'s functions but `random` and its 8 constants
(§4), `Number` and its functions and constants (§5), `parseInt`, `parseFloat`, `isNaN` and
`isFinite` (§3), and a number's `toFixed`, `toPrecision`, `toExponential` and `toString` (§5), as
the eleven number leaves. The operators are mapped but for `void`, `instanceof` and the comma
(§1), and the literals mostly (§2). Of the catalog's computing leaves, those §10 lists can only be called by name.

## 1. Operators

| JavaScript | Leaf | Status | Note |
|---|---|---|---|
| `a + b` (numbers) | `add` | mapped | a chain `a + b + c` is one `add` with three inputs |
| `a + b` (a text on a side) | `add` | mapped | both written as `String(x)` writes them and joined, left to right: `1 + 2 + "a"` is `"3a"`; a list or a record: nothing ([§9](#9-what-compiles-but-differs)) |
| `a - b` | `subtract` | mapped | |
| `a * b` | `multiply` | mapped | a chain is one node |
| `a / b` | `divide` | differs | by zero: nothing, where JavaScript gives `Infinity` or `NaN` |
| `a % b` | `remainder` | mapped | fmod, with `a`'s sign; by zero: nothing |
| `a ** b` | `power` | mapped | right-associative; `Math.pow(a, b)` too; nothing where JavaScript gives `NaN` or an infinity |
| `-x` | `const`; `multiply` | mapped | a number literal is itself (`-1`); any other value is `x * -1`, one node, exact at `-0` where `0 - x` isn't |
| `+x` | `to_number` | mapped | `Number(x)` |
| `~x` | `bitwise {op: xor}` | mapped | `x ^ -1` |
| `typeof x` | `type_of`, `default` | mapped | `type_of(x) ?? "undefined"`; a record, a list and `null` are `"object"` |
| `void` | | js() | "void isn't an operator Holon has" |
| `x++`, `x--`, `x += 1`, `x %= 2`, `x **= 2`, `x &= m`, `x <<= 1`, … | `add`, `subtract`, and each operator's leaf | mapped as statements | "++ and -- are a statement of their own: n++" in an expression |
| `a === b`, `a == b` | `equals` | mapped; differs on records and lists | `==` is `===` (no coercion); records and lists compare by value |
| `a !== b`, `a != b` | `equals`, `not` | mapped | |
| `a < b`, `<=`, `>`, `>=` | `compare {op}` | mapped | two numbers, or two texts in UTF-16 code-unit order; a number and a text: nothing (JavaScript converts) |
| `a && b`, `a \|\| b` | `and`, `or` | mapped for booleans | a literal on the right is refused: "takes booleans: a value for where one is absent is ??"; other values: differs |
| `!x` | `not` | mapped for booleans | other values: differs, nothing (JavaScript's truthiness is `is_truthy`, by name) |
| `a ?? b` | `default` (b a literal), `coalesce` | mapped | `b` is computed only when `a` is absent |
| `c ? a : b` | a `guard` on each side of `c`, then `coalesce` | mapped | only the branch taken runs, and it is the answer; `c ? a : undefined` is the first `guard` alone |
| `x?.k`, `x?.[k]` | `field`, `lookup` | mapped | absence passes along anyway, so `?.` and `.` lower alike |
| `k in r` | `has` | mapped | a record has no prototype: its own keys |
| `instanceof` | | refused | "a flow's values have no classes: Array.isArray(x) tells a list, typeof x the rest" |
| `&`, `\|`, `^`, `<<`, `>>`, `>>>` | `bitwise {op}` | mapped | on ToInt32 (ToUint32 for `>>>`), with TypeScript's precedence |
| `a, b` (comma), assignment in an expression | | refused | "a write is a statement of its own" |
| `delete store[k]` (a statement) | `collection_delete`, `file_delete` | mapped | for a collection or folder; in an expression, "delete is a statement" |

## 2. Literals and access

| JavaScript | Leaf | Status | Note |
|---|---|---|---|
| `1`, `1_000`, `"t"`, `true` | `const` | mapped | |
| `` `a ${x} b` `` | `concat`, `as_text` per part | mapped | each value as it reads in a message |
| `[a, b]` | `list` | mapped | |
| `[...xs, y]`, `[...xs, ...ys]` | `append`, `extend` | mapped | `[x, ...xs]` is `extend` of a one-item list (`cons`, by name, isn't used) |
| `{ k: v }` | `record`, `with_field` per field | mapped | |
| `{ ...base, k: v }` | `with_field` over `base` | mapped | |
| `{ a: 1, ...r, ...s, k: v }` | `merge`, `with_field` | mapped | read left to right: a spread after anything is `merge`, one for several in a row |
| `{ [k]: v }` | `insert` | mapped | the key a text; read in place, left to right |
| `r.k` | `field` | mapped | |
| `r[k]` | `lookup` | mapped for a record | a list: differs, `xs[0]` answers nothing; `get(xs, 0)` by name |
| `store[k]` (a collection, folder, database) | `collection_get`, `file_read`, … | mapped | §8 |
| `xs.length`, `s.length` | `length`, `text_length` | mapped where the type is known | by the type of the node that made the value, a `List` or a `Text`; on anything else, ".length is a text's text_length or a list's length, and this value isn't known to be either" |
| `undefined` | | as a branch | `c ? a : undefined`, `x !== undefined` in an `if`; else "undefined is written as a branch" |
| `null` | | refused | "null isn't a value here: a value that isn't there is absent, undefined" |
| `NaN`, `Infinity` | | refused | "NaN isn't a value a flow has: a leaf gives nothing where JavaScript gives NaN, so test for presence, x !== undefined, or give a fallback, x ?? 0" |
| `/re/` | | js() | regular expressions wait; as a method's argument, "a regular expression, which a flow has none of yet" |
| `new Date(t)` | `date_part {part: time}` | mapped, by its methods | §8; alone: refused, naming `.getTime()` and `.toISOString()` |
| `new Map()`, `new Set()` | | refused | a record by text key; `[...new Set(xs)]` is read (§7) |
| `new X(…)` | | js() | |
| `class`, getters, setters, methods in a record | | refused | "a record holds values: no getters, setters or methods" |
| `(x) => …` | the function of `map`, `fold`, `filter`, `find`, `sort_by` and the other higher-order leaves | mapped there only | elsewhere: "a function written in place goes to a list's method: xs.map((x) => …)"; a parameter it doesn't use is fine (`_`, or any name) |

## 3. Global functions

| JavaScript | Status | In Holon |
|---|---|---|
| `String(x)` | mapped | `as_text(x)`, as `` `${x}` `` writes it; `String()` is `""` |
| `Number(s)`, `parseFloat(s)`, `parseInt(s)`, `parseInt(s, r)` | mapped | `to_number {grammar: number, float, int}`, the radix wired when given; a flow's own function of the name is the one called |
| `Boolean(x)`, `!!x` | by name | `is_truthy(x)` |
| `isNaN(x)`, `isFinite(x)` | mapped | `number_test {test, coerce: number}`: `isNaN("abc")` is true |
| `encodeURIComponent`, `decodeURIComponent`, `encodeURI`, `decodeURI` | mapped | `uri_encode {set: component, uri}`, `uri_decode {set}`; a malformed escape: nothing (JavaScript's `URIError`). A route's `req.params` and `req.query` come decoded |
| `btoa(s)`, `atob(s)` | mapped | `base64 {bytes: latin1}`, with `mode: decode` for `atob` (WHATWG's forgiving decode); a character past U+00FF: nothing |
| `escape`, `unescape` | refused | Annex B's legacy form, naming `encodeURIComponent` and `decodeURIComponent` |
| `structuredClone(x)` | mapped | `x`: values are never changed in place |
| `setTimeout`, `setInterval` | refused, import | a flow has no globals: a clock is a handler, `every("5s", () => …)`; a wait is `await event("paid", { expires: "1d" })` |
| `fetch` | refused, import | `import api from "host:api.example.com"`, `await api.get("/p")` (§8) |
| `console.log` | refused | not needed: each node's value is in the run's record (`holon explain`) |
| `globalThis`, `window`, `process`, `require`, `Promise` | refused | "a flow has no globals; import what it uses" |

## 4. `Math`

`Math` is a namespace the reader knows, as it knows `JSON` (`ts.rs` `math_call`); `Math.max` is
the leaf whatever the flow defines.

| Member | Leaf | Status | Note |
|---|---|---|---|
| `abs`, `sign`, `floor`, `ceil`, `trunc`, `round`, `sqrt`, `cbrt`, `exp`, `expm1`, `log`, `log2`, `log10`, `log1p`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `fround`, `f16round`, `clz32` | `math {fn}` | mapped | IEEE-exact, or `libm` 0.2.16's bits (§9) |
| `max(a, b, …)`, `min(a, b, …)` | `max`, `min` | mapped | variadic; one argument is refused |
| `max(...xs)`, `min(...xs)`, `hypot(a, b, …)`, `hypot(...xs)`, `sumPrecise(xs)` | `aggregate {fn}` | mapped | `hypot`'s numbers made a list in place; V8's `hypot` |
| `atan2(y, x)`, `pow(a, b)`, `imul(a, b)` | `atan2`, `power`, `bitwise {op: imul}` | mapped | |
| `random()` | | refused, import | `import { random } from "holon:random"`: one number for the run, recorded so it replays; `Math.random` gives a new one each call |
| `PI`, `E`, `LN2`, `LN10`, `LOG2E`, `LOG10E`, `SQRT2`, `SQRT1_2` | `const` | mapped | the double's literal, checked against Rust's constants |

## 5. `Number`

| Member | Status | In Holon |
|---|---|---|
| `Number.isInteger`, `isFinite`, `isSafeInteger` | mapped | `number_test {test, coerce: none}` |
| `Number.isNaN(x)` | mapped | `false`: a flow has no NaN (§9) |
| `Number.parseFloat`, `Number.parseInt` | mapped | as `parseFloat` and `parseInt` (§3) |
| `MAX_SAFE_INTEGER`, `MIN_SAFE_INTEGER`, `EPSILON`, `MAX_VALUE`, `MIN_VALUE` | mapped | the double's literal |
| `NaN`, `POSITIVE_INFINITY`, `NEGATIVE_INFINITY` | refused | "a flow has no NaN or Infinity" |
| `x.toFixed(d)`, `x.toPrecision(p)`, `x.toExponential(d)`, `x.toString(radix)` | mapped | `number_format {style}`; exact digits, half up, as V8 writes them |
| `x.toString()` | mapped | `number_format {style: string}` on a node whose output is a `Number`; otherwise `as_text`, as `String(x)` |
| `x.toLocaleString()` | refused | "a flow has no locale" |

## 6. `String.prototype`

| Method | Leaf | Status | Note |
|---|---|---|---|
Positions and lengths count UTF-16 code units, as JavaScript's do; a result that would begin or
end inside a surrogate pair gives nothing (js-library-races/texts.md, "Units").

| Method | Leaf | Status | Note |
|---|---|---|---|
| `toUpperCase()`, `toLowerCase()`, `toLocaleUpperCase()`, `toLocaleLowerCase()` | `uppercase`, `lowercase` | mapped | no locale: one given is refused |
| `split(sep)`, `split(sep, n)` | `split` | mapped | `split("")` cuts every code unit; the limit as ToUint32 |
| `indexOf(t, i)`, `lastIndexOf(t, i)`, `includes(t, i)` | `index_of`, `last_index_of`, `includes` | mapped | one leaf each for a list and a text; the position, when given, the `from` input |
| `at(i)` | `at` | mapped | one leaf for a list and a text; from the end when negative |
| `length` | `text_length` | mapped where the text is known to be one | §2 |
| `concat(a, …)` | `concat`, `as_text` | mapped on a text written in place or known to be one | each argument as `String(x)` writes it; on anything else it is a list's (§7) |
| `substr(start, len)` | `slice` | by name | the leaf's default mode takes a length, as `substr` does: `slice(s, start, len)`; Annex B's, not read |
| `slice(start, end)`, `substring(start, end)` | `slice {mode: end}`, `slice {mode: substring}` | mapped | one `slice` for a list and a text |
| `charAt(i)`, `charCodeAt(i)`, `codePointAt(i)` | `text_at`, `code_at {mode: unit, point}` | mapped | the index left out is 0 |
| `startsWith(t, i)`, `endsWith(t, i)` | `text_contains {at: start, end}` | mapped | |
| `trim()`, `trimStart()`, `trimEnd()` | `trim {side}` | mapped | JavaScript's white space and line terminators |
| `padStart(n, f)`, `padEnd(n, f)`, `repeat(n)` | `pad {side}`, `repeat` | mapped | the fill left out is a space |
| `replace(a, b)`, `replaceAll(a, b)` (texts) | `replace {count: first, all}` | mapped | `$$`, `$&`, `` $` `` and `$'` as JavaScript's; a function as the replacement: js() |
| `normalize(form)` | `normalize {form}` | mapped | the form written in place; `unicode-normalization` 0.1.26's tables |
| `isWellFormed()`, `toWellFormed()` | `is_well_formed`; `concat` | mapped | a flow's text is always well formed: `true`, and `concat(s, "")` |
| `toString()`, `valueOf()` | `as_text`; none | mapped | the text itself |
| `match`, `matchAll`, `search`; `replace`, `replaceAll`, `split` with a regular expression | | js() | regular expressions wait: "… which a flow has none of yet" |
| `localeCompare` | | refused | "a flow has no locale": `a < b ? -1 : a > b ? 1 : 0` |
| `String.fromCharCode(…)`, `String.fromCodePoint(…)` | `from_codes {mode: unit, point}` | mapped | variadic; none is `""` |
| `` String.raw`…` `` | `concat`, `as_text` | mapped | the template of its raw text |

## 7. Arrays

Each method that takes a function takes a name or a function written in place, as `map` does;
the leaf's function gets the item (and its captures), and the reader turns an index parameter into
`indices`. A list never changes: the methods that change one in place are read as the list they
give, or refused naming the form that gives it. Items compare by value (§9).

| Method | Leaf | Status | Note |
|---|---|---|---|
| `map((x) => …)`, `map(f)` | `map` | mapped | |
| `map((x, i) => …)` | `indices`, `map`, `get` | mapped | `indices(xs).map((i) => …)`, `x` read as `get(xs, i)`; `xs` a name or a field of one, as it is read twice |
| `reduce((acc, x) => …, init)` | `fold` | mapped | |
| `reduce(f)` | `head`, `tail`, `fold` | mapped | `tail(xs).reduce(f, head(xs))`, `xs` a name or a field of one; an empty list: nothing, where JavaScript throws |
| `reduceRight(f, init)` | `reverse`, `fold` | mapped | `xs.toReversed().reduce(f, init)`; without a start: refused |
| `filter(f)` | `filter {fn}` | mapped | `f`'s answer read as JavaScript reads a condition |
| `some(f)`, `every(f)` | `any {fn}`, `all {fn}` | mapped | apply `f` to nothing after the item that decides |
| `find(f)`, `findIndex(f)`, `findLast(f)`, `findLastIndex(f)` | `find {fn}`, `find_index {fn}`, `find_last {fn}`, `find_last_index {fn}` | mapped | as `some`; `find` gives nothing when nothing matches |
| `flatMap(f)`, `flat()`, `flat(d)`, `flat(Infinity)` | `map`, `flat {depth}` | mapped | `flat(xs.map(f))`; the depth a number written in place, `all` for `Infinity` |
| `toSorted()`, `toSorted(cmp)` | `sort`, `sort_by {fn}` | mapped | stable; `sort` orders by `String(x)` in UTF-16 code units, as JavaScript's does |
| `sort()`, `sort(cmp)`, `reverse()`, `fill(v, s, e)`, `copyWithin(t, s, e)` | `sort`, `sort_by`, `reverse`, `fill`, `copy_within` | mapped as a value | on a list made in place (`[...xs].sort()`) or a handler's own list not read after; as a statement, on a parameter, a part of one or a list another name holds: refused, naming the value form (`xs.toSorted()`) |
| `toReversed()`, `toSpliced(s, n, ...items)`, `with(i, v)` | `reverse`, `to_spliced`, `with_item` | mapped | `with` out of range: nothing, where JavaScript throws |
| `at(i)` | `at` | mapped | from the end when negative |
| `includes(x, i)`, `indexOf(x, i)`, `lastIndexOf(x, i)` | `includes`, `index_of`, `last_index_of` | mapped | by value; the position, when given, the `from` input, as JavaScript's |
| `slice(s, e)`, `slice(s)`, `slice()` | `slice {mode: end}` | mapped | `slice()` is the list itself |
| `join(sep)`, `join()` | `join` | mapped | texts; `join()` is `join(xs, ",")` |
| `concat(ys, x)` | `extend`, `append` | mapped | on a value not known to be a text: `extend` for a list written as one, `append` for a literal that isn't one; a name, which could be either: refused, naming `[...xs, ...ys]` and `[...xs, y]` |
| `length` | `length` | mapped where the list is known to be one | §2 |
| `xs[i]` | `get` | by name | `xs[i]` lowers to `lookup` and answers nothing (§9) |
| `[x, ...xs]` (`unshift`) | `extend` (`cons`) | mapped | |
| `[...xs, x]`, `xs.push(x)` as a statement | `append` | mapped | `xs.push(x)` alone is `xs = [...xs, x]` |
| `push` as a value, `pop`, `shift`, `unshift`, `splice` | | refused | naming the value form: `[...xs, x]`; `xs.at(-1)` and `xs.slice(0, -1)`; `xs.at(0)` and `xs.slice(1)`; `[x, ...xs]`; `xs.toSpliced(…)` and `xs.slice(s, s + n)` |
| `xs.length === 0` | `is_empty` | by name | |
| sum: `reduce((a, b) => a + b, 0)` | `fold` and `add`; `sum` | mapped; by name | |
| `forEach` | | refused | a statement per item has no form yet; `map` computes |
| `[...xs.keys()]`, `[...xs.entries()]`, `[...xs.values()]` | `indices`, `entries`, the list | mapped | spread, or `Array.from(…)`; the iterator itself: refused, naming the spread |
| `[...new Set(xs)]`, `Array.from(new Set(xs))` | `unique` | mapped | by value: two equal records are one |
| `Array.isArray(x)`, `Array.of(a, b)` | `is_list`, `list` | mapped | |
| `Array.from(xs)`, `Array.from(xs, f)` | the list, `map` | mapped | |
| `Array.from({ length: n }, (_, i) => …)`, `Array.from({ length: n })` | `range`, `map` | mapped; refused | `range(n).map((i) => …)`, n up to 10000; without a function the items are `undefined`, which a flow has none of |

## 8. `Object`, `JSON`, and what a flow imports

| JavaScript | Leaf | Status | Note |
|---|---|---|---|
| `Object.keys(r)`, `Object.getOwnPropertyNames(r)` | `keys` | mapped | |
| `Object.values(r)`, `Object.entries(r)`, `Object.fromEntries(ps)` | `values`, `entries`, `from_entries` | mapped | key order (§11, open 2) |
| `Object.groupBy(xs, f)` | `group_by {fn}` | mapped | an item whose key isn't a text is left out |
| `Object.assign({}, a, b)` | `merge` | mapped | into a record written in place; alone on a local, `t = merge(t, …)`; into anything else, refused: "Object.assign changes its first record in place …" |
| `Object.hasOwn(r, k)`, `r.hasOwnProperty(k)` | `has` | mapped | |
| `Object.is(a, b)` | `same_value` | mapped | |
| `Object.freeze(x)`, `seal`, `preventExtensions`, `Object.create(null)` | | mapped | the value itself, a local marked frozen (a later change to it refused, where JavaScript throws); `{}` |
| `Object.defineProperty`, `getPrototypeOf`, `isFrozen`, `create(proto)`, … | | refused | a record has no descriptors or prototype; every value is frozen already |
| `delete r.k` (a record value) | `without_field`, `remove` | by name | |
| `JSON.parse(t)`, `JSON.parse(t, (k, v) => …)` | `json_parse {fn}` | mapped | invalid JSON: nothing, where JavaScript throws; the reviver bottom-up |
| `JSON.stringify(v)`, `JSON.stringify(v, replacer, space)` | `json_stringify {fn, keys, indent}` | mapped | the replacer a function, a list of texts written in place or `null`; the space a number or a text written in place |
| `new Date(t).getTime()`, `.getUTCFullYear()` and the other `getUTC…` | `date_part {part}` | mapped | `t` a time value or a text; on a Date bound to a name, its time value |
| `new Date(t).toISOString()`, `.toUTCString()` | `date_text {format}` | mapped | |
| `new Date(t).setUTCHours(…)` and the other `setUTC…` | `date_set {part}` | mapped | as a value; on a Date bound to a name, alone or in `const t = d.setUTC…(…)`, a new value for it, and elsewhere in an expression the Date read no more |
| `Date.UTC(…)`, `Date.parse(t)` | `date_utc`, `date_parse` | mapped | |
| `getHours()` and the other local-time methods, `new Date(y, m, …)` | | refused | "a flow has no local time, only UTC", naming the UTC method or `Date.UTC` |
| `Date.now()`, `new Date()` | `clock` | refused, import | `import { now } from "holon:clock"`: seconds, fixed for the run, so `now * 1000` |
| `Intl` | | js() | not inside `js()` either: locale is the environment |
| `Map`, `Set`, `WeakMap`, `Symbol`, `BigInt`, `Proxy`, `Reflect`, typed arrays | | js() | inside `js()` only (a `BigInt` can't cross out) |
| `Promise.all` | | refused | in slice 7, a started run's handle may be awaited or passed to it |
| `throw new Error(m)`, `try`/`catch {}`/`finally` | `fail`, a choice on absence | mapped | slice 5: `catch` catches absence too; `catch (e)` refused until a failure carries its reason (slice 10) |

What a service reaches outside the language, a flow imports; each is a line of what the deploy
asks for:

| A service writes | A flow writes | Leaves |
|---|---|---|
| `fetch(url, …)` | `import api from "host:api.example.com"`, `await api.post("/p", { body })` | `http_request`; `http_stream` with `stream: true`; `http_upload` for a file |
| `process.env.KEY` | `import key from "secret:KEY"` | `secret` |
| a module variable, `localStorage` | `import state from "state"`, `state.hits` | `store_read`, `store_write` |
| an in-memory `Map` kept across requests | `import notes from "collection:notes"`, `notes[k]`, `notes.list(…)` | `collection_*` |
| `fs.readFile`, `fs.writeFile`, `fs.unlink`, `fs.readdir` | `import inbox from "folder:inbox"`, `inbox[name]`, `inbox[name] = t`, `delete inbox[name]`, `inbox.list(…)` | `file_*` |
| `fs.watch` | `on.file.created(inbox, (f) => …)` | `file_arrival` (`on_file`) |
| a database client | `import db from "database:vendor"`, `db.query(…)`, `db.find(…)` | `db_*` |
| an `EventEmitter`, server-sent events, a socket | `import updates from "channel:updates"`, `updates.send(…)`, `updates.subscribe()` | `channel_send`, `channel_subscribe` |
| `app.get(path, handler)` (Express) | `route.get(path, (req) => …)` | `request`, `respond` |
| `setInterval` | `every("5s", () => …)` | `every` |
| `crypto.createHash("sha256")`, `crypto.subtle.digest` | `sha256(t)`, `{ encoding: "base64" }` for base64 | `sha256` (by name) |
| `crypto.createHmac("sha256", key)` | `hmac_sha256(key, message)` | `hmac_sha256` (by name) |
| `crypto.timingSafeEqual` | `secure_equals(a, b)` | `secure_equals` (by name) |
| `btoa`, `atob` | `btoa(t)`, `atob(t)` (§3) | `base64 {bytes: latin1}` |
| `Buffer…toString("base64")` | `base64(t)`, `base64(t, { mode: "decode" })`, `{ alphabet: "url" }` | `base64` (by name) |
| `Buffer…toString("hex")` | `hex(t)`, `{ mode: "decode" }` | `hex` (by name) |
| `crypto.randomUUID()` | none | `random` is one number per run |
| `new URL`, `URLSearchParams`, `TextEncoder` | js() | |

## 9. What compiles but differs

These are read and lowered, but the leaf computes something else than JavaScript does. The
author sees no mistake; the run answers nothing where JavaScript answers something:

| JavaScript | What happens | Why |
|---|---|---|
| `xs[0]` on a list | `lookup`: nothing | `lookup` reads a record by a text key (`primitives.rs`, `Op::Lookup`); a list's item is `get` |
| `!x`, `x && y`, `x \|\| y` on a value that isn't true or false | `not`, `and`, `or`: nothing | they take booleans (`as_bool`); JavaScript's truthiness is `is_truthy` |
| `a + b` with a list or a record and no text, or `true + 1` | `add`: nothing | `add` joins texts and adds numbers; JavaScript converts the others first |
| `"a" + [1, 2]`, `` `${{ a: 1 }}` `` | `"a[1,2]"`, `{"a":1}` | `as_text` writes a list or a record as JSON; JavaScript writes `"a1,2"` and `[object Object]` |
| `[[1]].includes([1])`, `new Set([[1], [1]])` | `true`, one item | compared by value; JavaScript compares objects by identity |
| `a / 0`, and `+`, `-`, `*` or `/` past the largest number | nothing | JavaScript gives `Infinity` or `NaN`, which Holon has no value for |
| `Math.log(0)`, `0 ** -1`, `(-8) ** (1/3)`, `a % 0`, `parseInt("x")`, `Number("abc")`, `(1e21).toFixed(101)` | nothing | the same: JavaScript gives `NaN`, an infinity, or throws a RangeError |
| `Number.isNaN(x)` | `false` | the reader writes `false`: no leaf gives `NaN`, so a flow tests for presence, `x !== undefined` |
| `Math.max("5", 3)`, `"5" & 3`, `"7" % 2`, `Math.sqrt("4")` | nothing | a number leaf takes numbers; JavaScript converts the text first. `Number(s)` converts |
| `Math.sin`, `Math.exp`, `a ** b` and the other transcendental functions | `libm` 0.2.16's bits | JavaScript leaves them approximated by the engine; one may differ from Holon in the last bit |
| `Math.hypot(a, b)` | V8's bits | other engines may differ in the last bit |
| `{ a: 1 } === { a: 1 }` | `true` | compared by value; JavaScript compares objects by identity |
| `1 == "1"` | `false` | `==` is `===` |
| `JSON.parse("bad")` | nothing | JavaScript throws; Holon's `catch {}` catches the nothing |
| `"a😀b".slice(1, 2)`, `"😀".charAt(0)` | nothing | half a surrogate pair, which a flow's text can't hold; JavaScript gives a text holding a lone surrogate |
| `Object.keys(r)`, `JSON.stringify(r)` | keys sorted | JavaScript keeps insertion order, integer keys first; §11, open 2 |
| `JSON.parse` of a key given twice | nothing | `json.rs` refuses it; JavaScript keeps the last |

Each is a candidate for the reader to refuse with the right form, or to lower by the value's type
(`xs[i]` to `get`, `!x` to `not(is_truthy(x))`), as `.length` is, once the reader knows the
types it reads.

## 10. Leaves with no JavaScript form

The computing leaves an author can only call by name. Most have a JavaScript form waiting to be
read; reading it is a line in `ts.rs` each, with a test of its meaning:

| Leaf | Its JavaScript form | Note |
|---|---|---|
| `length`, `text_length` | `.length` | read where the value's type is known (§2); by name elsewhere |
| `get` | `xs[i]` | §9: `xs[i]` lowers to `lookup`; `xs.at(i)` is `at` |
| `slice` (`mode: length`) | `s.substr(start, len)` | Annex B's, not read; `s.slice(a, b)` is `slice {mode: end}`, `s.substring(a, b)` `slice {mode: substring}` |
| `head`, `tail` | `xs[0]` | `xs.at(0)` is `at` and `xs.slice(1)` `slice`; `xs.reduce(f)` lowers to both |
| `is_empty` | `xs.length === 0` | |
| `sum` | `xs.reduce((a, b) => a + b, 0)`, which lowers to `fold` and `add` today | |
| `is_truthy` | `Boolean(x)`, `!!x`, and the truthiness of `!`, `&&`, `\|\|` | §9 |
| `insert` | `{ ...r, [k]: v }` | a computed key |
| `remove`, `without_field` | `delete r[k]`, `const { k, ...rest } = r` | on a record value |
| `cons` | `[x, ...xs]` | lowers to `extend` today, the same list |
| `select` | `c ? a : b` | the ternary lowers to a `guard` on each side instead, so only the branch taken runs |
| `sha256`, `hmac_sha256`, `secure_equals`, `hex`, `base64` | none in the language standard (Node's `crypto` and `Buffer`) | by name is their form; `btoa` and `atob` are `base64 {bytes: latin1}` (§3) |
| `to_text` | none (`JSON.stringify` of a text is near it) | |
| `pair`, `some`, `none` | none | the lowering's own: two values together, an optional value |

## 11. Closing the gaps, measured

Each gap was decided by measurement, by the rule in
[js-library-races/README.md](js-library-races/README.md): where a composite of today's leaves can
give JavaScript's result, it was written in TypeScript, compiled by the reader, checked against a
native candidate with JavaScript's meaning (numbers by their bits, so `-0` counts), and raced
through `eval`; where none can, the row is a leaf by expressibility. The evaluator was then made
cheaper without changing a value or a hash ([leaves.md](js-library-races/leaves.md)), and every race
run again ([rerace.md](js-library-races/rerace.md)). The pages hold every row, each composite's
text, each leaf's spec and the races' output:

| Domain | Rows | Reader | Composite | Leaf | Refused, or differs and stays |
|---|---:|---:|---:|---:|---:|
| [Numbers](js-library-races/numbers.md) | 83 | 18 | 0 | 61, in 11 families | 4 |
| [Texts](js-library-races/texts.md) | 57 | 13 | 0 | 41 (10 of them changes to a leaf) | 3 |
| [Lists](js-library-races/lists.md) | 50 | 13 | 4 | 27 | 6 |
| [Records, JSON, dates](js-library-races/records.md) | 64 | 12 | 3 | 33 (5 of them changes) | 16 |

**Why nearly every gap is a leaf.** A composite pays for each node it places; the leaf it would
replace pays only its body. Before the evaluator change a node in a function cost about 90 ns and
a function called from another 600 to 770 ns more, against 5 to 40 ns for a leaf's whole body, so
every scalar composite lost by 15 to 70 times. The change took a node to about 30 ns and a nested
call to 40 to 90 ns, and made every fold that grows its accumulator linear; the races halved, but a
composite of k nodes over a scalar still costs about k times its leaf. A composite stays within 3
times only where it wraps a leaf that does O(n) work, which few gaps are: `map((x, i) => …)` over
`indices`, `reduce` without a start over `head` and `tail`, `reduceRight` over `toReversed`,
`flatMap` over `flat`, and `has` (`k in r`) over `lookup`.

The evaluator change itself: a `map`'s or `fold`'s function keeps its nested composites prepared
after the first item, a value moves to the last node that reads it (a fold gives its accumulator,
and `append`, `extend`, `insert`, `with_field` grow it in place), a choice lends what it passes on
instead of copying it, and `join`, `concat` and `slice` copy once. Abs mapped went from 1.5 µs to
347 ns an item; a filter written as a fold from 170 to 2.3 ms at 4096 items.

**The leaves.** Grouped by a frozen parameter where they share a shape, as `compare {op}` is: the
parameter is part of the unit's hash, so adding a value later re-hashes nothing, and the family
measured 1.07 times a single leaf. Across the four pages, about 60 new leaves after the names the
pages share are made one (a leaf taking a list or a text, so the reader needn't know which):

- **Numbers:** `math {fn}` (29 one-number functions: rounding, roots, logs, trigonometry, `fround`,
  `clz32`, IEEE-exact or `libm` 0.2.16), `max` and `min` (variadic, `-0` below `0`),
  `aggregate {fn}` over a list (max, min, hypot, sumPrecise), `atan2`, `power`, `remainder`,
  `bitwise {op}`, `number_test {test}`, `to_number {grammar}` (`Number`, `parseFloat`,
  `parseInt`), `number_format {style}` (`toString(radix)`, `toFixed`, `toPrecision`,
  `toExponential`, exact digits).
- **Texts** (built), counting UTF-16 code units: `text_at`, `code_at {mode}`, `from_codes {mode}`,
  `text_contains {at: start|end}`, `trim {side}`, `pad {side}`, `repeat`, `replace {count}`,
  `normalize {form}`, `uri_encode {set}`, `uri_decode {set}`, `is_well_formed`. Regular
  expressions on `regress` (Boa's engine) wait: `regex_test`, `regex_exec`, `regex_matches`,
  `regex_search`, `regex_replace`, `regex_split`.
- **Lists** (built, and read): `indices`, `entries`, `any`, `all`, `filter`, `find`, `find_index`,
  `find_last`, `find_last_index` (each taking its function by a `fn` param, as `map` does),
  `reverse`, `to_spliced`, `with_item`, `fill`, `copy_within`, `sort` and `sort_by` (a pinned
  stable merge sort), `flat {depth}`, `unique`, `is_list`.
- **Shared by lists and texts** (built): `at`, `index_of`, `includes` and `last_index_of` (the
  last three with a `from`), and `slice {mode: end}`, each taking either.
- **Records and values:** `values`, `entries` (built with the lists: a record's pairs in key
  order), `from_entries`, `merge`, `group_by {fn}`, `type_of`, `same_value` (`Object.is`).
- **Dates**, a date being its time value in milliseconds: `date_part {part}`, `date_utc`,
  `date_set {part}`, `date_text {format}`, `date_parse`.
- **Changes to leaves:** `text_length`, `slice {mode}`, `index_of` (a start), `split` (`""` and a
  limit), `compare` on texts, `add` joining texts, `as_text` and `json_stringify` writing numbers as
  JavaScript does, `json_stringify {indent, keys, fn}`, `json_parse {fn}`, `base64 {bytes}`.

**What only the reader needs to read**, over leaves that exist: the `Math` and `Number` constants
(each bit-checked) and `-x` as `x * -1` (exact at `-0`), both read now, as are `++`, `--` and the
compound assignments, and `concat`, `Array.from(xs)` and `Array.of`; `.length` and `xs[i]` by
the value's type, `{ [k]: v }` (read now), `Object.freeze` and `structuredClone` as the value
itself (read now), a date's `getTime` (read now).

**What the races found wrong**, all fixed since: `add` gave `0` for `-0 + -0` (it summed
from `+0`; it starts from `-0` now); `add`, `subtract`, `multiply` and `divide` could overflow to an
infinity Holon has no value for (they give nothing now); `as_text` and `json_stringify` wrote `1e21`
as `1000000000000000000000` (they write JavaScript's `1e+21` and `1e-7` now, `primitives.rs`
`number_text`); and a function returning a record literal gave only its first field when read whole
(it is the record now: answered, passed on, mapped or folded, `lower.rs` `whole`); and `c ? a : b`
gave `b` when `c` was true and `a` absent, which [LANGUAGE.md](../../LANGUAGE.md) §6.4 had chosen
(a fallback, not a branch): a guard on each side now, it is absent there, as in JavaScript.

**Decided, and what is open:**

1. **Text units.** Decided and built: the text leaves count UTF-16 code units, as JavaScript's do
   (measured 1.0 to 1.7 times counting scalar values, the same class), so `length`, `slice`,
   `indexOf` and `charCodeAt` are JavaScript's; a result that would split a surrogate pair gives
   nothing ([texts.md](js-library-races/texts.md#units)).
2. **Key order.** Decided: a record's keys stay sorted, where JavaScript keeps insertion order,
   integers first; insertion order would make two equal records hash or print differently.
   `JSON.stringify(v, keys)` writes keys in a chosen order
   ([records.md](js-library-races/records.md#key-order)).
3. **Inline library composites.** Placed in its caller's graph instead of called, `x * -1` costs
   1.2 times a leaf, `abs` 2.05, `max` 2.07, `Number.isFinite` 1.48. A reader that inlines a
   library composite would turn several leaves into reader rows, at the price of the one named node
   the canvas shows ([numbers.md](js-library-races/numbers.md)).
4. **A family's parameter checked when wired.** Decided and built: core's `validate` refuses a
   family's parameter outside its values, `compare {op}`'s included (`ValidationError::FamilyParam`,
   `catalog::FAMILIES`), and the author refuses it where it is written.
5. **Regular expressions.** Decided: they wait. The reader refuses a pattern, naming `js()`.
   When they come, their budget is open: `regress` backtracks and has no step limit (literal
   patterns first, a step budget, or a cap on the text's length).
6. **Smaller ones.** Refuse `escape`/`unescape` (Annex B); what `+` and `String(x)` give for a list
   or a record (JSON today, `"1,2"` and `"[object Object]"` in JavaScript); `JSON.parse` of a key
   given twice (refused by design, JavaScript keeps the last); whether `==` is refused rather than
   read as `===`.
7. **The evaluator's next steps** ([leaves.md](js-library-races/leaves.md) §8): a composite's
   preparation is kept in the server's resolver now (one call of a 25-node function from 1.61 to
   0.93 µs, §10); left, the 10 to 15 ns a node one call pays since the change, which needs a
   profiler to place.

## 12. What stays refused

Not every JavaScript expression has a Holon meaning; these are refused by design, each naming
Holon's form, and a leaf wouldn't change that:

- **Values Holon hasn't:** `null` (absence is the one "not there"), `NaN` and `Infinity` (a
  computation that fails gives nothing), `BigInt`, `Symbol`. Holon's values are numbers, texts,
  booleans, lists and records.
- **Identity and change in place:** objects compare by value; `push`, `sort` or `splice` on a shared
  value become their value forms (`[...xs, x]`, `toSorted`, `toSpliced`).
- **Behaviour on values:** classes, prototypes, getters, setters, `Proxy`, `Reflect`, generators.
  `Map` and `Set` as types are records with text keys and lists.
- **The environment:** `Date.now()`, `Math.random()`, `crypto.randomUUID()`, `Intl` and locales
  read the machine; a flow imports the clock and randomness (`holon:clock`, `holon:random`), fixed
  and recorded for the run, and has no locale.
- **Time and concurrency:** timers and `Promise` are handlers (`every`), waits (`await event(…)`)
  and the `await` of requests and actions.
- **Statements** are the L4's slices, not the library's: `try`, `catch`, `throw` (slice 5, built),
  `while` and loops (slice 6).
