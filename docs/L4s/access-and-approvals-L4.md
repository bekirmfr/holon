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
- **Ways in.** Whoever made the link a person's password was last set from (an invite, a reset),
  or set the email a provider matches them by, could act as them: who used a link can't be told.
  So could an agent's or a service's owner, and whoever else made a token of it. The accounts book
  keeps who made each (`ways_in`), and `GET /people` says it.
- **The machine is the root, and no account is.** Whoever holds the server's machine holds the
  data folder (the accounts, the access document, the secrets key), and its run file is admin on
  the host. A root account would be one person who could take over anyone, alone, and the best
  account to steal; it would add power without taking any from the machine.

## 4. Permissions: the catalogue

A permission is an action on a resource type. The catalogue is fixed by the host (code); admins
compose roles from it, never invent actions.

| Resource | Actions |
|---|---|
| `flow` (saved and deployed) | `view`, `edit` (save), `propose`, `deploy`, `approve-deploy` (acknowledge what needs approval), `restore`, `restore-dry-run`, `lifecycle` (run, pause, stop), `delete` |
| `run` | `view` (the list: when each ran, what started it, its status, how long, who by), `steps` (a run's steps and values: its timeline, request, answer, what it read and sent, a waiting run's input), `call` (send a request, run an action), `decide` (fill an approval slot), `export` (evidence) |
| `state` | `view`, `write` (by hand, a cell's history) |
| `secret` | `list` (names, history), `set`, `roll-back` |
| `resource` (folders, databases, hosts, keys) | `view`, `bind`, `unbind` |
| `people` | `view`, `manage` (principals, tokens), `assign` (bindings) |
| `role` | `view`, `define` |
| `policy` | `view`, `edit` (host limits, ceilings) |
| `host` | `view` (health, disks, store), `check` (read the run logs again for what needs attention), `operate` (sweep, settings) |

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

Built in, not deletable, their permissions fixed to reproduce today (`can.rs`'s `BUILTIN`, which a
test holds to this table):

| Role | Permissions | Kinds |
|---|---|---|
| `viewer` | `flow.view`, `run.view`, `run.steps`, `run.export`, `state.view`, `secret.list`, `policy.view` | any |
| `editor` | viewer's, and `flow.edit`, `flow.propose`, `flow.deploy`, `flow.approve-deploy`, `flow.restore`, `flow.restore-dry-run`, `flow.lifecycle`, `flow.delete`, `run.call`, `run.decide`, `state.write`, `policy.edit`, `host.check` | person, service |
| `admin` | everything | person |
| `agent` | `flow.view`, `run.view`, `run.export`, `state.view`, `secret.list`, `policy.view` (a viewer's but run.steps), and `flow.edit` (its own), `flow.propose`, `flow.restore-dry-run`, `run.call` | agent |

None but `admin` holds `secret.set`, `secret.roll-back`, `resource.*`, `people.*`, `role.*`,
`host.view` or `host.operate`: a refusal of one of those says "that needs an admin".

*Done (the audit register's R55):* reading a run's steps and values is `run.steps`, which the
agent role doesn't hold: an agent reads a flow's runs' steps (`explain`, a run's detail, `export`,
`replay`, what a waiting run shows) where a person bound it a role that holds them there, the
viewer role on the flow or a tag. The routes that answer them ask it, and refuse whoever doesn't
hold it, naming the command that gives it; none answers a thinner run. The list of runs
(`GET /deployments/<slug>/log/heads`: each run's number, when, what started it, its status, how
long, who by, where it waits, never its values) stays `run.view`'s. Which kinds a role's
`run.steps` reaches is one line of `BUILTIN`. What a flow writes with `console.log` keeps its own
grant, the deploy setting "agents may read its logs": a person gives each, the first by binding a
role, the second with the flow's version. A run's status, how long it took and whether it waits
stay in the list, and can tell which way a choice went (the secrets capability's limits). Not
yet: a proposal's review offering to give the agent that proposed it the flow's runs' steps.

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
- **Changing access can need approval.** A host setting `access_changes: { people: 2 }`, kept as
  `approvals.changes: 2` beside the approval settings of §8.4 (`holon access approvals --changes
  2`, 1 to 5, or `off`): a change that gives someone `run.decide` or anything of an admin's (a
  binding, a member of a group bound to one, a role changed to hold one, a role set on the host),
  or that loosens the approval settings, is kept as a proposal until that many people approve it,
  none of them its author, each a person holding `people.assign` on the host and what the change
  gives (`access_proposals.rs`). Off by default. Taking away applies at once, as does a change
  through the run file to the settings (below). Proposals are a file of their own,
  `access.proposals.json` beside `access.json`, not versions of the document; one made is a
  version by its author, its why naming who approved it.
- **No one takes over someone whose access waits.** While `changes` is N, no account makes a way
  in to someone else who holds what giving waits for: their password reset, their email set, a
  token of theirs made but by its owner, another owner given them, or the identity provider set
  while another person holds any of it (403). Whoever did could act as them, a second person where
  people are counted, while gaining nothing D19 would stop. They do it themselves, or the run
  file does; or what they hold is taken away (at once) and given back (waits). A proposal says
  whom it gives more and who could act as them; none of those approves it, and a way in made to one
  of them since makes it stale. With `changes` off, an admin already gives anything alone, and
  this holds no more than that.
- **No one locks the host out.** The last binding of `admin` to an enabled person can't be removed;
  the OS user's run-file admin always works: a change to the approval settings through it is made
  at once, so a host whose approvers are gone turns `changes` down there.

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

**Routes.** The gate (`access.rs`) authenticates (who is this) and no longer authorizes by URL.
Every route the control plane answers (108 today) has a row in one table, `can.rs`'s `ROUTES`: open
to anyone, for anyone signed in, or an action and the resource its path names, which the gate asks
`can` about. So the mapping is reviewable in one place and tested, and a path no row has is
answered 404 once its caller is known: a new route without a row is never reached. A handler that
learns more (whose saved flow it is, whether the flow lets agents run an action) calls `can` again;
one that acts on several resources calls it for each.

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

- `steps`: 1 to 5 slots. A slot is `{ role, kind?, people?, starter? }`:
  - `role`: a role name; the principal filling it must hold that role at a scope covering this
    approval point (§5.3);
  - `kind`: `person` (default) or `agent`. An agent slot is always written out; a slot without
    `kind` never accepts an agent or a service;
  - `people`: how many different principals fill it, 1 to 5 (1 by default);
  - `starter: true`: whoever started the run may fill it, a person (§8.4). Left out, they may not.
    Only `true` is written: an agent's slot never says it.
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
- A slot that lets the starter in is marked `^` in the canonical text (`agent:drafter>person:
  editor^`, `person*2^`), a character no role name holds, and said "…, who may be whoever started
  the run": letting them in, or no longer, is a changed row like any change to who decides.
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
- **Not the starter, but where a step says so.** The principal whose request or action started
  the run fills no slot but one that says `starter: true`, and then only a person: an agent never
  decides a run it started, whatever the step says. (A run started by a schedule or a file has no
  starter.) Strict by default: a payout is a request check, whose asker never approves it; a
  draft an agent wrote for whoever asked is an output review, whose asker is its natural
  reviewer, and the flow says so on that step. One flow may hold both, so neither a host switch
  nor a deploy setting is fine enough: the step says it, and the version's review shows it.
- **The host's floor**: `approvals.starter: false` (`holon access approvals --starter off`) lets no
  step take whoever started the run, whatever the flow says: a version with a `starter` step is
  refused at deploy, and a run already waiting goes by it at once. On by default.
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

*Done (the launch review's H6):* a decider sees what they approve. An approval point lists `shows`,
what its flow named (`await approval("payout", { shows: { amount, payee } })`, the leaf's optional
`shows` input), or else `input`, what the run was started with (an action's argument, a request
without its headers, what a source delivered; nothing for a clock). Either is kept at park as the
run's log keeps it: redacted by the log's redactor (a tainted field `‹taint:card›`), withheld whole
when a secret may have made it, its size past 16 KiB, sealed on disk with the run. The route takes
`run.view` on the flow or `run.decide` on its approvals: one who only decides sees the runs waiting
at an approval they may decide, those points alone, nothing delivered, and no other run.

## 9. At deploy

The deploy preview (`POST /deploy/preview`) adds, for each approval point:

- the steps, and against the version deployed now: same, changed (a changed row, to approve);
- **not ready** when a slot can't be filled: no principal of the slot's kind holds its role at a
  scope covering this flow, or fewer than `people` do once the starter is set aside from each slot
  that doesn't take them ("approver: 1 person holds it here; this needs 2");
- refused when `approvals.agents` is false and a slot is an agent's, or `approvals.starter` is
  false and a slot says `starter: true`.

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
| `PUT /access/approvals` | `people.assign` | the host's approval settings (§8.4, §6) |
| `GET /access/proposals` | `people.view` | access changes waiting for people (§6) |
| `POST /access/proposals/<id>/approve` · `…/decline` | `people.assign`, a person holding what it gives | approve, decline (`{reason}`) |
| `GET /auth/me/can?actions=&flow=` | — | what the caller may do, for the editor |
| `GET /deployments/<slug>/runs` | `run.view`, or `run.decide` on its approvals; what a point shows, `run.steps` or deciding it | §8.7 |
| `GET /deployments/<slug>/log` · `…/log/heads` | `run.steps` / `run.view` | a run's steps and values / the list of runs (R55) |
| `GET /deployments/<slug>/export` · `…/replay` | `run.steps` and `run.export` | evidence, made of a run's steps |
| `GET /deployments/<slug>/history` | `run.steps` | the last clock ticks, each with its outputs and effects (R56) |
| `GET /deployments/<slug>/state` · `…/changes` · `…/sessions` | `state.view` | its state; a cell's values before and after each run that changed it; each session with the state it ended with (R56) |
| `POST /deployments/<slug>/tick` | `flow.lifecycle`; its outputs and effects, `run.steps` | a clock ticks now; without `run.steps`, when and which, and what was left out (R57) |
| `POST /deployments/<slug>/restore` | `flow.restore-dry-run` (a restore, `flow.restore`); with `"state": true`, `state.view` too | a state put back, or what it would change (R57) |
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
- **Approvals › Access changes**: each access change that waits for people, what it gives, who
  could act as whom it gives more, Approve / Decline (its author's Withdraw), as `can` lets them.
- **Approvals › Runs waiting**: each row shows its chain, `✓ classifier (refunds-bot: refund, 0.93)
  → ◻ reviewer → ◻ approver`, expiry, and Approve / Decline only for the caller who can fill an open
  slot; "you reviewed" where they already did; the reason dialog (`Dialog.tsx`), required to
  decline.
- **The Review page and deploy consent**: the approval point's steps as a row of what the flow may
  do, changed rows when they change, not ready when a slot can't be filled (§9).
- **The run's timeline** and the evidence view show each decision with its slot and role.
- **Buttons follow `can`**: disabled with the refusal as their title, rather than failing on click.

*Done:* Runs waiting's chain, across flows through `GET /runs/waiting` (7a); People's tabs, the
form, and each principal's page saying what they may do (7b); buttons from `can` (7c); the
review's not-ready rows, deploying asked again (6, 7d); each decision on the run's page and on
hover in Runs waiting, with the binding and the access version that let it (5, 7a); Access
changes, and a change made in People that waits saying so, linked to them (8d, 9). The editor
guide says each ([The web editor](../manual/guides/editor.md#the-hosts-pages)).

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

*Done:* `approved-payout` is the three-step chain, tagged `payments` (5, 6), its walkthrough
binding the roles from the command line or the editor's form, and, with `--changes 1`, a binding
waiting for another admin (9); run live with three principals in step 9, as its commit says. The
refusals are held as classes, not per sample: `durable.rs`'s decide table (out of turn, one
principal a step, the starter, a step's kind, the host's settings) and the server's tests in
`tests.rs`; the sample's own test keeps the deposits made while it waits. End to end: Runs
waiting with an agent, a reviewer and an admin (`system.e2e.ts`, `approvals.e2e.ts`); buttons
that follow `can`, the deploy review's not-ready rows, and access changes that wait
(`access.e2e.ts`).

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
   *Done:* an approval's row is new, gone or changed against the version deployed (its
   capability names its steps); one whose steps can't all be taken here is not ready, saying
   why (`deploy::approvals_unready`: a step's holders of its kind, where the approval is as the
   version's tags would make it, one principal a step; whoever deploys it set aside where
   `exclude_deployer` says so, and, for an approval in an action, each who may run the action,
   as its starter); a tag's row says who it lets in (step 3d). With it, the launch review's H1
   (a flow call is deployed only by someone who may run the action it calls) and H9 (an agent's
   change to what a flow sends out waits for a person).
7. **Editor.** People pages; Runs waiting chain; review rows; buttons from `can`. *Done (7a to
   7d).*
8. **Governed access changes.** `access_changes: { people: N }` through proposals. *Done
   (server):* `approvals.changes` (8b); `access_proposals.rs` (8c): what a change gives is read by
   making it on a copy of the accounts (`Accounts::trial`) and comparing what each principal may
   do before and after, for as long, through `can`; a route's change that gives more waits (202),
   approved by people holding what it gives, made at the last approval, stale if it no longer
   applies; no one makes a way in to someone else holding what waits, and whoever could act as
   whom a change gives more doesn't approve it (8e). *Done (8d):* `holon access proposals`,
   `approve` and `decline`; Approvals › Access changes in the editor, History's waiting changes,
   and a change made in People and access that waits says so; `holon mcp` says an agent never
   approves one.
9. **The sample and docs.** `approved-payout` on the chain, run live; the manual's access and
   approvals pages; the editor guide. *Done (9):* the sample's walkthrough binds its roles in the
   editor's form and shows a binding waiting for another admin; the people guide maps People and
   access's tabs; the editor guide says what in People waits and where it is decided; the server
   reference lists and decides waiting runs (`GET /runs/waiting`, `POST …/runs/<id>/decide`).

Later, if asked for: admin-written deny rules; conditions on attributes (amount, time of day, a
run's input) in the style of AWS Cedar; per-slot deadlines and escalation; delegation; withdrawing a
decision before the count is reached; nested groups; SCIM; notifications.

## 15. Open questions

1. **Tags or folders** for selecting flows: tags in the flow's text (reviewed at deploy) are
   proposed; folders on the flows page would be set by whoever manages the host instead.
   *Settled: tags (3d).* A version that adds one asks for it (`Tag("payments")`), its review says
   who the tag lets in, and only a host admin tags a flow its deployer holds nothing on or takes
   a tag off.
2. **The short form's meaning**: `people: 2` with no role means "anyone holding `run.decide` here";
   should it rather require a role, so every approval names who?
   *Settled: it stays (4).* `people: N` is one step with no role: N people holding `run.decide`
   where the approval is, never its starter. The deploy review marks it not ready when fewer
   than N may take it, saying how many do (6), so it never means "whoever" unseen.
3. **Agent notes as decisions**: an agent slot always approves or declines; should an agent be able
   to only annotate (a classifier that never blocks), as a slot of kind `note` that can't decline?
   *Open.* An agent's step approves or declines, with a note the run is given (5).
4. **Editors and `run.decide`**: built-in `editor` keeps deciding (today's rule). With custom roles,
   should it lose it, so deciding is only ever granted on purpose?
   *Open.* `editor` keeps it (2); a host that wants deciding granted on purpose binds a role of
   its own instead (3), and `--changes N` makes giving it wait for people (8).
5. **Where a decision is made from**: only the editor, CLI and MCP; or also a signed link sent by
   the flow (email, Slack), which then needs a short-lived, single-use, principal-bound token.
   *Open.* The editor and the command line (`holon decide`), and an agent's step through `holon
   mcp`.
