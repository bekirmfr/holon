//! holon-author — authoring flows: what a flow, as the editor's canvas or the command line holds
//! it, turns into in core. Its nodes become units (a palette primitive, a template rebuilt from its
//! params, a grouped composite by hash), its connected groups holding a `request` node become the
//! routes of a server, and the whole flow becomes the one unit a deploy deploys.
//!
//! Pure, like core, so it is the one implementation everywhere: the editor runs it compiled to
//! wasm (holon-wasm), the `holon` command line and the server natively.
//!
//! - [`wire`]: the JSON forms of core's types (a unit, a port, a param, a type) that cross to the
//!   editor and are written in its flow documents.
//! - [`canvas`]: a flow's nodes and wires, as a flow document holds them.
//! - [`templates`]: `request`, `respond` and `stripe_verify`, small composites of genesis leaves.
//! - [`routes`]: which groups are routes, and the endpoint and server units they make.
//! - [`flow`]: a node's unit, a server derived from a flow, the unit a flow deploys as.
//! - [`group`]: a selection grouped into one composite, a saved flow placed as one node.
//! - [`composites`]: a composite's inside as a canvas, expanded in place, frozen back.
//! - [`builder`]: a flow document built node by node, as the samples and the text format do.
//! - [`text`]: a flow document's own units and names put back where it is read.
//! - [`lang`]: the language (docs/LANGUAGE.md): a flow in TypeScript's syntax, `NAME.holon.ts`,
//!   read into a syntax tree with spans, lowered to a flow document (`lang::lower`), laid out as
//!   `holon fmt` writes it (`lang::format`), and a canvas written as text (`lang::print`).
//! - [`samples`]: the sample flows, one per capability: `.holon.ts` files in `samples/`, compiled
//!   from their text through the same lowering as any flow.
//! - [`describe`]: what a manifest item lets a flow do, in words.
//! - [`commands`]: `holon new`, `check`, `show` and `fmt`, text in and text out.
//! - [`editing`]: what an editor asks of a text as it is typed (`holon lsp`).

pub mod agent;
pub mod builder;
pub mod canvas;
pub mod commands;
pub mod composites;
pub mod describe;
pub mod editing;
pub mod flow;
pub mod graph;
pub mod group;
pub mod lang;
pub mod modules;
pub mod routes;
pub mod samples;
pub mod templates;
pub mod text;
pub mod types;
pub mod usage;
pub mod wire;
#[cfg(test)]
mod golden;

/// The limits a deployment may ask for, by name, and the unit each is counted in. Durations are
/// milliseconds (the durable-run wait is in seconds, like the `await` node's `expires`); sizes are
/// bytes. The two module limits are what a flow's module bodies may compute and hold per call
/// (`docs/L4s/wasm-module-L4.md` §4): derived from the units' own budgets, never a default a run
/// gets, so the policy has ceilings for them and no `[limits]` entry. A flow's text sets the others
/// with `limit` lines, and `keep_runs` and `keep_run_days` with its `keep` line (docs/LANGUAGE.md).
pub const LIMIT_NAMES: [&str; 32] = [
    "connect_timeout_ms",
    "request_timeout_ms",
    "claim_wait_ms",
    "commit_wait_ms",
    "run_budget_ms",
    "clock_run_budget_ms",
    "max_requests",
    "per_host_concurrency",
    "max_request_bytes",
    "max_response_bytes",
    "max_header_bytes",
    "max_redirects",
    "max_await_seconds",
    "max_waiting_runs",
    "max_call_depth",
    "inline_file_bytes",
    "max_file_read_bytes",
    "max_file_write_bytes",
    "max_files_per_run",
    "max_db_rows",
    "max_db_read_bytes",
    "db_statement_ms",
    "max_db_statements",
    "max_pack_bytes",
    "max_packs_per_run",
    "channel_keep",
    "channel_buffer",
    "max_subscribers",
    "keep_runs",
    "keep_run_days",
    holon_core::FUEL_LIMIT,
    holon_core::MEMORY_LIMIT,
];

/// A 32-byte hash as 64 lowercase hex chars: how hashes cross to the editor and sit in documents.
pub fn to_hex(h: &holon_core::Hash) -> String {
    holon_core::hex(h)
}
