# Access and approvals: principals, roles, and approvals in steps (L4)

Who may do what on a host, and who must agree before a run goes on. Written 2026-11-09, before
the code. It replaces the four fixed roles of `access.rs` and generalises `await approval(…)`
(gaps-web.md §8, built on this branch as `people: N`) into an approval in steps, each filled by a
principal holding a role.

The aim, in one example: a payout run waits until an agent has classified it, then an editor has
reviewed it, then an admin has approved it, each a different principal, in that order, and the
run's evidence shows all three by name, from the host, checkable offline.

## 1. Where it stands

**Identity** (who you are) is in place and stays:

- accounts (`accounts.rs`): a name, an argon2id password set through an invite link, an email,
  `disabled`, prefs; sessions; the first admin set up with a printed code;
- API tokens (`token:<label>`), made by an admin for a script;
- signing in through an identity provider (`oidc.rs`; an email taken only when verified);
- the OS user who runs the server, as a local admin through its run file (`local.rs`).

**Authorization** (what you may do) is what changes:

- each account and token has one `Role`: `viewer`, `editor`, `admin` or `agent`;
- `Access::check` decides by matching the role against the method and URL: a viewer GETs, `/admin/…`
  and secret writes are an admin's, `agent_may()` lists the routes an agent may POST to;
- an agent is either an account with role `agent`, or an editor or admin sending
  `X-Holon-Role: agent`, which turns into `ada (agent)` with the agent role;
- deciding an approval (`durable.rs` `decider()`): any person signed in as an editor or admin;
  never an agent, a viewer, an API token, or anyone on a server without accounts;
- `await approval(name, { people: N })` lowers to the `approval` leaf, whose capability is
  `Approval("name/N")`; N different people approve; one decline ends it.

What this can't say: "only payments people approve payouts", "this agent may call refunds but not
billing", "an agent classifies, then a person reviews", "the person who started the run may not
approve it". Each needs roles that aren't the four built in, scoped to less than the whole host,
and an approval that names roles and an order.

## 2. Goals and non-goals

Goals:

1. **Principals of three kinds**, people, agents and services, each with its own name, credentials
   and audit trail. An agent is a principal, not a person's session wearing a header.
2. **Roles an admin defines**, as sets of permissions from a fixed catalogue; the four of today kept
   as built-in roles.
3. **Bindings with a scope**: a role given to a principal or a group, on the host, on some flows, or
   on one approval point.
4. **One decision function**, `can(principal, action, resource)`, behind every route.
5. **Approvals in steps**: slots filled by principals holding a role, of a kind, in order or not,
   with separation of duties enforced by the host.
6. **Everything on the record**: changes to access, every decision, and who filled which slot, in
   the activity log, the run's timeline and the evidence export.
7. **Access is itself governed**: changing roles and bindings can require approval, as a deploy can.

Non-goals here (each a later step, §14): deny rules written by admins; attribute conditions
("amount under 500"); SCIM provisioning; notifications to the people asked; delegation and
out-of-office; withdrawing a decision.

Dev phase: no compatibility. The old `role` field, `X-Holon-Role` and `people: N` are replaced
outright; a host is set up again.

## 3. Principals

```rust
struct Principal {
    name: String,          // unique across kinds: "ada", "refunds-bot", "ci-deploy"
    kind: Kind,            // person | agent | service
    display: Option<String>,
    email: Option<String>, // people only; verified when from a provider
    disabled: bool,
    created_at: u64,
    created_by: String,
    owner: Option<String>, // agents and services: the person answerable for it
}
```

| Kind | Signs in with | Examples | Counts as a person |
|---|---|---|---|
| `person` | password (invite link), identity provider, the run file | ada, bob, the OS user | yes |
| `agent` | its own tokens, made by a person who may manage it | refunds-bot, the classifier | no |
| `service` | its own tokens | ci-deploy, a monitoring script | no |

- **Tokens belong to principals.** Today's `ApiToken { label, role }` becomes a credential of a
  `service` (or `agent`) principal. Its permissions come from the principal's bindings, not from the
  token. A token may be narrowed to a subset (§5.4), never widened.
- **`holon mcp` signs in as an agent.** `holon mcp setup` makes, or asks for, an agent principal
  owned by the person setting it up, and keeps its token as today. `X-Holon-Role: agent` goes. The
  audit says `refunds-bot`, owned by ada, not `ada (agent)`.
- **The OS user** stays a person named after its account, holding `admin` on the host while the
  server runs, as today.
- **A server without accounts** (the tests') stays open, with no principal. Anything that needs a
  person (deciding, §8) is refused there, as today.
- **Owners.** An agent or service has an owner, a person. Disabling the owner doesn't disable the
  agent, but the People page flags it ("its owner is disabled"). Only a principal holding
  `people.manage` on it, or its owner, makes or revokes its tokens.

## 4. Permissions: the catalogue

A permission is an action on a resource type. The catalogue is fixed by the host (code); admins
compose roles from it, never invent actions.

| Resource | Actions |
|---|---|
| `flow` (saved and deployed) | `view`, `edit` (save), `propose`, `deploy`, `approve-deploy` (acknowledge what needs approval), `restore`, `restore-dry-run`, `lifecycle` (run, pause, stop), `delete` |
| `run` | `view`, `call` (send a request, run an action), `decide` (fill an approval slot), `export` (evidence) |
| `state` | `view`, `write` (by hand, a cell's history) |
| `secret` | `list` (names, history), `set`, `roll-back` |
| `resource` (folders, databases, hosts, keys) | `view`, `bind`, `unbind` |
| `people` | `view`, `manage` (principals, tokens), `assign` (bindings) |
| `role` | `view`, `define` |
| `policy` | `view`, `edit` (host limits, ceilings) |
| `host` | `view` (health, disks, store), `operate` (sweep, settings) |

Names are written `resource.action`: `run.decide`, `flow.approve-deploy`. A wildcard
`resource.*` exists only in built-in roles.

Each route says the action and the resource it acts on (§7). A route that touches two (deploying a
flow that binds a secret) asks for both.

## 5. Roles, groups and bindings

### 5.1 Roles

```rust
struct RoleDef {
    name: String,              // "payout-approver"
    about: String,
    permissions: Vec<String>,  // "run.decide", "run.view", "flow.view"
    kinds: Vec<Kind>,          // who may hold it; empty: any kind
    builtin: bool,
}
```

Built in, not deletable, their permissions fixed to reproduce today:

| Role | Permissions | Kinds |
|---|---|---|
| `viewer` | every `*.view`, `secret.list` | any |
| `editor` | viewer's, and `flow.*` but `delete` of others' flows, `run.call`, `run.decide`, `run.export`, `state.write`, `resource.view` | person, service |
| `admin` | everything | person |
| `agent` | viewer's, and `flow.edit` (its own), `flow.propose`, `flow.restore-dry-run`, `run.call` | agent |

`kinds` is how "an agent never approves a deploy" stays true: `flow.approve-deploy` is in no role
an agent may hold unless an admin defines one, and §8.4 adds a host setting that forbids it outright.

### 5.2 Groups

A group is a named set of principals (`payments-team`). It holds bindings like a principal. A group
may hold members of any kind; a binding's role still only applies to members whose kind it allows.
Groups don't nest (one level; nesting is a later step if asked for).

### 5.3 Bindings and scopes

```rust
struct Binding {
    id: u64,
    who: Subject,        // Principal(name) | Group(name)
    role: String,
    scope: Scope,
    by: String,
    at: u64,
    expires_at: Option<u64>, // a time-boxed grant ("on call this week")
}

enum Scope {
    Host,
    Flows(Selector),           // one flow, or a set (§5.5)
    Approval { flow: Selector, name: String }, // one approval point
}
```

A principal's permissions on a resource are the union of the permissions of every role bound to it,
or to a group it is in, at a scope that covers the resource. Host covers everything; `Flows(s)`
covers the flows `s` selects, their runs and their state; `Approval` covers deciding that point
only.

### 5.4 Narrowed tokens

A token may carry `only: [permissions]` and `flows: Selector`: it can do the intersection of that
and its principal's permissions. For a CI token that only deploys `payments/*`.

### 5.5 Selecting flows

`Selector` is a flow's slug, or a tag. A flow gets tags in its text:
`flow("payout", { port: 8153, tags: ["payments"] })`. Tags are part of the flow's manifest, so a
version that adds or removes a tag is a change a reviewer sees; a tag that widens who may act on
the flow (it joins a scope a role is bound at) is shown in the deploy review (§9).

Why tags and not path globs: flows have no folders, and a tag set by the flow's author, reviewed
at deploy, is the thing being governed. Slugs stay for one-off bindings.

## 6. Where access is kept, and how it changes

- **One document**, `access.json` beside `accounts.json` (principals and credentials stay in the
  accounts book): roles, groups, bindings, host settings (§8.4). Versioned like a deployment: every
  change makes a new version with who, when and why, kept for good.
- **The activity log** names each change ("ada bound payout-approver to bob on payments").
- **Changing access can need approval.** A host setting `access_changes: { people: 2 }`: a change
  to roles or bindings is kept as a proposal (the same `proposals.rs` shape as an agent's deploy)
  until that many people holding `people.assign` approve it, none of them its author. Off by
  default; on, it covers granting `admin` and anything touching `run.decide`.
- **No one locks the host out.** The last binding of `admin` to an enabled person can't be removed;
  the OS user's run-file admin always works.

## 7. The decision function

```rust
fn can(who: &Caller, action: Action, on: &Resource) -> Result<(), Refusal>;
```

Order of checks:

1. the principal is enabled, its token not expired;
2. the action is in the catalogue for the resource's type;
3. the token's narrowing (§5.4), if any, allows it;
4. some binding of the principal or one of its groups, at a scope covering `on`, gives a role that
   holds the action and whose `kinds` include the principal's kind;
5. the host settings' fixed rules (§8.4) don't forbid it.

`Refusal` says which of these failed, in the words used today: "a viewer only reads",
"refunds-bot may not deploy payouts: no role it holds there does". The 403 body carries it.

**Routes.** `Access::check` keeps authenticating (who is this), and stops authorizing by URL. Each
handler in `http.rs` (48 today) calls `can` with its action and resource, through a small table
`route → (action, resource from the path)` so the mapping is reviewable in one place and tested
(every route has an entry; a test fails on a route without one). Handlers that act on several
resources call `can` for each.

**Performance.** Bindings are few (tens to hundreds). `can` resolves a principal's effective
bindings once per request from an in-memory index rebuilt on each access version: a lookup, no I/O.

**The editor** asks `GET /auth/me/can?actions=…&flow=…` for what to show (buttons enabled,
Approve shown), and the server decides anyway.

## 8. Approvals in steps

### 8.1 In the language

```ts
const decision = await approval("payout", {
  steps: [
    { role: "classifier", kind: "agent" },
    { role: "reviewer" },
    { role: "approver" },
  ],
  order: "sequence",      // or "any"
  expires: "1d",
})
if (!decision.approved) return { declined: decision.reason ?? "expired" }
```

- `steps`: 1 to 5 slots. A slot is `{ role, kind?, people? }`:
  - `role`: a role name; the principal filling it must hold that role at a scope covering this
    approval point (§5.3);
  - `kind`: `person` (default) or `agent`. An agent slot is always written out; a slot without
    `kind` never accepts an agent or a service;
  - `people`: how many different principals fill it, 1 to 5 (1 by default).
- `order`: `sequence` (each slot opens when the one before is filled) or `any` (all open at once).
  Stages are written as nested lists: `steps: [[a, b], c]` is a and b in any order, then c.
- The short form stays: `await approval("payout", { people: 2 })` is one slot of two people, and a
  slot without `role` is filled by anyone holding `run.decide` on this flow (the permission, not a
  named role), which is today's rule.
- `expires` as today: 7 days by default, at most the host's `max_await_seconds`.

What the run receives:

```ts
{ approved: true,  by: [Decision, ...] }
{ approved: false, by: [Decision, ...], declined: Decision, reason: string }
{ approved: false, by: [Decision, ...], expired: true }
// Decision = { slot: number, who: string, kind: "person" | "agent", at: number,
//              reason?: string, note?: Value }
```

### 8.2 Lowering, capability and what the flow may do

- The `approval` leaf keeps its name; its params become `name`, `expires`, and `steps` (a canonical
  text of the slots and order). The `people` param goes.
- Its capability becomes `Approval(name, steps)` with the steps in canonical form, e.g.
  `Approval("payout", "seq[agent:classifier, person:reviewer, person:approver]")`.
- What the flow may do reads: "waits for approval (payout): an agent with classifier, then a person
  with reviewer, then a person with approver".
- **Weakening needs approval.** Any change to the steps changes the capability, so a version that
  drops a slot, lowers `people`, turns `sequence` into `any`, renames a role, or turns a person slot
  into an agent slot asks for a new permission and waits for a person (an agent's deploy becomes a
  proposal, as today). The deploy review shows the change as a changed row (§9).
- The resume-route refusal stays: no token, no `started.waiting.payout`; only the host delivers it.

### 8.3 Deciding

`POST /deployments/<slug>/runs/<run_id>/decide`

```json
{ "name": "payout", "approve": true, "reason": "…", "note": { "label": "refund", "confidence": 0.93 }, "slot": 1 }
```

`slot` is optional: when omitted, the host picks the open slot the caller can fill (the only one,
or the first in order); when the caller can fill two open slots (in `any` order), it must say which.

The host, under the lock that resumes the run:

1. finds the run waiting at `name`; else 409 with what happened ("resumed 12:03, approved by …");
2. picks the slot; it must be open (in `sequence`, all slots before it filled);
3. checks the caller may decide it:
   - `can(caller, run.decide, approval point)`;
   - holds the slot's role at a scope covering this flow (or the permission, for the short form);
   - is of the slot's kind;
4. checks separation of duties (§8.4);
5. records the decision on the parked run (`decisions: name → [Decision]`);
6. on a decline: the wait ends at once, `approved: false`, `declined` this decision. A decline
   needs a reason, a person's or an agent's alike;
7. on an approve: when the slot has `people` decisions it is filled; when every slot is filled the
   run resumes with `approved: true`;
8. answers `{ name, slots: [{role, kind, people, by: [...], open}], outcome }`, `outcome` one of
   `waiting`, `approved`, `declined`.

An agent's `note` is any JSON value up to 16 KiB, logged with its decision and given to the run in
`by[i].note`. A person's decision may carry a note too. The note is input to the run, recorded, so
replay and `holon verify` read it from the log like any event's body.

### 8.4 Separation of duties, and the host's fixed rules

Enforced by the host, not the flow:

- **One principal, one slot.** A principal fills at most one slot of an approval, and counts once
  within it. An admin who reviewed may not also approve, though admin holds everything.
- **Not the starter.** The principal whose request or action started the run may not fill any
  slot. (A run started by a schedule or a file has no starter.)
- **Not the deployer**, optional per host: `approvals.exclude_deployer: true` keeps whoever deployed
  the version the run is on from filling a slot.
- **People are people.** A slot without `kind: "agent"` is filled only by a `person`. Services never
  fill a slot.
- **Agents may be forbidden outright**: `approvals.agents: false` refuses any flow with an agent slot
  at deploy, for hosts that want none.

The host settings live in the access document (§6) and change like it.

### 8.5 Expiry

At `expires` the run resumes with `expired: true` and the decisions so far. Per-slot deadlines and
escalation (a slot unfilled for 4 h opens to a wider role) are a later step (§14).

### 8.6 The record

- Each decision is on the run's timeline as it happens ("refunds-bot classified: refund, 0.93";
  "ada reviewed"; "bob approved; resumed").
- The decisions are the event the run receives: kept in the parked run's `events`, so the resumed
  part's log entry carries them, and replay and `holon verify` treat them as logged input.
- The evidence export includes each decision with the slot, the principal, its kind, the role and
  binding (by id and access version) that allowed it. An auditor checks offline that it was an
  agent, then a different person holding reviewer, then another holding approver, in order, under
  the access version in force then.

### 8.7 Listing

`GET /deployments/<slug>/runs` gives a waiting point
`approval: { order, slots: [{ role, kind, people, by: [...], open }], expires_at }`. Runs waiting
(the Approvals page) reads it across flows.

## 9. At deploy

The deploy preview (`POST /deploy/preview`) adds, for each approval point:

- the steps, and against the version deployed now: same, changed (a changed row, to approve);
- **not ready** when a slot can't be filled: no principal of the slot's kind holds its role at a
  scope covering this flow, or fewer than `people` do once the starter is set aside ("approver: 1
  person holds it here; this needs 2");
- refused when `approvals.agents` is false and a slot is an agent's.

A flow whose tags (§5.5) bring it into a scope with bindings shows who gains access to it: "joins
payments: payments-team may view and call it, payout-approver decides its approvals".

## 10. Routes

| Route | Action | What |
|---|---|---|
| `GET /people` | `people.view` | principals, their kind, groups, bindings |
| `POST /people` | `people.manage` | make a person (invite link), an agent or a service |
| `PATCH /people/<name>` | `people.manage` | display, email, disable, owner |
| `POST /people/<name>/tokens` · `DELETE …/<id>` | `people.manage` (or owner) | a token, narrowed or not |
| `GET /roles` · `PUT /roles/<name>` · `DELETE` | `role.view` / `role.define` | custom roles |
| `GET /groups` · `PUT /groups/<name>` | `people.view` / `people.assign` | members |
| `GET /bindings` · `POST` · `DELETE /bindings/<id>` | `people.view` / `people.assign` | bindings |
| `GET /access/versions` · `GET /access?version=n` | `people.view` | access history |
| `GET /auth/me/can?actions=&flow=` | — | what the caller may do, for the editor |
| `POST /deployments/<slug>/runs/<id>/decide` | `run.decide` | §8.3 |

`/admin/accounts…` and `/admin/tokens…` go (replaced). A change to access that needs approval
(§6) answers 202 with its proposal.

## 11. CLI and `holon mcp`

```
holon people [list | add NAME --person|--agent|--service [--owner P] | disable NAME | token NAME [--only …] [--flows …]]
holon roles [list | define NAME --allow run.decide,run.view [--kinds person]]
holon groups [list | add GROUP NAME…]
holon bind ROLE (--to NAME | --group G) [--flows SEL | --approval FLOW:NAME] [--until DATE]
holon unbind ID
holon decide SLUG RUN NAME --approve|--decline [--reason …] [--slot N] [--note JSON]
```

- `holon decide` with an agent's token may fill an agent slot (the classifier's way in when it runs
  outside a flow); it is refused for a person slot, as the route is.
- `holon mcp`: `runs` lists approval points with their slots and who filled each. A new tool,
  `decide`, exists only for agent slots: it fills one when the agent holds the slot's role, and says
  where a person decides otherwise. `whoami` says the agent's name, owner and what it may do.

## 12. The editor

- **People** (step 16 of the editor plan, replaced by this): tabs People, Agents and tokens, Groups,
  Roles, Bindings, History. A principal's page shows its bindings and what they let it do, per flow.
  Granting is a form (who, role, where, until), and a change needing approval shows as waiting.
- **Approvals › Runs waiting**: each row shows its chain, `✓ classifier (refunds-bot: refund, 0.93)
  → ◻ reviewer → ◻ approver`, expiry, and Approve / Decline only for the caller who can fill an open
  slot; "you reviewed" where they already did; the reason dialog (`Dialog.tsx`), required to
  decline.
- **The Review page and deploy consent**: the approval point's steps as a row of what the flow may
  do, changed rows when they change, not ready when a slot can't be filled (§9).
- **The run's timeline** and the evidence view show each decision with its slot and role.
- **Buttons follow `can`**: disabled with the refusal as their title, rather than failing on click.

## 13. Samples and tests

- `approved-payout` moves to the three-slot chain, tagged `payments`: an agent classifies (through
  `holon decide` or the MCP tool), an editor reviews, an admin approves. Run live once with three
  principals: the order refused out of turn, the same principal refused a second slot, the starter
  refused, then paid; a second run declined by the reviewer with a reason.
- `approval` (the machine event) stays as it is.
- Unit tests: `can` for every built-in role reproduces today's behaviour (a table test over all
  routes before and after); scopes, groups, kinds, narrowed tokens, expired bindings; the access
  version history; separation of duties; `sequence`, `any` and staged order; canonical step text
  and the capability change on any weakening.
- e2e: the editor's Runs waiting with three accounts (one an agent) through the real server; a
  viewer sees no buttons; an agent's person-slot decide gets 403; the deploy review's not-ready row.
- The manual's leaves are blessed for the changed `approval` leaf; wasm rebuilt.

## 14. Steps

One commit each; the first two change no behaviour.

1. **Principals.** `kind`, owner; tokens belong to principals; agents as accounts; `holon mcp`
   signs in as an agent; `X-Holon-Role` removed. Tests: agents act under their own name.
2. **`can` and the catalogue.** Built-in roles as data; the route table; every handler asks `can`;
   `Access::check` only authenticates. Test: today's behaviour, route by route, unchanged.
3. **Custom roles, groups, bindings, scopes, tags.** The access document and its versions; routes;
   `holon people / roles / groups / bind`; `/auth/me/can`.
4. **Approvals in steps (language and core).** Slots, order, the canonical steps text, the
   capability; the short form. Bless the leaves; rebuild wasm.
5. **Approvals in steps (server).** Deciding by slot, separation of duties, host settings, notes,
   listing, timeline and evidence; `holon decide --slot --note`; the MCP `decide` tool for agent
   slots.
6. **Deploy review.** Steps against the deployed version, not ready, who gains access through tags.
7. **Editor.** People pages; Runs waiting chain; review rows; buttons from `can`.
8. **Governed access changes.** `access_changes: { people: N }` through proposals.
9. **The sample and docs.** `approved-payout` on the chain, run live; the manual's access and
   approvals pages; the editor guide.

Later, if asked for: admin-written deny rules; conditions on attributes (amount, time of day, a
run's input) in the style of AWS Cedar; per-slot deadlines and escalation; delegation; withdrawing a
decision before the count is reached; nested groups; SCIM; notifications.

## 15. Open questions

1. **Tags or folders** for selecting flows: tags in the flow's text (reviewed at deploy) are
   proposed; folders on the flows page would be set by whoever manages the host instead.
2. **The short form's meaning**: `people: 2` with no role means "anyone holding `run.decide` here";
   should it rather require a role, so every approval names who?
3. **Agent notes as decisions**: an agent slot always approves or declines; should an agent be able
   to only annotate (a classifier that never blocks), as a slot of kind `note` that can't decline?
4. **Editors and `run.decide`**: built-in `editor` keeps deciding (today's rule). With custom roles,
   should it lose it, so deciding is only ever granted on purpose?
5. **Where a decision is made from**: only the editor, CLI and MCP; or also a signed link sent by
   the flow (email, Slack), which then needs a short-lived, single-use, principal-bound token.
