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

`holon fmt` lays a file out as Prettier does, at 100 columns and without semicolons, keeping its
comments and blank lines, `js()` bodies included. It writes a text only once that text compiles to
the same flow.

## Imports and settings

An import names something the flow uses from outside a run. Its source says what it is:

| Import | Names |
|---|---|
| `import { flow, route, status } from "holon"` | the vocabulary (`flow`, `route`, `every`, `on`, `action`, `js`, `respond`, `status`, `stripe`, `event`) and the [leaves](leaves.md) |
| `import key from "secret:NAME"` | a secret set for the deployment, read as `key`: someone sets it before the flow works |
| `import key from "secret:NAME" with { optional: "true" }` | a secret the flow can run without; the parts that need it have no value when it isn't set |
| `import state from "state"` | the flow's state: `state.KEY` reads a key, `state.KEY = …` writes it |
| `import notes from "collection:NAME"` | a collection, used as `notes[key]` |
| `import inbox from "folder:NAME"` | a named folder on the host, used as `inbox[file]` |
| `import shop from "database:NAME"` | a named database on the host, used as `shop.query(sql, …)` |
| `import updates from "channel:NAME"` | a channel: `updates.send(x)`, `updates.subscribe()`, `updates.pipe(…)` |
| `import api from "host:HOST"` | a host the flow calls over HTTPS, as `await api.get("/path")` |
| `import { now } from "holon:clock"` | the time the run started |
| `import { random } from "holon:random"` | one random number for the run |
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

## Names and literals

**Identifiers** are TypeScript's. A name bound in a handler is visible after its binding, in that
handler; functions and top-level `const`s are visible to the whole file.

**No globals.** JavaScript's globals aren't there: a flow has no `fetch`, `Date` or `Math`, and
the mistake says to import what it uses. `JSON.parse`, `JSON.stringify` and `Object.keys` are
there, as the leaves that do them.

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
| `const { a, b } = r`, `const [x, y] = xs` | Destructuring: a field each, an index each; `{ a = 1 }` takes a default, `[first, ...rest]` the head and the tail. |
| `state.key = expr` | Writes the flow's state under `key` when the run finishes. |
| `coll[key] = expr` | Writes an item of a collection (or a file of a folder) when the run finishes. |
| `delete coll[key]` | Deletes an item (or a file) when the run finishes. |
| `if (c) …; else …` | Chooses: writes in a branch happen only when it is taken, and writes to one key in both branches are one write. |
| `if (x !== undefined) { … }` | Its writes and calls tied to `x` (see [What a handler holds](#what-a-handler-holds)). |
| `switch (s) { case …: … }` | Chooses by value (see below). |
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

## Expressions

From lowest to highest precedence; binary operators group left to right.

| Form | Meaning |
|---|---|
| `c ? a : b` | `a` when `c` is `true` and `a` has a value; otherwise `b` |
| `c ? a : undefined` | `a` when `c` is `true`; otherwise no value |
| `a ?? b` | `a`, or `b` when `a` has no value |
| `a \|\| b`, `a && b`, `!a` | logic, on booleans; a text or number after `\|\|` is refused, with "use `??`" |
| `a === b`, `a !== b` | structural equality of any two values, never converted: `1` is not `"1"`. `==` and `!=` mean the same, and `fmt` writes `===` |
| `a < b`, `<=`, `>`, `>=` | comparison, on numbers |
| `a + b`, `a - b`, `a * b`, `a / b` | arithmetic, on numbers. There is no `+` on texts: use a template literal, `` `${a}${b}` `` |
| `f(…)` | a call (see [Calls](#calls)) |
| `a.name`, `a?.name` | an output named `name`, else the field `name` of a record; `?.` means the same |
| `a[e]` | the field whose name is `e`'s value; on a collection or folder, an item or a file |
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
- Some leaves take any number of inputs: `add`, `multiply`, `and`, `or`, `concat`, `list`.
- Some inputs are optional (a request's `headers` and `body`, a collection's `.list` `prefix` and
  `after`) and may be left out. Leaving out any other input is a mistake that names it.
- Parameters not given take their defaults.
- A leaf with no inputs still takes parentheses: `record()`, `none()`.

**Methods are leaves.** A text's or list's method is the leaf that does it: `t.toUpperCase()` is
`uppercase`, `t.toLowerCase()` `lowercase`, `t.split(",")` `split`, `xs.join("-")` `join`,
`t.indexOf(x)` `index_of`, `xs.map(f)` `map`, `xs.reduce(f, 0)` `fold`; `JSON.parse` and
`JSON.stringify` are `json_parse` and `json_stringify`, `Object.keys` is `keys`. A list's length
is `length(xs)`, a text's `text_length(t)`. A method no leaf does is refused, with "do it in a
`js()` function".

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
| `coll[k]`, `coll.get(k)` | the item under `k` in a collection |
| `coll[k] = e`, `coll.put(k, e)` | put it when the run finishes |
| `delete coll[k]`, `coll.delete(k)` | delete it when the run finishes |
| `coll.list({ prefix: p, after: a, limit: n })` | a page of items, in key order |
| `dir[name]`, `dir.get(name)` | a file's text, in a folder, as the run began (up to 1 MiB) |
| `dir[name] = text`, `dir.put(name, text)` | write the whole file when the run finishes |
| `delete dir[name]`, `dir.delete(name)` | delete the file when the run finishes |
| `dir.list({ prefix: p, after: a, limit: n })` | a page of files: `{size, hash, modified}` by name |
| `dir.file(name)` | a handle to a file of any size: `{folder, name, size, hash, modified}`, no bytes |
| `dir.lines(name, { from: i, limit: n })` | `n` lines from line `i` (from the end when negative) |
| `dir.slice(name, { offset: o, length: n })` | `n` bytes from offset `o`, as text |
| `dir[name] = handle`, `dir.copy(name, handle)` | copy a file by handle when the run finishes |
| `dir.create(name, text)`, `dir.create_copy(name, handle)` | the same write or copy, only if there is no such file yet: else the run fails, and nothing it did lands |
| `f` in `on.file.created(dir, (f) => …)` | the handle of the file that started the run: `{folder, name, size, hash, modified, event}` |
| `db.query(sql, v1, v2, …)` | the rows of a database, a list of records, as the run began; `$1`, `$2`, … in the SQL are the values after it |
| `db.one(sql, v1, …)` | the first row, or no value when there is none |
| `db.exec(sql, v1, …)`, `db.exec(sql, …, { expect: n })` | run an `insert`, `update` or `delete` when the run finishes; with `expect`, only if it changes exactly `n` rows, else nothing the run wrote lands and a route answers 409 |
| `await api.get(path, { headers: h, body: b })` | a request to the host `api` imports; also `post`, `put`, `patch`, `delete`, `head`. The answer is `{status, headers, body}` |
| `await api.put(path, { headers: h, file: handle })` | the same, sending a file as the body, by handle: the server streams its bytes, which never pass through the flow (any method) |
| `stripe.verify(req, secret, now, { tolerance: 300 })` | a verified Stripe webhook; `.event` is the event |
| `await charge(x)` (`import { charge } from "flow:billing"`) | run the action `charge` of the deployed flow `billing` with `x`, a value (`{}` when none is written); the answer is `{answer, run}` when it ran, `{error, message}` when it didn't |
| `payment(x)`, `charge(x)`, not awaited | start a run of this flow's action, or another flow's, with `x`; the answer is `{run, waiting}` once it waits, `waiting` the token for each point it waits at, `{answer, run}` if it finishes first, `{error, message}` when it didn't run |
| `await event(name, { expires: d })` | park the run until the event `name` arrives (in an action, not a route) |
| `JSON.stringify(x)`, `JSON.parse(text)` | JSON out, JSON in |

Rules:

- **A collection, folder, database or host must be imported** before it is used: its import is
  what the deployment is asked about.
- **A database's SQL must be written as text in the flow,** and name a table: the tables it reads
  and writes are the permissions the deployment asks for. The values go after it, as many as it
  has `$n`s. See [Databases on the host](../guides/databases.md).
- **A host is its import,** so it can't come from a value: the host is the permission the
  deployment asks for. The path, headers and body can come from anywhere.
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
  part that `req.params.name` reads. The most specific route wins: `/todos/new` before
  `/todos/:id`.
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
range(n)                     // [0, 1, …, n-1], at most 10000
```

`f` is a function's name or a function written in place, its body one expression:
`xs.map((x) => x * 2)`, `xs.reduce((acc, x) => acc + x, 0)`.

- **Names from around** a function written in place are passed to it automatically: in
  `Object.keys(r).map((k) => r[k])`, `r` comes from the route.
- `map`'s function takes the item and no index; `reduce`'s takes the result so far and the item.
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
  api.get(…)`) is made when the run reaches the statement, whatever reads `page`; one written in
  a branch (`x ?? (await api.get(…)).body`, `c ? await api.get(…) : undefined`) is made only when
  the branch needs it, so a branch not taken makes no call. To make a bound call only sometimes,
  tie it: `if (first !== undefined) charged = await api.post(…)`.
- **Waiting.** `await event(name)` parks the run, an action's: a route answers and is done, and
  starts the action that waits by calling it without `await`. A later request to a route with
  `{ resumes: name }`, carrying the token, delivers the event and the run continues. `expires`
  sets how long it waits.
- **Sharing is explicit.** A binding is one value used many times. The same expression written
  twice is computed twice.
- **Every run ends.** There are no loops and no recursion: `map` and `reduce` go over a list, and
  `range` makes at most 10000 items.

## Types

Values have one of nine types: `Number`, `Boolean`, `Text`, `Record`, `List<T>`, `Product<…>`,
`Sum<…>`, `Effect` and `Any`. You write types only on function parameters, as TypeScript writes
them (`number`, `boolean`, `string`, an object type, `string[]`, `T | undefined`), and only if you
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
| there is no `+` on texts; `\|\|` takes booleans | the expression |
| a collection is read after this run writes to it | the read |
| `status(…)` and `respond(…)` are a route's answer: return them | the expression |
| an action answers with a value alone | the `status(…)` or `respond(…)` |
| a route answers and is done, so it doesn't wait | the `await` |
| a method no leaf does; a global the flow has none of | the method or the name |
| a loop, `try` or `throw` | the statement |
| a period, a file handler or an action name written twice | the second handler |
| `map` gives its function `n` inputs; this one takes `m` | the function |
| a function's input never used, or an output that is an input passed through | the parameter or the return |
| two routes share a value | the shared binding |
| two routes clash; not a method; not a route path | the route |
| this isn't wired to the route's request (or the handler's trigger) | the statement |
