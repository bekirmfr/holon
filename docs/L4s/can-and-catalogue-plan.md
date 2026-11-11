# `can` and the catalogue: implementation plan

**Companion to** `access-and-approvals-L4.md`, the design, and `principals-plan.md`, its step 1.
This file says how to build the design's §14 step 2: the catalogue of permissions (§4), the
built-in roles as data (§5.1), one decision function `can` (§7) and the route table that calls
it, with `Access::check` left only authenticating. Step 2 changes no behaviour (§14). The single
exception is the answer to a path that no route has, which is D5.

Each sub-step names the files, functions and lines it changes at `2839ba4b`. It sketches the new
code, lists the tests it changes and adds, and gives the checks to run. Code here is a sketch of
shape and names, not final code. Where the design leaves a decision open, or the code raises one
the design doesn't, the sub-step names it (§D).

## 0. Conventions

The conventions of `principals-plan.md` §0 hold, with these additions:

- **Two sub-steps, one commit each.**
  - 2a pins today: a table of every route against every kind of caller, with today's answers.
  - 2b moves the decisions behind that table into `can`, and the table test passes unchanged,
    except for the rows D5 names.
  - A behaviour that §D recommends changing lands after 2b, as a commit of its own. That commit
    flips the rows it changes in 2a's table and names them.
- **Refusals keep today's words.** The table compares the status and the whole body of every
  refusal.
- **Checks**:
  - both sub-steps: `cargo test --workspace` and
    `cargo clippy --workspace --all-targets -- -D warnings`;
  - 2b also changes the gate that every request from the editor and `holon mcp` passes, so it
    also runs `npm run e2e` locally with `HOLON_E2E_SLOT=3`, and its pull request gets the `e2e`
    label;
  - neither sub-step touches `editor/`, `holon-core` or the catalog of leaves, so neither needs
    tsc, vitest, wasm or blessing the manual.
- **Run live (2b)** against `target/release/holon.exe` on scratch port 7530, with the editor and
  `holon mcp` from Claude Code:
  - the agent proposes a flow, and a person approves it;
  - the agent is refused stopping it, and refused deciding a run waiting at an approval;
  - a viewer is refused deploying;
  - a service with the editor role deploys with `--approve`;
  - the commit says this was run.
- **No sample flow.** Step 2 changes who may act, not what a flow does. The commit says why.

| Sub-step | Change | Where | Decisions |
|---|---|---|---|
| 2a | The table test: every route against every kind of caller, today's status and words; the conditions that aren't roles, against each kind of caller | `tests/access_table.rs` (new), `tests.rs` | — |
| 2b | The catalogue, the built-in roles, `can`, the route table; `Access::check` only authenticates; `agent: bool` becomes `can` | `can.rs` (new), `access.rs`, `http.rs`, `auth.rs`, `deploy.rs`, `flows.rs`, `proposals.rs`, `logs.rs`, `durable.rs`, tests, docs | D1–D5 |
| after | Behaviour changes chosen from §D, each in its own commit | as each says | D6–D11 |

## 1. What decides today, at `2839ba4b`

All paths are under `crates/holon/src/`.

- **The gate, `access.rs`.**
  - `Access::check` (:126-159):
    1. A server without accounts lets everyone in, as `Caller::default()` (:129).
    2. `is_open` (:73-77) lets anyone in, signed in or not.
    3. The run file's token (`accounts.local`, `accounts.rs:645-649`: a person, admin), then
       `signed_in` (:162-195). Before setup, `signed_in` accepts only agents and services; a
       disabled principal gets 401.
    4. The role match (:139-147):
       - admin: anything;
       - anyone: `is_own` (:96-98);
       - a person who is a viewer or an editor: `is_tokens_of_someone` (:102-109);
       - nobody else: `is_admins` (:112-114), meaning `/admin/…` and `/people…`;
       - viewer: GET;
       - editor: all but `is_secret_write` (:214-218);
       - agent: `agent_may` (:80-93), meaning GET, and POST to `/unit`, `/deploy/preview`,
         `/deploy`, `/flows`, `/deployments/<slug>/request`, `/trigger/<slug>/<name>` and
         `/deployments/<slug>/restore`.
    5. The words (:148-156): "that needs an admin" for `is_admins` or a secret write; for an
       agent, "an agent proposes and calls; approving, granting, starting, stopping and deleting
       are a person's"; otherwise "a viewer only reads". Each is `"<name> may not do that: <words>"`,
       a 403.
- **The routers.**
  - `http.rs` `route` (:142-430):
    - the editor's files are served first, open (:151-156);
    - then the caller (`reject_untrusted`, :158);
    - then `auth::route` (:171);
    - then an if-chain of 60 method-and-path pairs.
  - `auth.rs` `route` (:57-335) takes `/auth/…`, `/admin/…` and `/people…` (:59). It hands
    `/admin/…` paths in turn to each module's `route`:
    - `folders.rs:1253`, `storage.rs:1236`, `sweep.rs:433`, `databases.rs:1288`, `mails.rs:863`,
      `feeds.rs:826`, `queues.rs:773`;
    - then `/admin/grants` (:90), `/admin/secrets/env` (:95) and `/admin/secrets…` (:101-106);
    - then its own routes (:114-334).
- **Decisions after the gate.** `http.rs:163` sets `let agent = caller.role == Some(Role::Agent)`.
  - It is passed as `agent: bool` to:
    - `handle_post_deploy` (:219), then `deploy.rs:125`, then `deploy` (:147):
      - an agent's `acknowledge` is refused (:150-152);
      - "agents may propose" is off (:156-158);
      - `approval_needed` (:91-116, called at :245) withholds secrets and unrecorded handlers
        whose code an agent changed;
      - the browsers an agent adds need approving (:246-248);
      - an agent proposes instead of deploying (:252-…);
      - an agent doesn't start a stopped or paused deployment (:327-333);
      - only a person's deploy approves a proposal (:434-436).
    - `handle_post_flows` (:226), then `flows.rs:8-36` and `save` (:42-68): an agent replaces
      only a flow it saved (:54-60). `proposals.rs:188-207` saves an agent's proposed flow
      through `save` too.
    - `handle_post_deploy_preview` (:270), then `deploy.rs:484-541`: what an agent's deploy
      would need.
    - `handle_post_restore` (:365), then `deploy.rs:1260`: an agent's restore that isn't a dry
      run is saved as an ask (:1282-1284).
    - `handle_post_action` (:424), then `deploy.rs:918`, `run_action` (:981): an agent runs an
      action only with `AgentRuns("<name>")` in the grant (:1003-1006).
  - `logs.rs:365-370`: the role agent reads a deployment's log lines only with
    `AgentReadsLogs`.
  - `durable.rs:887-899` (`decider`):
    - a viewer is refused;
    - no name means no accounts, so it is refused;
    - an agent is refused, and so is a service;
    - otherwise the decider is the person, `local:` stripped.
  - `auth.rs:257-259`: someone's tokens are an admin's, or the owner's.
- **Tests that pin parts of this.**
  - `access.rs` tests (:221-…): `each_role_does_what_it_may` (:252),
    `people_are_an_admins_and_tokens_their_owners_too` (:292) and
    `an_agent_proposes_and_calls_and_does_nothing_else` (:308).
  - `tests.rs:4905` `the_gate_and_the_router_read_every_path_alike`: 19 routes, 5 callers,
    every spelling of each path.
  - `tests.rs:4974` `what_one_owns_is_listed_and_its_tokens_are_theirs_to_make`.
  - `mcp.rs:1625-1637`: what an agent is refused.

## 2. The routes

This is the route table 2b writes, and the rows 2a's test pins. There are 112 rows; the design
counts 48 (§7). "Today" says who gets past every check:

- **open**: anyone, signed in or not;
- **signed in**: any principal signed in;
- **all**: every role;
- **E**: editor and admin;
- **E+G**: editor, admin and the agent role;
- **A**: admin.

Each "On" value names a `Resource` (§2b): **Host**, **Flows** (every flow at once, or one that
isn't named yet), **Flow** (the path's `<slug>` or `<name>`), **Principal**. The arrow (→) names
the check a handler adds once it knows more.

**Open, and one's own** (`access.rs:73-77`, :96-98; `auth.rs:115-197`; `http.rs:166`, :175, :180)

| Route | Today |
|---|---|
| `OPTIONS *`; `GET /health`, `/auth/state`, `/auth/config`, `/auth/invite`; `POST /auth/setup`, `/auth/sign-in`, `/auth/invite` | open |
| `GET /auth/me`, `/auth/owned`, `/auth/prefs`, `/version`; `POST /auth/password`, `/auth/prefs`, `/auth/sign-out` | signed in |

**Flows and deployments**

| Route | Where | Today | Action | On |
|---|---|---|---|---|
| `GET /flows` | `http.rs:228` | all | flow.view | Flows |
| `POST /flows` | :225 | E+G | flow.edit | Flows → the saved flow (`flows.rs:54`) |
| `GET /flows/<name>` | :234 | all | flow.view | Flow |
| `DELETE /flows/<name>` | :237 | E | flow.delete | Flow |
| `GET /unit/<hash>` | :184 | all | flow.view | Flows |
| `POST /unit` | :189 | E+G | flow.propose | Flows |
| `POST /deploy/preview` | :269 | E+G | flow.propose | Flows |
| `POST /deploy` | :218 | E+G | flow.propose | Flows → approve-deploy, lifecycle on the flow (`deploy.rs`) |
| `GET /proposals`, `GET /proposals/<id>` | :243, :252 | all | flow.view | Flows |
| `POST /proposals/<id>/decline` | :247 | E | flow.approve-deploy | Flows |
| `GET /deployments` | :281 | all | flow.view | Flows |
| `GET /deployments/<slug>/document`, `/versions`, `/events`, `/changes`, `/history`, `/manifest`, `/grant` | :347, :355, :351, :322, :327, :369 | all | flow.view | Flow |
| `POST /deployments/<slug>/grant` | :375 | E | flow.approve-deploy | Flow |
| `POST /deployments/<slug>/settings` | :399 | E | flow.deploy | Flow |
| `POST /deployments/<slug>/run`, `/pause`, `/stop` | :382 | E | flow.lifecycle | Flow |
| `POST /deployments/<slug>/tick` | :403 | E | flow.lifecycle | Flow |
| `POST /deployments/<slug>/restore` | :363 | E+G | flow.restore-dry-run | Flow → flow.restore unless a dry run (`deploy.rs:1282`) |
| `DELETE /deployments/<slug>` | :407 | E | flow.delete | Flow |
| `GET /registry`, `GET /cache/<key>` | :197, :209 | all | flow.view | Flows |
| `POST /registry/publish`, `/registry/fork`, `/fire`, `/cache/ingest` | :193, :201, :214, :205 | E | flow.deploy (D4) | Flows |
| `POST /trigger/<slug>` (the hint naming an action, `deploy.rs:904`) | :425 | E | flow.deploy (D4) | Flow |

**Runs and state**

| Route | Where | Today | Action | On |
|---|---|---|---|---|
| `GET /deployments/<slug>/runs`, `/log`, `/sessions` | :297, :335, :378 | all | run.view | Flow |
| `GET /deployments/<slug>/logs` | :302 | all; the role agent with `AgentReadsLogs` | run.view | Flow → its logs (`logs.rs:365`) |
| `GET /deployments/<slug>/export`, `/replay` | :339, :343 | all | run.export (D8) | Flow |
| `POST /deployments/<slug>/request` | :395 | E+G | run.call | Flow |
| `POST /trigger/<slug>/<name>` | :420 | E+G; the role agent with `AgentRuns(name)` | run.call | Flow → its action (`deploy.rs:1004`) |
| `POST /deployments/<slug>/runs/<id>/decide` | :317 | E, people only | run.decide | Flow → the approval (`durable.rs:887`) |
| `GET /deployments/<slug>/state`, `/collections/<name>`, `/holds`, `/feeds`, `/queues` | :331, :292, :359, :307, :312 | all | state.view | Flow |
| `GET /summary` | :258 | all | run.view | Flows |
| `GET /activity` | :265 | all | flow.view | Flows |

**Secrets, the policy and the host**

| Route | Where | Today | Action | On |
|---|---|---|---|---|
| `GET /deployments/<slug>/secrets`, `/secrets/<name>` | :286, :289 | all | secret.list | Flow |
| `POST`, `DELETE /deployments/<slug>/secrets/<name>` | `deploy.rs:677`, :732 | A | secret.set | Flow |
| `POST /deployments/<slug>/secrets/<name>/rollback` | `deploy.rs:651` | A | secret.roll-back | Flow |
| `GET /admin/secrets/env` (before the next row) | `auth.rs:95` | A | resource.view (D4) | Host |
| `GET /admin/secrets`, `/admin/secrets/<name>` | `auth.rs:101-106` | A | resource.view (D4) | Host |
| `POST /admin/secrets/<name>`, `…/rollback`, `…/cap`; `DELETE /admin/secrets/<name>` | `deploy.rs:651-740` | A | resource.bind; unbind for DELETE (D4) | Host |
| `GET /permissions` | `http.rs:273` | all | policy.view | Host |
| `POST /permissions/reload` | :277 | E | policy.edit (D10) | Host |
| `POST /summary/check` | :261 | E | host.check (D4) | Host |
| `GET /admin/host`, `/admin/warnings` | `storage.rs:1238-1239` | A | host.view | Host |
| `GET /admin/grants` | `auth.rs:90` | A | host.view | Host |
| `POST /admin/storage` | `storage.rs:1240` | A | host.operate | Host |
| `POST /admin/store/sweep`, `/admin/folders/sweep` | `sweep.rs:433`, `folders.rs:1318` | A | host.operate | Host |
| `GET`, `POST /admin/folders`, `/admin/databases`, `/admin/mails`, `/admin/feeds`, `/admin/queues` | `folders.rs:1256`, `databases.rs:1292`, `mails.rs:867`, `feeds.rs:830`, `queues.rs:777` | A | resource.view; bind for POST | Host |
| `GET /admin/databases/<name>/tables` | `databases.rs:1350` | A | resource.view | Host |
| `DELETE /admin/folders/<name>`, `/admin/databases/<name>`, `/admin/mails/<name>`, `/admin/feeds/<name>`, `/admin/queues/<name>` | `folders.rs:1334`, `databases.rs:1360`, `mails.rs:906`, `feeds.rs:870`, `queues.rs:816` | A | resource.unbind | Host |

**People**

| Route | Where | Today | Action | On |
|---|---|---|---|---|
| `GET /people`, `GET /admin/oidc` | `auth.rs:200`, :236 | A | people.view | Host |
| `POST /people`, `PATCH`, `DELETE /people/<name>`, `POST /people/<name>/reset`, `POST /admin/oidc` | :201, :298, :292, :280, :237 | A | people.manage | Host |
| `GET`, `POST /people/<name>/tokens`, `DELETE /people/<name>/tokens/<id>` | :257-278 | A, or the person who owns it | people.manage | Principal (with its owner) |

### The built-in roles that reproduce this

| Role | Permissions | Kinds |
|---|---|---|
| `viewer` | flow.view, run.view, run.export, state.view, secret.list, policy.view | any |
| `agent` | viewer's, and flow.edit (its own), flow.propose, flow.restore-dry-run, run.call | agent |
| `editor` | viewer's, and flow.edit, flow.propose, flow.deploy, flow.approve-deploy, flow.restore, flow.restore-dry-run, flow.lifecycle, flow.delete, run.call, run.decide, state.write, policy.edit, host.check | person, service |
| `admin` | everything | person |

None but admin holds secret.set, secret.roll-back, resource.\*, people.\*, role.\*, host.view or
host.operate. That is today's "that needs an admin", said as data: a refusal says it when only
the admin role holds the action (§2b, the words).

The rules that aren't a role's:

- **Its own** (`flows.rs:54-60`). Under the agent role, flow.edit holds on a flow that isn't
  there yet, or that the agent saved, and on Flows: the gate passes, and the handler asks again
  once it knows the flow.
- **The flow lets agents** (`deploy.rs:1004`, `logs.rs:365`). For the role agent, run.call on an
  action needs `AgentRuns(name)`, and run.view on the log lines needs `AgentReadsLogs`. Today
  this is keyed on the role (D11).
- **People are people** (design §8.4; `durable.rs:891-898`). run.decide needs a person: not an
  agent, not a service, and not the open server's nameless caller.
- **The owner** (`auth.rs:257`). people.manage on a principal's tokens holds for the person who
  owns it, whatever their role.

## Step 2a. The table test

**Goal.** Pin today, route by route and caller by caller, before any code moves. The test's
expectations come from a small oracle written against §2, never from `access.rs`, so the same test
judges 2b.

### `tests/access_table.rs` (new), `mod access_table;` in `tests.rs`

It is a file of its own: other sessions edit `tests.rs`, and the table is long.

**Callers**, all on one server, plus the open server:

| Caller | Kind | Role |
|---|---|---|
| no one | — | — |
| cy | person | viewer |
| bob | person | editor |
| ada | person | admin |
| `local:bma` | the run file | admin |
| claude | agent (ada's) | agent |
| scout | agent (cy's) | viewer |
| ci | service (bob's) | editor |
| nightly | service (bob's) | viewer |
| off | service, disabled | editor |
| anyone on a server without accounts | — | — |

**Part 1: every route.**

- **The rows.** One row per §2 route, with a concrete path and body:
  - the targets don't exist (`/deployments/none/stop`, `/people/nobody`), or the body is one the
    handler refuses (`[]`), so a request let through changes nothing;
  - `POST /auth/sign-out` goes last for each caller;
  - after the sweep, the test asserts that the flows, the deployments and the principals are as
    they were.
- **The oracle.**
  - `Today` is one of: Open, SignedIn, All, Editors, EditorsAndAgents, Admins, Owners.
  - The expected answer for a caller comes from `Today`, the caller's kind and role, and the
    route's owner, in about 30 lines:
    - **refused**: the status and the whole body:
      - no one: 401 "sign in first";
      - `off`: 401 "your sign-in has ended…";
      - refused by a role: 403 with one of the gate's three phrasings;
      - not the owner, and a person: 403 with `auth.rs:258`'s words;
    - **let through**: any answer that isn't one of those refusals. For ada, it also isn't a
      bare 404, so the router knows every row. A row whose handler answers a bare 404 for a
      missing target, as `POST /trigger/<slug>` does (`deploy.rs:907`), uses a target that
      exists.
  - Every caller on the open server is let through.
- **Paths no route has.** `GET /nope`, `POST /nope`, `POST /admin/nope` and `DELETE /flows`, each
  with today's answers. D5 changes these rows in 2b.

**Part 2: the conditions.** One row per rule that isn't a role's, each on a real resource. The
expected status and words for each caller:

- An agent saving over a flow ada saved; over one it saved itself; a new one.
- `POST /deploy`:
  - with `acknowledge` (an agent is refused, `deploy.rs:150`);
  - needing approval without it (an agent gets 202, a proposal);
  - to a flow with "agents may propose" off;
  - to a stopped deployment (an agent is refused, `deploy.rs:331`);
  - a person's deploy of a proposed version approves it (`deploy.rs:434`).
- `POST /trigger/<slug>/reset`, with and without `AgentRuns("reset")`.
- `GET /deployments/<slug>/logs`, with and without `AgentReadsLogs`. scout, an agent holding the
  viewer role, reads them either way today (D11).
- `POST /deployments/<slug>/restore`, as a dry run and not: an agent's is saved as an ask (202).

Deciding an approval is in part 1: the decider is asked before anything is read, so a run that
isn't there is enough. Its row, `Deciders`, lets bob, ada and `local:bma` through, and refuses ci
in the service's words and the open server's caller in its words; the gate refuses the rest.

The owner rule is already swept by `what_one_owns_is_listed_and_its_tokens_are_theirs_to_make`.
What an agent's changed code withholds is already covered by `mcp.rs:1615-1619`. Part 2
cites both rather than repeating them.

### Checks

`cargo test --workspace` and clippy. The commit says the table was written from the code at
`2839ba4b` and passes against it.

## Step 2b. `can`, the catalogue and the route table

**Goal.** Who may do what is decided in one place, as data:

- the catalogue;
- the built-in roles;
- one function;
- a table of every route.

`Access::check` says who is asking, and nothing else. Every check after the gate asks `can`. The
table test passes unchanged, but for D5's rows.

### `can.rs` (new; `mod can;` in `main.rs`)

```rust
/// The catalogue (design §4): what may be done, to what. Fixed here; roles are made from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    FlowView, FlowEdit, FlowPropose, FlowDeploy, FlowApproveDeploy, FlowRestore, FlowRestoreDryRun, FlowLifecycle, FlowDelete,
    RunView, RunCall, RunDecide, RunExport,
    StateView, StateWrite,
    SecretList, SecretSet, SecretRollBack,
    ResourceView, ResourceBind, ResourceUnbind,
    PeopleView, PeopleManage, PeopleAssign,
    RoleView, RoleDefine,
    PolicyView, PolicyEdit,
    HostView, HostCheck, HostOperate,
}
impl Action {
    pub(crate) const ALL: [Action; 31] = [/* … */];
    /// `flow.approve-deploy`, as the design and the refusals write it.
    pub(crate) fn name(self) -> &'static str { /* … */ }
}

/// What an action is done to.
pub(crate) enum Resource<'a> {
    Host,
    /// Every flow at once (a list), or one not named yet (a save before its body is read).
    Flows,
    Flow(&'a str),
    /// A saved flow, and who saved it: None when there is none yet (flows.rs).
    Saved { name: &'a str, by: Option<&'a str> },
    /// A deployment's action, and whether its grant lets agents run it (`AgentRuns`).
    Action { slug: &'a str, agents_may: bool },
    /// A deployment's log lines, and whether its grant lets agents read them (`AgentReadsLogs`).
    Logs { slug: &'a str, agents_may: bool },
    /// A run waiting at an approval.
    Approval { slug: &'a str },
    /// A principal, and its owner.
    Principal { name: &'a str, owner: Option<&'a str> },
}

enum Holds { All, Do(Action), Own(Action) }

pub(crate) struct RoleDef { name: &'static str, about: &'static str, holds: &'static [Holds], kinds: &'static [Kind] }

/// The built-in roles (§2): today's, as data.
pub(crate) const BUILTIN: [RoleDef; 4] = [/* viewer, agent, editor, admin */];

pub(crate) struct Refusal { pub(crate) status: u16, pub(crate) said: String }

/// Whether `who` may do `action` on `on`, or why not (design §7).
pub(crate) fn can(who: &Caller, action: Action, on: &Resource) -> Result<(), Refusal>;
```

**The order of checks.** This is §7's order, as step 2 has it:

1. **Enabled.** Authentication already refused a disabled principal (401); `can` trusts the
   `Caller` it is given.
2. **The catalogue.** `action` is an `Action`. A debug assertion checks that `on` is a kind of
   resource that action takes.
3. **A token's narrowing.** None yet (design §5.4, step 3).
4. **The role.** The caller's role holds the action (`All`, `Do`, or `Own` on a `Saved` that is
   theirs or new, or on `Flows`), and the role's `kinds` include the caller's kind. A caller with
   no role is on a server without accounts, and holds everything. `Access::authenticate` makes
   such a caller only there, and a debug assertion says so.
5. **The fixed rules** (§2, "the rules that aren't a role's"):
   - a person decides;
   - the flow lets the role agent;
   - the owner may manage the tokens of what they own. This one passes where step 4 failed.

**The words.** A refusal by the role:

- "that needs an admin" when only the admin role holds the action;
- for the role agent, the agent's words;
- otherwise "a viewer only reads";
- each as `"<name> may not do that: …"`, 403, the gate's words.

A refusal by a fixed rule says the words its handler says today, moved here:

- `durable.rs:889-896`;
- `logs.rs:368`;
- `auth.rs:258`;
- the action's grant (`deploy.rs:1005`).

### The route table, in `can.rs`

```rust
pub(crate) enum Rule { Open, SignedIn, Can(Action, On) }
/// Where the resource comes from: the path's {slug} or {name}, or nothing in it.
pub(crate) enum On { Host, Flows, Slug, Principal }
pub(crate) struct Route { pub(crate) method: &'static str, pub(crate) path: &'static str, pub(crate) rule: Rule }

/// Every route, in the order the routers read them: the first row that matches is the route.
pub(crate) const ROUTES: &[Route] = &[
    Route { method: "GET", path: "/deployments/{slug}/runs", rule: Rule::Can(Action::RunView, On::Slug) },
    // … the 112 rows of §2
];

/// The route `method` and `url` ask for, and the path's parts.
pub(crate) fn find(method: &str, url: &str) -> Option<(&'static Route, Parts<'_>)>;
```

- **Patterns.** `{slug}`, `{name}`, `{id}`, `{hash}` and `{key}` each match one non-empty segment
  with no `/`. The query string is ignored. `OPTIONS` matches `*`.
- **Owners.** `On::Principal` reads the principal's owner from the accounts book at the gate.

### `http.rs` `route` (:142-173)

```rust
let route = can::find(method, &url);
let caller = match route {
    Some((r, _)) if matches!(r.rule, Rule::Open) => Caller::default(), // a stale token doesn't stop signing in
    _ => match reject_untrusted(request, state.port, &state.access) { Ok(c) => c, Err(r) => return r },
};
let Some((route, parts)) = route else { return Response::empty_404() }; // D5
if let Rule::Can(action, on) = route.rule
    && let Err(refusal) = can::can(&caller, action, &on.resource(&parts, state))
{
    return err(refusal.status, refusal.said);
}
```

- **Before the table.** The editor's files (:151-156) and the Host/Origin checks
  (`reject_untrusted`) stay ahead of it.
- **Unknown paths.** One no route has is authenticated first, so no one still gets 401. D5.
- **`by`.** It stays.
- **`agent`** (:163) goes. The five handlers it fed take `&caller` instead (:219, :226, :270,
  :365, :424).

### `access.rs`

- `check` becomes `authenticate(&self, request) -> Result<Caller, Response>`, which is today's
  :129 and :133-138, with the `Caller` built at :158.
- `is_open`, `agent_may`, `is_own`, `is_tokens_of_someone`, `is_admins`, `is_secret_write` and the
  role match go, and so do the module notes about them.
- `Role` and `Caller` stay. `Kind::may_hold` (`accounts.rs`) becomes a reading of `BUILTIN`'s
  `kinds`, so the two can't drift.

### Past the gate: `agent: bool` becomes `can`

- **`deploy.rs`.**
  - `handle_post_deploy`, `deploy`, `handle_post_deploy_preview`, `preview`, `preview_value`,
    `handle_post_restore`, `handle_post_action` and `run_action` take `who: &Caller` for
    `agent: bool`. `by` stays as it is.
  - In `deploy`:
    `let approves = can(who, Action::FlowApproveDeploy, &Resource::Flow(&body.slug)).is_ok();`.
    `!approves` replaces `agent` at :150, :156, :245 (`approval_needed`'s argument, renamed
    `proposes`), :246 and :252, and `approves` replaces `!agent` at :434.
  - :327: `can(who, Action::FlowLifecycle, …)` refused, and the deployment isn't running.
  - The preview (:541-542): `proposes`, as in `deploy`.
  - :1282: `can(who, Action::FlowRestore, …)` refused, and not a dry run.
  - :1004: `can(who, Action::RunCall, &Resource::Action { slug, agents_may })`.
  - Each refusal keeps its words where they explain the request ("deploy without
    `acknowledge`…"). The decision is `can`'s.
- **`flows.rs` `save`** (:42-60) takes `who` for `agent` and asks
  `can(who, FlowEdit, &Resource::Saved { name, by: saved.saved_by })`. `proposals.rs:188-207`
  passes the agent's `Caller`.
- **`logs.rs:365-370`** asks `can(caller, RunView, &Resource::Logs { slug, agents_may })`.
- **`durable.rs` `decider`** (:887-899) asks `can(caller, RunDecide, &Resource::Approval { slug })`
  and keeps answering the decider's name.
- **`auth.rs:257-259`** goes: the gate's `On::Principal` decides it. The comment at :199 and the
  module notes (:11-21) say so.

### Tests in 2b

- **2a's table** passes. Its D5 rows change, and the commit names them.
- **`the_gate_and_the_router_read_every_path_alike`** (`tests.rs:4905`) goes from its 19 kept
  routes to every `ROUTES` row that some caller may not take:
  - every spelling of the path, for every caller who may not, is refused or answered a bare 404.
    That the router knows every row is 2a's (ada's answers).
- **New, in `can.rs`:**
  - every row is reachable: no earlier row matches its canonical path;
  - the admin holds every `Action`;
  - `Action::name` matches the design's §4 names;
  - the `BUILTIN` kinds are those of `Kind::may_hold`, as the design's §5.1 says.
- **`access.rs` tests.**
  - `each_role_does_what_it_may`, `people_are_an_admins_and_tokens_their_owners_too` and
    `an_agent_proposes_and_calls_and_does_nothing_else` are rows of the table now, so they go
    (class-level tests, not one per route).
  - `a_new_server_answers_only_its_setup`, `a_service_acts_with_its_role` (disabled: 401),
    `a_role_header_changes_nothing`, `signed_in_through_a_provider_is_matched_by_email` and
    `unnamed_hosts_and_origins_are_refused` stay, as authentication tests.
- **`mcp.rs:1625-1637`** stays as it is: the agent's refusals through `holon mcp`.

### Docs in 2b

- **`access-and-approvals-L4.md`**:
  - §4 gains `host.check` (D4);
  - §5.1's table becomes §2's built-in roles (D3);
  - §7 says 112 routes, not 48, and says that a path no route has answers 404 (D5).
- **`principals-plan.md`**: "Step 2, for what step 1 leaves it" points here.
- **`docs/manual/reference/server.md`**: where it says what a role may, unchanged. Where it says
  what a path no route has answers, 404.
- **CHANGELOG**: who may do what is decided in one table; a path no route has answers 404 to
  anyone signed in.

## D. Decisions

These are for step 2 itself:

- **D1. Where the table sits.**
  - **Recommended**: in front of the routers as they are, matching every path once, with deny by
    default. A route added to a router without a row is unreachable: it answers 404, so its own
    test fails. The extended spellings test catches a row that the table and the router read
    apart.
  - Otherwise the table drives dispatch: one parse, each row naming its handler. That ends the
    question of the table and the router disagreeing, but it rewrites `http.rs` `route`,
    `auth.rs` `route` and the seven `/admin/` routers in the same commit as moving the
    decisions.
  - Recommended for step 2. Step 3 can make the table dispatch, since it adds routes anyway.
- **D2. The rules that aren't a role's** (its own, the flow lets agents, people are people, the
  owner).
  - **Recommended**: they go in `can`, as conditions on the resource, so one function answers,
    and step 3's `/auth/me/can` answers them too.
  - Otherwise they stay in their handlers, and `can` answers only roles.
- **D3. Today, or the design's §5.1.** The design says the built-in roles are "fixed to reproduce
  today", but its table doesn't (D6-D10).
  - **Recommended**: step 2 keeps today, and §5.1 is corrected to §2's roles in 2b's commit. Each
    difference is a decision of its own below, for a commit after 2b.
- **D4. Routes the catalogue doesn't place.** Each choice keeps today's answers:
  - **`POST /summary/check`.** Editors check the run logs again today. The design's place for
    it, host.operate, is the admin's, as the sweep is.
    - **Recommended**: §4 gains `host.check` ("check the logs again"), which the editor holds.
    - Otherwise it becomes an admin's (a change).
  - **The server's own secrets** (`/admin/secrets…`) are the admin's today, but `secret.list` is
    every viewer's, for a flow's secrets.
    - **Recommended**: they are a resource bound at the host (§4 lists "keys" with resources), so
      resource.view, resource.bind and resource.unbind.
  - **The registry, `/fire` and `/cache/ingest`**, the editor's and not the agent's: flow.deploy.
    Each runs or publishes code that a person hasn't approved as a deployment.
  - **`POST /trigger/<slug>`**, which answers a hint naming the action. Today the gate refuses an
    agent and a viewer; the router answers an editor.
    - **Recommended** for step 2: flow.deploy, which keeps exactly that.
    - Later, run.call, so an agent gets the hint too (`mcp.rs:1632`'s row changes then).
- **D5. A path no route has.**
  - Today:
    - an editor or an admin gets the router's 404;
    - a viewer's or an agent's write gets the gate's 403;
    - a non-admin gets 403 under `/admin/`.
  - **Recommended**: 404 for anyone signed in, after authenticating, so no one still gets 401.
    The table knows the routes; a path it doesn't know is not routed, and refusing it in the old
    URL rule's words would keep the rule step 2 removes. It is the one change in 2b, in three of
    the table's four unknown-path rows (`GET /nope` is a 404 already).
  - Otherwise 2b keeps a fallback that answers by method and prefix, as today.

These are differences between today and the design's §5.1, each for a commit after 2b:

- **D6. Viewers and the host's pages.** The design's "every `*.view`" would let a viewer read
  people, the host (`/admin/host`), its folders and databases, and the policy's grants. Today
  those are an admin's.
  - **Recommended**: keep today. §5.1 names the views a viewer holds (§2), and step 3's custom
    roles give the others to whoever needs them.
- **D7. Editors' `resource.view`.** The design gives it to editors; today an editor doesn't read
  `/admin/folders` and the rest.
  - **Recommended**: keep today, as D6.
- **D8. Evidence.** Today a viewer and an agent export runs and replay them
  (`GET …/export`, `…/replay`); the design gives `run.export` to editors only.
  - **Recommended**: keep today. An auditor holding the viewer role is who exports evidence to
    check offline, and the first outside user (payment ops) has auditors.
- **D9. Deleting others' flows.** The design's editor deletes only their own; today an editor
  deletes any flow or deployment.
  - **Recommended**: decide it in step 3, with custom roles and a flow's owner. Step 2 keeps
    today.
- **D10. Reloading the policy.** Today an editor may (`POST /permissions/reload`); it can pause
  every deployment whose grant no longer fits. In the design it is policy.edit, an admin's.
  - **Recommended**: keep it an editor's. A reload applies what the admin wrote in `holon.toml`,
    and chooses nothing.
  - Otherwise it becomes an admin's, and two rows flip.
- **D11. An agent holding the viewer role reads log lines without `AgentReadsLogs`**, because
  `logs.rs:365` keys on the role agent, not the kind.
  - **Recommended**: key the rule "the flow lets agents" on the kind, in its own commit after
    2b, which flips scout's row. `AgentRuns` can't differ, since a viewer can't run an action.
