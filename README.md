# holon

**Agents propose. Holon shows what they may do, runs only that, and keeps a record of every run.**

An agent (or a person) writes a flow: a small service, as text or on a canvas, wired from pure
parts. Before it runs:

- **Holon shows what it may do.** What a flow can touch (the hosts it calls, the secrets it
  reads, the state it keeps, the requests it answers) is derived from its graph, not declared by
  its author, so it can't be understated. `holon check` prints it; deploying asks for it.
- **It runs only that.** The host's policy sets what may ever be granted. A deploy has to name
  every capability that needs approval, and gets a grant; an effect outside the grant is refused
  when a run tries it, whatever the flow says. A grant can be narrowed while the flow runs.
- **It keeps a record of every run:** the request, the state it read, the clock, randomness and
  every response it got, with the version and grant it ran under. Evaluation is pure, so a
  record replays exactly: the editor steps through any past run node by node. The record is a
  hash chain: each run names the one before it, so a run changed or taken out afterwards shows.

```
$ holon new hook.holon --sample stripe-to-slack
wrote hook.holon
$ holon check hook.holon
stripe-to-slack: 17 nodes, 17 wires
a server on :8103
  POST /stripe
unit 0e4840c64ea23983126f5729cdfff0cc2dc55cdc2fcfa067a4f86bd04a69fca7
it may:
  read the clock
  call https://hooks.slack.com
  receive requests
  read the secret "slack_webhook_path"
  read the secret "stripe_webhook_secret"
```

A flow's identity is the hash of what it is (`unit` above): any change is a new version,
redeploying it asks for whatever the old grant didn't cover, and every run names the version it
ran.

## Installing

Each release (GitHub Releases) holds `holon` for Linux (x64, arm64), macOS (Apple silicon, Intel)
and Windows (x64), with the editor built in, their checksums, and install scripts that fetch the
right one, check it, and put it in `~/.holon/bin`, on your PATH. One command does everything:
`holon serve` runs the server, `holon new`/`check`/`show`/`fmt` work on flows written as text,
and `holon version`/`update`/`uninstall` look after the install.

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

Windows, in PowerShell:

```powershell
irm https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.ps1 | iex
```

(Not the `powershell -ExecutionPolicy Bypass -c "irm … | iex"` form some tools suggest: Microsoft
Defender flags that command line as a trojan pattern and blocks it. If an antivirus stops the
script anyway, download `holon-x86_64-pc-windows-msvc.zip` from the release and put `holon.exe`
somewhere on your PATH; `update` then can't replace it, but everything else works.)

`latest` is the newest full release; GitHub skips pre-releases there. For a pre-release (or any
version), name it: `…/releases/download/v0.1.0-rc.4/holon-installer.sh`. The scripts' settings:
`HOLON_INSTALL_DIR` (where), `HOLON_NO_MODIFY_PATH=1` (leave PATH alone).

Releases up to 0.1.0-rc.3 installed it as `holon-server`. `holon-server update` on one of those
installs `holon` in its place (each release also carries its installers under the old name for
this), and keeps a `holon-server` beside it for scripts that start the server by that name: it
runs `holon serve`.

To check a download came from this repository's release workflow (build provenance, from
v0.1.0-rc.2 on):

```sh
gh attestation verify holon-x86_64-unknown-linux-gnu.tar.xz --repo bekirmfr/holon
```

Once installed:

```sh
holon version            # this build's version
holon update --check     # is there a newer release?
holon update             # install it over this one (--to 0.1.0 for a given version)
holon uninstall          # remove the program, its PATH entry and install receipt
holon migrate            # move what an older release left elsewhere into ~/.holon
```

A pre-release build follows pre-releases; a full release, only full releases (`--pre` to include
them). `update` works on copies the install scripts put in place (they leave a receipt saying
where); a copy unpacked by hand is replaced by hand. `uninstall` keeps the server's data;
`uninstall --purge` removes all of `~/.holon` too: the data folder (`--data DIR`, else
`HOLON_DATA`, else `~/.holon/data`), its secrets key, run files, tokens and the list of agent
hosts set up. It asks first; `--yes` skips that.

Everything holon keeps is in one folder, `~/.holon` (`%USERPROFILE%\.holon` on Windows), or
`HOLON_HOME` when set: `bin/` the program, `data/` the server's state (the one folder to back
up), `keys/` secrets keys where there's no OS key store, `run/` a file per running server,
`tokens.json` and `mcp-hosts.toml`. Releases before 0.1.0-rc.17 kept a server's state as
`.holon-*` files in the folder it was started from; `holon migrate` (run there, or with
`--from DIR`) moves them into `~/.holon/data`, and `holon serve` says so when it sees them.

## Running

```sh
holon serve
```

Then open http://127.0.0.1:7420. The first time, the server prints a one-time setup code: enter
it in the editor to create the admin. It keeps all of its state in `~/.holon/data`, wherever it
is started from; `--data DIR` (or `HOLON_DATA`) puts it in another folder.
Each deployment keeps its newest 500 runs; `--keep-runs N` (or `HOLON_KEEP_RUNS`) sets how many.

`--export-heads URL` (or `HOLON_EXPORT_HEADS`) sends each run's head, the hash its log chains up
to, to a witness outside the server, a few seconds after the run; with `HOLON_EXPORT_HEADS_TOKEN`,
as a bearer token. Only hashes leave the server. Once the witness holds a head, a run before it
can't be changed, removed or cut off the end unnoticed, even by someone with the server's whole
data folder. Heads wait on disk while the witness is away; its receipts are kept beside the
logs. The `witness` sample (`holon new w.holon --sample witness`) is a witness you can run
yourself, on another machine.

A deployment's runs export to one file: its log, exactly as written, and every unit the runs
ran. In the editor, the run log's "export runs" (beside the head the log chains up to, and any
run changed outside holon); on the command line, `holon export <slug>`, from the `holon serve` you
run on the same machine, signed in as you, into `<slug>-runs.json` here (`--to <folder>` and
`--as <name>` put it elsewhere; `--server <port>` picks one of several servers; a server
elsewhere takes `--server <URL>` and an API token in `HOLON_TOKEN`, or the one `holon mcp setup
--server` kept for it); or `GET /deployments/<slug>/export`. `holon verify
runs.json` checks it anywhere, offline: the
chain is unbroken, each unit is what its hash says, and every run, run again from its log entry
(the same request, state, clock, random, outbound answers and collection reads, through the same
core), produces what the log says it did. `--head <hex>` checks a head a witness holds is in it.
A run that used a secret or a resume token can't be run again (the log never holds their
values), and is counted, not failed. The `ledger` sample is one to try it on.
`holon serve --at 8080` listens on another port (`--at 0.0.0.0:8080` on every interface);
`holon serve --help` lists every option. To reach it from other machines, see `--allow-host` in
`docs/PLATFORM_LAYOUT.md`, and put HTTPS in front (a reverse proxy).

Flows can also be written as text (`.holon` files), on any machine, a screen or not:

```sh
holon new my-flow.holon                  # a starter flow (--sample hello-api for a sample)
holon check my-flow.holon                # its routes, what it may do, or each mistake by line
holon show my-flow.holon                 # an outline: routes, nodes and what feeds them
holon fmt my-flow.holon                  # rewrite it in the canonical form
```

In an editor, `holon lsp` reads a flow as you type, as `holon check` does: each mistake where it
is, what a word is on hover (a leaf's signature, a noun's permission, on the flow's name its
routes and what it may do), completion (the text's names, the nouns and leaves, and after a `.`
the members: `posts.` → get, put, delete, list), and Format Document as `holon fmt` writes.
`editors/vscode` is the VS Code extension (highlighting, and a client that starts `holon lsp`);
each release carries it as `holon-vscode.vsix`, and its README says how to install it; any editor with a language client can start `holon lsp` itself. Design:
`docs/L4s/language-server-L4.md`.

In the web editor, a flow's text is beside its canvas (open by default on a wide window;
**text** in the toolbar opens and closes it), with the same mistakes, hover, completion and Format. Both are editors and the last
edit wins: what you type drives the canvas once it compiles (nodes you didn't touch stay where
they were), and a change on the canvas writes the text again. The text as you typed it, comments
and all, is saved with the flow. The two are linked by what each node is written as: the cursor
picks the node under it, and a node picked on the canvas marks its expression. Viewing a run, the
text shows it too: what didn't run is dimmed, the step is marked, and the values of what you
named are written beside it. Design: `docs/L4s/text-pane-L4.md`,
`docs/L4s/text-graph-link-L4.md`.

An agent writes, deploys and reads flows through `holon mcp`, a Model Context Protocol server
its host starts. It reaches the `holon serve` you run on this machine, as `holon export` does.
To add it to a host:

```sh
holon mcp hosts                          # the hosts it knows, which are here, and what they have
holon mcp setup claude-code cursor       # add it to those (or --all: every one found here)
```

It knows Claude Code, Claude Desktop, Cursor, VS Code (Copilot), Visual Studio, GitHub Copilot
CLI, Devin Desktop and CLI, Windsurf, Zed, Gemini CLI, Antigravity, OpenAI Codex, Cline, Junie,
Kiro, opencode, LM Studio and Continue, and shows Goose and JetBrains AI Assistant the entry to
paste. It writes the entry into the host's own settings file and leaves the rest of the file as
it was, comments and all; each line it prints says what that host needs to load it (a new
session, a restart). `--project DIR` writes a project's files instead of yours, `--print` shows
the entry without writing, `--remove` takes it out. A host it doesn't know is added once, as like
one it does, then set up like the others:

```sh
holon mcp hosts add my-ide --config ~/.my-ide/mcp.json --like cursor
holon mcp setup my-ide
```

For a `holon serve` on another machine, `holon mcp setup HOST --server https://holon.example.com`
asks for an API token with role `agent` (made on that server's admin page; or give it in
`HOLON_TOKEN`), checks it with the server, and keeps it in `~/.holon/tokens.json`, readable only
by you: the host's file names the server and never holds the token. `--server` with `--remove`
forgets it. Design: `docs/L4s/mcp-setup-L4.md`.

Its tools:

- `reference`: the language on one page, every leaf, the samples.
- `check`: what `holon check` says, mistakes with their line and a caret.
- `deployments` and `flows`: what is already on the server, as text. `deployments` lists what
  is deployed and what each waits on a person for (an approval, a start, a newer version to
  review); with a slug, one in full: its routes, what it may do, the secrets it reads and which
  are set, and its text as deployed. `flows` lists the saved flows, or gives one's text to revise.
- `save`: saves a flow on the editor's flows page without deploying it, never over a flow
  someone else saved.
- `deploy`: deploys a flow. It never approves what a flow may do. A first deploy, or a version
  that needs a new permission, is saved instead, and the answer says what a person must approve:
  they open it from the editor's flows page and deploy it there. After that, a version that needs
  nothing more deploys from the agent at once.
- `call`: sends a request to a deployed flow, logged as a run like any other, which names who
  sent it (`sent by token:claude`).
- `runs` and `explain`: a flow's latest runs, and one run as the flow's text with the value each
  statement had and where a missing value came from.

The server holds it to that too: `holon mcp` asks as an `agent`, a role that reads, proposes and
calls, and never approves, grants, starts, stops, deletes or sets a secret. Through the run file
it signs in as you, and its steps show as `local:<you> (agent)` in the activity log and on the
flows page. An agent that also has a shell could read the run file and ask as you; for one,
make it an API token with role `agent` in the editor's admin page and set it up with
`--server` as above: nothing done with that token approves. Design:
`docs/L4s/agent-surface-L4.md`.

## Releasing

Releases are built by `.github/workflows/release.yml` (cargo-dist; its config is
`dist-workspace.toml`). To release: set the version in `crates/holon/Cargo.toml`, commit,
then push a tag for it:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`dist plan` shows what a release holds; `dist build --artifacts=local` builds this machine's
archive (it needs the editor built first: `npm --prefix editor run build`).

## License

Proprietary: all rights reserved (see `LICENSE`). No use, copying or distribution without a
written commercial agreement.

## Developing

See `CLAUDE.md` for the conventions and `docs/README.md` for which design documents are
current. The server: `cargo run -p holon -- serve`; the editor: `npm --prefix editor run dev`;
tests: `cargo test`, `npm --prefix editor test`, `npm --prefix editor run e2e`.
