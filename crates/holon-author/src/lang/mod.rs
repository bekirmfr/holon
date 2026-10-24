//! The Holon language (docs/LANGUAGE.md): a `.holon` text as a syntax tree with spans. Both of
//! the text's forms parse here into one tree: the expression form (bindings, calls, the nouns for
//! the outside world, routes and holons as blocks) and the wiring form (`node`, `a.out -> b.in`),
//! so a file may hold both and a canvas that the expression form can't spell still round-trips.
//!
//! The tree is [`ast`], every node with its span; [`lex`] makes the tokens; [`parse`] is the
//! parser, whose errors say the line and column; [`lower`] turns the tree into a flow document
//! through the flow builder, every node from the live catalog, so a text and the same flow drawn
//! on the canvas are the same unit. Printing a flow document back as text is the build order's
//! next step and is not here yet.

pub mod ast;
pub mod lex;
pub mod lower;
pub mod parse;

pub use lower::{compile, lower, LowerError, Lowered};
pub use parse::{parse, ParseError};
