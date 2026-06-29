# REFERENCES — Adopt the design, build the code

*The build-time companion to `COMPOSABLE_VM_DESIGN.md` §12. Where §12 says **where
Holon sits** (positioning, novelty), this says **what to port when you build each
leaf** — the canonical prior-art design to copy, and, more importantly, the column
that keeps us honest: **where Holon deliberately diverges**, so a future
contributor doesn't "fix" a departure that is load-bearing.*

> **Governing principle: adopt *knowledge*, build *code*.** Almost every mechanism
> Holon needs is solved somewhere. Re-deriving it spends invention budget we owe to
> the genuinely-new bridge (visual + content-addressed + trustless memo market).
> Porting a proven design spends none. So: copy the design, write the code small
> and owned.

> **Why we don't adopt a runtime (the build-vs-adopt verdict).** Continue
> `holon-core`; adopt Unison's *designs*, not its *runtime*. The two invariants
> that define Holon's value are exactly the two things adoption cannot give:
> **P4.2** (one evaluator, native == WASM, bit-identical — Unison's runtime is a
> Haskell bytecode interpreter with no first-class →WASM) and **P3.3.2** (the
> trustless cross-user memo market — Unison Cloud is the *proprietary* part).
> Adoption hands us the easy, undifferentiated parts and blocks us on the hard,
> differentiating ones. Wrong trade.

Status legend: ✅ built · ⏳ pending / in-flight · ◇ deferred or open fork.

---

## The porting map

| Target (leaf / invariant) | Canonical design to port | What to copy | Where Holon **diverges** (do not "fix") | Status |
|---|---|---|---|---|
| **I5** — content-addressing, hash = identity | Unison (hash of name-stripped AST), Git objects, IPFS/IPLD, Nix CA derivations | Hash *is* identity; names are aliases; a canonical, name-independent encoding | BLAKE3 over our own length-prefixed / field-tagged / domain-separated encoding of the **wire model** — *not* Unison's SHA3-512 AST hash. We own the encoding, so I4 (derive-then-freeze) is not constrained by a foreign AST. | ✅ `hash.rs` |
| **I6** — transitive pinning (lockfile closure) | Nix flakes (`flake.lock`, graph-isomorphic, exact pins); Cargo.lock; Go modules + checksum/transparency log | Store the resolved hashes of the **entire** closure; exact pins, no ranges; tamper-evident | Closure = `children ∪ hash-valued params` (the `map` fn-ref rule). Go's transparency-log model is a later (P3.7) adoption. | ⏳ by-ref done; closure storage at P3.8.1/P3.1.1 |
| **I8 / P2.2.1 / P2.3.1–2** — effect inference (kind + capabilities) as union of children | **Unison abilities** (required set = union of callees, propagated into signatures, terminated by handlers); Koka / Eff / Effekt row effects | Union-of-callees inference; **surface before use / import** | **No discharge** — no handler terminates an axis: effects are *observable*, and design-mode mocks are *trust-excluded* (P2.7 guardrail). Kind is the coarse 4-bucket **projection** of capabilities. Content-pinned fn-refs **sidestep** Unison's open problem ("when to infer concrete effects"). | ✅ P2.2.1 kind; ⏳ P2.3.x caps |
| **P2.4** — firing / quiescence (settle-gate before sinks) | **FRP glitch-freedom**: height / topological propagation (FrTime, Flapjax, REScala); distributed-quiescence + CRDT for async | Assign heights > deps; process via priority queue so a node fires **once** after inputs settle; explicit settle/commit gate before irreversible sinks | **Eager now** (P2.4.1): the graph settles to a fixed point before any sink fires, so quiescence is *free* (the synchronous-DAG case the literature already proves). Async + distributed quiescence is **deferred** (P2.4.2). **Refuse** per-edge debounce/sleep — the anti-pattern the design doc forbids. | ⏳ P2.4.1; ◇ P2.4.2 |
| **P3.3** — universal memoization (shared cache) | **Bazel Remote Execution API**: action-cache (hash → result) vs CAS (content-addressed outputs), shared across users; Nix binary caches; Salsa/Adapton (single-process) | The **action-cache-vs-CAS split**; key by `(closure-hash, input-hash)`; the reproducibility caveats | **Trustless, public, cross-user** (Bazel/Unison caches are within-trust-domain or local/ephemeral). Only **pure** closures are keyed (gated by P2.2.1). The hash *is* the proof. This is the contested-novel piece — validate the economics early. | ⏳ P3.3.1 local; ◇ P3.3.2 shared |
| **P3.1** — registry (alias→hash, publish, immutability) | Unison Share / UCM (names as aliases, append-only); package registries | Alias→hash resolution; publish; append-only immutability | Local-first; our hash. | ⏳ P3.1.1 |
| **P3.2** — transitive inspection to the leaves | Nix (`nix store diff-closures`, source closures); Unison (browse deps by hash); Bazel | Descend the **entire** closure to leaf primitives, by the same pins that guarantee execution | Inspection follows `children ∪ hash-valued params` (no level stops short — that would let a mislabeled effect hide). | ⏳ P3.2.1 |
| **P3.5** — copy-on-write fork + explicit re-pin | Nix (override inputs); Go modules (explicit bumps); Unison (append-only, names re-point) | Fork is COW; pulling upstream is an explicit, reviewable **re-pin**; no silent update channel | Ancestor re-derivation (bump child → re-hash upward) over our content-addressed units. | ⏳ P3.5.1 (needs L4) |
| **P3.6** — advisory / yank channel | **crates.io yank + RustSec advisory DB**; npm deprecate; Go checksum DB; Sigstore / SLSA (provenance) | Flag a hash as advised-against + point to a fixed version; **never mutate frozen bytes** | The flag is *data about* a hash beside the immutable CAS, never an edit of it. | ⏳ P3.6.1 |
| **P3.4** — reputation by adoption, not raw usage | PageRank over the dependency graph (npm / Julia package-centrality studies); trust-PageRank | Count **independent units/authors** that compose a unit (the dep graph counting itself); show adoption *and* usage | Adoption as the **primary** signal; mind the known pitfalls (centrality decay, rank-vs-score). | ◇ P3.4.1 |
| **P3.7** — author identity, costly to forge | Sigstore / SLSA provenance; Go checksum / transparency log; web-of-trust | Identity costly enough that the adoption metric isn't trivially Sybil'd | Model still open. | ◇ P3.7.1 |

---

## Build, don't adopt — the two rows with no design to port

These are the moat. There is nothing to copy because the value *is* the absence of
the usual dependency.

- **P4.2 · one evaluator, native == WASM, bit-identical.** No adoptable runtime
  satisfies this — it is precisely the invariant that rules out adopting Unison's
  core. Port the *principle* (a single deterministic decider — Unison itself models
  "one runtime format"), not code. `holon-core` is WASM-targetable **by
  construction** (pure Rust, `blake3`-only); the editor embeds the *same* core
  compiled to WASM. Two evaluators would eventually disagree → I5 false → the P3.3
  cache poisoned. ✅ core WASM-ready by construction; ⏳ P4.2.1 bindings.
- **P3.3.2 · the trustless cross-user memo market.** The differentiator. The Unison
  component that does this (Unison Cloud) is the proprietary part — unadoptable by
  definition, so it would be built either way. Building it ourselves is the point,
  not a cost adoption could have saved.

---

## Divergences worth never "fixing"

A future contributor seeing one of these in isolation may mistake it for an
omission. Each is deliberate and load-bearing:

1. **No effect discharge** (vs Unison handlers) — effects are observable; only
   design-mode mocks are handler-like, and they are excluded from trust decisions.
2. **Trustless / public / cross-user** memo cache (vs Bazel/Unison within-trust-domain).
3. **Eager-now quiescence** (vs a general async engine) — free fixed-point settle
   before sinks; async deferred, never faked with debounce.
4. **Our own hash encoding** (vs inheriting a foreign AST hash) — so derive-then-freeze
   stays ours to control.

---

## Future: Unison as a differential test oracle

The strongest form of "adopt knowledge": not porting Unison's code, but using its
*correctness* as a check on ours. Its ability inference is the proven reference for
exactly the derivation we build at P2.2 / P2.3. With a free OSS eval license, we
could (later) cross-check `effects_of` against Unison's inferred abilities on
equivalent graphs. Adopt the lessons; still build — and now *validate* — the code.

---

## Provenance & tripwire

- Sources: two commissioned reviews (June 2026) — a prior-art / competitive-landscape
  analysis and a build-vs-adopt analysis — plus the primary docs they cite (Unison,
  Nix, Bazel, Roc, crates.io / RustSec) and the FRP / effect-system literature.
- **Competitive tripwire:** the one event that flips the memo-market verdict — Unison
  Cloud shipping a *public, trustless, cross-user* result cache. Watch their releases
  (Unison hit production v1.0 in Nov 2025; Cloud is actively evolving and partly
  behind a beta, so the public/proprietary boundary here may shift).
