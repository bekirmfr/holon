# Security

Who may use a Holon server, what a deployed flow may do and reach, how secrets are kept, what the
record proves afterwards, and what none of it defends against. Each guarantee is named with the
code that enforces it. The capabilities themselves are in their own docs:
[secrets](../capabilities/secrets.md), [outbound requests](../capabilities/outbound-requests.md),
[folders](../capabilities/folders.md), [databases](../capabilities/databases.md),
[run records and `holon verify`](../capabilities/run-records-and-verify.md),
[storage](../capabilities/storage.md). The server's modules are mapped in
[holon](../components/holon.md).

## 1. What it is

Two boundaries, held apart. The **control plane** (the editor's API, every `holon` command) lets
in only accounts and tokens, each held to a role. The **data plane** (a deployed flow's own port)
lets anyone in and holds the **flow** instead: it does only what its graph derives and the host
granted, checked again as each effect is performed. Under both, each run's record is a hash chain
whose heads can go to a witness. A unit needs no trust: every unit and value is checked against
its hash as it is read (`Store::get`, [holon-host](../components/holon-host.md)).

## 2. How it works

### 2.1 Accounts, sign-in and roles (accounts.rs, access.rs, can.rs, oidc.rs, local.rs)

- **First run.** A server with no accounts prints a setup code (`new_setup_code`: 8 characters
  from a 32-letter alphabet, `XXXX-XXXX`; or `HOLON_SETUP_CODE`) and answers 403 to all but the
  open paths below until `Accounts::setup` is given it; the code is then cleared. Whoever enters
  it is the first admin. `MAX_FAILURES` (5) wrong codes in a row refuse setting up for
  `LOCKED_FOR` (a minute; `setup_tries`), so the code (about 40 bits) can't be guessed at speed.
- **Passwords**: at least `MIN_PASSWORD` (10) characters, kept as argon2id hashes. `MAX_FAILURES`
  (5) wrong in a row lock the name, for the address they came from (`sign_in_from`: the name and
  the client's address), for `LOCKED_FOR` (a minute); sign-ins in flight count toward it, so
  sending them at once doesn't buy more tries. Tries older than a minute are forgotten once more
  than `TRIES_KEPT` (4,096) are kept (`forget_old_tries`). A name with no account is checked
  against `stand_in_hash`, so a refusal takes as long either way. No more than `CHECKS_AT_ONCE`
  (4) passwords are hashed or checked at once, on as many threads kept for it (`checking`,
  `on_a_checker`); one more waits its turn up to `CHECK_WAIT` (30 s), then is answered busy
  (503). The cap bounds the memory; the wait is long so a busy machine slows sign-ins rather
  than refusing them (2 s turned some away on a loaded two-core CI runner).
- **Requests at once**: the control plane admits `MAX_IN_FLIGHT` (512) being answered at once
  (`Admitted`); one more is answered 503 with `Retry-After: 1`, before any work. A body is read
  by `BODY_DEADLINE` (30 s, as the data plane's `body_timeout`): `read_body`, the only reader
  (a test says so), reads on a thread of its own and answers 408 at the deadline, so a body that
  trickles or stops gives its place back. Rouille hides the socket, so the reader is left to the
  connection and stops at its next read. tiny_http reads a body of 1 KiB or less before the
  request is handed on, holding a thread but no place; slow headers likewise.
- **Tokens** are a prefix and 32 random bytes: sessions `hs_` (`SESSION_SECONDS`, 7 days), invite
  and reset links `hi_` (`INVITE_SECONDS`, 7 days, once), an agent's or a service's tokens `ht_`
  (`credentials`, made by its owner or an admin, shown once), and the local token `hl_`. Only their BLAKE3 hashes are kept (`hash_of`),
  compared in constant time (`same`). A new password ends every other session; disabling or
  removing an account ends its sessions; `keeping_an_admin` refuses a change that leaves no
  enabled admin.
- **An identity provider** (OpenID Connect, optional, set by an admin): an ID token is accepted as
  a bearer only if it is signed with an RSA or EC algorithm (`ALGORITHMS`: never `none`, never
  HMAC) by a key the provider publishes, for this issuer and client id, unexpired (60 s leeway);
  its email, only when the provider says it verified it (`email_verified` true), is then matched
  to an account (`Accounts::by_email`), whose role applies; a token that doesn't say signs no
  one in, and is told why.
- **The OS user running the server** signs in through the run file `local::publish` writes
  (`~/.holon/run/<port>.json`, owner-only): an `hl_` token for this run only. It is the account
  that set the server up, the access document's `local` (`Accounts::tied`, the approval-steps
  plan's D45): that name, what is bound to it, then admin on the host (`access.rs`
  `authenticate`), with `Caller.local` the OS user, which each activity entry it makes says
  (`via`, `activity::Via`). Tied to none (before setup, its account disabled or removed), it is
  `local:<user>`, admin alone, and decides no approval (`durable::decider`; `can::holding` skips
  what no binding of the document holds).
- **Roles** (can.rs): the catalogue of actions (`Action`, the access design's §4) and the
  built-in roles made from it (`BUILTIN`, §5.1): `viewer` reads flows, runs, state, a secret's
  names and the policy, and exports runs; `editor` also saves, deploys and approves, restores,
  runs, pauses, stops, deletes, calls, decides approvals, writes state, reloads the policy and
  checks the logs again; `admin` everything, a flow's secrets and every `/admin/` and `/people`
  path included; `agent` reads and proposes: storing units, previewing and deploying, saving its
  own flows, calling, and a restore that is a dry run or saved as an ask (`ask_restore`). Only an
  agent, a principal of kind agent with its own token, holds `agent`; no header changes a
  caller's role. `holon mcp` signs in as one (`mcp::agent_http`), never with the run file's
  token: on a server the OS user runs, the run file's admin makes theirs
  (`mcphosts::local_agent`), before setup too, which only a person's setup ends
  (`Accounts::needs_setup`).
- **The gate** (`Access::admit`): it finds the request's route in `ROUTES`, one row for each route
  the control plane answers, each saying what it asks. Open without signing in: `OPTIONS`,
  `GET /health`, `/auth/state`, `/auth/config`, `/auth/invite`, and `POST /auth/setup`,
  `/auth/sign-in`, `/auth/invite`. For anyone signed in: their own account. The rest name an
  action and what the path names (the host, the flows, a flow by its slug, a principal), and
  `can` answers. Deny by default: a path no row has is answered 404 once its caller has signed in,
  so a route added to a router without a row is never reached. `can` also holds the rules no role
  decides: an agent replaces only a flow it saved (flows.rs asks it with the saved flow), runs an
  action and reads log lines only where the flow's grant lets agents (`AgentRuns`,
  `AgentReadsLogs`: deploy.rs and logs.rs ask), and never approves or starts what it deploys
  (deploy.rs asks `flow.approve-deploy` and `flow.lifecycle`); a service never decides an
  approval, and an agent takes only a step written for an agent (durable.rs's `Decider`), each
  decision kept with the binding that let it; a person manages the tokens of an agent or a service
  they own.

### 2.2 The request gate (http.rs)

Every control-plane request passes `reject_untrusted` before routing:

- **Host** must name this server: `localhost`, `127.0.0.1` or `[::1]` at its port, or a host given
  with `--allow-host` (`Access::allows_host`). A DNS-rebinding page arrives under its own name.
- **Origin**, when sent, must be `http://localhost` or `http://127.0.0.1` on any port
  (`is_allowed_origin`), or one given with `--allow-origin` (`Access::allows_origin`).
- **Content-Type** of a POST must be `application/json` or `application/octet-stream`, neither
  CORS-safelisted, so a cross-site POST needs a preflight; `with_cors` answers CORS only for an
  allowed origin, allowing the headers `Content-Type` and `Authorization`.
- **Bodies** are capped at `MAX_BODY_BYTES` (8 MiB) by `read_body`.

Sign-in is a bearer token in `Authorization`, never a cookie, so a page a browser opens can't make
a request carry it. The built-in editor's files (editor.rs) are answered before the gate, to GET
only, as any web page is.

A flow's own port (dataplane.rs `answer`) answers a request whose `Host` (or authority) is an
address, `localhost` or a name under it, or a name given with `--allow-host` (`host_allowed`,
`DataLimits::names`), and refuses any other with 421, saying to reach it by its address or to give
the name: a page loaded from a name rebound to this machine can't read a flow's answers as its own
origin's. A request with no `Host` (HTTP/1.0) is answered; a browser always sends one.

### 2.3 What `/admin/` and `/people` require

The `admin` role, for every path under them: no other built-in role holds `resource.*`,
`people.*`, `host.view` or `host.operate`, so `can` refuses any other there ("that needs an
admin"), and so for writing a deployment's secret (`secret.set`, `secret.roll-back`). One
exception: a person reaches the tokens of an agent or a service they own
(`/people/<name>/tokens[/<id>]`, the owner rule in `can`, with the owner the accounts name). Principals (accounts.rs): a `Kind` (person, agent, service) and the roles it may
hold (`may_hold`: a service is never an admin, an agent only agent or viewer), and an owner, a
person, for an agent or a service. The paths:
`/people[/<name>[/reset|/tokens[/<id>]]]` (PATCH changes one), `/admin/oidc` (auth.rs);
`/admin/secrets[/<name>]` (the server's own) and `/admin/secrets/env` (variable names only);
`/admin/grants`; `/admin/folders` and below, with `browse` and `pick` (pick.rs: the OS dialog
only for a request from the machine itself, not one a proxy forwarded); `/admin/databases` and below; `/admin/host`, `/admin/warnings`, `/admin/storage`
(which writes `holon.toml`) and `/admin/store/sweep`. A server built without accounts lets
everything through but deciding an approval, which needs a person (`can` with a caller that has
no role); only the tests build one.

### 2.4 What a flow may do: grants and approvals

permissions.rs: the **manifest** is what the graph derives (capabilities can't be understated)
plus what its author asks beyond the defaults; the host's **policy** (`holon.toml`, `Policy`) says
each limit's default and ceiling, what is granted without asking (`[grants] auto`) and which
exceptions are ever granted. At deploy (deploy.rs):

1. `Policy::refuses` turns down a limit over its ceiling, `PlainHttp("host")` unless
   `grants.allow_plain_http`, `PrivateAddresses` unless `grants.allow_private_addresses`, and
   `Unrecorded("…")` when `grants.allow_unrecorded` is false (on by default); so do a folder or
   database it needs that isn't bound, or is bound read-only when it writes (`Scoped::refuses`),
   and an admin's cap on a server secret (`SecretStore::refuses`).
2. `Policy::needs_approval` lists the rest, less `auto`, limits at or below their default, and
   what the previous grant held. Each must be in the request's `acknowledge`, or the deploy is
   refused (400). An agent's `acknowledge` is refused (403), and so is its deploy while anything
   needs approval; when an agent changes the code, every `SecretRead` needs a person again, or is
   withheld if optional (`approval_needed`), an `Unrecorded` whose handler's code changed is
   withheld (its runs logged) until a person grants it back, and a new `Browser(origin)` needs a
   person too, as does a taint key the deployment has and the version drops (`Untaint(key)`,
   `deploy::untainted`): the values under it would be recorded, where an agent may read them.
3. The `Grant` is kept with the deployment; the **`Permit`** applies it to the policy in force
   (a ceiling lowered since wins; an exception holds only while the policy still allows it).

At perform time the permit is the rule, whatever the graph says: `run_attempt` drops every
effect `Permit::allows` doesn't cover ("refused … not granted to this deployment"), and an answer
carrying a secret goes to the caller only under `SecretAnswer("name")` (listeners.rs). A grant
can be changed while deployed (grants.rs): a revoke that leaves the manifest uncovered pauses the
deployment (`attention`) until it is granted back; load rechecks every grant.

**What a person reads is what deploys.** A document carries its code twice: a canvas, which is
what deploys, and a text (`source`), which is what a person reads, and whoever saves or proposes a
document writes both (an agent's token can post any JSON). So a document's text is shown only
when it compiles to the unit its canvas deploys as, and its canvas printed otherwise
(`holon_author::agent::text_of`): the review of a proposal and the diff beside a deploy in the
editor (through the bridge, `document_text`), `holon flows`, `holon history`, a deploy's diff, the
agent's own tools and the terminal editor all read it there. Before, each showed the saved text
as it came, so a proposal could show one program and deploy another within the permissions shown.
The permissions were never in question: they are derived from the unit that deploys.

### 2.5 Outbound requests (outbound.rs)

`Outbound::send` holds every `http_request`, on every redirect too: only granted hosts (`check`:
`blocked_host`, `revoked`), HTTPS with certificate checks unless the host was granted plain HTTP
(`blocked_scheme`), never a private address unless granted (`blocked_address`; `is_private`:
loopback, RFC 1918, link-local and metadata, CGNAT, NAT64 (and its local-use prefix), unique-local,
an IPv4 address inside IPv4-mapped, IPv4-compatible or 6to4 forms judged as itself, Teredo whole,
and the rest it lists),
checked on the addresses DNS returned, in the connection's own resolver, so rebinding can't swap one in.
The host sets `Host`, length and framing headers itself (`refused_header`). A redirect to another
host carries only `CARRIED_ELSEWHERE` headers and is refused if it would resend a body
(`blocked_redirect`). A request carrying a secret's value to a host its approval doesn't show it
going to isn't sent (`folders::SecretSends`: `blocked_secret`).

### 2.6 Secrets (secrets.rs, sealing.rs, secret_sources.rs, taint.rs)

- **Kept sealed**: each value AES-256-GCM under a master key, a fresh nonce each, the slug, name
  and version as associated data (`sealing::context`), so a value moved elsewhere fails to open.
  The key is kept apart from the data (`--secrets-key`, `KeySource`): `os` (the OS key store, the
  default on Windows and macOS), `env:VAR`, or `file:PATH` (owner-only; the default elsewhere, in
  `~/.holon/keys`). The file records a `key_check`, so a wrong key refuses to start rather than open
  an empty store over the real one. A value can instead be a reference (`env:…`, `vault:…`),
  read again every `REFRESH`.
- **Written, never read back**, over the control plane, by an admin: listing gives names,
  versions and sources. At least `MIN_SECRET_LEN` (6) characters; `MAX_VERSIONS` (5) kept; every
  change audited beside the file (`<secrets>.audit.jsonl`), never a value.
- **Never stored**: a state, collection, database, channel or file write carrying a secret's
  value, or anything the graph computes from one (`SecretFed::derives_effect`), or a credential a
  call answered (`answered_tokens`), is refused (`run_attempt`).
- **Redacted**: `Redactor::entry` replaces every literal occurrence in a log entry, current and
  retired values, JSON-escaped forms too, with `‹secret:name v3 #fingerprint›` (a keyed BLAKE3 of
  the value, so which value a run had is told without holding it); what is computed from a secret
  is withheld (`taint::WITHHELD`); a run's own look-alike of a mark is escaped. Control-plane
  replies about a deployment are redacted the same way (`Redactor::json`). A waiting run's
  request is sealed under the same key on disk (`sealed_for_disk`): the vault carries the sealer
  however it is rebuilt (`SecretStore::vault`, `resolve`), and an in-memory store seals under a key
  never kept.
- **What a flow taints is redacted like a secret, not refused**: the flow's `taint` keys (a
  deploy setting, `Settings.taint`, frozen on its server unit by `routes::server_unit`, so the
  unit's hash covers them: `Server.taint`) and every run's
  `ALWAYS_TAINTED` (`authorization`, `proxy-authorization`, `cookie`, `set-cookie`).
  `data::tainted` collects their values from all the run's entry holds (`LogEntry::values_mut`
  and `texts_mut`, the list `Redactor::entry` walks: the request or input, the outputs, the
  effects, the events, the reads, the timeline and a call's rounds) and from its lines: a record's
  field, a header's `[name, value]` pair, a JSON text at any depth, a query's or a form's
  `name=value` (decoded and as sent), with a credential's parts (after its scheme, a Basic
  credential decoded and its password, each cookie's value, a set cookie's), at least
  `MIN_SECRET_LEN` long. `firing::redactor_of` adds `Redactor::tainted` to the run's tokens and
  secrets, so each value is marked `‹taint:key›` wherever it is in the entry, the lines, the packs
  and an unrecorded handler's last failure, a number held as a number included (a JSON text stays
  JSON). `taint:` is a mark kind of its own, so `holon verify` reads it as a placeholder (a node
  computed from one is taken from the log) without counting a forwarded credential as a token
  sent out.

### 2.7 What a flow can reach

Only what its capabilities name, each granted: its own deployment's state and collections (two
deployments never share one: claims.rs); the named folders an admin bound, read or read-write as
bound (`FolderAccess`), by a name, never a path, each file name checked (`bad_file_name`) and
walked to a part at a time (`existing`, `for_writing`): on Linux with no link followed
(`walk_beneath`: `openat` with `O_NOFOLLOW`, a missing folder made with `mkdirat` only in the one
just checked), and acted on through the folder as opened (`Placed`: `/proc/self/fd/<folder>/<file>`),
so a link swapped in after the check leads nowhere; elsewhere each folder on the way is checked to
be inside before one is made in it, and the file once links are followed; the
databases an admin bound, only the tables granted, no DDL (databases.rs); the granted hosts
(above); its secrets by name; other flows' actions (calls.rs, never handed a secret:
`a_secret_is_never_sent_to_another_flow`); the clock, random numbers and its request. A module
runs as WebAssembly with no imports under a fuel and memory budget (holon-module), its budgets
limits with ceilings (`MODULE_FUEL_CEILING`, `MODULE_MEMORY_CEILING`), and answers values only:
an answer holding an effect is refused whole (`run_module_outputs`), so a module performs nothing
and what a flow performs is what its leaves build. No folder or database
may be bound to, or over, the server's own files (`server_files`, `Folders::open`): those it was
given, what it keeps beside the deployments file (`BESIDE_DEPLOYMENTS`: the logs, the state heads,
the arrivals, the heads, the activity log), the secrets' audit, the `.env`, the key's file, and
`~/.holon`.

What a flow is made of is checked before it runs, since its units are anyone's who can stage
them and `POST /unit` only decodes what it stores: a closure nested past 64 units or holding more
than 20,000 is refused, and so is one with a unit core's `validate` refuses (an edge outside its
children's ports, two sources for one port, a cycle, types that don't fit, a module that isn't
one, a family leaf's parameter outside its values), each unit validated as a closure reads it from
the store, not again while the closure cache keeps it (units.rs `load_closure`), and before a
shape is made of it: a shape is laid out by the ports its edges name, and one edge to port four
billion asked for a list of 51 GiB, which ends the server where a panic is answered 500. Core's
shape no longer makes such a list either (`Incoming::room`), for the unit whose far edge goes into
a child nobody staged, which can't be validated against it. A deploy of one is
a 400 saying which unit and why; `holon verify` doesn't run one again (verify.rs `past_bounds`).
The bound is one a run's thread holds: a route nested to it, composites in composites or maps
whose function maps again, answers on a thread of 2 MiB, a runtime's, in a debug build.

What a run computes is bounded where a request's few bytes could make much more, since every
deployment on the server shares its memory and an overflowed stack aborts them all (a panic is
answered 500): a text past 2^24 code units (`texts::MAX_TEXT`: a pad, a repeat, a replace, a
laid-out `json_stringify`) is nothing; JSON is parsed 64 deep; a replacer's output stops at 256
deep; a range is 10,000 long. A value nests at most 256 deep (`MAX_VALUE_DEPTH`), however it is
built: each leaf that puts what it is handed a level down (`list`, `pair`, `cons`, `some`,
`append`, `insert`, `with_field`, `with_item`, `fill`, `entries`) refuses a value already at the
bound, an effect's payload is checked as it is built, and a map's results, a group's lists and a
reviver's answer fit it, so no value a run holds is deeper (primitives.rs `fits`). Only what is
put down is looked at, so a compute flow runs under 2% more instructions for it. What walks a
value recurses on a run thread's 2 MiB: a release server walks about 6,000 levels end to end, a
debug one about 470. A fold that wraps its accumulator gives nothing past 256 steps, where at
10,000 it aborted a release server. A value written to state is one a load reads back, so one nested
past 256 isn't written (`EncodeError::TooDeep`), where it would have failed the server's next
start. What Holon writes as JSON holding values (a log line, a waiting run's part, the deployments
file) it reads back to that depth: serde_json stops at 127 levels, which a value 42 records deep
passes as the log writes it, so `wire::read_json` reads past it once a count of the brackets finds
none deeper than a value at the bound makes (3 × 256 + 16), on a thread of its own with room for
that. A flow call sends data and is answered data: core makes no call whose input holds an
effect, and an answer with no JSON form fails the call (`a_flow_call_sends_and_hands_back_data_only`).

### 2.8 Files on disk (sealing.rs)

`write_private` writes a file aside created with mode `0o600` (a protected owner-and-SYSTEM DACL on
Windows, `restrict_to_owner`), syncs it and renames it over. It keeps the accounts, the deployments
file, the secrets file and key file, the host key, the folders' and databases' bindings, the run
files and `~/.holon/tokens.json`, each state head, and each log as a cut-back rewrites it.
Logs, events and the secrets audit are opened `0o600` when created; sealed log segments go through
`write_aside`. holon-host's `write_atomically` creates its file `0o600` too, so the content store
(units and every state and value), the packs, the kept file texts, the flows file, the policy as
the admin page writes it, the backup's batches and exports and `holon export`'s file are owner-only;
`append_private` opens the activity log and the witness's queue so. The data folder and the
store's folders are made `0o700` (`create_private_dir`); a data folder there already that others
may read is left as it is and said at start (`open_to_others`: "chmod 700 …"), since what it
holds was written before and isn't written again.

### 2.9 The record, the witness and `holon verify` (recorder.rs, heads.rs, verify.rs)

Each log line's `prev` is the BLAKE3 of the line before it (`line_hash`); opening a log reports a
line changed, removed or inserted as a break by seq. `--export-heads URL` (https, or http to this
machine: `Exporter::new`) sends each head, with `trim` heads for cut-backs and `event` heads for the
events' own chain, only after `sync_logs` (`heads::before_send`), its token from
`HOLON_EXPORT_HEADS_TOKEN`, never the command line; no run data leaves. `holon verify` checks an
export offline: the chain, each unit against its hash, each run run again and compared, each
state following from the one before (`check_states`), that each secret and token went only where
the unit sends it, and with `--head` that a head a witness holds is in it.

## 3. Why it is this way

- **Everyone signs in, loopback included**: a loopback connection proves nothing (a web page,
  another user's program, a port forward arrive there); the run file is how the user's own
  commands answer (local.rs).
- **Derived, granted, then held at perform time**: the approval is of what the graph can do; the
  check as each effect is performed makes it hold whatever the graph does
  ([capability model](../../L4s/P2.3.1-capability-model-L4.md), [host capabilities](../../L4s/host-capabilities-L4.md)).
- **An agent proposes; a person approves**, held by the agent's role, not by its tools' good
  behaviour ([agent surface, decision 6](../../L4s/agent-surface-L4.md)).
- **Where a secret goes is shown, not a grant item per pair**: the graph fixes it before the run,
  and a pair item would have paused every flow calling an API with a key
  ([secret destinations §3.1](../../L4s/secret-destinations-L4.md)).
- **The key goes with its file, kept apart from it**: a copy of the data folder carries no
  readable secret, and a moved one still finds its key ([holon home §3](../../L4s/holon-home-L4.md)).
- **No TLS of its own**: HTTPS ends at the operator's ingress; `serve` warns when it listens beyond
  loopback (`plain_http_warning`).

## 4. Reach and limits

What is not defended, or only partly:

- **Anyone who can act as the server's OS user**: they can read the key file (`file:` keys), the
  run file (an admin token), the logs and the store, and rewrite them (local.rs says so: it
  "trusts no more than the machine already does"). An `env:` or `os` key moves that to the
  environment or the key store, not out of reach of the same user.
- **Other users on the machine**: every file the server writes is owner-only, and the folders it
  makes are (§2.8). A data folder made before, or by someone else, open to others is said at start
  and not changed; its older files (a store's values, never written again) keep their mode until
  the admin restricts the folder. Folders inside the data folder (the logs', the state heads') are
  made with the umask, behind the data folder's own `0o700`. On Windows, `write_atomically`'s files
  take the folder's inherited ACL, not `restrict_to_owner`'s.
- **A log rewritten whole**: the chain is an unkeyed hash, so whoever can write the file can write
  a consistent new one; lines before the chain (no `prev`) check only once one with a `prev` is seen.
  Only a witness holding a later head shows it, and only up to the last head sent.
- **Redaction catches a secret's literal value**, as written or JSON-escaped, and what the graph
  shows computed from it; a value transformed where the graph can't see (a far side echoing it
  encoded) isn't recognised (secrets.rs's notes).
- **Taint is found by key**: one under six characters is kept (a CVC); a value found under a
  tainted key is kept out wherever its run has it, but a later run that reads it back under
  another key logs it; a waiting run's sealed request holds it, to resume with; and the flow's
  state, collections, databases and files hold what it writes. Unlike a secret's, a tainted
  value's write isn't refused: it is the caller's, and the flow decides what to keep.
- **What a secret chooses is derived from it**: a true or false computed from it (`startsWith`,
  `===`, `has`), and what a leaf gives when a secret is its condition, position or key (`c ? a :
  b`, `list[i]`, `rec[k]`), is withheld in the log, answered or sent only where approved, and
  never written, as the secret is (taint.rs: `selects`). Only the one-way leaves (`sha256`,
  `hmac_sha256`, `secure_equals`, `jwt_verify`'s key and clock) and a call's answer give out what
  they compute from it as clean. What a run's timing says isn't covered: a flow that does more
  work when a secret's first character is `a` answers later.
- **A value at the bound leaves a debug build's run thread under twice the room it takes**: a
  route walks a list about 6,000 levels deep end to end on 2 MiB in release, 23 times the bound,
  and about 470 in a debug build. Reading a log line back has a thread of its own (8 MiB): about
  18 times what a record at the bound takes in release, 5 in debug. A walker added later that
  takes more stack per level lowers these; the tests below walk lists and records at the bound on
  2 MiB threads in a debug build, so one that takes much more fails them.
- **The data plane has no sign-in**: a flow guards its own routes. A flow on a loopback port is
  reachable by any local program; a web page reaches it only by an address (a cross-origin
  request, which `browsers` decides whose pages may read), never under a name of its own (§2.2).
  A proxy that forwards the name it was reached by needs that name given (`--allow-host`).
- **The control plane's limits under load**: each request still has a thread of its own (rouille's
  default), and a stream's answer, once its handler returns, isn't among the `MAX_IN_FLIGHT`; a
  sign-in's lock is per address, so many addresses each get their tries (5 a minute a name each),
  and behind a proxy it is the name's for everyone; anyone can hold setting up off a minute at a
  time with wrong codes.
- **Approval is one editor's**: any editor or admin may acknowledge anything the policy allows.
- **The run file is the OS user's admin**: an agent with a shell, running as that user, can read
  it and act as them, as the account that set the server up, taking the approval steps bound to
  it, as it can read the data folder and the secrets key. The activity log says each entry it made
  came through the run file (`via`); a host where someone else has that shell moves the tie
  (`holon access local --as`) or leaves the account's steps to another. `holon mcp` never signs
  in with it, but only another OS user, or a server elsewhere, keeps such an agent to its token.
- **A folder's confinement off Linux is checked by names**: on macOS and Windows another process
  writing in the folder can swap a link between the check and the open (on Linux, without
  `/proc`, too). On Linux no link in a folder is followed, even one to inside it.
- **Secrets in memory are in the clear** (`Resolved`, wiped when dropped), as is any process's.
- **Plain HTTP** to the server and data plane unless an ingress ends TLS in front.

## 5. What the tests hold

- **A flow's port**: `a_flows_port_answers_only_an_address_localhost_or_a_name_it_was_given` (by
  function and through a real port), and `a_server_deployment_answers_on_its_own_port_within_limits`
  (a foreign name refused with how to let it in; localhost, `[::1]`, a name under localhost answered).
- **The gate**: `cross_site_and_rebinding_requests_are_refused_before_routing` (seven attacks: a
  text/plain or JSON POST from another site, `Origin: null`, a look-alike origin, a rebinding or
  wrong-port Host, a form POST; none saves anything, the editor and curl still do);
  `oversized_body_is_refused` (one byte past `MAX_BODY_BYTES` is a 413).
- **Roles and accounts**: `each_role_does_what_it_may`, `an_agent_proposes_and_calls_and_does_nothing_else`
  (15 paths refused it; the header lowers, never raises), `a_new_server_answers_only_its_setup`,
  `wrong_passwords_sent_at_once_lock_a_name_as_soon`, `an_unknown_name_is_checked_like_a_wrong_password`,
  `an_admin_is_always_left`, `kept_as_hashes_and_read_back` (no token or password in the file),
  `wrong_setup_codes_lock_setting_up_for_a_while`,
  `wrong_passwords_lock_a_name_for_their_address_only_and_old_tries_are_forgotten`,
  `passwords_are_checked_a_few_at_once_and_the_rest_wait_or_are_told_busy`,
  `the_control_plane_admits_so_many_requests_at_once`,
  `a_token_for_another_client_issuer_expired_or_forged_is_refused`,
  `a_token_the_provider_signed_for_this_client_says_who` and
  `signed_in_through_a_provider_is_matched_by_email` (an email not said to be verified signs no
  one in).
- **Files**: holon-host's `the_store_writes_its_files_and_folders_for_its_owner_only` (a unit, a
  value, their folders, one made above them, a file readable by others written again `0o600`),
  `the_server_keeps_its_files_for_its_owner_and_says_when_its_folder_isnt`.
- **Grants**: `deploy_asks_for_what_policy_doesnt_grant_and_only_once`,
  `an_effect_outside_the_grant_is_refused_at_perform_time`, `revoking_an_item_pauses_until_it_is_granted_back`.
- **Outbound**: `every_rule_is_enforced_at_perform_time` (an ungranted host, plain HTTP, a private
  address, five headers the host sets, credentials in a URL),
  `a_redirect_to_another_host_leaves_the_credentials_behind`, `private_addresses_are_recognised`.
- **Secrets**: `a_secret_reaches_its_runs_and_nothing_else` (listings, history and a rollback's
  dry run never show it; the log shows its mark), `a_secret_computed_into_something_else_stays_out_of_the_log`,
  `a_token_a_call_answers_with_a_secret_is_kept_out_of_the_log_and_out_of_state`,
  `a_secret_goes_only_where_its_approval_shows_it_going`, `a_waiting_runs_record_is_sealed_on_disk`
  (and `0o600`), `a_missing_or_wrong_key_refuses_instead_of_starting_empty`,
  `a_sealed_value_opens_only_where_it_was_sealed` (another slug, name, version or key fails).
- **Tainted values**: `a_tainted_keys_values_never_reach_what_the_host_keeps_wherever_a_run_has_them`
  (a request's every part, a call's request and answer and the cookie it sets, a WebSocket's
  message, an action's input, a row written and read back, every always-tainted key; none in the
  log, the listing, the lines, the export, verify's report or a replay, and the runs check as
  themselves), and `an_unrecorded_failure_is_kept_redacted_and_cut_short` (a tally's failure, of
  secrets, tainted values and resume tokens).
- **Reach**: `a_name_stays_inside_its_folder`, `a_link_out_of_the_folder_is_never_followed`,
  `a_write_through_a_link_makes_nothing_outside_and_links_arent_followed`,
  `a_folder_swapped_for_a_link_after_the_check_leads_nowhere` (a write and a read checked, the
  folder on their way then swapped for a link out: each lands, or reads, where it was checked),
  `no_folder_is_or_holds_what_the_server_keeps_beside_its_files`.
- **Units**: `a_staged_unit_loads_or_is_refused_whatever_its_wiring` (3,000 units drawn: children
  staged or not, edges to and from ports that exist, don't, or are far out, to one port twice,
  round in a cycle, an effect into a sink; each as the root and nested, and what the loader says
  is what `validate` says, every kind of mistake made at least ten times; it replaced a test of
  three wirings picked by hand, and one of an effect wired into a write), core's `the_edge_table_reads_as_a_map_of_its_edges_and_takes_no_more_room` (20,000 edge
  sets: the table the evaluator reads answers every lookup as a plain map would, in the room of
  its edges) and `audit_unvalidated_composites_never_panic_in_eval`, `a_closure_too_big_to_run_is_refused_and_every_sample_is_well_inside`
  (every sample's closure loads, so every sample validates; holon-author's
  `every_samples_units_pass_validation` says so before the server), and `holon verify`'s
  `a_unit_that_doesnt_validate_is_not_run_again` and `a_unit_nested_past_the_servers_bound_is_not_run_again`;
  `the_deepest_route_a_server_runs_fits_a_runtimes_thread` (a route nested to the bound answers on
  a 2 MiB thread through the server's own resolver).
- **What a run computes**: holon-core's `audit_leaves` (every pure leaf over hostile values on a
  2 MiB thread: no panic, finite numbers, the same answer twice, none taking 2 s),
  `a_text_padded_or_repeated_stops_at_the_bound`, `a_laid_out_text_stops_at_the_bound`,
  `a_replacer_nesting_without_end_gives_nothing`, `a_cell_never_holds_what_it_couldnt_load`,
  `a_flow_call_sends_and_hands_back_data_only`.
- **How deep a value nests**: `a_leaf_puts_nothing_past_the_depth_a_value_may_nest` (each leaf
  that puts a value down, at the bound and one short of it, lent and given),
  `no_leaf_builds_a_value_nested_past_the_bound` (every leaf, a value at the bound or one short of
  it in each input), `a_value_built_a_level_at_a_time_stops_at_the_bound` (a fold, a map, a group,
  a reviver), `audit_leaves` (values at the bound in its pool, nothing answered deeper), and
  `a_value_nested_a_level_a_step_stops_at_the_bound_and_the_server_answers_on` (a list and a
  record answered 255 deep, a level down in the response, and the text of one 256 deep; a level
  more, or 10,000, a 422; on a 2 MiB thread through the server).
- **Reading it back**: `json_holding_a_value_as_deep_as_one_may_be_reads_back` (and JSON 100,000
  deep refused before anything recurses), `a_log_holding_a_value_as_deep_as_one_may_be_reopens_whole`,
  and `a_durable_run_waits_resumes_by_token_and_expires` (a waiting run holding one survives a
  restart).
- **The record**: `a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break`,
  `exported_runs_verify_and_a_rewritten_log_doesnt`, `a_batch_goes_only_after_its_hook_has_run`,
  `a_witness_is_reached_over_https_or_on_this_machine`.

Gaps: none named here beyond the limits above.

## 6. History

The history in this clone starts at `e49e371`; accounts, sign-in, the gate and sealing are older.
Since then:

| Commit | Change |
|---|---|
| `2c15b96` | Answering a route's caller with a secret is a permission, `SecretAnswer` |
| `14c7163` | The approval says where each secret is sent |
| `f3b834d` | A redirect to another host doesn't resend the body |
| `21e63e6` | A secret goes only where its approval shows (`blocked_secret`), and verify checks it |
| `a053b40` | Where what a call answers to a secret goes is said, and verified |
| `71b895c` | The server's own secrets, and an admin's cap on where each may go |
| `707928d` | An agent's restore is saved for a person to approve |
| `07d83e4` | `holon deploy --approve` approves, as the person signed in |
| `4bb40a8` | An unrecorded run is never logged, failed or not: a handler may be unrecorded to keep its requests out |
| `cf1f961` | Tripwires on the redaction shortcut (D3) |
| `4c984bb` | An identity provider's email is taken only when it says it verified it |
| `46c44dd` | Wrong setup codes lock setting up for a minute |
| `5d0bdf2` | Every file the server writes is its owner's only; the data folder and the store's folders `0o700`; a data folder open to others said at start |
| `09f9ea6` | No folder may be the server's state heads, arrivals, heads, activity log or secrets' audit, wherever its data is |
| `08f6ccc` | A private address inside 6to4, IPv4-compatible or Teredo (and NAT64's local-use prefix) is private |
| `29c6088` | A waiting run is sealed on disk however its vault was rebuilt, and with secrets in memory |
| `dfd7c8b` | A folder's file reached through its folder as opened, no link followed (Linux); folders made only once checked |
| `3edb446` | A flow's port answers only its address, localhost, or a name given with `--allow-host` |
| `c0a3913` | Passwords checked a few at once on kept threads; a name locked per address; 512 requests at once |
| `a86994a` | The server's notes say who signs in |
| `6470bd0` | The server validates every unit of a closure before it runs one |
| `faf3ef7` | A family leaf's parameter outside its values is refused when the flow is wired |
| `1c1d2ee` | A value is encoded only as deep as it is read back: a state write too deep to load is refused |
| `76558df` | A text a leaf makes stops at 2^24 units; a replace's work is bounded by what it writes |
| `4c88fbf` | A replacer nesting without end, and a laid-out text past 2^24, give nothing |
| `f4f2df7` | `unique` hashes an option by what it holds, so distinct ones don't share a bucket |
| `428c4f5` | A duration ending in a character of more than one byte is refused, not a panic |
| `bd0fa4c` | A value nests at most 256 deep: a leaf that would build one deeper gives nothing, where a fold nesting 10,000 deep aborted the server |
| `eeaab65` | JSON holding a value as deep as one may be reads back: a log line, a waiting run's part, the deployments file |
| `157c1f6` | The depth bound's check costs a compute flow under 2% of its instructions, where it cost about 5% |
