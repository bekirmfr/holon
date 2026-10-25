# The Holon language: flows as programs

**Status:** design, 2026-10-24 (syntax revised the same day for readability). Nothing here is built
yet; §12 is the build order.
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

Total is the language's boundary, stated here so nobody finds it by surprise: every run ends.
There is no recursion and no loop; `map` and `fold` go over a list, and `range(n)` counts to at
most 10000. Anything with nested or unbounded structure (a tree of any depth, a search that
runs until it finds something) is out of reach, by design.

The guiding rule for the syntax: **the world the flow touches is written as nouns, what the flow
does as verbs, and a line should read aloud as a sentence.** Every construct is tested by reading
it to someone who has never seen Holon.

```
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond "Hello, {req.params.name}"
}
```

```
flow heartbeat
every 10s

state.last_beat = now
state.beats = (state.beats ?? 0) + 1
```

```
route POST /stripe (req) {
  event = stripe.verify(req, secret.stripe_webhook_secret, now, tolerance: 5m).event
  sent  = http.post("hooks.slack.com", secret.slack_webhook_path,
            headers: { "content-type": "application/json" },
            body: json({ text: "Stripe: {event.type}" }))
  respond sent.status, missing: 400
}
```

A nested call is a wire. A binding is a named wire. A holon is a function. A route is one
connected group with a `request` node. `state.x`, `secret.x`, `now`, `notes[id]` and `http.post`
are each exactly one catalog leaf, spelled as a noun. The hash of a program is the hash of the DAG
it lowers to, so names and formatting are cosmetic, and two differently written programs with the
same graph are the same unit.

**The decisions, in one table.** Each is argued in the section named.

| Decision | Choice | Where |
|---|---|---|
| What the language is | A surface over the existing IR; every construct lowers to catalog leaves, templates and composites that exist today (two leaves, `as_text` and `coalesce`, were added for it) | §1, §6 |
| What it is not | General-purpose. No loops, recursion, user types, modules, or standard library growth | §2 |
| The outside world | Nouns: `req`, `secret.name`, `state.key`, `now`, `random`, a declared `collection` used as `notes[key]`, a declared `folder` used as `inbox[name]`, `http.post(host, path, …)`, `await name` | §5.4 |
| Writes | Assignment: `state.key = v`, `notes[key] = v`, `delete notes[key]`, `reports[name] = text`. Method forms (`notes.put(k, v)`) when the effect needs a name | §5.4, §5.10 |
| Bindings | One form, `name = expr`, everywhere | §5.2 |
| Calls | `name(inputs…, named…)`: positional arguments are inputs; a named one is an input when the unit has that port, else a param | §5.3 |
| Literals | `1`, `"text"`, `true`, `10s`; `"Hello, {name}"` interpolates through `as_text`; `{…}` and `[…]` lower to `record`/`with_field` and `list` | §5.5 |
| Operators | Arithmetic and comparison on numbers, logic, `??` (fallback), `.` (field or output), `[ ]` (lookup), `when` (guard), `if … then … else …` (guard plus fallback), `match x { … }` (a choice by value) | §5.6, §6.5 |
| Multi-output values | `.name` selects an output when the name is one; otherwise it is a field. A bare multi-output value is its first output | §5.7 |
| Functions for `map`/`fold` | A holon by name, or a lambda `fn(a, b) -> …`; a name it uses from around it becomes an input of its own | §5.9 |
| js holons | `holon name(inputs) -> outputs js [fuel N] [memory N] { JavaScript }`: a holon whose body is JavaScript, run by an engine that is a WebAssembly module with no imports, under a budget | §5.8a |
| Effects | An unused Effect is performed, an unused value is an error; no `perform` keyword | §5.10 |
| Routes | `route METHOD /path (req) [resumes name] { … }`; `respond` is a statement wired to the request implicitly | §5.11 |
| Flows without routes | The top-level statements are the one unit | §5.12 |
| Sharing | A binding used twice is one node with two wires; the same expression written twice is two nodes. No common-subexpression elimination | §6.2 |
| Hash | The lowered DAG's. Binding names, holon names, layout and comments are not in it | §7 |
| Layout | Not in the text. Positions live in the flow document; import lays out automatically | §8 |
| Diagnostics | `file:line:col: message`; every node keeps its span, so `check`, replay and the agent tools point at the expression | §9 |
| The netlist form | Stays valid, as the wiring form, in the same file; `fmt` prints the expression form | §10 |

---

## 1. Why a language, and why now

Three reasons, in order of weight.

1. **Agents write the flows.** The product direction (agent actions you can prove) makes an agent
   the main author. An agent is far better at `join(split(lowercase(text), " "), "-")` than at
   five `node` lines and four wires: fewer tokens, fewer places to misname a port, and errors it
   can read. The MCP tools (`check`, `deploy`, `call`, `explain`, step 7) speak this form.
2. **Expressiveness without new primitives.** Most of what today takes five nodes is one nested
   expression. The vocabulary stays at its leaves (71, with `range`); the text gets shorter.
3. **Review, diff and version control** become ordinary. The derived manifest stays the thing a
   human approves; the text is what they read when the manifest surprises them.

And why it costs little: `holon-author` already holds the netlist parser, the `FlowBuilder` that
every sample is built with, the composite freezer, and the route derivation. The language is a
parser and a lowering onto that builder, plus a printer for the other direction.

## 2. Principles and refusals

**P1. The IR is the program.** A `.holon` text is a way to write a flow document; the flow
document is a way to write units; the units are what runs, hashes, deploys and replays. The
language adds no construct that does not lower to units that exist. If a construct needs a new
primitive, that primitive goes through the admission gate (design §13) on its own merits first,
and the language gets the sugar after. Two did, for this design, both built on 2026-10-24:
`as_text` (§5.5) and `coalesce` (§5.6).

**P2. Small, and deliberately not general.** The pull of every language is toward a type system
with generics, modules, a standard library, loops. Each of those would cost a year and would break
the thesis: the manifest is derivable and replay is exact *because* the vocabulary is closed and
every program is total. This language is closer to Dhall or Nix with effects at the edges than to
Python. When expressiveness genuinely runs out, the answer is a sandboxed deterministic
WebAssembly primitive (a module with no imports is pure, and is content-addressed by its bytes),
never a language feature.

**P3. Names are cosmetic, the graph is identity.** A binding name, a holon name, a comment, the
order of independent statements: none of it reaches the hash. Two programs that lower to the same
DAG are the same unit, and `fmt` prints them identically.

**P4. The canvas is a view of the same program.** Text to canvas is compilation; canvas to text
is printing; both go through the IR, and both directions preserve every unit hash. The canvas
stays what it is best at: showing a run, node by node.

**P5. One implementation.** Lexer, parser, lowering and printer live in `holon-author`, pure like
core, and run natively in `holon`, in the server, and in the editor through `holon-wasm`. No
TypeScript copy of any of it.

**P6. Nouns are sugar, never magic.** `state.beats` reads as a variable and *is* one
`store_read` node; `notes[id] = v` reads as a mutation and *is* one `collection_put` node whose
effect the host performs when the run finishes. The sugar table in §5.4 is exhaustive: a reader
who knows it knows exactly which leaf every noun is, and the manifest is still derived from the
leaves, never from the words.

**Refused, with the reason:**

| Refused | Why |
|---|---|
| `while`, recursion, unbounded iteration | Totality. `map` and `fold` over a list are the only iteration, bounded by the list; `range(n)` makes a list to count over, of at most 10000 |
| User-defined types, generics | The checker is wire-time compatibility over nine types. A type system is a different project |
| Modules, imports, packages | Reuse across files is the marketplace's job (a published holon placed by hash), not a `use` statement (the `marketplace` sample). Revisit when people share holons across flows |
| `+` on texts | An operator lowers to one node, and most values are `Any`, so `+` could not choose between `add` and `concat`. Interpolation is the natural spelling; `concat(a, b)` the explicit one |
| Common-subexpression elimination | The text's structure is the graph's. Sharing is explicit (a binding), so the canvas shows what was written |
| Null | Absence is not a value. `none()` is the optional's empty arm; "nothing" is the evaluator's absent slot (§6.3) |
| Sequential mutation | `state.x = 1` then `state.x = 2` is an error (a key written twice), because a run reads its starting state and writes once at the end. The syntax looks like assignment; the semantics are a single commit |

## 3. A tour, by the samples

Every sample, written in the language. `req` is the name the route gives its request; bare, it is
the whole request record. The samples' own files are `crates/holon-author/samples/*.holon`, kept
exactly as `holon fmt` writes them (step 6 of §12); the texts below are the same flows as a reader
would first write them, with a default here and there that `fmt` leaves out (`limit: 100`,
`encoding: hex`, `tolerance: 5m`).

### hello-api

```
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond "Hello, {req.params.name}"
}
```

Lowers to: `request{method: GET, path: /hello/:name}`, `field{name: name}` on its `params`
output, `as_text` of that, `const{type: Text, value: "Hello, "}`, `concat(a, b)`, and
`respond{status: 200, missing: 404}` with `request` and `body` wired. Six nodes, six wires.

### notes-api: a collection

```
flow notes-api
port 8102
collection notes

route GET /notes (req) {
  respond notes.list(after: req.query.after ?? "", limit: 50)
}

route PUT /notes/:id (req) {
  notes[req.params.id] = req.json
  respond req.json, missing: 400
}

route GET /notes/:id (req) {
  respond notes[req.params.id]
}

route DELETE /notes/:id (req) {
  delete notes[req.params.id]
  respond 204
}
```

`collection notes` names the collection; it grants nothing, the manifest is still derived from
the leaves. `notes[k]` is `collection_get{collection: notes}`, `notes[k] = v` is `collection_put`,
`delete notes[k]` is `collection_delete`, `notes.list(…)` is `collection_list` with its optional
inputs named and its `limit` param. `?? ""` is a `default{type: Text, value: ""}` node. The write
in the PUT route is an Effect nobody reads, so it is exposed and the host performs it when the
run finishes (§5.10). `respond 204` has no body. `respond page, headers: { "content-type":
"text/html; charset=utf-8" }` sets response headers (a record, text values): a page instead of
JSON, or `headers: { location: "/" }, status: 303` for a redirect; without it, a text body goes
out as plain text and anything else as JSON (the `guestbook` sample).

### stripe-to-slack

```
flow stripe-to-slack
port 8103

route POST /stripe (req) {
  event = stripe.verify(req, secret.stripe_webhook_secret, now, tolerance: 5m).event
  sent  = http.post("hooks.slack.com", secret.slack_webhook_path,
            headers: { "content-type": "application/json" },
            body: json({ text: "Stripe: {event.type}" }))
  respond sent.status, missing: 400
}
```

`stripe.verify` is the `stripe_verify` template; it has two outputs, so `.event` selects one
(§5.7). `secret.x` is a `secret{name: x}` node; `now` a `clock`; `5m` is 300. `http.post` is
`http_request{method: POST, host: "hooks.slack.com"}` with the path positional and the optional
inputs named; the host is a literal because it is the permission. `json(x)` is `json_stringify`.
When the signature fails, `event` is absent, nothing downstream fires, `sent.status` is absent,
and `respond` answers its `missing` status: 400, and nothing was posted.

### guarded-api

```
flow guarded-api
port 8109
optional secret.receipt_key

route GET /me (req) {
  ok      = secure_equals(req.headers.authorization, "Bearer {secret.api_token}")
  receipt = hmac_sha256(secret.receipt_key, req.path, encoding: hex) ?? "unsigned"
  respond { receipt: receipt } when ok, missing: 401
}
```

`optional secret.receipt_key` is the deploy setting `optional SecretRead("receipt_key")`.
`req.path`: `path` is not an output of `request`, so it is a field of the request record. `when`
is `guard(cond, value)`: when `ok` is false the body is absent and the route answers 401.

### slugs: holons

```
flow slugs
port 8110
collection posts
optional secret.receipt_key

holon slugify(text) -> slug {
  slug = join(split(lowercase(text), " "), "-")
}

holon publish(slug, author) -> (put, saved, receipt) {
  s       = slug ?? ""
  put     = posts.put(s, { author: author, at: now })
  saved   = state.set(last_post, s)
  receipt = hmac_sha256(secret.receipt_key, s, encoding: hex) ?? "unsigned"
}

route POST /slugs (req) {
  title     = slugify(req.json.title)
  author    = slugify(req.json.author)
  published = publish(title, author)
  respond { title: title, author: author, receipt: published.receipt }, missing: 400
}

route GET /slugs (req) {
  respond posts.list(limit: 100)
}
```

A holon is a composite with a given boundary, frozen exactly as `end_holon_exact` freezes it: the
declared port names are the unit's port names. Inside `publish`, the two writes are *named
outputs*, so they are spelled in the method form, `posts.put(k, v)` and `state.set(k, v)`, the
same leaves the assignment forms lower to. The route never mentions `put` or `saved`: they are
Effect outputs nobody reads, so they are exposed and performed. `slugify` is placed twice and is
one unit, one hash.

### heartbeat: a flow without routes

```
flow heartbeat
every 10s

state.last_beat = now
state.beats = (state.beats ?? 0) + 1
```

No routes, so the top-level statements are the one unit the clock fires (§5.12). `state.beats` on
the right is `store_read{key: beats}`, the run's starting value; the assignment is
`store_write{key: beats}`, committed when the run finishes. Which is why the line means exactly
what it says.

### approval: a durable run

```
flow approval
port 8105

route POST /orders (req) {
  respond "/paid/{resume_token(paid)}"
  paid = await paid, expires: 1d
  state.last_order = { ..req.json, paid_via: paid.method }
}

route GET /paid/:token (req) resumes paid {
  respond "thanks, the order is paid"
}
```

Statement order carries no meaning beyond names being defined before use: the response is
produced in the first round, the run parks at `await`, and the state write happens when the link
is opened. `{ ..base, k: v }` is `with_field` over an existing record (§5.5). `resumes paid` is
the request template's `resume` param.

### fetch-json, sign-payload, likes, witness

```
route GET /slideshow (req) {
  page = http.get("httpbin.org", "/json")
  respond parse_json(page.body).slideshow.title
}
```

```
route POST /sign (req) {
  respond { signature:   hmac_sha256(secret.signing_key, req.body, encoding: hex),
            body_base64: base64(req.body, mode: encode) }, missing: 400
}
```

```
flow likes
port 8108
collection likes

route POST /posts/:id/like (req) {
  id = req.params.id
  n  = (likes[id] ?? 0) + 1
  likes[id] = n
  respond n, missing: 400
}

route GET /posts/:id/likes (req) {
  respond "{likes[req.params.id] ?? 0} likes"
}

route GET /likes (req) {
  respond likes.list(limit: 100)
}
```

```
flow witness
port 8130
collection batches

route POST /heads (req) {
  ok    = secure_equals(req.headers.authorization, "Bearer {secret.witness_key}")
  batch = req.json.batch
  batches[batch] = { ..req.json, witnessed_at: now } when ok
  respond { batch: batch, witnessed_at: now } when ok, missing: 401
}

route GET /batches (req) {
  respond batches.list(after: req.query.after ?? "", limit: 200)
}
```

`now` written twice is two `clock` nodes (§6.2); both read the same instant, the run's start. To
share one node, bind it: `t = now`.

### inbox-report: folders on the host

```
flow inbox-report
port 8123
folder inbox
folder reports

route POST /report (req) {
  name = req.json.name
  summary = summarize(name, inbox[name])
  reports["{name}.md"] = summary.report
  respond { rows: summary.rows, report: "{name}.md" }
}

route GET /reports (req) {
  respond reports.list()
}
```

`folder inbox` names a folder on the host; an admin binds the name to a real folder
(`holon folder add inbox /srv/drop --read-only`, `--for <slug>` for one deployment only, or the
editor's admin page), and a flow never sees the path. A folder reads and writes as a collection does, by file name: `inbox[name]` is
`file_read{folder: inbox}`, the file's text as the run began (no value if there is none);
`reports[name] = text` is `file_write`, the whole file written when the run finishes;
`delete reports[name]` is `file_delete`; `reports.list(…)` is `file_list`, a record of file name
to `{size, hash, modified}`. A name is a path inside the folder (`2026/10/a.csv`) and never leaves
it. Reading asks `FileRead("inbox")`, writing and deleting `FileWrite("reports")`, and a deploy is
refused until each folder it needs is bound, and bound read-write where it writes. The run log
keeps what a run read (or only its hash, for a folder bound with `--keep hash`; for a while, then
its hash, with `--keep 7d`) and what it wrote by its hash; a folder bound with `--cap 10M` takes
no write past that (the `journal` sample). Design: `docs/L4s/named-folders-L4.md`.

### log-archive: big files, by handle and by range

```
route GET /logs/:name/tail (req) {
  respond logs.lines(req.params.name, from: -20, limit: 20)
}

route POST /archive (req) {
  log = logs.file(req.json.name)
  archive["{log.name}.{log.hash}"] = log
  respond { archived: "{log.name}.{log.hash}", size: log.size }
}
```

A file of any size never enters a run whole. `logs.file(name)` is a handle,
`{folder, name, size, hash, modified}`, without the bytes; writing a handle to a folder
(`archive[n] = log`, or `archive.copy(n, log)`) copies the file host to host when the run finishes,
and fails if it no longer has the handle's hash. `logs.lines(name, from: i, limit: n)` reads `n`
lines from line `i` (from the end when negative), and `logs.slice(name, offset: o, length: n)`
`n` bytes from `o`: only those enter the run, and the log keeps with them the hashes that prove them
part of the file, so `holon verify` checks the run without it.

### drop-box: a run per new file

```
flow drop-box
on file drop
folder receipts

sent = http.put("httpbin.org", "/put", headers: { "content-type": "application/octet-stream" }, file: ingress)
receipts["{ingress.name}.json"] = json({ file: ingress.name, size: ingress.size, hash: ingress.hash, status: sent.status, received: parse_json(sent.body).headers["Content-Length"] ?? "nothing" })
```

`on file drop` makes the flow run once for each new file in the folder `drop` (which it declares,
as `folder drop` would), once the file stops changing: the server looks at the folder every second,
by each file's size and time, and a file the same at two looks in a row starts a run. In that run
`ingress` is the file's handle, `{folder, name, size, hash, modified}`: the `file_arrival{folder:
drop}` leaf, and the deployment asks `FileArrival("drop")`, its trigger. Files already there when
it is first deployed aren't new; files that arrive while the server is down, or the deployment is
paused, start their runs when it is back; one its own runs wrote into the folder doesn't start it
again (unless it changes).

`file: h` in place of `body:` sends a file as the request's body, by handle: `http_upload{method,
host}(path, headers, h)`. The server streams the bytes from the folder, never through the flow,
checks before and as it sends that they still hash to the handle's hash (else the call answers
`{status: 0, error: "changed"}`), and the run log keeps the request with the handle, never the
bytes. The handle's folder must be one the deployment reads, or the one whose new file started the
run.

### file-audit: several folders, removals, some names

```
flow file-audit
on file inbox matching "*.csv"
on file inbox deleted
on file outbox
folder audit

f = ingress
audit.create("{f.folder}/{f.modified}-{f.event}-{f.name}.json", json(f))
```

Each `on file` line is a trigger: a CSV new or changed in `inbox`, any file removed from it, any
file new or changed in `outbox`. A file no line is for (a `.txt` dropped in the inbox) is seen, not
run on. `ingress` says which: its `event` is `new`, `changed` or `deleted`, and a removed file's
handle is as it was last seen, `{folder, name, size, modified, event}`, without a hash (its bytes
are gone). A file its own runs deleted from a folder it watches starts nothing. `audit.create(n,
e)` writes a file that must not exist yet (`file_create`): the run fails, and nothing it did
lands, rather than replace one; `audit.create_copy(n, h)` is the same for a copy by handle.

### map and fold

From the `quote` sample: each line's function uses `tax` from the route around it.

```
route POST /quote (req) {
  tax = req.json.tax ?? 0
  lines = map(req.json.items, fn: fn(item) -> { name: item.name, total: item.price * item.qty * (1 + tax) })
  total = fold(0, lines, fn: fn(acc, line) -> acc + line.total)
  respond { lines: lines, total: total, tax: tax }
}
```

A `fn:` param takes a holon's name or a lambda. Either lowers to a composite whose hash becomes
the frozen `fn` param, which is what `eval_map` and `eval_fold` resolve; what a lambda uses from
around it reaches it as an input after the item (§5.9).

## 4. Lexical structure

- **Encoding:** UTF-8. Lines end in `\n` or `\r\n`.
- **Comments:** `#` to end of line. `fmt` keeps a comment on the line it was written on, moving it
  with that statement.
- **Keywords:** `flow` `port` `every` `browsers` `optional` `collection` `folder` `holon` `js` (after a
  holon's outputs) `route`
  `resumes` `respond` `delete` `await` `if` `then` `else` `when` `match` `fn` `true` `false`, and the
  wiring form's `node` `at` `in` `out` `interval`. Keywords are valid after a dot (`req.in`) and
  as record keys (`{ in: 1 }`).
- **Nouns:** `req` (the name a route gives its request; any identifier works), `secret`, `state`,
  `now`, `random`, `ingress`, `http`, `stripe`, and every declared collection and folder. `secret`, `state`,
  `http` and `stripe` are namespaces: they take a dot. The nouns are reserved like keywords.
- **Identifiers:** `[A-Za-z_][A-Za-z0-9_]*`. A name that is a catalog leaf, a template, a keyword
  or a noun cannot be bound; the error names the clash. Route paths and consent names have their
  own rules below.
- **Numbers:** a finite decimal: `0`, `86400`, `-1`, `2.5`, `1e3`. No hex, no infinity, no NaN,
  since `const` refuses them. Lowered as written, trimmed.
- **Durations:** a number with a unit, `10s`, `5m`, `2h`, `1d`, lowered to the number of seconds
  (`5m` is `300`). A duration is a number; it may be used wherever a number may.
- **Text:** `"…"` with JSON escapes, and `{expr}` interpolation (§5.5). `{{` and `}}` are literal
  braces.
- **Bare words as param values:** after `param:`, a bare identifier or a route-path-like token is
  the text itself: `collection: notes`, `encoding: hex`, `host: "hooks.slack.com"` (a dot needs
  quotes). This keeps the common case unquoted, as the wiring form does.
- **Punctuation:** `( ) { } [ ] , : = . -> ..` and the operators of §5.6.
- **Route paths:** `/` followed by segments of `[A-Za-z0-9_.-]+` or `:name`, as `parse_path`
  accepts. Read as one token after `route METHOD`.
- **Consent names** (after `optional`): `secret.name` for a secret, or the server's own form,
  `Name` or `Name("text")` exactly as `consent_name` prints it (`HttpRequest("api.example.com")`).

## 5. Syntax and lowering

Grammar in EBNF; `*` zero or more, `?` optional, `|` choice. Newlines separate statements; a
statement may continue on the next line while parentheses, braces or brackets are open, or after
a binary operator or a comma. Semicolons are not used.

### 5.1 File and headers

```
file      = header* item*
header    = "flow" ident | "port" number | "every" duration
          | "browsers" origin ("," origin)* | "optional" consent | "collection" ident
          | "folder" ident | "on" "file" ident ["deleted"] ["matching" string]
item      = holon | route | statement | wiring        # wiring: the netlist form, §10
```

Headers are the flow's deploy settings, exactly as today (`Settings`): `every 10s` is `interval`
10. `collection notes` declares a name for the body (§5.4); several may be declared, one per line.
`folder inbox` declares a folder the same way. `on file inbox` declares it too, and makes `ingress`
the handle of the file that starts each run (`file_arrival{folder: inbox}`); only in a flow
without routes. `deleted` makes it for files removed from the folder (`on: deleted`), and
`matching "<glob>"` for the names that match (`*` and `?` within a folder, `**/` any number of
folders, letters either case). A flow may have several, each a different line: `ingress` is then
the `coalesce` of their `file_arrival`s in the order written, whichever started the run.
Statements at the top level are the body of a flow without routes (§5.12); with routes, only
bindings of literals are allowed at the top level (§5.11).

### 5.2 Definitions and statements

```
holon     = "holon" ident "(" ports? ")" "->" outputs (block | "js" budget* jsbody)
budget    = ("fuel" | "memory") number              # a js holon's budgets, §5.8a
jsbody    = "{" <JavaScript source, balanced> "}"
ports     = port ("," port)*
port      = ident (":" type)? (string)?            # the string: the unit's own port name, §10
outputs   = port | "(" ports ")"
block     = "{" statement* "}"
route     = "route" method path "(" ident ")" ("resumes" ident)? block
method    = "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
statement = ident "=" expr                          # a binding; in a holon, also an output
          | "state" "." ident "=" expr              # a state write
          | ident "[" expr "]" "=" expr             # a collection or folder write
          | "delete" ident "[" expr "]"             # a collection or folder delete
          | "respond" respond_args                  # in a route
          | expr                                    # an effect, §5.10
```

One binding form: `name = expr`. In a holon, a declared output is bound the same way; an input is
a name in scope. A name is bound once per block and is visible after its binding.

### 5.3 Calls: inputs are positional, names fill in the rest

```
call      = ident "(" args? ")"
args      = arg ("," arg)*
arg       = expr | ident ":" (expr | value)
value     = number | duration | string | bareword | ident     # a param's frozen text
```

Positional arguments are **inputs**, matched to the unit's input ports in catalog order; each
becomes a wire. A named argument is an **input** when the unit has an input port of that name
(`http.post(host, path, headers: h, body: b)`, `notes.list(after: a)`), otherwise a **param**,
frozen into the node (`encoding: hex`, `limit: 50`). Positional arguments come first.

- The one unit with an input and a param of the same name is `default`, and it is never written:
  `??` is its spelling.
- Too many positional arguments is an error, except on a variadic leaf (`add`, `multiply`, `and`,
  `or`, `concat`, `list`), where each extra argument grows the node by one input, named as the
  canvas names them (`c`, `d`, … then `in<n>`), through `FlowBuilder::arity`.
- An optional input (`http_request`'s `headers` and `body`, `collection_list`'s `prefix` and
  `after`, as `is_optional_input` and core's `has_optional_ports` say) may be left out. Any other
  input left out is an error naming the port.
- An unknown name, or a name given twice, is an error naming the unit. Params not given take the
  catalog's defaults, as a dropped node does.
- A zero-input leaf written as a call still takes parentheses: `record()`, `none()`. The nouns
  (§5.4) are the exceptions, and are the reason the exceptions exist.

Every catalog leaf and template is callable by its own name with this rule (`concat(a, b)`,
`field(r, name: id)`, `clock()`, `collection_get(k, collection: notes)`); the nouns and
statements below are sugar over those calls, and `fmt` prints the sugar.

### 5.4 The outside world, as nouns

Each form is exactly one leaf. This table is exhaustive; nothing else is sugared.

| Written | Lowers to | Reads as |
|---|---|---|
| `req` | the route's `request` template node, its `request` output | the request |
| `req.params`, `req.query`, `req.headers`, `req.body`, `req.json` | that output of the request node | its parts |
| `secret.api_token` | `secret{name: api_token}` | a secret |
| `state.beats` (in an expression) | `store_read{key: beats}` | the state, as the run began |
| `state.beats = e` | `store_write{key: beats}(e)` | set the state when the run finishes |
| `state.set(beats, e)` | the same `store_write`, as an expression | the write, as a value to name |
| `now` | `clock()` | the time the run started |
| `random` | `random()` | one random number for the run |
| `ingress` | `ingress()` | what arrived at the deployment |
| `ingress` (with `on file inbox`) | `file_arrival{folder: inbox}()` | the handle of the file that started the run, with its `event` |
| `ingress` (with several `on file` lines) | `coalesce` of their `file_arrival`s, in order (`on: deleted`, `matching: g` as written) | the same, whichever line started it |
| `notes[k]` (`notes` a declared collection) | `collection_get{collection: notes}(k)` | the item under `k` |
| `notes[k] = e` | `collection_put{collection: notes}(k, e)` | put it when the run finishes |
| `delete notes[k]` | `collection_delete{collection: notes}(k)` | delete it when the run finishes |
| `notes.get(k)`, `notes.put(k, e)`, `notes.delete(k)` | the same three leaves, as expressions | the operation, as a value to name |
| `notes.list(prefix: p, after: a, limit: n)` | `collection_list{collection: notes, limit: n}(p, a)` | a page of items, in key order |
| `inbox[n]` (`inbox` a declared folder) | `file_read{folder: inbox}(n)` | the file's text, as the run began |
| `reports[n] = e` | `file_write{folder: reports}(n, e)` | write the whole file when the run finishes |
| `delete reports[n]` | `file_delete{folder: reports}(n)` | delete it when the run finishes |
| `inbox.get(n)`, `reports.put(n, e)`, `reports.delete(n)` | the same three leaves, as expressions | the operation, as a value to name |
| `inbox.list(prefix: p, after: a, limit: n)` | `file_list{folder: inbox, limit: n}(p, a)` | a page of files, `{size, hash, modified}` by name, in name order |
| `inbox.file(n)` | `file_handle{folder: inbox}(n)` | a handle: `{folder, name, size, hash, modified}`, no bytes |
| `inbox.lines(n, from: i, limit: k)` | `file_lines{folder: inbox, limit: k}(n, i)` | `k` lines from line `i` (from the end when negative), without line endings |
| `inbox.slice(n, offset: o, length: k)` | `file_slice{folder: inbox}(n, o, k)` | `k` bytes from `o` (from the end when negative), as text |
| `reports[n] = h` (`h` a handle read here, or `ingress` with `on file`), `reports.copy(n, h)` | `file_copy{folder: reports}(n, h)` | copy the file when the run finishes, host to host |
| `reports.create(n, e)`, `reports.create_copy(n, h)` | `file_create{folder: reports}(n, e)`, `file_create_copy{folder: reports}(n, h)` | the same write or copy, only if there is no such file yet: else the run fails |
| `http.get(host, path, headers: h, body: b)` and `post`, `put`, `patch`, `delete`, `head` | `http_request{method, host}(path, h, b)` | a call to that host |
| `http.put(host, path, headers: h, file: f)` (`f` a handle), any method | `http_upload{method, host}(path, h, f)` | a call sending the file's bytes, streamed by the server |
| `stripe.verify(req, secret, now, tolerance: d)` | the `stripe_verify` template | a verified Stripe event |
| `await paid, expires: 1d` | `await{name: paid, expires: 86400}` | the event, when it arrives |
| `resume_token(paid)` | `resume_token{await: paid}` | the token that delivers it |
| `json(x)`, `parse_json(x)` | `json_stringify(x)`, `json_parse(x)` | JSON out, JSON in |

Rules the table implies:

- A collection must be declared in the header before `notes[k]` means a collection; an undeclared
  name followed by `[ ]` is a `lookup` on a record (§5.6). The declaration is a *name*, not a
  permission: `check` still derives `CollectionRead("notes")` from the `collection_get` leaves.
  A folder the same: `folder inbox`, and `FileRead("inbox")` from `file_read`. A file read whole
  is text (UTF-8, up to the deployment's `inline_file_bytes`, 1 MiB by default; a run reads 64 MiB
  in all, `max_file_read_bytes`, writes 1 GiB, `max_file_write_bytes`, and touches 1,000 files,
  `max_files_per_run`); a read of anything else fails the run, and it writes nothing. A
  bigger one is read by handle, lines or slice; a copy also asks `FileRead` of the folder its
  handle names.
- `http.*`'s host must be a text literal, because the host is the capability the deployment asks
  for; a host from a wire is an error that says so.
- `state.x` on the right of `=` and on the left are different leaves, read and write, and that is
  the whole semantics of state in Holon: read at the start, write at the end. The same key may be
  written once per flow (one `store_write` per key), or the error says which two lines collide.
- The assignment and `delete` forms are statements; their Effect is exposed and performed
  (§5.10). The method forms are the same leaves as expressions, for a holon that must name its
  effects as outputs, or for an effect whose value is passed on (`published.put`).

### 5.5 Literals and text

| Written | Lowers to |
|---|---|
| `1`, `-2.5`, `5m` | `const{type: Number, value: "<as written, seconds for a duration>"}` |
| `"text"` | `const{type: Text, value: …}` |
| `true`, `false` | `const{type: Boolean, value: …}` |
| `"Hello, {name}!"` | one `concat` over the pieces: `const` for each literal piece, `as_text(expr)` for each `{…}`; a string that is one `{expr}` with no literal text is `as_text(expr)` alone |
| `{ k: e, "k 2": e2 }` | `record()` then one `with_field{name: k}(record, e)` per field, in written order |
| `{ ..base, k: e }` | `with_field{name: k}` chained onto `base`'s value; fields in written order |
| `[ e1, e2, e3 ]` | `list(e1, e2, e3)`, variadic |
| `[ ]` | `const{type: Json, value: "[]"}`: a placed `list` has at least its two declared inputs, so the empty list is the one literal that is a `const` |
| `[ e ]` | `append(list: const "[]", item: e)`, for the same reason |

A part is any expression, a string of its own included: `"{slice(sha256("{now}"), 0, 6)}"` is
how the printer writes a value used once, and reads back as written.

Interpolation wraps every `{…}` part in `as_text`, the leaf built for it: a text stays itself, a
number or boolean is written as it reads, a record or list as JSON. So a part may be any value,
and the lowering never depends on knowing a part's type. The one `as_text` node per part is the
price of that rule; the alternative, skipping it when the part is statically `Text`, would make
the hash depend on type annotations elsewhere in the file.

Record keys are identifiers or strings; a key that is not a plain word must be quoted
(`"content-type"`). Record and list literals always lower structurally, never to a `const` of type
`Json`, so the lowering is one-to-one and the canvas shows the fields. A single-node JSON constant
is the right side of `??` (§5.6), or `const(value: "[]", type: Json)` spelled out.

### 5.6 Operators

Lowest to highest precedence; binary operators are left-associative.

| Operator | Lowers to | Note |
|---|---|---|
| `if c then a else b` | `coalesce(guard(cond: c, value: a), b)` | `a` if `c` held and `a` arrived, else `b` (§6.4) |
| `if c then a` | `guard(cond: c, value: a)` | the same as `a when c` |
| `a when c` | `guard(cond: c, value: a)` | absent unless `c` is `true` |
| `match s { p -> a, q, r -> b, else -> e }` | `guard(equals(s, p), a) ?? guard(or(equals(s, q), equals(s, r)), b) ?? guard(not(or(…)), e)` | an atom; the arm whose pattern equals `s`, and its absence is the match's (§6.5) |
| `a ?? b` | `coalesce(a, b)`; with a literal `b`, `default{type, value}(a)` | `a` when present, else `b`. The two leaves have one meaning; `default` is the one-node form for a literal (a number, text, boolean, or a record/list literal with only literals inside, frozen as `Json`) |
| `a \|\| b` | `or(a, b)` | variadic: `a \|\| b \|\| c` is one node |
| `a && b` | `and(a, b)` | variadic |
| `a == b`, `a != b` | `equals(a, b)`, `not(equals(a, b))` | structural, any two values, never coerced |
| `a < b`, `<=`, `>`, `>=` | `compare{op}(a, b)` | numbers only, as `compare` is |
| `a + b`, `a - b` | `add(a, b)`, `subtract(a, b)` | numbers only; `add` variadic |
| `a * b`, `a / b` | `multiply(a, b)`, `divide(a, b)` | `multiply` variadic |
| `!a` | `not(a)` | |
| `a.name` | an output, or `field{name}(a)` | §5.7 |
| `a[e]` | `lookup(record: a, key: e)` when `a` is not a declared collection | the key arrives on a wire |

`coalesce(a, b)` is the one leaf this language needed beyond `as_text`: `a` when present, else
`b`, the first input that arrived. It cannot be a composite, because no other leaf fires with a
wired input absent (the firing rule hands a node nothing), which is exactly the gap `default`
plugged for literals. It went through the admission gate as genuinely inexpressible, pure and
total (built 2026-10-24, the 70th leaf). It needed less of the evaluator than this design first
assumed: the leaf path already gathers present inputs in port order, so "the first present" is
"the first that arrived"; `eval_at` gives it an explicit arm anyway, so the contract is stated
where absence is visible rather than inherited from an implementation detail. The heartbeat
sample uses it to keep the time of its first beat: `state.first_beat = state.first_beat ?? now`.

Variadic flattening applies only to a chain of the *same* operator: `a && b && c` is one `and`
with three inputs; `(a && b) && c`, with explicit parentheses, is two nodes. The parentheses are
the author saying "two". There is no text operator: interpolation is the spelling, `concat(a, b)`
the explicit call.

### 5.7 Multi-output values: `.name`

A call to a holon with several outputs, or to the `request` or `stripe_verify` template, produces
several values. The rules:

1. `v.name`, where `name` is one of `v`'s output names, is that output. No node is placed.
2. Otherwise `v.name` is `field{name}` applied to `v`'s value (its first output), one node.
3. A bare multi-output `v` in input position is its first output. For `request` that is the
   request record, which is what `stripe.verify`, `respond` and `field` want.

Output names win over field names. To read a field of the request record that shares a name with
an output (`body`), write `req.request.body`: output `request`, then the field. The printer uses
the same rule in reverse, so the text round-trips.

A holon with several outputs may also be destructured: `(put, saved, receipt) = publish(s, a)`.
Each name is a wire from that output.

### 5.8 Holons

```
holon publish(slug, author) -> (put, saved, receipt) { … }
```

Lowers through `FlowBuilder::begin_holon` … `end_holon_exact`: the block's nodes are the inner
canvas, each input name is a boundary input wired to every use, each `name = expr` binding a
declared output is a boundary output wired from its expression. The declared names are the unit's
port names, in declared order, so the composite freezes exactly as the expand view would freeze
it. An input never used, or an output never bound, is an error.

Types on ports are optional annotations: `holon slugify(text: Text) -> (slug: Text)`. When given
they are checked against what the wires carry; when absent, a port takes its first wire's type, as
the canvas does today. A holon takes no params (as now); a flow's holon is placed by name,
`slugify(x)`, and the unit's hash is the same wherever it is placed.

A holon that writes names its writes as outputs in the method form (`put = posts.put(k, v)`),
because a holon's effects leave it through its boundary and a boundary port has a name. An
assignment statement inside a holon (`posts[k] = v`) is allowed too: the lowering exposes its
Effect under a generated name (`put`, `put_2`, …), which the printer then shows. Declaring the
name is clearer and is what `fmt` prints.

Holons are defined at the top level and are visible to the whole file, in any order, so a holon
may use one defined below it. They may not be nested in text; a holon placed inside another is
the nesting.

### 5.8a js holons

```
holon score(estimates, tags) -> (total, weight) js fuel 50M {
  const total = estimates.reduce((a, b) => a + b, 0);
  return { total, weight: total > 10 ? "heavy" : "light" };
}
```

A holon whose outputs are followed by `js` has a body in JavaScript (`docs/L4s/js-node-L4.md`):
the one keyword that defines a node is `holon`, and `js` says the language its body is written in.
Its ports are declared as any holon's (names, optional types, one output or a parenthesised list),
and `fuel` and `memory` come after `js`. Defined at the top level, visible to the whole file,
placed by name like any holon (`score(a, b)`,
`score(a, b).weight`, `fn: score`), and lowered to a composite the printer reads back: a `const`
holding `(estimates, tags) => { body }` (assembled from the declared names, so ports and source
can't disagree), the arguments as one list, the engine module placed by hash under its budgets,
and a `field` per output. The engine is Boa compiled to a WebAssembly module with **no imports**
(`fixtures/modules/js-engine/`): the body can reach no clock, randomness, network or storage,
which is why a `js` node adds nothing to what a flow may do; what it adds is two limits, its
`fuel` (instructions per call, 200M when not written) and `memory` (16 MiB), which the deploy asks
for and the host's policy caps. `50M`, `2K`, `16MB` are numbers with a scale; `fmt` writes them
out.

The body is read as JavaScript, not as Holon: a `}` in a string, a template literal, a comment or
a regex literal doesn't end it, a `#` in it is JavaScript (a colour, a private field), not a
comment, and nothing in it is interpolated or renamed, so the source is part of the unit's
identity, verbatim (P3 stops at the brace). A port may be called after a leaf (`clock`), as a
holon's may; it may not be a keyword, the wiring form's `at` included, or a collection's name. A body sees only its arguments: `undefined` for an
absent value inside a list, every core value as its JavaScript counterpart (§5 of the L4). It
returns one value (one output) or an object keyed by the output names (several); `undefined` or
`null` is an absent output, an exception is an absent output whose reason the run records and
`explain` says under the statement ("closest gave nothing: TypeError: …"), and a `Map`, a `Date`,
a function or a Promise can't cross and is an error naming its kind. A syntax error is caught
earlier: `check` compiles each body once (nothing is run) and says the mistake at its line in the
file. Each call is a fresh realm: nothing survives between calls, so two equal calls are equal
and a run replays. A body that never ends runs out of fuel and is absent, with that reason.

Output names are port names, so a keyword can't be one (`match` is `best` in the `nearest`
sample). The sample `nearest` shows a js node doing the one computation the vocabulary can't
spell, an edit-distance search, with the flow around it storing, routing and answering.

### 5.9 Functions for `map` and `fold`

The `fn` param of `map` and `fold` takes:

- a holon's name: `map(titles, fn: slugify)`, lowered to that holon's hash;
- a lambda: `fn(acc, p) -> acc + p`, lowered to an anonymous composite with the parameters as
  inputs in order and the body's value as its one output, then to its hash.

`map` and `fold` pass their function the values given after the list, after the item:
`map(xs, a, b, fn: f)` calls `f(item, a, b)`, `fold(init, xs, a, fn: f)` calls
`f(acc, item, a)`. They are ports of their own (`in2`, …, typed `Any`), and the call fires only
when every one is present, as any node does.

A lambda may use a name from around it (a binding of the route or holon it is written in, an
input of that holon, or a parameter of a lambda it is written in): that is a capture. Each
captured name becomes an input of the function after its parameters, sorted by name, and the
call passes the bound value there. So `map(keys(r), fn: fn(k) -> r[k])` is the same unit as
`map(keys(r), r, fn: fn(k, r) -> r[k])`, the explicit form; in a nested lambda the inner
capture passes through the outer one. Constants are lowered inside, not captured. A name bound
nowhere around the lambda is "unknown", as anywhere else; a holon's body sees its own names
only, so a lambda inside a holon cannot reach a route's.

Arity is checked at the call, counting what the call gives: `map` gives one input plus one per
extra value, `fold` two plus one per extra. A function that takes another number is an error at
the function: "`map` gives its function 1 inputs (the item); this one takes 2".

The lambda's hash is its content's, so two identical lambdas are one unit, and the printer names
anonymous composites `fn_1`, `fn_2`… only when it has to print them as holons. A lambda whose
body is one expression prints back as a lambda; when the values passed after the list are the
names its trailing parameters take, in order, they print as captures, as they were written.
Parameter names are part of the unit (ports are named), so `fn(k, r)` and `fn(k, rec)` are two
units that compute the same.

A function reads from outside as anything else does: a collection read, an outbound request or a
waiting point inside it is answered by the host, once per item, recorded at the item's own path,
and replayed from there (the `basket` sample prices each line from a collection). View mode shows
a `map` or `fold` whose function reads from outside with what the run recorded it produced, as it
does a holon that reads.

### 5.10 Effects and statements

The IR has one rule for effects: an Effect-typed output that nothing consumes is exposed by the
group, and the host performs it when the run finishes (writes, puts, deletes, egress). The
language keeps that rule and adds no `perform` keyword:

- The write statements, `state.k = e`, `notes[k] = e` and `delete notes[k]`, are effects.
- An expression statement whose type is `Effect` is an effect: `egress(payload)`,
  `notes.put(k, v)`.
- An expression statement of any other type is an error: "a value nobody uses". Bind it or
  remove it. (A binding never used is a warning, printed by `check`.)
- A binding to an Effect and never used is still performed; naming it changes nothing.
- A multi-output holon's Effect outputs that the caller does not read are performed. The `slugs`
  route performs `publish`'s `put` and `saved` without mentioning them.

Effects are values, so `if c then state.set(k, a) else state.set(k, b)` places two writes and a
`select`-like pair, and only the chosen Effect reaches the boundary: one write is performed. The
idiom the samples use, and the one the printer prefers, guards the value instead:
`state.k = a when ok`.

### 5.11 Routes

```
route POST /orders (req) { … }
route GET /paid/:token (req) resumes paid { … }
```

Lowers to a `request{method, path, resume}` template node named by the parenthesised identifier,
and the block's nodes. Every `respond` and every use of `req` wires into the same connected group,
which is what makes the block one route (`find_routes` works by connectivity, unchanged).

```
respond_args = expr ("," "status" ":" number)? ("," "missing" ":" number)?
             | number                                # a status alone: no body (204)
             | "status" ":" number
```

- `respond body, status: 200, missing: 404` is the `respond` template; `status` and `missing`
  default as the catalog's defaults do. `respond 204`, a bare status with nothing after it, is
  `respond{status: 204}` with no body input; to answer a literal number as a body, write
  `respond body: 42`. The hidden `request` input is wired to the route's request node by the
  lowering; `respond` outside a route is an error.
- A route needs one `respond`, or at least one effect (answered 204); more than one `respond` is
  an error naming the route, as `route_spec` says today.
- Two routes may not share a node. A top-level binding would wire two routes into one group, so at
  the top level of a flow with routes only bindings of literals are allowed, lowered by placing a
  fresh `const` in each route that uses them. A top-level binding of anything else is an error
  that says why: "routes are separate units; put shared logic in a holon".
- Clashing routes, bad methods and bad paths are the existing errors, now with a column.
- A route is everything wired to its request, so a statement in the block that never touches the
  request (`state.hits = (state.hits ?? 0) + 1`, say) would not be part of the route and would be
  left out of the server. The lowering refuses it, pointing at the statement: use the request in
  it, or move it to a flow of its own.

### 5.12 Flows without routes

With no `route` in the file, the top-level statements are the flow: a clock flow (`every`), a flow
that runs on new files (`on file`), or a flow placed as a holon in another. They lower to the whole canvas grouped as one composite,
exactly `flow_unit`'s existing rule. The flow's outputs are its unused values, which is why a
non-Effect expression statement is allowed here and only here: it is an output. `respond` is an
error.

## 6. Semantics

The language has no semantics of its own: a program means what its DAG means under the one
evaluator. This section says what that is, in the language's terms, because it differs from a
conventional language in four places.

### 6.1 Dataflow, not control flow

A statement does not "run"; a node computes when its inputs are present. Statement order only
constrains names (defined before use) and the printer's layout. `respond` first and a state
write last, or the reverse, is the same unit.

### 6.2 Sharing is explicit

A binding is one node and as many wires as uses. The same call written twice is two nodes, each
computed (and, for `now` and `secret.x`, each sourced, with the same value within a run). There
is no common-subexpression elimination, so the canvas shows exactly what was written, and a
replay's node-by-node values match the text's expressions one to one.

### 6.3 Absence

A node whose wired input did not arrive produces nothing, and nothing flows on from it. This is
the evaluator's firing rule and it is how the language expresses "not found", "unauthorized" and
"not verified" without exceptions:

- `notes[k]` of a missing key is absent; `?? 0` fills it; `respond` of an absent body answers the
  `missing` status.
- `x when c` is absent unless `c` is `true`.
- `stripe.verify(…).event` is absent when the signature fails, so nothing after it fires.

Absence is not a value: it cannot be stored, compared or passed to a lambda. `none()` is a value
(the optional's empty arm) and is unrelated.

### 6.4 `if` is a fallback, not a branch

`if c then a else b` is `coalesce(guard(c, a), b)`: `a` when `c` is `true` and `a` arrived,
otherwise `b`. So `c` absent gives `b`, and `c` true with `a` absent gives `b` too. This is the
meaning that fits a dataflow of absences, and it is one meaning: `if c then a` is the same
`guard`, and `a when c` is the same again, read the other way round.

Only what the choice needs is run (lazy choices, `docs/L4s/lazy-choices-L4.md`): `a` only when `c`
is `true`, and `b` only when `guard(c, a)` came out absent. So a branch not taken asks the host
nothing: its requests aren't sent, its `await`s don't park the run, its reads aren't made, and a
holon in it isn't entered. `x ?? y` is the same rule: `y` only when `x` is absent. This changes
no value, only what runs. While `c` (or `x`) waits on an answer from the host, the choice waits
too, and decides in the next round. A holon's inputs are computed before it is entered, as a
call's arguments are: `h(go, http.get(…))` sends the request even if `h` only uses it when `go`.

The strict three-way choice, where an absent arm makes the whole result absent whatever `c` says,
is `select(c, a, b)`, written as a call. Being strict, it needs, and runs, both arms. It is rarely
what a flow wants.

### 6.5 `match` is a choice

A choice of one value by another (an operator, an event's type, a command) is `match`:

```
respond match req.json.op {
  "add" -> a + b
  "sub", "minus" -> a - b
  "div" -> a / b
  else -> 0
}, missing: 400
```

The subject is compared with each pattern by `equals` (structural, never coerced: `1` is not
`"1"`), and the first arm with a matching pattern is chosen. A pattern is a literal: a number, a
text, `true` or `false`; to test a condition, use `if`. Arms go one to a line, or after commas
(`match b { true -> "yes", false -> "no" }`), and `else`, if there is one, comes last.

What makes it a choice and not a chain of `if`: **the chosen arm's absence is the match's.** With
`b = 0`, `a / b` has no value, and `match` with `"div"` has none either, so `respond … missing:`
answers 400. A chain `if op == "div" then a / b else 0` falls through and answers `0`, silently.
The arms not chosen don't matter at all, and don't run (§6.4): `"add"` answers `a + b` whatever
`a / b` would be. So an arm may send a request or `await`; only the chosen arm's are asked. With
the subject absent, the match is absent, and the `else` with it. With no pattern matching and no
`else`, it is absent too.

It is sugar, and one unit with what it lowers to: `guard(equals(s, p), a)` for each arm, chained
by `??`, and the `else` guarded by none of the patterns matching, so a failed arm can't fall
through to it. The canvas shows those nodes, and `fmt` prints them back as `match`
(`docs/L4s/match-L4.md`). The `calc` sample shows both halves.

### 6.6 Reads at the start, writes at the end

`state.x` and `notes[k]` on the right of `=` are the values as the run began. The writes are
Effects the host commits when the run finishes. So a run never sees its own writes, two runs that
start together both see the old value (and the host's retry, as the likes sample shows, runs one
again), and a key written twice in one flow has no meaning, so it is an error.

### 6.7 Totality and purity

Every program terminates: iteration is `map` and `fold` over a list, bounded by the list (the
one leaf that makes a list from a number, `range(n)`, makes at most 10000 items), and there is no
recursion (a holon cannot place itself, since its hash does not exist until it is
frozen). No construct performs an effect during evaluation; effects are values that the host
performs after the run settles. `http.*` is answered between rounds (§4.1 of
`PLATFORM_LAYOUT.md`); `await` parks the run. None of that is visible in the language beyond the
nodes' names.

### 6.8 Types

The nine wire types are core's: `Number`, `Boolean`, `Text`, `Record`, `List<T>`,
`Product<…>`, `Sum<…>`, `Effect`, `Any`. Type annotations are written as `Text`, `List<Text>`,
`Record`. Checking is the existing wire-time compatibility check at lowering: a `Text` into a
`Number` port is an error at the argument's span; `Any` is compatible with everything, in both
directions, as today. The language adds no inference beyond what the canvas does: a holon's port
takes its first wire's type.

## 7. Names, hashes and canonical form

- **The hash is the DAG's.** Binding names, holon names, lambda parameter names, collection
  declarations, comments, blank lines and statement order are not in the preimage. They are kept
  in the flow document's `names` map (holons) and the nodes' `extra` (spans), never in a unit.
- **The same node many times over is fine.** Each `{…}` part of a string is an `as_text`, so a
  route with many strings holds many copies of one unit. The hash tells copies apart by where each
  one's value goes, down to the route's response and effects (`docs/L4s/child-order-canonical-L4.md`
  §4), so a route holds as many as it needs, in any order they were written. The one thing it
  refuses is the same value computed twice and never used: nothing tells those two apart, and
  `check` says so ("remove one, or use it"). The sample `issue-links` is a route of thirteen parts.
- **Resolution happens at check time.** A catalog name resolves to `interface_of`; a template to
  its builder; a holon to the hash frozen for it in this file. Every hash the flow depends on is in
  the document's `units` closure, which is its lockfile.
- **Canonical text.** `holon fmt` prints the expression form from the document (§11), with a fixed
  layout: headers, holons (inner before outer, in first-use order), routes in declared order,
  within a block the topological order of the nodes with a binding wherever a value is used more
  than once. `fmt(fmt(t)) = fmt(t)`, and two texts that lower to the same document print the same.

## 8. Layout

Positions are not part of a program. The expression form has no `at`. The flow document keeps
positions, as it does today, so a flow authored on the canvas keeps its layout through save and
open. When text is compiled, nodes are laid out automatically (`lang/layout.rs`): each connected
group (a route, the flow's own statements) is a band, stacked in the order the text writes them;
inside a band a node's column is its depth, a node nothing feeds sits just before the first node
it feeds, and a column keeps statement order. A holon's own canvas is laid out when it is
expanded, as before. When a document is printed, positions are dropped.
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
| unknown name `x` (not a leaf, template, holon, noun, collection or binding) | the name |
| `x` is a leaf (or keyword, or noun); a binding may not shadow it | the binding |
| `f` takes `n` inputs, `m` given | the extra or missing argument |
| `f` has no input or param `p` / `p` given twice | the name |
| input `p` can't be left out | the call |
| `Text` doesn't fit `Number` at `f`'s `p` | the argument |
| a value nobody uses | the statement |
| `notes` is not a declared collection (did you mean `collection notes` in the header?) | the name |
| `http.*`'s host must be a literal: it is the permission the deployment asks for | the host |
| `state.k` written twice | both assignments |
| `respond` outside a route / more than one `respond` | the statement |
| `map` gives its function `n` inputs; this one takes `m` | the function |
| holon `h`: input `p` never used / output `o` never bound / bound twice | the port or the binding |
| routes `A` and `B` share `x` | the shared binding |
| `A` clashes with `B` / not a method / not a route path | the route header |
| this isn't wired to the route's request, so it isn't part of the route | the statement |
| the wiring form's existing errors | their line, now with a column |

## 10. The wiring form stays

Today's statements remain valid and are the **wiring form**:

```
node greet = concat { } at 40 120
hi.out -> greet.a
holon slugify(text "lowercase · s") -> (slug "out") { … }
```

Both forms coexist in one file and share one namespace: a `node` name can be used in an
expression (`greet.out` is its output), and a binding can be a wire's end (`greet.out ->
res.body`). This matters for two reasons:

1. **Round-tripping arbitrary canvases.** A composite made on the canvas can have port names the
   expression form cannot spell (`lowercase · s`). The holon header's quoted unit name covers the
   boundary; the wiring form covers anything else. The printer uses the expression form wherever
   it can and falls back to a `node`/wire statement only where it cannot.
2. **Migration.** Every `.holon` file in the wild keeps working, and `fmt` upgrades it.

`fmt` prints the expression form. Nothing is removed from the parser.

## 11. Canvas to text: the printer

The printer turns a flow document into canonical text. The algorithm:

1. Headers: the settings, then one `collection` line per collection name any leaf in the flow
   names, sorted.
2. Holons: every composite the document carries (the `units` closure), inner before outer, each
   printed once under its name from the `names` map (or a generated one), its inner canvas opened
   with `inner_canvas` exactly as the expand view opens it. Effect outputs print in the method
   form under their port names.
3. Routes, in the order `find_routes` finds them; then, for a flow without routes, the top-level
   statements.
4. Within a group, nodes in topological order. A node whose one output has exactly one consumer
   prints inline, as a nested expression, at its consumer, unless the text named it: `fmt` keeps
   an author's bindings, as a formatter keeps variables. A node with several consumers, or a
   multi-output composite, or an `await`, gets a binding. The pass-through the lowering places for
   a holon input used twice (§12, step 3) prints as the input itself, so `n + n` comes back as
   written. An Effect with no consumer
   prints as a statement, in the assignment form where there is one (`store_write` as
   `state.k = …`, `collection_put` as `notes[k] = …`, `collection_delete` as `delete notes[k]`),
   else as a bare call. The nouns print for their leaves (`secret.x`, `state.x`, `now`,
   `notes[k]`, `notes.list(…)`, `http.post(…)`, `stripe.verify(…)`, `await x`); consts print as
   literals, a duration-shaped `expires`/`tolerance`/`every` as a duration;
   `concat` over `as_text` parts and consts prints as an interpolated string; `record`/`with_field`
   chains print as record literals when the chain starts at `record()` or as `{ ..base, … }` when
   it does not; `list` as a list literal; `default` with a literal param and `coalesce` as `??`;
   `guard` as `when`, and `coalesce` over a `guard` as `if … then … else …`; binary leaves as
   operators; `field` chains as `.name`; `respond` as the statement.
5. Names: a binding takes the node's name from the text it came from when it has one (the
   lowering's names, handed to the printer), else one from what the node is (a `field`'s field,
   a `secret`'s name, a state key, `t` for the clock, `item`, `page`, `response`), uniquified and
   kept clear of the leaves, keywords, collections and holons.
6. Any node the rules above cannot express prints in the wiring form: a group with a request node
   that isn't a route (the route errors as comments above it), or nodes beside the routes.

Properties, tested: `compile(print(doc))` carries the same unit hashes as `doc`;
`print(compile(t))` is the canonical form up to the author's bindings; `print` is idempotent
through `compile`. A `concat` of consts stays a `concat` (an interpolated string lowers with an
`as_text` per part, so only a `concat` with one prints as a string), and a JSON `const` prints as
a literal only when the literal writes back to the same text, with keys sorted as the lowering
writes them.

## 12. Build order

Each step is one inch: its tests, a live check, a commit. The whole is about two weeks. The one
step that changed core is done; none changes the host, the server or the document format.

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
   an `as_text`), carry a proposed holon's port names in quotes (§10), and bind one `now` where
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
   again. `fmt` keeps a text's comments, as §4 says.

Dependencies: 3 needs 1 and 2; 4 needs 3 (the golden test is what makes the printer checkable);
5 and 6 need 4; 7 and 8 need 5.

## 13. Open forks

Decided provisionally above; listed here so they are revisited on evidence, not forgotten.
Decided ones are kept, with the decision, for a reader who wonders why.

- **F1. Bare words as param values.** `collection: notes` without quotes is convenient and
  matches the wiring form, but a value like `hex` and a name like `paid` read the same as
  identifiers. Alternative: quotes always. Decide after agents have written a hundred flows:
  count the mistakes.
- **F2. Positional params for zero-input leaves.** *Decided (2026-10-24):* the nouns of §5.4
  replace the need. `secret.api_token` and `state.beats` read better than either
  `secret(name: api_token)` or `secret("api_token")`, and they keep the one call rule intact for
  everything else.
- **F3. String interpolation.** *Decided (2026-10-24):* `"Hello, {name}"` lowers to one `concat`
  over its pieces, each `{…}` part wrapped in `as_text`, the leaf added for this. `++` is gone;
  `+` stays numeric.
- **F4. Top-level bindings beyond literals.** The restriction exists because routes must be
  separate groups. An alternative is duplicating the whole subexpression per route, which keeps
  routes separate at the cost of hidden node copies. Refused for now: the error message points at
  holons, which is the honest fix.
- **F5. `if` without `else`.** *Decided (2026-10-24):* `if c then a` is `a when c`, and
  `if c then a else b` is the same guard with a `coalesce` fallback, so `if` has one meaning,
  fallback, and `select` is a plain call for the strict choice. The `coalesce` leaf it needs is
  built (§5.6).
- **F6. Named inputs.** *Decided (2026-10-24):* a named argument is an input when the unit has
  that port, else a param; `_` is gone. The one clash, `default`, is never written.
- **F7. Where the layout of a text-authored flow lives.** *Decided (2026-10-24):* in the
  editor's saved flow only, for now. Structure is identity and shape is only visual, so a
  `.holon` file carries no positions: outside the editor it is laid out by the builder on import,
  and inside it the saved flow keeps each node's position by its name in the text, so a text
  edit or a canvas edit leaves the arrangement where it was (`docs/L4s/text-pane-L4.md`). An
  author who exports and imports again gets a fresh layout. A sidecar `.holon.layout` (names to
  positions) is the next step if anyone asks; positions in the text are refused.
- **F8. Lambdas with captures, by lifting.** *Built (2026-10-25):* `map` and `fold` pass the
  values given after the list to their function, after the item, and a lambda's captures are
  lifted to those inputs (§5.9). The `quote` sample shows it.
- **F9. `respond` with a bare number.** `respond 204` is a status; `respond n` with `n` a bound
  number is a body; `respond 42` is a status and the author who wanted a body writes
  `respond body: 42`. The rule is small and the ambiguity rare; revisit if the error report shows
  people tripping on it.
- **F10. Effects inside holons.** Writes inside a holon leave it as named outputs, so the method
  form (`posts.put(k, v)`) is what `fmt` prints there, and the assignment form gets a generated
  output name. Alternative: let a holon's unnamed effects be exposed under names derived from
  their keys (`put_posts`). Decide when the first holon with several writes exists.
- **F11. A choice by value, `match`.** A record used as a table is spoiled by one absent entry,
  and a chain of `if` falls through when the chosen arm fails. `match op { "add" -> a + b, … }`
  chooses one arm by the subject, so arms not chosen don't matter and a chosen arm's failure is
  the result's. It lowers to `equals`, `guard` and `coalesce`, with the `else` guarded by "no
  pattern matched". Decided and built (§6.5); the `calc` sample shows it. `docs/L4s/match-L4.md`.
- **F12. Lazy choices.** Every node was evaluated, so a request inside an `if` branch not taken
  was sent anyway, and an `await` there parked the run. Decided and built: `guard` and `coalesce`
  evaluate only the inputs they need, with "not yet answered" kept apart from absent so a choice
  waits instead of deciding early (§6.4). No value changes, only what the host is asked. `select`
  stays strict: an absent side makes it absent, so it needs both. The `cache-aside` sample shows
  it. `docs/L4s/lazy-choices-L4.md`.

## 14. What this document does not decide

The MCP server and its tools (decided in `docs/L4s/agent-surface-L4.md`), the export and
verifier for run logs, and the marketplace's `use`. Each depends on this language existing and
none changes it. The sandboxed WebAssembly module is decided and built (`docs/L4s/wasm-module-L4.md`);
the language's spelling for one is the js holon of §5.8a, and a `module` statement placing a
module by hash waits for a second module.
