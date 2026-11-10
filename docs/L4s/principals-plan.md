# Principals: implementation plan

**Companion to** `access-and-approvals-L4.md`, the design. This file says how to build its §14 step 1,
principals, and what that step leaves for step 2. Each sub-step names the files, functions and lines
it changes at `25d6c7b`, sketches the new code, lists the tests it changes and adds, and gives the
checks to run. Code here is a sketch of shape and names, not final code. Where the design leaves a
decision open, or the code raises one the design doesn't, the sub-step names it (§D).

## 0. Conventions for every step

- **One commit per sub-step** (`CLAUDE.md`), each ending with the `Co-Authored-By` line. Each one
  leaves every check green on its own, so a route and every client that calls it change in the same
  commit: the server, `holon` commands, the TUI, the editor and the e2e tests.
- **Checks.** These are the commands CI runs (`.github/workflows/ci.yml`, `e2e.yml`):
  - every sub-step: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`;
  - a sub-step that touches `editor/`: in `editor/`, `npx tsc --noEmit -p .`, `npx vitest run` and
    `npm run build`, then `cargo test -p holon --features embed-editor editor`;
  - both sub-steps change routes that the editor or `holon mcp` calls, so each also runs
    `npm run e2e` locally with `HOLON_E2E_SLOT=3`. Its pull request gets the `e2e` label, since
    e2e.yml runs the suite on a pull request only when it is labelled.
  - Not needed: `wasm-pack test`, a wasm rebuild, or blessing the manual's leaves. No `holon-core`,
    `holon-author` or catalog code changes.
- **Run live.** 1b runs against a real server built from the repo (`target/release/holon.exe`; the
  installed one lags), with `holon mcp` from Claude Code. The agent deploys a flow, is refused an
  approval, and the flow's history shows the agent's name. The commit says it was run.
- **No migration** (dev phase). An accounts file keeps its people, since `kind` defaults to person.
  API tokens made before step 1, and accounts with role agent, are not read (§D5). No step adds a
  reader for the old shapes. The CHANGELOG says to make them again.
- **Sample flow.** Step 1 adds no language feature, node or host capability that a flow uses. It
  changes who calls. The design's access sample is step 9 (`approved-payout` on the three-slot
  chain). In step 1, the e2e tests in 1b cover an agent principal through the real editor and
  server. The commit says why there is no sample.
- **Tokens are never printed**: not in tests, logs or command output, except `holon people token`'s
  one stdout line, as `holon token add` prints today.

| Sub-step | Change | Where | On disk | Decisions |
|---|---|---|---|---|
| 1a | Principals with a kind and an owner. Tokens belong to agents and services. `/people` routes, with `holon people`, the TUI and the editor following. The decider asks for a person. | `accounts.rs`, `access.rs`, `auth.rs`, `durable.rs`, `http.rs`, `accountscmd.rs`, `main.rs`, `workbench/admin.rs`, `editor/`, docs | accounts file: `kind`, `owner`, `display`; `credentials` replaces `tokens` | D1, D3, D4, D5, D6 |
| 1b | `holon mcp` signs in as an agent principal. `X-Holon-Role` goes. | `access.rs`, `durable.rs`, `mcp.rs`, `mcphosts.rs`, `local.rs`, test helpers, `editor/e2e`, docs | `~/.holon/tokens.json` may also hold `agent:<URL>` | D2, D7 |

## 1. What is there at `25d6c7b`

All paths are under `crates/holon/src/` unless they start with `editor/` or `docs/`.

- **`accounts.rs`**
  - `Account { name, role, password, email, disabled, created_at, created_by, prefs }` (:41-59).
  - `ApiToken { id, label, role, hash, created_at, created_by }` (:79-87).
  - `Book { users, invites, sessions, tokens, oidc }` (:97-108), and `Who { name, role }` (:112-115).
  - `valid_name` (:173-179) allows `[A-Za-z0-9._-@]`, at most 64. Its `token:` refusal is dead: `:`
    is not an allowed character.
  - `who()` (:494-508):
    - `hs_…` is a session's account, refused if the account is disabled;
    - `ht_…` is `token:<label>` with the token's own role. There is nothing to disable but the
      token itself.
  - `local()` (:487-491) is `local:<user>` with role Admin.
  - `invite` (:566), `reset` (:582) and `accept` (:601).
  - `set_role` (:630), `set_disabled` (:640) and `remove` (:660), each run inside
    `keeping_an_admin` (:619).
  - `create_token(label, role, by)` (:681) and `revoke_token(id)` (:696).
  - Tests: :738-982.
  - A file that doesn't parse stops the server from starting (`open`, :321-335).
- **`access.rs`**
  - `Role` (:44) and `Caller { name, role }` (:55).
  - `check` (:114-153):
    1. With no accounts, the server is open, and `X-Holon-Role: agent` makes the caller
       `Caller { "agent", Agent }` (:117-119).
    2. The local token is tried first, then `signed_in` (:125-128).
    3. An editor or an admin who sends the header becomes `<name> (agent)` with role Agent
       (:129-133).
    4. Then the role table (:134-141): admin anything; anyone their own routes (`is_own`, :100);
       `/admin/` refused to the rest; viewer GET only; editor all but secret writes; agent
       `agent_may` (:79-92).
  - Tests :218-346. The header cases are at :298-315.
- **`auth.rs`**
  - `route` answers only `/auth/` and `/admin/` (:43).
  - `/auth/me` (:149-153) answers `{name, role, email}` for an account in the book. For tokens and
    the local admin it answers `{name: by, role}`.
  - `/admin/users…` (:181-195, :233-273) and `/admin/tokens…` (:196-208, :224-232). Each change
    writes an activity line.
  - `shown` (:28-34).
- **What reads the caller.**
  - `http.rs:123-124`: `let by = caller.name…; let agent = caller.role == Some(Role::Agent)`. `agent`
    is passed to deploy (:180), flows (:187), restore and the others (:231, :326, :385).
  - `deploy.rs` (:91-116, :434 `if !agent { proposals.approved }`, :1004, :1282), `flows.rs:54-60`,
    `logs.rs:365`.
  - All of these key on the role, and step 1 keeps the role. Only `durable::decider` (:886-899)
    reads the name's form (`token:`, `local:`), so only it changes.
- **`holon mcp`**
  - `Http::send` sets `X-Holon-Role: agent` on every request (`mcp.rs:88-91`).
  - `connect` (:1385-1388) signs in with the token from `local::target(server)` (`local.rs:171-177`,
    `target_from` :234-271), tried in this order:
    1. `HOLON_TOKEN`;
    2. with `--server`, the token kept for that URL in `~/.holon/tokens.json`. It is deliberately
       chosen over the run file's (tested at `local.rs:317-319`);
    3. the run file's admin token.
  - `holon mcp setup --server URL` keeps a token only for a server not on this machine
    (`mcphosts.rs:697-707`). `keep_token_for` (:497-526) asks for role agent, and only warns when
    the token has another role (:516-519).
- **Clients of the admin routes.**
  - `holon user` and `holon token`: `accountscmd.rs:110-293`, `main.rs:734-736`.
  - The TUI: `workbench/admin.rs` :130, :164-185, :344, :360, :461-477, :545-556, :602.
  - The editor:
    - `editor/src/server.ts:25` (`Role`), :95 (`whoAmI`) and :117-133;
    - `AuthGate.tsx:13` (`Me`);
    - `decisions.ts:17-21` (`deciderOf`);
    - `HostPages.tsx` `PeoplePage` (:312-526): `ROLES` :85, the tokens section :420-480, and the
      agents section :500-524.
  - Tests:
    - `access.rs` :242-316;
    - `tests.rs` :2245-2270 (approvals), :2316-2328 (`only_a_person_decides_an_approval`) and
      :4783-4830 (`accounts_through_the_routes`);
    - `mcp.rs:1633`;
    - `editor/e2e/accounts.e2e.ts:84-108`.
- **Uses of the header.**
  - `tests.rs:69-72` (`agent_req`), used at :726 and by `mcp.rs`'s `InProcess` (:1411-1427). Every
    `tool(` call goes through it: 32 calls in 18 tests built on `test_state()`.
  - `mcp.rs`'s `Signed { agent }` (:1564-1585): 12 constructions.
  - `tests.rs:2254`.
  - e2e: `cli.e2e.ts:159`, `samples.e2e.ts:2856`, `system.e2e.ts:117`.
  - `agent.e2e.ts:17-20` runs `holon mcp --server PORT` on the run file's admin token.

## Step 1a. Principals

**Goal.** Every signed-in caller is a principal with a kind. An API token is a credential of an
agent or a service. The routes and their clients speak of people, agents and services. For a person
nothing changes. `holon mcp` still sends the header, and 1b removes it.

### `accounts.rs`

```rust
/// What a principal is (access-and-approvals-L4.md §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Person,
    Agent,
    Service,
}

impl Kind {
    /// The built-in roles it may hold (design §5.1): a person any but agent, a service viewer or
    /// editor, an agent viewer or agent. Step 2 moves this into the roles' `kinds`.
    pub(crate) fn may_hold(self, role: Role) -> bool {
        match self {
            Kind::Person => role != Role::Agent,
            Kind::Service => matches!(role, Role::Viewer | Role::Editor),
            Kind::Agent => matches!(role, Role::Viewer | Role::Agent),
        }
    }
}
```

- **`Account` becomes `Principal`** (:41-59). It gains `kind` (`#[serde(default)]`), `display` and
  `owner`, both `Option<String>`. `role` stays until step 3's bindings. `password` and `email` stay,
  and only a person has them. Every `Account` in `auth.rs` and `workbench/admin.rs` is renamed.
- **`ApiToken`** (:79-87) loses `role` and gains `principal: String`, the agent or service it signs
  in as.
- **`Book`** (:97-108): its `tokens` field becomes `credentials`. The old key's entries are not read,
  since `Book` doesn't deny unknown fields, and the next save drops them (§D5). `users` keeps its
  key, so a file keeps its people.
- **`Who`** (:112-115) gains `kind`.
- **`valid_name`** (:173): drop the dead `token:` check and keep `admin-token`. Names are unique
  across kinds as they are today: every kind lives in `users`, and `invite` (:569) already checks
  the list.
- **`who()`** (:503-506) finds the token's principal, and refuses it if it is disabled:
  ```rust
  if token.starts_with("ht_") {
      let t = book.credentials.iter().find(|t| same(&t.hash, &hash))?;
      let p = book.users.iter().find(|p| p.name == t.principal && !p.disabled)?;
      return Some(Who { name: p.name.clone(), kind: p.kind, role: p.role });
  }
  ```
  The session branch (:499-501) adds `kind`.
- **`local()`** (:490): `Who { name: "local:<user>", kind: Kind::Person, role: Role::Admin }`.
- **`by_email`** (:511-515) matches persons only.
- **New: `add(name, kind, role, owner, by)`**, for an agent or a service (a person comes through
  `invite`). It checks, in order:
  1. `valid_name`, and that the name is unique;
  2. `kind.may_hold(role)`, else "a service may be a viewer or an editor";
  3. that the owner is a person in the book who isn't disabled, or the local user (§D3);
  4. that the principal is not a person ("a person is invited: …").
- **Changed checks:**
  - `invite` (:566) checks `Kind::Person.may_hold(role)`, so an invite with role agent is refused.
  - `reset` (:582) refuses anything but a person: a reset would give an agent a password and a
    session.
  - `set_role` (:630) checks `may_hold` for the principal's kind.
- **New:** `set_owner(name, owner)`, with the same check as `add`, and `set_display`.
- **`remove`** (:660-674) also drops the principal's credentials. It refuses a person who still owns
  principals (§D6):
  ```rust
  let owns: Vec<_> = book.users.iter().filter(|p| p.owner.as_deref() == Some(name)).map(|p| p.name.clone()).collect();
  if !owns.is_empty() { return Err(format!("{name} owns {}: give them another owner first", owns.join(", "))) }
  ```
- **`tokens()` (:676)** becomes `tokens(of: &str)`.
- **`create_token`** (:681) becomes `create_token(principal, label, by)`, refused unless the
  principal is an agent or a service.
- **`revoke_token`** (:696) becomes `revoke_token(principal, id)`, refused unless the id is that
  principal's.
- **`keeping_an_admin`** (:619) is unchanged, since only a person may be an admin.

### `access.rs`

- **`Caller`** (:55): add `pub(crate) kind: Option<Kind>`. `check` (:152) fills it from `Who`.
- **The role table** (:134-141): `/people` is refused like `/admin/`, but a person may manage the
  tokens of a principal they own (design §3). `auth.rs` checks the ownership.
  ```rust
  _ if who.kind == Kind::Person && is_tokens_of_someone(method, &url) => true,
  _ if url.starts_with("/admin/") || url == "/people" || url.starts_with("/people/") => false,
  ```
  `is_tokens_of_someone` matches `GET|POST /people/<name>/tokens` and
  `DELETE /people/<name>/tokens/<id>`.
- **The 403 text** (:143-144): `/people` also gives "that needs an admin".
- **The header** (:129-133) still lowers a role in 1a, and its `Who` keeps the sender's kind.
  `decider` keeps refusing it by role (below). 1b deletes it.

### `durable.rs` (`decider`, :886-899)

The kind decides, not the name's form:

```rust
pub(crate) fn decider(caller: &access::Caller) -> Result<String, String> {
    if caller.role == Some(access::Role::Viewer) {
        return Err("a viewer only reads: an editor or an admin decides an approval".into());
    }
    let Some(name) = caller.name.as_deref() else {
        return Err("deciding an approval needs a person signed in to Holon, and this server has no accounts".into());
    };
    match caller.kind {
        Some(Kind::Agent) => Err(format!("{name} is an agent: deciding an approval is a person's")),
        Some(Kind::Service) => Err(format!("{name} is a service, a script, not a person: sign in to decide an approval")),
        // A person sending X-Holon-Role: agent, until 1b removes it.
        _ if caller.role == Some(access::Role::Agent) => Err("an agent may not decide an approval: deciding is a person's".into()),
        _ => Ok(name.strip_prefix("local:").unwrap_or(name).to_string()),
    }
}
```

### `auth.rs`

- **`route`** (:43) also answers `/people`.
- **The new routes** replace `/admin/users…` (:181-195, :233-273) and `/admin/tokens…` (:196-208,
  :224-232). The shapes follow design §10; groups and bindings wait for step 3.

| Route | Body | Answer | Activity line |
|---|---|---|---|
| `GET /people` | | each principal from `shown`, with an agent's or service's tokens inline | |
| `POST /people` | `{name, kind, role, owner?, email?, display?}` | a person: `{name, invite}`; else the principal | "invited" / "added an agent" / "added a service" |
| `PATCH /people/<name>` | `{role?, disabled?, email?, display?, owner?}` | the principal | as today ("made editor", "disabled"), and "gave an owner to" |
| `POST /people/<name>/reset` | | `{name, invite}`, persons only | "reset the password of" |
| `DELETE /people/<name>` | | 204 | "removed" |
| `GET /people/<name>/tokens` | | `[entry]` | |
| `POST /people/<name>/tokens` | `{label}` | `{token, entry}` | "made a token for" |
| `DELETE /people/<name>/tokens/<id>` | | 204 | "revoked a token of" |

- **Who may use the token routes:** an admin, or the principal's owner. The owner is matched
  against `by`, with `local:` stripped as `decider` does. Anyone else gets a 403.
- **`shown`** (:28-34) adds `kind`, `display`, `owner` and `owner_disabled` (the People page flags
  "its owner is disabled", §3). For an agent or a service it also adds `tokens`, without hashes, so
  the editor and the TUI need one call.
- **`/auth/me`** (:149-153) answers `{name, kind, role, email, owner}`. Tokens now resolve to a
  principal in the book, so the fallback branch (:152) is left only for `local:<user>`, which
  answers `kind: "person"`.

### `http.rs`

- :73: CORS `Access-Control-Allow-Methods` gains `PATCH`.

### `holon people`: `main.rs`, `accountscmd.rs`

The routes and the command move together (§D1). `holon user` and `holon token` (`main.rs:734-736`)
become one command, `holon people`, in the form of design §11. Narrowed tokens (`--only`, `--flows`)
wait for step 3.

```
holon people [list]
holon people add NAME --person --role viewer|editor|admin [--email ADDRESS]   an invite link, as `user invite` gives
holon people add NAME --agent [--role agent|viewer] [--owner PERSON]           the owner defaults to you
holon people add NAME --service --role viewer|editor [--owner PERSON]
holon people set NAME [--role ROLE] [--disable | --enable] [--email ADDRESS] [--owner PERSON]
holon people reset NAME
holon people remove NAME --yes
holon people token NAME [--label TEXT]   the token once, on stdout, as `token add` prints it
holon people revoke NAME ID
```

- `ROLES` (:84) and `role()` (:86-96) check the role against the kind given.
- `user_lines` (:204) and `token_lines` (:277) merge into one listing, with columns for kind and
  owner and each principal's tokens under it.
- The test at :344 changes to match.

### The TUI: `workbench/admin.rs`

- It lists `/people` once (:130), in place of `/admin/users` and `/admin/tokens`.
- People rows (:164-173) show the kind and the owner.
- Keys rows (:176-185) become each agent's or service's tokens, under it.
- New: a key to add an agent or a service.
- The role question (:360) offers only the roles the principal's kind may hold.
- The calls at :461-477, :545-556 and :602 move to the `/people` paths. A change is a `PATCH`, which
  needs a `patch` method beside the client's `post` and `delete`.

### The editor

- **`server.ts`**:
  - Add `type Kind = "person" | "agent" | "service"`.
  - `whoAmI` (:95) and `Session` (:27) gain `kind` and `owner`.
  - `AccountEntry` (:117) becomes `Principal`, adding `kind`, `display`, `owner`, `owner_disabled`
    and `tokens?`. `TokenEntry` (:118) loses `role` and gains `principal`.
  - The calls (:120-133) become:
    - `listPeople`;
    - `invitePerson`;
    - `addPrincipal(name, kind, role, owner)`;
    - `changePrincipal`, a `PATCH`;
    - `resetPerson` and `removePrincipal`;
    - `createToken(principal, label)` and `revokeToken(principal, id)`.
- **`AuthGate.tsx:13`**: `Me` gains `kind`.
- **`decisions.ts:17-21`** (`deciderOf`): `null` unless `me.kind === "person"` and the role is
  editor or admin. Strip `local:` as today. `decisions.test.ts:17` becomes
  `{ name: "ci", kind: "service", role: "editor" }`.
- **`HostPages.tsx` `PeoplePage`** (:312-526):
  - The People section lists persons. Its role select (:347, :408) offers only a person's roles
    (`ROLES`, :85, without agent).
  - The API tokens section (:420-480) and the agents section (:500-524) become one **Agents and
    services** section. Each row shows the kind, owner, role and tokens, with make and revoke per
    principal, and "its owner is disabled" when it is.
  - Its add form takes a name, a kind, a role and an owner (default: me).
  - The `holon mcp setup claude-code --server …` line (:506-507) stays, and its text says it needs
    an agent's token.
- **`App.tsx`** gates pages on `me.role === "admin"`, which is unchanged.

### Tests in 1a

- **`access.rs`**:
  - `api_tokens_act_with_their_role` (:263) becomes `a_service_acts_with_its_role`. Make `ci` with
    `add("ci", Kind::Service, Role::Editor, Some("ada"), "ada")`, then its token. It acts as `"ci"`,
    not `"token:ci"`.
  - Add: disabling `ci` refuses its token with 401, and so does removing it.
  - `an_agent_proposes_and_calls_and_does_nothing_else` (:272): `claude` becomes an agent principal
    that ada owns. Its name reads `"claude"` (:277, :310), and `/admin/users` (:294) becomes
    `/people`.
  - Add: bob (an editor) manages the tokens of an agent he owns, and is refused another's (403 from
    `auth.rs`).
- **`accounts.rs`**, beside the tests at :738-982:
  - each kind holds only its roles (`invite` with agent, `add` with a service as admin, `set_role`
    across kinds);
  - names are unique across kinds;
  - an agent can't be reset or signed in with a password;
  - an owner must be a person, and an owner can't be removed while they own principals;
  - `who()` refuses a disabled principal's token;
  - `local()` is a person.
  - `disabling_ends_sessions_and_api_tokens_are_shown_once` (:941) and `kept_as_hashes_and_read_back`
    (:969) move to the new shape. Add: a file with the old `tokens` key loads its people and no tokens.
- **`tests.rs`**:
  - `accounts_through_the_routes` (:4783-4830) moves to `/people`.
  - The approvals test (:2245, :2268): `ci` becomes a service, and the expected text is "ci is a
    service, a script, not a person".
  - `only_a_person_decides_an_approval` (:2316-2328) builds callers with a kind. Its cases are
    `claude` (agent), `ci` (service), cy (viewer) and no one. Add: a person whose role is admin
    decides.
- **`mcp.rs:1633`**: `("GET", "/admin/users")` becomes `/people`.
- **`editor/e2e/accounts.e2e.ts:84-108`**:
  - ada adds the service `ci` (editor) and makes its token, and the agent `claude` (owned by ada)
    and makes its token.
  - The assertions stay, with `/admin/users` becoming `/people`.
  - The activity text "made an API token ci · by ada" becomes "made a token for ci · by ada".

### Docs in 1a

- `docs/manual/reference/cli.md`: :34, and :605-629 (`holon user`, `holon token`) become
  `holon people`.
- `docs/manual/reference/server.md`:
  - :30, `accounts.json` holds principals and their tokens;
  - :206, a token acts as its agent or service, not as `token:<label>`.
- `docs/manual/concepts.md:164`, `guides/actions.md:56`, `guides/public-https.md:120` and
  `guides/editor.md:249`: tokens belong to services and agents.
- The editor guide's People page section.
- `docs/engineering/themes/security.md:60`: the names a caller has.
- CHANGELOG: principals; tokens made before are not read; `holon people`.

## Step 1b. `holon mcp` signs in as an agent

**Goal.** An agent is only ever an agent principal with its own token. `X-Holon-Role` is ignored.
The audit says `bma-agent`, not `local:bma (agent)`.

### What the MCP work needs kept (agreed with the session that reworked `holon mcp`)

- **`Connect` runs per tool call, on worker threads.** It is `dyn Fn() -> Result<Box<dyn Api>> +
  Sync`, called by `answer_call` (mcp.rs:1237) for each call. So `agent_target` must be `Sync` and
  cheap, and the `/auth/me` kind check is a shared cache (a `Mutex` or `OnceLock`), not a field of
  one connection.
- **`tool_as` takes `&(dyn Api + Sync)`.** `InProcess` and `Signed` stay `Sync` when they gain a
  token.
- **An agent's deploy that needs approval still becomes a proposal.** `wait_for_decision` polls
  `GET /proposals` as the agent and must still see `decided_by` and `reason`. `holon wait`
  (waitcmd.rs) reads `/proposals` as the person: `/proposals` stays readable by both.
- **The deploy proposal's answer quotes `holon wait {slug} --server …`** with the person's
  `--server`, not the agent's target.
- **Tests on `agent_req`/`Signed`:** `a_waiting_call_is_pinged_around_told_of_and_cancelled` and
  `deployments_waits_for_a_proposal_to_be_decided_and_says_when_it_is_still_waiting` move to the
  agent-token helpers with the rest.
- **An existing setup fails clearly.** A host entry from before (no agent token kept, a person's
  token kept for the URL) is refused with "run holon mcp setup", never signed in as the person.
- **Others' work that sends the header.** The Gatehouse e2e (fa6429a, another session's, not
  merged) decides with `"X-Holon-Role": "agent"` in its `sneakyRun`: tell that sample's owner, so it
  moves to an agent token when 1b lands.
- **Done in 1a already:** `mcphosts::who` returns `(name, kind, role)` from `/auth/me`, and
  `keep_token_for` asks for "an agent's token" and says so when a token is a person's or a
  service's (it still keeps it; 1b refuses it).

### `access.rs`

- Delete:
  - `asks_as_agent` (:94-97);
  - the agent made on an open server (:116-119), which becomes `return Ok(Caller::default())`;
  - the rewrite (:129-133).
- The module notes (:17-20) say instead that an agent is a principal of kind agent signing in with
  its own token, and that `holon mcp` signs in as one.
- Tests (:298-315): the header cases go. Add: an admin who sends `X-Holon-Role: agent` is answered
  as the admin (a grant passes), so the header changes nothing.

### `durable.rs`

`decider` drops the 1a arm for a person sending the header. Only an agent holds role Agent now.

### `local.rs`: where an agent's token is kept

`target_from` keeps its order (:244, tested at :317-319). A token kept for a URL is used by every
`holon … --server URL`. So a local server's agent token is kept under its own key, `agent:<URL>`,
which `target` never reads:

```rust
/// Which server, and the agent token `holon mcp` signs in with: HOLON_TOKEN, else the agent token
/// kept for that server (`agent:<URL>`, then `<URL>` for a server elsewhere, as setup keeps it),
/// else, for a server this user runs, a new agent's (mcphosts::local_agent). Never the run
/// file's admin token.
pub(crate) fn agent_target(given: Option<&str>) -> Result<AgentTarget, String>
```

`AgentTarget` is `{ server, token: Option<String>, run_token: Option<String> }`. The run file's
token is kept only so that `local_agent` can provision, never to sign in. `keep_token` and
`saved_token` (:66-84) are unchanged; the key is a string.

### `mcphosts.rs`

- **New: `local_agent(api, server, user)`**, which provisions an agent, as the run file's admin:
  1. The name is `--agent NAME` from setup, else `<user>-agent` (§D7).
  2. `GET /people`. If the name is missing, `POST /people {name, kind: "agent", role: "agent",
     owner: <user>}`.
  3. `POST /people/<name>/tokens {label: "holon mcp on <machine>"}`.
  4. Keep the token under `agent:<URL>`.
  It takes an `Api`-like sender, so a test drives it through `route` in-process.
- **`who`** (:483-495) returns `(name, kind, role)`, reading `/auth/me`'s new `kind`.
- **`keep_token_for`** (:497-526) asks for "an agent's token". Where today it warns about a token
  with another role (:516-519), it refuses one that isn't an agent's: "This token signs in as ada, a
  person: holon mcp signs in as an agent. An admin adds one for you (owned by you); you make its
  token on the People page."
- **`setup_with`** (:697-707): for a server on this machine, it calls `local_agent` and says "holon
  mcp signs in to http://127.0.0.1:7420 as bma-agent, an agent owned by bma". For a server
  elsewhere it calls `keep_token_for`, as today. With `--remove` it forgets `agent:<URL>` too.
- **The entries** setup writes (`holon mcp [--server URL]`) are unchanged.

### `mcp.rs`

- **`Http::send`** (:88-91): no header, and the comment at :89 goes.
- **`connect`** (:1385-1388) uses `local::agent_target`.
  - When no token is kept and the server is this user's, it provisions through `local_agent` once,
    and again once if the kept token gets a 401 (a new accounts file) (§D2).
  - The first time a server and token are used in the process, it reads `/auth/me`. A kind other
    than agent is refused, with the text above. A `Mutex<HashSet<(String, String)>>` of URL and
    token hash makes this one request per process, not one per tool call. A server without
    accounts answers no kind, and is let be.
- **The module doc** (:5-…): how it signs in.

### Test helpers: `tests.rs`, `mcp.rs`

- **`agent_req`** (`tests.rs:69-72`) goes. New, beside `req`:

```rust
/// Who signs in on a server with accounts: ada, an admin, signed in; claude, an agent ada owns,
/// with a token.
pub(crate) struct People { pub(crate) ada: String, pub(crate) claude: String }

/// Give `state` accounts: ada and claude (above).
pub(crate) fn with_people(state: &mut AppState) -> People { … }

/// `req`, signed in with `token`.
pub(crate) fn req_as(token: &str, method: &str, url: &str, body: Vec<u8>) -> Request {
    req_with(method, url, body, vec![("Authorization".into(), format!("Bearer {token}"))])
}
```

  Today each test builds its signed requests in its own closure: `decide` (:2248), `call` in
  `accounts_through_the_routes` (:4783) and `as_person` (:8302). Those built on `req` take `req_as`.
- **`tests.rs:715-727`** (the logs test): `with_people`. The reads at :724 use `req_as(&p.ada, …)`,
  and the refusal at :726 uses `req_as(&p.claude, …)`.
- **`tests.rs:2248-2270`**: `decide` drops its `agent` flag. "an agent never decides" is claude's
  token (403).
- **`mcp.rs` `InProcess`** (:1411-1427) becomes `InProcess { state, token }`, sending `req_as`.
  - `tool(state, …)` (:1430) takes the agent's token, and so does the test that builds an
    `InProcess` itself (:2266).
  - The 18 tests on `test_state()` call `with_people`. Their 14 plain `req(` and 13 `route(&` calls,
    which set things up as a person, take `req_as(&p.ada, …)`.
- **`mcp.rs` `Signed`** (:1564-1585) loses `agent`. Its 12 constructions with `agent: true` use
  claude's token.
  - `an_agent_with_an_admins_sign_in_only_proposes` (:1593) tested a sign-in that no longer exists.
    It becomes `an_agent_only_proposes`, with claude and ada.
- **Names.** `"ada (agent)"` becomes `"claude"`:
  - in assertions at `mcp.rs` :1601, :1622, :1736, :2064, :2165-2166;
  - in test data at `proposals.rs:312` and `waitcmd.rs:146`, and the comment at `recorder.rs:145`;
  - in editor test data at `approvals.test.ts:27`, :43, :79-84 and `cellChanges.test.ts:40`, :47.
    These are strings, kept current.
- **New tests:**
  - `local.rs`: `agent_target` never returns the run file's token, and reads `agent:<URL>` before
    `<URL>`. `target_from` is unchanged: `holon deploy --server 7420` stays the person even when
    `agent:<URL>` is kept.
  - `mcphosts.rs`: `local_agent` through `route` makes `bma-agent` owned by bma once, and a second
    call reuses it and makes only a token. `keep_token_for`'s check refuses a person's token.

### e2e (`editor/e2e`)

- **`fixtures.ts`**: an `agentToken` fixture. As the admin (`adminToken`), it makes an agent once
  per worker and then a token for it.
- **`cli.e2e.ts:159`**, **`samples.e2e.ts:2856`** and **`system.e2e.ts:117`** use `agentToken` in
  place of the admin's token and the header.
- **`agent.e2e.ts:17-20`**: `holon mcp --server PORT` with `HOLON_HOME` set to a fresh temporary
  folder, so the provisioned token is kept there and not in the user's `~/.holon`. `HOLON_TOKEN`
  stays empty, so the test drives the real provisioning path. :126 expects the agent's name, not
  `"(agent)"`.

### Docs in 1b

- `docs/manual/guides/agents.md`:
  - :40-44, how `holon mcp` signs in: locally an agent is made for you; elsewhere you give an
    agent's token;
  - :74 and :81, "by *bma-agent*".
- `docs/manual/guides/editor.md:73`.
- `docs/manual/reference/server.md`:
  - :18, `tokens.json` also keeps a local server's agent token;
  - delete the header paragraph at :211.
- `docs/manual/reference/cli.md`: :187, :280, :441, :452, :881 (`setup`) and :922.
- `docs/LANGUAGE.md:3006`.
- `docs/PLATFORM_LAYOUT.md:447-448`.
- `docs/engineering/themes/security.md`:
  - :60-61;
  - :301: what replaces "the header is a promise its sender keeps" is that an agent with a shell on
    the server's machine can still read the run file, which is the same caveat, now said of the run
    file alone.
- CHANGELOG.

## Step 2, for what step 1 leaves it

Step 2 (`can` and the catalogue) starts from step 1's end:

- `Caller` has a kind.
- `Kind::may_hold` is the §5.1 table's "Kinds" column.
- What `check` decides is in these places:
  - the role match (`access.rs:134-141`);
  - `agent_may` (:79-92), `is_own` (:100) and `is_secret_write` (:204);
  - the owner rule in `auth.rs`;
  - the checks downstream: `deploy.rs` for `agent` and acknowledge, `flows.rs:54-60` for "its own",
    and `logs.rs:365` for the grant.

Its first commit is the table test the design asks for: every route in `http.rs` and `auth.rs`,
against every built-in role and kind, with what step 1 answers. Its second moves those decisions
into built-in roles as data and a route table, with `Access::check` left only authenticating, and
the table test still passing.

## D. Decisions

- **D1. Routes now.** Step 1 replaces `/admin/users` and `/admin/tokens` with `/people` (design
  §10), still for admins only, and `holon user` and `holon token` with `holon people`.
  **Recommended**: clients change once, and step 3 adds only groups, roles and bindings. Otherwise
  1a reshapes the `/admin/…` routes in place and step 3 moves them, so every client changes twice.
- **D2. A local agent comes into being.** Who provisions it:
  - **Recommended**: `holon mcp setup` provisions it, and `holon mcp` does too, the first time it
    has no token for a server this user runs (and once more after a 401). An existing setup then
    keeps working after the upgrade, and a server on a new port needs nothing.
  - This widens nothing: the run file already makes that process an admin.
  - Otherwise only setup provisions, and `holon mcp` answers each tool "run holon mcp setup" until
    it has.
- **D3. The owner of a local agent** is the OS user's name without `local:` (`bma`). That is the
  person `decider` already treats `local:bma` as (`durable.rs:898`). **Recommended.** The owner check
  in `add` accepts the local user's name even with no person `bma` in the book. Otherwise the owner
  must be a person in the book, and a server whose only admin is the run file can't make an agent.
- **D4. Services can't be admins** (design §5.1). A script that needs admin today
  (`holon token add --role admin`) has only the run file on the server's machine, or a person.
  **Recommended**, as the design says.
- **D5. The accounts file.** It keeps its people. Not read: API tokens (the key changes to
  `credentials`), and accounts with role agent, which are dropped at load with one line on stderr
  naming them. **Recommended**. The alternative refuses to start the server with the old file,
  which strands a set-up server.
- **D6. Removing an owner** is refused while they own principals ("give them another owner
  first"). **Recommended**: it keeps "an agent has a person answerable for it" true. Otherwise it is
  allowed and the People page flags it.
- **D7. A local agent's name** is `<os user>-agent`, and `holon mcp setup --agent NAME` picks
  another (say, one per host). **Recommended**: one agent per person and machine, shared by their
  hosts.
