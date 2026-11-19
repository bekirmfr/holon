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
once; a clock (`every`) first runs at once, then every period; a file handler
(`on.file.created`) runs for each file that arrives from then on; an action runs
when someone runs it ([Actions](actions.md)). A flow with none of these, and no route, is refused:
it would start nothing.

## The approval dialog

> **Deploy "stripe-to-slack"?**
> It needs your approval to:
> - read the clock
> - call https://hooks.slack.com
> - receive requests
> - read the secret "slack_webhook_path", and send it to https://hooks.slack.com
> - read the secret "stripe_webhook_secret"
>
> What it touches (+ new, - no longer, ~ changed, ! not ready on this server):
> What starts it
> &nbsp;&nbsp;+ requests: receive — approved by confirming
> Folders, databases and secrets
> &nbsp;&nbsp;! secret slack_webhook_path: read, and send it to https://hooks.slack.com — not set: the steps that read it get nothing until it is
> &nbsp;&nbsp;! secret stripe_webhook_secret: read — not set: the steps that read it get nothing until it is
> What it reaches outside
> &nbsp;&nbsp;+ host hooks.slack.com: call — approved by confirming
> Also
> &nbsp;&nbsp;+ the clock: read — approved by confirming

Read the list before you approve: it is everything the deployment will be allowed to do, and the
server refuses anything else when a run tries it. An item marked **(optional: it runs without
it)** can be left out (see [Optional permissions](#optional-permissions)). **Approve and deploy**
grants the list; **Cancel** changes nothing.

**What it touches** is the same list by thing rather than by permission, beside what the
deployment touches now. A new version of a deployed flow begins with **Its handlers**: each
handler whose code it changes (**~**), adds (**+**) or removes (**-**), by its label (`GET /a`, `every 1m`). Then
what starts it (requests, actions, waits), the folders, databases and
secrets it is given, what it reaches outside (each host it calls, the flows whose actions it runs,
its channels, processes, the sites whose pages it answers), what it keeps (each state key and
collection), and its limits. Each thing is one row, marked **+** new, **-** no longer (the new
version doesn't touch it), **~** changed (some of its uses are new or gone, each marked), or **·**
as now. A row marked **!** is one this server isn't ready for, and says why: a secret not set, a
folder or database not bound here (or bound read-only, and the flow writes it), a flow it calls
that isn't deployed here, or whose action you may not run yourself (a call runs it under that
flow's grant, with what this flow sends, so someone who may run it deploys the version), a host or
limit this server's policy refuses, an approval whose steps no one here can take (`approver: 1
person holds it here; it needs 2`, or `a run pat starts can't be decided`: whoever starts a run,
and their party, decide none of its steps but one that says `starter: true`, nor, where the host
says so, whoever deploys the version and theirs; one party takes one step, an agent or a service
counted with whoever owns it or made its token), or that this host refuses (a step for an agent where it lets no agent
decide, a `starter` step where it lets no step take whoever started the run). When no
run could be decided there, the editor asks again before deploying it, naming each such
approval, and its runs wait there until someone is bound to take each step; when only the runs
someone starts couldn't be (`a run pat starts…`), it says so and doesn't ask: a run a route, a
schedule or a file starts has no starter, and can be decided. A row marked **+** that says
*asked again* is granted now, and asked of a person again: an agent changed the code that uses
it. A row that is ready says what the server has for it: a secret's version and where it comes from (`set, v3
(env:HOLON_SECRET_SIGNING_KEY)`), a folder or a database bound. Below the rows, **Its settings**
lists the deploy settings that change: how often it runs, the sites whose pages it answers, the
keys it keeps out of its records (`taint`) and those it never keeps (`never_keep`), its port, each
handler's log level.
`holon editor`, `holon deploy` (when it needs approval), `holon restore --dry-run` and the agent
tools show the same rows, worked out by the server.

Only a person can approve. Each version an agent deploys with its own token is saved instead,
for you to read and deploy ([Working with agents](agents.md)); an agent whose shell runs as you on
the server's machine signs in as you, though ([An agent with a
shell](agents.md#an-agent-with-a-shell)). What you read is what deploys: the server refuses
a proposal whose text builds another version than the one proposed, or whose `browsers` line
isn't the sites it sends.

## Deploy settings

**Settings**, in the editor's bottom pane (**b**), are the deployment's settings:

| Setting | |
|---|---|
| port | the port a server flow answers on (the text's `port` line) |
| web pages from | origins whose web pages may call the routes from a browser (CORS): `*`, or `https://site`; the text's `browsers` line |
| clocks | its `every` handlers' periods, `every 2s`, as the text says them (shown, not set here) |
| needs | limits for this deployment, each blank for the host's default: timeouts, run budgets, requests per run, sizes, redirects, the longest `await`, runs waiting at once |
| plain HTTP to | hosts it may call over plain `http://` (only if the policy allows plain HTTP at all) |
| private addresses | whether it may call this machine and its network (only if the policy allows it) |
| permissions | which of its optional needs to grant |
| agents may run | for a flow with actions, a box for each: ticked, an agent may run it, which asks for `AgentRuns("name")` ([Who may run an action](../reference/permissions.md#who-may-run-an-action)) |
| agents may read its logs | for a flow that writes lines with `console.log`: ticked, an agent may read them, which asks for `AgentReadsLogs` ([Who may read a flow's log lines](../reference/permissions.md#who-may-read-a-flows-log-lines)) |
| secrets | the secrets it reads, each set, replaced or removed |
| keeps out of its records | the keys it taints, beside the four every flow does; the text's `taint` line ([Tainted keys](../language/reference.md#imports-and-settings)) |
| never keeps | the keys it never keeps, guarded where they stand; the text's `never_keep` line ([Keys never kept](../language/reference.md#imports-and-settings)) |

A limit above the host's default needs approval; one above the host's ceiling can't be granted.
[Permissions](../reference/permissions.md#limits) lists the limits, their defaults and ceilings.

In `holon editor`, the terminal one, a flow's page has the same settings: its **Settings**
section lists them, **enter** changes the one picked and **→** says what it means; its **Access**
section has the permissions, granted or taken back, and the secrets.

## Secrets

A flow imports the secrets it reads (`import stripe_webhook_secret from
"secret:stripe_webhook_secret"`); their values are set per deployment, by an admin, under **Settings →
secrets**. The flow must have a name first. Each secret has a source:

- **value**: typed in, sealed on the server (XChaCha20-Poly1305, under keys whose root is in the OS key store by default on Windows and macOS, in `~/.holon/keys` elsewhere).
  It is never shown again.
- **env**: read from an environment variable of the server, `HOLON_SECRET_<NAME>`, or from a
  `.env` file the server reads (see [Secrets from a `.env` file](#secrets-from-a-env-file)).
- **vault**: read from HashiCorp Vault (KV v2), as `mount/path#field`, with `VAULT_ADDR` and
  `VAULT_TOKEN` in the server's environment.

Env and vault secrets are read again every minute. A value must be at least six characters.

Each change makes a new version; **▸** shows the last five, and **roll back** makes an old one
current again (as a new version), once a dry run has said which version comes back, as what, and
that the current one is kept (`POST …/secrets/<name>/rollback` with `"dry_run": true` says it and
changes nothing). Its **diff** tells the two values apart by their fingerprints, as the log marks
them (`v2 #a1b2c3d4 → v4 #e5f6a7b8`). The history says who changed what, never the values.

A run that waits (`await event`, `await approval`) and goes on after its secret changed runs
again from its start, and recognises the requests it made before the wait that held the secret as
it is (in a header, a URL or a JSON body): they are answered from its record, not sent again, and
the requests after the wait go with the new value. For example, a flow that posted "authorize"
with the key in a header, then waited, posts only "capture", with the new key. A request that
held a value the flow computed from the secret (a signature, a Basic credential, another
encoding) isn't recognised, and is sent again with the new value: for a flow that does that,
change the secret when no run waits. This is so for a value set with `holon secret set`, and for
an env or vault secret whose value changes while the run waits. `holon runs <slug> --waiting`
lists the runs that wait.

A secret's value never appears in the run log, the editor, an export or a replay: wherever it
would, it shows as `‹secret:name v3 #a1b2c3d4›`: the name, the version the value is of, and a
fingerprint of it (keyed by the server's master key, so a guess can't be checked against the log).
That tells which value a run had (the key you set an hour ago or the one before, the same token in
two runs) without the log holding it. A token an API answers a request that held a secret with
(an OAuth exchange's `access_token`) is kept out of the log the same way, and a write of it is
refused too. A state write that contains a secret is refused. A route whose answer carries a
secret, as it is or computed from it (`return token`, `return base64(token)`),
asks for a permission of its own, `SecretAnswer("token")`: "answer its callers with the secret
"token"", which a person approves at deploy, apart from reading it. A hash or HMAC of a secret
isn't one. So does a route that reads its flow's own data by a secret: a collection's item under
a key computed from it, a file it names (`notes[key.slice(0, 3)]`). Whether it found one, waited
on another run for it (503) or failed (500) tells the caller something of the secret, whatever
the route returns. A deployment without that grant answers such a request 500, and its log says why; so
does one whose answer holds a secret's value it didn't compute (a service echoing a header back).

When a service sends your secret back in its reply, what your flow reads from that reply is
treated as the secret too. A number, a true or false, or a text shorter than six characters read
from it passes as before (a balance, a count, a currency code). A longer text read from it is
refused to callers (500) and to storage (the write doesn't land), even a harmless one such as a
customer's name, and the run's log says so, which service sent the secret back, and what to do. The server decides this by where the
value came from and what kind of value it is, never by looking for the secret's characters in
it, so nobody calling your flow can learn anything of the secret from which answers are refused.
The ways on: answer with a number or a shorter value; stop the service sending the secret back
(most have a setting, or another endpoint); or, where callers really may have that text, give
the flow the permission to answer with that secret. A flow asks for that permission where one
of its routes answers with the secret, so today that last way means the flow says so in its
text. What stays open: a piece shorter than six characters; a number worked out from a piece;
a flow that itself compares the reply with what its caller sent and answers yes or no; and a
reply that holds only a hint of the secret, never all of it, which isn't noticed.

The approval also says where each secret goes. A secret that reaches a request's path, headers or
body, as it is or computed from it, is listed with the hosts it is sent to: "read the secret
"slack_webhook_path", and send it to https://hooks.slack.com". A secret passed as a database
read's value (`vendor.one("… where license_key = $1", license_key)`) goes to the
database's server with the query: "…, and send it to the database "vendor"". The rows a read
answers are what the database said, as a request's response is what the far side said, so
answering with them isn't answering with the secret. That leaves the caller whatever the query
makes of it: a route that reads a database with a secret answers with the rows it found by it,
or none, or fails as the database fails with it, and asks nothing more for that; approving the
secret's way to the database approves what comes back. What a call answers to a request that
carried a secret (the access token of a token exchange, a service's echo) is followed too, and
where it goes is said after: "read the secret "client_secret", and send it to
https://demo.duendesoftware.com; what is answered to it goes to its callers and
https://httpbin.dev" (the **client-credentials** sample). "Its callers" is there because the
route answers with what came back: a token handed to whoever calls is as good as the secret
until it expires, and what an API or a database says to a request the secret opened is what the
secret unlocks. The graph can't tell a token from the rest of an answer, so it says this of any
of it (a route that returns a chat API's reply, a licence's row) but a response's `status`
alone. A hash or a signature made
with a secret (`sha256`, `hmac_sha256`) isn't the secret either, and a flow must be free to sign
what it sends; but a route that answers `hmac_sha256(key, req.body)` signs whatever its caller
sends, so where one goes is said the same way: "read the secret "signing_key"; a hash or
signature made with it goes to its callers" (the **sign-payload** sample), or "… goes to
https://api.example.com" for a signed request. A route that only says whether a signature matches
hands out none, and says nothing. Read these lines as you read "send it to": nothing is refused,
and it is yours to decide whether the callers of that route should hold what it gives them. None
of this is a permission of its own
(the hosts are, and the graph fixes which secret goes to which before the flow runs); it is said
there so you see it when you approve, and when you update a flow whose version moves a key to
another host. `holon check`, `holon deploy` and the editor's hover on a secret say the same.
The host holds a run to it: a request that holds a secret's value bound for a host the secret
isn't shown going to (a key a service echoed back, passed on elsewhere) isn't sent, and answers
`{status: 0, error: "blocked_secret", message}` naming the secret. A secret is never handed to
another flow: a call to a flow's action whose input holds one, or something computed from one,
isn't made, and answers `{error: "refused", message}`. So no approval says "send it to the
flow …". `holon verify` checks it from
an export (see [Checking runs](checking-runs.md)).

Until a secret is set, the nodes that read it have no value, and what depends on them doesn't
run.

### The server's own secrets

One key for several flows is bound once, for the server, as a folder or a database is: under
**admin → resources → secrets**, or `holon secret bind stripe_key`. Every deployment that reads a
secret of that name and has none of its own reads the server's; its Settings say "the server's", and
setting one there overrides it for that deployment.

An admin can say where the server's may go: hosts and databases (`holon secret bind stripe_key
--to api.stripe.com`, or **where…** on its row). A deploy whose flow sends it anywhere else is
refused before anything is approved: "the server's secret "stripe_key" may go only to
https://api.stripe.com, and this flow sends it to https://hooks.slack.com". A deployment with its
own isn't held to it, since the key isn't the server's. Narrowed later (`holon secret cap`), it
holds from the next request: a run's request with the server's value to a host outside it isn't
sent, and answers `blocked_secret`. Setting, capping and removing one is audited, never its value.
The **shared-key** sample reads one.

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

**Pointing a secret at it.** Under **Settings → secrets**, choose **env** for the secret, then the
variable from the list: every `HOLON_SECRET_…` name in the file and in the server's environment,
never their values. The one the secret's name suggests is chosen for you
(`HOLON_SECRET_GEMINI_API_KEY` for `secret:gemini_api_key`: the name in capitals, after
`HOLON_SECRET_`). Opening Settings reads the file again, so a line you just added is in the list. Choose
**another name…** to type one the list doesn't have. The list is shown to admins only, as only an
admin sets secrets.

In `holon editor`, the terminal one, the same choice is on the flow's page: its **Access**
section lists the secrets it reads, and **enter** on one asks where its value comes from (a
**value** typed as dots, one of the server's **env** names, the one its name suggests picked
first, **another name…**, or a **vault** field). Holon never writes the `.env` file: add the line
there yourself, and it is in the list within a minute.

**How it is served.**

- The server reads the file when it starts, then again every minute. An edit reaches the runs
  within a minute.
- If the editor answers "HOLON_SECRET_… isn't set in the server's environment or its dotenv
  file", the server hasn't read your edit yet: leave Settings and open it again, which reads the file,
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

A flow can mark a secret as optional in its import, so it runs without it:

```ts
import { flow, route } from "holon"
import slack_webhook_path from "secret:slack_webhook_path" with { optional: "true" }
import hooks from "host:hooks.slack.com"

flow("digest", { port: 8130 })

route.post("/digest", async (req) => {
  const sent = await hooks.post(slack_webhook_path, { body: JSON.stringify({ text: req.body }) })
  return { posted: sent.status ?? "not configured" }
})
```

The approval dialog marks it optional, and **Settings → permissions** grants it or not. Without it,
`slack_webhook_path` has no value, the post isn't sent, and the route still answers.

## New versions

Any change to a flow is a new version. **update** deploys it in place:

- it asks only for what the deployment's grant doesn't already cover;
- state, collections and sessions are kept;
- runs parked at an `await` finish on the version they started on;
- each run records its version, and view mode shows a run on the canvas it ran on.

Every deploy is a version, numbered from 1 and kept for good: its unit, its canvas, when and by
whom, what its deploy asked, and the state it was deployed with. `GET /deployments/<slug>/versions`
lists them. Every run keeps the state it left, too: its line in the log names it.

### Replayed on recent runs

Before a new version goes live, the approval dialog and the version's review (**Approvals** →
**Review**) replay it on the deployment's last 20 runs: each run again, whatever started it (a
request, a WebSocket's message, a tick, a file, a mail, a feed's event, a queue's message, an
action, a waiting run's later part), with the state, clock and random that run had, through the
new version's handler for it. The card
**On recent runs** says how it went, `20 replayed · 17 same · 3 differ`, and for each run that
differs, how: the status it answers where it answered another (`it answers 422 where it answered
200`), its answer, the effects it performs (`StateWrite("count") 2 where 1`), where it waits.
The **Price rules** sample is a flow to try it with.

A replay is offline: nothing is sent and nothing is written. Each request the version sends is
answered as that run was answered to the same request, and each read as that run read it, wherever
in the flow it is asked from. So a run can't always be told, and it is counted as **can't tell**,
with why, rather than as one that differs: one where the version sends a request, or reads
something, that run didn't; one where it reads the clock or a random number that run didn't; one
where a node computes with a secret or a token, which the log never holds (a node that only passes
one along, or picks a field past it, computes the same and is told); one that failed then.

A replay computes no more than the deployment is granted: each part of a run between two of its
outbound calls within the deployment's `max_steps`, `max_fuel` and `max_memory`, and the run within
its `run_budget` and `max_requests`, as its own runs of that kind are (a request's limits, or a
clock's: [Permissions](../reference/permissions.md)), since a replay needs no approval. A version
that asks for more has it once deployed; replayed, a run that needs it differs, `it doesn't finish
within what the deployment is granted`, with the limit it ran past. A run that sends a request, or
reads something, its run didn't goes no further: it can't be told, whatever it does next.

A replay as a whole runs for 30 seconds at most, and the server runs two at a time. The runs it
hasn't reached when its time runs out, the oldest, are counted, not run: `20 replayed · 18 same ·
2 differ · 12 not reached in time`; a run its time ran out in can't be told. A third replay while
two run is answered 503, to try again in a moment.
`GET /deployments/<slug>/replay?unit=<hash>&runs=N` answers the same for any version staged
(`POST /unit`) or proposed, for anyone who may read the deployment. From a file,
[`holon replay`](../reference/cli.md#holon-replay) stages and replays it; an agent has the
`replay` tool ([Working with agents](agents.md)).

### Going back to an earlier version

`holon restore <slug>` lists the versions. Going back takes a point, a version (`--to N`) or a run
(`--to-run SEQ`, its number in the log), and what of it to restore, its code (`--code`), its state
(`--state`), or both:

| | `--code` | `--state` |
|---|---|---|
| `--to N` | version N's code, deployed again | the state version N left: the one the version after it was deployed with |
| `--to-run SEQ` | the code the run ran on, unless that is the code deployed now | the state the run left |

Nothing is rewritten: what a restore replaces stays restorable in turn.

- **Code** is deployed again as a new version that names the one it restores. It is checked against
  **today's grant**, not the one it had: a secret revoked since stays revoked, and what it needs
  that isn't granted now is listed. `--approve` approves exactly that list; without it nothing
  changes.
- **State** (state, collections) is put back as the live state, in a **new session**: the session
  ends as a stop would, keeping the state it replaces as its own, and a new one starts from the
  state put back, named after it (`run 41`, `version 3`). The deployment must be running or paused;
  a stopped one runs **from session N** instead. Runs waiting at an `await` carry on against the
  state put back.
- **Both**: the code first, then the state; nothing changes if either can't be done.
- **`--dry-run`** says what it would do: what the code would ask, which keys the state would change,
  and how many runs were logged since that point and what they did outside the state: calls out,
  file and database writes, messages. A restore doesn't undo those: a refund sent stays sent.
  Its **what it touches** rows (as the [approval dialog](#the-approval-dialog) shows them) are the
  restored version's beside the deployment's now, and say what the restore does to each: a key
  or collection `putting the state back changes it` (or `brings it back`, `drops it`, or `its value
  stays as it is`), each value now → as it would be below them; a host, folder, database, channel
  or flow `done 3 times by the runs since, not undone`. A secret keeps its current version: a
  restore never rolls one back (`holon secret rollback` does).
- **Files** in a bound folder are never put back, but a state restore's dry run names each one the
  runs since wrote, once however many times: how many writes, by which runs, and whether it is on
  disk as the last one left it. One changed by anything but a run (an edit by hand, another
  program) says `changed outside a run since run 48`; one removed, `gone from disk`. The log keeps
  a write by its hash, never its text, so the editor's diff shows a file's text then beside now
  only when a run read that content (the log keeps reads as the folder says) and it is small;
  otherwise it says the two hashes.
- **Before rc.30**, versions open on their canvas but don't restore their code (what their deploys
  asked wasn't kept), and neither versions nor runs kept the state they left. Open or import one and
  update again.
- Restoring is an editor's or an admin's: an agent can't (`POST /deployments/<slug>/restore`,
  `{"version": N}` or `{"run": SEQ}`, with `"code": true`, `"state": true` or both, and
  `"dry_run": true` or `"acknowledge": […]`). A state, restored or its dry run, is read as the
  state is: it takes `state.view` on the flow too.

The sample **price-rollback** walks through it; **stock-sheet** restores a state whose runs also
wrote a file.

## Running, pausing, stopping

- **pause**: the deployment stops answering (its routes answer 503) and its clock stops; nothing
  is lost. **resume** carries on.
- **stop**: the same, and ends the session. **run** starts a new one, **from latest** (the state
  as it was), **fresh** (empty state) or **from session N** (the state a kept session ended with).
  A state put back with `holon restore --state` starts a new session too, without stopping.
- **delete** (once stopped, on the deployed page) removes the deployment and its stored state.
  It can't be undone. `holon export` its runs first if you need them.

In `holon editor`, the terminal one, `g`, `p` and `x` on a flow's page run, pause and stop it,
and its **Settings** removes it once stopped. A saved flow that was never deployed (or the saved
text of one that was) is deleted from the **Flows** page with `del`, asked first.

## Changing a grant

On the deployed page, a deployment's **permissions ▸** shows what is granted. While it runs, you
can narrow it:

- **revoke** an item: from the next run on, the server refuses that effect (a run already going
  keeps the grant it began with, until it ends). Revoking something optional
  makes those parts absent, and the flow carries on. Revoking something required **pauses** the
  deployment, unless **keep running when something required is revoked** is ticked: then those
  calls fail and the rest runs. What starts its runs always pauses it when revoked, ticked or
  not: the first it has of receiving requests, a folder's new files, its schedule and an action.
  A flow with several kinds of handler (routes and a clock, say) has the others' triggers as
  ordinary required items.
- **grant back** restores a revoked item. A call of another flow's action comes back only from
  someone who may run that action, as its deploy would (a call runs it under that flow's grant,
  with what this flow sends).
- a limit can be **lowered**. Raising one needs a new deploy, and approval.

Each change goes on the deployment's [timeline](#the-timeline), with who made it, and each run
records the grant it ran under.

## The timeline

Beside its runs, a deployment keeps what was done to it, each with who and when: its **events**.

| Kind | |
|---|---|
| deploy | each version deployed, or a version's code restored |
| restore | a state put back, and the session it started |
| grant | an item revoked or granted back, a limit lowered |
| secret | a secret set, removed or rolled back: its name, version and source, never its value |
| binding | a folder or database bound or unbound for this deployment (`--for SLUG`) |
| lifecycle | paused, resumed, stopped, run (and from which state), by a person or by the host (a grant that no longer covers it) |

`holon history <slug>` shows them among the runs, oldest first:

```
#3      run        2026-10-30T09:12:01  PUT /prices/tea  200  v4
        deploy     2026-10-30T09:12:03  deployed version 5  by maria
#4      run        2026-10-30T09:12:05  PUT /prices/tea  200  v5
        restore    2026-10-30T09:14:40  put back the state version 4 left, in session 2  by maria
```

`--last N` shows the newest N runs (20 by default) and the events among them; `--events`, every
event, without the runs. `GET /deployments/<slug>/events` answers them as JSON.

In the editor, view mode's **history** button (beside "filter runs…", above the run list) shows
the same timeline, newest first, without the runs (the run list has them; **runs** puts them
back), filtered by kind: deploys, access (grants, secrets, bindings) and lifecycle, and by the
words in the filter. Any row of it is picked, and its details show beside it; and any row is gone
back to: a run or a version to itself, anything else (a restore, a grant, a pause) to how it was
right after it: the version deployed then, and the state then, which is what the run before it
left, or what the restore before it put back (a restore names the point it put back, `from` in
its event). Picked:

- **canvas** (a version) shows the canvas it was deployed with;
- **code**, **state** or **both** restore it, as `holon restore` does: a dry run first, its
  result in a dry-run window (nothing has changed yet) that says what it would change, part by
  part (the code; what it touches, row by row, as `holon restore --dry-run` says it; the keys,
  and what the runs since did outside, which a restore doesn't undo), and asks to approve what
  the code needs that today's grant doesn't cover: as a deploy's review, it does so once you
  tick that you read them (`t` marks it in `holon editor`), and a dry run read again asks for
  the tick again. It opens with what
  it changes, a line each (`code: version 5's is deployed again, as version 12, in place of
  version 11`, the permissions added and dropped, the keys and items of the state). Its **diff**
  button compares each part with what is there now: the flow's text, line by line, deployed now
  beside the restored version's; the grant, item by item; and each state key, a collection's
  items by key as the flow reads them (`prices["tea"]: 40` → `4`). A button that can't apply is
  off, and says why: the version deployed now, a state not kept.

**↺** on a row, in the history or the run list, restores whatever of it can be, through the same
dry run and window.

### A cell's history

Which run changed a value? `holon history <slug> --cell NAME` lists the runs that changed one
cell (`state.NAME`) or a collection, oldest first, each with the value before and after it; with
`--item KEY`, one item of the collection NAME:

```
balances["a"], oldest first, now 101:
#5      run        2026-10-30T09:12:01  POST /accounts/a/move  200  v1   nothing → 100
#7      run        2026-10-30T09:12:09  POST /accounts/a/move  200  v1   100 → 70
        outside    2026-10-30T09:14:40  between runs 7 and 9: restore: put back the state run 5 left, in session 2   70 → 100
#9      run        2026-10-30T09:15:02  POST /accounts/a/move  200  v1   100 → 101
```

A run that only read it isn't named, nor one that changed another item. A restore or a deploy
that moved it between two runs is a row of its own, so each value follows from the one before.
For a whole collection, each run's row names the items it changed. `GET
/deployments/<slug>/changes?name=balances&item=a` answers it as JSON, newest first, to whoever may
read the flow's state (`state.view`); it reads the newest 2000 runs the log keeps, and `before=SEQ`
reads further back.

In the editor, **⧗** beside a cell or a collection's item in view mode's **Flow state** (and in
its full state) opens the same history, newest first; a run's row opens that run. The sample
**balance-trail** walks through it. In `holon editor`, the terminal one, a flow's **State**
section does the same: **enter** on a cell or a collection opens its history, a collection shows
100 items at a time (`]` and `[` page through them), and `i` asks for an item's key and shows
its history.

Events are **recorded, never restored**: [going back](#going-back-to-an-earlier-version) puts back
code and state only, and the grant, secrets and bindings stay as they are now. They are kept for
good, in a hash chain of their own beside the run log, which an export carries and `holon verify`
checks ([Checking runs](checking-runs.md)). Deploy settings aren't events: each version's deploy
holds them. Saved drafts, accounts and bindings for every deployment are in the server's activity
log.

## The host's policy

What may ever be granted, and what is granted without asking, is the server's policy:
`holon.toml` in its data folder. It isn't edited in the editor. See
[Server configuration](../reference/server.md#the-policy).
