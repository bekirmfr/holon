//! holon-server — the browser↔host transport (P4.5.1 / P4.6.1's blocker), local-first.
//!
//! Grounded and scoped in `docs/L4s/P4.5-P4.6-transport-scoping-L4.md`. The load-bearing
//! finding from that note, worth repeating here since it's *why* this file is this thin: a
//! network boundary's security is already fully provided by content-addressing
//! (`decode_verified` inside every `Store::get`, `entry_key`/`ingest`, `verify_entry`). This
//! server adds no trust logic of its own — every endpoint is a direct call into `holon-host`
//! code that already exists and is already tested. It is plumbing, not a second decider.
//!
//! **This inch (the first, deliberately small — see the L4's own "pin one boundary at a
//! time"):** the crate scaffold, CORS, and the two simplest endpoints — `GET`/`POST /unit`,
//! wrapping `Store` only. `registry/publish`, `registry/fork`, `cache`, and `fire` are each
//! their own inch on top of this, `fire` most of all since it's the one endpoint that performs
//! a real effect and deserves its own review pass, not a bundle with the read-only ones.
//!
//! Localhost-only by design (the L4's Fork 1 resolution): binds `127.0.0.1`, and CORS is
//! scoped to `localhost`/`127.0.0.1` origins only — never `*`. No auth: there is no "who" to
//! gatekeep on a single local process, and content-addressing already answers "can I trust
//! what I got back," which is the only question a stranger-facing boundary would have needed
//! auth *for*.

use std::io::Read;
use std::sync::Mutex;

use holon_core::{decode_preimage, encode_preimage, parse_hash, Param, Port, Type, Unit, UnitBody};
use holon_host::{FileStore, MemStore, Store, StoreError};
use rouille::{Request, Response};
use serde::Serialize;

/// Where units are persisted. `Mutex` because `Store::put` takes `&mut self` and rouille
/// dispatches each request on its own thread (a small threadpool, not one thread per
/// connection forever) — one lock, held only for the duration of a single put/get, which is
/// exactly the low-contention shape this workload has (a human clicking a button, not a load
/// test).
enum Backend {
    File(Mutex<FileStore>),
    Mem(Mutex<MemStore>),
}

impl Backend {
    fn get(&self, key: &[u8; 32]) -> Result<Option<holon_core::Unit>, StoreError> {
        match self {
            Backend::File(s) => s.lock().unwrap().get(key),
            Backend::Mem(s) => s.lock().unwrap().get(key),
        }
    }
    fn put(&self, unit: &holon_core::Unit) -> Result<[u8; 32], StoreError> {
        match self {
            Backend::File(s) => s.lock().unwrap().put(unit),
            Backend::Mem(s) => s.lock().unwrap().put(unit),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Serialize)]
struct HashBody {
    hash: String,
}

fn to_hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn err(status: u16, message: impl Into<String>) -> Response {
    Response::json(&ErrorBody { error: message.into() }).with_status_code(status)
}

/// Whether `origin` is a localhost origin this server will answer CORS for. Scoped
/// deliberately narrow — see the L4's Fork 1: this server is not meant to be reached from
/// anywhere but the editor's own dev server on the same machine.
fn is_allowed_origin(origin: &str) -> bool {
    origin.starts_with("http://localhost:")
        || origin == "http://localhost"
        || origin.starts_with("http://127.0.0.1:")
        || origin == "http://127.0.0.1"
}

/// Add CORS headers to every response, in one place — so a new endpoint can never forget them
/// (the class of bug the `pub use` re-export list and the `WEncapError` variant drift are both
/// named for elsewhere in this project: a thing that must be repeated per call site eventually
/// isn't repeated somewhere).
fn with_cors(request: &Request, mut response: Response) -> Response {
    if let Some(origin) = request.header("Origin") {
        if is_allowed_origin(origin) {
            response = response
                .with_additional_header("Access-Control-Allow-Origin", origin.to_string())
                .with_additional_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
                .with_additional_header("Access-Control-Allow-Headers", "Content-Type")
                .with_additional_header("Vary", "Origin");
        }
    }
    response
}

fn handle_get_unit(backend: &Backend, hash_hex: &str) -> Response {
    let Some(key) = parse_hash(hash_hex) else {
        return err(400, "not a 64-char hex hash");
    };
    match backend.get(&key) {
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

fn handle_post_unit(backend: &Backend, request: &Request) -> Response {
    let Some(mut body) = request.data() else {
        return err(400, "body already consumed");
    };
    let mut bytes = Vec::new();
    if body.read_to_end(&mut bytes).is_err() {
        return err(400, "failed to read request body");
    }
    let unit = match decode_preimage(&bytes) {
        Ok(u) => u,
        Err(e) => return err(400, format!("not a valid unit preimage: {e:?}")),
    };
    match backend.put(&unit) {
        Ok(hash) => Response::json(&HashBody { hash: to_hex(&hash) }),
        Err(e) => err(500, format!("store error: {e:?}")),
    }
}

fn route(backend: &Backend, request: &Request) -> Response {
    // `request.url()` returns an owned `String` (not a borrow of `request`) — bound once here
    // rather than called again per branch, both so `strip_prefix` below has something with a
    // long enough lifetime to borrow from, and so the routing table has one clear read of "what
    // path is this."
    let url = request.url();
    let method = request.method();

    // Preflight — answered before touching any endpoint logic.
    if method == "OPTIONS" {
        return Response::empty_204();
    }

    if method == "GET" && url == "/health" {
        return Response::text("ok");
    }

    if let Some(hash_hex) = url.strip_prefix("/unit/") {
        if method == "GET" {
            return handle_get_unit(backend, hash_hex);
        }
    }

    if method == "POST" && url == "/unit" {
        return handle_post_unit(backend, request);
    }

    Response::empty_404()
}

/// A trivial, valid unit for round-trip testing only — the same `const` shape already used as
/// a test helper elsewhere in this project (e.g. `holon-host`'s own test module), not a new
/// invention. It doesn't need to be evaluable for this purpose, just a real, canonically
/// encodable `Unit`.
fn test_unit() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", "Number"), Param::frozen("value", "42")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

fn main() {
    let addr = std::env::var("HOLON_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:7420".to_string());

    // `--store <dir>` for an on-disk FileStore; `--store :memory:` for an in-process MemStore
    // (nothing persisted across restarts — useful for a quick local session or a test run).
    // Defaults to a `.holon-store` directory next to wherever the binary is run from.
    //
    // `--emit-test-unit <path>` is a dev-only escape hatch, not a server flag: it writes a
    // trivial unit's canonical preimage bytes to `path` and exits *without* starting the
    // server, so the `/unit` round trip can be checked with plain `curl` + a file diff —
    // no HTTP client dependency needed just to generate a valid test payload.
    let mut args = std::env::args().skip(1);
    let mut store_arg = ".holon-store".to_string();
    while let Some(a) = args.next() {
        if a == "--store" {
            if let Some(v) = args.next() {
                store_arg = v;
            }
        } else if a == "--emit-test-unit" {
            let Some(path) = args.next() else {
                eprintln!("--emit-test-unit needs a path");
                std::process::exit(1);
            };
            let bytes = encode_preimage(&test_unit()).expect("test_unit() is a fixed, valid unit");
            std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write {path}: {e}"));
            println!("wrote {} bytes to {path}", bytes.len());
            return;
        }
    }

    let backend = if store_arg == ":memory:" {
        println!("holon-server: in-memory store (nothing persists past this run)");
        Backend::Mem(Mutex::new(MemStore::new()))
    } else {
        let store = FileStore::new(&store_arg)
            .unwrap_or_else(|e| panic!("could not open store at {store_arg:?}: {e}"));
        println!("holon-server: file store at {store_arg:?}");
        Backend::File(Mutex::new(store))
    };

    println!("holon-server: listening on http://{addr} (localhost-only, no auth — see the L4)");
    rouille::start_server(addr, move |request| with_cors(request, route(&backend, request)));
}