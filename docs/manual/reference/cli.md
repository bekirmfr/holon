# Command line

Everything is one program, `holon`. `holon help` lists the commands; `serve`, `export`,
`verify`, `load`, `folder`, `database`, `mail`, `feed`, `queue`, `mcp` and `lsp` say more with `--help`.

| Command | |
|---|---|
| [`holon serve`](#holon-serve) | run the server, with the editor at its address |
| [`holon new`](#holon-new) | write a starter flow, or a sample |
| [`holon check`](#holon-check) | compile a flow: its routes, what it may do, or each mistake |
| [`holon show`](#holon-show) | an outline of a flow |
| [`holon fmt`](#holon-fmt) | rewrite a flow in the canonical form |
| [`holon types`](#holon-types) | write `holon.d.ts`, so an editor's TypeScript completes and checks flows |
| [`holon deploy`](#holon-deploy) | compile a flow and deploy it to a running server |
| [`holon replay`](#holon-replay) | a new version run offline on a deployment's recent runs, before it is deployed |
| [`holon editor`](#holon-editor) | the editor in a terminal: flows, runs, approvals, and text in tabs |
| [`holon deployments`](#holon-deployments) | what is deployed, and each one's waiting runs |
| [`holon run`, `pause`, `stop`, `remove`](#holon-run-pause-stop-remove) | start, hold, end and delete a deployment |
| [`holon call`](#holon-call) | send a deployed flow a request |
| [`holon load`](#holon-load) | a deployed flow's routes under load: requests a second, latency, and what the server did with the runs |
| [`holon action`](#holon-action) | run one of a deployment's actions |
| [`holon tick`](#holon-tick) | fire one of a flow's clocks now |
| [`holon state`](#holon-state) | a deployment's state, collections and sessions |
| [`holon decide`](#holon-decide) | approve, or decline with a reason, an approval a run waits at |
| [`holon logs`](#holon-logs) | the lines a flow wrote with `console.log`, kept apart from its runs |
| [`holon wait`](#holon-wait) | hold until a person decides a flow's proposal, and say how and why |
| [`holon grant`](#holon-grant) | what a deployment is granted; revoke, grant back, lower |
| [`holon grants`](#holon-grants) | who holds what |
| [`holon policy`](#holon-policy) | the host's policy, and reading it again |
| [`holon host`](#holon-host) | what the host holds: memory, its disks, what it keeps on them, its sweeps |
| [`holon secret`](#holon-secret) | a deployment's secrets: set, roll back, remove, their history; the server's own, and where each may go |
| [`holon flows`](#holon-flows) | the flows saved on the server |
| [`holon activity`](#holon-activity) | what was done on the server, by whom |
| [`holon people`, `oidc`](#people-and-sign-in) | people, agents and services, and their tokens; signing in through a provider |
| [`holon roles`](#holon-roles) | the roles principals are bound, and defining your own |
| [`holon groups`](#holon-groups) | groups of principals, to bind a role to all of them |
| [`holon bind`, `unbind`](#holon-bind) | a role to a principal or a group: on the host, a flow, a tag or an approval point, until a time |
| [`holon access`](#holon-access) | who may do what, every version of it, for an audit |
| [`holon export`](#holon-export) | write a deployment's runs to a file |
| [`holon verify`](#holon-verify) | check exported runs offline |
| [`holon history`](#holon-history) | a deployment's timeline: its runs, and what was done to it |
| [`holon restore`](#holon-restore) | a deployment's versions, and an earlier version's or run's code or state back |
| [`holon folder`](#holon-folder) | bind the folders flows use |
| [`holon store`](#holon-store) | sweep what nothing kept names from the content store |
| [`holon database`](#holon-database) | bind the databases flows use, and list their tables |
| [`holon mail`](#holon-mail) | bind the mail flows send through and receive |
| [`holon feed`](#holon-feed) | bind the feeds flows listen to: server-sent events, WebSockets, Postgres channels |
| [`holon queue`](#holon-queue) | bind the SQS queues flows take messages from and send to |
| [`holon mcp`](#holon-mcp) | the tools for agents, and adding them to an agent's host |
| [`holon lsp`](#holon-lsp) | a language server for editors |
| [`holon version`, `update`, `uninstall`, `migrate`](#looking-after-the-install) | the install itself |

## Install

The install scripts are on each [release](https://github.com/bekirmfr/holon/releases):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/download/v0.1.0-rc.35/holon-installer.sh | sh
```

```powershell
cd $env:TEMP
irm https://github.com/bekirmfr/holon/releases/download/v0.1.0-rc.35/holon-installer.ps1 -OutFile holon-installer.ps1
powershell -ExecutionPolicy Bypass -File .\holon-installer.ps1
```

They put `holon` in `~/.holon/bin` and add it to your PATH. `HOLON_INSTALL_DIR` installs
elsewhere; `HOLON_NO_MODIFY_PATH=1` leaves PATH alone. The URLs name the version: GitHub's
`latest` skips pre-releases, and every release is one until 0.1.0, so each release's page has
its own.

On Windows the script is downloaded, then run. Run straight from the download (`irm … | iex`),
Windows PowerShell 5.1 can hand it over a line at a time and fail ("The terminator '#>' is
missing"), and Microsoft Defender flags `powershell -ExecutionPolicy Bypass -c "irm … | iex"`.
If an antivirus stops the script anyway, download `holon-x86_64-pc-windows-msvc.zip` from the
release and put `holon.exe` on your PATH: everything works but `holon update`.

Each release also has the program for each platform as an archive, with checksums. A download can
be checked against this repository's release workflow with
`gh attestation verify <archive> --repo bekirmfr/holon`.

## Flows as text

These work on any machine, with no server.

### `holon new`

```
holon new <file> [--sample <name>]
```

Writes a starter flow to `<file>` (a `NAME.holon.ts`), or the sample named, whose comments say
what it does and how to try it. It never overwrites a file.
`holon new --help` lists the samples. Beside it, it writes what [`holon types`](#holon-types)
does.

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

A websocket is listed under its server with the blocks it has, `WebSocket /room/:name: open,
message, close`. A flow that touches nothing outside says `it only computes: it reads and changes
nothing outside`. Each mistake goes to stderr as `file:line:col: message`, with the line and a caret
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

Lays the file out in the canonical form, as Prettier does, at 100 columns and without
semicolons, keeping its comments and blank lines. It writes only a text that compiles to the same
flow.

### `holon types`

```
holon types [dir]
```

Writes `holon.d.ts` into `dir`, the current folder when left out: the types of `"holon"` and of
every import source (`secret:`, `host:`, `collection:`, …), so an editor's TypeScript completes
`route.post`, shows each leaf's signature on hover, and marks a misspelled method, option or
import as you type. A `tsconfig.json` is written beside it when there is none: no DOM and no
Node, so `fetch` and `console` are unknown there, as they are to Holon. A `holon.d.ts` that holon
didn't write is left as it is. The leaves it lists are this `holon`'s, so run it again after an
upgrade. Holon's compiler decides what a flow means; one difference TypeScript can't be told is in
[the reference](../language/reference.md#files).

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
| `--allow-host HOST` | a host name it answers to besides localhost, on its own port and each flow's (one a proxy in front forwards); repeatable |
| `--allow-origin ORIGIN` | an origin the editor may be served from, besides localhost's; repeatable |
| `--keep-runs N` | runs each deployment's log keeps unless its deploy settings say (the `keep_runs` limit's default: 500 without it; or `HOLON_KEEP_RUNS`) |
| `--log-stdout` | also print each line a flow writes with `console.log` as it is kept, one to a line, `11:48:03.678 traced-payout: LOG   run 0  payout asked p-1 250` (or `HOLON_LOG_STDOUT=1`): for a server run under systemd or in a container that collects what it prints |
| `--log-size SIZE`, `--log-files N`, `--log-rate N` | how each deployment's log lines are kept: the size of each file (`64K` at least; default `2M`), how many files (1 to 10; default 3), and the most lines a second it writes (default 1000), the rest dropped and counted; or `HOLON_LOG_SIZE`, `HOLON_LOG_FILES`, `HOLON_LOG_RATE` ([`holon logs`](#holon-logs)) |
| `--flow-cache SIZE` | memory kept for the flows runs have loaded, so a run doesn't read its flow from the store again: `64M`, `1G`, or `0` to keep none (default `256M`; or `HOLON_FLOW_CACHE`). What it counts is the memory a flow takes once loaded (measured within a few percent, rounded up so it keeps a little less, never more), each unit once, however many flows place it; past the size, the flows used least recently are let go and load again when next run. Beside it, the store keeps the values runs read from disk (a collection's items, state), up to `--value-cache` |
| `--value-cache SIZE` | memory kept for the values runs read from the store's disk (a collection's items and trie, a file's text by hash), so a read doesn't open the file again: `64M`, `1G`, or `0` to keep none (default `64M`; or `HOLON_VALUE_CACHE`). What it counts is the memory a value takes in it. Past the size, the values used least recently are let go |
| `--export-heads URL` | send each run's head to a witness (or `HOLON_EXPORT_HEADS`); its token in `HOLON_EXPORT_HEADS_TOKEN` |
| `--secrets-key os\|env:VAR\|file:PATH` | where the secrets file's key is kept (default: the OS key store) |
| `--new-secrets-key` | print a fresh key for `--secrets-key env:VAR`, and exit |
| `--dotenv FILE` | a `.env` file `HOLON_SECRET_*` values are also read from, again every minute while the server runs ([Secrets from a `.env` file](../guides/deploying.md#secrets-from-a-env-file)) |
| `--editor DIR` | serve the editor from `DIR` instead of the built-in one (or `HOLON_EDITOR`) |
| `--policy PATH` | the policy file (default `holon.toml` in the data folder) |
| `--store`, `--deployments`, `--flows`, `--accounts`, `--secrets`, `--folders`, `--databases`, `--mails`, `--feeds`, `--queues`, `--host-key PATH` | where each of those is kept, if not in the data folder. `--store` and `--deployments` also take `:memory:`. The mail on its way waits in `mail-spool` beside `--mails`, the messages to queues in `queue-spool` beside `--queues` |

`HOLON_SETUP_CODE` sets the setup code instead of a random one.

Ctrl+C (or SIGTERM, or closing its console) stops it: it takes no new runs, finishes those going
on, syncs its logs and exits. A second ctrl+c stops it at once ([Stopping
it](server.md#stopping-it)).

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

## `holon replay`

```
holon replay <file> [--on SLUG] [--runs N] [--server PORT|URL]
```

Compiles the flow and runs it, offline, on the newest runs of the deployment `--on` names (the
flow's own name unless it says), 20 unless `--runs` says (at most 200), before anything is
deployed: each run's request, tick or action again, through this version. It prints how many it
answers the same, then each run that differs, and how, or can't be told, and why:

```
This version replayed on "price-rules"'s last 3 runs, offline (nothing sent or written): 3 replayed · 2 same · 1 differs
  run 0  POST /price  differs: its answer
```

Nothing is deployed, sent or written; only the version's units are staged on the server. It exits 0
when no run differs, 1 when one does or it can't replay, 2 for a wrong argument, so a script can
hold back a deploy that would answer differently. What "can't tell" means is under
[Replayed on recent runs](../guides/deploying.md#replayed-on-recent-runs). It reaches the server as
`holon deploy` does.

## `holon editor`

```
holon editor [file.holon.ts | slug] [--server PORT|URL]
```

The editor in this terminal, on one screen: a bar along the top with the server and what waits
for you, the pages down the side (folded away below 110 columns), and the keys that work at the
bottom. It reaches the server as `holon deploy` does, signed in as you. The sidebar ends with the
server's health: what runs, what is paused, and whether the run logs' chains hold. The top bar
says where you are after the server's name (`server › Flows › refunds › Runs`), a click on a
crumb going back to it; the line above the keys says what just happened.

| Page | |
|---|---|
| 1 Overview | the page in a line, the last day's runs and failures, what is deployed (each with its version, its last day and its last run, and `v4 waits for you` when a version saved since waits), what waits for a person with what it would add (`+`), and what was done lately |
| 2 Approvals | each thing waiting for you, and beside it the one picked, reviewed as its deploy would be, with what it may do once approved (`+` the new); enter approves it there, `→` reads the review, `]` `[` go to the next and the one before, `e` opens it in the editor |
| 3 Flows | every deployment and saved flow; enter opens its page, `e` its text; `/` filters them, `f` goes round all, live, waiting and paused |
| 4 the open flow | its status (routes, what it may do and whether its grant covers it, what a version waiting would add, the secrets it reads), its runs (enter shows one over its text, with each statement's value; `/` filters them, `i` explains one, `F` follows the newest, `o` writes them to `<slug>-runs.json` for [`holon verify`](#holon-verify); each run says where it is on record; `L` opens its log lines), its logs (what its flow wrote with `console.log`, a line each, the newest picked as they come: `w` shows warnings and louder, then errors, `u` the picked line's run's lines up to its end, then its own, `F` pauses, enter steps through the run; [`holon logs`](#holon-logs) reads the same), its history, state and access (enter on a secret sets it: a value typed as dots, a `HOLON_SECRET_…` name the server reads, or a Vault field), and its graph; its status says what it holds in memory and on disk; `g`, `p`, `x` run, pause or stop it |
| 5 Editor | flows' text in tabs, coloured and checked as you type, with its problems below and, beside it, what it may do (`+` marks what deploying would add) and what the name at the cursor is; each tab's dot is its deployment's state, and the text's title says how it stands against what is live (`v3, live`, `changed from v3 *`). `⌂ Home` starts a draft or opens a sample, a tab closed lately or a flow on the server |
| 6 Library | the samples, with what each shows and its `holon new` line; enter opens a copy, `/` filters them |
| 7 Admin | people (invite, role, email, disable, a reset link, remove), API keys, the folders, databases, mail and feeds flows use (bound with the same flags as [`holon folder`](#holon-folder), [`holon database`](#holon-database), [`holon mail`](#holon-mail) and [`holon feed`](#holon-feed)), the secrets each flow reads, and the server: its policy, the grants held, the sign-in provider, sweeping the store and the folder texts. An admin's page |

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
deploying it (`s` there saves it on the server instead, for whoever may approve it), `ctrl+r` shows a run over its text (what it played lit, the rest dimmed, each
statement's value under it; `=` or the tab bar's Values hides the values for the code alone,
and shows them again). Under it, as in the web editor, a panel: the runs on its left
(`↑`/`↓` pick one), and on its right the run picked, in tabs (`tab` goes round them): Details
(what it answered, did and called), Errors (when it failed: why, and on which line) and its
Timeline, whose steps `←`/`→` (or a click on the line under the code) go through, each marked
in the code and the graph, coloured by what it touches (its data purple, a call outside teal);
`,` and `.` go to the run before and after. `alt+f`
formats it, and `ctrl+pgup`/`pgdn` and `ctrl+w` move
between and close tabs. `:` takes a command (`open <file|slug>`, `new [sample]`, `w [file]`,
`deploy`, `flow <slug>`, `call`, `restore`, `sessions`, `run`, `pause` or `stop <slug>`,
`server <port|url>`, `theme [light|dark]`, `q`), suggested as it is typed (`tab` takes the
first), `ctrl+p` finds anything by name, and `?` lists the keys. Every line you type (`:`,
`/`, find, the palette, the shell line, a dialog's field) edits where its cursor is: `←`/`→`,
`home`/`end`, `del` and backspace, and a click puts the cursor there. On a light terminal,
`:theme light` (or `HOLON_THEME=light`) makes its dark grounds light.

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
run: what it waits for, until when, and who has approved an approval so far. People decide an
approval in the editor's Approvals or with [`holon decide`](#holon-decide); an event comes through
the flow's own route.

### `holon run`, `pause`, `stop`, `remove`

```
holon run <slug> [--fresh | --session N]
holon pause <slug>
holon stop <slug>
holon remove <slug> --yes
```

`run` starts a paused or stopped deployment again, from its latest state, from none (`--fresh`),
or from the state session N left (`holon state <slug> --sessions` lists them). `pause` holds its
runs, its state kept; `stop` ends its session. `remove` deletes a stopped deployment for good:
its runs' log, its timeline, its state and its secrets (`holon export` first keeps its runs).
Editors and admins.

### `holon call`

```
holon call <slug> METHOD PATH [-H 'Name: value']… [--body TEXT | --body - | --body-file FILE] [--json]
```

Sends a deployed server flow a request, as the editor's request panel does, logged as a run, and
prints its answer's body (its status on stderr; `--json`: status, headers and body). `--body -`
reads the body piped in. Exits 1 when it answers 400 or more. In Git Bash on Windows, set
`MSYS_NO_PATHCONV=1`, or the shell rewrites `/orders` into a Windows path.

### `holon load`

```
holon load <slug> [--route "METHOD /path"]… [--for SECONDS] [--connections N] [--body JSON]
                  [--to http://HOST:PORT] [--json]
```

Sends a deployed flow's routes as many requests as they answer, one route after another, from N
connections kept open (default 8), for SECONDS each (default 10), and prints a line a second for
each: requests answered, p50 and p99. Then the routes side by side:

```
route            log         req/s   p50      p90      p99      max      errors  recorded  syncs   counted
GET /strict      strict      7,731   1.85 ms  2.99 ms  5.41 ms  36.9 ms  0       77,309    12,136  0
GET /lazy        lazy        14,596  0.83 ms  1.96 ms  4.02 ms  73.6 ms  0       145,959   9       0
GET /unrecorded  unrecorded  49,110  0.32 ms  0.48 ms  0.70 ms  40.8 ms  0       0         0       491,104
```

`errors` are answers other than 2xx and requests with no answer, each kind said below the table.
The last three columns are the server's, read before and after each route: the runs it wrote to
the log, the times it synced the log to disk for them (one sync covers every run written before
it, so strict runs arriving together share one), and the runs it only counted (unrecorded). They
are the deployment's: other requests to it meanwhile are in them.

Its routes are every GET route, strict first, then lazy, then unrecorded, a path parameter sent as
`1`; `--route` names others (repeat it for more, loaded in the order given; any method, a path
parameter given or not), and a route other than GET is sent `--body` (`{}` without one). The flow's port is on the server's host, over
plain HTTP; `--to` names where else it answers, such as a proxy's address. `--json` prints all of
it as JSON, each second's numbers too (the live lines still go to stderr). Exits 1 when nothing
answered a route. The [`stress-test`](../../../crates/holon-author/samples/stress-test.holon.ts)
sample has one route of each log level. Load from the same machine shares its processors with the
server, so read a route beside another rather than its numbers alone. Anyone signed in, a
viewer too: it only reads the server, and sends the flow's port what anyone may.

```
holon load stress-test
holon load stress-test --for 5 --connections 32
holon load stress-test --route "GET /strict" --route "GET /unrecorded"
holon load notes-api --route "PUT /notes/7" --body '{"text":"hi"}'
holon load stress-test --json > load.json
```

### `holon action`

```
holon action <slug> <name> [--input JSON | --input - | --input-file FILE]
```

Runs one of a deployment's actions (`export const reset = action(…)`), as the editor's ▶ does, with
that input (`{}` without one), and prints what it answered and the run's number. A run that waits
for an approval or an event says so. Exits 1 when the action answers 400 or more.

### `holon tick`

```
holon tick <slug> <clock>
```

Fires one of a flow's clocks now, by its label as `holon deployments` shows it (`"every 2s"`):
what the run did, as a tick on its own would.

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

### `holon logs`

```
holon logs <slug> [--level LEVEL] [--run N | --until N] [--limit N] [--follow] [--json]
```

The lines a deployment's flow wrote with `console.log`, `info`, `debug`, `warn` and `error`,
oldest first, each with when, its level and the run that wrote it:

```
21:15:33.419 LOG   run 3  payout asked p-1 250
21:15:33.420 WARN  run 3  over the usual limit p-2
```

The host keeps them beside the deployment's runs: none is an effect a run performed,
[`holon verify`](#holon-verify) checks a run without them, and a flow can't read them back.
`--level warn` shows that level and louder (`log` and `info` are the same); `--run N` only the
lines run N wrote, `--until N` every line up to the end of run N's; `--limit N` the newest N (200
if left out); `--follow` keeps reading as new lines come; `--json` prints each line as JSON. A
deployment keeps three files of 2 MB by default (`holon serve --log-size`, `--log-files`): the oldest
lines go first, and a line says when some were dropped. A person signed in reads them. An agent, `holon mcp`, only where the deployment's grant lets agents
(`AgentReadsLogs`, asked for by the deploy settings' **agents may read its logs**, [permissions](permissions.md#who-may-read-a-flows-log-lines)):
its `logs` tool answers, and otherwise is refused.

### `holon decide`

```
holon decide <slug> [--json]
holon decide <slug> <run> <name> --approve [--reason TEXT] [--slot N] [--note JSON]
holon decide <slug> <run> <name> --decline --reason TEXT [--slot N] [--note JSON]
```

Decides an approval a run waits at (`await approval("payout", { people: 2 })` in its flow), as
the editor's Approvals page does. With the slug alone, what its runs wait at, a line a point: the
run (the first 12 characters of its id), what started it and since when, then an approval with
who decides it and each step as it stands, who approved it so far or whether it is open, or an
event, and until when:

```
run 7b379ca1dd65 (action pay, since 2026-11-09T14:42:57) waits at approval "payout" (2 people): 1 of 2: dan ("matches the invoice") approved, until 2026-11-09T15:42:57
run 0c2e81f4a9d3 (action pay, since 2026-11-09T14:50:12) waits at approval "release" (an agent holding classifier, then a person holding reviewer): classifier scout approved; reviewer open, until 2026-11-09T15:50:12

Yours to decide:
  run 0c2e81f4a9d3, "release": reviewer (slot 1)
```

Then `<run>` (its id, or enough of it to pick it) and the approval's name, and `--approve`, the
reason optional, or `--decline`, which needs one. You take the open step you may fill: of your
kind, bound to its role where the flow is; when you may fill several, `--slot N` names one (from
0, as the steps are written, and as *Yours to decide* numbers them). `--note` sends any JSON with
the decision, up to 16 KiB, which the run is given with it. It says what the decision did: the
step it took, what the approval still waits for, approved by whom, or declined; the run goes on
with the decision once it is made. Each decides one step, once, and not of a run they started;
one decline ends it, and the host's [approval settings](#holon-access) may keep out whoever
deployed the version, and agents. A person decides a person's step, signed in, as you are on
your own machine. An agent decides a step written for an agent with its token in `HOLON_TOKEN`
(or `holon mcp`'s `decide`), never a person's; a service's token is refused.

### `holon wait`

```
holon wait <slug> [--timeout SECONDS] [--json]
```

Holds until a person has decided the proposal a flow's deploy made, then prints how they decided,
and the reason of a decline. A deploy that needs approval and comes from an agent (`holon mcp`'s
`deploy`, or an agent's API token) waits in the editor's Approvals, and the agent has nothing to
do until then. An agent with a shell runs this in the background after it proposes; the command
ends when the decision is made, which a host that restarts an agent when a background command
ends takes as its message. Already decided, it prints that at once.

```
proposal 1 was declined by bma at 2026-11-10 09:05:31 UTC: make the nodes grouped by function area
```

It ends with 0 when the proposal was approved (and so deployed), 1 when it was declined, 2 when
the flow has no proposal, 3 when a newer proposal replaced it (it follows the newer one while it
waits), and 4 when `--timeout` ran out with nobody having decided. It decides nothing; that is
[`holon decide`](#holon-decide) for an approval a run waits at, and the Approvals page for a
proposal. Where there is no shell, `holon mcp`'s `deployments` with a slug and `wait` holds the
same way, for up to 45 seconds a call, or up to 600 when the request asks for progress
(`holon mcp` answers it with a notification every second, and stops it when the host cancels).

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
holon policy [--reload] [--sweep-every 10m|off] [--warn-at 85%|5G] [--full-at 95%|1G] [--when-full warn|pause|roll] [--backup DIR|none] [--json]
```

The host's policy: what it grants without asking, whether flows may call plain `http` or private
addresses, how often the server sweeps, and each limit's default and ceiling. `--reload` reads the
policy file again and holds every deployment to it at once, saying which it no longer covers
(those pause). Editors and admins reload. `--sweep-every` sets how often the server sweeps what
nothing kept names (`90s`, `10m`, `2h`, `1d`, a minute at the least, or `off`), written into the
policy file's [`[storage]`](server.md#the-policy); an admin's. `--warn-at` sets when a disk is
full enough to warn of: how full for the server (`85%`), or how much is left (`5G`); an admin's,
written there too ([Warnings](server.md#warnings)). `--full-at` and `--when-full`: past where it
warns, when it acts and what it does then: `warn`, `pause` (every flow that records runs, until
there is room) or `roll` (cut the oldest runs of the biggest logs); an admin's
([The policy](server.md#the-policy)). `--backup DIR` keeps the runs logs let go of there, as
gzipped exports (`none`: they go); an admin's. See [The host's policy](../guides/deploying.md#the-hosts-policy).

### `holon host`

```
holon host [--for SLUG] [--json] [--server PORT|URL]
```

What the host warns of, first (each with what to do), then what it holds: the server's memory and
its caches against their sizes; each disk it keeps
things on and how full it is for the server; what it keeps there (the content store's values,
the logs, the folders' kept texts, counted at start and after each round of sweeps, and what a
crash left, removed once old); and how often it sweeps, when next, and what the last sweeps
removed; and what each flow holds, the most first. An admin's. `--for SLUG`: what one flow holds,
in memory and on disk (anyone who may read the deployment). See [What it holds](server.md#what-it-holds).

```
memory       742 MiB (at most 811 MiB so far)
  flow cache   12 flows, 32 MiB of 256 MiB (12%)
  value cache  1234 values, 48 MiB of 64 MiB (75%)
disk         60% full: 40 GiB free of 100 GiB (60 GiB in use): data, logs, state heads, store (/srv/holon/data)
kept         counted 3 min ago
  store        432808 values, 2 GiB on disk (1 GiB of bytes); 3200 put since
  logs         34 logs, 192 MiB on disk
sweeps       every 10m; the next in 7 min
  store        last: 3 min ago: 271 values, 56 KiB freed in 8 s
  file texts   last: 3 min ago: nothing to remove (0 s)
```

## `holon secret`

```
holon secret list <slug>
holon secret history <slug> <name>
holon secret set <slug> <name> [--from env:HOLON_SECRET_X | --from vault:PATH#FIELD]
holon secret rollback <slug> <name> --to VERSION [--dry-run]
holon secret remove <slug> <name>
holon secret bind <name> [--from SOURCE] [--to HOST,…]
holon secret cap <name> --to HOST,… | --anywhere
holon secret bound
holon secret unbind <name>
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

`bind` sets the server's own secret, read by every deployment that reads a secret of that name
and has none of its own (one set for a deployment overrides it), its value piped in or `--from`,
as `set`. `--to` says where it may go: hosts (`api.stripe.com`, with `:port` if not 443) and
databases (`db:shop`). A deploy whose flow sends it elsewhere is refused, and a request a run
makes with it to another host isn't sent (`blocked_secret`); a deployment with its own isn't
held to it. `cap` changes only where it may go (`--anywhere`: wherever its flows' approvals
show), from the next request. `bound` lists the server's secrets and where each may go;
`unbind` removes one. See [Deploying](../guides/deploying.md#the-servers-own-secrets).

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

Admins only, as the editor's People and access; an agent's or a service's owner also makes and
revokes its tokens (in the editor, *Your agents and services*).

### `holon people`

```
holon people [list]
holon people add <name> --person --role ROLE [--email ADDRESS]
holon people add <name> --agent [--role ROLE] [--owner PERSON]
holon people add <name> --service --role ROLE [--owner PERSON]
holon people set <name> [--role ROLE] [--disable | --enable] [--email ADDRESS] [--owner PERSON] [--display TEXT]
holon people reset <name>
holon people remove <name> --yes
holon people token <name> [--label TEXT]
holon people revoke <name> <id>
```

Everyone who acts on the server is one of three kinds, each name unique across them. A **person**
signs in with a password or through a provider: `add --person` prints the link they open to
choose a password, once, to send them. An **agent** (an AI through `holon mcp`, or one with a
shell) and a **service** (a script, CI) sign in with tokens of their own, and each has an
**owner**, the person answerable for it (whoever adds it, unless `--owner` names another). An
agent's role is `agent` (it reads, proposes and calls, never approves) or `viewer`; a service's is
`viewer` or `editor`, never `admin`; any of them may instead hold a role an admin defined for its
kind ([`holon roles`](#holon-roles)). That `--role` is their role on the whole host;
[`holon bind`](#holon-bind) binds one on a single flow or approval point instead. `list` shows what
each holds, bound to them or to their groups: `viewer, editor on payout via payments`.

`set` changes their role on the host (one the kind may hold), stops someone signing in (`--disable`: an agent's or
a service's tokens are refused) or lets them again, sets a person's email for a provider (`""`
clears it), or gives an agent or a service another owner. `reset` clears a person's password and
ends their sessions, with a new link. `remove` takes an agent's or a service's tokens with it, and
is refused for a person who still owns one: give it another owner first. The server refuses a
change that would leave no admin.

`token` makes a token for an agent or a service and prints it once, on stdout (what it is, on
stderr), so `holon people token ci --label "CI on GitHub" > ci.token` keeps it without showing it;
`list` shows each token's id, and `revoke` ends one at once. Its owner or an admin may do either.

### `holon oidc`

```
holon oidc [show]
holon oidc set <issuer> <client-id>
holon oidc off
```

Signing in through an identity provider: its issuer, and the client id this server is registered
with there. The server reads the issuer's discovery document before it takes them.

## Roles and bindings

Who may do what is a role bound to a principal or a group, where it holds: on the whole host, on
one flow, or at one approval point of a flow, until a time if you say. Every change is a version
kept for good, with who made it, when and why (`--why`). Admins, or whoever holds a role allowing
`role.define` (roles) or `people.assign` (groups and bindings), and `role.view` and `people.view`
to read them. No one gives more
than they hold: a role allows, a binding gives and a member added to a group gets only what its
author holds there. See [People and roles](../guides/people.md).

### `holon roles`

```
holon roles [list] [--json]
holon roles define <name> --allow ACTION,… [--kinds KIND,…] [--about TEXT] [--why TEXT]
holon roles remove <name> [--why TEXT]
```

The roles principals are bound: the built-in ones (viewer, agent, editor, admin), then the ones an
admin defined, each with who may hold it, how many bindings hold it, and what it allows. `define`
makes a role of your own, or a new version of one, which changes each binding of it: `--allow`
names actions from the catalogue (`run.view,run.decide`; a mistake lists them), `--kinds` who may
hold it (`person`, the default; `agent`, `service`). Whatever role it holds, an agent never
approves a deploy, starts, stops, deletes or restores a flow, writes a secret, takes a person's
step of an approval, operates the host, or reads or changes people, roles or resources; a service
never decides an approval. `remove` takes a role nothing binds any more.

```sh
holon roles define payout-approver --allow run.view,run.decide --about "decides payouts"
```

### `holon groups`

```
holon groups [list] [--json]
holon groups add <group> <name>… [--about TEXT] [--why TEXT]
holon groups drop <group> <name>… [--why TEXT]
holon groups remove <group> [--why TEXT]
```

Groups of principals: a role bound to a group holds for each member whose kind the role allows.
`add` makes the group or adds members to it, `drop` takes them out, `remove` deletes a group
nothing is bound to any more.

### `holon bind`

```
holon bind <role> (--to NAME | --group GROUP) [--flow SLUG | --tag TAG | --approval FLOW:POINT]
           [--until DATE|WHILE] [--why TEXT] [--json]
holon unbind <id> [--why TEXT]
```

Binds a role to a principal or a group: on the host, unless `--flow` names one flow (its runs,
state, secrets and approvals), `--tag` a tag (every flow deployed with it, while it is: a flow's
text tags it, `flow("payout", { tags: ["payments"] })`), or `--approval` one approval point of a
flow (deciding it, and nothing else, not even viewing its runs). `--until` takes a date in UTC (`2026-12-01`,
`2026-12-01T09:00`) or a while (`30m`, `12h`, `7d`, `2w`); an expired binding holds nothing, and
stays until unbound. It prints the binding's id, which `unbind` takes (`holon access show` lists
them all). Binding at an approval point, it says when they can't see the flow's runs, and the
command that lets them.

```sh
holon bind editor --to bob --flow payout --why "owns the payout flow"
holon bind editor --to bob --tag payments --why "on the payments team"
holon bind payout-approver --group payments --approval payout:ok --until 7d
holon unbind 12 --why "back from leave"
```

### `holon access`

```
holon access [versions] [--json]
holon access show [--version N] [--json]
holon access approvals [--exclude-deployer on|off] [--agents on|off] [--why TEXT] [--json]
holon access local [--as NAME] [--why TEXT] [--json]
```

Who may do what on this server, as kept: `versions` (what `holon access` does alone) lists every
version, newest first, with when, by whom, what it changed and why; `show` prints one whole, the
newest by default: its approval settings, roles, groups and bindings, each binding with its id,
where it holds, until when and who made it. It holds no password or token, so it can be handed to
an auditor.

`approvals` shows how this server decides approvals beyond each step's role, and with a flag sets
it, as a new version (holding `people.assign` on the host):

```
exclude-deployer  on   whoever deployed the version a run is on decides none of its approvals
agents            on   agents take the steps written for them
(version 7)
```

`--exclude-deployer on` keeps whoever deployed the version a run is on from deciding any of its
approvals, whatever they hold (off by default). `--agents off` lets no agent decide one: a flow
whose approval has a step for an agent doesn't deploy, and an agent takes no step of a run
already waiting (on by default). Who started a run never decides its approvals, whatever these
say.

`local` says who this machine's run file signs in as: the person who set the server up, so the
commands run on its machine (`holon decide` among them) are theirs. `--as NAME` ties it to
another person here who isn't disabled, as a new version (holding `people.assign` on the host):

```
this machine's run file signs in as bob (version 9)
```

Tied to no one (its person removed, or disabled), the run file is the OS user's admin,
`local:<user>`, and decides no approval.

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
holon verify <export.json | export.json.gz> [--head HEX] [--files DIR]
```

Checks exported runs, offline (an export, or a backup's gzipped one, [`[storage]
backup`](server.md#the-policy)): the chain, each unit's hash, every run, run again, and that each
run's state follows from the one before. `--head`
checks that a head a witness holds is in the export. `--files` gives the files runs read whose
texts the export doesn't hold. Exits 0 when it all checks, every run run again; 1 when something
doesn't check; 3 when nothing fails but some runs couldn't be run again (it says which, and why);
2 for a file it can't read.

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
| `--dry-run` | say what it would do: that point's text beside the text deployed now; what it touches, a row per folder, database, secret, host, flow, channel, state key and collection, marked new, gone or as now, with whether this server is ready for it, what putting the state back does to it, and what the runs since did through it (not undone); the state's values now → as it would be; change nothing |

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

Binds a name flows use (`"folder:inbox"`) to a folder on the server's machine. Needs an admin; each
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

Removes from the server's content store, now, the values nothing kept names (the server does it
on its own every ten minutes): the states of runs the
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

Binds a name flows use (`"database:shop"`) to a SQLite file on the server's machine (`PATH`), or
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

## `holon mail`

```
holon mail add NAME [--smtp SOURCE --from SENDER --to LIST]
                     [--imap SOURCE [--folder F] [--done F] [--verdict-from SERVER] [--every T]]
                     [--oauth SOURCE] [--for SLUG]
holon mail list
holon mail remove NAME [--for SLUG]
```

Binds a name flows import (`"mail:ops"`) to an SMTP server they send through (`--smtp`), a mailbox
whose messages start their runs (`--imap`, `on.mail(ops, …)`), or both. Each server's URL, with its
login, the server fetches from `SOURCE` (`env:HOLON_SECRET_…`, `vault:<mount>/<path>#<field>`),
never storing or showing it: `smtps://user:password@host` (TLS, port 465), or
`smtp://user:password@host:587` (STARTTLS, required); `imaps://user:password@host` (TLS, port
993), or `imap://user:password@host:143` (STARTTLS, required); plain `smtp://` and `imap://` only
to a server on the same machine. A URL with a user and no password (Microsoft 365's, which take
none) logs in with a token the server asks the endpoint in `--oauth` for, and keeps in memory. It
logs in to each when it is bound, and makes the mailbox's `--done` folder. Removing it gives up
the mail on its way through it. Needs an admin; each takes
`--server PORT|URL`. See [Mail](../guides/mail.md).

| Option | |
|---|---|
| `--smtp SOURCE` | where the server reads the SMTP server's URL |
| `--from SENDER` | who the mail is from: `"Treasury <treasury@acme.com>"`; never the flow's choice |
| `--to LIST` | every address it may ever send to, with commas: addresses, `*@domain`, or `*` for anyone |
| `--imap SOURCE` | where the server reads the mailbox's URL |
| `--folder F` | the folder read: `INBOX` unless given |
| `--done F` | where each message is moved once its run is over: `Processed` unless given |
| `--verdict-from SERVER` | whose `Authentication-Results` verdict on a sender counts: the name the mailbox's own server writes there; without it, no message is authenticated |
| `--every T` | how often the folder is looked at: `1m` unless given, `10s` at least |
| `--oauth SOURCE` | where the server reads the token request, for a server whose URL names a user and no password: `https://id:secret@host/…/token?scope=…` (client credentials) or `?refresh_token=…`; see [Logging in with OAuth](../guides/mail.md#logging-in-with-oauth) |
| `--for SLUG` | for that deployment only |

## `holon feed`

```
holon feed add NAME --url SOURCE [--header NAME=SOURCE]… [--buffer N] [--idle T] [--for SLUG]
holon feed add NAME --database NAME --channel CHANNEL [--buffer N] [--idle T] [--for SLUG]
holon feed list
holon feed remove NAME [--for SLUG]
```

Binds a name flows import (`"feed:prices"`) to a feed of server-sent events, a WebSocket or a
Postgres channel, whose events start their runs (`on.feed(prices, …)`), one at a time, in the
order they came. The feed's URL, and each header's value, the server fetches from `SOURCE`
(`env:HOLON_SECRET_…`, `vault:<mount>/<path>#<field>`) each time it opens the connection, never
storing or showing it: `https://…`, a login in it sent as Basic authentication; `wss://…` for a
WebSocket; plain `http://` and `ws://` only to this machine. A channel's is its database's
connection, as the listening deployment sees the database's name. It reads the feed when it is
bound (listens on the channel), and refuses one that doesn't answer as a feed does. Removing it
closes its connections. Needs an admin; each takes `--server PORT|URL`. A WebSocket's options
(`--subscribe`, `--heartbeat`, `--id`, `--only`, `--resume`) and what comes when events wait
(`--on-full`, `--key`, `--sequence`) are in [Feeds](../guides/feeds.md#binding-a-feed).

| Option | |
|---|---|
| `--url SOURCE` | where the server reads the feed's URL |
| `--header NAME=SOURCE` | a header each request sends, its value read from a source (`Authorization=env:HOLON_SECRET_PRICES_AUTH`); again for another |
| `--buffer N` | how many events may wait for their run before the connection waits too: 1000 unless given |
| `--idle T` | how long the feed may say nothing before the connection is opened again: `90s` unless given, `5s` at least |
| `--database NAME` | a Postgres channel's: the database it is on, as bound ([`holon database`](#holon-database)) |
| `--channel CHANNEL` | the channel listened on (`LISTEN`): letters, digits and `_` |
| `--for SLUG` | for that deployment only |

## `holon queue`

```
holon queue add NAME --url SOURCE [--visibility T] [--keep-failed] [--for SLUG]
holon queue list
holon queue remove NAME [--for SLUG]
```

Binds a name flows import (`"queue:payout_events"`) to an Amazon SQS queue, or a service that
speaks its API (ElasticMQ, LocalStack). A flow sends to it once its run is over
(`payout_events.send({ data, group })`), and its messages start runs
(`on.queue(payout_events, …)`), one at a time, each deleted once its run is over. The queue's
URL, with its access key, the server fetches from `SOURCE` (`env:HOLON_SECRET_…`,
`vault:<mount>/<path>#<field>`), never storing or showing it:
`https://<key id>:<secret>@sqs.<region>.amazonaws.com/<account>/<queue>`, the secret
percent-encoded; `?region=` for a service under another name, `?token=` for a temporary key; plain
`http://` only to this machine. It asks the queue when it is bound, and refuses one that doesn't
answer. Removing it stops its listener. Needs an admin; each takes `--server PORT|URL`. See
[Queues](../guides/queues.md).

| Option | |
|---|---|
| `--url SOURCE` | where the server reads the queue's URL |
| `--visibility T` | how long a message received is kept out of sight while it waits for its run and runs: `30s` to `12h`, `5m` unless given |
| `--keep-failed` | leave a failed run's message for the queue to hand out again, for its redrive policy to count, rather than delete it |
| `--for SLUG` | for that deployment only |

## `holon mcp`

```
holon mcp [--server PORT|URL]
holon mcp setup [HOST…|--all] [--server PORT|URL] [--agent NAME] [--project DIR] [--name NAME] [--print] [--remove]
holon mcp hosts [add NAME --config PATH --like HOST | remove NAME]
```

`holon mcp` is an MCP server on stdin and stdout, for an agent's host to start. It reaches the
server you run on this machine (or `--server`, or `HOLON_SERVER`), and signs in as an agent,
never as you: on a server you run, as yours (`<your user>-agent`, owned by you, made through the
run file the first time); elsewhere, with the agent's token `setup` kept (or `HOLON_TOKEN`). A
person's or a service's token is refused. `setup` adds it to a host's settings; `hosts` lists the
hosts Holon knows and keeps your own. See [Working with agents](../guides/agents.md).

| `setup` option | |
|---|---|
| `HOST…` | the hosts to set up (`holon mcp hosts` lists them) |
| `--all` | every host found on this machine |
| `--server PORT\|URL` | a server you don't run: asks for an agent's token, checks it is one, and keeps it in `~/.holon/tokens.json` |
| `--agent NAME` | on a server you run, sign in as the agent `NAME` (made if it isn't there) rather than `<your user>-agent` |
| `--project DIR` | the hosts' project files in `DIR` |
| `--name NAME` | the entry's name (default `holon`), for a second server |
| `--print` | show the entry and where it goes; write nothing |
| `--remove` | take the entry out, and forget the token `holon mcp` signs in with there |
| `--config PATH --like HOST [--key KEY]` | a host file Holon doesn't know, written as `HOST`'s is |

## `holon lsp`

```
holon lsp
```

A language server on stdin and stdout, for an editor to start on `.holon.ts` files: each mistake
`holon check` finds, as you type; hover; completion; and formatting. It needs no server. The VS
Code extension starts it.

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
| `HOLON_FLOW_CACHE` | `holon serve --flow-cache` |
| `HOLON_VALUE_CACHE` | `holon serve --value-cache` |
| `HOLON_SETUP_CODE` | the first admin's setup code |
| `HOLON_EXPORT_HEADS`, `HOLON_EXPORT_HEADS_TOKEN` | the witness heads are sent to, and its token |
| `HOLON_SECRET_<NAME>` | values for secrets whose source is env |
| `VAULT_ADDR`, `VAULT_TOKEN`, `VAULT_NAMESPACE` | for secrets whose source is vault |
| `HOLON_SERVER` | the server `export`, `folder` and `mcp` reach |
| `HOLON_TOKEN` | an API token to sign in to it with |
| `HOLON_EDITOR` | `holon serve --editor` |
