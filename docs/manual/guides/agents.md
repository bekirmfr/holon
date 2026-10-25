# Working with agents

Holon is built for flows that agents write and people approve. An agent connects through
`holon mcp`, a [Model Context Protocol](https://modelcontextprotocol.io) server: it reads the
language, checks its flow, saves and deploys it, sends it requests and reads its runs. It can't
approve what a flow may do: that stays with a person.

## Connecting an agent

On the machine where you run `holon serve`:

```sh
holon mcp setup claude-code
```

This adds a `holon` entry to the host's own settings file, and leaves the rest of the file as it
was. Restart the agent to pick it up.

- `holon mcp setup` with no host lists the hosts Holon knows and which are on this machine;
  `--all` sets up every one found.
- Known hosts: Claude Code, Claude Desktop, Cursor, VS Code (Copilot), Visual Studio, GitHub
  Copilot CLI, Devin, Windsurf, Zed, Gemini CLI, Antigravity, OpenAI Codex, Cline, Junie, Kiro,
  opencode, LM Studio and Continue. For Goose and JetBrains AI Assistant it shows the entry to
  paste. `holon mcp hosts` lists them with where each keeps its settings.
- `--project DIR` writes the hosts' project files in `DIR` (for a repository shared with others)
  instead of your own; `--print` shows the entry without writing it; `--remove` takes it out.
- A host Holon doesn't know: `holon mcp setup --config PATH --like HOST` writes its file as
  `HOST`'s is written, and `holon mcp hosts add NAME --config PATH --like HOST` keeps it for next
  time.

On the same machine, `holon mcp` finds the server you run and signs in as you, but every request
it makes asks as the **agent** role, so the server holds it to proposing.

### A server on another machine

```sh
holon mcp setup claude-code --server https://holon.example.com
```

`setup` asks for an API token with role **agent** (an admin makes one on the server's **admin**
page, under API tokens), checks it, and keeps it in `~/.holon/tokens.json`, readable only by you.
The host's settings file never holds the token. `HOLON_TOKEN` gives one without asking.

Give an agent that has a shell a token with role agent, not your own sign-in: a role-agent token
can't approve anything, whatever the agent runs.

## What an agent can do

The tools `holon mcp` offers:

| Tool | Does |
|---|---|
| `reference` | the language on one page, every leaf, and the samples; with a topic, that one in full |
| `check` | compile a flow: its routes, its version, and what it may do; or each mistake, with line and column |
| `save` | save a flow on the server, for a person to open; never replaces a flow someone else saved |
| `deploy` | deploy a flow; when it needs approval, save it for a person instead |
| `call` | send a request to a deployed flow's route, logged as a run like any other |
| `deployments` | what is deployed; with a slug, one in full: routes, what it may do, what isn't granted, which secrets are set, its text |
| `flows` | the saved flows; with a name, its text |
| `runs` | a deployment's latest runs, one line each |
| `explain` | one run as the program: each statement's value, where a missing value came from, the effects and calls |

`check` and `reference` need no server; the rest reach the one `holon mcp` connects to.

The agent role **never** approves a grant, starts, pauses, stops or deletes a deployment, sets a
secret, or replaces a flow someone else saved. The server enforces this, not the tool.

## Approving what an agent proposes

The first deploy of a flow always needs approval, so an agent's first `deploy` answers:

```
Not deployed yet: a person has to approve what "counter" may do. It asks to:
  - receive requests
  - read items of its collection "counts"
  - put and delete items of its collection "counts"

It is saved as the flow "counter" on http://127.0.0.1:7420: they open the editor there, open
"counter" from the flows page, and deploy it, which asks them to approve exactly these. …
```

and the agent tells you so. In the editor:

1. On the **flows** page, the flow reads "saved … by *you* (agent)". Press **open**.
2. Read it: the canvas and the text show what it does, and **deploy** shows what it may do.
3. Press **deploy**, then **Approve and deploy** if you agree. Set any secrets it needs under
   **⚙ → secrets**.

From then on, a new version that asks for nothing more deploys from the agent at once. A version
that asks for something new (another host, another secret) is saved for you again.

## A good loop

Agents work well with Holon when they use the tools in this order:

1. `reference` once, to learn the language;
2. write the flow, and `check` it until it compiles and asks for no more than it should;
3. `deploy`; if it needs approval, ask the person;
4. `call` it with a few requests, then `runs` and `explain` to see what each statement did;
5. fix, `check`, `deploy` again.

`explain` is the agent's view of what the editor's view mode shows you: the flow's text with the
value of each statement in a run, and, for a value that is missing, where it went missing.

## Without MCP

An agent that can only write files can still write flows: `holon check file.holon` gives it the
same answers as the `check` tool, and you deploy the file with **import .holon** on the flows
page.
