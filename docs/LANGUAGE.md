# The Holon language: flows as programs

**Status:** design, 2026-10-24. Nothing here is built yet; §12 is the build order.
**Companions:** `COMPOSABLE_VM_DESIGN.md` (the invariants this language must keep),
`PLATFORM_LAYOUT.md` (the runtime it compiles for), `crates/holon-author/src/text.rs` (the
wiring form it extends).

---

## 0. In one page

Holon already has a text form (`.holon`, read by `holon check`, written by `holon fmt`), but it
is a netlist: one line per node, one line per wire. It is the graph serialized, not a language.
The hello sample is twelve lines for one expression.

This document designs the **expression form**: a small, pure, total language whose programs
*lower to the exact units the canvas would have placed*. The core's hashed DAG is the
intermediate representation, and this language is a second surface over it, beside the canvas.
Nothing in the semantics moves. Purity, totality, derived manifests, content addressing and exact
replay are properties of the IR, so they hold for every program by construction.

```
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond("Hello, " ++ req.params.name)
}
```

```
holon slugify(text) -> slug {
  slug = join(split(lowercase(text), " "), "-")
}

route POST /stripe (req) {
  let event = stripe_verify(req, secret(name: stripe_webhook_secret), clock(), tolerance: 300).event
  let sent  = http_request(secret(name: slack_webhook_path),
                           { "content-type": "application/json" },
                           json_stringify({ text: "Stripe: " ++ event.type }),
                           method: POST, host: "hooks.slack.com")
  respond(sent.status, missing: 400)
}
```

A nested call is a wire. `let` is a named wire. A holon is a function. A route is one connected
group with a `request` node. The hash of a program is the hash of the DAG it lowers to, so names
and formatting are cosmetic, and two differently written programs with the same graph are the
same unit.

**The decisions, in one table.** Each is argued in the section named.

| Decision | Choice | Where |
|---|---|---|
| What the language is | A surface over the existing IR; every construct lowers to catalog leaves, templates and composites that exist today | §1, §6 |
| What it is not | General-purpose. No loops, recursion, user types, modules, or standard library growth | §2 |
| Calls | `name(inputs…, param: value…)`: positional arguments are inputs (wires), named ones are params (frozen) | §5.3 |
| Absent inputs | `_` skips an optional input; trailing optional inputs may be omitted | §5.3 |
| Literals | `1`, `"text"`, `true` lower to `const`; `{…}` and `[…]` to `record`/`with_field` and `list` | §5.4 |
| Operators | Arithmetic, comparison, logic, `++` (concat), `??` (default), `.` (field or output), `[ ]` (lookup), `if/then/else` (select), `when` (guard) | §5.5 |
| Multi-output values | `.name` selects an output when the name is one; otherwise it is a field. A bare multi-output value is its first output | §5.6 |
| Functions for `map`/`fold` | A holon by name, or a closed lambda `fn(a, b) -> …`; lambdas may not capture | §5.8 |
| Effects | Effect-typed values need no statement; an unused Effect is performed, an unused non-Effect is an error | §5.9 |
| Routes | `route METHOD /path (req) [resume name] { … }`; `respond` is wired to the request implicitly | §5.10 |
| Flows without routes | The top-level statements are the one unit | §5.11 |
| Sharing | A `let` used twice is one node with two wires; the same expression written twice is two nodes. No common-subexpression elimination | §6.2 |
| Hash | The lowered DAG's. Let names, holon names, layout and comments are not in it | §7 |
| Layout | Not in the text. Positions live in the flow document; import lays out automatically | §8 |
| Diagnostics | `file:line:col: message`; every node keeps its span, so `check`, replay and the agent tools point at the expression | §9 |
| The netlist form | Stays valid, as the wiring form, in the same file; `fmt` prints the expression form | §10 |

---

## 1. Why a language, and why now

Three reasons, in order of weight.

1. **Agents write the flows.** The product direction (agent actions you can prove) makes an agent
   the main author. An agent is far better at `join(split(lowercase(text), " "), "-")` than at
   five `node` lines and four wires: fewer tokens, fewer places to misname a port, and errors it
   can read. The MCP tools (`check`, `deploy`, `run`, `explain`) will speak this form.
2. **Expressiveness without new primitives.** Most of what today takes five nodes is one nested
   expression. The vocabulary stays at its 68 leaves; the text gets shorter.
3. **Review, diff and version control** become ordinary. The derived manifest stays the thing a
   human approves; the text is what they read when the manifest surprises them.

And why it costs little: `holon-author` already holds the netlist parser, the `FlowBuilder` that
every sample is built with, the composite freezer, and the route derivation. The language is a
parser and a lowering onto that builder, plus a printer for the other direction.

## 2. Principles and refusals

**P1. The IR is the program.** A `.holon` text is a way to write a flow document; the flow
document is a way to write units; the units are what runs, hashes, deploys and replays. The
language adds no construct that does not lower to units that exist today. If a construct needs a
new primitive, that primitive goes through the admission gate (design §13) on its own merits
first, and the language gets the sugar after.

**P2. Small, and deliberately not general.** The pull of every language is toward a type system
with generics, modules, a standard library, loops. Each of those would cost a year and would break
the thesis: the manifest is derivable and replay is exact *because* the vocabulary is closed and
every program is total. This language is closer to Dhall or Nix with effects at the edges than to
Python. When expressiveness genuinely runs out, the answer is a sandboxed deterministic
WebAssembly primitive (a module with no imports is pure, and is content-addressed by its bytes),
never a language feature.

**P3. Names are cosmetic, the graph is identity.** A `let` name, a holon name, a comment, the
order of independent statements: none of it reaches the hash. Two programs that lower to the same
DAG are the same unit, and `fmt` prints them identically.

**P4. The canvas is a view of the same program.** Text to canvas is compilation; canvas to text
is printing; both go through the IR, and both directions preserve every unit hash. The canvas
stays what it is best at: showing a run, node by node.

**P5. One implementation.** Lexer, parser, lowering and printer live in `holon-author`, pure like
core, and run natively in `holon`, in the server, and in the editor through `holon-wasm`. No
TypeScript copy of any of it.

**Refused, with the reason:**

| Refused | Why |
|---|---|
| `while`, recursion, unbounded iteration | Totality. `map` and `fold` over a list are the only iteration, bounded by the list |
| User-defined types, generics | The checker is wire-time compatibility over nine types. A type system is a different project |
| Modules, imports, packages | Reuse across files is the marketplace's job (a published holon placed by hash), not a `use` statement. Revisit when packs exist |
| String interpolation `"Hello, {name}"` | Sugar for `concat`, but `{x}` with a non-text `x` fails at run time. Deferred until `to_text` insertion can be typed (fork F3) |
| Lambdas that capture | A `fn` is a pinned unit applied by `map`/`fold` with fixed arity; a capture would need an input nobody supplies. Closed lambdas only |
| Common-subexpression elimination | The text's structure is the graph's. Sharing is explicit (`let`), so the canvas shows what was written |
| Null | Absence is not a value. `none()` is the optional's empty arm; "nothing" is the evaluator's absent slot (§6.3) |

## 3. A tour, by the samples

Every sample in `crates/holon-author/src/samples.rs`, written in the language. Each lowers to the
same nodes the builder places today, so each is also a golden test (§12). `req` is the name the
route gives its request; bare, it is the whole request record.

### hello-api

```
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond("Hello, " ++ req.params.name)
}
```

Lowers to: `request{method: GET, path: /hello/:name}`, `field{name: name}` on its `params`
output, `const{type: Text, value: "Hello, "}`, `concat(a, b)`, `respond{status: 200, missing: 404}`
with `request` and `body` wired. Five nodes, five wires: the sample.

### notes-api

```
flow notes-api
port 8102

route GET /notes (req) {
  respond(collection_list(_, req.query.after ?? "", collection: notes, limit: 50))
}

route PUT /notes/:id (req) {
  collection_put(req.params.id, req.json, collection: notes)
  respond(req.json, missing: 400)
}

route GET /notes/:id (req) {
  respond(collection_get(req.params.id, collection: notes))
}

route DELETE /notes/:id (req) {
  collection_delete(req.params.id, collection: notes)
  respond(status: 204)
}
```

`_` leaves `collection_list`'s optional `prefix` unwired. `?? ""` is a `default{type: Text,
value: ""}` node. The `collection_put` statement is an Effect nobody reads, so it is exposed and
the host performs it when the run finishes (§5.9). `respond(status: 204)` has no body input.

### stripe-to-slack

```
flow stripe-to-slack
port 8103

route POST /stripe (req) {
  let event = stripe_verify(req, secret(name: stripe_webhook_secret), clock(), tolerance: 300).event
  let sent  = http_request(secret(name: slack_webhook_path),
                           { "content-type": "application/json" },
                           json_stringify({ text: "Stripe: " ++ event.type }),
                           method: POST, host: "hooks.slack.com")
  respond(sent.status, missing: 400)
}
```

`stripe_verify` has two outputs, so `.event` selects one (§5.6). The record literal lowers to
`record()` then `with_field{name: content-type}` (§5.4). `sent.status` is a field of
`http_request`'s one output. When the signature fails, `event` is absent, nothing downstream
fires, `sent.status` is absent, and `respond` answers its `missing` status: 400, and nothing was
posted. Exactly the sample's behaviour, from the same nodes.

### guarded-api

```
flow guarded-api
port 8109
optional SecretRead("receipt_key")

route GET /me (req) {
  let ok      = secure_equals(req.headers.authorization, "Bearer " ++ secret(name: api_token))
  let receipt = hmac_sha256(secret(name: receipt_key), req.path, encoding: hex) ?? "unsigned"
  respond({ receipt: receipt } when ok, missing: 401)
}
```

`req.path`: `path` is not an output of `request`, so it is a field of the request record.
`when` is `guard(cond, value)`: when `ok` is false the body is absent and the route answers 401.

### slugs: holons

```
flow slugs
port 8110
optional SecretRead("receipt_key")

holon slugify(text) -> slug {
  slug = join(split(lowercase(text), " "), "-")
}

holon publish(slug, author) -> (put, saved, receipt) {
  let s   = slug ?? ""
  put     = collection_put(s, { author: author, at: clock() }, collection: posts)
  saved   = store_write(s, key: last_post)
  receipt = hmac_sha256(secret(name: receipt_key), s, encoding: hex) ?? "unsigned"
}

route POST /slugs (req) {
  let title     = slugify(req.json.title)
  let author    = slugify(req.json.author)
  let published = publish(title, author)
  respond({ title: title, author: author, receipt: published.receipt }, missing: 400)
}

route GET /slugs (req) {
  respond(collection_list(collection: posts, limit: 100))
}
```

A holon is a composite with a given boundary, frozen exactly as `end_holon_exact` freezes it: the
declared port names are the unit's port names. (A holon grouped on the canvas, or by the samples'
builder, gets the names core proposes, such as `s` and `out`; the quoted form of §10,
`holon slugify(text "s") -> (slug "out")`, carries those, so such a holon round-trips to the
same hash.) `slugify` is placed twice and is one unit, one hash. `publish`'s `put` and `saved` outputs are Effects nobody reads in the route, so they are
exposed and performed; its manifest (a collection write, a state write, the clock, an optional
secret) is derived from its leaves as always.

### heartbeat: a flow without routes

```
flow heartbeat
interval 10

store_write(clock(), key: last_beat)
store_write((store_read(key: beats) ?? 0) + 1, key: beats)
```

No routes, so the top-level statements are the one unit the clock fires (§5.11).

### approval: a durable run

```
flow approval
port 8105

route POST /orders (req) {
  respond("/paid/" ++ resume_token(await: paid))
  let paid = await(name: paid, expires: 86400)
  store_write({ ..req.json, paid_via: paid.method }, key: last_order)
}

route GET /paid/:token (req) resume paid {
  respond("thanks, the order is paid")
}
```

Statement order carries no meaning beyond names being defined before use: the response is
produced in the first round, the run parks at `await`, and the state write happens when the link
is opened. `{ ..base, k: v }` is `with_field` over an existing record (§5.4).

### fetch-json, sign-payload, likes, witness

```
route GET /slideshow (req) {
  let page = http_request("/json", method: GET, host: "httpbin.org")
  respond(json_parse(page.body).slideshow.title)
}
```

```
route POST /sign (req) {
  respond({ signature:   hmac_sha256(secret(name: signing_key), req.body, encoding: hex),
            body_base64: base64(req.body, mode: encode, alphabet: standard) }, missing: 400)
}
```

```
route POST /posts/:id/like (req) {
  let id = req.params.id
  let n  = (collection_get(id, collection: likes) ?? 0) + 1
  collection_put(id, n, collection: likes)
  respond(n, missing: 400)
}

route GET /posts/:id/likes (req) {
  respond(collection_get(req.params.id, collection: likes) ?? 0)
}

route GET /likes (req) {
  respond(collection_list(collection: likes, limit: 100))
}
```

```
route POST /heads (req) {
  let ok    = secure_equals(req.headers.authorization, "Bearer " ++ secret(name: witness_key))
  let batch = req.json.batch
  let now   = clock()
  collection_put(batch, { ..req.json, witnessed_at: now } when ok, collection: batches)
  respond({ batch: batch, witnessed_at: now } when ok, missing: 401)
}

route GET /batches (req) {
  respond(collection_list(_, req.query.after ?? "", collection: batches, limit: 200))
}
```

### map and fold

Not in the samples yet; the shape the language gives them:

```
holon total(prices) -> sum {
  sum = fold(0, prices, fn: fn(acc, p) -> acc + p)
}

holon slug_all(titles) -> slugs {
  slugs = map(titles, fn: slugify)
}
```

A `fn:` param takes a holon's name or a closed lambda. Either lowers to a composite whose hash
becomes the frozen `fn` param, which is what `eval_map` and `eval_fold` resolve today.

## 4. Lexical structure

- **Encoding:** UTF-8. Lines end in `\n` or `\r\n`.
- **Comments:** `#` to end of line. `fmt` keeps a comment on the line it was written on, moving it
  with that statement.
- **Keywords:** `flow` `port` `interval` `browsers` `optional` `holon` `route` `resume` `let`
  `if` `then` `else` `when` `fn` `node` `at` `in` `out` `true` `false`. Keywords are valid after
  a dot (`req.in`) and as record keys (`{ in: 1 }`).
- **Identifiers:** `[A-Za-z_][A-Za-z0-9_]*`. Route paths and consent names have their own rules
  below. A name that is a catalog leaf or template (`clock`, `respond`) cannot be bound by `let`,
  `holon` or a lambda parameter; the error names the clash.
- **Numbers:** a finite decimal: `0`, `86400`, `-1`, `2.5`, `1e3`. No hex, no infinity, no NaN,
  since `const` refuses them. Lowered as written, trimmed.
- **Text:** `"…"` with JSON escapes. Lowered as the unescaped string.
- **Bare words as param values:** after `param:`, a bare identifier or a route-path-like token is
  the text itself: `collection: notes`, `method: POST`, `host: "hooks.slack.com"` (a dot needs
  quotes). This keeps the common case unquoted, as the wiring form does.
- **Punctuation:** `( ) { } [ ] , : = . _ -> ..` and the operators of §5.5.
- **Route paths:** `/` followed by segments of `[A-Za-z0-9_.-]+` or `:name`, as `parse_path`
  accepts. Read as one token after `route METHOD`.
- **Consent names** (after `optional`): `Name` or `Name("text")`, exactly the `consent_name` form
  the server prints (`SecretRead("receipt_key")`, `HttpRequest("api.example.com")`).

## 5. Syntax and lowering

Grammar in EBNF; `*` zero or more, `?` optional, `|` choice. Newlines separate statements; a
statement may continue on the next line while parentheses, braces or brackets are open, or after
a binary operator. Semicolons are not used.

### 5.1 File

```
file      = header* item*
header    = "flow" ident | "port" number | "interval" number
          | "browsers" origin ("," origin)* | "optional" consent
item      = holon | route | statement | wiring        # wiring: the netlist form, §10
```

Headers are the flow's deploy settings, exactly as today (`Settings`). Statements at the top level
are the body of a flow without routes (§5.11); with routes, only `let` of literals is allowed at
the top level (§5.10).

### 5.2 Definitions

```
holon     = "holon" ident "(" ports? ")" "->" outputs block
ports     = port ("," port)*
port      = ident (":" type)? (string)?            # the string: the unit's own port name, §10
outputs   = port | "(" ports ")"
block     = "{" statement* "}"
route     = "route" method path "(" ident ")" ("resume" ident)? block
method    = "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
statement = "let" ident "=" expr
          | ident "=" expr                          # in a holon: an output's value
          | expr                                    # an effect, §5.9
```

A holon's block binds each declared output exactly once with `name = expr`; an input is a name in
scope. A route's block binds the request name; `respond` may appear once (or not at all when the
route only has effects, answered 204, the existing rule).

### 5.3 Calls: inputs are positional, params are named

```
call      = ident "(" args? ")"
args      = arg ("," arg)*
arg       = expr | "_" | ident ":" value
value     = number | string | bareword | ident      # a param's frozen text; ident for fn:
```

Positional arguments are **inputs**, matched to the unit's input ports in catalog order; each
becomes a wire. Named arguments are **params**, frozen into the node. A unit may have an input and
a param of the same name (`default` has both `value`s); the position-versus-name rule keeps them
apart without a second syntax.

- Too many positional arguments is an error, except on a variadic leaf (`add`, `multiply`, `and`,
  `or`, `concat`, `list`), where each extra argument grows the node by one input, named as the
  canvas names them (`c`, `d`, … then `in<n>`), through `FlowBuilder::arity`.
- `_` leaves an input unwired. Only an optional input may be left unwired
  (`http_request`'s `headers` and `body`, `collection_list`'s `prefix` and `after`, as
  `is_optional_input` and core's `has_optional_ports` say); for any other, `_` is an error
  naming the port. Trailing optional inputs may be omitted.
- An unknown param name, or a param given twice, is an error naming the unit. Params not given
  take the catalog's defaults, as a dropped node does.
- A zero-input call still takes parentheses: `clock()`, `record()`. This is what keeps a bare
  identifier unambiguous as a bound name.

### 5.4 Literals

| Written | Lowers to |
|---|---|
| `1`, `-2.5` | `const{type: Number, value: "<as written>"}` |
| `"text"` | `const{type: Text, value: …}` |
| `true`, `false` | `const{type: Boolean, value: …}` |
| `{ k: e, "k 2": e2 }` | `record()` then one `with_field{name: k}(record, e)` per field, in written order |
| `{ ..base, k: e }` | `with_field{name: k}` chained onto `base`'s value; fields in written order |
| `[ e1, e2, e3 ]` | `list(e1, e2, e3)`, variadic |
| `[ ]` | `list()` with zero inputs (`list` is well-defined at any count) |

Record keys are identifiers or strings; a key that is not a plain word must be quoted
(`"content-type"`). Record and list literals always lower structurally, never to a `const` of type
`Json`, so the lowering is one-to-one and the canvas shows the fields. A single-node JSON constant
is still available as `const(value: "[]", type: Json)` and as the right side of `??` (§5.5).

### 5.5 Operators

Lowest to highest precedence; all binary operators are left-associative.

| Operator | Lowers to | Note |
|---|---|---|
| `a when c` | `guard(cond: c, value: a)` | Absent unless `c` is `true` |
| `if c then a else b` | `select(cond: c, if_true: a, if_false: b)` | A prefix form; both arms are evaluated (§6.4) |
| `a ?? lit` | `default{type, value}(a)` | `lit` must be a literal: a number, text, boolean, or a record/list literal with only literals inside (frozen as `Json`). The fallback is a frozen param, which is why it cannot be an expression |
| `a \|\| b` | `or(a, b)` | Variadic: `a \|\| b \|\| c` is one node |
| `a && b` | `and(a, b)` | Variadic |
| `a == b`, `a != b` | `equals(a, b)`, `not(equals(a, b))` | Structural, any two values |
| `a < b`, `<=`, `>`, `>=` | `compare{op}(a, b)` | Numbers only, as `compare` is |
| `a ++ b` | `concat(a, b)` | Variadic; texts only |
| `a + b`, `a - b` | `add(a, b)`, `subtract(a, b)` | `add` variadic |
| `a * b`, `a / b` | `multiply(a, b)`, `divide(a, b)` | `multiply` variadic |
| `!a` | `not(a)` | |
| `a.name` | an output, or `field{name}(a)` | §5.6 |
| `a[e]` | `lookup(record: a, key: e)` | The key arrives on a wire |

Variadic flattening applies only to a chain of the *same* operator: `a ++ b ++ c` is one `concat`
with three inputs; `(a ++ b) ++ c`, with explicit parentheses, is two nodes. The parentheses are
the author saying "two".

### 5.6 Multi-output values: `.name`

A call to a holon with several outputs, or to the `request` or `stripe_verify` template, produces
several values. The rules:

1. `v.name`, where `name` is one of `v`'s output names, is that output. No node is placed.
2. Otherwise `v.name` is `field{name}` applied to `v`'s value (its first output), one node.
3. A bare multi-output `v` in input position is its first output. For `request` that is the
   request record, which is what `stripe_verify`, `respond` and `field` want.

Output names win over field names. To read a field of the request record that shares a name with
an output (`body`), write `req.request.body`: output `request`, then the field. The printer uses
the same rule in reverse, so the text round-trips.

A holon with several outputs may also be destructured: `let (put, saved, receipt) = publish(s, a)`.
Each name is a wire from that output.

### 5.7 Holons

```
holon publish(slug, author) -> (put, saved, receipt) { … }
```

Lowers through `FlowBuilder::begin_holon` … `end_holon_exact`: the block's nodes are the inner
canvas, each input name is a boundary input wired to every use, each `name = expr` is a boundary
output wired from its expression. The declared names are the unit's port names, in declared order,
so the composite freezes exactly as the expand view would freeze it. An input never used, or an
output never assigned, is an error.

Types on ports are optional annotations: `holon slugify(text: Text) -> (slug: Text)`. When given
they are checked against what the wires carry; when absent, a port takes its first wire's type, as
the canvas does today. A holon takes no params (as now); a flow's holon is placed by name,
`slugify(x)`, and the unit's hash is the same wherever it is placed.

Holons are defined at the top level and are visible to the whole file, in any order, so a holon
may use one defined below it. They may not be nested in text; a holon placed inside another is
the nesting.

### 5.8 Functions for `map` and `fold`

The `fn` param of `map` and `fold` takes:

- a holon's name: `map(titles, fn: slugify)`, lowered to that holon's hash;
- a closed lambda: `fn(acc, p) -> acc + p`, lowered to an anonymous composite with the
  parameters as inputs in order and the body's value as its one output, then to its hash.

A lambda's body may name only its own parameters, holons, and catalog units. A free name is an
error: "`fn` may only use its own arguments; a function is a sealed unit, applied to each item".
`map`'s function takes one argument; `fold`'s takes two, `(acc, item)`. Arity is checked at the
call.

The lambda's hash is its content's, so two identical lambdas are one unit, and the printer names
anonymous composites `fn_1`, `fn_2`… only when it has to print them as holons (a lambda whose
body is one expression prints back as a lambda).

### 5.9 Effects and statements

The IR has one rule for effects: an Effect-typed output that nothing consumes is exposed by the
group, and the host performs it when the run finishes (writes, puts, deletes, egress). The
language keeps that rule and adds no `perform` keyword:

- An expression statement whose type is `Effect` is an effect: `store_write(x, key: k)`.
- An expression statement of any other type is an error: "a value nobody uses". Bind it or
  remove it. (A `let` never used is a warning, printed by `check`.)
- A `let` bound to an Effect and never used is still performed; naming it changes nothing.
- A multi-output holon's Effect outputs that the caller does not read are performed. The `slugs`
  route performs `publish`'s `put` and `saved` without mentioning them.

Effects are values, so `if c then store_write(a, key: k) else store_write(b, key: k)` places two
writes and a `select`, and only the chosen Effect reaches the boundary: one write is performed.
The idiom the samples use, and the one the printer prefers, guards the value instead:
`store_write(a when ok, key: k)`.

### 5.10 Routes

```
route POST /orders (req) { … }
route GET /paid/:token (req) resume paid { … }
```

Lowers to a `request{method, path, resume}` template node named by the parenthesised identifier,
and the block's nodes. Every `respond(…)` and every use of `req` wires into the same connected
group, which is what makes the block one route (`find_routes` works by connectivity, unchanged).

- `respond(body, status: 200, missing: 404)` is the template; `status` and `missing` default as
  the catalog's defaults do; `respond(status: 204)` takes no body. Its hidden `request` input is
  wired to the route's request node by the lowering; writing `respond` outside a route is an
  error.
- A route needs one `respond`, or at least one effect (answered 204); more than one `respond` is
  an error naming the route, as `route_spec` says today.
- Two routes may not share a node. A top-level `let` would wire two routes into one group, so at
  the top level of a flow with routes only `let` of a literal is allowed, and it is lowered by
  placing a fresh `const` in each route that uses it. A top-level `let` of anything else is an
  error that says why: "routes are separate units; put shared logic in a holon".
- Clashing routes, bad methods and bad paths are the existing errors, now with a column.

### 5.11 Flows without routes

With no `route` in the file, the top-level statements are the flow: a clock flow (`interval`), or
a flow placed as a holon in another. They lower to the whole canvas grouped as one composite,
exactly `flow_unit`'s existing rule. The flow's outputs are its unused values, which is why a
non-Effect expression statement is allowed here and only here: it is an output. `respond` is an
error.

## 6. Semantics

The language has no semantics of its own: a program means what its DAG means under the one
evaluator. This section says what that is, in the language's terms, because it differs from a
conventional language in three places.

### 6.1 Dataflow, not control flow

A statement does not "run"; a node computes when its inputs are present. Statement order only
constrains names (defined before use) and the printer's layout. `respond` first and `store_write`
last, or the reverse, is the same unit.

### 6.2 Sharing is explicit

A `let` is one node and as many wires as uses. The same call written twice is two nodes, each
computed (and, for `clock()` and `secret(…)`, each sourced, with the same value within a run).
There is no common-subexpression elimination, so the canvas shows exactly what was written, and a
replay's node-by-node values match the text's expressions one to one.

### 6.3 Absence

A node whose wired input did not arrive produces nothing, and nothing flows on from it. This is
the evaluator's firing rule and it is how the language expresses "not found", "unauthorized" and
"not verified" without exceptions:

- `collection_get` of a missing key is absent; `?? 0` fills it; `respond` of an absent body answers
  the `missing` status.
- `x when c` is absent unless `c` is `true`.
- `stripe_verify(…).event` is absent when the signature fails, so nothing after it fires.

Absence is not a value: it cannot be stored, compared or passed to a lambda. `none()` is a value
(the optional's empty arm) and is unrelated.

### 6.4 `if` is eager and strict

`if c then a else b` is `select`. Both `a` and `b` are computed; `select` chooses which value
flows on. If either arm is absent, the result is absent, whatever `c` is, because `select` needs
all three inputs. `if` is therefore not a guard and not short-circuit. For "only when", use
`when`; for "this or that fallback", use `??`.

### 6.5 Totality and purity

Every program terminates: iteration is `map` and `fold` over a list, bounded by the list, and
there is no recursion (a holon cannot place itself, since its hash does not exist until it is
frozen). No construct performs an effect during evaluation; effects are values that the host
performs after the run settles. `http_request` is answered between rounds (§4.1 of
`PLATFORM_LAYOUT.md`); `await` parks the run. None of that is visible in the language beyond the
nodes' names.

### 6.6 Types

The nine wire types are core's: `Number`, `Boolean`, `Text`, `Record`, `List<T>`,
`Product<…>`, `Sum<…>`, `Effect`, `Any`. Type annotations are written as `Text`, `List<Text>`,
`Record`. Checking is the existing wire-time compatibility check at lowering: a `Text` into a
`Number` port is an error at the argument's span; `Any` is compatible with everything, in both
directions, as today. The language adds no inference beyond what the canvas does: a holon's port
takes its first wire's type.

## 7. Names, hashes and canonical form

- **The hash is the DAG's.** `let` names, holon names, lambda parameter names, comments, blank
  lines and statement order are not in the preimage. They are kept in the flow document's `names`
  map (holons) and the nodes' `extra` (spans), never in a unit.
- **Resolution happens at check time.** A catalog name resolves to `interface_of`; a template to
  its builder; a holon to the hash frozen for it in this file. Every hash the flow depends on is in
  the document's `units` closure, which is its lockfile.
- **Canonical text.** `holon fmt` prints the expression form from the document (§11), with a fixed
  layout: headers, holons (inner before outer, in first-use order), routes in declared order,
  within a block the topological order of the nodes with a `let` wherever a value is used more
  than once. `fmt(fmt(t)) = fmt(t)`, and two texts that lower to the same document print the same.

## 8. Layout

Positions are not part of a program. The expression form has no `at`. The flow document keeps
positions, as it does today, so a flow authored on the canvas keeps its layout through save and
open. When text is compiled, nodes are laid out automatically: columns by topological depth, rows
by statement order, holons and routes stacked. When a document is printed, positions are dropped.
The `at` clause of the wiring form (§10) stays accepted for compatibility, and `fmt` drops it.

## 9. Diagnostics and source maps

Every token carries `line:col`; every AST node a span; every lowered node keeps the span of the
expression that placed it, in the document node's `extra` as `source: {line, col, endLine,
endCol}`. From that:

- `holon check` prints `file:line:col: message`, with the offending expression on the next line
  and a caret under it.
- The editor highlights the node a check error names, and view mode, which already walks a run by
  node path, can show the text line beside each node's value.
- The agent tools' `explain` renders a run as the program with each expression's value, which is
  the observability surface the product direction asks for.

The error catalogue the lowering must produce, each with the span it points at:

| Error | Points at |
|---|---|
| unknown name `x` (not a leaf, template, holon or binding) | the name |
| `x` is a leaf; a binding may not shadow it | the binding |
| `f` takes `n` inputs, `m` given | the extra or missing argument |
| `f` has no param `p` / `p` given twice | the param |
| input `p` can't be left unwired | the `_` |
| `Text` doesn't fit `Number` at `f`'s `p` | the argument |
| a value nobody uses | the statement |
| `respond` outside a route / more than one `respond` | the call |
| `fn` may only use its own arguments | the free name |
| `x ?? e`: the fallback must be a literal | `e` |
| holon `h`: input `p` never used / output `o` never set / set twice | the port or the assignment |
| routes `A` and `B` share `x` | the shared binding |
| `A` clashes with `B` / not a method / not a route path | the route header |
| the wiring form's existing errors | their line, now with a column |

## 10. The wiring form stays

Today's statements remain valid and are the **wiring form**:

```
node greet = concat { } at 40 120
hi.out -> greet.a
holon slugify(text "lowercase · s") -> (slug "out") { … }
```

Both forms coexist in one file and share one namespace: a `node` name can be used in an
expression (`greet.out` is its output), and a `let` name can be a wire's end (`greet.out ->
res.body`). This matters for two reasons:

1. **Round-tripping arbitrary canvases.** A composite made on the canvas can have port names the
   expression form cannot spell (`lowercase · s`). The holon header's quoted unit name covers the
   boundary; the wiring form covers anything else. The printer uses the expression form wherever
   it can and falls back to a `node`/wire statement only where it cannot.
2. **Migration.** Every `.holon` file in the wild keeps working, and `fmt` upgrades it.

`fmt` prints the expression form. Nothing is removed from the parser.

## 11. Canvas to text: the printer

The printer turns a flow document into canonical text. The algorithm:

1. Holons first: every composite the document carries (the `units` closure), inner before outer,
   each printed once under its name from the `names` map (or a generated one), its inner canvas
   opened with `inner_canvas` exactly as the expand view opens it.
2. Routes, in the order `find_routes` finds them; then, for a flow without routes, the top-level
   statements.
3. Within a group, nodes in topological order. A node whose one output has exactly one consumer
   and is not an Effect prints inline, as a nested call, at its consumer. A node with several
   consumers, or an Effect, or a multi-output composite, gets a `let` (an Effect with no consumer
   prints as a bare statement). Consts print as literals; `record`/`with_field` chains print as
   record literals when the chain starts at `record()` or as `{ ..base, … }` when it does not;
   `list` prints as a list literal; `default` with a literal param prints as `??`; `guard` as
   `when`; `select` as `if`; binary leaves as operators; `field` chains as `.name`.
4. Names: a `let` takes the node's name from the document when it has one (the wiring form's
   names, or the editor's labels), else the leaf's name, uniquified.
5. Any node the rules above cannot express prints in the wiring form.

Properties, tested: `compile(print(doc))` carries the same unit hashes as `doc`;
`print(compile(t))` is the canonical form; `print` is idempotent through `compile`.

## 12. Build order

Each step is one inch: its tests, a live check, a commit. The whole is about two weeks, and no
step changes core, the host, the server or the document format.

1. **Lexer and parser with spans** (`holon-author/src/lang/{lex,ast,parse}.rs`). The existing
   `text.rs` statements parse into the same AST as the wiring form. Tests: every construct, every
   error with its column, the eleven samples as text.
2. **Lowering onto `FlowBuilder`** (`lang/lower.rs`): resolution, calls, literals, operators,
   `.name`, holons via `end_holon_exact`, lambdas as anonymous holons, routes with the implicit
   request wires, effects, the error catalogue of §9, spans into `extra.source`. Golden test:
   each sample written in the language lowers to the same unit hash the sample's builder gives,
   through `flow_unit`; where a sample's holon was grouped by proposal, the text carries the
   proposed port names in quotes (§10), since the unit's port names are part of its hash.
3. **Printer** (`lang/print.rs`), replacing the wiring-form printer as `fmt`'s output. Tests:
   the round-trip properties of §11 over the samples and over canvases grouped by hand (odd port
   names, variadic nodes, multi-output composites).
4. **`holon` and the editor.** `check` prints `line:col` and the caret; the editor's export and
   import use the new printer and parser (through `holon-wasm`); view mode shows the line of the
   selected node. Samples page: "as text" shows the expression form.
5. **Samples as text.** Each sample becomes a `.holon` file in `crates/holon-author/samples/`,
   compiled at test time and embedded; the Rust builders go. The rule in `CLAUDE.md` becomes
   "every feature ships with a sample written in the language", and the sample is the
   documentation of the feature. (Only after step 2's golden test has held for a release.)
6. **Agent surface.** The MCP tools take and return this form. Not part of this document; it is
   why the order above puts the parser before the printer.

Dependencies: 2 needs 1; 3 needs 2 (the golden test is what makes the printer checkable); 4 and 5
need 3; 6 needs 4.

## 13. Open forks

Decided provisionally above; listed here so they are revisited on evidence, not forgotten.

- **F1. Bare words as param values.** `collection: notes` without quotes is convenient and
  matches the wiring form, but a value like `GET` and a name like `paid` read the same as
  identifiers. Alternative: quotes always. Decide after agents have written a hundred flows:
  count the mistakes.
- **F2. Positional params for the one-param leaves.** `secret("api_token")`, `store_read("beats")`
  read better than `secret(name: api_token)`. It breaks the "positional is an input" rule for
  zero-input leaves only. Tempting; refused for now because one rule is worth more than two
  characters, and because `default` shows the rule can't be bent in general.
- **F3. String interpolation.** Decided (2026-10-24): `"Hello, {name}"` lowers to one `concat`
  over its pieces, each `{…}` part wrapped in `as_text`, the leaf added for this: the identity
  on text, a number or boolean as written, a record or list as JSON. So a part may be any
  value. `++` is dropped; `+` stays numeric (an operator must lower to one node, and most
  values are `Any`, so `+` could not choose between `add` and `concat`). `concat(a, b)` remains
  the spelled form. The syntax sections are to be rewritten to this.
- **F4. Top-level `let` beyond literals.** The restriction exists because routes must be separate
  groups. An alternative is duplicating the whole subexpression per route, which keeps routes
  separate at the cost of hidden node copies. Refused for now: the error message points at
  holons, which is the honest fix.
- **F5. `if` as a guard pair.** Some will expect `if c then a` without `else` to mean `a when c`.
  Allowed as sugar? It would make `if` mean two different nodes. Refused; the error suggests
  `when`.
- **F6. Named inputs.** `collection_list(after: x, collection: notes)` would let an author skip
  optional inputs without `_`. It needs a rule for the name clash (`default`'s `value`). Deferred
  until `_` proves annoying.
- **F7. Where the layout of a text-authored flow lives.** Auto-layout on import is enough for
  review; an author who then arranges the canvas and exports loses the arrangement. A sidecar
  `.holon.layout` is the obvious fix if anyone asks.
- **F8. Lambdas with captures, by lifting.** A capture could be lifted to an extra input of a
  composite that `map` cannot call today. It would need a `map` that takes extra fixed inputs,
  which is a core change. Not now.

## 14. What this document does not decide

The MCP server and its tools, the export and verifier for run logs, the sandboxed WebAssembly
primitive, and the marketplace's `use`. Each depends on this language existing and none changes
it.
