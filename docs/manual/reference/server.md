# Server configuration

`holon serve` is the whole platform: the control plane (the editor, accounts, deployments, the
API the editor and `holon mcp` use) on one address, and one small HTTP server per deployed server
flow, on the flow's own port. This page covers what it keeps, its policy, accounts, secrets and
how it answers. Its options are in the [command line reference](cli.md#holon-serve).

## Where it keeps things

Everything is under one folder, `~/.holon` (`%USERPROFILE%\.holon` on Windows), or `HOLON_HOME`:

| Path | |
|---|---|
| `bin/` | the program |
| `data/` | the server's state: **the one folder to back up** |
| `keys/` | the root key, where there is no OS key store |
| `run/` | a file per running server, so commands on this machine find it and sign in |
| `tokens.json` | the session `holon login` began on each server you signed in to, and the agents' tokens `holon mcp` signs in with: the one it made on a server you run, and those `holon mcp setup` keeps for servers elsewhere; readable only by you |
| `mcp-hosts.toml` | agent hosts you added to `holon mcp hosts` |

In the data folder (`--data`, `HOLON_DATA`):

| File | Holds | Option |
|---|---|---|
| `store/` | every unit, and under it the values (`values/`, the long-lived ones packed in `values/packs/`) and the texts of files runs read (`blobs/`): each sealed, and named by a keyed hash of its hash, not the hash itself | `--store` |
| `deployments.json` | the deployments, their grants, sessions and parked runs | `--deployments` |
| `deployments.logs/` | each deployment's run log: its newest 128 runs a line each, `<slug>.jsonl`, and the older ones it keeps gzipped in `<slug>.sealed/`; its events, `<slug>.events.jsonl`; its console lines in `<slug>.console.jsonl`, rolled to `<slug>.console.1.jsonl` and on. All sealed (see [The root key](#the-root-key)): `holon export` gives a run log readable | |
| `deployments.state/` | each stateful deployment's state, `<slug>`: the hash of its current snapshot, whose values are in `store/`, written as each change lands | |
| `flows.json` | saved flows | `--flows` |
| `accounts.json` | people, agents and services, sessions, invites and tokens (as hashes) | `--accounts` |
| `access.json` | the roles bound to each of them; `access.versions.jsonl` beside it, every version, with who changed what and when | beside `--accounts` |
| `secrets.json` | sealed secrets; `secrets.audit.jsonl` beside it, who changed what (never values) | `--secrets` |
| `folders.json` | the named folders bound | `--folders` |
| `databases.json` | the named databases bound | `--databases` |
| `holon.toml` | the policy | `--policy` |
| `host.key` | the key resume tokens are signed with | `--host-key` |

Every file the server writes there is readable only by the server's user, and a data folder it
makes is that user's alone. One there already that other users can read is said at start, with
the command that makes it yours alone (`chmod 700 …`); the server leaves it as it is. The server
refuses to start when a file with secrets or tokens in it can't be read, rather than starting
without it.

To move a server, stop it and copy the data folder; the root key goes with it only if it is in a
file (see [The root key](#the-root-key)).

### Stopping it

Ctrl+C, SIGTERM (what a service manager or a container sends) or closing its console stops the
server, and it says what it waits for:

```
holon: stopping: no new runs, 2 going on, waited for up to 1m (ctrl+c again stops at once)
holon: stopped
```

From then on it starts no run: its flows' requests and the editor's are answered 503, and its
clocks, folders, mailboxes, feeds and queues start none (a waiting run's deadline passes it by,
and the run times out once the server is back). The runs going on finish, each to its line in
the log; then the logs and the witness's queue are synced and it exits. It waits for them up to
`--stop-within` (default `1m`; `HOLON_STOP_WITHIN`), since a run held only to its
[limits](#limits-a-run-has) may take longer than a service manager waits before it kills the
server. Past it, the server syncs what is written and exits with status 1, saying how many runs
it cut off, as a crash cuts them. A second ctrl+c stops it at once, as a power loss would: a
`lazy` run's line in the last second may be lost. [Running it as a
service](../guides/running-as-a-service.md) says how long to have systemd, a container or a
Windows service wait.

A stop leaves a mark beside the deployments file (`deployments.stopped`), which the next start
takes away; one that cut runs off leaves none. A start that finds none follows a crash, which may
have cut a run off between its commit and its line: a deployment whose state isn't the one its
last run left says so on its timeline, and
[`holon verify`](../guides/checking-runs.md#the-state-run-to-run) doesn't check where the next run
began, rather than report the state as changed outside holon.

Closing its console on Windows gives it a few seconds before Windows ends it.

A path given to the server (an option, the `backup` setting, a folder or a database to bind, a
folder to browse) is read as its machine reads one, and kept and shown in that machine's form. On
Windows `/` and `\` both separate (`D:/archive` is `D:\archive`), and a whole path starts at a
drive or a share (`D:\archive`, `\\nas\runs`); on Linux and macOS it starts at `/`, and `\` is
part of a name. An option's path that isn't whole is taken from the folder the server was started
in; a setting or a binding asks for a whole one.

### What it holds

`holon host`, the admin page's **the host** and the terminal editor's admin view say what the
server holds now:

- **memory**: the server's, and at most so far; each cache's use against its size
  (`--flow-cache`, `--value-cache`);
- **each disk** it keeps things on, once: how full it is for the server (what isn't free to it: a
  quota, or the space a disk keeps for root, counts as full), what is free, and what it keeps
  there;
- **what it keeps**: the content store's values (their bytes, and the disk they take: a small
  value's file takes a whole 4 KiB block, which is why the sweep packs the values it keeps once
  they are an hour old: deflated together in `store/values/packs/`, read back as any), the logs,
  the folders' kept file texts. The store and
  the texts are counted shortly after the server starts and after each round of sweeps, so a
  view never lists millions of files; it says when, and how many values were put since;
- **what crashes left**: a file is written aside and renamed over, so a server stopped in the
  middle of a write can leave a `….tmp-…` or `….cut-…` file. Once one is older than any write
  takes (15 minutes), the count removes it, and says so in the server's log and its activity;
- **the sweeps**: how often, when next, and what the last of each removed;
- **warnings** ([below](#warnings));
- **each flow**, the most first (`holon host --for SLUG`, `GET /deployments/<slug>/holds`, each
  flow's page in both editors): its log in memory (the newest 32 of the runs it keeps, about three
  times the lines they were written as: measured 1.1 to 3.6 times; the others it reads from its
  file when a page asks) and on disk, its live state, its flow in the
  cache (and what it shares with other flows there), and its states and file texts in the store,
  as the store's last sweep found them: a value two flows name is counted once, as shared.


### Warnings

The server looks at what it holds every ten seconds, and warns of:

- **the deployments file that can't be written**: a deploy, a grant, a pause or a waiting run kept
  while it fails is lost when the server starts again (it is written again at the next change);
- **the run logs that can't be synced to disk**: a strict run answers 500 until they are, and a
  `lazy` run's newest lines may be lost to a power loss (it is tried again every second);
- **a witness that doesn't take the runs' heads** (`--export-heads`): they wait on disk and are
  sent again;
- **a disk past `warn_at`** (or past `full_at`, if that is reached first), naming the flow whose share grew most since the store was last
  counted;
- **a cache at its size** most of the last hour: it let go of flows (or values) to make room at
  most looks, so runs load them from disk again;
- **sweeping off** while the store takes more than a tenth of its disk;
- **a backup that can't be written to**: a log keeps the runs it would let go of until it can;
- **a batch of runs in the backup that can't be made an export**: its runs stay in it as the log
  wrote them, it is tried again at each look, and the other batches are made exports meanwhile;
- **a pack of the store's values that can't be read** (cut short, or changed on disk): it is left
  as it is and its values are missing until it is restored; the server starts, and the store's
  other values read as they were.

Each says what to do. They are shown first in `holon host` and on the admin page's **the host**,
on the home page for admins, and in the terminal editor's top bar and overview
(`GET /admin/warnings`); and each is said once in the server's log and its activity as it starts,
and in the log as it ends.

`GET /health`, which anyone who reaches the server may ask, answers `ok`; while the deployments
file can't be written or the logs can't be synced, 503 `not ok: ` and which (`not ok: the run logs
can't be synced to disk`), never a path or an error's text; and 503 while the server stops. A
witness that doesn't take heads is a warning only: one witness down would take every server it
witnesses out of a load balancer at once. It is a readiness check, not one to restart the server
on ([running it as a service](../guides/running-as-a-service.md)).
## The policy

`holon.toml` in the data folder (or `--policy PATH`) says what this server may ever grant, and
what it grants without asking. A missing or empty file means the defaults. It is read at start;
`POST /permissions/reload` reads it again (a file that doesn't parse then changes nothing). The
editor shows it but doesn't edit it.

```toml
[limits]
request_timeout_ms = 20000

[ceilings]
max_requests = 200

[grants]
auto = ["StateRead(*)", "HttpRequest(\"hooks.slack.com\")"]
allow_plain_http = true
```

- **`[limits]`**: the defaults a deployment gets when it asks for nothing more.
- **`[ceilings]`**: the most a deployment may be granted. A default can't be above its ceiling.
- **`[grants]`**:
  - `auto`: items granted without asking, each exactly (`HttpRequest("hooks.slack.com")`) or a
    whole kind (`StateRead(*)`). Names are in [Permissions](permissions.md).
  - `allow_plain_http`: whether a deployment may ever be granted plain `http://` calls (default
    `false`).
  - `allow_private_addresses`: whether it may ever reach this machine and its network (default
    `false`).
  - `allow_unrecorded`: whether a handler's runs may ever be kept out of its log, each approved as
    `Unrecorded("GET /health")` (default `true`: a person approves each).
- **`[agents]`**:
  - `propose`: whether an agent's version that needs a person's approval waits for one, as a
    proposal in **Approvals** (default `true`). `false`: it is refused, and nothing is proposed
    or saved; a version that asks for nothing new still deploys from the agent, and a person
    deploys the rest. A flow's own **agents may propose versions** setting can only refuse more.
    See [Agents](../guides/agents.md#approving-what-an-agent-proposes).

- **`[storage]`**: how the server keeps what it stores.
  - `sweep_every`: how often the server sweeps, on its own, what nothing kept names any more: the
    content store's states and values (an hour after they were put) and the texts of files runs
    read and wrote ([Files on the host](../guides/files.md)). `90s`, `10m` (the default), `2h`,
    `1d`, a minute at the least; or `off`, when only an admin's sweep removes anything (`holon
    store sweep`, `holon folder sweep`, the admin page). The store's sweep paces itself (about
    2,500 values a second at most) so the server's runs hardly notice it. An admin sets it from the
    admin page or with `holon policy --sweep-every`, which write it here; a change applies within
    ten seconds.
  - `warn_at`: when a disk the server keeps things on is full enough to warn of: how full it is
    for the server, `85%` (the default; a quota, or the space a disk keeps for root, counts as
    full), or how much is left, `5G` (`500M`, `1T`). Set as `sweep_every` is (`holon policy
    --warn-at`). See [Warnings](#warnings).
  - `full_at` and `when_full`: past `full_at` (`95%` unless said; fuller than `warn_at`, or less
    free), what the server does. `warn` (the default): nothing but the warning; once the disk
    fills, a strict run fails its sync and answers 500. `pause`: every deployment that records
    runs is paused by the host (its routes answer 503, its clocks stop; one whose runs are all
    unrecorded goes on), and resumed once every disk is back under `warn_at` (and under `full_at`:
    one written in another unit, `85%` and `500G` free, may be reached first); one a person resumes
    while the disk is still full is paused again, one a person paused stays paused. `roll`: every
    ten seconds, the biggest log on disk (its file and its sealed segments) is cut to half the runs it keeps (never below 20), its
    file cut back at once, and the first cut sweeps the store after it; once back under
    both, each log keeps its runs as granted again (what was cut stays cut). Each is said on
    the deployment's timeline. Set with `holon policy --full-at --when-full`, or on the admin page.
  - `backup`: a folder's whole path (an external or second drive, or a folder a cloud client
    keeps in sync: Dropbox, Google Drive, OneDrive), or `none` (the default). The runs a log lets
    go of, as it cuts back to what it keeps (or as `roll` cuts it), are written there first, and
    within ten seconds made into exports `holon verify` checks, gzipped, then sealed under the
    server's keys as its data folder is (`holon verify --root-key` opens one; keep the root key
    apart from the backup): `<backup>/<slug>/holon-archive-<slug>-runs-<first>-<last>.json.gz`,
    with the units the runs
    ran and the file texts they read that the server keeps. Every file the server writes there
    starts with `holon-archive-`, and the folder may hold other things: the server only ever
    exports and removes the batches it wrote (`<slug>/holon-archive-<first>-<last>.jsonl`, each
    seq 20 digits), and counts only its exports. Gzipped, a small route's run takes about 360
    bytes (13 times smaller than its line), a listing's about 2 KB (30 times). If it can't be
    written to, a log doesn't cut back: it keeps the runs until it can, and the host warns; only
    `roll` past `full_at` lets them go without. Object storage (`s3://`) is to come, and Holon
    Cloud. Set with `holon policy --backup`, or on the admin page.

An unknown section or key is a mistake, and so is a limit name the server doesn't know. The limits,
their defaults and their ceilings are in [Permissions](permissions.md#limits).

At deploy, each item of the manifest is either **refused** (above a ceiling, or plain HTTP or
private addresses the policy doesn't allow), **granted** without asking (in `auto`, a limit at or
below its default, or already in this deployment's grant) or **needs a person**. A grant is clamped
to the current ceilings whenever the server loads.

## Accounts and roles

Everyone signs in; there is no trust for requests from this machine. A new server prints a
setup code (`XXXX-XXXX`, or `HOLON_SETUP_CODE`); whoever enters it in the editor is the first
admin. Until then the server answers everything but setting up with 403, but for an agent or a
service made already (below); five wrong codes in a row refuse setting up for a minute, so the code
can't be guessed.

| Role | May |
|---|---|
| viewer | read: flows, deployments, logs |
| editor | everything a viewer may, and save, deploy and approve, run, pause, stop, delete, change grants |
| admin | everything, and set secrets, bind folders and databases, manage people, agents, services and sign-in |
| agent | read, check, save flows (never replacing someone else's), deploy what needs no approval, call routes |

What a role may not do is refused with 403, saying why. A path the control plane has no route for
is answered 404, once its caller has signed in (401 before).

A role is bound to a principal on the server when it is invited or added with one, and in place
of it when it is changed. What is bound is kept apart from the accounts, in `access.json` beside
them (`.holon-access.json` beside `.holon-accounts.json`), and each change is a new version, kept
for good in `access.versions.jsonl` with who made it, when and what it changed: "bound editor to
bob on this host". It holds no password or token, so it can be handed to an auditor. Removing a
principal removes what is bound to it, so a new one of the same name holds only what it is given.
`GET /people` and `/auth/me` answer each principal's bindings, and its `role`.

A role may also be bound at one flow, or at one approval point of a flow, and until a time. An
admin binds it (`POST /bindings`):

```json
{ "who": { "principal": "bob" }, "role": "editor", "scope": { "flows": { "slug": "payout" } },
  "expires_at": 1798761600, "why": "on call this week" }
```

- **At a flow**, it holds there (the flow's runs, state, secrets and approvals) and nowhere else.
  The lists (flows, deployments, proposals, the overview, the activity) keep the flows its holder
  may view. A flow they saved that no one has deployed is theirs wherever they hold the action, so
  they draft one, and deploy it where they may.
- **A flow that runs another flow's action** (`import { payout } from "flow:payments"`) is
  deployed only by someone who may run that action themselves (`run.call` on it, and for an
  agent, the other flow letting agents run it): its call runs the action under that flow's grant,
  with what the caller sends. Anyone else is refused (403, saying which action), whether the call
  is new or granted already; an agent's version waits for someone who may, as a proposal.
- **At an approval point** (`{"approval": {"flow": {"slug": "payout"}, "name": "ok"}}`), it
  decides that point and nothing else, not even viewing the runs: bind `viewer` on the flow with
  it for that.
- **A binding that has expired** holds nothing. It stays listed, `expired: true`, until it is
  unbound.
- A binding gives only what its author holds where it is made, and is refused where its role holds
  nothing (a viewer at an approval point). It is taken away only by someone who holds what it
  gives there (403 saying what): a role that assigns people doesn't unbind an admin.

`GET /bindings` (`?who=&role=&flow=`) lists what is bound, and `DELETE /bindings/<id>` (`?why=`)
unbinds one. `GET /access/versions` lists every version, newest first, and `GET /access?version=N`
answers one whole. Each change is also in the activity log.

`PUT /access/approvals` (`{"exclude_deployer": true, "agents": false, "starter": false,
"changes": 2, "why": "…"}`, any of them; `people.assign` on the host) sets how the host decides
approvals beyond each step's role, as a new version: with `exclude_deployer`, whoever deployed the
version a run is on decides none of its approvals; with `agents` false, no agent decides one, and
a deploy whose approval has a step for an agent is refused; with `starter` false, no step takes
whoever started the run, and a deploy whose approval has a step that would is refused; with
`changes` N (1 to 5; `null` turns it off), an access change that gives more waits for N people.
They are `false`, `true`, `true` and `null` by default, and `GET /access` has them, `approvals`.

While `changes` is N, a change that gives someone more (`POST /bindings`, `PUT /roles/<name>`,
`PUT /groups/<name>`, `PATCH /people/<name>` with a role, `PUT /access/approvals` loosening them)
isn't made: it is answered 202, `{"proposal": {…}, "version": n}` (`PATCH` answers the principal
with its `proposal`), and kept as a proposal: `id`, `author`, `why`, `change` (as the route got
it), `what` (what its version will say), `gives` (in words), `to` (whom it gives more, by name),
`ways_in` (who could act as them, in words: "ada made bob's reset link"), `base` (the version it
was proposed on), `needs`, `approvals` and `state` (`waiting`; `applied` with its `version`;
`declined` with `declined_by` and `reason`; `stale` with why). `POST /people` with a role that would wait is
refused, 409: invite or add them with one that gives less, then give it. A change through this
machine's run file to `/access/approvals` is made at once.

- `GET /access/proposals` (`people.view`) lists every proposal, newest first.
- `POST /access/proposals/<id>/approve` (`people.assign`): a person other than its author, holding
  what it gives and able to act as no one it gives it to (403 if not), approves it; the N-th
  approval makes it, by its author, its why saying who approved it, and answers it `applied`. One
  that no longer applies, or to one of whose `to` a way in was made since, is `stale`: 409, saying
  why.
- `POST /access/proposals/<id>/decline` (`{"reason": "…"}`, `people.assign`): declines it; its
  author withdraws it so. One that no longer waits is 409.

An admin also defines roles of their own, and groups:

- **A role** (`PUT /roles/<name>`, `{"allows": ["run.view", "run.decide"], "kinds": ["person"],
  "about": "decides payouts"}`) allows the actions it names, from the catalogue
  (`GET /roles` lists every role with what it allows), to the kinds of principal it names. Its
  author holds on the host everything it allows. Changing it changes every binding of it, and is
  refused when one is to a principal whose kind it would no longer allow. A built-in role's name
  is taken, and a built-in role isn't changed.
- **A group** (`PUT /groups/<name>`, `{"members": ["bob", "cy"]}`: all of them) is bound as a
  principal is, `{"who": {"group": "payments"}, …}`, and the binding holds for each member whose
  kind its role allows. Its members are principals here, never groups; removing a principal takes
  it out of its groups.
- A role or a group still bound isn't removed (409, saying how many bindings hold it): unbind
  them first.
- **No one gives or takes more than they hold**, whichever way: binding or unbinding a role,
  inviting, adding or setting someone with one, adding a member to a group that is bound or
  taking one out, or changing a role that is defined (holding what it allows before and after).
  **Nor changes anyone who holds more**: whoever changes a principal (`PATCH` or
  `DELETE /people/<name>`, its email, owner, role or whether it is enabled; resetting its password;
  making or revoking its tokens) holds all it holds, since they could then act as it, or is
  refused (403, `ada holds admin on this host, which holds flow.edit there, which you don't: you
  change, or act as, only someone who holds no more than you`). So a role that manages people
  without holding everything an admin does can't make, become or lock out an admin. An agent's
  or a service's owner still makes and revokes its tokens. Setting the identity provider takes
  holding what each person holds.
- **Nor makes a way in to anyone whose access waits for people to be given**, while `changes` is
  N: resetting the password of, setting the email of, making a token of (but its owner), or giving
  another owner to someone else who holds `run.decide` or anything of an admin's, or setting the
  identity provider while another person does, is refused (403, `bob holds run.decide on this
  host, as admin, and giving that waits here for 1 person other than its giver: whoever resets
  bob's password could act as bob, counted as another person, so it is done only through this
  server's machine (its run file); or take that from bob first, which applies at once, and give it
  back after, which waits`). They do it themselves, or this machine's run file does. `GET /people`
  answers who could act as each principal, `ways_in`: whoever made the link a person's password
  was last set from, whoever set their email, an agent's or a service's owner and whoever else
  made a token of it.
- **No role gives an agent what is a person's**: whatever it holds, an agent never approves a
  deploy, starts, stops, deletes or restores a flow (it may dry-run a restore), writes a secret,
  or manages people, roles, resources or the host.

`GET /auth/me/can?actions=flow.deploy,run.decide&flow=payout` answers, for each action, `true`
or the words its refusal would say: on the flow named (on its approval point for `run.decide`,
with `&approval=ok`), and with no flow, on the host for the host's actions and on any flow for the
others, as the lists ask. The editor's buttons follow it. `/auth/me` answers `access_version`, the
access document's version, which moves with every change to who may do what: what `can` answers
changes only with it (or with a flow's tags, on deploying). `/auth/me` and `GET /people` answer what holds for a principal, each binding
with what its role allows (an agent may not read `/roles`), and one to one of its groups with the
group's name. `POST` and `PATCH /people` take any role the principal's kind may hold, an admin's
own included. From the command line: `holon roles`, `groups`, `bind`, `unbind` and `access`
([the command line](cli.md#roles-and-bindings)).

A run waiting at an approval is listed to whoever views the flow's runs or decides one of its
approvals: `GET /deployments/<slug>/runs` (`run.view` on the flow, or `run.decide` at one of its
approvals) lists the runs that wait, and `GET /runs/waiting` (anyone signed in) every
deployment's, each with its slug, so that someone bound only at an approval point, who lists no
flow, finds the runs waiting there. To whoever only decides, a run lists the points they decide
alone. Each approval lists its steps, who took each and their note, what it shows, and which
steps are the caller's to take (`yours`). `POST /deployments/<slug>/runs/<id>/decide`
(`{"name": "payout", "approve": true, "reason": "…", "slot": 1, "note": {…}}`, `run.decide` at
that point) takes a step: `slot` counts from 0 as the steps are written, and none is the one open
step the caller may take. A decline says why (`reason`, at most 1000 characters), and a note is
any JSON up to 16 KiB, given to the run with the decision. It answers `{name, slot, slots,
outcome}`, `outcome` being `waiting` until the last step, then `approved` or `declined`. A step
out of turn, a second step by whoever took one, a step of a run its caller started (but one that
says `starter: true`), a person's step to an agent or an agent's to a person, and whatever the
host's approval settings keep out are refused, saying why (`dee already decided slot 1 (reviewer)
of "payout": each step of an approval is someone else's`).

- **Passwords** are at least ten characters, kept as Argon2id hashes. Five wrong in a row lock the
  name for a minute, from the address they came from (its owner still signs in from elsewhere;
  behind a proxy, every address is the proxy's), and five wrong setup codes lock setting up from
  that address the same way. A few passwords are checked at once: past that, a sign-in waits its
  turn, and past thirty seconds is told the server is busy (503). While 64 wait, or 8 from one
  address (an IPv6 address by its /64), whatever the names, one more is told so at once: a flood
  of sign-ins keeps sign-ins waiting, never the rest of what the server answers. An invite link is
  checked before its password is.
- The server answers up to 512 requests at once; one more is told to try again (503). A request's
  body must arrive within 30 seconds, or it is told 408 and its place goes back.
- **Sessions** last seven days. **Invite** and **reset** links work once, within seven days.
- The server never lets the last enabled person holding admin be removed, disabled, given
  another role or unbound, bound to them or to a group of theirs. The run file's admin (below)
  doesn't count: it is there only while the server runs; nor does an admin binding that expires.
- **Who acts** is a principal of one of three kinds, each name unique across them: a **person**,
  who signs in with a password or a provider and may hold any role but agent; an **agent**, role
  agent or viewer; a **service** (a script, CI), role viewer or editor, never admin. Agents and
  services sign in with tokens of their own (People and access, or `holon people token`), sent as
  `Authorization: Bearer <token>`, and act as themselves, by their name. Each has an **owner**, a
  person, who makes and revokes its tokens, as an admin may (an owner who isn't an admin, in the
  editor's *Your agents and services*). Tokens are shown once and kept only
  as hashes; revoking one, or disabling its agent or service, stops it at once. A service never
  decides an approval, and an agent takes only a step written for an agent. A person who still owns
  an agent or a service can't be removed until it has
  another owner. An accounts file from before `access.json` keeps its principals but not their
  roles: each holds none until the run file's admin gives it one (`holon people set NAME --role
  ROLE`), and the server names them when it starts. Its API tokens from before principals, which
  had no owner, are not read: add each as an agent or a service, and make it a token.
- Commands on this machine (`holon export`, `holon folder`, `holon decide`) sign in through the
  run file in `~/.holon/run/`, while that server runs, as the person who set the server up: their
  name, what is bound to them, and admin on the host. The access document keeps the tie
  (`local`); `holon access local` (`GET /access/local`) says it, and whether it holds now, and
  `--as NAME` (`PUT /access/local`, `people.assign` on the host) moves it to another person who
  isn't disabled, as a version. Removing that person clears it.
  Before setup, or tied to no one (its person removed or disabled), the run file is
  `local:<your user>`, admin on the host, and decides no approval. Each activity entry made
  through it says `via: "local:<your user>"`, and `/auth/me` answers `local`.
- `holon mcp` never does: it signs in as an agent. On a server you run, the run file's admin makes
  yours the first time, `<your user>-agent`, even before anyone has set the server up (it is set
  up by the first person, as ever), owned by you: by `local:<your user>` until then (a name no
  one here can have, so a person later invited as `<your user>` owns none of it), and by the
  person who sets it up from then on; its token is kept in `tokens.json`.
- **The command line** sends its token over https, or over plain http only to this machine: given
  `http://` to another machine with a token (`HOLON_TOKEN`, or one kept for it), it refuses, and
  says to give the server's https address, or to reach it through an SSH tunnel. A host given
  bare (`--server holon.example.com`) is reached over https.

### Signing in through an identity provider

An admin can turn on OpenID Connect on People and access, with the provider's issuer URL and a
client id (a public client; the editor signs in with the authorization code flow and PKCE). The
issuer, and every address its discovery document gives, must be https (plain http only to this
machine), since the keys that check its tokens come from there. The first time someone signs in
through the provider, they are the person here with their email, when the provider says it
verified that email (`email_verified` true in its ID token): one that doesn't say so signs no one
in, since some providers let a person type any email. From then on they are the subject the
provider names them by (`sub`, with its issuer), and no one else is: another subject with that
email is refused, saying whose email it is, until an admin sets that person's email again, which
matches them anew. A token for several audiences must say it was issued to this client (`azp`).
People are still added here, with a role; the provider only proves who is signing in. Since the
provider says who each person here is, setting it (`POST /admin/oidc`) takes holding what each
person holds: an admin; and while an access change that gives more waits for people, it is set
only through this machine's run file. An ID token is good until it expires, wherever it is shown, so signing out
with one (`POST /auth/sign-out`, the editor's **Sign out**) keeps it refused until then; another
sign-in of theirs at the provider still is one.

## Secrets

Secrets are set per deployment, by an admin, in the editor (**Settings → secrets**); see
[Deploying](../guides/deploying.md#secrets). The server keeps them in `secrets.json`, each value
sealed with XChaCha20-Poly1305, the deployment, name and version bound in.

### The root key

What the server seals in its data folder is sealed under data keys kept in `keys.json`, each
wrapped by one root key, which is never in the folder: secret values, what a waiting run keeps,
and every table: the deployments, saved flows and their proposals, the secrets file itself, the
accounts, `access.json` and each line of its history, the folders, databases, mails, queues and
feeds bound, and what a deployment has seen of its queues, mailboxes, feeds and folders; and every
log, a line at a time: each deployment's run log (its older runs gzipped, then sealed whole), its
events and console lines, the activity and the secrets audit; and the content store: each unit,
value and kept file text, and a pack's blocks (deflated first) and its index, each file named by a
keyed hash of its content hash, so a name says nothing of what it holds; and what waits to be
sent (`mail-spool/`, `queue-spool/`), a removed deployment's records (`removed/`), and what a
backup holds. Each is sealed for what it is, so
one is never read as another, and a plain file where a sealed one belongs is refused, not read.
`keys.json` is the one kept as it is. A run log's chain is over its lines as they were made, not
as they are sealed: an export (`holon export`) is plain, and `holon verify` checks it with no key.
`--root-key` says where the root is:

| Value | |
|---|---|
| `os` | the OS key store, as `holon` / `root-key-<id>` (the default on Windows and macOS) |
| `file:PATH` | a file of 64 hex digits, readable only by the server's user, made if missing (the default on Linux: `~/.holon/keys/<id>.key`) |
| `env:VAR` | an environment variable holding 64 hex digits; `holon serve --new-root-key` prints one |

`<id>` is the one `keys.json` records. Without the right root the server doesn't start; nor does it
when `keys.json` is missing beside data it sealed. Back the root up apart from the data: one
without the other opens nothing. One server at a time runs on a data folder: it holds
`keys.lock` while it runs, and a second one started on the same folder refuses.

### Rotating and forgetting keys

`keys.json` keeps a data key for each scope: `server` (its tables and store), `secrets`, `names`
(the store's file names) and `d:<slug>` for what each deployment wrote. Each has versions, and
each record names the version that sealed it. [`holon key`](cli.md#holon-key) shows and changes
them; none of it ever shows a key.

- **A new data key**, `holon key rotate <scope>`: what is written from then on is sealed under the
  new version; what was sealed under the ones before still opens, so they are kept. `names` is
  never rotated: a store file's name must stay what it was.
- **A new root**, `holon key rotate --root --to <SOURCE>`, with the server stopped: every data key
  is wrapped under a new root, kept where `--to` says; start the server with `--root-key` set to
  it. No data file is touched. A rotation cut short loses nothing: the next start finishes it, or
  undoes it, whichever root it is given.
- **Forgetting a deployment**, `holon remove <slug> --forget` (or the box in the editor's remove
  section): its keys leave `keys.json`, so its kept records, its backups and every file it left
  open nowhere again, and say they were forgotten rather than damaged. Then the root is rotated
  where it is kept, so a copy of the whole folder taken before (which holds the old `keys.json`)
  needs the old root. A copy of that root kept elsewhere, as a backup should be, still opens such
  a copy; forgetting doesn't reach it. A root in an environment variable can't be changed by the
  server: it says so, and `holon key rotate --root` does it with the server stopped.

### What the folder still says

Sealing hides what flows were given and kept, not that they ran. Someone with the data folder and
without the root learns: which deployments exist (file names are slugs), how many runs each kept
and when (file sizes and times, a segment's sequence range in its name), how large what they keep
is, and the state heads (hashes, as an export carries them). `holon.toml`, the witness's queue and
receipts, and the clean-stop mark are kept as they are: none holds run data.

### Secrets from elsewhere

A secret's source can be `env` (`HOLON_SECRET_<NAME>` in the server's environment, or in the
`--dotenv` file; the real environment wins) or `vault` (`mount/path#field`, from HashiCorp Vault
KV v2 at `VAULT_ADDR`, with `VAULT_TOKEN` and `VAULT_NAMESPACE`). Both are read again every
minute; a value that can't be read is kept for five minutes at most. [Secrets from a `.env`
file](../guides/deploying.md#secrets-from-a-env-file) says where the file goes and what it holds.

## Deployed flows' servers

Each deployed server flow listens on its own port, on `--data-host` (127.0.0.1 unless told
otherwise). It speaks HTTP/1.1, without TLS: HTTPS ends at your ingress or proxy in front of it
([A public HTTPS address](../guides/public-https.md)). `holon serve` warns as it starts when the
server or these ports listen beyond loopback. It has no sign-in: a flow guards its own routes.

| Case | Answer |
|---|---|
| no route for the path | 404 `{"error":"no route for this path"}` |
| a route, but not that method | 405, with `Allow` |
| a body that isn't UTF-8 | 400 |
| a body over 10 MiB | 413 |
| a body not sent within 30 seconds | 408 |
| paused or stopped | 503 |
| the server stopping | 503 |
| more runs waiting at `await` than allowed | 503 |
| the flow already answering `max_concurrent_runs` runs (64), or the server 448 | 503, with `Retry-After: 1` |
| the server holding all the memory it may (`--max-memory`) | 503, with `Retry-After: 1` |
| a resume token that isn't valid | 404 |
| the route's answer has no value | 404, or the status its `?? status(…)` says |

A browser page from another site may call the routes only if the flow lists its origin in
`browsers` (or the deploy settings' **web pages from**).

Every request is a run in the log, even one no route took.

The server holds every flow's connections together to 4,096 at once (1,024 a port), fewer where
the open-file limit leaves less room: on Linux and macOS it raises its own limit to the most the
system allows as it starts, and keeps 256 files for its own. An open WebSocket holds its
connection's place until it closes. Past these, a caller waits in the system's queue until a
connection closes, and isn't refused.

## The run log

Each deployment's log is `deployments.logs/<slug>.jsonl`: one JSON entry per run, each with the
hash of the one before. It keeps the deployment's retention: the newest `keep_runs` (the host's
`--keep-runs`, 500 by default), none older than `keep_run_days` (any age by default), both limits
a deployment may ask for in its deploy settings or its text's `keep` line
([Permissions](permissions.md#limits)). Older runs
are cut from the front of the file, which keeps its newest line, so the chain goes on; a witness
hears a `trim`. An entry holds the
run's request, what it read (clock, randomness, state, files, each outbound response), its
outputs, the effects it performed, its answer (up to 64 KiB of the body), its timings, the unit
and the grant it ran under, and who started it. Secrets and resume tokens are replaced by
`‹secret:name v3 #a1b2c3d4›` (the version and a keyed fingerprint of the value; the entry's
`secrets` lists each it was given) and `‹token:name›` before anything is written, and so are the
values of the keys its flow taints, by `‹taint:key›`, wherever the run had them (the flow's
`taint` keys, and every flow's `authorization`, `proxy-authorization`, `cookie` and `set-cookie`;
see [Tainted keys](../language/reference.md#imports-and-settings)): no one, an admin included, can
read them back from the log, the run's lines, its export or an unrecorded handler's tally. Text a run received that only
looks like one is written escaped (each `‹` before it doubled, and `"escaped": true` on the
entry), so a placeholder in the log is always the server's own.

A run's entry is on disk before its answer is sent: an answered run is recorded, a power loss
or not. Runs at once share the sync, so a busy deployment syncs its log far less often than once
a run. A handler may say `lazy` on its line ([Language
reference](../language/reference.md#clocks-files-and-actions)): its runs' entries are written
before the answer and synced within a second, as the deployment's events are, so a power loss can
lose the last second of them, never more (a stop syncs them first: [Stopping it](#stopping-it)). So are requests no route took (a 404, a 405). A run
whose entry couldn't be written or synced is answered with a 500: it ran, but isn't known to be
on record. The next entry written says which runs before it weren't, so `holon verify` reads them
as lost, not as the log changed.

A handler whose line says `unrecorded`, approved as `Unrecorded("GET /health")`, leaves no entry:
its runs are counted instead, by handler (how many, how many failed, how long they took in all,
when the last one started, and the last failure's time, status and error, redacted and cut short,
never its request), kept with the deployment and given by the log's API (`tallies`), and shown
where its runs would be, in view mode, the terminal editor and `holon history`. A failed run (an
error, or a 5xx answer) isn't logged either, and isn't run again: the handler was declared so. So
an unrecorded run can't be replayed, reverted or explained; to see why one fails, make it `lazy`
for a while. Not approved, or taken back, the handler is strict.

`--export-heads URL` sends each new head to a witness every five seconds, with
`HOLON_EXPORT_HEADS_TOKEN` as a bearer token; heads that can't be sent wait in
`deployments.heads/`. A head is sent only once the entry it names is on disk, so a witness never
holds a head the server's disk lost. See [Checking runs](../guides/checking-runs.md#witnesses).

## Limits a run has

Every run has a time budget (25 seconds for a request, 60 for a clock run or a new file, by
default), a number
of outbound requests, and size limits on what it sends and receives. A run past its budget fails,
and performs nothing. The defaults and ceilings are in [Permissions](permissions.md#limits).

Every run's memory together, with the server's caches, is held to `--max-memory` (half the
machine's memory by default, or three quarters of what its container may hold, whichever is
less; `holon serve` says what it holds itself to as it starts). A run that is going on as the
server passes it fails, its error ending `(--max-memory)`, and performs nothing; a request that
comes meanwhile is answered 503 with `Retry-After: 1`. A parse of a big body (`JSON.parse`,
`csv_parse`, `xml_parse`) stops as it passes either the run's `max_memory` or the server's, not
once it is done.
