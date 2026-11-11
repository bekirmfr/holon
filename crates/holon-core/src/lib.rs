//! holon-core — the pure evaluator substrate.
//!
//! This crate is the canonical, deterministic core that compiles to both native
//! and WASM. It must never gain an IO / async / networking dependency: the
//! purity boundary (invariants I2 / I7) is enforced by this crate's dependency
//! graph, so "the core cannot perform an effect" stays a compile-time fact.
//!
//! Modules (docs/engineering/components/holon-core.md goes deeper):
//!
//! The model:
//! - `hash` — units, their canonical encoding and content hash (I5), transitive pinning (I6).
//! - `value`, `value_hash` — the runtime [`Value`] on wires (P1.4.1), its encoding and hash,
//!   `state_root` and `run_id`.
//! - `resolve` — turn a child hash into its [`Unit`] (shared by validation and evaluation).
//! - `validate` — graph validation: well-formedness, acyclicity, wire-time type
//!   compatibility (P1.3.1).
//! - `module` — what a WebAssembly module must be to be a unit; the host runs it.
//!
//! Evaluation:
//! - `eval` — the graph evaluator, lazy at its choices (P1.4.3, docs/L4s/lazy-choices-L4.md).
//! - `eager` — the evaluator before lazy choices, kept only as the tests' oracle.
//! - `determinism` — float discipline + the determinism harness (P1.4.4).
//! - `simulate` — design mode: a service run with its drivers mocked.
//!
//! The catalog of primitives:
//! - `catalog` — the genesis names and each one's interface; `docs` — each one's words.
//! - `primitives` — native compute for every pure primitive (P1.4.2), by `Op`.
//! - The families' leaves: `numbers`, `texts`, `lists`, `records`, `dates`, `decimals`,
//!   `json`, `csv`, `xml`, `compress`, `crypto`, `jwt`, `pgp`; and what a mail, SQL or
//!   document leaf sends asks for: `mail`, `sql`, `documents`.
//!
//! What a flow may do:
//! - `kind` — effect classification: a leaf's capability manifest (P2.3.1, the primary
//!   classification) and its Pure/Driver/State/Sink kind projection (P2.1.1), derived over a
//!   closure (P2.2.1); the trust basis of the effect layer.
//! - `manifest` — the manifest as consent names it, for the server and the editor alike.
//! - `taint` — which nodes compute with a secret's value, read off the graph before it runs.
//! - `declare`, `triggers` — declaration units (servers, endpoints, clocks, handlers) and the
//!   file triggers that start runs.
//!
//! Building and changing units:
//! - `encapsulate` — derive-then-freeze a selection into a hashed `Unit` (P1.5.1).
//! - `promote` — open params into holes and instantiate templates (P1.5.2).
//! - `fork` — re-pin a changed child up the spine (P3.5.1); `inspect` — a unit's dependencies.
//! - `memo`, `advisory` — the parked marketplace's pure parts: cache keys and advisory facts.

mod determinism;
mod advisory;
mod catalog;
mod dates;
mod decimals;
mod declare;
mod docs;
mod encapsulate;
mod eval;
/// The evaluator before lazy choices: the oracle the tests check `eval` against.
#[cfg(any(test, feature = "eager-oracle"))]
#[doc(hidden)]
pub mod eager;
mod crypto;
mod csv;
mod fork;
mod kind;
mod lists;
mod memo;
mod promote;
mod records;
mod hash;
mod inspect;
mod json;
mod manifest;
mod module;
mod numbers;
mod resolve;
mod simulate;
pub mod taint;
mod texts;
mod triggers;
mod validate;
mod value;
mod value_hash;
mod xml;
mod compress;
mod jwt;
pub mod mail;
mod pgp;

pub mod primitives;
/// What a database leaf's SQL reads and writes (docs/L4s/databases-L4.md §4).
pub mod sql;
/// What a document database leaf's templates say (docs/L4s/databases-L4.md §3, slice 7).
pub mod documents;
pub use primitives::{content_type, MAX_AWAIT_SECONDS, MAX_RANGE};

pub use determinism::is_finite_number;
pub use encapsulate::{default_spec, derive_and_freeze, EncapError, EncapSpec, Expose, Promote};
pub use promote::{instantiate, open_param, open_params, ParamError};
pub use eval::{choice_next, eval, eval_observed, eval_one, would_plan, Answer, Choice, NoObserver, Observer, Resolved, Shape, Shapes, Slot};
pub use fork::{refork, Refork};
pub use advisory::{advise, Advisory};
pub use catalog::{family_values, function_takes, genesis_units, interface_of, open_trigger, sourced_value, takes_trigger, CALLS, FAMILIES, GENESIS, PASSES_EXTRA, SOURCED, TRIGGER, TRIGGERED, VARIADIC};
pub use docs::describe;
pub use manifest::{consent_name, manifest_items, permission_kind, reads_from_outside, trigger, PermissionKind};
pub use inspect::{adoption_counts, child_refs, dependencies};
pub use memo::{entry_key, memo_key, verify_entry, CacheEntry};
pub use value_hash::{decode_value, encode_value, hash_value, run_id, state_root, MAX_VALUE_DEPTH};
pub use hash::{
    canonical_child_order,
    decode_preimage, encode_preimage, hash_unit, hex, parse_hash, DecodeError, Edge, EncodeError, Hash,
    Param, Port, Type, Unit, UnitBody, ENCODING_VERSION, MAX_TYPE_DEPTH,
};
pub use kind::{
    capabilities, capabilities_of, effects_of, kind_of, Capabilities, Capability,
    Effects, Kind,
};
pub use module::{
    module_limits, module_limits_of, validate_module, ModuleError, ModuleLimits, DEFAULT_MODULE_FUEL,
    DEFAULT_MODULE_MEMORY, FUEL_LIMIT, FUEL_PARAM, MEMORY_LIMIT, MEMORY_PARAM,
};
pub use resolve::{MapResolver, Resolver};
pub use simulate::{design_eval, mock_of, Simulated};
pub use taint::{answered_secrets, deployed_capabilities, destination_of, sent_secrets, ANSWER};
pub use triggers::{file_triggers, glob_matches, is_glob, FileTrigger};
pub use validate::{compatible, validate, ValidationError};
pub use value::Value;
pub use json::{parse as parse_json, stringify as stringify_json};
pub use crypto::{bytes_text, text_bytes};
pub use declare::{
    clock_label, declaration_of, log_level, match_route, parse_path, pick_path, same_requests, period_text, read_clock, read_action, read_endpoint,
    read_feed_handler, read_file_handler, read_mail_handler, read_queue_handler, read_server, read_websocket, feed_label, mail_label, queue_label, socket_label, match_websocket, Action, Clock, DeclError, EndpointError, FeedHandler, FileHandler, LogLevel, MailHandler, QueueHandler, Route, Segment, Server, SocketBlock, WebSocket, ALWAYS_TAINTED, MAX_PERIOD, METHODS, SOCKET_BLOCKS,
    is_taint_key,
};

/// Reserved edge endpoint denoting a composite's **boundary** (its own interface)
/// rather than one of its children.
///
/// The composite's interface↔inner binding rides the ordinary edge mechanism — no
/// new fields, no encoder change:
/// - an edge with `from_child == BOUNDARY` carries the composite's input `from_port`
///   *inward* to a child (the boundary acts as a **source**);
/// - an edge with `to_child == BOUNDARY` exposes a child's output as the composite's
///   output `to_port` (the boundary acts as a **sink**).
///
/// Because the boundary is a source for inputs and a sink for outputs, it can never
/// participate in an internal cycle, and a graph traversal that only follows
/// child→child edges (e.g. acyclicity) naturally ignores it. A composite cannot have
/// `u32::MAX` children, so the sentinel never collides with a real index.
pub const BOUNDARY: u32 = u32::MAX;