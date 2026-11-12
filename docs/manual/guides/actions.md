# Actions: runs by hand

Some runs aren't anyone's request: setting a counter back, sending a report again, clearing what
a test left. An **action** is a handler a person runs by its name, beside a flow's routes and
clocks. The network never reaches it, whatever the flow's port: it runs through Holon, from the
editor, a script or an agent, as someone signed in, and its run is logged like any other.

## Writing one

```ts
import { action, flow, route } from "holon"
import state from "state"

flow("counter", { port: 8170 })

route.post("/hit", () => {
  const n = (state.hits ?? 0) + 1
  state.hits = n
  return n
})

export const reset = action((input) => {
  state.hits = input.to ?? 0
  return { was: state.hits ?? 0, set: input.to ?? 0 }
})
```

`export const reset = action((input) => { … })` names the action `reset`, and `input` what the
person running it sent: a JSON value, `{}` when they sent nothing. So `{"to": 5}` sets the count
to 5, and nothing sets it to 0.

- **Its answer** is what it returns, a value alone (a route's `status(…)` and `respond(…)` aren't
  an action's). An action that only writes answers `null`.
- **Its name** is the name it is exported as, one action per name. It is exported because it is
  what reaches the flow from outside. A flow may have several actions, and actions alone: a flow
  of only actions has no port and listens on none.
- **What it reads** is the run's start, as in any run: `state.hits` on the right above is the count
  before the write, so `was` is the old count.
- **Its writes** are tied to it, as a route's are to its request, and land when the run ends.

The **ticker** sample (`holon new --sample ticker`) has one beside a clock and three routes.

## Running one

Deploy the flow first: an action runs on a running deployment, the version deployed.

**In the editor**, the bottom pane's [Try a request](editor.md#the-bottom-pane) lists the flow's actions under
its routes. Each has a box for what to send, as JSON (empty is `{}`), and **run**. The answer
shows under it with the run's number, and **view run** opens it in view mode. When the canvas has
changes not deployed, the panel says the run runs the deployed version.

**On [the flow's page](editor.md#a-flows-page)**, its Status tab has the same
rows.

**From a script**, `POST /trigger/<slug>/<name>` on the holon server, with an
[service's token](../reference/server.md) and what to send as the body. Each row in the editor shows the
command, with what is in its box:

```sh
curl -X POST http://127.0.0.1:7420/trigger/counter/reset -H "Authorization: Bearer $HOLON_TOKEN" \
  -H "Content-Type: application/json" -d '{"to": 5}'
```

The holon server takes a POST only as `application/json` (or `application/octet-stream`), so the
`Content-Type` matters: without it the answer is a 415.

The answer is the action's answer and its run's number. After three hits:

```json
{"answer":{"set":5,"was":3},"seq":4}
```

and the next `POST /hit` answers 6.

| Answer | When |
|---|---|
| 200, `{answer, seq}` | it ran (`answer` is `null` for an action that only writes) |
| 202, `{answer: null, seq, waiting: true}` | it is waiting at an `await`, and continues later |
| 400 | what was sent isn't JSON |
| 401 | the caller isn't signed in |
| 403 | the caller may not run it (below) |
| 404 | there is no such deployment or action |
| 409 | the deployment isn't running (paused or stopped) |
| 500 | the run failed: its log entry, `seq`, says why |

## Who may run one

On a server with accounts, an editor or an admin may run an action; a viewer sees the rows, and
not **run**. Each run's log entry says who ran it and what they sent, under the action's label
(`action reset`), and view mode replays it like any run ([The web editor](editor.md)).

An agent may run an action only when a person has let it. In the deploy settings (the editor's **Settings**), **agents
may run** lists the flow's actions; ticking one asks for `AgentRuns("reset")`, which a person
approves as they do any permission, at the next deploy. It is optional: without it the flow runs
the same, and an agent that tries is refused ("agents may not run …"). Revoking it on the deployed
page (**permissions ▸**) refuses agents again at once. An agent runs an action with `holon mcp`'s
**run** tool, which answers what the action answered and the run's number, for **explain**
([Working with agents](agents.md)).

## Run by another flow

A flow runs another's action in its own text. It imports the action from the other flow, by its
slug, and calls it:

```ts
import { flow, route, status } from "holon"
import orders from "collection:orders"
import { charge } from "flow:billing"

flow("shop", { port: 8159 })

route.post("/orders", async (req) => {
  const paid = await charge({ id: req.json.id, amount: req.json.amount })
  orders[req.json.id] = { amount: req.json.amount, total: paid.answer.total }
  return status(201, paid.answer) ?? status(502)
})
```

Deploying `shop` asks a person to approve "run the action charge of the flow billing", as it asks
for a host to call, and the approval is checked again at each call. The other flow, `billing`
here, changes nothing: its action runs as it does for anyone, under its own grant and limits, and
its log says who asked: `by` "flow shop, run …". The `shop`'s own log keeps the call and its
answer, so replaying or verifying the shop's runs never calls billing again.

The call answers `{answer, run}` (`answer` is what the action answered, absent for one that only
writes; `run` is its number in billing's log), or `{error, message}` when nothing ran. `error` is:

| `error` | Why |
|---|---|
| `refused` | the call isn't granted or was revoked; the flow is already in the chain of calls, itself included; or it would send a secret (or a value computed from one) |
| `not_found` | no flow is deployed under that name |
| `not_running` | it is paused or stopped |
| `no_action` | it has no action of that name |
| `waiting` | the action parked at an `await`; its run goes on when the event arrives, and `run` says which |
| `too_deep` | the chain of calls is past `max_call_depth` (3 by default) |
| `failed` | the action failed: its run in the other log says why |

Handle them as a failed request is handled: by absence. `paid.answer` has no value when the call
failed, so the order above isn't stored and `?? status(502)` answers 502. To say why, bind the
whole answer and read `error` and `message`.

A flow isn't called back: `shop` calls `billing`, which can't call `shop` (or `billing`) again,
because the call would wait for the run that made it. Calls to one flow in a round go one at a
time, in the order they are written.

The **billing** and **shop** samples (`holon new --sample shop`) are this pair: deploy billing
first.

## Flows nothing starts

A flow with no route, clock, file handler or action starts nothing, and deploying it is refused.
To run its work by hand, put its statements in an action, `export const run = action((input) =>
{ … })`, returning what it computes.

See also: [Permissions](../reference/permissions.md#who-may-run-an-action) for the items an action
asks for.
