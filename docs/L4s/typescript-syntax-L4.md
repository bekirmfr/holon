# Holon in TypeScript's syntax: a flow is a TypeScript module the compiler holds to Holon (L4)

**Status:** design (2026-11-03; TypeScript, awaited requests and authored log levels
2026-11-04; checked again once authored log levels landed, 2026-11-05). Nothing here is built.
It gathers a discussion of the language's reading frictions into one shape, revised after a review
that checked every claim against the code; §16 lists the choices made and the ones left open.
Every claim about the code as it is cites the file and line it was read from.

Holon is in development, so this replaces today's form outright: nothing is kept compatible with
it, its unit hashes or what is deployed with it (§14).

Holon's text reads unevenly. Each kind of handler has its own shape (`route GET /p (req) { … }`,
`every 1h { … }`, `on file uploads (f) { … }`, `action forget (input) { … }`), a function is a
`holon name(inputs) -> outputs js { … }` or an in-place `fn(a) -> expr`, and the `js` bodies inside
a flow are JavaScript while the code around them is not. This note makes the whole file one
language, TypeScript, whose code is JavaScript's with types that stay optional:

> **A Holon file is a valid TypeScript module with Holon's meaning.** It parses as standard
> TypeScript, ES2025 with optional types. Holon's compiler lowers it to the same kind of graph as today, keeps Holon's semantics
> where they differ from JavaScript's (absence, writes at the end, logic on booleans; §6 lists
> every difference), and refuses, with a message that says what to write instead, what a graph
> can't keep or where the two meanings would mislead.

Starlark is the precedent: Python's syntax, with semantics of its own where it needs them (values
frozen after a module loads, no unbounded loops by default), and read without effort by everyone
who knows Python. The graph, the manifest, the approval, the log and `holon verify` stay what they
are.

## 1. What is already there

**The language and its tools**

- holon-author's own lexer, parser, printer and lowering (`crates/holon-author/src/lang/`: `lex.rs`,
  `parse.rs`, `print.rs`, `lower.rs`). Headers, handlers and statements are in
  `docs/manual/language/reference.md`; the keywords at lines 85-90.
- **`holon fmt` prints from the graph, not the text.** It compiles the file and prints the result
  back (`crates/holon-author/src/commands.rs:333-338`); the printer never changes the graph, and
  `compile(print(doc))` carries the same hashes (`print.rs:1-14`).

**Identity and approval**

- A unit's hash covers its kind, params, input and output ports by name and type, and a
  composite's wiring and children; positions, labels and descriptions are out
  (`crates/holon-core/src/hash.rs:1-17`). A `js` body's text is part of the flow's identity,
  exactly as written (reference.md:540-541); its engine compiles `(inputs) => { body }`
  (`lower.rs:67-72`).
- **A declaration names something; it grants nothing.** What a flow may do is worked out from what
  it uses (reference.md:77-78). Reading and writing are separate capabilities, for example
  `ChannelSend` and `ChannelRead`, from the leaves `channel_send` and `channel_subscribe`
  (`crates/holon-core/src/kind.rs:338-339`). A scheduled
  flow implies `Clock` whether or not it reads the time (`kind.rs:247-255`).
- A redeploy asks only for what the previous grant didn't cover (`crates/holon/src/deploy.rs:239-243`);
  an agent can't acknowledge (`deploy.rs:167-168`).
- An outbound host must be literal text, as it is the permission (`lower.rs:1862`). `now` lowers to
  a `clock` node (`lower.rs:1524`), the `Clock` capability (`kind.rs:340`); in an `every` block it
  is the tick's time (`lower.rs:1522-1523`). `now` and a secret are the same within a run
  (reference.md:245).
- The limit names are `LIMIT_NAMES` (`crates/holon-author/src/lib.rs:49-88`): `request_timeout_ms`,
  `run_budget_ms`, `max_await_seconds`, `max_waiting_runs`, `max_call_depth`, `keep_runs`,
  `keep_run_days` and the rest.

**How a flow runs**

- **Absence.** A value that doesn't arrive makes everything built from it absent
  (reference.md:565-568). `if c then a else b` is `a` when `c` is true *and `a` has a value*,
  otherwise `b`; `a ?? b` is `b` when `a` has none; `||`, `&&`, `!` work on booleans; `==` is
  structural, never converting; there is no `+` on texts (reference.md:129-138).
- **A value is present, absent or pending.** There is no failed state (`crates/holon-core/src/eval.rs:49-55`).
  A `js` body that throws, or runs out of fuel, gives an absent output, and the reason goes to the
  observer for the log (`eval.rs:141-144`, `377-384`; reference.md:538-539). `??` is the
  `coalesce` leaf (`primitives.rs:269`). `respond body` answers 404 when `body` has no value,
  unless `missing:` says otherwise (reference.md:285; `lower.rs:945`); a route with no `respond`
  that writes answers 204 (`crates/holon/src/data.rs:170-171`), and one with no answer at all 500
  (`data.rs:163-165`). `a ?? 1`, with a literal, is the `default` leaf (`lower.rs:1796-1803`).
- **Reads at the start, writes at the end** (reference.md:569-571). Writes are effect values;
  `collection_put` takes its key as an input (`catalog.rs:280`), `state.key` is a frozen param of
  `store_write` (`catalog.rs:269`), and a run writes a state key once (`lower.rs:726-729`). The host
  performs a list of effects in order, the "computed fan-out" of `map`
  (`crates/holon-host/src/lib.rs:141-165`).
- **`after x`** ties a statement's writes and calls to `x`: they happen once `x` has a value, and not
  at all when it has none (reference.md:402-404).
- **Calls out happen between rounds** (reference.md:572-573); an answer is `{status, headers, body}`
  (reference.md:228).
- **Every run ends.** No loops, no recursion; `map` and `fold` go over a list and `range` makes at
  most 10000 items (reference.md:578-579; `MAX_RANGE`, `primitives.rs:734`), building the list
  (`primitives.rs:381-387`).

**Handlers and functions**

- An action answers with a value alone, with no `status:`, `missing:`, `headers:` or `stream:`
  (reference.md:332-334). At the top level of a flow with handlers only literal bindings are allowed
  (reference.md:294-296). An `every` block places a tick node that is its trigger
  (`lower.rs:1109-1121`), which a flow without handlers doesn't: its `every` header sets the
  server's interval (`lower.rs:512`), and its `on file` header reads `ingress` (`lower.rs:1640-1666`).
- **In-place functions** are anonymous holons whose body is one expression; their one output is
  `out`, fed by the body's node, and each captured name becomes an input port named after it,
  sorted (`lower.rs:1448-1484`). They go only in a `fn:` argument of `map` or `fold`
  (`lower.rs:1587`); extras are passed after the item (reference.md:553-554); a parameter never used
  is refused (`lower.rs:1477-1479`).
- **A `js` holon with several outputs** returns a record, one field per output
  (`lower.rs:1376-1387`). Three samples have multi-output `js` holons: inbox-report
  (`inbox-report.holon:23`), nearest (`nearest.holon:16`) and tally (`tally.holon:18`); slugs has a
  multi-output graph holon (`slugs.holon:28`).
- **`js` holons** run Boa compiled to WebAssembly under `wasmi`, with fuel and memory budgets (default
  200M and 16 MiB), native on the host and wasm in the editor (`crates/holon-module/src/lib.rs:6-9`, `23-28`,
  `38-51`; Boa: `docs/LANGUAGE.md:1288`).
- **Secrets.** Taint is followed port by port from each `secret` leaf; a node whose insides aren't
  followed, *a `map` or `fold` over a function* or a module, counts everything under it as derived
  when a secret reaches it (`crates/holon-core/src/taint.rs:1-26`, `69-70`, `292-298`).

**Waiting and calls**

- A request answered "not yet" parks the run under a named waiting point
  (`crates/holon-host/src/rounds.rs:359-373`). An `await` with no `expires:` waits 7 days, the
  leaf's default (`catalog.rs:257`), and none waits more than `MAX_AWAIT_SECONDS`, 90 days
  (`primitives.rs:891`, `894-897`); a deployment's own limit, `max_await_seconds`, is 30 days by
  default (`crates/holon/src/permissions.rs:54`). The name is a frozen param of the `await`
  leaf, which takes no inputs (`catalog.rs:257`; `durable.rs:37-62`). An event delivered under a
  name answers every read of it (`rounds.rs:361`). Tokens are derived per run and waiting point
  (`crates/holon/src/durable.rs:122-129`), and a resume route claims the run waiting at that point
  (`durable.rs:614-619`).
- `resume_parked` delivers any event to any parked run (`durable.rs:635-715`); a deadline delivers
  `{expired: true}` (`durable.rs:718-736`). Deadlines are held to `max_await_seconds`, and a
  deployment's waiting runs to `max_waiting_runs` (`durable.rs:578-583`). A resumed run reads state
  as it was when it started (`durable.rs:25-26`) and is evaluated again from its record
  (`durable.rs:653-665`). A resume turned away by a run lock waits for its event to be delivered
  again (`durable.rs:682-686`).
- A route whose run parks answers its caller with what it answered meanwhile (`durable.rs:4-5`;
  `crates/holon/src/listeners.rs:183-185`).
- **Flow calls.** `flow.action(x)` runs the other flow's action in-process between rounds and answers
  `{answer, run}` (`answer` absent for an action that only writes) or `{error, message}`, with `run`
  when the callee ran (`calls.rs:104-113`); a callee that parks answers `waiting` at once
  (`calls.rs:68-117`, `96-100`). The chain of calls lives in `RunWaits.chain`
  (`durable.rs:172-174`), which a `ParkedRun` doesn't keep (`durable.rs:265-313`). What a caller gets
  is withheld and redacted in `run_action` (`deploy.rs:1200-1209`). One sample calls another flow, twice:
  `shop.holon:22`, `29`.

## 2. A file

Every file has the same three parts, in order: imports, the one statement that says what kind of
file it is, and what that kind holds.

```ts
import { flow, route, every, js, respond, status } from "holon"
import geminiKey from "secret:gemini_api_key"
import games     from "collection:games"
import replies   from "channel:replies"
import gemini    from "host:generativelanguage.googleapis.com"
import { now }   from "holon:clock"

flow("chess", { port: 8168, limits: { request_timeout_ms: "50s", run_budget_ms: "1m" } })

route.get("/game", () => games["current"] ?? rules(opponent(""), ""))
```

- **The file is `NAME.holon.ts`.** The double extension says what `vite.config.ts` says: TypeScript,
  run by a particular tool. GitHub, Prettier, ESLint (with typescript-eslint) and every editor read
  it as TypeScript. Run with Node, which strips types, it fails at the `"holon"` import (or, with the
  type package of §13 installed, at the first `secret:` one).
- **The vocabulary is imported** from `"holon"`, so nothing is a magic global; an incompatible change
  later is `"holon@3"`. No directive.
- **The kind statement** is `flow(…)`, `tour(…)` or `component(…)`, once, after the imports, with
  literal arguments.

**Why not classes.** `class Chess extends Flow { … }` was considered. A method calls another
through `this`, so every call to `rules` would read `this.rules(…)`; letting bare names work would
stop it being TypeScript. A class also suggests instances, `new`, mutable fields and inheritance,
none of which a flow has. A module is already the single unit a flow is. React moved from class
components to function components because a class around one `render` is ceremony; a Holon
component is one `view`.

## 3. Names and the manifest

**An import names what the flow uses; it grants nothing,** as a declaration does today
(reference.md:77-78). The manifest is still worked out from the graph: a collection the flow only
reads asks only to read it.

| Import | Today |
|---|---|
| `import k from "secret:NAME"` | `secret NAME` |
| `import state from "state"` | `state KEY`, its keys by use (`state.hits`) |
| `import c from "collection:NAME"` | `collection NAME` |
| `import d from "folder:NAME"` | `folder NAME` |
| `import db from "database:NAME"` | `database NAME` |
| `import ch from "channel:NAME"` | `channel NAME` |
| `import h from "host:HOST"` | the literal host of `http.*` (`lower.rs:1862`) |
| `import { now } from "holon:clock"` | `now`, outside an `every` handler (§4) |
| `import { random } from "holon:random"` | `random` |
| `import { f } from "flow:SLUG"` | `calls SLUG` and `SLUG.f(…)` |
| `import { stripe } from "holon"` | `stripe.verify(…)` |

- **ES imports are static by definition**: top-level, with a literal source, which is the rule a
  manifest needs. `secret(someVariable)` can't be written. A state key is written as a name
  (`state.hits`), never computed, as `store_write`'s key is a frozen param (`catalog.rs:269`).
- **Three things are named that were inferred:** the outbound host, the clock and randomness. A
  reviewer reads them in the first lines, and a request says its host once: `gemini.post("/v1beta/…",
  { … })`. The clock and randomness take the `holon:` prefix, like the other capabilities' prefixes:
  `"holon:clock"`, `"holon:random"`.
- **An `every` handler receives its tick's time** as its parameter, `every("5s", (now) => …)`, as a
  route receives its request: the tick is the block's trigger, and `now` in it is already the tick's
  output (`lower.rs:1522-1523`). It needs no import, as a scheduled flow implies `Clock` whether or
  not it reads the time (`kind.rs:247-255`). Elsewhere `now` is a read of the host's clock, its own
  `Clock` capability (`lower.rs:1524`, `kind.rs:340`), and is imported: the chess sample's
  `remember(…, now)` in `POST /move` and `POST /chat` (`chess.holon:958`, `968`).
- **The rest stays derived, as today:** waiting points (`Await`), a secret a route may answer, an
  action another flow runs, file arrivals, database tables.
- **Options are import attributes** where they belong to the thing imported:
  `with { optional: "true" }` for today's `optional`, in quotes, as an import attribute's value is
  a string.
- **Items by brackets, methods by dot.** `games["current"]`, as the samples write today
  (`chess.holon:938-983`); `games.list()`, `replies.pipe(…)`, `replies.subscribe()`. A key named
  `list` is `games["list"]`, so items never collide with methods.
- **Flow settings** are the kind statement's options, under today's names:
  `flow("desk", { port: 8300, keep: { runs: 100, days: 30 }, browsers: ["https://desk.example"],
  limits: { run_budget_ms: "1m" } })` for `port`, `keep N runs for Dd` (reference.md:66; the
  limits `keep_runs` and `keep_run_days`, `lib.rs:54`, `84-85`), `browsers` and `limit NAME VALUE`.

## 4. Handlers

| Handler | Today |
|---|---|
| `route.get("/path/:id", (req) => { … })`, and `.post`, `.put`, `.delete`, `.patch` | `route GET /path/:id (req) { … }` |
| `route.post("/pay/:t", (req) => { … }, { resumes: "paid" })` | `route POST /pay/:t (req) resumes paid { … }` |
| `every("1h", (now) => { … })` | `every 1h { … }`, `now` the tick |
| `on.file.created(uploads, (f) => { … }, { matching: "*.csv" })` | `on file uploads matching "*.csv" (f) { … }` |
| `on.file.deleted(uploads, (f) => { … })` | `on file uploads deleted (f) { … }` |
| `export const approve = action((input) => { … })` | `action approve (input) { … }` |
| `on.email.received(support, (mail) => { … })` | none: when email support lands |

- **Routes follow Express.** The path is a literal. A handler that uses `await` is `async`, as
  TypeScript requires.
- **An action is an exported `action(…)`**: what reaches the flow from outside, a person running it
  or another flow importing it (§9). Its name is the export's, so `import { approve } from
  "flow:desk"` names it. An `action(…)` that isn't exported is refused: nothing could reach it.
- **Every handler is a call**: a function, then options. An action is one too, so all four take
  their options the same way, and an option added later has a place on every handler.
- **`matching`** is an option of the file handler, not of the folder: one folder can have several
  handlers, as drop-board's `board` has two (`drop-board.holon:39`, `45`), each with its own pattern.
- **Every file has handlers.** A flow without handlers (`every 10s` or `on file X` as a header,
  its body the run) becomes one handler, whose block places its trigger node as `every` and `on
  file` blocks do (`lower.rs:1109-1121`, `1137-1157`): drop-box, file-audit, heartbeat and
  rolling-log. This changes how they lower: today the header sets the server's interval
  (`lower.rs:512`) or reads `ingress` through `file_arrival` (`lower.rs:1640-1666`). `ingress`, the input of
  a flow without handlers, becomes the handler's parameter.
- **A handler that uses nothing it receives** leaves the parameter out: `every("1h", () => …)` when
  the tick's time isn't read.
- **A handler's parameter can be destructured**: `({ params, json }) => …` for a route (§5).
- **How its runs are recorded** is the option `log`, on every handler, strict when left out:

  ```ts
  route.get("/health", () => "ok", { log: "unrecorded" })
  route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })
  every("1m", () => …, { log: "lazy" })
  on.file.created(inbox, (f) => …, { log: "unrecorded" })
  export const report = action((input) => notes.list({ limit: 50 }), { log: "unrecorded" })
  ```

  It is today's word before the block (reference.md:339-375), and lowers to the handler node's `log`
  parameter as that word does (`lower.rs:1061-1063`, `1115`, `1147`, `1180`); strict is the default
  and never written, as today.
- **The level is the author's**: any handler may say it, whatever it does, as today
  (reference.md:363-372); core reads it as written (`declare.rs:39-44`, `275`). A person approves an
  unrecorded handler at deploy, `Unrecorded("GET /health")` (`manifest.rs:50-54`), told it keeps
  that handler's runs out of the log (`describe.rs:39`). What such a run did is in no record, and
  what reads the log says so rather than taking it for a break: `holon verify` leaves the state it
  may have moved unchecked, as it does a lazy run's lost last second (`verify.rs:508-511`), a
  restore's dry run says its runs aren't counted (`restore.rs:294`), a flow it calls logs the call
  as from a run not in its log (`calls.rs:89-92`), and a channel message it sends reaches whoever
  listens then and isn't kept. Ungranted, it runs strict (`permissions.rs:1099-1103`), and a host
  may refuse unrecorded handlers altogether (`permissions.rs:766-769`). Waiting, and §9's record
  before a request is sent, work at every level: a parked run keeps its own record, with the
  messages it sent as it parked (`durable.rs:298-302`).
- **The top level** holds imports, the kind statement, handlers, functions and `const`s of
  literals (reference.md:294-296); anything else is refused (§12).

**Answering.**

- `return value` answers 200 with it.
- `status(code, body)` answers `code`. When `body` is absent the answer is absent, and an absent
  answer is 404 as today (reference.md:285): `return status(201, paid.answer) ?? status(502)` is
  shop's `respond …, status: 201, missing: 502`.
- `respond(body, { headers, stream })` is for the rest.
- `?? status(404)` is today's `missing: 404`.
- **Several `return`s are one answer**, the route's one `respond`: each body and status is chosen
  where its branch is taken, as `if` and `match` choose (§5), with one `missing:` for all, and two
  `return`s that say it differently are refused. A `return status(s)` alone answers `s` with an
  empty body; where `s` is what an absent body answers anyway, it is that absent body. So calc's
  `switch`, each case `return a + b ?? status(400)` and `return status(400)` after it, is the
  sample's `respond match …, missing: 400`.
- `status` and `respond` are a route's answer and nothing else: a `status(…)` stored, passed to a
  function or returned from an action is refused, as an action answers with a value alone
  (reference.md:332-334).
- **A route answers and is done.** It never waits, so it never answers first and goes on: one that
  would hand out a link and wait for it starts an action that waits (§9, "Routes answer, actions
  wait"), and `return` stays its only answer.

**Tours and components** take the same file shape (`docs/L4s/tours-L4.md`, `docs/L4s/components-L4.md`):
`tour(…)` then `step("attack", "Now attack it", () => { … })` in the order written, and
`component(…)` then `export default view((data) => column([ … ]))`. A tour's directions (`say`,
`you open …`) become calls imported from `"holon/tour"` that compile into the document, not the
unit, as tours-L4 §11 fork 2 requires. Both notes are rewritten in this form when they are built.

## 5. Statements

- **`const`** is a binding, as `x = …` is today.
- **`let` and reassignment** become one new name per assignment. Where branches assign the same
  variable, the merge is Holon's `if` expression: `let fee = 0; if (big) fee = 5; else if (vip) fee
  = 1` lowers to `if big then 5 else if vip then 1 else 0`. An early `return` lowers the same way.
- **`var`** lowers like `let`; reading it before it is set reads absent. `holon fmt` rewrites it to
  `const` when it's never reassigned and to `let` otherwise, as `eslint --fix` does with `no-var`
  and `prefer-const`; where that would change the meaning (read before set, read outside its
  block), the compiler says so instead.
- **`if` around writes** is `when`: `if (fresh.status === "playing") games["chat"] = []` is today's
  `games["chat"] = [] when fresh.status == "playing"`.
- **Writes in two branches to one key merge into one write**: `if (a) state.k = 1; else state.k =
  2` lowers to `state.k = if a then 1 else 2`, as a run writes a key once (`lower.rs:726-729`).
- **`if (x !== undefined) { … }`** is `after x`: the statements inside happen once `x` has a value,
  and not at all when it has none (reference.md:402-404). A `let` with no value before it, given
  one inside, is tied to `x` too: `let sent; if (x !== undefined) sent = await api.post(…)` is
  today's `sent = http.post(…) after x`, absent when `x` is. `undefined` is the way to write "no
  value", in this test and as a branch: `c ? a : undefined` is today's `if c then a`.
- **`switch`** with a `return` or an assignment in each `case` lowers to `match` (`lower.rs:1579`, `1746`);
  fall-through is refused.
- **Writes** keep their meaning: `coll[k] = v`, `delete coll[k]`, `state.k = v`, performed when the
  run ends (reference.md:569-571). A JavaScript reader expects a write to be seen at once, so
  **reading a collection after writing to it in the same handler is refused**, per collection, as
  keys are values only known when the flow runs (`catalog.rs:280`); a state key, which is a name, is
  checked exactly.
- **Writes in a loop.** Each turn's collection write is an effect value, and the host performs the
  list in order (`holon-host/src/lib.rs:141-165`). A state key in a loop would be written once per
  turn, which is twice (`lower.rs:726-729`): compute in the loop, write once after it.
- **Local mutation**: `list.push(x)` is `list = [...list, x]`, `obj.k = v` is `obj = { ...obj, k: v
  }`, on locals only.

**Destructuring** in `const` and `let`, in assignment and in a handler's parameter. A function's
parameter is a name, destructured in the body (below). Each pattern lowers to leaves the catalog
has; none is new.

| Pattern | Lowers to |
|---|---|
| `const { a, b } = r` | a `field` leaf per name (`catalog.rs:185`) |
| `const { a, b } = f(x)`, `f` with outputs `a` and `b` | those outputs, no node: a name that is one of a multi-output node's outputs selects it, as `.a` does today (`lower.rs:1527-1538`) |
| `{ a: x }` | the same `field`, bound as `x` |
| `{ a: { b } }` | a `field` of a `field` |
| `{ a = 1 }` | `r.a ?? 1`: `default`, the literal frozen (`lower.rs:1796-1800`, `catalog.rs:233-237`); `coalesce` when the default is computed (`catalog.rs:232`, `primitives.rs:269`) |
| `{ [k]: v }` | `lookup`, the key from a wire (`catalog.rs:194`) |
| `{ a, ...rest }` | `rest` is `without_field` per named key (`catalog.rs:191`, `primitives.rs:453-458`) |
| `const [x, y] = xs` | `get` at 0 and 1, each index a `const` (`catalog.rs:167`, `primitives.rs:357-360`) |
| `[, y]` | the hole is skipped: `get` at 1 only |
| `[first, ...rest]` | `head` and `tail` (`catalog.rs:141-142`, `primitives.rs:419-426`); `[a, b, ...rest]` is `tail` twice |
| `[a, b] = [b, a]` | no node: two new names (`let`'s renaming, above) |

- **Defaults mean what they mean in JavaScript.** A missing field, an index past the end and the
  head of an empty list are absent (`primitives.rs:445`, `359`, `421`), Holon's `undefined`, so `= 1`
  applies exactly where JavaScript's would.
- **A name a pattern binds is a binding like any other**: one that is never read is a warning, as
  an unused binding is today (reference.md:120-121), and places no node.
- **A handler's parameter** destructures the record it receives. A route's request node already
  splits it into `params`, `query`, `headers`, `body` and `json` (`templates.rs:135-145`), so
  `route.post("/move", ({ json: { move } }) => …)` selects the `json` output and reads one field,
  the graph `req.json.move` gives. `method` and `path` are fields of the request (`data.rs:153-154`).
  An action's `action(({ amount, currency }) => …)` reads fields of its `input`
  (`templates.rs:149`). A handler's port has a fixed name, so its pattern never needs one.
- **A function's parameter is a name**, destructured on the first line:

  ```ts
  function total(order) {
    const { price, qty } = order
    return price * qty
  }
  ```

  A parameter is an input port named after it (`lower.rs:1239-1244` for a holon, `1475-1476`
  for an in-place function), and the hash covers port
  names (`hash.rs:1-17`): `order` is what the canvas shows, what the caller passes, and what the
  hash keeps; the `const` is two `field` leaves inside. The same holds wherever a value becomes a
  function's port: an arrow passed in place (`xs.map((x) => { const { id } = x; … })`), a `for … of`
  binding, which lowers to the same function (§10), and a `js()` function, whose ports are its
  parameters even though its body runs as written. A pattern there is refused with this rewrite
  (§12, fork 16).

## 6. Where it means something else

The syntax is TypeScript's, and its code means what JavaScript's does; these meanings are Holon's. Each is in the manual's language pages, and
where a JavaScript reader would be misled the compiler refuses instead (§12).

| JavaScript | Holon | Why |
|---|---|---|
| `a.b` on `undefined` throws | absent | absence propagates (reference.md:565-568); `?.` is accepted and means the same |
| `{ ...x }` with `x` undefined is `{}` | absent | a leaf with an absent input gives none (`primitives.rs:260-262`); the spread is the record's base (`lower.rs:1550-1553`) |
| `c ? a : b` with `a` undefined is `undefined` | `b` | Holon's `if` (reference.md:131) |
| `a \|\| b` on any values | booleans only; refused otherwise, with "use `??`" | reference.md:135 |
| `"a" + "b"` | refused, with "use a template literal" | no `+` on texts (reference.md:138) |
| `==` converts | `==` and `===` are structural, never converting; `fmt` writes `===` | reference.md:136 |
| `xs.map((x, i) => …)` passes an index | refused: extras are passed after the item, not an index | reference.md:553-554 |
| a write is seen at once | performed when the run ends | reference.md:569-571 |
| `Date.now()` moves | `now` is fixed for the run, the tick's time in `every` | reference.md:245; `lower.rs:1522-1523` |
| `await` waits for a promise | waits for an answer: a request's or a start's within the run, a call's or an event's possibly parking it, never in a route (§9, §10) | `rounds.rs:1-8`; `durable.rs:1-26` |

**The library.** `JSON.parse` and `JSON.stringify` are the `json_parse` and `json_stringify` leaves,
`Object.keys` is `keys`, a list's `.length` is `length` and a text's is `text_length`
(`catalog.rs:155`), and string and list methods map to the leaves that have them
(`catalog.rs:20-74`, and the manual's leaves page). No leaf computes a `Math` function (`floor`,
`round`, `min`, `max`, `abs`): the numeric leaves are `add`, `subtract`, `multiply`, `divide`,
`compare`, `sum` and `range`. A method without a leaf is refused, with "do it in a `js()` function".

## 7. Functions

```ts
function slugify(text) { const slug = …; return slug }
function summarize(rows) { return { rows: kept, report: lines } }
const { rows, report } = summarize(input)

const page  = js(function page(board, talk) { … })
const rules = js(function rules(board, move) { … }, { fuel: 1_000_000_000 })
const near  = js(function closest(points, to) { … }, { outputs: ["best", "distance"] })
```

- **A plain function or arrow is a graph holon**, drawn on the canvas, as `holon name(…) -> … { … }`
  and `fn(…) -> …` are today. Its body may be statements (§5), not only one expression.
- **Its output's name** is in the hash, and the canvas writes it where the wire leaves the node. A
  function that returns a binding names its output after it (`return slug` gives `slug`, as
  `holon slugify(text) -> slug` does today), and `out` otherwise: `return text.toLowerCase()`
  gives `slugify -> out`. A function whose every `return` is an object literal with the same keys
  has one output per key (`summarize -> (rows, report)`), and its callers destructure it. So
  `const slug = …; return slug` and `return …` are two units computing one thing; a caller with one
  output never writes it. A lone `out` shows on the canvas as its type, as a leaf's single output
  does today, and any other name as written (`PrimitiveNode.tsx:585-593`).
- **An arrow passed in place** (`xs.map((x) => …)`) is today's `fn(x) -> …`, whose output is `out`
  (`lower.rs:1484`), as a function's is. Each name it captures becomes an input port
  named after the variable (`lower.rs:1448-1480`). `let`'s renaming (§5) never renames a captured variable's port.
- **`js(function name(…) { … }, { fuel, memory, outputs })`** is today's `holon name(…) -> out
  js fuel N { … }`: the body runs in the Boa sandbox under its budgets, with all of JavaScript. Its
  output is named after the function, as the body is opaque; `outputs` names several, each a field
  of the record it returns, as today (`lower.rs:1376-1387`). Its text is its identity, exactly as written
  (reference.md:540-541): no formatter touches it. A decorator (`@js`) was considered and dropped:
  TypeScript, like JavaScript, allows decorators only on classes and their members.

### Types

```ts
type Item = { name: string; price: number }

function slugify(text: string): string { … }
function total(items: Item[]): number { … }
const rules = js(function rules(board: Board, move: string): Board { … }, { fuel: 1_000_000_000 })
```

Today a holon's ports may carry types, checked when given (reference.md:491, `581-586`), and a
unit's hash covers them (`hash.rs:1-17`). JavaScript has no way to write them; TypeScript does:

- **A parameter's type is its port's type, a return type its output's**, both optional, as today.
  Without one the port is untyped, as `holon slugify(text)` is.
- **They map onto Holon's nine types** (reference.md:583-584):

  | TypeScript | Holon |
  |---|---|
  | `number`, `boolean`, `string` | `Number`, `Boolean`, `Text` |
  | `T[]`, `Array<T>` | `List<T>` |
  | `[A, B]` | `Product<A, B>` |
  | an object type, `{ name: string }`, `Record<string, T>` | `Record`; its fields' types: fork 19 |
  | `unknown`, `any` | `Any` |
  | `T \| undefined`, `name?: T` | `T`: every type's value may be absent (reference.md:565-568) |
  | a union of different shapes, `A \| B` | `Sum<…>`: fork 19 |

  `Effect` isn't written: a write is a statement, not a value (§5).
- **`type` and `interface`** are allowed at the top level. They name types and place no node; a
  type parameter (`function first<T>(xs: T[])`) is refused for now (fork 19).
- **Holon checks the types itself**, against the catalog: wiring a `string` into a `number` port is
  today's mistake at the argument (reference.md:584-586). `tsc` isn't needed. An editor running
  TypeScript's checker over `holon.d.ts` (§13) shows mistakes earlier: a request that isn't awaited
  is a `Promise`, so reading `.body` from it is a type error (§9), and `games["current"]` is
  `Board | undefined`, so the checker asks for `??` or `?.` where Holon's absence can happen (`?.`
  means what plain `.` does, fork 5). Where the two checkers disagree, Holon's compiler decides and
  `holon.d.ts` is fixed.
- **A `js()` body may carry types**, removed by the compiler before Boa sees it. Boa runs JavaScript
  only (`boa_engine` 0.22, `fixtures/modules/js-engine/Cargo.toml:16`), and the compiler already
  builds the engine's source, `(inputs) => { body }` with the body as written (`lower.rs:67-72`).
  - **Each type is replaced by spaces**, as Node's type stripping does, so every line and column
    stays where it was, and an error Boa reports points at the file. The engine module doesn't
    change.
  - **The identity is the body with its types blanked**: everything else exactly as written
    (reference.md:540-541), but a change to a type alone, which changes nothing that runs, is not a
    new version.
  - **Only types that blanking can remove**: annotations, `type`, `interface`, `as`, `satisfies`.
    What emits code (`enum`, `namespace`, a constructor's parameter properties) is refused in a
    `js()` body, as TypeScript's `erasableSyntaxOnly` refuses it.
  - Dropped: a stripper inside the engine module, which keeps the types in the identity but grows
    the module by a TypeScript parser and costs each run fuel to strip.

## 8. `try`, `catch`, `throw`

These reduce, because in Holon a failure already is an absence (`eval.rs:49-55`).

```ts
function parse(order) {
  if (!order.items) throw new Error("an order has items")
  return { id: order.id, total: order.items.reduce((s, i) => s + i.price, 0) }
}

route.post("/orders", (req) => {
  let order
  try {
    order = parse(JSON.parse(req.body))
    orders[order.id] = order
  } catch {
    return status(400, { error: "not an order" })
  } finally {
    seen["last"] = now
  }
  return status(201, order)
})
```

- **`throw new Error(message)`** is a new `fail(message)` leaf: absent, with the message given to the
  observer for the log, as a `js` body's failure is (`eval.rs:141-144`, `377-384`). Uncaught, it
  makes the handler's answer absent: a route answers 404, as `respond` does with no value
  (reference.md:285, `lower.rs:945`), or what its `?? status(…)` says.
- **`try { A } catch { B }`** (no binding, standard since ES2019): each value the `try` block
  produces becomes `tryValue ?? catchValue` (`coalesce`, `primitives.rs:269`); the `try` block's
  writes are tied to its values having arrived, the `catch` block's to their not having arrived.
  The test is built from existing strict leaves (`equals(x, x) ?? false` is `true` exactly when `x`
  arrived, a `NaN` aside, as `NaN` isn't equal to itself, `primitives.rs:163-169`); a `present(x)` leaf would say it plainly.
- **`finally`** holds statements tied to neither branch.
- **`catch` catches absence, not only exceptions.** JavaScript tells `undefined` from a thrown error;
  Holon doesn't. A missing item that something in the `try` block depends on reaches `catch`, where
  JavaScript would carry on with `undefined`. For a flow that is usually right, as absence is how a
  flow says something failed (`respond …, missing: 404`), but it is a difference, and the manual
  says so.
- **`catch (e)`** needs the reason as a value, and an absence carries none: it only reaches the
  observer. A later slice makes a failure a slot state of its own, carrying its reason, which `??`
  treats as absent and `catch (e)` reads as `e.message`. That changes `Slot` (`eval.rs:49-55`) and
  every place a failure is produced (modules, `fail`, a leaf that gives none), and it closes the gap
  above: `catch` then catches failures only, and absence goes on as `undefined` does. Until then,
  `catch (e)` is refused with "write `catch { … }`; the reason is in the run's log".

## 9. Calling out: requests and other flows

```ts
export const summarize = action(async (input) => {
  const answer = await gemini.post("/v1beta/interactions", { body: prompt(input) })
  reports[input.id] = answer.body
})
```

**A request is awaited, as in JavaScript.**

- **`await` is required on a request**, as on a flow call: `holon.d.ts` types a request as a
  `Promise` (§13), and reading its answer without `await` is a compile error that says so.
- **It still happens between rounds, within the run** (reference.md:572-573; `rounds.rs:1-8`), held
  to `request_timeout_ms`. A request never parks a run.
- **A request not awaited where it is written is started**, and `await` takes its answer later:

  ```ts
  const a = gemini.post("/v1beta/interactions", { body: ask })   // both go out in this round
  const b = bank.get(`/payments/${id}`)
  const [asked, paid] = await Promise.all([a, b])
  ```

  `Promise.all` sends them together, as today's graph sends every request that is ready in a round
  (`rounds.rs:394-419`).
- **Two `await`s in a row run in order**, as in JavaScript: the second request is built once the
  first is answered, tied to it as `after` ties a statement (reference.md:402-404), through a
  `present` leaf, new (§8). Today the graph sends both in one round whatever the text's order.
- **A request written as a statement of its own**, its answer read by nothing, is made for what it
  does, an effect (LANGUAGE.md §5.4): not awaited, it is started and nothing waits for it, as the
  `fan-out` sample's two posts are.
- **A request is recorded before it is sent.** Each round's requests are written, with the run's
  inputs and its rounds so far, before they go out, as a parked run already keeps them
  (`durable.rs:265-313`). After a restart, an action's, a clock's or a file handler's run resumes
  from that record, and a request that was out is answered with an error saying `lost`, as a
  request that breaks a limit gets an error answer (`rounds.rs:10-13`). It is never sent again: a
  request goes out at most once, and the flow decides what to do. A route's run ends there instead,
  its caller gone, its writes not performed (reference.md:569-571). Today a run in flight is lost
  with the process.
- **The cost** is one write per round of requests, outside routes. It is the run's own record, not
  its log line, so it holds at every log level (§4).

**A call to another flow** is awaited the same way:


```ts
import { action } from "holon"
import { propose } from "flow:desk-model"

export const review = action(async (input) => {
  const { answer, error } = await propose({ order: input.order }, { timeout: "10m" })
  if (answer !== undefined) refunds[input.order] = answer
  else failures[input.order] = { error }
})
```

**An awaited call to another flow waits for its result, outside a route; a timeout is optional.**

- **The answer stays a record**: `{ answer, run }`, or `{ error, message }` with `error` one of
  `refused`, `not_found`, `not_running`, `no_action`, `failed`, `too_deep` (`calls.rs:13-15`) and,
  new, `timeout`; `run` is there whenever the callee ran (`calls.rs:104-113`). `waiting` goes away,
  except in a route (below).
  `(await propose(x)).answer ?? fallback` handles every failure at once, for an action that answers
  a value: one that only writes answers `{ run }` (`calls.rs:12`), and its success has no `answer`
  either.
- **`await` is required to read the call's answer**, as on a request. A call not awaited is
  started (below): its handle is there at once, and awaited later or passed to `Promise.all` it
  is waited for then; written as a statement of its own, it isn't waited for at all, as a request
  isn't (above).
- **A fast answer stays immediate**: a callee that answers at once is answered at once, as today.
- **A slow one parks the caller**, durably, until the callee's run finishes, fails or expires.
  Nothing is held open.
- **A route never parks** (below). A call it awaits is answered within its run: what the callee
  answered, or, when the callee waits, `{ error: "waiting", run }`, as today (`calls.rs:96-100`).
  A route that wants the callee to go on waiting calls it without `await`, starting it, and
  answers with where it waits.
- **The timeout** is the call's waiting point's deadline. Without one it is the `await` leaf's
  default, 7 days (`catalog.rs:257`), held to the deployment's `max_await_seconds` (30 days by
  default, `permissions.rs:54`) at deploy (`durable.rs:66-72`) and when the run parks
  (`durable.rs:581-583`). When it
  passes, the `{expired: true}` delivered (`durable.rs:718-736`) is answered to the call as
  `{ error: "timeout" }`. The callee's run goes on; its answer goes nowhere.
- **Unchanged:** a secret isn't sent (`calls.rs:85-87`), a flow isn't called from its own chain
  (`calls.rs:78-80`), a chain is at most `max_call_depth` long, 3 by default and 8 at most
  (`calls.rs:81-84`; `permissions.rs:54`, `59`), a limit like the others (`lib.rs:49-88`), and the import is a permission, approved like any
  other.

**What has to be built** (the rest exists, §1):

1. **The call answers "not yet", and the record says it was made.** When the callee parks
   (`calls.rs:96`), the call node answers `Answer::NotYet` under a waiting point of its own (the
   call's path) instead of the value `{error: "waiting"}`. The run's record keeps "call made, callee
   run N", so evaluating the caller again (`durable.rs:653-665`) never runs the callee again.
2. **The link, and the chain, persisted.** The callee's `ParkedRun` (`durable.rs:265-313`) keeps the
   caller's slug, run and waiting point, and the chain of calls, which today lives only in
   `RunWaits.chain` (`durable.rs:172-174`). Without the chain, a callee that parks and resumes later
   loses the no-call-back rule and its depth.
3. **Delivery, after the lock.** When `resume_parked` lets a finished run go (`durable.rs:687-688`)
   the deployments are locked (from `durable.rs:680`), and `resume_parked` takes that lock itself
   (`durable.rs:647`). The answer is delivered to the caller once the lock is released, through
   `resume_parked`.
4. **Redelivery.** A resume turned away by a run lock waits for its event to be delivered again
   (`durable.rs:682-686`). An expiry is: the scheduler sweeps every second (`scheduler.rs:224-228`).
   A resume route's caller isn't told, as the route answers before the run resumes
   (`listeners.rs:301-307`), but its token still works. A callee's answer has neither: the host
   keeps undelivered answers and tries again.
5. **Redaction on the delayed path.** What a caller gets is withheld and redacted in `run_action`
   (`deploy.rs:1200-1209`); an answer delivered later goes through the same.
6. **The deploy check.** `awaits_in` lists only `await` nodes (`durable.rs:37-62`). A unit that calls
   a flow can park too: its timeouts are held to `max_await_seconds` and its parked runs count
   against `max_waiting_runs` (`durable.rs:578-583`).
7. **Requests.** The record written before a round's requests go out, the `lost` answer after a
   restart, a route's run ending there, the order two `await`s give, and `Promise.all`.

The one sample that calls another flow (`shop.holon:22`, `29`) is rewritten with `await`. Its
calls are in routes, so they are answered within the run (below).

**Routes answer, actions wait.**

A route that answers and then waits is two handlers in one. In the approval sample
(`approval.holon:21-27`), what comes before `await paid` runs because an order came in, and its
answer goes to that caller; what comes after runs because someone paid, perhaps a day later, long
after that caller was answered. TypeScript can't write it: `return` ends the handler, and an
`await` before the `return` parks the run, which answers its caller what it answered meanwhile,
404 (`listeners.rs:183-185`). So a route doesn't wait. It starts an action that does, by calling
it without `await`:

```ts
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
```

- **`await` is the only thing that waits, and a run waits as long as whoever waits for it.** An
  action started has nobody holding it open: it waits for days. A route's caller holds a
  connection, which can't be put on disk, and browsers, proxies and Stripe give up in seconds. So
  `await event(…)` and `sleep(…)` in a route are refused (§12), as `await` in a `.holon` route is;
  a wait its caller outlasts, held in memory within the request's limit, comes with slice 8. A route
  that answers before it writes needs nothing new: without a wait, where its answer is written
  changes nothing its caller sees, as the answer goes out when the run is done.
- **An action called without `await` is started.** `payment(x)` runs the action in-process between
  the route's rounds, as a call's callee does (`calls.rs:68-117`), until it finishes or first
  waits, and answers then: `{ run, answer }` when it finished, `{ run, waiting }` when it waits,
  `waiting` naming each point it waits at with that point's token, or `{ error, message, run }` as
  a call does. That is JavaScript's own order: an async function runs to its first `await` before
  it hands back its promise, so the handle is there when the call returns, and reading `.waiting`
  needs no `await`. It never parks its caller, in any handler. `await charge(x)` calls it instead,
  its answer read (above). Temporal and .NET draw the same line with an API of their own
  (`client.workflow.start(…)` and `execute(…)`; `ScheduleNewOrchestrationInstanceAsync` and
  `CallSubOrchestratorAsync`); here the language's `await` draws it.
- **The handle is data, not a promise.** A promise lives in one process's memory; a started run's
  handle goes into a link, an email or a payment's metadata and still works days and restarts
  later, and its tokens can't be guessed, each delivering one event to one run. `holon.d.ts`
  types `payment(x)` as a `Run`, which can be awaited and has `run` and `waiting` (§13), so
  `charge(x).answer`, an `await` forgotten, is a type error. Awaiting a handle later, to wait for
  its run's end, comes with calls that park their caller (slice 7).
- **In `.holon`**, which has no `await` on calls, it is `started = start(payment, req.json)`, or
  `start(billing.charge, x)` for another flow's action, whose import is approved as a call's is
  (`calls.rs:75-77`). The flow's own action needs the flow's name, `flow approval`, as the pair
  names it. `start` isn't a word of TypeScript's form: importing it is refused, saying to call the
  action.
- **The token is the started run's.** It is derived from the run and the point
  (`durable.rs:122-129`), so the start can hand it out the moment the run waits, and the resume
  route claims the run waiting there as today (`durable.rs:614-619`). The run's later waiting
  points hand out their own: `resume_token(…)` today, and with slice 7 an `event(…)` not awaited,
  a wait point whose `.token` is there before the run waits on it.
- **A flow may start its own action.** A flow isn't called back today (`calls.rs:78-80`); a start of
  its own action is let through, as nothing waits on it, so no two runs can wait on each other, and
  its depth still counts (`calls.rs:81-84`). It asks `RunsAction("approval/payment")`, as a call
  does, said "run the action payment of the flow approval". The action stays exported (§4): a
  person or another flow may start it too, as approved today.
- **What crosses the wait is the action's input.** Today's route carries every name it bound across
  `await paid`; the action carries what it was given, and its record shows it.
- **approved-payout splits the same way.** Its route reads the balance to answer it and starts the
  payout; the action waits for `approved`, then reads the balance again, the reading it pays on,
  checked at its commit as today (`approved-payout.holon:1-9`).

**What was built** (slice 3, 2026-11-05):

1. **The start** in both forms, `start(payment, x)` in `.holon` and `payment(x)` without `await` in
   TypeScript: the leaf `flow_start`, a call whose request says `start: true`,
   answering `{ run, waiting }` with the waiting points and their tokens (`token_for`,
   `durable.rs:122-129`) where a call answers `{ error: "waiting" }`. It is one of the calls,
   made for what it does (`CALLS`, LANGUAGE.md §5.4).
2. **Its own action:** a start let through the rule against calling back, its depth counted.
3. **The record:** the start is an exchange of the round, so a run evaluated again answers it
   from its record and never starts a second run. The token handed back is kept out of the
   starter's log (`‹token:paid›`), and a resumed part keeps its own tokens out of its entry, the
   event that woke it carrying one; `holon verify` re-runs a resumed action's part as it does a
   route's.
4. **The refusal:** `await` and `resume_token` in a route, in both forms and on the canvas, saying
   to start an action.
5. **The samples:** approval and approved-payout rewritten as a route that starts an action, run
   live; a TypeScript twin of approval; LANGUAGE.md states the rule (§5.4, F14).

## 10. Loops

You write an ordinary `while` (or `do … while`, `for (;;)`, `for … of`, with `break` and
`continue`). The compiler chooses how it runs from whether its body holds an `await`.

### Computing loops

```ts
let n = 0, x = start
while (x > 1) {
  x = x % 2 === 0 ? x / 2 : 3 * x + 1
  n++
}
```

- **It lowers to a `fold` over `range(N)`**, with the loop's variables and a done flag as the
  accumulator: each turn checks the condition and runs the body, or passes the accumulator on once
  done. If the flag is still false after the fold, it is `fail("this loop didn't end within 1000
  turns")`, the same leaf `throw` uses (§8): the handler's answer is absent, and the log says why.
  `for … of` over a list lowers to `map` or `fold` directly.
- **Every run still ends by construction**, and the replay is exact.
- **`N` is part of the unit**: `flow(…, { loops: { turns: 5000 } })`, default 1000. It is a constant
  in the graph, not a host limit: raising it changes the hash (a new version) and asks for nothing,
  as no capability changes. A single `range` is at most 10000 (`primitives.rs:734`); more turns are
  two folds nested, which the compiler writes. Nothing else bounds the cost today: the run's
  budget, `run_budget_ms`, is checked between rounds (`rounds.rs:257-271`), so a loop that calls
  nothing out runs all its turns whatever the budget, and `fold` has no step budget
  (`eval.rs:1340-1370`). A step budget in core, counted per turn, is part of the slice.
- **The cost:** turns after the loop ends still run, as pass-throughs, and `range` builds its list
  (`primitives.rs:381-387`). For large `N`, `fold` should stop once its accumulator says done: a
  change to `fold`, not a new node.
- **Secrets:** a `fold` over a function isn't followed by taint (`taint.rs:69-70`). A loop a secret
  reaches makes everything under it derived, as a `js` function does, and its writes are refused if
  they would keep it (`taint.rs:11-13`). What a graph loop keeps over `js()` is the canvas, the replay
  and termination, not finer secret tracking.

### Waiting loops

```ts
let paid = false
while (!paid) {
  const e = await event("paid", { expires: "1m" })
  paid = e.expired ? JSON.parse((await bank.get(`/payments/${id}`)).body).cleared : true
}
```

- **The same fold, with an `await` in each turn.** The run parks at each `await` and resumes as the
  same run, each turn a logged part of it. A resume route delivering `paid` wakes it at once (push);
  the deadline delivers `{expired: true}` and the turn polls. `sleep("1m")` is an `await` nobody
  delivers. `event` and `sleep` are imported from `"holon"`.
- **A waiting point per turn.** An event delivered under a name answers every later read of it
  (`rounds.rs:361`), so a loop's second turn wouldn't wait. The `await` leaf, which takes no inputs
  today (`catalog.rs:257`), takes the turn, and names its waiting point for the event and the turn
  (`paid#0`, `paid#1`, …). `awaits_in` still reads its base name and deadline statically
  (`durable.rs:37-62`).
- **One token for every turn.** A token is derived from the run and the waiting point
  (`durable.rs:122-129`). In a loop it is derived from the base name, so the link handed out
  at the first turn keeps working; a route that `resumes: "paid"` delivers to whichever turn the run
  is waiting at. The `Await` capability is per base name.
- **Requests, events, folders and databases bring news.** `now` is fixed for the run
  (reference.md:245), and state and collections are read as they were when the run started
  (`durable.rs:25-26`; `firing.rs:560-566`): a loop that polls them or the clock sees the same value
  every turn. Folders and databases are read as the run finds them (`firing.rs:567-570`), and an
  outbound request is made again each turn.
- **Bounds:** the loop's turns, `max_await_seconds` for each wait, and the run counts as one of the
  deployment's `max_waiting_runs` while it waits (`durable.rs:578-583`).
- **The cost:** a resumed run is evaluated again from its record (`durable.rs:653-665`), so turn *k*
  re-evaluates the *k* before it, answered from the record and not sent again. Tens or hundreds of
  turns, not thousands.

### Which to use

- **`while` with `await`:** one item, one run, its own context and evidence: an order waiting to be
  paid, in an action the route that took the order starts (§9).
- **`every` with a collection:** many items checked together on a schedule, one run per tick, what
  waits a collection the editors already show.
- **`js()`:** heavy computing, or what needs the whole language: the chess rules.

## 11. The chess sample, converted

The manifest, the functions and every route of `chess.holon`; the `js` bodies, over 800 of its 985
lines, are left out, as they don't change.

```ts
import { flow, route, js, respond, status } from "holon"
import geminiKey from "secret:gemini_api_key"
import games     from "collection:games"
import replies   from "channel:replies"
import gemini    from "host:generativelanguage.googleapis.com"
import { now }   from "holon:clock"

flow("chess", { port: 8168, limits: { request_timeout_ms: "50s", run_budget_ms: "1m" } })

const page       = js(function page(board, talk) { … })
const rules      = js(function rules(board, move) { … }, { fuel: 1_000_000_000 })
const opponent   = js(function opponent(model) { … })
const player     = js(function player(board, move) { … })
const movePrompt = js(function movePrompt(board, move) { … })
const choose     = js(function choose(board, asked, move) { … })
const remember   = js(function remember(talk, said, time) { … })
const chatPrompt = js(function chatPrompt(board, talk, line) { … })
const hear       = js(function hear(sent, line) { … })

route.get("/", () => {
  const board = games["current"] ?? rules(opponent(""), "")
  return respond(page(board, games["chat"] ?? []), { headers: { "content-type": "text/html; charset=utf-8" } })
})

route.get("/game", () => games["current"] ?? rules(opponent(""), ""))

route.post("/move", async ({ json: { move } }) => {
  const board = games["current"] ?? rules(opponent(""), "")
  const mine = rules(board, player(board, move))
  const asked = await gemini.post("/v1beta/interactions", {
    headers: { "content-type": "application/json", "x-goog-api-key": geminiKey },
    body: movePrompt(mine, move),
  })
  const pick = choose(mine, asked ?? { status: 0 }, move)
  const theirs = rules(pick.game, pick.move)
  const kept = remember(games["chat"] ?? [], pick.aside, now)
  games["current"] = theirs
  games["chat"] = kept.talk
  return { ...theirs, aside: kept.last ?? {} } ?? status(400)
})

route.post("/chat", async ({ json: { line } }) => {
  const talk = games["chat"] ?? []
  const board = games["current"] ?? rules(opponent(""), "")
  const sent = await replies.pipe(gemini.post("/v1beta/interactions", {
    headers: { "content-type": "application/json", "x-goog-api-key": geminiKey },
    body: chatPrompt(board, talk, line),
  }))
  const kept = remember(talk, hear(sent ?? { status: 0 }, line), now)
  games["chat"] = kept.talk
  return { reply: kept.last.agent } ?? status(400)
})

route.get("/replies", () => replies.subscribe())

route.post("/new", ({ json: { model = "" } }) => {
  const fresh = rules(opponent(model), "")
  games["current"] = fresh
  if (fresh.status === "playing") games["chat"] = []
  return fresh ?? status(400)
})
```

The two routes that call Gemini are `async` and await it; `/chat` hands the request to
`replies.pipe`, which streams its answer to subscribers and is what is awaited. The routes
destructure the body they read: `({ json: { move } })` is the graph today's
`req.json.move` gives (§5), and `{ model = "" }` is `req.json.model ?? ""` (chess.holon:981).
`games["chat"] = [] when fresh.status == "playing"` (chess.holon:983) becomes the `if`; `respond …,
missing: 400` becomes `?? status(400)`, which works because a refused move makes `theirs` absent and
so the record. Each `js` holon's output takes its function's name (`rules` where today's is
`next`, `page` where it is `html`, and so for every holon), which is all the canvas needs to label
it; `move_prompt` and `chat_prompt` (`chess.holon:757`, `876`) take JavaScript's camelCase. Both
change the units' hashes, as any conversion does (§14).

## 12. What is refused

Each refusal says what to write instead.

| Refused | Write instead |
|---|---|
| A closure capturing a variable reassigned after the closure is made | copy it into a `const` first |
| Mutating a local through an alias (`const b = a; b.push(…)`) | `const b = [...a]` |
| Recursion | a loop, or `js()` |
| Reading a collection after writing to it in the same handler | use the value written |
| A state key written in a loop, or computed | compute in the loop, write once after; a name |
| `catch (e)`, until failures carry a reason (§8) | `catch { … }` |
| `\|\|`, `&&` on non-booleans; `+` on texts; an index in a `map` callback | `??`; a template literal; extras |
| `switch` with fall-through | a `case` per value |
| `status(…)` or `respond(…)` outside a route's answer | a value, in an action |
| `await` on anything but a request, a call or an event | leave it out |
| `await event(…)` or `sleep(…)` in a route | an action that waits, which the route starts by calling it without `await`: `const started = payment(req.json)` (§9); a wait the caller outlasts comes with slice 8 |
| `start(…)`, `.holon`'s word | the action called without `await`: `payment(x)` |
| `await payment(x)`, the flow's own action | `payment(x)`, started, until a call waits for its run's end (slice 7) |
| A pattern that writes (`({ a: state.x } = r)`, `[coll[k]] = xs`) | bind the name, then write it |
| An `action(…)` that isn't exported; `export function` | `export const name = action(…)` |
| A pattern as a function's parameter, an arrow's, a `js()` function's or a `for … of` binding | a name, destructured on the first line: `(order) => { const { price, qty } = order; … }` |
| `class`, `this`, `new`, getters and setters | functions and records |
| Generators, `eval`, dynamic `import()`, `with` | `js()` for computing |
| Globals: `fetch`, `Date`, `setTimeout`, `process`, `globalThis` | `import … from "host:…"`, `"holon:clock"` |
| A top-level statement other than an import, the kind statement, a handler, a function or a `const` of a literal | move it into a handler |
| A computed import source or kind argument | a literal |

## 13. Tooling

- **A TypeScript parser.** Valid TypeScript means a real parser, not today's own
  (`lang/parse.rs`). It must build for wasm too, as the editor compiles through the bridge
  (`crates/holon-wasm`); `oxc_parser` is one, and parses TypeScript. A `js()` body's types are
  blanked to spaces before Boa runs it (§7). The refusals come from the lowering, with the parser's
  spans; the lowering's back half, from bindings to graph, stays.
- **Two printers.** Today's `fmt` prints the graph back (`commands.rs:333-338`), which would turn a
  hand-written `let` or `while` into ternaries and folds. So:
  - **`holon fmt` becomes a source formatter**: it lays out the TypeScript as written (Prettier's
    layout), rewrites `var` (§5) and `==`, and never touches a `js()` body, whose text is its
    identity (reference.md:540-541). CLAUDE.md's "kept as `fmt` writes it" then means this.
  - **The graph printer stays** for exporting a canvas as text, printing the `const` form.
- **Types, checked.** A `holon.d.ts` for `"holon"` and the import prefixes, so any TypeScript
  language server completes `route.post` and `on.file.created` and checks what they take and give:
  a request is `Promise<Response>`, an action awaited `Promise<{ answer?, error?, message?, run? }>`,
  one not awaited a `Run`, a `PromiseLike` of the same that is already
  `{ run?, answer?, waiting?, error?, message? }`, as the start has answered by the time the call
  returns,
  `event(…)` a `Promise` of what is delivered. `holon lsp` (`docs/L4s/language-server-L4.md`) adds
  Holon's checks over it.
- **The VS Code extension** uses TypeScript's grammar for `.holon.ts`; its `.holon` grammar
  (`editors/vscode/syntaxes/holon.tmLanguage.json`) goes.
- **Both editors** compile and highlight the new form through the same bridge.

## 14. Migration

Holon is in development, so the new form replaces today's at once:

- **One form.** The TypeScript parser replaces today's (`lang/parse.rs`); `.holon` files are no
  longer read. Nothing is kept compatible: units get new hashes, and what was deployed with today's
  form is deployed again from the new one.
- **No converter is shipped.** The samples, the manual's examples and the tests' flows are rewritten
  once, by hand or with a throwaway script, and checked as every sample is today: built by
  `editor/src/samples.test.ts`, deployed by the e2e tests, and run live once.
- **Coverage.** Every construct the samples used when this was written (54 of them, in
  `crates/holon-author/samples/`) has a form here: `state` (9 samples), `after` (counter), `await`
  and its token, in an action a route starts (approval, approved-payout, §9), `match` (calc), flows
  without handlers (4, each written with one handler now: heartbeat and rolling-log a clock,
  drop-box and file-audit a file handler), `browsers` (marketplace), `stripe.verify`
  (stripe-to-slack), a channel's `pipe` (chess, chat-proxy), a handler's log level (status-board,
  one of them `lazy` on a handler that writes, and stress-test; `lazy` also in notes-api and
  orders-api), multi-output holons (inbox-report, nearest, slugs, tally). The samples are converted in one slice, with `samples.rs`,
  `editor/src/samples.test.ts` and the e2e tests reading `.holon.ts`, and the manual's language
  pages rewritten.

## 15. Slices

1. **Parse and lower the core.** A TypeScript parser; imports; `flow(…)`; routes, `every`,
   `on.file.*`, exported actions, handler options (`resumes`, `matching`, `log`); `const`, `return`, `status`, `respond`; the absence operators and
   §6's differences; graph functions, in-place arrows, `js()` with its output names; the library
   mapping; type annotations as port types (§7). Until slice 4 the samples are read as `.holon`
   (`samples.rs`, the bridge, `editor/src/samples.test.ts`, the e2e tests), so this slice ships no
   sample of its own. A parity test stands in: each sample that uses only this slice's constructs,
   written in TypeScript, lowers to the same server unit, by hash, as its `.holon` form.
2. **Statements.** `let`, `var`, reassignment, destructuring, merged branch writes, `if` around writes, `if (x !==
   undefined)` for `after`, `switch` for `match`, early `return`, local mutation, the refusals of
   §12, and a single `await event("paid", { expires: "1d" })`, today's `await paid, expires: 1d`.
3. **Routes answer, actions wait.** *Built.* §9's five pieces: the start in both forms (`start(…)`
   in `.holon`, an action called without `await` in TypeScript), a start of the flow's
   own action, the start in the run's record, `await` refused in a route, and approval and
   approved-payout split into a route and the action it starts. It comes before the switch, as
   those two samples can't be written in TypeScript without it; a TypeScript twin of approval stands
   in, as slice 1's parity tests do.
4. **The source formatter and the switch.** `holon fmt` on TypeScript; every sample, test flow and
   manual example rewritten; today's parser and the `.holon` grammar removed; the canvas showing a
   lone `out` as its type (§7). *The formatter is built* (`lang/format.rs`, 2026-11-05): Prettier's
   layout at 100 columns without semicolons, its comments and blank lines kept, a comment inside an
   expression keeping its statement as written, and one rule of Holon's own, a handler's options
   kept after its function's closing brace (`}, { log: "lazy" })`). It answers only once the text
   it wrote compiles to the same flow, its comments all there; the twins are kept as it writes
   them. *The converter is built* (`lang/to_ts.rs`, 2026-11-06): a `.holon` tree written in
   TypeScript's syntax, its comments carried over by line, laid out by `fmt`. 54 of the 55 samples
   convert: 50 to the same unit (a test holds them to it), and the four without handlers each to
   one handler. slugs binds a `state.set`'s answer, which TypeScript has no form for, so it is written by
   hand. `compile` takes a text in either form, told apart by how it starts, while the samples and
   the tools switch. *The samples are switched* (2026-11-06): all 55 are `.holon.ts`, read so by
   `samples.rs`, the bridge, `editor/src/samples.test.ts`, the e2e tests and `holon new`, whose
   starter is TypeScript too. Heartbeat and rolling-log tick by an `every` handler, so a sample no
   longer has an interval and `holon tick` runs heartbeat by hand where `holon trigger` did;
   drop-box and file-audit answer by `on.file.*` handlers, their triggers labeled `on file inbox
   matching "*.csv"`. A write inside a function is its output `write` (slugs' `publish`). The
   twins are retired. The tools that still read only `.holon` (the LSP's hover, completion and
   format; the workbench graph; the inspector's examples, which now come from no sample; the
   editor's canvas printer and a saved flow's export) keep their fixtures in
   `crates/holon-author/tests/holon/` until they move to TypeScript with the parser's removal.
   *The manual is switched* (2026-11-06): its flows are `ts` blocks, checked as `.holon.ts` (a
   `holon` block is now a mistake in `tests/manual.rs`), the reference and the tour are rewritten
   for the TypeScript forms, and the guides, the CLI page and the README say imports, `return`,
   `status(…)`, handlers and options where they said headers, `respond`, blocks and lines. The
   test flows in `tests.rs`, `commands.rs` and `mcp.rs` and the agent's guide (`agent.rs`) are
   TypeScript too. Flows without handlers are gone from the manual, as TypeScript can't write
   one. What reads only `.holon` stays so until the parser goes: `docs/LANGUAGE.md` (whose §5
   `usage.rs` reads for the inspector), the editor guide's text pane, LSP and **import .holon**,
   the editor's test flows, the workbench's, and the handler-less server tests.
   *The parser is removed* (2026-11-06): `lang/parse.rs`, `lex.rs`, the converter and the
   `.holon` grammar are gone, and `compile` reads TypeScript alone. The lowering keeps as words
   of its own only those it still means something by (a TypeScript binding of one is renamed
   `name_`), and its messages say the TypeScript forms: `return respond(body, { headers })`,
   `import key from "secret:key"`, `export const pay = action((input) => …)`. The printer
   writes TypeScript: a canvas printed is a `.holon.ts` text that compiles back to the same
   unit, and the editor's text pane, its export and a run's view print so. The LSP, the
   workbench's graph and the inspector's examples read TypeScript, a handler's band titled as
   the log names its runs (`action bump`, `every 5s`, `on file inbox`), so the fixtures in
   `tests/holon/` are gone. `docs/LANGUAGE.md`, the editor guide, the engineering pages, the
   editor's and the workbench's test flows, the server's tests and the VS Code extension's test
   are TypeScript; to VS Code a flow is a TypeScript file, `holon lsp` beside its own server.
   *Flows without handlers are gone from the server* (2026-11-06): deploy refuses a unit that
   isn't a server with a route, clock, file handler or action, saying to add one, and a saved
   one is skipped at load. The deploy setting `interval_seconds`, the scheduler's whole-unit
   ticks, the `ingress` leaf (catalog 95 leaves), `holon trigger` and the unnamed
   `POST /trigger/<slug>` run went with it, and the editor's interval field and the deployed
   page's box that ran such a flow; `file_arrival` stays, read inside `on.file.*` handlers. The
   server's tests that deployed a bare canvas deploy TypeScript flows instead, run through an
   action, a route or a clock.
5. **`try`, `catch {}`, `throw`, `finally`,** and the `fail` leaf.
6. **Computing loops.** `while` to `fold`, the loop's turns, nested folds past 10000, `fold`
   stopping when done, a step budget in core.
7. **Calls and requests that wait.** §9's seven pieces, `timeout`, `await` required on requests and
   calls, the sample that calls. A started run's handle awaited later, or passed to
   `Promise.all`, waits for its run's end; an `event(…)` not awaited is a wait point with its
   `.token`, and `resume_token` goes.
8. **Waits a route's caller outlasts.** A route's `await` held in memory, within the request's
   limit, the event delivered to a run that isn't on disk, and a wait that could outlast the limit
   refused, saying to start an action. And, as Durable Functions'
   `WaitForCompletionOrCreateCheckStatusResponseAsync` does, a route awaiting a start for a few
   seconds: the result if its run ended by then, its link if it waits.
9. **Waiting loops.** The turn-numbered `await` leaf, one token for every turn, `event` in a loop,
   and `sleep`.
10. **Failures with a reason.** A failed slot state, `catch (e)`, `catch` catching failures only.
11. **Editors and tools.** `holon.d.ts`, the LSP over it, both editors, the VS Code extension. Tours
    and components are built in this form from their first slice.

## 16. Forks, surfaced

1. **Keyword blocks with TypeScript bodies, or calls?** Calls (`route.post(path, handler)`): the point
   is a file every tool already reads.
2. **A language directive (`"use holon 2"`)?** No: it says "a dialect", and nothing needs it; there
   is one form, and the import can carry a version later.
3. **Classes for the kinds?** No (§2).
4. **`require secret("…")` for what a flow uses?** The idea, a mark of its own, is kept; its
   spelling is ES `import`, static by definition. It names; the graph still derives what is asked
   (§3).
5. **Plain `.` propagating absence, or requiring `?.`?** Plain `.`, with `?.` accepted: today's flows
   rely on propagation everywhere.
6. **`await` on outbound requests?** Required, as in JavaScript (§9): a request not awaited is
   started, `Promise.all` runs several together, two `await`s in a row run in order, and a request
   is recorded before it is sent, so after a restart it is answered `lost` and never sent twice.
   Considered and dropped:
   - Refusing `await` on requests, keeping it for parking: a TypeScript reader reads an unawaited
     request as a promise, and the order the text shows wouldn't be the order it runs in.
   - Allowing it with no meaning: two spellings, and code reading in order while running together.
   - Every request parking the run: a route would answer before its request did (`listeners.rs:183-185`),
     each request would write to disk, and a request out during a restart would be sent again.
   - Parking only when slow, `await` optional: the two spellings would differ only after a restart.
7. **Output names.** From the returned binding or object keys, else `out`, as an arrow written in
   place has (`lower.rs:1484`); `outputs` for a `js()` with several (§7). The canvas shows a lone
   `out` as its type, as it shows a leaf's single output, and a name an author chose as written
   (`PrimitiveNode.tsx:585-593`): `out` says nothing the node's title doesn't. Considered and dropped:
   - The function's name, `slugify -> slugify`: a port may not take a holon's name
     (`lower.rs:1214-1217`), and the node already says it.
   - The function's name with `_out`, `slugify_out`: a name nobody would write.
   - `out` written on the node, as the canvas writes a placed holon's output name today: a word on
     most nodes that tells nothing.
8. **`while` as a node of its own, or a fold?** A fold (§10): no new primitive, every run still ends
   by construction.
9. **`await` inside `while`?** Allowed, with a turn-numbered waiting point (§10): one run per item
   keeps its context and evidence.
10. **The loop's turns: a host limit or part of the unit?** Part of the unit, a constant of the graph
    (§10): it isn't a capability, and a host limit couldn't change the graph's `range`.
11. **`var`: refused, or rewritten?** Rewritten by `fmt` (§5).
12. **`try`/`catch`: refused, reduced now, or exact later?** Reduced now with `catch {}`, and the
    difference stated (§8); exact, with `catch (e)`, once failures carry a reason.
13. **`fmt` and the canvas.** `fmt` formats the source (§13). Open: what a canvas edit does to a
    file written with `let` or `while`, which the graph printer can't print back as written; either
    the canvas edits the source through the spans the parser keeps (`docs/L4s/text-graph-link-L4.md`),
    or editing such a block on the canvas converts it to the `const` form, said before it does.
14. **JSX for components?** Open, decided with components.
15. **Keep today's form alongside, or replace it?** Replace it (§14): Holon is in development, so
    one parser, one grammar and no compatibility with today's hashes or deployments.
16. **A pattern as a function's parameter: what is its port called?** It isn't allowed: a parameter
    is a name, destructured on the first line (§5), so every port is named by the person, as today.
    Considered and dropped:
    - A port named by position, `arg1`: a name nobody wrote, on the canvas and in the hash.
    - A port named after the pattern, `{ price, qty }`: readable, but the hash then depends on the
      keys' order and spacing, and two parameters with the same pattern collide.
    - One port per name in the pattern, the caller's record split at the call: `map` and `fold` hand
      a function one item, which that would break.
    - `{ price, qty } as order`, a name and its parts, as Rust's `@` does: not TypeScript, whose `as`
      asserts a type inside an expression, nor JavaScript, which has it only in imports and exports.
    A handler's parameter may still be a pattern, as its port's name is fixed (§5).
17. **How a handler says its log level?** The option `log` on every handler, strict when left
    out, and so actions are calls too, `export const report = action((input) => …, { log:
    "unrecorded" })` (§4): one shape for every handler, the level on the handler's own line. The
    level is the author's, not limited to handlers that only read (§4). Considered and dropped:
    - A wrapper, `unrecorded(route.get(…))`, `export const approve = lazy(action(…))`: the word
      comes first, but a handler's options would then be in two places, and a second shape around
      every kind of handler says what one option says.
    - A keyword, `export lazy function`, `unrecorded route.get(…)`, as `async` is: TypeScript's
      grammar, like JavaScript's, has no such word, and every tool rejects the file. On a line of its
      own, `unrecorded` parses as a statement of its own, a name nobody uses.
    - `export function` for actions, with the level elsewhere: an exported function has no options.
    - The levels in `flow(…)`, keyed by label: away from the handler, and a mistyped label needs
      its own check.
    - A C#-style `[unrecorded]` above the handler: in TypeScript, as in JavaScript, it is an array expression, and the
      line before it without a semicolon reads it as an index (`flow(…)[unrecorded]`).
    - A label, `unrecorded: route.get(…)`: an `export` can't be labeled.
    - A JSDoc tag, `/** @unrecorded */`: valid and familiar, but a comment would change how runs are
      recorded, and a misspelled tag would have to be caught.
    - A parameter with a default, `(input, log = "unrecorded")`: a default is what a caller may
      override, and no caller passes one.
18. **JavaScript or TypeScript?** TypeScript. Today's optional port types (reference.md:491, `581-586`)
    had no JavaScript form, and TypeScript's checker catches a request not awaited and an absence not
    handled as the file is written (§7). Its types stay optional, so a flow without them is plain
    JavaScript in a `.holon.ts` file; `oxc_parser` reads both. Dropped: JSDoc types (`/** @param
    {string} text */`), which TypeScript also reads in JavaScript files, as they put types in
    comments.
19. **Types Holon doesn't have.** Open: an object type's fields (`{ price: number }`): Holon's `Record`
    has no field types, so they are either checked by core, which is new, or refused; a union of
    different shapes as `Sum<…>`; type parameters, which a port of one type can't carry. Until
    decided, an object type checks as `Record`, and the other two are refused.
20. **A route that answers, then waits?** No: routes answer, actions wait (§9). A route that hands
    out a link and waits for it is two handlers in one, the code after the wait running for the
    event, not for the request. It starts an action that waits, calling it without `await` (fork
    21), and answers with the token the start hands back. Considered and dropped:
    - A `respond(…)` statement, the route going on after it, as Express's `res.json()` does: one
      handler doing two handlers' work, and a route that waits without answering first answers 404
      (`listeners.rs:183-185`).
    - Two routes, the order stored between them: plain, but the deadline, the link working once and
      the one record become the author's: a clock sweeping stale orders, a check that the order is
      still pending, and two runs joined by an id where `holon verify` replays one.
    - A continuation, `return respond(x, { then: async () => … })`: the rest of the handler nested
      in an option.
    Left open: a resume that finds its run by a key the route chose, which a payment provider's
    webhook needs, as it carries the provider's reference, not Holon's token.
21. **How a handler starts an action?** By calling it without `await`, as JavaScript does: an
    async function runs to its first `await` and hands back its promise, which is where a started
    run answers with its handle (§9). `await` is the only thing that waits, and a run waits as long
    as whoever waits for it: an action for days, a route as long as its caller (slice 8). Considered
    and dropped:
    - `await start(payment, x)`, a word of Holon's, built first in slice 3: a waiting word around a
      word that says it doesn't wait. `.holon`, which has no `await` on calls, keeps `start(…)`
      until slice 4.
    - A route that parks and is answered `202` with its run's handle, as Durable Functions'
      `CreateCheckStatusResponseAsync` answers: a route's answer is its author's contract with its
      client (Stripe wants a 2xx within seconds, a browser a page, the approval its own link), which
      a generic `202` breaks.
    - A start as an API of its own, as Temporal's `client.workflow.start(…)` and .NET's
      `ScheduleNewOrchestrationInstanceAsync` are: the language's own `await` already says it.
