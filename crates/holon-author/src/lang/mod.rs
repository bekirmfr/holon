//! The Holon language (docs/LANGUAGE.md): a `.holon` text as a syntax tree with spans. Both of
//! the text's forms parse here into one tree: the expression form (bindings, calls, the nouns for
//! the outside world, routes and holons as blocks) and the wiring form (`node`, `a.out -> b.in`),
//! so a file may hold both and a canvas that the expression form can't spell still round-trips.
//!
//! The tree is [`ast`], every node with its span; [`lex`] makes the tokens; [`parse`] is the
//! parser, whose errors say the line and column; [`lower`] turns the tree into a flow document
//! through the flow builder, every node from the live catalog, so a text and the same flow drawn
//! on the canvas are the same unit; [`print`] writes a flow document back as canonical text, the
//! same unit again; [`layout`] places a compiled text's nodes on the canvas; [`ts`] reads a flow
//! written in TypeScript's syntax (`NAME.holon.ts`) into the same tree, so it lowers the same way;
//! [`format`] lays such a text out as `holon fmt` writes it, from the text, not the graph;
//! [`to_ts`] writes a `.holon` tree in TypeScript's syntax, the same unit again. [`compile`]
//! takes a text in either form.

pub mod ast;
pub mod format;
pub mod layout;
pub mod lex;
pub mod lower;
pub mod parse;
pub mod print;
pub mod to_ts;
pub mod ts;

pub use lower::{lower, LowerError, Lowered};
pub use parse::{parse, ParseError};
pub use print::print;

/// Parse and lower a flow's text in either form, told apart by the text: TypeScript's syntax
/// (`NAME.holon.ts`) or `.holon`, while the samples and the tools switch to the first (slice 4).
pub fn compile(src: &str) -> Result<Lowered, LowerError> {
    if is_typescript(src) { ts::compile(src) } else { lower::compile(src) }
}

/// Whether a flow's text is in TypeScript's syntax: past its first blanks it starts with a word
/// only TypeScript begins with (`import`, `export`, `const`, `function`), with `flow(`, or with a
/// `//` or `/*` comment. A `.holon` text starts with `#`, `flow name`, or a header.
pub fn is_typescript(src: &str) -> bool {
    let rest = src.trim_start_matches('\u{feff}').trim_start();
    if rest.starts_with("//") || rest.starts_with("/*") {
        return true;
    }
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let ts_word = matches!(word.as_str(), "import" | "export" | "const" | "let" | "var" | "function" | "type" | "interface" | "declare");
    ts_word || (word == "flow" && rest[word.len()..].trim_start().starts_with('('))
}
