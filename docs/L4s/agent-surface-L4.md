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

## The tools

| Tool | Takes | Answers | Needs a server |
|---|---|---|---|
| `reference` | `topic?` | the language on one page, every leaf with its ports, the samples; or one leaf, or one sample's text | no |
| `check` | `text` | what `holon check` prints: the routes, the unit, what it may do; or each mistake with its line, column and a caret | no |
| `deploy` | `text`, `slug?` | deployed (its unit, its routes and where they answer), or what a person must approve first, or why the host refuses it | yes |
| `call` | `slug`, `method`, `path`, `headers?`, `body?` | the response, and the run it made | yes |
| `runs` | `slug`, `limit?` | the deployment's latest runs, one line each | yes |
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
   needs more goes back to the person. The host owner can pre-approve kinds of capability in the
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
   `X-Holon-Role: agent`, which lowers whoever sends it to an agent, named `<sign-in> (agent)`:
   through the run file the agent acts as `local:<user> (agent)`, so the activity log, the flows
   page and `deployed_by` tell its steps from the person's, and the server, not only the tools,
   keeps it from approving. That header is a promise its sender keeps: an agent with a shell can
   read the run file and leave it off. For an agent that holds a shell, an API token of its own
   with role `agent` (`POST /admin/tokens`, or the admin page), used with `--server` and
   `HOLON_TOKEN`, is the guarantee: no request made with it approves, whatever sends it.

## What it does not do

No tool stops, deletes, grants, sets secrets or edits the policy. No streaming, no resources or
prompts: tools only. The protocol versions accepted are the published ones up to 2025-11-25; the
server answers with the client's version when it knows it, else its newest.

## Tests

The tool logic talks to the server through one small trait, so the tests run it against the
in-process router (`tests.rs`'s `route`) with no socket: check a sample, deploy it (saved for
approval), approve it as a person would, deploy a new version from the agent, call it, list its
runs, explain a run that answered and one that failed at a missing field. The protocol layer is
tested on raw JSON lines. A live check runs `holon mcp` against `holon serve` before the commit.
