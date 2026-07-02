# L4 — ingress-bound deployment (scoping, before any code)

**Status:** scoping note, same discipline as every prior leaf this arc. First slice of the
standing-deploy pillar — deliberately the small half (see `fire-scoping-L4.md`'s own split):
no background thread, no scheduler, nothing this project hasn't already built a version of.

---

## 1 · What's already grounded and needs no new work

- `fire_sourced(root: &Unit, sourced: &[(Capability, Value)], real: &impl Resolver, host: &mut
  impl Host) -> Option<Value>` — settles a graph with real driver values injected. Already
  built, already tested (`drive-sourced-resolver-L4.md`).
- `ingress`'s real interface: `Unit { inputs: [], outputs: [Port{"out", Any}] }` — zero inputs,
  output type `Any`. The value `holon-server` supplies for `Capability::NetworkIngress` can be
  any shape `SValue` can carry, not fixed to `Text`.
- `SourcedResolver` (inside `fire_sourced`) already substitutes *any* leaf whose capability
  matches a supplied one — one `[(Capability::NetworkIngress, value)]` pair reaches every
  `ingress` leaf in the closure, at any depth, automatically.
- `materialize_closure`, `RecordingHost`, `SValue` — all already built, all reused as-is.

**The honest consequence: this genuinely is the small half.** Nothing above needs touching.
The only new surface is *remembering a deployment* and *one new route*.

## 2 · What's new

**A `Deployments` registry** — `Mutex<BTreeMap<String, Hash>>` (slug → unit hash), a new
`AppState` field, in-memory only (same as `Registry`'s published set and `SharedCache` —
consistent with everything else in this crate; restarting the server forgets deployments, the
same way it forgets what's published).

**`POST /deploy`** — body `{"slug": "...", "unit": "<hex hash>"}`. Registers the mapping after
confirming the unit actually resolves in the store (reuses `Store::get`, not a new check).

**`GET /deployments`** — lists the current slug → hash table, same shape as `GET /registry`.

**`POST /deployed/<slug>`** — the actual trigger. Looks up `slug`, `materialize_closure`s the
target unit's closure (same helper `/cache` and `/fire` already use), builds
`sourced = [(Capability::NetworkIngress, <value from the request body>)]`, calls
`fire_sourced`, returns `{output, performed}` — the identical response shape `/fire` already
returns, since it's the same underlying settle-once-and-report behavior, just reached by slug
instead of by hash and with a driver value supplied instead of positional inputs.

## 3 · Real forks, surfacing rather than picking silently

**Fork 1 — does a deployment require the unit to be *published*, or just *resolvable in the
store*?** `Registry`'s "published" is a separate, marketplace-facing concept (a permanent
public commitment). Requiring it before deploy would couple two things that don't obviously
need coupling — a person iterating locally shouldn't have to "publish" every draft before
testing it live. Leaning toward: **just resolvable in the store** (staged via `/unit`, or
published — either satisfies it), matching how `/fire` already only requires the unit be
resolvable, not published.

**Fork 2 — slug collision: overwrite or reject?** Leaning toward **overwrite** — matches
`SharedCache::ingest`'s and `Store::put`'s own "dumb, idempotent" precedent elsewhere in this
crate; a re-`POST /deploy` to the same slug is a redeploy, not an error. If you want deploy
history or a "confirm before replacing a live slug" gate, that's a deliberate addition on top,
not the default.

**Fork 3 — what shape does the raw HTTP request body become as the injected `Value`?** Since
`ingress`'s output type is `Any`, there's no single correct answer forced by the type system.
Simplest default: wrap the raw body bytes as `Value::Text(body_as_utf8)` — matches how every
other text-shaped payload in this project already crosses (JSON bodies, `egress` messages).
A flow wanting structured data would need to parse that Text itself (e.g. via a JSON-decoding
composite, if one exists) — no different from how a real webhook payload usually arrives as a
raw body a handler then parses.

**Fork 4 (naming only, not architecture) — `/deploy` + `/deployed/<slug>`, or different
names?** `/deploy` registers; `/deployed/<slug>` triggers. Worth confirming since it's the one
thing external callers will actually type.

## 4 · What this deliberately does not do

- No background thread, no scheduler — a `Clock`-bound deployment is still its own, separate,
  later grounding pass, exactly as split last turn.
- No authentication on `/deployed/<slug>` beyond what already exists (localhost-only, CORS to
  localhost origins) — an ingress-triggered flow reachable by anyone who can reach this
  process at all, same trust boundary as every other endpoint so far.
- No capability-consent gate before deploy (surfacing what a unit *may* do before letting
  someone deploy it) — a real, good idea in the spirit of this project's own P4.4.2, but a
  deliberate, separate addition, not bundled into this first slice.
