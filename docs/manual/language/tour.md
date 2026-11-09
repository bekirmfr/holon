# A tour of the language

This tour builds up the language one idea at a time. Every example is a whole flow: save it as a
`.holon.ts` file and run `holon check` on it to see what it is and what it may do. The syntax is
TypeScript's, so any editor reads it. The full rules are in the [reference](reference.md).

## The smallest server

```ts
import { flow, route } from "holon"

flow("hello", { port: 8101 })

route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
```

- `import { flow, route } from "holon"` names the parts of the language the file uses.
- `flow("hello", { port: 8101 })` names the flow, which deploys under that name, and the port its
  server answers on.
- `route.get("/hello/:name", (req) => …)` answers `GET` requests to that path. `req` is the
  request; `req.params.name` is the `:name` part of the path.
- What the function returns is what the route answers. `` `Hello, ${…}` `` puts a value into text.

```
$ holon check hello.holon.ts
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

```ts
import { flow, route, status } from "holon"
import { now } from "holon:clock"

flow("greet", { port: 8102 })

route.post("/greet", (req) => {
  const name = req.json.name ?? "stranger"
  return status(201, { greeting: `Hello, ${name}`, at: now })
})
```

- `const name = …` binds a name to a value. A binding is one value, used as often as you like.
- `a ?? b` is `a`, or `b` when `a` has no value.
- `{ greeting: …, at: … }` is a record; it goes out as JSON. A text goes out as plain text.
- `now` is the time the run started, in seconds since the epoch. It is imported from
  `"holon:clock"`, so the top of the file says the flow reads the clock.
- `status(201, …)` sets the status. Without it, a route answers 200.

## Absence: how a flow says no

A value that doesn't arrive makes everything built from it absent too. That is how a flow says
"not found", "not allowed" or "not verified", with no exceptions:

```ts
import { flow, route, status } from "holon"
import notes from "collection:notes"

flow("notes", { port: 8103 })

route.get("/notes/:id", (req) => notes[req.params.id] ?? status(404))

route.put("/notes/:id", (req) => {
  notes[req.params.id] = req.json
  return req.json ?? status(400)
})
```

`import notes from "collection:notes"` names a store of items by key. `notes[key]` reads one; a
key with nothing under it has no value, so the route answers `status(404)`. In the `PUT` route, a
body that isn't JSON has no `req.json`, so the route answers 400, and the write, which needs that
value too, never happens.

`notes[key] = value` writes, and `delete notes[key]` deletes. Writes happen when the run finishes,
and only if the deployment was granted them.

## Choosing

```ts
import { flow, route, status } from "holon"

flow("calc", { port: 8104 })

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

`switch` picks the case equal to the value. Only the chosen case runs. If it has no value
(dividing by zero has none), the route answers 400; it never falls through into another case. An
op no case names reaches the last `return`, and answers 400 too.

Three smaller forms:

- `c ? a : undefined` is `a` only when `c` is `true`, and no value otherwise.
- `c ? a : b` is `a` when `c` is `true`, otherwise `b`. Like a `switch` case, the side chosen is the
  answer: if `a` has no value, neither has the whole. `if (c) … else …` chooses between statements
  the same way.
- `a ?? b`, from before.

Like `switch`, they only run what they need: the branch not taken isn't computed, so its requests
aren't sent and its reads aren't made.

## State and the clock

A flow with no port and an `every` runs on a clock:

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

`state.beats` on the right is the value as the run began; `state.beats = …` writes it when the
run finishes. So the line means exactly what it says, and a run never sees its own writes. A run
writes a key once. `state.beats++` says the same, `++` counting from 0 where there is no value
yet; written as a value, `times[state.beats++] = t`, it is the count before this beat.

Durations are texts: `"10s"`, `"5m"`, `"2h"`, `"1d"`.

A flow with routes may have a clock beside them, and actions too:

```ts
import { action, every, flow, route } from "holon"
import state from "state"
import { now } from "holon:clock"

flow("visits", { port: 8207 })

every("1h", () => {
  state.last_hour = now
})

export const reset = action(() => {
  state.visits = 0
})

route.post("/visit", () => {
  state.visits = (state.visits ?? 0) + 1
  return state.visits ?? 0
})

route.get("/status", () => ({ visits: state.visits ?? 0, last_hour: state.last_hour }))
```

`every("1h", …)` runs on its own every hour, with no request, and `now` in it is the time of the
tick. Each handler is a run of its own, and the state is the flow's, shared by all of them:
`/status` reads what the clock wrote. `state.visits = …` in the route never looks at the request,
and is still the route's: a handler is everything tied to its trigger.

`export const reset = action(…)` is run by hand, by its name, from the editor or a script, never
from the network: it zeroes the count when a person runs it. See [Actions](../guides/actions.md)
and the [reference](reference.md#clocks-files-and-actions).

## Calling other services

```ts
import { flow, route } from "holon"
import httpbin from "host:httpbin.org"

flow("slideshow", { port: 8105 })

route.get("/slideshow", async () => {
  const page = await httpbin.get("/json")
  return JSON.parse(page.body).slideshow.title
})
```

`import httpbin from "host:httpbin.org"` names a host, and `await httpbin.get(path)` calls it;
`post`, `put`, `patch`, `delete` and `head` work the same way, with `{ headers, body }` after the
path. The answer is a record of `status`, `headers` and `body`. A handler that waits for a call is
`async`, as TypeScript has it.

The host is written in the import, never computed, because the host is the permission:

```
it may:
  call https://httpbin.org
  receive requests
```

## Secrets

```ts
import { flow, route, secure_equals, status } from "holon"
import api_token from "secret:api_token"

flow("guarded", { port: 8106 })

route.get("/me", (req) => {
  const ok = secure_equals(req.headers.authorization, `Bearer ${api_token}`)
  return (ok ? { you: "in" } : undefined) ?? status(401)
})
```

`import api_token from "secret:api_token"` names a secret an admin sets for the deployment, and
`api_token` reads it. The imports name everything a flow needs from outside it, so the top of the
file says what to set before it works. Its value never appears in a log, a replay or the editor.
`secure_equals` compares in constant time. A caller without the token gets 401.

`import name from "secret:name" with { optional: "true" }` names a secret the flow can run
without: the deploy can leave it out, and `name` then has no value.

## A webhook

Putting it together: a Stripe webhook, verified, posted to Slack.

```ts
import { flow, route, status, stripe } from "holon"
import slack_webhook_path from "secret:slack_webhook_path"
import stripe_webhook_secret from "secret:stripe_webhook_secret"
import hooks from "host:hooks.slack.com"
import { now } from "holon:clock"

flow("stripe-to-slack", { port: 8107 })

route.post("/stripe", async (req) => {
  const event = stripe.verify(req, stripe_webhook_secret, now).event
  const sent = await hooks.post(slack_webhook_path, {
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text: `Stripe: ${event.type}` }),
  })
  return sent.status ?? status(400)
})
```

When the signature doesn't verify, `event` has no value, nothing after it runs, nothing is posted,
and the route answers 400. The [Stripe to Slack guide](../guides/stripe-to-slack.md) runs this
against the real services.

## Lists

There are no loops. `map` and `reduce` go over a list, and `range(n)` makes a list to count over:

```ts
import { flow, route } from "holon"

flow("quote", { port: 8108 })

route.post("/quote", (req) => {
  const tax = req.json.tax ?? 0
  const lines = req.json.items.map((item) => ({
    name: item.name,
    total: item.price * item.qty * (1 + tax),
  }))
  const total = lines.reduce((acc, line) => acc + line.total, 0)
  return { lines, total }
})
```

`(item) => …` is a function written in place. It can use names from around it (`tax` here).
`reduce(f, start)` carries a value through the list.

Because there are no loops and no recursion, every run ends.

## Functions: flows inside flows

A function is a piece of flow with named inputs and outputs, placed as one node.

```ts
import { flow, route, status } from "holon"
import posts from "collection:posts"

flow("slugs", { port: 8109 })

function slugify(text) {
  const slug = text.toLowerCase().split(" ").join("-")
  return slug
}

route.post("/posts", (req) => {
  const slug = slugify(req.json.title)
  posts[slug] = req.json
  return { slug } ?? status(400)
})
```

A function that returns a record has an output for each of its keys, used as `f(…).name`. A
function is one unit, with one hash, wherever it is placed, and it can be passed to `map` and
`reduce` by name: `titles.map(slugify)`.

## JavaScript, for what the vocabulary can't say

When a computation is too awkward to write with the building blocks, a function can run as
JavaScript, as written:

```ts
import { flow, js, route, status } from "holon"

flow("stats", { port: 8110 })

const summarize = js(function summarize(numbers) {
  const total = numbers.reduce((a, b) => a + b, 0);
  return { total, mean: numbers.length ? total / numbers.length : 0 };
}, { outputs: ["total", "mean"] })

route.post("/stats", (req) => {
  const s = summarize(req.json.numbers)
  return { total: s.total, mean: s.mean } ?? status(400)
})
```

The body runs in a sandbox with no clock, no randomness, no network and no storage: it can reach
nothing outside. What the flow asks for is its budget:

```
it may:
  receive requests
  run code it carries (a js node): up to 200000000 instructions and 16 MiB per call, reaching nothing outside
```

A call that runs past its budget runs out of fuel and has no value; `{ fuel: "50M" }`
(instructions) or `{ memory: "32MB" }` (32 MiB) in its options change the budget. Each call starts
fresh, so the same call always gives the same answer, and a run still replays exactly.

## Waiting for a person

A run can park and finish later, when someone follows a link:

```ts
import { action, event, flow, route } from "holon"
import state from "state"

flow("approval", { port: 8111 })

route.post("/orders", (req) => {
  const started = payment(req.json)
  return `/paid/${started.waiting.paid}`
})

export const payment = action(async (order) => {
  const paid = await event("paid", { expires: "1d" })
  state.last_order = { ...order, paid_via: paid.method }
})

route.get("/paid/:token", () => "thanks, the order is paid", { resumes: "paid" })
```

A route answers and is done, so what waits is a run it starts: `payment(req.json)`, not awaited,
starts the action `payment` and answers once that run waits, with the token that resumes it, and
the route answers at once with the link. When the link is opened, the third route delivers the
event, and the order's run finishes: it writes the order. If nobody opens the link within a day,
the run expires. Order in the text doesn't matter: a value is computed when what it needs has
arrived.

When what the run waits for is people's say, not a request, it waits at an approval:

```ts
import { action, approval, flow, route, status } from "holon"
import state from "state"

flow("refunds", { port: 8116 })

route.post("/refunds", (req) => {
  refund(req.json)
  return status(202, "waiting for two people")
})

export const refund = action(async (asked) => {
  const decision = await approval("refund", { people: 2, expires: "1d" })
  if (!decision.approved) {
    return { declined: decision.reason ?? "expired" }
  }
  state.last_refund = { ...asked, by: decision.by }
  return { refunded: asked.amount, by: decision.by }
})
```

No link is handed out and no route resumes it: two different people signed in to Holon approve it,
in the editor's Approvals or with `holon decide`, or one declines, with a reason. An agent never
decides one. The run then goes on with `decision.by`, who approved it; declined, it writes
nothing. The `approved-payout` sample does this with a bank's balance. Where approvals go next,
in steps filled by roles and in order, is the target design,
[`docs/L4s/access-and-approvals-L4.md`](../../L4s/access-and-approvals-L4.md) §8.

## Files

A flow can read and write files in folders an admin has bound to a name on the host:

```ts
import { flow, length, route, status } from "holon"
import inbox from "folder:inbox"
import reports from "folder:reports"

flow("inbox-report", { port: 8112 })

route.post("/report", (req) => {
  const name = req.json.name
  const text = inbox[name]
  reports[`${name}.md`] = `# ${name}\n\n${length(text.split("\n"))} lines`
  return { report: `${name}.md` } ?? status(404)
})
```

The flow names the folders, never a path. A flow can also run on files instead of requests:
`on.file.created(drop, (f) => { … })` runs once for each new file in the folder `drop`, with the
file's handle as `f`. See [Files on the host](../guides/files.md).

## Databases

A flow can query and change a SQL database an admin has bound to a name, with the SQL in quotes
and its values after it:

```ts
import { flow, route } from "holon"
import shop from "database:shop"

flow("orders-api", { port: 8150 })

route.post("/order/:id/refund", (req) => {
  shop.exec("update orders set refunded = 1 where id = $1 and refunded = 0", req.params.id, {
    expect: 1,
  })
  return { refunded: req.params.id }
})
```

The tables its SQL names are what it asks for (`DbWrite("shop/orders")`); `expect: 1` makes the
refund happen once, and a second answers 409. A MongoDB is called with records instead, its
fields and operators written in the call and each value passed in as data:
`notes.find("notes", { owner: req.params.owner })`. See
[Databases on the host](../guides/databases.md).

## Where to next

- The [reference](reference.md) has every construct and rule.
- The [leaves](leaves.md) are every building block you can call.
- Every sample is a complete flow: `holon new my.holon.ts --sample NAME` writes one, and `holon
  new --help` lists them.
