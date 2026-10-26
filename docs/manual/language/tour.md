# A tour of the language

This tour builds up the language one idea at a time. Every example is a whole flow: save it as a
`.holon` file and run `holon check` on it to see what it is and what it may do. The full rules are
in the [reference](reference.md).

## The smallest server

```holon
flow hello
port 8101

route GET /hello/:name (req) {
  respond "Hello, {req.params.name}"
}
```

- `flow hello` names the flow; it deploys under that name.
- `port 8101` is the port its server answers on.
- `route GET /hello/:name (req) { … }` answers `GET` requests to that path. `(req)` names the
  request; `req.params.name` is the `:name` part of the path.
- `respond` says what the route answers. `"Hello, {…}"` puts a value into text.

```
$ holon check hello.holon
hello: 6 nodes, 6 wires
a server on :8101
  GET /hello/:name
unit 7714237e7d00edfc…
it may:
  receive requests
```

Each piece of the text is a node in a graph, and `check` counts them. The `unit` is the hash of
that graph: the flow's identity. `it may:` is what the flow can do, worked out from the text. This
one can only receive requests.

## Requests and answers

A request has its parts: `req.params`, `req.query`, `req.headers`, `req.body` (the body as text)
and `req.json` (the body parsed as JSON). A missing part has no value, which isn't an error:

```holon
flow greet
port 8102

route POST /greet (req) {
  name = req.json.name ?? "stranger"
  respond { greeting: "Hello, {name}", at: now }, status: 201
}
```

- `name = …` binds a name to a value. A binding is one value, used as often as you like.
- `a ?? b` is `a`, or `b` when `a` has no value.
- `{ greeting: …, at: … }` is a record; it goes out as JSON. A text body goes out as plain text.
- `now` is the time the run started, in seconds since the epoch.
- `status: 201` sets the status. Without it, a route answers 200.

## Absence: how a flow says no

A value that doesn't arrive makes everything built from it absent too. That is how a flow says
"not found", "not allowed" or "not verified", with no exceptions:

```holon
flow notes
port 8103
collection notes

route GET /notes/:id (req) {
  respond notes[req.params.id], missing: 404
}

route PUT /notes/:id (req) {
  notes[req.params.id] = req.json
  respond req.json, missing: 400
}
```

`collection notes` declares a store of items by key. `notes[key]` reads one; a key with nothing
under it has no value, so `respond` answers its `missing` status, 404. In the `PUT` route, a body
that isn't JSON has no `req.json`, so the route answers 400, and the write, which needs that value
too, never happens.

`notes[key] = value` writes, and `delete notes[key]` deletes. Writes happen when the run finishes,
and only if the deployment was granted them.

## Choosing

```holon
flow calc
port 8104

route POST /calc (req) {
  a = req.json.a
  b = req.json.b
  respond match req.json.op {
    "add" -> a + b
    "sub" -> a - b
    "mul" -> a * b
    "div" -> a / b
  }, missing: 400
}
```

`match` picks the arm whose pattern equals the value. Only the chosen arm runs. If it has no value
(dividing by zero has none), neither does the `match`, and the route answers 400; it never falls
through to another arm.

Three smaller forms:

- `x when ok` is `x` only when `ok` is `true`, and no value otherwise.
- `if ok then a else b` is `a` when `ok` is `true` (and `a` arrived), otherwise `b`.
- `a ?? b`, from before.

Like `match`, they only run what they need: the branch not taken isn't computed, so its requests
aren't sent and its reads aren't made.

## State and the clock

A flow with `every` instead of `port` runs on a clock:

```holon
flow heartbeat
every 10s

t = now
state.last_beat = t
state.beats = (state.beats ?? 0) + 1
state.first_beat = state.first_beat ?? t
```

`state.beats` on the right is the value as the run began; `state.beats = …` writes it when the
run finishes. So the line means exactly what it says, and a run never sees its own writes. A key
may be written once per flow.

Durations are numbers of seconds: `10s`, `5m`, `2h`, `1d`.

## Calling other services

```holon
flow slideshow
port 8105

route GET /slideshow (req) {
  page = http.get("httpbin.org", "/json")
  respond parse_json(page.body).slideshow.title
}
```

`http.get(host, path)` calls another service; `post`, `put`, `patch`, `delete` and `head` work the
same way, with `headers:` and `body:`. The answer is a record of `status`, `headers` and `body`.

The host must be written in the text, because the host is the permission:

```
it may:
  call https://httpbin.org
  receive requests
```

## Secrets

```holon
flow guarded
port 8106

route GET /me (req) {
  ok = secure_equals(req.headers.authorization, "Bearer {secret.api_token}")
  respond { you: "in" } when ok, missing: 401
}
```

`secret.api_token` is a secret an admin sets for the deployment. Its value never appears in a
log, a replay or the editor. `secure_equals` compares in constant time. A caller without the token
gets 401.

`optional secret.name` in the headers marks a secret the flow can run without: the deploy can
leave it out, and `secret.name` then has no value.

## A webhook

Putting it together: a Stripe webhook, verified, posted to Slack.

```holon
flow stripe-to-slack
port 8107

route POST /stripe (req) {
  event = stripe.verify(req, secret.stripe_webhook_secret, now).event
  sent = http.post("hooks.slack.com", secret.slack_webhook_path,
    headers: { "content-type": "application/json" },
    body: json({ text: "Stripe: {event.type}" }))
  respond sent.status, missing: 400
}
```

When the signature doesn't verify, `event` has no value, nothing after it runs, nothing is posted,
and the route answers 400. The [Stripe to Slack guide](../guides/stripe-to-slack.md) runs this
against the real services.

## Lists

There are no loops. `map` and `fold` go over a list, and `range(n)` makes a list to count over:

```holon
flow quote
port 8108

route POST /quote (req) {
  tax = req.json.tax ?? 0
  lines = map(req.json.items, fn: fn(item) -> { name: item.name, total: item.price * item.qty * (1 + tax) })
  total = fold(0, lines, fn: fn(acc, line) -> acc + line.total)
  respond { lines: lines, total: total }
}
```

`fn(item) -> …` is a function written in place. It can use names from around it (`tax` here).
`fold(start, list, fn: …)` carries a value through the list.

Because there are no loops and no recursion, every run ends.

## Holons: flows inside flows

A holon is a function: a piece of flow with named inputs and outputs, placed as one node.

```holon
flow slugs
port 8109
collection posts

holon slugify(text) -> slug {
  slug = join(split(lowercase(text), " "), "-")
}

route POST /posts (req) {
  slug = slugify(req.json.title)
  posts[slug] = req.json
  respond { slug: slug }, missing: 400
}
```

A holon with several outputs is used as `h(…).name`. A holon is one unit, with one hash, wherever
it is placed, and it can be passed to `map` and `fold` by name: `map(titles, fn: slugify)`.

## JavaScript, for what the vocabulary can't say

When a computation is too awkward to write with the building blocks, a holon can be written in
JavaScript:

```holon
flow stats
port 8110

holon summarize(numbers) -> (total, mean) js {
  const total = numbers.reduce((a, b) => a + b, 0);
  return { total, mean: numbers.length ? total / numbers.length : 0 };
}

route POST /stats (req) {
  s = summarize(req.json.numbers)
  respond { total: s.total, mean: s.mean }, missing: 400
}
```

The body runs in a sandbox with no clock, no randomness, no network and no storage: it can reach
nothing outside. What the flow asks for is its budget:

```
it may:
  receive requests
  run code it carries (a js node): up to 200000000 instructions and 16 MiB per call, reaching nothing outside
```

A call that runs past its budget runs out of fuel and has no value; `fuel 50M` (instructions) or
`memory 32MB` (32 MiB) after `js` change the budget. Each call starts fresh, so the same call always gives the same
answer, and a run still replays exactly.

## Waiting for a person

A run can park and finish later, when someone follows a link:

```holon
flow approval
port 8111

route POST /orders (req) {
  respond "/paid/{resume_token(paid)}"
  paid = await paid, expires: 1d
  state.last_order = { ..req.json, paid_via: paid.method }
}

route GET /paid/:token (req) resumes paid {
  respond "thanks, the order is paid"
}
```

The first route answers at once with a link, then waits at `await`. When the link is opened, the
second route delivers the event, and the first run finishes: it writes the order. If nobody opens
the link within a day, the run expires. Order in the text doesn't matter: a value is computed when
what it needs has arrived.

## Files

A flow can read and write files in folders an admin has bound to a name on the host:

```holon
flow inbox-report
port 8112
folder inbox
folder reports

route POST /report (req) {
  name = req.json.name
  text = inbox[name]
  reports["{name}.md"] = "# {name}\n\n{length(split(text, "\n"))} lines"
  respond { report: "{name}.md" }, missing: 404
}
```

The flow names the folders, never a path. A flow can also run on files instead of requests:
`on file drop` in place of `port` runs it once for each new file in the folder `drop`, with the
file's handle as `ingress`. See [Files on the host](../guides/files.md).

## Databases

A flow can query and change a SQL database an admin has bound to a name, with the SQL in quotes
and its values after it:

```holon
flow orders-api
port 8150
database shop

route POST /order/:id/refund (req) {
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", req.params.id, expect: 1)
  respond { refunded: req.params.id }
}
```

The tables its SQL names are what it asks for (`DbWrite("shop/orders")`); `expect: 1` makes the
refund happen once, and a second answers 409. A MongoDB is called with records instead, its
fields and operators written in the call and each value passed in as data:
`notes.find("notes", { owner: req.params.owner })`. See
[Databases on the host](../guides/databases.md).

## Where to next

- The [reference](reference.md) has every construct and rule.
- The [leaves](leaves.md) are every building block you can call.
- Every sample is a complete flow: `holon new my.holon --sample NAME` writes one, and `holon new
  --help` lists them.
