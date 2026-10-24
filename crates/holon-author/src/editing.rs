//! What an editor asks of a `.holon` text as it is typed (docs/L4s/language-server-L4.md): its
//! mistakes, with their spans; what the word under the cursor is; and the text in the canonical
//! form. `holon lsp` speaks these to an editor; the answers are the ones `holon check` and
//! `holon fmt` give, and the reference `holon mcp` gives an agent, so an editor, the command line
//! and an agent read a flow alike.

use holon_core::MapResolver;

use crate::agent;
use crate::commands;
use crate::composites::Names;
use crate::lang::ast::{HeaderKind, Item, Pos, Span};
use crate::lang::lex::{lex, Tok, Token};
use crate::lang::{self, parse};
use crate::text;

/// Something wrong with a text, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Mistake {
    pub span: Span,
    pub message: String,
}

/// What a hover says, and the span of the word it is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub span: Span,
    /// Markdown.
    pub text: String,
}

/// The span of the `flow` header, where a mistake about the whole flow is shown; else the start.
fn flow_span(source: &str) -> Span {
    let at = parse(source).ok().and_then(|f| f.headers.iter().find(|h| matches!(h.kind, HeaderKind::Flow(_))).map(|h| h.span));
    at.unwrap_or(Span { start: Pos { line: 1, col: 1 }, end: Pos { line: 1, col: 1 } })
}

/// Every mistake `holon check` finds: the first one the text can't be compiled past, with its
/// span; or, compiled, what is wrong with the flow as a whole (at its `flow` header).
pub fn mistakes(source: &str) -> Vec<Mistake> {
    let compiled = match lang::compile(source) {
        Ok(c) => c,
        Err(e) => return vec![Mistake { span: e.span, message: e.message }],
    };
    let mut store = MapResolver::new();
    text::restore(&compiled.doc, &mut store, &Names::new());
    let span = flow_span(source);
    commands::problems(&compiled, &mut store).into_iter().map(|message| Mistake { span, message }).collect()
}

/// The text in the canonical form, as `holon fmt` writes it; or why it can't be.
pub fn format(source: &str) -> Result<String, String> {
    commands::fmt("text", source).map_err(|r| r.err.join("\n"))
}

/// The language's words, said in a line each.
const KEYWORDS: &[(&str, &str)] = &[
    ("port", "`port <n>`: the port the flow's server answers its routes on."),
    ("every", "`every <duration>`: how often a flow without routes runs (`10s`, `5m`, `1h`, `1d`)."),
    ("collection", "`collection <name>`: a named store of items, used as `name[key]`, `name[key] = v`, `delete name[key]` and `name.list(prefix: p, after: a, limit: 50)`. One permission for each use."),
    ("optional", "`optional <permission>`: one the flow can run without. It isn't asked for at deploy; a run without it gets no value where it would have read."),
    ("browsers", "`browsers <origin>, …`: the web pages it answers from other sites (CORS), or `*` for any."),
    ("holon", "`holon name(inputs) -> outputs { … }`: a flow inside a flow, a function placed as one node. One with several outputs is used as `h(…).name`."),
    ("route", "`route METHOD /path/:param (req) { … }`: answers requests on the flow's port. Every statement in it uses `req`, directly or through a binding."),
    ("resumes", "`route … (req) resumes name { … }`: the route that resumes a run parked at `await name`, taking its `resume_token`."),
    ("respond", "`respond <body>, status: 201, missing: 400`: the route's answer; `missing:` is the status when the body has no value. `respond 204` answers a status alone."),
    ("delete", "`delete name[key]`: removes an item from a collection when the run finishes."),
    ("await", "`await event, expires: 1d`: parks the run until the event arrives; the route that `resumes` it goes on."),
    ("if", "`if c then a else b`: `a` when `c` is true, else `b`. A fallback, not a branch: both sides are computed."),
    ("then", "`if c then a else b`: `a` when `c` is true, else `b`."),
    ("else", "`if c then a else b`: `b` when `c` isn't true, or `a` has no value."),
    ("when", "`a when cond`: `a` only when `cond` is true; else no value."),
    ("fn", "`fn(x) -> expr`: a function for `map` and `fold`, using only its parameters."),
    ("node", "`node name = leaf { params }`: the wiring form's node, wired with `a.out -> b.in`."),
    ("interval", "`interval <seconds>`: the wiring form's `every`."),
];

/// The nouns for the outside world: each read or write is one permission the flow asks for.
const NOUNS: &[(&str, &str)] = &[
    ("secret", "`secret.name`: a secret, set by an admin under the deployment's ⚙ and never in the run log. One permission: reading it."),
    ("state", "`state.name`: the flow's own value, read at the start of the run; `state.name = v` writes it when the run finishes. Each is one permission."),
    ("now", "`now`: the run's time, in epoch seconds, recorded with the run so a replay sees the same. A permission: reading the clock."),
    ("random", "`random`: one random number for the run, recorded so a replay sees the same. A permission: randomness."),
    ("http", "`http.get(host, path, headers: h, body: b)`, also `post`, `put`, `patch`, `delete`, `head`: a call out. The host, a text literal, is the permission; the answer is recorded with the run."),
    ("stripe", "`stripe.verify(req, secret.whsec, now)`: the request as a verified Stripe event (`.event`), or no value."),
    ("ingress", "`ingress`: what a flow without routes receives when triggered."),
];

/// What a route's request holds.
const REQUEST: &str = "the route's request: `.params` (the path's `:names`), `.query`, `.headers`, `.body` (text) and `.json` (the body parsed).";

fn code(sig: &str, doc: &str) -> String {
    if doc.is_empty() {
        format!("```holon\n{sig}\n```")
    } else {
        format!("```holon\n{sig}\n```\n{doc}")
    }
}

fn line_of<'a>(table: &'a [(&str, &str)], word: &str) -> Option<&'a str> {
    table.iter().find(|(w, _)| *w == word).map(|(_, l)| *l)
}

/// A leaf or template, as the reference says it: its signature, then what it does.
fn leaf(word: &str) -> Option<String> {
    let said = agent::reference(Some(word)).ok()?;
    if said.starts_with('#') {
        return None; // a sample, by its name
    }
    let (sig, doc) = said.split_once("\n\n").unwrap_or((&said, ""));
    Some(code(sig, doc))
}

fn covers(span: Span, at: Pos) -> bool {
    let after_start = (at.line, at.col) >= (span.start.line, span.start.col);
    let before_end = (at.line, at.col) < (span.end.line, span.end.col);
    after_start && before_end
}

/// What the word at `at` is: a keyword, a noun, a leaf, a holon of this text, a collection, a
/// route's request, a binding (the node it is), or the flow itself (what `holon check` says of it).
pub fn hover(source: &str, at: Pos) -> Option<Hover> {
    let tokens = lex(source).ok()?;
    let i = tokens.iter().position(|t| t.tok == Tok::Ident && covers(t.span, at))?;
    let token = &tokens[i];
    let word = token.text.as_str();
    let before = |n: usize| i.checked_sub(n).map(|j| &tokens[j]);
    let is = |t: Option<&Token>, tok: Tok| t.is_some_and(|t| t.tok == tok);
    let file = parse(source).ok();
    let collections: Vec<&str> = file.iter().flat_map(|f| &f.headers).filter_map(|h| if let HeaderKind::Collection(c) = &h.kind { Some(c.as_str()) } else { None }).collect();
    let requests: Vec<&str> = file.iter().flat_map(|f| &f.items).filter_map(|item| if let Item::Route(r) = item { Some(r.request.text.as_str()) } else { None }).collect();
    let said = |text: String| Some(Hover { span: token.span, text });

    // A member: `secret.key`, `http.post`, `posts.put`, `req.json`; another field says nothing.
    if is(before(1), Tok::Dot) {
        let head = before(2).filter(|t| t.tok == Tok::Ident)?.text.as_str();
        if let Some(line) = line_of(NOUNS, head) {
            return said(line.into());
        }
        if collections.contains(&head) {
            return said(format!("`{head}.{word}`: on the collection `{head}`. {}", line_of(KEYWORDS, "collection").unwrap_or("")));
        }
        if requests.contains(&head) {
            return said(format!("`{head}.{word}`: part of {REQUEST}"));
        }
        return None;
    }
    if is(before(1), Tok::Ident) && before(1).is_some_and(|t| t.text == "flow") || word == "flow" {
        let report = commands::check("this flow", source);
        let mut lines = report.out;
        lines.extend(report.err);
        return said(code(&lines.join("\n"), "What `holon check` says of it, and what deploying it asks a person to approve."));
    }
    if let Some(line) = line_of(NOUNS, word) {
        return said(line.into());
    }
    if collections.contains(&word) {
        return said(format!("`{word}`: a collection of this flow. {}", line_of(KEYWORDS, "collection").unwrap_or("")));
    }
    if requests.contains(&word) {
        return said(format!("`{word}`: {REQUEST}"));
    }
    if let Some(line) = line_of(KEYWORDS, word) {
        return said(line.into());
    }
    let holon = file.iter().flat_map(|f| &f.items).find_map(|item| if let Item::Holon(h) = item { (h.name.text == word).then_some(h) } else { None });
    if let Some(h) = holon {
        let names = |ports: &[lang::ast::PortDecl]| ports.iter().map(|p| p.name.text.clone()).collect::<Vec<_>>().join(", ");
        let outputs = if h.outputs.len() == 1 { names(&h.outputs) } else { format!("({})", names(&h.outputs)) };
        return said(code(&format!("holon {word}({}) -> {outputs}", names(&h.inputs)), &format!("A holon of this flow, defined on line {}.", h.span.start.line)));
    }
    if let Some(text) = leaf(word) {
        return said(text);
    }
    // A binding: the node it is.
    let compiled = lang::compile(source).ok()?;
    let id = compiled.names.iter().find(|(_, name)| name.as_str() == word).map(|(id, _)| id)?;
    let node = compiled.doc.nodes.iter().find(|n| &n.id == id)?;
    let what = leaf(&node.data.name).map(|l| format!("\n\n{l}")).unwrap_or_default();
    said(format!("`{word}`: a `{}` node.{what}", node.data.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples;

    const SLUGS: &str = include_str!("../samples/slugs.holon");

    /// The position of the `nth` (from 0) occurrence of `needle` in `source`, plus `into` chars.
    fn at(source: &str, needle: &str, nth: usize, into: u32) -> Pos {
        let mut seen = 0;
        for (l, line) in source.lines().enumerate() {
            let mut from = 0;
            while let Some(c) = line[from..].find(needle) {
                let col = from + c;
                if seen == nth {
                    return Pos { line: l as u32 + 1, col: line[..col].chars().count() as u32 + 1 + into };
                }
                seen += 1;
                from = col + needle.len();
            }
        }
        panic!("{needle} #{nth} isn't in the text")
    }

    fn says(source: &str, needle: &str, nth: usize) -> String {
        hover(source, at(source, needle, nth, 0)).map(|h| h.text).unwrap_or_default()
    }

    #[test]
    fn every_sample_has_no_mistakes() {
        for s in samples::samples() {
            assert_eq!(mistakes(s.text), vec![], "{}", s.name);
        }
    }

    #[test]
    fn a_mistake_has_its_span_and_the_words_check_uses() {
        let m = mistakes("flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].message, "unknown name \"nope\"");
        assert_eq!((m[0].span.start, m[0].span.end), (Pos { line: 5, col: 11 }, Pos { line: 5, col: 15 }));
        // Compiled, but no driver: said at the `flow` header.
        let m = mistakes("# two of one route\nflow f\nport 8200\n\nroute GET /a (req) {\n  respond 204\n}\nroute GET /a (req) {\n  respond 200\n}\n");
        assert_eq!(m.iter().map(|m| m.message.as_str()).collect::<Vec<_>>(), ["GET /a clashes with GET /a"], "once");
        assert_eq!((m[0].span.start.line, m[0].span.end.line), (2, 2), "at the flow header");
    }

    #[test]
    fn the_word_under_the_cursor_is_said() {
        assert!(says(SLUGS, "lowercase", 0).contains("lowercase("), "a leaf, by its signature");
        assert!(says(SLUGS, "secret", 1).contains("set by an admin"), "a noun");
        assert!(hover(SLUGS, at(SLUGS, "secret.receipt_key", 1, 8)).unwrap().text.contains("set by an admin"), "a noun's member");
        assert!(hover(SLUGS, at(SLUGS, "posts.put", 0, 6)).unwrap().text.contains("collection `posts`"), "a collection's method");
        assert!(says(SLUGS, "posts.put", 0).contains("a collection of this flow"), "a collection");
        assert!(says(SLUGS, "route", 0).contains("Every statement in it uses `req`"), "a keyword");
        assert!(says(SLUGS, "req)", 0).contains(".params"), "the request");
        assert!(hover(SLUGS, at(SLUGS, "req.json", 0, 4)).unwrap().text.contains(".json"), "a part of it");
        assert_eq!(says(SLUGS, "slugify(req", 0), "```holon\nholon slugify(text) -> slug\n```\nA holon of this flow, defined on line 6.");
        assert!(says(SLUGS, "published =", 0).contains("`published`: a `publish` node"), "a binding");
        let flow = says(SLUGS, "slugs", 0);
        assert!(flow.contains("POST /slugs") && flow.contains("it may:") && flow.contains("receive requests"), "the flow: {flow}");
        assert_eq!(hover(SLUGS, at(SLUGS, "\"-\"", 0, 1)), None, "inside a string");
        assert_eq!(hover(SLUGS, at(SLUGS, "published.receipt", 0, 10)), None, "a field of a value");
    }

    #[test]
    fn format_is_fmt() {
        assert_eq!(format(SLUGS).unwrap(), SLUGS);
        assert_eq!(format("flow f\nport 8200\nroute GET /a (req) {\n    respond   204\n}").unwrap(), "flow f\nport 8200\n\nroute GET /a (req) {\n  respond 204\n}\n");
        assert!(format("flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n").unwrap_err().contains("5:11: unknown name"));
    }
}
