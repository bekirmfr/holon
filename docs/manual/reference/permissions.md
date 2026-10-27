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
| `NetworkIngress` | receive requests | a `route`; in a flow without handlers that reads `ingress`, shown as "can be run by hand" |
| `Clock` | read the clock | `now`, or `every` (a clock starts runs) |
| `Action("name")` | can be run by hand: name | `action name (input) { … }` |
| `Random` | use randomness | `random` |
| `HttpRequest("host")` | call https://host | `http.get("host", …)`, `http.post(…)`, …; the host is written in the text |
| `SecretRead("name")` | read the secret "name" | `secret.name` |
| `StateRead("key")` | read its state "key" | `state.key` read |
| `StateWrite("key")` | write its state "key" | `state.key = …` |
| `CollectionRead("c")` | read items of its collection "c" | `c[k]`, `c.list(…)` |
| `CollectionWrite("c")` | put and delete items of its collection "c" | `c[k] = …`, `delete c[k]` |
| `FileRead("f")` | read files in the folder "f" | `f[name]`, `f.list`, `f.file`, `f.lines`, `f.slice`, or copying a file from `f` |
| `FileWrite("f")` | write, copy into and delete files in the folder "f" | `f[name] = …`, `delete f[name]`, `f.copy(…)` |
| `DbRead("d/t")` | read the table "t" of the database "d" | `d.query("select … from t …")`, `d.one(…)` |
| `DbWrite("d/t")` | insert, update and delete rows of the table "t" of the database "d" | `d.exec("update t …")` |
| `FileArrival("f")` | run on each file new, changed or removed in the folder "f" (as its on file lines say), and pass it on | `on file f` |
| `Await("e")` | wait for an event "e" and continue later | `await e` |

A flow's triggers, what start its runs, are among them: `NetworkIngress` for its routes, `Clock`
for an `every` block (or an `every` header), `FileArrival` for each folder an `on file` watches,
`Action("name")` for each action, and `NetworkIngress` for a flow without handlers that reads
`ingress`, run by hand. A flow with a clock asks for `Clock` even if it never reads `now`, because
the clock starts its runs. A deploy with no trigger is refused, saying how to run it by hand, and
none of a flow's triggers can be optional.

A few leaves the language doesn't write have items of their own: `NetworkEgress` (send outbound
messages), `SecretAccess` (read secrets) and `Spawn` (start processes).

## Exceptions

Two items lift a protection that is on by default. A deployment can only be granted them when the
server's policy allows them at all.

| Name | Shown as | |
|---|---|---|
| `PlainHttp("host")` | call host over plain HTTP (not encrypted) | the deploy settings' **plain HTTP to**; needs `allow_plain_http = true` |
| `PrivateAddresses` | reach private addresses (this machine and its network) | the deploy settings' **private addresses**; needs `allow_private_addresses = true` |

Without `PrivateAddresses`, a call is refused when the host resolves to a private address
(loopback, the private ranges, link-local and cloud metadata addresses, and their IPv6 forms),
checked after DNS, on every redirect.

## Limits

A deployment runs within limits. Each has a default, which is granted without asking, and a
ceiling, which can't be passed. A deployment that needs more than the default asks for
`Limit(name=n)`, shown as "raise *name* to *n*", from the deploy settings' **needs**. The policy
can change both columns ([Server configuration](server.md#the-policy)).

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
| `module_fuel` | instructions a js holon may run per call | 200000000 per holon, unless it says | 1000000000 |
| `module_memory` | memory a js holon may use per call | 16 MiB per holon, unless it says | 256 MiB |
| `max_db_rows` | rows a run reads from databases | 10000 | 1000000 |
| `max_db_read_bytes` | bytes a run reads from databases | 16777216 (16 MiB) | 268435456 |
| `db_statement_ms` | time one SQL statement may take | 5000 | 60000 |
| `max_db_statements` | SQL statements a run sends, reads and writes | 100 | 10000 |

A run past its time budget fails and performs nothing. An outbound call past a limit doesn't
fail the run: it answers `{status: 0, error, message}`, with `error` one of `timeout`, `connect`,
`dns`, `too_large`, `too_many_redirects`, `blocked_host`, `blocked_address`, `blocked_scheme`,
`revoked`, and a few more. A js holon's budget is part of its manifest:

```
run code it carries (a js node): up to 200000000 instructions and 16 MiB per call, reaching nothing outside
```

## Optional items

`optional` in a flow's headers marks an item it can run without:

```
optional secret.slack_webhook_path
```

The deploy dialog marks it optional, and a deployment may be granted it or not. Without it, the
nodes that need it have no value. A trigger can't be optional.

## Who may run an action

An action is never reached from the network: the flow's port answers only its routes. It runs
when someone runs it by its name, from the editor's [server panel](../guides/editor.md#the-server-panel), the
[deployed page](../guides/editor.md#the-deployed-page), or
`POST /trigger/<slug>/<name>` on the holon server, its body the JSON the action receives
(nothing is `{}`). On a server with accounts, an editor or an admin may run one, and a viewer may
not. A flow without handlers that reads `ingress` runs the same way, by `POST /trigger/<slug>`.

An agent may run an action only when a person has let it:

| Name | Shown as | |
|---|---|---|
| `AgentRuns("name")` | let agents run its action name | the deploy settings' **agents may run**, one per action |

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
