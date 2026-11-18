<!-- holon:dev-phase-only -->
*A dev-phase document: a plan for this repository's development, not for the launch repository.*

# The terminal editor catches up with the web console: implementation plan

`holon editor` (`crates/holon/src/workbench/`, ratatui) was built on 2026-11-01, and its last
feature was the Logs section (2026-11-10, `690b7751`). Since then main has had about 45 PRs:
principals and access (#72–#77, #89, #90), approvals in steps (#81), declines with a reason (#67),
taint and secret settings (#82, #98, #105), flow-data encryption at rest (#91), and the web console's
steps 1–12 (`web-console-L4.md` §14). The web console took them all in. The terminal took in only
what reached it through shared code: the review that shows everything a version changes (R25/R24),
`holon login`, and refusals for disabled owners.

This plan says what to add to the terminal, in what order, and where each piece comes from. Every
route, field and function named here was read in the code on `efa5db89` (main, 2026-11-17).

## 1. Where it stands

**Routes.** The server has 131 routes (`can.rs:539` `ROUTES`). The editor's row in the surfaces
table (`surfaces.rs:104`) names 65, and a test holds the editor to it
(`workbench/tests.rs:1663` `the_editor_shows_what_the_server_keeps_only_to_its_grants`, which
records every call through `Recorded` at `:1642` and checks it against the row at `:1743`). The 66
routes it never calls are the gap: decisions (`/runs/waiting`, `…/runs/{id}/decide`, `/proposals*`,
`/access/proposals*`), access (`/groups*`, `/roles` writes, `/bindings*`, `/access*`,
`/auth/me/can`, `/auth/owned`), resources (`/admin/mails|feeds|queues*`, `/admin/secrets*`,
`/admin/storage`, `/admin/keys/*`, `/admin/folders/browse|pick`), flow settings
(`…/settings`, `…/secrets/{name}/rollback`), replay (`…/replay`), run facts (`/summary`,
`/summary/check`, `…/feeds`, `…/queues`, `…/channels`, `/version`), and saved-flow deletion
(`DELETE /flows/{name}`).

**What the terminal has.** Seven pages (`app.rs:24` `Page`): Overview, Approvals, Flows, Flow
(sections `flow.rs:15`: Status, Runs, Logs, History, State, Access, Graph), Editor (tabs, checking,
completion, review, graph, View runs), Library (samples), Admin (`admin.rs:14` `Part`: People, API
keys, Folders, Databases, Secrets, Server). It talks through `client.rs` (`Client::get/post/patch/
delete`, `:100-114`; there is no `put`) over the `mcp::Api` trait (`mcp.rs:84`), and its tests
drive the real router in-process (`workbench/tests.rs:17` `InProcess`, `:48` `admin_app`).

**A risk in what it has.** The Approvals page treats an agent's proposal as "a version saved since"
(`mcp::waits_for`, `mcp.rs:639`) and rebuilds it from the newest saved text: `sync_approval`
(`app.rs:1839`) → `latest_text` (`app.rs:2383`) → `agent::plan` → `staging`. The server marks a
proposal approved only when the deploy's slug **and unit hash** match it (`proposals.rs:119`
`approved`, called from `deploy.rs:705`). If the saved text compiles to another unit (a later save,
or a round trip that changes the unit), the terminal deploys something nobody proposed, and the
proposal stays waiting with no word said. Step 2 fixes this.

## 2. Goals and non-goals

**Goals**

- A person who works in the terminal can make every **decision** the web console offers:
  approve or decline a run waiting at an approval, a proposed version, an access change.
- Every deploy review in the terminal shows what the web review shows: replay on recent runs,
  host-policy lines, "while it runs", a note kept with the grant, and the warning for an approval
  nobody can decide.
- A flow's settings, people and access, host resources, policy and a run's record can be read and
  changed from the terminal as far as the web allows.
- Keys and rows the caller may not use are marked so, with the server's refusal, before a key is
  pressed (as the web's `can.tsx` does).

**Non-goals**

- The canvas: placing and wiring nodes, composites, projection, tidy layout. The terminal keeps its
  read-only graph (`graph.rs`).
- Signing in, the first-run setup, invites and password changes: `holon login` does these
  (`logincmd.rs:67`, `:106`).
- Account preferences (`/auth/prefs`), the marketplace, the built-in manual and Getting started.
  The Library keeps its samples; the Inspector keeps hover and forms.
- New server routes, except where §14 asks for one as an open question.

## 3. Principles

- **Reuse the command line's code.** Most of what the terminal needs is already a formatter or a
  request builder in a `*cmd.rs` file. Make it `pub(crate)` and call it, rather than writing a
  second one. Where the only logic is in TypeScript (`editor/src/*.ts`), port it once into Rust
  next to the command line's code, so `holon` commands can use it too.
- **One module per new area.** `app.rs` is 2477 lines. New pages and parts go in their own
  modules (`workbench/approvals.rs`, `workbench/access.rs`, `workbench/host.rs`, `workbench/settings.rs`),
  each with its `ui/` drawing, as `admin.rs` and `flow.rs` do now.
- **Ask `can` once, keep it.** One `GET /auth/me/can?actions=…[&flow=…]` per page and flow, kept
  until `/auth/me`'s `access_version` changes, a request is refused, or a minute passes, as
  `editor/src/can.tsx:240` does.
- **Same words as the web.** Confirm texts, refusals and toasts follow the web's (cited per step),
  so a person reads the same thing in both.
- **Every new route goes in the editor's row** (`surfaces.rs:104`) in the same commit. The
  workbench test at `tests.rs:1663` fails otherwise, and the manual's page
  (`docs/manual/reference/surfaces.md`) is written again with `HOLON_BLESS=1`
  (`src/tests/run_steps.rs:316`).

## 4. Step 0: groundwork

One commit, no new screen.

- `Client::put` (`client.rs:100-114`): `deploycmd::call(…, "PUT", …)`, as the others.
- `Client::can(actions, flow, approval)`: `GET /auth/me/can?actions=a,b[&flow=][&approval=]`
  (`bindings.rs:879` `me_can`). It answers `{"<action>": true | "<refusal>"}`. Keep the answers in
  `Client`, keyed by flow, cleared when `/auth/me`'s `access_version` (`auth.rs:264`) changes. Read
  `access_version` in `Client::refresh` (`client.rs:117`), which already reads `/auth/me` once.
- A `May` helper for drawing: a key hint drawn dim with the refusal beside it, and a key that says
  the refusal instead of sending. Use it first on the Admin page: today a non-admin sees "This page
  is the server's admins'" (`ui/admin.rs:22`) for every part. Instead, each part asks its action
  (`people.view`, `role.view`, `resource.view`, `policy.view`, `host.view`; catalogue at
  `can.rs:54`, meanings in `access-and-approvals-L4.md:108-126`) and shows what the caller may see.
- A reason line: `Modal::Input` (`app.rs:320`) with a `required` flag, so a decline can't be sent
  empty. The web requires one for every decline (`ApprovalsPage.tsx:453-455`, `:50-60`, `:686-687`).
- Make these `pub(crate)`: `accesscmd::proposal_state` (`:750`), `proposal_lines` (`:766`),
  `decided_line` (`:790`), `approval_lines` (`:742`), `role_lines` (`:704`), `group_lines` (`:717`),
  `version_lines` (`:731`), `document_lines` (`:821`), `scope` (`:578`), `until` (`:593`);
  `mcp::proposals_of` (`mcp.rs:817`); `deploycmd::proposed_lines` (`deploycmd.rs:231`).
- Fix in passing: `surfaces.rs:17` says `docs/manual/guides/surfaces.md`; the page is
  `docs/manual/reference/surfaces.md`.

**Routes added:** `GET /auth/me/can`.
**Tests:** a person bound only `viewer` opens Admin and sees People read-only, with the refusal on
`n` (invite); ada sees every part as today.

## 5. Step 1: runs waiting at an approval

**The page.** Approvals gets three parts, cycled with `tab` as Admin's are (`admin.rs:24` `PARTS`):
**Deploys**, **Runs waiting**, **Access changes**, each with its count, or `?` when its read failed
(the web never shows an unread list as empty: `useApprovals.ts`). The top bar's "N waiting for you"
and the sidebar's Approvals count all three. Move the page's state (`Approval`, `Pending`,
`sync_approval`, `approval_key`, `approve_picked`, `resolve`: `app.rs:68-85`, `:1839-1929`) into
`workbench/approvals.rs`.

**Reading.** `GET /runs/waiting` (`durable.rs:1559`, signed in; filtered per caller at `:1579`)
→ `{runs:[{slug, run_id, kind, route, by, parked_at, waiting:[point], delivered:[…]}]}` (`summary()`
`durable.rs:1068`). Each approval point (`StoredWaiting::listed`, `durable.rs:730`) has `name`,
`expires_at`, `steps` (canonical, e.g. `agent:classifier>person:reviewer+person:approver`), `said`,
`slots:[{slot, stage, role?, kind, people, starter?, by:[…], open}]` (`durable.rs:517`),
`decisions:[{slot, who, kind, role?, at, approved, reason?, note?, binding?, access?, via?}]`
(`durable.rs:422`), `shows` or `input` (only to a holder of run.steps or a decider), and
`yours:[slot]` (`durable.rs:1594`): the steps this caller may take now. Read it on every refresh
(`REFRESH`, `app.rs:569`), oldest `parked_at` first, as the web sorts (`ApprovalsPage.tsx:534`).

**The list.** Columns as the web's `RunsWaiting` (`ApprovalsPage.tsx:514-659`): flow, what it is
(route or kind, the point's name, delivered events), started by, waits for (the chain: `✓` filled,
`◻` open, dim not open yet, as `Chain` `:475` and `decisions.ts:98` `chainOf`), expires, since.
Reuse `decidecmd::waiting_lines` (`decidecmd.rs:212`) and `yours_lines` (`:239`) for the
right-hand pane's words, and `decidecmd::step_numbered` (`:187`) for each step.

**Deciding.** `a` approves, `d` declines the first step in `yours` of the picked run; with several,
a picker of the steps. A confirm with the web's words (`decideAsked`, `ApprovalsPage.tsx:434-443`):
"Approve "payout" as approver for run 3f2a… of payouts (POST /pay)?", what it shows, then "It then
waits for …" or "The run goes on once you do." Approve takes an optional reason; decline needs one.
Send `POST /deployments/{slug}/runs/{run_id}/decide {name, approve, reason?, slot}`
(`durable.rs:1784` `DecideBody`, `deny_unknown_fields`; `slot` always sent, so the server's 409
"say which, with slot" at `durable.rs:1694` can't happen). The answer is `{name, slot, slots,
outcome: waiting|approved|declined, run_error?}` (`durable.rs:1858-1875`); say it with
`decidecmd::decided_said` (`:145`). A row with an empty `yours` shows no keys, and says why from
`can` (`run.decide` on the point) or "you started it" / "you deployed it" from the refusal.

**Elsewhere.** The flow page's "◷ a run waits" lines (`ui/flow.rs:242`) and a run's "Waits at"
(`flow.rs:1226`) become rows: `enter` opens Approvals › Runs waiting with that run picked. The
palette (`palette.rs:39-97`) gets "approve run #N of X", as the web's (`commands.ts`).

**Routes added:** `GET /runs/waiting`, `POST /deployments/{slug}/runs/{id}/decide`.
**Tests** (`workbench/tests.rs`; accounts as `admin_app`, flows as `src/tests.rs:2487-2520` and
`:2804-2856`):
- a `people: 2` approval: ada approves, the row shows one of two; bob approves, the run goes on;
- a decline with no reason is not sent; with one, the run ends declined and the reason is on it;
- steps with roles: wes, bound at the approval point only, sees his step and no other;
- the run's starter sees the row and no keys, with the reason;
- every width (`tests.rs:1545` `every_page` gets the Runs waiting part).

## 6. Step 2: proposed versions

**Reading.** `GET /proposals` (`proposals.rs:235`, flow.view) → `{proposals:[…]}`: waiting ones
oldest first, then up to 50 decided. Each: `{id, slug, unit, why, note?, by?, agent, at_ms,
needs:[…], saved_at?, state, decided_by?, decided_at_ms?, reason?, version?}` (`proposals.rs:31`,
`:170`). `GET /proposals/{id}` adds its `document`.

**The Deploys part.** One card per waiting proposal, first: slug, "v3 → v4", its place in the queue
("1 of 2"), who proposed it (agent or person) and when, why (or "no why was given"), note, and the
rows that change, `+` new and `−` no longer, with "N the same" (the web's `ProposalCard`,
`ApprovalsPage.tsx:203-310`). Then today's rows from `mcp::waits_for`, without "a version saved
since" for a slug that has a waiting proposal (the web merges the same way: `approvals.ts:56-80`).

**Reviewing and approving.** Stage **the proposal's own document**, not the newest saved text:
`GET /proposals/{id}` → `agent::text_of(slug, &document)` (`holon-author/src/agent.rs:100`) →
`agent::plan` → `staging` (`app.rs:2395`). If `plan.unit` isn't `proposal.unit` (or
`agent::deploys_as(&document)`, `agent.rs:141`, says another unit), refuse to approve: "This
version's text builds another unit than the one proposed: decline it, and say so." (the web's check,
`ReviewPage.tsx:130-136`; the server doesn't check a person's deploy, `deploy.rs:480`). Approving
deploys with that document (`deploycmd::commit`, `deploycmd.rs:306`). Then read the proposal again;
if it isn't `approved`, say so rather than nothing.

**Declining.** `d`, with a required reason, the web's words (`useDecline`,
`ApprovalsPage.tsx:50-60`): "Why it was declined. payouts v4, proposed by scout. Tell them what to
change. It is kept on the flow's history, and the agent reads it through holon mcp. The version
stays saved, and what runs keeps running." → `POST /proposals/{id}/decline {reason}`
(`proposals.rs:260`, flow.approve-deploy). A second decline is a 409 (`proposals.rs:137`): say it.

**Routes added:** `GET /proposals`, `GET /proposals/{id}`, `POST /proposals/{id}/decline`.
**Tests** (proposal as `mcp.rs:2407`, an agent with `with_people`, `src/tests.rs:118`):
- an agent proposes; ada sees the card with why and note, approves; the proposal is approved and
  the flow runs the proposed unit;
- ada saves another text after the proposal; approving still deploys the proposed unit (the risk
  in §1);
- a document that builds another unit is refused, with the words above;
- a decline needs a reason; the agent reads it (`GET /proposals/{id}` as the agent).

## 7. Step 3: access changes

**Reading.** `GET /access/proposals` (`access_proposals.rs:658`, people.view) →
`{proposals:[newest first], version}`. Each (`access_proposals.rs:120`, state flattened at
`:160-177`): `{id, author, at, why?, change:{kind: bind|role|group|host_role|approvals|local,…},
what, gives, to?, ways_in?, made, base, needs, approvals:[{who, at, via?}], state:
waiting|applied|declined|stale, …}`.

**The part.** Waiting changes: what, "gives …", who could act as whom (`ways_in`), author, when,
why, approvals so far (`proposal_lines`, `accesscmd.rs:766`). Then the last ten decided
(`decided_line`, `:790`), as the web (`AccessChanges`, `ApprovalsPage.tsx:704-797`).

**Keys.** `a` approve, with the web's confirm (`changeAsked`, `:666-671`) → `POST
/access/proposals/{id}/approve` (people.assign; body ignored). `d` decline, reason required →
`POST …/decline {reason}`. For the author, `d` is **withdraw**: the same route, with "Withdraw
access change N? … Why: it is kept with it." (`:686`; withdraw is the author's decline,
`access_proposals.rs:622`). A stale proposal answers 409 "… no longer applies, so it is stale":
say it and read the list again.

**Routes added:** `GET /access/proposals`, `POST /access/proposals/{id}/approve`,
`POST /access/proposals/{id}/decline`.
**Tests** (the world of `tests/scopes.rs:570-601`: ada makes bob an admin, `PUT /access/approvals
{"changes":1}`, then a binding that gives more waits as a proposal):
- bob approves it; the binding is made;
- ada, its author, sees withdraw and not approve;
- a decline needs a reason.

## 8. Step 4: the deploy review, as the web's

Applies to the editor's deploy dialog (`Modal::Approve`, built by `deploy_review`, `app.rs:2355`)
and the Approvals pane (step 2), which share `staging` (`app.rs:2395`).

- **Replay on recent runs**, for a flow already deployed. The units are already staged, so
  `GET /deployments/{slug}/replay?unit={plan.unit}&runs=20` (`verify/replay.rs:139`; needs run.steps
  **and** run.export, `can.rs:671`) and `replaycmd::lines` (`replaycmd.rs:120`). It is slow (a 30 s
  deadline, two at once, 503 with `Retry-After` past that: `replay.rs:71-72`), so it runs after the
  review is drawn, and the review says "replaying…" until it answers. Without the grants, say which
  is missing (from `can`).
- **Replay the draft** from an editor tab: `:replay` and a key in the tab's page keys. Its table as
  the web's `ReplayDraft` (`BottomPane.tsx:256-283`): run, what came in, was (`status`), now
  (`now_status`, `?` for can't tell), same or differs; "no recorded answer" for a can't-tell whose
  why says the run didn't answer (`outcomeSaid`, `:173`). `replaycmd::replay(c.api.as_ref(), text,
  Some(slug), None)` (`replaycmd.rs:103`) stages and replays in one call.
- **A note kept with the grant.** `deploycmd::commit` never sets `note`, and `Staged.body` is private
  (`deploycmd.rs:247`). Add `Staged::with_note(&mut self, note)` (setting `body["note"]`, as
  `mcp.rs:539` does). In the dialog, `n` opens a line for it. The server keeps it on the deploy
  event and the activity log (`deploy.rs:541`, `:643`, `:703`).
- **Host policy lines** (with policy.view): port the web's `policyRows` (`editor/src/reviewMay.ts:164`)
  to Rust beside `grantcmd::policy_lines` (`grantcmd.rs:284`), from `GET /permissions` (`deploy.rs:1425`)
  and `staged.touches`: each host it calls and whether the policy allows it (and grants it without
  asking, `auto`); private addresses; each raised limit against its ceiling; an agent's proposal
  against `agents_propose`; last, whether the caller may approve (`flow.approve-deploy`) or deploy
  (`flow.deploy`), from `can`.
- **While it runs**: port `whileItRuns` (`reviewMay.ts:90`): its port (`plan.port`), run budget
  (`run_budget_ms`, `clock_run_budget_ms`), runs kept (`keep_runs`, `keep_run_days`), from
  `plan.requirements.limits` or the policy's `defaults`; what it holds between runs (touches with
  group `state`); its claims.
- **An approval nobody can decide.** Rows of `staged.touches` with `kind == "approval"`,
  `change != "gone"`, `ready != null` (`touches.ts:83` `cannotDecide`); `touches::lines` already marks
  them `!` (`touches.rs:465-468`). When one has `waits == "every run"`, approving asks again, with the
  web's words (`askAgain`, `touches.ts:94`): "… approve it anyway?"
- **"I read the N new permissions."** The web won't approve until a box is ticked
  (`ReviewPage.tsx:400-406`). The terminal's dialog ends "You approve the N marked +"; see §14 Q2.

**Routes added:** `GET /deployments/{slug}/replay`, `GET /permissions` is already there.
**Tests:**
- a version that changes an answer: the review says "1 differs" and names the run;
- a viewer without run.export sees why there is no replay;
- the note is on the deploy event (`GET /deployments/{slug}/events`);
- a flow whose approval nobody holds: approving asks again;
- a host the policy doesn't allow shows `✗`.

## 9. Step 5: a flow's settings

A new section, **Settings**, after Access (`flow.rs:15` `Section`, `:26` `SECTIONS`), in
`workbench/settings.rs`. Access keeps what it may do and its secrets; Settings takes the rest, as
the web's Settings tab (`editor/src/FlowSettings.tsx`).

- **Limits and runs kept.** Rows from `GET …/grant` (already read: `Detail.grant`, `client.rs:44`) and
  `GET /permissions`'s `defaults` and `ceilings`, computed as the web's `limitRows`
  (`flowFacts.ts:120`; named limits at `:106`: `max_requests`, `run_budget_ms`,
  `max_request_bytes`, `keep_runs`, `keep_run_days`, then any other asked): now, and where it comes
  from (asked, default, lowered, the ceiling). `enter` on one asks for a lower value →
  `POST /deployments/{slug}/settings {lower:{name:n}}` (`proposals.rs:317`, flow.deploy). A higher
  value is refused by the server ("raising it takes a new version, which a person approves",
  `permissions.rs:979-1035`): say it, and offer to open the editor.
- **Network.** Plain-HTTP hosts and private addresses it was granted, each with revoke →
  `POST …/grant {revoke:["PlainHttp(\"host\")"]}` / `PrivateAddresses` (`FlowSettings.tsx:366-422`).
- **Keep running when something required is revoked.** `POST …/grant {keep_running}`
  (`permissions.rs:942` `ChangeRequest`, flow.approve-deploy). Extend `Client::change_grant`
  (`client.rs:283`), which sends only `revoke` and `grant` now.
- **Agents.** "Agents may propose versions" and "agents' versions deploy at once" →
  `POST …/settings {agents_propose}` / `{agents_deploy_at_once}`. The second needs
  flow.approve-deploy, and the policy's `agents_deploy_at_once` (`proposals.rs:294-330`). The owner's
  rule is that agents' versions wait for a person unless a person grants this explicitly: the
  confirm says what turning it on means.
- **Waiting runs.** The flow's parked runs (`Detail.parked`, already read), each a link to
  Approvals › Runs waiting.
- **Sessions.** `GET …/sessions` (today only a `:sessions` dialog, `app.rs:1344`) and `holds`. On a
  stopped flow, "run from it" on a session with state → `POST …/run {from:n}`.
  `Client::lifecycle` already takes `from` (`client.rs:229`); nothing passes it yet (`flow.rs:706`).
- **Lifecycle.** Stop says how many waiting runs it would end. Remove asks for the slug typed, and
  offers "forget its keys too" (host.operate) → after `DELETE /deployments/{slug}`,
  `POST /admin/keys/forget {slug}` (`keys.rs:38`; 409 while it is still deployed, so after).
- **A secret's roll back** (in Access, beside its history): `POST
  /deployments/{slug}/secrets/{name}/rollback {version, dry_run:true}` → `secretcmd::rollback_preview`
  (`secretcmd.rs:316`), then confirm → the same without `dry_run` (secret.roll-back, `can.rs:651`).

**Routes added:** `POST /deployments/{slug}/settings`, `POST
/deployments/{slug}/secrets/{name}/rollback`, `POST /admin/keys/forget`.
**Tests:** lower `keep_runs` to 5 (applied; "lowered"); raise it (refused with the server's words);
keep running on and off; agents may propose off, then an agent's deploy is refused; run from a
session of a stopped flow; remove with forget; a secret rolled back after a dry run.

## 10. Step 6: people and access

Admin's People part stays. New parts, in `workbench/access.rs`, each shown by `can`:

- **Groups** (people.view; changes people.assign): `GET /groups` → `{version, groups:[{name, about,
  members, bound}]}` (`bindings.rs:1127`). Add and remove members, make one, remove one
  (`PUT /groups/{name} {members, about?, why?, base}` with `base` the version read, so a change made
  since is refused; `DELETE /groups/{name}?why=`, 409 while bound). Reuse
  `accesscmd::groups`'s merge (`accesscmd.rs:254`) and `group_lines` (`:717`).
- **Roles** (role.view; changes role.define): `GET /roles` → `{name, about, allows, kinds, builtin,
  bound}` (`bindings.rs:790`). Define or change one: name, about, kinds, the actions it allows
  (picked from the catalogue, `accesscmd::catalogue`, `:563`), why → `PUT /roles/{name} {allows,
  kinds, about?, why?, base?}`. The field is `allows`. Remove (409 while bound; built-ins stay).
- **Bindings** (people.view; changes people.assign): `GET /bindings` (`bindings.rs:967`). Bind:
  to whom (person, agent, group), role, where (host, a flow, flows with a tag, one approval point of
  a flow), until, why → `POST /bindings` with `accesscmd::scope` (`:578`) and `until` (`:593`).
  Today the role dialog sends flow and approval-point bindings to `holon bind` (`admin.rs:409`):
  this replaces that. Unbind with a why → `DELETE /bindings/{id}?why=`.
- **Any change that gives more**, on a host that requires approval, answers 202 `{proposal,
  version}` (`access_proposals.rs:652`): say it with `accesscmd::waiting` (`:176`) and point to
  Approvals › Access changes.
- **History** (people.view): `GET /access/versions` (`version_lines`, `:731`); `enter` on one →
  `GET /access?version=N` (`document_lines`, `:821`). Approval settings (`GET /access`'s
  `approvals`; `ApprovalSettings::each`, `bindings.rs:107`) with change → `PUT /access/approvals`
  (409 when `changes` is above the people who may approve, `bindings.rs:1057-1075`).
- **A principal's page** (`enter` on a person, agent or service): its facts, tokens, bindings (own
  and through groups) and a bind form for it. "What it may do" is computed in the browser
  (`editor/src/access.ts:142` `givesOf`) and there is no server route for another principal's
  permissions; see §14 Q4.
- **Your agents and services**, for someone without people.view: `GET /auth/owned`
  (`auth.rs:275`), with make and revoke token (`GET/POST /people/{name}/tokens`, people.manage on the
  principal, which its owner holds) and its `holon mcp setup` line, as the web's `OwnedPage`.

**Routes added:** `GET/PUT/DELETE /groups…`, `PUT/DELETE /roles/{name}`, `GET/POST /bindings`,
`DELETE /bindings/{id}`, `GET /access/versions`, `GET /access`, `PUT /access/approvals`,
`GET /auth/owned`, `GET /people/{name}/tokens`.
**Tests:** a group made, a member added, a binding to it at one flow; a role defined with two
actions; a binding that gives more waits as a proposal on a host that requires approval; someone
holding role.view only sees Roles and no change keys; an agent's owner, not an admin, makes a token
for it.

## 11. Step 7: host resources, server secrets, policy

In `workbench/host.rs`. Admin's parts become: People, Groups, Roles, Bindings, Access history (step
6); Folders, Databases, Mail, Feeds, Queues, Secrets, Storage (this step); Policy, Server.

- **Mail, Feeds, Queues** (resource.view; bind and unbind as their names say): `GET /admin/{kind}`
  (a bare array, the server's then each deployment's). Add with a line of the command's own flags,
  as the folder form does today, sent as `POST /admin/{kind} {name, deployment?, …}`
  (`mails.rs:120`, `feeds.rs:179-219`, `queues.rs:83`). Their rows are formatted by closures inside
  `mails::cli` (~`:997`), `feeds::cli` (~`:966`), `queues::cli` (`:905`): lift each to a
  `pub(crate) fn line(&Value) -> String` the command and the editor share, and the flag parsing
  likewise. Unbind → `DELETE /admin/{kind}/{name}[?deployment=]`.
- **Used by.** Each resource row lists each deployment that uses it and how, and in amber what a
  waiting version would add. Port `editor/src/holdings.ts` (`reaches` `:24`, `holdersOf` `:61`,
  `waitingFor` `:88`, `secretReaders` `:107`, `databaseMatrix` `:123`) to Rust, over
  `GET /admin/grants` (`grants.rs:68`) and `GET /proposals` (step 2).
- **Server secrets** (replaces today's Secrets part, which lists only env names and each flow's):
  `GET /admin/secrets` → `{names, secrets:[{name, version, source, set_at, problem?, server?, to?}]}`
  (`secrets.rs:350`); set from a value, `env:` or `vault:`, with where it may go
  (`POST /admin/secrets/{name} {value | from, to?}`); "where" alone (`…/cap {to | null}`); history
  (`GET /admin/secrets/{name}`, `secretcmd::history_lines`, `:288`); roll back after a dry run
  (`…/rollback`); remove (`DELETE`). The value is never shown, as everywhere.
  `Client::secret_history` (`client.rs:365`) is tied to a deployment's path: give it the server's too.
- **Storage** (host.operate): warn at, full at, when full, backup, sweep every →
  `POST /admin/storage` (`storage.rs:1250`; 409 without a policy file). `GET /admin/host` is read
  already (`admin.rs:291`).
- **Policy** (policy.view; reload policy.edit): its own part. `GET /permissions` answers the file's
  `text` (`permissions.rs:479`): show it with line numbers. The limits table with default and
  ceiling. Held off: deployments with `attention` (from `GET /deployments`, as
  `HostPages.tsx:1482-1497`). Reload keeps today's confirm (`admin.rs:270`). Checking flows and
  waiting versions against it: see §14 Q3.
- **Folders** gain browse (`GET /admin/folders/browse?path=`, `pick.rs:60`): a list of folders to
  walk, in place of typing a path. The native pick (`POST /admin/folders/pick`) opens a dialog on
  the server's machine: leave it out of the terminal.

**Routes added:** `GET/POST/DELETE /admin/mails…|feeds…|queues…`, `GET/POST/DELETE
/admin/secrets…`, `POST /admin/secrets/{name}/cap|rollback`, `POST /admin/storage`,
`GET /admin/folders/browse`, `GET /proposals` (already in step 2).
**Tests:** a queue bound to one deployment and unbound; a server secret set from `env:` with `to`,
its history, a roll back; a resource used by a deployment and by a waiting version (amber); the
policy text shown; a viewer holding resource.view only sees resources and no bind keys.

## 12. Step 8: a run's record, state and the run log

- **A run's detail** (`flow.rs:1226-1264`) gains, from the log entry it already has
  (`recorder.rs:87-232`):
  - **Decided**: each approval's decisions, from `events[point]`'s `decisions` (who, role, when,
    approved, why, binding), as `decisions.ts:153-197` reads them.
  - **Secrets it read**: `secrets` (`"v3 #a1b2c3d4"`) and `committed` (`recorder.rs:184-189`), and where
    each went: the hosts of requests in `rounds[].exchanges[].request` whose text holds
    `‹secret:name` (`flowFacts.ts:51-64`). Never the value.
  - **Read**: `sourced` (clock, random, what came in, `recorder.rs:113`) and `reads`.
  - **Witnessed**: from `GET /summary`'s `chain.witness.taken_ms` against the run's `at_ms +
    duration_ms` (`flowFacts.ts:74`).
  - **Export this run**: `GET …/export?run=N` (`verify.rs:213`) to `<slug>-run-N.json`, beside
    today's whole export (`flow.rs:586`), with the `holon verify` line.
- **State**: collections page with `next` (`collections.rs:131`: `{items, next}`), not the first 100
  only (`client.rs:329`); an item's history (`…/changes?name=&item=`, `changes.rs:55-100`, through
  `history::change_lines`, `history.rs:239`).
- **A flow's feeds, queues and channels** on Status: `GET …/feeds`, `…/queues`, `…/channels`
  (`feeds.rs:780`, `queues.rs:731`, `channels.rs:33`; state.view): connected or not, since when,
  waiting, sent.
- **The run log**: the sidebar's SERVER footer gets "check now" (host.check) →
  `POST /summary/check` (`summary.rs:192`): runs checked, breaks (each a link to its run), heads.
- **Overview from `/summary`.** Today the overview reads `…/log?limit=50` for each deployment that
  ran (`client.rs:136`). `GET /summary?hours=24` (`summary.rs:132`) answers the counts, sparks,
  `p50_ms`, latest runs and the chain in one call. Use it when the caller holds run.view on every
  flow, and keep the per-flow reads otherwise.

**Routes added:** `GET /summary`, `POST /summary/check`, `GET /deployments/{slug}/feeds|queues|channels`.
**Tests:** a run decided in two steps shows both decisions; a run that read a secret and sent it
to a host names the host and not the value; a collection of 250 items pages to the end; check now
on a log edited outside holon reports the break.

## 13. Step 9: the smaller items

- **Delete a saved flow** from Flows: `DELETE /flows/{name}` (`flows.rs:159`, flow.delete), confirm.
- **The palette** (`palette.rs:39-97`): runs waiting, proposals, access changes, a run by number
  (the web's `commands.ts`).
- **Keys help** (`ui/modal.rs:26` `HELP`): every key this plan adds.
- **The manual**: `docs/manual/reference/cli.md`'s `holon editor` section and
  `docs/manual/guides/deploying.md:245` say what the terminal now does.

**Routes added:** `DELETE /flows/{name}`.

## 14. Open questions

1. **How far.** Steps 1–5 give a terminal user every decision and every flow setting. Steps 6–7
   are the host's administration, which an admin may well do in the browser. Proposed: do 1–5
   first, then decide on 6–9.
2. **"I read the N new permissions."** The web won't approve until a box is ticked. The terminal's
   dialog says "You approve the N marked +" and takes `enter`. Proposed: when there are new
   permissions, `enter` asks once more, naming them ("Approve 2 new permissions: Secret("stripe"),
   Http("api.stripe.com")?").
3. **Checking policy, and the limits' meanings.** The web checks flows and waiting versions against
   the policy in TypeScript (`policyMeans.ts:67-98`, a copy of `permissions.rs`'s `refuses_item`
   and `is_auto`), and the forty limit meanings exist only in TypeScript (`policyMeans.ts:9-51`).
   Proposed: move both to Rust (the meanings beside `holon_author::LIMIT_NAMES`,
   `holon-author/src/lib.rs:56`), export them through the wasm bridge, and have the web read them
   there, so there is one copy.
4. **What another principal may do.** The web works it out in the browser (`access.ts:142`
   `givesOf`); there is no server route. Proposed: a server route, `GET /people/{name}/can`, that
   answers as `/auth/me/can` does, so the web, the terminal and `holon` share it, rather than a
   third copy of the rules.
5. **Polling.** Steps 1–3 add three reads every five seconds (`/runs/waiting`, `/proposals`,
   `/access/proposals`). Proposed: read them every refresh while Approvals is shown, and every 30 s
   otherwise, for the counts (the web's 4 s and 30 s, `useApprovals.ts`).

## 15. Tests and checks, for every step

- `workbench/tests.rs`: the step's own tests, against the real router in-process, with accounts
  where a decision needs a named person (nobody can decide on a server without accounts,
  `durable.rs:1803-1816`).
- The editor's row in `surfaces.rs:104` names every route added; the test at `tests.rs:1663` holds
  it, and `HOLON_BLESS=1` writes `docs/manual/reference/surfaces.md` again.
- `every_page` (`tests.rs:1545`) gets each new page and part, so each is drawn at every width, and
  the light-theme test covers them.
- Builds and tests go through the turn queue (`/c/Users/BMA/.holon-turns/turn.sh`).
- **Run live once before each commit**: `target/release/holon.exe serve` with a sample that uses
  the feature (step 1: `approved-payout`, `reviewed-draft`; step 2: an agent's version through
  `holon mcp`), driven from `target/release/holon.exe editor`. The commit says so.
- **No new sample.** As in `web-console-L4.md` §13, this is a console over features that each have
  one.

## 16. Steps

One commit each, in this order:

0. Groundwork: `put`, `can`, the reason line, `pub(crate)` formatters, Admin parts shown by `can`.
1. Runs waiting at an approval: Approvals' three parts, the list, deciding, links from the flow page.
2. Proposed versions: the cards, approving the proposed unit, declining with a reason.
3. Access changes: approve, decline, withdraw.
4. The deploy review: replay, the draft's replay, the note, policy lines, while it runs, an
   approval nobody can decide.
5. A flow's Settings section, and a secret's roll back.
6. People and access: groups, roles, bindings, history, a principal's page, your agents and services.
7. Mail, feeds, queues, server secrets, storage, policy, used by, folder browse.
8. A run's record, state paging and item history, a flow's feeds and queues, check now, overview
   from `/summary`.
9. Delete a saved flow, the palette, keys help, the manual.
