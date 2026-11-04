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

### 2.1 Accounts and sign-in (accounts.rs, access.rs, oidc.rs, local.rs)

- **First run.** A server with no accounts prints a setup code (`new_setup_code`: 8 characters
  from a 32-letter alphabet, `XXXX-XXXX`; or `HOLON_SETUP_CODE`) and answers 403 to all but the
  open paths below until `Accounts::setup` is given it; the code is then cleared. Whoever enters
  it is the first admin. `MAX_FAILURES` (5) wrong codes in a row refuse setting up for
  `LOCKED_FOR` (a minute; `setup_tries`), so the code (about 40 bits) can't be guessed at speed.
- **Passwords**: at least `MIN_PASSWORD` (10) characters, kept as argon2id hashes. `MAX_FAILURES`
  (5) wrong in a row lock the name for `LOCKED_FOR` (a minute); sign-ins in flight count toward it,
  so sending them at once doesn't buy more tries. A name with no account is checked against
  `stand_in_hash`, so a refusal takes as long either way.
- **Tokens** are a prefix and 32 random bytes: sessions `hs_` (`SESSION_SECONDS`, 7 days), invite
  and reset links `hi_` (`INVITE_SECONDS`, 7 days, once), API tokens `ht_` (made by an admin with a
  role, shown once), and the local token `hl_`. Only their BLAKE3 hashes are kept (`hash_of`),
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
  (`~/.holon/run/<port>.json`, owner-only): an `hl_` token, an admin's, for this run only.
- **Roles** (`Access::check`): `viewer` reads (GET); `editor` also saves, deploys and approves,
  runs, pauses, stops, deletes and changes grants; `admin` everything, writing secrets and every
  `/admin/` path included; `agent` reads and proposes (`agent_may`: `POST /unit`,
  `/deploy/preview`, `/deploy`, `/flows`, a deployment's `/request`, an action, a restore, which
  is a dry run or saved as an ask: `ask_restore`). `X-Holon-Role: agent` lowers an editor or admin
  to an agent, named `<name> (agent)`; it never raises a viewer. Open without signing in
  (`is_open`): `OPTIONS`, `GET /health`, `/auth/state`, `/auth/config`, `/auth/setup`,
  `/auth/sign-in`, `/auth/invite`.

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

### 2.3 What `/admin/` requires

The `admin` role, for every path under it: `Access::check` refuses any other role there ("that
needs an admin"), and so for writing a deployment's secret (`is_secret_write`). The paths:
`/admin/users[/<name>[/reset]]`, `/admin/tokens[/<id>]`, `/admin/oidc` (auth.rs);
`/admin/secrets[/<name>]` (the server's own) and `/admin/secrets/env` (variable names only);
`/admin/grants`; `/admin/folders` and below, with `browse` and `pick` (pick.rs: the OS dialog
only for a request from the machine itself, not one a proxy forwarded); `/admin/databases` and below; `/admin/host`, `/admin/warnings`, `/admin/storage`
(which writes `holon.toml`) and `/admin/store/sweep`. A server built without accounts lets
everything through (`Access::check`'s first line); only the tests build one.

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
   withheld if optional (`approval_needed`), and a new `Browser(origin)` needs one too.
3. The `Grant` is kept with the deployment; the **`Permit`** applies it to the policy in force
   (a ceiling lowered since wins; an exception holds only while the policy still allows it).

At perform time the permit is the rule, whatever the graph says: `run_attempt` drops every
effect `Permit::allows` doesn't cover ("refused … not granted to this deployment"), and an answer
carrying a secret goes to the caller only under `SecretAnswer("name")` (listeners.rs). A grant
can be changed while deployed (grants.rs): a revoke that leaves the manifest uncovered pauses the
deployment (`attention`) until it is granted back; load rechecks every grant.

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
  request is sealed under the same key on disk (`sealed_for_disk`).

### 2.7 What a flow can reach

Only what its capabilities name, each granted: its own deployment's state and collections (two
deployments never share one: claims.rs); the named folders an admin bound, read or read-write as
bound (`FolderAccess`), by a name, never a path, each file name checked (`bad_file_name`) and
refused if it leads outside the folder once links are followed (`existing`, `for_writing`); the
databases an admin bound, only the tables granted, no DDL (databases.rs); the granted hosts
(above); its secrets by name; other flows' actions (calls.rs, never handed a secret:
`a_secret_is_never_sent_to_another_flow`); the clock, random numbers and its request. A module
runs as WebAssembly with no imports under a fuel and memory budget (holon-module), its budgets
limits with ceilings (`MODULE_FUEL_CEILING`, `MODULE_MEMORY_CEILING`). No folder or database
may be bound to, or over, the server's own files (`server_files`, `Folders::open`): those it was
given, what it keeps beside the deployments file (`BESIDE_DEPLOYMENTS`: the logs, the state heads,
the arrivals, the heads, the activity log), the secrets' audit, the `.env`, the key's file, and
`~/.holon`.

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
- **The data plane has no sign-in and no Host check**: a flow guards its own routes. A flow on a
  loopback port is reachable by any local program and, through DNS rebinding, by a web page;
  `browsers` decides only which other origins' pages may read its answers.
- **The control plane is not hardened against load**: a thread per request (rouille's default),
  and sign-in throttled per name only (so a known name can be locked out a minute at a time;
  likewise anyone can hold setting up off a minute at a time with wrong codes).
- **Approval is one editor's**: any editor or admin may acknowledge anything the policy allows.
- **`X-Holon-Role` is a promise its sender keeps**: an agent with a shell can read the run file
  and leave it off; an `agent`-role API token is the guarantee (agent-surface L4, decision 6).
- **A folder's confinement is checked, then opened**: another process writing in the folder
  between the two can swap a link, and `for_writing` makes a name's folders before it checks where
  they lead.
- **Secrets in memory are in the clear** (`Resolved`, wiped when dropped), as is any process's.
- **Plain HTTP** to the server and data plane unless an ingress ends TLS in front.

## 5. What the tests hold

- **The gate**: `cross_site_and_rebinding_requests_are_refused_before_routing` (seven attacks: a
  text/plain or JSON POST from another site, `Origin: null`, a look-alike origin, a rebinding or
  wrong-port Host, a form POST; none saves anything, the editor and curl still do);
  `oversized_body_is_refused` (one byte past `MAX_BODY_BYTES` is a 413).
- **Roles and accounts**: `each_role_does_what_it_may`, `an_agent_proposes_and_calls_and_does_nothing_else`
  (15 paths refused it; the header lowers, never raises), `a_new_server_answers_only_its_setup`,
  `wrong_passwords_sent_at_once_lock_a_name_as_soon`, `an_unknown_name_is_checked_like_a_wrong_password`,
  `an_admin_is_always_left`, `kept_as_hashes_and_read_back` (no token or password in the file),
  `wrong_setup_codes_lock_setting_up_for_a_while`,
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
- **Reach**: `a_name_stays_inside_its_folder`, `a_link_out_of_the_folder_is_never_followed`,
  `no_folder_is_or_holds_what_the_server_keeps_beside_its_files`.
- **The record**: `a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break`,
  `exported_runs_verify_and_a_rewritten_log_doesnt`, `a_batch_goes_only_after_its_hook_has_run`,
  `a_witness_is_reached_over_https_or_on_this_machine`.

Gaps: no data-plane request under a foreign `Host` is sent; no link is swapped between a folder's
check and its open.

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
