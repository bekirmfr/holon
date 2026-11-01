# Command line

Everything is one program, `holon`. `holon help` lists the commands; `serve`, `export`,
`verify`, `folder`, `database`, `mcp` and `lsp` say more with `--help`.

| Command | |
|---|---|
| [`holon serve`](#holon-serve) | run the server, with the editor at its address |
| [`holon new`](#holon-new) | write a starter flow, or a sample |
| [`holon check`](#holon-check) | compile a flow: its routes, what it may do, or each mistake |
| [`holon show`](#holon-show) | an outline of a flow |
| [`holon fmt`](#holon-fmt) | rewrite a flow in the canonical form |
| [`holon deploy`](#holon-deploy) | compile a flow and deploy it to a running server |
| [`holon editor`](#holon-editor) | the editor in a terminal: flows, runs, approvals, and text in tabs |
| [`holon deployments`](#holon-deployments) | what is deployed, and each one's waiting runs |
| [`holon run`, `pause`, `stop`, `remove`](#holon-run-pause-stop-remove) | start, hold, end and delete a deployment |
| [`holon call`](#holon-call) | send a deployed flow a request |
| [`holon action`](#holon-action) | run one of a deployment's actions |
| [`holon trigger`, `holon tick`](#holon-trigger-holon-tick) | run a flow by hand, or fire one of its clocks now |
| [`holon state`](#holon-state) | a deployment's state, collections and sessions |
| [`holon grant`](#holon-grant) | what a deployment is granted; revoke, grant back, lower |
| [`holon grants`](#holon-grants) | who holds what |
| [`holon policy`](#holon-policy) | the host's policy, and reading it again |
| [`holon secret`](#holon-secret) | a deployment's secrets: set, roll back, remove, their history |
| [`holon flows`](#holon-flows) | the flows saved on the server |
| [`holon activity`](#holon-activity) | what was done on the server, by whom |
| [`holon user`, `token`, `oidc`](#people-and-sign-in) | people, API tokens, signing in through a provider |
| [`holon export`](#holon-export) | write a deployment's runs to a file |
| [`holon verify`](#holon-verify) | check exported runs offline |
| [`holon history`](#holon-history) | a deployment's timeline: its runs, and what was done to it |
| [`holon restore`](#holon-restore) | a deployment's versions, and an earlier version's or run's code or state back |
| [`holon folder`](#holon-folder) | bind the folders flows use |
| [`holon store`](#holon-store) | sweep what nothing kept names from the content store |
| [`holon database`](#holon-database) | bind the databases flows use, and list their tables |
| [`holon mcp`](#holon-mcp) | the tools for agents, and adding them to an agent's host |
| [`holon lsp`](#holon-lsp) | a language server for editors |
| [`holon version`, `update`, `uninstall`, `migrate`](#looking-after-the-install) | the install itself |

## Install

The install scripts are on each [release](https://github.com/bekirmfr/holon/releases):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

```powershell
irm https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.ps1 | iex
```

They put `holon` in `~/.holon/bin` and add it to your PATH. `HOLON_INSTALL_DIR` installs
elsewhere; `HOLON_NO_MODIFY_PATH=1` leaves PATH alone. `latest` is the newest full release; for a
pre-release, name it: `…/releases/download/v0.1.0-rc.21/holon-installer.sh`.

Each release also has the program for each platform as an archive, with checksums. A download can
be checked against this repository's release workflow with
`gh attestation verify <archive> --repo bekirmfr/holon`.

## Flows as text

These work on any machine, with no server.

### `holon new`

```
holon new <file> [--sample <name>]
```

Writes a starter flow to `<file>`, or the sample named, whose comments say what it does and how
to try it. It never overwrites a file.
`holon new --help` lists the samples.

### `holon check`

```
holon check <file>
```

Compiles the flow and prints what it is:

```
hello: 6 nodes, 6 wires
a server on :8101
  GET /hello/:name
unit 7714237e7d00edfc…
it may:
  receive requests
```

A flow that touches nothing outside says `it only computes: it reads and changes nothing
outside`. Each mistake goes to stderr as `file:line:col: message`, with the line and a caret
under the place, and the exit code is 1.

### `holon show`

```
holon show <file>
```

An outline: the routes, the nodes, and what feeds each.

### `holon fmt`

```
holon fmt <file>
```

Rewrites the file in the canonical form, keeping its comments. Two flows that are the same graph
format the same.

## `holon serve`

```
holon serve [options]
```

Runs the server, and the editor at its address. A new server prints a one-time setup code for
its first admin. [Server configuration](server.md) covers what it keeps and where.

| Option | |
|---|---|
| `--at [IP:]PORT` | where the server and editor listen. Default `127.0.0.1:7420`; a port alone means 127.0.0.1. Or `HOLON_SERVER_ADDR` |
| `--data DIR` | where everything is kept. Or `HOLON_DATA`; default `~/.holon/data` (or `HOLON_HOME/data`) |
| `--data-host IP` | where deployed flows' own servers listen (default `127.0.0.1`) |
| `--allow-host HOST` | a host name it answers to besides localhost; repeatable |
| `--allow-origin ORIGIN` | an origin the editor may be served from, besides localhost's; repeatable |
| `--keep-runs N` | runs each deployment's log keeps unless its deploy settings say (the `keep_runs` limit's default: 500 without it; or `HOLON_KEEP_RUNS`) |
| `--export-heads URL` | send each run's head to a witness (or `HOLON_EXPORT_HEADS`); its token in `HOLON_EXPORT_HEADS_TOKEN` |
| `--secrets-key os\|env:VAR\|file:PATH` | where the secrets file's key is kept (default: the OS key store) |
| `--new-secrets-key` | print a fresh key for `--secrets-key env:VAR`, and exit |
| `--dotenv FILE` | a `.env` file `HOLON_SECRET_*` values are also read from, again every minute while the server runs ([Secrets from a `.env` file](../guides/deploying.md#secrets-from-a-env-file)) |
| `--editor DIR` | serve the editor from `DIR` instead of the built-in one (or `HOLON_EDITOR`) |
| `--policy PATH` | the policy file (default `holon.toml` in the data folder) |
| `--store`, `--deployments`, `--flows`, `--accounts`, `--secrets`, `--folders`, `--databases`, `--host-key PATH` | where each of those is kept, if not in the data folder. `--store` and `--deployments` also take `:memory:` |

`HOLON_SETUP_CODE` sets the setup code instead of a random one.

## `holon deploy`

```
holon deploy <file> [--server PORT|URL] [--slug NAME] [--approve]
```

Compiles the flow and deploys it, as the editor's deploy does: under its own name, or `--slug`.
From the server you run on this machine, signed in as you; with several running, `--server PORT`
picks one. A server elsewhere: `--server URL` (or `HOLON_SERVER`), with an API token in
`HOLON_TOKEN`.

A first deploy, and a version that asks for something its grant doesn't cover, needs a person's
approval. Without `--approve`, deploy prints what it asks for and exits 1, deploying nothing:
approve it in the editor, by deploying it from there. `--approve` approves it here, as you, and
prints what it approved, in the deployment's timeline as yours. Agents' tools
([`holon mcp`](#holon-mcp)) never approve: the server refuses an agent's deploy that would.

## `holon editor`

```
holon editor [file.holon | slug] [--server PORT|URL]
```

The editor in this terminal, on one screen: a bar along the top with the server and what waits
for you, the pages down the side (folded away below 110 columns), and the keys that work at the
bottom. It reaches the server as `holon deploy` does, signed in as you.

| Page | |
|---|---|
| 1 Overview | what is deployed, what waits for a person, the saved flows |
| 2 Approvals | each thing waiting for you; enter reviews it: a saved or changed flow opens with what it may do, and you approve it by deploying it |
| 3 Flows | every deployment and saved flow; enter opens its page, `e` its text |
| 4 the open flow | its status (routes, what it may do and whether its grant covers it, the secrets it reads), its runs (enter shows one over its text, with each statement's value), its history, state and access, and its graph; `g`, `p`, `x` run, pause or stop it |
| 5 Editor | flows' text in tabs, coloured and checked as you type, with its problems below and, beside it, what it may do (`+` marks what deploying would add) and what the name at the cursor is |
| 6 Library | the samples; enter opens a copy |
| 7 Admin | people (invite, role, email, disable, a reset link, remove), API keys, the folders and databases flows use (bound with the same flags as [`holon folder`](#holon-folder) and [`holon database`](#holon-database)), the secrets each flow reads, and the server: its policy, the grants held, the sign-in provider, sweeping the store and the folder texts. An admin's page |

A flow's graph is its statements, one a row, under what starts them (a route, a clock, an
action), with the lines between them showing what feeds what, as `git log --graph` draws
branches. A statement's own plumbing is folded into it: its detail lists the nodes it is made of,
and `a` shows every node instead. On a deployment it marks what the newest run reached. A tab
shows its code, its code with the graph beside it (following the cursor), or the graph alone:
`ctrl+b` goes round them, and enter on a statement edits it there.

In the editor, keys type; `esc` leaves the text for the page keys and `i` comes back. `ctrl+t`
(or `ctrl+n`, or the tab bar's `+`) opens a draft in a new tab from anywhere (`n` on the
editor's page keys): its first save asks for a file. `ctrl+s` saves (to its file, else on the
server, not deployed), `ctrl+d` deploys, showing what it may do and what you approve by
deploying it, `ctrl+r` shows a run over its text (what it played lit, the rest dimmed, each
statement's value under it). Under it, as in the web editor, a panel: the runs on its left
(`↑`/`↓` pick one), and on its right the run picked, in tabs (`tab` goes round them): Details
(what it answered, did and called), Errors (when it failed: why, and on which line) and its
Timeline, whose steps `←`/`→` go through, each marked in the code and the graph. `alt+f`
formats it, and `ctrl+pgup`/`pgdn` and `ctrl+w` move
between and close tabs. `:` takes a command (`open <file|slug>`, `new [sample]`, `w [file]`,
`deploy`, `flow <slug>`, `call`, `restore`, `sessions`, `run`, `pause` or `stop <slug>`,
`server <port|url>`, `q`), `ctrl+p` finds anything by name, and `?` lists the keys.

The shell line at the bottom right (`!`, or `` ctrl+` `` from anywhere) runs a command in this
terminal, in the folder its prompt shows. The prompt then stays, as the folder is now, for the
next command until `esc` brings the editor back; `cd` stays, and `↑` brings back the ones before.

A file opens in a tab, and a missing one starts as a new flow; a deployment's name opens its page.
With no server it still opens, checks and formats files.

## Running deployments

What the editor's deployed page does, from the command line. Each command below reaches the
server you run on this machine, signed in as you; with several running, `--server PORT` picks
one. A server elsewhere: `--server URL` (or `HOLON_SERVER`), with an API token in `HOLON_TOKEN`.
Those that list take `--json`, for the server's answer as it is.

### `holon deployments`

```
holon deployments [<slug>] [--json]
```

Every deployment, a line each: its status, how it starts (its port, its schedule), its runs and
failures, the runs waiting for an approval or an event, and who deployed it when. `!` marks one
that needs attention. With a slug, that one's details, why it needs attention, and each waiting
run: what it waits for, until when. A waiting run is approved by its own link, on the flow's port.

### `holon run`, `pause`, `stop`, `remove`

```
holon run <slug> [--fresh | --session N]
holon pause <slug>
holon stop <slug>
holon remove <slug> --yes
```

`run` starts a paused or stopped deployment again, from its latest state, from none (`--fresh`),
or from the state session N left (`holon state <slug> --sessions` lists them). `pause` holds its
runs, its state kept; `stop` ends its session. `remove` deletes a stopped deployment and forgets
its secrets; its runs' log stays on disk. Editors and admins.

### `holon call`

```
holon call <slug> METHOD PATH [-H 'Name: value']… [--body TEXT | --body - | --body-file FILE] [--json]
```

Sends a deployed server flow a request, as the editor's request panel does, logged as a run, and
prints its answer's body (its status on stderr; `--json`: status, headers and body). `--body -`
reads the body piped in. Exits 1 when it answers 400 or more. In Git Bash on Windows, set
`MSYS_NO_PATHCONV=1`, or the shell rewrites `/orders` into a Windows path.

### `holon action`

```
holon action <slug> <name> [--input JSON | --input - | --input-file FILE]
```

Runs one of a deployment's actions (`action reset (input) { … }`), as the editor's ▶ does, with
that input (`{}` without one), and prints what it answered and the run's number. A run that waits
for an approval or an event says so. Exits 1 when the action answers 400 or more.

### `holon trigger`, `holon tick`

```
holon trigger <slug> [--body TEXT | --body - | --body-file FILE]
holon tick <slug> <clock>
```

`trigger` runs a flow with no route of its own once (a scheduled flow, say), with that body: what
it produced, and what it did. `tick` fires one of a server flow's clocks now, by its label as
`holon deployments` shows it (`"every 2s"`).

### `holon state`

```
holon state <slug> [--json]
holon state <slug> --collection NAME [--key KEY | --prefix TEXT] [--limit N] [--all]
holon state <slug> --sessions
```

A deployment's live state, a line per value, written as the language writes values; its
collections by name. `--collection`: a collection's items, 50 at a time (`--limit` up to 1000,
`--all` for every one, all read at the same root); `--key`, one item. `--sessions`: each session
it ran, from where, how many runs, and the state it left.

## Grants and policy

### `holon grant`

```
holon grant <slug> [--json]
holon grant <slug> [--revoke ITEM]… [--grant ITEM]… [--lower LIMIT=N]… [--keep-running | --pause-when-uncovered]
```

What a deployment is granted, as its card's grant panel shows it: each item, those revoked
since, the limits lowered, what its version asks for that the grant doesn't cover, and the last
changes. Changed without a redeploy, the whole change or none of it: `--revoke` takes an item
back (written as this prints it: `StateWrite("orders")`), `--grant` gives back one its version
asks for, `--lower` holds a limit below what it was (raising one takes a deploy, which asks for
approval). A grant that no longer covers the deployment pauses it until it does again, unless
`--keep-running`: then the calls it lacks fail. Editors and admins. See [Changing a
grant](../guides/deploying.md#changing-a-grant).

### `holon grants`

```
holon grants [--json]
```

Who holds what: each deployment's granted items and those revoked since, its status, and why it
can't run, if it can't. Admins.

### `holon policy`

```
holon policy [--reload] [--json]
```

The host's policy: what it grants without asking, whether flows may call plain `http` or private
addresses, and each limit's default and ceiling. `--reload` reads the policy file again and holds
every deployment to it at once, saying which it no longer covers (those pause). Editors and
admins reload. See [The host's policy](../guides/deploying.md#the-hosts-policy).

## `holon secret`

```
holon secret list <slug>
holon secret history <slug> <name>
holon secret set <slug> <name> [--from env:HOLON_SECRET_X | --from vault:PATH#FIELD]
holon secret rollback <slug> <name> --to VERSION [--dry-run]
holon secret remove <slug> <name>
holon secret env
```

A deployment's secrets, as its deploy settings show them: `list`, the ones it reads and the
version, source and age of each one set; `history`, one secret's kept versions and what was done
to it, by whom. Never a value. `set` makes a new version: its value piped in (`… | holon secret
set shop api_key`; typed at a terminal, it is refused, since it would show as typed, and it is
never an argument, which the shell's history would keep), or read by the server from its
environment or a vault. `rollback` makes a kept version current again, as a new one; `--dry-run`
names the two values by their fingerprints. `remove` forgets every version. `env`: the
`HOLON_SECRET_*` names the server can read. Admins set, roll back and remove.

## Saved flows and activity

### `holon flows`

```
holon flows [list] [--json]
holon flows show <name> [--out FILE]
holon flows save <file> [--name NAME] [--replace]
holon flows remove <name>
```

The flows saved on the server, as the editor's flows page lists them: who saved each, when, and
whether it is deployed (and whether what is deployed is its latest). `show` prints one's text.
`save` compiles a file and saves it, not deployed, with its text as written, for someone to open
in the editor; a flow someone else saved under that name is kept, unless `--replace`. `remove`
deletes a saved flow; a deployment of it stays.

### `holon activity`

```
holon activity [--last N] [--json]
```

What was done on the server, newest first, and by whom: saves, deploys, runs, pauses, stops,
grants, deletes, bindings, sweeps, people and tokens. The newest 50; `--last` up to 500.

## People and sign-in

Admins only, as the admin page's people and sign-in.

### `holon user`

```
holon user list
holon user invite <name> --role viewer|editor|admin [--email ADDRESS]
holon user set <name> [--role ROLE] [--disable | --enable] [--email ADDRESS]
holon user reset <name>
holon user remove <name> --yes
```

`invite` makes an account and prints the link its person opens to choose a password, once: send
it to them. `set` changes a role, stops someone signing in (`--disable`) or lets them again, or
sets the email they sign in with through a provider (`""` clears it). `reset` clears a password
and ends their sessions, with a new link. The server refuses a change that would leave no admin.

### `holon token`

```
holon token list
holon token add <label> --role viewer|editor|admin|agent
holon token revoke <id>
```

API tokens, for scripts and CI (`HOLON_TOKEN`). `add` prints the new token once, on stdout (what
it is, on stderr), so `holon token add ci --role editor > ci.token` keeps it without showing it.

### `holon oidc`

```
holon oidc [show]
holon oidc set <issuer> <client-id>
holon oidc off
```

Signing in through an identity provider: its issuer, and the client id this server is registered
with there. The server reads the issuer's discovery document before it takes them.

## Runs

### `holon export`

```
holon export <slug> [--server PORT|URL] [--to FOLDER] [--as NAME] [-o PATH]
                    [--max-files-bytes N | --without-files]
```

Writes a deployment's runs, and every unit they ran on, to `<slug>-runs.json`, for `holon
verify`. From the server you run on this machine, signed in as you; with several running,
`--server PORT` picks one. A server elsewhere: `--server URL` (or `HOLON_SERVER`), with an API
token in `HOLON_TOKEN`.

`--max-files-bytes N` (`64K`, `10M`, `2G`) ships at most that much of the file texts the runs
read; `--without-files` ships none. See [Checking runs](../guides/checking-runs.md).

### `holon verify`

```
holon verify <export.json> [--head HEX] [--files DIR]
```

Checks exported runs, offline: the chain, each unit's hash, every run, run again, and that each
run's state follows from the one before. `--head`
checks that a head a witness holds is in the export. `--files` gives the files runs read whose
texts the export doesn't hold. Exits 0 when it all checks, 1 when something doesn't, 2 for a file
it can't read.

## `holon history`

```
holon history <slug> [--last N] [--events] [--server PORT|URL]
```

A deployment's timeline, oldest first: its runs, and its events, what was done to it and by whom
(deploys, restores, grant changes, secrets, bindings for it, pausing and running). `--last N`: the
newest N runs, 20 by default, with the events among them. `--events`: every event, without the
runs. See [The timeline](../guides/deploying.md#the-timeline).

## `holon restore`

```
holon restore <slug> [--server PORT|URL]
holon restore <slug> --to N [--code] [--state] [--dry-run] [--approve] [--server PORT|URL]
holon restore <slug> --to-run SEQ [--code] [--state] [--dry-run] [--approve] [--server PORT|URL]
```

Without a point, lists a deployment's versions, newest first: every deploy is one, kept for good;
`state kept` marks those whose state can be put back.

| Option | |
|---|---|
| `--to N` | version N |
| `--to-run SEQ` | run SEQ of the log |
| `--code` | deploy that point's code again, as a new version, checked against the grant the deployment has now |
| `--state` | put back the state that point left, in a new session; the deployment must be running or paused |
| `--approve` | approve what the code needs that today's grant doesn't cover |
| `--dry-run` | say what it would do: that point's text beside the text deployed now, the state it would put back, and what the runs since did outside the state; change nothing |

At least one of `--code` and `--state`. Exits 0 when it is done (or a dry run answered), 1 when the
server refused it or it needs approving, 2 for a mistake on the command line. See [Going back to an
earlier version](../guides/deploying.md#going-back-to-an-earlier-version).

## `holon folder`

```
holon folder add NAME PATH [--read-only] [--keep hash|30d] [--cap SIZE] [--shared] [--lock run] [--for SLUG]
holon folder list
holon folder remove NAME [--for SLUG]
holon folder sweep
```

Binds a name flows use (`folder inbox`) to a folder on the server's machine. Needs an admin; each
takes `--server PORT|URL`. See [Files on the host](../guides/files.md).

| Option | |
|---|---|
| `--read-only` | flows may read it, not write it |
| `--keep hash` | the run log keeps a hash of what runs read, not the text |
| `--keep 30d` | the text for 30 days (`s`, `m`, `h`, `d`), then its hash |
| `--cap 10M` | no write takes the folder past 10 MiB |
| `--shared` | other servers bind it too: a commit also locks `.holon-tmp-commit.lock` in it |
| `--lock run` | each run that touches it holds it for its whole length, shared to read, alone to write |
| `--for SLUG` | for that deployment only |

`sweep` removes, now, the file texts no kept run refers to (the server does it every ten
minutes).

## `holon store`

```
holon store sweep [--dry-run] [--server PORT|URL]
```

Removes from the server's content store the values nothing kept names: the states of runs the
logs no longer keep (retention cut them, and the log's file has been cut back too: it holds up to
twice what it keeps, so a keep raised later gets them back), and the values only those states
held. A value stays
while a kept run, a version, an event, a session, a waiting run or a live state names it, or a
value they name does; and for an hour after it was put, whoever names it. Code is never swept.
Needs an admin. `--dry-run` says how many values and bytes it would remove, removing nothing.
Prints what it swept, what it keeps, and what it spared as new. An in-memory store
(`--store :memory:`) has nothing to sweep.

## `holon database`

```
holon database add NAME PATH|SOURCE [--read-only] [--schema SQL | --schema-file FILE] [--lock run] [--for SLUG]
holon database list
holon database remove NAME [--for SLUG]
holon database tables NAME [--for SLUG]
```

Binds a name flows use (`database shop`) to a SQLite file on the server's machine (`PATH`), or
to a Postgres, MySQL or MongoDB database whose URL the server fetches from `SOURCE`
(`env:HOLON_SECRET_…`, `vault:<mount>/<path>#<field>`), never storing or showing it. `tables`
lists its tables, read now. Needs an admin; each takes `--server PORT|URL`. See [Databases on the
host](../guides/databases.md).

| Option | |
|---|---|
| `--schema SQL`, `--schema-file FILE` | run it first, creating a SQLite file if there is none (not for a MongoDB) |
| `--read-only` | flows may read it, not write it |
| `--lock run` | each run that touches it holds it for its whole length, shared to read, alone to write |
| `--for SLUG` | for that deployment only |

## `holon mcp`

```
holon mcp [--server PORT|URL]
holon mcp setup [HOST…|--all] [--server PORT|URL] [--project DIR] [--name NAME] [--print] [--remove]
holon mcp hosts [add NAME --config PATH --like HOST | remove NAME]
```

`holon mcp` is an MCP server on stdin and stdout, for an agent's host to start. It reaches the
server you run on this machine (or `--server`, or `HOLON_SERVER`), and always asks as the agent
role. `setup` adds it to a host's settings; `hosts` lists the hosts Holon knows and keeps your
own. See [Working with agents](../guides/agents.md).

| `setup` option | |
|---|---|
| `HOST…` | the hosts to set up (`holon mcp hosts` lists them) |
| `--all` | every host found on this machine |
| `--server PORT\|URL` | a server elsewhere: asks for an API token with role agent, checks it, and keeps it in `~/.holon/tokens.json` |
| `--project DIR` | the hosts' project files in `DIR` |
| `--name NAME` | the entry's name (default `holon`), for a second server |
| `--print` | show the entry and where it goes; write nothing |
| `--remove` | take the entry out (with `--server`, and forget its token) |
| `--config PATH --like HOST [--key KEY]` | a host file Holon doesn't know, written as `HOST`'s is |

## `holon lsp`

```
holon lsp
```

A language server on stdin and stdout, for an editor to start: each mistake `holon check` finds,
as you type; hover; completion; and formatting. It needs no server. The VS Code extension starts
it.

## Looking after the install

| Command | |
|---|---|
| `holon version` | this build's version (also `--version`, `-V`) |
| `holon update [--check] [--to VERSION] [--pre]` | install the newest release over this one; `--check` only says whether there is one. A pre-release build follows pre-releases; `--pre` makes a full release follow them too |
| `holon uninstall [--purge] [--yes]` | remove the program, its PATH entry and install receipt. The data stays; `--purge` removes all of `~/.holon` too, after asking you to type `purge` (`--yes` skips asking) |
| `holon migrate [--yes] [--check] [--from DIR]` | move what releases before 0.1.0-rc.17 left elsewhere (`.holon-*` files in a folder, old secrets keys) into `~/.holon` |

## Environment

| Variable | |
|---|---|
| `HOLON_HOME` | the Holon folder (default `~/.holon`) |
| `HOLON_DATA` | the server's data folder (default `HOLON_HOME/data`) |
| `HOLON_SERVER_ADDR` | `holon serve --at` |
| `HOLON_KEEP_RUNS` | `holon serve --keep-runs` |
| `HOLON_SETUP_CODE` | the first admin's setup code |
| `HOLON_EXPORT_HEADS`, `HOLON_EXPORT_HEADS_TOKEN` | the witness heads are sent to, and its token |
| `HOLON_SECRET_<NAME>` | values for secrets whose source is env |
| `VAULT_ADDR`, `VAULT_TOKEN`, `VAULT_NAMESPACE` | for secrets whose source is vault |
| `HOLON_SERVER` | the server `export`, `folder` and `mcp` reach |
| `HOLON_TOKEN` | an API token to sign in to it with |
| `HOLON_EDITOR` | `holon serve --editor` |
