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
once; a clock (an `every` block, or a clock flow's `every` header) first runs at once, then every
period; a file handler (`on file`) runs for each file that arrives from then on; an action runs
when someone runs it ([Actions](actions.md)). A flow with none of these, and no route, is refused:
it would start nothing.

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
| interval (s) | how often a clock flow runs (the text's `every` line). A flow with `every` blocks has its periods in the text and shows them here instead, `clocks every 2s`: it takes no interval |
| needs | limits for this deployment, each blank for the host's default: timeouts, run budgets, requests per run, sizes, redirects, the longest `await`, runs waiting at once |
| plain HTTP to | hosts it may call over plain `http://` (only if the policy allows plain HTTP at all) |
| private addresses | whether it may call this machine and its network (only if the policy allows it) |
| permissions | which of its optional needs to grant |
| agents may run | for a flow with actions, a box for each: ticked, an agent may run it, which asks for `AgentRuns("name")` ([Who may run an action](../reference/permissions.md#who-may-run-an-action)) |
| secrets | the secrets it reads, each set, replaced or removed |

A limit above the host's default needs approval; one above the host's ceiling can't be granted.
[Permissions](../reference/permissions.md#limits) lists the limits, their defaults and ceilings.

## Secrets

A flow names the secrets it reads (`secret.stripe_webhook_secret`); their values are set per
deployment, by an admin, under **⚙ → secrets**. The flow must have a name first. Each secret
has a source:

- **value**: typed in, sealed on the server (AES-256-GCM, its key in the OS key store by default).
  It is never shown again.
- **env**: read from an environment variable of the server, `HOLON_SECRET_<NAME>`, or from a
  `.env` file the server reads (see [Secrets from a `.env` file](#secrets-from-a-env-file)).
- **vault**: read from HashiCorp Vault (KV v2), as `mount/path#field`, with `VAULT_ADDR` and
  `VAULT_TOKEN` in the server's environment.

Env and vault secrets are read again every minute. A value must be at least six characters.

Each change makes a new version; **▸** shows the last five, and **roll back** makes an old one
current again (as a new version). The history says who changed what, never the values.

A secret's value never appears in the run log, the editor, an export or a replay: wherever it
would, it shows as `‹secret:name v3 #a1b2c3d4›`: the name, the version the value is of, and a
fingerprint of it (keyed by the server's master key, so a guess can't be checked against the log).
That tells which value a run had (the key you set an hour ago or the one before, the same token in
two runs) without the log holding it. A token an API answers a request that held a secret with
(an OAuth exchange's `access_token`) is kept out of the log the same way, and a write of it is
refused too. A state write that contains a secret is refused. What the
flow answers its own caller is not redacted: if a flow puts a secret in its response, the caller
gets it, so don't.

Until a secret is set, the nodes that read it have no value, and what depends on them doesn't
run.

### Secrets from a `.env` file

A `.env` file keeps your keys in a file on the server's machine, outside Holon. You can add or
change one without restarting the server.

**Where it goes.** Anywhere you like: the server reads only the file you name when you start it.
`~/.holon/secrets.env` (on Windows, `C:\Users\<you>\.holon\secrets.env`) keeps it with Holon's
other files and out of any repository. Give the path in full: a relative one is read from the
folder the server was started in.

**What goes in it.** One `HOLON_SECRET_<NAME>=value` line per key:

```
# Gemini, for the gemini-chat sample
HOLON_SECRET_GEMINI_API_KEY=AIza...
HOLON_SECRET_SLACK_WEBHOOK_URL="https://hooks.slack.com/services/..."
```

- Blank lines and lines starting with `#` are skipped, and so is a line without `=`.
- An `export ` in front of a line is allowed.
- A value in matching single or double quotes is taken as it is, `#` included. An unquoted value
  ends at ` #` (a space, then `#`).
- Only `HOLON_SECRET_…` names can be used by a flow's secret. The file may hold other lines
  (`VAULT_ADDR` and `VAULT_TOKEN` for vault secrets are read from it too), but no secret can name
  them.

**Starting the server with it.** This is the only restart you need:

```bash
holon serve --dotenv ~/.holon/secrets.env
```

Add the options you usually start it with. If something else starts the server for you (a
shortcut, a startup task, a service), add `--dotenv …` there. A file that doesn't exist yet reads
as empty, so you can name it before you create it.

**Pointing a secret at it.** Under **⚙ → secrets**, choose **env** for the secret, then the
variable from the list: every `HOLON_SECRET_…` name in the file and in the server's environment,
never their values. The one the secret's name suggests is chosen for you
(`HOLON_SECRET_GEMINI_API_KEY` for `secret.gemini_api_key`: the name in capitals, after
`HOLON_SECRET_`). Opening ⚙ reads the file again, so a line you just added is in the list. Choose
**another name…** to type one the list doesn't have. The list is shown to admins only, as only an
admin sets secrets.

**How it is served.**

- The server reads the file when it starts, then again every minute. An edit reaches the runs
  within a minute.
- If the editor answers "HOLON_SECRET_… isn't set in the server's environment or its dotenv
  file", the server hasn't read your edit yet: close ⚙ and open it again, which reads the file,
  and set it again.
- If the same name is also an environment variable of the server, that one wins, as with other
  `.env` tools.
- Holon keeps only the reference (`env:HOLON_SECRET_GEMINI_API_KEY`), never the value: the value
  stays in your file, and in the server's memory while it runs.
- A run gets the value as it was at the last read. It never shows in the log, the editor or an
  export (`‹secret:name›`).
- If you remove a line, or the file can't be read, runs keep the last value for five minutes at
  most, and the secrets list says why. After that, the secret has no value until it can be read
  again. To revoke a key, take it out of the file, and also revoke it where it was issued.
- A value shorter than six characters isn't used, as with a typed-in one.

**Keeping it safe.** The file holds your keys in plain text: keep it readable only by you (or by
the account the server runs as), and out of any repository. The server counts the file among its
own: a folder that holds it can't be bound as a [named folder](files.md), so no flow can read it
as a file.

For values that should never sit in a file on the server, use a typed-in value (sealed on the
server) or **vault**.

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

Every deploy is a version, numbered from 1 and kept for good: its unit, its canvas, when and by
whom, and what its deploy asked. `GET /deployments/<slug>/versions` lists them.

### Going back to an earlier version

`holon restore <slug>` lists the versions; `holon restore <slug> --to N --code` deploys version
N's code again. It is a new version that names the one it restores, never a rewrite: the history
keeps the version it replaces, which can be restored in turn.

- **Checked against today's grant**, not the one it had: a secret revoked since stays revoked, and
  what it needs that isn't granted now is listed. `--approve` approves exactly that list; without
  it nothing changes. `--dry-run` says what it would ask.
- **Its state stays as it is.** Only the code goes back; the state, collections and sessions are
  the deployment's as they are now. To start from an earlier state, stop the deployment and run it
  **from session N**.
- **Versions deployed before rc.30** open on their canvas, but don't restore: what their deploys
  asked wasn't kept. Open or import one and update again.
- Restoring is an editor's or an admin's: an agent can't (`POST /deployments/<slug>/restore`,
  `{"version": N, "code": true}`, with `"dry_run": true` or `"acknowledge": […]`).

The sample **price-rollback** walks through it.

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
  calls fail and the rest runs. What starts its runs always pauses it when revoked, ticked or
  not: the first it has of receiving requests, a folder's new files, its schedule and an action.
  A flow with several kinds of handler (routes and a clock, say) has the others' triggers as
  ordinary required items.
- **grant back** restores a revoked item.
- a limit can be **lowered**. Raising one needs a new deploy, and approval.

Each change is logged, with who made it, and each run records the grant it ran under.

## The host's policy

What may ever be granted, and what is granted without asking, is the server's policy:
`holon.toml` in its data folder. It isn't edited in the editor. See
[Server configuration](../reference/server.md#the-policy).
