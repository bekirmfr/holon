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
//! - [`text`]: the `.holon` text format: parsed, compiled, printed.

pub mod builder;
pub mod canvas;
pub mod composites;
pub mod flow;
pub mod group;
pub mod routes;
pub mod templates;
pub mod text;
pub mod wire;

/// A 32-byte hash as 64 lowercase hex chars: how hashes cross to the editor and sit in documents.
pub fn to_hex(h: &holon_core::Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}
