# Concepts

This page explains the ideas the rest of the manual relies on: what a flow is, how Holon knows
what it may do, how that is enforced, and what is kept of every run.

## Flows

A **flow** is a small program: a server's routes, a job on a clock, or a run for each new file
in a folder. It is written in the Holon language, or drawn on the editor's canvas. The two are
the same thing: the text compiles to a graph of **nodes** joined by **wires**, the canvas draws
that graph, and a change in either one is a change to the graph.

```holon
flow greet
port 8102

route POST /greet (req) {
  name = req.json.name ?? "stranger"
  respond { greeting: "Hello, {name}" }
}
```

Most nodes are **leaves** from a fixed catalog: text, numbers, lists, records, JSON, hashing,
signatures, comparison ([the leaves](language/leaves.md)). They are pure: given the same inputs,
they always give the same output, and they touch nothing outside. The few nodes that touch the
outside world (the request, the clock, a secret, state, an HTTP call, a file) sit at the edges of
the graph, and each of them is a **capability** the flow uses.

A **holon** is a piece of flow with named inputs and outputs, used as one node. A whole flow can
be placed inside another as a holon, and the name "Holon" comes from that: every part is itself
a whole.

### Every run ends

There are no loops and no recursion. Lists are handled with `map` and `fold`, and `range` makes a
list of at most 10,000 numbers. A JavaScript holon runs under a fuel budget. So every run ends,
and a deployment's limits (how long a run may take, how many calls it may make) are the only
thing that can stop one early.

### Absence instead of errors

A value that doesn't arrive (a missing query parameter, a key that isn't stored, a signature that
doesn't verify) is **absent**, and everything computed from it is absent too. Nothing throws.
A route then answers its `missing` status (404 unless it says otherwise), a write that needed the
value doesn't happen, and a call whose body needed it isn't sent. Choosing a fallback is explicit:
`a ?? b`.

## Units: what a flow is

Every node, holon and flow is a **unit**, identified by the hash of what it is: its leaves, its
wiring, its parameters. Two flows that compile to the same graph have the same hash, however
they were written; any change, however small, is a new hash.

The hash is the flow's **version**. A deployment runs one version, every run records the version
it ran on, and a holon placed in a flow is pinned to its version until you update it.

## The manifest: what a flow may do

Because every capability is a node, Holon can list what a flow may do by reading its graph. That
list is the flow's **manifest**, and `holon check` prints it as `it may:`:

```
it may:
  read the clock
  call https://hooks.slack.com
  receive requests
  read the secret "slack_webhook_path"
  read the secret "stripe_webhook_secret"
```

Nobody writes the manifest, so nobody can understate it. A flow can't call a host without the host
being written in its text; it can't read a secret without naming it; it can't write a state key it
didn't name. Whatever a flow places inside it (a holon, a marketplace listing) adds its own needs
to the list.

A flow can mark some needs **optional** (`optional secret.x`): it runs without them, and the parts
that need them stay absent.

## Policy, grants and enforcement

Three things decide what a deployment may actually do:

1. **The host's policy** (`holon.toml`, set by whoever runs the server) says what may ever be
   granted on this machine, and which things a person must approve: the ceiling on each limit,
   whether plain HTTP or private network addresses may ever be granted, and what is granted
   without asking.
2. **The grant.** When a flow is deployed, Holon shows its manifest against the policy and asks a
   person to approve whatever needs approving. What they approve is the deployment's grant.
   Redeploying a new version asks only for what the old grant didn't cover.
3. **Enforcement.** The grant is checked at the moment an effect is performed: when a call is
   about to be sent, a secret read, a state key written. An effect outside the grant is refused,
   whatever the flow says, and the grant is checked again whenever the server loads.

A grant can be narrowed while the flow runs. Revoking something required pauses the deployment
(or, if you choose, keeps it running with those calls failing); revoking something optional just
makes those parts absent. See [Deploying and permissions](guides/deploying.md) and the full list
of [permissions](reference/permissions.md).

## Deployments

A **deployment** is a flow running on a server, under the flow's name (its **slug**). It has:

- a **trigger**: its own HTTP server on a port, a clock interval, or a folder whose new files it
  runs on;
- a **grant**, and the **secrets** set for it;
- its **state**: the values under `state.x`, and its collections;
- its **run log**.

A deployment is **running**, **paused** (requests get 503, and nothing is lost) or **stopped**.
Starting a stopped deployment begins a new **session**, from its latest state or fresh.

## Runs

A **run** is one evaluation of the flow: one request answered, one clock tick, or one new file. A run:

- **reads** everything from outside at its start: the request, the clock, randomness, the state
  it uses. A run never sees its own writes.
- **evaluates** the graph. Only what is needed is computed: the branch of an `if` or `match` not
  taken is never evaluated, so its calls aren't sent.
- makes its **outbound calls** between rounds of evaluation, and records each response.
- **writes** at its end: state, collections and files are committed together, or not at all.

Runs of one deployment can happen side by side. A run that read a collection key someone else
wrote meanwhile is run again, so none is lost.

A run can also **wait**: `await` parks it, sometimes for days, until a later request delivers the
event it waits for, or it expires. A parked run keeps the version it started on.

## The run record

Each deployment keeps a log of its runs. A run's entry holds everything it read from outside (the
request, the clock, randomness, the state it read, every response it got) and everything it did,
with the version and the grant it ran under.

Evaluation is pure, so this is enough to **replay** a run exactly, node by node: the editor's view
mode does it to show you the value each node made, and says whether the replay reproduces the run.

The log is a **hash chain**: each entry includes the hash of the one before, and the newest hash
is the log's **head**. A run changed or taken out afterwards breaks the chain. `holon export`
writes a deployment's runs to one file, and `holon verify` checks the chain and runs every run
again offline, through the same core, on any machine. A **witness** (another server that is sent
each head as it is made) makes the log tamper-evident even to someone who holds the server's
data. See [Checking runs](guides/checking-runs.md).

Secrets never enter the log: their values are redacted from everything it holds. A run that used a
secret can be shown, but not run again offline.

## People, roles and agents

Everyone signs in to the server. Each account has a **role**:

| Role | May |
|---|---|
| viewer | read flows, deployments and logs |
| editor | also save, deploy, run and delete |
| admin | also write secrets, bind folders and manage accounts |
| agent | read, propose flows and call them; never approve, start, stop or delete |

An **agent** (through `holon mcp`) writes flows, checks them, saves them and deploys them, but a
deploy that needs approval stops at a person. Scripts and CI use **API tokens**, each with a role.
See [Working with agents](guides/agents.md).
