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
  holon-host      runtime: Host, fire, effect loop, state, stores, recorder/replay, consent
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
| `primitives.rs`: arithmetic, text kit, list kit, `const`, `guard`, `default`, `equals`, `to_text`… | **+ record kit** (`record`, `field`, `with_field`, `keys`, `has_field`; keyed by an input: `lookup`, `insert`, `remove`; `find` over a list of records), **+ `json_parse` / `json_stringify`**, **+ `sha256`, `hmac_sha256`, `hex`, `base64`** (admission gate: inexpressible / hot-and-pervasive) |
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
| (new) | **consent check at load**: persist the acknowledged capabilities and refuse to run if derived ⊄ acknowledged. |

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
- **Control plane** (`127.0.0.1:7420`): editor-only, localhost, hardened against CSRF and DNS rebinding.
- **Data plane** (user-declared ports): public-facing by intent. It does no CORS magic; each endpoint's
  own logic plus `packs/auth` decides who is allowed.

### editor/src
| Today | Target |
|---|---|
| `App.tsx` (~1700 lines) | `canvas/` (nodes, wiring, group/expand), `flows/` (save/open, history hook), `deploy/` (projection, consent screen) |
| (new) | `test-request/`: build a request record, eval the endpoint locally via wasm, show the response |
| (new) | `replay/`: request log list → open one in the canvas with recorded sourced values → step and inspect |
| (new) | `doors/`: one config per door (title, palette sections, templates, first-run flow). The door is selected by route or subdomain. |
| `holon.ts`, `server.ts`, `diagnostics.ts` | stay. `holon.ts` gains the Record type, `server.ts` gains the request-log API. |

---

## 4. Effects that answer, and runs that last (design, 2026-10-22)

**Status:** design for review; nothing in this section is built yet, except the recorder it extends
(the flow log, `holon-server/src/recorder.rs`).

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

- **`http_request`** (core, a sink-like node): inputs `method`, `url`, `headers`, `body`; output
  `response`, a record `{status, headers, body}`. In round 0 it constructs its request and its
  `response` is absent; the host fills it in for the next round, the way `store_read` is filled in
  today. On the canvas it reads as a node that simply has a response.
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

**Rules** (on by default; a deployment opts out explicitly, and the opt-out is shown at consent):

- **Allowed hosts are part of consent.** Deploy derives each `http_request`'s host where it is a
  literal (`https://hooks.slack.com/...`), and the person acknowledges
  `NetworkEgress("hooks.slack.com")`, not just "the network". A URL built at run time needs an
  explicit allowlist entry; a host not on it is refused at perform time.
- **No private destinations.** Loopback, link-local, private, CGNAT and metadata ranges (127/8,
  10/8, 172.16/12, 192.168/16, 100.64/10, 169.254/16, ::1, fc00::/7, fe80::/10) are refused after
  DNS resolution, against the address actually connected to, and again on every redirect. This is
  OWASP's SSRF guidance; it also stops DNS rebinding, since the check is on the resolved address.
- **HTTPS only**, with certificate verification; plain HTTP is a per-host opt-out.

**Limits** (numbers, so they are configurable; §4.2.1 says who sets them). Defaults follow what the
systems a flow talks to already expect:

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

A request that breaks a limit doesn't fail the run: its node gets a response record
`{status: 0, error: "timeout" | "too_large" | "blocked_host" | ...}`, and the flow decides. Breaking
the **run** budget does fail the run, and nothing is performed.

#### 4.2.1 Who sets the limits
Limits are authored params at two levels, inside ceilings the host operator manages:

| Level | Where it is set | What it can do |
|---|---|---|
| **Host** (the operator) | server config file (`holon.toml`, `[limits]`), or flags and env vars | sets each limit's **default** and **ceiling**, for every deployment on this host |
| **Deployment** (the author) | deploy settings in the editor (⚙, beside port and interval today), sent with the deploy | raises or lowers any limit, up to the ceiling; the allowlist and rule opt-outs live here too |
| **Node** (the author) | `http_request` params (`timeout`, `max_response_bytes`, `follow_redirects`) | tightens one call further, never past the deployment's limit |

- **The effective limit** is the node's value if set, else the deployment's, else the host default,
  always clamped to the host ceiling.
- **Deploy refuses a deployment limit above a ceiling,** with a message naming the ceiling, rather
  than clamping silently. Consent shows any limit raised above the host default and any rule
  opted out.
- **Node params are frozen params,** so they are part of the unit's hash: changing a call's timeout
  is a new version, like any other behaviour change. Deployment limits are not in the hash, but are
  stored with the deployment and recorded in each flow-log entry as the limits in effect, so a log
  always explains why a call timed out.
- **Lowering a ceiling** later clamps running deployments to it, flags them in the deployment list,
  and logs the change. Nothing is redeployed silently.
- **The control plane exposes** `GET /limits` (defaults and ceilings) so the editor can show the
  bounds while editing and validate before deploy.
- **Limits that exist today move under the same config:** the data plane's inbound body cap
  (hardcoded 8 MiB, `MAX_BODY_BYTES`; default becomes 10 MiB to match the outbound one) and the
  flow log's retention (hardcoded 500 entries, `recorder::CAPACITY`) become host defaults with
  ceilings, and the inbound cap a per-deployment setting. The control plane's own cap stays fixed:
  it serves only the editor.

- **Client:** `ureq` (synchronous, rustls), which fits the current thread-per-request server; it
  moves with the data plane if that moves to `hyper` (§7b).
- **Secrets** (separate step): a per-deployment secret store on the server, written over the
  control plane, never returned by it. A flow references a secret by name (a `secret` source node,
  capability `SecretRead("stripe_key")`, acknowledged at deploy). Values are redacted from the flow
  log, from replay and from the editor; a replay substitutes a placeholder.

### 4.3 Durable runs: continuing a run later
Rounds cover waiting inside one request: the caller is on the line, the wait is seconds. Many
integrations wait longer: a payment confirmed by a webhook minutes later, an approval clicked
tomorrow, an export polled until ready. The first run has to end, and a later event has to continue
it. That needs an identity for the run and a way to resume it.

- **Park.** A flow ends a round waiting on an `await` node (a source, like `http_request`'s
  response) that no request will answer. The host stores a **run record**:
  `{run id, deployment hash, input record, rounds so far, the awaiting node, expiry}`, and answers
  the current request with whatever the flow chose to say meanwhile (e.g. `202 Accepted`).
- **Resume.** An event arrives on a route declared as resuming (`declare=endpoint`,
  `resume=<await node name>`), carrying a resume token (§4.4). The host loads the run record and
  evaluates the run's flow again from its recorded history, with the event delivered to the awaiting
  node. The run finishes, or parks again on another `await`.
- **Pinned version.** A run continues on the deployment hash it started on. Units are
  content-addressed, so the old version stays resolvable for as long as runs need it, even across
  redeploys; new runs start on the new version.
- **Expiry.** Every parked run has a deadline: default 7 days, ceiling 90 days, set like the other
  limits (§4.2.1; an `await` node may shorten it). Expiring is itself an event delivered to the
  awaiting node (`{expired: true}`), so the flow decides what timing out means. For scale: Stripe
  retries a webhook for up to 3 days, so a payment wait fits the default.
- **Parked runs per deployment:** default 10,000, ceiling set by the host, so a flood of starts
  can't fill the disk.
- **Visible.** The flow log groups one run's entries under its run id; the timeline shows the gap.
  The editor lists parked runs per deployment, with their expiry.
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
- **Resume token: what lets an outsider continue it.** A separate 128-bit random value (above
  OWASP's 64-bit minimum for session identifiers), minted by
  the host when a flow asks for one (a `resume_token` node, naming the `await` it resumes), mapped
  to `(run id, await node, expiry)`. It goes into callback URLs, Stripe metadata, approval links.
  A run can hand out several (approve, reject), each can expire or be revoked, and a token is only
  accepted by the resuming routes of the deployment that issued it. Tokens are never derivable
  from the run id.
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
- **Large values by reference.** A value over a threshold (default 64 KiB encoded; a host setting,
  not an author one, since it changes storage, not behaviour), and any uploaded file,
  is stored in the content store under its hash, like a unit; state, the flow log and run records
  hold the hash. Hashing it later is free, identical content is stored once, and logs stay small.
- **Request bodies** are hashed as they are read.
- **What else it buys:**
  - **Session snapshots share structure.** A snapshot is its list of `(key, value hash)`; unchanged
    values are shared between snapshots and sessions instead of copied.
  - **Diffs are cheap.** Equal hashes mean equal values, so the Flow state pane shows exactly what
    a run changed without comparing values.
  - **Replay checks its inputs.** A replay can confirm it feeds exactly the recorded state by its
    root.

### 4.6 Build order for this section
One commit each, tests with each. M2 is §4.1 to §4.2 plus the editor; durable runs follow.

| Step | What | Done when |
|---|---|---|
| 1 | Core: public canonical `encode_value` / `hash_value` (±0, NaN pinned) | the encoding is fixed by tests; memo uses it |
| 2 | Host: Merkle state (value hashes on write, state root) and large values by reference | state root stable across restarts; a snapshot shares unchanged values |
| 3 | Core: `http_request` node, its capability `NetworkEgress(host)` and validation | a flow with it validates and its derived hosts show at deploy |
| 4 | Host: the round loop, the `Performer` with `ureq`, the §4.2 rules, and limits resolved node → deployment → host default, clamped to the ceiling | a run calls a local test server, uses the answer, and each limit stops it as specified |
| 5 | Server: `holon.toml` limits and `GET /limits`; deployment limits and consent with hosts; run ids and the nonce; `rounds` and limits in effect in the flow log | a deployment lists its hosts and limits, a limit above a ceiling is refused, and each log entry has a verifiable run id |
| 6 | Editor: the node and its params, limits in deploy settings (bounded by `GET /limits`), rounds in the log and timeline, replay from recorded responses | a recorded run replays offline, round by round |
| 7 | Secrets: store, `secret` node, redaction | a token never appears in a log, a replay or the editor |
| 8 | Durable runs: `await`, park/resume, resume tokens, pinned versions, expiry | an approval link clicked later finishes the run on the version it started on |

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
    stripe_verify.holon     composite: hmac_sha256 + hex + compare + text kit
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
| **M0 harden** | variadic leaves fail closed on absent inputs (`and`/`or`/`list`) ✅, eval edge bounds (out of range → None) ✅, `factorial` `checked_mul` ✅, `compare` default op ✅ | no panic on Effect in state ✅, List/Sum read-back ✅, atomic `FileStore` ✅ | **redeploy keeps state** ✅, atomic persist + refuse to start on parse error ✅, no disk I/O or per-deployment locks under the table lock ✅, Host check / required Content-Type / body cap ✅, split `main.rs` ✅ | reopen-flow fix (save composite preimages) ✅, clear projection on New/Open/edit ✅, undo flushes the pending debounce ✅, unsaved-changes and delete confirmations ✅ (in-app dialogs), split `App.tsx` | — | nothing in the editor can crash the server, lose a flow, or wipe state; see §7a. **Met**; splitting `App.tsx` remains |
| **M1 API** | Record + record kit ✅ (+ keyed `lookup`/`insert`/`remove`, `find`, list `extend`), declaration units ✅ (§5; `:name` or `{name}` params), JSON ✅ (depth limit in `json_parse`; size bounded by the 8 MiB body cap), `instantiate` follows `fn` params | — | `data/` listener on a maintained HTTP stack (§7b): runs today on the control plane's `rouille`, move pending | test-request panel ✅; request/respond nodes and routes derived from the canvas ✅; a missing value answers 404 ✅ | `http` | a todo CRUD built visually works with `curl`. **Met** |
| **M2 effects + replay** | `http_request`, hashes/HMAC, Effects refused by `Any`-typed sink payloads ✅ | effect loop, `Performer`, `Recorder` ✅ (flow log per deployment: request, state read, sourced values, outputs, per-node timings), secrets | request-log API ✅ (`GET /deployments/<slug>/log`); run / pause / stop with a state snapshot per session ✅ | `replay/` ✅ (view mode: live log, step through a run, timeline, flow state per step, component inspector) | `auth` | Stripe → Slack works live, and a failure is replayed, fixed and redeployed in under 5 min. **Replay half done; effects half designed in §4 (build order §4.6), not started** |
| **M3 door 1** | — | consent at load | single binary / Docker | `doors/hooks` | `hooks` + templates | someone else ships a webhook |
| later | — | — | `market` feature | — | — | `holon-market` returns as pack distribution |

**Status (2026-10-22).** M0 and M1 meet their "done when"; what is left of them is splitting
`App.tsx`, the data plane's move to `hyper`, and `instantiate` following `fn` params. M2's replay half
landed early (the recorder came with view mode); sinks now refuse Effects; next is
`http_request` and the effect loop, as designed in §4, followed by durable runs.

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
  `map` / `fold` with promoted params.
- `json_parse` needs depth and size limits, because request bodies are untrusted. ✅ (depth 64;
  size bounded by the body cap)
- `Any`-typed sink payloads accept Effects (`egress(egress(x))` validates). ✅ Closed: a sink builds nothing from a payload holding an
  Effect, and validation refuses the wiring. This must be closed
  before `http_request` exists.
- Consent isn't re-checked at load (M3, §3).

**Deferred: these only matter when running other people's units (they return with `holon-market`).**
- Exponential closure walks (`collect_caps`, `refork`, `instantiate`, `eval` on doubling DAGs).
- The decoder accepts non-canonical bytes, so `encode(decode(b)) ≠ b`.
- `refork` silently does a partial fork when a re-hash fails.
- The memo key folds ±0 while `to_text` doesn't (parked cache only).
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

---

## 8. What this deliberately does *not* do
- No per-door crates (`holon-hooks` as Rust code). Doors are packs plus editor config.
- No marketplace work before M3. `holon-market` compiles but is not developed.
- No `holon-auth` product. Auth is a shared pack used by every door.
- No tensor/ML work. That is a possible later door, not part of the platform now.
