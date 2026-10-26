# Deploying and permissions

A deployment is a flow running on the server, with a grant: what a person approved it may do.
This guide covers deploying, approving, secrets, new versions and changing a grant while it
runs. [Concepts](../concepts.md#policy-grants-and-enforcement) explains the model.

## What a deploy does

Pressing **deploy** in the editor (or an agent's `deploy`):

1. compiles the flow into its unit, and works out its **manifest**, what it may do;
2. checks the manifest against the server's **policy**: anything the policy can never grant
   (a limit above its ceiling, plain HTTP the host doesn't allow) stops the deploy here, with
   "this host can't grant that: …";
3. sorts the rest into what is **granted without asking** (what the policy grants automatically,
   limits at or below the default, and what this deployment's last grant already had) and what
   **needs your approval**;
4. shows you both, and deploys when you approve.

The deployment takes the flow's name (its slug). A server flow's routes answer on its `port` at
once; a clock flow's first run is at once, then every interval; a flow `on file` runs for each
file that arrives from then on.

## The approval dialog

> **Deploy "stripe-to-slack"?**
> It needs your approval to:
> - read the clock
> - call https://hooks.slack.com
> - receive requests
> - read the secret "slack_webhook_path"
> - read the secret "stripe_webhook_secret"
>
> Not set yet: slack_webhook_path, stripe_webhook_secret (set under ⚙ → secrets; until then those
> nodes stay empty)

Read the list before you approve: it is everything the deployment will be allowed to do, and the
server refuses anything else when a run tries it. An item marked **(optional: it runs without
it)** can be left out (see [Optional permissions](#optional-permissions)). **Approve and deploy**
grants the list; **Cancel** changes nothing.

Only a person can approve. An agent's deploy that needs approval is saved instead, for you to
open and deploy ([Working with agents](agents.md)).

## Deploy settings (⚙)

**⚙** in the deploy bar opens the deployment's settings:

| Setting | |
|---|---|
| port | the port a server flow answers on (the text's `port` line) |
| web pages from | origins whose web pages may call the routes from a browser (CORS): `*`, or `https://site`; the text's `browsers` line |
| interval (s) | how often a clock flow runs (the text's `every` line) |
| needs | limits for this deployment, each blank for the host's default: timeouts, run budgets, requests per run, sizes, redirects, the longest `await`, runs waiting at once |
| plain HTTP to | hosts it may call over plain `http://` (only if the policy allows plain HTTP at all) |
| private addresses | whether it may call this machine and its network (only if the policy allows it) |
| permissions | which of its optional needs to grant |
| secrets | the secrets it reads, each set, replaced or removed |

A limit above the host's default needs approval; one above the host's ceiling can't be granted.
[Permissions](../reference/permissions.md#limits) lists the limits, their defaults and ceilings.

## Secrets

A flow names the secrets it reads (`secret.stripe_webhook_secret`); their values are set per
deployment, by an admin, under **⚙ → secrets**. The flow must have a name first. Each secret
has a source:

- **value**: typed in, sealed on the server (AES-256-GCM, its key in the OS key store by default).
  It is never shown again.
- **env**: read from an environment variable of the server, `HOLON_SECRET_<NAME>` (or a `.env`
  file given with `holon serve --dotenv`).
- **vault**: read from HashiCorp Vault (KV v2), as `mount/path#field`, with `VAULT_ADDR` and
  `VAULT_TOKEN` in the server's environment.

Env and vault secrets are read again every minute. A value must be at least six characters.

Each change makes a new version; **▸** shows the last five, and **roll back** makes an old one
current again (as a new version). The history says who changed what, never the values.

A secret's value never appears in the run log, the editor, an export or a replay: wherever it
would, it shows as `‹secret:name›`. A state write that contains a secret is refused. What the
flow answers its own caller is not redacted: if a flow puts a secret in its response, the caller
gets it, so don't.

Until a secret is set, the nodes that read it have no value, and what depends on them doesn't
run.

## Optional permissions

A flow can mark a need as optional in its headers, so it runs without it:

```holon
flow digest
port 8130
optional secret.slack_webhook_path

route POST /digest (req) {
  sent = http.post("hooks.slack.com", secret.slack_webhook_path, body: json({ text: req.body }))
  respond { posted: sent.status ?? "not configured" }
}
```

The approval dialog marks it optional, and **⚙ → permissions** grants it or not. Without it,
`secret.slack_webhook_path` has no value, the post isn't sent, and the route still answers.

## New versions

Any change to a flow is a new version. **update** deploys it in place:

- it asks only for what the deployment's grant doesn't already cover;
- state, collections and sessions are kept;
- runs parked at an `await` finish on the version they started on;
- each run records its version, and view mode shows a run on the canvas it ran on.

The last versions' canvases are kept for that. To go back to an earlier version, open or import
it and update again.

## Running, pausing, stopping

- **pause**: the deployment stops answering (its routes answer 503) and its clock stops; nothing
  is lost. **resume** carries on.
- **stop**: the same, and ends the session. **run** starts a new one, **from latest** (the state
  as it was), **fresh** (empty state) or **from session N** (the state a kept session ended with).
- **delete** (once stopped, on the deployed page) removes the deployment and its stored state.
  It can't be undone. `holon export` its runs first if you need them.

## Changing a grant

On the deployed page, a deployment's **permissions ▸** shows what is granted. While it runs, you
can narrow it:

- **revoke** an item: from now on, the server refuses that effect. Revoking something optional
  makes those parts absent, and the flow carries on. Revoking something required **pauses** the
  deployment, unless **keep running when something required is revoked** is ticked: then those
  calls fail and the rest runs. What starts its runs (receiving requests, its schedule, a
  folder's new files) always pauses it when revoked, ticked or not.
- **grant back** restores a revoked item.
- a limit can be **lowered**. Raising one needs a new deploy, and approval.

Each change is logged, with who made it, and each run records the grant it ran under.

## The host's policy

What may ever be granted, and what is granted without asking, is the server's policy:
`holon.toml` in its data folder. It isn't edited in the editor. See
[Server configuration](../reference/server.md#the-policy).
