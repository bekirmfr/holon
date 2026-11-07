# Texts: how each gap closes

Measured 2026-11-06 on `c6d350e` (races: a harness that races a TypeScript composite against a
native candidate through eval), by `crates/holon-author/benches/races_texts.rs`. Times are the
harness's medians of seven samples. Other agents' builds were running on the same machine, so
between runs an absolute time moved by up to 2× and a ratio by up to a third (both sides of a race
are timed in the same window). No verdict changed across the four full runs made today; the tables
below come from the last one.

The leaves were built on 2026-11-07, counting code units: core's `texts.rs` holds the native
candidates, ported, and its tests assert the oracle's 1,525 cases. Since then the bench checks and
races each composite through those leaves. The tables below were measured before, through leaves
that counted scalars, except the race of `type_of(s) === "string"` for `isWellFormed`, run that
day. Where a leaf came out other than specified here, [Leaf specs](#leaf-specs) says so.

The bench gives three kinds of evidence, in order:

1. **The oracle.** Each native candidate counts UTF-16 code units and gives what JavaScript gives
   on 1,525 cases that Node 22.21.1 computed (`races_texts_oracle.json`, next to the bench: one
   entry per method, each case its inputs and JavaScript's answer, `null` where JavaScript's
   answer has no Holon value). It covers `at`, `charAt`, `charCodeAt`, `codePointAt`, `length`,
   `slice`, `substring`, `indexOf`, `lastIndexOf`, `includes`, `startsWith`, `endsWith`, the three
   trims, `padStart`, `padEnd`, `repeat`, `replace`, `replaceAll`, `split`,
   `String.fromCharCode`, `String.fromCodePoint`, `<` and the other comparisons, the four URI
   functions, `btoa`, `atob`, `String(x)` and `+`.
2. **The checks.** Each composite, compiled by the reader, gives what its candidate gives,
   through the leaves as built, in code units. Before the leaves counted code units, a second run
   simulated them (see [Composites](#composites)).
3. **The races.** Each composite against its candidate, through the real `eval`. The tables
   below counted scalars on both sides, as the leaves then did.

## Units

JavaScript counts a text in UTF-16 code units. Holon's leaves counted Unicode scalar values
(`text_length`, `slice` and `index_of`) until the text leaves were built, and count code units
since, as recommended below. The two agree on every text without an
astral character (one above U+FFFF, such as an emoji) and disagree from the first one on. For
`s = "a😀b"`:

| JavaScript | Code units (JavaScript's, the leaves') | Scalar values (the leaves before) |
|---|---|---|
| `s.length` | 4 | 3 |
| `s.slice(1, 2)` | `"\uD83D"`, a lone surrogate | `"😀"` |
| `s.indexOf("b")` | 3 | 2 |
| `s.charCodeAt(1)` | 55357 (U+D83D, the pair's first half) | no meaning: the character at 1 is two code units |
| `s.codePointAt(1)` | 128512 | 128512 (the scalar at 1, by coincidence) |
| `s.codePointAt(2)` | 56832 (U+DE00, the pair's second half) | 98 (`"b"`) |

**What Holon can't represent.** A Holon text is UTF-8 and can't hold a lone surrogate, so any
JavaScript result that is a text holding one has no Holon value. That covers `slice`, `substring`,
`charAt` and `at` cutting a pair, `split("")` on astral text, `padStart` and `padEnd` cutting an
astral fill, `replaceAll("", x)` on astral text with a non-empty `x`, `String.fromCharCode(0xD83D)`,
and a regular expression without `u` matching half a pair. Each gives nothing. Numbers are fine:
`charCodeAt(1)` is 55357 and `codePointAt(2)` is 56832.

**What switching costs.** Counting code units is the same pass as counting scalars, plus a count
of the bytes that begin an astral character. The races put each count on a text with an astral
and a two-byte character in every 29 characters:

| Leaf | 8 | 64 | 4096 |
|---|---:|---:|---:|
| `text_length`, code units / scalars | 1.17 | 1.09 | 1.67 |
| `slice(1, -1)`, code units / scalars | 1.10 | 1.38 | 1.66 |
| `indexOf(t)`, `t` absent, code units / scalars | 1.03 | 1.01 | 1.02 |

The meaning of `text_length`, `slice`, `index_of` and `split` changes on astral text. A flow that
slices such text answers differently and its hash changes; in the dev phase no old meaning is kept.

**Recommendation: code units** (the owner's decision, and built). Every text leaf counts UTF-16
code units, as JavaScript does, and
a result that would hold a lone surrogate gives nothing. Then every position JavaScript code
computes agrees with JavaScript on every text, astral included: an `indexOf` fed to `slice`,
`length` arithmetic, a regular expression's `index`. Under scalars they agree only until the first
astral character, and they disagree silently. `charCodeAt` and `codePointAt` also need code units
to mean anything: under scalars they are wrong at every index past an astral character.

The rows this changes: `length`, `at`, `charAt`, `charCodeAt`, `codePointAt`, `slice`,
`substring`, `indexOf`, `lastIndexOf`, the positions of `includes`, `startsWith` and `endsWith`,
`padStart` and `padEnd` (where the fill is cut), `split("")`, `replaceAll` with `""`,
`String.fromCharCode`, and the indexes of `search`, `exec` and `matchAll`. Each composite below
gives the same answers in code units as in scalars (it inherits its leaves' unit), except
`encodeURIComponent`, which cuts pairs in code units.

## Verdicts

The rule is [README.md](README.md)'s. "Measured" gives the ratio composite / candidate at each size
(`n` characters unless the row says otherwise), from the race output below. "Mapped" is the
composite prepared once and applied to 16 texts, `xs.map(f)`: it leaves out the floor a composite
call pays on its own (about 470 ns today: `concat(s, "")` alone in a composite takes 700 ns, where
a leaf behind `eval` takes 50 to 80 ns), so it shows what each application costs. Every verdict
counts code units ([Units](#units)).

| JavaScript | Verdict | How (leaf, or composite name) | Measured (ratio composite / candidate, by size) | Why |
|---|---|---|---|---|
| `s.length` | reader | `text_length` (code units) | | rule 1: the leaf exists; read where the value is known to be a text |
| `s.at(i)` | leaf | `at`, the leaf lists and texts share | `js_at`: 25.8 / 23.0 / 2.2 (8/64/4096); mapped 11.0 / 10.9 / 13.9 | rule 3: same class, 23 at 64 (mapped 11) is above 3 |
| `s.charAt(i)` | leaf | `text_at` | `js_char_at`: 20.2 / 18.3 / 1.7; mapped 7.1 / 7.5 / 7.2 | rule 3: same class, 18 at 64 (mapped 7.5) is above 3 |
| `s.charCodeAt(i)` | leaf | `code_at {mode: "unit"}` | | rule 2: no leaf turns a character into a number (`hex` gives UTF-8 bytes as text) |
| `s.codePointAt(i)` | leaf | `code_at {mode: "point"}` | | rule 2: the same |
| `s.concat(a, …)` | reader | `concat` of `as_text` of each argument | | rule 1: as template literals lower; numbers need the `as_text` change |
| `s.endsWith(t, e)` | leaf | `text_contains {at: "end"}` | `js_ends_with`: 39.9 / 41.2 / 50.6 / 190.2 (8/64/4096/65536) | rule 3: worse class (two `text_length` walks where JavaScript compares `t`'s units) |
| `s.includes(t, p)` | leaf | `includes`, shared with lists, with a `from` input | `js_includes`: 16.2 / 12.9 / 1.7; mapped 6.2 / 5.1 / 1.3 | rule 3: same class, 12.9 at 64 (mapped 5.1) is above 3 |
| `s.indexOf(t)` | reader | `index_of` (exists, code units) | | rule 1: mapped today |
| `s.indexOf(t, p)` | leaf change | `index_of` gains a `from` input | `js_index_from`: 54.2 / 36.2 / 3.9 | rule 3: same class, 36 at 64 is above 3 |
| `s.isWellFormed()` | leaf | `is_well_formed` | `js_is_well_formed`: 21.9 / 21.9 / 22.2, mapped 7.2 / 7.3 / 7.2; `s === s` (leaf `equals`): 1.21 / 1.24 / 3.25; `type_of(s) === "string" ? true : undefined` (5 leaves, 2026-11-07): 13.6 / 20.2 / 13.7, mapped 10.2 / 9.5 / 9.5 | rule 3: both O(1) composites are above 3; `equals(s, s)` is a worse class and true for a number, and `type_of(s) === "string"` is false for a number where JavaScript throws |
| `s.lastIndexOf(t, p)` | leaf | `last_index_of`, shared with lists, with a `from` input | `js_last_index_of`: 86.2 / 246.2 / 604.3 (8/64/512) | rule 3: worse class (a `fold` over every position, a `slice` each) |
| `s.localeCompare(t)` | refused | `a < b ? -1 : a > b ? 1 : 0` | | rule 4: a locale reads the machine (javascript-library.md §12) |
| `s.match(re)` | leaf | `regex_exec`; with `g`, `regex_matches {shape: "texts"}` | | rule 2: no pattern matching among the leaves |
| `s.matchAll(re)` | leaf | `regex_matches {shape: "records"}` | | rule 2 |
| `s.normalize(form)` | leaf | `normalize {form}` | | rule 2: no leaf decomposes a character |
| `s.padEnd(len, fill)` | leaf | `pad {side: "end"}` | as `padStart` | rule 3: as `padStart` |
| `s.padStart(len, fill)` | leaf | `pad {side: "start"}` | `js_pad_start`: 29.4 / 36.0 / 207.4; `js_pad_start_proxy` (`repeat` as a leaf): 15.8 / 13.1 / 4.3 | rule 3: worse class over today's leaves; above 3 even with a `repeat` leaf |
| `s.repeat(k)` | leaf | `repeat` | `js_repeat`: 29.9 / 92.0 / 4241.7 (`k` = 8/64/4096) | rule 3: worse class (`join` of a `range` of copies), and `range` stops at 10,000 |
| `s.replace(a, b)`, texts | leaf | `replace {count: "first"}` | `js_replace`: 23.4 / 24.5 / 51.6 | rule 2: inexact, ``("abc", "b", "[$`]")`` gives ``"a[$`]c"``, JavaScript `"a[a]c"`; above 3 besides |
| `s.replace(re, b)` | leaf | `regex_replace` (`g` gives every match) | | rule 2 |
| `s.replaceAll(a, b)`, texts | leaf | `replace {count: "all"}` | `js_replace_all`: 7.9 / 5.2 / 5.0 | rule 2: inexact, `("abc", "b", "$&$&")` gives `"a$&$&c"`, JavaScript `"abbc"`; above 3 besides |
| `s.replaceAll(re, b)` | leaf | `regex_replace` (`re` must have `g`) | | rule 2 |
| `s.search(re)` | leaf | `regex_search` | | rule 2 |
| `s.slice(a, b)` | leaf change | `slice {mode: "end"}` | `js_slice`: 37.1 / 28.9 / 3.8; mapped 13.8 / 12.1 / 2.7 | rule 3: same class, 29 at 64 (mapped 12) is above 3 |
| `s.split(t)` | reader | `split` (exists) | | rule 1: mapped today |
| `s.split("")` | leaf change | `split` gives each code unit for `""` | `js_split_units`: 8.8 / 5.9 / 4.8 (8/64/512) | rule 3: same class, 5.9 at 64 is above 3; the leaf gave nothing for `""` before |
| `s.split(t, limit)` | leaf change | `split` gains a `limit` input | `js_split_limit`: 10.1 / 20.4 / 217.1 | rule 2: inexact, `limit` -1 (ToUint32's 4294967295) gives nothing; a worse class besides |
| `s.split(re, limit)` | leaf | `regex_split` | | rule 2 |
| `s.startsWith(t, p)` | leaf | `text_contains {at: "start"}` | `js_starts_with`: 25.9 / 27.7 / 27.7 / 26.2 (8/64/4096/65536); mapped 9.5 / 9.1 / 9.1 | rule 3: same class (no walk: `slice` stops at `t`'s length), 27.7 at 64 is above 3 |
| `s.substring(a, b)` | leaf change | `slice {mode: "substring"}` | `js_substring`: 37.9 / 29.8 / 2.8 | rule 3: same class, 30 at 64 is above 3 |
| `s.toLocaleLowerCase()`, `toLocaleUpperCase()` | reader | `lowercase`, `uppercase` | | rule 1: no locale, as inside `js()`; a locale argument is refused (§12) |
| `s.toLowerCase()`, `toUpperCase()` | reader | `lowercase`, `uppercase` (exist) | | rule 1: mapped today |
| `s.toString()` | reader | `as_text` | | rule 1: the text itself |
| `s.toWellFormed()` | reader | `concat(s, "")` | `js_to_well_formed`: 10.1 / 8.6 / 8.7 (a composite's floor); mapped 2.96 / 2.92 / 2.90 | rule 1: the text for a text, nothing for anything else (JavaScript's TypeError) |
| `s.trim()`, `trimStart()`, `trimEnd()` | leaf | `trim {side}` | `js_trim_start`: 145.7 / 764.9 / 6365.8 (8/64/512) | rule 3: worse class (a `fold` over every character, a node each) |
| `s.valueOf()` | reader | the value itself, no node | | rule 1 |
| `String.fromCharCode(…)` | leaf | `from_codes {mode: "unit"}` | | rule 2: no leaf turns a number into a character |
| `String.fromCodePoint(…)` | leaf | `from_codes {mode: "point"}` | | rule 2 |
| `` String.raw`…` `` | reader | `concat` of the raw quasis and `as_text` of each expression | | rule 1 |
| `String(x)` | leaf change | `as_text` writes numbers as JavaScript does | `js_number_text`, the layout test alone, mapped: 14.9 / 4.9 / 3.6 (1/64/4096 numbers) | rule 3: a lower bound already above 3 at 64; today 1e21 gives `"1000000000000000000000"` where JavaScript gives `"1e+21"` |
| `a + b`, texts not literal | leaf change | `add`: a text operand concatenates | `js_plus`: 40.1 / 41.8 / 65.9 | rule 2: inexact, `1e21 + ""` gives `"1000000000000000000000"`, JavaScript `"1e+21"`; above 3 besides |
| `` `…${x}…` `` | reader | `concat` of `as_text` (exists) | | rule 1: lowered today; numbers need the `as_text` change |
| `<`, `>`, `<=`, `>=` on texts | leaf change | `compare` on two texts, code-unit order | | rule 2: no leaf orders texts (`compare` takes numbers) |
| `encodeURI(s)` | leaf | `uri_encode {set: "uri"}` | as `encodeURIComponent` | rule 3: as `encodeURIComponent` |
| `encodeURIComponent(s)` | leaf | `uri_encode {set: "component"}` | `js_encode_component`: 171.9 / 369.9 / 494.1 (8/64/512) | rule 2: inexact in code units, `"a😀"` gives nothing; a worse class besides (a node per character) |
| `decodeURI(s)` | leaf | `uri_decode {set: "uri"}` | as `decodeURIComponent` | rule 3: as `decodeURIComponent` |
| `decodeURIComponent(s)` | leaf | `uri_decode {set: "component"}` | `js_decode_component`: 60.4 / 93.2 / 114.4 | rule 3: grows 1.9× from 8 to 4096 (worse class), and 93 at 64 |
| `escape(s)`, `unescape(s)` | refused (proposed) | `encodeURIComponent`, `decodeURIComponent` | | rule 4, proposed for §12: Annex B's legacy form; undecided |
| `btoa(s)` | leaf change | `base64 {bytes: "latin1"}` | | rule 2: no leaf turns a character into one byte (`base64` and `hex` read UTF-8) |
| `atob(s)` | leaf change | `base64 {bytes: "latin1", mode: "decode"}` | | rule 2: the same, and WHATWG's forgiving decode |
| `/re/flags` | reader | the regex leaves' `pattern` and `flags`, checked as it is read | | rule 1: a literal is no value of its own |
| `new RegExp(p, flags)` | reader | `p` a wired `pattern` input, `flags` a literal | | rule 1 |
| `re.test(s)` | leaf | `regex_test` | | rule 2 |
| `re.exec(s)` | leaf | `regex_exec` | | rule 2 |
| flags `d i m s u v`, `g`, `y` | leaf | the `flags` param; `g` picks the leaf; `y` refused | | rule 2; `y` needs `lastIndex` |
| `re.lastIndex` | refused | `regex_matches`, or `regex_exec` on `s.slice(k)` | | rule 4: state kept between calls |

In all, 57 rows: 13 *reader*, 41 *leaf* and 3 *refused* (one of them proposed); no *composite*.
Of the 41 leaf rows, 10 change an existing leaf (`index_of`, `slice`, `split`, `as_text`, `add`,
`compare`, `base64`; `text_length` changes too, for `length`'s units). Since the list step, `at`,
`includes` and `last_index_of` are leaves lists and texts share, so their rows build a leaf's text
side. The regular-expression rows wait: no `regex_*` leaf is built (the owner's decision). By reason: 22 are not
expressible (17 lack an atom, 5 have a composite that is inexact on the input the row names), 9 are
in a worse class, and 10 are in the same class with a ratio above 3 at n = 64.

No text composite passes the rule. The same-class rows (`at`, `charAt`, `slice`, `substring`,
`includes`, `startsWith`, `indexOf` with a position, `split("")`, `isWellFormed`, and `String(x)`'s
lower bound) lose to the floor a composite call pays and to the cost of each node. Prepared once,
as `map` applies them, they still cost 5 to 14 times a leaf, because a node costs about as much as
the native call. `padStart` over a `repeat` leaf would be in the same class too, at 13 times. They
are re-raced after the evaluator change ([leaves.md](leaves.md)). For a composite of `k` nodes to
come within 3× of one leaf, a node has to cost under 3/k of the leaf's call: under a fifth for
`at`'s 17 leaves.

## Leaf specs

Common to every text leaf:

- **Units.** Positions and lengths are UTF-16 code units. A result that would begin or end
  inside a surrogate pair gives nothing (JavaScript gives a text holding a lone surrogate).
- **Positions.** A position is a number, truncated toward zero (JavaScript's
  ToIntegerOrInfinity; Holon's numbers are finite, so there is no `NaN` case). A position that
  isn't a number gives nothing: JavaScript would convert it, Holon doesn't
  ([Differences](#differences-from-javascript-that-stay)). An optional position the form leaves out
  is an input left unwired.
- **Length.** A result longer than 2^29 − 24 code units (V8's maximum, past which Node throws a
  `RangeError`) gives nothing.
- **Bits.** Texts and integers only, apart from `trunc`, which is exact in IEEE: the same result
  natively and in wasm. `normalize` and the regex leaves use pinned pure-Rust tables.

| Leaf | Inputs, params | Meaning | Gives nothing where | JavaScript |
|---|---|---|---|---|
| `text_length` (change) | `text` | code units: the scalars, plus one more for each astral character | not a text | `s.length` |
| `slice` (change) | `text`, `start`, `end`; `mode` | `"length"` (today's, `substr`'s): `end` is a length, both whole and not negative. `"end"`: a negative position counts from the end, both clamped to the text, empty when `end` ≤ `start`; `end` defaults to the length. `"substring"`: both clamped to 0 and the length, swapped when `start` > `end` | a cut pair; `"length"` with a negative or fractional input | `s.substr(a, n)`, `s.slice(a, b)`, `s.substring(a, b)` |
| `index_of` (change) | `haystack`, `needle`, `from`? | the first `needle` at or after `from` (clamped to 0 and the length; 0 when unwired), as a unit index; `""` is found at `from`; -1 when absent | not texts | `s.indexOf(t, p)` |
| `split` (change) | `text`, `separator`, `limit`? | the pieces between each `separator`. `""` cuts every code unit, so `"".split("")` is `[]`. `limit` is ToUint32 (truncated, modulo 2^32) and keeps that many pieces; unwired, all | `""` on a text with an astral character | `s.split(t)`, `s.split(t, n)` |
| `at` (text side) | `value`, `index` | a negative index counts from the end; the one code unit at `index` | half a pair; an index outside the text (`undefined`) | `s.at(i)` |
| `text_at` | `text`, `index` | the one code unit at `index`; a negative index or one past the end gives `""` | half a pair | `s.charAt(i)` (`charAt()` reads 0) |
| `code_at` | `text`, `index`; `mode` | `"unit"`: the code unit at `index`, 0 to 65535. `"point"`: at a pair's first half the code point, otherwise the unit | an index outside the text (`NaN`, `undefined`) | `s.charCodeAt(i)`, `s.codePointAt(i)` |
| `from_codes` | numbers, any count; `mode` | `"unit"`: each ToUint16 (truncated, modulo 2^16). `"point"`: each a code point. The units joined; none gives `""` | a lone surrogate left over; `"point"`: a number that isn't whole, is negative or is past U+10FFFF (`RangeError`) | `String.fromCharCode(…)`, `String.fromCodePoint(…)` |
| `includes` (text side) | `haystack`, `needle`, `from`? | `needle` found at or after `from` (clamped to 0 and the length; 0 when unwired) | not texts | `s.includes(t, p)` |
| `text_contains` | `text`, `part`, `position`?; `at` | `"start"`: `part` begins at `position` (default 0). `"end"`: `part` ends at `position` (default the length). Positions clamped to 0 and the length | not texts | `s.startsWith(t, p)`, `s.endsWith(t, e)` |
| `last_index_of` (text side) | `haystack`, `needle`, `from`? | the last `needle` beginning at or before `from` (clamped to 0 and length − `needle`'s length; unwired, the end); -1 when absent or longer than the text | not texts | `s.lastIndexOf(t, p)` |
| `trim` | `text`; `side` | `"both"`, `"start"`, `"end"`: drops JavaScript's WhiteSpace and LineTerminator from that side: U+0009 to U+000D, U+0020, U+00A0, U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F, U+3000, U+FEFF. That is not Rust's `char::is_whitespace`, which takes U+0085 and leaves out U+FEFF | not a text | `s.trim()`, `s.trimStart()`, `s.trimEnd()` |
| `pad` | `text`, `length`, `fill`?; `side` | unchanged when `length` (truncated) is at most the text's or `fill` is `""`; otherwise `fill` (default `" "`) repeated and cut to the units missing, before (`"start"`) or after (`"end"`) | a cut pair in the fill; past the maximum length | `s.padStart(n, f)`, `s.padEnd(n, f)` |
| `repeat` | `text`, `count` | the text `count` times (truncated); `""` for 0 or `""` | a negative count (`RangeError`); past the maximum length | `s.repeat(k)` |
| `replace` | `text`, `pattern`, `replacement`; `count` | `"first"`: the first match of the text `pattern`; `"all"`: every match, left to right, not overlapping. `""` matches at 0 (`"first"`) or at every unit boundary (`"all"`). The replacement is GetSubstitution's: `$$` is `$`, `$&` the match, `` $` `` what precedes it, `$'` what follows; any other `$` stays | a result holding a lone surrogate (`"all"` with `""` on astral text) | `s.replace(a, b)`, `s.replaceAll(a, b)` with texts |
| `normalize` | `text`; `form` | Unicode normalization `"NFC"` (the default), `"NFD"`, `"NFKC"` or `"NFKD"`, by `unicode-normalization` 0.1.26 (`holon-core` depends on it directly, pinned) | not a text; the reader refuses a form that isn't one of the four literals (`RangeError`) | `s.normalize(f)` |
| `compare` (change) | `a`, `b`; `op` | two numbers as today; two texts in code-unit order, unit by unit, a prefix first (not Rust's `str` order, which differs between U+E000–U+FFFF and astral characters) | a number and a text (JavaScript converts) | `a < b`, `>`, `<=`, `>=` |
| `add` (change) | values, any count | a left fold from the first operand (not from 0, so `-0 + -0` stays `-0`). Each step: two numbers add; with a text on either side, both are written as `as_text` writes them and joined. `1 + 2 + "a"` is `"3a"`, `"a" + 1 + 2` is `"a12"` | a boolean without a text, a list or a record (see Differences) | `a + b` |
| `as_text` (change) | `value` | a number in Number::prototype::toString's layout: the shortest digits that read back, with an exponent from 1e21 up and below 1e-6 (`1e+21`, `1e-7`); `-0` is `"0"` as today. Texts, booleans, lists and records unchanged | never | `String(x)`, `` `${x}` ``, `x.toString()` |
| `uri_encode` | `text`; `set` | keeps A–Z, a–z, 0–9 and `-_.!~*'()`, and for `"uri"` also `;/?:@&=+$,#`; every other character as its UTF-8 bytes, `%XX` in capitals | not a text | `encodeURIComponent(s)`, `encodeURI(s)` |
| `uri_decode` | `text`; `set` | each `%XX` run read as UTF-8 (hex digits in either case); `"uri"` leaves an escape of `;/?:@&=+$,#` as written | a malformed escape, invalid UTF-8, an overlong form, a surrogate or a code point past U+10FFFF (`URIError`) | `decodeURIComponent(s)`, `decodeURI(s)` |
| `base64` (change) | `text`; `mode`, `alphabet`, `bytes` | `bytes: "utf8"` is today's. `"latin1"` (standard alphabet only): encoding takes each character as one byte and pads; decoding is WHATWG's forgiving-base64: ASCII whitespace dropped, one or two `=` dropped when the length is a multiple of 4, extra bits dropped, each byte one character | `"latin1"`: a character past U+00FF, a length of 1 modulo 4, a character outside the alphabet (InvalidCharacterError) | `btoa(s)`, `atob(s)` |
| `is_well_formed` | `value` | `true` for a text (a Holon text is always well formed) | not a text (JavaScript's TypeError) | `s.isWellFormed()` |

**Made redundant by the shared leaves, so not built.** `at`, `includes`, `index_of`,
`last_index_of` and `slice` each take a list or a text, so `text_at {mode: "at"}` is `at` on a
text and `text_contains {at: "any"}` is `includes` with its `from`. `text_at` keeps `charAt`'s
meaning alone and has no mode; `text_contains` takes `"start"` and `"end"`.

`is_well_formed` is a leaf only because nothing cheaper gives `true` for a text alone. Once
records.md's `type_of` landed, `type_of(s) === "string" ? true : undefined` was raced against it
(2026-11-07): 20.2 times the leaf at 64, 9.5 mapped, and it gives `false` for a number where
JavaScript throws. The leaf stays. The regex leaves are specified in
[Regular expressions](#regular-expressions); they wait.

## Composites

The TypeScript as raced. A leaf with no JavaScript form is imported by name, and `slice(s, start,
length)` is today's leaf. What the checks found closes the section.

`s.at(i)` and `s.charAt(i)`:

```ts
import { text_length, slice } from "holon"
function js_at(s, i) {
  const n = text_length(s)
  const k = i < 0 ? n + i : i
  return k >= 0 && k < n ? slice(s, k, 1) : undefined
}
```

```ts
import { text_length, slice } from "holon"
function js_char_at(s, i) {
  return slice(s, i < 0 ? 0 : i, i < 0 ? 0 : 1)
}
```

`s.slice(a, b)` and `s.substring(a, b)`:

```ts
import { text_length, slice } from "holon"
function js_slice(s, a, b) {
  const n = text_length(s)
  const lo = a < 0 ? (n + a < 0 ? 0 : n + a) : a
  const hi = b < 0 ? (n + b < 0 ? 0 : n + b) : (b > n ? n : b)
  return slice(s, lo, hi > lo ? hi - lo : 0)
}
```

```ts
import { text_length, slice } from "holon"
function js_substring(s, a, b) {
  const n = text_length(s)
  const x = a < 0 ? 0 : (a > n ? n : a)
  const y = b < 0 ? 0 : (b > n ? n : b)
  return slice(s, x < y ? x : y, x < y ? y - x : x - y)
}
```

The searches. `js_starts_with_at` (a position) is checked, not raced:

```ts
function js_includes(s, t) {
  return s.indexOf(t) !== -1
}
```

```ts
import { text_length, slice } from "holon"
function js_starts_with(s, t) {
  return (slice(s, 0, text_length(t)) === t) ?? false
}
```

```ts
import { text_length, slice } from "holon"
function js_starts_with_at(s, t, p) {
  return (slice(s, p < 0 ? 0 : p, text_length(t)) === t) ?? false
}
```

```ts
import { text_length, slice } from "holon"
function js_ends_with(s, t) {
  const n = text_length(s)
  const m = text_length(t)
  return n >= m ? (slice(s, n - m, m) === t) ?? false : false
}
```

```ts
import { text_length, slice } from "holon"
function js_index_from(s, t, p) {
  const n = text_length(s)
  const k = p < 0 ? 0 : (p > n ? n : p)
  const rest = slice(s, k, n) ?? slice(s, k + 1, n)
  const i = rest.indexOf(t)
  return t === "" ? k : (i === -1 ? -1 : i + n - text_length(rest))
}
```

```ts
import { text_length, slice, range } from "holon"
function js_last_index_of(s, t) {
  const n = text_length(s)
  const m = text_length(t)
  return m > n ? -1 : range(n - m + 1).reduce((acc, i) => ((slice(s, i, m) === t) ?? false ? i : acc), -1)
}
```

`s.trimStart()`. After its escapes, the literal holds the other whitespace characters themselves,
which don't show: U+00A0, U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F, U+3000 and
U+FEFF:

```ts
import { text_length, slice, range } from "holon"
function js_trim_start(s) {
  const n = text_length(s)
  const k = range(n).reduce((acc, i) => (acc === i && (" \t\n\v\f\r                 　﻿".indexOf(slice(s, i, 1)) ?? -1) !== -1 ? i + 1 : acc), 0)
  return slice(s, k, n)
}
```

`s.repeat(k)` and `s.padStart(len, fill)`. `s ?? i` is `s`: a function in place must use its
parameter. The second `padStart` stands in for a `repeat` leaf: `fills` is the fill already
repeated, and the inner `slice` takes `repeat`'s place (one node, the same copy of `k` units for
a one-unit fill):

```ts
import { range } from "holon"
function js_repeat(s, k) {
  return range(k).map((i) => s ?? i).join("")
}
```

```ts
import { text_length, slice, range } from "holon"
function js_pad_start(s, len, fill) {
  const k = len - text_length(s)
  const m = k > 0 && fill !== "" ? k : 0
  return `${slice(range(m).map((i) => fill ?? i).join(""), 0, m)}${s}`
}
```

```ts
import { text_length, slice } from "holon"
function js_pad_start_proxy(s, len, fills) {
  const k = len - text_length(s)
  const m = k > 0 && fills !== "" ? k : 0
  return `${slice(slice(fills, 0, m), 0, m)}${s}`
}
```

`s.split("")` and `s.split(sep, limit)`:

```ts
import { text_length, slice, range } from "holon"
function js_split_units(s) {
  return range(text_length(s)).map((i) => slice(s, i, 1))
}
```

```ts
import { length, range, get } from "holon"
function js_split_limit(s, sep, lim) {
  const parts = s.split(sep)
  return lim >= length(parts) ? parts : range(lim).map((i) => get(parts, i))
}
```

`replaceAll` and `replace` with texts. Both are exact while the replacement has no `$`:

```ts
function js_replace_all(s, a, b) {
  return s.split(a).join(b)
}
```

```ts
import { text_length, slice } from "holon"
function js_replace(s, a, b) {
  const i = s.indexOf(a)
  return i === -1 ? s : `${slice(s, 0, i)}${b}${slice(s, i + text_length(a), text_length(s))}`
}
```

`isWellFormed` and `toWellFormed`. The first is O(1) and gives nothing for anything but a text;
the second is the reader's lowering:

```ts
import { text_length, slice } from "holon"
function js_is_well_formed(s) {
  return slice(s, 0, 0) === ""
}
```

```ts
import { concat } from "holon"
function js_to_well_formed(s) {
  return concat(s, "")
}
```

Once `type_of` landed, `isWellFormed` was raced against it too, made exact (nothing for anything
but a text):

```ts
import { type_of } from "holon"
function js_is_string_exact(s) {
  return type_of(s) === "string" ? true : undefined
}
```

`a + b`. The kind of each side comes from the first character of its JSON:

```ts
import { json_stringify, slice, text_length } from "holon"
function kind(x) {
  return slice(json_stringify(x), 0, 1)
}
function js_plus(a, b) {
  const ka = kind(a)
  const kb = kind(b)
  const listy = ka === "[" || ka === "{" || kb === "[" || kb === "{"
  return !listy ? (ka === '"' || kb === '"' ? `${a}${b}` : a + b) : undefined
}
```

`String(x)` in JavaScript's layout. This is a lower bound: only the test for the exponent's
ranges, before any reformatting (which needs folds over the digits). Mapped over the numbers:

```ts
function js_number_text(x) {
  const a = x < 0 ? 0 - x : x
  return a >= 1e21 || (a < 0.000001 && a !== 0) ? `${x}e` : `${x}`
}
```

`encodeURIComponent` one character at a time; `decodeURIComponent` by one `hex` decode of the
whole text, the literal parts hex-encoded first:

```ts
import { text_length, slice, range, hex, uppercase } from "holon"
function pct(h) {
  const n = text_length(h)
  return `%${slice(h, 0, 2)}${n > 2 ? `%${slice(h, 2, 2)}` : ""}${n > 4 ? `%${slice(h, 4, 2)}` : ""}${n > 6 ? `%${slice(h, 6, 2)}` : ""}`
}
function enc(c) {
  return "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()".indexOf(c) !== -1 ? c : pct(uppercase(hex(c)))
}
function js_encode_component(s) {
  return range(text_length(s)).map((i) => enc(slice(s, i, 1))).join("")
}
```

```ts
import { text_length, slice, hex, head, tail } from "holon"
function js_decode_component(s) {
  const parts = s.split("%")
  const rest = tail(parts).map((p) => (text_length(p) >= 2 ? `${slice(p, 0, 2)}${hex(slice(p, 2, text_length(p)))}` : undefined))
  return hex(`${hex(head(parts))}${rest.join("")}`, { mode: "decode" })
}
```

Before the leaves counted code units, the check ran twice. The second run simulated code units:
each code unit of the inputs became one scalar (a pair's first half U+F0000 to U+F03FF, its second
U+F0400 to U+F07FF), and every `slice(` in a composite became a `strict_slice(` that gave nothing
where it cut a pair. The simulation went once the leaves counted code units.

**What the checks found.** Every composite above gives its candidate's answer on its cases (empty
texts, one character, astral characters alone, together and between others, a combining mark,
matches first, last and overlapping, positions negative, fractional and past either end), through
the code-unit leaves, except these. The bench asserts each difference, so this list stays true:

- `split` with a limit: `("a,b", ",", -1)` gives nothing; JavaScript's ToUint32 makes -1
  4294967295, so it gives `["a", "b"]`.
- `replaceAll`: `("abc", "b", "$&$&")` gives `"a$&$&c"` where JavaScript gives `"abbc"`;
  `("abc", "", "-")` gives `"a-b-c"` (`split("")` has no piece before the first unit or after the
  last) where JavaScript gives `"-a-b-c-"`.
- `replace`: ``("abc", "b", "[$`]")`` gives ``"a[$`]c"`` where JavaScript gives `"a[a]c"`.
- `encodeURIComponent`: `"a😀"` gives nothing, because the one-unit `slice` cuts the pair, where
  JavaScript gives `"a%F0%9F%98%80"`.
- `isWellFormed`: `js_is_well_formed([1])` gives `false`, because `slice` takes a list too, and
  `type_of(1) === "string"` gives `false`, where JavaScript throws for both.

`+` differed before the number step: `(1e21, "")` gave `"1000000000000000000000"` and `(-0, -0)`
gave 0. `as_text` now writes `"1e+21"` and `add` starts from its first operand, so both agree.

## Regular expressions

**Engine: `regress` 0.12.0**, pinned. It implements ECMAScript's own pattern grammar and
semantics: backreferences, lookbehind, named groups, `\p{…}` and the `v` flag's sets. Rust's
`regex` crate has none of the first two and a different syntax. `regress` is pure Rust, runs the
same natively and in wasm, and is the engine Boa 0.22 uses
(`fixtures/modules/js-engine/Cargo.lock`), so a pattern in a leaf means what it means inside
`js()`. Its `utf16` feature matches over code units: `find_from_ucs2` without `u` or `v`, where
JavaScript matches code units, and `find_from_utf16` with them, where it matches code points. (To
confirm against Boa's use when the dependency is added.) A leaf turns the text into UTF-16 once
per call (O(n)), so every index is in code units, as [Units](#units) recommends. The dependency
is not added by this page.

**The leaves.** Each takes the pattern as a text input and the flags as a literal param, `flags`.
A compiled pattern is cached by (pattern, flags) for the process. A captured group that took part
in no match is absent (JavaScript's `undefined`), and a match that would hold a lone surrogate
gives nothing.

| Leaf | Inputs; params | Gives | JavaScript |
|---|---|---|---|
| `regex_test` | `pattern`, `text`; `flags` | whether the pattern matches anywhere | `re.test(s)` |
| `regex_exec` | `pattern`, `text`; `flags` | the first match as a record: `"0"` (the match), `"1"`, `"2"`… (the groups, absent when unmatched), `index` (code units), `input`, `length` (groups + 1), `groups` (a record by name, when the pattern names any), and with `d`, `indices`: `[start, end]` keyed as the groups are. No match: nothing (`null`) | `re.exec(s)`, `s.match(re)` without `g` |
| `regex_matches` | `pattern`, `text`; `flags`, `shape` | every match, left to right, an empty match moving on by one unit (by one code point with `u` or `v`). `"records"`: a list of `regex_exec`'s records. `"texts"`: a list of the matched texts, nothing when there is none (`null`) | `s.matchAll(re)`; `s.match(re)` with `g` |
| `regex_search` | `pattern`, `text`; `flags` | the first match's index, -1 when none | `s.search(re)` |
| `regex_replace` | `pattern`, `text`, `replacement`; `flags`, `count` | the first match (`"first"`) or every match (`"all"`, which `g` selects) replaced; the replacement is GetSubstitution's: `$$`, `$&`, `` $` ``, `$'`, `$1` to `$99`, `$<name>`. The leaf writes it itself: `regress`'s own `replace` reads `$0` as the match, which JavaScript doesn't | `s.replace(re, b)`, `s.replaceAll(re, b)` |
| `regex_split` | `pattern`, `text`, `limit`?; `flags` | the pieces between matches, each match's groups spliced in after its piece, as JavaScript does; `limit` as `split`'s | `s.split(re, n)` |

**Flags.** `i`, `m`, `s`, `u`, `v` and `d` are supported (`regress`'s `Flags` takes the first five;
`d` is the leaf's `indices`). `g` picks the leaf: `match`, `replace` and `split` read it, and
`matchAll` and `replaceAll` require it, as JavaScript does (TypeError: refused). `g` on `test` and
`exec` is refused, because there it means "go on from `lastIndex`". `y` (sticky) is refused for
the same reason. The reader refuses any other flag, or one written twice, as JavaScript does
(`regress` would skip an unknown flag silently).

**Patterns.** A literal `/re/flags` is compiled as the reader reads it: an invalid pattern is a
read error, as JavaScript's SyntaxError is a parse error. `new RegExp(p, flags)` takes `p` from a
wire. An invalid pattern there gives nothing (a SyntaxError at run time), and `flags` must be a
literal. A regex is not a value of its own: it can be held in a `const` and passed where a method
takes it, but it can't be stored, sent or compared.

**Refused or deferred.** `lastIndex` and anything that reads it, because it is state between
calls. Its forms are `regex_matches` for every match, or `regex_exec` on `s.slice(k)` for a
search from `k`. A function as the replacement is deferred: it needs `regex_replace` to take a
function as `map` takes one. `RegExp.prototype.source`, `flags` and `toString` are deferred; they
are texts the reader knows.

**ReDoS: open.** `regress` backtracks (its PikeVM backend reads UTF-8 only), and its API has no
step budget. A pattern such as `(a+)+$` on a long text of `a`s takes exponential time. Options:
literal patterns only at first, which the deploy shows and a person approves; a step budget,
which needs it upstream or a fork; or a bound on the text's length. Undecided.

**Verification, when the dependency lands.** An oracle like the texts' one: the same patterns and
texts through each leaf and through `js()`'s Boa, compared value for value.

## Differences from JavaScript that stay

- **A lone surrogate** has no Holon value. Every JavaScript result that is a text holding one
  gives nothing ([Units](#units)).
- **`NaN`, `undefined`, `null` and a throw** give nothing. Examples: `charCodeAt` past the end,
  `at` outside the text, `repeat(-1)`, `fromCodePoint(1.5)`, `decodeURIComponent("%")`,
  `btoa("😀")`, `"abc".match(/z/)`, and a text longer than 2^29 − 24 units.
- **No conversion of arguments.** JavaScript converts an argument to the kind it wants:
  `s.slice("1")` slices from 1, `"10" < 9` compares numbers, `true + 1` is 2. A Holon leaf given
  the wrong kind gives nothing; the oracle keeps `true + 1` as the one such case it asserts. A
  missing argument is refused by the reader, where JavaScript reads `undefined` (`s.indexOf()`
  searches for the text `"undefined"`).
- **A list or record as text.** In a template literal, `String(x)` and `concat`, `as_text` writes
  a list or a record as JSON (`[1,2]`, `{"a":1}`), where JavaScript writes `"1,2"` and
  `"[object Object]"`. With `+` the candidate gives nothing. Undecided: whether `+` follows
  `as_text`, or both stay as they are.
- **No locale.** `toLocaleUpperCase("tr")` and `localeCompare` are refused (§12). `toUpperCase`
  and `toLowerCase` use Unicode's default mappings, special casing and the final sigma included,
  as JavaScript does without a locale. The two can differ only where Rust's tables and V8's are
  of different Unicode versions. Neither the oracle nor the checks cover casing, an existing
  leaf's.
- **Unicode versions.** `normalize` follows `unicode-normalization` 0.1.26's tables and `\p{…}`
  follows `regress`'s tables, each pinned. A character newly assigned in a later Unicode version
  can be treated differently from a given V8 build.
- **Regular expressions:** no `lastIndex`, no `y`, no `g` on `test` and `exec`, no function
  replacements yet; patterns from a wire are subject to the ReDoS decision.

## Reader changes

Built on 2026-11-07, as this section says, but where it says otherwise below.

- **Methods to leaves.** Each row above maps a JavaScript form to its leaf, with the param
  (`mode`, `at`, `side`, `count`, `set`, `bytes`, `form`, `shape`) fixed by the method's name. An
  optional argument left out is an input left unwired, except `charAt()`, `charCodeAt()` and
  `codePointAt()`, which read 0 as JavaScript does (the last was left out here, and is read so
  too).
- **`.length`** on a text lowers to `text_length`. A list's `.length` is lists.md's, so the form
  needs the value's type: the lowering takes it from the leaf that made the value, `text_length`
  for a node whose output is a `Text`, `length` for a `List`, and refuses `.length` on anything
  else (a request's field, a function's parameter), naming both. `fmt` writes `.length` back
  where it would read back so.
- **`+` with a text** lowers to `add`, which concatenates. The reader's refusal is gone, and so
  is the printer's parenthesizing of a text after `+`.
- **`<` and the other comparisons** on texts need no type: `compare` takes either two numbers or
  two texts.
- **`String(x)`, `x.toString()`** lower to `as_text` (`toString()` on a value known to be a
  number stays `number_format`, as the number step read it); **`x.valueOf()`** to the value
  itself; **`s.toWellFormed()`** to `concat(s, "")`; **`s.concat(a, …)`** to `concat` of `s` and
  `as_text` of each argument (a text as itself) where `s` is a text written in place or known to
  be one, and to a list's `concat` otherwise; **`` String.raw`…` ``** to `concat` of the raw
  quasis (`raw`, not `cooked`) and `as_text` of each expression.
- **Regular expression literals** wait (the owner's decision): where a method takes one
  (`match`, `matchAll`, `search`, a pattern in `replace` or `split`), it is refused, naming
  `js()`.

Four things the reader did when these races ran constrain what a composite can say. The first
changed meaning, and is fixed:

- **`c ? a : b` fell through to `b` when `a` was absent.** It lowered to `guard(c, a) ?? b`
  (`lower.rs`), so with `c` true and `a` absent it gave `b`, where JavaScript gives `undefined`.
  The natural `slice` and `at` composites were wrong in code units because of it (their absent
  `strict_slice` fell through to the other branch); the composites above keep a call that can be
  absent out of a branch. Fixed 2026-11-07: it lowers to `guard(c, a) ?? guard(!(c ?? false),
  b)`, a `default`, a `not` and a `guard` more, the way `match` guards its `else`; `c` absent
  still takes `b`, as `if` does. `reader_facts()` in the bench asserts the fix and fails if the
  fall-through comes back. `undefined` is also accepted only as the else branch.
- **A function can't hand back a parameter untouched, and every parameter must be used.**
  `(i) => s` had to be `(i) => s ?? i` in `js_repeat`, and `return s` couldn't be written.
- **A leaf called by name needs every input.** `concat(s)` is refused ("the input b can't be
  left out"), hence `concat(s, "")`.
- **A function in place, `xs.map((x) => f(x))`, calls `f` as a nested composite** for each item,
  paying a composite call's floor each time. `xs.map(f)` prepares `f` once. The mapped races here
  map by name for that reason (the harness's `mapped` writes the first form).

## Race output

The bench's output, verbatim (`cargo bench -p holon-author --bench races_texts`, 53 s, exit 0).
Its `reader:` line is from before the ternary's fix; the bench now prints that a chosen `a` that
is absent makes the choice absent.

```text
oracle: every candidate gives JavaScript's answer, in code units, on 1525 cases (Node 22.21.1)
reader: `c ? a : b` with c true and a absent gives b (JavaScript gives undefined)

### text_length, code units (left) against scalars (right)

| n | native candidate | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 55 ns | 47 ns | 1.17 |
| 64 | 50 ns | 46 ns | 1.09 |
| 4096 | 317 ns | 189 ns | 1.67 |

### s.slice(1, -1), code units (left) against scalars (right)

| n | native candidate | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 93 ns | 85 ns | 1.10 |
| 64 | 152 ns | 110 ns | 1.38 |
| 4096 | 3.35 µs | 2.02 µs | 1.66 |

### s.indexOf(t), t absent, code units (left) against scalars (right)

| n | native candidate | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 52 ns | 51 ns | 1.03 |
| 64 | 65 ns | 65 ns | 1.01 |
| 4096 | 1.18 µs | 1.15 µs | 1.02 |

### s.at(i)

| n | composite js_at (17 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 1.98 µs | 77 ns | 25.77 |
| 64 | 2.05 µs | 89 ns | 23.04 |
| 4096 | 2.32 µs | 1.08 µs | 2.15 |

### s.at(3), mapped over 16 texts

| n | composite each_at_3 (17 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 13.57 µs | 1.24 µs | 10.97 |
| 64 | 13.60 µs | 1.24 µs | 10.94 |
| 4096 | 17.15 µs | 1.23 µs | 13.93 |

### s.charAt(i)

| n | composite js_char_at (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 1.58 µs | 78 ns | 20.15 |
| 64 | 1.64 µs | 89 ns | 18.33 |
| 4096 | 1.68 µs | 994 ns | 1.69 |

### s.charAt(3), mapped over 16 texts

| n | composite each_char_at_3 (13 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 8.81 µs | 1.24 µs | 7.12 |
| 64 | 9.57 µs | 1.28 µs | 7.46 |
| 4096 | 9.14 µs | 1.26 µs | 7.24 |

### s.slice(a, b)

| n | composite js_slice (37 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.12 µs | 84 ns | 37.10 |
| 64 | 3.18 µs | 110 ns | 28.94 |
| 4096 | 7.90 µs | 2.09 µs | 3.78 |

### s.slice(1, -1), mapped over 16 texts

| n | composite each_slice_1 (36 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 19.45 µs | 1.41 µs | 13.77 |
| 64 | 21.93 µs | 1.81 µs | 12.11 |
| 4096 | 85.97 µs | 32.10 µs | 2.68 |

### s.substring(a, b)

| n | composite js_substring (32 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.12 µs | 83 ns | 37.85 |
| 64 | 3.20 µs | 107 ns | 29.84 |
| 4096 | 5.44 µs | 1.96 µs | 2.78 |

### s.includes(t), t absent

| n | composite js_includes (4 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 824 ns | 51 ns | 16.20 |
| 64 | 835 ns | 65 ns | 12.94 |
| 4096 | 1.84 µs | 1.07 µs | 1.72 |

### s.includes("zq"), mapped over 16 texts

| n | composite each_includes_zq (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 5.32 µs | 865 ns | 6.15 |
| 64 | 5.47 µs | 1.08 µs | 5.06 |
| 4096 | 21.82 µs | 16.84 µs | 1.30 |

### s.startsWith(t)

| n | composite js_starts_with (7 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 1.18 µs | 46 ns | 25.93 |
| 64 | 1.24 µs | 45 ns | 27.67 |
| 4096 | 1.27 µs | 46 ns | 27.66 |
| 65536 | 1.21 µs | 46 ns | 26.16 |

### s.startsWith("The "), mapped over 16 texts

| n | composite each_starts_the (7 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 6.73 µs | 713 ns | 9.45 |
| 64 | 6.52 µs | 719 ns | 9.06 |
| 4096 | 6.59 µs | 721 ns | 9.13 |

### s.endsWith(t), t of 4

| n | composite js_ends_with (13 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 1.66 µs | 42 ns | 39.89 |
| 64 | 1.73 µs | 42 ns | 41.22 |
| 4096 | 2.12 µs | 42 ns | 50.64 |
| 65536 | 7.89 µs | 41 ns | 190.22 |

### s.indexOf(t, p), t absent

| n | composite js_index_from (33 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.04 µs | 56 ns | 54.24 |
| 64 | 3.25 µs | 90 ns | 36.22 |
| 4096 | 6.14 µs | 1.57 µs | 3.90 |

### s.lastIndexOf(t)

| n | composite js_last_index_of (23 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.21 µs | 60 ns | 86.20 |
| 64 | 26.11 µs | 106 ns | 246.19 |
| 512 | 208.34 µs | 345 ns | 604.26 |

### s.trimStart(), 4 spaces first

| n | composite js_trim_start (25 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 9.87 µs | 68 ns | 145.69 |
| 64 | 53.97 µs | 71 ns | 764.92 |
| 512 | 447.44 µs | 70 ns | 6365.75 |

### s.repeat(k), s of 2

| n | composite js_repeat (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.48 µs | 83 ns | 29.90 |
| 64 | 8.75 µs | 95 ns | 92.01 |
| 4096 | 612.11 µs | 144 ns | 4241.73 |

### s.padStart(len, "0"), half filled

| n | composite js_pad_start (24 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.28 µs | 146 ns | 29.41 |
| 64 | 7.43 µs | 207 ns | 35.97 |
| 4096 | 248.70 µs | 1.20 µs | 207.35 |

### s.padStart(len, "0"), repeat as a leaf (stand-in)

| n | composite js_pad_start_proxy (21 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.60 µs | 165 ns | 15.76 |
| 64 | 2.72 µs | 208 ns | 13.06 |
| 4096 | 5.97 µs | 1.38 µs | 4.34 |

### s.split("")

| n | composite js_split_units (7 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 3.11 µs | 355 ns | 8.76 |
| 64 | 12.61 µs | 2.14 µs | 5.90 |
| 512 | 95.80 µs | 19.94 µs | 4.81 |
split limit: inexact, as texts.md says: on [Text("a,b"), Text(","), Number(-1.0)] the composite gives None, JavaScript Some(List([Text("a"), Text("b")]))

### s.split(" ", 2)

| n | composite js_split_limit (10 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 1.58 µs | 156 ns | 10.12 |
| 64 | 3.06 µs | 150 ns | 20.42 |
| 4096 | 35.36 µs | 163 ns | 217.07 |
replaceAll: inexact, as texts.md says: on [Text("abc"), Text("b"), Text("$&$&")] the composite gives Some(Text("a$&$&c")), JavaScript Some(Text("abbc"))
replaceAll: inexact, as texts.md says: on [Text("abc"), Text(""), Text("-")] the composite gives None, JavaScript Some(Text("-a-b-c-"))

### s.replaceAll(" ", "_")

| n | composite js_replace_all (2 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 899 ns | 114 ns | 7.85 |
| 64 | 1.91 µs | 369 ns | 5.18 |
| 4096 | 60.15 µs | 12.00 µs | 5.01 |
replace: inexact, as texts.md says: on [Text("abc"), Text("b"), Text("[$`]")] the composite gives Some(Text("a[$`]c")), JavaScript Some(Text("a[a]c"))

### s.replace("e", "E")

| n | composite js_replace (19 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 2.47 µs | 106 ns | 23.36 |
| 64 | 2.76 µs | 113 ns | 24.51 |
| 4096 | 6.72 µs | 130 ns | 51.55 |

### s.isWellFormed()

| n | composite js_is_well_formed (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 878 ns | 40 ns | 21.94 |
| 64 | 872 ns | 40 ns | 21.91 |
| 4096 | 878 ns | 40 ns | 22.15 |

### s.isWellFormed(), mapped over 16 texts

| n | composite each_well_formed (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 4.80 µs | 666 ns | 7.21 |
| 64 | 4.83 µs | 663 ns | 7.29 |
| 4096 | 4.83 µs | 674 ns | 7.16 |

### s.isWellFormed() as s === s, the leaf equals

| n | leaf equals | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 48 ns | 40 ns | 1.21 |
| 64 | 50 ns | 40 ns | 1.24 |
| 4096 | 134 ns | 41 ns | 3.25 |

### s.toWellFormed()

| n | composite js_to_well_formed (2 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 699 ns | 69 ns | 10.11 |
| 64 | 705 ns | 82 ns | 8.57 |
| 4096 | 717 ns | 83 ns | 8.66 |

### s.toWellFormed(), mapped over 16 texts

| n | composite each_well_formed_text (3 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 3.45 µs | 1.17 µs | 2.96 |
| 64 | 3.38 µs | 1.16 µs | 2.92 |
| 4096 | 4.64 µs | 1.60 µs | 2.90 |

### String(x), JavaScript's layout: the composite's test alone (a lower bound), mapped

| n | composite each (24 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 1 | 2.94 µs | 198 ns | 14.85 |
| 64 | 61.15 µs | 12.57 µs | 4.87 |
| 4096 | 3.75 ms | 1.05 ms | 3.56 |
+: inexact, as texts.md says: on [Number(1e21), Text("")] the composite gives Some(Text("1000000000000000000000")), JavaScript Some(Text("1e+21"))
+: inexact, as texts.md says: on [Number(-0.0), Number(-0.0)] the composite gives Some(Number(0.0)), JavaScript Some(Number(-0.0))

### a + b, two texts of n/2

| n | composite js_plus (34 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 5.67 µs | 141 ns | 40.09 |
| 64 | 5.97 µs | 143 ns | 41.82 |
| 4096 | 12.93 µs | 196 ns | 65.86 |
encodeURIComponent: inexact, as texts.md says: on [Text("a😀")] the composite gives None, JavaScript Some(Text("a%F0%9F%98%80"))

### encodeURIComponent(s)

| n | composite js_encode_component (62 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 21.41 µs | 125 ns | 171.85 |
| 64 | 172.80 µs | 467 ns | 369.88 |
| 512 | 1.43 ms | 2.89 µs | 494.13 |

### decodeURIComponent(s), n characters encoded

| n | composite js_decode_component (28 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 4.53 µs | 75 ns | 60.39 |
| 64 | 19.73 µs | 212 ns | 93.21 |
| 4096 | 1.12 ms | 9.77 µs | 114.37 |
```

### `isWellFormed`, re-raced on 2026-11-07

Through the code-unit leaves, once `type_of` had landed
(`cargo bench -q -p holon-author --bench races_texts -- isWellFormed`, exit 0), the part of its
output on `isWellFormed`:

```text
isWellFormed, a list: inexact, as texts.md says: on [List([Number(1.0)])] the composite gives Some(Boolean(false)), JavaScript None

### s.isWellFormed()

| n | composite js_is_well_formed (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 569 ns | 44 ns | 13.03 |
| 64 | 581 ns | 41 ns | 14.27 |
| 4096 | 818 ns | 42 ns | 19.68 |

### s.isWellFormed(), mapped over 16 texts

| n | composite each_well_formed (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 4.55 µs | 598 ns | 7.61 |
| 64 | 5.05 µs | 594 ns | 8.50 |
| 4096 | 8.31 µs | 592 ns | 14.03 |

### s.isWellFormed() as s === s, the leaf equals

| n | leaf equals | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 48 ns | 43 ns | 1.13 |
| 64 | 50 ns | 42 ns | 1.18 |
| 4096 | 130 ns | 42 ns | 3.07 |
isWellFormed as type_of(s) === "string": inexact, as texts.md says: on [Number(1.0)] the composite gives Some(Boolean(false)), JavaScript None

### s.isWellFormed() as type_of(s) === "string" ? true : undefined

| n | composite js_is_string_exact (5 leaves) | native candidate | ratio |
|---:|---:|---:|---:|
| 8 | 599 ns | 44 ns | 13.63 |
| 64 | 880 ns | 44 ns | 20.24 |
| 4096 | 581 ns | 42 ns | 13.71 |

### s.isWellFormed() as type_of(s) === "string" ? true : undefined, mapped over 16 texts

| n | composite each_is_string (6 leaves) | native candidate, mapped | ratio |
|---:|---:|---:|---:|
| 8 | 6.06 µs | 592 ns | 10.23 |
| 64 | 5.60 µs | 589 ns | 9.50 |
| 4096 | 5.58 µs | 590 ns | 9.46 |
```
