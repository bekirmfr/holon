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
//! [`StateHost`] adds the fenced state cell (`P2.6.1`, keyed — see the
//! keyed-state-capability L4): its storage is the `Model`, now a `BTreeMap<String, Value>`
//! rather than one anonymous slot; a `store_read` sources one key, a `store_write` (a
//! Fork-B sink, capability `StateWrite`) commits to one key. The feedback that looked
//! like a graph cycle is broken by storage *between rounds* — no `validate` change, no
//! `eval` change.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod collection;
pub mod packs;
mod rounds;
mod state;
pub use rounds::{run_rounds, Claim, Exchange, Limits, NoWaits, Performer, Recorded, Round, RunError, RunOutcome, Waiting, Waits};
pub use state::{LoadError, State};

use holon_core::{
    capabilities, capabilities_of, child_refs, decode_preimage, decode_value, design_eval, encode_preimage, encode_value,
    eval, eval_observed, hash_value,
    entry_key, hash_unit, memo_key, refork, verify_entry, adoption_counts, CacheEntry,
    MapResolver, NoObserver, Observer, Refork, Capability, DecodeError, EncodeError, Hash, Param, Resolver,
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
/// re-implemented (`I5`). Under representation **B** an output is a self-describing
/// `Effect { sink, payload }`, so performance is driven by the **output** (what actually
/// reached the boundary), not the manifest — the manifest stays the trust disclosure.
///
/// Calls core's `eval` directly now, returning its own `Vec<Option<Value>>` unchanged — no
/// longer narrowed through `eval_one`. That narrowing was named, when it was written, as
/// deliberately deferred rather than permanent: "not the multi-output data-composite case
/// P4.7 exists for" — a scoping choice made before a real need for it existed, not a ruling
/// against it. The need since showed up concretely: a deployed flow that produces a genuine
/// data response *and* performs a genuine side effect are two separate things, and forcing
/// them into one artificial `list(...)` just to satisfy a single-output requirement doesn't
/// just add an unrelated wiring step — the settled value it produces redundantly echoes the
/// effect inside what's supposed to be the plain answer, since `perform_effects` reads a
/// value without removing anything from it. `perform_effects` is called once per output slot
/// (skipping any that settled to `None`), so a genuinely multi-output root now performs
/// every effect any of its outputs carries — not fewer, not more, and never a silent pick of
/// just one, which `eval_one` refused for the identical reason.
pub fn fire(root: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver, host: &mut impl Host) -> Vec<Option<Value>> {
    fire_observed(root, inputs, resolver, host, &mut NoObserver)
}

/// [`fire`], reporting each composite child's evaluation to `observer` (core's `eval_observed`):
/// how the server times every step of a run for the flow log.
pub fn fire_observed(
    root: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    host: &mut impl Host,
    observer: &mut impl Observer,
) -> Vec<Option<Value>> {
    let outputs = eval_observed(root, inputs, resolver, observer).into_vec();
    for output in outputs.iter().flatten() {
        perform_effects(output, host);
    }
    outputs
}

/// Parse a settled `Effect`'s `sink` string back into `(primitive name, synthetic params)` —
/// the exact inverse of how `primitives.rs`'s `store_write` arm encodes its own `"key"` param
/// into the sink string (`"store_write:var1"`), since the leaf's real params don't survive
/// evaluation into the output value — this string is the only thing that does. A bare name
/// with no `:` (every non-keyed sink — `egress`, and `store_write` itself before this feature
/// existed) parses as itself with no params, unchanged from before. Reused by
/// `capabilities_of` below rather than adding a second function that answers a similar
/// question a different way.
fn parse_sink(sink: &str, payload: &Value) -> (String, Vec<Param>) {
    match sink.split_once(':') {
        // A collection's writes name the collection, a folder's the folder; a state write, its key.
        Some((name, collection)) if name.starts_with("collection_") => {
            (name.to_string(), vec![Param::frozen("collection", collection)])
        }
        Some((name, folder)) if name.starts_with("file_") => (name.to_string(), vec![Param::frozen("folder", folder)]),
        // A document database's write names the database; its collection and templates are the
        // payload's, as the leaf built it (`{update: <collection>, filter, change, …}`).
        Some((name, database)) if matches!(name, "db_insert" | "db_update" | "db_delete") => {
            let Value::Record(write) = payload else { return (name.to_string(), Vec::new()) };
            let text = |k: &str| match write.get(k) {
                Some(Value::Text(t)) => t.as_str(),
                _ => "",
            };
            let mut params = vec![Param::frozen("database", database), Param::frozen("collection", text(name.trim_start_matches("db_")))];
            for (param, field) in [("document", "document"), ("filter", "filter"), ("update", "change")] {
                if write.contains_key(field) {
                    params.push(Param::frozen(param, text(field)));
                }
            }
            if let Some(Value::Number(n)) = write.get("expect") {
                params.push(Param::frozen("expect", n.to_string()));
            }
            (name.to_string(), params)
        }
        // A database's write names the database; the tables are its statement's, which the
        // payload carries as the leaf built it.
        Some((name, database)) if name.starts_with("db_") => {
            let sql = match payload {
                Value::Record(exec) => match exec.get("sql") {
                    Some(Value::Text(sql)) => sql.as_str(),
                    _ => "",
                },
                _ => "",
            };
            (name.to_string(), vec![Param::frozen("database", database), Param::frozen("sql", sql)])
        }
        Some((name, channel)) if name.starts_with("channel_") => (name.to_string(), vec![Param::frozen("channel", channel)]),
        Some((name, key)) => (name.to_string(), vec![Param::frozen("key", key)]),
        None => (sink.to_string(), Vec::new()),
    }
}

/// Perform the effect(s) a settled output carries. A single `Effect`, or a `List` of them
/// (and nested lists) — the **computed fan-out**: `map(egress, xs)` yields a `List[Effect]`,
/// performed in order. Pure data carries nothing. The capability is *derived* from the sink's
/// name (and, for keyed sinks, the key `parse_sink` recovers from that same string), so the
/// `Host` trait is unchanged and a graph can only perform a capability it structurally
/// declares — the effect was constructed by a leaf that has it. A server also walks a waiting
/// run's settled outputs with it, to find the packs that didn't wait (its progress).
pub fn perform_effects(value: &Value, host: &mut impl Host) {
    match value {
        Value::Effect { sink, payload } => {
            let (name, params) = parse_sink(sink, payload);
            if let Some(caps) = capabilities_of(&name, &params) {
                for cap in caps.iter() {
                    if !cap.is_sourced() {
                        host.perform(cap, payload);
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
/// `value` at every leaf carrying that sourced capability — `Clock`, ingress, `StateRead(key)`,
/// … This is the one entry every mode funnels through; a leaf with no pure `compute` (a
/// driver, a state read) gets its value here, or — if unsupplied — evaluates to nothing
/// (deploy's honest asymmetry with design mode, which substitutes a default).
pub fn fire_sourced(
    root: &Unit,
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) -> Vec<Option<Value>> {
    fire_sourced_with(root, &[], sourced, real, host)
}

/// [`fire_sourced`] for a root that also takes inputs: an endpoint, whose one input is the
/// request record.
pub fn fire_sourced_with(
    root: &Unit,
    inputs: &[Option<Value>],
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) -> Vec<Option<Value>> {
    fire_sourced_observed(root, inputs, sourced, real, host, &mut NoObserver)
}

/// [`fire_sourced_with`], reporting each composite child's evaluation to `observer`.
pub fn fire_sourced_observed(
    root: &Unit,
    inputs: &[Option<Value>],
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
    observer: &mut impl Observer,
) -> Vec<Option<Value>> {
    let resolver = SourcedResolver::new(sourced, real);
    fire_observed(root, inputs, &resolver, host, observer)
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
/// settled output(s) either way. Design substitutes mocks at resolve time (`design_eval`); deploy
/// supplies real sourced values (`fire_sourced`) — the same `eval` underneath both, and now the
/// same multi-output shape underneath both too.
pub fn run(
    mode: Mode,
    graph: &Unit,
    drivers: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) -> Vec<Option<Value>> {
    match mode {
        Mode::Design => design_eval(graph, &[], real), // host untouched — nothing performed
        Mode::Deploy => fire_sourced(graph, drivers, real, host),
    }
}

// ── State: keyed cells threaded across rounds (P2.6.1, keyed) ──────────────────────

/// A host that threads keyed state across firing rounds. Storage is the `Model`: a
/// `store_read(key)` sources that key's round-start snapshot, a `store_write(key)` commits
/// a new value at that key, applied at the end of the round — Elm's `update(old) → (new,
/// cmds)`, atomically, now per-key rather than for one anonymous cell.
///
/// Every round snapshots **every** key currently in storage into the sourced set, whether or
/// not the fired graph actually reads all of them — harmless (`SourcedResolver` only hands a
/// leaf the value of its own key), and far simpler than pre-deriving which keys a
/// given graph touches before firing it.
pub struct StateHost {
    storage: BTreeMap<String, Value>,
    pending: Vec<(Capability, Value)>,
}

impl StateHost {
    pub fn new(initial: BTreeMap<String, Value>) -> Self {
        StateHost { storage: initial, pending: Vec::new() }
    }

    pub fn storage(&self) -> &BTreeMap<String, Value> {
        &self.storage
    }

    /// One firing round: source every keyed `StateRead` from storage (internal, threaded) and
    /// the given `drivers` from the world (external, this event's inputs), fire, then apply
    /// every `StateWrite(key)` in what was performed back to that same key in storage. Reads
    /// see the round-start values; writes land at round-end and loop back. With `drivers`
    /// empty this is the pure state loop.
    pub fn round(&mut self, graph: &Unit, drivers: &[(Capability, Value)], real: &impl Resolver) {
        // Sourced inputs: every stored key (threaded), the world for the drivers. The resolver
        // injects both uniformly, matching each leaf's own key exactly; only `StateWrite` is
        // committed back below — drivers have no write-back.
        let mut sourced: Vec<(Capability, Value)> = self
            .storage
            .iter()
            .map(|(key, value)| (Capability::StateRead(key.clone()), value.clone()))
            .collect();
        sourced.extend_from_slice(drivers);
        self.pending.clear();
        fire_sourced(graph, &sourced, real, self);
        for (cap, payload) in std::mem::take(&mut self.pending) {
            if let Capability::StateWrite(key) = cap
                && is_storable(&payload)
            {
                self.storage.insert(key, payload); // the Model-out (loops back to next round's read)
            }
        }
    }
}

/// Whether `value` may be committed to a state cell. A cell holds data, never an effect
/// *request*: `store_write` is `Any`-typed, so `store_write(egress(x))` reaches the host with an
/// `Effect` as its payload, and committing it would put a request into storage that no later
/// round can source back. Recursive — an effect nested in a list or sum is no more data. The one
/// rule both write paths (`StateHost::round` here, holon-server's `fire_with_state`) apply.
pub fn is_storable(value: &Value) -> bool {
    match value {
        Value::Effect { .. } => false,
        Value::List(xs) => xs.iter().all(is_storable),
        Value::Sum { payload, .. } => is_storable(payload),
        Value::Record(fields) => fields.values().all(is_storable),
        Value::Number(_) | Value::Boolean(_) | Value::Text(_) => true,
    }
}

impl Host for StateHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.pending.push((capability, payload.clone()));
    }
}

/// A resolver that supplies a host's value at every **sourced** leaf (`holon_core::SOURCED`:
/// `store_read`, `clock`, `secret`, …), which the leaf produces as it is (`Resolver::sourced`);
/// every unit it hands out is the real resolver's, so a composite keeps its children whatever the
/// run reads. Matching is by full `Capability` equality, which — now that `StateRead`/`StateWrite`
/// carry a key — already does correct per-key routing with no new logic: two leaves reading
/// different keys derive different `Capability` values (via `capabilities_of` reading each leaf's
/// own `params`), so a leaf only ever matches a sourced entry for its *own* key. Performed leaves
/// (sinks, `store_write`) are never sourced, so the effect manifest is intact.
struct SourcedResolver<'a, R: Resolver> {
    /// Each sourced value, and whether it is data ([`is_storable`]), found the first time a leaf
    /// reads it.
    sourced: Vec<(&'a Capability, &'a Value, OnceCell<bool>)>,
    real: &'a R,
}

impl<'a, R: Resolver> SourcedResolver<'a, R> {
    fn new(sourced: &'a [(Capability, Value)], real: &'a R) -> Self {
        SourcedResolver { sourced: sourced.iter().map(|(cap, value)| (cap, value, OnceCell::new())).collect(), real }
    }
}

impl<R: Resolver> Resolver for SourcedResolver<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.real.resolve(hash)
    }

    fn shape(&self, unit: &holon_core::Unit) -> Option<&holon_core::Shape> {
        self.real.shape(unit)
    }

    fn resolved(&self, unit: &holon_core::Unit) -> Option<&holon_core::Resolved> {
        self.real.resolved(unit)
    }

    /// The value for `unit`'s capability. One that isn't data (an `Effect`: writes refuse them,
    /// see `is_storable`, but storage loaded from disk predates that rule) isn't handed out, which
    /// is the seam `fire_sourced`'s doc already names for an unsupplied capability: "evaluates to
    /// nothing", never a request a read could carry to the output and have performed.
    fn sourced(&self, unit: &Unit) -> Option<&Value> {
        let UnitBody::Primitive { name } = &unit.body else { return None };
        let caps: Vec<Capability> = capabilities_of(name, &unit.params)?.iter().collect();
        self.sourced
            .iter()
            .filter(|(cap, ..)| caps.contains(cap))
            .find(|(_, value, data)| *data.get_or_init(|| is_storable(value)))
            .map(|(_, value, _)| *value)
    }

    fn run_module(&self, bytes: &[u8], limits: &holon_core::ModuleLimits, inputs: &[Option<Value>]) -> Result<Vec<Option<Value>>, String> {
        self.real.run_module(bytes, limits, inputs)
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
///
/// Values are stored the same way, by their content hash (`holon_core::hash_value`), kept apart
/// from units: a deployment's state and its session snapshots hold value hashes, so a value
/// shared by several snapshots is stored once.
pub trait Store {
    /// Persist `unit`; returns the hash it is filed under (its identity).
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError>;
    /// Fetch the unit filed under `key`. `Ok(None)` if absent; `Err(Corrupt)` if
    /// the stored bytes do not hash to `key`.
    fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError>;
    /// Persist `value`; returns its content hash. Storing a value that is already there is a
    /// no-op.
    fn put_value(&mut self, value: &Value) -> Result<Hash, StoreError>;
    /// Fetch the value filed under `key`, verified against it like a unit.
    fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError>;
}

/// A value's canonical bytes and its content hash.
fn encode_value_keyed(value: &Value) -> Result<(Hash, Vec<u8>), StoreError> {
    let bytes = encode_value(value).map_err(StoreError::Encode)?;
    Ok((hash_value(value).map_err(StoreError::Encode)?, bytes))
}

/// Decode a fetched value and verify it against its key: the bytes must hash to it.
fn decode_value_verified(key: &Hash, bytes: &[u8]) -> Result<Value, StoreError> {
    let value = decode_value(bytes).map_err(StoreError::Decode)?;
    let actual = hash_value(&value).map_err(StoreError::Encode)?;
    if &actual != key {
        return Err(StoreError::Corrupt { key: *key, actual });
    }
    Ok(value)
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
    holon_core::hex(key)
}

/// In-memory content-addressed store. For tests and any in-process need; shares
/// the exact encode / verify / decode path with the on-disk store.
#[derive(Default)]
pub struct MemStore {
    blobs: BTreeMap<Hash, Vec<u8>>,
    values: BTreeMap<Hash, Vec<u8>>,
}

impl MemStore {
    pub fn new() -> Self {
        MemStore::default()
    }

    /// How many values it holds: what a test counts to see how much a write stored.
    pub fn value_count(&self) -> usize {
        self.values.len()
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
    fn put_value(&mut self, value: &Value) -> Result<Hash, StoreError> {
        let (key, bytes) = encode_value_keyed(value)?;
        self.values.entry(key).or_insert(bytes);
        Ok(key)
    }
    fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError> {
        match self.values.get(key) {
            Some(bytes) => decode_value_verified(key, bytes).map(Some),
            None => Ok(None),
        }
    }
}

/// On-disk content-addressed store: one file per unit, named by its hex hash,
/// under a single root directory. Where units actually live on a machine.
pub struct FileStore {
    root: PathBuf,
    /// Its long-lived values, packed (packs.rs): read when a value's own file isn't there.
    packs: std::sync::Arc<packs::Packs>,
}

impl FileStore {
    /// Open (creating if needed) a store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        create_private_dir(&root)?;
        let packs = packs::Packs::shared(&root.join("values"))?;
        Ok(FileStore { root, packs })
    }

    /// Its packs, shared with whoever packs its values (the server's sweep).
    pub fn packs(&self) -> std::sync::Arc<packs::Packs> {
        std::sync::Arc::clone(&self.packs)
    }
    fn path(&self, key: &Hash) -> PathBuf {
        self.root.join(hex(key))
    }
    /// Where its values live: `values/` under the root, apart from units, one file each named by
    /// its hex hash (the server sweeps the ones nothing names).
    pub fn values_dir(&self) -> PathBuf {
        self.root.join("values")
    }
    fn value_path(&self, key: &Hash) -> PathBuf {
        self.values_dir().join(hex(key))
    }
}

/// Replace `path` with `bytes` so a crash leaves either the old file or the new one, never a
/// truncated mix: write a sibling temp file, flush it to disk, then rename it over `path` (an
/// atomic replace on Unix and Windows alike). The temp name is unique per process and call, so
/// two threads writing the same path never share one.
/// Make `path` and any folder above it that isn't there, each it makes open to its owner only
/// (`0o700` where the system has modes); folders there already are left as they are.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        // Readable by its owner only, whatever the umask: what the server writes this way (the
        // store's units and values, file texts, flows, backups, its policy) is its own.
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

impl Store for FileStore {
    fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError> {
        let (key, bytes) = encode_keyed(unit)?;
        write_atomically(&self.path(&key), &bytes).map_err(StoreError::Io)?;
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
    fn put_value(&mut self, value: &Value) -> Result<Hash, StoreError> {
        let (key, bytes) = encode_value_keyed(value)?;
        let path = self.value_path(&key);
        // Content-addressed: a file already there, or the value packed, holds these very bytes (a
        // read verifies it).
        if !path.exists() && !self.packs.contains(&key) {
            create_private_dir(&self.values_dir()).map_err(StoreError::Io)?;
            write_atomically(&path, &bytes).map_err(StoreError::Io)?;
        }
        Ok(key)
    }
    fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError> {
        let bytes = match fs::read(self.value_path(key)) {
            Ok(b) => b,
            // Not a file of its own: packed, or not there.
            Err(e) if e.kind() == io::ErrorKind::NotFound => match self.packs.get(key).map_err(StoreError::Io)? {
                Some(b) => b,
                None => return Ok(None),
            },
            Err(e) => return Err(StoreError::Io(e)),
        };
        decode_value_verified(key, &bytes).map(Some)
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
            return eval(unit, inputs, resolver).into_vec(); // impure ⇒ never cached
        }
        // Un-keyable (un-hashable unit, or an input too large to encode) ⇒ just evaluate.
        let key = match hash_unit(unit).ok().and_then(|h| memo_key(&h, inputs).ok()) {
            Some(k) => k,
            None => return eval(unit, inputs, resolver).into_vec(),
        };
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let out = eval(unit, inputs, resolver).into_vec();
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
    use holon_core::{MapResolver, Port, Type};

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
    use holon_core::{MapResolver, Param, Port, Type};

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

/// Sourced values handed to the leaves that read them (`SourcedResolver`): two of them that are
/// lists and records, read in one run, each come back whole; one no leaf reads is never looked
/// at; one that isn't data reads as nothing; and a composite keeps its children whatever it reads.
#[cfg(test)]
mod sourced_tests {
    use super::*;
    use holon_core::{Edge, Port, Type, BOUNDARY};

    fn read(key: &str) -> Unit {
        Unit { params: vec![Param::frozen("key", key)], ..holon_core::interface_of("store_read").unwrap() }
    }

    #[test]
    fn two_sourced_lists_read_in_one_run_each_come_back_whole() {
        let mut real = MapResolver::new();
        let (a, b, c) = (real.insert(read("a")).unwrap(), real.insert(read("b")).unwrap(), real.insert(read("c")).unwrap());
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![a, b, c],
                edges: (0..3).map(|i| Edge { from_child: i, from_port: 0, to_child: BOUNDARY, to_port: i }).collect(),
            },
            params: vec![],
            inputs: vec![],
            outputs: (0..3).map(|i| Port { name: format!("out{i}"), ty: Type::Any }).collect(),
        };
        let text = |s: &str| Value::Text(s.into());
        let list_a = Value::List(vec![text("x"), Value::Record(BTreeMap::from([("k".to_string(), Value::List(vec![Value::Number(1.0), Value::Number(2.0)]))])), Value::List(vec![])]);
        let list_b = Value::List(vec![Value::Record(BTreeMap::from([("n".to_string(), text("y")), ("m".to_string(), Value::Boolean(true))])), text("z")]);
        let effect = Value::Effect { sink: "egress".into(), payload: Box::new(text("no")) };
        let sourced = vec![
            (Capability::StateRead("unread".into()), Value::List(vec![text("never made")])),
            (Capability::StateRead("a".into()), list_a.clone()),
            (Capability::StateRead("b".into()), list_b.clone()),
            (Capability::StateRead("c".into()), effect),
        ];
        let resolver = SourcedResolver::new(&sourced, &real);
        let outputs = holon_core::eval(&root, &[], &resolver).into_vec();
        assert_eq!(outputs, vec![Some(list_a), Some(list_b), None], "each whole; one that isn't data reads as nothing");
        assert!(resolver.sourced[0].2.get().is_none(), "one no leaf reads is never looked at");
    }

    /// A resolver that keeps each composite's children resolved, as the server's does (holon's
    /// `units.rs`, `Loaded`).
    struct Keeping {
        units: BTreeMap<Hash, std::sync::Arc<Unit>>,
        shapes: holon_core::Shapes,
    }

    impl Keeping {
        fn of(units: Vec<Unit>) -> Keeping {
            let units: BTreeMap<Hash, std::sync::Arc<Unit>> = units.into_iter().map(|u| (hash_unit(&u).unwrap(), std::sync::Arc::new(u))).collect();
            let shapes = holon_core::Shapes::kept(units.values(), |h| units.get(h));
            Keeping { units, shapes }
        }
    }

    impl Resolver for Keeping {
        fn resolve(&self, hash: &Hash) -> Option<&Unit> {
            self.units.get(hash).map(|u| u.as_ref())
        }
        fn shape(&self, unit: &Unit) -> Option<&holon_core::Shape> {
            self.shapes.get(unit)
        }
        fn resolved(&self, unit: &Unit) -> Option<&holon_core::Resolved> {
            self.shapes.resolved(unit)
        }
    }

    /// The real resolver keeps a composite's children, one of them a read the run sources: the
    /// wrapper hands them on, and each run's read is that run's value, never one an earlier run
    /// read; with nothing sourced, the read is nothing.
    #[test]
    fn a_sourced_value_reaches_a_composite_whose_children_are_kept() {
        let one = Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", "1")],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let inner = Unit {
            body: UnitBody::Composite { children: vec![hash_unit(&one).unwrap()], edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![hash_unit(&read("a")).unwrap(), hash_unit(&inner).unwrap()],
                edges: (0..2).map(|i| Edge { from_child: i, from_port: 0, to_child: BOUNDARY, to_port: i }).collect(),
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "read".into(), ty: Type::Any }, Port { name: "one".into(), ty: Type::Number }],
        };
        let (root_hash, inner_hash) = (hash_unit(&root).unwrap(), hash_unit(&inner).unwrap());
        let keeping = Keeping::of(vec![read("a"), one, inner, root]);
        let (root, inner) = (keeping.resolve(&root_hash).unwrap(), keeping.resolve(&inner_hash).unwrap());
        assert!(keeping.resolved(root).is_some(), "the real resolver keeps the root's children");
        for held in [Value::Text("held".into()), Value::Record(BTreeMap::from([("n".to_string(), Value::Number(2.0))])), Value::Number(3.0)] {
            let sourced = vec![(Capability::StateRead("a".into()), held.clone())];
            let resolver = SourcedResolver::new(&sourced, &keeping);
            assert!(std::ptr::eq(resolver.resolved(root).unwrap(), keeping.resolved(root).unwrap()), "handed on as kept");
            assert!(resolver.resolved(inner).is_some());
            assert_eq!(eval(root, &[], &resolver).into_vec(), vec![Some(held), Some(Value::Number(1.0))], "this run's value");
        }
        let nothing = SourcedResolver::new(&[], &keeping);
        assert_eq!(eval(root, &[], &nothing).into_vec(), vec![None, Some(Value::Number(1.0))]);
        assert_eq!(eval(root, &[], &nothing).into_vec(), eval(root, &[], &keeping).into_vec());
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use holon_core::{Edge, Port, Type, BOUNDARY};

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
    use holon_core::{Edge, MapResolver, Port, Type, BOUNDARY};

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
    /// A `store_read`/`store_write` primitive carrying a real `"key"` param — both now
    /// require one to classify or (for `store_write`) even construct an effect at all.
    fn keyed_state_prim(name: &str, key: &str, inputs: Vec<Port>, output: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![Param::frozen("key", key)],
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
        // A store_write is now an output-side effect (`!is_sourced`): fire performs it, with
        // its key threaded through the settled sink string (`primitives.rs`'s own encoding).
        let mut r = MapResolver::new();
        let wh = r.insert(keyed_state_prim("store_write", "var1", vec![nport("v")], Type::Effect)).unwrap();
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
        assert_eq!(host.performed, vec![(Capability::StateWrite("var1".to_string()), num(42.0))]);
    }

    /// store_read → +1 → store_write, both at key `"counter"`: the canonical counter, now
    /// keyed. Shared by the state and fork tests.
    fn counter_graph() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let rd = r.insert(keyed_state_prim("store_read", "counter", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let wr = r.insert(keyed_state_prim("store_write", "counter", vec![nport("v")], Type::Effect)).unwrap();
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
        // store_read → +1 → store_write, at key "counter". The host threads that one key's
        // storage through each round: reads see the prior write, and `eval` performed none of it.
        let (r, counter) = counter_graph();
        let mut host = StateHost::new(BTreeMap::from([("counter".to_string(), num(0.0))]));
        assert_eq!(host.storage().get("counter"), Some(&num(0.0)));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage().get("counter"), Some(&num(1.0)));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage().get("counter"), Some(&num(2.0)));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage().get("counter"), Some(&num(3.0)));
    }

    /// Regression: a stored List read back as `[]` (a constant rebuilt from it was an empty `list`)
    /// and a stored Option read as absent. Every storable shape reads back exactly as stored, a
    /// sum that isn't an Option too, now that a read is handed the value itself; one holding an
    /// effect reads as absent.
    #[test]
    fn stored_values_read_back_exactly() {
        let mut r = MapResolver::new();
        let rd = r.insert(keyed_state_prim("store_read", "var1", vec![], Type::Any)).unwrap();
        let passthrough = Unit {
            body: UnitBody::Composite {
                children: vec![rd],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let read_back = |held: Value| {
            let sourced = [(Capability::StateRead("var1".to_string()), held)];
            fire_sourced(&passthrough, &sourced, &r, &mut Recorder::default())
        };
        let some = |v: Value| Value::Sum { arm: 0, payload: Box::new(v) };
        let none = Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) };
        let list = Value::List;

        for held in [
            num(5.0),
            text("hi"),
            Value::Boolean(true),
            list(vec![]),
            list(vec![num(1.0), text("two"), Value::Boolean(false)]),
            list(vec![num(0.0); 12]), // 12 equal elements: fine, the pieces are never hashed
            list(vec![list(vec![num(1.0)]), list(vec![]), some(text("x"))]),
            some(num(5.0)),
            some(list(vec![num(1.0), num(2.0)])),
            none.clone(),
            some(none.clone()),
            Value::Record(BTreeMap::new()),
            Value::Record(
                [("id".to_string(), num(1.0)), ("done".to_string(), Value::Boolean(false)), ("tags".to_string(), list(vec![text("a")]))]
                    .into(),
            ),
            list(vec![Value::Record([("n".to_string(), some(num(2.0)))].into())]),
            Value::Sum { arm: 2, payload: Box::new(num(1.0)) },
            Value::Sum { arm: 1, payload: Box::new(num(1.0)) },
        ] {
            assert_eq!(read_back(held.clone()), vec![Some(held.clone())], "{held:?}");
        }

        // Not data: absent, never a request a read could carry to the output.
        assert_eq!(read_back(list(vec![num(1.0), Value::Effect { sink: "egress".into(), payload: Box::new(num(2.0)) }])), vec![None]);
    }

    /// Regression: `store_write(egress(x))` used to commit the `Effect` itself to the cell, and
    /// the next round, rebuilding the value as a constant, hit `unreachable!` — a panic while the
    /// caller held the state lock. Writes now refuse effects, and an effect already in storage
    /// reads as absent.
    #[test]
    fn effect_is_never_stored_and_never_panics() {
        let mut r = MapResolver::new();
        let msg = r.insert(number("1")).unwrap();
        let eg = r.insert(prim("egress", vec![Port { name: "msg".into(), ty: Type::Any }], Type::Effect)).unwrap();
        let wr = r
            .insert(keyed_state_prim("store_write", "cell", vec![Port { name: "value".into(), ty: Type::Any }], Type::Effect))
            .unwrap();
        let rd = r.insert(keyed_state_prim("store_read", "cell", vec![], Type::Any)).unwrap();
        // out0 = store_write(cell, egress(1)); out1 = store_read(cell)
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![msg, eg, wr, rd],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 1 },
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![
                Port { name: "write".into(), ty: Type::Effect },
                Port { name: "read".into(), ty: Type::Any },
            ],
        };

        let mut host = StateHost::new(BTreeMap::new());
        host.round(&graph, &[], &r);
        assert_eq!(host.storage().get("cell"), None, "an effect request is not data");
        host.round(&graph, &[], &r); // used to panic here

        // Storage that already holds an effect (persisted before the write gate existed).
        let held = Value::Effect { sink: "egress".into(), payload: Box::new(num(1.0)) };
        let read = fire_sourced(&graph, &[(Capability::StateRead("cell".into()), held.clone())], &r, &mut Recorder::default());
        assert_eq!(read[1], None);
        let mut host = StateHost::new(BTreeMap::from([("cell".to_string(), held)]));
        host.round(&graph, &[], &r);

        assert!(!is_storable(&Value::List(vec![num(1.0), Value::Effect { sink: "egress".into(), payload: Box::new(num(2.0)) }])));
        assert!(is_storable(&Value::Sum { arm: 0, payload: Box::new(Value::List(vec![num(1.0)])) }));
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
        let sw = r.insert(keyed_state_prim("store_write", "var1", vec![nport("v")], Type::Effect)).unwrap();
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
                (Capability::StateWrite("var1".to_string()), num(7.0)),
                (Capability::NetworkEgress, text("hi")),
            ]
        );
    }

    // ── drive: source the world ────────────────────────────────────────────────
    /// clock (Clock) → store_write at key "counter": read the clock, commit it. A graph whose
    /// driver leaf (`clock`) has no pure compute — it must be sourced.
    fn timer_to_store() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let th = r.insert(prim("clock", vec![], Type::Number)).unwrap();
        let wh = r.insert(keyed_state_prim("store_write", "counter", vec![nport("v")], Type::Effect)).unwrap();
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
        let mut host = StateHost::new(BTreeMap::from([("counter".to_string(), num(0.0))]));
        host.round(&graph, &[(Capability::Clock, num(99.0))], &r);
        assert_eq!(host.storage().get("counter"), Some(&num(99.0)));
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
        // the current state (now the whole keyed map, not one scalar); the two then diverge
        // independently. Copy-on-write falls out of storage living in the host (P2.6.1), not the
        // unit — no new mechanism. Identity is shared (both run the same `&counter`, one graph);
        // state is per-host.
        let (r, counter) = counter_graph();
        let mut original = StateHost::new(BTreeMap::from([("counter".to_string(), num(5.0))]));
        original.round(&counter, &[], &r); // original: 6

        let mut fork = StateHost::new(original.storage().clone()); // CoW: copy the whole map (6)
        original.round(&counter, &[], &r); // original: 7
        original.round(&counter, &[], &r); // original: 8
        fork.round(&counter, &[], &r); // fork: 7 (from its own 6, untouched by original)

        assert_eq!(original.storage().get("counter"), Some(&num(8.0)));
        assert_eq!(fork.storage().get("counter"), Some(&num(7.0))); // diverged — independent state
    }

    #[test]
    fn saturating_counter_is_one_atomic_conditional_update() {
        // P2.6.2 by dissolution. A conditional read-modify-write — write N+1 only while N < cap,
        // else hold — *composes* from store_read → compare → select → store_write, and the round
        // makes it atomic: the value written is computed from the value read with no gap, because
        // a round is snapshot → fire → apply and rounds are serialized (`&mut self`). The round
        // IS the transaction; no special primitive. Caps at 2: 0 → 1 → 2 → 2 → 2.
        let mut r = MapResolver::new();
        let rd = r.insert(keyed_state_prim("store_read", "counter", vec![], Type::Number)).unwrap();
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
        let wr = r.insert(keyed_state_prim("store_write", "counter", vec![nport("v")], Type::Effect)).unwrap();
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

        let mut host = StateHost::new(BTreeMap::from([("counter".to_string(), num(0.0))]));
        let mut seen = vec![host.storage().get("counter").cloned().unwrap()];
        for _ in 0..4 {
            host.round(&saturating, &[], &r);
            seen.push(host.storage().get("counter").cloned().unwrap());
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
            vec![Some(Value::Effect { sink: "egress".into(), payload: Box::new(num(0.0)) })]
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
    fn write_atomically_replaces_and_leaves_no_temp_files() {
        let dir = unique_temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.json");
        write_atomically(&path, b"first").unwrap();
        write_atomically(&path, b"second, longer than the first").unwrap();
        write_atomically(&path, b"third").unwrap(); // shorter: no leftover tail from a truncate
        assert_eq!(fs::read(&path).unwrap(), b"third");
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("doc.json")]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// What the store writes, and the folders it makes, are its owner's only, whatever the umask:
    /// they hold every state and value a flow keeps.
    #[cfg(unix)]
    #[test]
    fn the_store_writes_its_files_and_folders_for_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = unique_temp_dir().join("nested").join("store");
        let mut store = FileStore::new(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700, "the store's folder");
        assert_eq!(mode(dir.parent().unwrap()), 0o700, "and one it made above it");
        let unit = store.put(&number("7")).unwrap();
        let value = store.put_value(&Value::Text("a secret-ish state".into())).unwrap();
        assert_eq!(mode(&store.path(&unit)), 0o600, "a unit");
        assert_eq!(mode(&store.values_dir()), 0o700, "the values' folder");
        assert_eq!(mode(&store.value_path(&value)), 0o600, "a value");
        // A file there already, readable by others, is written again for its owner only.
        let other = dir.join("doc.json");
        fs::write(&other, b"old").unwrap();
        fs::set_permissions(&other, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomically(&other, b"new").unwrap();
        assert_eq!(mode(&other), 0o600);
        fs::remove_dir_all(dir.parent().unwrap().parent().unwrap()).unwrap();
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