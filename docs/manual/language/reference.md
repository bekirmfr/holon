# Language reference

The full rules of the Holon language. For an introduction, read the [tour](tour.md) first. The
building blocks you can call are listed in [leaves](leaves.md). The syntax is TypeScript's; how
each construct reads, and why, is in
[`docs/L4s/typescript-syntax-L4.md`](../../L4s/typescript-syntax-L4.md).

- [Files](#files)
- [Imports and settings](#imports-and-settings)
- [Names and literals](#names-and-literals)
- [Statements](#statements)
- [Expressions](#expressions)
- [Calls](#calls)
- [The outside world](#the-outside-world)
- [Routes](#routes)
- [Clocks, files and actions](#clocks-files-and-actions)
- [What a handler holds](#what-a-handler-holds)
- [Functions](#functions)
- [JavaScript functions](#javascript-functions)
- [map and reduce](#map-and-reduce)
- [How a flow runs](#how-a-flow-runs)
- [Types](#types)
- [Identity and canonical form](#identity-and-canonical-form)
- [Mistakes](#mistakes)

## Files

A flow is a `NAME.holon.ts` file: a TypeScript module, UTF-8, lines ending in `\n` or `\r\n`. A
file is its imports, then the one `flow(…)` statement that says what it is, then its functions and
its handlers (routes, clocks, file handlers and actions):

```ts
// A comment runs from // to the end of the line.
import { flow, route } from "holon"

flow("example", { port: 8200 })

route.get("/ping", () => "pong")
```

Every editor, GitHub and Prettier read it as TypeScript. It doesn't run as a Node program: the
`"holon"` import is Holon's, and the meanings that differ from JavaScript's are listed under
[How a flow runs](#how-a-flow-runs).

**Types for the editor.** `holon new` writes `holon.d.ts` and a `tsconfig.json` beside the flow
(`holon types` writes them into a folder), and TypeScript's own checker, the one in every editor,
then reads the flow: it completes `route.post` and a host's `get`, shows a leaf's signature on
hover, and marks a misspelled method or option, a path without its `/`, a duration without its
unit, an import source Holon hasn't, and a request read before it is awaited. A value is `any` to
it, as Holon's absence goes through `.`. Where the two disagree, Holon's compiler decides. One
place they always do: TypeScript calls the right side of `{ total } ?? status(400)` or
`a / b ?? status(400)` unreachable ("the left operand is never nullish"), as in JavaScript a record
or a quotient is never `undefined`. In Holon a record with a field that has no value has none, and
so does a division by zero, so that `??` is reached; no setting tells TypeScript so.

`holon fmt` lays a file out as Prettier does, at 100 columns and without semicolons, keeping its
comments and blank lines, `js()` bodies included. It writes a text only once that text compiles to
the same flow.

## Imports and settings

An import names something the flow uses from outside a run. Its source says what it is:

| Import | Names |
|---|---|
| `import { flow, route, status } from "holon"` | the vocabulary (`flow`, `route`, `every`, `on`, `action`, `js`, `respond`, `status`, `stripe`, `event`, `approval`) and the [leaves](leaves.md) |
| `import key from "secret:NAME"` | a secret set for the deployment, read as `key`: someone sets it before the flow works |
| `import key from "secret:NAME" with { optional: "true" }` | a secret the flow can run without; the parts that need it have no value when it isn't set |
| `import state from "state"` | the flow's state: `state.KEY` reads a key, `state.KEY = …` writes it |
| `import notes from "collection:NAME"` | a collection, used as `notes[key]` |
| `import inbox from "folder:NAME"` | a named folder on the host, used as `inbox[file]` |
| `import shop from "database:NAME"` | a named database on the host, used as `shop.query(sql, …)` |
| `import updates from "channel:NAME"` | a channel: `updates.send(x)`, `updates.subscribe()`, `updates.pipe(…)` |
| `import ops from "mail:NAME"` | a mail the server sends through, bound by an admin: `ops.send({ to, subject, text })`; bound to a mailbox, `on.mail(ops, (m) => …)` runs on what comes to it |
| `import prices from "feed:NAME"` | a feed of server-sent events, a WebSocket or a Postgres channel, bound by an admin: `on.feed(prices, (e) => …)` runs on each event it sends |
| `import payouts from "queue:NAME"` | an Amazon SQS queue, bound by an admin: `payouts.send({ data, group })` sends to it once the run is over; `on.queue(payouts, (m) => …)` runs on each message it holds |
| `import api from "host:HOST"` | a host the flow calls over HTTPS, as `await api.get("/path")` |
| `import { now } from "holon:clock"` | the time the run started |
| `import { random } from "holon:random"` | one random number for the run |
| `import { random_bytes } from "holon:random"` | the run's 32 random bytes |
| `import { charge } from "flow:SLUG"` | an action of another deployed flow, by its slug (see [The outside world](#the-outside-world)) |

An import names; it grants nothing. What the flow may do is always worked out from what it does
with what it imports: a collection the flow only reads asks only to read it. A name used without
its import is an unknown name, and the mistake says so. Imports are static, at the top, with a
literal source: a secret or a host can't come from a value.

**Settings.** `flow(NAME, { … })` comes once, after the imports. Its name is what the flow deploys
as; its options are the deploy settings:

| Option | Meaning |
|---|---|
| `port: 8102` | A flow with routes: the port its server answers on. |
| `keep: { runs: 500, days: 30 }` | How many runs the deployment's log keeps, and for how many days; either alone works. The deploy settings' **runs the log keeps** and **days the log keeps runs** (see [Checking runs](../guides/checking-runs.md)). |
| `limits: { run_budget_ms: "1m", request_timeout_ms: "50s", max_requests: 100 }` | Any other limit of the deploy settings, by its name there (see [Permissions](../reference/permissions.md#limits)). A `_ms` or `_seconds` limit takes a duration as text or a whole number in its unit, the others a whole number. More than the host's default asks for approval. |
| `browsers: ["https://app.example"]` | The web origins allowed to call the flow from a browser (CORS), or `"*"`. One origin may be written alone, `browsers: "*"`. |
| `taint: ["x-api-key", "number"]` | Keys whose values the server never keeps of the flow's runs (see **Tainted keys**, below). |
| `tags: ["payments"]` | The flow's tags: lowercase letters, digits and `-`, up to 40 of them each. Whoever is bound on a tag acts on every flow deployed with it ([People and access](../guides/people.md)). A version that adds a tag asks for approval, and the review says who the tag lets in. |

**Tainted keys.** The values of the keys a flow taints are kept out of everything the server keeps
of its runs, wherever a run has them, whatever started it: a request's header of that name (any
case), query or path parameter, or a JSON body's field at any depth; a WebSocket's message; an
action's input; what a call sends and what the far side answers; the run's own answer; a row it
writes or reads. The run computes with them and the caller is answered as always; the log, the
run's lines, its export, a replay and `holon verify` show `‹taint:number›` where each was, and a
value computed from one isn't checked again, as with a secret ([Checking
runs](../guides/checking-runs.md)). Every flow taints `authorization`, `proxy-authorization`,
`cookie` and `set-cookie` without saying so: a bearer token, a Basic password, each cookie a caller
sends and the one a far side sets. A value is found by its key, and then kept out wherever the run
has it, under any key or inside text. One shorter than six characters is kept as it is (a CVC, a
PIN: too short to find safely in other text). What the flow writes stays as written in its state,
a collection, a database or a file: a later run that reads it back under a tainted key is logged
without it, one that reads it under another key isn't.

## Names and literals

**Identifiers** are TypeScript's. A name bound in a handler is visible after its binding, in that
handler; functions and top-level `const`s are visible to the whole file.

**No globals.** JavaScript's globals aren't there: a flow has no `fetch` or `setTimeout`, and the
mistake says to import what it uses. `JSON`, `Object`'s record functions, `Math`, `Number`,
`parseInt`, `parseFloat`, `isNaN`, `isFinite`, `structuredClone` and `Date` in UTC are there, as
the leaves that do them (see [Numbers](#numbers), [Records](#records) and [Dates](#dates)). `NaN`
and `Infinity` aren't values: where JavaScript gives one, a leaf gives no value.

| Literal | Examples | Notes |
|---|---|---|
| Number | `0`, `86400`, `-1`, `2.5`, `1e3`, `1_000_000` | finite decimals; no infinity or NaN |
| Text | `"hello"`, `'line\n'`, `` `Hello, ${name}` `` | `${expr}` puts a value in |
| Duration | `"10s"`, `"5m"`, `"2h"`, `"1d"` | a text, where one is asked for: `every("10s", …)`, `expires: "1d"`, a `_ms` limit |
| Boolean | `true`, `false` | |
| No value | `undefined` | as a branch, `c ? a : undefined`, and in `x !== undefined` |
| Record | `{ title: t, "content-type": "text/html" }`, `{ title }` | a key that isn't a plain word is quoted; `{ title }` is `{ title: title }` |
| Record, extended | `{ ...base, extra: v }` | `base`'s fields, plus or replacing `extra`; one base, first |
| List | `[a, b, c]`, `[]`, `[...xs, x]` | |

Interpolation takes any value: a text stays itself, a number or boolean is written as it reads,
and a record or list is written as JSON.

## Statements

| Statement | Meaning |
|---|---|
| `const name = expr` | Binds a name. |
| `let name = expr`, `name = expr` | A name that may be bound again: each assignment is a new value, and where branches assign it, the value is the branch taken's. `var` is read as `let`. |
| `n++`, `n += 2`, `n %= 3`, `x ??= y` | An assignment of `n + 1`, `n + 2`, `n % 3`, `x ?? y`; each of TypeScript's compound operators the same way. `++` and `--` count from 0 where `n` has no value yet, so `state.hits++` starts a count. A state key changed twice in a run is refused: it reads as the run began. |
| `const { a, b } = r`, `const [x, y] = xs` | Destructuring: a field each, an index each; `{ a = 1 }` takes a default, `[first, ...rest]` the head and the tail. |
| `state.key = expr` | Writes the flow's state under `key` when the run finishes. |
| `coll[key] = expr` | Writes an item of a collection (or a file of a folder) when the run finishes. |
| `delete coll[key]` | Deletes an item (or a file) when the run finishes. |
| `if (c) …; else …` | Chooses: writes in a branch happen only when it is taken, and writes to one key in both branches are one write. |
| `if (x !== undefined) { … }` | Its writes and calls tied to `x` (see [What a handler holds](#what-a-handler-holds)). |
| `switch (s) { case …: … }` | Chooses by value (see below). |
| `try { … } catch { … } finally { … }` | Runs the catch block where something in the try block failed, and the `finally` block either way (see below). |
| `throw new Error(message)` | A failure: nothing, its message in the run's log (see below). |
| `return …` | What a route or an action answers (see [Routes](#routes)). |
| `expr` | An effect, such as `notes.put(k, v)` or an `await api.post(…)` whose answer isn't used: a call is made for what it does, read or not. |

An expression statement that isn't an effect is a mistake ("a value nobody uses"): bind it or
remove it. A binding that is never used is a warning. A run reads its starting state and writes
once, at its end: a handler reads a collection's item it writes before the write, so reading one
after writing to it is a mistake. Two handlers may each write the same key.

**`switch`.** Each `case` is a literal (a number, a text, `true` or `false`), several cases may
share a body, and `default` comes last. Each body returns or assigns; there is no fall-through
into another body:

```ts
import { flow, route } from "holon"

flow("switch-example", { port: 8201 })

route.post("/size", (req) => {
  switch (req.json.size) {
    case "s":
    case "small":
      return 1
    case "m":
      return 2
    case "l":
    case "large":
      return 3
    default:
      return 0
  }
})
```

The first case equal to the subject is chosen, and only its body runs. If the chosen case has no
value, the `switch` has none: it never falls through to another case. With no case equal and no
`default`, or with the subject absent, the `switch` has no value.

**`try`, `catch` and `throw`.** A failure is a value that didn't arrive (see
[How a flow runs](#how-a-flow-runs)), and `throw new Error(message)` makes one: nothing, with the
message in the run's log. The try block succeeds when every value it makes arrives; its writes are
made only then, all together, and otherwise the catch block runs instead. `finally` runs either
way:

```ts
import { flow, route, status } from "holon"
import notes from "collection:notes"
import state from "state"

flow("try-example", { port: 8208 })

function checked(note) {
  if (note.text === "") throw new Error("a note has some text")
  return note
}

route.post("/notes", (req) => {
  try {
    const note = checked(JSON.parse(req.body))
    notes[note.id] = note.text
  } catch {
    return status(400, { error: "not a note: the run's log says why" })
  } finally {
    state.tried = (state.tried ?? 0) + 1
  }
  return status(201, "kept")
})
```

A body that isn't JSON, a note with empty text and a note without an `id` each answer 400, and no
note is kept; every request counts. A throw nothing catches ends the handler with nothing: a route
answers 404, or what its `?? status(…)` says, and a function gives nothing to its caller. Where
this differs from JavaScript:

- **`catch` catches absence too.** A field the try block reads that isn't there is a failure,
  where JavaScript carries on with `undefined`. Give it a fallback inside the block
  (`req.json.kind ?? "plain"`) when its absence isn't one.
- **The try block's writes are all or none.** JavaScript keeps the writes made before the throw.
- **A request the try block makes is made when what it reads arrives,** even after a failure that
  it doesn't read. If it fails, the catch block runs.
- **The catch block sees names as they were before the try,** not as the try block left them.
- **`catch (e)` is refused:** a failure's reason is the run log's, not a value; write
  `catch { … }`. A `return` or a `throw` in `finally` is refused too.

## Expressions

From lowest to highest precedence; binary operators group left to right. A condition (`if (c)`,
`c ? a : b`, `!c`) holds as it would in JavaScript: `false`, `0`, `NaN` and `""` don't, anything
else does, an empty list or record included. A `c` with no value doesn't hold, and `!c` has none.

| Form | Meaning |
|---|---|
| `c ? a : b` | `a` when `c` holds, otherwise `b` (`c` with no value too); if the side chosen has no value, neither has the whole |
| `c ? a : undefined` | `a` when `c` holds; otherwise no value |
| `a ?? b` | `a`, or `b` when `a` has no value |
| `a \|\| b`, `a && b`, `!a` | logic: `!a` is `true` where `a` doesn't hold; `&&` and `\|\|` take booleans, but in a condition read their operands as a condition does; a text or number after `\|\|` is refused, with "use `??`" |
| `a \| b`, `a ^ b`, `a & b` | bitwise or, xor and and, on 32-bit integers |
| `a === b`, `a !== b` | structural equality of any two values, never converted: `1` is not `"1"`. `==` and `!=` mean the same, and `fmt` writes `===` |
| `a < b`, `<=`, `>`, `>=` | comparison, of two numbers or of two texts (in UTF-16 code units, as JavaScript compares them) |
| `a << b`, `a >> b`, `a >>> b` | shifts, on 32-bit integers; `>>>` gives one without a sign |
| `a + b`, `a - b`, `a * b`, `a / b` | arithmetic, on numbers; `+` with a text on either side joins the two, a number written as JavaScript writes it: `"n=" + 1` is `"n=1"` |
| `a % b`, `a ** b` | the remainder, with `a`'s sign, and `a` to the power `b`; `**` groups right to left |
| `-a`, `+a`, `~a` | `a * -1`; `a` as a number, `Number(a)`; the bits of `a` flipped |
| `n++`, `n--`, `++n`, `--n` | `n` changed as the statement does, before the statement it is in: `n++` is the value it had, `++n` the one it has after, read left to right (`[i++, i++, i]` is `[0, 1, 2]`); none yet counts as 0. Refused after `?`, `&&`, `\|\|` or `??` and in a function written in place, where a statement of its own does it |
| `f(…)` | a call (see [Calls](#calls)) |
| `a.name`, `a?.name` | an output named `name`, else the field `name` of a record; `?.` means the same |
| `a[e]` | with a number, a list's item (nothing past its end, or for a negative one); on a record, the field whose name is `e`'s value; on a collection or folder, an item or a file, a number naming one by its text (`beats[3]` is `beats["3"]`). A text's character is `s.at(i)` or `s.charAt(i)` |
| `(e)` | grouping |

**Choices only run what they need.** In `?:`, `??`, `if` and `switch`, the branch that isn't
taken isn't computed: its requests aren't sent, its reads aren't made, its `await` doesn't park
the run. `select(c, a, b)`, written as a call, is the strict form that needs both.

**Outputs and fields.** Some values have several outputs: a request, a verified Stripe event, a
function with several outputs. `v.name` is the output called `name` if there is one, else the
field `name` of `v`'s first output. Used bare, such a value is its first output.

## Calls

```
leaf(input, input, { name: value, … })
```

Positional arguments are the unit's **inputs**, in order. A last record, when the leaf takes each
of its keys, names inputs and **parameters**, fixed in the flow:

```
api.post(path, { headers: h, body: b })    // headers, body: inputs
hmac_sha256(key, text, { encoding: "hex" })   // encoding: a parameter
```

- A leaf is imported from `"holon"` before it is called: `import { hmac_sha256 } from "holon"`.
- Some leaves take any number of inputs: `add`, `multiply`, `and`, `or`, `concat`, `list`,
  `from_codes`.
- Some inputs are optional (a request's `headers` and `body`, a collection's `.list` `prefix` and
  `after`) and may be left out. Leaving out any other input is a mistake that names it.
- Parameters not given take their defaults.
- A leaf with no inputs still takes parentheses: `record()`, `none()`.

**Methods are leaves.** A text's or list's method is the leaf that does it: `t.toUpperCase()` is
`uppercase`, `t.split(",")` `split`, `xs.join("-")` `join`, `xs.map(f)` `map`, `xs.reduce(f, 0)`
`fold`, and a text's other methods those under [Texts](#texts), a list's under [Lists](#lists), a
record's under [Records](#records) and a date's under [Dates](#dates); `JSON.parse` and
`JSON.stringify` are `json_parse` and `json_stringify`, `Object.keys` is `keys`. A method no leaf
does is refused, with "do it in a `js()` function".

### Numbers

A number is a finite double. Where JavaScript would give `NaN` or an infinity (`Math.log(0)`,
`parseInt("x")`, `0 ** -1`), the leaf gives no value, so a flow tests for one with `x !==
undefined` or gives a fallback with `??`. A number leaf given a text gives no value too: convert
it first, `Number(s)`.

| Form | Meaning |
|---|---|
| `Math.round(x)`, `Math.sqrt(x)`, `Math.sin(x)`, … | `Math`'s one-number functions, `math` with the function's name; `Math.round` rounds a half up, as JavaScript's does |
| `Math.max(a, b, …)`, `Math.min(a, b, …)` | the largest, the smallest |
| `Math.max(...xs)`, `Math.min(...xs)`, `Math.hypot(a, b)`, `Math.sumPrecise(xs)` | over a list (`aggregate`) |
| `Math.atan2(y, x)`, `Math.pow(a, b)`, `Math.imul(a, b)` | as JavaScript's |
| `Math.PI`, `Math.E`, `Number.EPSILON`, `Number.MAX_SAFE_INTEGER` | the constants, as their literals |
| `Number(x)`, `parseFloat(s)`, `parseInt(s, 16)` | a value as a number (`to_number`); `Number.parseFloat` and `Number.parseInt` are the same |
| `Number.isInteger(x)`, `Number.isFinite(x)`, `isNaN(s)`, `isFinite(s)` | tests (`number_test`); `Number.isNaN(x)` is `false`, as a flow has no NaN |
| `x.toFixed(2)`, `x.toPrecision(3)`, `x.toExponential()`, `x.toString(16)` | a number as text (`number_format`) |
| `x.toString()` | the same, when `x` is known to be a number, as an arithmetic result is; otherwise `String(x)` |

`Math.random()` is refused: a run's random number is `import { random } from "holon:random"`, one
for the run, recorded.

### Texts

A text's positions and lengths count UTF-16 code units, as JavaScript's do: `"a😀b".length` is 4.
A result that would cut an emoji in half gives no value, as a flow's text can't hold half of one.
A text leaf given anything but a text gives no value too.

| Form | Meaning |
|---|---|
| `s.length` | the length (`text_length`), when `s` is known to be a text, as a text method's result is; on a list known to be one, its length (`length`). A function's parameter written with its type (`s: string`, `xs: string[]`) is known to be one. On a value of no known type, such as a request's field, refused: call `text_length(s)` or `length(xs)` |
| `s.at(i)`, `s.charAt(i)`, `s.charCodeAt(i)`, `s.codePointAt(i)` | the character at `i` (`at`, from the end when negative; `text_at`, `""` outside the text), its code unit, its code point (`code_at`); the index left out is 0 |
| `s.slice(a, b)`, `s.substring(a, b)` | a part (`slice`), as JavaScript's: `slice` counts a negative position from the end, `substring` swaps its two |
| `s.indexOf(t, i)`, `s.lastIndexOf(t, i)`, `s.includes(t, i)` | where `t` is, or whether it is, from `i` when given; the same leaves as a list's |
| `s.startsWith(t, i)`, `s.endsWith(t, i)` | whether `s` begins or ends with `t` (`text_contains`) |
| `s.split(t, n)` | the pieces between each `t`, `n` of them at most when given; `s.split("")` each code unit |
| `s.trim()`, `s.trimStart()`, `s.trimEnd()` | without JavaScript's white space at both ends, the start or the end (`trim`) |
| `s.padStart(n, f)`, `s.padEnd(n, f)`, `s.repeat(n)` | padded to `n` with `f`, a space when left out (`pad`); `n` times over (`repeat`) |
| `s.replace(a, b)`, `s.replaceAll(a, b)` | the first or every `a` replaced (`replace`); in `b`, `$&` is what was found, `$$` a dollar |
| `s.normalize()`, `s.normalize("NFD")` | the Unicode normal form, `"NFC"` when left out (`normalize`) |
| `s.toUpperCase()`, `s.toLowerCase()`, `s.toLocaleUpperCase()`, `s.toLocaleLowerCase()` | in capitals, in small letters, by Unicode's own rules: a flow has no locale |
| `s.concat(a, b)`, `"a" + b`, `` `${a}${b}` `` | joined, numbers written as JavaScript writes them |
| `String(x)`, `x.toString()` | `x` as a template writes it (`as_text`) |
| `s.isWellFormed()`, `s.toWellFormed()` | `true`, and `s`: a flow's text is always well formed |
| `String.fromCharCode(a, b)`, `String.fromCodePoint(a, b)` | the text of those code units or code points (`from_codes`) |
| `` String.raw`a\n${x}` `` | the template's text as written, its escapes kept |
| `encodeURIComponent(s)`, `encodeURI(s)`, `decodeURIComponent(s)`, `decodeURI(s)` | a text escaped for a URL and back (`uri_encode`, `uri_decode`); a malformed escape gives no value |
| `btoa(s)`, `atob(s)` | base64 of a text whose characters are bytes, and back (`base64` with `bytes: "latin1"`) |

`s.localeCompare(t)` is refused, as a flow has no locale; `a < b` compares texts in code units.
Regular expressions (`s.match(re)`, `s.search(re)`, a pattern in `replace` or `split`) are
refused: do them in a `js()` function.

### Lists

| Form | Meaning |
|---|---|
| `xs.filter(f)`, `xs.find(f)`, `xs.findIndex(f)`, `xs.findLast(f)`, `xs.findLastIndex(f)`, `xs.some(f)`, `xs.every(f)` | as JavaScript's: `f`'s answer is read as JavaScript reads a condition, and the ones that stop early apply `f` to nothing after the item that decides |
| `xs.toSorted((a, b) => a - b)`, `xs.toSorted()` | sorted, stably; without a function, by each item as JavaScript writes it, as its `sort()` does |
| `xs.map((x, i) => …)`, `xs.reduce(f)`, `xs.reduceRight(f, start)`, `xs.flatMap(f)` | with each item's index; the first item as the start; from the end; each result's list spread |
| `xs.at(-1)`, `xs.includes(x)`, `xs.indexOf(x)`, `xs.lastIndexOf(x)` | an item, from the end when negative; whether `x` is there; where, or `-1` |
| `xs.slice(1, -1)`, `xs.toSpliced(i, n, ...items)`, `xs.with(i, v)`, `xs.toReversed()` | a part; `n` items at `i` replaced by `items`; one item replaced (no value out of range); reversed |
| `xs.flat()`, `xs.flat(2)`, `xs.flat(Infinity)` | nested lists spread, one level deep, two, all |
| `[...new Set(xs)]` | each item once, in order |
| `[...xs.keys()]`, `[...xs.entries()]` | the indices; the `[index, item]` pairs |
| `Array.isArray(x)`, `Array.of(a, b)`, `Array.from(xs, f)`, `Array.from({ length: n }, (_, i) => …)` | whether `x` is a list; `[a, b]`; `xs.map(f)`; `range(n).map((i) => …)` |

A list never changes. `xs.sort()`, `xs.reverse()`, `xs.fill(v)` and `xs.copyWithin(t, s)` are read
as the list they give, on a list made where they are called (`[...xs].sort()`) or one of the
handler's own that isn't read after; alone, or on a list something else holds, they are refused,
naming the form to write (`xs.toSorted()`). `xs.push(x)` alone is `xs = [...xs, x]`; `pop`,
`shift`, `unshift` and `splice` are refused, naming what to write (`xs.at(-1)` and
`xs.slice(0, -1)`, `[x, ...xs]`, `xs.toSpliced(…)`). Items are compared by value, where
JavaScript compares objects by identity: `[[1]].includes([1])` is `true`, and `new Set` keeps one
of two equal records.

### Records

A record's keys are kept sorted, so `Object.keys`, `Object.values` and `Object.entries` list them
in key order.

| Form | Meaning |
|---|---|
| `{ a: 1, ...r, ...s, [k]: v }` | built left to right, as JavaScript builds it: a later key replaces an earlier one (`merge` for the spreads, `insert` for a computed key) |
| `Object.values(r)`, `Object.entries(r)`, `Object.fromEntries(pairs)` | the values; the `[key, value]` pairs; a record of pairs, a later one for a key winning |
| `Object.groupBy(xs, (x) => x.kind)` | a record of lists, each item under the key its function gives (`group_by`) |
| `Object.hasOwn(r, k)`, `r.hasOwnProperty(k)`, `k in r` | whether `r` has the key (`has`) |
| `Object.is(a, b)` | whether two values are the same, `NaN` and `-0` by value (`same_value`) |
| `typeof x` | `"number"`, `"string"`, `"boolean"`, `"object"` (a record, a list or `null`), or `"undefined"` when `x` has no value (`type_of`) |
| `Object.assign({}, a, b)`, `structuredClone(x)`, `Object.freeze(x)`, `Object.create(null)` | `{ ...a, ...b }`; `x`; `x`; `{}` |
| `JSON.stringify(x, ["id", "name"], 2)`, `JSON.stringify(x, null, "\t")`, `JSON.stringify(x, (k, v) => …)`, `JSON.parse(t, (k, v) => …)` | JSON with only the keys listed, laid out with an indent; with a replacer or a reviver applied to each key and value |

A record never changes. `Object.assign(t, a)` alone on one of the handler's own records is
`t = { ...t, ...a }`; on a parameter, or into anything but a record written in place, it is
refused, naming `{ ...a, ...b }`. A record given to `Object.freeze` can't be changed after, as in
JavaScript: `r.k = v` on it is refused. A replacer's keys and an indent are written in place. Records
have no prototype or property descriptors: `Object.defineProperty`, `Object.getPrototypeOf`,
`Object.isFrozen` and `instanceof` are refused, and a flow has no `Map`.

### Dates

A date is JavaScript's `Date` in UTC: its time value, milliseconds since 1970-01-01T00:00:00Z. A
flow has no time zone, so only the UTC methods are there.

| Form | Meaning |
|---|---|
| `new Date(t).getTime()` | the time value of `t`, a number of milliseconds or a text `Date.parse` reads (`date_part`) |
| `new Date(t).getUTCFullYear()`, `.getUTCMonth()`, `.getUTCDate()`, `.getUTCDay()`, `.getUTCHours()`, `.getUTCMinutes()`, `.getUTCSeconds()`, `.getUTCMilliseconds()` | a part, the month from 0, the day 0 for Sunday (`date_part`) |
| `new Date(t).toISOString()`, `new Date(t).toUTCString()` | `2000-02-29T12:00:00.000Z`; `Tue, 29 Feb 2000 12:00:00 GMT` (`date_text`) |
| `new Date(t).setUTCHours(0, 0, 0, 0)`, `.setUTCDate(1)`, … | the time value with those parts replaced, a part past its end carried (`date_set`) |
| `Date.UTC(2000, 1, 29)`, `Date.parse("2000-02-29T12:00:00Z")` | the time value of a date's parts; of a date's text, in JavaScript's format, with `Z` or an offset after a time |
| `const d = new Date(t)`, then `d.getUTCDay()`, `d.setUTCDate(1)` | a Date held by a name, read by its methods; a setter alone, or `const ms = d.setUTCDate(1)`, changes `d` |

**At an offset, and business days.** A flow has no time zones, but it has fixed offsets, `"+01:00"`
or a number of minutes east of UTC, and calendars given as data. These are called by name,
imported from `"holon"`, since JavaScript has none of them:

| Call | Gives |
|---|---|
| `date_part(t, "+01:00", { part: "hours" })` | a part at that offset: its hours, its date (`part: "date"`), its weekday (`"day"`) |
| `date_text(t, "+01:00")`, `date_text(t, offset, { format: "date" })` | `2026-11-06T16:30:00.000+01:00`; the date alone there, `2026-11-06`. `date_text(t, { format: "date" })` is the date in UTC |
| `date_add(t, 1, { unit: "months" })` | the time value a month later (`days`, the default, `months`, `years`), at an offset when given one after the amount: January 31 and a month is February 28 |
| `business_day_add(d, n, holidays)` | the date, `YYYY-MM-DD`, `n` business days after `d` (before it when `n` is negative); with 0, `d` itself when it is a business day, else the next |
| `business_days_between(from, to, holidays)` | the business days after `from` up to `to`, `to` included; negative when `to` is earlier |
| `is_business_day(d, holidays)` | whether `d` is one |

A business day is in neither the weekend, `{ weekend: "sat_sun" }` (the default), `"fri_sat"` or
`"none"`, nor `holidays`, an optional list of dates such as `"2026-12-25"`: a bank's closing days
are the flow's data, not built in. A calendar date is a `YYYY-MM-DD` text or a time value, read as
its date in UTC; a text with a time of day is refused, since its date depends on an offset: write
`date_text(t, offset, { format: "date" })` first. An offset doesn't follow summer time: Frankfurt
is `+01:00` in winter and `+02:00` in summer, so which applies is the flow's to say. The due-dates
sample schedules payouts against a cut-off and TARGET2's closing days.

A date out of range (±8.64e15 ms) or a text that isn't one gives no value. A Date is read only by
its methods: `new Date(t)` alone, or `d` returned or put in a record, is refused, naming
`d.getTime()` and `d.toISOString()`. Local time is refused, naming the UTC method (`getHours`
names `getUTCHours`, `new Date(y, m)` names `new Date(Date.UTC(y, m))`), and `new Date()` and
`Date.now()` name the clock: `now * 1000`, `now` imported from `"holon:clock"`.

### XML

An XML element is a list, as JsonML writes one: its name, then a record of its attributes when it
has any, then its children in order, each an element or a text. A list keeps the order a schema
fixes (a pain.001's `MsgId` before its `CreDtTm`), where a record's keys would be in order of their
names, and one child or several is the same shape:

```
const amount = ["InstdAmt", { Ccy: "EUR" }, "100.00"] // <InstdAmt Ccy="EUR">100.00</InstdAmt>
const header = ["GrpHdr", ["MsgId", id], ...lines] // its children in order
```

| Call | Gives |
|---|---|
| `xml_parse(t)` | the root element; nothing for text that isn't well-formed XML, or that has a DOCTYPE |
| `xml_format(e)`, `xml_format(e, { layout: "indented" })`, `{ declaration: "none" }` | the text, the XML declaration first; indented two spaces a level; without the declaration |
| `xml_find(e, "Rpt/Pmt/Tx")` | the elements along a path of names (`*` any), from `e`'s children, in document order |
| `xml_text(e)`, `xml_text(e, "Tx/Sts")` | the element's text, every text in it joined; the first match's, or nothing |
| `xml_attribute(e, "Ccy")` | an attribute's value, or nothing |

Names are kept as written, a prefix too, and a namespace is the `xmlns` attribute that declares it.
Reading drops comments, instructions and the white space between elements, and reads the five
named entities, character references and CDATA; no entity is ever defined or fetched, so a DOCTYPE
is refused. Writing escapes what it must and writes a number as `String(n)` does. Elements nest at
most 64 deep. The payment-batch sample writes a pain.001 and reads a pain.002.

### Zip and gzip

| Call | Gives |
|---|---|
| `zip(files)` | a zip's bytes: each of the record's texts or bytes deflated under its name, in order of the names |
| `unzip(b)` | a record of each file's bytes under its name, a folder's own entry left out; nothing for bytes that aren't a whole zip |
| `gzip(x)` | a text's UTF-8, or bytes, gzipped |
| `gunzip(b)` | the bytes gzipped, each member in turn; nothing for bytes that aren't a whole gzip |

The same input gives the same bytes on every host: the deflate is pinned, and a zip's entries are
dated 1980-01-01. Reading checks every checksum and length, and gives nothing for a file that is
encrypted, zip64 or compressed other than by deflate, for a name given twice or one that would
land outside its folder (a `..` part, a leading `/`, a `\`), and past 64 MiB inflated or 10,000
files, so a zip bomb stops early. `zip` refuses the same names. The statement-archive sample
unzips deliveries of bank statements and archives each as a zip.

### JWT

| Call | Gives |
|---|---|
| `jwt_verify(token, key, now)` | the token's claims, its payload as a record, when its signature verifies with `key` and they hold at `now`; nothing else |
| `jwt_verify(token, key, now, { issuer, audience, leeway })` | the same, with `iss` equal to `issuer` and `aud` holding `audience`; `exp` and `nbf` widened by `leeway` seconds (`"0"`, `"30"`, `"60"`, `"120"`, `"300"`) |
| `jwt_decode(token)` | `{header, payload}`, verifying nothing: who a token says signed it, to pick the key that checks it |

The key decides the algorithm, never the token: a shared secret (a text that isn't PEM, or bytes,
at least 32, 48 or 64 bytes) allows HS256, HS384 or HS512; an RSA key of 2,048 to 4,096 bits
RS256 to RS512 and PS256 to PS512; a P-256 key ES256; an Ed25519 key EdDSA. A public key is a PEM
text (SPKI, or PKCS #1 for RSA) or a JWK record, whose `alg`, `use` and `key_ops` narrow it; a JWKS
(`{keys: [...]}`) gives the key the token's `kid` names, or its one key when the token names none.
So a token saying `alg: none`, or one signed with HMAC over a public key, is nothing. `exp` is
required, and `now` is a number of seconds, as `exp` is: `now` from `"holon:clock"`, so a rerun
checks the token at the time its run saw. A token with `crit`, padding or a payload that isn't an
object is nothing too. The key's taint stops at `jwt_verify`, as at `hmac_sha256`. The jwt-api
sample verifies partners' tokens with keys a person trusts.

### OpenPGP

| Call | Gives |
|---|---|
| `pgp_encrypt(data, keys, random_bytes, now)` | an OpenPGP message, as bytes: `data` (a text's UTF-8, or bytes) encrypted to each of `keys`, to its newest encryption key valid at `now`, as GnuPG reads it |
| `pgp_encrypt(data, keys, random_bytes, now, { armor: "ascii" })` | the same, armored (`-----BEGIN PGP MESSAGE-----`), as bytes to write to a file |
| `pgp_verify(data, signature, keys, now)` | `{signer, key, created}` when one of the detached `signature`'s signatures over `data` verifies with one of `keys`: the primary key's fingerprint, the signing key's, and when it signed; nothing else |
| `pgp_key(key)` | `{fingerprint, user_ids, created, expires, revoked}`, as the key's own signatures bear them out (`expires` absent when it doesn't): what a person compares before trusting it |

A key is a public key as `gpg --export` writes it, armored or bytes; `keys` is one or a list, and a
signature is armored or bytes too. Version 4 keys of the two kinds GnuPG makes are understood: RSA
of 2,048 to 4,096 bits, and Curve25519 (EdDSA to sign, ECDH to encrypt). A key's own signatures
decide what it does: a subkey counts only when its primary binds it, a signing subkey only with its
own back-signature too, and a key or subkey only while it is valid: created, not expired, not
revoked. The message is a session key for each key, then AES-256 with the modification detection
code, uncompressed, its data dated `now` and given no file name. Its every random byte is drawn
from `random_bytes` (`import { random_bytes } from "holon:random"`, the run's 32 recorded bytes),
so `holon verify` writes the same message again from the log; fewer than 32 give nothing. A
signature must be over SHA-224 to SHA-512 (SHA-1 gives nothing), made no later than `now` by a key
that could sign then, and not expired at `now`; a text signature (`gpg --textmode`) is over the
data with CR LF line endings. Other keys (NIST and Brainpool curves, DSA, ElGamal, version 6) and
messages that carry their own signature give nothing. The pgp-outbox sample encrypts payment files
to a bank's key and verifies the bank's signed reports.

## The outside world

Each of these is one building block, and each is one thing the flow may do.

| Written | What it is |
|---|---|
| `req`, `req.params`, `req.query`, `req.headers`, `req.body`, `req.json` | the request and its parts (`body` as text, `json` parsed), in a route |
| `req.path`, `req.method` | fields of the request record |
| `key` (`import key from "secret:key"`) | a secret set for the deployment |
| `state.key` | the state under `key`, as the run began |
| `state.key = e` | write it when the run finishes |
| `now` | the time the run started, in seconds since the epoch; in a clock, the tick's |
| `random` | one random number for the run |
| `random_bytes` | the run's 32 random bytes, the ones `random` is read from |
| `coll[k]`, `coll.get(k)` | the item under `k` in a collection |
| `coll[k] = e`, `coll.put(k, e)` | put it when the run finishes |
| `delete coll[k]`, `coll.delete(k)` | delete it when the run finishes |
| `coll.list({ prefix: p, after: a, limit: n })` | a page of items, in key order |
| `dir[name]`, `dir.get(name)` | a file's text, in a folder, as the run began (up to 1 MiB) |
| `dir.bytes(name)` | a file's bytes, any file, as the run began (up to 1 MiB): a `Uint8Array`, its `.length` and `.slice(start, end)` read as JavaScript's |
| `dir[name] = text`, `dir.put(name, text)` | write the whole file when the run finishes: a text as its UTF-8, bytes as they are |
| `delete dir[name]`, `dir.delete(name)` | delete the file when the run finishes |
| `dir.list({ prefix: p, after: a, limit: n })` | a page of files: `{size, hash, modified}` by name |
| `dir.file(name)` | a handle to a file of any size: `{folder, name, size, hash, modified}`, no bytes |
| `return dir.serve(name)`, `respond(dir.serve(name), { headers })` | a route's answer that is the file: the host sends its bytes, with the type its name says and its hash as its `etag`, if they are still the handle's; no such file is no value (`dir.serve(a) ?? dir.serve("index.html")`). See [Serving a folder](../guides/files.md#serving-a-folder) |
| `dir.lines(name, { from: i, limit: n })` | `n` lines from line `i` (from the end when negative) |
| `dir.slice(name, { offset: o, length: n })` | `n` bytes from offset `o`, as text |
| `dir[name] = handle`, `dir.copy(name, handle)` | copy a file by handle when the run finishes |
| `dir.create(name, text)`, `dir.create_copy(name, handle)` | the same write (a text or bytes) or copy, only if there is no such file yet: else the run fails, and nothing it did lands |
| `f` in `on.file.created(dir, (f) => …)` | the handle of the file that started the run: `{folder, name, size, hash, modified, event}` |
| `m` in `on.mail(ops, (m) => …)` | the message that started the run: `{from, name, to, cc, subject, date, message_id, text, html, attachments, authenticated, verdict}`; `authenticated` is the mailbox's own server's word that the domain in `from` sent it |
| `e` in `on.feed(prices, (e) => …)` | the event that started the run: `{data, event, id}`; `data` a value when it is JSON, else its text; `event` its name, `"message"` when it gives none; `id` only when it has one |
| `db.query(sql, v1, v2, …)` | the rows of a database, a list of records, as the run began; `$1`, `$2`, … in the SQL are the values after it |
| `db.one(sql, v1, …)` | the first row, or no value when there is none |
| `db.exec(sql, v1, …)`, `db.exec(sql, …, { expect: n })` | run an `insert`, `update` or `delete` when the run finishes; with `expect`, only if it changes exactly `n` rows, else nothing the run wrote lands and a route answers 409 |
| `await api.get(path, { headers: h, body: b })` | a request to the host `api` imports; also `post`, `put`, `patch`, `delete`, `head`. The answer is `{status, headers, body}` |
| `await api.put(path, { headers: h, file: handle })` | the same, sending a file as the body, by handle: the server streams its bytes, which never pass through the flow (any method) |
| `stripe.verify(req, secret, now, { tolerance: 300 })` | a verified Stripe webhook; `.event` is the event |
| `await charge(x)` (`import { charge } from "flow:billing"`) | run the action `charge` of the deployed flow `billing` with `x`, a value (`{}` when none is written); the answer is `{answer, run}` when it ran, `{error, message}` when it didn't |
| `payment(x)`, `charge(x)`, not awaited | start a run of this flow's action, or another flow's, with `x`; the answer is `{run, waiting}` once it waits, `waiting` the token for each point it waits at, `{answer, run}` if it finishes first, `{error, message}` when it didn't run |
| `await event(name, { expires: d })` | park the run until the event `name` arrives (in an action, not a route) |
| `await approval(name, { expires: d, people: n })` | park the run until `n` different people (1 to 5, 1 when left out), signed in to Holon, approve it, or one declines; the answer is `{approved, by}`, with `declined_by` and `reason` when declined, `expired: true` when no one decided in time (in an action, not a route) |
| `ops.send({ to, cc, subject, text, html, attachments })` | mail a message when the run is over, from the binding's sender: `to` and `cc` an address or a list, `html` beside the text, `attachments` a record of a file name to its text or bytes. A message that isn't one (a field it doesn't know, no recipient, a subject on two lines) has no value, and nothing is sent |
| `payouts.send({ data, group })` | send a message to the queue `payouts` once the run is over and its writes have landed: `data` text as it is, anything else as JSON; `group` a FIFO queue's message group. A message without `data` has no value, and nothing is sent |
| `JSON.stringify(x)`, `JSON.parse(text)` | JSON out, JSON in (see [Records](#records) for a replacer, a reviver and an indent) |

Rules:

- **A collection, folder, database or host must be imported** before it is used: its import is
  what the deployment is asked about.
- **A database's SQL must be written as text in the flow,** and name a table: the tables it reads
  and writes are the permissions the deployment asks for. The values go after it, as many as it
  has `$n`s. See [Databases on the host](../guides/databases.md).
- **A host is its import,** so it can't come from a value: the host is the permission the
  deployment asks for. The path, headers and body can come from anywhere.
- **A mail's recipients are part of its grant.** Each one written as text in quotes is granted by
  name; one computed from a value is granted as "an address it computes", which the binding's
  list of addresses holds. The sender is the binding's, never the flow's. See
  [Mail](../guides/mail.md).
- **`now` and a secret are the same within a run,** however often they are written.
- **A file sent with `file:`** must be in a folder the flow reads, or be the new file that
  started the run. The server checks, before and as it sends, that the bytes still have the
  handle's hash; if not, the call answers `{status: 0, error: "changed"}`.
- **A call to another flow's action** goes out between rounds, as a request does, and its answer
  is kept in the run's log, so a retry, a replay and `holon verify` never make it again. The other
  flow runs its action as it would for a person, under its own grant and limits, and logs it as
  run by "flow shop, run …". The error is one of `refused` (not granted, revoked, or the flow is
  already in the chain of calls, itself included), `not_found`, `not_running`, `no_action`,
  `waiting` (the action parked at an `await`; its `run` goes on later), `failed` and `too_deep` (a
  chain is at most `max_call_depth` calls long: 3 by default). A secret, or a value computed from
  one, isn't sent: the other flow's log would keep it. `return status(201, paid.answer) ??
  status(502)` handles every failure at once, as the answer has no value then. A call nobody uses
  is a statement; to say why one failed, bind it and look at `error` and `message`. See
  [Actions](../guides/actions.md#run-by-another-flow).
- **A route answers and is done.** It doesn't wait: `await event(…)` in a route is refused. What
  waits is an action the route starts, `const started = payment(req.json)`, and the route answers
  with its link, `` return `/paid/${started.waiting.paid}` ``. A start goes out and is kept as a
  call is, so a retry never starts the run again; a start of the flow's own action is allowed, as
  the route doesn't wait for it.
- **The method forms** (`notes.put(k, v)`, `notes.delete(k)`) are the same writes as values: for a
  function that names its writes as outputs, or to pass a write on.

## Routes

```
route.METHOD("/path", (req) => answer, { resumes, log })
route.METHOD("/path", async (req) => { statements; return answer })
```

- **Methods:** `get`, `post`, `put`, `patch`, `delete`.
- **Paths:** a literal, `/` and segments of letters, digits, `_`, `.` and `-`, or `:name` for a
  part that `req.params.name` reads, and last, perhaps, `*name` for the rest of the path, one
  segment or more: `/files/*path` answers `/files/css/site.css` with `req.params.path`
  `css/site.css`, each segment decoded. A segment that is empty, `.` or `..`, or that decodes to
  `/` or `\`, matches no rest. The most specific route wins: `/todos/new` before `/todos/:id`,
  and any route without a rest before one with it, the longest prefix first (`/api/*rest` before
  `/*path`).
- **`(req)`** names the request; any name works, it may be destructured (`({ params, json }) =>
  …`), and a route that reads nothing of it leaves it out: `() => "ok"`.
- **`async`** goes on a handler that `await`s, as TypeScript requires.
- **`{ resumes: "NAME" }`** makes the route deliver the event a parked run is waiting for (see
  [`await`](#how-a-flow-runs)): the request carries the token the run's start was answered
  with, `started.waiting.NAME`.

**Answering.** A route answers with what it returns, or, with no `return`, with at least one
effect (it then answers 204). A call's answer is the far side's, never the route's: a route whose
only effects are calls answers 204 too.

| Form | Answers |
|---|---|
| `return body` | 200 with `body`: text as plain text, anything else as JSON |
| `return status(201, body)` | that status |
| `return status(code, body)` | a status computed by the flow (a number from 200 to 599) |
| `return body ?? status(404)` | 404 when `body` has no value (an absent answer is 404 anyway) |
| `return status(201, body) ?? status(502)` | 201 with `body`, or 502 when it has no value |
| `return respond(body, { headers: { "content-type": "text/html; charset=utf-8" } })` | with those headers (a record of texts); how the reply is framed (`content-length`, `transfer-encoding`, `connection`, …) is the host's, and those are dropped. `status` goes there too, `respond(body, { status: 201, headers })` |
| `return status(400)` | the status alone, with an empty body |
| `return status(204)` | the status alone, no body |

A redirect: `return respond("", { status: 303, headers: { location: "/" } })`. A streamed reply,
`return respond(sent.body, { stream: true })`, answers with its call's status: a refused call's
error reaches the caller as it was.

**Several `return`s are one answer.** Each body and status is chosen where its branch is taken, as
`if` and `switch` choose. `status(…)` and `respond(…)` are a route's answer and nothing else: one
bound to a name, passed to a function or returned from an action is refused.

**Handlers are separate.** Two routes (or any two handlers) can't share a value. At the top level
of a flow, beside handlers and functions, only `const`s of literals are allowed (`const limit =
50`); put shared logic in a function.

## WebSockets

```
route.websocket("/room/:name", {
  open: (req) => answer,
  message: (m) => value,
  close: (c) => { … },
}, { log: { message: "lazy" } })
```

A websocket holds the WebSocket connections pages open on its path, on the flow's port beside its
routes, and runs each block as a handler of its own. One block at least, each written once:

| Block | Runs | `req`, `m`, `c` hold | Answers |
|---|---|---|---|
| `open` | as a page opens a connection | the opening request, and `conn`, the connection's id | as a route does: `return ch.subscribe()` accepts and subscribes it to the channel; a `status` of 300 or more refuses it; `status(204)`, or effects alone, accept and send nothing; anything else accepts and is the first message |
| `message` | for each text the page sends, one at a time, in order | `conn`, `text`, `json`, and the opening's `path`, `params`, `query` | with a value alone, sent back on the connection; nothing sends nothing; `status` and `respond` are refused |
| `close` | once, when the connection has ended, after its last message | `conn`, `code`, `reason`, and the opening's `path`, `params`, `query` | nobody |

- **The path** is a route's; a websocket whose path matches a GET route's, or another
  websocket's, is refused: a path is a page or a connection.
- **`conn`** is the same in each run of a connection: the key to keep what the flow knows of it.
- **To reach the other connections,** send on a channel they are subscribed to: a run answers
  only its own.
- **The `log` option** is one level for every block, or each block's own: `{ log: "lazy" }`,
  `{ log: { message: "lazy" } }`.

The host's side (pings, limits, the codes it closes with, what a new version does to open
connections) is in [WebSockets](../guides/websockets.md).

## Clocks, files and actions

Beside its routes, a flow may have other handlers, each starting runs of its own:

```ts
import { action, every, flow, on, route } from "holon"
import state from "state"
import seen from "collection:seen"
import uploads from "folder:uploads"
import { now } from "holon:clock"

flow("handlers", { port: 8203 })

every("1h", (now) => {
  state.last_tick = now
})

on.file.created(uploads, (f) => {
  seen[f.name] = { size: f.size, at: now }
})

export const forget = action((input) => {
  delete seen[input.name]
  return { forgot: input.name }
})

route.get("/seen/:name", (req) => seen[req.params.name])
```

- **`every(DURATION, (now) => { … })`** runs every `DURATION`; `now` is the time of the tick, and
  a clock that doesn't read it leaves it out, `every("1h", () => …)`. A whole number of seconds
  from `"1s"` to `"366d"`, one clock per period. Its clock starts its runs, so such a flow asks to
  read the clock even when it never reads the time.
- **`on.file.created(dir, (f) => { … })`** runs for each file new or changed in the folder `dir`
  imports, once it stops changing; `f` is its handle. `on.file.deleted` runs for files removed
  from the folder, and `{ matching: "*.csv" }` for the names that match (`*` and `?` within a
  folder, `**/` any number of folders, letters either case). Several handlers may watch one
  folder, each with its own pattern.
- **`on.mail(ops, (m) => { … })`** runs for each message that comes to the mailbox an admin bound
  `ops` to (`holon mail add ops --imap …`); `m` is the message, read, and it is moved to the
  binding's done folder once the run is over. One handler a mail; what the mailbox held when the
  flow was deployed isn't run on. `From` proves nothing: check `m.authenticated`
  ([Receiving mail](../guides/mail.md#receiving-mail)).
- **`on.feed(prices, (e) => { … })`** runs for each event the feed an admin bound `prices` to
  sends (`holon feed add prices --url …`), one at a time, in the order they came; `e` is the
  event. One handler a feed; it answers nobody. The host keeps where the feed is: a connection
  lost is opened again from the last event run on, and an event sent again is passed over
  ([Feeds](../guides/feeds.md)).
- **`on.queue(payouts, (m) => { … })`** runs for each message the queue an admin bound `payouts`
  to holds (`holon queue add payouts --url …`), one at a time, in the order received; `m` is the
  message, `{data, id, attributes, sent, count, group}`, and it is deleted once its run's record
  is on disk. One handler a queue, one deployment listening to it; it answers nobody. A message
  run already is passed over ([Queues](../guides/queues.md)).
- **`export const NAME = action((input) => { … })`** runs when someone runs it by its name,
  `input` what they sent (a JSON value; `{}` for nothing). It answers with what it returns, a
  value alone (no `status(…)` or `respond(…)`); one that only writes answers nothing. It is
  exported, as it is what reaches the flow from outside: an `action(…)` that isn't exported is
  refused. The network never reaches it: a person runs it from the editor or a script, an agent
  only one a person lets agents run, and another flow by importing it
  ([Actions](../guides/actions.md)).

A file starts a run once it stops changing: the server looks at the folder every second, and a
file that is the same at two looks in a row is new. Files already there when the flow is first
deployed aren't new; files that arrive while the server is down or the deployment is paused start
their runs when it is back; a file the flow's own runs wrote into the folder doesn't start it
again.

**How runs are recorded.** Every handler is *strict* unless its options say otherwise: each run
is a line of the deployment's log, which is what the runs table, replay, revert and explain read.
The `log` option changes that:

| `log` | Its runs |
|---|---|
| (left out) | strict: recorded |
| `"lazy"` | recorded, and may reach the disk up to a second after the answer |
| `"unrecorded"` | not in the log, failed or not: counted (how many, how many failed, the last failure's status and error); they can't be replayed, reverted or explained. To see why one fails, make it `"lazy"` for a while. A person approves it at deploy, `Unrecorded("GET /health")` |

```ts
import { flow, route } from "holon"
import notes from "collection:notes"

flow("board", { port: 8170 })

route.get("/health", () => "ok", { log: "unrecorded" })

route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })
```

What strict costs is the disk's: a strict run answers once its record is synced, and runs that
arrive together share one sync. On a desktop SSD a sync takes a few milliseconds (strict at
about 1,600 requests a second at 8 connections on one Windows machine, about half that under
WSL2); on a server drive with power-loss protection it takes microseconds. `holon load` measures
it on yours.

Which to say is the flow author's choice, on any handler. `"unrecorded"` is for runs whose record
isn't worth keeping: a health check, a handler that only reads, one called so often its records
would only fill the log. `"lazy"` is for a handler that must answer fast, where losing its last
second of records to a crash is fine. A handler that writes, calls out or waits may say either,
and is what it says: an unrecorded one's writes, calls and messages are in no run, and a crash
may lose a lazy one's last second. `holon verify` leaves the state they may have moved unchecked
rather than calling it a break, a restore's dry run says unrecorded runs aren't counted, a
channel message an unrecorded run sends reaches whoever is listening then and isn't kept, and a
lazy run's goes out once its record is on disk, within a second. A run that waits keeps what it
waits with, and what it sent as it parked, whatever its level.

The option goes in the same place on every handler: `every("1m", () => { … }, { log: "lazy" })`,
`on.file.created(inbox, (f) => { … }, { log: "unrecorded" })`, `export const report = action(()
=> …, { log: "unrecorded" })`.

A clock's or a file's run answers nobody: a `return` there returns nothing, and only ends the
run. Every file has handlers: a flow nothing starts is refused at deploy, and to run it by hand,
put its statements in an exported action.

## What a handler holds

**A handler is everything tied to its trigger:** a route's request, a clock's tick, a file
handler's file, an action's input. What reads from the trigger is the handler's, and so is every
write and call placed in it, whether or not it reads the trigger:

```ts
import { flow, route } from "holon"
import state from "state"

flow("hits", { port: 8204 })

route.get("/hit", () => {
  const hits = (state.hits ?? 0) + 1
  state.hits = hits
  return `hit ${hits}`
})
```

`state.hits = hits` never looks at the request, and is still the route's: the write is tied to
the request. The tie is never read; it only has to be there.

**`if (x !== undefined)` ties a statement to `x` instead.** Its writes and calls happen once `x`
has a value in the run, and not at all when `x` has none:

```ts
import { flow, route, status } from "holon"
import state from "state"
import notes from "collection:notes"

flow("first-seen", { port: 8205 })

route.post("/notes/:id", (req) => {
  const id = req.params.id
  const found = notes[id]
  if (found !== undefined) state.seen = id
  return found ?? status(404)
})
```

`state.seen` is written only when the note was found. A `let` with no value before it, given one
inside, is tied to `x` too: `let sent; if (x !== undefined) sent = await api.post(…)` is absent
when `x` is. A function with a write inside it is tied as a write is; a function that only
computes needs no tie. The run still commits as one, its writes at its end.

**A statement tied to nothing is a mistake.** A binding nothing uses and that reads nothing of the
handler wouldn't be part of it. The mistake says what would tie it: in a route, use the request in
it; in a clock, write something with it.

## Functions

A function is a piece of flow with inputs and outputs, placed as one node:

```
function NAME(input, input: Type, …) {
  statements
  return output
}
```

- **Its inputs** are its parameters, each a name: a pattern is written on the first line of the
  body, `const { price, qty } = order`. Types on parameters are optional (`text: string`,
  `items: { id: string }[]`); when given, they're checked.
- **Its outputs** are what it returns. A function that returns a binding names its output after
  it (`return slug` gives `slug`), and `out` otherwise. One whose every `return` is a record with
  the same keys has one output per key: `return { rows, report }` gives `rows` and `report`,
  read as `const { rows, report } = summarize(x)` or `summarize(x).rows`.
- An input never used is a mistake, and so is an output that is just an input passed straight
  through.
- A function is placed by calling it: `slugify(x)`. Functions are defined at the top level and
  visible to the whole file, in any order. One function may place another.
- A function is one unit with one hash wherever it is placed, and can be given to `map` and
  `reduce` by name.
- **A function may be `async`** and await a request, as a route does: `async function
  toSlack(event) { const sent = await hooks.post(…); return sent.status }`, placed with `await
  toSlack(event)`. It is the same unit as without `async`. It never waits for an event (an action
  does), it doesn't call another flow's action (its handler does), and a method that takes a
  function (`map`, `filter`, `sort`…) takes no `async` one.
- **Writes inside a function** leave through its boundary as outputs: a collection's write in the
  method form, bound and returned, and a state write as the output `write`. Writes the caller
  doesn't use are performed:

```ts
import { flow, route } from "holon"
import state from "state"
import posts from "collection:posts"
import { now } from "holon:clock"

flow("publish-example", { port: 8202 })

function publish(slug, body) {
  state.last_post = slug
  const put = posts.put(slug, { ...body, at: now })
  return { put }
}

route.post("/posts/:slug", (req) => {
  const p = publish(req.params.slug, req.json)
  return { slug: req.params.slug }
})
```

## JavaScript functions

```
const NAME = js(function NAME(inputs) {
  JavaScript
}, { fuel, memory, outputs })
```

The function runs as JavaScript, as written. It receives its inputs by name and returns one value
(one output, named after the function) or, with `outputs: ["best", "distance"]`, a record with
those fields (several).

- **What crosses:** numbers, texts, booleans, records (as objects) and lists (as arrays), both
  ways. `undefined` or `null` returned is an output with no value. A `Map`, `Date`, function or
  Promise can't cross, and is an error naming its kind.
- **Sandbox:** no clock, no randomness, no timers, no network, no storage, no imports. Each call
  starts fresh; nothing survives between calls. So a call depends only on its inputs, and a run
  replays exactly.
- **Budgets:** `fuel` is instructions per call (default 200M), `memory` the memory per call
  (default 16 MiB). The deploy asks for the budgets, and the server's policy caps them.
- **Failures:** an exception, or a call that runs out of fuel, gives no value, and the run
  records why: `explain` shows it under the statement. A syntax error is found by `holon check`.
- **Its text is its identity,** exactly as written: `holon fmt` lays the body out with the rest, so
  a body it lays out anew is a new version that does the same.

## map and reduce

```
xs.map(f)                    // f(item) for each item: a list of the results
xs.reduce(f, start)          // f(acc, item), carried through the list
xs.filter(f)                 // the items f says yes to; find, some, every, toSorted, … likewise
range(n)                     // [0, 1, …, n-1], at most 10000
```

`f` is a function's name or a function written in place, its body one expression:
`xs.map((x) => x * 2)`, `xs.reduce((acc, x) => acc + x, 0)`, `xs.filter((x) => x.open)`.

- **Names from around** a function written in place are passed to it automatically: in
  `Object.keys(r).map((k) => r[k])`, `r` comes from the route.
- A function takes the item; `reduce`'s takes the result so far and the item, `toSorted`'s the two
  items it compares, and `map`'s may take the index after the item. A parameter it doesn't use is
  fine, `_` or any name: `Array.from({ length: 3 }, () => 0)`.
- A function may read from outside (a collection, a request): each item's read is made,
  recorded and replayed on its own.

## How a flow runs

The syntax is TypeScript's, and these meanings are Holon's. Where a JavaScript reader would be
misled, the compiler refuses instead.

- **Dataflow, not control flow.** A value is computed when what it needs has arrived. The order
  of statements only matters for names (defined before use).
- **Absence.** A value that doesn't arrive makes everything built from it absent: `a.b` on no
  value is no value, never an error. That is how a flow says "not found" (`?? status(404)`), "not
  allowed" (`ok ? x : undefined`) and "not verified". Absence isn't a value: it can't be stored or
  compared. `none()` is a value, and unrelated.
- **Reads at the start, writes at the end.** `state.x`, `coll[k]` and `dir[name]` are the values as
  the run began. Writes are performed when the run finishes, and only if granted. A run never sees
  its own writes. When two runs change the same item at once, one is run again, so none is lost.
- **Calls out happen between rounds.** A request is made when the run gets to it; the run
  continues with the answer. One bound on a statement of its own (`const page = await
  api.get(…)`) is made when the run reaches the statement, whatever reads `page`, and not when it
  isn't reached: in an `if` whose condition doesn't hold, or after a `return` that answered. One
  written in a branch (`x ?? (await api.get(…)).body`, `c ? await api.get(…) : undefined`) is
  made only when the branch needs it, so a branch not taken makes no call. To make a bound call
  only sometimes, put it in an `if`, or tie it to a value: `if (first !== undefined) charged =
  await api.post(…)`.
- **Waiting.** `await event(name)` parks the run, an action's: a route answers and is done, and
  starts the action that waits by calling it without `await`. A later request to a route with
  `{ resumes: name }`, carrying the token, delivers the event and the run continues. `expires`
  sets how long it waits.
- **People decide an approval.** `await approval(name, { people: 2 })` parks the run as an event
  does, but nothing the flow hands out resumes it: it has no token, and a route `{ resumes: name }`
  that names it is refused. People signed in to Holon decide it, never an agent, and how many is
  part of what the flow asks for, `Approval("name/2")`: a version that asks fewer asks anew.
  The target design, approvals in steps filled by roles in order, is
  [`docs/L4s/access-and-approvals-L4.md`](../../L4s/access-and-approvals-L4.md) §8; `people: n`
  stays its short form.
- **Sharing is explicit.** A binding is one value used many times. The same expression written
  twice is computed twice.
- **Every run ends.** There are no loops and no recursion: `map`, `reduce` and a list's other
  methods that take a function go over a list, and `range` makes at most 10000 items.
- **A value nests at most 256 deep.** Each list in a list, record in a record, or field's value
  is a level. Something built a level deeper is no value: `reduce((acc, i) => [acc], 0)` gives
  nothing past 256 steps, so `?? status(422)` answers it. JavaScript nests until its stack runs
  out. JSON is read 64 deep.

## Types

Values have one of ten types: `Number`, `Boolean`, `Text`, `Record`, `Bytes`, `List<T>`,
`Product<…>`, `Sum<…>`, `Effect` and `Any`. You write types only on function parameters, as
TypeScript writes them (`number`, `boolean`, `string`, an object type, `string[]`, `Uint8Array`,
`T | undefined`), and only if you
want them checked. Wiring a `Text` into a `Number` input is a mistake at the argument. `any` fits
everything.

## Identity and canonical form

- **A flow's identity is the hash of its graph** (`unit` in `holon check`). Names of bindings and
  functions, comments, blank lines and the order of independent statements aren't in it: two
  texts that build the same graph are the same unit. A function's parameter and output names are,
  as they are its ports.
- **Any change to what the flow computes is a new version** with a new hash. Every run names the
  version it ran.
- **`holon fmt`** lays the text out. Formatting twice changes nothing.

## Mistakes

`holon check`, the editor and `holon lsp` report each mistake at its line and column, with the
expression underlined:

```
$ holon check broken.holon.ts
broken.holon.ts:5:23: unknown name "nope"
  route.get("/a", () => nope)
                        ^^^^
```

| Mistake | Where it points |
|---|---|
| unknown name `x` (not bound, not imported) | the name |
| `f` takes `n` inputs, `m` given | the extra or missing argument |
| `f` has no input or parameter `p`; `p` given twice | the name |
| input `p` can't be left out | the call |
| `Text` doesn't fit `Number` | the argument |
| a value nobody uses | the statement |
| `\|\|` takes booleans | the expression |
| a collection is read after this run writes to it | the read |
| `status(…)` and `respond(…)` are a route's answer: return them | the expression |
| an action answers with a value alone | the `status(…)` or `respond(…)` |
| a route answers and is done, so it doesn't wait | the `await` |
| a method no leaf does; a global the flow has none of | the method or the name |
| a loop | the statement |
| `catch (e)`; a throw that isn't `new Error(message)`; a `return` in `finally` | the name, the throw, the `finally` |
| nothing in a try block can fail, so its catch never runs | the `try` |
| a period, a file handler or an action name written twice | the second handler |
| `map` gives its function `n` inputs; this one takes `m` | the function |
| a list changed in place (`xs.sort()` alone, `xs.pop()`, a list read after it was sorted) | the call, or the read |
| a function's input never used, or an output that is an input passed through | the parameter or the return |
| two routes share a value | the shared binding |
| two routes clash; not a method; not a route path | the route |
| this isn't wired to the route's request (or the handler's trigger) | the statement |
