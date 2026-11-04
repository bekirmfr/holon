//! Units, the registry, forks and the shared cache: the content-addressed side of the control plane.

use crate::*;
use std::collections::HashMap;
use holon_core::{Shape, Shapes};

pub(crate) fn handle_get_unit(store: &SharedStore, hash_hex: &str) -> Response {
    let Some(key) = parse_hash(hash_hex) else {
        return err(400, "not a 64-char hex hash");
    };
    match store.get(&key) {
        // Re-encode through the ONE core encoder rather than caching raw bytes separately —
        // decode∘encode is proven byte-identical (hash.rs's own round-trip tests), so this is
        // not a second encoding path, just the one encoder used twice.
        Ok(Some(unit)) => match encode_preimage(&unit) {
            Ok(bytes) => Response::from_data("application/octet-stream", bytes),
            Err(e) => err(500, format!("re-encode failed: {e:?}")),
        },
        Ok(None) => Response::empty_404(),
        Err(StoreError::Corrupt { key, actual }) => err(
            500,
            format!(
                "stored blob failed verify-on-read: filed under {}, hashes to {}",
                to_hex(&key),
                to_hex(&actual)
            ),
        ),
        Err(e) => err(500, format!("store error: {e:?}")),
    }
}

pub(crate) fn handle_post_unit(store: &SharedStore, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let unit = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };
    let mut store = store.clone();
    match store.put(&unit) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        Err(e) => err(500, format!("store error: {e:?}")),
    }
}

/// `POST /registry/publish` — body is the root unit's own preimage bytes (same convention as
/// `/unit`, deliberately: one "how a unit crosses the wire" shape everywhere, not two).
/// `Registry::publish` itself does the real work: verify the whole closure (every child, every
/// `fn`-param reference) is already present in the store, fail closed on anything dangling,
/// else commit the root and record it published. This handler adds nothing beyond decoding the
/// body and translating `PublishError` to an HTTP status.
pub(crate) fn handle_post_publish(registry: &Mutex<Registry<SharedStore>>, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let unit = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };
    let mut reg = registry.lock().unwrap();
    match reg.publish(&unit) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        // Fails closed exactly as Registry::publish already does — this handler doesn't loosen
        // or re-check anything, just names the missing hash back to the caller so the editor
        // can say "stage this child first" instead of a bare 400.
        Err(PublishError::DanglingReference(h)) => err(
            400,
            format!(
                "publish refused: {} is referenced but not yet in the store — POST it via /unit first",
                to_hex(&h)
            ),
        ),
        Err(PublishError::Store(e)) => err(500, format!("store error: {e:?}")),
    }
}

/// `GET /registry` — the published set, in hash order (`Registry::published()`'s own order,
/// unchanged). See the module doc: this list is in-memory only and resets on restart, by
/// `Registry`'s own existing, documented scope — not something this endpoint adds or hides.
pub(crate) fn handle_get_registry(registry: &Mutex<Registry<SharedStore>>) -> Response {
    let reg = registry.lock().unwrap();
    let published: Vec<String> = reg.published().map(to_hex).collect();
    Response::json(&RegistryBody { published })
}

/// `POST /registry/fork?root=<hex>&target=<hex>` — body is the replacement unit's own preimage
/// bytes (same "unit crosses as its own preimage" convention as `/unit` and
/// `/registry/publish`). `root`/`target` are hashes, so they ride the query string as hex —
/// consistent with how `/unit/<hash>` already puts a hash in the URL rather than the body.
///
/// `Registry::fork` does the real work: re-pin every reference to `target` inside `root`'s
/// closure to the newly-stored replacement, then publish the re-pinned spine (reusing
/// `publish`'s own dangling-reference gate verbatim). This handler only decodes the wire shapes
/// and translates `ForkError` to a status — the same "thin wrapper" discipline as every other
/// endpoint here.
pub(crate) fn handle_post_fork(registry: &Mutex<Registry<SharedStore>>, request: &Request) -> Response {
    let Some(root_hex) = request.get_param("root") else {
        return err(400, "missing ?root=<hex> query param");
    };
    let Some(target_hex) = request.get_param("target") else {
        return err(400, "missing ?target=<hex> query param");
    };
    let Some(root_hash) = parse_hash(&root_hex) else {
        return err(400, "root is not a 64-char hex hash");
    };
    let Some(target_hash) = parse_hash(&target_hex) else {
        return err(400, "target is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let replacement = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };

    let mut reg = registry.lock().unwrap();
    match reg.fork(&root_hash, &target_hash, &replacement) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        Err(ForkError::RootNotFound(h)) => {
            err(404, format!("no unit stored at root {}", to_hex(&h)))
        }
        Err(ForkError::TargetNotInClosure(h)) => err(
            400,
            format!("target {} is not referenced anywhere in root's closure", to_hex(&h)),
        ),
        Err(ForkError::DanglingInClosure(h)) => err(
            400,
            format!("root's closure is incomplete in the store: {} does not resolve", to_hex(&h)),
        ),
        Err(ForkError::Publish(PublishError::DanglingReference(h))) => err(
            400,
            format!("re-pinned root refused: {} is referenced but not in the store", to_hex(&h)),
        ),
        Err(ForkError::Publish(PublishError::Store(e))) => {
            err(500, format!("store error during publish: {e:?}"))
        }
        Err(ForkError::Store(e)) => err(500, format!("store error: {e:?}")),
        Err(ForkError::Encode(e)) => err(500, format!("encode error: {e:?}")),
    }
}

/// A flow's closure, as runs resolve it: `root` and every unit it reaches through `child_refs`
/// (the closure definition `Registry::publish`'s dangling-reference check uses, so there is one
/// definition of "what a unit references", not two). Cheap to clone: each unit is an `Arc`, shared
/// with every other closure that places it, so a js node's engine module is held once however
/// many flows place it.
///
/// A hash that doesn't resolve in the store is left out rather than an error: `eval` and
/// `verify_entry` already treat an unresolved reference as "nothing evaluable" and fail closed on
/// it, so a closure promises no more than the store has. Such a closure isn't kept (`closure`).
///
/// It keeps each composite's shape beside its units (docs/L4s/interpreter-path-L4.md, step 1b),
/// found by the unit itself: the units are `Arc`s the closure holds, so each stays where the
/// shapes found it for as long as the closure lives.
#[derive(Clone, Default)]
pub(crate) struct Loaded(Arc<Held>);

#[derive(Default)]
pub(crate) struct Held {
    units: BTreeMap<Hash, Arc<Unit>>,
    /// Built over `units`, never changed after.
    shapes: Shapes,
}

impl Loaded {
    fn of(units: BTreeMap<Hash, Arc<Unit>>) -> Loaded {
        let shapes = Shapes::of(units.values().map(|u| u.as_ref()));
        Loaded(Arc::new(Held { units, shapes }))
    }
}

impl Resolver for Loaded {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.0.units.get(hash).map(|u| u.as_ref())
    }

    fn shape(&self, unit: &Unit) -> Option<&Shape> {
        self.0.shapes.get(unit)
    }
}

#[cfg(test)]
impl Loaded {
    /// Whether it is the very closure `other` is (the same kept one), not just an equal one.
    pub(crate) fn is(&self, other: &Loaded) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// The unit it holds under `hash`, as shared.
    pub(crate) fn unit(&self, hash: &Hash) -> Option<Arc<Unit>> {
        self.0.units.get(hash).cloned()
    }
}

/// The closure of `root`, from memory when it was loaded whole before (docs/L4s/request-path-L4.md,
/// C1). A hash names one closure forever, so nothing kept ever goes stale: a redeploy is another
/// hash, a rollback or a resumed run of an older version is a hash already kept, and units are
/// never swept (sweep.rs). Only a whole closure is kept (a unit put later could complete a partial
/// one), and never an error.
///
/// A closure past [`MAX_CLOSURE_UNITS`] units, or nested deeper than [`MAX_CLOSURE_DEPTH`], is
/// refused ([`Closure::TooBig`]): evaluating one recurses a level per nested unit, and its units
/// are anyone's who can stage them.
pub(crate) fn closure(store: &SharedStore, root: Hash) -> Result<Loaded, Closure> {
    let cache = &store.3;
    if let Some(kept) = cache.get(&root) {
        return Ok(kept);
    }
    let (loaded, whole) = load_closure(store, root)?;
    if whole {
        cache.keep(root, &loaded);
    }
    Ok(loaded)
}

/// Walk and fetch `root`'s closure, taking each unit the cache already holds from memory and the
/// rest from the store (which checks each against its hash). Says whether every reference
/// resolved. No cache lock is held while the store is read.
fn load_closure(store: &SharedStore, root: Hash) -> Result<(Loaded, bool), Closure> {
    let mut units: BTreeMap<Hash, Arc<Unit>> = BTreeMap::new();
    let mut whole = true;
    // Each unit's references, as it was fetched: the depth below is measured over these.
    let mut refs: BTreeMap<Hash, Vec<Hash>> = BTreeMap::new();
    let mut queue = vec![root];
    while let Some(h) = queue.pop() {
        if refs.contains_key(&h) {
            continue;
        }
        let unit = match store.3.unit(&h) {
            Some(unit) => unit,
            None => match store.get(&h).map_err(Closure::Store)? {
                Some(unit) => Arc::new(unit),
                None => {
                    whole = false;
                    continue;
                }
            },
        };
        if refs.len() >= MAX_CLOSURE_UNITS {
            return Err(Closure::TooBig(format!("its closure has more than {MAX_CLOSURE_UNITS} units")));
        }
        let children = child_refs(&unit);
        queue.extend(children.iter().copied());
        refs.insert(h, children);
        units.insert(h, unit);
    }
    let depth = closure_depth(root, &refs);
    if depth > MAX_CLOSURE_DEPTH {
        return Err(Closure::TooBig(format!("it nests more than {MAX_CLOSURE_DEPTH} units deep")));
    }
    Ok((Loaded::of(units), whole))
}

/// What the cache's own entry for a unit takes beside it: its hash, its count and its `Arc`, in a
/// map that doubles as it grows.
const KEPT_UNIT_BYTES: u64 = 96;

/// About what `unit` takes in memory, behind its `Arc`: the unit itself and what it owns on the
/// heap (a primitive's name, a composite's children and edges, a module's bytes, its params' and
/// ports' texts and types), each allocation a little more for the allocator's own.
fn unit_footprint(unit: &Unit) -> u64 {
    // A unit is mostly short texts (names like `out`, `i0`), each its own allocation, which the
    // allocator rounds up to its smallest chunk.
    const ALLOC: u64 = 32;
    fn type_heap(ty: &holon_core::Type) -> u64 {
        use holon_core::Type;
        let own = std::mem::size_of::<Type>() as u64;
        match ty {
            Type::List(t) => ALLOC + own + type_heap(t),
            Type::Product(ts) | Type::Sum(ts) => ALLOC + ts.iter().map(|t| own + type_heap(t)).sum::<u64>(),
            _ => 0,
        }
    }
    let ports = |ps: &[holon_core::Port]| ALLOC + ps.iter().map(|p| std::mem::size_of::<holon_core::Port>() as u64 + p.name.len() as u64 + ALLOC + type_heap(&p.ty)).sum::<u64>();
    let body = match &unit.body {
        UnitBody::Primitive { name } => name.len() as u64 + ALLOC,
        UnitBody::Composite { children, edges } => (children.len() * 32 + edges.len() * std::mem::size_of::<holon_core::Edge>()) as u64 + 2 * ALLOC,
        UnitBody::Module { bytes } => bytes.len() as u64 + ALLOC,
    };
    let params = ALLOC + unit.params.iter().map(|p| std::mem::size_of::<Param>() as u64 + 2 * ALLOC + p.name().len() as u64 + p.value().map_or(0, str::len) as u64).sum::<u64>();
    std::mem::size_of::<Unit>() as u64 + 16 + ALLOC + body + params + ports(&unit.inputs) + ports(&unit.outputs)
}

/// What the flow cache counts for `unit`, held once however many closures place it.
#[cfg(test)]
pub(crate) fn counted_for_tests(unit: &Unit) -> u64 {
    (unit_footprint(unit) + KEPT_UNIT_BYTES) * 5 / 4
}

/// About what a closure takes beside its units: its tree of them (nodes of 11 hashes and `Arc`s,
/// about two thirds full) and its shapes (`Shapes::footprint`).
fn closure_footprint(loaded: &Loaded) -> u64 {
    let n = loaded.0.units.len() as u64;
    let node = 11 * (32 + 8) + 24 + 16;
    n.div_ceil(8) * node + loaded.0.shapes.footprint() + 64
}

/// How much memory the flow cache holds by default: every sample flow about 40 times over, with
/// units shared (docs/L4s/request-path-L4.md, C1). `holon serve --flow-cache SIZE` sets it.
pub(crate) const FLOW_CACHE_BYTES: u64 = 256 << 20;

/// The closures runs have loaded, kept by their root hash up to a budget in bytes (a unit's
/// encoded size, each unit counted once however many closures hold it), the least recently used
/// let go first. Its lock is a leaf: nothing else is locked under it, and the store is never read
/// while it is held.
pub(crate) struct FlowCache(Mutex<Kept>);

struct Kept {
    budget: u64,
    bytes: u64,
    tick: u64,
    closures: HashMap<Hash, KeptClosure>,
    units: HashMap<Hash, KeptUnit>,
}

struct KeptClosure {
    loaded: Loaded,
    used: u64,
    /// What the closure itself takes beside its units: its own map of them and its shapes.
    bytes: u64,
}

struct KeptUnit {
    unit: Arc<Unit>,
    bytes: u64,
    /// How many kept closures hold it.
    holders: usize,
}

impl Default for FlowCache {
    fn default() -> FlowCache {
        FlowCache::with_budget(FLOW_CACHE_BYTES)
    }
}

impl FlowCache {
    pub(crate) fn with_budget(budget: u64) -> FlowCache {
        FlowCache(Mutex::new(Kept { budget, bytes: 0, tick: 0, closures: HashMap::new(), units: HashMap::new() }))
    }

    /// Keep at most `budget` bytes from now on, letting go of what no longer fits.
    pub(crate) fn set_budget(&self, budget: u64) {
        let mut kept = self.0.lock().unwrap();
        kept.budget = budget;
        kept.shrink(None);
    }

    /// Whether the closure of `root` is kept in memory, without loading it or counting it as used.
    pub(crate) fn holds(&self, root: &Hash) -> bool {
        self.0.lock().unwrap().closures.contains_key(root)
    }

    /// The closure kept for `root`, now the most recently used.
    fn get(&self, root: &Hash) -> Option<Loaded> {
        let mut kept = self.0.lock().unwrap();
        kept.tick += 1;
        let tick = kept.tick;
        kept.closures.get_mut(root).map(|c| {
            c.used = tick;
            c.loaded.clone()
        })
    }

    /// A unit some kept closure holds.
    fn unit(&self, hash: &Hash) -> Option<Arc<Unit>> {
        self.0.lock().unwrap().units.get(hash).map(|u| Arc::clone(&u.unit))
    }

    /// Keep `loaded` as `root`'s closure, when it fits the budget at all.
    fn keep(&self, root: Hash, loaded: &Loaded) {
        // Sized before the lock is taken: what each takes in memory, not its encoding, so the
        // budget is the memory kept (resilience-audit-L4.md, S1: counted by encoding, a cache of
        // flows without a js node held 7.8 times its budget).
        // Rounded up a quarter for what the estimate can't see (each small allocation's rounding,
        // the maps' spare room): a cache that keeps a little less than its budget, never more.
        let sizes: Vec<(Hash, u64)> = loaded.0.units.iter().map(|(h, u)| (*h, (unit_footprint(u) + KEPT_UNIT_BYTES) * 5 / 4)).collect();
        let own = closure_footprint(loaded);
        if sizes.iter().map(|(_, n)| n).sum::<u64>() + own > self.0.lock().unwrap().budget {
            return;
        }
        let mut kept = self.0.lock().unwrap();
        if kept.closures.contains_key(&root) {
            return; // Another run loaded it meanwhile.
        }
        for (hash, bytes) in sizes {
            match kept.units.get_mut(&hash) {
                Some(unit) => unit.holders += 1,
                None => {
                    kept.bytes += bytes;
                    kept.units.insert(hash, KeptUnit { unit: Arc::clone(&loaded.0.units[&hash]), bytes, holders: 1 });
                }
            }
        }
        kept.tick += 1;
        let used = kept.tick;
        kept.bytes += own;
        kept.closures.insert(root, KeptClosure { loaded: loaded.clone(), used, bytes: own });
        kept.shrink(Some(root));
    }

    /// How many closures and units it holds, and their bytes.
    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, usize, u64) {
        let kept = self.0.lock().unwrap();
        (kept.closures.len(), kept.units.len(), kept.bytes)
    }

    /// What it holds against its budget, for the host's view (storage.rs): closures, bytes, budget.
    pub(crate) fn usage(&self) -> (usize, u64, u64) {
        let kept = self.0.lock().unwrap();
        (kept.closures.len(), kept.bytes, kept.budget)
    }
}

impl Kept {
    /// Let go of the least recently used closures, never `spare`, until it fits the budget.
    fn shrink(&mut self, spare: Option<Hash>) {
        while self.bytes > self.budget {
            let Some(oldest) = self.closures.iter().filter(|(h, _)| Some(**h) != spare).min_by_key(|(_, c)| c.used).map(|(h, _)| *h) else {
                break;
            };
            let gone = self.closures.remove(&oldest).expect("it was just found");
            self.bytes -= gone.bytes;
            for hash in gone.loaded.0.units.keys() {
                let unit = self.units.get_mut(hash).expect("a kept closure's units are kept");
                unit.holders -= 1;
                if unit.holders == 0 {
                    self.bytes -= unit.bytes;
                    self.units.remove(hash);
                }
            }
        }
    }
}

/// The most units a closure holds.
pub(crate) const MAX_CLOSURE_UNITS: usize = 20_000;
/// The deepest a closure nests, its root and leaves counted: far past any flow written, and well
/// inside the stack of the threads that run one.
pub(crate) const MAX_CLOSURE_DEPTH: usize = 64;

/// Why a closure isn't materialized.
#[derive(Debug)]
pub(crate) enum Closure {
    Store(StoreError),
    /// Past the bounds above: a request's fault, a 400 where one asked for it.
    TooBig(String),
}

impl Closure {
    /// As an answer: a closure past the bounds is the caller's to fix, a store error the host's.
    pub(crate) fn response(&self) -> Response {
        match self {
            Closure::TooBig(why) => err(400, format!("this unit is too big to run: {why}")),
            Closure::Store(e) => err(500, format!("store error while materializing closure: {e:?}")),
        }
    }
}

/// The longest chain of references from `root` down, in units (`root` alone is 1); a reference
/// not fetched counts as a leaf. Iterative, each unit measured once: content hashes make the
/// graph acyclic, and a diamond of shared children costs nothing twice.
pub(crate) fn closure_depth(root: Hash, refs: &BTreeMap<Hash, Vec<Hash>>) -> usize {
    let children = |h: &Hash| refs.get(h).map_or(&[][..], Vec::as_slice);
    let mut depth: BTreeMap<Hash, usize> = BTreeMap::new();
    // The chain being walked, each unit with the next of its references to look at.
    let mut path: Vec<(Hash, usize)> = vec![(root, 0)];
    while let Some(&(h, next)) = path.last() {
        match children(&h).get(next) {
            Some(&c) => {
                path.last_mut().unwrap().1 += 1;
                if !depth.contains_key(&c) {
                    if path.len() > MAX_CLOSURE_DEPTH {
                        // Past the bound along this chain already: no need to walk the rest.
                        return path.len() + 1;
                    }
                    path.push((c, 0));
                }
            }
            None => {
                let below = children(&h).iter().map(|c| depth[c]).max().unwrap_or(0);
                depth.insert(h, below + 1);
                path.pop();
            }
        }
    }
    depth[&root]
}

/// `GET /cache/<key>` — the trustless read: re-executes the cached computation against a
/// resolver materialized from the store and returns the outputs **only if they check out**
/// (`SharedCache::get_verified`). A syntactically-present entry whose claimed output is wrong
/// is refused (422), not served — the whole point of this endpoint over the cache's raw,
/// unverified `get`, which this server deliberately does not expose at all.
pub(crate) fn handle_get_cache(cache: &Mutex<SharedCache>, store: &SharedStore, key_hex: &str) -> Response {
    let Some(key) = parse_hash(key_hex) else {
        return err(400, "not a 64-char hex hash");
    };
    let unit_hash = {
        let cache = cache.lock().unwrap();
        match cache.get(&key) {
            Some(entry) => entry.unit_hash,
            None => return Response::empty_404(),
        }
    };
    let resolver = match closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return e.response(),
    };
    let cache = cache.lock().unwrap();
    match cache.get_verified(&key, &resolver) {
        Some(outputs) => Response::json(&CacheOutputsBody {
            outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        }),
        // 422, not 404: the entry exists (we already found it above) but failed the semantic
        // check — a different failure than "nothing here."
        None => err(422, "cache entry failed verification — refused, not served"),
    }
}

/// `POST /cache/ingest?key=<hex>` — the trustless *write*: admits a peer-supplied entry only if
/// it actually hashes to the key it's claimed under (`SharedCache::ingest`, P3.8.2's own gap,
/// exposed here). Cheap structural check only — it does **not** re-execute the computation
/// (that's `GET /cache/<key>`'s job on read); a correctly-keyed entry with a wrong output is
/// admitted here and caught there, exactly as `SharedCache`'s own two-tier design intends.
pub(crate) fn handle_post_cache_ingest(cache: &Mutex<SharedCache>, request: &Request) -> Response {
    let Some(key_hex) = request.get_param("key") else {
        return err(400, "missing ?key=<hex> query param — the claimed cache key");
    };
    let Some(claimed_key) = parse_hash(&key_hex) else {
        return err(400, "key is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let sentry: SCacheEntry = match serde_json::from_slice(&bytes) {
        Ok(e) => e,
        Err(e) => return err(400, format!("not a valid cache entry JSON: {e}")),
    };
    let Some(unit_hash) = parse_hash(&sentry.unit_hash) else {
        return err(400, "entry.unit_hash is not a 64-char hex hash");
    };
    let entry = CacheEntry {
        unit_hash,
        inputs: sentry.inputs.into_iter().map(|o| o.map(SValue::into_core)).collect(),
        outputs: sentry.outputs.into_iter().map(|o| o.map(SValue::into_core)).collect(),
    };
    let mut cache = cache.lock().unwrap();
    match cache.ingest(claimed_key, entry) {
        Ok(()) => Response::text("ingested"),
        Err(CacheIngestError::KeyMismatch { claimed, actual }) => err(
            400,
            format!(
                "key mismatch: claimed {}, entry actually hashes to {} — refused",
                to_hex(&claimed),
                to_hex(&actual)
            ),
        ),
        Err(CacheIngestError::Encode(e)) => err(500, format!("encode error: {e:?}")),
    }
}

/// How many bytes of values the store keeps in memory beside the disk (`ValueCache`).
pub(crate) const VALUE_CACHE_BYTES: u64 = 64 << 20;

/// What the cache's own entry for a value takes beside the value: its hash and its counts, in a
/// map that doubles as it grows (a bucket and a half on average, each with its control byte).
const ENTRY_BYTES: u64 = 104;

/// About what `value` takes in memory: its own `Value`, and what it owns on the heap (a text's
/// bytes, a list's items, a record's entries and keys, a payload's box), each allocation a little
/// more for the allocator's own (resilience-audit-L4.md, S1: counted by encoding, the cache held
/// 2.8 times its budget).
pub(crate) fn footprint(value: &Value) -> u64 {
    const ALLOC: u64 = 16;
    let own = std::mem::size_of::<Value>() as u64;
    own + match value {
        Value::Number(_) | Value::Boolean(_) => 0,
        Value::Text(t) => t.len() as u64 + ALLOC,
        Value::List(xs) => ALLOC + xs.iter().map(footprint).sum::<u64>(),
        // A tree's nodes, each with room for 11 keys and values whatever it holds, about two thirds
        // full; each key's text; each value's own heap (the value itself is in its node).
        Value::Record(fields) if fields.is_empty() => 0,
        Value::Record(fields) => {
            let slot = std::mem::size_of::<String>() as u64 + own;
            let nodes = (fields.len() as u64).div_ceil(8);
            nodes * (11 * slot + 24 + ALLOC) + fields.iter().map(|(k, v)| k.len() as u64 + ALLOC + footprint(v) - own).sum::<u64>()
        }
        Value::Effect { sink, payload } => sink.len() as u64 + 2 * ALLOC + footprint(payload),
        Value::Sum { payload, .. } => ALLOC + footprint(payload),
    }
}

/// The largest value it keeps: a file's text kept by hash can be big, and is read once.
const VALUE_CACHE_LARGEST: u64 = 1 << 20;

/// The values runs have read from the on-disk store, kept in memory up to a budget (by encoded
/// size), the least recently used let go first (docs/L4s/request-path-plan.md, 12.4): a collection
/// read walks its trie, a value a node, each a file named by its hash, opened, decoded and checked
/// under the store's lock. A value is its hash's forever, so a kept one is never stale, and one a
/// sweep has since removed from disk is one no live root reaches, so no run asks for it. Its lock
/// is a leaf: the store is never read while it is held.
pub(crate) struct ValueCache(Mutex<KeptValues>);

struct KeptValues {
    budget: u64,
    bytes: u64,
    tick: u64,
    values: HashMap<Hash, (Value, u64, u64)>,
}

impl Default for ValueCache {
    fn default() -> ValueCache {
        ValueCache::with_budget(VALUE_CACHE_BYTES)
    }
}

impl ValueCache {
    pub(crate) fn with_budget(budget: u64) -> ValueCache {
        ValueCache(Mutex::new(KeptValues { budget, bytes: 0, tick: 0, values: HashMap::new() }))
    }

    /// Keep at most `budget` bytes from now on (`holon serve --value-cache`), letting go of the
    /// least recently used past it.
    pub(crate) fn set_budget(&self, budget: u64) {
        let mut kept = self.0.lock().unwrap();
        kept.budget = budget;
        if kept.bytes > budget {
            let mut by_use: Vec<(u64, Hash, u64)> = kept.values.iter().map(|(h, (_, n, used))| (*used, *h, *n)).collect();
            by_use.sort_unstable();
            for (_, h, n) in by_use {
                if kept.bytes <= budget {
                    break;
                }
                kept.values.remove(&h);
                kept.bytes -= n;
            }
        }
    }

    /// The value kept for `key`, now the most recently used.
    pub(crate) fn get(&self, key: &Hash) -> Option<Value> {
        let mut kept = self.0.lock().unwrap();
        if kept.budget == 0 {
            return None;
        }
        kept.tick += 1;
        let tick = kept.tick;
        kept.values.get_mut(key).map(|(v, _, used)| {
            *used = tick;
            v.clone()
        })
    }

    /// Keep `value` as `key`'s, when it is small enough; let go of the least recently used past the
    /// budget, down to three quarters of it, so the next few fit without another pass.
    pub(crate) fn keep(&self, key: Hash, value: &Value) {
        if self.0.lock().unwrap().budget == 0 {
            return;
        }
        // Sized before the lock is taken: what it takes in memory, not its encoding, so the budget is
        // the memory kept (resilience-audit-L4.md, S1).
        let bytes = footprint(value) + ENTRY_BYTES;
        let mut kept = self.0.lock().unwrap();
        if bytes > VALUE_CACHE_LARGEST.min(kept.budget) || kept.values.contains_key(&key) {
            return;
        }
        kept.tick += 1;
        let tick = kept.tick;
        kept.values.insert(key, (value.clone(), bytes, tick));
        kept.bytes += bytes;
        if kept.bytes > kept.budget {
            let mut by_use: Vec<(u64, Hash, u64)> = kept.values.iter().map(|(h, (_, n, used))| (*used, *h, *n)).collect();
            by_use.sort_unstable();
            let target = kept.budget / 4 * 3;
            for (_, h, n) in by_use {
                if kept.bytes <= target {
                    break;
                }
                kept.values.remove(&h);
                kept.bytes -= n;
            }
        }
    }

    /// How many values it keeps, and their bytes.
    #[cfg(test)]
    pub(crate) fn size(&self) -> (usize, u64) {
        let kept = self.0.lock().unwrap();
        (kept.values.len(), kept.bytes)
    }

    /// What it holds against its budget, for the host's view (storage.rs): values, bytes, budget.
    pub(crate) fn usage(&self) -> (usize, u64, u64) {
        let kept = self.0.lock().unwrap();
        (kept.values.len(), kept.bytes, kept.budget)
    }
}
