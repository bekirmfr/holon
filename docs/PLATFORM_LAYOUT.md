# Platform layout: one engine, many front doors

**Status:** proposal (2026-10-21). Nothing here is built yet.
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
| `primitives.rs`: arithmetic, text kit, list kit, `const`, `guard`, `default`, `equals`, `to_text`… | **+ record kit** (`record`, `field`, `with_field`, `keys`, `has_field`; keyed by an input: `lookup`, `insert`, `remove`), **+ `json_parse` / `json_stringify`**, **+ `sha256`, `hmac_sha256`, `hex`, `base64`** (admission gate: inexpressible / hot-and-pervasive) |
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

## 4. The two new runtime mechanisms

### Effect loop (dependent effects within one request)
Today a request is **one round**: sourced values are fixed before eval and effects are performed
after it settles (`fire_sourced`, `holon-host/src/lib.rs:116`). Webhooks need "verify → fetch →
decide → post". The loop:

```
round 0: sourced = {ingress: request}            → eval → output
         output is Response?        → done
         output is Effect + `next`? → perform effect, record (effect, result)
round n: sourced = {ingress: request, prior results by correlation token} → eval → …
         bounded by a max-rounds limit (totality per request, not just per eval)
```

- Elm-shaped, as the design always intended: the core *decides*, the host *performs*. The existing
  correlation-token design (`P2.5.1`) carries "which effect is this result for".
- **Replay falls out of it:** a recorded request is just the list of each round's sourced values.
  Feed them back into the same `eval` (natively or in wasm) and you get a bit-identical run, with no
  network involved.

### Recorder
One record per request: `{deployment hash, request, rounds: [sourced, effects], response, timings}`.
Stored per deployment with a bounded retention. The editor's `replay/` reads it over the control plane.

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
| **M0 harden** | variadic leaves fail closed on absent inputs (`and`/`or`/`list`), eval edge bounds (out of range → None), `factorial` `checked_mul`, `compare` default op | no panic on Effect in state, List/Sum read-back, atomic `FileStore` | **redeploy keeps state**, atomic persist + refuse to start on parse error, no disk I/O or per-deployment locks under the table lock, Host check / required Content-Type / body cap, split `main.rs` | reopen-flow fix (save composite preimages), clear projection on New/Open/edit, undo flushes the pending debounce, unsaved-changes and delete confirmations, split `App.tsx` | — | nothing in the editor can crash the server, lose a flow, or wipe state; see §7a |
| **M1 API** | Record + record kit ✅, declaration units ✅ (§5), JSON (**depth and size limits on `json_parse`**), `instantiate` follows `fn` params | — | `data/` listener on a maintained HTTP stack (§7b) | test-request panel | `http` | a todo CRUD built visually works with `curl` |
| **M2 effects + replay** | `http_request`, hashes/HMAC, Effects refused by `Any`-typed sink payloads | effect loop, `Performer`, `Recorder`, secrets | request-log API | `replay/` | `auth` | Stripe → Slack works live, and a failure is replayed, fixed and redeployed in under 5 min |
| **M3 door 1** | — | consent at load | single binary / Docker | `doors/hooks` | `hooks` + templates | someone else ships a webhook |
| later | — | — | `market` feature | — | — | `holon-market` returns as pack distribution |

### 7a. Review findings, triaged against this direction
The 2026-10-21 project review is sorted by what matters for a **single-user backend builder with a
public data plane**, not for an open marketplace.

**M0: urgent now.**
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
- `json_parse` needs depth and size limits, because request bodies are untrusted.
- `Any`-typed sink payloads accept Effects (`egress(egress(x))` validates). This must be closed
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
