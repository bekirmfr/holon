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

`holon mcp` signs in as an **agent**, a principal of its own, never as you, so the server holds it
to proposing, and what it does is under the agent's name. On the same machine it finds the server
you run, and the first time, makes your agent there: *you*-agent (`bma-agent`), owned by you, with
the **agent** role. Its token is kept in `~/.holon/tokens.json`, readable only by you; the
server's **People and access** lists it with its tokens. `setup` makes it at once when the server
is running, and says so:

```
holon mcp signs in to http://127.0.0.1:7420 as bma-agent, an agent owned by bma.
```

`--agent NAME` signs in as another agent instead (made if it isn't there): one per host, say.

### A server on another machine

```sh
holon mcp setup claude-code --server https://holon.example.com
```

`setup` asks for an agent's token, checks it, and keeps it in `~/.holon/tokens.json`, readable
only by you. An admin adds the agent, owned by you (**People and access**, or `holon people add
NAME --agent --owner YOU`), and you make its token in the editor's **Your agents and services**
(an admin, on **People and access** or with `holon people token NAME`); see
[People, agents and services](people.md#an-agent-of-your-own).
The host's settings file never holds the token. `HOLON_TOKEN` gives one without asking.

`holon mcp` takes only an agent's token: a person's or a service's is refused, at setup and when
it connects, so an agent with a shell can't approve anything, whatever it runs. A setup from before
agents were principals, which kept a person's token, says to run `setup` again with an agent's.

## What an agent can do

The tools `holon mcp` offers:

| Tool | Does |
|---|---|
| `reference` | the language on one page, every leaf, and the samples; with a topic, that one in full |
| `check` | compile a flow: its routes, its version, and what it may do; or each mistake, with line and column |
| `save` | save a flow on the server, for a person to open; never replaces a flow someone else saved |
| `deploy` | deploy a flow; when it needs approval, propose it for a person instead, with a `why` and a `note` |
| `replay` | a new version of a deployed flow run offline on its recent runs: how many it answers the same, and how the others differ or why they can't be told; nothing is deployed, sent or written |
| `call` | send a request to a deployed flow's route, logged as a run like any other |
| `run` | run one of a deployed flow's actions by its name, with what to send it: only an action a person lets agents run |
| `whoami` | who it is signed in as, its owner, and what it may do: each role bound to it or to a group it is in, where (this host, a flow, an approval point), until when, and the actions it allows |
| `deployments` | what is deployed; with a slug, one in full: routes, what it may do, what isn't granted, which secrets are set, its proposals (a declined one with its reason), its text |
| `flows` | the saved flows; with a name, its text |
| `runs` | a deployment's latest runs, one line each; then the runs waiting now, at an approval with who has approved it so far, or at an event |
| `state` | what a deployment keeps now: each state value, and its collections; with a collection, its items (with a key, one) |
| `explain` | one run as the program: each statement's value, where a missing value came from, the effects and calls |
| `history` | a deployment's timeline: its runs, each with the version it ran on, and what was done to it between them, by whom (deploys, restores, grant changes, secrets, pausing); with a `cell`, the runs that changed that value, before and after |
| `restore` | a dry run of going back to a version or a run: the code's text beside today's, the permissions, what it touches (each folder, database, secret, host, key… new, gone or as now, and whether the server is ready for it), the state's keys and items now → then, the runs since and the files they wrote; it changes nothing. With `ask`, it saves the restore for you to approve |

`check` and `reference` need no server; the rest reach the one `holon mcp` connects to.

The agent role **never** approves a grant, starts, pauses, stops or deletes a deployment, sets a
secret, restores one, decides an [approval](../language/leaves.md#approval) a run waits at, or
replaces a flow someone else saved. The server enforces this, not the tool: it answers an agent's restore only as a dry run, or saves it for you. An agent that asks
(`restore` with `ask`) leaves a row on the deployment's timeline, "asks to put back the state run
41 left", by *bma-agent*; in the editor's history, pick that row and restore what it asks, after
the same dry run as any restore. Nothing changes until you do.

Within that, an agent does what it is bound: its role on the host (agent, or only viewer), and
any role an admin binds it on one flow or through a group it is in
([`holon bind`](../reference/cli.md#holon-bind),
[a role on one flow](people.md#a-role-on-one-flow-and-roles-of-your-own)), a role of the admin's
own making included. `whoami` lists each, where it holds and what it allows, so the agent
knows before it tries what it must ask you for.

An agent runs a flow's [action](actions.md) only once you let it: in the deploy settings (**⚙**),
**agents may run** lists the flow's actions, and each one you tick asks for `AgentRuns("name")`,
which you approve at the next deploy. Until then, `run` answers that agents may not run it, and
revoking it on the deployed page refuses them again. The agent's runs are logged as anyone's,
"by *bma-agent*".

An agent reads a flow's log lines, what its flow wrote with `console.log`, only once you let it too: **agents may read
its logs** in the deploy settings asks for `AgentReadsLogs`, which you approve at the next deploy. Then the `logs` tool
answers, oldest first, a line each (when, level, the run, the text), at a level or louder, or a run's lines; until then it
answers that agents may not read them, and so it does once you revoke it.

A run that waits at an approval (`await approval("payout", { people: 2 })`) waits for people:
`runs` shows which run waits and who has approved each step so far, and the agent tells you. You
decide it in the editor's Approvals (Runs waiting) or with [`holon decide`](../reference/cli.md#holon-decide);
no tool decides one. An approval in steps may have a step written for an agent
(`{ role: "classifier", kind: "agent" }`), which an agent bound to that role takes, never a
person's step ([`docs/L4s/access-and-approvals-L4.md`](../../L4s/access-and-approvals-L4.md) §8).

## Approving what an agent proposes

The first deploy of a flow always needs approval, so an agent's first `deploy` is a proposal: the
server keeps the version, with the agent's `why` (what it changes, in a line) and `note`, and
saves the flow under the agent's name. It answers:

```
Not deployed yet: proposed as version 1 of "counter" (proposal 1), with your why. A person has to
approve what it may do. It asks to:
  - receive requests
  - read items of its collection "counts"
  - put and delete items of its collection "counts"

What it touches (+ new, - no longer, ~ changed, ! not ready on http://127.0.0.1:7420):
What starts it
  + requests: receive — approved by confirming
What it keeps
  + collection counts: read, put and delete — approved by confirming

It waits in the editor's Approvals on http://127.0.0.1:7420 (#approvals.counter.v1), where they
approve it, which deploys it, or decline it with a reason. …
```

and the agent tells you so. In the editor:

1. **Approvals** lists it: the flow, the agent's why and note, and how many new permissions it
   asks for.
2. **Review** opens it as it was proposed: what it may do, its changes, its whole text and its
   canvas, and *If approved*, what approving it does.
3. Tick that you read it and press **Approve and deploy** if you agree. Set any secrets it needs
   in the flow's settings.
4. Or **Decline with a reason…** it, saying why. The reason is kept on the flow's history, and the agent reads
   it with `deployments` and the flow's name. The version stays saved, and what runs keeps
   running.

Opening the flow from the **flows** page and deploying it there approves the proposal too.

A flow's settings have **agents may propose versions**. Turned off, an agent's deploy of it is
refused, neither proposed nor saved. The **Proposal** sample is a flow to try this with. For the
whole host, the policy's `[agents] propose = false` refuses every agent version that needs a
person, on every flow ([The policy](../reference/server.md#the-policy)); the **Policy** page and
`holon policy` say which it is.

From then on, a new version that asks for nothing more deploys from the agent at once. A version
that asks for something new (another host, another secret, a site in its `browsers` line whose
pages it doesn't answer yet) is proposed to you again. So is a
version whose code changed and that reads a secret: what a secret is used for is the code, so a
person approves it again. A secret the flow marks optional doesn't hold it up: the version
deploys with that secret withheld (its reads refused), and you grant it back from the
deployment's permissions once you've read the change. An agent never restarts a deployment a
person paused or stopped; its `deploy` answers that a person runs it first.

## A good loop

Agents work well with Holon when they use the tools in this order:

1. `reference` once, to learn the language;
2. write the flow, and `check` it until it compiles and asks for no more than it should;
3. for a flow already deployed, `replay` the new version on its recent runs, and say in the
   deploy's `why` which runs it answers differently; then `deploy`; if it needs approval, ask the
   person;
4. `call` it with a few requests (or `run` an action it is let run), then `runs` and `explain` to
   see what each statement did;
5. fix, `check`, `deploy` again.

`explain` is the agent's view of what the editor's view mode shows you: the flow's text with the
value of each statement in a run, and, for a value that is missing, where it went missing.

When a flow that worked starts answering differently, `history` is where to look: the deploy, the
grant change or the secret set just before the first run that went wrong. `explain` that run and
the one before it to see what changed, and `restore` with the run before says what going back would
do: the code's text beside today's, the prices or counts it would put back, and what the runs since
did outside their state, which a restore doesn't undo. The agent then tells you; you restore it, in
the editor's history or with `holon restore`.

## Without MCP

An agent that can only write files can still write flows: `holon check file.holon.ts` gives it the
same answers as the `check` tool, and you deploy the file with **import text** on the flows
page.
