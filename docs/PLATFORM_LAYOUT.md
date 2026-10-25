# Platform layout: one engine, many front doors

**Status:** living plan (since 2026-10-21). What has landed is tracked in §7; §4 is the design for
what comes next.
**Goal:** split the codebase into a shared **platform** and thin **packs**, so each front door
(`holon-hooks` first, then `holon-agents`, `holon-rules`, …) is a pack plus onboarding on top of the
same engine. Webhooks are the first consumer. Nothing built for them should block the other doors.

---

## 1. The rule

```
packs  ──depend on──►  platform          (never the reverse)
host   ──depends on──► core              (existing rule, unchanged)
wasm   ──depends on──► core              (existing rule, unchanged)
```

- **Nothing vendor-specific or door-specific goes into the platform.** There is no `stripe`, `slack`
  or `github` anywhere in a crate.
- **Packs are data, not code.** A pack is a set of content-addressed units, plus templates, plus a
  manifest. A new integration should never need a Rust change. The only exception is a primitive
  that is genuinely inexpressible or too slow as a composite (e.g. HMAC). That goes into core as a
  **generic** primitive, through the existing admission gate (design §13).

The second rule keeps the genesis trust root small and frozen. It also makes the marketplace idea
come back for free: packs are just published units.

---

## 2. Target layout

```
crates/
  holon-core      pure language: values (+ Record), types, hashing, eval, primitives, validate
  holon-host      runtime: Host, fire, effect loop, state, stores, recorder/replay, manifests and permissions
  holon-market    PARKED: registry, memo, shared cache, reputation, fork (moved out of host)
  holon-wasm      bridge (unchanged role)
  holon-server    lib + bin, split into modules:
                    control/   editor API (units, flows, deploy) on 127.0.0.1:7420
                    data/      the user's servers: listeners on declared ports, routing
                    persist/   atomic JSON persistence
                    scheduler/ clock ticks
  holon-pack      CLI: build a pack dir → hashed unit bundle + manifest; load into a store
packs/
  auth/           shared building blocks: signature verify, bearer/basic checks, secret refs
  http/           shared: request/response helpers, JSON body parse, status helpers
  hooks/          door 1: stripe/github/slack units + 3–5 templates + manifest
  (agents/, rules/ … later)
editor/src/
  canvas/  flows/  deploy/  test-request/  replay/  doors/
```

### Why `holon-host` keeps its name
Renaming it to `holon-runtime` would touch every import and every doc for no functional gain. Moving
the **marketplace** pieces out of it gives the same clarity: host becomes "the runtime", and
`holon-market` holds everything parked.

---

## 3. What moves where (current → target)

### holon-core (stays; additions only)
| Today | Target |
|---|---|
| `value.rs`: Number, Boolean, Text, List, Effect, Sum | **+ `Record(BTreeMap<String, Value>)`**. Keys sorted, so canonical encoding is free. `ENCODING_VERSION` bump. |
| `kind.rs` `Type` | **+ `Type::Record`**. Start with open records (`Record(Any)`), add field-typed records later. |
| `primitives.rs`: arithmetic, text kit, list kit, `const`, `guard`, `default`, `equals`, `to_text`… | **+ record kit** (`record`, `field`, `with_field`, `keys`, `has_field`; keyed by an input: `lookup`, `insert`, `remove`; `find` over a list of records), **+ `json_parse` / `json_stringify`**, **+ `sha256`, `hmac_sha256`, `hex`, `base64`, `secure_equals`** ✅ (admission gate: inexpressible / hot-and-pervasive) |
| `kind.rs` sinks: `egress` | **+ `http_request` sink** (method, url, headers, body), classified `NetworkEgress`. Its response comes back via the effect loop (§4). |
| `memo.rs`, `fork.rs`, `inspect.rs` (adoption), `advisory.rs` | Stay in core: they are pure and cost nothing. They're marked parked in docs only, not moved. |

### holon-host → runtime + `holon-market`
| Today (`holon-host/src/lib.rs`) | Target |
|---|---|
| `Host`, `fire`, `fire_sourced`, `SourcedResolver`, `Mode`, `run` | stay in **host** |
| `StateHost`, `constant_unit` | stay in **host**. Fix: List/Sum/Record read-back, and never panic on an Effect (M0). |
| `Store`, `MemStore`, `FileStore` | stay in **host**. `FileStore::put` becomes atomic. |
| `Registry`, `Memo`, `SharedCache`, `CacheIngestError`, `PublishError`, `ForkError` | move to **`holon-market`** (parked) |
| (new) | **effect loop**: multi-round request handling (§4) |
| (new) | **`Performer` trait**: real effects (HTTP client, secret store). `RecordingHost` becomes a `Performer` used in tests and Design mode. |
| (new) | **`Recorder`**: per-request log of sourced values per round, effects, response. Replay input. |
| (new) | **permissions**: per-deployment grants against each flow's manifest, enforced at perform time and re-checked at load (§4.2.1). |

### holon-server → modules + control/data planes
| Today (`main.rs`) | Target module |
|---|---|
| wire types (`SValue`, `*Body`, `*Entry`, lines 161–505) | `wire.rs` |
| `is_allowed_origin`, `with_cors`, `read_body`, `route` | `control/http.rs`. **+ Host check, required `Content-Type: application/json`, body cap** (M0) |
| unit/registry/fork/cache handlers (553–800) | `control/units.rs`. Registry/cache routes go behind a `market` feature flag. |
| `RecordingHost`, `fire_with_state`, `handle_post_fire` | `host` crate (`Performer`) + `control/fire.rs` |
| `persist_*`, `load_*` (941–1125) | `persist.rs`: temp file → fsync → rename, and refuse to start on a parse error |
| flows handlers (1127–1190) | `control/flows.rs`. Flow documents also store the composites' preimages (fixes reopen bug). |
| deploy/deployments/history/state/trigger (1191–1494) | `control/deploy.rs` |
| clock scheduler (1495–1617) | `scheduler.rs` (deduplicated with the trigger path) |
| (new) | **`data/`**: one listener per deployed server unit, routing method + path → endpoint handler |
| test-unit emitters (1724–1840) + CLI flags | `bin/holon-fixtures.rs` (separate binary). Root `*.bin` / `*.json` move to `fixtures/`. |

**Two planes, two security postures:**
- **Control plane** (`127.0.0.1:7420`, or where `holon serve --at` says): editor-only, localhost, hardened against CSRF and DNS rebinding.
- **Data plane** (user-declared ports): public-facing by intent. It does no CORS magic; each endpoint's
  own logic plus `packs/auth` decides who is allowed.

### editor/src
| Today | Target |
|---|---|
| `App.tsx` (~2200 lines) | ✅ split by concern: `canvasOps.ts` (project, group, expand), `canvasUnits.ts`, `CanvasPanels.tsx`, `BoundaryField.tsx`, `Pages.tsx` (flows, deployed), `useCanvasHistory.ts`, `useDeployments.ts`; `App.tsx` (~1100 lines) keeps the canvas's state, save/open, deploy and view mode |
| (new) | `test-request/`: build a request record, eval the endpoint locally via wasm, show the response |
| (new) | `samples.ts` ✅ (the samples themselves are holon-author's `samples.rs` since the move to Rust): one sample flow per capability (hello API, notes API with state, Stripe → Slack, a clock heartbeat, a durable order approval, fetch-and-reshape, signing), opened from the flows page as an unsaved copy, built from the live catalog and derived through core in `samples.test.ts`. Every new feature ships with a sample that shows it (`CLAUDE.md`). |
| (new) | `replay/`: request log list → open one in the canvas with recorded sourced values → step and inspect |
| (new) | `doors/`: one config per door (title, palette sections, templates, first-run flow). The door is selected by route or subdomain. |
| `holon.ts`, `server.ts`, `diagnostics.ts` | stay. `holon.ts` gains the Record type, `server.ts` gains the request-log API. |

---

## 4. Effects that answer, and runs that last (design, 2026-10-22)

**Status:** built (2026-10-22), steps 1 to 8 of §4.6, each with its tests and a live check. Where
the build settled a detail differently from the design, the section says so; what is left for
later is listed in §4.6.

**The principle all of it rests on:** an evaluation stays a pure, total function. A flow never
blocks, never suspends, never runs open-ended. Waiting, suspending and resuming belong to the host,
and happen *between* evaluations. A run that is "in progress" is just data: what it started from and
what it has learned since. Continuing it means evaluating the same flow again with more of that data.
This is what keeps replay, the timeline and per-step state exact, and it is Temporal's model
(deterministic re-execution against a recorded history), which Holon gets almost for free.

### 4.1 Rounds: an outbound request inside one run
Today a run is one evaluation: sourced values are fixed before eval and effects are performed after
it settles (`fire_sourced_observed`). Webhooks need "verify → fetch → decide → post". A run becomes a
sequence of **rounds**:

```
round 0:  sourced = {request, state, clock, random, run nonce}       → eval
          the http_request nodes whose request is ready have no response yet: their requests
          are collected (deduplicated by node and request value) and performed by the host,
          concurrently within the round
round n:  sourced += {responses from rounds < n, by node}            → eval again from the start
done:     a round with no unanswered request
          → its output is the response, its effects are performed; earlier rounds' outputs and
            effects are discarded (they were computed with responses missing)
bounded:  by the run's limits (§4.2): its wall-clock budget and its outbound-request budget
          (every round past round 0 needs at least one request, so that budget also bounds the
          rounds); past either, the run fails and nothing is performed
```

- **`http_request`** (core; built ✅): frozen params `method` and `host`, inputs `path`, `headers`,
  `body`; output `response`, a record `{status, headers, body}`. Core builds the request
  `{method, url, headers, body}` (always `https://<host><path>`; a text body as is, anything else
  as JSON; an Effect anywhere refused) and asks the run's observer for its answer, by the node's
  path. With no answer yet, `response` is absent; the host performs the request and answers it in
  the next round, the way `store_read` is filled in. On the canvas it reads as a node that simply
  has a response. A request inside a `map`/`fold` function is asked for at the node's path then
  the item's index (`[…, map, k, …]`), so each item's is sent and answered on its own; `map`
  applies every item before giving up, so all of a round's requests go out together. Collection
  reads and waiting points inside a function are asked for the same way.
- **Absent means "not yet" for one round, and "no value" at the end.** Nodes downstream of an
  unanswered request are absent, so a `default`/`missing` may fire early; that round's output is
  discarded anyway. Only the final round's answer counts.
- **Only final-round effects are performed.** A state write computed while a response was missing
  never happens. Outbound requests are the one effect performed between rounds, because the next
  round needs their answer.
- **Cost:** upstream nodes re-run each round. Evaluation is cheap and pure, and per-round results
  of pure nodes can be cached by content hash. Independent requests share a round; dependent ones
  take one round each.
- **Recording:** the flow-log entry gains `rounds: [{requests, responses, timeline}]`. Replay feeds
  the recorded responses back instead of calling the network, so a recorded run replays
  bit-identically offline, and the editor steps through it round by round.

### 4.2 Outbound safety and limits
An outbound request is the first thing a flow does to the outside world, so the limits come with
the node, not after it. Two kinds:

**Rules** (always on; a flow can ask for an exception in its manifest, and only the host can grant
it, §4.2.1):

- **Hosts are in the manifest.** ✅ An `http_request`'s host is a frozen param, so each one puts
  `HttpRequest("hooks.slack.com")` in the flow's manifest, exactly, not just "the network"; deploy
  needs it acknowledged by name. A host chosen at run time (a later `host = *`, with the host in
  the path input) would need its possible hosts declared as a requirement; a host the deployment
  wasn't granted is refused at perform time.
- **No private destinations.** Loopback, link-local, private, CGNAT and metadata ranges (127/8,
  10/8, 172.16/12, 192.168/16, 100.64/10, 169.254/16, ::1, fc00::/7, fe80::/10) are refused after
  DNS resolution, against the address actually connected to, and again on every redirect. This is
  OWASP's SSRF guidance; it also stops DNS rebinding, since the check is on the resolved address.
- **HTTPS only**, with certificate verification; plain HTTP is a per-host opt-out.

**Limits** (numbers: a flow asks for what it needs, the host grants within its ceilings, §4.2.1).
Host defaults follow what the systems a flow talks to already expect:

| Limit | Default | Ceiling | Why this number |
|---|---|---|---|
| Connect timeout, per request | 10 s | 30 s | A healthy TLS handshake takes well under a second; 10 s fails a dead host fast. Common client default. |
| Request timeout (whole request, to the last byte) | 30 s | 300 s | Matches an inbound request's budget, below; long calls belong to durable runs (§4.3). |
| Run budget, request-triggered run | 25 s | 120 s | The caller is waiting: webhook senders give up early (GitHub: 10 s; Slack wants an ack in 3 s), and gateways cut at about 30 s (AWS API Gateway: 29 s). A run must answer before its caller hangs up. |
| Run budget, clock / trigger run | 60 s | 900 s | Nobody is waiting; the ceiling is AWS Lambda's 15 min. |
| Outbound requests per run (also bounds the rounds) | 50 | 1,000 | Cloudflare Workers' subrequest limits (50 free, 1,000 paid). |
| Concurrent requests per host, within a round | 6 | 32 | Browsers' per-host connection limit; polite to the APIs called. |
| Response body size | 10 MiB | 100 MiB | AWS API Gateway's payload limit; larger downloads go to the content store by reference (§4.5). |
| Request body size | 10 MiB | 100 MiB | Same as responses. |
| Redirects followed | 10 | 20 | Go's `net/http` default (10); browsers stop near 20. |
| Response header size | 64 KiB | 256 KiB | Above common server limits (8–16 KiB per line), well below abuse. |
| Longest wait at one `await` (`max_await_seconds`) ✅ | 30 days | 90 days | A month covers the usual waits (Stripe retries a webhook for 3 days; approvals and invoices run to weeks); 90 days is core's longest `expires`. |
| Runs waiting at once, per deployment (`max_waiting_runs`) ✅ | 10,000 | 100,000 | Bounds what one deployment keeps on disk and lists; the old fixed cap is the default. |
| Wait for a key another run holds (`claim_wait_ms`) ✅ | 10 s | 60 s | A claim (`docs/L4s/key-claims-L4.md`) lasts one run, so the default need only exceed a run's budget to make "busy" mean contention, not slowness. |

A request that breaks a limit doesn't fail the run: its node gets a response record
`{status: 0, error: "timeout" | "too_large" | "blocked_host" | ...}`, and the flow decides. Breaking
the **run** budget does fail the run, and nothing is performed.

#### 4.2.1 Manifests and permissions: the flow asks, the host grants
Every deployable flow has a **manifest**: what it needs. The **host** is the only authority over
what it gets. A flow never grants itself anything, and nothing it says is trusted at run time.
This is the model of Android's manifest and runtime permissions, or Deno's: the program declares,
the platform decides, enforces and can take back.

**The manifest** has two parts, and is content-addressed like everything else (its hash changes
exactly when what the flow needs changes):

| Part | Where it comes from | Contents |
|---|---|---|
| **Capabilities and effects** | **derived** from the graph by core (`capabilities()`, as consent works today), so they can't be understated | `NetworkIngress(port)`, `HttpRequest(host)` per request host, `StateRead(key)` / `StateWrite(key)`, `SecretRead(name)`, `Clock`, `Random`; which sinks the flow can reach |
| **Requirements** | **declared** by the author: frozen node params (in the unit's hash) and the flow's deploy settings | limits it needs above the host defaults (e.g. a 60 s request timeout), hosts for URLs built at run time, rule exceptions (plain HTTP to a host, a private address), durable-run needs (parked runs, expiry) |

**The host** keeps three things, and is the only place they change:

- **Policy** (`holon.toml`, flags or env; read over `GET /permissions`): each limit's default and
  ceiling (the table above), rules that grant without asking (e.g. state under the flow's own keys,
  egress to hosts on an operator allowlist, limits within the defaults), and what can never be
  granted (e.g. private addresses on a shared host).
- **Grants, per deployment,** persisted with it: `{manifest hash, granted capabilities, granted
  limits and exceptions, granted when, by whom}`.
- **Enforcement:** every effect, at perform time, is checked against the deployment's grant: a
  state key, host, secret or limit it wasn't granted is refused and logged, even if derivation had
  a bug. The grant is the boundary, not the graph.

**The lifecycle:**

1. **Deploy is a request.** The host compares the manifest with its policy. What policy allows is
   granted; what needs approval is shown in the editor's consent screen (the existing acknowledge
   step, generalized to hosts, secrets, limits and exceptions); what policy forbids, or a limit
   above its ceiling, refuses the deploy with a message naming the rule or ceiling. The host never
   silently grants less than asked: a requirement is what the flow needs to work.
2. **Redeploy asks only for the difference.** A new version whose manifest needs nothing new deploys
   without a prompt; one that needs a new host or a higher limit asks for that alone, like an app
   update requesting a new permission.
3. **Load checks again.** On restart the host re-derives each manifest and compares it with the
   stored grant; a deployment that now needs more than it was granted doesn't start, and is flagged.
   (This is the review's "consent isn't re-checked at load", §7a.)
4. **The host can change a grant at any time,** without a redeploy ✅: revoke a host or a secret, lower
   a limit, or lower a ceiling for everyone. The deployment is paused (the existing lifecycle) until
   its grant covers its manifest again, or, where the operator chooses, keeps running and the
   revoked call fails with `{status: 0, error: "revoked"}`. Every change is logged.
   (`POST /deployments/<slug>/grant` with `revoke`, `grant` (back), `lower` and `keep_running`,
   applied whole or not at all; `POST /permissions/reload` reads `holon.toml` again and re-checks
   every deployment. A grant that covers it again resumes a deployment the grant paused, not one
   the person paused. Raising a limit still takes a deploy, which asks for approval; a redeploy
   starts a fresh grant, so a limit the host lowered on a limit the flow didn't ask about is not
   carried over.)
   **Required or optional** ✅: the author marks which capabilities the flow can run without
   (deploy settings, `requirements.optional`, part of the manifest and its hash; Chrome's
   `optional_permissions`, Android's `required="false"`). One not granted, or revoked, leaves it
   running with those calls refused, so the flow should cope with that node staying empty; a
   required one pauses it as above. Everything else is required, and its trigger
   (`NetworkIngress`, or `Clock` for a clock flow) can never be optional; marking it, or something
   the flow doesn't use, is refused at deploy. Each item shows its kind in the manifest, the
   permissions and the consent screen.
5. **Everything is visible.** `GET /deployments/<slug>/manifest` and `.../grant`; the editor's
   deployment view shows asked against granted, pending approvals and revoke controls; each
   flow-log entry records the grant hash in effect, so a log always explains why a call was
   refused or timed out.

**The effective limit** for a call is the node's requirement if set, else the deployment's, else
the host default, always within what was granted.

**Limits that exist today move under the same model:** the data plane's inbound body cap
(hardcoded 8 MiB, `MAX_BODY_BYTES`; the default becomes 10 MiB to match the outbound one) and the
flow log's retention become host defaults with ceilings, which a deployment may ask to raise.
Retention is a host setting already (`--keep-runs N` or `HOLON_KEEP_RUNS`, 500 runs per deployment
without it); the ceiling and a deployment's own ask are left for later. The kept log is a hash
chain (`recorder.rs`): each entry names the line before it, and a run changed or removed outside
holon shows as a break, at load and in `GET /deployments/<slug>/log` (`head`, `breaks`). Heads go
to a witness outside the server with `--export-heads URL` (`heads.rs`): batched every 5 s, queued
on disk while it fails, a `trim` head when a log is cut back, the witness's answers kept as
receipts. The `witness` sample is one; a hosted witness that signs its receipts, and publishes
its own log's root, is the paid version. `GET /deployments/<slug>/export` writes the log's lines
and every unit its runs ran; `holon verify` (`verify.rs`) checks that offline, running every run
again through `run_rounds` from its entry, its outbound answers and reads from the record, and
comparing run id, outputs, effects and status (the `ledger` sample). The control plane's own cap stays fixed: it serves
only the editor.

- **Client:** `ureq` (synchronous, rustls). Runs are synchronous, and on the `hyper` data plane
  (§7b) they run on its blocking pool, so a synchronous client still fits.
- **Named folders** ✅ (`holon/src/folders.rs`, slice 1 of `docs/L4s/named-folders-L4.md`): files
  a flow reads and writes on the host, in folders an admin binds by name (`--folders`, its own
  file, owner-only; `GET`/`POST /admin/folders`, `DELETE /admin/folders/<name>`; `holon folder`),
  each read-only or read-write, its reads kept in the log as text or by hash. The server's own
  files, and any folder holding them, can't be bound. A flow asks `FileRead("inbox")` /
  `FileWrite("reports")`; a deploy is refused until they are bound as it needs. A file's name is
  checked before it reaches the OS (no `..`, root, drive, `:` or reserved name) and its real
  path must stay in the folder once links are resolved; listings never follow links. Reads are
  answered as the run finds the folder, recorded with their path like a collection's, and checked
  again before the run commits; text only, 1 MiB a file and 64 MiB a run. Writes are effects,
  applied at commit: written beside the file, synced and renamed over it. The log keeps a read's
  text up to 4 KiB, a bigger one in the content store by hash (exported as the export's `blobs`),
  and a write as `{name, size, hash}`; a node output carrying a text it keeps by hash is left out
  of the timeline. `holon verify --files DIR` answers reads a folder kept only by hash. A run
  whose read fails answers 500 and performs nothing.
- **Secrets** ✅ (`holon-server/src/secrets.rs`): a per-deployment secret store on the server
  (`--secrets`, its own file, owner-only), written over the control plane
  (`POST`/`DELETE /deployments/<slug>/secrets/<name>`), never returned by it (`GET` lists names,
  versions and sources).
  A flow references a secret by name (a `secret` source node, capability `SecretRead("stripe_key")`
  in its manifest, granted by the host); a run reads only the secrets it was granted, and its run
  id doesn't include them. Every literal occurrence of a value is redacted, as `‹secret:name›`,
  from the flow log and from what the control plane says about a deployment (state, sessions,
  `/trigger` and "send live" replies); a replay substitutes the same placeholder. A state write
  carrying a secret's value is refused, so one never reaches the state store, snapshots or disk.
  Values under 6 characters are refused (too short to redact reliably), and a value transformed
  before it's logged (hashed, encoded, sliced) can't be recognised. A flow may still send a secret
  where it is meant to go: an outbound request's header, or its own response to a caller.
  - **Sealed at rest** ✅ (`sealing.rs`): each value is AES-256-GCM sealed, bound to its
    deployment, name and version, under a master key kept apart from the file: the OS key store
    by default (Credential Manager, Keychain), else `--secrets-key env:VAR` (a container, or a
    cloud KMS or secret manager injecting it; `--new-secrets-key` prints one) or `file:PATH`
    (the default without an OS store, under the user's config directory). The file records a
    check value, so a missing or different key refuses to start rather than start empty. The
    secrets file, its audit log and the host key are written owner-only from the first byte (on
    Windows, a protected DACL: owner and SYSTEM). A file of plain values is sealed on first open.
  - **Kept elsewhere** ✅ (`secret_sources.rs`): a secret can name where its value is instead of
    holding it: `env:HOLON_SECRET_<NAME>` (the server's environment, or its `--dotenv` file in
    development, the real environment winning) or `vault:<mount>/<path>#<field>` (HashiCorp
    Vault KV v2, with `VAULT_ADDR` / `VAULT_TOKEN` / `VAULT_NAMESPACE`). Only `HOLON_SECRET_`
    variables can be named, so no flow can ask for the server's own credentials. A source must
    read when set; references are read again every minute, and one that can't be read keeps its
    last value for five minutes at most, so a revoked value stops working. The environment is an
    input here, never the store: it is process-wide, inherited by children and shows up in
    dumps, where the store is per deployment and granted.
  - **Versions, rollback, audit** ✅: setting a secret adds a version; the last five are kept,
    listed with their source and time (`GET /deployments/<slug>/secrets/<name>`), and any can be
    made current again (`POST …/<name>/rollback`, a new version, so history only grows). A value
    a secret no longer has is still redacted and still refused in state, so a client still
    sending a rotated-out token doesn't put it in the log. Every change, and a source that stops
    or starts reading, is appended to `<secrets>.audit.jsonl` (owner-only), never with a value.
  - **Accounts and access** ✅ (`holon-server/src/accounts.rs`, `access.rs`, `auth.rs`; the
    editor's `AuthGate.tsx`, `AdminPage.tsx`): everyone signs in. A new server answers nothing
    but its setup until its first admin is created with the one-time code it prints at startup
    (or `HOLON_SETUP_CODE`), so nobody else on the network can claim it first. Accounts
    (`--accounts`, owner-only) have a role: `viewer` (reads), `editor` (also saves, deploys,
    runs, deletes), `admin` (also writes secrets and the admin panel), `agent` (reads, proposes
    and calls: stores units, saves its own flows, deploys what needs no approval it hasn't had,
    and sends a deployed flow requests; never approves, grants, starts, stops, deletes or writes
    a secret). A request with `X-Holon-Role: agent` acts as one under its sender's name with
    ` (agent)` after it, whoever sends it: how `holon mcp` keeps to proposing while it signs in
    as the person who runs it. The header only lowers a role. Passwords are argon2id
    hashes, sessions last 7 days, and a few wrong passwords lock a name for a minute. The admin
    panel adds people by name and role and gives a one-time link (7 days) for them to set their
    own password; it changes roles, disables (their sessions end), resets (a new link), removes,
    and refuses any change that would leave no admin. API tokens for scripts and CI are made
    there with a role, shown once, revoked any time. Only hashes of tokens are kept. The OS user
    running the server needs none on that machine (`local.rs`): `holon serve` writes a run file
    readable only by that user (`~/.holon/run/<port>.json`, or `HOLON_RUN_DIR`) with its address
    and a token made for this run, an admin's, named `local:<user>` in the activity log; `holon
    export` finds the one server running and signs in with it. The server
    answers as loopback, and as the hosts and editor origins named with `--allow-host` /
    `--allow-origin`. Who did what is kept: the secrets audit (`by`), each saved flow
    (`saved_by`) and deployment (`deployed_by`), and the activity log
    (`<deployments>.activity.jsonl`, `GET /activity`, shown on the deployed page and the admin
    panel), which outlives what it names.
  - **Signing in through an identity provider** ✅ (optional, set in the admin panel; oidc.rs):
    the editor signs in with the provider (authorization code flow with PKCE, a public client)
    and sends the ID token as its bearer. The server checks it against the provider's published
    keys (RS/ES only), its issuer, audience and expiry, then finds the account with that
    (verified) email, which decides the role; someone without an account is refused. The
    end-to-end tests sign in through a stand-in provider.
  - **Left for later:** wiping values from memory beyond the store's own copies; more managers
    (AWS, GCP, Azure, 1Password) behind the same reference form; refresh tokens (a sign-in
    lasts as long as the provider's ID token).

  - **One binary, one port** ✅ (`holon-server/src/editor.rs`): the server serves the editor at
    `/`, beside its API, from the same address, so a machine without a screen is used from any
    browser that reaches it. Releases build it in (`npm run build` in editor/, then
    `cargo build --release -p holon-server --features embed-editor`): one file to install;
    `--editor DIR` serves a folder instead, for working on the editor. The built editor calls the
    server it is served from. All the server's state is in one folder, `~/.holon/data` (or
    `HOLON_HOME/data`; `--data DIR` or `HOLON_DATA` names another): store, flows, deployments,
    secrets, accounts, host key, policy. Its secrets key is beside it in `~/.holon/keys` (or the
    OS key store), named by the id the secrets file records, so the folder can move
    (docs/L4s/holon-home-L4.md).
  - **Web pages from other sites** ✅ (⚙ "web pages from"; listeners.rs): a deployment says
    which origins may call its routes (`*`, or `https://…`), none by default. For those the data
    plane answers the browser's preflight and adds `Access-Control-Allow-Origin`; the deploy
    dialog says so first. The Marketplace sample answers any.
  - **The marketplace** ✅ (the "marketplace" sample; editor marketplace.ts): a Holon flow
    keeping listings in a collection: `POST /listings` (with its publish key) publishes a holon
    (name, description, ports, manifest, and its units as hex preimages, kept under
    `name@root`), `GET /listings` and `/listings/:id` read them (open, to any page),
    `POST /listings/:id/yank` flags a version, never editing it. The editor sets which
    marketplace it uses (flows page), publishes saved flows there, and lists its holons in the
    palette, each with its adoption (how many other listings place it) and yanked flag. Placing
    one shows its manifest first, then inserts its units, each re-hashed, and refuses it unless
    its root is among them: a marketplace can describe a holon, never swap one.

### 4.3 Durable runs: continuing a run later
Rounds cover waiting inside one request: the caller is on the line, the wait is seconds. Many
integrations wait longer: a payment confirmed by a webhook minutes later, an approval clicked
tomorrow, an export polled until ready. The first run has to end, and a later event has to continue
it. That needs an identity for the run and a way to resume it.

**As built** (`holon-server/src/durable.rs`, the round loop's `Waits` in `holon-host`):

- **Park.** A run settles still waiting at an `await` node (params `name`, `expires` in seconds)
  that no event has answered. It **performs nothing** then: its caller gets its response output
  (what the flow says meanwhile, e.g. a `202` with a link) and the host keeps a **run record**:
  `{run id, the unit it ran, its inputs and sourced values, the state it read (by reference), its
  nonce, the requests it made, its tokens, the events delivered so far, what it waits for and until
  when}`, persisted with the deployment. Its other effects (state writes, egress) happen when it
  finishes, once.
- **Resume.** A request node may declare `resume = <await name>`; its route is then a resume route.
  A request on it must carry a token the host issued for that waiting point of a run of this
  deployment (path param or query `token`), or it is refused (`404`) before its handler runs. With
  one, the route's own handler answers the caller, and the request record is delivered as the
  event to the run's `await`: the run is evaluated again from its record (same inputs, nonce and
  state read, its earlier requests answered from the record) and finishes, or waits again. A token
  resumes once.
- **State.** A resumed run reads the state as it was when it started (that is what makes it the
  same run, and replayable); its writes apply when it finishes, over what is there then.
- **Pinned version.** A run continues on the deployment hash it started on. Units are
  content-addressed, so the old version stays resolvable for as long as runs need it, even across
  redeploys; new runs start on the new version.
- **Expiry.** Every waiting point has a deadline: the `await`'s `expires`, default 7 days, at most
  90 (the node's param), and no longer than the deployment's granted `max_await_seconds` ✅ (a
  deploy whose `await` waits longer is refused, naming the limit to ask for; parking holds to the
  grant in force, so a lowered one shortens new waits). Expiring is
  itself an event delivered to the awaiting node (`{expired: true}`), so the flow decides what
  timing out means. For scale: Stripe retries a webhook for up to 3 days, so a payment wait fits
  the default.
- **Parked runs per deployment:** at most its granted `max_waiting_runs` ✅ (default 10,000); past
  it, a run that would wait is refused with a `503` instead.
- **Visible.** The flow log has an entry per part of a run, under one run id (`parked` names what it
  waits for; a resumed part shows the events delivered). `GET /deployments/<slug>/runs` lists the
  waiting runs, never their tokens; the editor's deployment card shows "⏸ N waiting" with each one's
  route, waiting points and deadlines.
- **Today, by hand:** the same pattern works now with state: the first route `insert`s its context
  under an id, the callback route `lookup`s it. Durable runs automate that and make it safe
  (minted ids, pinned versions, expiry, visibility).

### 4.4 Run ids and resume tokens
Two different things, kept apart:

- **Run id: what names a run.** `run_id = H(deployment hash ‖ input root)`, where the input root
  is the Merkle root (§4.5) of everything the run started from: the request, the state it read, and
  its sourced values, including a **host nonce** of 128 bits from the operating system's secure
  generator. Because the input record is exactly what the flow log stores and replay consumes, the
  id, the log entry and the replay input are one thing, and the host can verify a run record
  against its id. The nonce makes two runs from the same flow and state distinct. A flow reads its
  id from a `run_id` source node; the id appears in logs and in the editor and grants nothing.
- **Resume token: what lets an outsider continue it.** A separate 128-bit value (above
  OWASP's 64-bit minimum for session identifiers), handed out by
  the host when a flow asks for one (a `resume_token` node, naming the `await` it resumes), for
  `(run id, await node)`. It goes into callback URLs, Stripe metadata, approval links.
  A run can hand out several (approve, reject), each can expire or be revoked, and a token is only
  accepted by the resuming routes of the deployment that issued it. Tokens are never derivable
  from the run id. ✅ A token is a MAC, not a stored random value: the first 128 bits of BLAKE3
  keyed with a 32-byte **host key** (`--host-key`, its own owner-only file, made at first start)
  over the run id and the waiting point. The same run and point always give the same token, so a
  resumed run hands it out again with nothing stored, and a run record, a log or a copy of the
  deployments file holds nothing that would continue a run. A presented token is compared in
  constant time. Runs parked before this kept their random tokens, which are still accepted.
  Losing or replacing the host key invalidates every outstanding token.
- **Why not one id for both:** the hashed part of a run id is computable by anyone who knows the
  flow and its state, so it cannot be the secret; and a single id could not be revoked, scoped to
  one waiting point, or handed out more than once.

### 4.5 Merkle state and content-addressed large values
A run id hashes the run's whole input, so hashing has to stay cheap however large the state gets.
The answer is to hash hashes, the way units already work.

- **Canonical value encoding, public.** Core already has a collision-free canonical `Value`
  encoder, private to the parked memo (`memo.rs`). It moves next to the unit encoder as
  `encode_value` / `hash_value`, with one decision pinned and tested first: `-0` and `0` encode
  alike (as the memo does) and `to_text` follows, closing the drift the 2026-10-21 review noted;
  `NaN` has one encoding.
- **State as a Merkle map.** Each key's value is hashed **once, when it is written**, and the hash
  kept beside it. The state root is `H(sorted (key, value hash) pairs)`. A run that writes one key
  re-hashes one value and a short list. Reads cost nothing.
- **Persisted state is references.** ✅ Every state value is stored in the content store under its
  hash (`values/`, apart from units, verified on every read); a deployment record and each
  session snapshot hold only `key → value hash`. Identical values are stored once, however many
  snapshots hold them. Files written before this load as before, and move their values into the
  store.
- **Large values by reference in the flow log and run records** (with run ids, step 5). A value
  over a threshold (default 64 KiB encoded; a host setting, not an author one, since it changes
  storage, not behaviour), and any uploaded file, is recorded by its hash, not inline, so logs
  stay small.
- **Request bodies** are hashed as they are read.
- **What else it buys:**
  - **Session snapshots share structure.** A snapshot is its list of `(key, value hash)`; unchanged
    values are shared between snapshots and sessions instead of copied.
  - **Diffs are cheap.** Equal hashes mean equal values, so the Flow state pane shows exactly what
    a run changed without comparing values.
  - **Replay checks its inputs.** A replay can confirm it feeds exactly the recorded state by its
    root.

#### 4.5.1 Collections, and runs side by side ✅ (2026-10-22)
State cells hold one value each, so a growing set of records kept in one cell is read whole and
rewritten whole by every run. A **collection** is a keyed, ordered set of records that doesn't:

- **Nodes.** `collection_get` (key → item), `collection_list` (an optional `prefix` and `after` →
  a record of up to `limit` items in key order, so a list pages: the next page starts after the
  last key), `collection_put` and `collection_delete` (effects, committed when the run finishes).
  Each names its collection; consent asks `CollectionRead(name)` and `CollectionWrite(name)`.
- **Storage.** A radix trie in the content store (`holon-host/src/collection.rs`): canonical (the
  same items, the same root, however they got there), persistent (a write stores only the nodes
  on its key's path, at most 8 values in 5,000 items; every version stays readable), ordered.
  Its root is a state cell under a reserved key (`#collection:<name>`; flows can't name `#` keys),
  so persistence, session snapshots, run ids and the log's state before cover it unchanged.
- **Reads** are answered on the spot from the collection as the run found it, logged with their
  node, and replayed from the log.

**Runs side by side.** A run no longer holds its deployment's state while it runs (outbound
calls included). It runs on a copy, and commits only if everything it could have read is still as
it found it: its granted state cells (by hash) and its collection reads (asked again). Writes to a
collection apply key by key to the live one, so writes to different keys merge. A run whose reads
changed runs again from the new state (`retries` in its log entry), its outbound requests
answered from its own record rather than sent twice; after four tries it runs holding the lock,
so a busy key can't starve it. A resumed durable run stays pinned to the state it started from.
A run about to send its first outbound request first **claims** the collection keys it read and
may write, re-checking those reads, and holds them until it commits, so two runs that would call
out over one key do so one at a time and the second reads what the first wrote before it can call
anyone (`docs/L4s/key-claims-L4.md`; a run that sends nothing claims nothing).
Tested: 24 concurrent increments lose none; a run waiting on a slow call no longer holds up the
next; live, 200 notes from 20 concurrent clients all land and page back in order.

Left for later: shared stores across deployments (granted like secrets), an external database
node (performed by the host between rounds, like `http_request`), blobs by reference (§4.5's
large values), and indexes beyond key order.

### 4.6 Build order for this section
One commit each, tests with each. M2 is §4.1 to §4.2 plus the editor; durable runs follow.

| Step | What | Done when |
|---|---|---|
| 1 ✅ | Core: public canonical `encode_value` / `hash_value` (±0, NaN pinned) | the encoding is fixed by tests; memo uses it |
| 2 ✅ | Host: Merkle state (value hashes on write, state root), persisted by reference | state root stable across restarts; a snapshot shares unchanged values |
| 3 ✅ | Core: `http_request` node, its capability `HttpRequest(host)` and validation | a flow with it validates and its derived hosts show at deploy |
| 4 ✅ | Host: the round loop (`holon-host`, network-free, with a `Performer` trait); the `ureq` performer and the §4.2 rules (`holon-server/src/outbound.rs`); the host-default limits per run kind | a run calls a local test server, uses the answer, and each limit stops it as specified. Resolving limits node → deployment → host, and the exceptions (private addresses, plain HTTP), arrive with grants in step 5 |
| 5 ✅ | Host and server (`holon-server/src/permissions.rs`): manifests (derived capabilities with hosts, declared requirements), policy in `holon.toml` (`--policy`) and `GET /permissions`, grants per deployment (`GET /deployments/<slug>/manifest`, `.../grant`), enforcement at perform time, the re-check at load (older records migrate); run ids and the nonce; `rounds` and the grant in effect in the flow log | a deploy asks only for what policy doesn't already allow; an ungranted host or a limit above a ceiling is refused; each log entry has a verifiable run id. Since then: changing or revoking a grant without a redeploy, and reloading the policy (§4.2.1 lifecycle point 4, `holon-server/src/grants.rs`) ✅. **Left for later:** large values by reference in the log (§4.5) |
| 6 ✅ | Editor: the node and its params, requirements in deploy settings (bounded by `GET /permissions`), the consent screen showing asked against granted (`POST /deploy/preview`), attention on deployments, rounds in the log and timeline, replay from recorded responses | a recorded run replays offline, round by round. Checked live: a deployed route called `https://example.com`, answered with its page, and view mode replayed the run offline with "✓ replay reproduces the run". Revoke controls ✅ since: the deployment card's permissions (revoke, grant back, keep running); a manifest on saved and deployed flows, and required or optional permissions ✅ (sample: `guarded-api`) |
| 7 ✅ | Secrets: store, `secret` node, redaction, set from the editor's ⚙ | a token never appears in a log, a replay or the editor. Checked live: set from ⚙, read by a run, found nowhere but the secrets file, and gone with its deployment. Since: sealed at rest under a key kept apart, sources in the environment or Vault, versions with rollback, and an audit log ✅ (sample: `guarded-api`) |
| 8 ✅ | Durable runs: `await`, park/resume, resume tokens, pinned versions, expiry | an approval link clicked later finishes the run on the version it started on. Checked live: an order waited, was paid by a link (even across a server restart), and wrote its state once; the token appeared in no log or reply. Since: expiry and the waiting cap as host-granted limits ✅, and tokens derived under a host-held key, so a run record stores none ✅ |

---

## 5. Server and endpoint units: **decided, option A** (2026-10-21)

| Option | How | For | Against |
|---|---|---|---|
| **A. Declaration units** (chosen) | `endpoint` = composite with frozen `method` and `path` params, one child = handler. `server` = composite with frozen `port`, children = endpoints. | The whole API is one hash, so versioning, rollback and diffing are free. Consent derives from the closure as it does today. | A little core code to read and check the declarations. |
| B. Runtime manifest | `ServerSpec { port, routes: [(method, path, handler_hash)] }` as JSON in the host | No core change | The API version is not a unit hash, which breaks "hand the host a hash". |

**As built** (`holon-core/src/declare.rs`):
- The markers are ordinary frozen params, so there is no encoding change and no new unit
  kind. The earlier idea of a `Kind::Declaration` did not fit: in core, "kind" means effect
  kind (Pure, Driver, State, Sink).
- **An endpoint** is any composite with `declare=endpoint`, `method` and `path`, one
  `Record`-compatible input (the request), and output 0 the response; further outputs carry
  effects such as state writes. Evaluating it runs the handler, so testing
  an endpoint is plain `eval`.
- **A server** has `declare=server` and `port`. Its children are endpoints; it has no wiring
  and no ports, so evaluating it yields nothing. Its capability manifest includes
  `NetworkIngress`, because it listens, plus whatever its handlers touch.
- `read_server` checks everything and fails closed: port, methods
  (`GET POST PUT PATCH DELETE`), paths (`/todos/:id`), endpoint shape, and duplicate
  routes. `match_route` picks the most specific match, so `/todos/new` beats
  `/todos/:id`. The host and the editor both call these.

A handler is an ordinary unit: `request: Record → response` (or an Effect plus `next`, via the
effect loop). It can be tested in the editor with no server running.

---

## 6. A pack, concretely

```
packs/hooks/
  pack.toml            name, version, door = "hooks", requires = ["auth", "http"]
  units/
    stripe_verify.holon     composite: split + fold + hmac_sha256 + secure_equals (a template today, crates/holon-author/src/templates.rs)
    github_event.holon      composite: json_parse + field + select
    slack_post.holon        composite: record + json_stringify + http_request
  templates/
    stripe-to-slack.flow.json
    github-push-check.flow.json
    form-to-email.flow.json
  onboarding.md
```

- `holon-pack build packs/hooks` turns each unit source into its canonical preimage, hashes it, and
  writes a bundle plus a manifest of `{name → hash, capabilities}`.
- A template references units **by hash**, so an integration never changes underneath a user.
- The manifest's capability summary is what a door shows before install ("this pack can make
  outbound HTTP requests").

---

## 7. Build order

| Milestone | Core | Host | Server | Editor | Packs | Done when |
|---|---|---|---|---|---|---|
| **M0 harden** | variadic leaves fail closed on absent inputs (`and`/`or`/`list`) ✅, eval edge bounds (out of range → None) ✅, `factorial` `checked_mul` ✅, `compare` default op ✅ | no panic on Effect in state ✅, List/Sum read-back ✅, atomic `FileStore` ✅ | **redeploy keeps state** ✅, atomic persist + refuse to start on parse error ✅, no disk I/O or per-deployment locks under the table lock ✅, Host check / required Content-Type / body cap ✅, split `main.rs` ✅ | reopen-flow fix (save composite preimages) ✅, clear projection on New/Open/edit ✅, undo flushes the pending debounce ✅, unsaved-changes and delete confirmations ✅ (in-app dialogs), split `App.tsx` ✅ | — | nothing in the editor can crash the server, lose a flow, or wipe state; see §7a. **Met** |
| **M1 API** | Record + record kit ✅ (+ keyed `lookup`/`insert`/`remove`, `find`, list `extend`), declaration units ✅ (§5; `:name` or `{name}` params), JSON ✅ (depth limit in `json_parse`; size bounded by the 8 MiB body cap), `instantiate` follows `fn` params ✅ | — | `data/` listener on a maintained HTTP stack (§7b) ✅: `hyper` on `tokio`, with timeouts, a body cap and a connection cap | test-request panel ✅; request/respond nodes and routes derived from the canvas ✅; a missing value answers 404 ✅ | `http` | a todo CRUD built visually works with `curl`. **Met** |
| **M2 effects + replay** | `http_request` ✅, hashes/HMAC ✅ (`sha256`, `hmac_sha256`, `hex`, `base64`, `secure_equals`), Effects refused by `Any`-typed sink payloads ✅ | effect loop ✅, `Performer` ✅, `Recorder` ✅ (flow log per deployment: request, state read, sourced values, outputs, per-node timings), secrets ✅, durable runs ✅ | request-log API ✅ (`GET /deployments/<slug>/log`); run / pause / stop with a state snapshot per session ✅ | `replay/` ✅ (view mode: live log, step through a run in the order it ran, timeline, flow state per step, component inspector; a failed run is traced to the node where it started, with why, and the path to the answer drawn; a run's data never changes the flow's layout: values are chips beside each output, pinned in full on a click, shown on a wire on hover, or switched off) | `auth` | Stripe → Slack works live, and a failure is replayed, fixed and redeployed in under 5 min. **Met (2026-10-22), with an echo service standing in for Slack** (see the status below) |
| **M3 door 1** | — | (consent at load moved to M2, §4.2.1) | single binary / Docker | `doors/hooks` | `hooks` + templates | someone else ships a webhook |
| later | — | — | `market` feature | — | — | `holon-market` returns as pack distribution |

**Status (2026-10-22).** M0 and M1 are done. M2's replay half
landed early (the recorder came with view mode); its effects half, and durable runs, are built (§4):
outbound requests in rounds, host permissions, secrets, and runs that wait and resume. The signature
primitives are in core too (`sha256`, `hmac_sha256`, `hex`, `base64`, and `secure_equals`, a
constant-time compare), checked against RFC 4231 / FIPS 180 / RFC 4648 vectors and GitHub's
webhook example.

**M2's live run (2026-10-22).** A flow built in the editor: `request` (POST `/stripe`) →
`stripe_verify` (a template: the `stripe-signature` header taken apart with `split` and `fold`,
HMAC-SHA256 of `<t>.<body>` under a `secret`, compared in constant time, `t` within 300 s of a
`clock`) → `{"text": "Stripe: <type>"}` → `http_request` POST, its path a `secret` →
`respond` (400 when unverified). Deployed through the consent screen (the clock, one host, two named
secrets), it answered webhooks signed the way Stripe signs them with 200 and posted the Slack-shaped
message; a tampered body or no signature got 400 and posted nothing. The outbound host was httpbin.org,
an echo service, standing in for `hooks.slack.com`: pointing it at Slack is the node's `host` param
and the `slack_webhook_path` secret. A bug deployed on purpose (the message read the event's `kind`,
not `type`) was found in view mode by stepping to the node whose output was absent, and fixed and
redeployed; from the failing webhook to a succeeding one took 76 s.

The run found two real bugs, both fixed with regression tests: an `http_request` whose wired body
was absent (the check upstream had failed) still went out, bodiless; and a replay recomputed a
node fed a secret from its placeholder, so the signature check replayed as failing when it had
passed. The flow log now keeps each node's outputs (redacted), and replay shows those for such a
node. Left for M3: the `hooks` pack (`stripe_verify` is an editor template today, not a published
unit), and a run against Stripe and Slack themselves (`stripe listen`, a real incoming webhook).

### 7a. Review findings, triaged against this direction
The 2026-10-21 project review is sorted by what matters for a **single-user backend builder with a
public data plane**, not for an open marketplace.

**M0: urgent now.** All fixed (2026-10-21/22), each with a regression test; kept as the record.
- **State and persistence**
  - **Redeploying a slug wipes its state.** For a backend this means a bug-fix deploy erases the
    database. This is the top item.
  - An Effect stored in a state cell hits `unreachable!` (`holon-host/src/lib.rs:282`), poisons the
    state mutex and then the deployment table. Every endpoint returns 500 and the clock scheduler
    dies.
  - Persistence uses truncate-then-write, and a file that fails to parse at startup is treated as
    empty, so the next save wipes everything.
  - A stored List reads back as `[]`, and a stored Sum disappears. This blocks the todo MVP.
- **Server under load**
  - Disk I/O and per-deployment locks happen under the global table lock, so one slow fire stalls
    every endpoint.
  - There is no request body cap.
  - The control plane is exposed to CSRF and DNS rebinding: there's no Host check, and no
    Content-Type requirement on JSON endpoints.
- **Core semantics**
  - Variadic leaves drop absent inputs (`eval.rs:57`). `and(authenticated, <absent>)` returns
    `authenticated`, so an auth check fails open. They must fail closed.
  - `eval` indexes `scratch[fc]` without bounds-checking edges (`eval.rs:133/149`).
  - `factorial` overflows in `hash.rs:371`: it wraps to 0, so the symmetry budget passes.
  - The catalog's `compare` default is `op="eq"`, but `compare` only accepts `"="`.
- **Editor**
  - Reopening a flow loses its composites: their preimages are never saved or re-inserted.
  - A stale projection survives New/Open/edits and can still be deployed.
  - Undo inside the 400 ms debounce window drops the last edit.
  - There's no unsaved-changes warning, and deleting a flow takes one click with no confirmation.
- **Hygiene**
  - Move the root `*.bin` / `*.json` files into `fixtures/`, and gitignore `.holon-*.json`.
  - Pin the toolchain to an exact version, and use one edition across crates.
  - Move the stray `editor/src/src/vite-env.d.ts` up one level, with its reference line.

**M1–M2: fix alongside the feature that needs it.**
- `instantiate` / `open_params` don't follow `fn` params (`promote.rs`). Needed once handlers use
  `map` / `fold` with promoted params. ✅ (both walk `child_refs`, so a hole in a `fn` is found and
  filled, and the `fn` re-pinned to the instance)
- `json_parse` needs depth and size limits, because request bodies are untrusted. ✅ (depth 64;
  size bounded by the body cap)
- `Any`-typed sink payloads accept Effects (`egress(egress(x))` validates). ✅ Closed: a sink builds nothing from a payload holding an
  Effect, and validation refuses the wiring. This must be closed
  before `http_request` exists.
- Consent isn't re-checked at load. Now part of the permissions design (§4.2.1), built with M2.

**Deferred: these only matter when running other people's units (they return with `holon-market`).**
- Exponential closure walks (`collect_caps`, `refork`, `instantiate`, `eval` on doubling DAGs).
- The decoder accepts non-canonical bytes, so `encode(decode(b)) ≠ b`.
- `refork` silently does a partial fork when a re-hash fails.
- The memo key folds ±0 while `to_text` doesn't (parked cache only). ✅ Fixed: `to_text` and `json_stringify` print `-0` as `0` (§4.5).
- Unbounded recursion on deeply nested units or values.
- Unicode case mapping can drift between toolchains.

### 7b. Data-plane HTTP stack
The control plane stays on `rouille`. That was chosen deliberately for a low-volume, localhost,
click-driven API, and it remains right for that.

The **data plane** is a different workload: public-facing, concurrent, and not trusted. `rouille`'s
dependency chain (`buf_redux`, `multipart`) already raises future-incompatibility warnings. Serve
the data plane from a maintained stack (`hyper`, or `axum` on top of it) with timeouts, body limits
and connection limits from the start. This deliberately reverses the earlier "no async runtime
needed" decision, because the workload changed.

✅ Done (`holon-server/src/dataplane.rs`): each server deployment's listener is `hyper` 1.x (HTTP/1)
on one shared multi-threaded `tokio` runtime. Routing stays our own (`data.rs` knows HTTP only as
strings and bytes), so `axum` would add nothing. Every connection and request is held to:

| Limit | Value | Why |
|---|---|---|
| Header read timeout (also an idle kept-alive connection's wait) | 10 s | Closes slow-header (slowloris) connections; well above any real client. |
| Body read timeout | 30 s | Matches the outbound request timeout; a slow body gets `408`. |
| Request body | 10 MiB | As §4.2.1 sets for inbound (matching the outbound limit); over it is `413`, before reading when `Content-Length` says so. |
| Connections per listener | 1,024 | Past it, connections wait in the OS accept queue instead of each taking a thread. |

A run is synchronous, so the handler runs on the runtime's blocking pool, bounded by its run
budget. Listeners bind `127.0.0.1` unless the operator opens another address (`--data-host 0.0.0.0`).
Left for later: these limits as host policy and per-deployment grants (they are host-wide constants
today), HTTP/2, and TLS (terminate it in front, e.g. a reverse proxy, until then). The control plane
keeps `rouille`, so its future-incompatibility warnings remain until it moves too.

---

## 8. What this deliberately does *not* do
- No per-door crates (`holon-hooks` as Rust code). Doors are packs plus editor config.
- No marketplace work before M3. `holon-market` compiles but is not developed.
- No `holon-auth` product. Auth is a shared pack used by every door.
- No tensor/ML work. That is a possible later door, not part of the platform now.
