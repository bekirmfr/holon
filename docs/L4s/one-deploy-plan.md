<!-- holon:dev-phase-only -->
*A dev-phase document: a plan for this repository's development, not for the launch repository.*

# Flows and their deployments: one deploy, whatever sends it

The owner, 2026-11-18:

- "both deployments need to be exactly the same": a flow deployed from the web console and the
  same flow deployed from `holon editor`, `holon deploy` or an agent (`holon mcp`) must be granted
  the same things, and a redeploy from one must not undo what the other set.
- "the deploy settings should stay with the host": a flow from a marketplace that says how it is
  deployed clashes with each host's policy.
- "a deployment is like an instance of a class. saved flows are classes, and deployed ones are
  their instances."
- Going back to an old version keeps today's settings, "as long as we surface what changes and
  what is kept on the diff."

This replaces the first version of this plan (b10b4654), which put every setting in the flow's
text. Every file and line named here was read on `origin/main` at `b65296cd` (2026-11-18).

## 1. Where it stands

**A deploy carries its settings beside the flow.** Every path sends `POST /unit`,
`POST /deploy/preview`, `POST /deploy` with one body, `DeployRequestBody`
(`crates/holon/src/wire.rs:240-264`): `slug`, `unit`, `acknowledge`, `document`, `requirements`,
`uses`, `browsers`, `why`, `note`. The manifest is the unit's capabilities plus the body's
`requirements` (`deploy.rs:461`); each version keeps the `requirements`, `uses` and `browsers` its
own deploy sent (`VersionRequest`, `deploy.rs:664-667`, `wire.rs:529`), and a restore deploys an
old version with the requirements it was deployed with (`deploy.rs:2320-2338`).

**Some settings already live on the deployment.** Agents may propose, agent versions deploy at
once, and limits lowered are changed by `POST /deployments/{slug}/settings`
(`proposals.rs:294` `SettingsBody`, `:317`; asks `flow.deploy`, `can.rs:700`) and carried from
version to version (`deploy.rs:537`, `Grant::lowered`, `permissions.rs:942`). Grants taken back or
given back go through `ChangeRequest` (`permissions.rs:970`).

**The text says some settings, the browser's panel the rest.** The flow line takes `port`,
`browsers`, `keep`, `limits`, `taint`, `never_keep` and `tags` (`holon-author/src/lang/ts.rs:785`,
`:789`, `:849`, `:870`); port, taint, never_keep and tags are frozen params of the server unit
(`holon-author/src/routes.rs:842` `server_unit`). Plain HTTP, private addresses, agents may run an
action, agents may read its logs, and optional on anything but a secret are only in the browser's
panel (`editor/src/DeploySettings.tsx:32-47` `FlowSettings`, its `Requirements`). The panel's
settings are kept inside the saved flow's document.

**A saved flow and a deployment are joined by their name, and nothing else.** A saved flow is one
record per name, with no versions (`FlowRecord`, `wire.rs:179`); a deployment has its versions
(`DeploymentRecord`, `wire.rs:315`). Saving asks whether a deployment has the same name, to decide
who may edit it (`flows.rs:43` `may`, `:54` `deployed`). The Flows page shows one row per name,
holding the saved copy, the deployment, or both (`editor/src/FlowsPage.tsx:28`). A deploy names
the deployment only: which saved flow it came from, and which save, isn't recorded.

## 2. What goes wrong today

- **D1. The terminal, `holon deploy` and agents can't ask for four settings.** The text has no way
  to say plain HTTP to a host, private addresses, agents may run an action, or agents may read its
  logs; only a secret can be optional (`ts.rs` import attributes). *Example:* a flow calling
  `http://192.168.1.20:8080`, deployed from the terminal, has every call stopped; from the browser,
  a ticked box makes it work.
- **D2. A redeploy from one path undoes the other's settings.** Each version is granted only what
  its own deploy sent. *Example:* Ada lets agents run `reset` in the browser; Bob fixes a typo in
  the terminal and deploys; agents can no longer run `reset`, and no one is told.
- **D3. The browser keeps limits its text no longer says** (`editor/src/tabs.ts:127`
  `settingsSaid`, with a stale `TEXT_LIMITS` at `:110`).
- **D4. The two paths acknowledge different lists.** The browser acknowledges what the preview
  says needs approval; the terminal also everything the flow may do, its modules' limits included
  (`holon-author/src/agent.rs` `plan.may`), and a lowered limit stays lowered only while a deploy
  doesn't acknowledge it again (`deploy.rs:537`). As the code reads, a module limit an admin
  lowered comes back after a terminal redeploy. To be confirmed by a test (step 1).
- **D5. A flow can't be deployed twice on one host.** Two customers of one marketplace flow, each
  with its own port, secrets and limits, need two copies under two names; a fix to the flow then
  reaches one of them.
- **D6. Which saved flows a flow places** (`uses`) is sent by the browser only; the terminal's
  deployments drop out of the Flows page's "used by".

Who is affected: anyone who deploys from the terminal, `holon deploy` or an agent a flow that needs
one of D1's settings; any team where people use different paths on one flow (D2, D4); anyone who
edits the text in the browser (D3). Someone using one path and none of those settings is not.

## 3. The model

**A flow is the class; a deployment is an instance of it.**

| | The flow (saved; the text and its unit) | The deployment (on a host) |
|---|---|---|
| What it does | its code; the hosts, secrets, actions and modules it uses, derived from the unit (`Requirements::of_unit`, `permissions.rs:382`) | |
| How it treats data | `taint`, `never_keep` | |
| What it needs | a limit it can't run without (`limits:`), the permissions it can do without (`optional` on any import that doesn't start a run), the port it suggests | |
| What it is given | | its name; its port; how long its runs are kept; its limits; plain HTTP to which hosts; private addresses; which actions agents may run; whether agents read its logs; which permissions are optional here; the web pages that may call it; agents may propose, deploy at once; its grants, secrets, state and runs |
| Versions | its saves | the flow and save each version runs |

One rule: **the flow says what it does and needs; the deployment holds what the host gives; a
deploy, from any path, sends the flow and nothing else that changes a grant.** The server already
works this way for tags and for handlers that keep no record (`with_declared`,
`permissions.rs:373`: "what the unit says is what is asked").

What a deploy shows (the preview, the review, the terminal's dialog, an agent's answer): what the
flow needs against what the deployment gives, each difference named ("this flow needs 60s runs;
this deployment gives 10s; approve?"), as permissions are approved today.

**Going back to an old version** deploys its code under today's settings. Its review, like every
deploy's, has two parts (§7): what this version changes, and what stays as set on the deployment.

**Moving a flow to another host** is deploying the flow there and choosing that host's settings.
Moving a deployment as it is (its settings, secrets' names and state; a test server to a live one,
a backup) is a deployment's export, a separate thing.

## 4. Part 1: settings live on the deployment

The fix for D1-D4 and D6, useful on its own; the name still joins a flow and its deployment.

1. **One settings record on the deployment.** `DeploymentRecord` holds one `DeploySettings`: port,
   keep, limits, plain HTTP, private addresses, agents, agent logs, optional, browsers, agents
   propose, deploy at once (the last two moved from the record's own fields). One function
   applies a change to it, checked against the host's policy (`Policy::refuses`), recorded as an
   event, and taken by `POST /deployments/{slug}/settings`, whose body grows to every field.
2. **The deploy body loses `requirements` and `browsers`** (dev phase: no compatibility). It may
   carry `settings`, a change applied by that same function, so a first deploy can be set up in
   one step; a deploy that carries none keeps every setting. The manifest is the unit's
   requirements (`with_declared` for every field the flow says) plus the deployment's settings.
   `VersionRequest` goes; a version keeps its unit and document; restore deploys the unit under
   today's settings.
3. **The text.** `keep`, `browsers` and `port` leave the unit: `keep` and `browsers` leave the
   language (dev phase); `port` stays on the flow line as the port the flow suggests (§7 Q1).
   `limits:` and `optional` say what the flow needs and can do without, as frozen params of the
   server unit, read back by core's `Server` (`holon-core/src/declare.rs`) and by
   `with_declared`. Plain HTTP, private addresses and the agent settings never go in the text.
4. **One acknowledge rule** (D4): every path acknowledges what the preview says needs approval;
   `deploycmd.rs` `commit` stops adding `plan.may`.
5. **Who changes settings.** Settings ask `flow.deploy` today (`can.rs:700`). A change an agent
   asks for waits for a person with what it changes, as an agent version does (§7 Q3).
6. **The browser.** The settings panel reads and changes the deployment's settings (for a flow
   never deployed, its choices go with the first deploy as `settings`). `FlowSettings` leaves the
   saved document; `settingsSaid` and `TEXT_LIMITS` go; the deploy, preview, Approvals and Review
   pages send no requirements.
7. **The terminal, `holon deploy` and agents.** The terminal's Settings page shows and changes
   every setting (`workbench/settings.rs`); its first-deploy dialog offers them. `holon deploy`
   sends none (a first deploy gets the host's defaults); `holon settings <slug>` changes them. The
   MCP `deploy` sends none.

**Class tests** (one per class, every case through one place):

- *Same flow, same deploy:* for every sample, the browser's way (vitest, the wasm Evaluator) and
  the terminal's (`agent::plan`) give the same unit hash; with step 2 the body has nothing else
  that grants.
- *A redeploy keeps every setting:* a table, one row per `DeploySettings` field: set it, redeploy
  from each path, restore an older version: it is as set.
- *The text round trip loses nothing the flow says:* for every field the text says, a document
  with it set, written as text and compiled back, has it set the same.
- *Policy holds for every field:* a change a host never grants is refused, whichever endpoint
  carries it.

## 5. Part 2: a flow and its deployments

The fix for D5. It builds on part 1: settings on the deployment are what let one flow run twice.

1. **A deployment records its flow.** `DeploymentRecord` gains `flow` (the saved flow's name) and,
   per version, the save it ran (`saved_at`, as `HolonUse` already names a save, `wire.rs`). A
   deployment's name is its own; a first deployment takes its flow's name unless told otherwise.
2. **A saved flow keeps its saves.** `FlowRecord` keeps its last saves, numbered, so "billing runs
   save 3; save 5 is the latest" can be said and a deployment brought up to date.
3. **Deploying is "this save of this flow, to this deployment"** on every path: the deploy body
   names the flow and save; `holon deploy <file> --as <name>`; the MCP `deploy` takes both; the
   terminal's and the browser's deploy ask which deployment, new or one of the flow's.
4. **Access splits.** `flow.edit` is the flow's; `flow.deploy`, lifecycle, settings, runs and
   state are the deployment's. `flows.rs` `may`'s draft rule (saved, not deployed, by the caller)
   becomes "a flow none of whose deployments the caller doesn't own". Agent proposals: a new save
   of the flow plus a deploy to a deployment that waits, as now.
5. **The Flows page, web and terminal,** lists flows, each with its deployments under it, and says
   which run an older save.
6. **`uses` is the flow's** (a placed flow is a class): a save records it; a deploy doesn't send
   it (closes D6 for good).
7. **A deployment's export** carries its settings and secrets' names for a move to another host.

Class tests: one flow deployed twice runs each under its own settings, secrets and state; every
access row of the split (`tests/access_table.rs`) asks the flow's or the deployment's right as the
table says; the Flows page's grouping for every sample.

## 6. Steps, one commit each

Part 1:

1. Tests that show today's differences, failing now: D2 (a redeploy from the terminal drops a
   browser setting), D3, D4 (confirms or corrects it).
2. The settings record and its one change function; `POST …/settings` takes every field; the
   policy class test.
3. The deploy body without `requirements` and `browsers`; `VersionRequest` gone; restore under
   today's settings, its review listing what is kept; the redeploy class test. Goldens move.
4. The language: `keep` and `browsers` gone, `limits:` and `optional` as needs in the unit, `port`
   as suggested; `holon fmt` stable; samples, `LANGUAGE.md`, `holon.d.ts`; the round-trip class
   test.
5. One acknowledge rule.
6. The browser: the panel on the deployment's settings; wasm rebuilt; the same-deploy class test.
7. The terminal, `holon deploy`, `holon settings`, the MCP tools.
8. A sample flow that calls a service on this machine over plain HTTP, can run without it, and
   lets agents run one action and read its logs, set up after its first deploy; run live; e2e.
9. The manual: deploying.md, cli.md, permissions.md.

Part 2, after part 1 is on main:

10. A deployment records its flow and save; saves numbered.
11. Deploying a save to a named deployment, every path; a flow deployed twice (a sample, run live).
12. The access split.
13. The Flows page, web and terminal; `uses` from saves.
14. A deployment's export; the manual.

## 7. Fit with other work

From the coordinator, 2026-11-18, on `b65296cd` (#114-#117 merged).

**Order.**

1. This branch (the terminal editor, steps 0-9) takes main in now (it conflicts in `feeds.rs`,
   `mails.rs` and `queues.rs`, from #116: where a feed, a queue or a mail server is goes only to
   whoever may bind), again once `r72-run-error-paths` and R70 land, and goes to main as its own
   pull request before this redesign starts.
2. Part 1 is one pull request, begun after `r72-run-error-paths` (the answers a failed deploy
   gives, `deploy.rs` ~:602-616, :1575) and R70 (the echoed-key sample's routes in `eval.txt`) are
   on main. Its sample changes are one mechanical commit, as `fmt` writes them: step 4 touches
   every sample's unit (the port leaves it) and both goldens, which collides with any branch that
   adds or edits a sample.
3. Part 2 is a second pull request, on part 1.
4. The next release candidate waits for R92. Whether this goes in before or after it is the
   owner's; the coordinator advises after, since part 1 changes what every deploy sends.

**What it must keep holding.**

- **R25** (owner, 2026-11-15): every agent version of a deployed flow waits for a person with a
  diff of all it changes; "deploys at once" is the deployment's, off by default, set only by
  someone who may approve its deploys, and a policy can forbid it. With part 1 an agent's version
  can change less (no settings), and the settings route becomes the one place to guard: an agent's
  settings change waits as its version does (§8 Q3). `tests/meanwhile.rs` must still hold with a
  settings change racing a deploy.
- **The review** builds on R25's (`~ handler GET /hello/:name`, `+ host example.com`; restore goes
  through it). It has two parts: *what this version changes* (the text, what it may do, what it
  needs, its taint and never_keep keys: for an agent's version, what a person approves) and *what
  stays as set on the deployment* (plain HTTP, private addresses, what agents may do there,
  deploys at once, the web origins, lowered limits), shown so no one thinks approving changes it.
- **#117:** an agent version that drops a never_keep key waits (`KeepAgain`): that is the flow's
  text, so it stays in a version's diff.
- **R60:** every MCP tool and `holon` command goes through the server's routes as whoever signed
  in. `holon settings` and an MCP settings tool go through `POST /deployments/{slug}/settings`, with
  their rows in the sweep that drives every agent tool.
- **The route sweeps:** `tests/whereabouts.rs` (#116) sends every GET route as two callers who may
  not bind; R56's and R57's check which right each route asks; R89's editor test reads `can.rs`'s
  route table for the open routes. Part 1 changes the settings route's body, not its right. Part 2
  changes rows: `flow.edit` on `/flows/{name}`, every `/deployments/{slug}/…` route on the
  deployment's rights, and a deploy asking both (`flow.deploy` on the deployment, `flow.view` on
  the flow); step 12 lists each row it changes.
- **R92:** a restore review says when a run's state can't be restored in R92's words, not its own.

Nothing in the register is made obsolete; no open item is about `requirements` in the deploy body.
`deploycmd.rs`, `mcp.rs` and the terminal are touched by no one else.

## 8. Open questions

1. **The port.** 83 of the 90 samples say `port:`. Proposed: it stays on the flow line as the port
   the flow suggests; a deployment takes it when it is free and the host allows it, and its own
   port otherwise. Two deployments of one flow then differ by the deployment's port.
2. **Tags.** They are the flow's today and access binds on them ("a binding on one covers it").
   With several deployments, a team may own one customer's deployment and not another's. Proposed:
   stay the flow's in part 1; decide in part 2 whether a deployment adds its own.
3. **An agent changing settings.** Proposed: waits for a person, as an agent version does.
4. **How many saves a flow keeps** (part 2). Proposed: as many as a deployment runs, plus the
   last ten.
