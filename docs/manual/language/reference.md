# Language reference

The full rules of the Holon language. For an introduction, read the [tour](tour.md) first. The
building blocks you can call are listed in [leaves](leaves.md). The design behind the language,
and how each construct lowers to the graph, is in [`docs/LANGUAGE.md`](../../LANGUAGE.md).

- [Files](#files)
- [Headers](#headers)
- [Names and literals](#names-and-literals)
- [Statements](#statements)
- [Expressions](#expressions)
- [Calls](#calls)
- [The outside world](#the-outside-world)
- [Routes](#routes)
- [Flows without routes](#flows-without-routes)
- [Holons](#holons)
- [JavaScript holons](#javascript-holons)
- [Functions for map and fold](#functions-for-map-and-fold)
- [How a flow runs](#how-a-flow-runs)
- [Types](#types)
- [Identity and canonical form](#identity-and-canonical-form)
- [Mistakes](#mistakes)

## Files

A flow is a `.holon` file: UTF-8 text, lines ending in `\n` or `\r\n`. A file is its headers, then
its holons, routes and statements:

```holon
# A comment runs from # to the end of the line.
flow example
port 8200

route GET /ping (req) {
  respond "pong"
}
```

`holon fmt` rewrites a file in the canonical form: headers first, then holons, then routes, each
statement on its own line. It keeps comments on the line they were written on.

A statement may continue on the next line while a parenthesis, brace or bracket is open, or after
a binary operator or a comma. There are no semicolons.

## Headers

Headers say what the flow is and how it is deployed. Each goes on its own line, before anything
else.

| Header | Meaning |
|---|---|
| `flow NAME` | The flow's name: what it deploys as. Required. |
| `port N` | A flow with routes: the port its server answers on. |
| `every DURATION` | A flow without routes: how often it runs (`every 10s`, `every 1h`). |
| `collection NAME` | Declares a collection, used as `NAME[key]`. One per line; as many as needed. |
| `folder NAME` | Declares a named folder on the host, used as `NAME[file]`. |
| `on file NAME` | A flow without routes: runs once for each new file in the folder `NAME` (which it declares too). One per flow. |
| `optional PERMISSION` | A permission the flow can run without (see below). |
| `browsers ORIGIN, …` | The web origins allowed to call the flow from a browser (CORS), or `*`. |

`optional` takes `secret.NAME` for a secret, or a permission as the server writes it, such as
`optional HttpRequest("api.example.com")`. A deploy may leave an optional permission out; the
parts of the flow that need it then have no value.

A declaration names something; it grants nothing. What the flow may do is always worked out from
what it uses.

## Names and literals

**Identifiers** are `[A-Za-z_][A-Za-z0-9_]*`. A name that is a leaf, a template, a keyword or a noun
can't be bound; the mistake names the clash.

**Keywords:** `flow` `port` `every` `browsers` `optional` `collection` `folder` `holon` `js` `route`
`resumes` `respond` `delete` `await` `if` `then` `else` `when` `match` `fn` `true` `false`. A keyword
is fine after a dot (`req.json.in`) and as a record key (`{ in: 1 }`).

**Nouns** are the outside world: `req` (or whatever a route names its request), `secret`, `state`,
`now`, `random`, `http`, `stripe`, and every declared collection and folder.

| Literal | Examples | Notes |
|---|---|---|
| Number | `0`, `86400`, `-1`, `2.5`, `1e3` | finite decimals; no hex, infinity or NaN |
| Duration | `10s`, `5m`, `2h`, `1d` | a number of seconds: `5m` is `300` |
| Text | `"hello"`, `"line\n"`, `"Hello, {name}"` | JSON escapes; `{expr}` puts a value in; `{{` and `}}` are braces |
| Boolean | `true`, `false` | |
| Record | `{ title: t, "content-type": "text/html" }` | a key that isn't a plain word is quoted |
| Record, extended | `{ ..base, extra: v }` | `base`'s fields, plus or replacing `extra` |
| List | `[a, b, c]`, `[]` | |

Interpolation takes any value: a text stays itself, a number or boolean is written as it reads,
and a record or list is written as JSON.

## Statements

| Statement | Meaning |
|---|---|
| `name = expr` | Binds a name. One form, everywhere. A name is bound once per block and visible after its binding. |
| `state.key = expr` | Writes the flow's state under `key` when the run finishes. |
| `coll[key] = expr` | Writes an item of a collection (or a file of a folder) when the run finishes. |
| `delete coll[key]` | Deletes an item (or a file) when the run finishes. |
| `respond …` | What a route answers (see [Routes](#routes)). |
| `expr` | An effect, such as `notes.put(k, v)` or an `http.post(…)` whose answer isn't used. |

An expression statement that isn't an effect is a mistake ("a value nobody uses"): bind it or
remove it. A binding that is never used is a warning. Each state key may be written once in a
flow: a run reads its starting state and writes once, at the end.

## Expressions

From lowest to highest precedence; binary operators group left to right.

| Form | Meaning |
|---|---|
| `if c then a else b` | `a` when `c` is `true` and `a` has a value; otherwise `b` |
| `if c then a`, `a when c` | `a` when `c` is `true`; otherwise no value |
| `match s { p -> a, … else -> e }` | the arm whose pattern equals `s` (see below) |
| `a ?? b` | `a`, or `b` when `a` has no value |
| `a \|\| b`, `a && b`, `!a` | logic, on booleans |
| `a == b`, `a != b` | structural equality of any two values, never converted: `1` is not `"1"` |
| `a < b`, `<=`, `>`, `>=` | comparison, on numbers |
| `a + b`, `a - b`, `a * b`, `a / b` | arithmetic, on numbers. There is no `+` on texts: interpolate, or call `concat(a, b)` |
| `f(…)` | a call (see [Calls](#calls)) |
| `a.name` | an output named `name`, else the field `name` of a record |
| `a[e]` | the field whose name is `e`'s value; on a declared collection or folder, an item or a file |
| `(e)` | grouping |

**`match`.** Patterns are literals (numbers, texts, `true`, `false`), several to an arm if
separated by commas, and `else` comes last:

```holon
flow match-example
port 8201

route POST /size (req) {
  respond match req.json.size {
    "s", "small" -> 1
    "m" -> 2
    "l", "large" -> 3
    else -> 0
  }, missing: 400
}
```

The first arm whose pattern equals the subject is chosen, and only that arm runs. If the chosen
arm has no value, the `match` has none: it never falls through to another arm. With no match and
no `else`, or with the subject absent, the `match` has no value.

**Choices only run what they need.** In `if`, `when`, `??` and `match`, the branch that isn't
taken isn't computed: its requests aren't sent, its reads aren't made, its `await` doesn't park
the run. `select(c, a, b)`, written as a call, is the strict form that needs both.

**Outputs and fields.** Some values have several outputs: a request, a verified Stripe event, a
holon with several outputs. `v.name` is the output called `name` if there is one, else the field
`name` of `v`'s first output. Used bare, such a value is its first output.

## Calls

```
leaf(input, input, name: value, …)
```

Positional arguments are the unit's **inputs**, in order. A named argument is an input when the
unit has an input of that name, otherwise a **parameter**, fixed in the flow:

```
http.post("hooks.slack.com", path, headers: h, body: b)   # headers, body: inputs
hmac_sha256(key, text, encoding: hex)                      # encoding: a parameter
```

- A parameter's value may be a bare word (`encoding: hex`, `collection: notes`); a value with a dot
  needs quotes.
- Some leaves take any number of inputs: `add`, `multiply`, `and`, `or`, `concat`, `list`.
- Some inputs are optional (`http.*`'s `headers` and `body`, a collection's `.list` `prefix` and
  `after`) and may be left out. Leaving out any other input is a mistake that names it.
- Parameters not given take their defaults.
- A leaf with no inputs still takes parentheses: `record()`, `none()`.

Every [leaf](leaves.md) is callable by its own name. The nouns and statements below are a nicer
way to write some of them, and `holon fmt` prints that form.

## The outside world

Each of these is one building block, and each is one thing the flow may do.

| Written | What it is |
|---|---|
| `req`, `req.params`, `req.query`, `req.headers`, `req.body`, `req.json` | the request and its parts (`body` as text, `json` parsed) |
| `req.path`, `req.method` | fields of the request record |
| `secret.name` | a secret set for the deployment |
| `state.key` | the state under `key`, as the run began |
| `state.key = e`, `state.set(key, e)` | write it when the run finishes |
| `now` | the time the run started, in seconds since the epoch |
| `random` | one random number for the run |
| `coll[k]`, `coll.get(k)` | the item under `k` in a declared collection |
| `coll[k] = e`, `coll.put(k, e)` | put it when the run finishes |
| `delete coll[k]`, `coll.delete(k)` | delete it when the run finishes |
| `coll.list(prefix: p, after: a, limit: n)` | a page of items, in key order |
| `dir[name]`, `dir.get(name)` | a file's text, in a declared folder, as the run began (up to 1 MiB) |
| `dir[name] = text`, `dir.put(name, text)` | write the whole file when the run finishes |
| `delete dir[name]`, `dir.delete(name)` | delete the file when the run finishes |
| `dir.list(prefix: p, after: a, limit: n)` | a page of files: `{size, hash, modified}` by name |
| `dir.file(name)` | a handle to a file of any size: `{folder, name, size, hash, modified}`, no bytes |
| `dir.lines(name, from: i, limit: n)` | `n` lines from line `i` (from the end when negative) |
| `dir.slice(name, offset: o, length: n)` | `n` bytes from offset `o`, as text |
| `dir[name] = handle`, `dir.copy(name, handle)` | copy a file by handle when the run finishes |
| `ingress` (with `on file NAME`) | the handle of the new file that started the run |
| `http.get(host, path, headers: h, body: b)` | a request to `host`; also `post`, `put`, `patch`, `delete`, `head`. The answer is `{status, headers, body}` |
| `http.put(host, path, headers: h, file: handle)` | the same, sending a file as the body, by handle: the server streams its bytes, which never pass through the flow (any method) |
| `stripe.verify(req, secret, now, tolerance: 5m)` | a verified Stripe webhook; `.event` is the event |
| `await name, expires: d` | park the run until the event `name` arrives |
| `resume_token(name)` | the token a resuming route takes to deliver `name` |
| `json(x)`, `parse_json(text)` | JSON out, JSON in |

Rules:

- **A collection or folder must be declared** in the headers (`collection notes`, `folder
  inbox`). Without the declaration, `notes[k]` is a field lookup on a record named `notes`.
- **An `http.*` host must be written as text in the flow.** The host is the permission the
  deployment asks for, so it can't come from a value. The path, headers and body can.
- **`now` and a secret are the same within a run,** however often they are written.
- **A file sent with `file:`** must be in a folder the flow reads, or be the new file that
  started the run. The server checks, before and as it sends, that the bytes still have the
  handle's hash; if not, the call answers `{status: 0, error: "changed"}`.
- **The method forms** (`notes.put(k, v)`, `state.set(k, v)`) are the same writes as values: for a
  holon that names its writes as outputs, or to pass a write on.

## Routes

```
route METHOD /path (req) [resumes NAME] {
  statements
}
```

- **Methods:** `GET`, `POST`, `PUT`, `PATCH`, `DELETE`.
- **Paths:** `/` and segments of letters, digits, `_`, `.` and `-`, or `:name` for a part that
  `req.params.name` reads. The most specific route wins: `/todos/new` before `/todos/:id`.
- **`(req)`** names the request; any name works.
- **`resumes NAME`** makes the route deliver the event a parked run is waiting for (see
  [`await`](#how-a-flow-runs)): the request carries the token from `resume_token(NAME)`.

**Answering.** A route has exactly one `respond`, or at least one effect (it then answers 204):

| Form | Answers |
|---|---|
| `respond body` | 200 with `body`: text as plain text, anything else as JSON |
| `respond body, status: 201` | that status |
| `respond body, status: code` | a status computed by the flow (a number from 100 to 599) |
| `respond body, missing: 404` | 404 when `body` has no value (the default is 404) |
| `respond body, headers: { "content-type": "text/html; charset=utf-8" }` | with those headers (a record of texts) |
| `respond 204` | the status alone, no body |
| `respond body: 42` | a literal number as the body |

A redirect: `respond "", status: 303, headers: { location: "/" }`. A streamed reply, `respond
sent.body, stream: true`, answers with its call's status: a refused call's error reaches the
caller as it was.

**A route is everything wired to its request.** Every statement in a route must use `req`,
directly or through a binding. A statement that doesn't (a counter that never looks at the
request, say) wouldn't be part of the route, so it is a mistake; move it to a flow of its own.

**Routes are separate.** Two routes can't share a value. At the top level of a flow with routes,
only bindings of literals are allowed (`limit = 50`); put shared logic in a holon.

## Flows without routes

A flow with no routes is one body of statements. With `every`, the server runs it on that
interval:

```holon
flow tick
every 1m

state.ticks = (state.ticks ?? 0) + 1
state.last = now
```

The clock starts its runs, so such a flow asks to read the clock even when it never reads `now`.

With `on file NAME`, it runs once for each new file in that folder, and `ingress` is the file's
handle:

```holon
flow sizes
on file uploads
folder notes

notes["{ingress.name}.txt"] = "{ingress.size} bytes, hash {ingress.hash}"
```

A file starts a run once it stops changing: the server looks at the folder every second, and a
file that is the same at two looks in a row is new. Files already there when the flow is first
deployed aren't new; files that arrive while the server is down or the deployment is paused start
their runs when it is back; a file the flow's own runs wrote into the folder doesn't start it
again.

A flow without routes, `every` or `on file` is a building block: it can be placed in another flow as
a holon. Its outputs are its values that nothing uses, which is why an expression statement is
allowed here, and only here.

## Holons

A holon is a function: a piece of flow with declared inputs and outputs, placed as one node.

```
holon NAME(input, input: Type, …) -> output {
  statements
}

holon NAME(inputs) -> (out1, out2, …) {
  statements
}
```

- Each output is bound inside with `output = expr`. An input never used, or an output never
  bound, is a mistake, and so is an output that is just an input passed straight through.
- Types on ports are optional (`text: Text`, `items: List<Record>`); when given, they're checked.
- A holon is placed by calling it: `slugify(x)`. With several outputs, `h(…).name` is one of them;
  bind the call (`r = h(x)`) to use several: `r.a`, `r.b`.
- Holons are defined at the top level and visible to the whole file, in any order. One holon may
  place another.
- A holon is one unit with one hash wherever it is placed, and can be given to `map` and `fold`
  by name.
- **Writes inside a holon** are named outputs, in the method form, so they leave through the
  holon's boundary. Writes the caller doesn't use are performed:

```holon
flow publish-example
port 8202
collection posts

holon publish(slug, body) -> (put, saved) {
  put = posts.put(slug, { ..body, at: now })
  saved = state.set(last_post, slug)
}

route POST /posts/:slug (req) {
  p = publish(req.params.slug, req.json)
  respond { slug: req.params.slug }, missing: 400
}
```

## JavaScript holons

```
holon NAME(inputs) -> outputs js [fuel N] [memory N] {
  JavaScript
}
```

The body is a JavaScript function body. It receives the inputs by name and returns one value (one
output) or an object keyed by output name (several).

- **What crosses:** numbers, texts, booleans, records (as objects) and lists (as arrays), both
  ways. `undefined` or `null` returned is an output with no value. A `Map`, `Date`, function or
  Promise can't cross, and is an error naming its kind.
- **Sandbox:** no clock, no randomness, no timers, no network, no storage, no imports. Each call
  starts fresh; nothing survives between calls. So a call depends only on its inputs, and a run
  replays exactly.
- **Budgets:** `fuel` is instructions per call (default 200M), `memory` the memory per call
  (default 16 MiB). `50M` is fifty million; `16MB` is 16 MiB. The deploy asks for the budgets, and
  the server's policy caps them.
- **Failures:** an exception, or a call that runs out of fuel, gives no value, and the run
  records why: `explain` shows it under the statement. A syntax error is found by `holon check`.
- **The body is read as JavaScript:** `#`, `{` and `}` inside it are JavaScript's, not Holon's, and
  its text is part of the flow's identity, exactly as written.

## Functions for map and fold

```
map(list, fn: f)                 # f(item) for each item: a list of the results
fold(start, list, fn: f)         # f(acc, item), carried through the list
range(n)                         # [0, 1, …, n-1], at most 10000
```

`fn:` takes a holon's name or a function written in place, `fn(a, b) -> expr`.

- **Extra values** after the list are passed after the item: `map(xs, a, fn: f)` calls `f(item,
  a)`.
- **Names from around** a function written in place are passed to it automatically: in
  `map(keys(r), fn: fn(k) -> r[k])`, `r` comes from the route.
- The number of parameters is checked against what the call gives.
- A function may read from outside (a collection, an `http` call): each item's read is made,
  recorded and replayed on its own.

## How a flow runs

- **Dataflow, not control flow.** A value is computed when what it needs has arrived. The order
  of statements only matters for names (defined before use).
- **Absence.** A value that doesn't arrive makes everything built from it absent. That is how a
  flow says "not found" (`respond …, missing: 404`), "not allowed" (`x when ok`) and "not
  verified". Absence isn't a value: it can't be stored or compared. `none()` is a value, and
  unrelated.
- **Reads at the start, writes at the end.** `state.x`, `coll[k]` and `dir[name]` are the values as
  the run began. Writes are performed when the run finishes, and only if granted. A run never sees
  its own writes. When two runs change the same item at once, one is run again, so none is lost.
- **Calls out happen between rounds.** An `http.*` call is made when the run gets to it; the run
  continues with the answer. A branch not taken makes no call.
- **Waiting.** `await name` parks the run; a later request to a route that `resumes name`, with
  the token, delivers the event and the run continues. `expires:` sets how long it waits.
- **Sharing is explicit.** A binding is one value used many times. The same expression written
  twice is computed twice.
- **Every run ends.** There are no loops and no recursion: `map` and `fold` go over a list, and
  `range` makes at most 10000 items.

## Types

Values have one of nine types: `Number`, `Boolean`, `Text`, `Record`, `List<T>`, `Product<…>`,
`Sum<…>`, `Effect` and `Any`. You write types only on holon ports, and only if you want them
checked. Wiring a `Text` into a `Number` input is a mistake at the argument. `Any` fits
everything.

## Identity and canonical form

- **A flow's identity is the hash of its graph** (`unit` in `holon check`). Names of bindings and
  holons, comments, blank lines and the order of independent statements aren't in it: two texts
  that build the same graph are the same unit.
- **Any change to what the flow computes is a new version** with a new hash. Every run names the
  version it ran.
- **`holon fmt`** prints the canonical text. Formatting twice changes nothing, and two texts with
  the same graph print the same.

## Mistakes

`holon check`, the editor and `holon lsp` report each mistake at its line and column, with the
expression underlined:

```
$ holon check broken.holon
broken.holon:5:11: unknown name "nope"
    respond nope
            ^^^^
```

| Mistake | Where it points |
|---|---|
| unknown name `x` | the name |
| `x` is a leaf (or a keyword, or a noun); a binding may not take its name | the binding |
| `f` takes `n` inputs, `m` given | the extra or missing argument |
| `f` has no input or parameter `p`; `p` given twice | the name |
| input `p` can't be left out | the call |
| `Text` doesn't fit `Number` | the argument |
| a value nobody uses | the statement |
| `notes` is not a declared collection | the name |
| `http.*`'s host must be written as text | the host |
| `state.k` written twice | both writes |
| `respond` outside a route, or more than one `respond` | the statement |
| `map` gives its function `n` inputs; this one takes `m` | the function |
| a holon's input never used, output never bound, or bound twice | the port or binding |
| two routes share a value | the shared binding |
| two routes clash; not a method; not a route path | the route header |
| this isn't wired to the route's request | the statement |

## The wiring form

Older flows, and flows exported by older editors, may be written in the **wiring form**: one line
per node (`node add-1 add`) and one per wire. It is still read, in the same file as the expression
form, and `holon fmt` rewrites it in the expression form. It is described in
[`docs/LANGUAGE.md`](../../LANGUAGE.md) §10.
