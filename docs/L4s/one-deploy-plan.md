<!-- holon:dev-phase-only -->
*A dev-phase document: a plan for this repository's development, not for the launch repository.*

# One deploy: the browser, the terminal, `holon deploy` and agents deploy a flow exactly alike

The owner, 2026-11-18: "both deployments need to be exactly the same." Today a flow deployed
from the web console and the same flow deployed from `holon editor`, `holon deploy` or an agent
(`holon mcp`) can be granted different things, and a redeploy from one can quietly undo what the
other set. This plan says why, from the code, and how to make one deploy that every path shares.
Every file, line and field named here was read on `6854900c` (2026-11-18).

## 1. Where it stands

Every path sends the server the same three requests: the units (`POST /unit`), what deploying
would ask (`POST /deploy/preview`), then the deploy (`POST /deploy`). The server takes one body
for both, `DeployRequestBody` (`crates/holon/src/wire.rs:239-264`): `slug`, `unit`,
`acknowledge`, `document`, `requirements`, `uses`, `browsers`, `why`, `note`.

What fills that body differs:

| Field | The browser | The terminal, `holon deploy`, agents |
|---|---|---|
| `unit` | built from the canvas and the settings panel, through the wasm Evaluator, with the panel's tags and taint (`editor/src/canvasOps.ts:22-27`) | compiled from the text (`holon-author/src/agent.rs` `plan`), the text's tags and taint |
| `requirements` | the settings panel's (`editor/src/useDeployFlow.ts:150`, sent at `:155` and `:215`) | the text's: only `optional` (secrets) and `limits` (`holon-author/src/lang/lower.rs:592-598`, `agent.rs:83`, sent at `crates/holon/src/deploycmd.rs:291-293`) |
| `acknowledge` | what the preview said needs approval, `preview.needs_approval` (`useDeployFlow.ts:215`) | everything the flow may do plus what needs approval, `plan.may` + `needs` (`deploycmd.rs` `commit`) |
| `uses` | which saved flows the canvas places, from each node's `data.source` (`editor/src/holonUsage.ts:21-23`, sent at `server.ts:754`) | never sent |
| `browsers` | the panel's (`useDeployFlow.ts:155`) | the text's (`deploycmd.rs:294-296`) |
| `document` | the canvas, its viewport, the settings, the text as typed, its names (`useDeployFlow.ts:205-213`) | the plan's document (`deploycmd.rs` `commit`) |

The paths that compile from the text all go through `agent::plan`: the terminal's deploy and
`holon deploy` (`deploycmd.rs:88`), a save from the command line (`flowscmd.rs:136`), and an
agent's check and deploy (`mcp.rs:507`, `:608`).

## 2. The differences, and what each does to someone

### D1. Four settings the text can't say

The settings panel's settings are `FlowSettings` (`editor/src/DeploySettings.tsx:32-44`) with its
`Requirements` (`editor/src/server.ts:765-778`). The text's flow line has six options: `port`,
`keep`, `limits`, `browsers`, `taint` and `tags` (`holon-author/src/lang/ts.rs:762-866`; anything
else is refused at `:866`). It has no way to say:

- **plain HTTP to** a host (`requirements.plain_http`, the panel at `DeploySettings.tsx:728`);
- **private addresses**, this machine and its network (`private_addresses`, `:747-753`);
- **agents may run** an action (`agents`, `:430-459`);
- **agents may read its logs** (`agent_logs`, `:468-487`).

**Example.** A flow calls `http://192.168.1.20:8080`. Deployed from the terminal, its deploy asks
for no plain HTTP, the server grants none, and each call is stopped before it leaves (seen live on
2026-11-18, in step 8 of the terminal plan). From the browser, ticking the panel's box makes it work.

### D2. Only a secret can be optional in the text

`with { optional: "true" }` is refused on any import but a secret (`ts.rs:516-517`: "only a secret
is imported as optional, for now"). The panel marks any permission optional that doesn't start a
run (`DeploySettings.tsx:367-392`). A flow that can run without a host it calls is optional in the
browser and required from the terminal: revoking that host pauses the one and not the other.

### D3. Writing a flow as text drops those settings without a word

A flow opened in the terminal is its saved or deployed document written as text
(`crates/holon/src/workbench/app.rs:2657-2666` `latest_text`, through
`holon-author/src/agent.rs:100-109` `text_of`). `text_of` keeps the text as typed when it compiles
to the document's unit (`says_its_canvas`, `:111`), and the unit doesn't hold the requirements, so
a text that doesn't say them still "says its canvas". Otherwise the printer writes the flow line
from the document's settings (`lang/print.rs:1124-1163`): port, keep, limits, browsers, tags and
taint, and nothing else; it refuses a non-secret optional (`print.rs:629-632`) but drops plain
HTTP, private addresses and the two agent settings silently.

The server then grants each version only what its own deploy asked: the manifest is built from
the body's `requirements` (`crates/holon/src/deploy.rs:442`) and kept as the grant's (`:630`).

**Example.** Ada ticks "agents may run reset" in the browser and deploys. Bob opens the flow in
`holon editor`, fixes a typo and deploys. The new version asks for no `AgentRuns("reset")`, so
agents can no longer run `reset`, and neither Ada nor Bob is told. The same happens when Bob saves
it from the terminal (`workbench/client.rs:411-418` saves the plan's document) and Ada deploys
that save.

### D4. The same text deploys differently in the browser

When the browser reads a typed text, it keeps what the text doesn't say from the panel's earlier
state (`editor/src/tabs.ts:126-133` `settingsSaid`): every limit but `keep_runs` and
`keep_run_days` (`TEXT_LIMITS`, `:110`, a list from before the text had `limits:`), and the
exceptions. **Example:** a text with `limits: { run_budget_ms: "60s" }` deployed from the browser,
the line then deleted and deployed again from the browser: the browser still asks for 60s, while
the terminal asks for none.

### D5. They acknowledge different lists

The browser acknowledges exactly what the preview said needs approval; the terminal acknowledges
everything the flow may do as well (`plan.may`), which includes its modules' limits
(`agent.rs:64-67`, `Limit(module_fuel=…)`). The server keeps a limit the host lowered only while
the new deploy doesn't acknowledge that limit again (`deploy.rs:514-523`). So, as the code reads, a
module limit an admin lowered stays lowered after a browser redeploy and goes back up after a
terminal one. Step 1 confirms this with a test before anything is changed.

### D6. Which saved flows a flow places

The browser sends `uses`, read from canvas nodes placed from a saved flow (`data.source`). The
text has no record of that: neither `lang/lower.rs` nor `lang/print.rs` reads a node's
`data.source`, and a placed flow is written out in full. The terminal sends none, so the Flows
page's "used by" (`holonUses`, `holonUsage.ts:30`) loses a deployment deployed from the terminal.
It changes no grant.

### D7. The document

The browser sends its canvas layout with the deploy; the terminal sends the plan's. That is what
view mode draws, not what runs, so it changes nothing a person is granted.

## 3. Who is affected

- Anyone who deploys from the terminal, with `holon deploy` or through an agent a flow that calls
  plain HTTP or a local address, that agents may run or read, or that can run without a host:
  D1, D2.
- Any team where one person uses the browser and another the terminal or an agent, on the same
  flow: D3, D5.
- Someone who only uses the browser and edits the text: D4.
- Someone who only uses one path and none of these settings: not affected.

## 4. The design: the unit says everything, and the server reads only the unit

The server already does this for two requirements: `with_declared`
(`crates/holon/src/permissions.rs:359-366`) sets `unrecorded` and `tags` from the unit "in place
of any a client sent: what the unit says is what is asked", and the fields say "never taken from
what a client sends" (`:331`, `:340-341`). Tags and taint are frozen params of the server unit
(`holon-author/src/routes.rs:842-855` `server_unit`), written only when set, so a flow without
them hashes as before; core reads them back (`holon-core/src/declare.rs:279-287` `Server`, `:474`
`read_server`).

The plan extends that to every setting a deploy asks for:

1. **The text says every setting.** New options on the flow line, beside the ones there (names
   to settle, §7 Q3): `plainHttp: ["192.168.1.20:8080"]`, `privateAddresses: true`,
   `agents: ["reset"]`, `agentLogs: true`. `with { optional: "true" }` on any import whose
   permission doesn't start a run. Parsed in `ts.rs` `kind` and the import attributes
   (`:480-548`), lowered in `lower.rs` `settings` (`:563-600`), printed in `print.rs` `settings`
   (`:1124-1163`) and its optional check (`:629-632`), typed in `holon-author/src/holon.d.ts:40-59`,
   documented in `docs/LANGUAGE.md`'s settings table (`:1747-1754`).
2. **The unit holds them.** Each is a frozen param of the server unit, written only when set
   (`server_unit`, `UnitSettings` at `holon-author/src/builder.rs:62-70`, the wasm Evaluator's unit
   arguments at `canvasOps.ts:22-27`), read back by core's `Server`. The limits and `browsers` go
   there too, so nothing a deploy asks for comes from beside the unit.
3. **The server reads them only from the unit.** `with_declared` takes every field of
   `Requirements` from the unit, and `DeployRequestBody` loses `requirements` and `browsers`
   (dev phase: no compatibility). Then two deploys of the same unit ask for the same things,
   whichever path sent them, and a text that doesn't say a setting can't carry it.
4. **One acknowledge rule.** Every path acknowledges what the preview says needs approval, as the
   browser does (D5); `deploycmd.rs` `commit` stops adding `plan.may`.
5. **The browser's panel edits the text.** A change in the panel rewrites the flow line (the
   printer already writes it from settings); `settingsSaid`'s carry-over and `TEXT_LIMITS` go;
   the deploy and its preview send no requirements (`useDeployFlow.ts:150-215`,
   `ApprovalsPage.tsx:172`, `ReviewPage.tsx:137`).
6. **`uses`** is settled by §7 Q1.

What stays as it is: lowering a limit, revoking or granting back after a deploy is the grant's,
not the unit's (`POST /deployments/{slug}/settings` and `…/grant`), on both paths already.

## 5. How it stays the same: three class tests

- **Same text, same deploy.** For every sample, the browser's way (vitest, the wasm Evaluator,
  canvas and settings from the compiled text) and the terminal's (`agent::plan`) give the same
  unit hash. With §4.3, the same hash means the same deploy.
- **The server ignores what a client sends.** For every field of `Requirements`, a deploy body
  that says otherwise than its unit is granted what the unit says (a table test, one row a field).
- **Text round trip loses nothing.** For every field of `Settings`, a document with it set,
  written as text and compiled back, has it set the same; the printer refuses a setting it can't
  write rather than dropping it.

## 6. Steps, one commit each

1. **Tests that show today's differences**, failing now: the round trip (D3), the browser's
   carry-over (D4), the acknowledged module limit (D5). Confirms D5 or corrects §2.
2. **The language**: the new options and optional imports; print, lower, `holon.d.ts`,
   `LANGUAGE.md`; `holon fmt` stable; the round-trip class test passes.
3. **The unit and the server**: the new params, core's `Server`, `with_declared` for every field,
   `requirements` and `browsers` gone from the body; the server's class test passes. Goldens move
   for flows that set them.
4. **One acknowledge rule** (D5).
5. **The browser**: the panel writes the text; the deploy and preview send no requirements;
   wasm rebuilt (`wasm-pack build`, then `wasm-pack test --node crates/holon-wasm`); the
   same-deploy class test passes.
6. **The terminal**: its deploy dialog and Settings rows already show what the server previews
   and grants (`workbench/settings.rs:84-97`); checked, and its tests extended.
7. **A sample flow** that calls a service on this machine over plain HTTP, can run without it,
   and lets agents run one action and read its logs; its entry in `samples.rs`, covered by
   `editor/src/samples.test.ts`, deployed by `e2e/sweep.e2e.ts`, run live.
8. **The manual**: `docs/manual/guides/deploying.md`'s settings table says where each lives in
   the text; `docs/manual/reference/cli.md`.

## 7. Open questions

1. **`uses`.** The text doesn't record which saved flow a placed one came from (D6). Either the
   deploy stops sending `uses` and the Flows page reads it from saves only, or the text records
   it. Proposed: from saves only; it changes no grant.
2. **`browsers` in the unit.** It is the same on both paths today (both read the text), but sent
   beside the unit. Proposed: into the unit with the rest, so nothing is sent beside it.
3. **The options' names.** Proposed: `plainHttp`, `privateAddresses`, `agents`, `agentLogs`, as
   the flow line's other options are single words or camelCase in TypeScript.
