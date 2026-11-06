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

**In numbers:** of JavaScript's library functions, ten are read: `JSON.parse`, `JSON.stringify`,
`Object.keys`, and the methods `.map`, `.reduce`, `.split`, `.join`, `.indexOf`, `.toUpperCase`,
`.toLowerCase`. None of `Math`'s 35 functions and 8 constants is. The operators and literals are
mostly mapped (§1, §2). Of the catalog's 56 computing leaves, 19 can only be called by name
(§10).

## 1. Operators

| JavaScript | Leaf | Status | Note |
|---|---|---|---|
| `a + b` (numbers) | `add` | mapped | a chain `a + b + c` is one `add` with three inputs |
| `a + b` (a text literal on a side) | | refused | "there is no + on texts: use a template literal, `${a}${b}`" |
| `a + b` (texts that aren't literals) | `add` | differs | answers nothing ([§9](#9-what-compiles-but-differs)) |
| `a - b` | `subtract` | mapped | |
| `a * b` | `multiply` | mapped | a chain is one node |
| `a / b` | `divide` | differs | by zero: nothing, where JavaScript gives `Infinity` or `NaN` |
| `a % b`, `a ** b` | | js() | "% isn't an operator Holon has" |
| `-x` | `const` | mapped for a number literal | any other: "a minus needs a number after it (to negate a value, write 0 - x)" |
| `+x`, `typeof`, `void`, `~` | | js() | "… isn't an operator Holon has" |
| `x++`, `x--`, `x += 1` | `add`, `subtract` | mapped as statements | "++ and -- are a statement of their own: n++" in an expression |
| `a === b`, `a == b` | `equals` | mapped; differs on records and lists | `==` is `===` (no coercion); records and lists compare by value |
| `a !== b`, `a != b` | `equals`, `not` | mapped | |
| `a < b`, `<=`, `>`, `>=` | `compare {op}` | mapped for numbers | texts: differs, nothing (JavaScript compares them in order) |
| `a && b`, `a \|\| b` | `and`, `or` | mapped for booleans | a literal on the right is refused: "takes booleans: a value for where one is absent is ??"; other values: differs |
| `!x` | `not` | mapped for booleans | other values: differs, nothing (JavaScript's truthiness is `is_truthy`, by name) |
| `a ?? b` | `default` (b a literal), `coalesce` | mapped | `b` is computed only when `a` is absent |
| `c ? a : b` | `guard`, then `default` or `coalesce` | mapped | only the branch taken runs; `c ? a : undefined` is the `guard` alone |
| `x?.k`, `x?.[k]` | `field`, `lookup` | mapped | absence passes along anyway, so `?.` and `.` lower alike |
| `k in r`, `instanceof` | | js() | "in isn't an operator Holon has" |
| `&`, `\|`, `^`, `<<`, `>>`, `>>>` | | js() | |
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
| `{ ...base, k: v }` | `with_field` over `base` | mapped | one spread, first: "a record spreads one base, first" |
| `{ [k]: v }` | `insert` | by name | "a computed key comes in a later slice: name the key, or use insert(record, key, value)" |
| `r.k` | `field` | mapped | |
| `r[k]` | `lookup` | mapped for a record | a list: differs, `xs[0]` answers nothing; `get(xs, 0)` by name |
| `store[k]` (a collection, folder, database) | `collection_get`, `file_read`, … | mapped | §8 |
| `xs.length`, `s.length` | `length`, `text_length` | by name | "a list's length is length(xs), a text's text_length(s), imported from "holon": .length comes in a later slice" |
| `undefined` | | as a branch | `c ? a : undefined`, `x !== undefined` in an `if`; else "undefined is written as a branch" |
| `null` | | refused | "null isn't a value here: a value that isn't there is absent, undefined" |
| `NaN`, `Infinity` | | refused | unknown names |
| `/re/` | | js() | "a regular expression isn't something Holon computes" |
| `new X(…)` | | js() | |
| `class`, getters, setters, methods in a record | | refused | "a record holds values: no getters, setters or methods" |
| `(x) => …` | `map`'s or `fold`'s function | mapped there only | elsewhere: "a function written in place goes to map or reduce" |

## 3. Global functions

| JavaScript | Status | In Holon |
|---|---|---|
| `String(x)` | by name; compose | `as_text(x)`, or `` `${x}` `` |
| `Number(s)`, `parseFloat(s)`, `parseInt(s)` | js(); compose | no leaf reads a number from text; `JSON.parse(s)` reads `"42"` as 42 |
| `Boolean(x)`, `!!x` | by name | `is_truthy(x)` |
| `isNaN`, `isFinite` | js() | a computation that fails gives nothing, never `NaN` |
| `encodeURIComponent`, `decodeURIComponent`, `encodeURI`, `decodeURI` | js() | a route's `req.params` and `req.query` come decoded |
| `structuredClone` | not needed | values are never changed in place |
| `setTimeout`, `setInterval` | refused, import | a flow has no globals: a clock is a handler, `every("5s", () => …)`; a wait is `await event("paid", { expires: "1d" })` |
| `fetch` | refused, import | `import api from "host:api.example.com"`, `await api.get("/p")` (§8) |
| `console.log` | refused | not needed: each node's value is in the run's record (`holon explain`) |
| `globalThis`, `window`, `process`, `require`, `Promise` | refused | "a flow has no globals; import what it uses" |

## 4. `Math`

Every member is refused today, with "Math isn't there: a flow has no globals; import what it
uses" (there is nothing to import), so each is **js()**. The numeric leaves are `add`,
`subtract`, `multiply`, `divide`, `compare`, `sum` and `range`.

| Member | Status | Compose from mapped forms |
|---|---|---|
| `max(a, b)`, `min(a, b)` | js(); compose | `a > b ? a : b`, `a < b ? a : b` |
| `abs(x)` | js(); compose | `x < 0 ? 0 - x : x` |
| `sign(x)` | js(); compose | `x > 0 ? 1 : x < 0 ? -1 : 0` |
| `max(...xs)`, `min(...xs)` | js(); compose | `xs.reduce((m, x) => (x > m ? x : m), xs0)`, the start given |
| `round`, `floor`, `ceil`, `trunc` | js() | no leaf rounds |
| `pow`, `sqrt`, `cbrt`, `hypot`, `exp`, `expm1`, `log`, `log10`, `log2`, `log1p` | js() | `pow(x, 2)` composes as `x * x` |
| `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh` | js() | |
| `clz32`, `fround`, `imul` | js() | |
| `random()` | refused, import | `import { random } from "holon:random"`: one number for the run, recorded so it replays; `Math.random` gives a new one each call |
| `PI`, `E`, `LN2`, `LN10`, `LOG2E`, `LOG10E`, `SQRT2`, `SQRT1_2` | refused | write the number |

## 5. `Number`

| Member | Status | In Holon |
|---|---|---|
| `Number.isInteger`, `isFinite`, `isNaN`, `isSafeInteger` | js() | (`Number` is an unknown name) |
| `Number.parseFloat`, `Number.parseInt` | js() | as `parseFloat` (§3) |
| `MAX_SAFE_INTEGER`, `MIN_SAFE_INTEGER`, `EPSILON`, `MAX_VALUE`, `MIN_VALUE` | refused | write the number |
| `x.toString()` | compose | `` `${x}` `` |
| `x.toString(radix)`, `toFixed`, `toPrecision`, `toExponential`, `toLocaleString` | js() | "no leaf does .toFixed(…)" |

## 6. `String.prototype`

| Method | Leaf | Status | Note |
|---|---|---|---|
| `toUpperCase()`, `toLowerCase()` | `uppercase`, `lowercase` | mapped | |
| `split(sep)` | `split` | mapped | `split("")`: differs, nothing (JavaScript gives the characters); a limit: js() |
| `indexOf(t)` | `index_of` | mapped | counts characters; a start position: js() |
| `length` | `text_length` | by name | §2 |
| `concat(t)` | `concat` | by name; compose | `` `${s}${t}` `` |
| `substr(start, len)` | `slice` | by name | the leaf takes a length, as `substr` does: `slice(s, start, len)` |
| `slice(start, end)`, `substring(start, end)` | `slice` | by name | `slice(s, start, end - start)`; negative positions: js() |
| `charAt(i)` | `slice` | by name | `slice(s, i, 1)` |
| `includes(t)` | `index_of` | compose | `s.indexOf(t) !== -1` |
| `startsWith(t)` | `index_of` | compose | `s.indexOf(t) === 0` (it searches the whole text) |
| `replaceAll(a, b)` (texts) | `split`, `join` | compose | `s.split(a).join(b)` |
| `at(i)`, `endsWith`, `lastIndexOf` | | js() | |
| `trim`, `trimStart`, `trimEnd` | | js() | |
| `padStart`, `padEnd`, `repeat` | | js() | |
| `replace`, `match`, `matchAll`, `search` | | js() | regular expressions |
| `charCodeAt`, `codePointAt`, `normalize`, `localeCompare`, `isWellFormed`, `toWellFormed` | | js() | |
| `toLocaleUpperCase`, `toLocaleLowerCase` | | js() | |
| `String.fromCharCode`, `String.fromCodePoint`, `String.raw` | | js() | |

## 7. Arrays

| Method | Leaf | Status | Note |
|---|---|---|---|
| `map((x) => …)`, `map(f)` | `map` | mapped | the item only: "map's function takes the item: (x) => …; an index isn't passed" |
| `reduce((acc, x) => …, init)` | `fold` | mapped | the start is needed; without it: "reduce takes a function and the start" |
| `join(sep)` | `join` | mapped | texts; `join()` (JavaScript's `","`): refused |
| `indexOf(x)` | `index_of` | differs | `index_of` searches texts: on a list, nothing |
| `length` | `length` | by name | §2 |
| `xs[i]`, `at(i)` | `get` | by name | `xs[i]` lowers to `lookup` and answers nothing (§9); `at(-1)`: js() |
| `[x, ...xs]` (`unshift`) | `extend` (`cons`) | mapped | |
| `[...xs, x]` (`push`), `concat(ys)` as `[...xs, ...ys]` | `append`, `extend` | mapped | the methods themselves (`push`, `concat`): js() |
| `xs[0]` as `shift()`'s answer, `slice(1)` | `head`, `tail` | by name | |
| `xs.length === 0` | `is_empty` | by name | |
| sum: `reduce((a, b) => a + b, 0)` | `fold` and `add`; `sum` | mapped; by name | |
| `Array.from({ length: n }, (_, i) => i)`, `[...Array(n).keys()]` | `range` | by name | n up to 10000 |
| `find((x) => x.id === v)` | `find {name}` | by name | equality on one field only: `find(xs, v, { name: "id" })` |
| `filter(f)` | `fold` | compose | `xs.reduce((acc, x) => (f(x) ? [...acc, x] : acc), [])`; the method: "a function written in place goes to map or reduce" |
| `some(f)`, `every(f)` | `fold` | compose | `xs.reduce((acc, x) => acc \|\| f(x), false)` (`some`, the leaf, is something else: an optional value) |
| `includes(x)` | `fold`, `equals` | compose | `xs.reduce((acc, y) => acc \|\| y === x, false)` |
| `flat()` | `fold`, `extend` | compose | `xs.reduce((acc, ys) => [...acc, ...ys], [])` |
| `flatMap`, `findIndex`, `findLast`, `findLastIndex`, `lastIndexOf`, `reduceRight` | | js() | |
| `sort`, `toSorted`, `reverse`, `toReversed` | | js() | |
| `slice(a, b)`, `splice`, `toSpliced`, `with`, `fill`, `copyWithin` | | js() | |
| `forEach` | | refused | a statement per item has no form yet; `map` computes |
| `entries`, `keys`, `values` (iterators) | | js() | |
| `Array.isArray`, `Array.of`, `Array.from` | | js() | `Array` is an unknown name |

## 8. `Object`, `JSON`, and what a flow imports

| JavaScript | Leaf | Status | Note |
|---|---|---|---|
| `Object.keys(r)` | `keys` | mapped | |
| `Object.values`, `entries`, `fromEntries`, `assign`, `freeze`, `hasOwn`, `groupBy`, … | | js() | "Object has keys, not "values": do the rest in a js() function" |
| `delete r.k` (a record value) | `without_field`, `remove` | by name | |
| `JSON.parse(t)` | `json_parse` | mapped | invalid JSON: nothing, where JavaScript throws; a reviver: refused |
| `JSON.stringify(v)` | `json_stringify` | mapped | a replacer or indent: refused |
| `Date`, `Date.now()`, `new Date()` | `clock` | refused, import | `import { now } from "holon:clock"`: seconds, fixed for the run (`Date.now()` is milliseconds, and moves); formatting a date: js(), the time passed in |
| `Intl` | | js() | not inside `js()` either: locale is the environment |
| `Map`, `Set`, `WeakMap`, `Symbol`, `BigInt`, `Proxy`, `Reflect`, typed arrays | | js() | inside `js()` only (a `BigInt` can't cross out) |
| `Promise.all` | | refused | in slice 7, a started run's handle may be awaited or passed to it |
| `throw`, `Error`, `try`/`catch` | | refused | slice 5 (`fail` leaf) |

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
| `btoa`, `atob`, `Buffer…toString("base64")` | `base64(t)`, `base64(t, { mode: "decode" })`, `{ alphabet: "url" }` | `base64` (by name) |
| `Buffer…toString("hex")` | `hex(t)`, `{ mode: "decode" }` | `hex` (by name) |
| `crypto.randomUUID()` | none | `random` is one number per run |
| `new URL`, `URLSearchParams`, `TextEncoder` | js() | |

## 9. What compiles but differs

These are read and lowered, but the leaf computes something else than JavaScript does. The
author sees no mistake; the run answers nothing where JavaScript answers something:

| JavaScript | What happens | Why |
|---|---|---|
| `xs[0]` on a list | `lookup`: nothing | `lookup` reads a record by a text key (`primitives.rs`, `Op::Lookup`); a list's item is `get` |
| `"a" < "b"` | `compare`: nothing | `compare` takes numbers (`bin_numbers`) |
| `!x`, `x && y`, `x \|\| y` on a value that isn't true or false | `not`, `and`, `or`: nothing | they take booleans (`as_bool`); JavaScript's truthiness is `is_truthy` |
| `a + b` on texts that aren't literals | `add`: nothing | `add` takes numbers; refused only when a side is a text literal |
| `xs.indexOf(x)` on a list | `index_of`: nothing | `index_of` searches texts |
| `s.split("")` | nothing | an empty separator gives nothing (`Op::Split`) |
| `a / 0` | nothing | JavaScript gives `Infinity` or `NaN`, which Holon has no value for |
| `{ a: 1 } === { a: 1 }` | `true` | compared by value; JavaScript compares objects by identity |
| `1 == "1"` | `false` | `==` is `===` |
| `JSON.parse("bad")` | nothing | JavaScript throws; Holon has no `catch` until slice 5 |

Each is a candidate for the reader to refuse with the right form, or to lower by the value's type
(`xs[i]` to `get`, `!x` to `not(is_truthy(x))`, `<` on texts to a text comparison), once the
reader knows the types it reads.

## 10. Leaves with no JavaScript form

The computing leaves an author can only call by name. Most have a JavaScript form waiting to be
read; reading it is a line in `ts.rs` each, with a test of its meaning:

| Leaf | Its JavaScript form | Note |
|---|---|---|
| `length`, `text_length` | `.length` | "comes in a later slice"; which one depends on the value's type |
| `get` | `xs[i]`, `xs.at(i)` | §9: `xs[i]` lowers to `lookup` |
| `slice` | `s.substr(start, len)`; `s.slice(a, b)` as `slice(s, a, b - a)` | |
| `head`, `tail` | `xs[0]`, `xs.slice(1)` | |
| `is_empty` | `xs.length === 0` | |
| `sum` | `xs.reduce((a, b) => a + b, 0)`, which lowers to `fold` and `add` today | |
| `range` | `Array.from({ length: n }, (_, i) => i)` | |
| `find` | `xs.find((x) => x.id === v)` | only that shape |
| `is_truthy` | `Boolean(x)`, `!!x`, and the truthiness of `!`, `&&`, `\|\|` | §9 |
| `as_text` | `String(x)` | `` `${x}` `` already uses it |
| `insert` | `{ ...r, [k]: v }` | a computed key |
| `remove`, `without_field` | `delete r[k]`, `const { k, ...rest } = r` | on a record value |
| `cons` | `[x, ...xs]` | lowers to `extend` today, the same list |
| `select` | `c ? a : b` | the ternary lowers to `guard` and a fallback instead, so only the branch taken runs |
| `sha256`, `hmac_sha256`, `secure_equals`, `hex`, `base64` | none in the language standard (Node's `crypto` and `Buffer`, the browser's `btoa`) | by name is their form |
| `to_text` | none (`JSON.stringify` of a text is near it) | |
| `pair`, `some`, `none` | none | the lowering's own: two values together, an optional value |

## 11. Closing the gaps, by the gate

Each gap of §1–§8, and the way [§0](#0-the-rule) closes it. The reasons are those of
COMPOSABLE_VM_DESIGN.md §13: a composite
where it stays in the leaf's class and close to its cost, a leaf where it wouldn't. Two facts of
the evaluator decide most rows: `fold` visits every item (JavaScript's `some`, `find` and
`includes` stop at the first), and `append`, `extend`, `cons` and `insert` copy their whole input
(`primitives.rs`, `Op::Append`: `as_list(l)?.to_vec()`), so a `fold` that grows a list or a record
by them is O(n²) where JavaScript is O(n). "Measure" marks a row whose class is the leaf's and
whose constant the benchmark decides.

Both facts belong to the evaluator, not to the language, and changing them moves rows. A list is a
`Vec` and a record a `BTreeMap` (`value.rs`), and a leaf reads its inputs by reference, so it must
copy to grow one; a `fold` that handed its accumulator to the step by value, with `append` and
`insert` growing an input nobody else holds in place, would make `filter`, `reverse`, `flat`,
`fromEntries` and `groupBy` O(n) composites; a `fold` that can stop (slice 6, "`fold` stopping when
done") does the same for `some`, `every`, `find` and `includes`. Those rows are leaves only until
then, and the benchmark decides once they are composites in the same class.

**Numbers**

| Gap | Closes by | Why |
|---|---|---|
| `Math.round`, `floor`, `ceil`, `trunc` | leaf | not expressible: no atom drops a fraction. IEEE-exact; `round` is JavaScript's (halves toward +∞, `Math.round(-2.5)` is −2), not Rust's `f64::round` (−3) |
| `a % b` | leaf | `a - b * Math.trunc(a / b)` rounds in the division where JavaScript's `%` is exact (fmod), so the composite differs |
| `-x` | composite: `x * -1` | exact in IEEE, `-0` included (`0 - x` gives `0` for `0`, where `-0` is right) |
| `Math.sqrt` | leaf | correctly rounded by IEEE |
| `a ** b`, `Math.pow`, `exp`, `expm1`, `log`, `log2`, `log10`, `log1p`, `cbrt`, `hypot`, trigonometry | leaf, pinned | not expressible; the same bits natively and in wasm need one pure-Rust implementation (`libm`) |
| `Math.abs`, `sign`, `min(a, b)`, `max(a, b)` | measure; likely leaf | a comparison and a choice are the leaf's class, but they must match at `-0` (`Math.max(-0, 0)` is `0`), which a plain `a > b ? a : b` doesn't, and they are paid in inner loops (§13's "hot and pervasive") |
| `Math.min(...xs)`, `Math.max(...xs)` | composite: `fold` of the two-input one | O(n) like a leaf; measure the fold's step |
| `Number(s)`, `parseFloat`, `parseInt`, `+s` | leaf | not expressible: text to number. `JSON.parse` takes only JSON's grammar (no `" 42 "`, no `"0x1F"`) |
| `x.toFixed`, `toString(radix)`, `toPrecision`, `toExponential` | leaf | JavaScript's exact formatting |
| `Number.isInteger(x)` | composite: `x === Math.trunc(x)` | once `trunc` is a leaf |
| `Number.isNaN`, `Number.isFinite` | composite: a constant | Holon's numbers are always finite (§12) |
| `isNaN(s)`, `isFinite(s)` on a text | composite: whether `Number(s)` gives a number | once `Number` is a leaf |

**Texts**

| Gap | Closes by | Why |
|---|---|---|
| `s.length` | `text_length` (exists) | read the form, by the value's type |
| `s.slice(a, b)`, `substring`, `charAt`, `at` | composite over `slice` and `text_length` | a few O(1) nodes around an O(n) leaf |
| `includes(t)` | composite: `s.indexOf(t) !== -1` | the same scan |
| `startsWith`, `endsWith` | leaf | `index_of` scans the whole text: O(n) where JavaScript compares the m characters |
| `lastIndexOf` | leaf | a composite loops over positions |
| `trim`, `trimStart`, `trimEnd` | leaf | a composite walks the characters through `fold`, a node per character |
| `repeat(n)` | leaf | a `fold` of `concat` copies the text each step: O(n²) |
| `padStart`, `padEnd` | composite over `repeat`, `slice`, `text_length` | once `repeat` is a leaf |
| `replaceAll(a, b)` with texts | composite: `s.split(a).join(b)` | two passes, the same class |
| `replace(a, b)` with texts | composite over `index_of`, `slice`, `concat` | the same class |
| `split("")`, `split(sep, limit)` | leaf change: `split` takes `""` as the characters; a limit as a parameter | the leaf answers nothing for `""` today |
| `charCodeAt`, `codePointAt`, `String.fromCharCode`, `fromCodePoint` | leaf | not expressible |
| `normalize` | leaf | Unicode's tables (`unicode-normalization` is in the build) |
| `a < b` on texts, `localeCompare` | leaf change: `compare` on texts | code-unit order, as JavaScript's `<`; no locale (§12) |
| `toLocaleUpperCase`, `toLocaleLowerCase` | the same as `toUpperCase`, `toLowerCase` | no locale, as inside `js()` |
| `isWellFormed`, `toWellFormed` | composite: `true`, the text itself | a Holon text is always well formed (UTF-8) |
| `/re/.test`, `match`, `matchAll`, `search`, `replace` with a pattern | leaf | not expressible. ECMAScript's own pattern grammar, through `regress` (the pure-Rust engine Boa uses), so a pattern means what it means inside `js()` |
| `encodeURIComponent`, `decodeURIComponent`, `encodeURI`, `decodeURI` | leaf | byte by byte |

**Lists**

| Gap | Closes by | Why |
|---|---|---|
| `xs.length`, `xs[i]`, `at(i)` | `length`, `get` (exist); `at(-1)` a composite with `length` | read the forms, by the value's type (§9) |
| `filter(f)` | leaf, with its function as `map` takes one | a `fold` of `[...acc, x]` copies the list each step: O(n²) |
| `some`, `every`, `includes`, `indexOf`, `find(f)`, `findIndex` | leaf, stopping at the first | `fold` visits every item; JavaScript stops |
| `findLast`, `findLastIndex`, `lastIndexOf` | leaf | the same, from the end |
| `flat()` | leaf | a `fold` of `extend` copies: O(n²) |
| `flatMap(f)` | composite: `flat` of `map` | once `flat` is a leaf |
| `sort`, `toSorted`, with or without a comparator | leaf | not expressible in O(n log n). JavaScript's default compares as texts, and its sort is stable |
| `reverse`, `toReversed` | leaf | a `fold` of `cons` copies: O(n²) |
| `slice(a, b)` | leaf | one copy, where a composite gets each item through its own node |
| `splice`, `toSpliced`, `with`, `fill` | composite over list `slice` and `extend` | the same class once `slice` is a leaf |
| `join()` | composite: `join(xs, ",")` | |
| `reduceRight` | composite: `fold` over `reverse` | |
| `keys()`, `entries()`, `Array.from({ length: n }, f)` | composite over `range` and `map` | |
| `[...new Set(xs)]` | leaf `unique` | a composite compares each item with every other: O(n²) |
| `Array.isArray`, `typeof` | leaf `type_of` | not expressible: nothing tells a value's kind today |

**Records, JSON, dates**

| Gap | Closes by | Why |
|---|---|---|
| `Object.values`, `Object.entries` | composite: `map` over `keys`, a `lookup` each | the same class; measure |
| `Object.fromEntries`, `Object.assign`, `{ ...a, ...b }`, `Object.groupBy` | leaf | a `fold` of `insert` copies the record each step: O(n²) |
| `k in r`, `Object.hasOwn(r, k)` | composite: `r[k] !== undefined` | a record never holds an absent value, so it is exact |
| `{ [k]: v }`, `delete r[k]` | `insert`, `remove` (exist) | read the forms |
| `structuredClone(v)`, `Object.freeze(v)` | composite: the value itself | values never change in place |
| `JSON.stringify(v, null, 2)` | leaf change: an indent parameter | JavaScript's exact layout |
| a date from `now`: `new Date(now * 1000).toISOString()`, its UTC year, month, day | leaf (UTC parts and ISO text) | calendar arithmetic, pinned; a composite of `divide` and `floor` is possible but long. No time zones (§12) |

Besides the leaves, some rows need the reader to change, not the catalog: a form whose leaf
depends on the value's type (`.length`, `xs[i]`, `<`, `!`) needs the reader to know that type
where the form is, from a type annotation (L4 §7) or from the leaf that made the value; where it
can't, the form is refused, saying which leaf to call.

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
- **Statements** are the L4's slices, not the library's: `try`, `catch`, `throw` (slice 5),
  `while` and loops (slice 6).
