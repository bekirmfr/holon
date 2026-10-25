# Holon

**Agents propose. Holon shows what they may do, runs only that, and keeps a record of every run.**

[![ci](https://github.com/bekirmfr/holon/actions/workflows/ci.yml/badge.svg)](https://github.com/bekirmfr/holon/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/bekirmfr/holon?include_prereleases&label=release)](https://github.com/bekirmfr/holon/releases)
![license](https://img.shields.io/badge/license-proprietary-lightgrey)

Holon runs small services (APIs, webhooks, scheduled jobs, runs that wait for a person) written
as **flows**: programs in a small language, or the same graph on a canvas. Flows are written by
agents or by people. Before a flow runs, and every time it runs:

- **Holon shows what it may do.** What a flow can touch (the hosts it calls, the secrets it
  reads, the state it keeps, the requests it answers, the folders it reads and writes) is derived
  from the flow itself, not declared by its author, so it can't be understated.
- **It runs only that.** The host's policy sets what may ever be granted, and a person approves
  what a deployment may do. An effect outside the grant is refused when a run tries it, whatever
  the flow says. A grant can be narrowed while the flow runs.
- **It keeps a record of every run.** The request, the state it read, the clock, randomness and
  every response it got, with the version and grant it ran under. Evaluation is pure, so a run
  replays exactly, node by node, and anyone can check a deployment's runs offline. The record
  is a hash chain, so a run changed or taken out afterwards shows.

> **Status:** pre-release (0.1.0 release candidates). It runs on one machine for one team; see
> [Status](#status).

## A flow, and what it may do

A webhook that verifies Stripe's signature and posts the event to Slack:

```
flow stripe-to-slack
port 8103

route POST /stripe (req) {
  verified = stripe.verify(req, secret.stripe_webhook_secret, now)
  sent = http.post("hooks.slack.com", secret.slack_webhook_path, headers: { "content-type": "application/json" }, body: json({ text: "Stripe: {verified.event.type}" }))
  respond sent.status, missing: 400
}
```

`holon check` reads it and says what it is and what it may do:

```
$ holon check hook.holon
stripe-to-slack: 18 nodes, 18 wires
a server on :8103
  POST /stripe
unit cf3e7839f760eba0…
it may:
  read the clock
  call https://hooks.slack.com
  receive requests
  read the secret "slack_webhook_path"
  read the secret "stripe_webhook_secret"
```

That list is what a deploy asks a person to approve, and all the flow will ever be allowed to do.
An unsigned or tampered request never reaches Slack: `verified` has no value, so nothing that
depends on it runs, and the route answers 400.

A flow's identity is the hash of what it is (`unit` above). Any change is a new version,
redeploying asks for whatever the old grant didn't cover, and every run names the version it ran.

## Quick start

**1. Install** (Linux, macOS; see [Installing](#installing) for Windows and details):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

**2. Start the server and open the editor:**

```sh
holon serve
```

Open http://127.0.0.1:7420. The first time, the server prints a one-time setup code: enter it in
the editor to create the admin.

**3. Deploy a flow.** On the editor's **flows** page, open **Hello API**, press **deploy**, and
approve what it asks for (to receive requests). It answers on its own port:

```sh
curl http://127.0.0.1:8101/hello/ada
# Hello, ada
```

Open the deployment in **view** mode to see the run: the value each node made, step by step, on
the canvas and in the flow's text (**text** in the toolbar opens it).

**4. Or let an agent write one.** Add Holon to your agent's host, then ask it for a flow:

```sh
holon mcp setup claude-code      # or cursor, vscode, … (holon mcp hosts lists them)
```

The agent writes the flow, checks it and deploys it through `holon mcp`. It can't approve what a
flow may do: a first deploy waits on the flows page for you to approve.

**5. Or write flows as text,** on any machine:

```sh
holon new my-flow.holon          # a starter flow (--sample NAME for a sample)
holon check my-flow.holon        # its routes, what it may do, or each mistake by line
holon fmt my-flow.holon          # rewrite it in the canonical form
```

## How it works

**Flows.** A flow is a graph of pure parts (a catalog of leaves for text, numbers, lists,
records, JSON, hashing and signatures) with effects only at its edges: the requests it answers,
the calls it makes, the state it reads and writes. Every run ends: there are no loops or
recursion, only `map` and `fold` over lists. The language and the canvas are two views of the same
graph: text compiles to it, the canvas prints back to text, and both hash to the same unit.
Language reference: [`docs/LANGUAGE.md`](docs/LANGUAGE.md).

**Permissions.** What a flow may do is derived from its graph (its *manifest*). The server's
policy (`holon.toml`) says what may ever be granted and what needs a person; a deploy shows the
manifest against the policy and asks for the rest; the grant is enforced at the moment an effect
is performed, and checked again whenever the server loads. Design:
[`docs/PLATFORM_LAYOUT.md`](docs/PLATFORM_LAYOUT.md) §4.2.

**The run record.** Each deployment keeps a hash-chained log of its runs: everything the run
read from outside, and what it did. The editor replays any run from it; `holon export` writes a
deployment's runs to one file, and `holon verify` runs every one again offline, through the same
core, and checks it made what the log says. A witness outside the server (`--export-heads`) makes
the log tamper-evident even to someone holding the server's data.

**Agents.** `holon mcp` gives an agent tools to read the language, check, save, deploy, call and
explain flows. It acts as the `agent` role: it reads, proposes and calls, and never approves,
grants, starts, stops, deletes or sets a secret.

## What a flow can do

| | In the language | Sample |
|---|---|---|
| Answer HTTP requests | `route GET /notes/:id (req) { … respond … }` | `hello-api`, `guestbook` |
| Call other services | `http.get(host, path)`, `http.post(…)`, …: each host is a permission | `fetch-json`, `stripe-to-slack` |
| Keep state | `state.beats`, and collections keyed by id: `collection notes`, `notes[id]` | `notes-api`, `likes` |
| Read secrets | `secret.name`: sealed at rest, never in a log or replay | `guarded-api`, `sign-payload` |
| Run on a clock | `every 10s` in place of a port: the flow runs on the clock | `heartbeat` |
| Run on new files | `on file drop`: a run per file dropped in a folder, its handle as `ingress` | `drop-box` |
| Wait for a person | `await`: a run parks, and a later request resumes it, days later if need be | `approval` |
| Call out once per key | a collection key read before a call is held until the run commits | `idempotent` |
| Choose | `if … then … else`, `match`, `??`: the branch not taken doesn't run | `calc`, `cache-aside` |
| Repeat | `map` and `fold` over lists, `range` | `quote`, `tickets` |
| Reuse | holons: a flow placed as one node | `slugs` |
| Compute what the vocabulary can't | a `js` holon: JavaScript with no imports, under a fuel budget | `nearest`, `tally` |
| Read and write files | named folders an admin binds (`folder inbox`) | `inbox-report`, `log-archive`, `journal` |

## Samples

Every sample is a `.holon` file in [`crates/holon-author/samples/`](crates/holon-author/samples),
opens from the editor's flows page, and is deployed by the end-to-end tests.
`holon new file.holon --sample NAME` writes one.

| Sample | Shows |
|---|---|
| `hello-api` | the smallest server: a route with a path parameter |
| `notes-api` | create, read, list a page at a time, and delete, in a collection |
| `stripe-to-slack` | a verified webhook posting to Slack; anything unsigned gets 400 |
| `guarded-api` | answering only token holders, with a signed receipt; secrets kept elsewhere |
| `slugs` | holons: a whole flow placed as one node, twice |
| `issue-links` | one route with many interpolated parts |
| `quote`, `basket`, `tickets` | functions over lists: `map`, `fold`, `range`, reads inside a function |
| `cache-aside` | a fallback that isn't fetched when it isn't needed |
| `calc` | `match`: one arm runs, the others don't |
| `heartbeat` | a flow on a clock, counting in its state |
| `approval` | a durable run: a pay link that finishes the order hours later |
| `fetch-json` | calling an API and answering with one field |
| `sign-payload` | HMAC-SHA256 signing with a secret |
| `likes` | runs side by side on one collection, none lost |
| `idempotent` | one charge per `Idempotency-Key`, even when two arrive at once |
| `guestbook` | a web page served by the flow, rendered by a `js` holon |
| `nearest`, `tally` | `js` holons: edit distance, parsing a CSV |
| `inbox-report`, `log-archive` | files in named folders; big files by range, proved by hash |
| `journal` | a folder whose reads are kept a week and whose size is capped |
| `drop-box` | a run per new file in a folder, the file sent on by handle as a request's body |
| `ledger` | every run checked again offline by `holon verify` |
| `witness` | a witness for another server's run heads, run as a flow |
| `marketplace` | a marketplace for holons, built as a flow |

## Tools

| | |
|---|---|
| `holon serve` | the server, with the web editor at its address |
| `holon new` / `check` / `show` / `fmt` | write, check, outline and format flows as text |
| `holon export` / `verify` | write a deployment's runs to a file; check them offline |
| `holon folder add` / `list` / `remove` | bind the folders flows read and write (admin) |
| `holon mcp`, `holon mcp setup` | the agent surface, and adding it to an agent's host |
| `holon lsp` | a language server: mistakes as you type, hover, completion, formatting |
| `holon version` / `update` / `migrate` / `uninstall` | look after the install |

The **web editor** has a canvas and the flow's text side by side, linked by expression: the
cursor in the text picks the node under it, and a node picked on the canvas marks its
expression. In **view** mode it replays any run of a deployment: what didn't run is dimmed, and
the value each node made is beside it. It also manages deployments, their permissions and
secrets, accounts and API tokens, and the folders flows may use.

The **VS Code extension** ([`editors/vscode`](editors/vscode)) highlights `.holon` files and
starts `holon lsp`; each release carries it as `holon-vscode.vsix`. Any editor with a language
client can start `holon lsp` itself.

`holon mcp setup` knows Claude Code, Claude Desktop, Cursor, VS Code (Copilot), Visual Studio,
GitHub Copilot CLI, Devin, Windsurf, Zed, Gemini CLI, Antigravity, OpenAI Codex, Cline, Junie,
Kiro, opencode, LM Studio and Continue, and shows Goose and JetBrains AI Assistant the entry to
paste. It edits the host's own settings file and leaves the rest of it as it was.

## Installing

Each release on [GitHub Releases](https://github.com/bekirmfr/holon/releases) holds `holon` for
Linux (x64, arm64), macOS (Apple silicon, Intel) and Windows (x64), with the editor built in, its
checksums, and install scripts that fetch the right one, check it, and put it in
`~/.holon/bin`, on your PATH.

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

Windows, in PowerShell:

```powershell
irm https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.ps1 | iex
```

`latest` is the newest full release; GitHub skips pre-releases there. For a pre-release, name the
version: `…/releases/download/v0.1.0-rc.21/holon-installer.sh`. The scripts' settings:
`HOLON_INSTALL_DIR` (where), `HOLON_NO_MODIFY_PATH=1` (leave PATH alone).

```sh
holon version            # this build's version
holon update --check     # is there a newer release?
holon update             # install it over this one (--to VERSION for a given one)
holon uninstall          # remove the program, its PATH entry and install receipt
```

A pre-release build follows pre-releases, a full release only full releases (`--pre` to include
them). `uninstall` keeps the server's data; `uninstall --purge` removes all of `~/.holon` after
asking (`--yes` skips that).

<details>
<summary>Windows antivirus, checking a download, and older releases</summary>

- Use the `irm … | iex` form above, not `powershell -ExecutionPolicy Bypass -c "irm … | iex"`:
  Microsoft Defender flags that command line as a trojan pattern. If an antivirus stops the
  script anyway, download `holon-x86_64-pc-windows-msvc.zip` from the release and put `holon.exe`
  on your PATH; `update` then can't replace it, but everything else works.
- To check a download came from this repository's release workflow (from v0.1.0-rc.2 on):
  `gh attestation verify holon-x86_64-unknown-linux-gnu.tar.xz --repo bekirmfr/holon`.
- Releases up to 0.1.0-rc.3 installed it as `holon-server`. `holon-server update` installs
  `holon` in its place and keeps a `holon-server` beside it that runs `holon serve`.
- Releases before 0.1.0-rc.17 kept a server's state as `.holon-*` files in the folder it was
  started from. `holon migrate` (run there, or with `--from DIR`) moves them into
  `~/.holon/data`; `holon serve` says so when it sees them.

</details>

### Where it keeps things

Everything is in one folder, `~/.holon` (`%USERPROFILE%\.holon` on Windows), or `HOLON_HOME`:
`bin/` the program, `data/` the server's state (the one folder to back up), `keys/` secrets keys
where there's no OS key store, `run/` a file per running server, `tokens.json` and
`mcp-hosts.toml`.

## Running a server

`holon serve --help` lists every option. The ones most often needed:

| Option | |
|---|---|
| `--at [IP:]PORT` | where the server and editor listen (default `127.0.0.1:7420`) |
| `--data DIR` | where the state is kept (or `HOLON_DATA`; default `~/.holon/data`) |
| `--data-host IP` | where deployed flows' own servers listen (default `127.0.0.1`) |
| `--allow-host HOST` | a host name it answers to besides localhost, to reach it from other machines |
| `--keep-runs N` | runs each deployment's log keeps (default 500) |
| `--export-heads URL` | send each run's head to a witness (only hashes leave the server) |
| `--policy PATH` | the policy: what may be granted, and what needs a person |

There is no TLS in the server yet: to reach it from other machines, put HTTPS in front (a reverse
proxy).

**Checking runs.** A deployment's runs export to one file, from the editor's run log ("export
runs") or the command line:

```sh
holon export notes-api           # from the server you run here, into notes-api-runs.json
holon verify notes-api-runs.json # anywhere, offline: the chain, each unit, every run again
```

`verify` checks the chain is unbroken, each unit is what its hash says, and every run, run again
from its log entry, produces what the log says. `--head HEX` checks a head a witness holds is in
it. A run that used a secret or a resume token can't be run again (the log never holds their
values) and is counted, not failed. A server elsewhere takes `--server URL` and an API token in
`HOLON_TOKEN`.

**Files.** Flows read and write files only in folders an admin binds to a name:

```sh
holon folder add inbox ~/drop --read-only    # flows may read it, not write it
holon folder add reports ~/reports           # read and write; --keep hash: the log keeps hashes
holon folder add inbox ~/acme --for acme     # deployment acme sees this folder as inbox
holon folder add journal ~/journal --keep 7d --cap 10M   # reads kept a week; 10 MiB at most
holon folder list                            # holon folder remove NAME [--for SLUG] unbinds one
```

or from the editor's admin page. A flow names the folder, never a path; the server's own data
can't be bound; a write lands whole when the run finishes, unless it would take a capped folder
past its cap (the run then answers 500). A text a run reads past 4 KiB is kept beside the store
by content, in chunks, so a file that grows adds its new chunks, not a copy; the server sweeps
the texts no kept run refers to within their folder's `--keep` (`holon folder sweep` does it now).
A flow `on file drop` runs once for each file that arrives in `drop` (once it stops changing),
and `http.put(host, path, file: ingress)` sends a file's bytes without them passing through the
flow.
`holon export` takes `--max-files-bytes 10M` or `--without-files` to leave those texts out.

**Agents on another machine.** For a server elsewhere, `holon mcp setup HOST --server URL` asks
for an API token with role `agent` (made on that server's admin page, or given in `HOLON_TOKEN`),
checks it, and keeps it in `~/.holon/tokens.json`, readable only by you; the host's settings file
never holds it.

## Status

Holon is pre-release: 0.1.0 release candidates, each in [`CHANGELOG.md`](CHANGELOG.md).

- It runs on one machine, its state in files, for one team. There is no TLS in the server, and
  request limits are server-wide.
- The next milestone, and the bar for 0.1.0, is someone outside the project doing a real job with
  it end to end: [`docs/PLATFORM_LAYOUT.md`](docs/PLATFORM_LAYOUT.md) §7, M3.

## Documentation

**[The Holon manual](docs/manual/README.md)**: [getting started](docs/manual/getting-started.md),
[concepts](docs/manual/concepts.md), [a tour of the language](docs/manual/language/tour.md), the
[language reference](docs/manual/language/reference.md), guides (the editor, deploying, agents,
[Stripe to Slack](docs/manual/guides/stripe-to-slack.md), HTTPS, files, checking runs) and the
[command line](docs/manual/reference/cli.md).

Design documents:

- [`docs/LANGUAGE.md`](docs/LANGUAGE.md): the language, by example and in full.
- [`docs/PLATFORM_LAYOUT.md`](docs/PLATFORM_LAYOUT.md): the runtime (permissions, effects,
  durable runs, the run record) and the plan.
- [`docs/L4s/`](docs/L4s): a design note per feature, with what was built.
- [`docs/README.md`](docs/README.md): which documents are current.

## Developing

Rust (the toolchain is pinned in `rust-toolchain.toml`, with the `wasm32-unknown-unknown`
target), `wasm-pack`, and Node 22 for the editor. See [`CLAUDE.md`](CLAUDE.md) for the
conventions: every feature ships with a sample flow.

```sh
cargo run -p holon -- serve                     # the server
wasm-pack build crates/holon-wasm --target web --out-dir ../../editor/wasm   # the editor's core
npm --prefix editor run dev                     # the editor, against that server
cargo test                                      # the Rust tests
npm --prefix editor test                        # the editor's unit tests
npm --prefix editor run e2e                     # end to end, through a real server and editor
```

**Releasing.** Releases are built by `.github/workflows/release.yml` (cargo-dist, configured in
`dist-workspace.toml`): set the version in `crates/holon/Cargo.toml` and `CHANGELOG.md`, commit,
and push a signed tag for it (`git tag -s v0.1.0 -m v0.1.0 && git push origin v0.1.0`).

## License

Proprietary: all rights reserved (see [`LICENSE`](LICENSE)). No use, copying or distribution
without a written commercial agreement.
