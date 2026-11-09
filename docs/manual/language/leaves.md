# Leaves

<!-- Generated from the catalog by holon_author::agent::leaves_page: don't edit it by hand.
     HOLON_BLESS=1 cargo test -p holon-author --test manual writes it again. -->

Every building block a flow is made of. Each is shown as `name(inputs) -> outputs {params}`:
call a leaf by its name, imported from `"holon"`, with its inputs in order and its parameters in a
last record (see [Calls](reference.md#calls)). Many are written another way (`a + b` is `add`,
`x ?? y` is `coalesce`, `` `${x}` `` is `concat`, `t.toUpperCase()` is `uppercase`); the
[reference](reference.md) has them all.

## Computing

These touch nothing outside: given the same inputs, they always give the same output.

[`const`](#const) · [`add`](#add) · [`multiply`](#multiply) · [`subtract`](#subtract) · [`divide`](#divide) · [`compare`](#compare) · [`and`](#and) · [`or`](#or) · [`not`](#not) · [`select`](#select) · [`cons`](#cons) · [`head`](#head) · [`tail`](#tail) · [`is_empty`](#is_empty) · [`fold`](#fold) · [`map`](#map) · [`math`](#math) · [`max`](#max) · [`min`](#min) · [`aggregate`](#aggregate) · [`atan2`](#atan2) · [`power`](#power) · [`remainder`](#remainder) · [`bitwise`](#bitwise) · [`number_test`](#number_test) · [`to_number`](#to_number) · [`number_format`](#number_format) · [`concat`](#concat) · [`uppercase`](#uppercase) · [`lowercase`](#lowercase) · [`text_length`](#text_length) · [`slice`](#slice) · [`index_of`](#index_of) · [`split`](#split) · [`text_at`](#text_at) · [`code_at`](#code_at) · [`from_codes`](#from_codes) · [`text_contains`](#text_contains) · [`trim`](#trim) · [`pad`](#pad) · [`repeat`](#repeat) · [`replace`](#replace) · [`normalize`](#normalize) · [`uri_encode`](#uri_encode) · [`uri_decode`](#uri_decode) · [`is_well_formed`](#is_well_formed) · [`list`](#list) · [`get`](#get) · [`append`](#append) · [`extend`](#extend) · [`length`](#length) · [`sum`](#sum) · [`join`](#join) · [`at`](#at) · [`indices`](#indices) · [`entries`](#entries) · [`any`](#any) · [`all`](#all) · [`filter`](#filter) · [`find`](#find) · [`find_index`](#find_index) · [`find_last`](#find_last) · [`find_last_index`](#find_last_index) · [`sort_by`](#sort_by) · [`includes`](#includes) · [`last_index_of`](#last_index_of) · [`reverse`](#reverse) · [`to_spliced`](#to_spliced) · [`with_item`](#with_item) · [`fill`](#fill) · [`copy_within`](#copy_within) · [`sort`](#sort) · [`flat`](#flat) · [`is_list`](#is_list) · [`unique`](#unique) · [`range`](#range) · [`pair`](#pair) · [`some`](#some) · [`none`](#none) · [`record`](#record) · [`field`](#field) · [`with_field`](#with_field) · [`without_field`](#without_field) · [`keys`](#keys) · [`lookup`](#lookup) · [`insert`](#insert) · [`remove`](#remove) · [`values`](#values) · [`from_entries`](#from_entries) · [`merge`](#merge) · [`has`](#has) · [`group_by`](#group_by) · [`same_value`](#same_value) · [`type_of`](#type_of) · [`json_parse`](#json_parse) · [`json_stringify`](#json_stringify) · [`date_part`](#date_part) · [`date_utc`](#date_utc) · [`date_set`](#date_set) · [`date_text`](#date_text) · [`date_parse`](#date_parse) · [`date_add`](#date_add) · [`business_day_add`](#business_day_add) · [`business_days_between`](#business_days_between) · [`is_business_day`](#is_business_day) · [`sha256`](#sha256) · [`hmac_sha256`](#hmac_sha256) · [`hex`](#hex) · [`base64`](#base64) · [`secure_equals`](#secure_equals) · [`text_to_bytes`](#text_to_bytes) · [`bytes_to_text`](#bytes_to_text) · [`bytes_length`](#bytes_length) · [`bytes_concat`](#bytes_concat) · [`bytes_to_list`](#bytes_to_list) · [`list_to_bytes`](#list_to_bytes) · [`decimal_add`](#decimal_add) · [`decimal_sub`](#decimal_sub) · [`decimal_mul`](#decimal_mul) · [`decimal_div`](#decimal_div) · [`decimal_round`](#decimal_round) · [`decimal_sum`](#decimal_sum) · [`decimal_compare`](#decimal_compare) · [`csv_parse`](#csv_parse) · [`csv_format`](#csv_format) · [`xml_parse`](#xml_parse) · [`xml_format`](#xml_format) · [`xml_find`](#xml_find) · [`xml_text`](#xml_text) · [`xml_attribute`](#xml_attribute) · [`to_text`](#to_text) · [`as_text`](#as_text) · [`is_truthy`](#is_truthy) · [`guard`](#guard) · [`default`](#default) · [`coalesce`](#coalesce) · [`equals`](#equals) · [`fail`](#fail)

### `const`

```
const() -> out: Number  {value: 0, type: Number}
```

A fixed value set on the node: a number, true or false, text, or JSON. The type picks which.

### `add`

```
add(a: Any, b: Any) -> out: Any
```

Adds its numbers, as JavaScript's + does; with a text among them, joins them, writing a number or a boolean as text (1 + 2 + "a" is "3a"). Takes more inputs with +.

### `multiply`

```
multiply(a: Number, b: Number) -> out: Number
```

Multiplies its numbers. Takes more inputs with +.

### `subtract`

```
subtract(a: Number, b: Number) -> out: Number
```

Subtracts b from a: a − b.

### `divide`

```
divide(a: Number, b: Number) -> out: Number
```

Divides a by b: a ÷ b. Dividing by zero produces nothing.

### `compare`

```
compare(a: Any, b: Any) -> out: Boolean  {op: =}
```

Compares two numbers, or two texts code unit by code unit as JavaScript does, with the chosen operator (=, !=, <, <=, >, >=): true or false.

### `and`

```
and(a: Boolean, b: Boolean) -> out: Boolean
```

True when every input is true. Takes more inputs with +.

### `or`

```
or(a: Boolean, b: Boolean) -> out: Boolean
```

True when any input is true. Takes more inputs with +.

### `not`

```
not(v: Boolean) -> out: Boolean
```

The opposite of a true or false.

### `select`

```
select(cond: Boolean, if_true: Any, if_false: Any) -> out: Any
```

Passes on if_true when cond is true, if_false when it's false.

### `cons`

```
cons(item: Any, list: List<Any>) -> out: List<Any>
```

A list with item put first, before the items of list.

### `head`

```
head(list: List<Any>) -> out: Any
```

A list's first item; nothing for an empty list.

### `tail`

```
tail(list: List<Any>) -> out: List<Any>
```

A list without its first item; nothing for an empty list.

### `is_empty`

```
is_empty(list: List<Any>) -> out: Boolean
```

Whether a list has no items.

### `fold`

```
fold(init: Any, list: List<Any>) -> out: Any  {fn}
```

Runs its function over a list, carrying a result along: starting from init, each item and the result so far give the next result.

### `map`

```
map(list: List<Any>) -> out: List<Any>  {fn}
```

Runs its function on each item of a list: the list of results.

### `math`

```
math(x: Number) -> out: Number  {fn: abs}
```

One of JavaScript's Math functions of a number, chosen by fn: abs, sign, floor, ceil, trunc, round, sqrt, cbrt, exp, expm1, log, log2, log10, log1p, sin, cos, tan, their inverses and hyperbolic forms, fround, f16round, clz32. Produces nothing where JavaScript gives NaN or Infinity.

### `max`

```
max(a: Number, b: Number) -> out: Number
```

The largest of its numbers (0 over -0). Takes more inputs with +.

### `min`

```
min(a: Number, b: Number) -> out: Number
```

The smallest of its numbers (-0 under 0). Takes more inputs with +.

### `aggregate`

```
aggregate(xs: List<Number>) -> out: Number  {fn: max}
```

One number from a list of numbers, chosen by fn: its max, its min, hypot (the square root of the sum of the squares) or sum_precise (the exact sum, rounded once). The max or min of an empty list produces nothing.

### `atan2`

```
atan2(y: Number, x: Number) -> out: Number
```

The angle from the x axis to the point (x, y), in radians: Math.atan2(y, x).

### `power`

```
power(a: Number, b: Number) -> out: Number
```

a to the power b, as Math.pow(a, b). Produces nothing where JavaScript gives NaN or Infinity (0 to a negative power, a negative number to a fraction).

### `remainder`

```
remainder(a: Number, b: Number) -> out: Number
```

What is left of a after dividing by b, with a's sign: a % b. Dividing by zero produces nothing.

### `bitwise`

```
bitwise(a: Number, b: Number) -> out: Number  {op: and}
```

A 32-bit integer operation on two numbers, chosen by op: and, or, xor, shl, shr, ushr (JavaScript's &, |, ^, <<, >>, >>>) or imul (Math.imul).

### `number_test`

```
number_test(x: Any) -> out: Boolean  {test: finite, coerce: none}
```

Whether a value is a finite number, an integer, a safe integer or NaN, chosen by test. With coerce "none" only a number can pass (Number.isFinite); with "number" the value is read as a number first (isFinite, isNaN).

### `to_number`

```
to_number(x: Any, radix: Number) -> out: Number  {grammar: number}
```

A value read as a number, by grammar: number (Number(x)), float (parseFloat) or int (parseInt, in radix when it is wired). Produces nothing when it doesn't read as a finite number.

### `number_format`

```
number_format(x: Number, digits: Number) -> out: Text  {style: string}
```

A number written as text, by style: string (toString, in radix digits when wired), fixed (toFixed), precision (toPrecision) or exponential (toExponential), digits as each takes them. Digits out of range produce nothing.

### `concat`

```
concat(a: Text, b: Text) -> out: Text
```

Joins texts end to end. Takes more inputs with +.

### `uppercase`

```
uppercase(s: Text) -> out: Text
```

The text in upper case.

### `lowercase`

```
lowercase(s: Text) -> out: Text
```

The text in lower case.

### `text_length`

```
text_length(s: Text) -> out: Number
```

How long a text is in UTF-16 code units, as JavaScript's s.length counts: a character past U+FFFF, an emoji say, is two.

### `slice`

```
slice(value: Any, start: Number, end: Number) -> out: Any  {mode: length}
```

Part of a text or a list, by mode, a text in UTF-16 code units. length: end units or items from start (counting from 0). end: from start up to end, each counting from the end when negative, end the length when unwired (s.slice(start, end)). substring: a text's, from start up to end, each clamped to the text, the two swapped when end comes first. Nothing when a piece would cut an emoji in half.

### `index_of`

```
index_of(haystack: Any, needle: Any, from: Number) -> out: Number
```

Where needle first appears at or after from (0 when unwired): in a list, the first item that equals it, from counting from the end when negative; in a text, in UTF-16 code units. −1 when it doesn't.

### `split`

```
split(text: Text, sep: Text, limit: Number) -> out: List<Text>
```

Cuts a text at each sep into a list of pieces; join's inverse. An empty sep cuts every UTF-16 code unit (nothing for a text with an emoji); limit, when wired, keeps that many pieces.

### `text_at`

```
text_at(text: Text, index: Number) -> out: Text
```

The UTF-16 code unit of a text at index, as a text: s.charAt(i). The empty text past either end; nothing for half of an emoji.

### `code_at`

```
code_at(text: Text, index: Number) -> out: Number  {mode: unit}
```

The number of a text's character at index: unit, the UTF-16 code unit (s.charCodeAt(i)); point, the code point an emoji begins there (s.codePointAt(i)). Nothing past either end.

### `from_codes`

```
from_codes(a: Number) -> out: Text  {mode: unit}
```

A text made of character numbers: unit, UTF-16 code units (String.fromCharCode); point, code points (String.fromCodePoint). Takes more inputs with +. Nothing when a half of an emoji is left alone, or a number isn't a code point.

### `text_contains`

```
text_contains(text: Text, part: Text, position: Number) -> out: Boolean  {at: start}
```

Whether part begins a text at position (start, s.startsWith(t, p)), or ends it there (end, s.endsWith(t, p)). Unwired, position is the start or the end.

### `trim`

```
trim(text: Text) -> out: Text  {side: both}
```

A text without JavaScript's white space and line ends on one side or both: s.trim(), s.trimStart(), s.trimEnd().

### `pad`

```
pad(text: Text, length: Number, fill: Text) -> out: Text  {side: start}
```

A text filled to length UTF-16 code units at its start or end with fill, a space when unwired: s.padStart(n, f), s.padEnd(n, f).

### `repeat`

```
repeat(text: Text, count: Number) -> out: Text
```

A text count times over: s.repeat(k). Nothing for a negative count.

### `replace`

```
replace(text: Text, pattern: Text, replacement: Text) -> out: Text  {count: first}
```

A text with the first match of pattern, or every match, replaced: s.replace(a, b), s.replaceAll(a, b). In the replacement, $& is the match, $` what comes before it, $' what comes after and $$ a $.

### `normalize`

```
normalize(text: Text) -> out: Text  {form: NFC}
```

A text in a Unicode normalization form, NFC, NFD, NFKC or NFKD: s.normalize(form).

### `uri_encode`

```
uri_encode(text: Text) -> out: Text  {set: component}
```

A text escaped for a URL: component (encodeURIComponent) escapes all but letters, digits and -_.!~*'(); uri (encodeURI) keeps ;/?:@&=+$,# too.

### `uri_decode`

```
uri_decode(text: Text) -> out: Text  {set: component}
```

A text with its %XX escapes read back as UTF-8: decodeURIComponent, decodeURI (uri, which leaves ;/?:@&=+$,# escaped). Nothing for an escape that isn't valid.

### `is_well_formed`

```
is_well_formed(value: Any) -> out: Boolean
```

True for a text, which is always well formed in Holon: s.isWellFormed(). Nothing for anything else.

### `list`

```
list(a: Any, b: Any) -> out: List<Any>
```

A list of its inputs, in order. Takes more inputs with +.

### `get`

```
get(list: List<Any>, index: Number) -> out: Any
```

The item of a list at index (counting from 0); nothing past its end.

### `append`

```
append(list: List<Any>, item: Any) -> out: List<Any>
```

A list with item added at the end.

### `extend`

```
extend(list: List<Any>, items: List<Any>) -> out: List<Any>
```

A list with every item of items added at the end.

### `length`

```
length(list: List<Any>) -> out: Number
```

How many items a list has.

### `sum`

```
sum(list: List<Number>) -> out: Number
```

The total of a list of numbers.

### `join`

```
join(list: List<Text>, sep: Text) -> out: Text
```

Joins a list of texts into one, with sep between each.

### `at`

```
at(value: Any, index: Number) -> out: Any
```

The item of a list, or the UTF-16 code unit of a text, at index, counting from the end when it is negative (xs.at(-1) is the last); nothing out of range or for half of an emoji.

### `indices`

```
indices(list: List<Any>) -> out: List<Number>
```

A list's positions: 0 to its length − 1 (xs.keys()).

### `entries`

```
entries(value: Any) -> out: List<Any>
```

A list's [index, item] pairs, or a record's [key, value] pairs in key order.

### `any`

```
any(list: List<Any>) -> out: Boolean  {fn}
```

Whether its function answers true for any item of a list, stopping at the first (xs.some). Its answer is read as JavaScript reads a condition: false, 0 and "" are false, anything else true.

### `all`

```
all(list: List<Any>) -> out: Boolean  {fn}
```

Whether its function answers true for every item of a list, stopping at the first false (xs.every).

### `filter`

```
filter(list: List<Any>) -> out: List<Any>  {fn}
```

The items of a list its function answers true for, in order.

### `find`

```
find(list: List<Any>) -> out: Any  {fn}
```

The first item of a list its function answers true for, stopping there; nothing when none does.

### `find_index`

```
find_index(list: List<Any>) -> out: Number  {fn}
```

The position of the first item its function answers true for, stopping there; −1 when none does.

### `find_last`

```
find_last(list: List<Any>) -> out: Any  {fn}
```

The last item of a list its function answers true for, looking from the end; nothing when none does.

### `find_last_index`

```
find_last_index(list: List<Any>) -> out: Number  {fn}
```

The position of the last item its function answers true for, looking from the end; −1 when none does.

### `sort_by`

```
sort_by(list: List<Any>) -> out: List<Any>  {fn}
```

The items of a list in the order its function puts them: given two items a and b, it answers a number, below 0 for a first, above 0 for b first, 0 to keep their order. Stable, and the same order every time. An answer that isn't a number produces nothing.

### `includes`

```
includes(haystack: Any, needle: Any, from: Number) -> out: Boolean
```

Whether an item of a list at or after from equals needle (lists and records by what they hold), or a text holds needle at or after from; from 0 when unwired.

### `last_index_of`

```
last_index_of(haystack: Any, needle: Any, from: Number) -> out: Number
```

The position of the last item of a list that equals needle, or of needle in a text, at or before from (the end when unwired); −1 when there is none.

### `reverse`

```
reverse(list: List<Any>) -> out: List<Any>
```

The items of a list in reverse order.

### `to_spliced`

```
to_spliced(list: List<Any>, start: Number, count: Number, items: List<Any>) -> out: List<Any>
```

A list with count items from start replaced by the items of items: start counts from the end when negative; count unwired is the rest of the list, items unwired is none.

### `with_item`

```
with_item(list: List<Any>, index: Number, value: Any) -> out: List<Any>
```

A list with the item at index replaced by value, index counting from the end when negative; nothing out of range.

### `fill`

```
fill(list: List<Any>, value: Any, start: Number, end: Number) -> out: List<Any>
```

A list with the positions from start up to end set to value, each counting from the end when negative; start unwired is 0, end the length.

### `copy_within`

```
copy_within(list: List<Any>, target: Number, start: Number, end: Number) -> out: List<Any>
```

A list with the items from start up to end copied over those from target, as they were before; each counts from the end when negative, start unwired is 0, end the length.

### `sort`

```
sort(list: List<Any>) -> out: List<Any>
```

The items of a list in JavaScript's default order: each written as text and compared character by character (so 10 comes before 9). Stable.

### `flat`

```
flat(list: List<Any>) -> out: List<Any>  {depth: 1}
```

A list with the items of its lists spread into it, depth levels deep (a whole number, or all).

### `is_list`

```
is_list(value: Any) -> out: Boolean
```

Whether a value is a list.

### `unique`

```
unique(list: List<Any>) -> out: List<Any>
```

Each item of a list once, where it first appears (lists and records by what they hold).

### `range`

```
range(n: Number) -> out: List<Number>
```

The numbers from 0 to n-1, as a list: map over it to do something n times. n is a whole number up to 10000; any other is nothing.

### `pair`

```
pair(a: Any, b: Any) -> out: (Any, Any)
```

Two values held together as one.

### `some`

```
some(value: Any) -> out: Any | Unit
```

An optional value that is there: holds its input.

### `none`

```
none() -> out: Any | Unit
```

An optional value that isn't there.

### `record`

```
record() -> out: Record
```

An empty record, to add fields to with with_field.

### `field`

```
field(record: Record) -> out: Any  {name: id}
```

One field of a record, by the name set on the node; nothing when the record hasn't it.

### `with_field`

```
with_field(record: Record, value: Any) -> out: Record  {name: id}
```

The record with a field set to value, by the name set on the node (added, or replaced).

### `without_field`

```
without_field(record: Record) -> out: Record  {name: id}
```

The record without the named field.

### `keys`

```
keys(record: Record) -> out: List<Text>
```

The names of a record's fields, in order.

### `lookup`

```
lookup(record: Record, key: Text) -> out: Any
```

One field of a record, by a key that arrives on a wire (a route's path param, say); nothing when it isn't there.

### `insert`

```
insert(record: Record, key: Text, value: Any) -> out: Record
```

The record with key set to value, the key arriving on a wire.

### `remove`

```
remove(record: Record, key: Text) -> out: Record
```

The record without key, the key arriving on a wire.

### `values`

```
values(record: Record) -> out: List<Any>
```

A record's values, in key order: Object.values(r).

### `from_entries`

```
from_entries(entries: List<Any>) -> out: Record
```

A record made of [key, value] pairs, a later key replacing an earlier: Object.fromEntries(es). Nothing when a pair isn't a list of a text key and a value.

### `merge`

```
merge(a: Record, b: Record) -> out: Record
```

Every field of its records, a later record's where two have one: { ...a, ...b }. Shallow: a record inside is replaced, not merged. Takes more records with +.

### `has`

```
has(record: Record, key: Text) -> out: Boolean
```

Whether a record has the key, with any value, null included: k in r, Object.hasOwn(r, k).

### `group_by`

```
group_by(list: List<Any>) -> out: Record  {fn}
```

The items of a list in lists by the key its function answers for each, a text: Object.groupBy(xs, f). Each list keeps the items' order; the keys come sorted. A key that isn't a text produces nothing.

### `same_value`

```
same_value(a: Any, b: Any) -> out: Boolean
```

Whether two values are the same as JavaScript's Object.is says: -0 isn't 0. Lists and records compare by what they hold.

### `type_of`

```
type_of(value: Any) -> out: Text
```

What JavaScript's typeof says of a value: "number", "boolean", "string" or "object" (a list, a record or null).

### `json_parse`

```
json_parse(text: Text) -> out: Any  {fn}
```

Reads JSON text into a value (records, lists, numbers, text…); nothing when it isn't valid JSON. With fn, a function of each key and value, applied from the inside out, its answer standing in for the value (JSON.parse(t, reviver)); one it answers nothing for is left out.

### `json_stringify`

```
json_stringify(value: Any) -> out: Text  {indent, keys, fn}
```

Writes a value out as JSON text, as JavaScript's JSON.stringify: indent lays it out a member a line, keys (a JSON list) keeps only those keys of every record, in that order, and fn, a function of each key and value applied from the outside in, writes its answer instead (the replacer); one it answers nothing for is left out.

### `date_part`

```
date_part(time: Any, offset: Any) -> out: Number  {part: time}
```

A date's time value, or one of its parts in UTC, chosen by part: time (new Date(x): a number of milliseconds since 1970, or a text as date_parse reads it), year, month (0 to 11), date, day (0 is Sunday), hours, minutes, seconds, milliseconds (getUTCFullYear() and the rest). Given an offset (+01:00, or minutes east of UTC), the part there: its hours, its date. Nothing for a date out of range.

### `date_utc`

```
date_utc(year: Number) -> out: Number
```

The time value of a date and time in UTC: Date.UTC(year, month, date, hours, minutes, seconds, ms), month counting from 0, the parts after the year optional (month 0, date 1, the rest 0); a year 0 to 99 is 1900 plus it. Takes the parts after the year with +.

### `date_set`

```
date_set(time: Any, value: Number) -> out: Number  {part: year}
```

A date with parts replaced, from part on, the rest its own: new Date(t).setUTCHours(h, m, s, ms) and the other setUTC… methods, as the time value they give. The date is a time value or a text, as for new Date(t). A part past its end carries (hour 25 is 1 the next day).

### `date_text`

```
date_text(time: Any, offset: Any) -> out: Text  {format: iso}
```

A date as text, by format: iso (new Date(t).toISOString(): 2000-02-29T12:00:00.000Z), date (2000-02-29) or utc (toUTCString(): Tue, 29 Feb 2000 12:00:00 GMT). Given an offset (+01:00, or minutes east of UTC), iso and date are the time there, iso written with it: 2000-02-29T13:00:00.000+01:00. The date is a time value or a text, as for new Date(t).

### `date_parse`

```
date_parse(text: Text) -> out: Number
```

The time value a date's text gives: Date.parse(t), in JavaScript's date format (2000-02-29, 2000-02-29T12:00:00Z, …). A time without Z or an offset produces nothing: Holon has no local time.

### `date_add`

```
date_add(time: Any, amount: Number, offset: Any) -> out: Number  {unit: days}
```

A date amount days, months or years later (earlier when negative), by unit, as a time value. A month keeps the day of the month, or is the month's last day when it has fewer: January 31 and a month is February 28. Given an offset, its calendar's months.

### `business_day_add`

```
business_day_add(date: Any, days: Number, holidays: List<Any>) -> out: Text  {weekend: sat_sun}
```

The date days business days after date (before it when negative), as YYYY-MM-DD; with 0, date itself when it is a business day, else the next. A business day is in neither the weekend (sat_sun by default, fri_sat or none) nor holidays, a list of dates such as 2026-12-25.

### `business_days_between`

```
business_days_between(from: Any, to: Any, holidays: List<Any>) -> out: Number  {weekend: sat_sun}
```

How many business days come after from up to to, to included; negative when to is before from. Dates are YYYY-MM-DD texts or time values; holidays, a list of dates, and the weekend as for business_day_add.

### `is_business_day`

```
is_business_day(date: Any, holidays: List<Any>) -> out: Boolean  {weekend: sat_sun}
```

Whether a date (YYYY-MM-DD, or a time value's date in UTC) is in neither the weekend (sat_sun by default, fri_sat or none) nor holidays, a list of dates.

### `sha256`

```
sha256(text: Any) -> out: Text  {encoding: hex}
```

The SHA-256 digest of a text (its UTF-8) or of bytes, as hex or base64.

### `hmac_sha256`

```
hmac_sha256(key: Any, message: Any) -> out: Text  {encoding: hex}
```

The HMAC-SHA256 signature of message under key, each a text or bytes, as hex or base64: how webhooks are signed.

### `hex`

```
hex(text: Text) -> out: Text  {mode: encode}
```

Encodes text as hex, or decodes hex back to text.

### `base64`

```
base64(text: Text) -> out: Text  {mode: encode, alphabet: standard, bytes: utf8}
```

Encodes text as base64, or decodes it back; standard or URL-safe alphabet. Its bytes are the text's UTF-8, or with latin1 each character one byte: btoa(s), atob(s).

### `secure_equals`

```
secure_equals(a: Any, b: Any) -> out: Boolean
```

Whether two texts, or two byte strings, are equal, compared in constant time: for checking signatures and tokens without leaking where they differ.

### `text_to_bytes`

```
text_to_bytes(text: Text) -> out: Bytes  {encoding: utf8}
```

A text's bytes: its UTF-8, or what its hex, base64 or base64url says (nothing if it isn't that). new TextEncoder().encode(s), Uint8Array.fromHex(s), Uint8Array.fromBase64(s).

### `bytes_to_text`

```
bytes_to_text(bytes: Bytes) -> out: Text  {encoding: utf8}
```

Bytes as a text: read as UTF-8 (nothing if they aren't), or written as hex, padded base64 or unpadded base64url. new TextDecoder().decode(b), b.toHex(), b.toBase64().

### `bytes_length`

```
bytes_length(bytes: Bytes) -> out: Number
```

How many bytes: b.length.

### `bytes_concat`

```
bytes_concat(a: Bytes, b: Bytes) -> out: Bytes
```

Bytes joined, in order.

### `bytes_to_list`

```
bytes_to_list(bytes: Bytes) -> out: List<Number>
```

Bytes as a list of numbers from 0 to 255: Array.from(b).

### `list_to_bytes`

```
list_to_bytes(list: List<Any>) -> out: Bytes
```

A list of whole numbers from 0 to 255 as bytes: new Uint8Array(xs). Nothing if an item isn't one, where JavaScript wraps it.

### `decimal_add`

```
decimal_add(a: Any, b: Any) -> out: Text
```

The exact sum of decimals written as text (1234.56), or of numbers as their shortest text: 0.1 and 0.2 give 0.3. Its scale is the largest of theirs: 1.10 and 2 give 3.10. Nothing for a text that isn't a decimal.

### `decimal_sub`

```
decimal_sub(a: Any, b: Any) -> out: Text
```

a − b, exactly, its scale the larger of theirs.

### `decimal_mul`

```
decimal_mul(a: Any, b: Any) -> out: Text
```

The exact product of decimals, its scale the sum of theirs: 19.99 times 3 is 59.97.

### `decimal_div`

```
decimal_div(a: Any, b: Any, scale: Number) -> out: Text  {rounding: half_even}
```

a ÷ b to scale places, rounded by rounding (half_even by default). Nothing when b is zero.

### `decimal_round`

```
decimal_round(a: Any, scale: Number) -> out: Text  {rounding: half_even}
```

A decimal to scale places, padded with zeros (5 to 2 places is 5.00), rounded by rounding: half_even (a bank's, the default), half_up, half_down, up (away from zero), down (toward zero), ceiling or floor.

### `decimal_sum`

```
decimal_sum(list: List<Any>) -> out: Text
```

The exact sum of a list's decimals: a control sum. 0 for an empty list.

### `decimal_compare`

```
decimal_compare(a: Any, b: Any) -> out: Number
```

−1, 0 or 1 as a is less than, equal to or more than b, by value: 1.10 and 1.1 are equal.

### `csv_parse`

```
csv_parse(text: Text) -> out: List<Any>  {header: first_row, separator: ,}
```

Reads CSV (RFC 4180): with header first_row (the default), a list of records keyed by the first row's names; with none, a list of lists. Every field is a text. Nothing for a malformed file: a quote that never closes, a row with more or fewer fields than the header, a column named twice.

### `csv_format`

```
csv_format(rows: List<Any>, columns: List<Text>) -> out: Text  {header: first_row, separator: ,}
```

Writes CSV: given columns, each row a record written in that order (a missing field empty), the names first unless header is none; without, each row a list. A field holding the separator, a quote or a line break is quoted, and each line ends with CRLF.

### `xml_parse`

```
xml_parse(text: Text) -> out: List<Any>
```

Reads XML 1.0 into its root element, a list as JsonML writes one: the name, then a record of its attributes when it has any, then its children in order, each an element or a text. Names are kept as written, a prefix too; comments, instructions and the white space between elements are dropped. Nothing for text that isn't well-formed, or that has a DOCTYPE: no entity is ever defined or fetched.

### `xml_format`

```
xml_format(element: List<Any>) -> out: Text  {declaration: utf8, layout: compact}
```

Writes an element, a list as xml_parse gives one, as XML: the declaration first unless declaration is none, on one line or indented two spaces a level (layout). Texts and attributes are escaped, a number or a Boolean written as JavaScript writes it. Nothing for a name that isn't an XML name, a character XML can't hold, or an item that is neither an element, a text, a number nor a Boolean.

### `xml_find`

```
xml_find(element: List<Any>, path: Text) -> out: List<Any>
```

The elements along a path of names separated by /, the first among the element's children, in document order; * is any name. An empty list when none matches.

### `xml_text`

```
xml_text(element: List<Any>, path: Text) -> out: Text
```

An element's text, every text in it at any depth joined, as the DOM's textContent; given a path, the first match's. Nothing when none matches.

### `xml_attribute`

```
xml_attribute(element: List<Any>, name: Text) -> out: Text
```

An element's attribute, by name; nothing when it has none so named.

### `to_text`

```
to_text(value: Any) -> out: Text
```

Any value written out as text, as a literal: a text comes out in quotes.

### `as_text`

```
as_text(value: Any) -> out: Text
```

Any value as it reads in a message: a text is itself, a number or true/false as written, a record or list as JSON. For putting a value into a text.

### `is_truthy`

```
is_truthy(value: Any) -> out: Boolean
```

Whether a value counts as true, as JavaScript's Boolean(x) says: false for false, 0, NaN, empty text and none; true for anything else, an empty list or record included. How if (x) and !x read a value that isn't true or false.

### `guard`

```
guard(cond: Boolean, value: Any) -> out: Any
```

Passes value on only when cond is true; otherwise nothing, so what follows doesn't run.

### `default`

```
default(value: Any) -> out: Any  {value: 0, type: Number}
```

Passes its input on, or the value set on the node when the input didn't arrive: how a flow fills a gap.

### `coalesce`

```
coalesce(a: Any, b: Any) -> out: Any
```

Passes a on, or b when a didn't arrive: a fallback that is itself computed, where default's is a fixed value.

### `equals`

```
equals(a: Any, b: Any) -> out: Boolean
```

Whether two values are the same.

### `fail`

```
fail(message: Text) -> out: Any
```

Nothing, with message given to the run's log as the reason: JavaScript's throw new Error(message). What reads it has no value, unless a ?? or a catch gives one.

## The outside world

Each of these is something a flow may do, and so a line of what it asks for when it is deployed. The language writes most of them as names it imports (`now`, a secret, `state.x`, a host's `get`, a collection or a folder) or a handler receives (`req`). The writes and the calls out take a `trigger` last: what starts them, the request of the route they are in, the trigger of the holon they are in, or the value an `if (x !== undefined)` around them waits for. The language ties it, so it is never an argument.

[`http_request`](#http_request) · [`http_upload`](#http_upload) · [`http_stream`](#http_stream) · [`flow_call`](#flow_call) · [`flow_start`](#flow_start) · [`clock`](#clock) · [`random`](#random) · [`secret`](#secret) · [`file_arrival`](#file_arrival) · [`await`](#await) · [`resume_token`](#resume_token) · [`approval`](#approval) · [`store_read`](#store_read) · [`store_write`](#store_write) · [`collection_get`](#collection_get) · [`collection_list`](#collection_list) · [`collection_put`](#collection_put) · [`collection_delete`](#collection_delete) · [`file_read`](#file_read) · [`file_list`](#file_list) · [`file_write`](#file_write) · [`file_delete`](#file_delete) · [`file_handle`](#file_handle) · [`file_lines`](#file_lines) · [`file_slice`](#file_slice) · [`file_copy`](#file_copy) · [`file_read_bytes`](#file_read_bytes) · [`file_create`](#file_create) · [`file_create_copy`](#file_create_copy) · [`db_query`](#db_query) · [`db_one`](#db_one) · [`db_exec`](#db_exec) · [`db_find`](#db_find) · [`db_find_one`](#db_find_one) · [`db_insert`](#db_insert) · [`db_update`](#db_update) · [`db_delete`](#db_delete) · [`channel_send`](#channel_send) · [`channel_subscribe`](#channel_subscribe) · [`egress`](#egress)

### `http_request`

```
http_request(path: Text, headers: Record, body: Any, trigger: Any) -> out: Record  {method: GET, host: example.com}
```

Calls the host set on the node (HTTPS) with the method, path, headers and body, and answers {status, headers, body}. Performed by the server between rounds, within the deployment's limits; the host is a permission it asks for. Holds back when a wired input didn't arrive.

### `http_upload`

```
http_upload(path: Text, headers: Record, file: Record, trigger: Any) -> out: Record  {method: PUT, host: example.com}
```

Calls the host set on the node like http_request, with the file a handle names as the body: the server streams its bytes from the folder (one the deployment reads), never through the flow, and fails the call if the file changed since the run read it.

### `http_stream`

```
http_stream(path: Text, headers: Record, body: Any, trigger: Any) -> out: Record  {method: POST, host: example.com, pipe}
```

Calls the host set on the node like http_request, and the server forwards the answer's body to the route's caller as it arrives (a 2xx answer; the reply is 200 with the answer's content type): a model's tokens, say, as they come. With pipe set to a channel's name, the pieces go to that channel's subscribers instead, as they arrive (a shared live transcript). The flow still gets the whole answer, as http_request's.

### `flow_call`

```
flow_call(input: Any, trigger: Any) -> out: Record  {flow: billing, action: charge}
```

Runs the action set on the node of the deployed flow set on the node, with the input (a value; {} when none is wired), and answers {answer, run} when it ran, or {error, message} when it didn't (not_found, not_running, no_action, waiting, failed, too_deep, refused). Performed by the server between rounds, in-process, as an ordinary action run of the other flow, logged there as run by this flow; the pair is a permission it asks for. Holds back when a wired input didn't arrive.

### `flow_start`

```
flow_start(input: Any, trigger: Any) -> out: Record  {flow: billing, action: charge}
```

Starts the action set on the node of the deployed flow set on the node (this flow's own, or another's), with the input (a value; {} when none is wired), and doesn't wait for it to end: the server runs it in-process until it finishes or first waits, and answers then, {answer, run} when it finished, {run, waiting} when it waits (waiting names each point it waits at, with the token that resumes it), or {error, message} when it didn't run. Never waits itself, so a route hands out the link of a run that waits. The pair is a permission it asks for. Holds back when a wired input didn't arrive.

### `clock`

```
clock() -> out: Number
```

The time the run started, in seconds since 1970. The same everywhere in one run, and recorded, so a replay sees it too.

### `random`

```
random() -> out: Number
```

A random number from 0 up to 1, the same everywhere in one run, and recorded.

### `secret`

```
secret() -> out: Text  {name: api_key}
```

A secret's value, by the name set on the node, from the deployment's secrets (set under ⚙). Never logged: it shows as ‹secret:name›. Reading it is a permission.

### `file_arrival`

```
file_arrival() -> out: Record  {folder: files, on: new, matching}
```

The file in a folder that started the run: its handle {folder, name, size, hash, modified, event}, event new or changed; for a file deleted (on: deleted), as last seen, without a hash. A flow with one runs once for each file that arrives there and settles (or, on: deleted, is removed), its name matching the glob matching if set, not on requests.

### `await`

```
await() -> out: Any  {name: event, expires: 604800}
```

Waits for an event: the run parks here and continues, perhaps days later, when the event arrives (through its resume_token), or answers {expired: true} after expires seconds.

### `resume_token`

```
resume_token() -> out: Text  {await: event}
```

The token that delivers an event to the waiting point (await) named on the node: put it in a link or a callback. Never logged.

### `approval`

```
approval() -> out: Record  {name: approval, expires: 604800, people: 1}
```

Waits for people to decide: the run parks here until as many different people as people says (1 to 5), signed in to Holon, approve it, or one declines it, and answers {approved, by} (by: who approved), with declined_by and reason when declined, or expired: true after expires seconds. Only the server delivers it: it has no token, and no route resumes it. Waiting for it is a permission.

### `store_read`

```
store_read() -> out: Any  {key: value}
```

The deployment's state under the key set on the node, as the run started. Reading it is a permission.

### `store_write`

```
store_write(value: Any, trigger: Any) -> out: Effect  {key: value}
```

Sets the deployment's state under the key set on the node to value, when the run finishes. Writing it is a permission.

### `collection_get`

```
collection_get(key: Text) -> out: Any  {collection: items}
```

The item of a collection under key, as the run found the collection; nothing when there is none. Reading it is a permission.

### `collection_list`

```
collection_list(prefix: Text, after: Text) -> out: Record  {collection: items, limit: 100}
```

A record of a collection's items, by key, in key order, up to limit: those whose key starts with prefix, after the key after (both optional, for paging).

### `collection_put`

```
collection_put(key: Text, value: Any, trigger: Any) -> out: Effect  {collection: items}
```

Puts value into a collection under key when the run finishes (added, or replaced). Writing it is a permission.

### `collection_delete`

```
collection_delete(key: Text, trigger: Any) -> out: Effect  {collection: items}
```

Deletes a collection's item under key when the run finishes.

### `file_read`

```
file_read(name: Text) -> out: Text  {folder: files}
```

The text of the file name in a folder, as the run found it; nothing when there is none. The folder is a name an admin binds to a real folder. Reading it is a permission.

### `file_list`

```
file_list(prefix: Text, after: Text) -> out: Record  {folder: files, limit: 100}
```

A record of a folder's files (name, size, hash, modified) in name order, up to limit: those whose name starts with prefix, after the name after (both optional, for paging).

### `file_write`

```
file_write(name: Text, text: Text, trigger: Any) -> out: Effect  {folder: files}
```

Writes text to the file name in a folder when the run finishes (created, or replaced whole). Writing it is a permission.

### `file_delete`

```
file_delete(name: Text, trigger: Any) -> out: Effect  {folder: files}
```

Deletes the file name in a folder when the run finishes.

### `file_handle`

```
file_handle(name: Text) -> out: Record  {folder: files}
```

A handle to the file name in a folder, of any size, without its bytes: {folder, name, size, hash, modified}; nothing when there is none. Copy it to another folder, or pass it on.

### `file_lines`

```
file_lines(name: Text, from: Number) -> out: List<Text>  {folder: files, limit: 100}
```

Up to limit lines of the file name in a folder, from line from (0 is the first; -100 is the 100th from the end), without their line endings. Only those lines are read into the flow; the log proves them part of the file.

### `file_slice`

```
file_slice(name: Text, offset: Number, length: Number) -> out: Text  {folder: files}
```

length bytes of the file name in a folder from byte offset (from the end when negative), as text. Only those bytes are read into the flow; the log proves them part of the file.

### `file_copy`

```
file_copy(name: Text, from: Record, trigger: Any) -> out: Effect  {folder: files}
```

Copies the file a handle names into this folder as name when the run finishes, host to host: its bytes never pass through the flow. Fails if the file changed since the run read it.

### `file_read_bytes`

```
file_read_bytes(name: Text) -> out: Bytes  {folder: files}
```

The bytes of the file name in a folder, any file, as the run found it; nothing when there is none. folder.bytes(name). Reading it is a permission.

### `file_create`

```
file_create(name: Text, text: Text, trigger: Any) -> out: Effect  {folder: files}
```

Writes text to the file name in a folder when the run finishes, only if no file has that name then: it never replaces one (the run answers 500 if one is there).

### `file_create_copy`

```
file_create_copy(name: Text, from: Record, trigger: Any) -> out: Effect  {folder: files}
```

Copies the file a handle names into this folder as name when the run finishes, only if no file has that name then: it never replaces one.

### `db_query`

```
db_query(params: List<Any>) -> out: List<Record>  {database: db, sql: select * from t}
```

The rows sql selects from a database, as records by column name, as the run found them; params are its $1, $2, …. The database is a name an admin binds. Reading each table it names is a permission.

### `db_one`

```
db_one(params: List<Any>) -> out: Record  {database: db, sql: select * from t}
```

The first row sql selects from a database, as a record by column name; nothing when there is none. params are its $1, $2, ….

### `db_exec`

```
db_exec(params: List<Any>, trigger: Any) -> out: Effect  {database: db, sql: delete from t where false, expect}
```

Runs sql (an insert, update or delete) on a database when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, …. With expect, it must change exactly that many rows, or nothing the run did there lands.

### `db_find`

```
db_find(params: List<Any>) -> out: List<Record>  {database: db, collection: items, filter: {}, sort, limit}
```

The documents filter selects from a collection of a document database (MongoDB), as records, as the run found them; params are its $1, $2, …. sort orders them and limit caps how many. Reading the collection is a permission.

### `db_find_one`

```
db_find_one(params: List<Any>) -> out: Record  {database: db, collection: items, filter: {}, sort}
```

The first document filter selects from a collection, as a record; nothing when there is none. params are its $1, $2, ….

### `db_insert`

```
db_insert(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, document: {}}
```

Adds document to a collection when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, ….

### `db_update`

```
db_update(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, filter: {}, update: {"$set":{}}, expect}
```

Applies update (its $set, $inc, … operators) to the documents filter selects, when the run finishes; params are their $1, $2, …. With expect, it must match exactly that many documents, or nothing the run did there lands.

### `db_delete`

```
db_delete(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, filter: {}, expect}
```

Deletes the documents filter selects when the run finishes; params are its $1, $2, …. With expect, it must delete exactly that many, or nothing the run did there lands.

### `channel_send`

```
channel_send(pack: Any, trigger: Any) -> out: Effect  {channel: updates}
```

Sends pack on the channel set on the node once the run is over and its writes have landed: every caller subscribed to the channel gets it as a server-sent event. Sending on it is a permission.

### `channel_subscribe`

```
channel_subscribe() -> out: Effect  {channel: updates, from: new}
```

A subscription to the channel set on the node, for a route to answer with: the caller gets each pack sent on it from then on (from "kept": the packs the channel keeps first), as server-sent events. Letting callers subscribe is a permission.

### `egress`

```
egress(payload: Any, trigger: Any) -> out: Effect
```

Sends its payload out of the flow as a message, when the run finishes.

## Templates

Templates are written through the language's own forms (a route, `respond`, `stripe.verify`),
not called by name.

### `request`

A route: answers requests with the method and path set on the node (path params like :id). Gives the whole request, and its params, query, headers, raw body, and body parsed as JSON. With a respond it wires to, one route of the flow's server.

### `respond`

Answers the route's request: body as the response, with the status set on the node (a record or list is sent as JSON). When body didn't arrive, it answers the missing status instead (404 unless set) with an error.

### `stripe_verify`

Checks a Stripe webhook's signature as Stripe's own libraries do: the HMAC-SHA256 of the timestamp and raw body under the endpoint's secret, compared in constant time with each v1 signature (any one matching passes, as while a secret is rolled), the timestamp within tolerance seconds of now. Gives the event (the body parsed) only when it verifies; otherwise nothing, so a respond refuses it.

### `every`

A clock: a run every seconds set on the node, with now, the time of the tick. Everything wired to it is that run: its writes and calls out are tied to it. Beside a flow's routes, it shares their state.

### `on_file`

A file handler: a run for each file that settles in the folder set on the node (or, on: deleted, is removed from it), its name matching the glob matching if set, with file, its handle {folder, name, size, hash, modified, event}. Everything wired to it is that run: its writes and calls out are tied to it. Beside a flow's routes, it shares their state.

### `action`

An action: a run by hand, by the name set on the node, with input, what was sent (a JSON value; {} when nothing). A respond it wires to gives the answer. Everything wired to it is that run: its writes and calls out are tied to it. Beside a flow's routes, it shares their state.
