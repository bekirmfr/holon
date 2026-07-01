//! # holon-host — the imperative shell (deploy-mode firing + state)
//!
//! The first and only impurity. `holon-core` settles a graph to a *request* (Fork B: a
//! sink purely constructs its payload, so the request is ordinary output data); this
//! crate **performs** it. The crate dependency `holon-host → holon-core` (never the
//! reverse) *is* the purity boundary — `holon-core` carries no IO, async, or network
//! dependency, ever. The only impure function in the system is `Host::perform`.
//!
//! Deploy firing is **eager**: `eval` returns the settled value before anything is
//! performed, so no sink ever acts on a mid-propagation transient. For an acyclic graph
//! the fixed point is one `eval` pass, so each effect is performed once with its
//! committed payload.
//!
//! [`StateHost`] adds the one fenced state cell (`P2.6.1`): its storage is the `Model`;
//! a `store_read` sources it, a `store_write` (a Fork-B sink, capability `StateWrite`)
//! commits to it. The feedback that looked like a graph cycle is broken by storage
//! *between rounds* — no `validate` change, no `eval` change.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::PathBuf;

use holon_core::{
    capabilities, capabilities_of, child_refs, decode_preimage, design_eval, encode_preimage, eval,
    eval_one, entry_key, hash_unit, memo_key, refork, verify_entry, adoption_counts, CacheEntry,
    MapResolver, Refork, Capability, DecodeError, EncodeError, Hash, Param, Port, Resolver, Type,
    Unit, UnitBody, Value,
};

/// The platform: the one place IO happens. A real host sends the mail / writes the file
/// / stores the value; a test host records. `holon-core` never sees this trait.
pub trait Host {
    /// Perform one settled request — the **only** impure operation in the system.
    fn perform(&mut self, capability: Capability, payload: &Value);
}

/// Deploy-mode firing: settle the graph (core — the one decider), then perform the
/// effect(s) its output *carries* (host — the only impurity). `eval` is *called*, never
/// re-implemented (`I5`). Under representation **B** the output is a self-describing
/// `Effect { sink, payload }`, so performance is driven by the **output** (what actually
/// reached the boundary), not the manifest — the manifest stays the trust disclosure.
///
/// Uses `eval_one`, not core's `eval` directly (P4.7: `eval` now returns
/// `Vec<Option<Value>>`, since multi-output is a composite capability). Deliberately kept
/// `Option<Value>` here, not widened: `fire` is the host's firing loop for a *service*
/// graph — a settled request the host inspects for effects — not the multi-output
/// data-composite case P4.7 exists for. `eval_one` already fails closed for a genuinely
/// multi-output root (refusing rather than silently picking one), so nothing here hides
/// a real multi-output unit; it just isn't what this function is for.
pub fn fire(root: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver, host: &mut impl Host) -> Option<Value> {
    let output = eval_one(root, inputs, resolver)?;
    perform_effects(&output, host);
    Some(output)
}

/// Perform the effect(s) a settled output carries. A single `Effect`, or a `List` of them
/// (and nested lists) — the **computed fan-out**: `map(egress, xs)` yields a `List[Effect]`,
/// performed in order. Pure data carries nothing. The capability is *derived* from the sink's
/// name (`capabilities_of`), so the `Host` trait is unchanged and a graph can only perform a
/// capability it structurally declares — the effect was constructed by a leaf that has it.
fn perform_effects(value: &Value, host: &mut impl Host) {
    match value {
        Value::Effect { sink, payload } => {
            if let Some(caps) = capabilities_of(sink) {
                for cap in caps.iter() {
                    if !cap.is_sourced() {
                        host.perform(cap, &**payload);
                    }
                }
            }
        }
        Value::List(items) => {
            for item in items {
                perform_effects(item, host); // flatten lists of effects (and nested lists)
            }
        }
        _ => {} // pure data at the root carries nothing to perform
    }
}

// ── Drive: source the world (and state) at sourced leaves ──────────────────────────

/// Fire a graph with its **sourced** inputs supplied: each `(capability, value)` injects
/// `value` at every leaf carrying that sourced capability — `Clock`, ingress, `StateRead`, …
/// This is the one entry every mode funnels through; a leaf with no pure `compute` (a driver,
/// a state read) gets its value here, or — if unsupplied — evaluates to nothing (deploy's
/// honest asymmetry with design mode, which substitutes a default).
pub fn fire_sourced(
    root: &Unit,
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) -> Option<Value> {
    let resolver = SourcedResolver::new(sourced, real);
    fire(root, &[], &resolver, host)
}

/// The two firing modes the host offers over one graph. The **core never learns about them** —
/// both call the same `eval`; the mode only chooses what the resolver injects and whether effects
/// are performed.
pub enum Mode {
    /// Drivers substituted by mocks — the whole graph goes pure, so the run is deterministic,
    /// replayable, and **performs nothing**. A safe preview: a sink still *constructs* its request
    /// (visible in the settled output), but no effect is ever performed.
    Design,
    /// Drivers sourced from the world, effects **performed** for real.
    Deploy,
}

/// Run a graph in `Design` (mocked, performs nothing) or `Deploy` (real, performs), returning the
/// settled output either way. Design substitutes mocks at resolve time (`design_eval`); deploy
/// supplies real sourced values (`fire_sourced`) — the same `eval` underneath both.
pub fn run(
    mode: Mode,
    graph: &Unit,
    drivers: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) -> Option<Value> {
    match mode {
        Mode::Design => design_eval(graph, &[], real), // host untouched — nothing performed
        Mode::Deploy => fire_sourced(graph, drivers, real, host),
    }
}

// ── State: one anonymous cell threaded across rounds (P2.6.1) ──────────────────────

/// A host that threads a single anonymous state cell across firing rounds. Storage is
/// the `Model`: a `store_read` sources the round-start snapshot, a `store_write` commits
/// the new value, which the round applies at the end — Elm's `update(old) → (new,
/// cmds)`, atomically.
pub struct StateHost {
    storage: Value,
    pending: Vec<(Capability, Value)>,
}

impl StateHost {
    pub fn new(initial: Value) -> Self {
        StateHost { storage: initial, pending: Vec::new() }
    }

    pub fn storage(&self) -> &Value {
        &self.storage
    }

    /// One firing round: source `StateRead` from storage (internal, threaded) and the given
    /// `drivers` from the world (external, this event's inputs), fire, then apply the
    /// `StateWrite` to storage. Reads see the round-start values; the write lands at round-end
    /// and loops back. With `drivers` empty this is the pure state loop.
    pub fn round(&mut self, graph: &Unit, drivers: &[(Capability, Value)], real: &impl Resolver) {
        // Sourced inputs: storage for `StateRead` (threaded), the world for the drivers. The
        // resolver injects both uniformly; only the internal one (`StateWrite`) is committed
        // back below — drivers have no write-back.
        let mut sourced = vec![(Capability::StateRead, self.storage.clone())];
        sourced.extend_from_slice(drivers);
        self.pending.clear();
        fire_sourced(graph, &sourced, real, self);
        for (cap, payload) in std::mem::take(&mut self.pending) {
            if cap == Capability::StateWrite {
                self.storage = payload; // the Model-out (loops back to next round's read)
            }
        }
    }
}

impl Host for StateHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.pending.push((capability, payload.clone()));
    }
}

/// A resolver that injects a host-supplied constant at every **sourced** leaf — `StateRead`,
/// `Clock`, ingress, … — keyed by capability (one value per kind: the anonymous cell, the
/// anonymous clock; keyed/multiple resources defer with the capability model). It **owns** the
/// constants it hands out, since `resolve` returns `&Unit`. Performed leaves (sinks,
/// `store_write`) are never substituted, so the effect manifest is intact.
struct SourcedResolver<'a, R: Resolver> {
    sourced: Vec<(Capability, Unit)>,
    real: &'a R,
}

impl<'a, R: Resolver> SourcedResolver<'a, R> {
    fn new(sourced: &[(Capability, Value)], real: &'a R) -> Self {
        // A value that can't be faithfully reconstructed (`constant_unit` → `None`, currently
        // only `Sum`) is dropped here rather than substituted — which routes it through the
        // SAME seam `fire_sourced`'s doc comment already names for an unsupplied capability:
        // "evaluates to nothing." No new failure mode; reuse of the existing honest one.
        let sourced = sourced
            .iter()
            .filter_map(|(cap, value)| constant_unit(value).map(|u| (*cap, u)))
            .collect();
        SourcedResolver { sourced, real }
    }
}

impl<R: Resolver> Resolver for SourcedResolver<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        let u = self.real.resolve(hash)?;
        if let UnitBody::Primitive { name } = &u.body {
            if let Some(caps) = capabilities_of(name) {
                // Inject the first sourced capability we hold a value for (one per leaf in the
                // minimal set: clock→Clock, store_read→StateRead).
                for (cap, constant) in &self.sourced {
                    if caps.contains(*cap) {
                        return Some(constant);
                    }
                }
            }
        }
        Some(u)
    }
}

/// A pure leaf that evaluates to `value` — used to inject the held state at the
/// `store_read` boundary. Scalar; non-scalar state is deferred (a standalone unit can't
/// carry list items without children-by-hash, and the minimal cell is scalar).
///
/// Fallible (`None`) where `value` cannot be **faithfully** reconstructed by a single
/// niladic primitive — currently only `Sum`. This is reachable, not hypothetical: both
/// `store_read`/`store_write` are `Any`-typed (no type-system gate), so a state cell can
/// genuinely hold an `Option`. Unlike `Effect` (architecturally impossible here — `unreachable!`
/// is correct), a stored `Sum` is a missing *capability*, not a bug: there is no niladic
/// constructor for an arbitrary arm (`some(x)` needs `x` as an input; only `none()` is
/// niladic). Defaulting to `none()` regardless of the value's real arm would silently
/// **fabricate the wrong arm** — exactly the "value to fabricate" the `Effect` comment above
/// already refuses to do, just for a reachable case instead of an impossible one. So this
/// fails honestly; the caller drops the capability rather than inject a lie.
fn constant_unit(value: &Value) -> Option<Unit> {
    Some(match value {
        Value::Number(n) => scalar("Number", n.to_string(), Type::Number),
        Value::Boolean(b) => scalar("Boolean", b.to_string(), Type::Boolean),
        Value::Text(s) => scalar("Text", s.clone(), Type::Text),
        Value::List(_) => Unit {
            body: UnitBody::Primitive { name: "list".into() },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        },
        // A state cell never holds an effect: a write commits the *payload* (`&**payload`),
        // never the request itself. If this fires, the round stored an `Effect` by mistake —
        // a bug to fix, not a value to fabricate.
        Value::Effect { .. } => unreachable!("a state cell never holds an effect"),
        Value::Sum { .. } => return None,
    })
}

fn scalar(ty_name: &str, value: String, ty: Type) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", ty_name), Param::frozen("value", value)],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty }],
    }
}

// ── P3.8.1 · Content-addressed store (local CAS) ─────────────────────────────
//
// Where units live, keyed by their 32-byte hash. A `Store` is **not** a
// `Resolver`: a `Resolver` hands out `&Unit` for the in-memory eval hot path,
// whereas a store persists and returns *owned* units — so evaluation never
// reaches for disk (IO stays out of the core). The stored bytes are exactly the
// hash preimage (`encode_preimage`), so verify-on-read is pure content-
// addressing: decode the blob, re-hash the decoded unit, and refuse it unless it
// equals the key it was filed under. No `blake3` in the host; the proof is the
// hash itself.

/// What can go wrong putting or getting a unit. Holds an `io::Error`, so it is
/// deliberately not `PartialEq` — match on the variant.
#[derive(Debug)]
pub enum StoreError {
    /// Disk IO failed.
    Io(io::Error),
    /// The unit could not be encoded (a field over the 32-bit budget).
    Encode(EncodeError),
    /// The stored bytes are not a well-formed preimage.
    Decode(DecodeError),
    /// The blob's bytes do not hash to the key it was filed under — corruption,
    /// tampering, or a key/blob mismatch. The store refuses it.
    Corrupt { key: Hash, actual: Hash },
}

/// A content-addressed store: persist a unit by its hash, fetch it back by that
/// hash. Implementations differ only in the byte backend (memory, disk, and
/// later a networked CAS, `P3.8.2`); the encode / key / verify / decode path is
/// shared, so the content-addressing guarantee is identical across all of them.
pub trait Store {
    /// Persist `unit`; returns the hash it is filed under (its identity).
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError>;
    /// Fetch the unit filed under `key`. `Ok(None)` if absent; `Err(Corrupt)` if
    /// the stored bytes do not hash to `key`.
    fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError>;
}

/// Encode a unit to its canonical bytes and the key they must be filed under.
fn encode_keyed(unit: &Unit) -> Result<(Hash, Vec<u8>), StoreError> {
    let bytes = encode_preimage(unit).map_err(StoreError::Encode)?;
    let key = hash_unit(unit).map_err(StoreError::Encode)?;
    Ok((key, bytes))
}

/// Decode a fetched blob and verify it against the key it was filed under — the
/// content-addressing guarantee: the bytes are trusted only because they hash to
/// their own name.
fn decode_verified(key: &Hash, bytes: &[u8]) -> Result<Unit, StoreError> {
    let unit = decode_preimage(bytes).map_err(StoreError::Decode)?;
    let actual = hash_unit(&unit).map_err(StoreError::Encode)?;
    if &actual != key {
        return Err(StoreError::Corrupt { key: *key, actual });
    }
    Ok(unit)
}

fn hex(key: &Hash) -> String {
    let mut s = String::with_capacity(64);
    for b in key {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// In-memory content-addressed store. For tests and any in-process need; shares
/// the exact encode / verify / decode path with the on-disk store.
#[derive(Default)]
pub struct MemStore {
    blobs: BTreeMap<Hash, Vec<u8>>,
}

impl MemStore {
    pub fn new() -> Self {
        MemStore::default()
    }
}

impl Store for MemStore {
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError> {
        let (key, bytes) = encode_keyed(unit)?;
        self.blobs.insert(key, bytes);
        Ok(key)
    }
    fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError> {
        match self.blobs.get(key) {
            Some(bytes) => decode_verified(key, bytes).map(Some),
            None => Ok(None),
        }
    }
}

/// On-disk content-addressed store: one file per unit, named by its hex hash,
/// under a single root directory. Where units actually live on a machine.
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    /// Open (creating if needed) a store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(FileStore { root })
    }
    fn path(&self, key: &Hash) -> PathBuf {
        self.root.join(hex(key))
    }
}

impl Store for FileStore {
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError> {
        let (key, bytes) = encode_keyed(unit)?;
        fs::write(self.path(&key), &bytes).map_err(StoreError::Io)?;
        Ok(key)
    }
    fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError> {
        let bytes = match fs::read(self.path(key)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e)),
        };
        decode_verified(key, &bytes).map(Some)
    }
}

// ── P3.1.1 The registry — the publish boundary ───────────────────────────────
/// The first **permanent public commitment** of a hash. Publishing a unit moves it from a
/// private working artifact to one others may depend on, so its meaning is a single
/// invariant: **closure-completeness** (I6 at the boundary). A composite references its
/// children by hash (I3); a higher-order unit references its `fn` by hash in a param (I6).
/// [`Registry::publish`] walks that closure and **fails closed** on any reference the store
/// cannot resolve — a dangling hash would be fetchable yet un-evaluable, poisoning the trust
/// root the instant it is committed. The store's verify-on-read means every resolved
/// reference is, by construction, the right bytes, so the walk integrity-checks for free.
///
/// Scope (P3.1.1): hash-only (no names — hash *is* identity); the published record lives in
/// memory (the *units* are persisted by the store; persisting the record is a short
/// follow-on); no networking. The closure walk uses the shared [`holon_core::child_refs`]
/// (P3.2.1) — children plus the `fn`-named param — the one definition of "what a unit
/// references" that capability classification, design-mode mocking, and this publish path all
/// agree on, so none can disagree about what must be present.
#[derive(Debug)]
pub enum PublishError {
    /// The store failed to read or write.
    Store(StoreError),
    /// A referenced hash (a child, or a `fn` param) is absent from the store. Publishing it
    /// would commit a dangling reference — refused.
    DanglingReference(Hash),
}

/// Why a [`Registry::fork`] failed.
#[derive(Debug)]
pub enum ForkError {
    /// The store failed to read or write.
    Store(StoreError),
    /// A re-pinned unit could not be encoded for the resolver (input too large).
    Encode(EncodeError),
    /// Publishing the forked root failed (e.g. a dangling reference).
    Publish(PublishError),
    /// No unit is stored at the given root hash.
    RootNotFound(Hash),
    /// `target` is not referenced anywhere in the root's closure — nothing to fork.
    TargetNotInClosure(Hash),
    /// The root's own closure was incomplete in the store (a reference did not resolve). A
    /// published root cannot hit this; an unpublished/partial one can.
    DanglingInClosure(Hash),
}

/// A published catalog backed by a content-addressed [`Store`].
pub struct Registry<S: Store> {
    store: S,
    published: BTreeSet<Hash>,
}

impl<S: Store> Registry<S> {
    /// A registry over `store`, nothing published yet.
    pub fn new(store: S) -> Self {
        Registry { store, published: BTreeSet::new() }
    }

    /// Publish `root`: verify its whole closure is present (else fail closed), then commit it
    /// to the store and record its hash. Verify-first-then-commit means a refused publish
    /// leaves nothing extra. Idempotent — re-publishing returns the same hash.
    pub fn publish(&mut self, root: &Unit) -> Result<Hash, PublishError> {
        verify_closure(root, &self.store)?; // every reference resolves, or DanglingReference
        let h = self.store.put(root).map_err(PublishError::Store)?;
        self.published.insert(h);
        Ok(h)
    }

    /// Whether `h` is an explicitly published entry. (Dependencies inside a published closure
    /// are verified-present and resolvable, but are not themselves listed unless published.)
    pub fn is_published(&self, h: &Hash) -> bool {
        self.published.contains(h)
    }

    /// The published entries, in hash order.
    pub fn published(&self) -> impl Iterator<Item = &Hash> {
        self.published.iter()
    }

    /// The backing store (read-only) — for resolving a published unit's closure.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Fork a published unit: re-pin every reference to `target` → `replacement` in the unit at
    /// `root_hash`, then store the re-pinned spine and **publish** the new root. The new root's
    /// closure is complete by construction (minted spine ∪ shared-unchanged ∪ the replacement),
    /// so publishing reuses [`Registry::publish`]'s dangling gate verbatim. Returns the new
    /// root's hash — a clean, independent identity (no provenance link; see the L4).
    pub fn fork(
        &mut self,
        root_hash: &Hash,
        target: &Hash,
        replacement: &Unit,
    ) -> Result<Hash, ForkError> {
        let root = self
            .store
            .get(root_hash)
            .map_err(ForkError::Store)?
            .ok_or(ForkError::RootNotFound(*root_hash))?;
        let replacement_hash = self.store.put(replacement).map_err(ForkError::Store)?;

        // The store yields owned units (`get`); `refork` wants a `Resolver`. Materialize the
        // root's bounded closure into one. (For a published root this is always complete.)
        let mut resolver = MapResolver::new();
        self.materialize(&root, &mut resolver)?;

        let Refork { new_root, minted } = refork(&root, target, &replacement_hash, &resolver)
            .ok_or(ForkError::TargetNotInClosure(*target))?;

        for unit in minted.values() {
            self.store.put(unit).map_err(ForkError::Store)?;
        }
        // `new_root` is always a key in `minted`; publishing it reuses the closure gate.
        let new_root_unit = minted.get(&new_root).ok_or(ForkError::TargetNotInClosure(*target))?;
        self.publish(new_root_unit).map_err(ForkError::Publish)?;
        Ok(new_root)
    }

    /// Load a unit's transitive closure from the store into `resolver` (via the shared
    /// `child_refs`). A published root's closure is complete, so this never finds a gap.
    fn materialize(&self, unit: &Unit, resolver: &mut MapResolver) -> Result<(), ForkError> {
        for h in child_refs(unit) {
            if resolver.resolve(&h).is_some() {
                continue; // already materialized (a DAG can reach a node by many paths)
            }
            let u = self
                .store
                .get(&h)
                .map_err(ForkError::Store)?
                .ok_or(ForkError::DanglingInClosure(h))?;
            resolver.insert(u.clone()).map_err(ForkError::Encode)?;
            self.materialize(&u, resolver)?;
        }
        Ok(())
    }

    /// Adoption over the **published universe** (every unit in any published root's closure): for
    /// each unit, how many *distinct* units cite it. **Derived fresh from the store on every call —
    /// never a stored tally** (I8), so adoption cannot be inflated except by publishing real,
    /// inspectable units. The marketplace's discovery signal: which units are widely built upon.
    /// (Flat citation count — not Sybil-resistant; recursive weighting is deferred, see the L4.)
    pub fn adoption(&self) -> Result<BTreeMap<Hash, usize>, StoreError> {
        let mut corpus = BTreeMap::new();
        for root in &self.published {
            self.collect_closure(root, &mut corpus)?;
        }
        Ok(adoption_counts(corpus.values()))
    }

    /// Collect a hash's transitive closure (it + everything it reaches) from the store into `acc` —
    /// the `verify_closure` walk, collecting instead of checking. A published root's closure is
    /// complete (P3.1.1), so this never finds a gap.
    fn collect_closure(&self, h: &Hash, acc: &mut BTreeMap<Hash, Unit>) -> Result<(), StoreError> {
        if acc.contains_key(h) {
            return Ok(()); // a DAG reaches the same unit by many paths
        }
        if let Some(u) = self.store.get(h)? {
            let refs = child_refs(&u);
            acc.insert(*h, u);
            for r in refs {
                self.collect_closure(&r, acc)?;
            }
        }
        Ok(())
    }
}

/// Walk a unit's transitive closure via the shared [`holon_core::child_refs`] (children +
/// the `fn` param), requiring every reference to resolve in `store`. `None` ⇒
/// `DanglingReference`. Terminates for free: content-addressing forbids a cycle.
fn verify_closure<S: Store>(unit: &Unit, store: &S) -> Result<(), PublishError> {
    for h in child_refs(unit) {
        let referenced = store
            .get(&h)
            .map_err(PublishError::Store)?
            .ok_or(PublishError::DanglingReference(h))?;
        verify_closure(&referenced, store)?;
    }
    Ok(())
}

// ── P3.3.1 Local memoization — the cache thesis, made real ───────────────────
/// A memoizing evaluator: a content-addressed cache of pure units' results. The thesis (§8)
/// is that a pure, content-pinned unit is a function from inputs to output that never changes,
/// so its result is cacheable and re-servable, keyed by `(unit-hash, inputs)`. The cache *is*
/// a state cell keyed by a content hash ([`memo_key`]) — the recursion the design points at:
/// the OS's universal donor (a state cell) storing the language's pure results.
///
/// **Soundness guard:** only [`is_pure`](holon_core::Capabilities::is_pure) units (∅
/// capabilities) are cached. A driver (`clock`/`random`) is not a function of its inputs (it
/// varies per call); a sink (`egress`/`store_write`) *performs* — serving a cached value would
/// skip the effect. Impure units fall through to ordinary [`eval`], uncached.
///
/// Scope (P3.3.1): in-memory (no persistence); top-level only (the root pure unit, not every
/// pure sub-node within one eval); no networking (the shared cache, P3.3.2, is the actual
/// differentiator, still deferred). The purity walk and the re-hash per call are honest
/// overhead — this inch is about correctness, not yet speed.
#[derive(Default)]
pub struct Memo {
    cache: BTreeMap<Hash, Vec<Option<Value>>>,
}

impl Memo {
    /// An empty cache.
    pub fn new() -> Self {
        Memo::default()
    }

    /// Evaluate `unit` on `inputs`, serving a cached result when the unit is pure and this
    /// `(unit-hash, inputs)` key has been seen. Impure units are never cached. Behaviourally
    /// identical to [`eval`] — memoization is a pure optimization, so this mirrors `eval`'s
    /// own `Vec<Option<Value>>` shape exactly (P4.7), not `Option<Value>` narrowed to one
    /// output: a multi-output composite is exactly as cacheable as a single-output one, and
    /// narrowing this to `eval_one` would silently refuse to cache any pure multi-output unit
    /// even though `eval` itself computes it fine — breaking the "identical to eval" promise
    /// this function makes. A result with some (or all) slots `None` is still cached: for a
    /// *pure* unit, "this output is always absent for these inputs" is exactly as stable and
    /// deterministic a fact as any concrete value, the same reasoning `CacheEntry`'s own
    /// `outputs: Vec<Option<Value>>` already applies in the shared cache below.
    pub fn eval(
        &mut self,
        unit: &Unit,
        inputs: &[Option<Value>],
        resolver: &impl Resolver,
    ) -> Vec<Option<Value>> {
        if !capabilities(unit, resolver).is_pure() {
            return eval(unit, inputs, resolver); // impure ⇒ never cached
        }
        // Un-keyable (un-hashable unit, or an input too large to encode) ⇒ just evaluate.
        let key = match hash_unit(unit).ok().and_then(|h| memo_key(&h, inputs).ok()) {
            Some(k) => k,
            None => return eval(unit, inputs, resolver),
        };
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let out = eval(unit, inputs, resolver);
        self.cache.insert(key, out.clone());
        out
    }

    /// Number of cached results.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// A shared, **trustless** memo cache (P3.3.2): portable [`CacheEntry`]s filed under their
/// universal [`entry_key`]. The cache itself is *untrusted* — `put` is a dumb insert, and trust is
/// established on **read** via [`SharedCache::get_verified`], never assumed. This is what turns the
/// private local [`Memo`] (bare values it trusts because it computed them) into something a result
/// one party computed another can look up and *check*. (Local in-memory now; a networked backing is
/// P3.8.2, where the cheap `entry_key` integrity check earns its keep against a peer-supplied key.)
#[derive(Default)]
pub struct SharedCache {
    entries: BTreeMap<Hash, CacheEntry>,
}

impl SharedCache {
    pub fn new() -> Self {
        SharedCache { entries: BTreeMap::new() }
    }

    /// File an entry under its derived universal key (returned). **Dumb on purpose:** the store
    /// makes no correctness claim — readers verify. (Key integrity holds by construction here,
    /// since the key is *derived* from the entry, not taken on a peer's word.)
    pub fn put(&mut self, entry: CacheEntry) -> Result<Hash, EncodeError> {
        let key = entry_key(&entry)?;
        self.entries.insert(key, entry);
        Ok(key)
    }

    /// Fast path: the raw entry as filed, **unverified**. The caller is choosing to trust the
    /// source (or will verify separately).
    pub fn get(&self, key: &Hash) -> Option<&CacheEntry> {
        self.entries.get(key)
    }

    /// Trustless path: the outputs, returned **only if they re-execute correctly** against
    /// `resolver`. A liar — wrong output(s) filed under the right key — yields `None`. Returns
    /// the whole `Vec<Option<Value>>` (P4.7), mirroring `CacheEntry.outputs` exactly rather than
    /// narrowing to one value — a cached multi-output composite is exactly as verifiable as a
    /// single-output one, and this is a general-purpose cache read, not something inherently
    /// about single-output service graphs (unlike `fire`, which genuinely is).
    pub fn get_verified(&self, key: &Hash, resolver: &impl Resolver) -> Option<Vec<Option<Value>>> {
        let entry = self.entries.get(key)?;
        if verify_entry(entry, resolver) {
            Some(entry.outputs.clone())
        } else {
            None
        }
    }

    /// Trustless **ingest** of a peer-supplied entry claimed to live at `claimed_key` (P3.8.2 —
    /// the network boundary `entry_key` was built for). The cheap *structural* check: does the
    /// entry actually hash to the key it's offered under? Rejects a peer lying about *which
    /// computation* an entry is for — the cache's analogue of the `Store`'s verify-on-read.
    /// Semantic correctness (the *output*) remains the consumer's `get_verified` (re-execution):
    /// `ingest` is the cheap admission gate, not the trust decision, so a structurally-valid but
    /// wrong-output entry is admitted here and caught there. (Local `put` derives the key, so it
    /// can't model a claimed key; `ingest` is how an entry from *elsewhere* enters — a peer, or a
    /// cache dump.)
    pub fn ingest(&mut self, claimed_key: Hash, entry: CacheEntry) -> Result<(), CacheIngestError> {
        let actual = entry_key(&entry).map_err(CacheIngestError::Encode)?;
        if actual != claimed_key {
            return Err(CacheIngestError::KeyMismatch { claimed: claimed_key, actual });
        }
        self.entries.insert(claimed_key, entry);
        Ok(())
    }
}

/// Why a [`SharedCache::ingest`] was refused.
#[derive(Debug)]
pub enum CacheIngestError {
    /// The entry could not be encoded to derive its key (input too large).
    Encode(EncodeError),
    /// The entry does not hash to the key it was offered under — the peer mis-keyed it (lied about
    /// which computation it is for). The cache's analogue of `StoreError::Corrupt`.
    KeyMismatch { claimed: Hash, actual: Hash },
}

#[cfg(test)]
mod memo_tests {
    use super::*;
    use holon_core::{MapResolver, Port};

    fn prim(name: &str, n_in: usize, out: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: (0..n_in).map(|i| Port { name: format!("in{i}"), ty: Type::Any }).collect(),
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    #[test]
    fn pure_unit_is_cached() {
        let add = prim("add", 2, Type::Number);
        let r = MapResolver::new();
        let mut memo = Memo::new();
        assert!(memo.is_empty());
        assert_eq!(memo.eval(&add, &[Some(num(2.0)), Some(num(3.0))], &r), vec![Some(num(5.0))]);
        assert_eq!(memo.len(), 1); // result cached
        assert_eq!(memo.eval(&add, &[Some(num(2.0)), Some(num(3.0))], &r), vec![Some(num(5.0))]);
        assert_eq!(memo.len(), 1); // a hit — no new entry
    }

    #[test]
    fn different_inputs_make_different_entries() {
        let add = prim("add", 2, Type::Number);
        let r = MapResolver::new();
        let mut memo = Memo::new();
        memo.eval(&add, &[Some(num(2.0)), Some(num(3.0))], &r);
        memo.eval(&add, &[Some(num(2.0)), Some(num(4.0))], &r);
        assert_eq!(memo.len(), 2);
    }

    #[test]
    fn impure_unit_is_not_cached() {
        // `egress` is a sink (NetworkEgress): it *performs*, so serving a cached value would
        // skip the effect. It evaluates (constructs an Effect) but is never cached.
        let egress = prim("egress", 1, Type::Effect);
        let r = MapResolver::new();
        let mut memo = Memo::new();
        let out = memo.eval(&egress, &[Some(Value::Text("hi".into()))], &r);
        assert!(matches!(out.as_slice(), [Some(Value::Effect { .. })]));
        assert!(memo.is_empty()); // impure ⇒ not cached, regardless of result
    }
}

#[cfg(test)]
mod shared_cache_tests {
    use super::*;
    use holon_core::{MapResolver, Param, Type};

    fn const_num(n: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("value", n), Param::frozen("type", "Number")],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }

    #[test]
    fn put_files_under_the_universal_key_and_round_trips() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(4.0))] };
        let mut cache = SharedCache::new();
        let key = cache.put(entry.clone()).unwrap();
        assert_eq!(cache.get(&key), Some(&entry));
    }

    #[test]
    fn get_verified_returns_an_honest_output() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        let mut cache = SharedCache::new();
        let key = cache
            .put(CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(4.0))] })
            .unwrap();
        assert_eq!(cache.get_verified(&key, &r), Some(vec![Some(Value::Number(4.0))]));
    }

    #[test]
    fn get_verified_catches_a_liar() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        // right computation key, wrong output.
        let mut cache = SharedCache::new();
        let key = cache
            .put(CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(999.0))] })
            .unwrap();
        assert!(cache.get(&key).is_some()); // the lie *is* stored — the cache is untrusted
        assert_eq!(cache.get_verified(&key, &r), None); // but the trustless read refuses it
    }

    #[test]
    fn ingest_accepts_a_correctly_keyed_entry() {
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(4.0))] };
        let key = entry_key(&entry).unwrap(); // the peer offers the honest key
        let mut cache = SharedCache::new();
        cache.ingest(key, entry).unwrap();
        assert_eq!(cache.get_verified(&key, &r), Some(vec![Some(Value::Number(4.0))])); // behaves like a put
    }

    #[test]
    fn ingest_rejects_a_mis_keyed_entry() {
        // a peer offers an entry under a key it does not hash to (lying about which computation).
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(4.0))] };
        let wrong_key = [0u8; 32]; // not entry_key(&entry)
        let mut cache = SharedCache::new();
        match cache.ingest(wrong_key, entry) {
            Err(CacheIngestError::KeyMismatch { claimed, .. }) => assert_eq!(claimed, wrong_key),
            other => panic!("expected KeyMismatch, got {other:?}"),
        }
    }

    #[test]
    fn ingest_admits_structurally_valid_but_get_verified_catches_wrong_output() {
        // The two tiers: ingest is the cheap structural gate (right key), get_verified the expensive
        // semantic one (right output). A correctly-keyed entry with a wrong output passes ingest...
        let mut r = MapResolver::new();
        let h = r.insert(const_num("4")).unwrap();
        let entry = CacheEntry { unit_hash: h, inputs: vec![], outputs: vec![Some(Value::Number(999.0))] };
        let key = entry_key(&entry).unwrap(); // correctly keyed for the (unit, inputs)...
        let mut cache = SharedCache::new();
        cache.ingest(key, entry).unwrap(); // ...so ingest admits it
        assert_eq!(cache.get_verified(&key, &r), None); // ...but re-execution catches the wrong output
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use holon_core::{Edge, Port, BOUNDARY};

    fn num_const(v: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", v)],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn passthrough(child: Hash) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children: vec![child],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }

    #[test]
    fn publishes_a_leaf() {
        let mut reg = Registry::new(MemStore::new());
        let h = reg.publish(&num_const("5")).expect("leaf has empty closure");
        assert!(reg.is_published(&h));
        assert!(reg.store().get(&h).unwrap().is_some()); // committed to the store
    }

    #[test]
    fn publishes_a_composite_whose_children_are_stored() {
        let mut store = MemStore::new();
        let ch = store.put(&num_const("7")).unwrap(); // child present first
        let comp = passthrough(ch);
        let mut reg = Registry::new(store);
        let h = reg.publish(&comp).expect("closure is complete");
        assert!(reg.is_published(&h));
    }

    #[test]
    fn dangling_child_is_refused() {
        // a composite referencing a child that was never stored
        let absent = hash_unit(&num_const("99")).unwrap();
        let comp = passthrough(absent);
        let mut reg = Registry::new(MemStore::new());
        match reg.publish(&comp) {
            Err(PublishError::DanglingReference(h)) => assert_eq!(h, absent),
            other => panic!("expected DanglingReference, got {other:?}"),
        }
        assert!(!reg.is_published(&hash_unit(&comp).unwrap())); // nothing committed
    }

    #[test]
    fn dangling_fn_param_is_refused() {
        // a `map` whose `fn` hash was never stored — the I6 subtlety a children-only walk misses
        let absent = hash_unit(&num_const("1")).unwrap();
        let map = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&absent))],
            inputs: vec![Port { name: "list".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let mut reg = Registry::new(MemStore::new());
        match reg.publish(&map) {
            Err(PublishError::DanglingReference(h)) => assert_eq!(h, absent),
            other => panic!("expected DanglingReference, got {other:?}"),
        }
    }

    #[test]
    fn republish_is_idempotent() {
        let mut reg = Registry::new(MemStore::new());
        let leaf = num_const("3");
        let h1 = reg.publish(&leaf).unwrap();
        let h2 = reg.publish(&leaf).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(reg.published().count(), 1); // one entry, not two
    }

    fn two_child(a: Hash, b: Hash) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children: vec![a, b],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn children_of(u: &Unit) -> Vec<Hash> {
        match &u.body {
            UnitBody::Composite { children, .. } => children.clone(),
            _ => vec![],
        }
    }

    #[test]
    fn fork_repins_and_republishes() {
        let mut store = MemStore::new();
        let ca = store.put(&num_const("1")).unwrap();
        let cb = store.put(&num_const("2")).unwrap();
        let root = two_child(ca, cb);
        let root_hash = store.put(&root).unwrap();
        let mut reg = Registry::new(store);
        reg.publish(&root).unwrap();

        // fork: replace child A with a new const.
        let cc = num_const("3");
        let new_root = reg.fork(&root_hash, &ca, &cc).unwrap();

        assert_ne!(new_root, root_hash); // a new identity
        assert!(reg.is_published(&new_root)); // and it's published
        let nr = reg.store().get(&new_root).unwrap().unwrap();
        let cc_hash = hash_unit(&cc).unwrap();
        // Child order is canonicalized in the encoder, so assert the fork's effect set-wise:
        // A was replaced by C, B is shared, A is gone.
        let kids = children_of(&nr);
        assert_eq!(kids.len(), 2);
        assert!(kids.contains(&cc_hash)); // A → C
        assert!(kids.contains(&cb)); // B shared
        assert!(!kids.contains(&ca)); // A gone
    }

    #[test]
    fn fork_target_not_in_closure_errors() {
        let mut store = MemStore::new();
        let ca = store.put(&num_const("1")).unwrap();
        let root = passthrough(ca);
        let root_hash = store.put(&root).unwrap();
        let mut reg = Registry::new(store);
        reg.publish(&root).unwrap();

        let absent = hash_unit(&num_const("99")).unwrap();
        let repl = num_const("3");
        match reg.fork(&root_hash, &absent, &repl) {
            Err(ForkError::TargetNotInClosure(h)) => assert_eq!(h, absent),
            other => panic!("expected TargetNotInClosure, got {other:?}"),
        }
    }

    // ── P3.4.1: adoption over the published universe ──

    #[test]
    fn a_published_root_cites_its_child() {
        let mut store = MemStore::new();
        let ch = store.put(&num_const("1")).unwrap();
        let root = passthrough(ch);
        store.put(&root).unwrap();
        let mut reg = Registry::new(store);
        reg.publish(&root).unwrap();
        assert_eq!(reg.adoption().unwrap().get(&ch), Some(&1));
    }

    #[test]
    fn two_roots_citing_one_child_sum() {
        let mut store = MemStore::new();
        let ch = store.put(&num_const("1")).unwrap();
        let ch2 = store.put(&num_const("2")).unwrap();
        let r1 = passthrough(ch); // cites ch
        let r2 = two_child(ch, ch2); // cites ch and ch2 — a distinct root
        store.put(&r1).unwrap();
        store.put(&r2).unwrap();
        let mut reg = Registry::new(store);
        reg.publish(&r1).unwrap();
        reg.publish(&r2).unwrap();
        let adoption = reg.adoption().unwrap();
        assert_eq!(adoption.get(&ch), Some(&2)); // r1 and r2
        assert_eq!(adoption.get(&ch2), Some(&1)); // r2 only
    }

    #[test]
    fn a_deep_library_unit_is_counted() {
        // root → lib → leaf. The leaf is cited by `lib` (an intermediate, not the root). Adoption
        // is over the whole published universe, so that citation still counts.
        let mut store = MemStore::new();
        let leaf = store.put(&num_const("1")).unwrap();
        let lib = store.put(&passthrough(leaf)).unwrap(); // lib cites leaf
        let root = passthrough(lib); // root cites lib
        store.put(&root).unwrap();
        let mut reg = Registry::new(store);
        reg.publish(&root).unwrap();
        let adoption = reg.adoption().unwrap();
        assert_eq!(adoption.get(&lib), Some(&1)); // cited by root
        assert_eq!(adoption.get(&leaf), Some(&1)); // cited by a non-root — still counted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{Edge, MapResolver, Port, BOUNDARY};

    /// A recording host: captures what *would* be performed, so the loop is testable
    /// without touching the world.
    #[derive(Default)]
    struct Recorder {
        performed: Vec<(Capability, Value)>,
    }
    impl Host for Recorder {
        fn perform(&mut self, capability: Capability, payload: &Value) {
            self.performed.push((capability, payload.clone()));
        }
    }

    fn egress() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "egress".into() },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        }
    }
    fn nport(name: &str) -> Port {
        Port { name: name.into(), ty: Type::Number }
    }
    fn prim(name: &str, inputs: Vec<Port>, output: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs,
            outputs: vec![Port { name: "out".into(), ty: output }],
        }
    }
    fn number(n: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", n)],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    #[test]
    fn bare_sink_is_performed_once_with_settled_payload() {
        let r = MapResolver::new();
        let mut host = Recorder::default();
        fire(&egress(), &[Some(text("hi"))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::NetworkEgress, text("hi"))]);
    }

    #[test]
    fn sink_in_a_composite_is_performed() {
        let mut r = MapResolver::new();
        let eh = r.insert(egress()).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![eh],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(text("ship it"))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::NetworkEgress, text("ship it"))]);
    }

    #[test]
    fn pure_graph_performs_nothing() {
        let r = MapResolver::new();
        let constant = number("5");
        let mut host = Recorder::default();
        fire(&constant, &[], &r, &mut host);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn state_write_is_performed() {
        // A store_write is now an output-side effect (`!is_sourced`): fire performs it.
        let mut r = MapResolver::new();
        let wh = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![wh],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![nport("v")],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(num(42.0))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::StateWrite, num(42.0))]);
    }

    /// store_read → +1 → store_write: the canonical one-cell counter. Shared by the state and
    /// fork tests.
    fn counter_graph() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let wr = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let counter = Unit {
            body: UnitBody::Composite {
                children: vec![rd, one, add, wr],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 2, to_port: 0 }, // read → add.a
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 1 }, // one  → add.b
                    Edge { from_child: 2, from_port: 0, to_child: 3, to_port: 0 }, // add  → write.v
                    Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        (r, counter)
    }

    #[test]
    fn counter_remembers_across_firings() {
        // store_read → +1 → store_write. The host threads storage through each round:
        // reads see the prior write, and `eval` performed none of it.
        let (r, counter) = counter_graph();
        let mut host = StateHost::new(num(0.0));
        assert_eq!(host.storage(), &num(0.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(1.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(2.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(3.0));
    }

    #[test]
    fn sum_in_state_fails_honestly_not_fabricated() {
        // store_write/store_read are Any-typed, so a Sum (Option) CAN reach a state cell —
        // this isn't hypothetical. constant_unit has no niladic constructor for an arbitrary
        // arm (only `none()` is niladic), so reconstructing a held `some(x)` is not possible
        // without fabricating an answer. The required property: it must NOT silently default
        // to `none()` (which would silently flip a Some to a None — a wrong-arm bug, not a
        // missing-precision one). It drops the capability instead, the same honest "evaluates
        // to nothing" path an unsupplied capability already takes.
        let held = Value::Sum { arm: 0, payload: Box::new(num(5.0)) }; // a stored `some(5)`
        assert_eq!(constant_unit(&held), None);

        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Any)).unwrap();
        let passthrough = Unit {
            body: UnitBody::Composite {
                children: vec![rd],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let mut host = Recorder::default();
        let sourced = [(Capability::StateRead, held)];
        // No panic, no fabricated `none()` — the read simply yields nothing, exactly like an
        // unsupplied capability.
        assert_eq!(fire_sourced(&passthrough, &sourced, &r, &mut host), None);
    }

    // ── slice 2: computed fan-out ──────────────────────────────────────────────
    #[test]
    fn map_fans_out_one_egress_per_element() {
        // map(egress, [a,b,c]) ⇒ List[Effect{egress,a}, …] ⇒ three sends, in order. The
        // one-effect ceiling is lifted: a single graph performs many effects.
        let mut r = MapResolver::new();
        let eh = r.insert(egress()).unwrap();
        let mapper = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&eh))],
            inputs: vec![Port { name: "xs".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let xs = Value::List(vec![text("a"), text("b"), text("c")]);
        let mut host = Recorder::default();
        fire(&mapper, &[Some(xs)], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::NetworkEgress, text("a")),
                (Capability::NetworkEgress, text("b")),
                (Capability::NetworkEgress, text("c")),
            ]
        );
    }

    #[test]
    fn heterogeneous_effects_fire_in_one_round() {
        // One output carrying two *different* effects — a state write AND a send — both fire,
        // with no multi-output composite: the self-describing List[Effect] is enough.
        let mut r = MapResolver::new();
        let sw = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let em = r.insert(egress()).unwrap();
        let lst = r
            .insert(prim(
                "list",
                vec![
                    Port { name: "x".into(), ty: Type::Effect },
                    Port { name: "y".into(), ty: Type::Effect },
                ],
                Type::list_any(),
            ))
            .unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![sw, em, lst],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, // v   → store_write
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 1, to_port: 0 }, // msg → egress
                    Edge { from_child: 0, from_port: 0, to_child: 2, to_port: 0 },        // write → list.x
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 1 },        // egress → list.y
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // list → out
                ],
            },
            params: vec![],
            inputs: vec![nport("v"), Port { name: "msg".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(num(7.0)), Some(text("hi"))], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::StateWrite, num(7.0)),
                (Capability::NetworkEgress, text("hi")),
            ]
        );
    }

    // ── drive: source the world ────────────────────────────────────────────────
    /// clock (Clock) → store_write: read the clock, commit it. A graph whose driver leaf
    /// (`clock`) has no pure compute — it must be sourced.
    fn timer_to_store() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let th = r.insert(prim("clock", vec![], Type::Number)).unwrap();
        let wh = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![th, wh],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // clock → write.v
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        (r, graph)
    }

    #[test]
    fn round_sources_a_driver() {
        // A driver (Clock) is sourced from the round's supply — the world flows in, through the
        // graph, and here lands in storage. One round threads state AND injects the world.
        let (r, graph) = timer_to_store();
        let mut host = StateHost::new(num(0.0));
        host.round(&graph, &[(Capability::Clock, num(99.0))], &r);
        assert_eq!(host.storage(), &num(99.0));
    }

    #[test]
    fn an_undriven_driver_yields_nothing() {
        // Deploy's deliberate asymmetry with design mode: an undriven driver has no compute, so
        // the graph evaluates to None and nothing is performed — no silent default.
        let (r, graph) = timer_to_store();
        let mut host = Recorder::default();
        fire_sourced(&graph, &[], &r, &mut host); // no Clock supplied
        assert!(host.performed.is_empty());
    }

    #[test]
    fn forking_a_cells_state_is_copy_on_write() {
        // P2.6.3, the only sub-part with observable behavior. Fork = a new host over a *clone* of
        // the current state; the two then diverge independently. Copy-on-write falls out of
        // storage living in the host (P2.6.1), not the unit — no new mechanism. Identity is
        // shared (both run the same `&counter`, one graph); state is per-host.
        let (r, counter) = counter_graph();
        let mut original = StateHost::new(num(5.0));
        original.round(&counter, &[], &r); // original: 6

        let mut fork = StateHost::new(original.storage().clone()); // CoW: copy current state (6)
        original.round(&counter, &[], &r); // original: 7
        original.round(&counter, &[], &r); // original: 8
        fork.round(&counter, &[], &r); // fork: 7 (from its own 6, untouched by original)

        assert_eq!(original.storage(), &num(8.0));
        assert_eq!(fork.storage(), &num(7.0)); // diverged — independent runtime state
    }

    #[test]
    fn saturating_counter_is_one_atomic_conditional_update() {
        // P2.6.2 by dissolution. A conditional read-modify-write — write N+1 only while N < cap,
        // else hold — *composes* from store_read → compare → select → store_write, and the round
        // makes it atomic: the value written is computed from the value read with no gap, because
        // a round is snapshot → fire → apply and rounds are serialized (`&mut self`). The round
        // IS the transaction; no special primitive. Caps at 2: 0 → 1 → 2 → 2 → 2.
        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let cap = r.insert(number("2")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let cmp = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "compare".into() },
                params: vec![Param::frozen("op", "<")],
                inputs: vec![nport("a"), nport("b")],
                outputs: vec![Port { name: "out".into(), ty: Type::Boolean }],
            })
            .unwrap();
        let sel = r
            .insert(prim(
                "select",
                vec![Port { name: "cond".into(), ty: Type::Boolean }, nport("then"), nport("else")],
                Type::Number,
            ))
            .unwrap();
        let wr = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let saturating = Unit {
            body: UnitBody::Composite {
                children: vec![rd, one, cap, add, cmp, sel, wr],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 3, to_port: 0 }, // N   → add.a
                    Edge { from_child: 1, from_port: 0, to_child: 3, to_port: 1 }, // 1   → add.b
                    Edge { from_child: 0, from_port: 0, to_child: 4, to_port: 0 }, // N   → cmp.a
                    Edge { from_child: 2, from_port: 0, to_child: 4, to_port: 1 }, // cap → cmp.b
                    Edge { from_child: 4, from_port: 0, to_child: 5, to_port: 0 }, // N<cap → sel.cond
                    Edge { from_child: 3, from_port: 0, to_child: 5, to_port: 1 }, // N+1 → sel.then
                    Edge { from_child: 0, from_port: 0, to_child: 5, to_port: 2 }, // N   → sel.else
                    Edge { from_child: 5, from_port: 0, to_child: 6, to_port: 0 }, // chosen → write.v
                    Edge { from_child: 6, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };

        let mut host = StateHost::new(num(0.0));
        let mut seen = vec![host.storage().clone()];
        for _ in 0..4 {
            host.round(&saturating, &[], &r);
            seen.push(host.storage().clone());
        }
        assert_eq!(seen, vec![num(0.0), num(1.0), num(2.0), num(2.0), num(2.0)]);
    }

    #[test]
    fn concurrent_requests_keep_their_tokens_no_crossing() {
        // P2.5.1: two requests, each `[token, data]`, processed by `map(egress, ·)`. Each reply
        // carries *its own* token because each flowed down its own pure path — non-crossing is
        // purity's theorem, not an engine feature. The token is just the boundary label the host
        // would route the reply on (real routing → platform host). Token 1 stays with "alpha",
        // token 2 with "beta"; neither crosses.
        let mut r = MapResolver::new();
        let egress = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "egress".into() },
                params: vec![],
                inputs: vec![Port { name: "req".into(), ty: Type::list_any() }],
                outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
            })
            .unwrap();
        let mapper = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&egress))],
            inputs: vec![Port { name: "requests".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let batch = Value::List(vec![
            Value::List(vec![num(1.0), text("alpha")]), // [token=1, data="alpha"]
            Value::List(vec![num(2.0), text("beta")]),  // [token=2, data="beta"]
        ]);
        let mut host = Recorder::default();
        fire(&mapper, &[Some(batch)], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::NetworkEgress, Value::List(vec![num(1.0), text("alpha")])),
                (Capability::NetworkEgress, Value::List(vec![num(2.0), text("beta")])),
            ]
        );
    }

    /// Models the external system's idempotency: it remembers the keys it has performed and
    /// ignores a repeat. Dedup is on the key (`payload[0]` of a `[key, body]` pair), not the body.
    #[derive(Default)]
    struct DedupeHost {
        seen: Vec<Value>,
        performed: Vec<(Capability, Value)>,
    }
    impl Host for DedupeHost {
        fn perform(&mut self, capability: Capability, payload: &Value) {
            let key = match payload {
                Value::List(items) => items.first().cloned(),
                _ => None,
            };
            if let Some(k) = key {
                if self.seen.contains(&k) {
                    return; // already performed this key — the boundary ignores the double-fire
                }
                self.seen.push(k);
            }
            self.performed.push((capability, payload.clone()));
        }
    }

    #[test]
    fn an_idempotency_key_dedupes_a_refire() {
        // P2.5.2: the same intent fired twice reconstructs the *same* key — a pure graph is
        // deterministic (P1.4.4), so re-evaluation rebuilds an identical effect, key included.
        // The external system (modeled by DedupeHost) recognizes the repeated key and performs
        // once. Purity is both why the re-fire reproduces the effect AND why dedup works (the key
        // is stable). The engine carries the key; the user derives it from intent (here, the id).
        let mut r = MapResolver::new();
        let welcome = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "const".into() },
                params: vec![Param::frozen("type", "Text"), Param::frozen("value", "welcome")],
                inputs: vec![],
                outputs: vec![Port { name: "out".into(), ty: Type::Text }],
            })
            .unwrap();
        let pair = r
            .insert(prim(
                "list",
                vec![
                    Port { name: "key".into(), ty: Type::Text },
                    Port { name: "body".into(), ty: Type::Text },
                ],
                Type::list_any(),
            ))
            .unwrap();
        let em = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "egress".into() },
                params: vec![],
                inputs: vec![Port { name: "msg".into(), ty: Type::list_any() }],
                outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
            })
            .unwrap();
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![welcome, pair, em],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 1, to_port: 0 }, // id → key (intent-derived)
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 1 },        // "welcome" → body
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },        // [key,body] → egress
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // egress → out
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "id".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };

        let order = Value::Text("order-42".into());
        let mut host = DedupeHost::default();
        fire(&graph, &[Some(order.clone())], &r, &mut host); // first fire — performed
        fire(&graph, &[Some(order)], &r, &mut host); // re-fire, same intent — same key, deduped
        assert_eq!(
            host.performed,
            vec![(Capability::NetworkEgress, Value::List(vec![text("order-42"), text("welcome")]))]
        );
    }

    #[test]
    fn the_same_graph_previews_in_design_and_fires_in_deploy() {
        // P2.7.2: one graph, two modes. Design mocks the driver (whole graph pure) and performs
        // NOTHING — a safe preview: the egress Effect appears in the output but is never sent.
        // Deploy sources the driver for real and performs the send. The core never learns of modes.
        let mut r = MapResolver::new();
        let tk = r.insert(prim("clock", vec![], Type::Number)).unwrap();
        let em = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "egress".into() },
                params: vec![],
                inputs: vec![Port { name: "msg".into(), ty: Type::Number }],
                outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
            })
            .unwrap();
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![tk, em],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // clock → egress
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // egress → out
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };

        // Design: driver mocked to 0, the egress Effect is *previewed*, nothing performed.
        let mut design = Recorder::default();
        let preview = run(Mode::Design, &graph, &[], &r, &mut design);
        assert_eq!(
            preview,
            Some(Value::Effect { sink: "egress".into(), payload: Box::new(num(0.0)) })
        );
        assert!(design.performed.is_empty()); // safe — design sends nothing

        // Deploy: driver sourced (42), the egress is performed for real.
        let mut deploy = Recorder::default();
        run(Mode::Deploy, &graph, &[(Capability::Clock, num(42.0))], &r, &mut deploy);
        assert_eq!(deploy.performed, vec![(Capability::NetworkEgress, num(42.0))]);
    }

    // ── P3.8.1 · content-addressed store ────────────────────────────────────

    fn unique_temp_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("holon-store-{nanos}-{n}"))
    }

    #[test]
    fn store_put_then_get_round_trips() {
        let mut s = MemStore::new();
        let u = number("42");
        let key = s.put(&u).unwrap();
        assert_eq!(key, hash_unit(&u).unwrap()); // filed under its identity
        assert_eq!(s.get(&key).unwrap(), Some(u));
    }

    #[test]
    fn store_get_missing_is_none() {
        let s = MemStore::new();
        assert!(s.get(&[7u8; 32]).unwrap().is_none());
    }

    #[test]
    fn store_is_content_addressed() {
        let mut s = MemStore::new();
        let add = || prim("add", vec![nport("a"), nport("b")], Type::Number);
        let mul = || prim("multiply", vec![nport("a"), nport("b")], Type::Number);
        let k1 = s.put(&add()).unwrap();
        let k2 = s.put(&add()).unwrap();
        let k3 = s.put(&mul()).unwrap();
        assert_eq!(k1, k2); // same unit ⇒ same key
        assert_ne!(k1, k3); // different unit ⇒ different key
    }

    #[test]
    fn store_rejects_a_corrupted_blob() {
        let mut s = MemStore::new();
        let key = s.put(&number("1")).unwrap();
        s.blobs.get_mut(&key).unwrap()[2] ^= 0xff; // tamper with the stored bytes
        match s.get(&key) {
            Err(StoreError::Corrupt { .. }) | Err(StoreError::Decode(_)) => {}
            other => panic!("expected a tampered blob to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn file_store_round_trips_on_disk() {
        let dir = unique_temp_dir();
        let u = number("7");
        let key = {
            let mut s = FileStore::new(&dir).unwrap();
            let key = s.put(&u).unwrap();
            assert!(dir.join(hex(&key)).exists()); // a file named by the hash
            key
        };
        // A fresh handle on the same root fetches it back, identical.
        let s2 = FileStore::new(&dir).unwrap();
        assert_eq!(s2.get(&key).unwrap(), Some(u));
        let _ = std::fs::remove_dir_all(&dir);
    }
}