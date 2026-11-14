//! The Holon language (docs/LANGUAGE.md): a flow written in TypeScript's syntax, `NAME.holon.ts`,
//! read into a syntax tree with spans and lowered to a flow document.
//!
//! [`ts`] reads the text with a TypeScript parser, oxc's, into the tree, [`ast`], every node with
//! its span, refusing what a flow can't say with the line and column; [`lower`] turns the tree
//! into a flow document through the flow builder, every node from the live catalog, so a text and
//! the same flow drawn on the canvas are the same unit; [`format`] lays a text out as `holon fmt`
//! writes it, from the text, not the graph; [`print`] writes a flow document as text, laid out as
//! `holon fmt` writes it, the same unit again, for a canvas drawn rather than written; [`layout`]
//! places a compiled text's nodes on the canvas. [`compile`] reads and lowers a text.

pub mod ast;
pub mod format;
pub mod layout;
pub mod lower;
pub mod nesting;
pub mod print;
pub mod ts;

pub use lower::{lower, LowerError, Lowered};
pub use print::print;

/// Read and lower a flow's text.
pub fn compile(src: &str) -> Result<Lowered, LowerError> {
    ts::compile(src)
}
