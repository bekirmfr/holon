# Setting up an agent's host: `holon mcp setup` (L4)

`holon mcp` (`agent-surface-L4.md`) is started by an agent's host, and each host is told about it
in its own settings file. Until now that meant a host's own command (`claude mcp add`, which only
Claude Code's terminal install has) or editing a JSON file by hand, a different file and shape per
host, and for a server elsewhere, a token pasted into it. `holon mcp setup` does it for every
major host, and for any other one a person names. Written 2026-10-24, with the code.

```
holon mcp setup [HOST…|--all] [--server PORT|URL] [--project DIR] [--name NAME] [--print] [--remove]
holon mcp setup --config PATH --like HOST [--key KEY]
holon mcp hosts
holon mcp hosts add NAME --config PATH --like HOST [--key KEY] [--title TITLE]
holon mcp hosts remove NAME
```

## The hosts

Each host's file, key and entry come from its own documentation, read 2026-10-24
(`crates/holon/src/mcphosts.rs`, `builtin`). "App" is `%APPDATA%` on Windows,
`~/Library/Application Support` on macOS and `$XDG_CONFIG_HOME` (or `~/.config`) on Linux;
"config" is `%APPDATA%` on Windows and `~/.config` elsewhere.

| Name | Host | Its file | Key | Entry |
|---|---|---|---|---|
| `claude-code` | Claude Code | `~/.claude.json` (project `.mcp.json`) | `mcpServers` | `type: stdio`, `command`, `args` |
| `claude-desktop` | Claude Desktop | app `/Claude/claude_desktop_config.json` | `mcpServers` | `command`, `args` |
| `cursor` | Cursor | `~/.cursor/mcp.json` (project `.cursor/mcp.json`) | `mcpServers` | `type: stdio`, … |
| `vscode` | VS Code (Copilot) | app `/Code/User/mcp.json` (project `.vscode/mcp.json`) | `servers` | `type: stdio`, … |
| `visual-studio` | Visual Studio | `%USERPROFILE%\.mcp.json`, Windows only (project `.vs/mcp.json`) | `servers` | `type: stdio`, … |
| `copilot-cli` | GitHub Copilot CLI | `$COPILOT_HOME` or `~/.copilot` `/mcp-config.json` (project `.github/mcp.json`) | `mcpServers` | `type: local`, …, `tools: ["*"]` |
| `devin` | Devin Desktop and Devin CLI | config `/devin/mcp_config.json` (project `.devin/mcp_config.json`) | `mcpServers` | `command`, `args` |
| `windsurf` | Windsurf, before it became Devin Desktop | `~/.codeium/windsurf/mcp_config.json` | `mcpServers` | `command`, `args` |
| `zed` | Zed | config `/zed/settings.json` (Windows `%APPDATA%\Zed`) | `context_servers` | `command`, `args` |
| `gemini` | Gemini CLI | `~/.gemini/settings.json` (project `.gemini/settings.json`) | `mcpServers` | `command`, `args` |
| `antigravity` | Antigravity | `~/.gemini/config/mcp_config.json` (project `.agents/mcp_config.json`) | `mcpServers` | `command`, `args` |
| `codex` | OpenAI Codex | `$CODEX_HOME` or `~/.codex` `/config.toml` (project `.codex/config.toml`) | `mcp_servers` | TOML table: `command`, `args` |
| `cline` | Cline CLI | `~/.cline/data/settings/cline_mcp_settings.json` (or `CLINE_MCP_SETTINGS_PATH`) | `mcpServers` | `command`, `args` |
| `cline-vscode` | Cline in VS Code | app `/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` | `mcpServers` | `command`, `args` |
| `junie` | Junie | `~/.junie/mcp/mcp.json` (project `.junie/mcp/mcp.json`) | `mcpServers` | `command`, `args` |
| `kiro` | Kiro and Kiro CLI | `$KIRO_HOME` or `~/.kiro` `/settings/mcp.json` (project `.kiro/settings/mcp.json`) | `mcpServers` | `command`, `args` |
| `opencode` | opencode | `~/.config/opencode/opencode.json` (project `opencode.json`) | `mcp` | `type: local`, `command: [program, args…]`, `enabled` |
| `lmstudio` | LM Studio | `~/.lmstudio/mcp.json` | `mcpServers` | `command`, `args` |
| `continue` | Continue | project only: `.continue/mcpServers/holon.json` | `mcpServers` | `command`, `args` |
| `goose` | Goose | config `/goose/config.yaml` (Windows `%APPDATA%\Block\goose\config`) | `extensions` | shown as YAML to paste |
| `jetbrains` | JetBrains AI Assistant | its settings dialog | `mcpServers` | shown as JSON to paste |

Documented but not confirmed by the host's own pages, and so the first places to look if a host
doesn't load the entry: VS Code's user `mcp.json` path (VS Code's docs open it with a command, not
a path), Cline-in-VS-Code's path, Claude Desktop's Linux path, and the Windows paths of the CLIs
that give only `~/…`. A host is "found" on this machine when its file, or the folder it goes in
(`~/.cursor`, `%APPDATA%\Claude`, …), is there; `--all` sets it up in every one found.

## Decisions

1. **One entry, and the rest of the file as it was.** Other programs own these files, and some
   hold a person's whole settings (Zed's, VS Code's). The entry is set in place: a small reader of
   JSON with comments and trailing commas (`jsonedit.rs`) finds the member's span and changes only
   that, keeping the order, the indentation it finds, and every comment; TOML (Codex) is edited
   with `toml_edit`, which does the same. The result is read back before it is written, and a file
   that isn't JSON (or TOML) is refused and left alone. Writing is whole or not at all (a
   temporary file, renamed over it). Setting it up again is a no-op; an entry that runs something
   else is replaced, keeping what else it held (an `env` someone added, a timeout); `--remove`
   takes out exactly the entry. Removing after adding gives back the file as it was, byte for
   byte (tested on a Zed settings file with comments and a Codex file with another server).
2. **No host's file holds a token.** For a server elsewhere, the entry is `holon mcp --server
   URL`, and the token is asked for once (not shown as it is typed; `HOLON_TOKEN` gives it
   instead), checked against the server's `/auth/me`, and kept by holon in `~/.holon/tokens.json`,
   readable only by the user (`sealing::write_private`), keyed by the server's URL. Hosts differ
   in whether and how they can keep a secret (`${env:…}`, `${input:…}`, `env_vars`, nothing at
   all); this way every host is as safe as the safest, a host's file can be shared or synced
   without a secret in it, and a new token is one command. `local::target` reads it for every
   command that takes `--server` (`holon export` too): `HOLON_TOKEN` first, then the kept token
   (chosen for that server), then a run file. A token whose role isn't `agent` is kept, with a
   warning that only an agent token keeps an agent with a shell from approving.
3. **The entry names this program by its full path.** Hosts started from a desktop don't always
   see the PATH a terminal does, which is why a bare `holon` can fail in one and not the other.
   `holon update` replaces the program in place, so the path stays good. A project file is shared,
   so `--project DIR` writes `holon` instead, found on each person's PATH.
4. **A host it doesn't know is added as like one it does.** `holon mcp hosts add NAME --config
   PATH --like HOST` keeps it in `~/.holon/mcp-hosts.toml` (written with `toml_edit`, so a
   person's comments there stay too): its file, the host whose entry shape it takes, and with
   `--key`, where in the file its servers are (dotted, `mcp.servers`). Then it is a host like the
   others: `holon mcp setup NAME`, `--all` (it is never "found", since its file is named, so
   `--all` sets it up only once the file exists), `holon mcp hosts`. `--config PATH --like HOST`
   on `setup` does the same once, without keeping it. Shapes are the built-in hosts' so a custom
   host can't ask for an entry nobody reads.
5. **Where holon can't write, it shows.** Goose keeps its settings in YAML, which holon doesn't
   edit (a YAML editor that keeps a file as it was is a project of its own), and JetBrains AI
   Assistant keeps its in a dialog. For those, `setup` prints the entry and where it goes.
   `--print` does that for any host, writing nothing.
6. **What to do next is said.** Each host loads a new entry differently (a new session, a
   restart, a refresh button, at once); each line `setup` prints ends with that host's.

## What it does not do

It doesn't run a host's own command (`claude mcp add`, `codex mcp add`, `code --add-mcp`): not
every host has one, a host may not be on PATH, and writing the file is the same entry those
commands write. It doesn't set up remote (HTTP) MCP: `holon mcp` is a local process by design,
since it carries the agent's role and the token. It doesn't keep the token in the OS key store:
that is Windows and macOS only here (secrets.rs), and the run file already trusts a file only the
user can read.

## Tests

`jsonedit.rs`: a member added after the others with nothing else moved (a trailing comment, its
indentation), into an object with comments and a trailing comma, replaced in place, added to an
empty file and an empty object (and on one line, kept on one line), removed with its comma, and
what isn't JSON refused. `mcphosts.rs`: every built-in host has a name, a place and a shape on
both systems, and a moved folder (`CODEX_HOME`) is followed; each host's entry as its docs show
one; set up twice is once, another command is replaced keeping the rest, removed and not there;
Codex keeps its comments and other servers, and removing gives the file back; a custom host is
written as the host it is like; a file is written whole, and one that isn't JSON is left as it
was. `local.rs`: a token kept per server, forgotten, and preferred over a run file's but not over
`HOLON_TOKEN`.

The live check: `holon mcp hosts` and `--print` on a real machine; `setup --all`, again, into
`--project`, and `--remove` in a scratch home with a Zed file with comments and a Codex file with
another server; and against a scratch `holon serve`, a wrong token (refused, nothing written), an
admin's (kept, with the warning), an agent's, and `HOLON_TOKEN`, then `holon mcp --server`
started as a host starts it, signed in with the kept token (`token:claude (agent)`), and the
token forgotten with `--remove`.
