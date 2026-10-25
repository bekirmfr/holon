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
    ("folder", "`folder <name>`: files on the host, in a folder an admin binds to the name: `name[file]` reads a file's text, `name[file] = text` writes it when the run finishes (`name.create(file, text)` only if no file has that name), `delete name[file]` deletes it, `name.list(prefix: p, after: a, limit: 50)` lists files. One permission to read, one to write."),
    ("on", "`on file <folder>`: a run for each file new or changed in the folder (an admin binds it), once it stops changing; `on file <folder> deleted`, for each file removed; `matching \"*.csv\"` after either, only for names that match. The run reads the file's handle as `ingress` (`.name`, `.size`, `.hash`, `.folder`, `.event`). A flow may have several lines; `ingress` is the file that started the run."),
    ("optional", "`optional <permission>`: one the flow can run without. It isn't asked for at deploy; a run without it gets no value where it would have read."),
    ("browsers", "`browsers <origin>, …`: the web pages it answers from other sites (CORS), or `*` for any."),
    ("holon", "`holon name(inputs) -> outputs { … }`: a flow inside a flow, a function placed as one node. One with several outputs is used as `h(…).name`. With `js` after the outputs, its body is JavaScript."),
    ("js", "`holon name(inputs) -> outputs js fuel 50M memory 16MB { … }`: a holon whose body is JavaScript, run with no clock, randomness or I/O, under its budgets (200M instructions and 16 MiB when not written)."),
    ("route", "`route METHOD /path/:param (req) { … }`: answers requests on the flow's port. Every statement in it uses `req`, directly or through a binding."),
    ("resumes", "`route … (req) resumes name { … }`: the route that resumes a run parked at `await name`, taking its `resume_token`."),
    ("respond", "`respond <body>, status: 201, missing: 400`: the route's answer; `missing:` is the status when the body has no value. `respond 204` answers a status alone."),
    ("delete", "`delete name[key]`: removes an item from a collection, or a file from a folder, when the run finishes."),
    ("await", "`await event, expires: 1d`: parks the run until the event arrives; the route that `resumes` it goes on."),
    ("if", "`if c then a else b`: `a` when `c` is true, else `b`. A fallback, not a branch: when `a` has no value, `b`. Only the side it needs runs."),
    ("then", "`if c then a else b`: `a` when `c` is true, else `b`."),
    ("else", "`if c then a else b`: `b` when `c` isn't true, or `a` has no value. In a `match`, `else -> v`: `v` when no pattern matched."),
    ("match", "`match x { \"a\" -> v, 1, 2 -> w, else -> z }`: the value of the arm whose pattern equals `x`; only that arm runs. A chosen arm with no value makes the match have none: it never falls to another arm."),
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
    ("http", "`http.get(host, path, headers: h, body: b)`, also `post`, `put`, `patch`, `delete`, `head`: a call out. The host, a text literal, is the permission; the answer is recorded with the run. `file: inbox.file(name)` in place of `body:` sends a file's bytes, streamed by the server."),
    ("stripe", "`stripe.verify(req, secret.whsec, now)`: the request as a verified Stripe event (`.event`), or no value."),
    ("ingress", "`ingress`: what a flow without routes receives when triggered; with `on file`, the new file's handle."),
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
    let folders: Vec<&str> = file.iter().flat_map(|f| &f.headers).filter_map(|h| if let HeaderKind::Folder(f) = &h.kind { Some(f.as_str()) } else { None }).collect();
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
        if folders.contains(&head) {
            return said(format!("`{head}.{word}`: on the folder `{head}`. {}", line_of(KEYWORDS, "folder").unwrap_or("")));
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
    if folders.contains(&word) {
        return said(format!("`{word}`: a folder of this flow. {}", line_of(KEYWORDS, "folder").unwrap_or("")));
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

/// What a completion offers is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    /// A leaf or a holon: called as `name(…)`.
    Function,
    /// A binding, a route's request, a secret or state key used in the text.
    Variable,
    /// A collection, or a noun for the outside world.
    Module,
    /// After a `.`: a part of the request, a noun's or collection's method.
    Member,
}

/// One word a completion offers, with what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub label: String,
    pub kind: Kind,
    /// A line: a signature, or what it is.
    pub detail: String,
    /// Markdown, if there is more to say.
    pub doc: Option<String>,
}

/// Leaves a text doesn't call by name: it writes them as nouns (`http.get`, `state.x`,
/// `notes[k]`, `now`, `secret.k`, `await`) or literals (`1`, `{ … }`).
const SPOKEN_OTHERWISE: &[&str] = &[
    "http_request", "http_upload", "clock", "random", "ingress", "file_arrival", "secret", "await", "store_read", "store_write", "collection_get", "collection_list", "collection_put",
    "collection_delete", "file_read", "file_list", "file_write", "file_delete", "file_handle", "file_lines", "file_slice", "file_copy", "file_create", "file_create_copy", "egress",
    "const", "record", "json_parse", "json_stringify",
];

/// The leaves a text calls by name, as it names them: their name, signature and what they do.
fn callable() -> Vec<(String, String, String)> {
    let mut all: Vec<(String, String, String)> = agent::leaves().into_iter().filter(|(n, _, _)| !SPOKEN_OTHERWISE.contains(&n.as_str())).map(|(n, s, d)| (n, s, d.to_string())).collect();
    for (written, leaf) in [("json", "json_stringify"), ("parse_json", "json_parse")] {
        if let Some((_, sig, doc)) = agent::leaves().into_iter().find(|(n, _, _)| n == leaf) {
            all.push((written.into(), sig.replacen(leaf, written, 1), doc.to_string()));
        }
    }
    all
}

/// Whether `line` (the text before the cursor on its line) ends inside a `#` comment.
fn in_comment(line: &str) -> bool {
    let mut quoted = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted => {
                chars.next();
            }
            '"' => quoted = !quoted,
            '#' if !quoted => return true,
            _ => {}
        }
    }
    false
}

fn word(label: &str, kind: Kind, detail: impl Into<String>, doc: Option<String>) -> Completion {
    Completion { label: label.to_string(), kind, detail: detail.into(), doc }
}

/// What may be written at `at`: after `head.`, `head`'s members; elsewhere, the language's words,
/// the leaves it calls by name, and this text's holons, bindings, collections and requests. Inside
/// a string or a comment, nothing.
pub fn complete(source: &str, at: Pos) -> Vec<Completion> {
    let lines: Vec<&str> = source.split('\n').collect();
    let line = lines.get(at.line.saturating_sub(1) as usize).map(|l| l.strip_suffix('\r').unwrap_or(l)).unwrap_or("");
    let before: String = line.chars().take(at.col.saturating_sub(1) as usize).collect();
    if in_comment(&before) {
        return vec![];
    }
    let mut prefix = lines[..at.line.saturating_sub(1) as usize].join("\n");
    if at.line > 1 {
        prefix.push('\n');
    }
    prefix.push_str(&before);
    // A prefix that doesn't lex ends inside a string (or its `{…}`): nothing to offer.
    let Ok(typed) = lex(&prefix) else { return vec![] };
    // (The lexer ends a text with a newline; one at the cursor isn't typed.)
    let typed: Vec<&Token> = typed.iter().filter(|t| t.tok != Tok::Eof && !(t.tok == Tok::Newline && (t.span.start.line, t.span.start.col) >= (at.line, at.col))).collect();
    let (partial, context) = match typed.last() {
        Some(t) if t.tok == Tok::Ident && t.span.end == at => (t.text.as_str(), typed.len().checked_sub(2).map(|i| typed[i])),
        last => ("", last.copied()),
    };
    // The whole text's words, as far as it lexes: names may be defined after the cursor.
    let all = lex(source).unwrap_or_else(|_| typed.iter().map(|t| (*t).clone()).collect());
    let after = |i: usize, tok: Tok| all.get(i + 1).is_some_and(|t| t.tok == tok);
    let ident_after = |i: usize| all.get(i + 1).filter(|t| t.tok == Tok::Ident).map(|t| t.text.clone());
    let header = |kw: &str| -> Vec<String> {
        let mut found: Vec<String> = all.iter().enumerate().filter(|(_, t)| t.tok == Tok::Ident && t.text == kw).filter_map(|(i, _)| ident_after(i)).collect();
        found.dedup();
        found
    };
    let collections = header("collection");
    let folders = header("folder");
    let holons = header("holon");
    // A noun's keys as used: `secret.k`, `state.k`.
    let keys = |noun: &str| -> Vec<String> {
        let mut found: Vec<String> = all.iter().enumerate().filter(|(i, t)| t.tok == Tok::Ident && t.text == noun && after(*i, Tok::Dot)).filter_map(|(i, _)| ident_after(i + 1)).filter(|k| k != "set").collect();
        found.sort();
        found.dedup();
        found
    };
    // Each route's request name: the word in `route METHOD /path (req)`'s parentheses.
    let requests: Vec<String> = all
        .iter()
        .enumerate()
        .filter(|(_, t)| t.tok == Tok::Ident && t.text == "route")
        .filter_map(|(i, _)| all[i..].iter().position(|t| t.tok == Tok::LParen).and_then(|p| ident_after(i + p)))
        .collect();

    let offered: Vec<Completion> = if context.is_some_and(|t| t.tok == Tok::Dot) {
        let head_at = typed.len().checked_sub(if partial.is_empty() { 2 } else { 3 });
        let Some(head) = head_at.and_then(|i| typed.get(i)).filter(|t| t.tok == Tok::Ident).map(|t| t.text.as_str()) else { return vec![] };
        match head {
            "secret" => keys("secret").iter().map(|k| word(k, Kind::Variable, format!("secret.{k}: used in this flow"), None)).collect(),
            "state" => {
                let mut v = vec![word("set", Kind::Member, "state.set(key, value): written when the run finishes", None)];
                v.extend(keys("state").iter().map(|k| word(k, Kind::Variable, format!("state.{k}: used in this flow"), None)));
                v
            }
            "http" => ["get", "post", "put", "patch", "delete", "head"]
                .iter()
                .map(|m| word(m, Kind::Member, format!("http.{m}(\"host\", path, headers: h, body: b)"), line_of(NOUNS, "http").map(str::to_string)))
                .collect(),
            "stripe" => vec![word("verify", Kind::Member, "stripe.verify(req, secret.whsec, now)", line_of(NOUNS, "stripe").map(str::to_string))],
            c if collections.iter().any(|k| k == c) => [
                ("get", "get(key): the item, or no value"),
                ("put", "put(key, value): written when the run finishes"),
                ("delete", "delete(key): removed when the run finishes"),
                ("list", "list(prefix: p, after: a, limit: 50): a page of items"),
            ]
            .iter()
            .map(|(m, d)| word(m, Kind::Member, format!("{c}.{d}"), None))
            .collect(),
            f if folders.iter().any(|k| k == f) => [
                ("get", "get(name): the file's text, or no value"),
                ("put", "put(name, text): written when the run finishes"),
                ("delete", "delete(name): deleted when the run finishes"),
                ("list", "list(prefix: p, after: a, limit: 50): a page of files, each {size, hash, modified}"),
            ]
            .iter()
            .map(|(m, d)| word(m, Kind::Member, format!("{f}.{d}"), None))
            .collect(),
            r if requests.iter().any(|q| q == r) => [
                ("params", "the path's :names"),
                ("query", "the query string's names"),
                ("headers", "the request's headers, by lowercase name"),
                ("body", "the body, as text"),
                ("json", "the body, parsed as JSON"),
            ]
            .iter()
            .map(|(m, d)| word(m, Kind::Member, format!("{r}.{m}: {d}"), None))
            .collect(),
            _ => vec![],
        }
    } else {
        let mut v: Vec<Completion> = vec![];
        v.extend(holons.iter().map(|h| word(h, Kind::Function, "a holon of this flow", None)));
        let bindings = all.iter().enumerate().filter(|(i, t)| {
            let starts = i.checked_sub(1).map(|j| &all[j]).is_none_or(|p| matches!(p.tok, Tok::Newline | Tok::LBrace));
            t.tok == Tok::Ident && starts && after(*i, Tok::Eq) && !["node", "state"].contains(&t.text.as_str())
        });
        v.extend(bindings.map(|(_, t)| word(&t.text, Kind::Variable, "a binding of this flow", None)));
        v.extend(requests.iter().map(|r| word(r, Kind::Variable, "a route's request", Some(REQUEST.to_string()))));
        v.extend(collections.iter().map(|c| word(c, Kind::Module, "a collection of this flow", line_of(KEYWORDS, "collection").map(str::to_string))));
        v.extend(folders.iter().map(|f| word(f, Kind::Module, "a folder of this flow", line_of(KEYWORDS, "folder").map(str::to_string))));
        v.extend(NOUNS.iter().map(|(n, d)| word(n, Kind::Module, "the outside world: one permission", Some(d.to_string()))));
        v.extend(callable().into_iter().map(|(n, sig, doc)| word(&n, Kind::Function, sig, (!doc.is_empty()).then_some(doc))));
        v.extend(KEYWORDS.iter().map(|(k, d)| word(k, Kind::Keyword, "keyword", Some(d.to_string()))));
        v.extend(["flow", "true", "false"].iter().map(|k| word(k, Kind::Keyword, "keyword", None)));
        v
    };
    let mut seen = std::collections::BTreeSet::new();
    offered.into_iter().filter(|c| c.label.starts_with(partial) && seen.insert(c.label.clone())).collect()
}

/// What a stretch of a text is, for colouring it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Comment,
    Keyword,
    /// `secret`, `state`, `now`, …: the outside world.
    Noun,
    /// A call, a holon's name, a `node`'s leaf.
    Function,
    /// What a statement or header names: a binding, the flow, a collection, a `node`.
    Definition,
    /// A route's request, the event it resumes.
    Parameter,
    /// After a `.`.
    Member,
    /// A named argument or field: `status:`.
    Property,
    /// A route's HTTP method.
    Method,
    /// A route's path.
    Path,
    String,
    Escape,
    /// The braces of a string's `{…}` part (what is inside is coloured as code).
    Interpolation,
    Number,
    Boolean,
    Operator,
}

/// One coloured stretch: within one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub span: Span,
    pub class: Class,
}

/// The words an expression may hold that are the language's own.
const EXPRESSION_KEYWORDS: &[&str] = &["respond", "delete", "await", "if", "then", "else", "when", "match", "fn"];
const HEADERS: &[&str] = &["port", "every", "interval", "browsers", "optional"];
const OPERATORS: &[&str] = &["->", "..", "??", "&&", "||", "==", "!=", "<=", ">=", "+", "-", "*", "/", "<", ">", "!", "="];

struct Colourer {
    chars: Vec<char>,
    line: u32,
    out: Vec<Highlight>,
    /// The last thing read was a `.`: a word after it is a member.
    dot: bool,
}

impl Colourer {
    fn mark(&mut self, from: usize, to: usize, class: Class) {
        if to > from {
            let at = |i: usize| Pos { line: self.line, col: i as u32 + 1 };
            self.out.push(Highlight { span: Span { start: at(from), end: at(to) }, class });
        }
    }

    fn skip_space(&self, mut i: usize) -> usize {
        while self.chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        i
    }

    fn word_end(&self, mut i: usize, also: &[char]) -> usize {
        while self.chars.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || also.contains(c)) {
            i += 1;
        }
        i
    }

    fn word(&self, from: usize, to: usize) -> String {
        self.chars[from..to].iter().collect()
    }

    /// The next word after spaces, coloured `class`; where it ends.
    fn next_word(&mut self, i: usize, class: Class, also: &[char]) -> usize {
        let from = self.skip_space(i);
        let to = self.word_end(from, also);
        self.mark(from, to, class);
        to
    }

    /// A line: its header or statement form first (what it names), then the rest as code.
    fn line(&mut self) {
        let start = self.skip_space(0);
        let end = self.word_end(start, &[]);
        let first = self.word(start, end);
        let mut i = end;
        match first.as_str() {
            "flow" => {
                self.mark(start, end, Class::Keyword);
                i = self.next_word(i, Class::Definition, &['-']);
            }
            "collection" | "folder" => {
                self.mark(start, end, Class::Keyword);
                i = self.next_word(i, Class::Definition, &[]);
            }
            "holon" => {
                self.mark(start, end, Class::Keyword);
                i = self.next_word(i, Class::Function, &[]);
            }
            "node" => {
                self.mark(start, end, Class::Keyword);
                i = self.next_word(i, Class::Definition, &[]);
                let eq = self.skip_space(i);
                if self.chars.get(eq) == Some(&'=') {
                    self.mark(eq, eq + 1, Class::Operator);
                    i = self.next_word(eq + 1, Class::Function, &[]);
                }
            }
            "route" => {
                self.mark(start, end, Class::Keyword);
                i = self.next_word(i, Class::Method, &[]);
                let path = self.skip_space(i);
                let mut to = path;
                while self.chars.get(to).is_some_and(|c| !c.is_whitespace() && *c != '(') {
                    to += 1;
                }
                self.mark(path, to, Class::Path);
                i = to;
                let open = self.skip_space(i);
                if self.chars.get(open) == Some(&'(') {
                    i = self.next_word(open + 1, Class::Parameter, &[]);
                    let close = self.skip_space(i);
                    if self.chars.get(close) == Some(&')') {
                        i = close + 1;
                        let kw = self.skip_space(i);
                        let kw_end = self.word_end(kw, &[]);
                        if self.word(kw, kw_end) == "resumes" {
                            self.mark(kw, kw_end, Class::Keyword);
                            i = self.next_word(kw_end, Class::Parameter, &[]);
                        }
                    }
                }
            }
            h if HEADERS.contains(&h) => self.mark(start, end, Class::Keyword),
            w if !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()) => {
                // `name = …` (not `==`): a binding.
                let eq = self.skip_space(end);
                if self.chars.get(eq) == Some(&'=') && self.chars.get(eq + 1) != Some(&'=') && !NOUNS.iter().any(|(n, _)| *n == w) && !EXPRESSION_KEYWORDS.contains(&w) {
                    self.mark(start, end, Class::Definition);
                } else {
                    i = start;
                }
            }
            _ => i = start,
        }
        self.code(i, false);
    }

    /// Code from `i` to the line's end, or (`inside`, in a string's `{…}`) to the `}` that closes
    /// it: where it stopped.
    fn code(&mut self, mut i: usize, inside: bool) -> usize {
        let mut depth = 0;
        while let Some(&c) = self.chars.get(i) {
            match c {
                '#' => {
                    self.mark(i, self.chars.len(), Class::Comment);
                    return self.chars.len();
                }
                '"' => {
                    i = self.string(i);
                    self.dot = false;
                }
                '{' if inside => {
                    depth += 1;
                    i += 1;
                }
                '}' if inside => {
                    if depth == 0 {
                        return i;
                    }
                    depth -= 1;
                    i += 1;
                }
                c if c.is_whitespace() => i += 1,
                c if c.is_ascii_digit() => {
                    let mut to = self.word_end(i, &[]);
                    if self.chars.get(to) == Some(&'.') && self.chars.get(to + 1).is_some_and(|c| c.is_ascii_digit()) {
                        to = self.word_end(to + 1, &[]);
                    }
                    self.mark(i, to, Class::Number);
                    self.dot = false;
                    i = to;
                }
                c if c.is_ascii_alphabetic() || c == '_' => {
                    let to = self.word_end(i, &[]);
                    let w = self.word(i, to);
                    let next = self.skip_space(to);
                    let class = if self.dot {
                        Some(Class::Member)
                    } else if EXPRESSION_KEYWORDS.contains(&w.as_str()) {
                        Some(Class::Keyword)
                    } else if w == "true" || w == "false" {
                        Some(Class::Boolean)
                    } else if NOUNS.iter().any(|(n, _)| *n == w) {
                        Some(Class::Noun)
                    } else if self.chars.get(next) == Some(&'(') {
                        Some(Class::Function)
                    } else if self.chars.get(next) == Some(&':') && self.chars.get(next + 1) != Some(&':') {
                        Some(Class::Property)
                    } else {
                        None
                    };
                    if let Some(class) = class {
                        self.mark(i, to, class);
                    }
                    self.dot = false;
                    i = to;
                }
                _ => {
                    let rest: String = self.chars[i..self.chars.len().min(i + 2)].iter().collect();
                    match OPERATORS.iter().find(|op| rest.starts_with(**op)) {
                        Some(op) => {
                            self.mark(i, i + op.len(), Class::Operator);
                            self.dot = false;
                            i += op.len();
                        }
                        None => {
                            self.dot = c == '.';
                            i += 1;
                        }
                    }
                }
            }
        }
        i
    }

    /// A string from its opening quote, with its escapes and `{…}` parts: where it ends (the
    /// line's end, if it isn't closed yet).
    fn string(&mut self, open: usize) -> usize {
        let mut from = open;
        let mut i = open + 1;
        while let Some(&c) = self.chars.get(i) {
            match c {
                '"' => {
                    self.mark(from, i + 1, Class::String);
                    return i + 1;
                }
                '\\' => {
                    self.mark(from, i, Class::String);
                    let to = (i + 2).min(self.chars.len());
                    self.mark(i, to, Class::Escape);
                    i = to;
                    from = i;
                }
                '{' => {
                    self.mark(from, i, Class::String);
                    self.mark(i, i + 1, Class::Interpolation);
                    self.dot = false;
                    i = self.code(i + 1, true);
                    if self.chars.get(i) == Some(&'}') {
                        self.mark(i, i + 1, Class::Interpolation);
                        i += 1;
                    }
                    from = i;
                }
                _ => i += 1,
            }
        }
        self.mark(from, i, Class::String);
        i
    }
}

/// The text's colours, line by line, as the language reads its words. Unlike the lexer it never
/// stops: a string not yet closed, or a line half typed, is coloured as far as it goes.
pub fn highlight(source: &str) -> Vec<Highlight> {
    let mut out = vec![];
    for (l, line) in source.split('\n').enumerate() {
        let mut c = Colourer { chars: line.strip_suffix('\r').unwrap_or(line).chars().collect(), line: l as u32 + 1, out: vec![], dot: false };
        c.line();
        out.append(&mut c.out);
    }
    out
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

    /// Each coloured stretch of `text` as `(its text, its class)`.
    fn coloured(text: &str) -> Vec<(String, Class)> {
        let lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        highlight(text)
            .into_iter()
            .map(|h| {
                assert_eq!(h.span.start.line, h.span.end.line, "within a line");
                let line = &lines[h.span.start.line as usize - 1];
                (line[h.span.start.col as usize - 1..h.span.end.col as usize - 1].iter().collect(), h.class)
            })
            .collect()
    }

    #[test]
    fn a_text_is_coloured_as_the_language_reads_it() {
        use Class::*;
        let has = |text: &str, word: &str, class: Class| {
            let c = coloured(text);
            assert!(c.contains(&(word.to_string(), class)), "{word} as {class:?} in {c:?}");
        };
        let text = "# hi\nflow hello-api\nport 8200\ncollection notes\n\nroute GET /hello/:name (req) {\n  greeting = \"Hi, {req.params.name}\\n\"\n  notes[req.params.name] = now\n  respond lowercase(greeting), status: 201\n}\n";
        has(text, "# hi", Comment);
        has(text, "flow", Keyword);
        has(text, "hello-api", Definition);
        has(text, "port", Keyword);
        has(text, "8200", Number);
        has(text, "notes", Definition);
        has(text, "GET", Method);
        has(text, "/hello/:name", Path);
        has(text, "req", Parameter);
        has(text, "greeting", Definition);
        has(text, "\"Hi, ", String);
        has(text, "{", Interpolation);
        has(text, "params", Member);
        has(text, "\\n", Escape);
        has(text, "now", Noun);
        has(text, "respond", Keyword);
        has(text, "lowercase", Function);
        has(text, "status", Property);
        has(text, "=", Operator);
        // Half typed: a string not closed is a string to the line's end, and what follows is read.
        let c = coloured("x = \"not closed\ny = lowercase(x)\n");
        assert!(c.contains(&("\"not closed".to_string(), String)), "{c:?}");
        assert!(c.contains(&("lowercase".to_string(), Function)), "{c:?}");
        // Every sample: in order, never overlapping.
        for s in samples::samples() {
            let h = highlight(s.text);
            for w in h.windows(2) {
                assert!((w[0].span.end.line, w[0].span.end.col) <= (w[1].span.start.line, w[1].span.start.col), "{}: {:?}", s.name, w);
            }
        }
    }

    /// The labels offered with the cursor where `|` is in `text` (the `|` taken out).
    fn offered(text: &str) -> Vec<String> {
        let (l, line) = text.lines().enumerate().find(|(_, l)| l.contains('|')).unwrap();
        let at = Pos { line: l as u32 + 1, col: line[..line.find('|').unwrap()].chars().count() as u32 + 1 };
        complete(&text.replacen('|', "", 1), at).into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn completion_offers_what_may_be_written_there() {
        let body = |line: &str| SLUGS.replacen("  respond posts.list()", line, 1);
        // A leaf, by the letters typed; with its signature.
        assert_eq!(offered(&body("  respond lowe|")), ["lowercase"]);
        let typed = body("  respond lowe");
        let lower = complete(&typed, Pos { line: typed.lines().position(|l| l.ends_with("lowe")).unwrap() as u32 + 1, col: 15 });
        assert_eq!((lower[0].kind, lower[0].detail.as_str()), (Kind::Function, "lowercase(s: Text) -> out: Text"));
        // This text's own words: a holon, a binding, a collection, the request.
        let words = offered(&body("  respond |"));
        for w in ["slugify", "publish", "title", "published", "posts", "req", "secret", "state", "now", "http", "concat", "json", "parse_json", "respond", "if"] {
            assert!(words.contains(&w.to_string()), "{w}: {words:?}");
        }
        for w in ["http_request", "collection_put", "json_stringify", "const"] {
            assert!(!words.contains(&w.to_string()), "{w} is written another way");
        }
        // After a dot: the members of what is before it.
        assert_eq!(offered(&body("  respond posts.|")), ["get", "put", "delete", "list"]);
        assert_eq!(offered(&body("  respond posts.l|")), ["list"]);
        assert_eq!(offered(&body("  respond req.|")), ["params", "query", "headers", "body", "json"]);
        assert_eq!(offered(&body("  respond secret.|")), ["receipt_key"]);
        assert_eq!(offered(&body("  respond state.|")), ["set"]);
        assert_eq!(offered(&body("  respond http.p|")), ["post", "put", "patch"]);
        assert_eq!(offered(&body("  respond published.|")), Vec::<String>::new(), "a value's fields aren't known");
        // Nothing inside a string or a comment.
        assert_eq!(offered(&body("  respond \"lo|")), Vec::<String>::new());
        assert_eq!(offered(&body("  respond 1 # lo|")), Vec::<String>::new());
        // A text cut off mid-statement still offers.
        assert!(offered("flow f\nport 8200\ncollection notes\n\nroute GET /n (req) {\n  respond no|").contains(&"notes".to_string()));
    }

    #[test]
    fn format_is_fmt() {
        assert_eq!(format(SLUGS).unwrap(), SLUGS);
        assert_eq!(format("flow f\nport 8200\nroute GET /a (req) {\n    respond   204\n}").unwrap(), "flow f\nport 8200\n\nroute GET /a (req) {\n  respond 204\n}\n");
        assert!(format("flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n").unwrap_err().contains("5:11: unknown name"));
    }
}
