//! A flow written in TypeScript's syntax, `NAME.holon.ts` (docs/L4s/typescript-syntax-L4.md): read
//! with a TypeScript parser, oxc's, into the same syntax tree as a `.holon` text ([`super::ast`]),
//! so the lowering after it is the one both forms share and a flow written either way is the same
//! unit. This is the note's first slice: imports, the kind statement, the four handlers and their
//! options, `const`, writes, `return`, `status` and `respond`, functions, in-place arrows, `js()`,
//! the library's methods and types as port types. What a later slice brings is refused, saying so.
//!
//! Each import names one thing the flow uses (§3): a secret, the state, a collection, a folder, a
//! database, a channel, an outbound host, the clock, randomness or another flow's action; from
//! `"holon"`, the vocabulary and the leaves. A name reads as what it was imported as, so the text
//! never declares what it uses twice, and the headers the lowering reads are made from the imports.

use std::collections::{BTreeSet, HashMap};

use holon_core::{interface_of, TRIGGER, VARIADIC};
use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

use super::ast::*;
use super::lower::{alias, is_optional, lower, LowerError, Lowered, RESERVED};
use crate::templates;

type R<T> = Result<T, LowerError>;

/// Read and lower a `.holon.ts` text.
pub fn compile(src: &str) -> R<Lowered> {
    lower(&parse(src)?)
}

/// Read a `.holon.ts` text into the syntax tree a `.holon` text parses to.
pub fn parse(src: &str) -> R<File> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    let lines = Lines::new(src);
    if let Some(e) = parsed.diagnostics.errors().next() {
        let at = e.labels.first().map(|l| l.offset()).unwrap_or(0);
        let pos = lines.pos(at);
        return Err(LowerError { span: Span { start: pos, end: pos }, message: e.message.to_string(), declare: None });
    }
    Reader::new(src, lines).program(&parsed.program)
}

/// Each line's first byte, to turn oxc's byte offsets into the line and column (in characters) a
/// span carries.
struct Lines<'s> {
    src: &'s str,
    starts: Vec<usize>,
}

impl<'s> Lines<'s> {
    fn new(src: &'s str) -> Lines<'s> {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        Lines { src, starts }
    }

    fn pos(&self, at: u32) -> Pos {
        let at = (at as usize).min(self.src.len());
        let line = self.starts.partition_point(|&s| s <= at) - 1;
        let col = self.src[self.starts[line]..at].chars().count();
        Pos { line: line as u32 + 1, col: col as u32 + 1 }
    }
}

fn fail<T>(span: Span, message: impl Into<String>) -> R<T> {
    Err(LowerError { span, message: message.into(), declare: None })
}

/// What a name imported from `"holon"` is.
#[derive(Clone, Debug, PartialEq)]
enum Vocab {
    Flow,
    Route,
    Every,
    On,
    Action,
    Js,
    Respond,
    Status,
    Stripe,
    /// A leaf (or template) called by its name: `sha256(text)`.
    Leaf(String),
}

/// What an imported name stands for in the flow.
#[derive(Clone, Debug, PartialEq)]
enum Noun {
    Secret(String),
    State,
    /// A collection, folder, database or channel, by its declared name, and which.
    Declared(&'static str, String),
    /// An outbound host: its requests, `gemini.post(path, …)`.
    Host(String),
    Now,
    Random,
    /// Another flow's action: `flow:billing`'s `charge`.
    Action { flow: String, action: String },
    Vocab(Vocab),
}

/// What a name in a handler or function reads as.
#[derive(Clone, Debug)]
enum Local {
    /// A binding or a parameter, by the name the lowering knows it by.
    Bound(String),
    /// A name that stands for an expression: a part of a destructured request, a clock's tick.
    Alias(Expr),
}

/// Which handler (or function) a block of statements is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Block {
    Route,
    Action,
    Every,
    File,
    Function,
}

/// The function `const`s and declarations at the top level, by name: graph holons and js ones.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Def {
    Holon,
    Js,
}

struct Reader<'s, 't> {
    src: &'s str,
    lines: Lines<'s>,
    imports: HashMap<String, Noun>,
    headers: Vec<Header>,
    /// The state keys used, in order, each declared once.
    state_keys: BTreeSet<String>,
    defs: HashMap<String, Def>,
    consts: HashMap<String, String>,
    /// The top level's `type` aliases, each read as the type it names where it is used, and its
    /// interfaces, each a record.
    aliases: HashMap<String, &'t js::TSType<'t>>,
    interfaces: BTreeSet<String>,
    scopes: Vec<HashMap<String, Local>>,
    /// The hidden bindings made so far, for destructuring a call's result once.
    hidden: usize,
    kind_seen: bool,
}

impl<'s, 't> Reader<'s, 't> {
    fn new(src: &'s str, lines: Lines<'s>) -> Reader<'s, 't> {
        Reader {
            src,
            lines,
            imports: HashMap::new(),
            headers: Vec::new(),
            state_keys: BTreeSet::new(),
            defs: HashMap::new(),
            consts: HashMap::new(),
            aliases: HashMap::new(),
            interfaces: BTreeSet::new(),
            scopes: Vec::new(),
            hidden: 0,
            kind_seen: false,
        }
    }

    fn span(&self, s: oxc_span::Span) -> Span {
        Span { start: self.lines.pos(s.start), end: self.lines.pos(s.end) }
    }

    fn text(&self, s: oxc_span::Span) -> &'s str {
        &self.src[s.start as usize..s.end as usize]
    }

    fn name(&self, text: &str, s: oxc_span::Span) -> Name {
        Name { text: text.to_string(), span: self.span(s) }
    }

    fn header(&mut self, kind: HeaderKind, span: Span) {
        if !self.headers.iter().any(|h| h.kind == kind) {
            self.headers.push(Header { kind, span });
        }
    }

    // ── the file ─────────────────────────────────────────────────────────────────────────────

    fn program(mut self, p: &'t js::Program<'t>) -> R<File> {
        // Imports first, wherever they are written, as ES hoists them; then the definitions,
        // which a handler may use before they appear; then the rest in order.
        for s in &p.body {
            if let js::Statement::ImportDeclaration(i) = s {
                self.import(i)?;
            }
        }
        for s in &p.body {
            self.declare(s)?;
        }
        let mut items = Vec::new();
        for s in &p.body {
            if let Some(item) = self.item(s)? {
                items.push(item);
            }
        }
        if !self.kind_seen {
            return fail(Span::default(), "a flow says what it is first: flow(\"name\", { port: 8080 }), after its imports");
        }
        let mut headers = self.headers;
        let at = headers.first().map(|h| h.span).unwrap_or_default();
        headers.extend(self.state_keys.iter().map(|k| Header { kind: HeaderKind::State(k.clone()), span: at }));
        Ok(File { headers, items })
    }

    fn import(&mut self, i: &js::ImportDeclaration) -> R<()> {
        let source = i.source.value.as_str();
        let at = self.span(i.span);
        if i.import_kind.is_type() {
            return Ok(());
        }
        let optional = match &i.with_clause {
            None => false,
            Some(w) => {
                let mut optional = false;
                for a in &w.with_entries {
                    let key = match &a.key {
                        js::ImportAttributeKey::Identifier(k) => k.name.as_str(),
                        js::ImportAttributeKey::StringLiteral(k) => k.value.as_str(),
                    };
                    match (key, a.value.value.as_str()) {
                        ("optional", "true") => optional = true,
                        ("optional", "false") => {}
                        _ => return fail(self.span(a.span), "an import takes one attribute, with { optional: \"true\" }, for what the flow can run without"),
                    }
                }
                optional
            }
        };
        let specifiers = i.specifiers.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
        let default = || specifiers.iter().find_map(|s| if let js::ImportDeclarationSpecifier::ImportDefaultSpecifier(d) = s { Some(d) } else { None });
        let named = || specifiers.iter().filter_map(|s| if let js::ImportDeclarationSpecifier::ImportSpecifier(n) = s { Some(n) } else { None });
        let imported = |n: &js::ImportSpecifier| match &n.imported {
            js::ModuleExportName::IdentifierName(i) => i.name.to_string(),
            js::ModuleExportName::IdentifierReference(i) => i.name.to_string(),
            js::ModuleExportName::StringLiteral(s) => s.value.to_string(),
        };
        if specifiers.iter().any(|s| matches!(s, js::ImportDeclarationSpecifier::ImportNamespaceSpecifier(_))) {
            return fail(at, "import the names one by one: import { route } from \"holon\"");
        }
        let one = |what: &str, example: &str| -> R<String> {
            match (default(), specifiers.len()) {
                (Some(d), 1) => Ok(d.local.name.to_string()),
                _ => fail(at, format!("{what} is imported by a name of its own: {example}")),
            }
        };
        let (kind, rest) = source.split_once(':').unwrap_or((source, ""));
        if optional && kind != "secret" {
            return fail(at, "only a secret is imported as optional, for now");
        }
        match (kind, rest) {
            ("holon", "") => {
                for n in named() {
                    let what = imported(n);
                    let vocab = match what.as_str() {
                        "flow" => Vocab::Flow,
                        "route" => Vocab::Route,
                        "every" => Vocab::Every,
                        "on" => Vocab::On,
                        "action" => Vocab::Action,
                        "js" => Vocab::Js,
                        "respond" => Vocab::Respond,
                        "status" => Vocab::Status,
                        "stripe" => Vocab::Stripe,
                        "tour" | "component" => return fail(self.span(n.span), format!("{what} comes with tours and components, not yet")),
                        leaf if interface_of(alias(leaf)).is_some() || templates::is_template(leaf) => Vocab::Leaf(leaf.to_string()),
                        other => return fail(self.span(n.span), format!("\"holon\" has no \"{other}\": its vocabulary (flow, route, every, on, action, js, respond, status, stripe) and its leaves")),
                    };
                    self.bind_import(&n.local, Noun::Vocab(vocab))?;
                }
                if default().is_some() {
                    return fail(at, "import what you use from \"holon\" by name: import { flow, route } from \"holon\"");
                }
            }
            ("secret", name) if is_word(name) => {
                let local = one("a secret", "import key from \"secret:api_key\"")?;
                self.header(if optional { HeaderKind::Optional(Consent::Secret(name.into())) } else { HeaderKind::Secret(name.into()) }, at);
                self.imports.insert(local, Noun::Secret(name.into()));
            }
            ("state", "") => {
                let local = one("the state", "import state from \"state\"")?;
                self.imports.insert(local, Noun::State);
            }
            (k @ ("collection" | "folder" | "database" | "channel"), name) if is_word(name) => {
                let local = one(&format!("a {k}"), &format!("import {name} from \"{k}:{name}\""))?;
                let (word, header): (&'static str, HeaderKind) = match k {
                    "collection" => ("collection", HeaderKind::Collection(name.into())),
                    "folder" => ("folder", HeaderKind::Folder(name.into())),
                    "database" => ("database", HeaderKind::Database(name.into())),
                    _ => ("channel", HeaderKind::Channel(name.into())),
                };
                if local != name {
                    return fail(at, format!("a {k} is imported under its own name, for now: import {name} from \"{k}:{name}\""));
                }
                self.header(header, at);
                self.imports.insert(local, Noun::Declared(word, name.into()));
            }
            ("host", host) if !host.is_empty() => {
                let local = one("a host", "import api from \"host:api.example.com\"")?;
                self.imports.insert(local, Noun::Host(host.into()));
            }
            ("holon", "clock") | ("holon", "random") => {
                for n in named() {
                    let noun = match (rest, imported(n).as_str()) {
                        ("clock", "now") => Noun::Now,
                        ("random", "random") => Noun::Random,
                        (_, other) => return fail(self.span(n.span), format!("\"holon:{rest}\" has no \"{other}\": import {{ {} }} from \"holon:{rest}\"", if rest == "clock" { "now" } else { "random" })),
                    };
                    self.bind_import(&n.local, noun)?;
                }
            }
            ("flow", slug) if !slug.is_empty() => {
                self.header(HeaderKind::Calls(slug.into()), at);
                for n in named() {
                    self.bind_import(&n.local, Noun::Action { flow: slug.into(), action: imported(n) })?;
                }
            }
            _ => {
                return fail(
                    self.span(i.source.span),
                    format!("\"{source}\" isn't something a flow imports: \"holon\", \"secret:NAME\", \"state\", \"collection:NAME\", \"folder:NAME\", \"database:NAME\", \"channel:NAME\", \"host:HOST\", \"holon:clock\", \"holon:random\" or \"flow:SLUG\""),
                );
            }
        }
        Ok(())
    }

    fn bind_import(&mut self, local: &js::BindingIdentifier, noun: Noun) -> R<()> {
        if self.imports.insert(local.name.to_string(), noun).is_some() {
            return fail(self.span(local.span), format!("\"{}\" is imported twice", local.name));
        }
        Ok(())
    }

    /// The names the top level defines, before any is read: functions, js holons, literal consts
    /// and types.
    fn declare(&mut self, s: &'t js::Statement<'t>) -> R<()> {
        match s {
            js::Statement::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    self.define(id.name.as_str(), Def::Holon, id.span)?;
                }
            }
            js::Statement::VariableDeclaration(v) => {
                for d in &v.declarations {
                    let (js::BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) else { continue };
                    match self.unwrap(init) {
                        js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_) => self.define(id.name.as_str(), Def::Holon, id.span)?,
                        js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Js) => self.define(id.name.as_str(), Def::Js, id.span)?,
                        _ => {}
                    }
                }
            }
            js::Statement::TSTypeAliasDeclaration(t) => {
                if t.type_parameters.is_some() {
                    return fail(self.span(t.span), "a type parameter is refused for now (the note's fork 19): name the type it stands for");
                }
                if self.aliases.insert(t.id.name.to_string(), &t.type_annotation).is_some() || self.interfaces.contains(t.id.name.as_str()) {
                    return fail(self.span(t.id.span), format!("the type {} is declared twice", t.id.name));
                }
            }
            js::Statement::TSInterfaceDeclaration(t) => {
                let fresh = self.interfaces.insert(t.id.name.to_string()) && !self.aliases.contains_key(t.id.name.as_str());
                if !fresh {
                    return fail(self.span(t.id.span), format!("the type {} is declared twice", t.id.name));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn define(&mut self, name: &str, def: Def, s: oxc_span::Span) -> R<()> {
        if self.imports.contains_key(name) || self.defs.insert(name.to_string(), def).is_some() {
            return fail(self.span(s), format!("\"{name}\" is defined twice"));
        }
        Ok(())
    }

    fn item(&mut self, s: &js::Statement) -> R<Option<Item>> {
        let at = self.span(s.span());
        match s {
            js::Statement::ImportDeclaration(_) | js::Statement::TSTypeAliasDeclaration(_) | js::Statement::TSInterfaceDeclaration(_) | js::Statement::EmptyStatement(_) => Ok(None),
            js::Statement::ExpressionStatement(e) => self.top_call(&e.expression),
            js::Statement::FunctionDeclaration(f) => {
                let id = f.id.as_ref().expect("a declared function has a name");
                Ok(Some(Item::Holon(self.function(id.name.as_str(), id.span, f)?)))
            }
            js::Statement::VariableDeclaration(v) => {
                if v.kind != js::VariableDeclarationKind::Const || v.declarations.len() != 1 {
                    return fail(at, "the top level holds one const per line: const limit = 50");
                }
                let d = &v.declarations[0];
                let js::BindingPattern::BindingIdentifier(id) = &d.id else {
                    return fail(at, "the top level binds a name: const limit = 50");
                };
                let Some(init) = &d.init else { return fail(at, "a const needs a value") };
                let name = id.name.as_str();
                match self.unwrap(init) {
                    js::Expression::ArrowFunctionExpression(a) => Ok(Some(Item::Holon(self.arrow_holon(name, id.span, a)?))),
                    js::Expression::FunctionExpression(f) => Ok(Some(Item::Holon(self.function(name, id.span, f)?))),
                    js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Js) => Ok(Some(Item::Js(self.js_holon(name, id.span, c)?))),
                    other => {
                        let value = self.literal(other).ok_or(()).or_else(|_| fail(self.span(other.span()), "the top level binds only literals (const limit = 50): put the rest in a handler, or in a function"))?;
                        let text = self.text(other.span()).to_string();
                        self.consts.insert(name.to_string(), text);
                        let stmt = Stmt { kind: StmtKind::Bind { name: self.name(name, id.span), value }, span: at, after: None };
                        Ok(Some(Item::Stmt(stmt)))
                    }
                }
            }
            js::Statement::ExportNamedDeclaration(_) | js::Statement::ExportDeclaration(_) => self.export(s).map(Some),
            _ => fail(at, "the top level holds imports, the flow(…) line, handlers, functions and consts of literals: move this into a handler"),
        }
    }

    /// `export const approve = action((input) => …, { log })`.
    fn export(&mut self, s: &js::Statement) -> R<Item> {
        let at = self.span(s.span());
        let js::Statement::ExportDeclaration(e) = s else {
            return fail(at, "an export is an action: export const approve = action((input) => …)");
        };
        let js::Declaration::VariableDeclaration(v) = &e.declaration else {
            return fail(at, "an action is a const: export const approve = action((input) => …), not an exported function");
        };
        let [d] = v.declarations.as_slice() else { return fail(at, "one action per export") };
        let (js::BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) else {
            return fail(at, "an action is named by its const: export const approve = action(…)");
        };
        let js::Expression::CallExpression(c) = self.unwrap(init) else {
            return fail(at, "only actions are exported: export const approve = action((input) => …)");
        };
        if self.vocab_of(&c.callee) != Some(Vocab::Action) || v.kind != js::VariableDeclarationKind::Const {
            return fail(at, "only actions are exported: export const approve = action((input) => …)");
        }
        let (handler, options) = self.handler_args(c, 0, "action((input) => …, { log })")?;
        let log = self.options(options, &["log"])?.remove("log").map(|(v, s)| self.name(&v, s));
        let input = self.handler_param(handler, Block::Action, "input")?;
        let body = self.handler_body(handler, Block::Action, &input)?;
        let head = self.span(c.span);
        Ok(Item::Action(ActionDef { name: self.name(id.name.as_str(), id.span), input: self.name(&input, handler.span()), head, log, body, span: at }))
    }

    /// A call at the top level: the kind statement, or a handler.
    fn top_call(&mut self, e: &js::Expression) -> R<Option<Item>> {
        let at = self.span(e.span());
        let js::Expression::CallExpression(c) = self.unwrap(e) else {
            return fail(at, "the top level holds imports, the flow(…) line, handlers, functions and consts of literals: move this into a handler");
        };
        match &c.callee {
            js::Expression::Identifier(i) if self.vocab(i.name.as_str()) == Some(Vocab::Flow) => {
                // The kind statement is headers, not an item.
                self.kind(c)?;
                Ok(None)
            }
            js::Expression::Identifier(i) if self.vocab(i.name.as_str()) == Some(Vocab::Every) => self.every(c).map(|e| Some(Item::Every(e))),
            js::Expression::StaticMemberExpression(m) => match (&m.object, m.property.name.as_str()) {
                (js::Expression::Identifier(r), method) if self.vocab(r.name.as_str()) == Some(Vocab::Route) => self.route(c, method, m.property.span).map(|r| Some(Item::Route(r))),
                (js::Expression::StaticMemberExpression(f), on) if f.property.name == "file" && matches!(&f.object, js::Expression::Identifier(o) if self.vocab(o.name.as_str()) == Some(Vocab::On)) => {
                    self.on_file(c, on, m.property.span).map(|o| Some(Item::OnFile(o)))
                }
                _ => fail(at, "a handler is route.get(…), every(…), on.file.created(…) or an exported action(…)"),
            },
            js::Expression::Identifier(i) if self.vocab(i.name.as_str()) == Some(Vocab::Action) => fail(at, "an action is exported, as nothing else could reach it: export const approve = action(…)"),
            _ => fail(at, "the top level holds imports, the flow(…) line, handlers, functions and consts of literals: move this into a handler"),
        }
    }

    /// `flow("chess", { port, keep, browsers, limits })`: the flow's name and deploy settings.
    fn kind(&mut self, c: &js::CallExpression) -> R<()> {
        let at = self.span(c.span);
        if std::mem::replace(&mut self.kind_seen, true) {
            return fail(at, "flow(…) is written once");
        }
        let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
        if args.len() != c.arguments.len() || args.is_empty() || args.len() > 2 {
            return fail(at, "flow takes its name, then its settings: flow(\"name\", { port: 8080 })");
        }
        let js::Expression::StringLiteral(name) = self.unwrap(args[0]) else {
            return fail(self.span(args[0].span()), "the flow's name is written in quotes: flow(\"name\")");
        };
        self.header(HeaderKind::Flow(name.value.to_string()), at);
        let Some(settings) = args.get(1) else { return Ok(()) };
        let js::Expression::ObjectExpression(o) = self.unwrap(settings) else {
            return fail(self.span(settings.span()), "a flow's settings are an object: { port: 8080 }");
        };
        for (key, value, span) in self.fields(o)? {
            match key.as_str() {
                "port" => match self.unwrap(value) {
                    js::Expression::NumericLiteral(n) => self.header(HeaderKind::Port(number_text(n)), span),
                    _ => return fail(self.span(value.span()), "port is a number: port: 8080"),
                },
                "browsers" => {
                    let origins = match self.unwrap(value) {
                        js::Expression::StringLiteral(s) => vec![s.value.to_string()],
                        js::Expression::ArrayExpression(a) => {
                            let mut origins = Vec::new();
                            for item in &a.elements {
                                match item.as_expression().map(|e| self.unwrap(e)) {
                                    Some(js::Expression::StringLiteral(s)) => origins.push(s.value.to_string()),
                                    _ => return fail(self.span(item.span()), "an origin is written in quotes: \"https://app.example\""),
                                }
                            }
                            origins
                        }
                        _ => return fail(self.span(value.span()), "browsers are origins in quotes: browsers: [\"https://app.example\"], or \"*\""),
                    };
                    self.header(HeaderKind::Browsers(origins), span);
                }
                "keep" => {
                    let js::Expression::ObjectExpression(k) = self.unwrap(value) else {
                        return fail(self.span(value.span()), "keep says how many runs and for how many days: keep: { runs: 500, days: 30 }");
                    };
                    let (mut runs, mut days) = (None, None);
                    for (key, value, _) in self.fields(k)? {
                        let n = match self.unwrap(value) {
                            js::Expression::NumericLiteral(n) if n.value >= 1.0 && n.value.fract() == 0.0 => n.value as u64,
                            _ => return fail(self.span(value.span()), format!("keep's {key} is a whole number, 1 or more")),
                        };
                        match key.as_str() {
                            "runs" => runs = Some(n),
                            "days" => days = Some(n),
                            other => return fail(self.span(value.span()), format!("keep has runs and days, not \"{other}\"")),
                        }
                    }
                    if runs.is_none() && days.is_none() {
                        return fail(self.span(value.span()), "keep says how many runs, or for how many days: keep: { runs: 500 }");
                    }
                    self.header(HeaderKind::Keep { runs, days }, span);
                }
                "limits" => {
                    let js::Expression::ObjectExpression(l) = self.unwrap(value) else {
                        return fail(self.span(value.span()), "limits are an object: limits: { run_budget_ms: \"1m\" }");
                    };
                    for (name, value, span) in self.fields(l)? {
                        let limit = self.limit(&name, value)?;
                        self.header(limit, span);
                    }
                }
                other => return fail(span, format!("a flow's settings are port, keep, browsers and limits, not \"{other}\"")),
            }
        }
        Ok(())
    }

    /// One limit of the deploy settings, as the `limit` line reads it: a whole number in its unit,
    /// or a duration in quotes for one counted in milliseconds or seconds.
    fn limit(&self, name: &str, value: &js::Expression) -> R<HeaderKind> {
        let at = self.span(value.span());
        let known = || crate::LIMIT_NAMES.iter().filter(|n| !n.starts_with("keep_") && !n.starts_with("module_")).copied().collect::<Vec<_>>().join(", ");
        if name.starts_with("keep_") {
            return fail(at, format!("{name} is said by keep: keep: {{ runs: 500, days: 30 }}"));
        }
        if !crate::LIMIT_NAMES.contains(&name) || name.starts_with("module_") {
            return fail(at, format!("{name} isn't a limit a flow can set (a js() function says its fuel in its own options); the limits: {}", known()));
        }
        let value = match self.unwrap(value) {
            js::Expression::NumericLiteral(n) => Some(n.value).filter(|v| v.fract() == 0.0 && *v >= 0.0).map(|v| v as u64),
            js::Expression::StringLiteral(s) => match duration(s.value.as_str()) {
                Some(seconds) if name.ends_with("_ms") => Some(seconds * 1000.0).filter(|ms| ms.fract() == 0.0).map(|ms| ms as u64),
                Some(seconds) if name.ends_with("_seconds") => Some(seconds).filter(|s| s.fract() == 0.0).map(|s| s as u64),
                Some(_) => return fail(at, format!("{name} isn't a duration: give it a whole number")),
                None => return fail(at, format!("{:?} isn't a duration: \"60s\", \"5m\", \"2h\", \"1d\"", s.value.as_str())),
            },
            _ => return fail(at, "a limit is a whole number, or a duration in quotes: \"60s\""),
        };
        let Some(value) = value else { return fail(at, format!("{name} takes a whole number in its unit")) };
        Ok(HeaderKind::Limit { name: name.to_string(), value })
    }

    // ── handlers ─────────────────────────────────────────────────────────────────────────────

    /// A handler's function and its options object, after `skip` arguments.
    fn handler_args<'a>(&self, c: &'a js::CallExpression<'a>, skip: usize, shape: &str) -> R<(&'a js::Expression<'a>, Option<&'a js::Expression<'a>>)> {
        let at = self.span(c.span);
        let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
        if args.len() != c.arguments.len() || args.len() < skip + 1 || args.len() > skip + 2 {
            return fail(at, format!("a handler is a call: {shape}"));
        }
        let handler = args[skip];
        if !matches!(self.unwrap(handler), js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_)) {
            return fail(self.span(handler.span()), format!("a handler takes a function: {shape}"));
        }
        Ok((handler, args.get(skip + 1).copied()))
    }

    /// A handler's options, each a literal, by name: `{ resumes: "paid", log: "lazy" }`.
    fn options(&self, options: Option<&js::Expression>, allowed: &[&str]) -> R<HashMap<String, (String, oxc_span::Span)>> {
        let mut out = HashMap::new();
        let Some(o) = options else { return Ok(out) };
        let js::Expression::ObjectExpression(o) = self.unwrap(o) else {
            return fail(self.span(o.span()), format!("a handler's options are an object: {{ {}: … }}", allowed.join(": …, ")));
        };
        for (key, value, span) in self.fields(o)? {
            if !allowed.contains(&key.as_str()) {
                return fail(span, format!("this handler's options are {}, not \"{key}\"", allowed.join(" and ")));
            }
            let js::Expression::StringLiteral(s) = self.unwrap(value) else {
                return fail(self.span(value.span()), format!("{key} is written in quotes"));
            };
            if key == "log" && !matches!(s.value.as_str(), "strict" | "lazy" | "unrecorded") {
                return fail(self.span(value.span()), "log is \"strict\", \"lazy\" or \"unrecorded\"");
            }
            if key == "log" && s.value == "strict" {
                // Strict is the default, and never written into the unit.
                continue;
            }
            out.insert(key, (s.value.to_string(), s.span));
        }
        Ok(out)
    }

    /// `route.post("/pay/:t", (req) => …, { resumes: "paid", log: "lazy" })`.
    fn route(&mut self, c: &js::CallExpression, method: &str, method_span: oxc_span::Span) -> R<RouteDef> {
        let m = method.to_ascii_uppercase();
        if !declares_method(&m) {
            return fail(self.span(method_span), format!("route has no \"{method}\": get, post, put, patch or delete"));
        }
        let shape = format!("route.{method}(\"/path\", (req) => …)");
        let Some(js::Expression::StringLiteral(path)) = c.arguments.first().and_then(|a| a.as_expression()).map(|e| self.unwrap(e)) else {
            return fail(self.span(c.span), format!("a route's path is written in quotes: {shape}"));
        };
        let (handler, options) = self.handler_args(c, 1, &shape)?;
        let mut options = self.options(options, &["resumes", "log"])?;
        let resumes = options.remove("resumes").map(|(v, s)| self.name(&v, s));
        let log = options.remove("log").map(|(v, s)| self.name(&v, s));
        let request = self.handler_param(handler, Block::Route, "req")?;
        let body = self.handler_body(handler, Block::Route, &request)?;
        Ok(RouteDef {
            method: self.name(&m, method_span),
            path: self.name(path.value.as_str(), path.span),
            request: self.name(&request, handler.span()),
            resumes,
            log,
            body,
            span: self.span(c.span),
        })
    }

    /// `every("1h", (now) => …, { log })`.
    fn every(&mut self, c: &js::CallExpression) -> R<EveryDef> {
        let shape = "every(\"1h\", () => …)";
        let Some(js::Expression::StringLiteral(period)) = c.arguments.first().and_then(|a| a.as_expression()).map(|e| self.unwrap(e)) else {
            return fail(self.span(c.span), format!("every takes its period in quotes: {shape}"));
        };
        let Some(seconds) = duration(period.value.as_str()) else {
            return fail(self.span(period.span), format!("{:?} isn't a period: \"10s\", \"5m\", \"1h\", \"1d\"", period.value.as_str()));
        };
        let (handler, options) = self.handler_args(c, 1, shape)?;
        let log = self.options(options, &["log"])?.remove("log").map(|(v, s)| self.name(&v, s));
        let tick = self.handler_param(handler, Block::Every, "")?;
        let body = self.handler_body(handler, Block::Every, &tick)?;
        let head = self.span(oxc_span::Span::new(c.span.start, period.span.end));
        Ok(EveryDef { seconds, text: period.value.to_string(), head, log, body, span: self.span(c.span) })
    }

    /// `on.file.created(inbox, (f) => …, { matching: "*.csv", log })`, and `deleted`.
    fn on_file(&mut self, c: &js::CallExpression, on: &str, on_span: oxc_span::Span) -> R<OnFileDef> {
        let deleted = match on {
            "created" => false,
            "deleted" => true,
            other => return fail(self.span(on_span), format!("on.file has no \"{other}\": created (a new or changed file) or deleted")),
        };
        let shape = format!("on.file.{on}(folder, (f) => …)");
        let folder = match c.arguments.first().and_then(|a| a.as_expression()).map(|e| self.unwrap(e)) {
            Some(js::Expression::Identifier(i)) => match self.imports.get(i.name.as_str()) {
                Some(Noun::Declared("folder", name)) => name.clone(),
                _ => return fail(self.span(i.span), format!("\"{}\" isn't an imported folder: import {} from \"folder:{}\"", i.name, i.name, i.name)),
            },
            _ => return fail(self.span(c.span), format!("a file handler takes its folder first: {shape}")),
        };
        let (handler, options) = self.handler_args(c, 1, &shape)?;
        let mut options = self.options(options, &["matching", "log"])?;
        let matching = options.remove("matching").map(|(v, _)| v);
        let log = options.remove("log").map(|(v, s)| self.name(&v, s));
        let file = self.handler_param(handler, Block::File, "f")?;
        let body = self.handler_body(handler, Block::File, &file)?;
        Ok(OnFileDef { on: FileOn { folder, deleted, matching }, file: self.name(&file, handler.span()), head: self.span(c.span), log, body, span: self.span(c.span) })
    }

    /// The name a handler's run knows what it received by: its parameter's, `fallback` when it has
    /// none or destructures it. A clock's tick has no name (`now` is it).
    fn handler_param(&mut self, handler: &js::Expression, block: Block, fallback: &str) -> R<String> {
        let params = function_params(self.unwrap(handler));
        if params.items.len() > 1 || params.rest.is_some() {
            return fail(self.span(params.span), "a handler takes one parameter: what it received");
        }
        Ok(match params.items.first().map(|p| &p.pattern) {
            Some(js::BindingPattern::BindingIdentifier(id)) if block != Block::Every => self.local_name(id.name.as_str()),
            _ => fallback.to_string(),
        })
    }

    /// A handler's statements, its parameter in scope as `received`: a route's request, an
    /// action's input, a file's handle, a clock's tick.
    fn handler_body(&mut self, handler: &js::Expression, block: Block, received: &str) -> R<Vec<Stmt>> {
        let f = self.unwrap(handler);
        if let js::Expression::FunctionExpression(f) = f
            && f.generator
        {
            return fail(self.span(f.span), "a handler isn't a generator");
        }
        let params = function_params(f);
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        if let Some(p) = params.items.first() {
            match (&p.pattern, block) {
                (js::BindingPattern::BindingIdentifier(id), Block::Every) => {
                    let now = Expr { kind: ExprKind::Noun(Noun::Now.into()), span: self.span(id.span) };
                    self.bind_local(id.name.as_str(), Local::Alias(now));
                }
                (js::BindingPattern::BindingIdentifier(id), _) => self.bind_local(id.name.as_str(), Local::Bound(received.to_string())),
                (js::BindingPattern::ObjectPattern(o), Block::Route) => {
                    // The request's parts are its outputs: each is the request node itself, so a
                    // name for one is no node, read where it is used.
                    let req = Expr { kind: ExprKind::Name(received.to_string()), span: self.span(o.span) };
                    for prop in &o.properties {
                        let key = self.property_key(&prop.key, prop.computed)?;
                        let part = Expr { kind: ExprKind::Field { base: Box::new(req.clone()), name: self.name(&key, prop.key.span()) }, span: self.span(prop.span) };
                        match &prop.value {
                            js::BindingPattern::BindingIdentifier(id) if REQUEST_PARTS.contains(&key.as_str()) => self.bind_local(id.name.as_str(), Local::Alias(part)),
                            pattern => self.destructure(pattern, part, &mut stmts)?,
                        }
                    }
                    if let Some(rest) = &o.rest {
                        return fail(self.span(rest.span), "a rest pattern comes in a later slice: name what you read");
                    }
                }
                (pattern, Block::Action | Block::File) => {
                    let whole = Expr { kind: ExprKind::Name(received.to_string()), span: self.span(pattern.span()) };
                    self.destructure(pattern, whole, &mut stmts)?;
                }
                (pattern, _) => return fail(self.span(pattern.span()), "a clock's tick is a name: every(\"1h\", (now) => …)"),
            }
        }
        let result = self.function_body(f, block, &mut stmts);
        self.scopes.pop();
        result?;
        Ok(stmts)
    }

    /// The statements of a function or handler body, onto `stmts`: an arrow's expression is its
    /// answer, as `return` is.
    fn function_body(&mut self, f: &js::Expression, block: Block, stmts: &mut Vec<Stmt>) -> R<()> {
        match f {
            js::Expression::ArrowFunctionExpression(a) => match &a.body {
                js::ArrowFunctionBody::FunctionBody(b) => self.statements(&b.statements, block, stmts),
                body => {
                    let e = body.as_expression().expect("an arrow's body is a block or an expression");
                    let s = self.answer_stmt(e, block)?;
                    stmts.push(s);
                    Ok(())
                }
            },
            js::Expression::FunctionExpression(f) => match &f.body {
                Some(b) => self.statements(&b.statements, block, stmts),
                None => fail(self.span(f.span), "a function needs a body"),
            },
            _ => unreachable!("checked to be a function"),
        }
    }

    fn statements(&mut self, body: &[js::Statement], block: Block, stmts: &mut Vec<Stmt>) -> R<()> {
        for (i, s) in body.iter().enumerate() {
            let at = self.span(s.span());
            match s {
                js::Statement::ReturnStatement(r) => {
                    if i + 1 != body.len() {
                        return fail(at, "a return comes last, for now: an early return comes in a later slice");
                    }
                    let Some(e) = &r.argument else {
                        return fail(at, "a bare return answers nothing: leave it out");
                    };
                    let stmt = self.answer_stmt(e, block)?;
                    stmts.push(stmt);
                }
                js::Statement::VariableDeclaration(v) => {
                    if v.kind != js::VariableDeclarationKind::Const {
                        return fail(at, "let and var come in a later slice: use const, one name per value");
                    }
                    for d in &v.declarations {
                        let Some(init) = &d.init else { return fail(self.span(d.span), "a const needs a value") };
                        let value = self.expr(init)?;
                        self.destructure(&d.id, value, stmts)?;
                    }
                }
                js::Statement::ExpressionStatement(e) => {
                    let stmt = self.effect(&e.expression)?;
                    stmts.push(stmt);
                }
                js::Statement::IfStatement(f) => {
                    // `if (c) coll[k] = v`: the write, guarded, as `… when c`.
                    if f.alternate.is_some() {
                        return fail(at, "an if with an else comes in a later slice: write the value with c ? a : b");
                    }
                    let inner = match &f.consequent {
                        js::Statement::ExpressionStatement(e) => &e.expression,
                        js::Statement::BlockStatement(b) if b.body.len() == 1 && matches!(&b.body[0], js::Statement::ExpressionStatement(_)) => {
                            let js::Statement::ExpressionStatement(e) = &b.body[0] else { unreachable!() };
                            &e.expression
                        }
                        _ => return fail(at, "an if around more than one write comes in a later slice: guard each write"),
                    };
                    let cond = self.expr(&f.test)?;
                    let mut stmt = self.effect(inner)?;
                    let guard = |value: &mut Expr| {
                        let span = value.span.to(cond.span);
                        let v = std::mem::replace(value, Expr { kind: ExprKind::Bool(false), span });
                        *value = Expr { kind: ExprKind::Binary { op: BinOp::When, lhs: Box::new(v), rhs: Box::new(cond.clone()) }, span };
                    };
                    match &mut stmt.kind {
                        StmtKind::StateWrite { value, .. } | StmtKind::CollectionWrite { value, .. } | StmtKind::Expr(value) => guard(value),
                        _ => return fail(at, "an if around a delete comes in a later slice"),
                    }
                    stmt.span = at;
                    stmts.push(stmt);
                }
                js::Statement::EmptyStatement(_) => {}
                js::Statement::FunctionDeclaration(_) => return fail(at, "a function is defined at the top level, where every handler can use it"),
                _ => return fail(at, format!("{} isn't something a {} holds yet", statement_kind(s), block_word(block))),
            }
        }
        Ok(())
    }

    /// `const pattern = value`: one binding per name, a `field` for each part read.
    fn destructure(&mut self, pattern: &js::BindingPattern, value: Expr, stmts: &mut Vec<Stmt>) -> R<()> {
        let at = self.span(pattern.span());
        match pattern {
            js::BindingPattern::BindingIdentifier(id) => {
                let name = self.local_name(id.name.as_str());
                self.bind_local(id.name.as_str(), Local::Bound(name.clone()));
                stmts.push(Stmt { kind: StmtKind::Bind { name: self.name(&name, id.span), value }, span: at, after: None });
                Ok(())
            }
            js::BindingPattern::AssignmentPattern(a) => {
                let fallback = self.expr(&a.right)?;
                let span = value.span;
                let value = Expr { kind: ExprKind::Binary { op: BinOp::Coalesce, lhs: Box::new(value), rhs: Box::new(fallback) }, span };
                self.destructure(&a.left, value, stmts)
            }
            js::BindingPattern::ObjectPattern(o) => {
                if let Some(rest) = &o.rest {
                    return fail(self.span(rest.span), "a rest pattern comes in a later slice: name what you read");
                }
                // A value that isn't a name is bound once, so each part reads the one value.
                let base = if matches!(value.kind, ExprKind::Name(_)) {
                    value
                } else {
                    self.hidden += 1;
                    let name = format!("parts_{}", self.hidden);
                    stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span: at }, value }, span: at, after: None });
                    Expr { kind: ExprKind::Name(name), span: at }
                };
                for prop in &o.properties {
                    let key = self.property_key(&prop.key, prop.computed)?;
                    let part = Expr { kind: ExprKind::Field { base: Box::new(base.clone()), name: self.name(&key, prop.key.span()) }, span: self.span(prop.span) };
                    self.destructure(&prop.value, part, stmts)?;
                }
                Ok(())
            }
            js::BindingPattern::ArrayPattern(_) => fail(at, "a list pattern comes in a later slice: read each item with xs[0]"),
        }
    }

    /// A statement for its effect: a write, a delete, or a call that does something.
    fn effect(&mut self, e: &js::Expression) -> R<Stmt> {
        let at = self.span(e.span());
        let stmt = |kind| Ok(Stmt { kind, span: at, after: None });
        match self.unwrap(e) {
            js::Expression::AssignmentExpression(a) => {
                if a.operator != js::AssignmentOperator::Assign {
                    return fail(at, format!("{} comes in a later slice: write the new value, x = …", a.operator.as_str()));
                }
                let value = self.expr(&a.right)?;
                match &a.left {
                    js::AssignmentTarget::StaticMemberExpression(m) if self.is_state(&m.object) => {
                        let key = m.property.name.to_string();
                        self.state_keys.insert(key.clone());
                        stmt(StmtKind::StateWrite { key: self.name(&key, m.property.span), value })
                    }
                    js::AssignmentTarget::ComputedMemberExpression(m) => {
                        let Some(collection) = self.store_name(&m.object) else {
                            return fail(self.span(m.object.span()), "only an imported collection or folder is written by key: import notes from \"collection:notes\"");
                        };
                        let key = self.expr(&m.expression)?;
                        stmt(StmtKind::CollectionWrite { collection: self.name(&collection, m.object.span()), key, value })
                    }
                    js::AssignmentTarget::AssignmentTargetIdentifier(i) => fail(self.span(i.span), format!("\"{}\" is a const: a new value is a new name, for now (let comes in a later slice)", i.name)),
                    _ => fail(at, "only state, a collection or a folder is written: state.k = v, notes[k] = v"),
                }
            }
            js::Expression::UnaryExpression(u) if u.operator == js::UnaryOperator::Delete => {
                let js::Expression::ComputedMemberExpression(m) = self.unwrap(&u.argument) else {
                    return fail(at, "delete takes an item of a collection or folder: delete notes[k]");
                };
                let Some(collection) = self.store_name(&m.object) else {
                    return fail(self.span(m.object.span()), "only an imported collection or folder has items to delete");
                };
                let key = self.expr(&m.expression)?;
                stmt(StmtKind::Delete { collection: self.name(&collection, m.object.span()), key })
            }
            other => {
                let value = self.expr(other)?;
                stmt(StmtKind::Expr(value))
            }
        }
    }

    /// What `return e` (or an arrow's expression) is in `block`: a route's or an action's answer,
    /// or a function's output.
    fn answer_stmt(&mut self, e: &js::Expression, block: Block) -> R<Stmt> {
        let at = self.span(e.span());
        match block {
            Block::Route => {
                let args = self.answer(e)?;
                Ok(Stmt { kind: StmtKind::Respond(args), span: at, after: None })
            }
            Block::Action => {
                if self.answers_with(e).is_some() {
                    return fail(at, "an action answers with a value alone: return it, without status or respond");
                }
                let value = self.expr(e)?;
                Ok(Stmt { kind: StmtKind::Respond(vec![Arg { name: None, value }]), span: at, after: None })
            }
            Block::Every | Block::File => {
                // An arrow's expression is a statement here: what the run does.
                if matches!(self.unwrap(e), js::Expression::AssignmentExpression(_) | js::Expression::UnaryExpression(_) | js::Expression::CallExpression(_)) {
                    return self.effect(e);
                }
                fail(at, format!("a {}'s run answers nobody: it writes, sends or calls", block_word(block)))
            }
            Block::Function => fail(at, "a function returns once, last: its value is its output"),
        }
    }

    /// `status(…)` or `respond(…)`, if `e` is one, and its call.
    fn answers_with<'a>(&self, e: &'a js::Expression<'a>) -> Option<(Vocab, &'a js::CallExpression<'a>)> {
        match self.unwrap(e) {
            js::Expression::CallExpression(c) => match self.vocab_of(&c.callee) {
                Some(v @ (Vocab::Status | Vocab::Respond)) => Some((v, c)),
                _ => None,
            },
            js::Expression::LogicalExpression(l) if l.operator == js::LogicalOperator::Coalesce => self.answers_with(&l.right).or_else(|| self.answers_with(&l.left)),
            _ => None,
        }
    }

    /// A route's answer as `respond`'s arguments: `return v` answers 200 with it, `status(code,
    /// body)` that code, `respond(body, { headers, stream })` the rest, `… ?? status(404)` what it
    /// answers when the body is absent.
    fn answer(&mut self, e: &js::Expression) -> R<Vec<Arg>> {
        let at = self.span(e.span());
        let named = |name: &str, value: Expr| Arg { name: Some(Name { text: name.into(), span: value.span }), value };
        match self.unwrap(e) {
            js::Expression::LogicalExpression(l) if l.operator == js::LogicalOperator::Coalesce && self.answers_with(&l.right).is_some() => {
                let Some((Vocab::Status, c)) = self.answers_with(&l.right) else {
                    return fail(self.span(l.right.span()), "after ?? comes the status for an absent answer: ?? status(404)");
                };
                let [code] = c.arguments.as_slice() else {
                    return fail(self.span(c.span), "after ?? comes a status alone: ?? status(404)");
                };
                let code = self.expr(code.as_expression().ok_or(()).or_else(|_| fail(at, "a status is a number"))?)?;
                let mut args = self.answer(&l.left)?;
                if args.iter().any(|a| a.name.as_ref().is_some_and(|n| n.text == "missing")) {
                    return fail(at, "one ?? status(…) says what an absent answer is");
                }
                args.push(named("missing", code));
                Ok(args)
            }
            js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Status) => {
                let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
                match args.as_slice() {
                    [code] => Ok(vec![Arg { name: None, value: self.expr(code)? }]),
                    [code, body] => {
                        let code = self.expr(code)?;
                        Ok(vec![Arg { name: None, value: self.expr(body)? }, named("status", code)])
                    }
                    _ => fail(at, "status takes the code, then the body: status(201, body)"),
                }
            }
            js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Respond) => {
                let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
                let (body, options) = match args.as_slice() {
                    [body] => (body, None),
                    [body, options] => (body, Some(*options)),
                    _ => return fail(at, "respond takes the body, then its options: respond(body, { headers, stream })"),
                };
                let mut out = vec![Arg { name: None, value: self.expr(body)? }];
                if let Some(o) = options {
                    let js::Expression::ObjectExpression(o) = self.unwrap(o) else {
                        return fail(self.span(o.span()), "respond's options are an object: { headers: { … } }");
                    };
                    for (key, value, span) in self.fields(o)? {
                        if !["headers", "stream", "status"].contains(&key.as_str()) {
                            return fail(span, format!("respond's options are headers, stream and status, not \"{key}\""));
                        }
                        let value = self.expr(value)?;
                        out.push(Arg { name: Some(Name { text: key, span }), value });
                    }
                }
                Ok(out)
            }
            js::Expression::NumericLiteral(_) => Ok(vec![named("body", self.expr(e)?)]),
            _ => Ok(vec![Arg { name: None, value: self.expr(e)? }]),
        }
    }

    // ── functions ────────────────────────────────────────────────────────────────────────────

    /// `function name(params) { …; return out }`: a graph holon, its output named after what it
    /// returns (a name, or each key of a returned object), else after the function.
    fn function(&mut self, name: &str, name_span: oxc_span::Span, f: &js::Function) -> R<HolonDef> {
        let at = self.span(f.span);
        if f.generator || f.r#async {
            return fail(at, "a function computes: no async, no generator");
        }
        if f.type_parameters.is_some() {
            return fail(at, "a type parameter is refused for now: a port has one type");
        }
        let Some(body) = &f.body else { return fail(at, "a function needs a body") };
        let inputs = self.ports(&f.params)?;
        let returns = f.return_type.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
        self.holon_def(name, name_span, inputs, returns, at, |me, stmts| me.statements_to_output(name, &body.statements, stmts))
    }

    /// `const name = (params) => …` at the top level: a graph holon, as a function is.
    fn arrow_holon(&mut self, name: &str, name_span: oxc_span::Span, a: &js::ArrowFunctionExpression) -> R<HolonDef> {
        let at = self.span(a.span);
        if a.r#async {
            return fail(at, "a function computes: no async");
        }
        if a.type_parameters.is_some() {
            return fail(at, "a type parameter is refused for now: a port has one type");
        }
        let inputs = self.ports(&a.params)?;
        let returns = a.return_type.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
        self.holon_def(name, name_span, inputs, returns, at, |me, stmts| match &a.body {
            js::ArrowFunctionBody::FunctionBody(b) => me.statements_to_output(name, &b.statements, stmts),
            body => {
                let e = body.as_expression().expect("an expression body");
                me.output_of(name, e, stmts)
            }
        })
    }

    fn holon_def(
        &mut self,
        name: &str,
        name_span: oxc_span::Span,
        inputs: Vec<PortDecl>,
        returns: Option<TypeExpr>,
        at: Span,
        body: impl FnOnce(&mut Self, &mut Vec<Stmt>) -> R<Vec<String>>,
    ) -> R<HolonDef> {
        self.scopes.push(inputs.iter().map(|p| (p.name.text.clone(), Local::Bound(p.name.text.clone()))).collect());
        let mut stmts = Vec::new();
        let outputs = body(self, &mut stmts);
        self.scopes.pop();
        let outputs = outputs?;
        if returns.is_some() && outputs.len() != 1 {
            return fail(at, "a return type names one output's type: a function returning an object has one output per key");
        }
        let outputs = outputs.into_iter().map(|o| PortDecl { name: Name { text: o, span: at }, ty: returns.clone(), unit: None }).collect();
        Ok(HolonDef { name: self.name(name, name_span), inputs, outputs, body: stmts, span: at })
    }

    /// A function body: its statements, then the output its last statement returns.
    fn statements_to_output(&mut self, function: &str, body: &[js::Statement], stmts: &mut Vec<Stmt>) -> R<Vec<String>> {
        let Some((js::Statement::ReturnStatement(r), rest)) = body.split_last() else {
            return fail(body.last().map(|s| self.span(s.span())).unwrap_or_default(), "a function returns its value last: return slug");
        };
        self.statements(rest, Block::Function, stmts)?;
        let Some(e) = &r.argument else { return fail(self.span(r.span), "a function returns a value") };
        self.output_of(function, e, stmts)
    }

    /// The outputs `return e` gives: a name returned names the output after it; an object
    /// literal, one output per key; anything else, one output named after the function.
    fn output_of(&mut self, function: &str, e: &js::Expression, stmts: &mut Vec<Stmt>) -> R<Vec<String>> {
        let at = self.span(e.span());
        match self.unwrap(e) {
            js::Expression::Identifier(i) if matches!(self.lookup(i.name.as_str()), Some(Local::Bound(_))) && self.is_binding(i.name.as_str(), stmts) => {
                let Some(Local::Bound(n)) = self.lookup(i.name.as_str()) else { unreachable!() };
                Ok(vec![n])
            }
            js::Expression::ObjectExpression(o) if !o.properties.is_empty() && o.properties.iter().all(|p| matches!(p, js::ObjectPropertyKind::ObjectProperty(_))) => {
                let mut outputs = Vec::new();
                for (key, value, span) in self.fields(o)? {
                    let bound_here = matches!(self.unwrap(value), js::Expression::Identifier(i) if i.name == key.as_str() && self.is_binding(&key, stmts));
                    if !bound_here {
                        let value = self.expr(value)?;
                        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: key.clone(), span }, value }, span, after: None });
                    }
                    outputs.push(key);
                }
                Ok(outputs)
            }
            _ => {
                let value = self.expr(e)?;
                let out = format!("{function}_out");
                stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: out.clone(), span: at }, value }, span: at, after: None });
                Ok(vec![out])
            }
        }
    }

    /// Whether `name` is bound by one of `stmts` (not a parameter passed straight through).
    fn is_binding(&self, name: &str, stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, StmtKind::Bind { name: n, .. } if n.text == name))
    }

    /// A function's parameters as ports: a name each, typed when annotated.
    fn ports(&mut self, params: &js::FormalParameters) -> R<Vec<PortDecl>> {
        if let Some(rest) = &params.rest {
            return fail(self.span(rest.span), "a function takes its inputs by name, not as a rest");
        }
        let mut ports = Vec::new();
        for p in &params.items {
            let js::BindingPattern::BindingIdentifier(id) = &p.pattern else {
                return fail(self.span(p.span), "a parameter is a name, destructured on the first line: (order) => { const { price, qty } = order; … }");
            };
            if p.initializer.is_some() {
                return fail(self.span(p.span), "a parameter has no default: every input is wired");
            }
            let ty = p.type_annotation.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
            ports.push(PortDecl { name: self.name(&self.local_name(id.name.as_str()), id.span), ty, unit: None });
        }
        Ok(ports)
    }

    /// `const near = js(function closest(points, to) { … }, { fuel, memory, outputs })`: a holon
    /// whose body is JavaScript, run by the engine. Its output is named after the function, or
    /// `outputs` names several; the body is passed as written, its types blanked to spaces.
    fn js_holon(&mut self, name: &str, name_span: oxc_span::Span, c: &js::CallExpression) -> R<JsDef> {
        let at = self.span(c.span);
        let shape = "js(function name(a, b) { … }, { fuel, memory, outputs })";
        let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
        if args.len() != c.arguments.len() || args.is_empty() || args.len() > 2 {
            return fail(at, format!("js takes a function, then its options: {shape}"));
        }
        let js::Expression::FunctionExpression(f) = self.unwrap(args[0]) else {
            return fail(self.span(args[0].span()), format!("js takes a named function: {shape}"));
        };
        let Some(id) = &f.id else { return fail(self.span(f.span), format!("js's function is named, and its output after it: {shape}")) };
        if f.r#async || f.generator {
            return fail(self.span(f.span), "a js() function runs to its end: no async, no generator");
        }
        let Some(body) = &f.body else { return fail(self.span(f.span), "a js() function needs a body") };
        let inputs = self.ports(&f.params)?;
        let returns = f.return_type.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
        let (mut fuel, mut memory, mut outputs) = (None, None, None);
        if let Some(o) = args.get(1) {
            let js::Expression::ObjectExpression(o) = self.unwrap(o) else {
                return fail(self.span(o.span()), format!("js's options are an object: {shape}"));
            };
            for (key, value, span) in self.fields(o)? {
                match key.as_str() {
                    "fuel" | "memory" => {
                        let budget = self.budget(value, &key)?;
                        let slot = if key == "fuel" { &mut fuel } else { &mut memory };
                        *slot = Some(budget);
                    }
                    "outputs" => {
                        let js::Expression::ArrayExpression(a) = self.unwrap(value) else {
                            return fail(self.span(value.span()), "outputs are names in quotes: outputs: [\"best\", \"distance\"]");
                        };
                        let mut names = Vec::new();
                        for item in &a.elements {
                            match item.as_expression().map(|e| self.unwrap(e)) {
                                Some(js::Expression::StringLiteral(s)) => names.push(PortDecl { name: self.name(s.value.as_str(), s.span), ty: None, unit: None }),
                                _ => return fail(self.span(item.span()), "an output's name is written in quotes"),
                            }
                        }
                        if names.len() < 2 {
                            return fail(self.span(value.span()), "outputs names two or more; one is the function's name");
                        }
                        outputs = Some(names);
                    }
                    other => return fail(span, format!("js's options are fuel, memory and outputs, not \"{other}\"")),
                }
            }
        }
        let outputs = outputs.unwrap_or_else(|| vec![PortDecl { name: self.name(id.name.as_str(), id.span), ty: returns, unit: None }]);
        let inner = oxc_span::Span::new(body.span.start + 1, body.span.end - 1);
        let text = blank_types(self.src, f, inner).map_err(|(s, why)| LowerError { span: self.span(s), message: why, declare: None })?;
        Ok(JsDef { name: self.name(name, name_span), inputs, outputs, fuel, memory, body: text, body_span: self.span(body.span), span: at })
    }

    /// A js() budget: a whole number (`1_000_000_000`), or one in quotes with its scale (`"50M"`,
    /// `"16MB"`).
    fn budget(&self, value: &js::Expression, which: &str) -> R<Budget> {
        let span = self.span(value.span());
        let (value, text) = match self.unwrap(value) {
            js::Expression::NumericLiteral(n) => (Some(n.value), number_text(n)),
            js::Expression::StringLiteral(s) => {
                let toks = super::lex::lex(s.value.as_str()).ok();
                let v = toks.as_ref().and_then(|t| t.first()).and_then(|t| match t.tok {
                    super::lex::Tok::Number => t.text.parse::<f64>().ok(),
                    super::lex::Tok::Scaled(n) => Some(n),
                    _ => None,
                });
                (v, s.value.to_string())
            }
            _ => (None, String::new()),
        };
        match value.filter(|v| *v >= 1.0 && v.fract() == 0.0) {
            Some(v) => Ok(Budget { value: v as u64, text, span }),
            None => fail(span, format!("{which} is a positive whole number: 1_000_000_000, or \"50M\", \"16MB\"")),
        }
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    fn expr(&mut self, e: &js::Expression) -> R<Expr> {
        let span = self.span(e.span());
        let ok = |kind: ExprKind| Ok(Expr { kind, span });
        match e {
            js::Expression::NumericLiteral(n) => ok(ExprKind::Number { value: n.value, text: number_text(n) }),
            js::Expression::StringLiteral(s) => ok(ExprKind::Text(s.value.to_string())),
            js::Expression::BooleanLiteral(b) => ok(ExprKind::Bool(b.value)),
            js::Expression::NullLiteral(_) => fail(span, "null isn't a value here: a value that isn't there is absent, undefined"),
            js::Expression::TemplateLiteral(t) => self.template(t, span),
            js::Expression::Identifier(i) => self.identifier(i.name.as_str(), span),
            js::Expression::ParenthesizedExpression(p) => ok(ExprKind::Paren(Box::new(self.expr(&p.expression)?))),
            js::Expression::TSAsExpression(t) => self.expr(&t.expression),
            js::Expression::TSSatisfiesExpression(t) => self.expr(&t.expression),
            js::Expression::TSNonNullExpression(t) => self.expr(&t.expression),
            js::Expression::ChainExpression(c) => match &c.expression {
                js::ChainElement::CallExpression(call) => self.call(call),
                js::ChainElement::StaticMemberExpression(m) => self.static_member(m),
                js::ChainElement::ComputedMemberExpression(m) => self.computed_member(m),
                js::ChainElement::TSNonNullExpression(t) => self.expr(&t.expression),
                js::ChainElement::PrivateFieldExpression(_) => fail(span, "a class's private field: a flow has no classes"),
            },
            js::Expression::StaticMemberExpression(m) => self.static_member(m),
            js::Expression::ComputedMemberExpression(m) => self.computed_member(m),
            js::Expression::CallExpression(c) => self.call(c),
            js::Expression::AwaitExpression(a) => {
                // Slice 1 reads a request or a call as today's graph does, between rounds; their
                // order and what `await` adds come with slice 6.
                self.expr(&a.argument)
            }
            js::Expression::ObjectExpression(o) => self.record(o, span),
            js::Expression::ArrayExpression(a) => {
                let mut items = Vec::new();
                for item in &a.elements {
                    match item {
                        js::ArrayExpressionElement::SpreadElement(s) => return fail(self.span(s.span), "a spread in a list comes in a later slice: append(list, item) or extend(list, items)"),
                        js::ArrayExpressionElement::Elision(s) => return fail(self.span(s.span), "a list has no holes"),
                        _ => items.push(self.expr(item.as_expression().expect("an item"))?),
                    }
                }
                ok(ExprKind::List(items))
            }
            js::Expression::UnaryExpression(u) => match u.operator {
                js::UnaryOperator::LogicalNot => ok(ExprKind::Not(Box::new(self.expr(&u.argument)?))),
                js::UnaryOperator::UnaryNegation => match self.unwrap(&u.argument) {
                    js::Expression::NumericLiteral(n) => ok(ExprKind::Number { value: -n.value, text: format!("-{}", number_text(n)) }),
                    _ => fail(span, "a minus needs a number after it (to negate a value, write 0 - x)"),
                },
                js::UnaryOperator::Delete => fail(span, "delete is a statement: delete notes[k]"),
                op => fail(span, format!("{} isn't an operator Holon has", op.as_str())),
            },
            js::Expression::BinaryExpression(b) => {
                use js::BinaryOperator as B;
                let op = match b.operator {
                    B::Equality | B::StrictEquality => BinOp::Eq,
                    B::Inequality | B::StrictInequality => BinOp::Ne,
                    B::LessThan => BinOp::Lt,
                    B::LessEqualThan => BinOp::Le,
                    B::GreaterThan => BinOp::Gt,
                    B::GreaterEqualThan => BinOp::Ge,
                    B::Addition => {
                        if is_text(self.unwrap(&b.left)) || is_text(self.unwrap(&b.right)) {
                            return fail(span, "there is no + on texts: use a template literal, `${a}${b}`");
                        }
                        BinOp::Add
                    }
                    B::Subtraction => BinOp::Sub,
                    B::Multiplication => BinOp::Mul,
                    B::Division => BinOp::Div,
                    op => return fail(span, format!("{} isn't an operator Holon has", op.as_str())),
                };
                let (lhs, rhs) = (self.expr(&b.left)?, self.expr(&b.right)?);
                ok(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) })
            }
            js::Expression::LogicalExpression(l) => {
                if self.answers_with(e).is_some() {
                    return fail(span, "status(…) and respond(…) are a route's answer: return them");
                }
                let op = match l.operator {
                    js::LogicalOperator::Or => BinOp::Or,
                    js::LogicalOperator::And => BinOp::And,
                    js::LogicalOperator::Coalesce => BinOp::Coalesce,
                };
                let (lhs, rhs) = (self.expr(&l.left)?, self.expr(&l.right)?);
                ok(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) })
            }
            js::Expression::ConditionalExpression(c) => {
                let cond = self.expr(&c.test)?;
                let then = self.expr(&c.consequent)?;
                let els = match self.unwrap(&c.alternate) {
                    js::Expression::Identifier(i) if i.name == "undefined" && self.lookup("undefined").is_none() => None,
                    other => Some(Box::new(self.expr(other)?)),
                };
                ok(ExprKind::If { cond: Box::new(cond), then: Box::new(then), els })
            }
            js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_) => fail(span, "a function written in place goes to map or reduce: xs.map((x) => …); define the others at the top level"),
            js::Expression::AssignmentExpression(_) => fail(span, "a write is a statement of its own"),
            _ => fail(span, format!("{} isn't something Holon computes: do it in a js() function", expression_kind(e))),
        }
    }

    fn template(&mut self, t: &js::TemplateLiteral, span: Span) -> R<Expr> {
        let mut parts = Vec::new();
        for (i, q) in t.quasis.iter().enumerate() {
            let Some(text) = &q.value.cooked else { return fail(self.span(q.span), "not an escape a text can hold") };
            if !text.is_empty() {
                parts.push(Part::Text(text.to_string()));
            }
            if let Some(e) = t.expressions.get(i) {
                parts.push(Part::Expr(self.expr(e)?));
            }
        }
        Ok(match parts.as_slice() {
            [] => Expr { kind: ExprKind::Text(String::new()), span },
            [Part::Text(only)] => Expr { kind: ExprKind::Text(only.clone()), span },
            _ => Expr { kind: ExprKind::Interpolated(parts), span },
        })
    }

    fn record(&mut self, o: &js::ObjectExpression, span: Span) -> R<Expr> {
        let mut spread = None;
        let mut fields = Vec::new();
        for (i, p) in o.properties.iter().enumerate() {
            match p {
                js::ObjectPropertyKind::SpreadProperty(s) if i == 0 => spread = Some(Box::new(self.expr(&s.argument)?)),
                js::ObjectPropertyKind::SpreadProperty(s) => return fail(self.span(s.span), "a record spreads one base, first: { ...base, k: v }"),
                js::ObjectPropertyKind::ObjectProperty(p) => {
                    if p.kind != js::PropertyKind::Init || p.method {
                        return fail(self.span(p.span), "a record holds values: no getters, setters or methods");
                    }
                    let quoted = matches!(&p.key, js::PropertyKey::StringLiteral(_));
                    let text = self.property_key(&p.key, p.computed)?;
                    let key = Key { text, quoted, span: self.span(p.key.span()) };
                    fields.push((key, self.expr(&p.value)?));
                }
            }
        }
        Ok(Expr { kind: ExprKind::Record { spread, fields }, span })
    }

    fn property_key(&self, key: &js::PropertyKey, computed: bool) -> R<String> {
        if computed {
            return fail(self.span(key.span()), "a computed key comes in a later slice: name the key, or use insert(record, key, value)");
        }
        match key {
            js::PropertyKey::StaticIdentifier(i) => Ok(i.name.to_string()),
            js::PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            js::PropertyKey::NumericLiteral(n) => Ok(number_text(n)),
            _ => fail(self.span(key.span()), "a key is a name or a text"),
        }
    }

    /// An object literal's fields: key, value, the field's span. Spreads and computed keys are
    /// refused, as options are named.
    fn fields<'a>(&self, o: &'a js::ObjectExpression<'a>) -> R<Vec<(String, &'a js::Expression<'a>, Span)>> {
        let mut out = Vec::new();
        for p in &o.properties {
            let js::ObjectPropertyKind::ObjectProperty(p) = p else {
                return fail(self.span(p.span()), "options are named one by one: no spread");
            };
            out.push((self.property_key(&p.key, p.computed)?, &p.value, self.span(p.span)));
        }
        Ok(out)
    }

    fn identifier(&mut self, name: &str, span: Span) -> R<Expr> {
        let ok = |kind: ExprKind| Ok(Expr { kind, span });
        if let Some(local) = self.lookup(name) {
            return match local {
                Local::Bound(n) => ok(ExprKind::Name(n)),
                Local::Alias(e) => Ok(Expr { span, ..e }),
            };
        }
        if self.consts.contains_key(name) || self.defs.contains_key(name) {
            return ok(ExprKind::Name(name.to_string()));
        }
        match self.imports.get(name).cloned() {
            Some(Noun::Secret(s)) => ok(ExprKind::Secret(s)),
            Some(Noun::Declared(_, n)) => ok(ExprKind::Name(n)),
            Some(Noun::Now) => ok(ExprKind::Noun(Noun::Now.into())),
            Some(Noun::Random) => ok(ExprKind::Noun(Noun::Random.into())),
            Some(Noun::State) => fail(span, format!("the state is read by key: {name}.hits")),
            Some(Noun::Host(h)) => fail(span, format!("a host is called: {name}.get(\"/path\"), a request to {h}")),
            Some(Noun::Action { action, .. }) => fail(span, format!("an action is called: {name}(input) runs {action}")),
            Some(Noun::Vocab(Vocab::Leaf(l))) => ok(ExprKind::Name(l)),
            Some(Noun::Vocab(_)) => fail(span, format!("{name} is a handler or an answer, not a value")),
            None if name == "undefined" => fail(span, "undefined is written as a branch: c ? a : undefined"),
            None => fail(span, format!("unknown name \"{name}\"{}", if GLOBALS.contains(&name) { ": a flow has no globals; import what it uses" } else { "" })),
        }
    }

    fn static_member(&mut self, m: &js::StaticMemberExpression) -> R<Expr> {
        let span = self.span(m.span);
        let prop = m.property.name.as_str();
        if self.is_state(&m.object) {
            self.state_keys.insert(prop.to_string());
            return Ok(Expr { kind: ExprKind::State(prop.to_string()), span });
        }
        if prop == "length" {
            return fail(span, "a list's length is length(xs), a text's text_length(s), imported from \"holon\": .length comes in a later slice");
        }
        let base = self.expr(&m.object)?;
        Ok(Expr { kind: ExprKind::Field { base: Box::new(base), name: self.name(prop, m.property.span) }, span })
    }

    fn computed_member(&mut self, m: &js::ComputedMemberExpression) -> R<Expr> {
        let span = self.span(m.span);
        let base = self.expr(&m.object)?;
        let key = self.expr(&m.expression)?;
        Ok(Expr { kind: ExprKind::Index { base: Box::new(base), key: Box::new(key) }, span })
    }

    fn call(&mut self, c: &js::CallExpression) -> R<Expr> {
        let span = self.span(c.span);
        let ok = |kind: ExprKind| Ok(Expr { kind, span });
        if c.type_arguments.is_some() {
            return fail(span, "a call takes no type arguments");
        }
        match self.unwrap(&c.callee) {
            js::Expression::Identifier(i) => {
                let name = i.name.as_str();
                let callee = self.name(name, i.span);
                if self.lookup(name).is_some() {
                    return fail(callee.span, format!("\"{name}\" is a value, not something to call"));
                }
                if self.defs.contains_key(name) {
                    let args = self.positional(&c.arguments)?;
                    return ok(ExprKind::Call { callee, args });
                }
                match self.imports.get(name).cloned() {
                    Some(Noun::Vocab(Vocab::Leaf(leaf))) => {
                        let args = self.args(&c.arguments, &Takes::of_leaf(&leaf))?;
                        ok(ExprKind::Call { callee: Name { text: leaf, span: callee.span }, args })
                    }
                    Some(Noun::Action { flow, action }) => {
                        let args = self.positional(&c.arguments)?;
                        let receiver = Name { text: super::lower::call_noun(&flow), span: callee.span };
                        ok(ExprKind::Method { receiver, method: Name { text: action, span: callee.span }, args })
                    }
                    Some(Noun::Vocab(Vocab::Status | Vocab::Respond)) => fail(span, "status(…) and respond(…) are a route's answer: return them"),
                    Some(Noun::Vocab(v)) => fail(span, format!("{} is a statement of the top level", vocab_word(&v))),
                    Some(_) => fail(callee.span, format!("\"{name}\" isn't something to call")),
                    None if self.consts.contains_key(name) => fail(callee.span, format!("\"{name}\" is a value, not something to call")),
                    None => fail(callee.span, format!("unknown function \"{name}\": define it at the top level, or import the leaf from \"holon\"")),
                }
            }
            js::Expression::StaticMemberExpression(m) => self.method_call(c, m),
            _ => fail(span, "a call names what it calls: a function, a leaf, or a method of something imported"),
        }
    }

    /// `notes.list({ limit: 50 })`, `api.post("/p", { body })`, `JSON.parse(text)`, `xs.map(f)`.
    fn method_call(&mut self, c: &js::CallExpression, m: &js::StaticMemberExpression) -> R<Expr> {
        let span = self.span(c.span);
        let ok = |kind: ExprKind| Ok(Expr { kind, span });
        let method = m.property.name.as_str();
        let method_name = self.name(method, m.property.span);
        if let js::Expression::Identifier(r) = self.unwrap(&m.object)
            && self.lookup(r.name.as_str()).is_none()
            && !self.defs.contains_key(r.name.as_str())
            && !self.consts.contains_key(r.name.as_str())
        {
            let receiver = r.name.as_str();
            match (self.imports.get(receiver).cloned(), receiver) {
                (Some(Noun::Host(host)), _) => {
                    let mut args = vec![Arg { name: None, value: Expr { kind: ExprKind::Text(host), span: self.span(r.span) } }];
                    args.extend(self.args(&c.arguments, &Takes::request())?);
                    return ok(ExprKind::Method { receiver: Name { text: "http".into(), span: self.span(r.span) }, method: method_name, args });
                }
                (Some(Noun::Declared(kind, name)), _) => {
                    let takes = Takes::of_method(kind, method);
                    let args = self.args(&c.arguments, &takes)?;
                    return ok(ExprKind::Method { receiver: Name { text: name, span: self.span(r.span) }, method: method_name, args });
                }
                (Some(Noun::Vocab(Vocab::Stripe)), _) => {
                    let args = self.args(&c.arguments, &Takes::of_leaf("stripe_verify"))?;
                    return ok(ExprKind::Method { receiver: Name { text: "stripe".into(), span: self.span(r.span) }, method: method_name, args });
                }
                (Some(Noun::State), _) => return fail(span, format!("the state is read and written by key: {receiver}.k, {receiver}.k = v")),
                (None, "JSON") => {
                    let leaf = match method {
                        "parse" => "json_parse",
                        "stringify" => "json_stringify",
                        other => return fail(method_name.span, format!("JSON has parse and stringify, not \"{other}\"")),
                    };
                    let args = self.positional(&c.arguments)?;
                    return ok(ExprKind::Call { callee: Name { text: leaf.into(), span: method_name.span }, args });
                }
                (None, "Object") => {
                    if method != "keys" {
                        return fail(method_name.span, format!("Object has keys, not \"{method}\": do the rest in a js() function"));
                    }
                    let args = self.positional(&c.arguments)?;
                    return ok(ExprKind::Call { callee: Name { text: "keys".into(), span: method_name.span }, args });
                }
                (None, g) if GLOBALS.contains(&g) => return fail(self.span(r.span), format!("{g} isn't there: a flow has no globals; import what it uses")),
                _ => {}
            }
        }
        // A method of a value: the library's, mapped to the leaves that have them.
        let subject = self.expr(&m.object)?;
        let call = |leaf: &str, args: Vec<Arg>| Ok(Expr { kind: ExprKind::Call { callee: Name { text: leaf.into(), span: method_name.span }, args }, span });
        let first = |subject: Expr| Arg { name: None, value: subject };
        match (method, c.arguments.len()) {
            ("map", 1) => {
                let f = self.function_arg(&c.arguments[0], 1)?;
                return call("map", vec![first(subject), f]);
            }
            ("reduce", 2) => {
                let f = self.function_arg(&c.arguments[0], 2)?;
                let init = self.positional(&c.arguments[1..])?.remove(0);
                return call("fold", vec![init, first(subject), f]);
            }
            ("map", _) | ("reduce", _) => {
                return fail(span, format!("{method} takes {}: extras come in a later slice", if method == "map" { "one function, xs.map((x) => …)" } else { "a function and the start, xs.reduce((acc, x) => …, 0)" }));
            }
            _ => {}
        }
        let rest = self.positional(&c.arguments)?;
        match (method, rest.len()) {
            ("split", 1) | ("join", 1) | ("indexOf", 1) => {
                let leaf = match method {
                    "split" => "split",
                    "join" => "join",
                    _ => "index_of",
                };
                let mut args = vec![first(subject)];
                args.extend(rest);
                call(leaf, args)
            }
            ("toUpperCase", 0) => call("uppercase", vec![first(subject)]),
            ("toLowerCase", 0) => call("lowercase", vec![first(subject)]),
            _ => fail(method_name.span, format!("no leaf does .{method}(…): do it in a js() function")),
        }
    }

    /// The function `map` or `reduce` takes: a holon by name, or an arrow written in place, its
    /// body one expression.
    fn function_arg(&mut self, a: &js::Argument, takes: usize) -> R<Arg> {
        let e = a.as_expression().ok_or(()).or_else(|_| fail(self.span(a.span()), "a function, not a spread"))?;
        let span = self.span(e.span());
        let fn_name = Name { text: "fn".into(), span };
        let value = match self.unwrap(e) {
            js::Expression::Identifier(i) if self.defs.contains_key(i.name.as_str()) => Expr { kind: ExprKind::Name(i.name.to_string()), span },
            js::Expression::ArrowFunctionExpression(arrow) => {
                if arrow.r#async {
                    return fail(span, "a function written in place computes: no async");
                }
                let mut params = Vec::new();
                for p in &arrow.params.items {
                    let js::BindingPattern::BindingIdentifier(id) = &p.pattern else {
                        return fail(self.span(p.span), "a parameter is a name: (x) => { const { id } = x; … }, in a later slice");
                    };
                    params.push(self.name(&self.local_name(id.name.as_str()), id.span));
                }
                if params.len() != takes || arrow.params.rest.is_some() {
                    return fail(span, if takes == 1 { "map's function takes the item: (x) => …; an index isn't passed" } else { "reduce's function takes the result so far and the item: (acc, x) => …" });
                }
                let body = match &arrow.body {
                    js::ArrowFunctionBody::FunctionBody(b) => match b.statements.as_slice() {
                        [js::Statement::ReturnStatement(r)] if r.argument.is_some() => r.argument.as_ref().expect("checked"),
                        _ => return fail(self.span(b.span), "a function written in place is one expression, for now: (x) => x.total, or { return … } alone"),
                    },
                    body => body.as_expression().expect("an expression body"),
                };
                let mut scope = HashMap::new();
                for (p, id) in params.iter().zip(&arrow.params.items) {
                    if let js::BindingPattern::BindingIdentifier(id) = &id.pattern {
                        scope.insert(id.name.to_string(), Local::Bound(p.text.clone()));
                    }
                }
                self.scopes.push(scope);
                let body = self.expr(body);
                self.scopes.pop();
                Expr { kind: ExprKind::Lambda { params, body: Box::new(body?) }, span }
            }
            _ => return fail(span, "map and reduce take a function: (x) => …, or a function's name"),
        };
        Ok(Arg { name: Some(fn_name), value })
    }

    /// Arguments in order, none named.
    fn positional(&mut self, args: &[js::Argument]) -> R<Vec<Arg>> {
        let mut out = Vec::new();
        for a in args {
            let Some(e) = a.as_expression() else { return fail(self.span(a.span()), "a spread argument isn't passed: name each") };
            out.push(Arg { name: None, value: self.expr(e)? });
        }
        Ok(out)
    }

    /// Arguments in order, and a last object literal, when what is called takes it so, as its
    /// named inputs and params: `notes.list({ limit: 50 })`, `api.post("/p", { body })`. It is
    /// read so when the arguments before it give every input that can't be left out, and each
    /// of its keys is one the call takes by name; otherwise it is a record, passed in order.
    fn args(&mut self, args: &[js::Argument], takes: &Takes) -> R<Vec<Arg>> {
        let options = match args.split_last() {
            Some((js::Argument::ObjectExpression(o), before)) if before.len() >= takes.required && !o.properties.is_empty() => {
                let named = self.fields(o).ok().filter(|fields| fields.iter().all(|(k, _, _)| takes.named.iter().any(|n| n == k)));
                named.map(|n| (before, n))
            }
            _ => None,
        };
        let Some((before, named)) = options else { return self.positional(args) };
        let mut out = self.positional(before)?;
        for (key, value, span) in named {
            let value = self.expr(value)?;
            out.push(Arg { name: Some(Name { text: key, span }), value });
        }
        Ok(out)
    }

    // ── types ────────────────────────────────────────────────────────────────────────────────

    /// A TypeScript type as Holon's (§7): `number` is Number, `string[]` List<Text>, an object
    /// type Record, `T | undefined` T.
    fn type_expr(&self, t: &js::TSType) -> R<TypeExpr> {
        self.type_at(t, 0)
    }

    /// `depth` aliases in: one that names itself, through others or not, is refused.
    fn type_at(&self, t: &js::TSType, depth: usize) -> R<TypeExpr> {
        let span = self.span(t.span());
        let simple = |name: &str| Ok(TypeExpr { name: name.into(), args: vec![], span });
        match t {
            js::TSType::TSNumberKeyword(_) => simple("Number"),
            js::TSType::TSBooleanKeyword(_) => simple("Boolean"),
            js::TSType::TSStringKeyword(_) => simple("Text"),
            js::TSType::TSAnyKeyword(_) | js::TSType::TSUnknownKeyword(_) => simple("Any"),
            js::TSType::TSObjectKeyword(_) | js::TSType::TSTypeLiteral(_) => simple("Record"),
            js::TSType::TSParenthesizedType(p) => self.type_at(&p.type_annotation, depth),
            js::TSType::TSArrayType(a) => Ok(TypeExpr { name: "List".into(), args: vec![self.type_at(&a.element_type, depth)?], span }),
            js::TSType::TSTupleType(tuple) => {
                let mut args = Vec::new();
                for e in &tuple.element_types {
                    match e {
                        js::TSTupleElement::TSOptionalType(_) | js::TSTupleElement::TSRestType(_) => return fail(self.span(e.span()), "a tuple's items are each there: no optional or rest items"),
                        _ => args.push(self.type_at(e.to_ts_type(), depth)?),
                    }
                }
                Ok(TypeExpr { name: "Product".into(), args, span })
            }
            js::TSType::TSUnionType(u) => {
                let present: Vec<&js::TSType> = u.types.iter().filter(|t| !matches!(t, js::TSType::TSUndefinedKeyword(_))).collect();
                match present.as_slice() {
                    [one] => self.type_at(one, depth),
                    _ => fail(span, "a union of different shapes is refused for now (the note's fork 19): give the port one type, or none"),
                }
            }
            js::TSType::TSTypeReference(r) => {
                let js::TSTypeName::IdentifierReference(id) = &r.type_name else { return fail(span, "a type is named plainly") };
                let args = r.type_arguments.as_ref().map(|a| a.params.iter().collect::<Vec<_>>()).unwrap_or_default();
                match (id.name.as_str(), args.as_slice()) {
                    ("Array", [item]) => Ok(TypeExpr { name: "List".into(), args: vec![self.type_at(item, depth)?], span }),
                    ("Record", [_, _]) => simple("Record"),
                    (name, []) if self.interfaces.contains(name) => simple("Record"),
                    (name, []) if let Some(named) = self.aliases.get(name) => {
                        if depth > 16 {
                            return fail(span, format!("the type {name} names itself"));
                        }
                        Ok(TypeExpr { span, ..self.type_at(named, depth + 1)? })
                    }
                    (other, _) => fail(span, format!("{other} isn't a type Holon has: number, boolean, string, T[], [A, B], an object type, unknown")),
                }
            }
            _ => fail(span, "this type isn't one Holon has: number, boolean, string, T[], [A, B], an object type, unknown"),
        }
    }

    // ── names ────────────────────────────────────────────────────────────────────────────────

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
    }

    fn bind_local(&mut self, name: &str, local: Local) {
        self.scopes.last_mut().expect("a scope").insert(name.to_string(), local);
    }

    /// The name the lowering knows a TypeScript binding by: its own, unless the `.holon` form
    /// keeps that word for itself (a leaf, a keyword, a noun), when an underscore follows it.
    fn local_name(&self, name: &str) -> String {
        let taken = RESERVED.contains(&name) || interface_of(alias(name)).is_some() || templates::is_template(name);
        if taken { format!("{name}_") } else { name.to_string() }
    }

    fn vocab(&self, name: &str) -> Option<Vocab> {
        match self.imports.get(name) {
            Some(Noun::Vocab(v)) if self.lookup(name).is_none() => Some(v.clone()),
            _ => None,
        }
    }

    fn vocab_of(&self, e: &js::Expression) -> Option<Vocab> {
        match self.unwrap(e) {
            js::Expression::Identifier(i) => self.vocab(i.name.as_str()),
            _ => None,
        }
    }

    fn is_state(&self, e: &js::Expression) -> bool {
        matches!(self.unwrap(e), js::Expression::Identifier(i) if self.lookup(i.name.as_str()).is_none() && self.imports.get(i.name.as_str()) == Some(&Noun::State))
    }

    /// The declared name of the collection or folder `e` names.
    fn store_name(&self, e: &js::Expression) -> Option<String> {
        match self.unwrap(e) {
            js::Expression::Identifier(i) if self.lookup(i.name.as_str()).is_none() => match self.imports.get(i.name.as_str()) {
                Some(Noun::Declared("collection" | "folder", n)) => Some(n.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `e` without the TypeScript around it that changes nothing that runs.
    fn unwrap<'a>(&self, e: &'a js::Expression<'a>) -> &'a js::Expression<'a> {
        match e {
            js::Expression::TSAsExpression(t) => self.unwrap(&t.expression),
            js::Expression::TSSatisfiesExpression(t) => self.unwrap(&t.expression),
            js::Expression::TSNonNullExpression(t) => self.unwrap(&t.expression),
            e => e,
        }
    }

    /// A literal a top-level const may hold.
    fn literal(&mut self, e: &js::Expression) -> Option<Expr> {
        match e {
            js::Expression::NumericLiteral(_) | js::Expression::StringLiteral(_) | js::Expression::BooleanLiteral(_) => self.expr(e).ok(),
            js::Expression::UnaryExpression(u) if u.operator == js::UnaryOperator::UnaryNegation && matches!(&u.argument, js::Expression::NumericLiteral(_)) => self.expr(e).ok(),
            js::Expression::TemplateLiteral(t) if t.expressions.is_empty() => self.expr(e).ok(),
            _ => None,
        }
    }
}

impl From<Noun> for super::ast::Noun {
    fn from(n: Noun) -> super::ast::Noun {
        match n {
            Noun::Now => super::ast::Noun::Now,
            Noun::Random => super::ast::Noun::Random,
            _ => unreachable!("only the clock and randomness are nouns of the tree"),
        }
    }
}

/// What a call takes by name, and how many inputs come before that can't be left out.
struct Takes {
    required: usize,
    named: Vec<String>,
}

impl Takes {
    /// A leaf's or template's inputs and params, from the catalog.
    fn of_leaf(name: &str) -> Takes {
        let leaf = alias(name);
        let (inputs, params): (Vec<String>, Vec<String>) = if let Some(u) = interface_of(leaf) {
            (u.inputs.iter().map(|p| p.name.clone()).collect(), u.params.iter().map(|p| p.name().to_string()).collect())
        } else if let Some(params) = templates::defaults(leaf) {
            let inputs = templates::interface(leaf, &params).map(|i| i.inputs.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
            (inputs, params.iter().map(|p| p.name().to_string()).collect())
        } else {
            (vec![], vec![])
        };
        let inputs: Vec<String> = inputs.into_iter().filter(|n| n != TRIGGER).collect();
        let required = inputs.iter().filter(|n| !is_optional(leaf, n)).count();
        // A variadic leaf's inputs are more of the same: only its params are named.
        let mut named = params.into_iter().filter(|p| p != "fn").collect::<Vec<_>>();
        if !VARIADIC.contains(&leaf) {
            named.extend(inputs);
        }
        Takes { required, named }
    }

    /// An outbound request's, after its host: the path, then headers, body, a file or stream.
    fn request() -> Takes {
        Takes { required: 1, named: ["headers", "body", "file", "stream"].map(String::from).to_vec() }
    }

    /// A method of a declared collection, folder, database or channel.
    fn of_method(kind: &str, method: &str) -> Takes {
        let leaf = match (kind, method) {
            ("collection", "list") => "collection_list",
            ("folder", "list") => "file_list",
            ("folder", "lines") => "file_lines",
            ("folder", "slice") => "file_slice",
            ("database", "exec") => return Takes { required: 1, named: vec!["expect".into()] },
            ("database", "find") => return Takes { required: 2, named: vec!["sort".into(), "limit".into()] },
            ("database", "find_one") => return Takes { required: 2, named: vec!["sort".into()] },
            ("database", "update") => return Takes { required: 3, named: vec!["expect".into()] },
            ("database", "delete") => return Takes { required: 2, named: vec!["expect".into()] },
            _ => return Takes { required: usize::MAX, named: vec![] },
        };
        let mut t = Takes::of_leaf(leaf);
        t.named.retain(|n| n != "collection" && n != "folder");
        t
    }
}

/// A route's request's parts, each an output of its node: a name for one is the node itself.
const REQUEST_PARTS: &[&str] = &["request", "params", "query", "headers", "body", "json"];

/// JavaScript's globals a flow doesn't have, named in the mistake.
const GLOBALS: &[&str] = &["fetch", "Date", "Math", "setTimeout", "setInterval", "process", "globalThis", "window", "console", "Promise", "require"];

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !s.starts_with(|c: char| c.is_ascii_digit())
}

fn declares_method(m: &str) -> bool {
    holon_core::METHODS.contains(&m)
}

fn is_text(e: &js::Expression) -> bool {
    matches!(e, js::Expression::StringLiteral(_) | js::Expression::TemplateLiteral(_))
}

/// A number as the `.holon` form writes it: its digits as written, without `_` separators.
fn number_text(n: &js::NumericLiteral) -> String {
    match &n.raw {
        Some(raw) if n.base == js::NumberBase::Decimal || n.base == js::NumberBase::Float => raw.as_str().replace('_', ""),
        _ => {
            let v = n.value;
            if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
        }
    }
}

/// `"10s"`, `"5m"`, `"2h"`, `"1d"` in seconds.
fn duration(text: &str) -> Option<f64> {
    let toks = super::lex::lex(text).ok()?;
    match toks.as_slice() {
        [t, ..] if t.text == text => match t.tok {
            super::lex::Tok::Duration(s) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn function_params<'a>(f: &'a js::Expression<'a>) -> &'a js::FormalParameters<'a> {
    match f {
        js::Expression::ArrowFunctionExpression(a) => &a.params,
        js::Expression::FunctionExpression(f) => &f.params,
        _ => unreachable!("checked to be a function"),
    }
}

fn block_word(b: Block) -> &'static str {
    match b {
        Block::Route => "route",
        Block::Action => "action",
        Block::Every => "clock",
        Block::File => "file handler",
        Block::Function => "function",
    }
}

fn vocab_word(v: &Vocab) -> &'static str {
    match v {
        Vocab::Flow => "flow(…)",
        Vocab::Route => "route",
        Vocab::Every => "every(…)",
        Vocab::On => "on.file",
        Vocab::Action => "action(…)",
        Vocab::Js => "js(…)",
        _ => "this",
    }
}

fn statement_kind(s: &js::Statement) -> &'static str {
    match s {
        js::Statement::ForStatement(_) | js::Statement::ForOfStatement(_) | js::Statement::ForInStatement(_) | js::Statement::WhileStatement(_) | js::Statement::DoWhileStatement(_) => "a loop",
        js::Statement::TryStatement(_) | js::Statement::ThrowStatement(_) => "try and throw",
        js::Statement::SwitchStatement(_) => "a switch",
        js::Statement::ClassDeclaration(_) => "a class",
        _ => "this statement",
    }
}

fn expression_kind(e: &js::Expression) -> &'static str {
    match e {
        js::Expression::NewExpression(_) => "new",
        js::Expression::ThisExpression(_) => "this",
        js::Expression::ClassExpression(_) => "a class",
        js::Expression::YieldExpression(_) => "yield",
        js::Expression::ImportExpression(_) => "a dynamic import",
        js::Expression::RegExpLiteral(_) => "a regular expression",
        js::Expression::SequenceExpression(_) => "a comma expression",
        js::Expression::UpdateExpression(_) => "++ and --",
        _ => "this",
    }
}

/// The text of a js() function's body (`inner`, between its braces) with every type written in
/// it replaced by spaces, so Boa, which runs JavaScript, reads the rest exactly where it was
/// (§7). What TypeScript would compile into code (an enum, a namespace, a parameter property)
/// can't be blanked, and is refused.
fn blank_types(src: &str, f: &js::Function, inner: oxc_span::Span) -> Result<String, (oxc_span::Span, String)> {
    struct Blanks {
        spans: Vec<oxc_span::Span>,
        refused: Option<(oxc_span::Span, String)>,
    }
    impl<'a> Visit<'a> for Blanks {
        fn visit_ts_type_annotation(&mut self, t: &js::TSTypeAnnotation<'a>) {
            self.spans.push(t.span);
        }
        fn visit_ts_type_parameter_declaration(&mut self, t: &js::TSTypeParameterDeclaration<'a>) {
            self.spans.push(t.span);
        }
        fn visit_ts_type_parameter_instantiation(&mut self, t: &js::TSTypeParameterInstantiation<'a>) {
            self.spans.push(t.span);
        }
        fn visit_ts_type_alias_declaration(&mut self, t: &js::TSTypeAliasDeclaration<'a>) {
            self.spans.push(t.span);
        }
        fn visit_ts_interface_declaration(&mut self, t: &js::TSInterfaceDeclaration<'a>) {
            self.spans.push(t.span);
        }
        fn visit_ts_as_expression(&mut self, t: &js::TSAsExpression<'a>) {
            self.spans.push(oxc_span::Span::new(t.expression.span().end, t.span.end));
            self.visit_expression(&t.expression);
        }
        fn visit_ts_satisfies_expression(&mut self, t: &js::TSSatisfiesExpression<'a>) {
            self.spans.push(oxc_span::Span::new(t.expression.span().end, t.span.end));
            self.visit_expression(&t.expression);
        }
        fn visit_ts_non_null_expression(&mut self, t: &js::TSNonNullExpression<'a>) {
            self.spans.push(oxc_span::Span::new(t.expression.span().end, t.span.end));
            self.visit_expression(&t.expression);
        }
        fn visit_ts_enum_declaration(&mut self, t: &js::TSEnumDeclaration<'a>) {
            self.refused.get_or_insert((t.span, "an enum compiles to code, which a js() body can't hold: use a record of constants".into()));
        }
        fn visit_ts_namespace_declaration(&mut self, t: &js::TSNamespaceDeclaration<'a>) {
            self.refused.get_or_insert((t.span, "a namespace compiles to code, which a js() body can't hold".into()));
        }
        fn visit_formal_parameter(&mut self, p: &js::FormalParameter<'a>) {
            if p.accessibility.is_some() || p.readonly {
                self.refused.get_or_insert((p.span, "a parameter property compiles to code, which a js() body can't hold".into()));
            }
            oxc_ast_visit::walk::walk_formal_parameter(self, p);
        }
    }
    let mut blanks = Blanks { spans: Vec::new(), refused: None };
    if let Some(body) = &f.body {
        blanks.visit_function_body(body);
    }
    if let Some(refused) = blanks.refused {
        return Err(refused);
    }
    let mut text: Vec<u8> = src.as_bytes()[inner.start as usize..inner.end as usize].to_vec();
    for s in blanks.spans {
        let (a, b) = ((s.start.max(inner.start) - inner.start) as usize, (s.end.min(inner.end) - inner.start) as usize);
        for byte in &mut text[a..b.max(a)] {
            // Spaces for what isn't a line break, so lines and columns stay; a character of
            // several bytes becomes as many spaces, which keeps the text valid UTF-8.
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    Ok(String::from_utf8(text).expect("blanking keeps UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Settings;
    use crate::composites::Names;
    use holon_core::MapResolver;

    /// What a flow deploys as: its name, its server unit's hash and its deploy settings.
    fn deployed(what: &str, lowered: R<Lowered>) -> (Option<String>, Option<String>, Option<Settings>) {
        let lowered = lowered.unwrap_or_else(|e| panic!("{what}: {e}"));
        let mut store = MapResolver::new();
        crate::text::restore(&lowered.doc, &mut store, &Names::new());
        let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
        let server = crate::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
        assert!(server.errors.is_empty(), "{what}: {:?}", server.errors);
        assert!(server.server_hash.is_some(), "{what}: no server unit");
        (lowered.name, server.server_hash, lowered.doc.server)
    }

    fn same_unit(what: &str, ts: &str, holon: &str) {
        assert_eq!(deployed(&format!("{what}.holon.ts"), compile(ts)), deployed(&format!("{what}.holon"), super::super::compile(holon)), "{what}");
    }

    macro_rules! twins {
        ($($name:literal),* $(,)?) => { [$(($name, include_str!(concat!("../../tests/ts-parity/", $name, ".holon.ts")))),*] };
    }

    /// The samples that use only what this slice reads, each written in TypeScript in
    /// tests/ts-parity.
    const TWINS: [(&str, &str); 16] = twins![
        "basket",
        "billing",
        "browser-key",
        "drop-board",
        "fetch-json",
        "guarded-api",
        "hello-api",
        "marketplace",
        "nearest",
        "notes-api",
        "quote",
        "sign-payload",
        "status-board",
        "stress-test",
        "ticker",
        "tickets",
    ];

    /// Until the samples are written in TypeScript (the note's slice 3), each one this slice can
    /// read deploys, written so, as the server unit its `.holon` form does: the same hash, the same
    /// settings.
    #[test]
    fn a_sample_written_in_typescript_is_the_same_unit() {
        for (name, ts) in TWINS {
            let holon = crate::samples::sample(name).unwrap_or_else(|| panic!("no sample {name}")).text;
            same_unit(name, ts, holon);
        }
    }

    /// What no twin says, each beside the `.holon` text it is.
    #[test]
    fn each_construct_lowers_as_its_holon_form() {
        let pairs = [
            (
                "guarded write",
                "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nflow(\"w\", { port: 8000 })\nroute.post(\"/n\", (req) => {\n  if (req.json.keep) notes[req.json.id] = req.json.text\n  return \"ok\"\n})\n",
                "flow w\nport 8000\ncollection notes\n\nroute POST /n (req) {\n  notes[req.json.id] = req.json.text when req.json.keep\n  respond \"ok\"\n}\n",
            ),
            (
                "function",
                "import { flow, route } from \"holon\"\nflow(\"s\", { port: 8000 })\nfunction slugify(text) {\n  const slug = text.toLowerCase().split(\" \").join(\"-\")\n  return slug\n}\nroute.post(\"/s\", (req) => ({ slug: slugify(req.json.title) }))\n",
                "flow s\nport 8000\n\nholon slugify(text) -> slug {\n  slug = join(split(lowercase(text), \" \"), \"-\")\n}\n\nroute POST /s (req) {\n  respond { slug: slugify(req.json.title) }\n}\n",
            ),
            (
                "function of several outputs",
                "import { flow, route } from \"holon\"\nflow(\"m\", { port: 8000 })\nconst split_name = (full) => {\n  const first = full.first\n  return { first, last: full.last }\n}\nroute.post(\"/m\", (req) => {\n  const { first, last } = split_name(req.json)\n  return { first, last }\n})\n",
                "flow m\nport 8000\n\nholon split_name(full) -> (first, last) {\n  first = full.first\n  last = full.last\n}\n\nroute POST /m (req) {\n  parts_1 = split_name(req.json)\n  first = parts_1.first\n  last = parts_1.last\n  respond { first: first, last: last }\n}\n",
            ),
            (
                "settings",
                "import { flow, every } from \"holon\"\nimport state from \"state\"\nflow(\"k\", { keep: { runs: 5, days: 1 }, limits: { run_budget_ms: \"1m\", max_waiting_runs: 10 } })\nevery(\"1m\", () => {\n  state.beats = (state.beats ?? 0) + 1\n})\n",
                "flow k\nkeep 5 runs for 1d\nlimit run_budget_ms 1m\nlimit max_waiting_runs 10\nstate beats\n\nevery 1m {\n  state.beats = (state.beats ?? 0) + 1\n}\n",
            ),
            (
                "defaults and constants",
                "import { flow, route } from \"holon\"\nflow(\"d\", { port: 8000 })\nconst limit = 50\nroute.get(\"/d\", ({ query }) => {\n  const { after = \"\", by } = query\n  return { after, by, limit }\n})\n",
                "flow d\nport 8000\n\nlimit = 50\n\nroute GET /d (req) {\n  after = req.query.after ?? \"\"\n  by = req.query.by\n  respond { after: after, by: by, limit: limit }\n}\n",
            ),
            (
                "a flow's action",
                "import { flow, route } from \"holon\"\nimport { charge } from \"flow:billing\"\nflow(\"c\", { port: 8000 })\nroute.post(\"/c\", (req) => charge(req.json))\n",
                "flow c\nport 8000\ncalls billing\n\nroute POST /c (req) {\n  respond billing.charge(req.json)\n}\n",
            ),
        ];
        for (what, ts, holon) in pairs {
            same_unit(what, ts, holon);
        }
    }

    fn holon_of(ts: &str) -> HolonDef {
        let file = parse(ts).unwrap_or_else(|e| panic!("{e}"));
        file.items.into_iter().find_map(|i| if let Item::Holon(h) = i { Some(h) } else { None }).expect("a function")
    }

    fn written(t: &TypeExpr) -> String {
        match t.args.as_slice() {
            [] => t.name.clone(),
            args => format!("{}<{}>", t.name, args.iter().map(written).collect::<Vec<_>>().join(", ")),
        }
    }

    #[test]
    fn a_parameters_type_is_its_ports_type() {
        let h = holon_of(
            "import { flow } from \"holon\"\nflow(\"t\", { port: 8000 })\ntype Item = { price: number }\ntype Items = Item[]\ninterface Order { items: Items }\ntype Count = number\n\
             function cost(items: Items, tax: number | undefined, names: Array<string>, pair: [Count, boolean], o: Order, x: unknown): number {\n  return tax\n}\n",
        );
        let types: Vec<String> = h.inputs.iter().map(|p| written(p.ty.as_ref().expect("typed"))).collect();
        assert_eq!(types, ["List<Record>", "Number", "List<Text>", "Product<Number, Boolean>", "Record", "Any"]);
        assert_eq!(h.outputs.iter().map(|p| written(p.ty.as_ref().unwrap())).collect::<Vec<_>>(), ["Number"]);
        let refused = parse("import { flow } from \"holon\"\nflow(\"t\", { port: 8000 })\ntype A = B\ntype B = A\nfunction f(a: A) {\n  return a\n}\n").unwrap_err();
        assert!(refused.message.contains("names itself"), "{refused}");
        let union = parse("import { flow } from \"holon\"\nflow(\"t\", { port: 8000 })\nfunction f(a: number | string) {\n  return a\n}\n").unwrap_err();
        assert_eq!(union.to_string(), "3:15: a union of different shapes is refused for now (the note's fork 19): give the port one type, or none");
    }

    /// Boa runs JavaScript: a js() body's types are spaces where they were, so its lines and
    /// columns are the text's.
    #[test]
    fn a_js_bodys_types_are_blanked_in_place() {
        let file = parse("import { flow, js } from \"holon\"\nflow(\"j\", { port: 8000 })\nconst f = js(function out(a: number) {\n  const b: number = a as number\n  return b!\n})\n").unwrap();
        let Some(Item::Js(j)) = file.items.first() else { panic!("a js holon") };
        assert_eq!(j.body, "\n  const b         = a          \n  return b \n");
        assert_eq!((j.name.text.as_str(), j.outputs[0].name.text.as_str()), ("f", "out"));
        assert_eq!(j.inputs[0].ty.as_ref().map(written).as_deref(), Some("Number"));
        let e = parse("import { flow, js } from \"holon\"\nflow(\"j\", { port: 8000 })\nconst f = js(function out(a) {\n  enum E { A }\n  return a\n})\n").unwrap_err();
        assert_eq!(e.to_string(), "4:3: an enum compiles to code, which a js() body can't hold: use a record of constants");
    }

    fn mistake(body: &str) -> String {
        let head = "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nflow(\"f\", { port: 8000 })\n";
        parse(&format!("{head}{body}")).map(|_| ()).unwrap_err().to_string()
    }

    /// What a later slice brings is refused where it is written, saying so.
    #[test]
    fn a_mistake_names_its_line_and_column() {
        assert_eq!(mistake("route.get(\"/\", () => {\n  let n = 1\n  return n\n})\n"), "5:3: let and var come in a later slice: use const, one name per value");
        assert_eq!(mistake("route.get(\"/\", (req) => \"a\" + req.query.b)\n"), "4:25: there is no + on texts: use a template literal, `${a}${b}`");
        assert_eq!(mistake("route.get(\"/\", (req) => fetch(req.query.u))\n"), "4:25: unknown function \"fetch\": define it at the top level, or import the leaf from \"holon\"");
        assert_eq!(mistake("route.get(\"/\", (req) => req.query.xs.length)\n"), "4:25: a list's length is length(xs), a text's text_length(s), imported from \"holon\": .length comes in a later slice");
        assert_eq!(mistake("route.fetch(\"/\", () => 1)\n"), "4:7: route has no \"fetch\": get, post, put, patch or delete");
        assert_eq!(mistake("route.get(\"/\", () => 1, { log: \"loud\" })\n"), "4:32: log is \"strict\", \"lazy\" or \"unrecorded\"");
        assert_eq!(mistake("route.get(\"/\", () => {\n  notes = 1\n  return 1\n})\n"), "5:3: \"notes\" is a const: a new value is a new name, for now (let comes in a later slice)");
        let unknown = parse("import x from \"npm:left-pad\"\n").unwrap_err();
        assert!(unknown.to_string().starts_with("1:15: \"npm:left-pad\" isn't something a flow imports"), "{unknown}");
        let syntax = parse("import { flow } from \"holon\"\nflow(\"f\", {\n").unwrap_err();
        assert_eq!(syntax.span.start.line, 3, "{syntax}");
        assert!(parse("import { route } from \"holon\"\nroute.get(\"/\", () => 1)\n").unwrap_err().message.starts_with("a flow says what it is first"));
    }
}
