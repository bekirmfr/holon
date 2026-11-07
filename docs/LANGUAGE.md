# The Holon language: flows as programs

**Status:** designed 2026-10-24 and built the same week (§12); written in TypeScript's syntax since
2026-11-06 (§12, step 9), its meanings Holon's. For writing flows, the rules are the
[reference](manual/language/reference.md) and the language by example is the
[tour](manual/language/tour.md); this document is the design: why a language, what each form
lowers to, and what is refused.
**Companions:** `COMPOSABLE_VM_DESIGN.md` (the invariants this language must keep),
`PLATFORM_LAYOUT.md` (the runtime it compiles for),
[`docs/L4s/typescript-syntax-L4.md`](L4s/typescript-syntax-L4.md) (the switch to TypeScript's
syntax, and the slices still to come).

---

## 0. In one page

A Holon flow is a TypeScript module, `NAME.holon.ts`, with Holon's meaning. Its text *lowers to the
exact units the canvas would have placed*: the core's hashed DAG is the intermediate
representation, and the language is a second surface over it, beside the canvas. Nothing in the
semantics moves. Purity, totality, derived manifests, content addressing and exact replay are
properties of the IR, so they hold for every program by construction.

The text began as a netlist (one line per node, one line per wire: the graph serialized, not a
language), became a small language of its own on 2026-10-24 (`.holon`, the expression form this
document first designed), and took TypeScript's syntax on 2026-11-06. The syntax is TypeScript's
so that every editor, every reviewer and every agent already reads it; the meanings that differ
from JavaScript's (absence, writes at the end, logic on booleans) are Holon's, and where a
JavaScript reader would be misled the compiler refuses, saying what to write instead (§1, §6).

Total is the language's boundary, stated here so nobody finds it by surprise: every run ends.
There is no recursion and no loop; `map`, `reduce` and a list's other methods that take a function
go over a list, and `range(n)` counts to at most 10000. Anything with nested or unbounded structure (a tree of any depth, a search that runs
until it finds something) is out of the graph's reach, by design; it goes in a `js()` function,
under a fuel budget.

The guiding rule for the text: **what the flow touches is imported, by name, at the top; what it
does is TypeScript a reader already knows; and a line should read aloud as a sentence.** Every
construct is tested by reading it to someone who has never seen Holon.

```ts
import { flow, route } from "holon"

flow("hello-api", { port: 8101 })

route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
```

```ts
import { every, flow } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("heartbeat")

every("10s", () => {
  const t = now
  state.last_beat = t
  state.beats = (state.beats ?? 0) + 1
  state.first_beat = state.first_beat ?? t
})
```

```ts
route.post("/stripe", async (req) => {
  const verified = stripe.verify(req, stripe_webhook_secret, now)
  const sent = await hooks.post(slack_webhook_path, {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text: `Stripe: ${verified.event.type}` }),
  })
  return sent.status ?? status(400)
})
```

A nested call is a wire. A `const` is a named wire. A function is a holon. A route is one
connected group with a `request` node. `state.x`, a secret's name, `now`, `notes[id]` and
`hooks.post(…)` are each exactly one catalog leaf, and what each reads or calls is imported at the
top of the file. The hash of a program is the hash of the DAG it lowers to, so names and
formatting are cosmetic, and two differently written programs with the same graph are the same
unit.

**The decisions, in one table.** Each is argued in the section named.

| Decision | Choice | Where |
|---|---|---|
| What the language is | A surface over the existing IR; every construct lowers to catalog leaves, templates and composites that exist (two leaves, `as_text` and `coalesce`, were added for it) | §1, §6 |
| Its syntax | TypeScript's: a flow is a valid TypeScript module, read by a TypeScript parser (oxc's). The meanings that differ from JavaScript's are Holon's; where a JavaScript reader would be misled, the compiler refuses and says what to write instead | §1, §2, §6 |
| What it is not | General-purpose. No loops, recursion, classes, generics, imports of code, or standard library growth | §2 |
| The outside world | Imported by name: `import key from "secret:NAME"`, `import state from "state"`, `"collection:NAME"`, `"folder:NAME"`, `"database:NAME"`, `"channel:NAME"`, `"host:HOST"`, `{ now } from "holon:clock"`, `{ random } from "holon:random"`, `{ charge } from "flow:SLUG"`; what started a run is its handler's parameter | §5.1, §5.4 |
| Settings | `flow("name", { port, keep, limits, browsers })`, once, after the imports | §5.1 |
| Writes | Assignment: `state.key = v`, `notes[key] = v`, `delete notes[key]`, `reports[name] = text`. Method forms (`notes.put(k, v)`) when the write is a value to name | §5.4, §5.10 |
| Bindings | `const name = expr`; a `let` given a new value is a new name; destructuring is a `field` per name | §5.2 |
| Calls | `leaf(inputs…, { named… })`: positional arguments are inputs; a last record whose keys the unit takes names inputs and params. A method of a text or list is the leaf that does it | §5.3 |
| Literals | Numbers, texts, `true`, `false`; a template literal interpolates through `as_text`; `{…}` and `[…]` lower to `record`/`with_field` (`merge` for a spread after anything, `insert` for a computed key) and `list`; a duration is a text, `"10s"`, where one is asked | §5.5 |
| Operators | Arithmetic and comparison on numbers, logic on booleans, `??` (fallback), `===` (structural), `.` (field or output), `[ ]` (lookup), `c ? a : undefined` (guard), `c ? a : b` (a guard on each side of `c`), `switch` (a choice by value) | §5.6, §6.4, §6.5 |
| Multi-output values | `.name` selects an output when the name is one; otherwise it is a field. A bare multi-output value is its first output | §5.7 |
| Functions | `function name(inputs) { …; return out }` is a holon, its outputs named by what it returns; an arrow written in place for a list's methods that take one (`map`, `reduce`, `filter`, `toSorted`, …), a name it uses from around it an input of its own | §5.8, §5.9 |
| js functions | `const name = js(function name(inputs) { JavaScript }, { fuel, memory, outputs })`: a function whose body runs as JavaScript, in an engine that is a WebAssembly module with no imports, under a budget | §5.8a |
| Effects | An unused Effect is performed, an unused value is an error; no `perform` keyword | §5.10 |
| Handlers | `route.get("/path", (req) => …)`, `every("10s", () => …)`, `on.file.created(folder, (f) => …)`, `export const name = action((input) => …)`, each with its options last (`{ resumes, matching, log }`); `return` is the answer | §5.1, §5.11 |
| Another flow's action | `import { charge } from "flow:billing"`, then `await charge(x)`: run by the host in-process, answered `{answer, run}` or `{error, message}`, a permission, never a loop | §5.4 |
| A run that waits | A route answers and is done: what waits (`await event(…)`) is an action, which the route starts by calling it without `await`, `payment(x)`, answered `{run, waiting}` with the token for each point it waits at | §5.4 |
| Sharing | A binding used twice is one node with two wires; the same expression written twice is two nodes. No common-subexpression elimination | §6.2 |
| Hash | The lowered DAG's. Binding names, function names, layout and comments are not in it | §7 |
| Layout | Not in the text. Positions live in the flow document; import lays out automatically | §8 |
| Diagnostics | `file:line:col: message`; every node keeps its span, so `check`, replay and the agent tools point at the expression | §9 |
| A canvas the text can't spell | Has no text; the editor says what to change. The wiring form and flows without handlers are gone | §10 |

---

## 1. Why a language, and why now

Three reasons, in order of weight.

1. **Agents write the flows.** The product direction (agent actions you can prove) makes an agent
   the main author. An agent is far better at `text.toLowerCase().split(" ").join("-")` than at
   five node lines and four wires: fewer tokens, fewer places to misname a port, and errors it
   can read. The MCP tools (`check`, `deploy`, `call`, `explain`, step 7) speak this form.
2. **Expressiveness without new primitives.** Most of what takes five nodes on the canvas is one
   nested expression. The vocabulary stays at its leaves (the catalog's, listed in
   [leaves](manual/language/leaves.md)); the text gets shorter.
3. **Review, diff and version control** become ordinary. The derived manifest stays the thing a
   human approves; the text is what they read when the manifest surprises them.

**Why TypeScript's syntax.** The first syntax was Holon's own, and it read unevenly: each kind of
handler (a route, a clock, a file handler, an action) had a shape of its own, a function had two
(one declared, one written in place), and the `js` bodies inside a flow were JavaScript while the
code around them was not. TypeScript makes the whole file one language that people and agents
already know. GitHub, Prettier, ESLint and every editor read it as it is. Its types are optional, as Holon's port
types are, and TypeScript is the one form of JavaScript that can write them. A TypeScript checker
can also say, as the file is written, where an absence isn't handled (with the type package of the
L4's slice 11). Starlark is the precedent: Python's syntax, with semantics of its own where it
needs them, read without effort by everyone who knows Python. The design of the switch, construct
by construct, and what it refuses, is [`docs/L4s/typescript-syntax-L4.md`](L4s/typescript-syntax-L4.md).

And why it costs little: `holon-author` already held the `FlowBuilder` every flow is built with, the
composite freezer and the route derivation. The language is a reader (oxc's TypeScript parser, into
a syntax tree), a lowering of that tree onto the builder, a source formatter, and a printer for
the other direction. The switch to TypeScript replaced the reader and the formatter; the lowering
is the one the first syntax had, so the units did not move with it.

## 2. Principles and refusals

**P1. The IR is the program.** A `.holon.ts` text is a way to write a flow document; the flow
document is a way to write units; the units are what runs, hashes, deploys and replays. The
language adds no construct that does not lower to units that exist. If a construct needs a new
primitive, that primitive goes through the admission gate (design §13) on its own merits first,
and the language gets the sugar after. Two did, for this design, both built on 2026-10-24:
`as_text` (§5.5) and `coalesce` (§5.6). The TypeScript statements still to come (the L4's later
slices: loops as folds over a bounded range, `try` as a fallback) follow the same rule.

**P2. Small, and deliberately not general.** The pull of every language is toward a type system
with generics, modules, a standard library, loops. Each of those would cost a year and would break
the thesis: the manifest is derivable and replay is exact *because* the vocabulary is closed and
every program is total. TypeScript's syntax doesn't change that: the syntax is general, the
language is not, and what TypeScript can write but a graph can't keep is refused. The language is
closer to Dhall or Nix with effects at the edges than to Python. When expressiveness genuinely runs
out, the answer is a sandboxed deterministic WebAssembly primitive (a module with no imports is
pure, and is content-addressed by its bytes), which a flow writes as a `js()` function (§5.8a),
never a language feature.

**P3. Names are cosmetic, the graph is identity.** A binding name, a function name, a comment, the
order of independent statements: none of it reaches the hash. Two programs that lower to the same
DAG are the same unit, and the printer (§11) writes them identically. `fmt` lays each out as its
author wrote it (§7).

**P4. The canvas is a view of the same program.** Text to canvas is compilation; canvas to text is
printing; both go through the IR, and both directions preserve every unit hash. The canvas stays
what it is best at: showing a run, node by node.

**P5. One implementation.** Reader, lowering, formatter and printer live in `holon-author`, pure
like core, and run natively in `holon`, in the server, and in the editor through `holon-wasm`. The
editor has no second copy of any of it in its own code.

**P6. Names are sugar, never magic.** `state.beats` reads as a variable and *is* one `store_read`
node; `notes[id] = v` reads as a mutation and *is* one `collection_put` node whose effect the host
performs when the run finishes. An import names; it grants nothing. The table in §5.4 is
exhaustive: a reader who knows it knows exactly which leaf every form is, and the manifest is still
derived from the leaves, never from the words.

**Refused, with the reason:**

| Refused | Why |
|---|---|
| `while`, `for`, recursion, unbounded iteration | Totality. A list's methods that take a function (`map`, `reduce`, `filter`, `find`, …) are the only iteration, bounded by the list; `range(n)` makes a list to count over, of at most 10000. The L4's slice 6 lowers a computing loop to a fold over a bounded range, which keeps totality; until it is built, a loop is refused |
| `class`, `this`, `new`, generators, `eval` | A flow is a module of functions and records; computing that needs all of JavaScript goes in a `js()` function |
| Generics, type parameters, types beyond the nine | The checker is wire-time compatibility over nine types (§6.8). A type system is a different project |
| Imports of code, packages | An import names something outside a run. Reuse across files is the marketplace's job (a published holon placed by hash), not an import of code (the `marketplace` sample). Revisit when people share holons across flows |
| Globals: `fetch`, `Date`, `setTimeout`, `process` | A flow uses only what it imports, so the top of the file is the whole of what it touches. `JSON`, `Object`'s record functions, `Math`, `Number`, `parseInt`, `parseFloat`, `isNaN`, `isFinite`, `String`, the URI functions, `btoa`, `atob`, `structuredClone`, and `Date` in UTC (`new Date(t)` read by its methods, `Date.UTC`, `Date.parse`) are there, as the leaves that do them (§5.3, §5.6). `new Date()` and `Date.now()` are the time now, which is the clock's: `now * 1000`, `now` imported from `"holon:clock"`. `NaN` and `Infinity` aren't values: a leaf gives nothing where JavaScript gives one |
| `\|\|` and `&&` on anything but booleans | `a \|\| "none"` is JavaScript's fallback, and Holon's logic is on booleans: the fallback is `??` |
| Common-subexpression elimination | The text's structure is the graph's. Sharing is explicit (a binding), so the canvas shows what was written |
| `null` | Absence is not a value. `undefined` is written only for "no value": as a branch, `c ? a : undefined`, and in `x !== undefined`. `none()` is the optional's empty arm (§6.3) |
| Reading a collection or folder after writing it | A run reads its starting state and writes once, at its end, and the syntax looks like JavaScript's, where a write is seen at once. So a read after a write in the same handler is refused, per collection or folder, as it would mislead |
| `try`, `catch`, `throw` | A failure is already an absence (§6.3). The L4's slice 5 reduces them to it; until then they are refused |

## 3. A tour, by the samples

The samples, in the language. `req` is the name a route gives its request; bare, it is the whole
request record. Each sample is a file, `crates/holon-author/samples/NAME.holon.ts`, kept exactly as
`holon fmt` writes it (§7); the texts below are those files with the comments that open them (what
the sample shows, how to try it) left out, and, where a subsection says so, some of their handlers.

### hello-api

```ts
import { flow, route } from "holon"

flow("hello-api", { port: 8101 })

// Greets the name in the path.
route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
```

Lowers to: `request{method: GET, path: /hello/:name}`, `field{name: name}` on its `params`
output, `as_text` of that, `const{type: Text, value: "Hello, "}`, `concat(a, b)`, and
`respond{status: 200, missing: 404}` with `request` and `body` wired. Six nodes, six wires. The
comment is the text's, not the unit's.

### notes-api: a collection

```ts
import { flow, route, status } from "holon"
import notes from "collection:notes"

flow("notes-api", { port: 8102 })

route.get(
  "/notes",
  (req) => notes.list({ after: req.query.after ?? "", limit: 50 }),
  { log: "lazy" },
)

route.put("/notes/:id", (req) => {
  notes[req.params.id] = req.json
  return req.json ?? status(400)
})

route.get("/notes/:id", (req) => notes[req.params.id])

route.delete("/notes/:id", (req) => {
  delete notes[req.params.id]
  return status(204)
})
```

`import notes from "collection:notes"` names the collection; it grants nothing, and the manifest is
still derived from the leaves. `notes[k]` is `collection_get{collection: notes}`, `notes[k] = v`
is `collection_put`, `delete notes[k]` is `collection_delete`, `notes.list({ … })` is
`collection_list` with its optional input `after` and its `limit` param named in a last record
(§5.3). `?? ""` is a `default{type: Text, value: ""}` node. The write in the PUT route is an
Effect nobody reads, so it is exposed and the host performs it when the run finishes (§5.10).
`req.json ?? status(400)` answers 400 when the body isn't JSON, and `GET /notes/:id` answers 404
when there is no such note, as an absent answer does. `status(204)` has no body. `{ log: "lazy" }`
says how the route's runs are recorded (§5.1). `respond(page, { headers: { "content-type":
"text/html; charset=utf-8" } })` sets response headers (a record, text values): a page instead of
JSON, or `respond("", { headers: { location: "/" }, status: 303 })` for a redirect; without it, a
text body goes out as plain text and anything else as JSON (the `guestbook` sample).

### stripe-to-slack

```ts
import { flow, route, status, stripe } from "holon"
import slack_webhook_path from "secret:slack_webhook_path"
import stripe_webhook_secret from "secret:stripe_webhook_secret"
import hooks from "host:hooks.slack.com"
import { now } from "holon:clock"

flow("stripe-to-slack", { port: 8103 })

route.post("/stripe", async (req) => {
  const verified = stripe.verify(req, stripe_webhook_secret, now)
  const sent = await hooks.post(slack_webhook_path, {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text: `Stripe: ${verified.event.type}` }),
  })
  return sent.status ?? status(400)
})
```

`stripe.verify` is the `stripe_verify` template, its `tolerance` param 300 seconds unless a last
record says otherwise; it has two outputs, so `.event` selects one (§5.7). A secret's name is a
`secret{name: …}` node, the secret its import names: someone sets it before the flow works, and
the top of the file says so. `now` is a `clock`. `hooks.post(path, { … })` is
`http_request{method: POST, host: "hooks.slack.com"}` with the path positional and the optional
inputs named; the host is the import's, because it is the permission. `JSON.stringify(x)` is
`json_stringify`. When the signature fails, `verified.event` is absent, nothing downstream fires,
`sent.status` is absent, and the route answers what `?? status(400)` says: 400, and nothing was
posted.

The request is written with `await`, as TypeScript reads one, so the route is `async`. Today
`await` changes nothing about it: a request is answered between rounds whether it is awaited or
not, as it always was; the order two `await`s in a row give, and `Promise.all`, come with the L4's
slice 7.

### guarded-api

```ts
import { flow, hmac_sha256, route, secure_equals, status } from "holon"
import api_token from "secret:api_token"
import receipt_key from "secret:receipt_key" with { optional: "true" }

flow("guarded-api", { port: 8109 })

route.get("/me", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${api_token}`)
  const receipt = hmac_sha256(receipt_key, req.path) ?? "unsigned"
  return (ok ? { receipt } : undefined) ?? status(401)
})
```

`import api_token from "secret:api_token"` names the token the route reads; `with { optional:
"true" }` names a secret it can run without, the deploy setting `optional
SecretRead("receipt_key")`, and its reads have no value when it isn't set. `secure_equals` and
`hmac_sha256` are leaves, imported from `"holon"` before they are called (`hmac_sha256`'s
`encoding` param is `hex` unless a last record says otherwise). `req.path`: `path` is not an
output of `request`, so it is a field of the request record. `ok ? { receipt } : undefined` is
`guard(cond, value)`: when `ok` is false the body is absent and the route answers 401.

### slugs: functions

```ts
import { flow, hmac_sha256, route, status } from "holon"
import state from "state"
import receipt_key from "secret:receipt_key" with { optional: "true" }
import posts from "collection:posts"
import { now } from "holon:clock"

flow("slugs", { port: 8110 })

function slugify(text) {
  const slug = text.toLowerCase().split(" ").join("-")
  return slug
}

function publish(slug, author) {
  state.last_post = slug
  const put = posts.put(slug, { author, at: now })
  const receipt = hmac_sha256(receipt_key, slug) ?? "unsigned"
  return { put, receipt }
}

route.post("/slugs", (req) => {
  const title = slugify(req.json.title)
  const author = slugify(req.json.author)
  const published = publish(title ?? "", author)
  return { title, author, receipt: published.receipt } ?? status(400)
})

route.get("/slugs", () => posts.list())
```

A function is a holon: a composite with a given boundary, frozen exactly as `end_holon_exact`
freezes it. Its parameters are its input ports; what it returns names its outputs: `return slug`
gives one output, `slug`, and `return { put, receipt }` two, `put` and `receipt`. Inside
`publish`, the collection's write is a value, bound and returned in the method form,
`posts.put(k, v)`, the same leaf the assignment form lowers to; the state write leaves the holon
as an output of its own, `write`, as a holon's effects leave it through its boundary and a
boundary port has a name. The route never mentions `put` or `write`: they are Effect outputs
nobody reads, so they are exposed and performed. `slugify` is placed twice and is one unit, one
hash. `text.toLowerCase()`, `.split(" ")` and `.join("-")` are the leaves `lowercase`, `split` and
`join` (§5.3).

### heartbeat: a clock flow

```ts
import { every, flow } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("heartbeat")

every("10s", () => {
  const t = now
  state.last_beat = t
  state.beats = (state.beats ?? 0) + 1
  state.first_beat = state.first_beat ?? t
})
```

`every("10s", () => { … })` is a clock handler: its tick node (`every{seconds: 10}`) is the
trigger of what the block writes, and the server runs it every ten seconds, with no input. A flow
with no port and only clocks is a server of clocks, listening on no port. In a clock, `now` is the
time of the tick. `state.beats` on the right is `store_read{key: beats}`, the run's starting
value; the assignment is `store_write{key: beats}`, committed when the run finishes, which is why
the line means exactly what it says. `const t = now` is one node read twice (§6.2), and
`state.first_beat ?? t` keeps the first beat's time through `coalesce` (§5.6).

In the first syntax heartbeat had no handler: a header set the server's interval and the file's
statements were the run. TypeScript has no way to write a flow without handlers, so it became
one handler, whose tick is its trigger (§10, §12).

### approval: a durable run

```ts
import { action, event, flow, route } from "holon"
import state from "state"
import orders from "channel:orders"

flow("approval", { port: 8105 })

route.post("/orders", (req) => {
  const started = payment(req.json)
  return `/paid/${started.waiting.paid}`
})

export const payment = action(async (order) => {
  orders.send({ item: order.item, status: "waiting for payment" })
  const paid = await event("paid", { expires: "1d" })
  state.last_order = { ...order, paid_via: paid.method }
  orders.send({ item: order.item, status: "paid", via: paid.method })
})

route.get("/paid/:token", () => "thanks, the order is paid", { resumes: "paid" })

route.get("/orders/live", () => orders.subscribe({ from: "kept" }))
```

A route answers and is done, so what waits is a run it starts: `payment(req.json)`, not awaited,
starts the action `payment` with the order (`flow_start{flow: approval, action: payment}`) and is
answered once that run parks, with the token for each point it waits at, so the route answers the
link at once. `await event("paid", { expires: "1d" })` is `await{name: paid, expires: 86400}`. When
the link is opened, the third route delivers the event (`{ resumes: "paid" }` is the request
template's `resume` param), and the order's run finishes: the state write happens then. Statement
order carries no meaning beyond names being defined before use. `{ ...order, paid_via: … }` is
`with_field` over an existing record (§5.5).

A waiting run performs nothing, but its progress goes out: the packs it has settled when it parks,
the ones that don't wait for the payment, are sent then (and logged with the parked part, its
`progress`); the rest when it finishes. The resumed part runs again from the start and settles the
first pack again, the same, and it isn't sent twice. Channels are §5.4 and the live-board sample.

### fetch-json, sign-payload, likes, witness

```ts
import { flow, route } from "holon"
import httpbin from "host:httpbin.org"

flow("fetch-json", { port: 8106 })

route.get("/slideshow", async () => {
  const page = await httpbin.get("/json")
  return JSON.parse(page.body).slideshow.title
})
```

```ts
import { base64, flow, hmac_sha256, route, status } from "holon"
import signing_key from "secret:signing_key"

flow("sign-payload", { port: 8107 })

route.post("/sign", (req) => {
  return (
    { signature: hmac_sha256(signing_key, req.body), body_base64: base64(req.body) } ?? status(400)
  )
})
```

```ts
import { flow, route, status } from "holon"
import likes from "collection:likes"

flow("likes", { port: 8108 })

route.post("/posts/:id/like", (req) => {
  const id = req.params.id
  const n = (likes[id] ?? 0) + 1
  likes[id] = n
  return n ?? status(400)
})

route.get("/posts/:id/likes", (req) => `${likes[req.params.id] ?? 0} likes`)

route.get("/likes", () => likes.list())
```

```ts
import { flow, route, secure_equals, status } from "holon"
import witness_key from "secret:witness_key"
import batches from "collection:batches"
import { now } from "holon:clock"

flow("witness", { port: 8130 })

route.post("/heads", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${witness_key}`)
  const batch = req.json.batch
  const t = now
  batches[batch] = ok ? { ...req.json, witnessed_at: t } : undefined
  return (ok ? { batch, witnessed_at: t } : undefined) ?? status(401)
})

route.get("/batches", (req) => batches.list({ after: req.query.after ?? "", limit: 200 }))
```

A route that reads nothing of its request leaves its parameter out: `async () => …`. likes reads
`likes[id]` before it writes it, as a run must (§2); two likes at once both read the old count,
and the host runs one of them again (§6.6). witness binds `const t = now` and uses it twice: one
`clock` node. Written twice, `now` is two `clock` nodes (§6.2), both reading the same instant, the
run's start. Its write is guarded: without the key, the value is absent and nothing is written.

### inbox-report: folders on the host

```ts
import { flow, js, respond, route } from "holon"
import inbox from "folder:inbox"
import reports from "folder:reports"

flow("inbox-report", { port: 8123 })

const summarize = js(function summarize(name, csv) {
  const lines = csv.split("\n").filter((line) => line.trim() !== "");
  if (lines.length === 0) throw new Error("the CSV has no header line");
  const header = lines[0].split(",").map((cell) => cell.trim());
  const records = lines.slice(1).map((line) => line.split(",").map((cell) => cell.trim()));
  const totals = header.flatMap((column, i) => {
    const numbers = records.map((record) => Number(record[i] ?? ""));
    const all = numbers.length > 0 && numbers.every((n) => !Number.isNaN(n));
    return all ? [`| ${column} | ${numbers.reduce((a, b) => a + b, 0)} |`] : [];
  });
  const report = [`# ${name}`, "", `${records.length} rows.`, "", "| column | total |", "|---|---|", ...totals, ""].join("\n");
  return { rows: records.length, report };
}, { outputs: ["rows", "report"] })

route.post("/report", (req) => {
  const name = req.json.name
  const summary = summarize(name, inbox[name])
  reports[`${name}.md`] = summary.report
  return { rows: summary.rows, report: `${name}.md` }
})

route.get("/reports", () => reports.list())

route.get("/reports/:name", (req) =>
  respond(reports[req.params.name], {
    headers: { "content-type": "text/markdown; charset=utf-8" },
  }),
)
```

`import inbox from "folder:inbox"` names a folder on the host; an admin binds the name to a real
folder (`holon folder add inbox /srv/drop --read-only`, `--for <slug>` for one deployment only, or
the editor's admin page), and a flow never sees the path. A folder reads and writes as a
collection does, by file name: `inbox[name]` is `file_read{folder: inbox}`, the file's text as the
run began (no value if there is none); `reports[name] = text` is `file_write`, the whole file
written when the run finishes; `delete reports[name]` is `file_delete`; `reports.list()` is
`file_list`, a record of file name to `{size, hash, modified}`. A name is a path inside the folder
(`2026/10/a.csv`) and never leaves it. Reading asks `FileRead("inbox")`, writing and deleting
`FileWrite("reports")`, and a deploy is refused until each folder it needs is bound, and bound
read-write where it writes. The run log keeps what a run read (or only its hash, for a folder
bound with `--keep hash`; for a while, then its hash, with `--keep 7d`) and what it wrote by its
hash; a folder bound with `--cap 10M` takes no write past that (the `journal` sample). Design:
`docs/L4s/named-folders-L4.md`. `summarize` is a `js()` function with two outputs (§5.8a).

### log-archive: big files, by handle and by range

The sample without its listing route:

```ts
import { flow, route } from "holon"
import archive from "folder:archive"
import logs from "folder:logs"

flow("log-archive", { port: 8124 })

route.get("/logs/:name/tail", (req) => logs.lines(req.params.name, { from: -20, limit: 20 }))

route.get("/logs/:name/head", (req) => logs.slice(req.params.name, { offset: 0, length: 256 }))

route.post("/archive", (req) => {
  const log = logs.file(req.json.name)
  archive[`${log.name}.${log.hash}`] = log
  return { archived: `${log.name}.${log.hash}`, size: log.size }
})
```

A file of any size never enters a run whole. `logs.file(name)` is a handle,
`{folder, name, size, hash, modified}`, without the bytes; writing a handle to a folder
(`archive[n] = log`, or `archive.copy(n, log)`) copies the file host to host when the run
finishes, and fails if it no longer has the handle's hash. `logs.lines(name, { from: i, limit: n
})` reads `n` lines from line `i` (from the end when negative), and `logs.slice(name, { offset: o,
length: n })` `n` bytes from `o`: only those enter the run, and the log keeps with them the hashes
that prove them part of the file, so `holon verify` checks the run without it.

### drop-box: a run per new file

```ts
import { flow, on } from "holon"
import receipts from "folder:receipts"
import drop from "folder:drop"
import httpbin from "host:httpbin.org"

flow("drop-box")

on.file.created(drop, async (f) => {
  const sent = await httpbin.put("/put", {
    headers: { "content-type": "application/octet-stream" },
    file: f,
  })
  receipts[`${f.name}.json`] = JSON.stringify({
    file: f.name,
    size: f.size,
    hash: f.hash,
    status: sent.status,
    received: JSON.parse(sent.body).headers["Content-Length"] ?? "nothing",
  })
})
```

`on.file.created(drop, (f) => { … })` makes the flow run once for each new file in the folder
`drop`, once the file stops changing: the server looks at the folder every second, by each file's
size and time, and a file the same at two looks in a row starts a run. In that run `f` is the
file's handle, `{folder, name, size, hash, modified, event}`: the `file` output of the handler's
trigger node, `on_file{folder: drop}`, and the deployment asks to run on the folder's files, its
trigger. Files already there when it is first deployed aren't new; files that arrive while the
server is down, or the deployment is paused, start their runs when it is back; one its own runs
wrote into the folder doesn't start it again (unless it changes).

`file: h` in place of `body:` sends a file as the request's body, by handle:
`http_upload{method, host}(path, headers, h)`. The server streams the bytes from the folder, never
through the flow, checks before and as it sends that they still hash to the handle's hash (else
the call answers `{status: 0, error: "changed"}`), and the run log keeps the request with the
handle, never the bytes. The handle's folder must be one the deployment reads, or the one whose
new file started the run.

In the first syntax drop-box had no handler: a header named the folder whose files started it, and
a word of the language was the handle. It is one handler now, and the handle is its parameter.

### file-audit: several folders, removals, some names

```ts
import { flow, on } from "holon"
import audit from "folder:audit"
import inbox from "folder:inbox"
import outbox from "folder:outbox"

flow("file-audit")

on.file.created(inbox, (f) => {
  audit.create(`${f.folder}/${f.modified}-${f.event}-${f.name}.json`, JSON.stringify(f))
}, { matching: "*.csv" })

on.file.deleted(inbox, (f) => {
  audit.create(`${f.folder}/${f.modified}-${f.event}-${f.name}.json`, JSON.stringify(f))
})

on.file.created(outbox, (f) => {
  audit.create(`${f.folder}/${f.modified}-${f.event}-${f.name}.json`, JSON.stringify(f))
})
```

Each `on.file` call is a handler: a CSV new or changed in `inbox` (`{ matching: "*.csv" }`, an
option of the handler, so one folder can have several), any file removed from it
(`on.file.deleted`), any file new or changed in `outbox`. A file no handler is for (a `.txt`
dropped in the inbox) is seen, not run on. `f.event` says which: `new`, `changed` or `deleted`,
and a removed file's handle is as it was last seen, `{folder, name, size, modified, event}`,
without a hash (its bytes are gone). A file its own runs deleted from a folder it watches starts
nothing. `audit.create(n, e)` writes a file that must not exist yet (`file_create`): the run
fails, and nothing it did lands, rather than replace one; `audit.create_copy(n, h)` is the same
for a copy by handle.

### orders-api: a database on the host

```ts
import { flow, route, status } from "holon"
import shop from "database:shop"

flow("orders-api", { port: 8150 })

route.post("/orders", (req) => {
  shop.exec(
    "insert into orders (customer, total) values ($1, $2)",
    req.json.customer,
    req.json.total,
  )
  return status(201, { placed: req.json.customer })
})

route.get(
  "/orders/:customer",
  (req) =>
    shop.query(
      "select id, total, refunded from orders where customer = $1 order by id",
      req.params.customer,
    ),
  { log: "lazy" },
)

route.get("/order/:id", (req) =>
  shop.one("select id, customer, total, refunded from orders where id = $1", req.params.id),
)

route.post("/order/:id/refund", (req) => {
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", req.params.id, {
    expect: 1,
  })
  return { refunded: req.params.id }
})
```

`import shop from "database:shop"` names a database on the host; an admin binds the name to a
SQLite file, or to a Postgres or MySQL database by where its URL is fetched from
(`env:HOLON_SECRET_SHOP_URL`, as a secret's source: `engines.rs`), with the schema that makes its
tables (`holon database add shop ~/shop.db --schema "create table orders (…)"`, `--read-only`,
`--for <slug>`, or the admin page), and a flow never sees where it is or runs DDL; its text is the
same on each engine. `shop.query(sql, v…)` is `db_query{database: shop, sql}(params)`, the rows as
the run began; `shop.one(…)` is `db_one`, the first row or no value; `shop.exec(…)` is `db_exec`,
the statement run when the run finishes, and with `{ expect: n }` only if it changes `n` rows, or
nothing the run wrote lands and the route answers 409. The SQL is a text literal, because its
tables are the grant: core parses it (`sql.rs`) and `select … from orders` asks
`DbRead("shop/orders")`, `update orders …` `DbWrite("shop/orders")`; the values after it are `$1`,
`$2`, …, as many as it takes. The commit re-reads what the run read in the same transaction as its
writes, and marks the run in the database's `holon_commits`. Design: `docs/L4s/databases-L4.md`.

### refund-once: a row held while a run calls out

```ts
import { flow, route, status } from "holon"
import shop from "database:shop"
import httpbin from "host:httpbin.org"

flow("refund-once", { port: 8151 })

route.post("/refund/:id", async (req) => {
  const order = shop.one(
    "select id, total from orders where id = $1 and refunded = 0",
    req.params.id,
  )
  const paid = await httpbin.post("/anything/refunds", {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ order: order.id, amount: order.total }),
  })
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", order.id, {
    expect: 1,
  })
  shop.exec(
    "insert into refunds (order_id, amount, status) values ($1, $2, $3)",
    order.id,
    order.total,
    paid.status,
  )
  return { refunded: order.total, payment: paid.status } ?? status(409)
})

route.get("/refunds/:id", (req) =>
  shop.query("select amount, status from refunds where order_id = $1", req.params.id),
)
```

A run that reads a row it may write and then calls out holds the row (`claims.rs`:
`ClaimKey::Row`) from its first outbound call until it commits, as a collection's key is held: the
second of two refunds at once waits at `httpbin.post`, reads the order again when it is let go,
finds no row, and runs again sending nothing. The row is the one a `where` pins by the table's
whole primary key (core's `sql::pins`, the key read from the database); any other read holds the
table, a file read the file, a listing the folder. These are held in the server's table, by the
resource's real address, so two deployments bound to one database exclude each other. Design:
`docs/L4s/databases-L4.md` §7.2.

### atlas-notes: documents

Two of the sample's routes:

```ts
import { flow, route } from "holon"
import notes from "database:notes"

flow("atlas-notes", { port: 8154 })

route.get("/notes/:owner/tagged/:tag", (req) =>
  notes.find(
    "notes",
    { owner: req.params.owner, tags: { "$in": [req.params.tag] } },
    { sort: { _id: 1 } },
  ),
)

route.post("/notes/:owner/:id/done", (req) => {
  const note = notes.find_one("notes", { _id: req.params.id, owner: req.params.owner, done: false })
  notes.update("notes", { _id: note._id }, { "$set": { done: true } }, { expect: 1 })
  return { done: note._id, text: note.text }
})
```

A database an admin binds to a MongoDB (`mongodb+srv://…`, fetched as a secret is: `mongo.rs`) is
called with records instead of SQL: `find`, `find_one`, `insert`, `update` and `delete`, the
collection in quotes first (the grant, as a statement's tables are), then record literals written
in the call, then the call's options (`{ sort, limit }`, `{ expect }`). The lowering freezes each
record's shape into the leaf as a **template**, JSON whose values may be holes (core's
`documents.rs`): every key, operator and constant is the flow's, and each other value is a hole,
`$1`, `$2`, … in the order written, wired as the leaf's `params` list. So `{ owner:
req.params.owner, tags: { "$in": [req.params.tag] } }` is the frozen `{"owner":$1,"tags":{"$in":[$2]}}`,
and a request can never add an operator: the host fills each hole with its value as data, and a
record value with a `$` field fails the run. A record from a value, or a spread, is an error that
says so; `$where`, `$function` and `$accumulator` are refused anywhere, an update is update
operators only, a sort fields of `1` and `-1`. The printer writes the records back from the
templates, the values in place.

### live-board: a channel

The sample without its page (a `js()` function whose `EventSource` adds each ticket as it arrives)
and the route that serves it:

```ts
import { as_text, flow, route, sha256, slice, status } from "holon"
import tickets from "collection:tickets"
import updates from "channel:updates"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("live-board", { port: 8126 })

route.post("/tickets", (req) => {
  const title = req.json.title
  tickets[`${now}-${slice(sha256(as_text(random)), 0, 6)}`] = { title, at: now }
  updates.send({ title, at: now })
  return status(201, { ok: true, title }) ?? status(400)
})

route.get("/live", () => updates.subscribe())
```

`import updates from "channel:updates"` names a channel: packs the flow sends to whoever
subscribes (`docs/L4s/channels-L4.md`). `updates.send(v)` is an effect,
`channel_send{channel: updates}(v)`: the pack goes out once the run is over, its writes have
landed and its record is on disk (a lazy run's within a second), so a subscriber never hears of a
ticket that wasn't stored, nor of one a power loss took back; a run that fails, or is run again,
sends nothing. A route answers with `updates.subscribe()`: a 200 whose body is the channel's packs
as server-sent events (`id: 12.0`, `data: {"at":…,"title":…}`), while the caller stays.
`subscribe({ from: "kept" })` sends the packs the channel keeps first; a browser's `EventSource`
that reconnects says the last id it saw and gets the kept ones past it. Sending on a channel is a
permission (`ChannelSend("updates")`), and so is letting callers subscribe
(`ChannelRead("updates")`).

### chat-proxy: a streamed reply

Its `/chat` route (the imports, the page and the `js()` function `answer` left out):

```ts
route.post("/chat", async (req) => {
  const prompt = req.json.prompt
  const sent = await anthropic.post("/v1/messages", {
    headers: {
      "content-type": "application/json",
      "x-api-key": anthropic_api_key,
      "anthropic-version": "2026-10-21",
      "anthropic-beta": "server-side-fallback-2026-07-01",
    },
    body: JSON.stringify({
      model: "claude-opus-5-5",
      max_tokens: 1024,
      stream: true,
      fallbacks: "default",
      output_config: { effort: "low" },
      messages: [{ role: "user", content: prompt }],
    }),
    stream: true,
  })
  chats[as_text(now)] = { prompt, said: answer(sent.body) }
  return respond(sent.body, { stream: true }) ?? status(400)
})
```

A call with `stream: true` in its options (after its headers and body) is `http_stream`: a call
like `http_request`, whose answer the server also forwards to the route's caller as it reads it.
The route says so where it answers, `respond(sent.body, { stream: true })`: once the upstream
answers 2xx, the caller gets a 200 with the upstream's content type and each piece as it arrives
(a model's tokens, here), before the run is over. The flow still gets the whole answer when it
ends, and goes on with it as with any call's: `answer`, a `js()` function, reads the text out of
the events, and the run stores it. Nothing the run computes can change what was sent, so a
streamed reply takes no `status` or `headers`: its status is the call's (the respond's status
input, wired from `sent.status`). A call that fails (not 2xx: a bad key, a limit) forwards
nothing, and the reply is its answer, whole, with that status, so the caller sees the 401 and why.
`?? status(400)` is the answer of a run whose call was never made (no prompt). A run that fails
after the first piece ends an event stream with `event: error`. The log keeps the whole answer,
as for any call, so the run replays.

```ts
route.post("/room", (req) => {
  const prompt = req.json.prompt
  const sent = room.pipe(
    anthropic.post("/v1/messages", {
      headers: {
        "content-type": "application/json",
        "x-api-key": anthropic_api_key,
        "anthropic-version": "2026-10-21",
        "anthropic-beta": "server-side-fallback-2026-07-01",
      },
      body: JSON.stringify({
        model: "claude-opus-5-5",
        max_tokens: 1024,
        stream: true,
        fallbacks: "default",
        output_config: { effort: "low" },
        messages: [{ role: "user", content: prompt }],
      }),
    }),
  )
  const said = answer(sent.body)
  room.send({ prompt, said })
  return { prompt, said } ?? status(400)
})
```

`room.pipe(call)` sends the call's answer to the channel `room` instead (`import room from
"channel:room"`): each piece, as it arrives, is a text pack to whoever is subscribed, without an id
and never kept, so a late subscriber gets the packs the run sends once it is over, with ids, and
not the pieces. Piping is sending on the channel (`ChannelSend("room")`); the piped call is still a
value, the whole answer, as any call's. The pieces are the upstream's bytes, never the flow's
computation, so they stay true whatever the run does after: a run that runs again answers the call
from its record and pipes nothing twice. A piped call needn't be in a route.

### ticker: a clock beside routes, and an action

The sample without its page and the route that serves it:

```ts
import { action, every, flow, route } from "holon"
import state from "state"
import ticks from "channel:ticks"
import { now } from "holon:clock"

flow("ticker", { port: 8156 })

every("2s", () => {
  const n = (state.ticks ?? 0) + 1
  state.ticks = n
  ticks.send({ n, at: now })
})

export const reset = action((input) => {
  state.ticks = input.to ?? 0
  return { was: state.ticks ?? 0, set: input.to ?? 0 }
})

route.get("/count", () => state.ticks ?? 0)

route.get("/live", () => ticks.subscribe())
```

A flow with routes may have clocks beside them: `every("2s", () => { … })` runs every two seconds,
apart from the routes, with no input, and `now` in it is the time of the tick. It is a clock
handler in the flow's server, beside the routes' endpoints (`declare = "every"`, `seconds: 2`), and
the deployment asks `Clock`. The handler is everything tied to its tick (§5.11): the write and the
send take it as their trigger, so they are the clock's, though neither reads it. The state is the
flow's, one for every handler: the clock counts, and `GET /count` reads the count.

`export const reset = action((input) => { … })` is run by hand, by its name, and the network never
reaches it: the port answers only the routes. It is exported because it is what reaches the flow
from outside; an `action(…)` that isn't exported is refused, as nothing could reach it. A person
runs it from the editor's server panel or the deployed page, or a script with `POST
/trigger/ticker/reset` on the holon server (as an editor or an admin); `input` is the JSON they
sent, `{}` for nothing, so `{"to": 0}` and nothing both set the count back to 0. What it returns is
the answer to whoever ran it: `state.ticks` on the right is the count at the start of the run
(§6.6), so `was` is the count before the write. The deployment asks `Action("reset")`. An agent
runs it (`holon mcp`'s `run` tool) only once a person lets agents run `reset` in the flow's deploy
settings, which asks `AgentRuns("reset")`: an optional permission, as the flow runs the same
without it.

### shop and billing: a flow calling another's actions

```ts
import { action, flow, route } from "holon"
import charges from "collection:charges"

flow("billing", { port: 8158 })

export const charge = action((input) => {
  charges[input.id] = { amount: input.amount, fee: 1 }
  return { id: input.id, total: input.amount + 1 }
})

export const refund = action((input) => {
  delete charges[input.id]
})

route.get("/charges/:id", (req) => charges[req.params.id])
```

```ts
import { flow, route, status } from "holon"
import orders from "collection:orders"
import { charge, refund } from "flow:billing"

flow("shop", { port: 8159 })

route.post("/orders", async (req) => {
  const paid = await charge({ id: req.json.id, amount: req.json.amount })
  orders[req.json.id] = { amount: req.json.amount, total: paid.answer.total }
  return status(201, paid.answer) ?? status(502)
})

route.delete("/orders/:id", async (req) => {
  await refund({ id: req.params.id })
  delete orders[req.params.id]
  return status(204)
})

route.get("/orders/:id", (req) => orders[req.params.id])
```

`import { charge, refund } from "flow:billing"` names the other flow's actions, and `await
charge(x)` runs one with `x` (`flow_call{flow: billing, action: charge}`): the shop asks to "run the
action charge of the flow billing" (and refund), which a person approves like any permission, and
the call is in the shop's hash (§5.4). The call answers `{answer, run}`; when billing can't be
reached (stopped, not deployed, charge revoked) it answers `{error, message}` instead, so
`paid.answer` has no value, the order isn't stored, and `?? status(502)` answers 502: the failure
is handled by absence, as a failed request is. `refund` only writes, so the shop ignores its
answer, and a call nobody uses is a statement. billing logs each as an ordinary action run by
"flow shop, run …", and the shop's log keeps the call and its answer, so `holon verify` replays the
shop from its own record without calling billing again.

### map and reduce

From the `quote` sample: each line's function uses `tax` from the route around it.

```ts
import { flow, route } from "holon"

flow("quote", { port: 8112 })

route.post("/quote", (req) => {
  const tax = req.json.tax ?? 0
  const lines = req.json.items.map((item) => ({
    name: item.name,
    total: item.price * item.qty * (1 + tax),
  }))
  const total = lines.reduce((acc, line) => acc + line.total, 0)
  return { lines, total, tax }
})
```

`xs.map(f)` is the `map` leaf, `xs.reduce(f, start)` the `fold` leaf with the start first. `f` is a
function's name or an arrow written in place; either lowers to a composite whose hash becomes the
frozen `fn` param, which is what `eval_map` and `eval_fold` resolve. What an arrow uses from around
it reaches it as an input after the item (§5.9): `tax` here.

## 4. Lexical structure

- **A file** is `NAME.holon.ts`: a TypeScript module, UTF-8, lines
  ending in `\n` or `\r\n`. It is read by oxc's TypeScript parser, so what isn't TypeScript is a
  mistake at its place, in the parser's words, before Holon's rules apply.
- **Comments:** `//` to the end of the line, and `/* … */`. They are the text's and never the
  unit's. `fmt` keeps every comment where it was written, with its statement (§7).
- **Keywords** are TypeScript's; Holon adds none. Its vocabulary is names imported from `"holon"`:
  `flow`, `route`, `every`, `on`, `action`, `js`, `respond`, `status`, `stripe` and `event`, and
  every leaf, each imported by name before it is used (`import { sha256 } from "holon"`). A name
  is imported once.
- **Identifiers** are TypeScript's. Any name may be bound; names aren't in the hash (§7), except a
  function's parameters, which are its ports.
- **Numbers:** a finite decimal: `0`, `86400`, `2.5`, `1e3`, `1_000_000` (the separators are left
  out of the value). A minus before a number is part of it (`-1`); before any other value it is
  `x * -1` (§5.6). No infinity, no NaN, since `const` refuses them.
- **Texts:** `"…"` or `'…'` with JavaScript's escapes, and template literals, `` `Hello, ${name}` ``,
  whose `${…}` parts are any expression (§5.5).
- **Durations** are texts where one is asked: `every("10s", …)`, `event("paid", { expires: "1d" })`,
  a `_ms` or `_seconds` limit (`"50s"`, `"1m"`). A number with a unit, `s`, `m`, `h` or `d`,
  lowered to a whole number of seconds (`"5m"` is `300`), or of milliseconds for a `_ms` limit.
  Anywhere else a duration is a number of seconds, written as one (`now + 86400`).
- **Budgets:** a `js()` function's `fuel` and `memory` are a whole number (`1_000_000_000`) or a
  text with its scale (`"50M"`, `"2K"`, `"16MB"`).
- **Route paths:** a text literal, `/` followed by segments of `[A-Za-z0-9_.-]+` or `:name`, as
  `parse_path` accepts.
- **Import sources** are text literals of a fixed shape: `"holon"`, `"state"`, `"secret:NAME"`,
  `"collection:NAME"`, `"folder:NAME"`, `"database:NAME"`, `"channel:NAME"`, `"host:HOST"`,
  `"holon:clock"`, `"holon:random"`, `"flow:SLUG"`. An import attribute, `with { optional: "true"
  }`, is in quotes, as an attribute's value is a text.

## 5. Syntax and lowering

The grammar is TypeScript's. This section says, for each form a flow may use, the leaf or template
it lowers to; a form not listed is refused, with a message that says what to write instead (§9).
The reader (`lang/ts.rs`) turns the text into a syntax tree; the lowering (`lang/lower.rs`) places
that tree's nodes through the `FlowBuilder`, every node from the live catalog, so a text and the
same flow drawn on the canvas are the same unit. A file, in shapes:

```
import { flow, route, every, on, action, js, respond, status, stripe, event, LEAF… } from "holon"
import NAME from "secret:NAME"                       (with { optional: "true" })
import state from "state"
import NAME from "collection:NAME"                   (and folder:, database:, channel:)
import NAME from "host:HOST"
import { now } from "holon:clock"
import { random } from "holon:random"
import { ACTION, … } from "flow:SLUG"

flow("NAME", { port, keep, limits, browsers })

const NAME = LITERAL
function NAME(input, input: type, …) { statements; return outputs }
const NAME = js(function NAME(inputs) { JavaScript }, { fuel, memory, outputs })

route.METHOD("/path", (req) => answer, { resumes, log })
every("DURATION", (now) => { … }, { log })
on.file.created(FOLDER, (f) => { … }, { matching, log })
on.file.deleted(FOLDER, (f) => { … }, { matching, log })
export const NAME = action((input) => { … }, { log })
```

### 5.1 Imports, settings and handlers

An import names something the flow uses from outside a run; its source says what it is.

| Import | Names | Lowers to |
|---|---|---|
| `import { flow, route, … } from "holon"` | the vocabulary and the leaves | nothing: names |
| `import key from "secret:NAME"` | a secret set for the deployment, read as `key` | `secret{name: NAME}` at each read |
| `import key from "secret:NAME" with { optional: "true" }` | a secret the flow can run without | the same, and the deploy setting `optional SecretRead("NAME")` |
| `import state from "state"` | the flow's state, its keys by use (`state.hits`) | `store_read{key}`, `store_write{key}` |
| `import notes from "collection:notes"` | a collection, under its own name | the collection leaves (§5.4) |
| `import inbox from "folder:inbox"` | a named folder on the host | the file leaves |
| `import shop from "database:shop"` | a named database on the host | the database leaves |
| `import updates from "channel:updates"` | a channel | `channel_send`, `channel_subscribe`, a piped call |
| `import api from "host:api.example.com"` | a host the flow calls | `http_request{host}` and its kin |
| `import { now } from "holon:clock"` | the time the run started | `clock()`; in a clock, the tick |
| `import { random } from "holon:random"` | one random number for the run | `random()` |
| `import { charge } from "flow:billing"` | an action of another deployed flow | `flow_call{flow: billing, action: charge}`, or `flow_start` when not awaited |

**The imports name everything that comes from outside a run or outlives it; the handlers use only
names the imports gave.** So the top of a file is what the flow needs provided, read without the
body: the review an approver makes. Three things are named that the first syntax inferred, the
outbound host, the clock and randomness, so a reviewer reads them in the first lines. An import
names; it grants nothing: what the flow may do is worked out from what it does with what it
imports, so a collection the flow only reads asks only to read it, and an import nothing uses asks
for nothing. ES imports are static by definition, at the top, with a literal source, which is the
rule a manifest needs: a secret or a host can't come from a value. A state key is a name
(`state.hits`), never computed, as `store_write`'s key is a frozen param. A name used without its
import is an unknown name, and the mistake says so. For now, a collection, folder, database or
channel is imported under its own name, and only a secret is optional.

**Settings.** `flow(NAME, { … })` comes once, after the imports, its arguments literals. Its name is
what the flow deploys as; its options are the deploy settings (`Settings`):

| Option | Meaning |
|---|---|
| `port: 8102` | A flow with routes: the port its server answers on |
| `keep: { runs: 500, days: 30 }` | How many runs the deployment's log keeps, and for how many days, the limits `keep_runs` and `keep_run_days`; either alone works |
| `limits: { run_budget_ms: "1m", request_timeout_ms: "50s" }` | Any other limit of the deploy settings, by its name there (`docs/manual/reference/permissions.md`): a `_ms` or `_seconds` limit takes a duration as text or a whole number in its unit, the others a whole number. Below the host's default asks nobody; above it asks for approval, `Limit(run_budget_ms=60000)`, up to the host's ceiling. A `js()` function's fuel and memory are its own options, not a limit |
| `browsers: ["https://app.example"]` | The web origins allowed to call the flow from a browser (CORS), or `"*"`; one may be written alone, `browsers: "*"` |

**Handlers start runs.** A flow has one or more, each a call at the top level whose function is
the handler and whose options come last; each is everything tied to its trigger (§5.11), and a
statement in it tied to nothing is an error, as is a handler that does nothing.

- **A route,** `route.get("/path", (req) => …)`, and `post`, `put`, `patch`, `delete`, runs on each
  request to the flow's port, `req` the request (§5.11). It lowers to a `request{method, path,
  resume, log}` template node, its trigger.
- **A clock,** `every("2s", (now) => { … })`, runs every 2 seconds, with no input: its parameter,
  or `now` imported, is the time of the tick, and a clock that reads neither leaves it out. Its
  period is a whole number of seconds, from `"1s"` to `"366d"`; one clock per period. It lowers to
  an `every{seconds, log}` node, its trigger, whose output `now` is the tick's time; a flow with a
  clock asks `Clock` whether or not it reads the time.
- **A file handler,** `on.file.created(inbox, (f) => { … })`, runs for each file new or changed in
  the folder `inbox` imports, once the file stops changing; `f` is its handle (`.name`, `.size`,
  `.hash`, `.folder`, `.modified`, `.event`). `on.file.deleted` runs for files removed from the
  folder, and the option `{ matching: "<glob>" }` for the names that match (`*` and `?` within a
  folder, `**/` any number of folders, letters either case). Several handlers may watch one
  folder, each with its own pattern. It lowers to an `on_file{folder, on, matching, log}` node, its
  trigger, whose output `file` is the handle.
- **An action,** `export const reset = action((input) => { … })`, runs when someone runs it by its
  name, `input` what they sent (a JSON value; `{}` when they sent nothing). It answers with what it
  returns, a value alone (a route's `status(…)` and `respond(…)` are not an action's); an action
  that only writes answers nothing. It is exported, as it is what reaches the flow from outside: a
  person runs it from the editor, the deployed page or a script (`POST /trigger/<slug>/<name>` on
  the holon server, as an editor or an admin), an agent only one a person lets agents run, in the
  flow's deploy settings, and another flow by importing it. An `action(…)` that isn't exported is
  refused. It lowers to an `action{name, log}` node, its trigger.

A handler that reads nothing it receives leaves its parameter out: `() => "ok"`. A route's or an
action's parameter may be destructured (`({ params, json }) => …`, §5.2).

**How a handler's runs are recorded.** Each run of a handler is a line of the deployment's log (its
record, what `holon runs`, revert and the witness read), and by default the line is on disk before
the run's answer: the handler is *strict*. Its option `log` says otherwise:

- `"lazy"`: the run is recorded, and on disk within a second of its answer. A crash in that second
  loses the line, and only the line: the run did nothing outside itself.
- `"unrecorded"`: the run is not a line of the log, failed or not. It is counted (how many, how
  many failed, the last failure's status and error, never its request), but it isn't in the runs
  table, so it can't be replayed, reverted or explained. To see why one fails, make it `"lazy"` for
  a while. Each such handler is an item a person approves at deploy, `Unrecorded("GET /health")`;
  not approved, or taken back, the handler is strict. For what is called often and says nothing
  worth keeping: a health check, a page of what is already recorded.

```ts
import { action, flow, route } from "holon"
import notes from "collection:notes"
import seen from "collection:seen"

flow("status-board", { port: 8170 })

route.get("/health", () => "ok", { log: "unrecorded" })

route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })

route.post("/notes/:id/seen", (req) => {
  seen[req.params.id] = req.json.by
  return "ok"
}, { log: "lazy" })

export const report = action(() => notes.list({ limit: 50 }), { log: "unrecorded" })
```

Either level goes on any handler, as its author chooses: `"unrecorded"` where a run's record isn't
worth keeping (a health check, a handler that only reads, one called so often its records would
only fill the log), `"lazy"` where a handler must answer fast and losing its last second of records
to a crash is fine. A handler that writes, calls out (a request, another flow's action), sends on a
channel or parks a durable run is what it says too: an unrecorded one's runs aren't in the log, and
a crash may lose a lazy one's last second, so `holon verify` leaves the state they may have moved
unchecked (not a break), a restore's dry run says unrecorded runs aren't counted, an unrecorded
run's channel messages go to whoever is listening then, not kept, and a lazy run's go out once its
record is on disk, within a second. A parked run keeps what it waits with, and what it sent as it
parked, whatever its level, and each part after is recorded at its handler's. `"strict"` may be
written and is what an unsaid handler is; it is never written into the unit. The level is the
trigger's `log` param (`docs/L4s/request-path-L4.md`, C5): a strict handler's unit is the one it
always was, and a lazy one's is another.

**The top level** holds the imports, the `flow(…)` line, the handlers, functions, `type` and
`interface` declarations, and `const`s of literals (§5.11); anything else is refused, saying to
move it into a handler. **Every file has handlers.** A file with none has nothing to run: `check`
finds its canvas empty and deploy refuses it; to run its statements by hand, put them in an
exported action. The first syntax's flows without handlers (a header naming a schedule or a
folder, the file's statements the run) are gone: TypeScript can't write one, and each became one
handler (§10, §12).

### 5.2 Statements

| Statement | Lowers to |
|---|---|
| `const name = expr` | a binding: a name for the node `expr` places, no node of its own |
| `let name = expr`, `name = expr`, `var name = expr` | a new name per assignment; where branches assign it, the value after them is the branch taken's, chosen as `?:` and `switch` choose (`guard`, `coalesce`). `var` reads as `let`, and `fmt` writes it as `const` or `let` |
| `n++`, `n += 2`, `x ??= y` | an assignment of `n + 1`, `n + 2`, `x ?? y`; `%=`, `**=`, `&=`, `\|=`, `^=`, `<<=`, `>>=` and `>>>=` likewise, each its operator's leaf (§5.6) |
| `xs.push(x)`, `order.total = t` (on a local) | a new value: `[...xs, x]` (`append`), `{ ...order, total: t }` (`with_field`) |
| `Object.assign(t, a, b)` alone, on a local | a new value, `{ ...t, ...a, ...b }` (`merge`); on a parameter or a record another name holds, refused, as for `push` |
| `const d = new Date(t)`; `d.setUTCHours(h, m)` alone; `d = new Date(t2)` | `d` is a Date, held as its time value (`date_part{part: time}(t)`) and read only by a Date's methods (§5.3); a setter alone gives `d` a new value, `date_set{part: hours}(d, h, m)`, and `const ms = d.setUTCHours(h)` gives it that and `ms` the time value; a setter elsewhere in an expression is the time value, and `d`, changed in place, is read no more; `d` read any other way is refused, naming `d.getTime()` and `d.toISOString()` |
| `Object.freeze(r)` alone, `const r = Object.freeze(x)` (`seal`, `preventExtensions`) | `r` itself, marked frozen: a later change in place (`r.k = v`, `delete r.k`, `Object.assign(r, …)`), which would throw in JavaScript, is refused, naming a new record |
| `xs.sort()`, `xs.reverse()`, `xs.fill(v)`, `xs.copyWithin(t, s)` alone; `xs.pop()`, `shift()`, `unshift(x)`, `splice(…)` | refused, naming what to write: `xs = xs.toSorted()`, `xs = xs.toReversed()`, `xs = [...xs].fill(…)`; `xs.at(-1)` and `xs.slice(0, -1)`, `xs.at(0)` and `xs.slice(1)`, `[x, ...xs]`, `xs.toSpliced(…)` (a flow's list doesn't change) |
| `const { a, b = 1, ...rest } = r` | a `field` per name; `b = 1` is `r.b ?? 1` (`default`, or `coalesce` when the default is computed); `rest` is `without_field` per named key |
| `const [x, y] = xs`, `const [first, ...rest] = xs` | `get` at each index; `head` and `tail` |
| `state.key = expr` | `store_write{key}`, performed when the run finishes |
| `coll[key] = expr`, `delete coll[key]` | a collection's or folder's write or delete (§5.4) |
| `if (c) …; else …` | its writes guarded by their branch's condition; writes to one key in both branches are one write, of `c ? a : b` |
| `if (x !== undefined) { … }` | its writes and calls tied to `x` (§5.11) |
| `switch (s) { case …: … }` | a choice by value (§6.5) |
| `return e` | the handler's answer (§5.11), or a function's outputs (§5.8) |
| `expr` | an effect (§5.10) |

A name bound in a handler is visible after its binding, in that handler; functions and top-level
`const`s are visible to the whole file, in any order. Destructuring places only the leaves the
catalog has (`field`, `default`, `without_field`, `get`, `head`, `tail`); a name a pattern binds
is a binding like any other.
A handler's parameter may be a pattern: a route's request is already split into `params`, `query`,
`headers`, `body` and `json` (its outputs), so `({ json: { move } }) => …` reads one field of one
output, the graph `req.json.move` gives. A function's parameter is a name, destructured on the
first line, as its ports are named by its parameters (§5.8).

```ts
import { flow, route } from "holon"
import state from "state"

flow("fees", { port: 8210 })

route.post("/fee", (req) => {
  const { amount, tier = "basic" } = req.json
  let fee = 0
  if (amount > 100) fee = 5
  else if (tier === "vip") fee = 1
  if (fee > 0) state.last_fee = fee
  else state.last_fee = 0
  return { amount, fee }
})
```

`fee` is three names in the graph, one per assignment, and the one the record reads is chosen
among them by the two conditions; the two writes to `state.last_fee` are one `store_write`, of the
value its branch chose, as a run writes a key once.

### 5.3 Calls: inputs in order, a last record for the rest

```
leaf(input, input, { name: value, … })
```

Positional arguments are **inputs**, matched to the unit's input ports in catalog order; each
becomes a wire. A last record is the call's named inputs and **params** when each of its keys is
one the unit takes by name, and with the arguments before it gives every input that can't be left
out; otherwise it is a record like any other, passed in order. A key is an input when the unit has
an input port of that name (`api.post(path, { headers: h, body: b })`, `notes.list({ after: a })`),
otherwise a param, frozen into the node (`{ encoding: "hex" }`, `{ limit: 50 }`).

- A leaf is imported from `"holon"` before it is called: `import { hmac_sha256 } from "holon"`.
- The one unit with an input and a param of the same name is `default`, and it is never written:
  `??` is its spelling.
- Too many positional arguments is an error, except on a variadic leaf (`add`, `multiply`, `and`,
  `or`, `concat`, `list`, `from_codes`), where each extra argument grows the node by one input, named as the
  canvas names them (`c`, `d`, … then `in<n>`), through `FlowBuilder::arity`; a variadic leaf's
  inputs aren't named.
- An optional input (`http_request`'s `headers` and `body`, `collection_list`'s `prefix` and
  `after`, as `is_optional_input` and core's `has_optional_ports` say) may be left out. Any other
  input left out is an error naming the port.
- An unknown name, or a name given twice, is an error naming the unit. Params not given take the
  catalog's defaults, as a dropped node does.
- A zero-input leaf written as a call still takes parentheses: `record()`, `none()`.

Every catalog leaf is callable by its own name with this rule (`concat(a, b)`, `select(c, a, b)`,
`field(r, { name: "id" })`); the forms of §5.4 and the methods below are sugar over those calls.

**Methods are leaves.** A text's or list's method is the leaf that does it:

| Written | Lowers to |
|---|---|
| `t.toLowerCase()`, `t.toUpperCase()` | `lowercase(t)`, `uppercase(t)`; `toLocaleLowerCase()` and `toLocaleUpperCase()` the same, without a locale (one given is refused, as a flow has none) |
| `t.split(sep)`, `t.split(sep, n)`, `xs.join(sep)` | `split(t, sep)`, the limit wired when given (`""` cuts every code unit); `join(xs, sep)` |
| `t.length`, `xs.length` | `text_length(t)` where `t` is known to be a text, `length(xs)` where `xs` is known to be a list: a node whose output is a `Text`, or a `List`. On anything else (a request's field, a function's parameter), refused, naming both, to call by name |
| `t.charAt(i)`, `t.charCodeAt(i)`, `t.codePointAt(i)` | `text_at(t, i)`, `code_at{mode: unit, point}(t, i)`; the index left out is 0 |
| `t.startsWith(s, p)`, `t.endsWith(s, e)` | `text_contains{at: start, end}(t, s, p)`, the position unwired when left out |
| `t.substring(a, b)` | `slice{mode: substring}(t, a, b)`, the end unwired when left out |
| `t.trim()`, `t.trimStart()`, `t.trimEnd()` | `trim{side: both, start, end}(t)` |
| `t.padStart(n, f)`, `t.padEnd(n, f)`, `t.repeat(n)` | `pad{side: start, end}(t, n, f)`, the fill unwired when left out (a space); `repeat(t, n)` |
| `t.replace(a, b)`, `t.replaceAll(a, b)` | `replace{count: first, all}(t, a, b)`: `a` a text, `b` JavaScript's replacement (`$&` the match, `$$`, `` $` ``, `$'`); a regular expression or a function in their place is refused, naming `js()` |
| `t.normalize()`, `t.normalize("NFD")` | `normalize{form}(t)`, the form one of `"NFC"`, `"NFD"`, `"NFKC"`, `"NFKD"` written in place |
| `t.isWellFormed()`, `t.toWellFormed()` | `is_well_formed(t)`; `concat(t, "")`: a flow's text is always well formed |
| `t.concat(a, b)` | `concat(t, a, b)` where `t` is a text written in place or known to be one, each argument as `String(x)` writes it (`as_text`), a text as itself; on anything else, a list's (below) |
| `String(x)`, `x.valueOf()` | `as_text(x)`, as a template writes `x`; `x` itself |
| `String.fromCharCode(a, b)`, `String.fromCodePoint(a, b)` | `from_codes{mode: unit, point}(a, b)`, variadic; with none, `""` |
| `` String.raw`a\n${x}` `` | the template of its text as written, escapes kept |
| `encodeURIComponent(t)`, `encodeURI(t)`, `decodeURIComponent(t)`, `decodeURI(t)` | `uri_encode{set: component, uri}(t)`, `uri_decode{set: component, uri}(t)` |
| `btoa(t)`, `atob(t)` | `base64{bytes: latin1}(t)`, `base64{bytes: latin1, mode: decode}(t)` |
| `xs.map(f)` | `map{fn: f}(xs)` (§5.9) |
| `xs.reduce(f, start)` | `fold{fn: f}(start, xs)` (§5.9) |
| `xs.map((x, i) => …)` | `indices(xs).map((i) => …)`, `x` read as `get(xs, i)`; `xs` is a name or a field of one, as it is read twice |
| `xs.reduce(f)`, `xs.reduceRight(f, start)` | `fold{fn: f}(head(xs), tail(xs))`, `xs` a name or a field of one; `fold{fn: f}(start, reverse(xs))` |
| `xs.filter(f)`, `xs.some(f)`, `xs.every(f)`, `xs.find(f)`, `xs.findIndex(f)`, `xs.findLast(f)`, `xs.findLastIndex(f)` | `filter`, `any`, `all`, `find`, `find_index`, `find_last`, `find_last_index`, each `{fn: f}(xs)`: `f`'s answer read as JavaScript's truthiness, and `some`, `every` and the finds apply `f` to nothing after the item that decides |
| `xs.toSorted(f)`, `xs.toSorted()` | `sort_by{fn: f}(xs)`, a stable merge sort on the sign of `f(a, b)`; `sort(xs)`, by each item's `String(x)` in UTF-16 code units, as JavaScript sorts |
| `xs.flatMap(f)`, `xs.flat()`, `xs.flat(2)`, `xs.flat(Infinity)` | `flat(map{fn: f}(xs))`; `flat{depth}(xs)`, `depth: all` for `Infinity` (a number written in place) |
| `xs.at(i)`, `xs.includes(x, i)`, `xs.indexOf(x, i)`, `xs.lastIndexOf(x, i)` | `at`, `includes`, `index_of`, `last_index_of`: each one leaf for a list and a text; the position to start from, when given, wired to `from` |
| `xs.slice(s, e)`, `xs.slice(s)`, `xs.slice()` | `slice{mode: end}(xs, s, e)`, the end unwired when left out, on a list or a text; `xs` itself |
| `xs.toReversed()`, `xs.toSpliced(s, c, ...items)`, `xs.with(i, v)` | `reverse(xs)`, `to_spliced(xs, s, c, items)` with the rest as one list, `with_item(xs, i, v)` |
| `xs.sort()`, `xs.sort(f)`, `xs.reverse()`, `xs.fill(v, s, e)`, `xs.copyWithin(t, s, e)` as a value | `sort`, `sort_by`, `reverse`, `fill`, `copy_within`: the list they give, on a list made where they are called (`[...xs].fill(0)`) or a binding of the handler's own that isn't read after; on a parameter, a part of one or a list another name holds, refused, naming the value form |
| `[...xs.keys()]`, `[...xs.entries()]`, `[...xs.values()]`, each also as `Array.from(…)` | `indices(xs)`, `entries(xs)` (the `[index, item]` pairs), `xs` |
| `[...new Set(xs)]`, `Array.from(new Set(xs))` | `unique(xs)`: each item's first occurrence, by value |
| `xs.join()`, `xs.concat(ys, x)` | `join(xs, ",")`; on a value not known to be a text, `extend` for a list written as one, `append` for a literal that isn't one, and refused for anything else, which could be either |
| `Array.isArray(x)`, `Array.of(a, b)`, `Array.from(xs)`, `Array.from(xs, f)`, `Array.from({ length: n }, (_, i) => …)` | `is_list(x)`, `[a, b]`, `xs`, `map{fn: f}(xs)`, `range(n).map((i) => …)` (the item, JavaScript's undefined, read as absent) |
| `JSON.parse(t)`, `JSON.stringify(x)` | `json_parse(t)`, `json_stringify(x)` |
| `JSON.parse(t, (k, v) => …)`, `JSON.stringify(x, (k, v) => …)` | `json_parse{fn}(t)`, `json_stringify{fn}(x)`: the reviver applied bottom-up, the replacer top-down, as JavaScript's; what the function gives nothing for is left out of a record and `null` in a list |
| `JSON.stringify(x, ["id", "name"], 2)`, `JSON.stringify(x, null, "\t")` | `json_stringify{keys: "[\"id\",\"name\"]", indent: "  "}(x)`: the keys a list of texts written in place, the indent a number written in place (that many spaces, ten at most) or a text; `null` and `undefined` leave either out |
| `Object.keys(r)`, `Object.getOwnPropertyNames(r)` | `keys(r)` |
| `Object.values(r)`, `Object.entries(r)`, `Object.fromEntries(pairs)` | `values(r)`, `entries(r)` (the `[key, value]` pairs, in key order), `from_entries(pairs)` (a later pair for a key replacing the earlier, a key a text) |
| `Object.groupBy(xs, (x) => …)` | `group_by{fn}(xs)`: a record of lists, each item under the text its function answers (an item it answers nothing or no text for is left out); `Map.groupBy` is refused, as a flow has no Map |
| `Object.assign({}, a, b)`, `Object.assign({ k: v }, a)` | `merge(a, b)`, `merge({ k: v }, a)`: into a record written in place; into anything else a change in place, refused, naming `{ ...a, ...b }` |
| `Object.hasOwn(r, k)`, `r.hasOwnProperty(k)`, `k in r` | `has(r, k)`: a record has no prototype, so `in` is its own keys |
| `Object.is(a, b)` | `same_value(a, b)`: `===` but for `NaN` and `-0`, by value |
| `typeof x` | `type_of(x) ?? "undefined"`: `"number"`, `"string"`, `"boolean"`, `"object"` (a record, a list or `null`), and `"undefined"` where nothing arrives |
| `structuredClone(x)`, `Object.freeze(x)`, `Object.seal(x)`, `Object.create(null)` | `x`, `x`, `x`, `{}`: a flow's values never change in place, so a copy is the value and each is frozen already |
| `new Date(t).getTime()`, `.getUTCFullYear()`, `.getUTCMonth()`, `.getUTCDate()`, `.getUTCDay()`, `.getUTCHours()`, … | `date_part{part: time, year, month, date, day, hours, minutes, seconds, milliseconds}(t)`, `t` a time value in milliseconds or a text `Date.parse` reads; on a Date bound to a name, the same of its time value |
| `new Date(t).toISOString()`, `.toUTCString()` | `date_text{format: iso, utc}(t)` (`toJSON` and `toGMTString` the same) |
| `new Date(t).setUTCHours(h, m)`, `.setUTCFullYear(y)`, … as a value | `date_set{part: hours, year, …}(t, h, m)`: the time value the setter gives, as many numbers as it takes |
| `Date.UTC(y, m, d, …)`, `Date.parse(text)` | `date_utc(y, m, d, …)` (variadic, 1 to 7 numbers), `date_parse(text)` |
| `Math.round(x)`, `Math.sqrt(x)`, … | `math{fn: round}(x)`: each of `Math`'s one-number functions, `f16round` and `clz32` among them, is `math` with its name |
| `Math.max(a, b, …)`, `Math.min(a, b, …)` | `max(a, b, …)`, `min(a, b, …)`, variadic |
| `Math.max(...xs)`, `Math.min(...xs)`, `Math.hypot(a, b, …)`, `Math.hypot(...xs)`, `Math.sumPrecise(xs)` | `aggregate{fn}(xs)`, the list built in place for `hypot`'s numbers |
| `Math.atan2(y, x)`, `Math.pow(a, b)`, `Math.imul(a, b)` | `atan2(y, x)`, `power(a, b)`, `bitwise{op: imul}(a, b)` |
| `Math.PI`, `Math.E`, `Number.EPSILON`, `Number.MAX_SAFE_INTEGER`, … | the constant's literal: `3.141592653589793` |
| `Number(x)`, `parseFloat(s)`, `parseInt(s)`, `parseInt(s, r)` | `to_number{grammar: number, float or int}(x)`, the radix wired when given; `Number.parseFloat` and `Number.parseInt` the same |
| `Number.isFinite(x)`, `Number.isInteger(x)`, `Number.isSafeInteger(x)`, `isNaN(x)`, `isFinite(x)` | `number_test{test, coerce}(x)`: `coerce: none` for `Number.`'s, `number` for the globals |
| `Number.isNaN(x)` | `false`: a flow has no NaN |
| `x.toFixed(d)`, `x.toPrecision(p)`, `x.toExponential(d)`, `x.toString(r)` | `number_format{style}(x, d)`, `d` wired when given |
| `x.toString()` | `number_format{style: string}(x)` when `x` is known to be a number (a node whose output is a `Number`); otherwise `as_text(x)`, as `String(x)` |

A Date is JavaScript's in UTC: a flow has no time zone, so a local-time method (`getHours`,
`setDate`, `toString`, …) is refused, naming its UTC one, and `new Date(y, m, …)` names
`new Date(Date.UTC(y, m, …))`. A Date is an object in JavaScript and a time value in a flow, so it
is read only by its methods: `new Date(t)` alone, or a Date's name, returned, compared or put in a
record, is refused, naming `getTime()` and `toISOString()`. `new Date()` and `Date.now()` are
refused, naming the clock. `Object`'s functions that describe properties or prototypes
(`defineProperty`, `getPrototypeOf`, `isFrozen`, …) are refused, as is `instanceof`, `new Map()`
and `new Set()` outside a spread.

`Math` and `Number` are namespaces the reader knows, as it knows `JSON`, `Object` and `Date`; a
flow's own function named `parseInt` is the one `parseInt(s)` calls, as in JavaScript, and
`Math.max` is the leaf whatever the flow defines. `Math.random()` is refused, with `import { random
} from "holon:random"`, a run's one recorded number. `.length` is a text's `text_length` or a
list's `length`, by the type of the node that made the value. A method no leaf does is
refused, with "do it in a `js()` function". `fmt` writes `entries` as `Object.entries(r)` where
`r` is known to be a record (a node whose output is a `Record`), else as `[...xs.entries()]`.

A text's positions and lengths count UTF-16 code units, as JavaScript's do; a result that would
begin or end inside a surrogate pair (half an emoji) gives no value, as a flow's text can't hold
half of one. `s.localeCompare(t)` is refused, as a flow has no locale (`a < b` compares texts in
code units), and so are the regular expressions (`s.match(re)`, `s.search(re)`, a pattern in
`replace` or `split`): they are done in a `js()` function. `escape` and `unescape` are refused,
naming the URI functions.

### 5.4 The outside world, as imported names

Each form is exactly one leaf. This table is exhaustive; nothing else is sugared.

| Written | Lowers to | Reads as |
|---|---|---|
| `req` (a route's parameter) | the route's `request` template node, its `request` output | the request |
| `req.params`, `req.query`, `req.headers`, `req.body`, `req.json` | that output of the request node | its parts |
| `key` (`import key from "secret:api_token"`) | `secret{name: api_token}` | a secret |
| `state.beats` (in an expression) | `store_read{key: beats}` | the state, as the run began |
| `state.beats = e` | `store_write{key: beats}(e)` | set the state when the run finishes |
| `now` (`import { now } from "holon:clock"`) | `clock()`; in a clock, its tick's `now` output | the time the run started; in a clock, the tick's |
| `random` (`import { random } from "holon:random"`) | `random()` | one random number for the run |
| `f` (`on.file.created(inbox, (f) => …)`) | the handler's `on_file{folder: inbox}` node, its `file` output | the handle of the file that started the run, with its `event` |
| `input` (`action((input) => …)`) | the handler's `action{name}` node, its `input` output | what whoever ran it sent |
| `notes[k]` (`import notes from "collection:notes"`) | `collection_get{collection: notes}(k)` | the item under `k` |
| `notes[k] = e` | `collection_put{collection: notes}(k, e)` | put it when the run finishes |
| `delete notes[k]` | `collection_delete{collection: notes}(k)` | delete it when the run finishes |
| `notes.get(k)`, `notes.put(k, e)`, `notes.delete(k)` | the same three leaves, as expressions | the operation, as a value to name |
| `notes.list({ prefix: p, after: a, limit: n })` | `collection_list{collection: notes, limit: n}(p, a)` | a page of items, in key order |
| `inbox[n]` (`import inbox from "folder:inbox"`) | `file_read{folder: inbox}(n)` | the file's text, as the run began |
| `reports[n] = e` | `file_write{folder: reports}(n, e)` | write the whole file when the run finishes |
| `delete reports[n]` | `file_delete{folder: reports}(n)` | delete it when the run finishes |
| `inbox.get(n)`, `reports.put(n, e)`, `reports.delete(n)` | the same three leaves, as expressions | the operation, as a value to name |
| `inbox.list({ prefix: p, after: a, limit: n })` | `file_list{folder: inbox, limit: n}(p, a)` | a page of files, `{size, hash, modified}` by name, in name order |
| `inbox.file(n)` | `file_handle{folder: inbox}(n)` | a handle: `{folder, name, size, hash, modified}`, no bytes |
| `inbox.lines(n, { from: i, limit: k })` | `file_lines{folder: inbox, limit: k}(n, i)` | `k` lines from line `i` (from the end when negative), without line endings |
| `inbox.slice(n, { offset: o, length: k })` | `file_slice{folder: inbox}(n, o, k)` | `k` bytes from `o` (from the end when negative), as text |
| `reports[n] = h` (`h` a handle read here, or a file handler's `f`), `reports.copy(n, h)` | `file_copy{folder: reports}(n, h)` | copy the file when the run finishes, host to host |
| `reports.create(n, e)`, `reports.create_copy(n, h)` | `file_create{folder: reports}(n, e)`, `file_create_copy{folder: reports}(n, h)` | the same write or copy, only if there is no such file yet: else the run fails |
| `shop.query(sql, v…)` (`import shop from "database:shop"`, `sql` a text literal) | `db_query{database: shop, sql}(list(v…))` | the rows, as the run began |
| `shop.one(sql, v…)` | `db_one{database: shop, sql}(list(v…))` | the first row, or no value |
| `shop.exec(sql, v…, { expect: n })` | `db_exec{database: shop, sql, expect: n}(list(v…))` | run the statement when the run finishes (only if it changes `n` rows) |
| `notes.find(c, { … }, { sort: { … }, limit: n })` (`c` a text literal, each record written in the call) | `db_find{database: notes, collection: c, filter, sort, limit}(list(v…))`, the records frozen as templates, `v…` their computed values | the documents, as the run began (no filter: all) |
| `notes.find_one(c, { … }, { sort: { … } })` | `db_find_one{database: notes, collection: c, filter, sort}(list(v…))` | the first document, or no value |
| `notes.insert(c, { … })` | `db_insert{database: notes, collection: c, document}(list(v…))` | add the document when the run finishes |
| `notes.update(c, { … }, { "$set": … }, { expect: n })` | `db_update{database: notes, collection: c, filter, update, expect: n}(list(v…))` | update what the filter selects when the run finishes (only if it matches `n`) |
| `notes.delete(c, { … }, { expect: n })` | `db_delete{database: notes, collection: c, filter, expect: n}(list(v…))` | delete what the filter selects when the run finishes (only if it deletes `n`) |
| `updates.send(e)` (`import updates from "channel:updates"`) | `channel_send{channel: updates}(e)` | send `e` to the channel's subscribers once the run is over |
| `return updates.subscribe({ from: "kept" })` | `channel_subscribe{channel: updates, from: kept}` as `respond`'s body | answer with the channel's packs, as server-sent events (`from: "new"`, the default: only those sent from now on) |
| `await api.get(path, { headers: h, body: b })` (`import api from "host:api.example.com"`), and `post`, `put`, `patch`, `delete`, `head` | `http_request{method, host: api.example.com}(path, h, b)` | a call to that host: `{status, headers, body}` |
| `await api.put(path, { headers: h, file: f })` (`f` a handle), any method | `http_upload{method, host}(path, h, f)` | a call sending the file's bytes, streamed by the server |
| `await api.post(path, { headers: h, body: b, stream: true })`, any method | `http_stream{method, host}(path, h, b)` | a call whose answer is forwarded to the route's caller as it arrives (`respond(sent.body, { stream: true })`) |
| `updates.pipe(api.post(path, { headers: h, body: b }))`, any method | `http_stream{method, host, pipe: updates}(path, h, b)` | a call whose answer goes to the channel's subscribers as it arrives, piece by piece |
| `stripe.verify(req, secret, now, { tolerance: 300 })` (`stripe` imported from `"holon"`) | the `stripe_verify` template | a verified Stripe event |
| `await event("paid", { expires: "1d" })` (`event` imported from `"holon"`) | `await{name: paid, expires: 86400}` | the event, when it arrives (in an action, never a route) |
| `resume_token({ await: "paid" })` | `resume_token{await: paid}` | the token that delivers it (in an action) |
| `await charge(x)` (`import { charge } from "flow:billing"`) | `flow_call{flow: billing, action: charge}(x, trigger)` | the other flow's action, run: `{answer, run}`, or `{error, message}` |
| `payment(x)` (the flow's own action), `charge(x)`, not awaited | `flow_start{flow, action}(x, trigger)` | the run of that action, started: `{run, waiting}` once it waits, `{answer, run}` if it finishes first |
| `JSON.stringify(x)`, `JSON.parse(x)` | `json_stringify(x)`, `json_parse(x)` | JSON out, JSON in |

Rules the table implies:

- A collection must be imported before `notes[k]` means a collection; `x[k]` on anything else is a
  `lookup` on a record (§5.6). The import is a *name*, not a permission: `check` still derives
  `CollectionRead("notes")` from the `collection_get` leaves. A folder the same: `import inbox
  from "folder:inbox"`, and `FileRead("inbox")` from `file_read`. A file read whole is text (UTF-8,
  up to the deployment's `inline_file_bytes`, 1 MiB by default; a run reads 64 MiB in all,
  `max_file_read_bytes`, writes 1 GiB, `max_file_write_bytes`, and touches 1,000 files,
  `max_files_per_run`); a read of anything else fails the run, and it writes nothing. A bigger one
  is read by handle, lines or slice; a copy also asks `FileRead` of the folder its handle names.
- A host is its import, because the host is the capability the deployment asks for: a request is
  a method of an imported host, never a host from a wire. The path, headers and body can come from
  anywhere. A database's SQL is a text literal for the same reason: its tables are
  `DbRead("shop/orders")` and `DbWrite("shop/orders")`, a statement that names no table is an
  error, and so is a count of values that isn't the SQL's `$n`s.
- A request is written `await api.get(…)`, as TypeScript reads one, in an `async` handler or
  function (§5.8). Today `await` there changes nothing: a request is answered between rounds
  whether it is awaited or not.
  The L4's slice 7 gives it JavaScript's order (two `await`s in a row run in order, `Promise.all`
  sends several together) and makes it required.
- A call, a request or another flow's action (`CALLS`, `catalog.rs`), is an effect: the host makes
  it, and once made it can't be taken back. So it is made for what it does, whether or not its
  answer is read. One nobody uses stands as a statement, `await api.post("/a", { body: b })`, as
  `await refund(…)` does in shop; bound and never read, it is made just the same. Its handler keeps
  it as it keeps a write, so the same call written twice is two calls. Its answer is the far
  side's, never its handler's: a route or an action whose only effects are calls answers 204 (with
  no `return`, the lowering writes the answer). A clock or file handler of calls alone is a
  handler. The `fan-out` sample shows it. A call bound on a statement is made when the run reaches
  it, though only a branch reads its answer (§6.4); to make it only sometimes, tie it: the
  `idempotent` sample's charge, `if (first_charge !== undefined) processed = await
  httpbin.post(…)`, says when it goes out.
- A streamed call is a route's, and the route answers with it: `stream: true` comes in the call's
  options, takes `true` or `false`, sends a body (not a `file`), and a route makes one; its answer
  is `respond(sent.body, { stream: true })`, without `status` or `headers` (its status is the
  call's, `sent.status`, wired to the respond's status input). A streamed call the route doesn't
  answer with, or `stream: true` on anything but a streamed call's body, is an error that says so.
  The call asks `HttpRequest(host)`, as any other.
- A piped call, `updates.pipe(api.post(…))`, takes one call, sending a body (no `stream` or `file`),
  and asks `ChannelSend("updates")` beside its `HttpRequest`. Its pieces aren't packs a run sends:
  they don't count against `max_packs_per_run`, are cut where a character ends, and reach only the
  subscribers there as they arrive.
- A subscription is only a route's answer: `return updates.subscribe()`, or `respond(…, {
  headers })` around it if it wants, never a status or `?? status(…)` (it answers 200). Elsewhere
  `updates.subscribe()` is an error that says so. A pack is any value with a JSON form, up to the
  deployment's `max_pack_bytes` (64 KiB by default), and a run sends up to `max_packs_per_run`
  (1,000); a run past either does nothing, and answers 500. Each channel keeps its newest
  `channel_keep` packs (100) for subscribers that ask; a subscriber that falls `channel_buffer`
  packs behind (256) is ended with `event: missed`, and catches up when it reconnects; one channel
  has up to `max_subscribers` (100). Pausing or stopping the deployment ends its subscriptions with
  `event: closed`.
- **Another flow's action** is run with `await charge(x)`, `charge` imported from `"flow:billing"`
  (a slug's `-` is kept in the source: `"flow:billing-v2"`). It lowers to the sink
  `flow_call{flow: billing, action: charge}(input, trigger)`: the host performs it between rounds,
  in-process, as a request is, and the answer goes into the run's record, so a retry, a replay and
  `holon verify` answer it from the log and never call again. `x` is one value (`{}` when none is
  written; a record to send more); the answer is `{answer, run}` (`answer` absent for an action
  that only writes, `run` the number of the run in the other flow's log) or `{error, message}` when
  nothing ran: `refused`, `not_found`, `not_running`, `no_action`, `waiting` (the action parked at
  an `await`; `run` says which), `failed` and `too_deep`. A value that isn't there is absent as
  anywhere (§6.3), so `return status(201, paid.answer) ?? status(502)` answers 502 whatever went
  wrong. The call is tied to the handler's trigger as a write is, `if (found !== undefined) …` ties
  it to a value (§5.11), and one nobody uses stands as a statement, made for what it does. It asks
  `RunsAction("billing/charge")`, which an approval says as "run the action charge of the flow
  billing", and the call is in the flow's hash: pointing it at another flow or action is a new
  version. The other flow runs an ordinary action run, under its own grant and limits, logged there
  `by` "flow shop, run <id>". A call refuses what it shouldn't carry: a secret, or a value computed
  from one, which the other flow's log would keep; and a flow already in its chain of calls (itself
  included), which would wait for the run that made the call. A chain is at most `max_call_depth`
  calls long (3 by default, 8 at most). Calls to one flow in a round go one at a time, in the order
  of their nodes. Revoking the permission pauses the caller, as any required one does, or with
  **keep running** ticked refuses the next call. Awaiting the flow's own action is refused: it
  would wait for its own run's end, which a flow can't yet (the L4's slice 7); it is started
  instead (below).
- **A route answers and is done.** A route that waits (`await event(…)`), or hands out a token of
  its own (`resume_token`), is refused, saying what to write instead, the canvas's route as the
  text's: what waits is an action, and the route starts it by calling it without `await`.
  `payment(x)` starts the flow's own action `payment`, and `charge(x)` another flow's, imported from
  `"flow:billing"`. It lowers to the sink `flow_start{flow, action}(input, trigger)`, performed and
  recorded as a call is (above), so a retry, a replay and `holon verify` answer it from the log and
  never start it again. It is answered once the started run first waits, `{run, waiting}`, `waiting`
  the token for each point it waits at (`started.waiting.paid` is the pay link's), or once it
  finishes, `{answer, run}`, or `{error, message}` when nothing ran, as JavaScript's async function
  runs to its first `await` before it hands back its promise. A start of the flow's own action is
  let through the rule against calling back, as the route doesn't wait for it; the started run is
  in the chain, so it can't start its own flow's action again. It asks
  `RunsAction("approval/payment")`, said "run the action payment of the flow approval". The token is
  the started run's: the starter's log keeps it as `‹token:paid›`, and the started run's own entries
  the same, never its value.
- `state.x` on the right of `=` and on the left are different leaves, read and write, and that is
  the whole semantics of state in Holon: read at the start, write at the end. A handler writes a
  key once (one `store_write` per key): assigned again, the key is written once with the value it
  was given last, as a `let` holds its last; assigned in both branches of an `if`, once with the
  branch taken's. Read anywhere in the run, `state.x` is the value the run began with. A key written
  in an `if (x !== undefined)` and elsewhere is an error that says so.
- The assignment and `delete` forms are statements; their Effect is exposed and performed
  (§5.10). The method forms are the same leaves as expressions, for a function that must name its
  writes as outputs, or for a write whose value is passed on (`const saved = visits.put(id, now)`,
  then `if (saved !== undefined) …`, in the `counter` sample).

### 5.5 Literals and text

| Written | Lowers to |
|---|---|
| `1`, `-2.5`, `1_000` | `const{type: Number, value: "<as written, without separators>"}` |
| `"text"`, `'text'` | `const{type: Text, value: …}` |
| `true`, `false` | `const{type: Boolean, value: …}` |
| `` `Hello, ${name}!` `` | one `concat` over the pieces: `const` for each literal piece, `as_text(expr)` for each `${…}`; a template that is one `${expr}` with no literal text is `as_text(expr)` alone, and one with no `${…}` is a text |
| `{ k: e, "k 2": e2 }`, `{ k }` | `record()` then one `with_field{name: k}(record, e)` per field, in written order; `{ k }` is `{ k: k }` |
| `{ ...base, k: e }` | `with_field{name: k}` chained onto `base`'s value, fields in written order |
| `{ a: 1, ...r, ...s, [k]: v, b: 2 }` | read left to right, as JavaScript builds it: a spread after anything `merge` (one for several in a row, `merge(with_field{name: a}(record(), 1), r, s)`), a computed key `insert(…, k, v)`, and the fields after either `with_field` on what is before them |
| `[e1, e2, e3]` | `list(e1, e2, e3)`, variadic |
| `[]` | `const{type: Json, value: "[]"}`: a placed `list` has at least its two declared inputs, so the empty list is the one literal that is a `const` |
| `[e]` | `append(list: const "[]", item: e)`, for the same reason |
| `[...xs, x]`, `[...xs, ...ys]`, `[...xs]` | `append(xs, x)`, `extend(xs, ys)`, `xs` |
| `"10s"`, where a duration is asked | its number of seconds, frozen in the param that asks (`every`'s `seconds`, `await`'s `expires`), or milliseconds for a `_ms` limit |

A part is any expression, a template of its own included: `` `${slice(sha256(`${now}`), 0, 6)}` ``
reads as written.

Interpolation wraps every `${…}` part in `as_text`, the leaf built for it: a text stays itself, a
number or boolean is written as it reads, a record or list as JSON. So a part may be any value, and
the lowering never depends on knowing a part's type. The one `as_text` node per part is the price
of that rule; the alternative, skipping it when the part is statically `Text`, would make the hash
depend on type annotations elsewhere in the file.

Record keys are names or texts; a key that is not a plain word must be quoted (`"content-type"`),
and a computed key, `{ [k]: v }`, is `insert`, its key a text.
Record and list literals always lower structurally, never to a `const` of type `Json`, so the
lowering is one-to-one and the canvas shows the fields. A single-node JSON constant is the right
side of `??` (§5.6).

### 5.6 Operators

Lowest to highest precedence, as TypeScript has them; binary operators are left-associative, but
`**`.

| Operator | Lowers to | Note |
|---|---|---|
| `c ? a : b` | `coalesce(guard(cond: c, value: a), guard(cond: not(default{false}(c)), value: b))` | `a` if `c` is `true`, else `b`; the side chosen is the answer, absent if it is (§6.4) |
| `c ? a : undefined` | `guard(cond: c, value: a)` | absent unless `c` is `true` |
| `a ?? b` | `coalesce(a, b)`; with a literal `b`, `default{type, value}(a)` | `a` when present, else `b`. The two leaves have one meaning; `default` is the one-node form for a literal (a number, text, boolean, or a record/list literal with only literals inside, frozen as `Json`) |
| `a \|\| b` | `or(a, b)` | variadic: `a \|\| b \|\| c` is one node; booleans only |
| `a && b` | `and(a, b)` | variadic; booleans only |
| `a \| b`, `a ^ b`, `a & b` | `bitwise{op: or, xor, and}(a, b)` | on int32s; each its own level, in that order, below `===` |
| `a === b`, `a !== b` | `equals(a, b)`, `not(equals(a, b))` | structural, any two values, never coerced; `==` and `!=` mean the same, and `fmt` writes `===` |
| `a < b`, `<=`, `>`, `>=` | `compare{op}(a, b)` | two numbers, or two texts in UTF-16 code-unit order; a number and a text give nothing |
| `a << b`, `a >> b`, `a >>> b` | `bitwise{op: shl, shr, ushr}(a, b)` | `>>>` gives a uint32 |
| `a + b`, `a - b` | `add(a, b)`, `subtract(a, b)` | `+` adds numbers, and with a text on either side joins both as `String(x)` writes them, left to right (`1 + 2 + "a"` is `"3a"`); `-` on numbers only; `add` variadic |
| `a * b`, `a / b`, `a % b` | `multiply(a, b)`, `divide(a, b)`, `remainder(a, b)` | `multiply` variadic; `%` is fmod, with `a`'s sign |
| `a ** b` | `power(a, b)` | right-associative; its left is never a unary expression, so `(-2) ** b` |
| `!a` | `not(a)` | |
| `-a` | `multiply(a, -1)` | in parentheses, one node; exact at `-0`, where `0 - a` isn't. A minus before a number literal is the literal (`-1`) |
| `+a` | `to_number(a)` | `Number(a)` |
| `~a` | `bitwise{op: xor}(a, -1)` | |
| `typeof a` | `default{type: Text, value: undefined}(type_of(a))` | §5.3; `fmt` writes it back as `typeof a` |
| `k in r` | `has(r, k)` | on the level of `<`; `instanceof` is refused, naming `Array.isArray` and `typeof` |
| `a.name`, `a?.name` | an output, or `field{name}(a)` | §5.7; `?.` means what `.` does |
| `a[e]` | `lookup(record: a, key: e)` when `a` is not an imported collection or folder | the key arrives on a wire |

`coalesce(a, b)` is the one leaf this language needed beyond `as_text`: `a` when present, else
`b`, the first input that arrived. It cannot be a composite, because no other leaf fires with a
wired input absent (the firing rule hands a node nothing), which is exactly the gap `default`
plugged for literals. It went through the admission gate as genuinely inexpressible, pure and
total (built 2026-10-24, the 70th leaf). It needed less of the evaluator than this design first
assumed: the leaf path already gathers present inputs in port order, so "the first present" is
"the first that arrived"; `eval_at` gives it an explicit arm anyway, so the contract is stated
where absence is visible rather than inherited from an implementation detail. The heartbeat
sample uses it to keep the time of its first beat: `state.first_beat = state.first_beat ?? t`.

Variadic flattening applies only to a chain of the *same* operator: `a && b && c` is one `and`
with three inputs; `(a && b) && c`, with explicit parentheses, is two nodes, and `fmt` keeps the
parentheses. They are the author saying "two". `+` joins texts as JavaScript's does; a template
literal is the other spelling, `concat(a, b)` the explicit call. `||` or `&&` with a text,
number, record or list literal on its right (`a || "none"`) is refused with "use `??`", as that
is JavaScript's fallback and not logic.

### 5.7 Multi-output values: `.name`

A call to a function with several outputs, or to the `request` or `stripe_verify` template,
produces several values. The rules:

1. `v.name`, where `name` is one of `v`'s output names, is that output. No node is placed.
2. Otherwise `v.name` is `field{name}` applied to `v`'s value (its first output), one node.
3. A bare multi-output `v` in input position is its first output. For `request` that is the
   request record, which is what `stripe.verify`, `respond` and `field` want.

Output names win over field names. To read a field of the request record that shares a name with
an output (`body`), write `req.request.body`: output `request`, then the field. The printer uses the
same rule in reverse, so the text round-trips.

A function with several outputs may also be destructured: `const { rows, report } =
summarize(x)`. Each name is that output, no node.

### 5.8 Functions

```
function publish(slug, author) { …; return { put, receipt } }
const slugify = (text) => text.toLowerCase().split(" ").join("-")
```

A function (or an arrow bound by a top-level `const`) is a holon. It lowers through
`FlowBuilder::begin_holon` … `end_holon_exact`: its statements are the inner canvas, each parameter
is a boundary input wired to every use, and each output a boundary output wired from its
expression. The parameters are the unit's input port names, in order, so the composite freezes
exactly as the expand view would freeze it. Its outputs are named by what it returns:

- `return slug`, a binding, gives one output named after it, `slug`;
- `return { rows, report }`, an object literal, one output per key; a function whose every `return`
  is an object literal with the same keys has those outputs, each chosen among the returns as
  `?:` chooses;
- anything else, one output, `out` (the L4's fork 7), shown on the canvas as its type, as a leaf's
  single output is.

An input never used is an error (a function written in place may leave one unused, §5.9), and so
is an output that is an input passed straight through. A
parameter is a name: a pattern is written on the body's first line (`const { price, qty } =
order`), as a port is named by its parameter, and it takes no default (every input is wired).

Types are optional annotations: `function slugify(text: string): string`. When given they are the
ports' types, checked against what the wires carry (§6.8); when absent, a port takes its first
wire's type, as the canvas does. A return type names one output's type. A function takes no params
(in the catalog's sense); it is placed by calling it, `slugify(x)`, and the unit's hash is the same
wherever it is placed. A function computes. It may be `async` and await what it sends, `const sent
= await hooks.post(…)`, as a handler writes a request, and its caller may await it, `await
toSlack(event)`: the `await` adds nothing yet (§5.4), so an `async` function is the same unit as
the function without it, and a request in it is made as a leaf's is (§6.4). It never waits: `await
event(…)` in a function is refused, as a wait is an action's, and so is another flow's action
awaited, as a call is a handler's. A method given a function (`map`, `filter`, `sort`, a
reviver…) takes no `async` one, by name or written in place, as it would be handed a promise for
each value, which `Promise.all` gathers (the L4's slice 7). The `stripe-to-slack` sample shows it:
`fromStripe`, and `async toSlack`, which posts to Slack.

A function that writes names its writes as outputs, because a holon's effects leave it through its
boundary and a boundary port has a name: a collection's write in the method form, bound and
returned (`const put = posts.put(k, v)`, `return { put, … }`), and a state write as the output
`write` (slugs' `publish`). Writes the caller doesn't read are performed (§5.10).

**A function with a sink has a `trigger` port it doesn't declare,** its last input, whether the sink
is in its body or in a function it places; it is the one port a function has without declaring
it, as `respond` has its request. Its sinks are tied to it, and a placement is tied as a sink is
(§5.11): to the handler's trigger, so `bump()` in a route writes with the route's request, or to
what an `if (x !== undefined)` around it names. A function with no sink, a calculation, has no
trigger.

Functions are defined at the top level and are visible to the whole file, in any order, so a
function may use one defined below it. They may not be nested in text (a function declared inside
a handler is refused); a function placed inside another is the nesting. A function can't place
itself, as its hash doesn't exist until it is frozen: there is no recursion.

### 5.8a js functions

```ts
import { flow, js, route, status } from "holon"
import words from "collection:words"
import { now } from "holon:clock"

flow("nearest", { port: 8117 })

const closest = js(function closest(word, candidates) {
  const dist = (a, b) => {
    const d = Array.from({ length: a.length + 1 }, (_, i) => [i]);
    for (let j = 1; j <= b.length; j++) d[0][j] = j;
    for (let i = 1; i <= a.length; i++)
      for (let j = 1; j <= b.length; j++)
        d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    return d[a.length][b.length];
  };
  let found = { best: undefined, distance: Infinity };
  for (const c of candidates) { const k = dist(word, c); if (k < found.distance) found = { best: c, distance: k }; }
  return found;
}, { outputs: ["best", "distance"] })

route.post("/words", (req) => {
  words[req.body] = { added_at: now }
  return status(201, { added: req.body })
})

route.get("/nearest/:word", (req) => {
  const found = closest(req.params.word, Object.keys(words.list()))
  return { query: req.params.word, best: found.best, distance: found.distance }
})
```

A function passed to `js(…)` has a body in JavaScript, run as written (`docs/L4s/js-node-L4.md`).
Its parameters are its input ports. Its output is named after the function itself (`js(function
html(board) { … })` gives the output `html`), or `outputs` names several, two or more, each a
field of the object it returns. `fuel` and `memory` are its other options. It is bound by a
top-level `const`, visible to the whole file, and placed by name like any function
(`closest(a, b)`, `closest(a, b).best`, `xs.map(score)`). It lowers to a composite the printer reads
back: a `const` holding `(word, candidates) => { body }` (assembled from the parameters, so ports
and source can't disagree), the arguments as one list, the engine module placed by hash under its
budgets, and a `field` per output. The engine is Boa compiled to a WebAssembly module with **no
imports** (`fixtures/modules/js-engine/`): the body can reach no clock, randomness, network or
storage, which is why a `js()` function adds nothing to what a flow may do; what it adds is two
limits, its `fuel` (instructions per call, 200M when not written) and `memory` (16 MiB), which the
deploy asks for and the host's policy caps. `"50M"`, `"2K"`, `"16MB"` are numbers with a scale.

The body is read by the same TypeScript parser, as a function, and is never Holon: nothing in it is
interpolated or renamed, so the source is part of the unit's identity, verbatim (P3 stops at the
brace). `fmt` lays a body out as it does the rest, so a body laid out anew is a new unit that does
the same. Types in it are blanked to spaces before Boa sees it (Boa runs
JavaScript), so its lines and columns stay where they were and a change to a type alone is not a
new version. It is never `async` or a generator. A body sees only its arguments: `undefined` for an
absent value inside a list, every core value as its JavaScript counterpart
(`docs/L4s/js-node-L4.md` §5). It returns one value (one output) or an object keyed by the output names (several); `undefined` or
`null` is an absent output, an exception is an absent output whose reason the run records and
`explain` says under the statement ("closest gave nothing: TypeError: …"), and a `Map`, a `Date`,
a function or a Promise can't cross and is an error naming its kind. A syntax error is caught
earlier, at its line in the file. Each call is a fresh realm: nothing survives between calls, so
two equal calls are equal and a run replays. A body that never ends runs out of fuel and is
absent, with that reason.

The sample `nearest` shows a `js()` function doing the one computation the vocabulary can't spell,
an edit-distance search, with the flow around it storing, routing and answering.

### 5.9 Functions for a list's methods

`xs.map(f)`, `xs.reduce(f, start)` and a list's other methods that take a function (`filter`,
`some`, `every`, `find`, `findIndex`, `findLast`, `findLastIndex`, `flatMap`, `toSorted`,
`reduceRight`) take, as `f`:

- a function's name: `titles.map(slugify)`, lowered to that holon's hash;
- an arrow written in place, its body one expression (or `{ return … }` alone): `(acc, p) => acc +
  p`, lowered to an anonymous composite with the parameters as inputs in order and the body's
  value as its one output, `out`, then to its hash.

Each function takes what its leaf hands it: the item; `reduce`'s the result so far and the item;
`toSorted`'s the two items it compares. `map`'s may take the index after the item: the reader
writes it `indices(xs).map((i) => …)`, the item read as `get(xs, i)`. A function that takes more is
an error at the function: "filter's function takes the item: (x) => …; an index isn't passed".
One that takes fewer, or doesn't use a parameter (`() => 0`, `(_, i) => i`), leaves it unused: as
a composite's input feeds one inner port, the lowering wires it into a node nothing reads
(`is_list`, or one `list` for two), which never runs, and the printer writes it back as a
parameter the body doesn't use. `_` is a name like any other. The leaves pass a function the values given after the list,
after the item: `map{fn: f}(xs, a, b)` calls `f(item, a, b)`. They are ports of their own (`in2`,
…, typed `Any`), and the call fires only when every one is present, as any node does. The text
reaches them through captures; an explicit form for them comes in a later slice.

An arrow may use a name from around it (a binding of the handler or function it is written in, a
parameter of that function, or a parameter of an arrow it is written in): that is a capture. Each
captured name becomes an input of the function after its parameters, named after the name and
sorted, and the call passes the bound value there. So `Object.keys(r).map((k) => r[k])` passes `r`
from the route. Constants are lowered inside, not captured. A name bound nowhere around the arrow
is "unknown", as anywhere else; a function's body sees its own names only, so an arrow inside a
function cannot reach a route's.

The arrow's hash is its content's, so two identical arrows are one unit, and the printer names
anonymous composites `fn_1`, `fn_2`… only when it has to print them as functions. Parameter names
are part of the unit (ports are named), so `(k) => …` and `(key) => …` are two units that compute
the same.

A function reads from outside as anything else does: a collection read, an outbound request or a
waiting point inside it is answered by the host, once per item, recorded at the item's own path,
and replayed from there (the `basket` sample prices each line from a collection). View mode shows a
`map` or `fold` whose function reads from outside with what the run recorded it produced, as it
does a function that reads.

### 5.10 Effects and statements

The IR has one rule for effects: an Effect-typed output that nothing consumes is exposed by the
group, and the host performs it when the run finishes (writes, puts, deletes, egress). A call's
answer that nothing consumes is exposed the same way, after them: the call was made, and the
handler keeps it (§5.4). The language keeps that rule and adds no `perform` keyword:

- The write statements, `state.k = e`, `notes[k] = e` and `delete notes[k]`, are effects.
- An expression statement whose type is `Effect` is an effect: `notes.put(k, v)`,
  `updates.send(v)`. So is a call: `await api.post(path, { body: b })`, `await refund(x)`.
- An expression statement of any other type is an error: "a value nobody uses: bind it, or remove
  it".
- A binding to an Effect and never used is still performed; naming it changes nothing.
- A multi-output function's Effect outputs that the caller does not read are performed. The `slugs`
  route performs `publish`'s `put` and `write` without mentioning them.

Effects are values, so a write's value may be guarded or chosen, and only a present value reaches
the boundary: `if (c) state.k = a; else state.k = b` is one write, of `c ? a : b`, and witness's
`batches[batch] = ok ? { … } : undefined` writes nothing without the key. That guarded value is the
idiom the samples use for a write that may not happen.

### 5.11 Routes, and what a handler holds

```
route.METHOD("/path", (req) => answer, { resumes: "NAME", log: "lazy" })
route.METHOD("/path", async (req) => { statements; return answer })
```

Lowers to a `request{method, path, resume, log}` template node, the route's trigger, and the
handler's nodes; `resume` is the option `resumes`, and `log` the option `log` (§5.1), empty for
strict, the same param a clock's `every`, a file handler's `on_file` and an action's `action`
carry. Every `return` and every use of `req` wires into the same connected group, which is what
makes the handler one route (`find_routes` works by connectivity, unchanged). `async` goes on a
handler that `await`s, as TypeScript requires.

**The answer.** A route answers with what it returns, through the `respond` template, whose hidden
`request` input the lowering wires to the route's request node:

| Written | Lowers to |
|---|---|
| `return body` | `respond{status: 200, missing: 404}(body)`: text as plain text, anything else as JSON |
| `return status(201, body)` | `respond{status: 201}` |
| `return status(code, body)`, `code` computed | `respond{status_input: true}`, its status input wired from `code` (a number out of 100–599 there answers 500) |
| `return body ?? status(400)` | `respond{missing: 400}`: the status of an absent body (404 when unsaid) |
| `return status(201, body) ?? status(502)` | `respond{status: 201, missing: 502}` |
| `return status(204)` | `respond{status: 204}` with no body input |
| `return respond(body, { headers, status })` | `respond` with its `headers` input: a record of texts |
| `return respond(sent.body, { stream: true })` | a streamed call's answer, forwarded as it arrives (§5.4) |
| `return updates.subscribe()` | a channel's packs as the body, streamed by the server (§5.4) |

- **Several `return`s are one answer,** the route's one `respond`: each body and status is chosen
  where its branch is taken, as `?:` and `switch` choose, with one status for an absent answer:
  two `return`s that say it differently are refused ("say it the same for both"). `status(…)` and
  `respond(…)` are a route's answer and nothing else: one bound to a name or passed to a function
  is refused, saying to return it, and an action answers with a value alone.
- A route needs an answer, or at least one effect (answered 204). A call's answer is never the
  route's: without a `return`, a write answers 204, and a route of calls alone answers 204 too.
- **Handlers are separate.** Two handlers may not share a node. A top-level binding would wire two
  of them into one group, so at the top level only `const`s of literals are allowed, lowered by
  placing a fresh `const` in each handler that uses them. A top-level binding of anything else is
  an error that says why: "the top level binds only literals (const limit = 50): put the rest in a
  handler, or in a function".
- Clashing routes and bad paths are the existing errors, said of the flow by its routes (`GET /a
  clashes with GET /a`), not yet at a column.

**A handler is everything tied to its trigger.** Each handler (§5.1) has a trigger node: a route's
`request`, a clock's tick (its value the time of the tick), a file handler's `on_file` (the file's
handle) and an action's `action` (what was sent). What reads from it is the handler's, and so is
every sink placed in it:

- **Every sink takes its trigger.** The writes (state, a collection, a folder, a database), the
  calls out (requests, another flow's actions), a send on a channel and `egress` take a `trigger`
  input, last, and the lowering wires it to the handler's trigger node. The text doesn't write it,
  as a `return` doesn't write its request. So a write that reads nothing of the request is still
  the route's:

  ```ts
  import { flow, route } from "holon"
  import state from "state"
  import visits from "collection:visits"
  import { now } from "holon:clock"

  flow("counter", { port: 8155 })

  route.get("/hit", () => {
    const hits = (state.hits ?? 0) + 1
    state.hits = hits
    return `hit ${hits}`
  })

  route.post("/visits/:id", (req) => {
    const id = req.params.id
    const saved = visits.put(id, now)
    if (saved !== undefined) state.last = id
    return `visited ${id}`
  })

  route.get("/last", () => state.last ?? "nobody yet")
  ```

  `state.hits = hits` never looks at the request, and is the route's. A function with a sink in it
  takes a trigger too, which its placement wires the same way (§5.8).
- **The trigger is never read;** it only has to be there. A sink whose trigger has no value does
  nothing.
- **`if (x !== undefined) …` ties a statement's sinks to `x` instead:** `if (saved !== undefined)
  state.last = id` writes once `saved`, the put before it, has its result in the run, so
  `state.last` never names a visit that isn't kept; `if (found !== undefined) state.seen = id`
  writes only when the lookup `found` found something. `x` is any value in scope in the handler. A
  `let` with no value before it, given one inside, is tied to `x` too: `let sent; if (x !==
  undefined) sent = await api.post(…)` is absent when `x` is. The tie is within the run: the run
  still commits as one, its writes at its end (§6.6). In the graph it is the trigger input wired
  from `x` in place of the handler's trigger.
- **A statement tied to nothing** (a binding nothing uses, `const t = now`) would not be part of the
  handler, and would be left out of the server. The lowering refuses it, pointing at the statement:
  in a route, "this isn't wired to the route's request, so it isn't part of the route … use the
  request in it". A handler with nothing tied to it is refused the same way ("the block does
  nothing: write something in it").

## 6. Semantics

The language has no semantics of its own: a program means what its DAG means under the one
evaluator. This section says what that is, in the language's terms, because it differs from a
conventional language, and from JavaScript, in a few places. The manual lists them for a reader
who writes flows ([How a flow runs](manual/language/reference.md#how-a-flow-runs)); where a
JavaScript reader would be misled, the compiler refuses instead.

### 6.1 Dataflow, not control flow

A statement does not "run"; a node computes when its inputs are present. Statement order only
constrains names (defined before use) and the printer's layout. A state write before the binding
the answer is built from, or after it, is the same unit.

### 6.2 Sharing is explicit

A binding is one node and as many wires as uses. The same call written twice is two nodes, each
computed (and, for `now` and a secret, each sourced, with the same value within a run). There is
no common-subexpression elimination, so the canvas shows exactly what was written, and a replay's
node-by-node values match the text's expressions one to one.

### 6.3 Absence

A node whose wired input did not arrive produces nothing, and nothing flows on from it. This is the
evaluator's firing rule and it is how the language expresses "not found", "unauthorized" and "not
verified" without exceptions:

- `notes[k]` of a missing key is absent; `?? 0` fills it; a route whose answer is absent answers
  404, or the status its `?? status(…)` says.
- `ok ? x : undefined` is absent unless `ok` is `true`.
- `stripe.verify(…).event` is absent when the signature fails, so nothing after it fires.

Where JavaScript differs, Holon's meaning holds: `a.b` on no value is no value, never a thrown
error (`?.` is accepted and means the same); `{ ...x }` with `x` absent is absent, not `{}`.
Absence is not a value: it cannot be stored, compared or passed to a function. `undefined` is the
text's word for it, written only as a branch and in `x !== undefined`; `null` is refused. `none()`
is a value (the optional's empty arm) and is unrelated.

### 6.4 `?:` is a choice

`c ? a : b` is `guard(c, a) ?? guard(!(c ?? false), b)`: `a` when `c` is `true`, otherwise `b`. So
`c` absent gives `b`, as JavaScript's `if` takes `undefined` as false, and the side chosen is the
answer: `c` true with `a` absent is absent, as JavaScript gives `undefined`, and doesn't fall to
`b`. `c ? a : undefined` is the first `guard` alone, and `if (c) …` chooses between statements the
same way. A fallback is written as one: `(c ? a : undefined) ?? b` is `b` when the chosen `a` is
absent too.

Only what the choice needs is run (lazy choices, `docs/L4s/lazy-choices-L4.md`): `a` only when `c`
is `true`, and `b` only when it isn't. So a branch not taken asks the host
nothing: its requests aren't sent, its `await`s don't park the run, its reads aren't made, and a
function in it isn't entered. `x ?? y` is the same rule: `y` only when `x` is absent. This changes
no value, only what runs. While `c` (or `x`) waits on an answer from the host, the choice waits
too, and decides in the next round. A function's inputs are computed before it is entered, as a
call's arguments are: `h(go, await api.get(…))` sends the request even if `h` only uses it when
`go`.

A binding isn't a branch. A call written in a branch, `ids[name] ?? JSON.parse((await
httpbin.get("/uuid")).body).uuid` (the `cache-aside` sample) or `c ? await api.get(…) : undefined`,
is made only when the branch needs it; a call bound on a statement of its own, `const page = await
api.get(…)`, is made when the run reaches the statement, whatever reads `page`, even a branch not
taken (§5.4). In the graph the difference is the call's trigger: one bound on a statement is tied
to its handler's trigger, or to what an `if (x !== undefined)` around it names, and its handler
keeps it as an output; one written in a branch is left untied, and is made when its answer is
needed. So whether a call goes out is said where it is written: in the branch that needs it, or by
its own tie, `if (first !== undefined) charged = await api.post(…)`, never by a reader somewhere
else. The printer keeps each where it was written. (In a function, a call bound on a statement is
still made when its answer is needed: F13.)

The strict three-way choice, where an absent arm makes the whole result absent whatever `c` says,
is `select(c, a, b)`, written as a call. Being strict, it needs, and runs, both arms. It is rarely
what a flow wants.

### 6.5 `switch` is a choice

A choice of one value by another (an operator, an event's type, a command) is `switch`:

```ts
import { flow, route, status } from "holon"

flow("calc", { port: 8116 })

route.post("/calc", (req) => {
  const a = req.json.a
  const b = req.json.b
  switch (req.json.op) {
    case "add":
      return a + b ?? status(400)
    case "sub":
      return a - b ?? status(400)
    case "mul":
      return a * b ?? status(400)
    case "div":
      return a / b ?? status(400)
  }
  return status(400)
})
```

The subject is compared with each case by `equals` (structural, never coerced: `1` is not `"1"`),
and the first case equal to it is chosen. A case is a literal: a number, a text, `true` or `false`;
to test a condition, use `?:` or `if`. Several cases may share a body (`case "s": case "small":`),
`default`, if there is one, comes last, and each body returns, assigns, or ends with `break`: there
is no fall-through into another body. A case that breaks, like a subject no case equals, goes on
after the `switch`.

What makes it a choice: **the chosen case's absence is the switch's.** With `b = 0`, `a / b` has no
value, and the `"div"` case has none either, so the route answers 400. A chain of `?:` chooses so
too (§6.4); it differs from the switch where the subject is absent. The cases not chosen
don't matter at all, and don't run (§6.4): `"add"` answers `a + b` whatever `a / b` would be. So a
case may send a request or `await`; only the chosen case's are asked. With the subject absent, the
switch is absent, and the `default` with it, where a chain's conditions are absent and each takes
its else, down to the last. With no case equal and no `default`, it is absent too.

It is sugar: it lowers to `guard(equals(s, p), a)` for each case, chained by `??`, and the
`default` guarded by none of the cases matching, so a failed case can't fall through to it
(`docs/L4s/match-L4.md`, which calls it `match`, its name before the switch). The calc route's
returns fold into one answer: the case's value, `missing: 400`. A `switch` that assigns a `let` in
each case chooses the name's value the same way.

### 6.6 Reads at the start, writes at the end

`state.x`, `notes[k]` and `inbox[name]` are the values as the run began. The writes are Effects the
host commits when the run finishes. So a run never sees its own writes, and two runs that start
together both see the old value (the host runs one of them again when they changed the same item,
as the likes sample shows). A JavaScript reader expects a write to be seen at once, so within a
handler reading a collection or folder after writing to it is refused, and a state key, which is a
name, is written once with its last value and read, anywhere, as the run began (ticker's reset
answers the count before its write).

### 6.7 Totality and purity

Every program terminates: iteration is `map` and `reduce` over a list, bounded by the list (the one
leaf that makes a list from a number, `range(n)`, makes at most 10000 items), there is no loop
statement, and there is no recursion (a function cannot place itself, since its hash does not exist
until it is frozen). Nor does a value nest without end: at most 256 levels (`MAX_VALUE_DEPTH`; a
list's item, a record's field and an option's value are each a level down), and a leaf that would
build one deeper gives nothing, so a fold wrapping its accumulator (`[acc]`) stops at 256 steps.
What walks a value recurses on a thread of a few megabytes, and an overflowed stack would end
every run on the server, not this one. No construct performs an effect during evaluation; effects are values that the
host performs after the run settles. A request is answered between rounds (§4.1 of
`PLATFORM_LAYOUT.md`); `await event(…)` parks the run. None of that is visible in the language
beyond the nodes' names.

### 6.8 Types

The nine wire types are core's: `Number`, `Boolean`, `Text`, `Record`, `List<T>`, `Product<…>`,
`Sum<…>`, `Effect`, `Any`. A flow writes types only on a function's parameters and its return, as
TypeScript writes them, and only if it wants them checked:

| TypeScript | Holon |
|---|---|
| `number`, `boolean`, `string` | `Number`, `Boolean`, `Text` |
| `T[]`, `Array<T>` | `List<T>` |
| `[A, B]` | `Product<A, B>` |
| an object type, `{ name: string }`, `Record<string, T>`, an `interface` | `Record` |
| `unknown`, `any` | `Any` |
| `T \| undefined` | `T`: every type's value may be absent |
| a union of different shapes, a type parameter | refused, for now (F16) |

`type` and `interface` declarations are allowed at the top level; they name types and place no
node. `Effect` isn't written: a write is a statement. Checking is the existing wire-time
compatibility check: a `Text` into a `Number` port is an error, said today of the handler as a
whole (`GET /a: the derived unit failed validation`) rather than at the argument; `Any` is
compatible with everything, in both directions. The language adds no inference beyond what the
canvas does: a function's port takes its first wire's type. `tsc` isn't needed; an editor running
TypeScript's checker shows some mistakes earlier, and where the two disagree, Holon's compiler
decides.

## 7. Names, hashes and canonical form

- **The hash is the DAG's.** Binding names, function names, comments, blank lines and the order of
  independent statements are not in the preimage. They are kept in the flow document's `names` map
  (functions) and the nodes' `extra` (spans), never in a unit. A function's parameter and output
  names are in it, as they are its ports, and so is a `js()` body's text.
- **The same node many times over is fine.** Each `${…}` part of a template is an `as_text`, so a
  route with many templates holds many copies of one unit. The hash tells copies apart by where
  each one's value goes, down to the route's response and effects
  (`docs/L4s/child-order-canonical-L4.md` §4), so a route holds as many as it needs, in any order
  they were written. The one thing it refuses is the same value computed twice and never used:
  nothing tells those two apart, and `check` says so ("remove one, or use it"). The sample
  `issue-links` is a route of thirteen parts.
- **Resolution happens at check time.** A catalog name resolves to `interface_of`; a template to its
  builder; a function to the hash frozen for it in this file. Every hash the flow depends on is in
  the document's `units` closure, which is its lockfile.
- **Canonical text.** `holon fmt` is a source formatter (`lang/format.rs`): it lays the TypeScript
  out as Prettier does, through Oxfmt's formatter (`oxc_formatter`): two spaces, double quotes, no
  semicolons, 100 columns, a list that doesn't fit broken one item to a line with a trailing comma,
  an object the text broke kept broken, a function passed last hugged by its call, a chain of
  calls that doesn't fit broken one call to a line. It keeps every comment and a blank line where
  the text had one, and an object's key quoted or not as written. It rewrites `var` to `const` (or
  `let` where it is assigned again) and `==` to `===`, outside a `js()` body, which it lays out as
  JavaScript. It writes a text only once that text is the same program but for its layout and
  compiles to the same flow but for its `js()` bodies' text, its comments all there.
  `fmt(fmt(t)) = fmt(t)`. Two
  texts that lower to the same document stay two texts under `fmt`, each as its author wrote it;
  the printer (§11) writes one text for one document. The samples are kept as `fmt` writes them,
  and a test says so.

## 8. Layout

Positions are not part of a program. The text has no positions. The flow document keeps positions,
as it always has, so a flow authored on the canvas keeps its layout through save and open. When
text is compiled, nodes are laid out automatically (`lang/layout.rs`): each connected group (a
handler) is a band, stacked in the order the text writes them; inside a band a node's column is its
depth, a node nothing feeds sits just before the first node it feeds, and a column keeps statement
order. A function's own canvas is laid out when it is expanded. When a document is printed,
positions are dropped.

## 9. Diagnostics and source maps

Every syntax-tree node the reader makes carries a span, `line:col` to `line:col` (columns in
characters, from oxc's byte offsets); every lowered node keeps the span of the expression that
placed it, in the document node's `extra` as `source: {line, col, endLine, endCol}`. From that:

- `holon check` prints `file:line:col: message`, with the offending line after it and a caret under
  the span:

  ```
  $ holon check broken.holon.ts
  broken.holon.ts:5:23: unknown name "nope"
    route.get("/a", () => nope)
                          ^^^^
  ```

- The editor highlights the node a check error names, and view mode, which already walks a run by
  node path, can show the text line beside each node's value.
- The agent tools' `explain` renders a run as the program with each expression's value, which is
  the observability surface the product direction asks for.

The error catalogue the reader and the lowering produce, each with the span it points at:

| Error | Points at |
|---|---|
| a syntax error | where the parser stopped, in its words |
| unknown name `x` (not bound, not imported; a global the flow has none of) | the name |
| `"x"` isn't something a flow imports; `x` is imported twice | the import |
| `f` takes `n` inputs, `m` given | the extra or missing argument |
| `f` has no input or param `p` / `p` given twice | the name |
| input `p` can't be left out | the call |
| a value nobody uses: bind it, or remove it | the statement |
| `\|\|` takes booleans | the expression |
| `notes` is read after this run writes to it | the read |
| `state.k` written twice, in an `if (… !== undefined)` and elsewhere | the second write |
| `status(…)` and `respond(…)` are a route's answer: return them | the expression |
| an action answers with a value alone | the `status(…)` or `respond(…)` |
| a route answers and is done, so it doesn't wait | the `await` |
| a function computes and never waits; calling another flow's action is a handler's | the `await` |
| a method's function (`map`'s, `filter`'s…) isn't async | the function |
| no leaf does `.m(…)`: do it in a `js()` function | the method |
| a loop, `try` or `throw` | the statement |
| a case runs on into the next; a case is a literal | the case |
| the top level holds imports, `flow(…)`, handlers, functions and `const`s of literals | the statement |
| the top level binds only literals | the binding |
| a method's function takes what its leaf hands it: the item (and `map`'s index), `reduce`'s the result so far and the item, `toSorted`'s the two items | the function |
| a list changed in place: `push`'s answer, `pop`, `shift`, `unshift`, `splice`; `sort`, `reverse`, `fill` or `copyWithin` alone, on what another holds, or read after | the call, or the read |
| a function's input never used; an output that is an input passed straight through | the parameter or the return |
| two answers whose status for an absent body differs | the second `return` |
| a period written twice: one clock per period | the second clock |
| this isn't wired to the route's request; a handler does nothing | the statement; the handler |

Three mistakes are said of the flow, not yet at a column: two routes that clash, a path that isn't
a route path (both by the route), and a value of the wrong type for a port (`the derived unit
failed validation`, by the handler).

## 10. A canvas the text can't spell

The first syntax kept its netlist statements valid as the **wiring form** (one statement placing a
node, one per wire), so that any canvas could be printed: what the expression form could not spell
fell back to a node and its wires, in the same file. It had flows without handlers too, whose
top-level statements were the run. Both are gone with the switch to TypeScript (Holon is in
development, and nothing is kept compatible: §12).

So a canvas the TypeScript form can't spell has no text: nodes under no handler, beside the
handlers or alone, and a request node that isn't a route's (a group with two `respond`s, or none).
The canvas still holds it and shows it; the editor says what to change to make it a flow (the route
errors, at the nodes they name), and the text is written again once it can be.

## 11. Canvas to text: the printer

The printer (`lang/print.rs`) turns a flow document into TypeScript, laid out as `holon fmt` writes
it: the text a canvas exports as, a run's view is printed over, and the other direction of the
lowering. The algorithm:

1. Imports and settings: from `"holon"`, the vocabulary and the leaves the text calls by name; one
   import per secret, collection, folder, database and channel a leaf names, the state, each host a
   request calls, the clock and randomness, and each other flow whose actions it calls; then
   `flow(…)` with the settings.
2. Functions: every composite the document carries (the `units` closure), inner before outer, each
   printed once under its name from the `names` map (or a generated one), its inner canvas opened
   with `inner_canvas` exactly as the expand view opens it. Effect outputs print in the method form
   under their port names; a `js` node prints as its `js(function …)`, read back from its shape.
3. Handlers, in the order the routes, clocks, file handlers and actions derive (`find_routes` and
   its kin).
4. Within a handler, nodes in topological order. A node whose one output has exactly one consumer
   prints inline, as a nested expression, at its consumer, unless the text named it: the printer
   keeps an author's bindings, as a formatter keeps variables. A node with several consumers, or a
   multi-output composite, or an `await`, gets a `const`. The pass-through the lowering places for a
   function input used twice (§12, step 3) prints as the input itself, so `n + n` comes back as
   written. An Effect with no consumer prints as a statement, in the assignment form where there is
   one (`store_write` as `state.k = …`, `collection_put` as `notes[k] = …`, `collection_delete` as
   `delete notes[k]`), else as a call. Each leaf prints as its form in §5.4 (a secret as its
   import's name, `state.x`, `now`, `notes[k]`, `notes.list({ … })`, `api.post(…)`,
   `stripe.verify(…)`, `await event(…)`); consts print as literals, a duration-shaped period or
   `expires` as a duration text; `concat` over `as_text` parts and consts prints as a template
   literal; `record`/`with_field` chains print as object literals when the chain starts at
   `record()` or as `{ ...base, … }` when it does not, a `merge` as its spreads and an `insert` as
   `[k]: v`, a record built in place under them written in the same literal where ts.rs reads it
   back as the same nodes (`{ a: 1, ...r, ...s, [k]: v }`); each date leaf as a method of
   `new Date(t)` (`new Date(t).getTime()` for the time value), `Date.UTC(…)` and `Date.parse(t)`;
   `list` as an array literal; `type_of` under a `default` of `"undefined"` as `typeof x`;
   `default` with a literal param and `coalesce` as `??`; `guard` as `c ? a : undefined`, and
   `coalesce` over a `guard` and a `guard` on `!(c ?? false)` of the same `c` as `c ? a : b`;
   binary leaves as operators; `field` chains as `.name`; `respond` as the
   handler's `return`; MongoDB templates as the records they were frozen from.
5. Names: a binding takes the node's name from the text it came from when it has one (the
   lowering's names, handed to the printer), else one from what the node is (a `field`'s field, a
   secret's name, a state key, `t` for the clock, `item`, `page`, `response`), made unique and kept
   clear of the imports, the vocabulary and the functions.
6. What it can't spell has no text (§10).

The printed form has no destructuring, no reassignment and no early `return`: a text written with
them prints back as the same unit, one `const` per binding. A match prints as a `switch` where the
reader takes that back as the same values (a route answering one as a `switch` of `return`s, one
bound by name as `let name` and a `switch` whose cases give it its value), else as `?:` and `??`; a
write or a call that waits for a value (`after`) goes in `if (x !== undefined) { … }`. What a
canvas edit does to such a text is F15.

Properties, tested: `compile(print(doc))` carries the same unit hashes as `doc`;
`print(compile(t))` is the canonical form up to the author's bindings; `print` is idempotent
through `compile`. A `concat` of consts stays a `concat` (a template literal lowers with an
`as_text` per part, so only a `concat` with one prints as a template), and a JSON `const` prints as
a literal only when the literal writes back to the same text, with keys sorted as the lowering
writes them.

## 12. Build order

Each step is one inch: its tests, a live check, a commit. Steps 1 to 8 took about two weeks; the
one that changed core is done, and none of them changed the host, the server or the document
format. Steps 1 to 8 built the first syntax, Holon's own (`.holon` files: headers, `route GET /p
(req) { … }` blocks, `respond`, `holon`, `fn`, `match`), and are told in its words; step 9 switched
the syntax to TypeScript's, keeping the lowering.

1. **Lexer and parser with spans** ✅ (2026-10-24, `holon-author/src/lang/{lex,ast,parse}.rs`):
   keywords, nouns, durations, interpolated strings (each `{…}` part parsed with the columns of
   where it sits), the statement forms, and the wiring form's statements into the same tree.
   Tests: every construct, every error with its column, the eleven samples in the natural form,
   and every sample as the wiring form prints it, read as the old parser reads it. Two details
   the parser settled: a newline continues a statement after an operator or a comma and inside
   parentheses, brackets and record literals, and `else` may start the next line; and a keyword
   followed by a dot is a plain name, so a wiring-form node named after a leaf (`await.out`)
   still wires.
2. **`coalesce` in core** ✅ (2026-10-24): the leaf, its catalog entry (70th), an explicit
   `eval_at` arm, tests at the leaf and inside a composite, and the heartbeat sample keeping its
   first beat's time through it. No change to the composite walk was needed.
3. **Lowering onto `FlowBuilder`** ✅ (2026-10-24, `lang/lower.rs`): resolution, calls with
   named inputs, the noun and statement sugar of §5.4, literals and interpolation through
   `as_text`, operators, `.name`, holons via `end_holon_exact`, lambdas as anonymous holons,
   routes with the implicit request wires, effects, the error catalogue of §9, spans into
   `extra.source`. Golden tests, both green: every sample as the wiring form prints it, and the
   eleven samples in the natural form, lower to the unit hash the sample's builder gives, through
   `flow_unit`. The natural golden texts spell `concat` where the builder does (interpolation adds
   an `as_text`), carry a proposed holon's port names in quotes, and bind one `now` where
   the builder shares one clock node, since all three are in the hash. Three things the lowering
   settled: a holon's input feeds exactly one inner port in core, so an input used twice passes
   through one shared `guard(true, input)`, placed on the second use; a port or binding may not
   take a leaf's name (`sum`, `out`), so `holon total(prices) -> amount`; and type annotations on
   ports are parsed and not yet checked (the wire-time check at freeze still applies).
4. **Printer** ✅ (2026-10-24, `lang/print.rs`), the output of `holon fmt` and `holon new`, and
   what `holon check` and `show` read through (the wiring-form printer stays for the editor's
   export until step 5). Tests: the round-trip properties of §11 over every sample from its
   builder and from its natural text, a canvas grouped by hand (core's proposed port names, a
   holon placed twice, a variadic leaf, a multi-output composite with an effect, a lookup), the
   wiring-form fallback for a request group that isn't a route, and the canonical text of two
   samples pinned. Two things it settled: `fmt` keeps the author's bindings rather than inlining
   every single-use value, and the lowering now refuses a route statement not wired to the
   request, which the printer's output first made visible.
5. **`holon` and the editor** ✅ (2026-10-24). `check`, `show` and `fmt` print a mistake as
   `file:line:col: message`, the line, and a caret under the span. The editor's export and import
   go through the language (`compile_holon` and `print_holon` in `holon-wasm`), so a `.holon`
   downloaded from the flows page is the expression form, and an imported text keeps its names
   through an export (`print_holon` takes the lowering's bindings). The Inspector shows, for the
   selected node in either mode, its line in the flow as text: the canvas printed through the
   language (`print_lines`, each top-level node to the line it is written on; an inline node to
   its consumer's statement, a route's request to the route header). The flows page offers each
   sample as `.holon`, written by the language.
6. **Samples as text** ✅ (2026-10-24). Each sample is a `.holon` file in
   `crates/holon-author/samples/`, written in the natural form and embedded with `include_str!`;
   its entry in `samples.rs` keeps what the flows page shows (title, description, what it shows,
   how to try it, secrets), and tests hold the two together: the text's headers match the entry,
   and each file is exactly what `holon fmt` writes, so the files are canonical (defaults such as
   `limit: 100` or `encoding: hex` are not written, and statements inside a holon follow the
   unit's own child order). The Rust builders are gone, and the samples' hashes are the texts'.
   `CLAUDE.md`'s rule now says a sample is written in the language, and the sample is the
   documentation of the feature. One consequence worth knowing: a holon input used more than once
   lowers through a shared pass-through (step 3), so `publish` in slugs has two leaves more than a
   hand-wired canvas would, and `title ?? ""` moved to the call site so the holon needs no inner
   binding. Two things the step needed: the layout of §8, since the builders had placed every
   node by hand and a compiled sample otherwise opened on a plain grid; and `holon new --sample`
   and the flows page's `.holon` download write the sample's file as it is, its names and all,
   rather than printing its document again.
7. **Agent surface** ✅ (2026-10-24, designed in `docs/L4s/agent-surface-L4.md`). `holon mcp`,
   a Model Context Protocol server on stdin and stdout, with nine tools that speak this form:
   `reference` (this language on one page, every leaf, the samples), `check`, `save`, `deploy`,
   `call`, `deployments`, `flows`, `runs` and `explain`. What is already on a server reads back
   as text: a saved flow as the text it was saved with, a deployment in full (what it may do,
   the secrets it reads, what it waits on a person for) with its text as deployed. A run made
   through `call` names who sent it. `explain` is §9's promise: the deployed flow printed through the
   language, with the agent's own binding names, and under each statement of the run's route the
   value it had, or the node a missing value first went missing at (`field "author" gave
   nothing`), read from the run's recorded timeline through the document's `trace`. The tools
   never approve: a deploy that needs approval is saved as a flow for a person to deploy from the
   editor, and once they have, a version that needs nothing more deploys from the agent at once.
   The server holds it to that too: `holon mcp` asks as an `agent`, a role that proposes and
   calls and never approves, grants, starts, stops or deletes, shown as `<sign-in> (agent)`; an
   API token with that role is the guarantee for an agent with a shell. Tests: the tools against the in-process router (text to an explained failing run), the
   protocol on raw lines, a person's saved flow never replaced, and an end-to-end test where a
   person approves the agent's flow in the real editor and the agent calls and explains it.
   `holon mcp setup` (`docs/L4s/mcp-setup-L4.md`) adds it to a host's own settings file, for
   21 hosts and any other a person adds, changing only its entry; for a server elsewhere it
   keeps the agent token itself, so no host's file holds one.

8. **Editors** ✅ (2026-10-24, designed in `docs/L4s/language-server-L4.md`). `holon lsp`, a
   Language Server Protocol server on stdin and stdout, and `editors/vscode`, the VS Code
   extension that starts it. As the text is typed: §9's mistake where the compiler stopped, in
   `holon check`'s words, and a mistake about the whole flow at its `flow` header; on hover, what
   a word is (a keyword or noun in a line, a leaf by its signature from the reference, a holon of
   the text, a binding as the node it is, the flow's name as what `check` says of it); completion
   of what may be written there (the text's names, the nouns, the leaves called by name, the
   keywords, and after a `.` the members of what is before it); Format Document as `holon fmt`
   writes. What an editor asks is holon-author's (`editing.rs`), so the
   web editor can ask it through the bridge too; the protocol only frames messages and converts
   positions to UTF-16. Highlighting is a TextMate grammar. Tests: each answer on the samples,
   the protocol on framed messages, the grammar on every sample through VS Code's own TextMate
   engine, and an integration test in a real VS Code that sees a mistake, completes, fixes it,
   hovers and formats. Each release carries the extension as `holon-vscode.vsix`. The web editor
   has the same answers in a text pane beside the canvas (designed in
   `docs/L4s/text-pane-L4.md`): the last edit wins, what is typed drives the canvas once it
   compiles with the nodes it didn't touch kept in place, and a canvas change writes the text
   again. `fmt` keeps a text's comments.

9. **TypeScript's syntax** ✅ (2026-11-05 to 2026-11-06, designed in
   `docs/L4s/typescript-syntax-L4.md`, its slices 1 to 4). The text became a TypeScript module,
   `NAME.holon.ts`, and the lowering stayed: a reader (`lang/ts.rs`) parses it with oxc's
   TypeScript parser into the syntax tree a `.holon` text parsed to, so while the rest switched
   both forms lowered to the same units.
   - *Slice 1, the core* (2026-11-05): imports for the outside world, `flow(…)` for the headers,
     the four handlers and their options (`resumes`, `matching`, `log`), `const`, writes, `return`
     with `status(…)` and `respond(…)`, functions and arrows written in place, `js()` with its
     output names, the library's methods as leaves, and type annotations as port types. Nothing
     read a `.holon.ts` from the command line yet, so a parity test stood in for a live run: 16
     samples written in TypeScript deployed as the units their `.holon` forms did, by hash. The
     L4's fork 7 was decided the same day: a function returning an expression names its output
     `out`, shown on the canvas as its type.
   - *Slice 2, statements* (2026-11-05): `let` and `var` as a new name per assignment,
     destructuring, `if` and `switch` choosing names and writes as `if` and `match` did, `if (x !==
     undefined)` as `after`, early returns folded into one answer, changes in place, `await
     event(…)`, and the L4's refusals, each saying what to write.
   - *Slice 3, routes answer and actions wait* (2026-11-05, F14): the leaf `flow_start`, the start
     in both forms (`start(payment, x)` in `.holon`, the action called without `await` in
     TypeScript), a start of the flow's own action let through the rule against calling back, the
     start kept in the run's record and its token kept out of the starter's log, `await` refused in
     a route; approval and approved-payout split into a route and the action it starts, run live.
   - *Slice 4, the switch* (2026-11-05 to 2026-11-06): `holon fmt` as a source formatter
     (`lang/format.rs`, §7); a converter (`lang/to_ts.rs`) that wrote 54 of the 55 samples in
     TypeScript, 50 to the same unit and the four without handlers (drop-box, file-audit,
     heartbeat, rolling-log) each to one handler, and slugs by hand; all 55 samples `.holon.ts`,
     read so by `samples.rs`, the bridge, `editor/src/samples.test.ts`, the e2e tests and `holon
     new`, whose starter is TypeScript too. Heartbeat and rolling-log tick by an `every` handler,
     so `holon tick` runs heartbeat by hand where `holon trigger` did. The manual's flows are `ts`
     blocks checked as `.holon.ts`, its reference and tour rewritten; the test flows and the agent's guide are TypeScript. The slice's last part
     removes the `.holon` parser and grammar, and with them the wiring form and flows without
     handlers (§10); the printer writes TypeScript (§11). Then the server stopped running a unit
     without handlers: no interval setting, no `ingress` leaf, no `holon trigger`.
   - *Async functions, early from slice 7* (2026-11-07): a function or a top-level arrow may be
     `async` and await a request, as a handler does, and is the same unit as without (§5.8); a
     wait in one stays refused (`await event(…)`, another flow's action awaited), and a method
     given a function (`map`, `filter`…) takes no `async` one until `Promise.all`. stripe-to-slack
     groups its two halves as `fromStripe` and `async toSlack`, run live. The printer still writes
     a function's request without `await`, which compiles to the same unit.

   Next are the L4's slices 5 to 11: `try`, `catch {}`, `throw` and `finally`, over a `fail` leaf;
   computing loops, as folds over a bounded range; requests and calls that wait (`await` with
   JavaScript's order and required, `Promise.all`, a call that parks its caller, a request recorded
   before it is sent); waits a route's caller outlasts; waiting loops; failures with a reason
   (`catch (e)`); and the language server and both editors over the type package `holon.d.ts`,
   which `holon new` and `holon types` write beside a flow (2026-11-07, ahead of slices 5 to 10).

Dependencies: 3 needs 1 and 2; 4 needs 3 (the golden test is what makes the printer checkable);
5 and 6 need 4; 7 and 8 need 5; 9 needs 3 (its lowering is 3's) and 6 (the samples it converted).

## 13. Open forks

Decided provisionally above; listed here so they are revisited on evidence, not forgotten.
Decided ones are kept, with the decision, for a reader who wonders why. F1 (bare words as param
values), F2 (positional params for zero-input leaves) and F9 (`respond` with a bare number) are
gone: TypeScript's syntax settled them (a text is quoted, the outside world is imported, a status
is `status(204)`).

- **F3. String interpolation.** *Decided (2026-10-24), and kept by the switch:* a template literal,
  `` `Hello, ${name}` ``, lowers to one `concat` over its pieces, each `${…}` part wrapped in
  `as_text`, the leaf added for this. `+` stayed numeric, refused beside a text, until
  *2026-11-07:* `add` takes any values and joins texts as JavaScript's `+` does, so `+` beside a
  text is read (js-library-races/texts.md).
- **F4. Top-level bindings beyond literals.** The restriction exists because handlers must be
  separate groups. An alternative is duplicating the whole subexpression per handler, which keeps
  them separate at the cost of hidden node copies. Refused for now: the error message points at
  functions, which is the honest fix.
- **F5. A choice is a fallback.** *Decided (2026-10-24, for the first syntax's choice; `?:`
  since the switch):* `c ? a : undefined` is the guard, and `c ? a : b` is the same guard with a
  `coalesce` fallback, so a choice has one meaning, fallback, which differs from JavaScript's where `a` has no
  value (§6.4), and `select` is a plain call for the strict choice. The `coalesce` leaf it needs is
  built (§5.6). *Reversed (2026-11-07):* `c ? a : b` is a guard on each side, `guard(c, a) ??
  guard(!(c ?? false), b)`, so a chosen `a` that is absent makes it absent, as JavaScript gives
  `undefined`; the fallback is written `(c ? a : undefined) ?? b`. The fall-through had made the
  natural text composites wrong (js-library-races/texts.md).
- **F6. Named inputs.** *Decided (2026-10-24):* a named argument is an input when the unit has that
  port, else a param. Since the switch the names are a last record, `{ headers: h, limit: 50 }`,
  read so when the unit takes each of its keys; otherwise the record is a value, passed in order.
  The one clash, `default`, is never written.
- **F7. Where the layout of a text-authored flow lives.** *Decided (2026-10-24):* in the editor's
  saved flow only, for now. Structure is identity and shape is only visual, so a `.holon.ts` file
  carries no positions: outside the editor it is laid out by the builder on import, and inside it
  the saved flow keeps each node's position by its name in the text, so a text edit or a canvas
  edit leaves the arrangement where it was (`docs/L4s/text-pane-L4.md`). An author who exports and
  imports again gets a fresh layout. A sidecar file (names to positions) is the next step if
  anyone asks; positions in the text are refused.
- **F8. Arrows with captures, by lifting.** *Built (2026-10-25):* `map` and `fold` pass the values
  given after the list to their function, after the item, and an arrow's captures are lifted to
  those inputs (§5.9). The `quote` sample shows it. The first syntax could also pass those values
  explicitly; TypeScript's form for that comes in a later slice.
- **F10. Effects inside functions.** Writes inside a function leave it as named outputs: a
  collection's write bound in the method form and returned, a state write as the output `write`.
  Alternative: names derived from their keys (`put_posts`, `write_last_post`). Decide when the
  first function with several writes exists.
- **F11. A choice by value, `switch`.** A record used as a table is spoiled by one absent entry,
  and a chain of `?:` fell through when the chosen arm failed (until F5's reversal). A `switch`
  chooses one case by the subject, so cases not chosen don't matter and a chosen case's failure is
  the result's. It lowers to `equals`, `guard` and `coalesce`, with the `default` guarded by "no
  case matched". Decided and built as `match` (§6.5), and written as `switch` since the switch,
  fall-through refused; the `calc` sample shows it. `docs/L4s/match-L4.md`.
- **F12. Lazy choices.** Every node was evaluated, so a request inside a branch not taken was sent
  anyway, and an `await` there parked the run. Decided and built: `guard` and `coalesce` evaluate
  only the inputs they need, with "not yet answered" kept apart from absent so a choice waits
  instead of deciding early (§6.4). No value changes, only what the host is asked. `select` stays
  strict: an absent side makes it absent, so it needs both. The `cache-aside` sample shows it.
  `docs/L4s/lazy-choices-L4.md`.
- **F13. Calls are effects.** A request nobody read was refused as a statement and dropped from its
  handler's effects, so a route or clock of requests alone did nothing and the same request twice
  was refused as a value computed twice; a route of `flow_call`s alone answered with the call's
  answer. Decided and built: a call is an effect, kept by its handler whether or not its answer is
  read, and its answer is never its handler's (§5.4, §5.10). The `fan-out` sample shows it. And a
  call bound on a statement was made only when something needed its answer, so whether a charge
  went out could be decided by a reader lines away. Decided and built: a call is made where it is
  written, when the run reaches its statement or when the branch it is written in needs it (§6.4);
  the `idempotent` sample's charge says its own condition. Left open: in a function, a call bound
  on a statement is still made when its answer is needed, as the function's boundary would need an
  output the printer can't yet write back.
- **F14. Routes answer, actions wait.** A route answered first and waited after, so one handler was
  two in disguise: a request answered with a link, and a run that went on for days. Decided and
  built: a route answers and is done, and what waits is an action it starts by calling it without
  `await`, `payment(x)`, answered with the started run's tokens once it waits (§5.4). `await
  event(…)` and `resume_token` in a route are refused, saying so. The `approval` and
  `approved-payout` samples show it. Left open: a resume that finds its run by a key the route
  chose, which a payment provider's webhook needs. `docs/L4s/typescript-syntax-L4.md` §9.
- **F15. `fmt` and the canvas** (the L4's fork 13). `fmt` formats the source as written (§7); the
  printer writes its own form (§11). Open: what a canvas edit does to a text written in forms the
  printer doesn't write (reassignment, destructuring, an early `return`): either the canvas edits
  the source through the spans the reader keeps (`docs/L4s/text-graph-link-L4.md`), or editing
  such a handler on the canvas converts it to the printed form, said before it does.
- **F16. Types Holon doesn't have** (the L4's fork 19). An object type's fields (`{ price: number
  }`): Holon's `Record` has no field types, so they are either checked by core, which is new, or
  refused. A union of different shapes as `Sum<…>`. Type parameters, which a port of one type can't
  carry. Until decided, an object type checks as `Record`, and the other two are refused.

## 14. What this document does not decide

The MCP server and its tools (decided in `docs/L4s/agent-surface-L4.md`), the export and verifier
for run logs, and the marketplace's `use`. Each depends on this language existing and none changes
it. The sandboxed WebAssembly module is decided and built (`docs/L4s/wasm-module-L4.md`); the
language's spelling for one is the `js()` function of §5.8a, and a form placing a module by hash
waits for a second module. The TypeScript statements still to come (`try` and `catch`, loops,
requests and calls that wait) and the type package are decided in
`docs/L4s/typescript-syntax-L4.md`, and arrive here as they are built.
