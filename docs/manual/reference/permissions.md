# Permissions

What a flow may do is worked out from the flow itself: each node that reaches outside is one item
of its **manifest**. This page lists every item, what in a flow produces it, and the limits a
deployment runs under. [Concepts](../concepts.md#the-manifest-what-a-flow-may-do) explains the
model; [Deploying](../guides/deploying.md) shows how they are granted.

Each item has a name (what the policy file and the API use) and words (what `holon check` and
the editor show).

## Capabilities

| Name | Shown as | From |
|---|---|---|
| `NetworkIngress` | receive requests | a `route` |
| `WebSocket("/room/:name")` | hold WebSocket connections on /room/:name, and run on each message | `route.websocket("/room/:name", { … })` |
| `Clock` | read the clock | `now`, or `every` (a clock starts runs) |
| `Action("name")` | can be run by hand: name | `export const name = action(…)` |
| `Random` | use randomness | `random` |
| `HttpRequest("host")` | call https://host | `await api.get(…)`, `api.post(…)`, … with `import api from "host:host"`; the host is its import |
| `SecretRead("name")` | read the secret "name"; when the flow sends it in a request, "…, and send it to https://host"; in a database read's values, "…, and send it to the database "shop""; where what a call answers to it goes (a token), "…; what is answered to it goes to https://api" | a secret's import, `import name from "secret:name"`, read; it is sent to a host when it, or something computed from it, reaches a request's path, headers or body (an `hmac_sha256`, `sha256` or `secure_equals` of it isn't), and to a database when it is one of a read's `$1`, `$2`, … |
| `SecretAnswer("name")` | answer its callers with the secret "name", or with what is computed from it | a route whose answer the secret reaches: `return name`, `return { k: base64(name) }`; an `hmac_sha256`, `sha256` or `secure_equals` of it, or a call's answer, isn't one |
| `StateRead("key")` | read its state "key" | `state.key` read |
| `StateWrite("key")` | write its state "key" | `state.key = …` |
| `CollectionRead("c")` | read items of its collection "c" | `c[k]`, `c.list(…)` |
| `CollectionWrite("c")` | put and delete items of its collection "c" | `c[k] = …`, `delete c[k]` |
| `FileRead("f")` | read files in the folder "f" | `f[name]`, `f.list`, `f.file`, `f.lines`, `f.slice`, `f.serve`, or copying a file from `f` |
| `FileWrite("f")` | write, copy into and delete files in the folder "f" | `f[name] = …`, `delete f[name]`, `f.copy(…)` |
| `FileServe("f")` | answer its callers with files from the folder "f" | a route answering with `f.serve(name)`: `return f.serve(name)`, `respond(f.serve(name), { headers })` |
| `DbRead("d/t")` | read the table "t" of the database "d" | `d.query("select … from t …")`, `d.one(…)` |
| `DbWrite("d/t")` | insert, update and delete rows of the table "t" of the database "d" | `d.exec("update t …")` |
| `MailSend("m/a@acme.com")`, `MailSend("m/*")` | send mail through the mail "m" to a@acme.com; with `*`, "to an address it computes, within its binding's" | `m.send({ to: "a@acme.com", … })`; `*` where a recipient is computed, `m.send({ to: req.json.by, … })` |
| `FileArrival("f")` | run on each file new, changed or removed in the folder "f" (as its on file lines say), and pass it on | `on.file.created(f, …)`, `on.file.deleted(f, …)` |
| `MailArrival("m")` | run on each mail that comes to "m", read it, and move it once its run is over | `on.mail(m, …)` |
| `FeedEvent("p")` | run on each event from the feed "p" | `on.feed(p, …)` |
| `QueueMessage("q")` | run on each message from the queue "q", and delete it once its run is over | `on.queue(q, …)` |
| `QueueSend("q")` | send messages to the queue "q" | `q.send({ data, group })` |
| `RunsAction("billing/charge")` | run the action charge of the flow billing | `charge(x)`, with `import { charge } from "flow:billing"` |
| `Await("e")` | wait for an event "e" and continue later | `await event("e")` |
| `Approval("a/2")` | wait for 2 people's approval "a" and continue later | `await approval("a", { people: 2 })` |

A flow's triggers, what start its runs, are among them: `NetworkIngress` for its routes,
`WebSocket("/path")` for each websocket, `Clock` for an `every`, `FileArrival` for each folder a
file handler watches, and `Action("name")` for each action. A flow with a clock asks for `Clock` even if it never reads `now`, because
the clock starts its runs. A deploy with no trigger is refused, saying how to run it by hand, and
none of a flow's triggers can be optional.

A few leaves the language doesn't write have items of their own: `NetworkEgress` (send outbound
messages), `SecretAccess` (read secrets) and `Spawn` (start processes).

## Exceptions

Three items lift a protection that is on by default. A deployment can only be granted them when the
server's policy allows them at all.

| Name | Shown as | |
|---|---|---|
| `PlainHttp("host")` | call host over plain HTTP (not encrypted) | the deploy settings' **plain HTTP to**; needs `allow_plain_http = true` |
| `PrivateAddresses` | reach private addresses (this machine and its network) | the deploy settings' **private addresses**; needs `allow_private_addresses = true` |
| `Unrecorded("GET /health")` | keep runs of GET /health out of its log, failed or not (they are counted) | a handler whose options say `log: "unrecorded"`; allowed unless `allow_unrecorded = false` |

`Unrecorded` is asked for by the flow itself, one item per handler whose options say `log: "unrecorded"`
([Language reference](../language/reference.md#clocks-files-and-actions)), whatever the handler
does. It is optional: without it, or once it is taken
back, the handler's runs are recorded again, from the next one, and the deployment runs on. Its
runs are counted (how many, how many failed, how long they took, the last one), and a failed one is
logged. A person approves it for the handler's code: when an agent's version changes that code, it
deploys with the item withheld, the handler's runs recorded, until a person grants it back.

Without `PrivateAddresses`, a call is refused when the host resolves to a private address
(loopback, the private ranges, link-local and cloud metadata addresses, and their IPv6 forms),
checked after DNS, on every redirect.

A redirect is followed only to a host the deployment may call. To another host than the first,
only `content-type`, `accept`, `accept-language` and `user-agent` go along: the flow's other
headers (`authorization`, an API key) stay with the host they were meant for.

## Limits

A deployment runs within limits. Each has a default, which is granted without asking, and a
ceiling, which can't be passed. A deployment that needs more than the default asks for
`Limit(name=n)`, shown as "raise *name* to *n*", from the deploy settings' **needs**, or from the
flow's own text: its `limits` (`flow("x", { limits: { run_budget_ms: "1m" } })`), or for the
log's two, its `keep` ([the language](../language/reference.md#imports-and-settings)). The policy
can change both columns
([Server configuration](server.md#the-policy)).

| Name | Is | Default | Ceiling |
|---|---|---|---|
| `connect_timeout_ms` | time to connect to a host | 10000 | 30000 |
| `request_timeout_ms` | time for an outbound request | 30000 | 300000 |
| `run_budget_ms` | time a request's run may take | 25000 | 900000 |
| `clock_run_budget_ms` | time a clock run, or a run on a new file, may take | 60000 | 900000 |
| `claim_wait_ms` | time a run waits for a key, row or file another run holds | 10000 | 60000 |
| `commit_wait_ms` | time a run's commit waits for the locks on files (or a folder, or a database) another run's commit holds | 2000 | 30000 |
| `max_requests` | outbound requests per run | 50 | 1000 |
| `per_host_concurrency` | outbound requests to one host at once | 6 | 32 |
| `max_request_bytes` | an outbound request's size | 10485760 (10 MiB) | 104857600 |
| `max_response_bytes` | an outbound response's size | 10485760 (10 MiB) | 104857600 |
| `max_header_bytes` | an outbound response's headers | 65536 | 262144 |
| `max_redirects` | redirects followed | 10 | 20 |
| `max_await_seconds` | the longest a run may wait at `await` | 2592000 (30 days) | 7776000 (90 days) |
| `max_waiting_runs` | runs waiting at once | 10000 | 100000 |
| `max_call_depth` | flows a chain of calls may go through, one calling another's action | 3 | 8 |
| `inline_file_bytes` | a file a run reads whole, or a range of one; a file a route answers with up to this is read before it is sent | 1048576 (1 MiB) | 16777216 |
| `max_file_read_bytes` | bytes a run reads from files | 67108864 (64 MiB) | 1073741824 |
| `max_file_write_bytes` | bytes a run writes and copies into files | 1073741824 (1 GiB) | 1099511627776 |
| `max_files_per_run` | files a run reads, takes a handle to, writes, copies or deletes | 1000 | 100000 |
| `max_served_bytes` | a file a route answers with (`f.serve(name)`) | 1073741824 (1 GiB) | 1099511627776 (1 TiB) |
| `module_fuel` | instructions a `js()` function may run per call | 200000000 per function, unless it says | 1000000000 |
| `module_memory` | memory a `js()` function may use per call | 16 MiB per function, unless it says | 256 MiB |
| `max_db_rows` | rows a run reads from databases | 10000 | 1000000 |
| `max_db_read_bytes` | bytes a run reads from databases | 16777216 (16 MiB) | 268435456 |
| `db_statement_ms` | time one SQL statement may take | 5000 | 60000 |
| `max_db_statements` | SQL statements a run sends, reads and writes | 100 | 10000 |
| `max_pack_bytes` | a pack a run sends on a channel, as JSON | 65536 (64 KiB) | 1048576 |
| `max_packs_per_run` | packs a run sends | 1000 | 100000 |
| `channel_keep` | packs each channel keeps for subscribers that ask for them | 100 | 10000 |
| `channel_buffer` | packs waiting for one subscriber before it is dropped as too slow | 256 | 4096 |
| `max_subscribers` | subscribers of one channel at once (a WebSocket subscribed to it is one) | 100 | 10000 |
| `max_websockets` | WebSocket connections the deployment holds at once | 100 | 10000 |
| `max_message_bytes` | a message a WebSocket connection sends | 65536 (64 KiB) | 1048576 (1 MiB) |
| `messages_per_second` | messages each WebSocket connection may send a second, in bursts of twice that | 10 | 1000 |
| `keep_runs` | runs the deployment's log keeps, the newest | 500 (`--keep-runs`) | 20000 |
| `keep_run_days` | days the deployment's log keeps a run | 3650 (any age) | 3650 |

A run past its time budget fails and performs nothing. An outbound call past a limit doesn't
fail the run: it answers `{status: 0, error, message}`, with `error` one of `timeout`, `connect`,
`dns`, `too_large`, `too_many_redirects`, `blocked_host`, `blocked_secret` (it holds a secret's value bound for a
host the flow's approval doesn't show it going to), `blocked_redirect` (a redirect to
another host that would resend the body), `blocked_address`, `blocked_scheme`,
`revoked`, and a few more. A `js()` function's budget is part of its manifest:

```
run code it carries (a js node): up to 200000000 instructions and 16 MiB per call, reaching nothing outside
```

## Optional items

`with { optional: "true" }` on a secret's import marks it as one the flow can run without:

```
import slack_webhook_path from "secret:slack_webhook_path" with { optional: "true" }
```

The deploy dialog marks it optional, and a deployment may be granted it or not. Without it, the
nodes that need it have no value. A trigger can't be optional.

## Who may read a flow's log lines

The lines a flow writes with `console.log` are kept beside its runs ([`holon logs`](cli.md#holon-logs)). A person
who may read the deployment reads them, in the console, in `holon editor` and with `holon logs`. An agent,
`holon mcp` and anything else that signs in as an agent, reads them only where the deployment's grant lets agents:
`AgentReadsLogs`, which the deploy settings' **agents may read its logs** asks for and a person approves at the
next deploy. Until then the route and `holon mcp`'s `logs` tool answer that agents may not read them, and revoking it
(`holon grant <slug> --revoke AgentReadsLogs`, or on the flow's Settings) refuses them again. The flow runs either way.

## Who may run an action

An action is never reached from the network: the flow's port answers only its routes. It runs
when someone runs it by its name, from the editor's [Try a request](../guides/editor.md#the-bottom-pane), the
[flow's page](../guides/editor.md#a-flows-page), or
`POST /trigger/<slug>/<name>` on the holon server, its body the JSON the action receives
(nothing is `{}`). On a server with accounts, an editor or an admin may run one, and a viewer may
not.

Another flow runs an action only through its own grant: a flow that says `charge(x)`, imported
from `"flow:billing"`, asks
for `RunsAction("billing/charge")`, and a person approves it with the rest of the flow, as one who
could run that action themselves (an editor or an admin). Revoking it pauses the calling flow like
any required item, or, with **keep running** ticked, makes its next call answer `refused`. The
other flow needs nothing from its side: its action runs as it would for a person, `by` the calling
flow ([Actions](../guides/actions.md#run-by-another-flow)).

An agent may run an action only when a person has let it:

| Name | Shown as | |
|---|---|---|
| `AgentRuns("name")` | let agents run its action name | the deploy settings' **agents may run**, one per action |
| `AgentReadsLogs` | let agents read its log lines | the deploy settings' **agents may read its logs**, for a flow that writes lines |

It is always optional: without it the flow runs as it would, and only an agent is refused
("agents may not run …"). Like any item, it is approved by a person, never by an agent, and can be
taken back while the deployment runs. An agent runs it with `holon mcp`'s `run` tool, or `POST
/trigger/<slug>/<name>` as an agent; it never runs a flow by hand that has no action.

## How an item is decided

At each deploy, every item of the manifest is:

1. **refused**, if the policy can never grant it (above a ceiling, or an exception the policy
   doesn't allow, or an `await` longer than `max_await_seconds`, or a folder that isn't bound);
2. **granted without asking**, if the policy's `auto` list has it, it is a limit at or below its
   default, or the deployment's last grant had it;
3. otherwise, **shown to a person**, who approves it or not. An agent can't.

At each effect, while it runs, the server checks the item is in the grant: an effect outside it
is refused. A grant can be narrowed while the deployment runs
([Changing a grant](../guides/deploying.md#changing-a-grant)).
