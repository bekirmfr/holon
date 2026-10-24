# Child-order canonicalization — is child order identity? (L4)

**Status:** design note for a core inch, taken *before* the editor's authoring inches (P4.1) so the
editor is built on a settled identity model. Touches the hash preimage — identity itself.

---

## 1. The fork

A `Composite { children: Vec<Hash>, edges: Vec<Edge> }` lists its children in *some* order, and the
edges reference children by **index** into that list. Today `encode_preimage` normalizes edge order
(sorts edges) but **preserves the given child order**, with a comment that a deterministic child
order is "the caller's responsibility, frozen at encapsulation time." There is even a test,
`order_is_identity`… but it is about the *promoted input* order (the interface `inputs` vector —
`add(a,b)` vs `add(b,a)`), which genuinely **is** identity (the calling convention). **Child order is
a different thing, and nothing in the system canonicalizes it.**

So: **should two composites with the same children and the same wiring, but listed in a different
child order (edges remapped to match), be the same unit or different units?**

- **Today:** different units (different child order ⇒ different preimage ⇒ different hash).
- **Proposed:** the same unit — canonicalize child order away in the encoder.

## 2. The decision, and why it is forced

**Child order is presentation, not structure, so it must not be part of identity.** The proof is in
`eval`: `eval_composite` resolves each child by hash and **topologically orders by the edges**, not
by the child-list order; the only use of a child's index is to look up which `Unit` to resolve.
Reorder the children and remap the edges to match, and every value `eval` produces is identical — the
topo order is the same structure with relabeled indices. (Even the topo tie-break "ascending child
index" can't change a value: tied nodes are mutually independent, so their relative evaluation order
is irrelevant.) **`eval` ignores child order exactly as it ignores edge order** — and edge order is
*already* canonicalized in the encoder. Treating the two differently is an inconsistency, and it
bakes a meaningless distinction (how the author happened to list the children) into the content
address. For a system whose entire thesis is "identity = what the thing **is**," that is a bug.

The payoff: two authors who build the identical wiring get the identical unit — strengthening
independent-rebuild dedup. (Honest scope: the shared-cache moat mostly rides on **import-by-
reference** — everyone references a published unit's one hash — so this is "structurally-identical
compositions share an identity," not "the moat collapses without it." It is still the right thing.)

## 3. Where it lives: the encoder, not the caller

Canonicalize **inside `encode_preimage`**, in the same place edge order is already normalized. This is
the natural boundary:

- `encode_preimage` is the *one* definition of the preimage, hence of the hash. Canonicalizing there
  makes child order provably non-identity for **every** caller — the editor, encapsulation, fork, a
  future CLI — with no way to forget. The alternative (a separate `canonicalize` that callers must
  remember to call) is a parallel mechanism and a future divergence bug.
- It keeps the store consistent: `put` stores `encode_preimage(unit)` (now canonical) under
  `hash_unit(unit)` (the canonical hash); `decode_verified` re-encodes and re-hashes, which is
  idempotent on canonical bytes. One canonical encoding flows to the hash, the store key, the store
  bytes, and the cache key — unchanged invariant, now actually true for composites.

**`ENCODING_VERSION` stays at 1 (a considered call).** The rule is "bump on any encoding change,"
whose purpose is to prevent a unit hashed under one version from colliding with a *different* unit
under another. This change introduces no such collision: it only *restricts* the encoder to canonical
orderings — primitives and already-canonical composites encode byte-for-byte as before, and every
canonical ordering was already a valid v1 encoding, so the new output set is a subset of the old. No
new byte pattern appears; order-equivalent composites simply collapse onto the canonical one (the
intent). Bumping would needlessly re-address every hash (including the genesis trust root and the
heartbeat's `const`) for zero collision-safety gain, so v1 stands.

**Consequence to state plainly:** `decode(encode(u))` is no longer structurally `u` for a
non-canonically-ordered `u` — it is the *canonical* form of `u`. That is correct (it's
canonicalization), and the property that matters — `encode(decode(canonical_bytes)) ==
canonical_bytes` — still holds. `MapResolver` keeps the unit *as inserted* (so it may hand back a
non-canonical child order), but its **hash is canonical**, and `eval` is order-inert, so nothing
downstream cares. Only the on-disk `Store` round-trips through bytes and therefore returns canonical
order.

## 4. How: hash-sort, ties broken by where each child's value goes

*Revised 2026-10-24. The first version broke ties by minimization (below, kept as history); a real
route outgrew it, and ties are now broken by path labels.*

Canonical child order is computed structurally:

1. **Order children by hash.** Distinct hashes ⇒ fully determined order (unchanged: a composite
   with no two children alike hashes as it always did).
2. **Ties = same-hash children** (two `as_text` nodes, one per `{…}` part of a string, are the same
   unit). Each child gets a **path label**: a hash of its own hash and of every edge leaving it,
   each edge named by its ports and by where it goes, the composite's output port or the label of
   the child it feeds. Labels are computed from the outputs back (`path_labels`), so a child's label
   covers every path from it to the outputs. Same-hash children are ordered by label.
3. **Remap edges** to the new indices (`BOUNDARY` fixed), then sort.

**Why labels can't tie for children that matter.** Every input port takes exactly one wire, so two
different children can't feed the same port of the same child. Following any two children's values
toward the outputs, their paths part somewhere, and so do their labels. Labels are defined by the
wiring alone (child hashes, port numbers, output ports), never by the order the author listed
anything, so the order is presentation-invariant. A route's response and every effect are its
outputs (`route_spec`), so every child that changes what a run does or answers has a path.

**What is left: dead twins, refused.** Two children still tied are the same unit and neither one's
value reaches an output: a value computed twice and never used. Nothing in the composite tells them
apart, so the encoder refuses (`EncodeError::UnusedTwins`, fail-closed: never an order that depends
on presentation), and the author is told to remove one or use it. A single unused child is fine;
it has no twin to tie with. A malformed blob with a cycle neither hangs nor panics: a child on (or
feeding) a cycle is labelled by its hash alone.

**Cost:** one pass over the edges from the outputs back, whatever the symmetry. There is no search
and no cap.

### The first version: minimization (2026-10, replaced)

For each run of equal-hash children it tried every within-run ordering and kept the one whose
`(children ++ remapped-sorted-edges)` serialization was smallest, capped at 8! orderings
(`MAX_CHILD_SYMMETRY`, refused past it as `TooSymmetric`). The cap multiplied across runs, and the
language lowers each `{…}` part to an `as_text`: nine parts in one route, or six parts sharing one
separator, were refused. This section's deferred trigger ("a real composite hits
`MAX_CHILD_SYMMETRY`") was hit by an agent's first medium-sized route, and path labels replaced the
search outright rather than refining it.

**Hashes changed** for composites with same-hash children (any route with two `{…}` parts, say);
composites without them hash as before. This was taken in development, with no deployments kept: a
store holding a composite under its old hash reads it as `Corrupt`, and a server holding such a
deployment refuses to start until its deployments file is moved aside. `ENCODING_VERSION` stays at 5
for the reason §3 gives for not bumping: every new encoding was already a valid one.

## 5. Deferred

Nothing for ordering itself. Unused values in general (a single one is legal today) could become an
error in the language with the line they're on; that is a language decision, not the encoder's.

## 6. Why now (empty corpus), not at P4.3

This is a **breaking change to every composite's hash**. The project already has the precedent and
the rule: *settle identity before you mint* — genesis reconciliation deliberately froze the trust
root **before** the registry's first publish. Child-order is the same shape of decision, and the
corpus is empty *right now*. The editor (P4.1) is about to start producing composites in volume;
doing this after a corpus exists is exactly the breaking-change pain we sequence to avoid. So this
core inch lands before the palette.

## 7. Blast radius (grounded against the actual code)

- **`encode_preimage`** (hash.rs): the Composite arm canonicalizes children + remaps/sorts edges. New
  `EncodeError::TooSymmetric`. ~6 small private helpers (factorial, permute, enumerate, apply-order,
  serialize-key, `canonical_composite`).
- **`decode_preimage`**: **no change** — it reads whatever order is encoded (now canonical).
- **`eval`, `child_refs`, `adoption`, `validate`**: **no change** — all order-inert (eval topo-sorts;
  child_refs/adoption use the child *set*, reordering doesn't change *which* children).
- **Tests that break (exactly two), both because the property changed:**
  - `hash.rs::decode_round_trips_a_composite_with_params` asserts `decode(encode(u)) == u`
    structurally — now holds only for canonical input ⇒ author its children already hash-sorted.
  - `host::fork_repins_and_republishes` asserts `children_of(nr) == [cc, cb]` in order after a
    *store* round-trip (now canonical order) ⇒ make the assertion order-agnostic (set membership:
    C present, B present, A gone).
- **Everything else verified safe:** the promote/instantiate tests and all deploy-firing tests build
  units and `eval`/fire them in-memory (order-inert) and never pin a composite hash; the store
  round-trip tests that assert structural equality use *primitives*; `edge_authoring_order_is_cosmetic`
  and `child_hash_changes_parent_hash` still pass (identical canonicalization both sides / different
  child multiset).
- **New tests:** `child_order_is_cosmetic` (same wiring, different child order ⇒ equal hash) and
  `duplicate_children_canonicalize` (two same-hash children wired to different roles ⇒ the two
  presentations produce one deterministic hash; swapping presentation doesn't change it).

## 8. Invariants

I5 (identity = content): strengthened — presentation order no longer leaks into the hash, matching
the existing edge-order treatment. I3 (compose by reference): untouched. I2 (eval determinism): the
linchpin — eval is order-inert, which is *why* this is safe.
