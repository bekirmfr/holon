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
| `keys/` | the secrets key, where there is no OS key store |
| `run/` | a file per running server, so commands on this machine find it and sign in |
| `tokens.json` | the agents' tokens `holon mcp` signs in with: the one it made on a server you run, and those `holon mcp setup` keeps for servers elsewhere; readable only by you |
| `mcp-hosts.toml` | agent hosts you added to `holon mcp hosts` |

In the data folder (`--data`, `HOLON_DATA`):

| File | Holds | Option |
|---|---|---|
| `store/` | every unit, by hash | `--store` |
| `deployments.json` | the deployments, their grants, sessions and parked runs | `--deployments` |
| `deployments.logs/` | each deployment's run log: its newest 128 runs as written, `<slug>.jsonl`, and the older ones it keeps sealed, gzipped, in `<slug>.sealed/` (`zcat` reads them) | |
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

To move a server, stop it and copy the data folder; the secrets key goes with it only if it is in
a file (see [The secrets key](#the-secrets-key)).

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
    within ten seconds made into exports `holon verify` checks, gzipped:
    `<backup>/<slug>/holon-archive-<slug>-runs-<first>-<last>.json.gz`, with the units the runs
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

- **Passwords** are at least ten characters, kept as Argon2id hashes. Five wrong in a row lock the
  name for a minute, from the address they came from (its owner still signs in from elsewhere;
  behind a proxy, every address is the proxy's). A few passwords are checked at once: past that,
  a sign-in waits its turn, and past thirty seconds is told the server is busy (503).
- The server answers up to 512 requests at once; one more is told to try again (503). A request's
  body must arrive within 30 seconds, or it is told 408 and its place goes back.
- **Sessions** last seven days. **Invite** and **reset** links work once, within seven days.
- The server never lets the last enabled person holding admin be removed, disabled or given
  another role. The run file's admin (below) doesn't count: it is there only while the server runs.
- **Who acts** is a principal of one of three kinds, each name unique across them: a **person**,
  who signs in with a password or a provider and may hold any role but agent; an **agent**, role
  agent or viewer; a **service** (a script, CI), role viewer or editor, never admin. Agents and
  services sign in with tokens of their own (People and access, or `holon people token`), sent as
  `Authorization: Bearer <token>`, and act as themselves, by their name. Each has an **owner**, a
  person, who makes and revokes its tokens, as an admin may (an owner who isn't an admin, in the
  editor's *Your agents and services*). Tokens are shown once and kept only
  as hashes; revoking one, or disabling its agent or service, stops it at once. Only a person
  decides an approval. A person who still owns an agent or a service can't be removed until it has
  another owner. An accounts file from before `access.json` keeps its principals but not their
  roles: each holds none until the run file's admin gives it one (`holon people set NAME --role
  ROLE`), and the server names them when it starts. Its API tokens from before principals, which
  had no owner, are not read: add each as an agent or a service, and make it a token.
- Commands on this machine (`holon export`, `holon folder`) sign in through the run file in
  `~/.holon/run/`, as `local:<your user>`, while that server runs.
- `holon mcp` never does: it signs in as an agent. On a server you run, the run file's admin makes
  yours the first time, `<your user>-agent`, owned by you, even before anyone has set the server up
  (it is set up by the first person, as ever); its token is kept in `tokens.json`.

### Signing in through an identity provider

An admin can turn on OpenID Connect on People and access, with the provider's issuer URL and a
client id (a public client; the editor signs in with the authorization code flow and PKCE). The
issuer, and every address its discovery document gives, must be https (plain http only to this
machine), since the keys that check its tokens come from there. A person signs in through the
provider when a person here has their email, and the provider says it verified that email
(`email_verified` true in its ID token): one that doesn't say so signs no one in, since some
providers let a person type any email. A token for several audiences must say it was issued to
this client (`azp`). People are still added here, with a role; the provider only proves who is
signing in.

## Secrets

Secrets are set per deployment, by an admin, in the editor (**⚙ → secrets**); see
[Deploying](../guides/deploying.md#secrets). The server keeps them in `secrets.json`, each value
sealed with AES-256-GCM, the deployment, name and version bound in.

### The secrets key

`--secrets-key` says where the key is:

| Value | |
|---|---|
| `os` | the OS key store (the default on Windows and macOS) |
| `file:PATH` | a file of 64 hex digits, readable only by the server's user, made if missing (the default on Linux: `~/.holon/keys/`) |
| `env:VAR` | an environment variable holding 64 hex digits; `holon serve --new-secrets-key` prints one |

Without the right key the server doesn't start. Back the key up apart from the data: one without
the other opens nothing.

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
| more runs waiting at `await` than allowed | 503 |
| a resume token that isn't valid | 404 |
| the route's answer has no value | 404, or the status its `?? status(…)` says |

A browser page from another site may call the routes only if the flow lists its origin in
`browsers` (or the deploy settings' **web pages from**).

Every request is a run in the log, even one no route took.

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
lose the last second of them, never more. So are requests no route took (a 404, a 405). A run
whose entry couldn't be written or synced is answered with a 500: it ran, but isn't known to be
on record.

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
