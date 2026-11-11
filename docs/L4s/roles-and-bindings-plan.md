# Roles, groups and bindings: implementation plan

**Companion to** `access-and-approvals-L4.md`, the design, and to `principals-plan.md` and
`can-and-catalogue-plan.md`, its steps 1 and 2. This file says how to build the design's §14
step 3:

- custom roles (§5.1), groups (§5.2), and bindings of a role to a principal or a group at a
  scope (§5.3);
- tags, which select flows, written in the flow's text (§5.5);
- the access document and its versions (§6);
- `can` answering from bindings (§7), and `GET /auth/me/can`;
- the routes (§10), and `holon roles / groups / bind / unbind` (§11).

Narrowed tokens (§5.4) are part of §5 but §14 doesn't name them in any step. This plan puts them
last, as 3e, or later (D21).

Each sub-step names the files, functions and lines it changes at `52d54817` (the head of
`principals-2`, PR #74). It sketches the new code, lists the tests it changes and adds, and gives
the checks to run. Code here is a sketch of shape and names, not final code. Where the design
leaves a decision open, or the code raises one the design doesn't, the sub-step names it (§D).
§D also says which of step 2's D6-D11 step 3 settles, and recommends answers to the design's
§15 questions 1, 2 and 4.

## 0. Conventions

The conventions of `principals-plan.md` §0 and `can-and-catalogue-plan.md` §0 hold, with these
additions:

- **One commit for D11, then four sub-steps, one commit each, and a fifth if D21 keeps it.**
  - 3.0 keys "the flow lets agents" on the caller's kind (step 2's D11). It changes one assertion of
    the access table's tests, not a row, and names it.
  - 3a moves each principal's role out of the accounts book into the access document, as a
    binding at the host. No one may do more or less: the access table passes unchanged.
  - 3b adds scopes (a flow by its slug, an approval point), groups and custom roles, with their
    routes and `/auth/me/can`.
  - 3c adds the CLI (`holon roles`, `groups`, `bind`, `unbind`, `access`) and `holon mcp`'s
    `whoami`. The editor's People page and the workbench show bindings, and read the roles from
    the server.
  - 3d adds tags: in the language, the manifest and the deploy review, and as a scope. Its
    sample is `approved-payout`, tagged `payments`.
  - 3e, narrowed tokens, if D21 keeps them in step 3.
  - Each sub-step's docs go in its own commit: the design where it is now settled, the server
    reference (`docs/manual/reference/server.md`), the security and component notes, the
    CHANGELOG; and from 3d the language (`docs/LANGUAGE.md`, the manual's reference).
- **The access table is the contract.** Every caller in `tests/access_table.rs` holds one
  built-in role at the host. From 3a, its `server()` makes each role a binding instead of an
  account's field; its rows and `expected` don't change, and it passes after every sub-step.
  New behaviour gets new tests beside it, never a changed row.
- **Refusals keep step 2's words** for a caller refused by built-in roles that cover the
  resource. A caller with no role there, or only custom ones, is told what §7's second example
  says: `bob may not do that: no role bob holds on "payout" holds flow.deploy` (D16).
- **Class-level tests** (the test-suite review's rule): one model test holds `can` to §5.3 over
  every combination of a small universe of roles, groups, scopes, tags and kinds (3b), rather than
  a test for each case found.
- **Checks**:

  | Sub-step | cargo test, clippy | `npm run e2e` (slot 3) | tsc, vitest | wasm-pack build and test | Bless the manual |
  |---|---|---|---|---|---|
  | 3.0 | yes | | | | |
  | 3a | yes | yes: every editor request passes the gate, and the People page sets roles | | | |
  | 3b | yes | yes: lists are filtered | | | |
  | 3c | yes | yes: the People page, `holon bind` | yes | | |
  | 3d | yes | yes: every sample deploys | yes | yes | the samples and `holon.d.ts` (`HOLON_BLESS`); not `leaves.md`, which flow options don't touch |
  | 3e | yes | | | | |

  `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` each time.
  e2e runs locally with `HOLON_E2E_SLOT=3`, and the pull request gets the `e2e` label.
- **Run live** against `target/release/holon.exe` on scratch port 7530 (flows on 8230 up), with a
  scratch data directory and `--secrets-key file:` there, as step 2's script did:
  - 3a: an editor and a viewer set up through `POST /people`, acting as before; the access
    document's first versions; a restart reads them back;
  - 3b: a person bound `editor` on one flow deploys it and is refused another; a group's member
    acts through the group; a custom role; an expired binding; the last admin's binding refused
    removal; `/auth/me/can`;
  - 3c: 3b's run again, through `holon bind` and the editor's People page;
  - 3d: a version adding a tag is a change to approve, and an agent's is a proposal; a binding on
    the tag lets its holder act on the flow once deployed;
  - 3d also: the sample, as CLAUDE.md says: `approved-payout` deployed with its tag, a payout
    asked for, and the run decided by a person bound on `payments` only (`holon bind`);
  - 3e: a token narrowed to one flow deploys it and is refused another.

  Each commit says what was run.
- **Samples** (CLAUDE.md): tags are a feature of the language, so 3d, which adds them, changes a
  sample: `approved-payout` gets `tags: ["payments"]`. 3a, 3b, 3c and 3e change who may call the
  control plane, not what a flow does: their commits say so.

| Sub-step | Change | Where | Decisions |
|---|---|---|---|
| 3.0 | "The flow lets agents" keyed on the kind | `can.rs`, `tests/access_table.rs` | D11 |
| 3a | The access document; a role is a binding at the host; `can` reads bindings | `bindings.rs` (new), `can.rs`, `access.rs`, `accounts.rs`, `auth.rs`, `main.rs`, tests | D12-D15 |
| 3b | Scopes by slug and approval point; groups; custom roles; lists filtered; the routes; `/auth/me/can` | `bindings.rs`, `can.rs`, `auth.rs`, `http.rs`, `deploy.rs`, `flows.rs`, `proposals.rs`, `summary.rs`, `activity.rs`, `durable.rs`, tests | D16-D20, D25, D26, D28 |
| 3c | The CLI, `holon mcp`'s `whoami`, bindings on the People pages | `accountscmd.rs` (or a new `accesscmd.rs`), `main.rs`, `mcp.rs`, `editor/src/HostPages.tsx`, `workbench/admin.rs` | D27 |
| 3d | Tags: the language, the manifest, the deploy review, the tag scope; the sample | `holon-author`, `holon-core`, `holon-wasm`, `editor/src`, `docs/LANGUAGE.md`, `permissions.rs`, `touches.rs`, `deploy.rs`, `bindings.rs`, `approved-payout` | D22-D24 |
| 3e | Narrowed tokens (§5.4) | `accounts.rs`, `auth.rs`, `can.rs` | D21 |

## 1. Where a role is today, at `52d54817`

All paths are under `crates/holon/src/`.

- **Kept** as `Principal.role` (`accounts.rs:110`), one per principal, in `accounts.json`.
  - Set by:
    - `setup`, the first admin (:555);
    - `invite(name, role, email, by)` (:736) and `add(name, kind, role, owner, by)` (:753),
      from `POST /people`, whose body names the role (`auth.rs:201-235`);
    - `set_role` (:830), from `PATCH /people/<name>` with `{role}` (`auth.rs:306-309`), recorded
      "made editor".
  - `held(kind, role)` refuses a role the kind may not hold; `Kind::may_hold` reads
    `can::role(role).kinds`.
  - `Accounts::open` (:466-471) drops a person whose role is agent (step 1's precedent for a
    field no longer read: the server says which, and goes on).
- **Read**:
  - `who` (:654-667), `by_email` (:670-674) and `local` (:641-645) copy it into `Who.role`
    (:210-214). The run file's is always admin.
  - The gate copies it into `Caller.role` (`access.rs:98`).
  - `can`:
    - whether the role is held (`can.rs:272-282`): the role's kinds, and what it holds;
    - `fixed` (:296) keys "the flow lets agents" on the role (D11);
    - `refused` (:322, :334) picks its words by the role.
  - `keeping_an_admin` (`accounts.rs:815-828`) keeps an enabled principal whose role is admin.
  - `shown` (`auth.rs:41`), `signed_in` (:53) and `/auth/me` (:167-171) answer it. The editor's
    People page and sign-in, the workbench (`workbench/client.rs:123`, `workbench/ui/mod.rs:276`)
    and `holon people` read it (3c).
- **What `can` is asked about** (`can.rs:132-153`): a `Resource` names its slug, and an
  approval's flow, for scopes. Nothing reads them yet but refusals, hence its
  `#[allow(dead_code)]`.
- **Routes asking about every flow at once** (`On::Flows`, `can.rs:449-465`):
  - reading: `GET /unit/{hash}`, `GET /flows`, `GET /proposals`, `GET /proposals/{id}`,
    `GET /summary`, `GET /activity`, `GET /deployments`;
  - writing, for a flow the body names: `POST /unit`, `POST /deploy`, `POST /flows`,
    `POST /deploy/preview`, `POST /proposals/{id}/decline`.

  With every role at the host, every flow and this flow get the same answer. With scopes they
  don't (3b).
- **Deciding**: the route asks `run.decide` on the slug (`can.rs:476`). `durable::decider`
  (`durable.rs:887-891`) asks again on `Approval { slug }`, before the body names the approval
  (it is read at :906).
- **Deploying**: the gate asks `flow.propose` on every flow (`can.rs:451`). `deploy` asks
  `flow.approve-deploy` (`deploy.rs:154`, :508) and `flow.lifecycle` (:331) on the slug, but
  never `flow.propose` or `flow.deploy` on it.
- **Saving**: the gate asks `flow.edit` on every flow (`can.rs:452`). `flows::save` asks again on
  `Saved { name, by }` only when the name is saved already (`flows.rs:55`).

## 2. The model

### `bindings.rs` (new; `mod bindings;` in `main.rs`)

The access document, its versions, and the index `can` reads.

```rust
//! Who holds what, and where (docs/L4s/access-and-approvals-L4.md §5 and §6): custom roles,
//! groups, and bindings of a role to a principal or a group at a scope. They are kept in one
//! document, `access.json` beside `accounts.json`. Every change makes a new version, with who
//! made it, when and why, kept for good in `access.versions.jsonl`. The accounts book keeps who
//! the principals are; this keeps what they may do.

/// One version of the access document.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Document {
    pub(crate) version: u64,
    pub(crate) at: u64,
    pub(crate) by: String,
    /// The change, in words: "bound payout-approver to bob on payments" (§6's words).
    pub(crate) what: String,
    /// Why, as its author gave it, if they did ("on call this week").
    pub(crate) why: Option<String>,
    #[serde(default)]
    pub(crate) roles: Vec<RoleText>,
    #[serde(default)]
    pub(crate) groups: Vec<Group>,
    #[serde(default)]
    pub(crate) bindings: Vec<Binding>,
    /// The next binding's id: ids are never reused, so an old version's names stay unambiguous.
    next_id: u64,
}

/// A custom role, as kept (§5.1): what it allows, by the catalogue's names, and which kinds of
/// principal may hold it.
pub(crate) struct RoleText { name: String, about: Option<String>, allows: Vec<String>, kinds: Vec<Kind> }

pub(crate) struct Group { name: String, about: Option<String>, members: Vec<String> }

pub(crate) struct Binding {
    pub(crate) id: u64,
    pub(crate) who: Subject,
    pub(crate) role: String,
    pub(crate) scope: Scope,
    pub(crate) by: String,
    pub(crate) at: u64,
    pub(crate) expires_at: Option<u64>,
}

#[serde(rename_all = "lowercase")]
pub(crate) enum Subject { Principal(String), Group(String) }

#[serde(rename_all = "lowercase")]
pub(crate) enum Scope { Host, Flows(Selector), Approval { flow: Selector, name: String } }

#[serde(rename_all = "lowercase")]
pub(crate) enum Selector { Slug(String), Tag(String) }
```

On disk, a binding reads
`{"id": 7, "who": {"group": "payments-team"}, "role": "editor", "scope": {"flows": {"tag": "payments"}}, "by": "ada", "at": …}`.

```rust
/// The access document as the server holds it: the newest version, the index made from it, and
/// each deployment's tags (3d).
pub(crate) struct Bindings {
    /// None in memory (the tests).
    path: Option<PathBuf>,
    now: RwLock<Arc<Version>>,
    /// One change at a time, here or to a principal (enabling, removing), so two admins can't
    /// each remove the other's admin binding at once.
    pub(crate) changing: Mutex<()>,
    pub(crate) tags: Arc<Tags>,
}

/// A version and its index: each principal's bindings, its groups' included, with their roles
/// resolved. Made once per version (§7's performance note), so `can` reads, never parses.
struct Version { document: Document, held: HashMap<String, Arc<[Held]>> }

/// A binding as `can` reads it.
pub(crate) struct Held {
    pub(crate) binding: u64,
    pub(crate) role: Arc<RoleDef>,
    pub(crate) scope: Scope,
    pub(crate) expires_at: Option<u64>,
}
```

`RoleDef` (`can.rs:181-186`) becomes the one shape for built-in and custom roles: a `name`
instead of `role: Role`, `holds` and `kinds` owned, and `builtin: bool`. The four built-in ones
are made once (`LazyLock`), and a custom role's `allows` are parsed into `Do(action)` when a
version is indexed. A name not in the catalogue is refused when the role is defined, never at
`can`. `Role` (`access.rs:35-42`) stays as the built-in roles' names, for the tests. A
role is named by a string where a request names one, so a custom role can be one too (3b).

### The caller

```rust
pub(crate) struct Caller {
    pub(crate) name: Option<String>,
    pub(crate) kind: Option<Kind>,
    /// What it holds, and where, as the access document said when the request came in; none on
    /// a server without accounts, where the caller holds everything.
    pub(crate) holds: Option<Holding>,
}

pub(crate) struct Holding {
    pub(crate) version: u64,
    /// When the request came: a binding that expires during the request still holds for it.
    pub(crate) at: u64,
    pub(crate) held: Arc<[Held]>,
    pub(crate) tags: Arc<Tags>,
}
```

`Caller.role` goes. `access.rs` `authenticate` (:89-99) resolves the holding once, from the index:
`bindings.holding(&who.name)`. The run file's admin gets `Holding::admin()`, one `Held` of
`admin` at the host with binding id 0, never in the document (§6: "the run-file admin always
works"). `version` is what step 5's evidence will name (§8.6).

### What covers what

| Scope | Covers |
|---|---|
| `Host` | everything |
| `Flows(Slug(s))` | the flow `s`: `Flow(s)`, `Saved { name: s }`, `Action { slug: s }`, `Logs { slug: s }`, `Approval { slug: s }`; and `Flows` (some flow: the lists, 3b) |
| `Flows(Tag(t))` | the same, for each flow whose deployment is tagged `t` (3d) |
| `Approval { flow, name }` | `Approval { slug, name }` for a flow `flow` selects; and `Approval { slug, name: None }`, the gate's row (3b) |

No scope but `Host` covers `Host` or `Principal`: `people.*`, `role.*`, `resource.*`, `host.*`,
`policy.*` are the host's actions (`Resource::takes`, `can.rs:157-169`). A role bound at a flow
that holds one of them holds nothing by it there; `POST /bindings` refuses such a binding (D19).

### `can`

```rust
pub(crate) fn can(who: &Caller, action: Action, on: &Resource) -> Result<(), Refusal> {
    debug_assert!(on.takes(action), "{} isn't done to {on:?}", action.name());
    let held = match &who.holds {
        None => true,
        Some(h) => h.held.iter().any(|b| {
            b.live(h.at)
                && b.covers(on, &h.tags)
                && who.kind.is_some_and(|k| b.role.kinds.contains(&k))
                && b.role.holds(action, on, who)
        }),
    };
    if !held && !owns(who, action, on) {
        return Err(refused(who, action, on));
    }
    fixed(who, action, on)
}
```

It is §7's order, steps 2, 4 and 5. Step 1, enabled, is authentication's (a disabled principal
has no session or token that signs in). Step 3, a token's narrowing, is 3e's. The union over
covering bindings is the `any`. The `kinds` test is per binding and per caller, so a binding to a
group holds for its members whose kind the role allows, and gives the others nothing (§5.2).

## Step 3.0. "The flow lets agents", by kind (D11)

- `can.rs` `fixed` (:296): `let agent = who.role == Some(Role::Agent);` becomes
  `who.kind == Some(Kind::Agent)`.
- Why before 3a: 3a removes `Caller.role`, so the rule needs a key that isn't the role. The kind
  is what the design's fixed rules key on (§8.4, "people are people"). With custom roles (3b), an
  agent could hold `run.call` through a role an admin defined; keyed on the kind, it still runs a
  flow's action only where the flow lets agents.
- What changes: an agent holding the viewer role (`scout` in the table) reads a flow's log lines
  only where the flow lets agents, as one holding the agent role does.
- Test: in `tests/access_table.rs`, `an_agent_is_held_to_its_own_flows_and_to_what_each_flow_lets_agents`
  (:328) asks `c.kind == Some(Kind::Agent)` where it asked `c.role == Some(Role::Agent)` for the
  log lines (:356), and its comment (:320-322) drops "an agent holding the viewer role reads them
  without". No row of the table changes: its `GET /deployments/d/logs` row lets every role
  through, and a refusal in the flow's words isn't the role's.
- Checks: cargo test and clippy. No e2e (no editor request changes: the editor's agent is a
  person's tool, not an agent principal).

## Step 3a. The access document: a role is a binding at the host

No one may do more or less than before. Each principal's one role moves out of `accounts.json`
into `access.json`, as a binding of that built-in role at the host, and `can` reads bindings.
`/auth/me` and `GET /people` also answer each principal's bindings.

### `bindings.rs` (new)

Only what 3a uses: `Scope::Host`, `Subject::Principal`, and a `Binding` of a built-in `Role`. 3b
adds the other scopes, groups, custom roles, expiry and `why` (with `Holding`'s version and time).

```rust
pub(crate) struct Document { version, at, by, what, bindings: Vec<Binding>, next_id }
pub(crate) struct Binding { id, who: Subject, role: Role, scope: Scope, by, at }
pub(crate) struct Held { role: Role, scope: Scope }   // what `can` reads

impl Document {
    pub(crate) fn of(&self, name: &str) -> impl Iterator<Item = &Binding>;
    /// In place of the roles bound to `name` at the host; nothing changes if it is that one.
    pub(crate) fn set_host_role(&mut self, name: &str, role: Role, by: &str, at: u64);
    /// Unbind everything bound to `name`.
    pub(crate) fn forget(&mut self, name: &str);
    pub(crate) fn after(self, before: &Document, by: &str, what: &str, at: u64) -> Document;
}

impl Bindings {
    /// The newest version at `path`: the history's last whole line when it is newer than
    /// `access.json` (the server stopped between the two writes); version 0, holding nothing,
    /// if neither is there.
    pub(crate) fn open(path: PathBuf) -> Result<Bindings, String>;
    pub(crate) fn document(&self) -> Arc<Document>;
    /// What `name` holds, from the index made once per version.
    pub(crate) fn held(&self, name: &str) -> Arc<[Held]>;
    /// Appended to the history, then written as `access.json`, then indexed.
    pub(crate) fn keep(&self, document: Document) -> Result<(), String>;
}
```

The checks are the accounts book's (below), since a binding names a principal there: `Bindings`
keeps versions and answers what each principal holds.

Kept:
- `access.json`: the newest version, whole, written with `sealing::write_private` (as
  `accounts.json` is).
- `access.versions.jsonl`: every version, whole, one per line, appended (`sealing::append_private`)
  before `access.json` is replaced.

So `access.json` is never ahead of its history. A version is a few KB (tens of bindings): a
thousand changes are a few MB. A diff per version is for later, if one is needed (D14).

The path is beside the accounts (`bindings::beside`): `access.json` beside `accounts.json`,
`.holon-access.json` beside `.holon-accounts.json`, `{name}.access.json` beside any other name. No
flag of its own (D14). `main.rs` counts both files among the server's own, which no folder binds.

### The accounts book keeps the document

`Accounts` holds the `Bindings`, and changes it only under the book's lock, so a principal and
what is bound to it change together. `change_access(by, what, f)` is `change` for both: `f`
changes a copy of the book and a copy of the document; then every binding must name a principal
in the book whose kind may hold its role (`held`, in step 1's words: "a person is a viewer, an
editor or an admin"), and an admin must be left if there was one (D15: an enabled person bound
admin at the host; the run file's admin doesn't count). Refused, nothing changes. When the
bindings changed, the document's next version is by `by` and says `what`.

| Change | `Accounts::` | The version's `what` |
|---|---|---|
| The first admin | `setup` | "set up the server: bound admin to ada on this host" |
| A person invited | `invite(name, role, email, by)` | "invited bob: bound editor to bob on this host" |
| An agent or a service added | `add(name, kind, role, owner, by)` | "added claude, an agent: bound agent to claude on this host" |
| A principal's role changed | `set_role(name, role, by)` | "bound viewer to bob on this host" |
| A principal removed | `remove(name, by)` | "removed bob, and what was bound to bob" |
| Disabled or enabled | `set_disabled` | none: nothing is bound anew, but D15 is checked |

`invite`, `add` and `setup` keep their signatures, so `auth.rs`, `mcphosts.rs` and the tests that
make principals don't change. The activity log's words in `auth.rs` stay as they were ("invited",
"made editor", "removed").

- **Ordering across the two files.** Accounts first, then access, so a principal is made with no
  role and then given one. A server stopped between the two leaves a principal holding nothing:
  closed, not open. The run file's admin binds it.
  - Removing goes the other way: access first, then accounts. So a principal is never removed
    while its bindings stay, which a new principal of the same name would hold. Its bindings go
    in the document, kept in its history.

### `accounts.rs`

- `Principal.role` and `Who.role` go; `keeping_an_admin` becomes `an_admin(book, document)`,
  checked by `change_access`.
- `holding(name)`, `host_role(name)` (the widest built-in role bound at the host) and
  `bindings_of(name)` answer from the document.
- `Accounts::open` opens the document beside the accounts. A file from before reads its
  principals and ignores their `role` (D13); while the document is at version 0 the server says
  so at start: "holon: alice, bob hold no role: access.json is new. The OS user gives each one:
  holon people set NAME --role ROLE". Step 1's message for a person with role agent goes, with the
  field.

### `can.rs`

- `can` asks each role bound to the caller where the resource is (`Held::covers`: `true` for
  `Host`), for the caller's kind. A caller with `holds: None` is on a server without accounts and
  may do everything.
- `refused`: step 2's words, chosen from the roles that cover the resource rather than one role.
  With only `Host` scopes those are the caller's roles, so every refusal reads as before. One
  holding none there is told D16's words: `eve may not do that: no role eve holds on this host
  holds people.view`.
- `RoleDef` and `BUILTIN` stay as they are; 3b makes roles data.

### `access.rs`

- `Caller { name, kind, holds: Option<Arc<[Held]>> }`, with `host_role()` and a test-only
  `Caller::holding(name, kind, Role)` for `tests.rs:2269` and `can.rs`'s tests.
- `authenticate`: the run file's token holds `Held::admin()`; any other caller what is bound to it.
- `Role::name()`: "editor", as the versions write it.

### `auth.rs`

- `shown` answers `bindings` (each `{id, role, scope, by, at}`) and `role`, the widest built-in
  role bound at the host (`host_role`), or none. The editor and the workbench gate what they show
  on `role` (3c lists where). It stays until step 7 moves the editor to `/auth/me/can` (§12,
  "buttons follow `can`").
- Signing in and `/auth/me` answer the same; the run file's admin `role` admin, with no
  bindings.

### Tests in 3a

- `tests/access_table.rs`: `server()` is as it was, since `invite` and `add` bind the role.
  `kept()` reads each principal's bindings in place of its role. The rows and `expected` don't
  change, and the table passes.
- `accounts.rs`: `no_change_leaves_no_admin` (D15: binding the last admin another role, disabling
  and removing them are refused in step 1's words, the run file's admin doesn't count);
  `each_change_to_what_is_bound_is_a_version_and_a_removed_principal_holds_nothing` (each
  change's version, by and what; no version when nothing is bound anew; a principal removed and
  made again holds only what it is given); `kept_as_hashes_and_read_back` reads the bindings back;
  `a_file_from_before_the_access_document_keeps_its_principals_holding_nothing` (D13).
- `bindings.rs`: `a_version_is_kept_with_who_when_and_what_and_read_back`;
  `the_history_is_never_behind_the_document` (the stop between the two writes, and a last line
  cut short); `the_document_is_kept_beside_the_accounts`.
- `can.rs`: `a_caller_may_do_what_any_role_it_holds_holds` (one holding two roles may do what
  either holds; one holding none is refused everything a role holds, in D16's words).

### Checks

cargo test, clippy, e2e (the editor's sign-in and People page read `role`; it is still answered).
Live on 7530: set up, add an editor and a viewer and act as each, change a role, restart and read
the versions back.

## Step 3b. Scopes, groups, custom roles and their routes

What a binding at a flow or an approval point holds, what a group's member holds through it, and
roles an admin defines. Every caller of the access table still holds one built-in role at the
host, so the table passes unchanged. New tests hold the rest.

Built in two parts. **Part 1** (built): scopes, expiry and `why`, the lists and the handlers that
know the flow, and the bindings and versions routes (`GET`/`POST /bindings`,
`DELETE /bindings/{id}`, `GET /access/versions`, `GET /access`). **Part 2** (built): groups,
custom roles, `/roles`, `/groups`, `/auth/me/can`, D20's fixed rule and D28. Where part 1 differs
from the sections below:

- **D19's third rule** ("a role holding a host action is bound only at the host") would refuse
  `editor` at a flow, since `editor` holds `policy.edit` and `host.check`. As built, a binding is
  refused only where its role holds nothing its scope covers (400: `viewer` or `agent` at an
  approval point). The author must hold every action of the role that the scope covers (403):
  each is asked of `can` on the host for a host action, and else on the flow, the approval point,
  or (at the host) the empty slug, which only the host covers (`can::everywhere`).
- **`GET /activity`**: a caller who views every flow (by a binding on the host) sees every
  entry, as today. Any other sees only what was done to the flows they view (`DONE_TO_A_FLOW` in
  `activity.rs`), not "the rest as today": that would show a flow's holder who was bound what, and
  the host's resources and sweeps.
- **`GET` and `DELETE /flows/{slug}`** ask on `On::Flows`, and the handler asks about the flow
  once it knows whose it is. D18's draft (saved by its caller, deployed by no one) is theirs
  wherever they hold the action, which the gate can't know from the path.
- **`GET /unit/{hash}` and `POST /unit`** stay on `On::Flows`: a unit is content-addressed, and
  isn't any flow's until a deploy names it.
- **A proposal's `agent`** is its author's kind: a person bound to propose but not to approve
  makes one too.
- **A version's words** for a binding: `bound editor to bob on "payout"`, with
  `, until <time>` when it expires; `unbound editor from bob on "payout"`. The activity log keeps
  the same, as `bound editor to` `bob on "payout"`.

Where part 2 differs:

- **The whole document is checked on every change** (`Document::check`, from
  `accounts::change_access`): each role is one (`RoleText::def`) and defined once; each group is
  named as a name is written, once, and its members are principals here; each binding is of a
  role here, to a group here or to a principal whose kind may hold it. So changing a role's kinds
  is refused while one of its bindings is to a principal it would no longer allow. A binding to a
  group isn't refused for members of other kinds: it holds nothing for them, as §5.2 says.
- **`bindings::route` comes before `auth::route`** in `http.rs`: `auth::route` answers every
  `/auth/…` path, an unknown one with 404, so `/auth/me/can` must be reached first.
- **`/auth/me/can` with no flow** asks on the host for the host's actions, on every flow at once
  (`Resource::Flows`) for `flow.*` and `run.*`, as the lists ask, and everywhere
  (`can::everywhere`'s empty slug) for `state.*` and `secret.*`, which no list asks. Its test,
  `me_can_answers_what_each_route_asks`, holds it to `can::may` for every route that asks of the
  host, the flows or a flow, for four callers.
- **`PUT /roles/<name>` (D19)** asks `can` for each action the role allows: on the host for the
  host's actions, everywhere for the others. 403 names the first its author doesn't hold.
- **D28's words**: `watcher is bound 2 times: unbind them first` (`once: unbind it first`), 409;
  `editor is a built-in role: it stays`, 400; `no role "x"`, 404.
- **A custom role's actions** are kept as written, and answered and compared in the catalogue's
  order (`RoleDef::custom`), so two roles allowing the same are the same role.
- **The `role` a principal is shown as** (`host_role`) counts only the built-in roles its kind may
  hold: an agent in a group bound `editor` isn't shown as one.
- **The last admin (D15)** may be held through a group: an admin binding on the host, for good, to
  a group with an enabled person in it counts.
- **D19 for every way access is given**, not only `POST /bindings` and `PUT /roles`: a custom role
  holding `people.manage` or `people.assign` would otherwise make an admin. Inviting, adding or
  setting someone with a role (`POST`, `PATCH /people`) is binding it on the host; a member added
  to a group gets each of its bindings; resetting a password or making a token lets its maker act
  as that principal, so they hold all it holds (`bindings::beyond`), save its owner making its
  tokens (§3). Removing, disabling and unbinding give nothing, and aren't asked. The test is
  `no_one_gives_more_than_they_hold`.
- **The versions' words**: `defined the role watcher: run.view, held by a person`,
  `changed the role …`, `removed the role watcher`; `made the group ops: bob`,
  `changed the group ops: bob, cy`, `removed the group ops`; `bound watcher to the group ops on
  "d"`.
- **Tests**: the D19 tests are part 1's `a_binding_gives_only_what_its_author_holds_there` and
  part 2's `a_role_an_admin_defines_holds_what_it_allows`; `an_expired_binding_holds_nothing` is
  part 1's `what_is_bound_is_listed_versioned_and_expires`; `bindings.rs` adds
  `what_is_kept_is_checked_whole` and `a_groups_binding_holds_for_each_member`. The People guide
  waits for 3c's pages and CLI.

### Scopes in `bindings.rs` and `can.rs`

- `Held::covers(on, tags)` as §2's table says, for `Flows(Slug(_))` and `Approval { .. }`.
  `Flows(Tag(_))` covers nothing until 3d gives deployments tags.
- `Resource::Approval` (`can.rs:149-150`) gains `name: Option<&str>`: none for the gate's row,
  the point's name for the handler.
- `On` (`can.rs:355-362`) gains `Approvals`: "some approval of the path's slug". The decide row
  (:476) asks `run.decide` on `On::Approvals`, which `may` (:538-552) makes
  `Approval { slug, name: None }`. A binding at one approval point covers it, so the request
  reaches the handler.
- `durable::decider` (`durable.rs:887-891`) takes the approval's name. `handle_post_decide`
  (:897-912) reads the body first, then asks `decider(caller, slug, &body.name)`, on
  `Approval { slug, name: Some(name) }`. A malformed body is answered 400 before the caller is
  refused, as any route with a body is.
- What an approval point's binding lets its holder see: only deciding, as §5.3 says (D25).

### The lists, and the handlers that know the flow

A list route's row asks about every flow at once (`On::Flows`). From 3b, `Flows` is covered by
any binding at the host or at some flow, so the gate lets through whoever holds the action
somewhere, and the handler keeps the flows the caller may view (D17):

| Route | Handler | Keeps |
|---|---|---|
| `GET /flows` | `flows::handle_get_flows` (`flows.rs:67`) | saved flows the caller may view (D18) |
| `GET /deployments` | `deploy::handle_get_deployments` (`deploy.rs:803`) | deployments the caller may view |
| `GET /proposals` | `proposals::handle_get_proposals` (`proposals.rs:227`) | proposals of flows the caller may view |
| `GET /proposals/{id}` | `handle_get_proposal` (:236) | 404 for a flow the caller may not view, as for no proposal |
| `GET /summary` | `summary::handle_get_summary` (`summary.rs:127`) | the deployments the caller may view, and their counts |
| `GET /activity` | `activity::handle_get_activity` (`activity.rs:76`) | entries whose target is a flow the caller may not view are left out; the rest as today |

Each takes the `Caller` from `http.rs` (:198-262). The handlers that act on a flow the body names
ask about that flow, where today only the gate asks, about every flow:

- `POST /deploy`: `deploy` (`deploy.rs:154`) asks `flow.propose` on the flow (D22 adds its
  version's tags in 3d).
- `POST /deploy/preview`: `handle_post_deploy_preview` (:490-508) asks `flow.propose` on the
  flow.
- `POST /flows`: `flows::save` (:42-60) asks `flow.edit` on `Saved { name, by }` for a new name
  too, as the caller's own (D18).
- `POST /proposals/{id}/decline`: `handle_post_decline` (`proposals.rs:251`) asks
  `flow.approve-deploy` on the proposal's flow.

`deploy.rs:154`'s refusal of `acknowledge` says "an agent doesn't approve…". A person bound to
propose but not to approve on this flow now gets there too: their words become
"`{name}` doesn't approve what `{slug}` may do: deploy without "acknowledge" …" for anyone who
isn't an agent. An agent keeps today's words.

### Groups and custom roles

- `Group { name, about, members }`. Members are principals' names; adding one that doesn't
  exist is refused; removing a principal removes it from its groups (3a's ordering). One level
  (§5.2): a member is never a group.
- `RoleText { name, about, allows, kinds }`:
  - each of `allows` is a catalogue name (`Action::named(&str)`, new beside `Action::name`);
  - `kinds` is not empty;
  - the name is not a built-in role's, and is a valid name (`accounts::valid_name`).
- A built-in role can't be defined, changed or removed. A custom role, or a group, is removed
  only when nothing binds it (D28).
- No custom role gives an agent what is a person's (D20): `fixed` (`can.rs:295-315`) gains the
  rule that an agent never approves a deploy, starts, stops or deletes a flow, restores one but
  as a dry run, writes or rolls back a secret, or holds `people.*`, `role.*`, `resource.*` or
  `host.operate`, whatever role it holds. Refused in step 2's agent words. The agent role holds
  none of these, so no row of the table changes.

### Who may bind what (D19)

`POST /bindings` and `PUT /roles` are `people.assign` and `role.define`, an admin's. And:

- a binding gives only what its author holds where it is made: every action of its role, at
  its scope. So a custom role holding `people.assign` doesn't make an admin;
- a role holding a host action (`people.*`, `role.*`, `resource.*`, `host.*`, `policy.*`) is
  bound only at the host: bound at a flow, it would hold nothing by it. Refused, with the
  action named;
- defining a role allows only what its author holds at the host.

### Routes

`can.rs`'s `ROUTES` gains 12 rows, after People (:435-445):

```rust
    // Roles, groups and bindings (bindings.rs).
    asks("GET", "/roles", RoleView, On::Host),
    asks("PUT", "/roles/{name}", RoleDefine, On::Host),
    asks("DELETE", "/roles/{name}", RoleDefine, On::Host),
    asks("GET", "/groups", PeopleView, On::Host),
    asks("PUT", "/groups/{name}", PeopleAssign, On::Host),
    asks("DELETE", "/groups/{name}", PeopleAssign, On::Host),
    asks("GET", "/bindings", PeopleView, On::Host),
    asks("POST", "/bindings", PeopleAssign, On::Host),
    asks("DELETE", "/bindings/{id}", PeopleAssign, On::Host),
    asks("GET", "/access/versions", PeopleView, On::Host),
    asks("GET", "/access", PeopleView, On::Host),
```

and, beside `/auth/me` (:396), `signed_in("GET", "/auth/me/can")`. `find` matches by segment, so
it doesn't collide with `/auth/me`. `Action`'s `#[allow(dead_code)]` (:44) goes: `people.assign`,
`role.view` and `role.define` are asked now.

| Route | Body | Answer |
|---|---|---|
| `GET /roles` | | every role, built-in ones first: `{name, about, allows, kinds, builtin, bound}`, `bound` the number of bindings |
| `PUT /roles/<name>` | `{about, allows, kinds, why}` | the role, and the version |
| `DELETE /roles/<name>` | | 204; 409 while bound |
| `GET /groups` | | `{name, about, members, bound}` |
| `PUT /groups/<name>` | `{about, members, why}`: the whole set | the group, and the version |
| `DELETE /groups/<name>` | | 204; 409 while bound |
| `GET /bindings` | `?who=&role=&flow=` filter | `{id, who, role, scope, by, at, expires_at, expired}` |
| `POST /bindings` | `{who: {principal\|group}, role, scope, expires_at, why}` | the binding, and the version |
| `DELETE /bindings/<id>` | `?why=` | 204; 409 for the last admin's (D15) |
| `GET /access/versions` | | `{version, at, by, what, why}`, newest first |
| `GET /access` | `?version=n` | that version, whole; the newest without |
| `GET /auth/me/can` | `?actions=flow.deploy,run.decide&flow=payout&approval=ok` | `{"flow.deploy": true, "run.decide": "bob may not do that: …"}` |

- The handlers are `bindings::route(state, request, caller)`, called from `http.rs` beside
  `auth::route`. Each change records its `what` in the activity log, with its author.
- `/auth/me/can` asks `can` for the caller:
  - for each action, on the named flow (`Flow(slug)`), or on its approval point for
    `run.decide` with `approval`;
  - with no flow, at the host for host actions, and on every flow at once for the others.

  An unknown action is answered 400. The editor's buttons use it from step 7. A caller's own
  bindings are in `/auth/me` (3a).
- CORS: `http.rs:73`'s `Access-Control-Allow-Methods` gains `PUT`. Its test (`tests.rs:4272-4292`,
  from a bug where it lacked one) covers the new method.

### Refusals (D16)

`refused` (`can.rs:319-340`) picks its words from the roles whose bindings cover the resource:

- some cover it, and all are built-in: step 2's words, chosen as before (`that needs an admin`
  when no built-in role but admin holds the action; the agent's words for a caller holding
  `agent` there; else `a viewer only reads`);
- none cover it, or a custom one does:
  `{name} may not do that: no role {name} holds on "payout" holds flow.deploy`, with "on this
  host" for a host action, and `for "payout"'s approval "ok"` for an approval point.

### Tests in 3b

- `bindings.rs` and `can.rs`, the model test, `can_is_the_union_of_the_bindings_that_cover_it`
  (the class-level test):
  - The universe:
    - two flows;
    - an approval point on one;
    - the four built-in roles and two custom ones;
    - a group;
    - a person, an agent and a service;
    - bindings at each scope, to a principal and to the group, one expired.
  - For every caller, action and resource, `can` agrees with a reading of §5.3 written in the
    test: some binding of the caller or its group, live, at a scope that covers the resource,
    of a role holding the action and allowing the caller's kind; then the fixed rules.
  - It enumerates every subset of a small binding set rather than sampling, so a failure names
    its case.
- `access_table.rs` gains the 12 rows (`Today::Admins`, and `SignedIn` for `/auth/me/can`). No
  existing row changes.
- New, in `tests.rs` (or `tests/scopes.rs`), through the real gate and handlers:
  - `a_binding_at_a_flow_lets_its_holder_act_there_and_nowhere_else`: bob, `editor` on `d`,
    deploys, stops and decides `d`, is refused on `e`, and lists only `d`;
  - `a_groups_binding_holds_for_its_members_of_the_kinds_its_role_allows`;
  - `a_binding_at_an_approval_point_decides_that_point_only`;
  - `an_expired_binding_holds_nothing`;
  - `no_one_binds_what_they_dont_hold`, and `a_host_role_binds_only_at_the_host` (D19);
  - `no_role_gives_an_agent_what_is_a_persons` (D20);
  - `a_bound_role_or_group_is_not_removed` (D28);
  - `me_can_answers_what_can_answers`: for every action, `/auth/me/can` answers what a request
    of that action gets.
- `durable.rs`'s decider test (`tests.rs:2269-2285`) passes the approval's name.

### Checks

cargo test, clippy, e2e (the lists are filtered, for an editor at the host the same lists). Live
on 7530: a person bound `editor` on one flow and one bound at its approval point; a group; a
custom `payout-approver`; an expired binding; the last admin's binding refused; `/auth/me/can`
for each.

## Step 3c. The CLI, `holon mcp` and the People pages

Built. Where it differs from the sections below:

- **No `--tag` yet**: a binding on a tag, and who may make one (D22), are 3d's, with tags.
  `holon bind` takes `--flow` and `--approval` now; 3d adds `--tag`.
- **The roles a kind may hold are the server's to say**, not the CLI's: `roles_of` is gone,
  rather than read from `GET /roles`, and `holon people add --role` and `set --role` send the
  name as given. `POST` and `PATCH /people` take any role here, an admin's own included
  (`Accounts::invite`, `add`, `set_role` take a role's name, `Document::check` refuses one the
  kind may not hold), so a mistake comes back in the server's words: `no role "nope": GET /roles
  lists them`, `an agent may not hold approver: it is held by a person`.
- **`/auth/me` and `GET /people` answer each binding's `allows`**: an agent may not read
  `/roles` (`role.*` is a person's, D20), and `whoami` says what each of its bindings allows.
- **`holon groups add` and `drop`** read `GET /groups` and `PUT` the whole member set, keeping
  the group's `about` unless `--about` says another.
- **`holon bind --approval`'s hint (D25)** asks only when its caller may read who holds what
  (`/auth/me/can` for `people.view` and `role.view`): a principal sees the runs by a binding of
  its own or of its groups', a group by its own, on the host or the flow, of a role allowing
  `run.view`.
- **`holon people list`'s role column is `holds`**: each binding, as `holon bind`'s flags write it
  (`viewer, editor on payout via payments, editor at payout:ok until … (expired)`).
- **The People page's role select** is the principal's own binding on the host (not a group's,
  not expired); one with none shows "no role on the host". The rest of what holds for it is a
  line beside it, read only. *Your agents and services*, which a non-admin opens, can't read
  `/roles`, and titles its row with the bindings instead.
- **Approvals asks `/auth/me/can` once per approval point** a run waits at, each time the runs
  are read (`useApprovals`), and keeps the answers by `pointKey(slug, name)`; `mayDecide` and
  the nav's count read them. Who decides is no longer a person who is an editor or an admin.

### `holon roles`, `groups`, `bind`, `unbind`, `access`

As §11 writes them, through the run file's admin by default, as `holon people` is
(`accountscmd.rs:91-96`):

```
holon roles [list | define NAME --allow run.decide,run.view [--kinds person] [--about TEXT] | remove NAME]
holon groups [list | add GROUP NAME… | drop GROUP NAME… | remove GROUP]
holon bind ROLE (--to NAME | --group G) [--flow SLUG | --tag TAG | --approval FLOW:NAME] [--until DATE] [--why TEXT]
holon unbind ID [--why TEXT]
holon access [versions | show [--version N]]
```

- `--flow` and `--tag` in place of §11's `--flows SEL`, so a slug and a tag can't be mistaken
  (D27). A binding on a tag is kept from 3c, and covers flows once 3d gives them tags.
  `--approval FLOW:NAME` takes a slug; an approval point selected by tag is for when it's asked
  for.
- `--until` takes a date or a duration (`2026-12-01`, `7d`).
- `holon bind --approval` says so when the subject can't view the flow's runs (D25):
  "bob decides payout's approval "ok" but can't see its runs: holon bind viewer --to bob --flow payout".
- `main.rs:586-625` dispatches the new commands; their usage text goes beside `PEOPLE_USAGE`
  (`accountscmd.rs:12-60`), in a new `accesscmd.rs` if `accountscmd.rs` grows too long.
- `holon people`:
  - `list` shows each principal's bindings (role at scope), where it showed a role;
  - `add --role` and `set --role` keep setting the host role (3a's `POST` and `PATCH`);
  - `roles_of` (:99-106), the roles a kind may hold, is read from `GET /roles`, so a custom role
    is offered too.

### `holon mcp`'s `whoami`

A tool in `tool_list` (`mcp.rs:148`): its name, kind, owner, bindings, and, for each, what its
role allows. It reads `GET /auth/me`. The instructions (:51-63) name it. A tool for agent slots
(`decide`) is step 5's.

### The editor and the workbench

Step 7 replaces the People page with §12's tabs and moves every button to `/auth/me/can`. Step 3
keeps the page and adds only what custom roles would otherwise break:

- `HostPages.tsx`:
  - `ROLES_OF` and `ROLE_TITLE` (:87-95), hard-coded, are read from `GET /roles`;
  - the role selects (:499-505, :565, :595-600, :650-660) offer the host roles a kind may hold,
    custom ones included;
  - each principal's row lists its bindings (read only).
- `workbench/admin.rs`'s roles (:384-389) are read from `GET /roles`.
- `decisions.ts`'s `mayDecide` (:25-29) asks `/auth/me/can` for `run.decide` on the run's flow
  and approval point, where it asked whether the person is an editor or an admin (:17-21). So a
  person bound at a flow, a tag or an approval point decides in Approvals
  (`ApprovalsPage.tsx:201-251`), as 3d's sample needs. It is the one button step 3 moves to
  `can` ahead of step 7.
- The rest of the editor's gating on `role` stays until step 7 (`App.tsx:820-828`, :1351-1358,
  :1376, :1384, :1426; `ui/Shell.tsx:57`, :121; `ActionRows.tsx:44-102`;
  `FlowSettings.tsx:70-72`). A person bound only at a flow has no host role, so those buttons
  don't know what they may do there. The server decides anyway, and refuses in its words.

### Tests in 3c

- The CLI's parsing, and `holon bind` through the run file to a scratch server
  (`editor/e2e/cli.e2e.ts:114-122` gains `holon roles define`, `holon bind`, `holon unbind`,
  `holon access versions`).
- `mcp.rs`: `whoami` for an agent bound at the host and at a flow.
- e2e: the People page offers a custom role, and lists a principal's bindings.

### Checks

cargo test, clippy, tsc, vitest, e2e. Live: 3b's run again through the CLI and the editor.

## Step 3d. Tags

A flow's tags are written in its text, part of its server declaration and so of its unit's hash,
and part of its manifest, so a version that adds one is a change to approve (§5.5). A binding at
`Flows(Tag(t))` covers every flow whose deployed version is tagged `t`.

### The language (`holon-author`)

`flow("payout", { port: 8153, tags: ["payments"] })`:

- `lang/ts.rs` `kind()` (:750-839) reads `tags`: an array of string literals, each a tag:
  lowercase ASCII letters, digits and `-`, 1 to 40 characters. They are kept sorted, once each,
  in a new `HeaderKind::Tags(Vec<String>)` (`lang/ast.rs:47-63`). An unknown option's message
  (:835) names `tags`.
- `lang/lower.rs` `settings()` (:550-587) puts them in `Settings.tags`. Its match over
  `HeaderKind` is exhaustive, so the new variant can't be missed.
- `builder.rs` `Settings` (:39-48) gains `tags: Vec<String>`, skipped when empty.
- The unit (D23): `routes.rs` `server_unit` (:842-856) writes a frozen `tags` param, only when
  there is one, as `taint` is (:852), so no other flow's hash moves. `flow.rs` `derive_server`
  (:193) and `flow_unit` (:534) take the settings' `tags` beside `port`, and so do their wasm
  bridges (`holon-wasm/src/lib.rs:786-803`) and the editor's `flowUnitHash`.
- `lang/print.rs` `settings()` (:1101-1140) prints `tags` after `browsers`.
- Where the options are listed:
  - the hover text (`editing.rs:79`, :208-215);
  - the agent's guide (`agent.rs:629-631`);
  - `holon.d.ts` `Settings` (:40-55), and its copy in `samples/` (`HOLON_BLESS`);
  - `docs/LANGUAGE.md` (:89, :1604, :1655-1663);
  - the manual's options table (`docs/manual/language/reference.md:88-97`).

  `leaves.md` comes from the catalog alone, so the manual's leaves aren't blessed.

### `holon-core`

- `declare.rs` `Server` (:271-287) gains `tags: Vec<String>`. `read_server` (:463-599) reads the
  param and checks each tag's form. It ignores params it doesn't know, so an older server would
  read a tagged unit without its tags: dev phase, no older servers (D23).
- `manifest.rs` `manifest_items` (:28-63) takes `tags`, and makes an item `Tag("payments")` for
  each. Its callers: `permissions.rs:374`, and the wasm bridge's `AskedFor`
  (`holon-wasm/src/lib.rs:336-361`) with the editor's `Requirements` type
  (`editor/src/server.ts:554-567`) and `manifestItems` call (`App.tsx:649`).

### The server

- `permissions.rs`: `Requirements` (:293-324) gains `tags`, set from the unit's server
  declaration in `of_unit` (:331-350), never from what the client sends. So the manifest's
  `Tag` items are the text's.
- `needs_approval` (:821-836): a `Tag` item not in the previous grant needs approval, like any
  item. `is_auto` (:777-782) never grants one: the policy says what a flow may reach, not who
  may act on it (D24).
- `touches.rs`:
  - `GROUPS` (:20-21) gains `access`, "Who may act on it".
  - `part` (:25-97) places `Tag`: verb "joins", name the tag. A removed tag is a removed row, as
    any removed item is.
  - `lines` (:319-341) and the editor's `touchLines` (`touches.ts:50-60`) print the group.
- Who gains access (§9's sentence, D24): the preview (`preview_value`, `deploy.rs:529-599`)
  answers, for each tag the version adds, the bindings on it now:
  `joins payments: payments-team may view and call it (editor); bob decides its approvals (payout-approver)`.
  It is the tag row's words, in `touches` and in the editor's review.
- Tags kept per deployment: `Deployment` (`main.rs:328`, beside `browsers` at :384),
  `DeploymentRecord` (`wire.rs:309`, :349) and `DeploymentEntry` (`wire.rs:379`, :415, so
  `GET /deployments` shows them). They are always the live version's, taken from its unit at
  deploy and restore.
- The tag index, `Bindings.tags`: slug to tags, set by deploy and restore, cleared by deleting a
  deployment (`DELETE /deployments/{slug}`), filled at start from the deployments
  (`main.rs`, after they load). `Held::covers` reads it for `Flows(Tag(t))`.

### Deploying across scopes (D22)

To deploy a version, the caller holds the action (`flow.propose`, and `flow.approve-deploy` to
approve what it needs) on the flow as it is deployed now, if it is, and on the flow as the
version would make it, with the version's tags:

- `Resource::Version { slug, tags }`, new, covered by `Flows(Slug(slug))` and by
  `Flows(Tag(t))` for each `t` in `tags`;
- `deploy` asks both. So bob, `editor` on `payments`:
  - deploys a new flow tagged `payments`;
  - is refused tagging `payments` onto a flow he holds nothing on (taking it over);
  - is refused removing `payments` from a flow, which would take it out of his own reach.

  A host admin does those.
- An agent adding a tag makes a proposal, as for any item it can't approve.

### The sample

`approved-payout` (`crates/holon-author/samples/approved-payout.holon.ts:35`) becomes
`flow("approved-payout", { port: 8153, tags: ["payments"] })`, and its comment says who acts on
it through the tag. Its entry (`samples.rs:698-708`):

- `shows` gains `"tags"`;
- `description` says it is tagged `payments`, so whoever is bound on the tag acts on it;
- `try_it` adds `holon bind editor --to bob --tag payments` (3c's command) before bob decides.

The design's three-slot chain for it (§13) stays step 9's.

- Its hash changes, so the goldens that print it change (`eval.txt`, `server.txt`), and so does
  `samples.test.ts`'s expected manifest for it (:499-523), which gains `Tag("payments")`.
- `samples.rs:1147-1165` checks that the text agrees with the entry. `samples.test.ts:101-108`
  reads the `flow(` line by a regex: both learn `tags`.

### Tests in 3d

- `holon-author`: `tags` lowers into the settings and the server unit; a bad tag, a duplicate, a
  non-literal and spread are refused, each in its words; a flow without tags has the hash it had
  (a class test over every sample: only `approved-payout`'s hash moves).
- `holon-core`: `read_server` reads tags; `manifest_items` makes `Tag` items.
- `permissions.rs`: tags come from the unit, not the client (a request claiming other tags gets
  the unit's); a new tag needs approval, a kept one doesn't, and the policy never grants one.
- `tests.rs`:
  - `a_binding_on_a_tag_covers_the_flows_deployed_with_it`: deploying, restoring to an untagged
    version and deleting each move the flow in and out of the tag's reach;
  - `a_version_is_deployed_only_by_who_holds_it_as_it_is_and_as_it_would_be` (D22);
  - `an_agents_new_tag_is_a_proposal`;
  - `the_preview_says_who_a_tag_lets_in` (D24).
- wasm: `wasm-pack test --node crates/holon-wasm` (the manifest bridge); vitest
  (`samples.test.ts`, `touches`).

### Checks

cargo test, clippy, `wasm-pack build` and test, tsc, vitest, e2e (every sample deploys; the
review shows the tag's row). `HOLON_BLESS=1` for the samples and `holon.d.ts`. Live: the sample
deployed with its tag, approved by ada; bob, bound `editor` on `payments` only, decides its run
and is refused a flow without the tag; claude's version adding a tag kept as a proposal.

## Step 3e. Narrowed tokens (if D21 keeps them in step 3)

- `ApiToken` (`accounts.rs:172`) gains `only: Option<Vec<String>>` (catalogue names) and
  `flows: Option<Selector>`. `POST /people/<name>/tokens` takes them, and `holon people token NAME
  --only … --flow …|--tag …` sends them.
- `who` (:654-667) answers them with the principal; the gate puts them on the `Holding`.
- `can` checks them as §7's third step: an action not in `only`, or a flow `flows` doesn't
  select, is refused ("ci's token may not do that: it is narrowed to flow.deploy on payments").
  Never widening: it is the intersection, so a token is never more than its principal.
- Tests: the model test gains a narrowed token; the access table is unchanged (no token there is
  narrowed).

## D. Decisions

These are for step 3 itself (step 2's plan has D1-D11):

- **D12. Where a role lives.** The design's `Principal` (§3) has no role: what a principal may do
  is its bindings.
  - **Recommended**: as the design says. `Principal.role` goes, and one role becomes a binding
    at the host (3a). `role` in `/auth/me` and `GET /people` is derived from the bindings, for
    the editor until step 7.
  - Otherwise `role` stays on the principal as its host role, and bindings add to it. That is two
    places to look, and two histories: the accounts book has none.
- **D13. An accounts file from before 3a.**
  - **Recommended**: its roles are not read (dev phase: no compatibility). The server says
    which principals hold nothing, and the run file's admin gives them roles
    (`holon people set NAME --role admin`, then the rest). Step 1's precedent did the same for a
    person with role agent. e2e and the tests start from no files.
  - Otherwise the first open makes version 1 from the file's roles: a one-time read of an old
    field, kept until it is removed.
- **D14. Where the access document is kept.**
  - **Recommended**: `access.json` beside `accounts.json`, with no flag of its own, and its
    versions whole, one per line, in `access.versions.jsonl`.
    - Kept apart from the accounts book, the access history holds no credentials, so an
      auditor can be given it.
    - Whole versions are simple to read back and small (tens of bindings).
  - Otherwise in `accounts.json`: one file and one lock, but its history would sit with the
    password hashes. Or versions as diffs: smaller, and harder to read back. Later, if a host's
    history grows past a few MB.
- **D15. No one locks the host out** (§6).
  - **Recommended**: some enabled person holds `admin` at the host with no expiry, by a binding
    to them or to a group they are in. Every change to access is checked, and so are disabling
    and removing a person. Refused in step 1's words.
    - An admin binding that expires doesn't count: its expiry would lock the host out with no
      change to check.
    - The run file's admin doesn't count: it is there only while its OS user runs the server.
  - Before setup (no person) there is nothing to keep, as today.
- **D16. Refusal words.**
  - **Recommended**: step 2's words when the roles that cover the resource are all built-in (so
    every caller in the access table reads as before). Otherwise §7's:
    `bob may not do that: no role bob holds on "payout" holds flow.deploy`. The action's
    catalogue name is in the words, so the admin knows what to bind.
  - Otherwise new words for every refusal, with every row of the table changed in one commit.
- **D17. Lists across flows.** The design doesn't say how a list answers a caller bound at some
  flows only.
  - **Recommended**: the gate lets through whoever holds the action at some scope, and the list
    keeps what they may view. A flow they may not view is absent, as if it weren't there (a
    proposal of one is 404). A caller at the host sees what they see today.
  - Otherwise lists stay a host action: someone bound only at flows reads them one by one, which
    the editor can't.
- **D18. Saved flows and scopes.** A saved flow isn't deployed, so it has no tags.
  - **Recommended**:
    - a saved flow is covered by the host, by a binding on its name, and by a tag its deployment
      of the same name carries;
    - one no one deployed, saved by the caller, is also the caller's own wherever they hold
      `flow.edit`, as the agent role's own flows are today (`Own`, `can.rs:177-178`). So bob,
      `editor` on `payments` only, drafts a new flow and deploys it with the tag.
  - Otherwise tags in a saved flow's text count before it is deployed. That is unreviewed text
    deciding who may act.
- **D19. Escalation.** §6's governed changes (step 8) put approval in front of access changes;
  until then, `people.assign` and `role.define` are trusted.
  - **Recommended**:
    - a binding gives only what its author holds at its scope;
    - a role is defined with only what its author holds at the host;
    - a role holding a host action is bound only at the host.

    An admin passes all three. A custom role with `people.assign` then can't make an admin.
  - Otherwise any holder of `people.assign` may bind anything, `admin` included, until step 8.
- **D20. Agents and a person's actions.** §5.1 lets an admin define a role giving an agent
  `flow.approve-deploy`. The agent surface (`agent-surface-L4.md`) says approving, granting,
  starting, stopping and deleting are a person's.
  - **Recommended**: a fixed rule keyed on the kind (as D11 is), which no role overrides:
    - an agent never approves a deploy, starts, stops or deletes a flow, restores one but as a
      dry run, or writes or rolls back a secret;
    - nor holds the host's people, roles, resources or operations.

    That is the first outside user's pitch, agent governance, held by the host rather than by
    each role an admin writes. Step 5's agent slots are `run.decide`, which this rule doesn't
    name.
  - Otherwise as §5.1, with §8.4's `approvals.agents` setting as the only guard (and it covers
    approval slots, not deploys).
- **D21. Narrowed tokens (§5.4).**
  - **Recommended**: after step 3, as 3e or a step of its own when a CI token needs one. Step 3
    is already five commits. Nothing in steps 4-9 depends on narrowing. Payment ops' first need
    is the approval chain.
  - Otherwise in step 3, as 3e.
- **D22. Deploying across scopes.**
  - **Recommended**: as 3d says: the caller holds the action on the flow as it is deployed now and
    as the version would make it. Neither taking a flow over by tagging it nor tagging a flow
    out of reach is open to someone bound by tag.
  - Otherwise only as it is now: a binding on a tag would let its holder tag any flow they hold
    by slug into it, and then act on it there.
- **D23. Tags in the unit.** A flow option is either part of the server unit (`port`, `taint`) or
  a field of the deploy request (`browsers`).
  - **Recommended**: in the unit, a frozen `tags` param written only when there are tags.
    - The text is the source: the server reads tags from the unit and never takes the client's
      word for them.
    - A tagged version's hash differs, so a review sees the change.
    - Untagged flows keep their hashes.
  - Otherwise a deploy-request field kept per version, as `browsers` is. That is simpler in
    `holon-author` and the wasm bridge, but the server takes tags from the client.
- **D24. Tag rows in the review.**
  - **Recommended**:
    - each tag is a `Tag` item, approved like any item, which the policy never grants;
    - its row says who gains what by it now (§9's sentence).

    §14 puts that sentence in step 6, but a tag row without it is reviewed blind, and it is a
    lookup in the access document. Step 6 keeps the approval points' rows.
  - Otherwise 3d shows only "joins payments", and step 6 adds who.
- **D25. What a binding at an approval point lets its holder see.** §5.3: deciding that point
  only. To decide, a person finds the run (its id, the payout's amount), which is `run.view`.
  - **Recommended**: keep §5.3. Seeing is bound on purpose: `holon bind --approval` says when the
    subject can't view the flow's runs, and step 7's Bind form offers to bind `viewer` with it.
  - Otherwise an approval binding also lets its holder view the runs waiting at that point:
    one binding instead of two, and `GET …/runs` filtered by point.
- **D26. Expired bindings.**
  - **Recommended**: kept in the document and ignored by `can` from their `expires_at`. Listed
    with `expired: true`, removed by `unbind`. Nothing sweeps them: the history keeps them
    anyway.
- **D27. The selector on the command line.**
  - **Recommended**: `--flow SLUG` and `--tag TAG`, where §11 writes `--flows SEL`. A slug and a
    tag may be spelled alike.
- **D28. Removing a role or a group that is bound.**
  - **Recommended**: refused (409), naming how many bindings hold it: "payout-approver is bound
    3 times: unbind them first". Each unbinding is then a change of its own, in the history.
  - Otherwise removing it removes its bindings in the same version.

### Step 2's decisions D6-D11

- **D6 (viewers and the host's pages)** and **D7 (editors' `resource.view`)**: settled as step 2
  recommended. The built-in roles keep today. A custom role gives those views to whoever needs
  them: an `auditor` holding `people.view`, `host.view` and `resource.view`, bound at the host.
- **D8 (evidence)**: settled in step 2. Viewers export runs.
- **D9 (deleting others' flows)**: settled. The built-in editor keeps deleting any flow it is
  bound on. Scopes limit which flows those are, and a custom role without `flow.delete` (a
  "deployer") is how a host keeps deleting from someone. There is no owner of a flow to add.
- **D10 (reloading the policy)**: settled as step 2 recommended. `policy.edit` stays an editor's,
  and a custom role may leave it out.
- **D11 (agents' log lines by kind)**: done in 3.0, which 3a needs.

### The design's §15

- **Q1. Tags or folders.** Recommended: tags, as this plan builds them. They are in the flow's
  text, so the author sets them, a reviewer approves them, and the history keeps them (D23,
  D24). A folder would be set by whoever manages the host, outside the review.
- **Q2. The short form `people: 2`.** Recommended: anyone holding `run.decide` at a scope
  covering the flow, a binding at that approval point included. That keeps step 2's meaning,
  where an editor decides.
  - A role named in the approval is the long form (step 4).
  - Step 6's review says "not ready" when fewer than two people hold it there.
  - Requiring a role in every approval would make every host define one before its first
    approval.
- **Q4. Editors and `run.decide`.** Recommended: the built-in editor keeps it (today's rule,
  pinned by the access table). A host that wants deciding granted only on purpose binds a custom
  role without it in place of `editor`, which step 3 makes possible.
