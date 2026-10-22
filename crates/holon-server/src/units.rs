//! Units, the registry, forks and the shared cache: the content-addressed side of the control plane.

use crate::*;

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

/// Build a `Resolver` over `root`'s full closure by walking `child_refs` (the same closure
/// definition `Registry::publish`'s own dangling-reference check already uses — one definition
/// of "what a unit references," not a second) and fetching each referenced unit from the store.
/// This is the same shape `Registry::fork` already needs internally for the identical reason
/// (`Store` yields owned units; `Resolver` wants borrowed ones) — reused, not reinvented.
///
/// A hash that doesn't resolve in the store is left out of the resolver rather than erroring —
/// `eval`/`verify_entry` already treat an unresolved reference as "nothing evaluable" and fail
/// closed on it, so this doesn't need its own dangling-reference error path; it just doesn't
/// promise more than the store actually has.
pub(crate) fn materialize_closure(store: &SharedStore, root: Hash) -> Result<MapResolver, StoreError> {
    let mut resolver = MapResolver::new();
    let mut seen = BTreeSet::new();
    let mut queue = vec![root];
    while let Some(h) = queue.pop() {
        if !seen.insert(h) {
            continue;
        }
        let Some(unit) = store.get(&h)? else {
            continue;
        };
        queue.extend(child_refs(&unit));
        resolver.insert(unit).map_err(StoreError::Encode)?;
    }
    Ok(resolver)
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
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
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
