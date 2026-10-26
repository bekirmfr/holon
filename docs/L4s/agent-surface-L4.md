# The agent surface: `holon mcp` (L4)

Step 7 of `docs/LANGUAGE.md` §12: the tools an agent uses to write, deploy, call and understand a
flow, speaking the language's text. Written 2026-10-24, before the code.

## What it is

`holon mcp` is a Model Context Protocol server on stdin and stdout (JSON-RPC 2.0, one message per
line), started by the agent's host (Claude Code, an IDE, any MCP client) as the same OS user who
runs `holon serve`. It has no state of its own. Each tool that needs a server finds one the way
`holon export` does (`local::target`): `--server` or `HOLON_SERVER` if given, else the one server
this user runs on this machine, signed in by its run file, or by `HOLON_TOKEN`.

```
holon mcp [--server PORT|URL]
```

`holon mcp setup HOST` adds it to a host's settings, for every major host and any other a person
names, and for a server elsewhere keeps the token so no host's file holds it:
`mcp-setup-L4.md`.

## The tools

| Tool | Takes | Answers | Needs a server |
|---|---|---|---|
| `reference` | `topic?` | the language on one page, every leaf with its ports, the samples; or one leaf, or one sample's text | no |
| `check` | `text` | what `holon check` prints: the routes, the unit, what it may do; or each mistake with its line, column and a caret | no |
| `save` | `text`, `name?` | saved on the flows page, not deployed, with what it may do once deployed; never over a flow someone else saved | yes |
| `deploy` | `text`, `slug?` | deployed (its unit, its routes and where they answer), or what a person must approve first, or why the host refuses it | yes |
| `call` | `slug`, `method`, `path`, `headers?`, `body?` | the response, and the run it made | yes |
| `deployments` | `slug?` | what is deployed, one line each (state, version, port, runs, who deployed it), what each waits on a person for, and the flows saved but never deployed; or one in full: its routes, what it may do, what its grant doesn't cover, the secrets it reads and which are set, what it waits for, and its text as deployed | yes |
| `flows` | `name?` | the saved flows, one line each (who saved it, when, whether it is deployed); or one flow's text | yes |
| `runs` | `slug`, `limit?` | the deployment's latest runs, one line each, with who sent a request made through `call` | yes |
| `explain` | `slug`, `run?` | one run as the program: the flow's text with each statement's value, what was absent and where, the effects, the outbound calls | yes |

Every answer is text written for a reader (the agent, and the person reading over its shoulder),
not JSON to be parsed: the same sentences `holon check` and the editor use.

## Decisions

1. **The tools never approve.** Today whoever sends `acknowledge` on `POST /deploy` approves, and
   the local sign-in is an admin. A tool that acknowledged for the agent would make the pitch
   ("agents propose; Holon shows what they may do") false. So `deploy` previews first
   (`POST /deploy/preview`) and deploys only when nothing needs approval: everything is either
   granted by the host's policy (`[grants] auto`) or by the slug's previous grant. Otherwise it
   saves the flow (`POST /flows`, under the slug) and answers what a person must approve and
   where: the editor's flows page, which deploys it with the approval dialog it already has.
   After that, a new version that needs nothing more deploys from the agent at once, and one that
   needs more goes back to the person. A version whose code changed and that reads a secret
   needs more too: the previous grant approved what the old code did with it. A required secret
   sends it back to the person; an optional one deploys withheld (`revoked`, its reads refused)
   until a person grants it back. An agent's deploy never starts a paused or stopped deployment. The host owner can pre-approve kinds of capability in the
   policy file, as today.
2. **Refusals and missing secrets are said, not worked around.** A host refusal (a limit over its
   ceiling, plain HTTP where the policy forbids it) is the answer. Secrets are set by an admin
   under ⚙; `deploy` names the ones not set yet, and the agent has no tool to set one.
3. **Calls go through the control plane.** `call` uses `POST /deployments/<slug>/request`, which
   routes, logs and counts a request exactly as the flow's own port does, so the agent needs no
   port and no CORS. The run it made is found in the log: `next` read before the call, the
   entries after it read after.
4. **`explain` is the program, with values.** The deployed document is printed through the
   language (`print_lines`, with the agent's own binding names when the document carries them),
   and each line gets the value of the statement on it from the run's `timeline` (by route and
   child index through the document's `trace`, as the editor's `nodeOutputs` does). A statement
   with no value says which node gave nothing first (`field "author" gave nothing`), which is
   what a 400 from `missing:` means. No re-evaluation: what is shown is what was recorded, and a
   node fed a secret shows its recorded, redacted value.
5. **The document an agent deploys is the one the editor would.** Compiled from the text, laid
   out (§8), with its `trace` and its settings, its units staged with `POST /unit`, plus the
   binding names under `bindings` so `explain` and the editor keep the agent's names. A person who
   opens it in the editor sees the same canvas.
6. **The server holds an agent to proposing: the `agent` role** (access.rs). An agent reads as a
   viewer does, and may store units, preview, deploy, save a flow and call a deployed one. Its
   deploy never approves: `acknowledge` from it is refused (403), as is a deploy that needs an
   approval it hasn't had, worded for the person who will give it. It may not replace a flow
   someone else saved (flows.rs), and grants, run/pause/stop, deletes, secrets, the registry,
   `/fire`, `/trigger` and the admin panel are refused it. `holon mcp` sends every request with
   `X-Holon-Role: agent`, which lowers an editor or admin who sends it to an agent (a viewer stays
   a viewer: an agent may do more than read), named `<sign-in> (agent)`:
   through the run file the agent acts as `local:<user> (agent)`, so the activity log, the flows
   page and `deployed_by` tell its steps from the person's, and the server, not only the tools,
   keeps it from approving. That header is a promise its sender keeps: an agent with a shell can
   read the run file and leave it off. For an agent that holds a shell, an API token of its own
   with role `agent` (`POST /admin/tokens`, or the admin page), used with `--server` and
   `HOLON_TOKEN`, is the guarantee: no request made with it approves, whatever sends it.
   `holon mcp setup --server` asks for that token once and keeps it (`mcp-setup-L4.md`).
7. **What is there reads back as text.** An agent that starts on a server with flows on it needs
   to see them before it changes one, so `deployments` and `flows` answer in the language, not
   as documents: a flow saved with its text (`source`, which `save` and a deploy saved for
   approval both keep, comments and all, as the editor's text pane does) reads back as that
   text, and one saved from the canvas alone is printed through the language with its binding
   names (`agent::text_of`). One deployment in full reads before it has ever run, which `explain`
   can't: what it may do from its manifest, the secrets it reads against the ones set (names
   only), and what it waits on a person for, each said as what they do: deploy it again (its
   `attention`), start it (paused or stopped), free its port (`listen_error`), or review a version
   saved under its name since it was deployed (from the flows page). `save` is `deploy`'s first
   half on its own: the agent keeps a draft, or hands one to a person, without asking for
   anything.
8. **A run says who sent it.** A request made over the control plane (`call`, or the editor's
   request tester) records its sender's name on the run (`by` on the log entry, part of the hash
   chain like the rest): `token:claude`, or `local:ada (agent)`. `runs` shows it (`sent by …`), so
   a person reading the log tells the agent's calls from their own, and from requests on the
   deployment's own port, where nobody signs in and the run names nobody.

## What it does not do

No tool stops, deletes, grants, sets secrets or edits the policy. No streaming, no resources or
prompts: tools only. The protocol versions accepted are the published ones up to 2025-11-25; the
server answers with the client's version when it knows it, else its newest.

## Tests

The tool logic talks to the server through one small trait, so the tests run it against the
in-process router (`tests.rs`'s `route`) with no socket: check a sample, deploy it (saved for
approval), approve it as a person would, deploy a new version from the agent, call it, list its
runs, explain a run that answered and one that failed at a missing field; save a flow, read it
back as the text saved, list it as never deployed, and once a person deploys it read the
deployment in full, text and all; a call by an agent token names the agent on its run. The
protocol layer is
tested on raw JSON lines. A live check runs `holon mcp` against `holon serve` before the commit.
