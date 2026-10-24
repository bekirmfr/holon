//! The Holon language (docs/LANGUAGE.md): a `.holon` text as a syntax tree with spans. Both of
//! the text's forms parse here into one tree: the expression form (bindings, calls, the nouns for
//! the outside world, routes and holons as blocks) and the wiring form (`node`, `a.out -> b.in`),
//! so a file may hold both and a canvas that the expression form can't spell still round-trips.
//!
//! The tree is [`ast`], every node with its span; [`lex`] makes the tokens; [`parse`] is the
//! parser, whose errors say the line and column.
//!
//! Lowering the tree onto the flow builder, and printing a flow document back as text, are the
//! next steps of the build order and are not here yet.

pub mod ast;
pub mod lex;
pub mod parse;

pub use parse::{parse, ParseError};
