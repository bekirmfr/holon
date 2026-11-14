//! The source formatter: a `.holon.ts` text as `holon fmt` writes it (docs/L4s/typescript-syntax-L4.md
//! §13). The layout is Prettier's, as Oxfmt writes it (`oxc_formatter`): two spaces, double
//! quotes, no semicolons, 100 columns, a list that doesn't fit broken one item to a line with a
//! trailing comma, a chain of calls that doesn't fit broken one call to a line, every comment kept
//! and a blank line where the text had one. A `js()` body is laid out like the rest. Before it, fmt
//! rewrites `var` to `const` (or `let` where it is assigned again) and `==` to `===`, outside a
//! `js()` body, where JavaScript gives them meanings of their own. An object's key keeps its
//! quotes as written.
//!
//! The text must compile, and what fmt writes is the same: the same program but for its layout,
//! the same flow but for its `js()` bodies' text, and every comment. It checks each before it
//! answers.

use std::collections::BTreeSet;

use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_ast_visit::{Visit, walk};
use oxc_formatter::{JsFormatOptions, QuoteProperties, Semicolons};
use oxc_formatter_core::LineWidth;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{ContentEq, GetSpan, SourceType};

use super::ast::{Pos, Span};
use super::lower::LowerError;
use super::{nesting, ts};

/// The width a line is laid out to.
const WIDTH: u16 = 100;

/// `src` laid out as `holon fmt` writes it, or why it can't be: the mistake that keeps it from
/// compiling, said at its place. Counted for its nesting first, then laid out on a stack with room
/// for it, whatever thread fmt is called on: Oxfmt's formatter, the parses and the walks recurse
/// once for each level, and in a debug build a level of the layout takes tens of KiB (chess, the
/// largest sample, needs about 1.2 MiB, more than the 1 MiB a Windows program's main thread has).
pub fn format(src: &str) -> Result<String, LowerError> {
    let src = src.replace("\r\n", "\n");
    nesting::check(&src)?;
    nesting::on_deep_stack(|| formatted(&src))
}

fn formatted(src: &str) -> Result<String, LowerError> {
    let before = ts::compile(src)?;
    let rewritten = rewrite(src)?;
    let out = laid_out(&rewritten)?;
    // Laid out, a text can nest a level or two deeper than it was written: fmt puts parentheses
    // around what it breaks over lines (`return (…)`), after a `**`, around a sign after a sign.
    if nesting::check(&out).is_err() {
        let at = nesting::deepest_at(src);
        let message = format!("laid out as fmt writes it, with the parentheses it adds, this would nest more than {} levels deep: name a part of it, with a const or a function", nesting::MAX_NESTING);
        return Err(LowerError { span: Span { start: at, end: at }, message });
    }
    let wrong =|e: LowerError| mistake(format!("fmt wrote a text that doesn't compile ({e}), so it leaves this one as it is: a mistake in fmt"));
    ts::compile(&out).map_err(wrong)?;
    if !same_program(&rewritten, &out) {
        return Err(mistake("fmt's layout would change the code, so it leaves the text as it is: a mistake in fmt".into()));
    }
    // A `js()` body's text is its node's identity, so the flow is compared with each body as written.
    let after = ts::compile(&with_bodies(&out, src)).map_err(wrong)?;
    if before.name != after.name || unplaced(&before.doc) != unplaced(&after.doc) {
        return Err(mistake("fmt's layout would change what this flow does, so it leaves the text as it is: a mistake in fmt".into()));
    }
    if comments(src) != comments(&out) {
        return Err(mistake("fmt's layout would lose a comment, so it leaves the text as it is: a mistake in fmt".into()));
    }
    Ok(out)
}

/// `src` laid out as fmt does, but each `js()` body as written: a canvas printed as text
/// (lang/print.rs), whose bodies are its nodes' identity, so the text lowers to the same units.
pub fn format_keeping_bodies(src: &str) -> Result<String, LowerError> {
    let src = src.replace("\r\n", "\n");
    nesting::check(&src)?;
    nesting::on_deep_stack(|| formatted(&src).map(|out| with_bodies(&out, &src)))
}

/// The layout, with no check that it is the same flow.
fn laid_out(src: &str) -> Result<String, LowerError> {
    let allocator = Allocator::default();
    let options = JsFormatOptions {
        line_width: LineWidth::try_from(WIDTH).expect("a width oxc takes"),
        semicolons: Semicolons::AsNeeded,
        quote_properties: QuoteProperties::Preserve,
        ..JsFormatOptions::default()
    };
    let laid = oxc_formatter::format(&allocator, src, SourceType::ts(), options).map_err(|e| mistake(e.message.to_string()))?;
    laid.print().map(|p| p.into_code()).map_err(|e| mistake(format!("fmt can't lay this text out ({e:?}): a mistake in fmt")))
}

/// `src` with its `var`s and `==`s rewritten, outside its `js()` bodies.
fn rewrite(src: &str) -> Result<String, LowerError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    if let Some(e) = parsed.diagnostics.errors().next() {
        return Err(mistake(e.message.to_string()));
    }
    let mut writes = Writes::default();
    writes.visit_program(&parsed.program);
    let mut r = Rewrites { src, comments: &parsed.program.comments, writes: &writes.0, edits: Vec::new() };
    r.visit_program(&parsed.program);
    r.edits.sort_by_key(|e| e.0);
    let mut out = src.to_string();
    for (at, len, with) in r.edits.into_iter().rev() {
        out.replace_range(at..at + len, with);
    }
    Ok(out)
}

/// Whether two texts are one program but for layout: the same tree, without places, comments or
/// parentheses, and literals by their values (`'a'` is `"a"`).
fn same_program(a: &str, b: &str) -> bool {
    let options = || ParseOptions { preserve_parens: false, ..ParseOptions::default() };
    let (x, y) = (Allocator::default(), Allocator::default());
    let p = Parser::new(&x, a, SourceType::ts()).with_options(options()).parse();
    let q = Parser::new(&y, b, SourceType::ts()).with_options(options()).parse();
    p.diagnostics.errors().next().is_none() && q.diagnostics.errors().next().is_none() && p.program.content_eq(&q.program)
}

/// `out` with each `js()` function as `src` wrote it: the two are one program, so their functions
/// pair up in order.
fn with_bodies(out: &str, src: &str) -> String {
    let mut text = out.to_string();
    for (o, s) in functions(out).into_iter().zip(functions(src)).rev() {
        text.replace_range(o, &src[s]);
    }
    text
}

/// Where each `js()` function is in `src`, in order.
fn functions(src: &str) -> Vec<std::ops::Range<usize>> {
    #[derive(Default)]
    struct Found(Vec<std::ops::Range<usize>>);
    impl<'a> Visit<'a> for Found {
        fn visit_call_expression(&mut self, c: &js::CallExpression<'a>) {
            match js_function(c) {
                Some(f) => {
                    self.0.push(f.span.start as usize..f.span.end as usize);
                    c.arguments.iter().skip(1).for_each(|a| self.visit_argument(a));
                }
                None => walk::walk_call_expression(self, c),
            }
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    let mut found = Found::default();
    found.visit_program(&parsed.program);
    found.0
}

/// The function a `js(function name(…) { … }, …)` call is passed.
fn js_function<'b, 'a>(c: &'b js::CallExpression<'a>) -> Option<&'b js::Function<'a>> {
    let js::Expression::Identifier(callee) = &c.callee else { return None };
    let mut first = c.arguments.first()?.as_expression()?;
    while let js::Expression::ParenthesizedExpression(p) = first {
        first = &p.expression;
    }
    match first {
        js::Expression::FunctionExpression(f) if callee.name == "js" => Some(&**f),
        _ => None,
    }
}

/// The text's comments in order, each as written but for the spaces around its lines, which a
/// layout indents.
fn comments(src: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    let trimmed = |c: &str| c.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    parsed.program.comments.iter().map(|c| trimmed(&src[c.span.start as usize..c.span.end as usize])).collect()
}

/// A flow document without where in the text each node was written, which a layout moves.
fn unplaced(doc: &crate::builder::FlowDocument) -> serde_json::Value {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("source").is_some_and(|s| s.get("line").is_some()) {
                    m.remove("source");
                }
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(doc).unwrap_or_default();
    strip(&mut v);
    v
}

fn mistake(message: String) -> LowerError {
    let at = Pos { line: 1, col: 1 };
    LowerError { span: Span { start: at, end: at }, message }
}

/// The names the text assigns again (`x = …`, `x++`, `({ x } = …)`): a `var` among them is a `let`.
#[derive(Default)]
struct Writes(BTreeSet<String>);

impl<'a> Visit<'a> for Writes {
    fn visit_simple_assignment_target(&mut self, it: &js::SimpleAssignmentTarget<'a>) {
        if let js::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = it {
            self.0.insert(id.name.to_string());
        }
        walk::walk_simple_assignment_target(self, it);
    }

    fn visit_assignment_target_property_identifier(&mut self, it: &js::AssignmentTargetPropertyIdentifier<'a>) {
        self.0.insert(it.binding.name.to_string());
        walk::walk_assignment_target_property_identifier(self, it);
    }
}

/// The rewrites, as edits to the text: where, how many bytes, and what instead.
struct Rewrites<'s> {
    src: &'s str,
    comments: &'s [oxc_ast::Comment],
    writes: &'s BTreeSet<String>,
    edits: Vec<(usize, usize, &'static str)>,
}

impl Rewrites<'_> {
    /// Where the operator between two operands is: past spaces, parentheses and comments.
    fn operator(&self, from: u32, to: u32) -> Option<usize> {
        let mut at = from;
        while at < to {
            if let Some(c) = self.comments.iter().find(|c| c.span.start == at) {
                at = c.span.end;
                continue;
            }
            let rest = &self.src[at as usize..to as usize];
            if rest.starts_with("==") || rest.starts_with("!=") {
                return Some(at as usize);
            }
            at += rest.chars().next().map_or(1, |c| c.len_utf8() as u32);
        }
        None
    }
}

impl<'a> Visit<'a> for Rewrites<'_> {
    fn visit_call_expression(&mut self, c: &js::CallExpression<'a>) {
        // A `js()` body is JavaScript, where `var` and `==` mean what they do there.
        if js_function(c).is_some() {
            c.arguments.iter().skip(1).for_each(|a| self.visit_argument(a));
            return;
        }
        walk::walk_call_expression(self, c);
    }

    fn visit_variable_declaration(&mut self, v: &js::VariableDeclaration<'a>) {
        if v.kind == js::VariableDeclarationKind::Var && self.src[v.span.start as usize..].starts_with("var") {
            let mut names = Vec::new();
            v.declarations.iter().for_each(|d| bound(&d.id, &mut names));
            let assigned = names.iter().any(|n| self.writes.contains(n));
            let kind = if assigned || v.declarations.iter().any(|d| d.init.is_none()) { "let" } else { "const" };
            self.edits.push((v.span.start as usize, 3, kind));
        }
        walk::walk_variable_declaration(self, v);
    }

    fn visit_binary_expression(&mut self, b: &js::BinaryExpression<'a>) {
        let strict = match b.operator {
            js::BinaryOperator::Equality => Some("==="),
            js::BinaryOperator::Inequality => Some("!=="),
            _ => None,
        };
        if let Some(with) = strict
            && let Some(at) = self.operator(b.left.span().end, b.right.span().start)
        {
            self.edits.push((at, 2, with));
        }
        walk::walk_binary_expression(self, b);
    }
}

/// The names a binding pattern binds.
fn bound(p: &js::BindingPattern, out: &mut Vec<String>) {
    match p {
        js::BindingPattern::BindingIdentifier(id) => out.push(id.name.to_string()),
        js::BindingPattern::ObjectPattern(o) => {
            o.properties.iter().for_each(|p| bound(&p.value, out));
            if let Some(r) = &o.rest {
                bound(&r.argument, out);
            }
        }
        js::BindingPattern::ArrayPattern(a) => {
            a.elements.iter().flatten().for_each(|p| bound(p, out));
            if let Some(r) = &a.rest {
                bound(&r.argument, out);
            }
        }
        js::BindingPattern::AssignmentPattern(a) => bound(&a.left, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fmt's own layout of `src`, which must be what it already is.
    fn kept(src: &str) {
        let out = format(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert_eq!(out, src, "fmt changed it");
    }

    fn becomes(src: &str, expected: &str) {
        let out = format(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert_eq!(out, expected);
        assert_eq!(format(&out).unwrap(), out, "fmt is not idempotent on its own output");
    }

    const HEAD: &str = "import { flow, js, respond, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\", { port: 8000 })\n";

    const PAGE: &str = "\nconst page = js(function html(count) {\n  return `<p>${count}</p>`\n})\n\n";

    #[test]
    fn a_text_as_fmt_writes_it_is_kept() {
        kept(&format!("{HEAD}\n// Counts.\nroute.post(\"/count\", (req) => {{\n  const n = (state.count ?? 0) + 1\n  state.count = n\n  return n\n}})\n"));
    }

    #[test]
    fn layout_is_canonical() {
        becomes(
            &format!("{HEAD}route.post( '/count',(req)=>{{ const n=(state.count??0)+1;\n\n\n state.count=n ; return n }} )\n"),
            &format!("{HEAD}route.post(\"/count\", (req) => {{\n  const n = (state.count ?? 0) + 1\n\n  state.count = n\n  return n\n}})\n"),
        );
    }

    #[test]
    fn var_and_loose_equality_are_rewritten() {
        becomes(
            &format!("{HEAD}route.post(\"/v\", (req) => {{\n  var a = req.json.a\n  var b = 1\n  if (a == 2) b = 3\n  return a != b\n}})\n"),
            &format!("{HEAD}route.post(\"/v\", (req) => {{\n  const a = req.json.a\n  let b = 1\n  if (a === 2) b = 3\n  return a !== b\n}})\n"),
        );
    }

    #[test]
    fn a_js_body_keeps_its_var_and_loose_equality() {
        kept(&format!("{HEAD}\nconst loose = js(function alike(a, b) {{\n  var c = a\n  return c == b\n}})\n\nroute.get(\"/\", (req) => loose(req.query.a, req.query.b))\n"));
    }

    #[test]
    fn comments_stay_where_they_were() {
        kept(&format!(
            "{HEAD}\n// The count.\n// Two lines.\nroute.get(\"/count\", () => state.count ?? 0) // on its line\n\nroute.post(\"/count\", (req) => {{\n  // before\n  state.count = req.json.n // after\n\n  /* block */ return \"ok\"\n  // last\n}})\n"
        ));
    }

    #[test]
    fn a_comment_inside_an_expression_is_laid_out_with_it() {
        becomes(
            &format!("{HEAD}\nroute.post(\"/c\", (req) => {{\n  state.count = add(req.json.n,  /* two */ 2)\n  return \"ok\"\n}})\n\nfunction add(a, b) {{\n  return a + b\n}}\n"),
            &format!("{HEAD}\nroute.post(\"/c\", (req) => {{\n  state.count = add(req.json.n, /* two */ 2)\n  return \"ok\"\n}})\n\nfunction add(a, b) {{\n  return a + b\n}}\n"),
        );
    }

    #[test]
    fn a_js_body_is_laid_out_like_the_rest() {
        becomes(
            &format!("{HEAD}\nconst page = js(function html(count) {{\n    return `<p>${{count}}</p>`;   \n}})\n\nroute.get(\"/\", () => page(state.count ?? 0))\n"),
            &format!("{HEAD}\nconst page = js(function html(count) {{\n  return `<p>${{count}}</p>`\n}})\n\nroute.get(\"/\", () => page(state.count ?? 0))\n"),
        );
    }

    #[test]
    fn a_long_call_breaks_hugging_its_function_or_object() {
        becomes(
            &format!("{HEAD}{PAGE}route.get(\"/\", () => respond(page(state.ticks ?? 0), {{ headers: {{ \"content-type\": \"text/html; charset=utf-8\" }}, status: 200 }}))\n"),
            &format!("{HEAD}{PAGE}route.get(\"/\", () =>\n  respond(page(state.ticks ?? 0), {{\n    headers: {{ \"content-type\": \"text/html; charset=utf-8\" }},\n    status: 200,\n  }}),\n)\n"),
        );
    }

    #[test]
    fn a_handlers_options_after_its_function_break_its_arguments_one_to_a_line() {
        becomes(
            &format!("{HEAD}\nroute.post(\"/n\", (req) => {{\n  state.n = req.json.n\n  return \"ok\"\n}}, {{ log: \"lazy\" }})\n"),
            &format!("{HEAD}\nroute.post(\n  \"/n\",\n  (req) => {{\n    state.n = req.json.n\n    return \"ok\"\n  }},\n  {{ log: \"lazy\" }},\n)\n"),
        );
    }

    /// Its head is the reads before the first call, `req.query.title`.
    #[test]
    fn a_long_chain_breaks_one_call_to_a_line() {
        becomes(
            &format!("{HEAD}\nroute.get(\"/slug\", (req) => {{\n  const slug = req.query.title.toLowerCase().split(\" \").map((word) => word.toUpperCase()).join(\"-\").split(\"--\").join(\"-\")\n  return slug\n}})\n"),
            &format!("{HEAD}\nroute.get(\"/slug\", (req) => {{\n  const slug = req.query.title\n    .toLowerCase()\n    .split(\" \")\n    .map((word) => word.toUpperCase())\n    .join(\"-\")\n    .split(\"--\")\n    .join(\"-\")\n  return slug\n}})\n"),
        );
    }

    #[test]
    fn a_short_chain_stays_on_its_line() {
        kept(&format!("{HEAD}\nroute.get(\"/slug\", (req) => req.query.title.toLowerCase().split(\" \"))\n"));
    }

    /// A flow written any which way, every construct a handler holds, as fmt lays it out.
    /// `HOLON_BLESS=1` writes the layout again, for a change to fmt's.
    #[test]
    fn a_flow_written_any_which_way() {
        let messy = include_str!("../../tests/fmt/messy.holon.ts");
        if std::env::var_os("HOLON_BLESS").is_some() {
            let laid = format(messy).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fmt/messy.laid.holon.ts"), laid).unwrap();
        }
        becomes(messy, include_str!("../../tests/fmt/messy.laid.holon.ts"));
    }

    #[test]
    fn an_object_the_text_broke_stays_broken() {
        kept(&format!("{HEAD}\nroute.get(\"/\", () => ({{\n  a: 1,\n  b: 2,\n}}))\n"));
        becomes(&format!("{HEAD}route.get(\"/\", () => ({{ a: 1,\n  b: 2 }}))\n"), &format!("{HEAD}route.get(\"/\", () => ({{ a: 1, b: 2 }}))\n"));
    }

    #[test]
    fn an_objects_key_keeps_its_quotes() {
        kept(&format!("{HEAD}\nroute.get(\"/\", () => ({{ \"$in\": [1], b: 2 }}))\n"));
    }
}
