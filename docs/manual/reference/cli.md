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
| [`holon export`](#holon-export) | write a deployment's runs to a file |
| [`holon verify`](#holon-verify) | check exported runs offline |
| [`holon folder`](#holon-folder) | bind the folders flows use |
| [`holon database`](#holon-database) | bind the databases flows use |
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

Writes a starter flow to `<file>`, or the sample named. It never overwrites a file.
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
| `--keep-runs N` | runs each deployment's log keeps (default 500; or `HOLON_KEEP_RUNS`) |
| `--export-heads URL` | send each run's head to a witness (or `HOLON_EXPORT_HEADS`); its token in `HOLON_EXPORT_HEADS_TOKEN` |
| `--secrets-key os\|env:VAR\|file:PATH` | where the secrets file's key is kept (default: the OS key store) |
| `--new-secrets-key` | print a fresh key for `--secrets-key env:VAR`, and exit |
| `--dotenv FILE` | a `.env` file `HOLON_SECRET_*` values are also read from |
| `--editor DIR` | serve the editor from `DIR` instead of the built-in one (or `HOLON_EDITOR`) |
| `--policy PATH` | the policy file (default `holon.toml` in the data folder) |
| `--store`, `--deployments`, `--flows`, `--accounts`, `--secrets`, `--folders`, `--databases`, `--host-key PATH` | where each of those is kept, if not in the data folder. `--store` and `--deployments` also take `:memory:` |

`HOLON_SETUP_CODE` sets the setup code instead of a random one.

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

Checks exported runs, offline: the chain, each unit's hash, and every run, run again. `--head`
checks that a head a witness holds is in the export. `--files` gives the files runs read whose
texts the export doesn't hold. Exits 0 when it all checks, 1 when something doesn't, 2 for a file
it can't read.

## `holon folder`

```
holon folder add NAME PATH [--read-only] [--keep hash|30d] [--cap SIZE] [--shared] [--for SLUG]
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
| `--for SLUG` | for that deployment only |

`sweep` removes, now, the file texts no kept run refers to (the server does it every ten
minutes).

## `holon database`

```
holon database add NAME PATH [--read-only] [--schema SQL | --schema-file FILE] [--for SLUG]
holon database list
holon database remove NAME [--for SLUG]
```

Binds a name flows use (`database shop`) to a SQLite file on the server's machine. Needs an
admin; each takes `--server PORT|URL`. See [Databases on the host](../guides/databases.md).

| Option | |
|---|---|
| `--schema SQL`, `--schema-file FILE` | run it first, creating the file if there is none |
| `--read-only` | flows may read it, not write it |
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
