# Records, JSON, dates: how each gap closes

As of 2026-11-06, on `c6d350e` (the races' harness). Raced by
`cargo bench -p holon-author --bench races_records` (`crates/holon-author/benches/races_records.rs`),
about 30 seconds, every check passing first. Other builds shared the machine while it ran: the
same race differs by up to 2× between runs (`Object.values` at n = 64 gave 37.6× in one run and
61.0× in the one below). No verdict is near a threshold. The four rows that grew with n were
re-raced after the evaluator change (#43), on 2026-11-07 at `02032df`, with the same bench; their
rows and race output below are that run's. The smallest ratio measured anywhere is 4.3×
(`{ ...a, ...b }` at 512 keys), and every verdict rests on a ratio of 5.4× or more at n = 64.

The decision rule is the [README](README.md)'s. **No composite in this domain comes within 3× of
its candidate.** A composite pays about 130 ns a leaf (a scalar composite of 9 leaves costs
1.1–1.4 µs); a candidate costs 45–60 ns with its dispatch. A `map` applying a function of a few
leaves pays 1.2–2 µs an item. So every expressible gap below is a leaf, most by 15× or more. Four
rows lost by more as n grew, because a `fold` copied the record it grows (`insert`, `append`):
`Object.fromEntries`, `Object.assign`, `{ ...a, ...b }` and `Object.groupBy`. **None lost only to
the copy**: at n = 8, where the copy is eight keys, they were already 15×, 17×, 17× and 20×. Since
the evaluator change (#43) gives a `fold` its accumulator, the copy is gone: re-raced, each is in
its candidate's class, the ratio falling as n grows (7.8×, 5.4× and 7.4× at 64; 4.8×, 4.3× and 5.1×
at 512), and each is still above 3. They stay leaves.

## Key order

JavaScript orders an object's own keys (OrdinaryOwnPropertyKeys, ECMA-262 §10.1.11.1) as follows:

1. Array-index keys first, ascending as numbers (`"2"` before `"10"`). An array-index key is the
   canonical text of an integer from 0 to 2³² − 2.
2. Every other text key in the order it was first created.

A reassigned key keeps its place; a deleted and re-added key goes last. `Object.keys`, `values`,
`entries`, `for…in`, spread, `Object.assign`, `JSON.stringify` and `Object.groupBy`'s groups all
follow that order, and `JSON.parse` creates keys in the text's order.

A Holon record is a `BTreeMap<String, Value>` (`value.rs`). Its keys are always sorted by their
UTF-8 bytes, which is code point order: `"10" < "2" < "a" < "é" < "\u{E000}" < "😀"`. That isn't
even what JavaScript's `Object.keys(r).sort()` gives. `sort()` compares UTF-16 code units, so `"😀"`
(`D83D DE00`) sorts before `"\u{E000}"`. `keys`, `json_stringify`, `to_text`, `as_text` and every
leaf here follow the sorted order.

**Recommendation: keep records sorted.** Keeping insertion order would cost this:

- **The value's identity.** `value_hash.rs` encodes a record's fields in key order ("canonical"),
  and the memo key, the state root and a collection item's hash rest on it. With insertion order,
  either `{ a, b }` and `{ b, a }` become two values, or the encoding sorts first.
  - Two values: they hash differently and compare unequal, a cached run misses on a rebuilt record,
    and an item read back from a store isn't the item written if a leaf rebuilt it.
  - Sort first: O(n log n) for every encode, and then two values that hash the same print
    differently through `json_stringify`. The run record shows a value and a replay recomputes it,
    so the two must print the same.
- **Equality.** JavaScript has no record equality to follow. Holon's `===` compares by value, so it
  must either ignore order (a sort or a set comparison each time) or count it (`{a:1,b:2} !==
  {b:2,a:1}`, unlike JSON's object model and every API that sends JSON).
- **Every leaf that builds a record** would need JavaScript's position rules: integer keys first,
  an existing key keeping its place, a removed key re-added last. That covers `with_field`,
  `insert`, `remove`, `json_parse`, `merge`, `from_entries` and `group_by`.
- **The representation.** It would need an ordered map: a `Vec` with an index (or a linear search,
  O(n) a `lookup`), and an O(n) `remove`. That is either a dependency (`indexmap`) or a
  hand-written map in core, which keeps to the few dependencies it has (`json.rs`, top).

What sorted order costs is visible only where a flow observes order. That covers the order of
`Object.keys`, `values` and `entries`, of `JSON.stringify`'s keys, and of `Object.groupBy`'s groups.
Where the written order matters, the author gives it: `JSON.stringify(v, ["id", "name", "items"])`
(the key list below) writes exactly that order, as JavaScript does. The reader's mistakes and hover
for these forms say that keys come sorted.

## Verdicts

"×" is the composite's time over the candidate's: at n = 64 for a race over sizes, else the scalar.
"Mapped" is the same composite applied by a `map` to 64 items, against the candidate applied by
`map`. The date composites are timed with `floor`, `trunc` and the date parts stood in by one-node
leaves (see the bench's header), so a date ratio is what the composite would cost with those
leaves.

| JavaScript | Verdict | How | Measured | Why |
|---|---|---|---|---|
| `Object.keys(r)` | reader (exists) | `keys` | | already read; the order is sorted ([Key order](#key-order)) |
| `Object.values(r)` | leaf | `values` | 61× (28× at 8, 43× at 512) | same class, above 3: a `map` pays ~300 ns an item to copy what the leaf copies in ~3 |
| `Object.entries(r)` | leaf | `entries` | 5.9× (8.7× at 8, 6.1× at 512) | same class, above 3 |
| `Object.fromEntries(es)` | leaf (re-raced) | `from_entries` | 7.8× (15× at 8, 4.8× at 512) | same class since the evaluator change (it was 25× at 64, 96× at 512, each `insert` copying the record); above 3 |
| `Object.assign(t, ...s)` | leaf (re-raced) | `merge` (variadic) | 5.4× (7.5× at 8, 4.3× at 512) | as `{ ...a, ...b }` |
| `Object.freeze(v)` | reader | the value itself; the local marked frozen | | values never change in place; a later change to the local is refused where JavaScript throws |
| `Object.isFrozen(v)` | refused | | | §12, change in place: no value is unfrozen, so the question has no Holon meaning |
| `Object.seal(v)` | reader | as `freeze` | | as `freeze` (stricter: a change to an existing key is refused too) |
| `Object.isSealed(v)` | refused | | | as `isFrozen` |
| `Object.preventExtensions(v)` (and `isExtensible`) | reader (`isExtensible` refused) | as `freeze` | | as `freeze`, `isFrozen` |
| `Object.hasOwn(r, k)` | leaf | `has` | 19× scalar; 10× in a `map`, against a `lookup` there | above 3 |
| `r.hasOwnProperty(k)` | leaf | `has` | as `hasOwn` | the same question; a record has no prototype |
| `Object.groupBy(xs, f)` | leaf (re-raced) | `group_by {fn}` | 7.4× (9.2× at 8, 5.1× at 512) | same class since the evaluator change (it was 62× at 64, 431× at 512, `insert` copying the record and `append` the group each item); above 3 |
| `Object.create(null)`; `Object.create(proto)` | reader; refused | `record` (`{}`) | | a record is an object with no prototype; a prototype is §12's behaviour on values |
| `Object.defineProperty`, `defineProperties` | refused | | | §12: attributes (not enumerable, not writable) and accessors are behaviour on values; Holon's form is `{ ...r, [k]: v }` |
| `Object.getOwnPropertyNames(r)` | reader | `keys` | | a record has no keys that aren't enumerable |
| `Object.getPrototypeOf`, `setPrototypeOf` | refused | | | §12, prototypes |
| `Object.is(a, b)` | leaf | `same_value` | | not expressible: no leaf tells `-0` from `0` (`divide` refuses a zero divisor, `to_text` and `json_stringify` write `0`) |
| `k in r` | leaf | `has` | as `hasOwn` | above 3; own keys only, there being no prototype chain |
| `delete r.k`, `delete r[k]` | reader (exists) | `without_field`, `remove` | | read as a statement on a local today; on a parameter, "copy it first"; in an expression, "delete is a statement" |
| `{ ...a, ...b }` | leaf (re-raced) | `merge`, and the reader | 5.4× (7.5× at 8, 4.3× at 512) | same class since the evaluator change (a `fold` of `insert` over `b`'s keys, 29× at 64 and 156× at 512 while it copied the record); above 3 |
| `{ [k]: v }` | reader | `insert` | | exact, for a text key |
| `a?.b?.c` | reader (exists) | `field`, `lookup` | | absence passes along, as `?.` gives undefined |
| `===`, `==`, `!==` on records and lists | refused (stays by value) | `equals` | | §12, identity: by value where JavaScript compares identity |
| `typeof x` | leaf | `type_of` | 37× on a number (42× mapped); 154× on 64 keys, 1552× on 512 | worse class: the composite reads `to_text`'s first character, and `to_text` writes the whole value |
| `x instanceof C` | refused | | | §12, classes and prototypes; `Array.isArray` (lists.md) and `typeof` say a value's kind |
| `structuredClone(v)` | reader | the value itself, as a copy not shared | | values never change in place; it must not be marked shared, or a change after it is refused |
| `JSON.parse(t, reviver)` | leaf change | `json_parse {fn}` | | not expressible: the reviver reaches every depth, and no function calls itself (`a holon can't contain itself`) |
| `JSON.stringify(v, replacer)` (a function) | leaf change | `json_stringify {fn}` | | not expressible, as the reviver |
| `JSON.stringify(v, keys)` (a list) | leaf change | `json_stringify {keys}` | | not expressible: the list filters every depth, and orders keys as a record can't |
| `JSON.stringify(v, null, indent)` | leaf change | `json_stringify {indent}` | | not expressible: a layout at every depth |
| `toJSON` | refused | | | a record holds values, no methods; a date's `toJSON` is `date_text` |
| key order | differs; stays | | | [Key order](#key-order) |
| `JSON.stringify(v)`: numbers and `\b`, `\f` | leaf change | `json_stringify` | | it writes `1e21` as `1000000000000000000000` and `1e-7` as `0.0000001`, and `\b`, `\f` as `\u0008`, `\u000c`; JavaScript's are `1e+21`, `1e-7`, `\b`, `\f` |
| `JSON.parse(t)` with a key twice | differs; stays | | | JavaScript keeps the last; `json.rs` refuses an ambiguous body by design, a choice this page leaves standing but doesn't decide |
| `new Date(ms)` | leaf | `date_part {part: "time"}` | 33× (33× mapped) | depends on `trunc`; above 3 |
| `Date.UTC(y, m, …)` | leaf | `date_utc` | 25× over `utc_time`; `utc_time` itself 168× over `floor` | above 3 |
| `Date.parse(t)`, `new Date(t)` | leaf | `date_parse` | not raced | expressible only as a composite testing each character (`slice` and `index_of` each), well over 150 leaves; every date composite raced, 8 to 209 leaves, lost 15× to 168× |
| `d.toISOString()` | leaf | `date_text {format: "iso"}` | 58× (65× mapped) | depends on `date_part`; above 3 |
| `d.toJSON()` | reader | `date_text {format: "iso"}` | | the same text for a valid date; Holon has no invalid one |
| `d.toUTCString()` (`toGMTString`) | leaf | `date_text {format: "utc"}` | as `toISOString` | the same shape: parts, padded and joined |
| `d.getUTCFullYear()` | leaf | `date_part {part: "year"}` | 117× (122× mapped) | depends on `floor`; above 3 |
| `d.getUTCMonth()` | leaf | `date_part {part: "month"}` | 103× (113× mapped) | depends on `floor`; above 3 |
| `d.getUTCDate()` | leaf | `date_part {part: "date"}` | 103× (120× mapped) | depends on `floor`; above 3 |
| `d.getUTCDay()` | leaf | `date_part {part: "day"}` | 29× (32× mapped) | depends on `floor`; above 3 |
| `d.getUTCHours()` | leaf | `date_part {part: "hours"}` | 25× (29× mapped) | depends on `floor`; above 3 |
| `d.getUTCMinutes()` | leaf | `date_part {part: "minutes"}` | 25× (28× mapped) | depends on `floor`; above 3 |
| `d.getUTCSeconds()` | leaf | `date_part {part: "seconds"}` | 25× (29× mapped) | depends on `floor`; above 3 |
| `d.getUTCMilliseconds()` | leaf | `date_part {part: "milliseconds"}` | 28× (30× mapped) | depends on `floor`; above 3 |
| `d.setUTCFullYear(y, m?, dt?)` | leaf | `date_set {part: "year"}` | 15× | depends on `date_part` and `utc_time`; above 3 |
| `d.setUTCMonth(m, dt?)` | leaf | `date_set {part: "month"}` | as `setUTCHours` | the same composite, one part replaced (checked) |
| `d.setUTCDate(dt)` | leaf | `date_set {part: "date"}` | as `setUTCHours` | as `setUTCMonth` |
| `d.setUTCHours(h, m?, s?, ms?)` | leaf | `date_set {part: "hours"}` | 15× (22× mapped) | depends on `date_part` and `utc_time`; above 3 |
| `d.setUTCMinutes(m, s?, ms?)` | leaf | `date_set {part: "minutes"}` | as `setUTCHours` | as `setUTCMonth` |
| `d.setUTCSeconds(s, ms?)` | leaf | `date_set {part: "seconds"}` | as `setUTCHours` | as `setUTCMonth` |
| `d.setUTCMilliseconds(ms)` | leaf | `date_set {part: "milliseconds"}` | as `setUTCHours` | as `setUTCMonth` |
| `d.getTime()`, `d.valueOf()`; `d.setTime(t)` | reader | the value itself; `date_part {part: "time"}` | | a date is its time value |
| `getFullYear`, `getHours`, … and every local `set…`; `toString`, `toDateString`, `toTimeString`; `new Date(y, m, …)` | refused | | | §12, the environment: no time zone; the mistake names the UTC form (`Date.UTC(y, m, …)` for the constructor) |
| `toLocaleString`, `toLocaleDateString`, `toLocaleTimeString` | refused | | | §12: no locale, no time zone; `date_text` |
| `d.getTimezoneOffset()` | refused | | | §12: the machine's zone; UTC is the only time |
| `Intl.DateTimeFormat` | refused | | | §12: locale is the environment |
| `Date.now()`, `new Date()` | refused, import (exists) | `now * 1000` | | `import { now } from "holon:clock"`: seconds, fixed for the run |
| `Map` | refused | a record with text keys | | §12: `insert`, `lookup`, `remove`, `has`, `keys`; other keys are a list of pairs and `find` |
| `Set` | refused | a list | | §12: `[...new Set(xs)]` is `unique`, `s.has(x)` is `includes` (lists.md) |

In numbers: 64 rows. 31 are new leaves, 13 distinct (`values`, `entries`, `from_entries`, `merge`,
`has`, `group_by`, `type_of`, `same_value`, `date_part`, `date_utc`, `date_set`, `date_text`,
`date_parse`). 5 are leaf changes (`json_stringify`'s `indent`, `keys` and `fn`, its numbers and
escapes; `json_parse`'s `fn`). 12 are reader rows, 14 refused, 2 differ and stay, and none is a
composite. 4 of the leaves were re-raced after the evaluator change; each is in its candidate's
class now and still above 3, so no verdict moved.

## Leaf specs

Each record leaf is structural: no arithmetic, the same natively and in wasm. Each takes a record
where it says so and gives nothing for anything else, which is where JavaScript would throw a
`TypeError` or convert. The native candidates in the bench are these meanings, written out.

**`values`** (`record`) → list. The record's values, in its key order. JavaScript form:
`Object.values(r)`. On a list or a text, JavaScript gives its items or characters; Holon gives
nothing.

**`entries`** (`record`) → list of `[key, value]`, in key order. `Object.entries(r)`.

**`from_entries`** (`entries`: list) → record. For each entry in order: a list whose item 0 is the
key and item 1 the value, items past 1 ignored, a later key replacing an earlier (as JavaScript's
CreateDataPropertyOrThrow does). The result is nothing in these cases:

- an entry that isn't a list, or has fewer than two items (JavaScript stores `undefined`, which a
  record can't hold);
- a key that isn't a text (JavaScript converts `1` to `"1"`).

`Object.fromEntries(es)`.

**`merge`** (`a`, `b`, …: records; variadic, as `add`) → record. Every field of every input, a later
input's where two have one; shallow (a nested record is replaced, not merged). The JavaScript forms:

- `{ ...a, ...b }`;
- `Object.assign({}, a, b)`;
- `Object.assign(t, s)` as a statement on a local, which becomes `t = merge(t, s)`.

`Object.assign` on a parameter is refused as a change in place ("copy it first").

**`has`** (`record`, `key`: text) → boolean. Whether the record has the key: present with any
value, JSON's `null` (`none()`) included, as `"k" in { k: null }` is `true`. JavaScript forms:
`k in r`, `Object.hasOwn(r, k)`, `r.hasOwnProperty(k)`. Where they differ:

- `in` also finds inherited keys (`"toString" in {}` is `true`), which a record hasn't;
- a number key is converted by JavaScript (`1 in { "1": 0 }`);
- on a list, JavaScript tests an index.

Holon gives `false` for an inherited key and nothing for a number key or a list.

**`group_by {fn}`** (`list`) → record of lists. `fn` is a function of the item, as `map`'s. For each
item in order, its key is the function's answer and the item is appended to that key's list.

- The key must be a text: JavaScript converts a number with ToPropertyKey; Holon gives nothing.
- The groups come in key order where JavaScript's come first-seen; each group keeps the items'
  order.
- JavaScript also passes the index to the callback; Holon doesn't, as `map` doesn't.

JavaScript form: `Object.groupBy(xs, (x) => …)`. `Map.groupBy` is refused with `Map`.

**`type_of`** (`value`) → text. What JavaScript's `typeof` gives:

| Holon value | `type_of` gives |
|---|---|
| number | `"number"` |
| boolean | `"boolean"` |
| text | `"string"` |
| list, record, `none()` | `"object"` (`none()` is JSON's `null`, and `typeof null` is `"object"`) |
| `some(x)` | `type_of(x)` (JSON writes `some(x)` as `x`) |
| an effect | nothing (it has no JavaScript value) |

It never gives `"function"`, `"symbol"` or `"bigint"`: Holon has none of those values. The reader
lowers `typeof x` to `type_of(x) ?? "undefined"`, because an absent `x` reaches no leaf.

**`same_value`** (`a`, `b`) → boolean. ECMA-262's SameValue. Numbers compare by bits, so
`Object.is(-0, 0)` is `false` (NaN can't occur). Lists and records compare by value, numbers in them
by bits, where JavaScript compares identity (§12). `Object.is(a, b)`.

**`json_stringify`**, changed. Today one input and no params; it gains three params, each frozen,
which the reader takes from literals and refuses as computed values, naming the literal form.

- **Numbers** are written as JavaScript's Number::toString (§6.1.6.1.20). Rust's shortest
  round-trip digits are the same; the change is when to use an exponent: when the decimal exponent
  n (as the spec defines it) is over 21 or at most −6. So `1e+21`, `1.5e-7`, `-1e+300`; `-0` stays
  `0`. `as_text`, and so a template literal, has the same difference (numbers.md and texts.md).
- **Escapes** as QuoteJSONString: `\b` and `\f` for U+0008 and U+000C, as `\n`, `\r` and `\t`
  already are. Any other code point below U+0020 stays `\u00xx` in lowercase hex.
- **`indent`** (text, default `""`): the gap, from JavaScript's `space`.
  - A number is that many spaces after ToIntegerOrInfinity, at most 10; below 1, none (`2.9` is
    2, `-3` is none, `20` is 10).
  - A text is its first 10 characters. JavaScript takes 10 UTF-16 code units, which can split an
    astral character; a Holon text can't hold half of one.
  - With a gap, SerializeJSONObject and SerializeJSONArray: each member on its own line, indented
    by one more gap, separated by `",\n"`, with `": "` after a key and the closing bracket at the
    outer indent. An empty list or record stays `[]` or `{}`.
  - JavaScript form: `JSON.stringify(v, null, 2)`, or `undefined` in the middle. The reader
    accepts `null` in that place only.
- **`keys`** (a JSON list of texts, default unset): JavaScript's PropertyList from a replacer list.
  - Every record, at every depth, is written with only the listed keys it has, in the list's
    order; lists are written whole.
  - A key listed twice counts once. An item that isn't a text is skipped: JavaScript skips items
    that aren't strings or numbers and converts a number (`1` is `"1"`), which Holon doesn't.
  - JavaScript form: `JSON.stringify(v, ["id", "name"])`.
- **`fn`** (a function of `(key, value)`, as `fold`'s takes two): the replacer.
  - It is applied top-down, as SerializeJSONProperty does: first to the whole value with key `""`,
    then to each member of what it returned, a list item's key being its index as text.
  - A member it answers nothing for is left out of a record and written `null` in a list; at the
    top, the whole leaf gives nothing (JavaScript returns `undefined`).
  - It is a pure function, so the order of the calls can't be seen.
  - JavaScript form: `JSON.stringify(v, (k, x) => …)`.
- `fn` and `keys` are exclusive, as in JavaScript, where the replacer is one argument.

The layouts are pinned in the bench against what JavaScript writes for the same keys in the same
order.

**`json_parse {fn}`**, changed: the reviver, InternalizeJSONProperty.

- It is applied bottom-up: to each member of a record or list (a list item's key is its index as
  text) before the record or list itself, and last to the whole value with key `""`.
- A record member it answers nothing for is removed. In a list, JavaScript leaves a hole, which
  reads as `undefined` and is written `null`; Holon has no holes and puts `none()` there (JSON's
  `null`).
- JavaScript visits a record's keys in the text's order and Holon in sorted order. The function is
  pure, so the order can't be seen.
- JavaScript form: `JSON.parse(t, (k, x) => …)`. The third argument of newer engines (`context`,
  with the source text) isn't passed.

**The date leaves** are in [Dates](#dates). Each takes and gives a time value, a number of
milliseconds, so `now` reaches them as `now * 1000`. Each is exact integer arithmetic in `i64`,
plus the IEEE additions and multiplications JavaScript's own MakeDay and MakeTime perform, in the
same order. Those are correctly rounded and so the same natively and in wasm; no `libm` function is
used.

| Leaf | Inputs, params | Gives | JavaScript forms |
|---|---|---|---|
| `date_part {part}` | a number | a number | `part` is `time` (TimeClip of the input: `new Date(ms)`, `setTime`); `year`, `month` (0–11), `date`, `day` (0 is Sunday), `hours`, `minutes`, `seconds`, `milliseconds`: `getUTCFullYear()`, … `getUTCMilliseconds()`. The input is TimeClipped first, so `date_part(1.5, { part: "milliseconds" })` is 1; beyond ±8.64e15, nothing (JavaScript's NaN) |
| `date_utc` | 1 to 7 numbers (variadic) | a time value | `Date.UTC(y, m, d, h, mi, s, ms)`: missing parts are month 0, date 1, the rest 0; a year from 0 to 99 after truncation is 1900 plus it (`Date.UTC(99, 0)` is 1999); then MakeDay, MakeTime, MakeDate, TimeClip. With no input, nothing (NaN) |
| `date_set {part}` | a time value, then 1 to 4 numbers | a time value | `setUTCFullYear(y, m?, d?)` (`part: "year"`), `setUTCMonth(m, d?)`, `setUTCDate(d)`, `setUTCHours(h, m?, s?, ms?)`, `setUTCMinutes(m, s?, ms?)`, `setUTCSeconds(s, ms?)`, `setUTCMilliseconds(ms)`: the parts given, starting at `part`, replace the date's; the rest are the date's; then MakeDay, MakeTime, MakeDate, TimeClip. No year mapping (`setUTCFullYear(99)` is year 99). The reader makes `d.setUTCHours(h)` on a local `d = date_set(d, h)` and gives the new value where the call is an expression, as JavaScript returns it |
| `date_text {format}` | a time value | a text | `format: "iso"`: `toISOString()`, `toJSON()`; `format: "utc"`: `toUTCString()`, `toGMTString()` |
| `date_parse` | a text | a time value | `Date.parse(t)`, `new Date(t)` |

## Composites

Each is as raced, compiled by the reader, and checked against its candidate first. The record
composites:

```ts
function values_of(r) {
  return Object.keys(r).map((k) => r[k])
}

function entries_of(r) {
  return Object.keys(r).map((k) => [k, r[k]])
}

import { insert, get } from "holon"
function from_entries(es) {
  return es.reduce((acc, e) => insert(acc, get(e, 0), get(e, 1)), {})
}

import { insert } from "holon"
function merge(a, b) {
  return Object.keys(b).reduce((acc, k) => insert(acc, k, b[k]), a)
}

import { is_truthy } from "holon"
function has(r, k) {
  return is_truthy([r[k]]) ?? false
}

import { insert } from "holon"
function add_to_group(acc, x) {
  const k = x.g
  return insert(acc, k, [...(acc[k] ?? []), x])
}
function group_by(xs) {
  return xs.reduce(add_to_group, {})
}

import { to_text, slice } from "holon"
function type_of(x) {
  const t = to_text(x)
  const c = slice(t, 0, 1)
  return c === "\"" ? "string" : c === "[" || c === "{" || c === "s" ? "object" : t === "true" || t === "false" ? "boolean" : "number"
}
```

The cases each was checked on:

| Composite | Cases |
|---|---|
| `values_of`, `entries_of` | an empty record; one key; a nested record; keys whose insertion order isn't their sorted order (`"10"` before `"2"`, `"😀"` before `""`); a `-0`, an empty list and `none()` as values |
| `from_entries` | an empty list; one entry; four keys inserted out of order; a key twice (the last wins); an entry of three items; an entry of one item (nothing); a number key (nothing); a `-0` value |
| `merge` | both empty; either empty; an overlapping key (`b`'s wins); nested records (replaced, not merged); each edge record on either side |
| `has` | each edge record with present, missing, empty, astral and `none()`-valued keys |
| `group_by` | an empty list; one item; three groups out of order; an item without the key, and one with a number key (nothing, both) |
| `type_of` | `0`, `-0`, `-1.5`, `1e300`, `""`, `"x"`, `"\"q"`, `"true"`, `"[1]"`, `true`, `false`, `[]`, a nested list, `{}`, a nested record, `none()` |

`type_of` is inexact for an effect (it says `"number"`) and for `some(x)` (it says `"object"`).
The check leaves both out; the candidate gives nothing and `type_of(x)` for them.

`has` is in a `map` in a second race: `ks.map((k) => has(r, k))` against `ks.map((k) => r[k])`,
where a `lookup` stands in for a `has` leaf (the same search, then a clone of a number for a
boolean).

The date composites need `floor` and `trunc`. For the checks, both are made exact of today's
leaves: `as_text` writes every finite number without an exponent, so the text before its point is
its truncation.

```ts
import { as_text, split, head, json_parse } from "holon"
function trunc(x) {
  const t = json_parse(head(split(as_text(x), ".")))
  return t === 0 ? x * 0 : t
}
function floor(x) {
  const t = trunc(x)
  return x < t ? t - 1 : t
}
```

(`x * 0` keeps the sign of `-0` and of the fractions between `-1` and `0`. This trunc is exact for
every finite number, so `Math.trunc` and `Math.floor` are expressible today, which matters to
numbers.md; at six leaves around a text conversion, it won't race within 3× of a leaf.)

For the races, each `floor(x)` and `trunc(x)` is stood in by `((x) ?? 0)`: one `default` node,
which costs what a `floor` leaf would. A composite over the date parts has each part stood in the
same way, and `utc_time(…)` by one variadic `add` of its seven arguments. A stood-in composite's
values are wrong, so it is timed, never checked.

```ts
function time_clip(ms) {
  return ms >= -8640000000000000 && ms <= 8640000000000000 ? trunc(ms) + 0 : undefined
}

function utc_ms(t) {
  return t - floor(t / 1000) * 1000
}
function utc_seconds(t) {
  const s = floor(t / 1000)
  return s - floor(s / 60) * 60
}
function utc_minutes(t) {
  const m = floor(t / 60000)
  return m - floor(m / 60) * 60
}
function utc_hours(t) {
  const h = floor(t / 3600000)
  return h - floor(h / 24) * 24
}
function utc_weekday(t) {
  const d = floor(t / 86400000) + 4
  return d - floor(d / 7) * 7
}

// utc_year, utc_month and utc_date share this body (civil_from_days), each with its own answer:
//   utc_year:  mp < 10 ? yoe + era * 400 : yoe + era * 400 + 1
//   utc_month: mp < 10 ? mp + 2 : mp - 10
//   utc_date:  doy - floor((153 * mp + 2) / 5) + 1
function utc_year(t) {
  const z = floor(t / 86400000) + 719468
  const era = floor(z / 146097)
  const doe = z - era * 146097
  const yoe = floor((doe - floor(doe / 1460) + floor(doe / 36524) - floor(doe / 146096)) / 365)
  const doy = doe - (365 * yoe + floor(yoe / 4) - floor(yoe / 100))
  const mp = floor((5 * doy + 2) / 153)
  return mp < 10 ? yoe + era * 400 : yoe + era * 400 + 1
}

function utc_time(year, month, date, h, mi, s, ms) {
  const y = trunc(year)
  const mo = trunc(month)
  const ym = y + floor(mo / 12)
  const mn = mo - floor(mo / 12) * 12
  const yy = mn < 2 ? ym - 1 : ym
  const era = floor(yy / 400)
  const yoe = yy - era * 400
  const doy = floor((153 * (mn < 2 ? mn + 10 : mn - 2) + 2) / 5)
  const doe = yoe * 365 + floor(yoe / 4) - floor(yoe / 100) + doy
  const day = era * 146097 + doe - 719468 + trunc(date) - 1
  const time = trunc(h) * 3600000 + trunc(mi) * 60000 + trunc(s) * 1000 + trunc(ms)
  const tv = day * 86400000 + time
  return y >= -1000000 && y <= 1000000 && mo >= -10000000 && mo <= 10000000 && tv >= -8640000000000000 && tv <= 8640000000000000 ? tv + 0 : undefined
}

function date_utc(y, mo, d, h, mi, s, ms) {
  return utc_time(y > -1 && y < 100 ? 1900 + trunc(y) : y, mo, d, h, mi, s, ms)
}

// One of seven, each replacing its own part:
function set_utc_hours(t, x) {
  return utc_time(utc_year(t), utc_month(t), utc_date(t), x, utc_minutes(t), utc_seconds(t), utc_ms(t))
}

function pad2(n) {
  return n < 10 ? `0${n}` : `${n}`
}
function pad3(n) {
  return n < 10 ? `00${n}` : n < 100 ? `0${n}` : `${n}`
}
function pad6(n) {
  return n < 10 ? `00000${n}` : n < 100 ? `0000${n}` : n < 1000 ? `000${n}` : n < 10000 ? `00${n}` : n < 100000 ? `0${n}` : `${n}`
}
function year_text(y) {
  return y < 0 ? `-${pad6(0 - y)}` : y > 9999 ? `+${pad6(y)}` : y < 10 ? `000${y}` : y < 100 ? `00${y}` : y < 1000 ? `0${y}` : `${y}`
}
function iso(t) {
  return `${year_text(utc_year(t))}-${pad2(utc_month(t) + 1)}-${pad2(utc_date(t))}T${pad2(utc_hours(t))}:${pad2(utc_minutes(t))}:${pad2(utc_seconds(t))}.${pad3(utc_ms(t))}Z`
}
```

The date checks use these cases:

- **The getters and `toISOString`:** 25 time values: 0, ±1, the edges of a second and a day,
  2000-02-29 and its last millisecond, 2024-02-29T23:59:59.999, 2100-01-01, years 1, 0 and −1,
  9999-12-31T23:59:59.999, +010000, ±8.64e15 and one inside each.
- **`time_clip`:** `±0`, `±0.5`, `±1.5`, ±8.64e15, ±(8.64e15 + 2), `±1e300`, and a fraction.
- **`utc_time` and `date_utc`:** 26 argument lists:
  - month and day overflow both ways, and an hour, minute, second and millisecond overflow;
  - each end of TimeClip, both just in and just out;
  - fractions in every part;
  - the years −0.5, 99.9, 100 and −1 for the year mapping;
  - a day offset of 100,000,001 (just in) and 100,000,002 (out);
  - a millisecond of 1e20, and one of 2⁵² + 1;
  - V8's year bound, ±1e6;
  - a year of 1,000,001 brought back into range by a month of −12,000,000 (nothing: the bound is
    on the input year, as in V8).
- **Each `setUTC…`:** 72 pairs: six dates, including both ends of the range, and twelve values,
  among them 13, 25, 61, 1001, 2.7, −0.5, 1e6 and 1e20.

Time values are never `-0` (TimeClip makes it `0`), so none is checked.

## Dates

A date is its time value, as in JavaScript: a whole number of milliseconds since
1970-01-01T00:00:00Z, within ±8.64e15 (ECMA-262 §21.4.1.1). |t| ≤ 8.64e15 < 2⁵³, so a time value
is exact as an `f64` and as an `i64`, and every leaf computes in `i64`:

- **Day and time of day:** `day = t.div_euclid(86_400_000)`, `ms = t.rem_euclid(86_400_000)`. From
  `ms` come the hours (`ms / 3_600_000`), minutes (`ms / 60_000 % 60`), seconds (`ms / 1000 % 60`)
  and milliseconds (`ms % 1000`); the weekday is `(day + 4).rem_euclid(7)` (1970-01-01 was a
  Thursday).
- **civil_from_days** (H. Hinnant):
  1. `z = day + 719_468`, the days since 0000-03-01.
  2. `era = z.div_euclid(146_097)` and `doe = z.rem_euclid(146_097)`: a 400-year era has
     146,097 days.
  3. `yoe = (doe − doe/1460 + doe/36524 − doe/146096) / 365`, the year within the era.
  4. `doy = doe − (365·yoe + yoe/4 − yoe/100)`, the day within a March-based year.
  5. `mp = (5·doy + 2) / 153`.
  6. The date is `doy − (153·mp + 2)/5 + 1`, the month `mp + 3` or `mp − 9`, and the year is one
     more when the month is January or February.

  Every quotient is of non-negative `i64`s, and for |t| ≤ 8.64e15 every value is below 2³¹. The
  proleptic Gregorian calendar is exact over the whole range: year −271,821 to +275,760.
- **days_from_civil** is its inverse. MakeDay (§21.4.1.28) uses it:
  1. truncate the year and month;
  2. refuse a year outside ±1,000,000 or a month outside ±10,000,000 (V8's bounds; the standard's
     "if this is not possible" leaves the bound to the engine);
  3. carry the month into the year (`div_euclid`, `rem_euclid` by 12);
  4. take the day count of the month's first day, then `+ date − 1` in IEEE arithmetic, as the
     standard writes `Day(t) + dt − 1𝔽`.
- **MakeTime** (§21.4.1.27) is `((h·3600000 + m·60000) + s·1000) + ms`, each part truncated first,
  in IEEE arithmetic and in that order. For huge parts the order decides the last bit, so it is
  kept.
- **MakeDate** is `day·86400000 + time`. TimeClip gives nothing beyond ±8.64e15 (or if not
  finite), else the number with `-0` made `0`.
- **The texts:**
  - `iso` is `YYYY-MM-DDTHH:mm:ss.sssZ`. A year outside 0–9999 is written as a sign and six digits
    (`-000001`, `+010000`, `+275760`).
  - `utc` is `Www, DD Mmm YYYY HH:mm:ss GMT`. A negative year is written `-` and at least four
    digits (DateString's yearSign and padded year).
- **`date_parse`** reads the Date Time String Format (§21.4.1.32) exactly:
  - The date is `YYYY`, `YYYY-MM` or `YYYY-MM-DD`. An expanded year is `±YYYYYY` and never
    `-000000`.
  - A month is 01–12, and a day lies within its month (Feb 30 is nothing).
  - Then, optionally, `THH:mm`, `THH:mm:ss` or `THH:mm:ss.sss` (exactly three digits). `24:00`,
    with zeros after it, is the end of the day.
  - Then `Z` or `±HH:mm`.
  - A date alone is UTC. A date and time without an offset is local time in JavaScript, and Holon
    has no local time, so it gives nothing. So does anything outside the format, where engines
    fall back to heuristics of their own (V8 reads `2019-02-30` as March 2 and `.5` as 500 ms;
    the standard doesn't), and anything beyond ±8.64e15.

**Pinned known answers.** The bench asserts what JavaScript gives:

- `toISOString` and `Date.parse` both ways for 1970, −1 ms, 2000-02-29, years 1, 0 and −1,
  9999-12-31T23:59:59.999, +010000, and both ends of the range;
- `toUTCString` of 0 and of 2000-02-29;
- `getUTCDay(0)` = 4;
- seven `Date.UTC` calls, among them the year mapping, overflow, and the range's end;
- sixteen `Date.parse` texts, valid and not.

The oracle a leaf's test runs against, javascript-library.md §0's, is Boa inside `js()`
(`fixtures/modules/js-engine`, `boa_engine` 0.22). These answers are the standard's; a leaf's
test must also run them there.

**Why a composite doesn't close a getter.** The composites are exact over `floor`: `floor(t / d)`
for a whole t with |t| ≤ 8.64e15 and each divisor d used gives the true quotient.

- t / d can be within 1/d of an integer and still not be one. It rounds to the integer only if
  1/d < ULP(t / d) / 2.
- The worst case is d = 3,600,000. There t / d < 2.4e9 < 2³², so ULP / 2 ≤ 2⁻²² ≈ 2.4e-7, while
  1/d ≈ 2.8e-7.
- The same holds for 1000, 60,000 and 86,400,000.

So the getters are expressible once `floor` is a leaf (numbers.md). They lose on cost:

- even the smallest, `getUTCHours` at 9 leaves with `floor` a leaf, is 25× a `date_part`;
- the civil date takes 52 to 55 leaves and is over 100×;
- MakeDay over `floor` (`utc_time`) takes 89 leaves and is 168×.

## Differences from JavaScript that stay

- **Key order:** sorted by code point, not insertion with integer keys first
  ([Key order](#key-order)). `Object.keys`, `values`, `entries`, `JSON.stringify`, `groupBy` and
  `JSON.parse` followed by `stringify` all show it.
- **Records and lists compare by value**, in `===`, `==` and `Object.is`; JavaScript compares
  objects by identity. `==` is `===`: no coercion, so `1 == "1"` is `false`.
- **No prototype.** `in` sees own keys only (`"toString" in {}` is `false`). `hasOwnProperty`
  isn't inherited: it is the reader's form for `has`.
- **Keys are texts.** JavaScript converts a number key (`{ [1]: x }`, `1 in r`, a `fromEntries` or
  `groupBy` key, a number in a replacer list); Holon gives nothing, and `${k}` writes the key.
  - The reader could wrap a number key in `as_text`. That matches JavaScript for 1e-6 ≤ |k| < 1e21
    until `as_text` takes JavaScript's exponent form (numbers.md, texts.md).
- **Lists and texts aren't records:** `Object.keys`, `values`, `entries` and `in` on a list or text
  give nothing, where JavaScript gives indices.
- **Freezing is checked when the flow compiles.** A change to a local after `Object.freeze`,
  `seal` or `preventExtensions` is refused when the flow compiles, where JavaScript (module code,
  always strict) throws when it runs. `seal` and `preventExtensions` refuse a change to an
  existing key as well, since the reader can't tell an added key from a changed one.
- **JSON:**
  - a key twice is refused (`json.rs`, by design), where JavaScript keeps the last;
  - a reviver dropping a list item leaves `none()`, where JavaScript leaves a hole;
  - an indent text is cut at 10 characters, not 10 UTF-16 code units;
  - no `toJSON` is called, there being no methods.
- **Dates:**
  - no local time or time zone, so no local getters and setters, `toString`, `toDateString`,
    `toTimeString`, `toLocale…`, `getTimezoneOffset` or `Intl`;
  - `Date.parse` gives nothing for a date-time without an offset (local in JavaScript) and for
    anything outside the standard format (engine heuristics);
  - an invalid date is nothing, not a NaN date, so `setUTCFullYear` on an invalid date, which
    starts from `+0` in JavaScript, doesn't arise;
  - MakeDay's year bound is V8's ±1,000,000.
- **`now` is seconds, fixed for the run.** `Date.now()` is `now * 1000` and doesn't move during the
  run.
- **`typeof` is never `"undefined"` on a value** (the reader adds `?? "undefined"` for an absent
  one), and never `"function"`, `"symbol"` or `"bigint"`.
- **`groupBy` passes no index** to its function, as `map` doesn't.

## Reader changes

Each is in `crates/holon-author/src/lang/ts.rs`, with a test of its JavaScript meaning. What it
said before ("today" below) was checked by compiling each form. All are built (2026-11-07), and
the printer writes each back in its JavaScript form (`print.rs`; the print and `fmt` round trips
hold). Where the build differs from the plan below:

- `new Date(text)` is `date_part {part: "time"}` of the text, not `date_parse`: every date leaf
  takes a text as `new Date(text)` does, so `new Date(t).getUTCHours()` is one node whichever `t`
  is. `new Date(ms)` is always a `date_part` node; no node is left out for a known whole time
  value.
- A date's methods are read on `new Date(x)` itself and on a local bound from `new Date`. A local
  from `Date.UTC` or `Date.parse` is a number in JavaScript, not a Date, so its date methods are
  refused, naming `new Date(x).getUTCHours()`, as on any other value; `: Date` annotations aren't
  read. A Date bound to a name is read only by its methods: returned, compared or put in a record,
  it is refused, naming `d.getTime()` and `d.toISOString()`, since JavaScript's would be an
  object (`JSON.stringify` writes it as its ISO text).
- A setter on a Date bound to a name, as a value, is the time value it gives; in `const t =
  d.setUTCHours(h)` the local becomes the new date too, and elsewhere in an expression the local is
  read no more.
- The printer writes a date leaf as a method of `new Date(t)`, `new Date(t).getTime()` for the
  time value, and `entries` as `Object.entries(r)` where `r` is known to be a record, else as a
  list's `[...xs.entries()]`.

**`method_call`'s `(None, "Object")` arm** today answers "Object has keys, not "…": do the rest in
a js() function".

- **Read these:**
  - `values`, `entries`, `fromEntries` and `groupBy` (its function through `function_arg`, one
    parameter);
  - `assign` (`merge`; as a statement on a local, `t = merge(t, …)`);
  - `hasOwn` (`has`), `is` (`same_value`) and `getOwnPropertyNames` (`keys`);
  - `create(null)` (`record`);
  - `freeze`, `seal` and `preventExtensions`: no node, the local marked frozen. A later change in
    place to it is refused: "… is frozen: give the new value a name of its own".
- **Refuse these, naming Holon's form:**
  - `isFrozen`, `isSealed`, `isExtensible`: "a value never changes in place: every value is
    frozen";
  - `defineProperty`, `defineProperties`: "a record holds values: `{ ...r, [k]: v }`";
  - `create(proto)`, `getPrototypeOf`, `setPrototypeOf`: "a flow has no prototypes".

**Methods of a value:** `r.hasOwnProperty(k)` is `has` (today "no leaf does .hasOwnProperty(…)").

**Operators:**

- `k in r` is `has(r, k)` (today "in isn't an operator Holon has").
- `typeof x` is `type_of(x) ?? "undefined"` (today "typeof isn't an operator Holon has").
- `instanceof` stays refused, naming `Array.isArray` and `typeof`.
- `delete` needs no change: it is read on a local, refused on a parameter, and refused in an
  expression.

**Record literals** (`record`, today "a record spreads one base, first" and "a computed key comes
in a later slice"): read left to right.

- A spread after the first is `merge(acc, s)`.
- A named key is `with_field`.
- A computed key `[k]` is `insert(acc, k, v)`.

So `{ ...a, x: 1, ...b, [k]: v }` is `insert(merge(with_field(a, x), b), k, v)`.

**`structuredClone(x)`** is `x` itself, **not marked shared**, as `{ ...x }` and `[...x]` aren't:
today `const c = a; c.x = 1` is refused as shared, and `structuredClone` exists to allow exactly
that.

**`JSON.stringify(v, a, b)`:**

- Accept `null` or `undefined` as `a` (today `null` is refused anywhere).
- A list literal of texts as `a` is the `keys` param, and a function as `a` is `fn` (through
  `function_arg`, two parameters).
- A number or text literal as `b` is `indent`.
- A computed `a` or `b` is refused, naming the literal form.

**`JSON.parse(t, f)`:** `f` through `function_arg` (two parameters) as `json_parse`'s `fn`; today
"a function written in place goes to map or reduce".

**Dates:** `Date` is in `GLOBALS` (today "Date isn't there") and `new` is refused ("new isn't
something Holon computes").

- Read `Date.UTC(…)` (`date_utc`) and `Date.parse(t)` (`date_parse`).
- Read `new Date(ms)` (`date_part {part: "time"}`, or no node when the reader knows `ms` is a whole
  time value, as `now * 1000` is) and `new Date(text)` (`date_parse`).
- Refuse `new Date(y, m, …)` ("parts are local time: Date.UTC(y, m, …)") and `new Date()` /
  `Date.now()` ("import { now } from "holon:clock"; now * 1000").
- A date's methods need the reader to know the value is a date. That holds for a local bound from
  `new Date`, `Date.UTC`, `Date.parse` or a `date_*` leaf, or annotated `: Date` (L4 §7). There:
  - `getUTC…` reads as `date_part`, `setUTC…` as `date_set` (and on a local, the local becomes the
    new value), `toISOString`, `toJSON` and `toUTCString` as `date_text`, and `getTime` and
    `valueOf` as the value itself;
  - the local-time methods are refused, each naming its UTC form ("getHours is local time, which a
    flow hasn't: getUTCHours").

  Elsewhere a date method is refused, naming the leaf by name (`date_part(t, { part: "hours" })`).

**`Map`, `Set`** (`new Map()`, `new Set()`): refused, naming the record and list forms.

## Race output

The run behind every number above, verbatim; `Object.fromEntries`, `{ ...a, ...b }` and
`Object.groupBy` are the re-race's (2026-11-07, `02032df`):

### Object.values

| n | composite values_of (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.64 µs | 95 ns | 27.67 |
| 64 | 20.49 µs | 336 ns | 60.95 |
| 512 | 92.71 µs | 2.17 µs | 42.71 |

### Object.entries

| n | composite entries_of (8 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.53 µs | 522 ns | 8.68 |
| 64 | 23.75 µs | 4.05 µs | 5.87 |
| 512 | 204.08 µs | 33.42 µs | 6.11 |

### Object.fromEntries

| n | composite from_entries (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.84 µs | 451 ns | 15.16 |
| 64 | 36.84 µs | 4.72 µs | 7.80 |
| 512 | 286.57 µs | 59.73 µs | 4.80 |

### { ...a, ...b }

| n | composite merge (8 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.41 µs | 852 ns | 7.53 |
| 64 | 36.62 µs | 6.76 µs | 5.42 |
| 512 | 323.13 µs | 74.68 µs | 4.33 |

### Object.hasOwn

| n | composite has (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 896 ns | 48 ns | 18.79 |
| 64 | 923 ns | 49 ns | 18.88 |
| 512 | 987 ns | 70 ns | 14.11 |

### Object.hasOwn, mapped

| n | composite ks.map((k) => has(r, k)) (6 leaves) | composite ks.map((k) => r[k]), for a has leaf (2 leaves) | ratio |
|---:|---:|---:|---:|
| 8 | 8.73 µs | 1.82 µs | 4.79 |
| 64 | 61.28 µs | 5.87 µs | 10.44 |
| 512 | 478.88 µs | 49.10 µs | 9.75 |

### Object.groupBy

| n | composite group_by (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 14.81 µs | 1.60 µs | 9.23 |
| 64 | 95.14 µs | 12.93 µs | 7.36 |
| 512 | 888.80 µs | 173.97 µs | 5.11 |

### typeof, a number

| n | composite type_of (27 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 5.50 µs | 149 ns | 36.98 |

### typeof, a number, mapped

| n | composite xs.map(type_of) (28 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 331.67 µs | 7.91 µs | 41.91 |

### typeof, a record

| n | composite type_of (27 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 6.56 µs | 78 ns | 84.04 |
| 64 | 9.72 µs | 63 ns | 153.75 |
| 512 | 112.71 µs | 73 ns | 1552.32 |

### new Date(ms)

| n | composite time_clip (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.29 µs | 39 ns | 33.24 |

### new Date(ms), mapped

| n | composite xs.map(time_clip) (12 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 83.36 µs | 2.50 µs | 33.30 |

### getUTCMilliseconds

| n | composite utc_ms (8 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.69 µs | 59 ns | 28.38 |

### getUTCMilliseconds, mapped

| n | composite xs.map(utc_ms) (9 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 81.29 µs | 2.68 µs | 30.29 |

### getUTCSeconds

| n | composite utc_seconds (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.11 µs | 45 ns | 24.78 |

### getUTCSeconds, mapped

| n | composite xs.map(utc_seconds) (10 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 73.84 µs | 2.57 µs | 28.75 |

### getUTCMinutes

| n | composite utc_minutes (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.12 µs | 46 ns | 24.62 |

### getUTCMinutes, mapped

| n | composite xs.map(utc_minutes) (10 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 75.04 µs | 2.69 µs | 27.89 |

### getUTCHours

| n | composite utc_hours (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.12 µs | 45 ns | 24.62 |

### getUTCHours, mapped

| n | composite xs.map(utc_hours) (10 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 75.43 µs | 2.58 µs | 29.27 |

### getUTCDay

| n | composite utc_weekday (11 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.30 µs | 45 ns | 28.84 |

### getUTCDay, mapped

| n | composite xs.map(utc_weekday) (12 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 83.58 µs | 2.59 µs | 32.30 |

### getUTCFullYear

| n | composite utc_year (55 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 5.32 µs | 45 ns | 117.45 |

### getUTCFullYear, mapped

| n | composite xs.map(utc_year) (56 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 307.96 µs | 2.52 µs | 122.10 |

### getUTCMonth

| n | composite utc_month (52 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 4.64 µs | 45 ns | 102.65 |

### getUTCMonth, mapped

| n | composite xs.map(utc_month) (53 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 297.59 µs | 2.63 µs | 113.25 |

### getUTCDate

| n | composite utc_date (54 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 4.68 µs | 46 ns | 102.62 |

### getUTCDate, mapped

| n | composite xs.map(utc_date) (55 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 308.17 µs | 2.56 µs | 120.17 |

### utc_time

| n | composite utc_time (89 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 7.80 µs | 46 ns | 168.11 |

### Date.UTC

| n | composite date_utc (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.42 µs | 57 ns | 24.75 |

### setUTCFullYear

| n | composite set_utc_full_year (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.39 µs | 94 ns | 14.81 |

### setUTCHours

| n | composite set_utc_hours (9 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 1.39 µs | 95 ns | 14.69 |

### setUTCHours, mapped

| n | composite xs.map(set_hours_7) (11 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 132.75 µs | 5.93 µs | 22.40 |

### toISOString

| n | composite iso (209 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 1 | 18.53 µs | 321 ns | 57.66 |

### toISOString, mapped

| n | composite xs.map(iso) (210 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 64 | 1.23 ms | 18.79 µs | 65.23 |
