//! A flow written in TypeScript's syntax, `NAME.holon.ts` (docs/L4s/typescript-syntax-L4.md): read
//! with a TypeScript parser, oxc's, into the syntax tree the lowering reads ([`super::ast`]), so a
//! flow written and the same flow drawn are the same unit. This is the note's first two slices: imports, the kind statement, the four handlers and
//! their options, `const`, writes, `return`, `status` and `respond`, functions, in-place arrows,
//! `js()`, the library's methods and types as port types; then the statements (§5): `let` and
//! `var`, each new value a new name, destructuring, `if` and `switch` choosing names and writes
//! as `if` and `match` do, `if (x !== undefined)` as `after`, early returns folded into one
//! answer, changes in place, and `await event(…)`. What a later slice brings is refused, saying so.
//!
//! Each import names one thing the flow uses (§3): a secret, the state, a collection, a folder, a
//! database, a channel, an outbound host, the clock, randomness or another flow's action; from
//! `"holon"`, the vocabulary and the leaves. A name reads as what it was imported as, so the text
//! never declares what it uses twice, and the headers the lowering reads are made from the imports.

use std::collections::{BTreeSet, HashMap};

use holon_core::{family_values, interface_of, PASSES_EXTRA, TRIGGER, VARIADIC};
use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

use super::ast::*;
use super::lower::{alias, is_optional, lower, LowerError, Lowered, RESERVED, ROUTE_WAITS};
use crate::templates;

type R<T> = Result<T, LowerError>;

/// Read and lower a `.holon.ts` text.
pub fn compile(src: &str) -> R<Lowered> {
    lower(&parse(src)?)
}

/// Read a `.holon.ts` text into the syntax tree the lowering reads.
pub fn parse(src: &str) -> R<File> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    let lines = Lines::new(src);
    if let Some(e) = parsed.diagnostics.errors().next() {
        let at = e.labels.first().map(|l| l.offset()).unwrap_or(0);
        let pos = lines.pos(at);
        return Err(LowerError { span: Span { start: pos, end: pos }, message: e.message.to_string() });
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
    Err(LowerError { span, message: message.into() })
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
    Event,
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
    /// A `let`'s or a `var`'s name, by the name the lowering knows its value by now: `None`
    /// before it is given one.
    Var(Option<String>),
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

/// A condition the statement being read runs under (§5).
#[derive(Clone, Debug)]
enum Guard {
    /// An `if`'s or a case's condition.
    Branch(Expr),
    /// That a `return` before it, in the same list of statements, didn't answer.
    Live(Expr),
    /// `x`, in `if (x !== undefined) { … }`: what the writes and calls in it wait for.
    After(Expr),
}

/// What a handler's or a function's body has read so far, beside its scopes.
#[derive(Default)]
struct Frame {
    /// The names bound so far, so that each new one is new.
    used: BTreeSet<String>,
    /// The parameters, by the text's names: each stays what it was passed.
    params: BTreeSet<String>,
    guards: Vec<Guard>,
    /// How many branches in.
    depth: usize,
    /// Each state key written, with what it holds by now: written once, where it is last settled.
    writes: Vec<(String, Expr, Span)>,
    /// The returns, in order.
    exits: Vec<Exit>,
    /// The collections and folders this run writes to: read after, refused (§12).
    written: BTreeSet<String>,
    /// The state keys written in an `if (x !== undefined)`, tied to `x`.
    tied: BTreeSet<String>,
    /// The bindings made for a condition or a subject, written back where read if read once.
    hidden: Vec<String>,
    /// Each call bound under a condition, by its binding's name, and what it is tied to if
    /// nothing reads it.
    guarded_calls: Vec<(String, Expr)>,
    /// The bindings another name, a record or a write holds too.
    shared: BTreeSet<String>,
    /// Whether it is a route's: a route answers and is done, so it doesn't wait.
    route: bool,
}

/// A `return`, under the branches it is in.
#[derive(Clone, Debug)]
struct Exit {
    cond: Option<Expr>,
    kind: ExitKind,
    span: Span,
}

#[derive(Clone, Debug)]
enum ExitKind {
    One(Answer),
    /// A switch's returns: each case's patterns and its answer (`None` where it breaks), and the
    /// default's.
    Switch { subject: Expr, arms: Vec<(Vec<Expr>, Option<Answer>)>, default: Option<Answer> },
}

/// An object's fields, each with its key and where it is written.
type Fields = Vec<(String, Expr, Span)>;

/// What a `return` answers.
#[derive(Clone, Debug)]
enum Answer {
    /// A route's.
    Response(Box<Response>),
    /// A function's or an action's; for a function, the binding it names (the text's name and
    /// the lowering's) or an object's fields.
    Value { value: Expr, name: Option<(String, String)>, fields: Option<Fields> },
    /// A clock's or a file handler's `return` alone.
    Nothing,
}

/// A route's answer, as one `respond` gives it.
#[derive(Clone, Debug)]
struct Response {
    body: Option<Expr>,
    status: Option<Expr>,
    missing: Option<Expr>,
    headers: Option<Expr>,
    stream: Option<Expr>,
    span: Span,
}

/// Whether a statement returns: never, always, or when its condition holds.
#[derive(Clone, Debug)]
enum Ret {
    Never,
    Always,
    When(Expr),
}

impl Ret {
    /// This statement's, then the next's.
    fn or(self, next: Ret) -> Ret {
        match (self, next) {
            (Ret::Always, _) | (_, Ret::Always) => Ret::Always,
            (Ret::Never, r) | (r, Ret::Never) => r,
            (Ret::When(a), Ret::When(b)) => Ret::When(binary(BinOp::Or, truthy(&a), b)),
        }
    }
}

/// What picks among branches: an `if`'s condition, or a switch's subject and each case's
/// patterns.
enum Choice {
    If(Expr),
    Match(Expr, Vec<Vec<Expr>>),
}

/// What a branch leaves: its names, its state writes, whether it returned, how many returns it
/// made.
struct Path {
    scopes: Vec<HashMap<String, Local>>,
    writes: Vec<(String, Expr, Span)>,
    ret: Ret,
    exits: usize,
}

/// What an assignment writes.
enum Place {
    /// A `let`'s or a `var`'s name.
    Local(String, Span),
    /// `state.k`.
    State(String, Span),
    /// `notes[k]`: the collection or folder, its name's span, the key.
    Store(String, Span, Expr),
    /// A field or an item of a local: `order.total`, `counts[k]`.
    Member(String, Span, Vec<Seg>),
}

/// A step into a value: a field by name, or an item by key.
#[derive(Clone, Debug)]
enum Seg {
    Field(String, Span),
    Key(Expr),
}

/// How a name is given its value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Decl {
    Const,
    Let,
    /// An assignment, a `var`'s included.
    Assign,
}

/// A change in place at the end of a path into a value.
enum Edit {
    Set(Expr),
    Remove(Seg),
}

/// An assignment's pattern: `[a, b] = [b, a]`, `({ total } = order)`.
enum TargetPattern {
    Name(String, Span),
    Default(Box<TargetPattern>, Expr),
    Record(Vec<(String, Span, TargetPattern)>, Span),
    List(Vec<Option<TargetPattern>>, Option<Box<TargetPattern>>, Span),
}

/// One case of a switch, the empty ones before it among its patterns.
struct Case<'a, 'b> {
    patterns: Vec<Expr>,
    default: bool,
    body: &'a [js::Statement<'b>],
    broke: bool,
    span: Span,
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
    /// The file's own actions (`export const payment = action(…)`), which a handler starts by
    /// calling one, `payment(x)`.
    actions: BTreeSet<String>,
    /// The top level's `type` aliases, each read as the type it names where it is used, and its
    /// interfaces, each a record.
    aliases: HashMap<String, &'t js::TSType<'t>>,
    interfaces: BTreeSet<String>,
    scopes: Vec<HashMap<String, Local>>,
    /// The body being read: its names, conditions, writes and returns.
    frame: Frame,
    /// Every name the text writes, so a name the reader makes is never one of them.
    taken: BTreeSet<String>,
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
            actions: BTreeSet::new(),
            aliases: HashMap::new(),
            interfaces: BTreeSet::new(),
            scopes: Vec::new(),
            frame: Frame::default(),
            taken: BTreeSet::new(),
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
        struct Words(BTreeSet<String>);
        impl<'a> Visit<'a> for Words {
            fn visit_binding_identifier(&mut self, id: &js::BindingIdentifier<'a>) {
                self.0.insert(id.name.to_string());
            }
            fn visit_identifier_reference(&mut self, id: &js::IdentifierReference<'a>) {
                self.0.insert(id.name.to_string());
            }
        }
        let mut words = Words(BTreeSet::new());
        words.visit_program(p);
        self.taken = words.0;
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
                        "event" => Vocab::Event,
                        "start" => return fail(self.span(n.span), START_IS_A_CALL),
                        "tour" | "component" => return fail(self.span(n.span), format!("{what} comes with tours and components, not yet")),
                        leaf if interface_of(alias(leaf)).is_some() || templates::is_template(leaf) => Vocab::Leaf(leaf.to_string()),
                        other => return fail(self.span(n.span), format!("\"holon\" has no \"{other}\": its vocabulary (flow, route, every, on, action, js, respond, status, stripe, event) and its leaves")),
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
            // An action, which a handler may start before it is written.
            js::Statement::ExportDeclaration(e) => {
                if let js::Declaration::VariableDeclaration(v) = &e.declaration {
                    for d in &v.declarations {
                        if let (js::BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init)
                            && let js::Expression::CallExpression(c) = self.unwrap(init)
                            && self.vocab_of(&c.callee) == Some(Vocab::Action)
                        {
                            self.actions.insert(id.name.to_string());
                        }
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
    /// action's input, a file's handle, a clock's tick. Its returns fold into its one answer.
    fn handler_body(&mut self, handler: &js::Expression, block: Block, received: &str) -> R<Vec<Stmt>> {
        let f = self.unwrap(handler);
        if let js::Expression::FunctionExpression(f) = f
            && f.generator
        {
            return fail(self.span(f.span), "a handler isn't a generator");
        }
        let saved = std::mem::take(&mut self.frame);
        self.frame.used.insert(received.to_string());
        self.frame.route = block == Block::Route;
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        let result = self.handler_statements(f, block, received, &mut stmts);
        self.scopes.pop();
        self.frame = saved;
        result.map(|()| stmts)
    }

    fn handler_statements(&mut self, f: &js::Expression, block: Block, received: &str, stmts: &mut Vec<Stmt>) -> R<()> {
        let params = function_params(f);
        if let Some(p) = params.items.first() {
            match (&p.pattern, block) {
                (js::BindingPattern::BindingIdentifier(id), Block::Every) => {
                    let now = Expr { kind: ExprKind::Noun(Noun::Now.into()), span: self.span(id.span) };
                    self.bind_local(id.name.as_str(), Local::Alias(now));
                }
                (js::BindingPattern::BindingIdentifier(id), _) => {
                    self.frame.params.insert(id.name.to_string());
                    self.bind_local(id.name.as_str(), Local::Bound(received.to_string()));
                }
                (js::BindingPattern::ObjectPattern(o), Block::Route) => {
                    // The request's parts are its outputs: each is the request node itself, so a
                    // name for one is no node, read where it is used.
                    let req = Expr { kind: ExprKind::Name(received.to_string()), span: self.span(o.span) };
                    for prop in &o.properties {
                        let key = self.property_key(&prop.key, prop.computed)?;
                        let part = Expr { kind: ExprKind::Field { base: Box::new(req.clone()), name: self.name(&key, prop.key.span()) }, span: self.span(prop.span) };
                        match &prop.value {
                            js::BindingPattern::BindingIdentifier(id) if REQUEST_PARTS.contains(&key.as_str()) => self.bind_local(id.name.as_str(), Local::Alias(part)),
                            pattern => self.destructure(pattern, part, Decl::Const, stmts)?,
                        }
                    }
                    if let Some(rest) = &o.rest {
                        return fail(self.span(rest.span), "the request's parts are named one by one: params, query, headers, body, json, method, path");
                    }
                }
                (pattern, Block::Action | Block::File) => {
                    let whole = Expr { kind: ExprKind::Name(received.to_string()), span: self.span(pattern.span()) };
                    self.destructure(pattern, whole, Decl::Const, stmts)?;
                }
                (pattern, _) => return fail(self.span(pattern.span()), "a clock's tick is a name: every(\"1h\", (now) => …)"),
            }
        }
        let ret = self.body(f, block, stmts)?;
        let at = self.span(f.span());
        self.finish(block, ret, stmts, at)
    }

    /// A function's or handler's body: its statements, or an arrow's expression, which answers as
    /// `return` does.
    fn body(&mut self, f: &js::Expression, block: Block, stmts: &mut Vec<Stmt>) -> R<Ret> {
        match f {
            js::Expression::ArrowFunctionExpression(a) => match &a.body {
                js::ArrowFunctionBody::FunctionBody(b) => self.top(&b.statements, block, stmts),
                body => {
                    let e = body.as_expression().expect("an arrow's body is a block or an expression");
                    self.arrow_answer(e, block, stmts)
                }
            },
            js::Expression::FunctionExpression(f) => match &f.body {
                Some(b) => self.top(&b.statements, block, stmts),
                None => fail(self.span(f.span), "a function needs a body"),
            },
            _ => unreachable!("checked to be a function"),
        }
    }

    /// A body's statements, each `var` in it declared first, as JavaScript does.
    fn top(&mut self, body: &[js::Statement], block: Block, stmts: &mut Vec<Stmt>) -> R<Ret> {
        let mut vars = Vec::new();
        hoisted(body, &mut vars);
        for name in vars {
            if self.scopes.last().is_some_and(|s| !s.contains_key(&name)) {
                self.bind_local(&name, Local::Var(None));
            }
        }
        self.list(body, block, stmts)
    }

    /// An arrow's expression as its body: a route's or an action's answer, a function's output,
    /// or what a clock's or a file's run does.
    fn arrow_answer(&mut self, e: &js::Expression, block: Block, stmts: &mut Vec<Stmt>) -> R<Ret> {
        if matches!(block, Block::Every | Block::File) {
            if matches!(self.unwrap(e), js::Expression::AssignmentExpression(_) | js::Expression::UnaryExpression(_) | js::Expression::CallExpression(_) | js::Expression::UpdateExpression(_)) {
                self.effect(e, stmts, 0)?;
                return Ok(Ret::Never);
            }
            return fail(self.span(e.span()), format!("a {}'s run answers nobody: it writes, sends or calls", block_word(block)));
        }
        self.exit(e, block)?;
        Ok(Ret::Always)
    }

    /// The end of a handler's body: its returns, folded, as its one `respond`.
    fn finish(&mut self, block: Block, ret: Ret, stmts: &mut Vec<Stmt>, at: Span) -> R<()> {
        let exits = std::mem::take(&mut self.frame.exits);
        let span = exits.last().map_or(at, |e| e.span);
        match self.fold(exits, matches!(ret, Ret::Always))? {
            Some(Answer::Response(r)) => stmts.push(Stmt { span: r.span, kind: StmtKind::Respond(respond_args(*r)), after: None }),
            Some(Answer::Value { value, .. }) if block == Block::Action => stmts.push(Stmt { kind: StmtKind::Respond(vec![Arg { name: None, value }]), span, after: None }),
            _ => {}
        }
        self.settle(stmts, &[]);
        Ok(())
    }

    /// What a body still owes once read. A call bound under a condition that nothing reads is
    /// tied to the condition (`after`), as a call nothing reads is made whatever else holds; one
    /// read is asked for only where it is read. A hidden condition read once is written back
    /// where it is read. `outputs` are a function's.
    fn settle(&mut self, stmts: &mut Vec<Stmt>, outputs: &[String]) {
        let live = live_bindings(stmts, outputs);
        for (name, trigger) in std::mem::take(&mut self.frame.guarded_calls) {
            if !live.contains(&name)
                && let Some(s) = stmts.iter_mut().find(|s| matches!(&s.kind, StmtKind::Bind { name: n, .. } if n.text == name))
            {
                s.after = Some(trigger);
            }
        }
        for h in std::mem::take(&mut self.frame.hidden) {
            if uses(stmts, &h) > 1 {
                continue;
            }
            let Some(i) = stmts.iter().position(|s| matches!(&s.kind, StmtKind::Bind { name, .. } if name.text == h)) else { continue };
            let StmtKind::Bind { value, .. } = stmts.remove(i).kind else { unreachable!("found as a binding") };
            each_expr(stmts, &mut |e| {
                if matches!(&e.kind, ExprKind::Name(n) if *n == h) {
                    *e = value.clone();
                }
                true
            });
        }
    }

    // ── statements ───────────────────────────────────────────────────────────────────────────

    /// A list of statements, a body's or a branch's: what follows a `return` that may have
    /// answered runs only when it didn't, until the list ends.
    fn list(&mut self, body: &[js::Statement], block: Block, stmts: &mut Vec<Stmt>) -> R<Ret> {
        let start = self.frame.guards.len();
        let ret = self.run(body, block, stmts, start);
        self.frame.guards.truncate(start);
        ret
    }

    /// `body`'s statements in order, in the list whose conditions begin at guard `start`.
    fn run(&mut self, body: &[js::Statement], block: Block, stmts: &mut Vec<Stmt>, start: usize) -> R<Ret> {
        let mut ret = Ret::Never;
        for s in body {
            if matches!(s, js::Statement::EmptyStatement(_)) {
                continue;
            }
            if matches!(ret, Ret::Always) {
                return fail(self.span(s.span()), "nothing after a return runs: remove it");
            }
            let r = match s {
                js::Statement::BlockStatement(b) => {
                    self.scopes.push(HashMap::new());
                    let r = self.run(&b.body, block, stmts, start);
                    self.scopes.pop();
                    r?
                }
                _ => {
                    let r = self.statement(s, block, stmts, start)?;
                    if let Ret::When(e) = &r {
                        self.frame.guards.push(Guard::Live(otherwise(e)));
                    }
                    r
                }
            };
            ret = ret.or(r);
        }
        Ok(ret)
    }

    fn statement(&mut self, s: &js::Statement, block: Block, stmts: &mut Vec<Stmt>, start: usize) -> R<Ret> {
        let at = self.span(s.span());
        match s {
            js::Statement::ReturnStatement(r) => {
                match &r.argument {
                    Some(e) => self.exit(e, block)?,
                    None => self.bare_exit(block, at)?,
                }
                Ok(Ret::Always)
            }
            js::Statement::VariableDeclaration(v) => {
                self.declaration(v, stmts)?;
                Ok(Ret::Never)
            }
            js::Statement::ExpressionStatement(e) => {
                self.effect(&e.expression, stmts, start)?;
                Ok(Ret::Never)
            }
            js::Statement::IfStatement(f) => self.if_statement(f, block, stmts, start),
            js::Statement::SwitchStatement(w) => self.switch(w, block, stmts, start),
            js::Statement::BreakStatement(_) => fail(at, "break ends a case, last in it"),
            js::Statement::FunctionDeclaration(_) => fail(at, "a function is defined at the top level, where every handler can use it"),
            _ => fail(at, format!("{} isn't something a {} holds yet", statement_kind(s), block_word(block))),
        }
    }

    /// `const`, `let` or `var`: a binding per name, and for `let` and `var` a new one at each
    /// assignment (§5).
    fn declaration(&mut self, v: &js::VariableDeclaration, stmts: &mut Vec<Stmt>) -> R<()> {
        let decl = match v.kind {
            js::VariableDeclarationKind::Const => Decl::Const,
            js::VariableDeclarationKind::Let => Decl::Let,
            // Declared where the body begins (`top`): here it is given its value.
            js::VariableDeclarationKind::Var => Decl::Assign,
            _ => return fail(self.span(v.span), "using isn't something a flow has: a const holds a value"),
        };
        for d in &v.declarations {
            let Some(init) = &d.init else {
                match decl {
                    Decl::Const => return fail(self.span(d.span), "a const needs a value"),
                    Decl::Let => {
                        let mut names = Vec::new();
                        pattern_names(&d.id, &mut names);
                        for n in names {
                            self.bind_local(&n, Local::Var(None));
                        }
                    }
                    Decl::Assign => {}
                }
                continue;
            };
            let value = self.expr(init)?;
            let from_local = self.share(init);
            self.destructure(&d.id, value, decl, stmts)?;
            if from_local {
                // `const b = a`, `const { items } = order`: what they bind holds what `a` does.
                let mut names = Vec::new();
                pattern_names(&d.id, &mut names);
                for n in names {
                    self.share_name(&n);
                }
            }
        }
        Ok(())
    }

    /// `pattern = value`: one binding per name, a `field`, `get`, `head` or `tail` for each part
    /// read (§5's table).
    fn destructure(&mut self, pattern: &js::BindingPattern, value: Expr, decl: Decl, stmts: &mut Vec<Stmt>) -> R<()> {
        let at = self.span(pattern.span());
        match pattern {
            js::BindingPattern::BindingIdentifier(id) => {
                let span = self.span(id.span);
                self.bind_name(id.name.as_str(), span, value, decl, stmts)
            }
            js::BindingPattern::AssignmentPattern(a) => {
                let fallback = self.expr(&a.right)?;
                let span = value.span;
                let value = Expr { kind: ExprKind::Binary { op: BinOp::Coalesce, lhs: Box::new(value), rhs: Box::new(fallback) }, span };
                self.destructure(&a.left, value, decl, stmts)
            }
            js::BindingPattern::ObjectPattern(o) => {
                let base = self.once(value, at, stmts);
                let mut read = Vec::new();
                for prop in &o.properties {
                    let span = self.span(prop.span);
                    let part = if prop.computed {
                        let key = self.expr(prop.key.as_expression().expect("a computed key is an expression"))?;
                        read.push(Seg::Key(key.clone()));
                        Expr { kind: ExprKind::Index { base: Box::new(base.clone()), key: Box::new(key) }, span }
                    } else {
                        let key = self.property_key(&prop.key, false)?;
                        let key_span = self.span(prop.key.span());
                        read.push(Seg::Field(key.clone(), key_span));
                        Expr { kind: ExprKind::Field { base: Box::new(base.clone()), name: Name { text: key, span: key_span } }, span }
                    };
                    self.destructure(&prop.value, part, decl, stmts)?;
                }
                if let Some(rest) = &o.rest {
                    // The record without each key read.
                    let mut without = base;
                    for seg in read {
                        without = match seg {
                            Seg::Field(name, span) => call("without_field", vec![arg(without), named("name", Expr { kind: ExprKind::Text(name), span })], span),
                            Seg::Key(k) => call("remove", vec![arg(without), arg(k)], at),
                        };
                    }
                    self.destructure(&rest.argument, without, decl, stmts)?;
                }
                Ok(())
            }
            js::BindingPattern::ArrayPattern(a) => {
                // `[a, b] = [b, a]`: each item bound as it is, and no list made.
                if a.rest.is_none()
                    && let ExprKind::List(items) = &value.kind
                {
                    let items = items.clone();
                    for (i, el) in a.elements.iter().enumerate() {
                        if let Some(el) = el {
                            let item = items.get(i).cloned().unwrap_or_else(|| absent(at));
                            self.destructure(el, item, decl, stmts)?;
                        }
                    }
                    return Ok(());
                }
                let base = self.once(value, at, stmts);
                match &a.rest {
                    None => {
                        for (i, el) in a.elements.iter().enumerate() {
                            if let Some(el) = el {
                                let index = Expr { kind: ExprKind::Number { value: i as f64, text: i.to_string() }, span: self.span(el.span()) };
                                self.destructure(el, call("get", vec![arg(base.clone()), arg(index)], self.span(el.span())), decl, stmts)?;
                            }
                        }
                    }
                    Some(rest) => {
                        // `[first, ...rest]` is `head` and `tail`; `[a, b, ...rest]` is `tail` twice.
                        let mut list = base;
                        let n = a.elements.len();
                        for (i, el) in a.elements.iter().enumerate() {
                            if let Some(el) = el {
                                self.destructure(el, call("head", vec![arg(list.clone())], self.span(el.span())), decl, stmts)?;
                            }
                            let tail = call("tail", vec![arg(list)], at);
                            list = if i + 1 < n { self.once(tail, at, stmts) } else { tail };
                        }
                        self.destructure(&rest.argument, list, decl, stmts)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// `ts` bound to `value`: a `const`'s one name, a `let`'s first, or an assignment's new one.
    fn bind_name(&mut self, ts: &str, span: Span, value: Expr, decl: Decl, stmts: &mut Vec<Stmt>) -> R<()> {
        if decl == Decl::Assign {
            return self.new_version(ts, span, value, stmts, false);
        }
        let name = self.fresh(ts);
        self.note_call(&name, &value, span);
        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span }, value }, span, after: None });
        self.bind_local(ts, if decl == Decl::Let { Local::Var(Some(name)) } else { Local::Bound(name) });
        Ok(())
    }

    /// `ts` given a new value: a `let`'s or `var`'s assignment, or a local changed in place
    /// (`mutation`, which a `const` may be too). Either is a new name for the new value (§5).
    fn new_version(&mut self, ts: &str, span: Span, value: Expr, stmts: &mut Vec<Stmt>, mutation: bool) -> R<()> {
        let Some(depth) = self.scopes.iter().rposition(|s| s.contains_key(ts)) else {
            return fail(
                span,
                if self.imports.contains_key(ts) {
                    format!("\"{ts}\" is imported, and stays what it is: give a value of its own a name of its own")
                } else if self.defs.contains_key(ts) || self.consts.contains_key(ts) {
                    format!("\"{ts}\" is defined at the top level, and stays what it is")
                } else {
                    format!("unknown name \"{ts}\": declare it with let first")
                },
            );
        };
        let kept = match &self.scopes[depth][ts] {
            Local::Var(None) if mutation => return fail(span, format!("\"{ts}\" has no value to change yet")),
            Local::Var(_) => false,
            Local::Bound(_) if mutation => true,
            Local::Bound(_) if self.frame.params.contains(ts) => return fail(span, format!("\"{ts}\" is a parameter, which stays what it was passed: give the new value a name of its own")),
            Local::Bound(_) => return fail(span, format!("\"{ts}\" is a const: declare it with let to give it a new value")),
            Local::Alias(_) => return fail(span, format!("\"{ts}\" is a part of what the handler received, which stays what it is: give the new value a name of its own")),
        };
        let name = self.fresh(ts);
        self.note_call(&name, &value, span);
        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span }, value }, span, after: None });
        self.scopes[depth].insert(ts.to_string(), if kept { Local::Bound(name) } else { Local::Var(Some(name)) });
        Ok(())
    }

    /// A binding of a call that does something, made under a condition: tied to it if nothing
    /// reads the binding ([`Self::settle`]).
    fn note_call(&mut self, name: &str, value: &Expr, span: Span) {
        if self.contains_sink(value)
            && let Some(trigger) = self.trigger(span)
        {
            self.frame.guarded_calls.push((name.to_string(), trigger));
        }
    }

    /// Whether `e` makes a call the host answers: a request, another flow's action, a send, a
    /// write.
    fn contains_sink(&self, e: &Expr) -> bool {
        let mut found = false;
        walk(&mut e.clone(), &mut |x| {
            if let ExprKind::Method { receiver, method, .. } = &x.kind {
                let m = method.text.as_str();
                found |= receiver.text == "http"
                    || self.imports.values().any(|n| match n {
                        Noun::Action { flow, .. } => super::lower::call_noun(flow) == receiver.text,
                        Noun::Declared(kind, name) if *name == receiver.text => match *kind {
                            "channel" => matches!(m, "send" | "pipe"),
                            "collection" => matches!(m, "put" | "delete"),
                            "folder" => !matches!(m, "list" | "lines" | "slice" | "read" | "get"),
                            "database" => matches!(m, "exec" | "insert" | "update" | "delete"),
                            _ => false,
                        },
                        _ => false,
                    });
            }
            !matches!(x.kind, ExprKind::Lambda { .. })
        });
        found
    }

    /// A statement for its effect: a write, a delete, a change to a local, or a call that does
    /// something. Under a condition, a write's value, a deleted key or a call's trigger is
    /// guarded by it, so nothing is done when it doesn't hold.
    fn effect(&mut self, e: &js::Expression, stmts: &mut Vec<Stmt>, start: usize) -> R<()> {
        let at = self.span(e.span());
        match self.unwrap(e) {
            js::Expression::AssignmentExpression(a) => self.assignment(a, stmts, start),
            js::Expression::UpdateExpression(u) => {
                let op = if u.operator == js::UpdateOperator::Increment { BinOp::Add } else { BinOp::Sub };
                let place = self.place(&u.argument)?;
                let old = self.read_place(&place, at)?;
                let one = Expr { kind: ExprKind::Number { value: 1.0, text: "1".into() }, span: at };
                self.write_place(place, binary(op, old, one), at, stmts, start)
            }
            js::Expression::UnaryExpression(u) if u.operator == js::UnaryOperator::Delete => self.delete(u, stmts),
            js::Expression::CallExpression(c) if let Some((ts, method)) = self.local_method(c) => self.mutate(c, &ts, &method, stmts),
            other => {
                let value = self.expr(other)?;
                let after = if self.contains_sink(&value) { self.trigger(at) } else { None };
                stmts.push(Stmt { kind: StmtKind::Expr(value), span: at, after });
                Ok(())
            }
        }
    }

    /// `target = value`, or `target op= value`.
    fn assignment(&mut self, a: &js::AssignmentExpression, stmts: &mut Vec<Stmt>, start: usize) -> R<()> {
        use js::AssignmentOperator as A;
        let at = self.span(a.span);
        // `%=`, `**=` and the bitwise ones: their operator's leaf.
        let number = a.operator.to_binary_operator().and_then(number_operator);
        let op = match a.operator {
            A::Assign => None,
            A::Addition => Some(BinOp::Add),
            A::Subtraction => Some(BinOp::Sub),
            A::Multiplication => Some(BinOp::Mul),
            A::Division => Some(BinOp::Div),
            A::LogicalNullish => Some(BinOp::Coalesce),
            A::LogicalOr => Some(BinOp::Or),
            A::LogicalAnd => Some(BinOp::And),
            _ if number.is_some() => None,
            op => return fail(at, format!("{} isn't an operator Holon has", op.as_str())),
        };
        if op == Some(BinOp::Add) && is_text(self.unwrap(&a.right)) {
            return fail(at, "there is no + on texts: use a template literal, `${a}${b}`");
        }
        let Some(target) = a.left.as_simple_assignment_target() else {
            return self.assign_pattern(&a.left, &a.right, stmts);
        };
        let place = self.place(target)?;
        let rhs = self.expr(&a.right)?;
        let value = match op {
            None if let Some((name, params)) = number => {
                let old = self.read_place(&place, at)?;
                leaf(name, params, vec![old, rhs], at)
            }
            None => {
                if self.share(&a.right)
                    && let Place::Local(ts, _) = &place
                {
                    let ts = ts.clone();
                    self.write_place(place, rhs, at, stmts, start)?;
                    self.share_name(&ts);
                    return Ok(());
                }
                rhs
            }
            Some(op) => {
                let old = self.read_place(&place, at)?;
                binary(op, old, rhs)
            }
        };
        self.write_place(place, value, at, stmts, start)
    }

    /// `[a, b] = [b, a]`, `({ total } = order)`: each name given its part, as a `let`'s new value.
    fn assign_pattern(&mut self, target: &js::AssignmentTarget, value: &js::Expression, stmts: &mut Vec<Stmt>) -> R<()> {
        let pattern = self.target_pattern(target)?;
        let v = self.expr(value)?;
        self.destructure_target(&pattern, v, stmts)
    }

    /// An assignment's pattern, its targets each a name: one that writes is refused (§12).
    fn target_pattern(&mut self, t: &js::AssignmentTarget) -> R<TargetPattern> {
        let at = self.span(t.span());
        let writes = || fail(at, "a pattern that writes is refused: bind the name, then write it");
        Ok(match t {
            js::AssignmentTarget::AssignmentTargetIdentifier(i) => TargetPattern::Name(i.name.to_string(), self.span(i.span)),
            js::AssignmentTarget::ArrayAssignmentTarget(a) => {
                let mut items = Vec::new();
                for el in &a.elements {
                    items.push(match el {
                        None => None,
                        Some(el) => Some(self.target_default(el)?),
                    });
                }
                let rest = match &a.rest {
                    Some(r) => Some(Box::new(self.target_pattern(&r.target)?)),
                    None => None,
                };
                TargetPattern::List(items, rest, at)
            }
            js::AssignmentTarget::ObjectAssignmentTarget(o) => {
                let mut fields = Vec::new();
                for p in &o.properties {
                    match p {
                        js::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(i) => {
                            let name = i.binding.name.to_string();
                            let span = self.span(i.binding.span);
                            let mut pattern = TargetPattern::Name(name.clone(), span);
                            if let Some(init) = &i.init {
                                pattern = TargetPattern::Default(Box::new(pattern), self.expr(init)?);
                            }
                            fields.push((name, span, pattern));
                        }
                        js::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            let key = self.property_key(&p.name, p.computed)?;
                            fields.push((key, self.span(p.name.span()), self.target_default(&p.binding)?));
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    return fail(self.span(r.span), "a rest in an assignment's pattern comes in a later slice: declare it, const { a, ...rest } = r");
                }
                TargetPattern::Record(fields, at)
            }
            _ => return writes(),
        })
    }

    fn target_default(&mut self, t: &js::AssignmentTargetMaybeDefault) -> R<TargetPattern> {
        match t {
            js::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                let inner = self.target_pattern(&d.binding)?;
                Ok(TargetPattern::Default(Box::new(inner), self.expr(&d.init)?))
            }
            other => match other.as_assignment_target() {
                Some(t) => self.target_pattern(t),
                None => fail(self.span(other.span()), "a pattern that writes is refused: bind the name, then write it"),
            },
        }
    }

    fn destructure_target(&mut self, p: &TargetPattern, value: Expr, stmts: &mut Vec<Stmt>) -> R<()> {
        match p {
            TargetPattern::Name(ts, span) => self.new_version(ts, *span, value, stmts, false),
            TargetPattern::Default(inner, fallback) => {
                let span = value.span;
                self.destructure_target(inner, binary_at(BinOp::Coalesce, value, fallback.clone(), span), stmts)
            }
            TargetPattern::Record(fields, at) => {
                let base = self.once(value, *at, stmts);
                for (key, span, pattern) in fields {
                    let part = Expr { kind: ExprKind::Field { base: Box::new(base.clone()), name: Name { text: key.clone(), span: *span } }, span: *span };
                    self.destructure_target(pattern, part, stmts)?;
                }
                Ok(())
            }
            TargetPattern::List(items, rest, at) => {
                if rest.is_none()
                    && let ExprKind::List(values) = &value.kind
                {
                    // `[a, b] = [b, a]`: no node, two new names, each value read before either is set.
                    let values = values.clone();
                    for (i, item) in items.iter().enumerate() {
                        if let Some(item) = item {
                            self.destructure_target(item, values.get(i).cloned().unwrap_or_else(|| absent(*at)), stmts)?;
                        }
                    }
                    return Ok(());
                }
                let base = self.once(value, *at, stmts);
                match rest {
                    None => {
                        for (i, item) in items.iter().enumerate() {
                            if let Some(item) = item {
                                let index = Expr { kind: ExprKind::Number { value: i as f64, text: i.to_string() }, span: *at };
                                self.destructure_target(item, call("get", vec![arg(base.clone()), arg(index)], *at), stmts)?;
                            }
                        }
                    }
                    Some(rest) => {
                        let mut list = base;
                        let n = items.len();
                        for (i, item) in items.iter().enumerate() {
                            if let Some(item) = item {
                                self.destructure_target(item, call("head", vec![arg(list.clone())], *at), stmts)?;
                            }
                            let tail = call("tail", vec![arg(list)], *at);
                            list = if i + 1 < n { self.once(tail, *at, stmts) } else { tail };
                        }
                        self.destructure_target(rest, list, stmts)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// What an assignment's target is.
    fn place(&mut self, t: &js::SimpleAssignmentTarget) -> R<Place> {
        let at = self.span(t.span());
        match t {
            js::SimpleAssignmentTarget::AssignmentTargetIdentifier(i) => Ok(Place::Local(i.name.to_string(), self.span(i.span))),
            js::SimpleAssignmentTarget::StaticMemberExpression(m) if self.is_state(&m.object) => Ok(Place::State(m.property.name.to_string(), self.span(m.property.span))),
            js::SimpleAssignmentTarget::ComputedMemberExpression(m) if self.is_state(&m.object) => fail(at, "a state key is a name: state.hits = …; a key computed is a collection's, notes[k] = …"),
            js::SimpleAssignmentTarget::ComputedMemberExpression(m) if let Some(collection) = self.store_name(&m.object) => {
                let key = self.expr(&m.expression)?;
                Ok(Place::Store(collection, self.span(m.object.span()), key))
            }
            js::SimpleAssignmentTarget::StaticMemberExpression(_) | js::SimpleAssignmentTarget::ComputedMemberExpression(_) => {
                let m = t.as_member_expression().expect("a member");
                self.member_place(m)
            }
            _ => fail(at, "only state, a collection, a folder or a local is written: state.k = v, notes[k] = v, x = v"),
        }
    }

    /// A field or an item of a local, as the target of a change in place: `order.total`,
    /// `counts[k]`, `a.b.c`.
    fn member_place(&mut self, m: &js::MemberExpression) -> R<Place> {
        let mut path = Vec::new();
        let mut m = m;
        loop {
            let object = match m {
                js::MemberExpression::StaticMemberExpression(s) => {
                    path.push(Seg::Field(s.property.name.to_string(), self.span(s.property.span)));
                    &s.object
                }
                js::MemberExpression::ComputedMemberExpression(c) => {
                    path.push(Seg::Key(self.expr(&c.expression)?));
                    &c.object
                }
                js::MemberExpression::PrivateFieldExpression(p) => return fail(self.span(p.span), "a class's private field: a flow has no classes"),
            };
            match self.unwrap(object) {
                js::Expression::Identifier(i) => {
                    return match self.lookup(i.name.as_str()) {
                        Some(Local::Bound(_) | Local::Var(Some(_))) => {
                            path.reverse();
                            Ok(Place::Member(i.name.to_string(), self.span(i.span), path))
                        }
                        Some(Local::Var(None)) => fail(self.span(i.span), format!("\"{}\" has no value to change yet", i.name)),
                        _ => fail(self.span(i.span), format!("\"{}\" isn't a local of this handler: only state, a collection, a folder or a local is written", i.name)),
                    };
                }
                other => match other.as_member_expression() {
                    Some(inner) => m = inner,
                    None => return fail(self.span(other.span()), "only a local's fields are changed in place: give the value a name first"),
                },
            }
        }
    }

    /// What `place` holds now, for `+=` and `++`.
    fn read_place(&mut self, place: &Place, at: Span) -> R<Expr> {
        match place {
            Place::Local(ts, span) => self.identifier(ts, *span),
            Place::State(key, _) => self.state_read(key, at),
            Place::Store(collection, span, key) => {
                self.check_unwritten(collection, at)?;
                Ok(Expr { kind: ExprKind::Index { base: Box::new(Expr { kind: ExprKind::Name(collection.clone()), span: *span }), key: Box::new(key.clone()) }, span: at })
            }
            Place::Member(root, span, path) => {
                let mut e = self.identifier(root, *span)?;
                for seg in path {
                    e = read_seg(e, seg, at);
                }
                Ok(e)
            }
        }
    }

    fn write_place(&mut self, place: Place, value: Expr, at: Span, stmts: &mut Vec<Stmt>, start: usize) -> R<()> {
        match place {
            Place::Local(ts, span) => self.new_version(&ts, span, value, stmts, false),
            Place::State(key, key_span) => self.state_write(key, value, at, key_span, stmts, start),
            Place::Store(collection, span, key) => {
                self.frame.written.insert(collection.clone());
                let value = self.guarded(value);
                stmts.push(Stmt { kind: StmtKind::CollectionWrite { collection: Name { text: collection, span }, key, value }, span: at, after: self.after_value() });
                Ok(())
            }
            Place::Member(root, span, path) => {
                self.check_unshared(&root, at)?;
                let base = self.identifier(&root, span)?;
                let changed = rebuilt(base, &path, Edit::Set(value), at);
                self.new_version(&root, span, changed, stmts, true)
            }
        }
    }

    /// `delete notes[k]`, or `delete order.note` on a local.
    fn delete(&mut self, u: &js::UnaryExpression, stmts: &mut Vec<Stmt>) -> R<()> {
        let at = self.span(u.span);
        let Some(m) = self.unwrap(&u.argument).as_member_expression() else {
            return fail(at, "delete takes an item of a collection or folder, or a field of a local: delete notes[k]");
        };
        if let js::MemberExpression::ComputedMemberExpression(c) = m
            && let Some(collection) = self.store_name(&c.object)
        {
            let key = self.expr(&c.expression)?;
            let key = self.guarded(key);
            self.frame.written.insert(collection.clone());
            let collection = self.name(&collection, c.object.span());
            stmts.push(Stmt { kind: StmtKind::Delete { collection, key }, span: at, after: self.after_value() });
            return Ok(());
        }
        let Place::Member(root, span, mut path) = self.member_place(m)? else { unreachable!("a member's place") };
        self.check_unshared(&root, at)?;
        let base = self.identifier(&root, span)?;
        let last = path.pop().expect("a member has a field");
        let changed = rebuilt(base, &path, Edit::Remove(last), at);
        self.new_version(&root, span, changed, stmts, true)
    }

    /// `xs.push(…)` and the like, on a local: its name and the method's.
    fn local_method(&self, c: &js::CallExpression) -> Option<(String, String)> {
        let js::Expression::StaticMemberExpression(m) = self.unwrap(&c.callee) else { return None };
        let js::Expression::Identifier(i) = self.unwrap(&m.object) else { return None };
        let method = m.property.name.as_str();
        (MUTATORS.contains(&method) && matches!(self.lookup(i.name.as_str()), Some(Local::Bound(_) | Local::Var(_)))).then(|| (i.name.to_string(), method.to_string()))
    }

    /// `xs.push(a, b)`: `xs` is now `append(append(xs, a), b)` (§5).
    fn mutate(&mut self, c: &js::CallExpression, ts: &str, method: &str, stmts: &mut Vec<Stmt>) -> R<()> {
        let at = self.span(c.span);
        if method != "push" {
            return fail(at, format!("{method} changes the list in place, which comes in a later slice: write the new list, {ts} = …"));
        }
        self.check_unshared(ts, at)?;
        let mut list = self.identifier(ts, at)?;
        for item in self.positional(&c.arguments)? {
            list = call("append", vec![arg(list), item], at);
        }
        self.new_version(ts, at, list, stmts, true)
    }

    /// A state key's value read: what it held when the run began, as a write is made when the
    /// run ends (§6).
    fn state_read(&mut self, key: &str, at: Span) -> R<Expr> {
        self.state_keys.insert(key.to_string());
        Ok(Expr { kind: ExprKind::State(key.to_string()), span: at })
    }

    fn check_unwritten(&self, collection: &str, at: Span) -> R<()> {
        if self.frame.written.contains(collection) {
            return fail(at, format!("{collection} is read after this run writes to it, and a write is made when the run ends: use the value written"));
        }
        Ok(())
    }

    /// A local changed in place must be its own: one another name, a record or a write holds
    /// too would change behind them (§12).
    fn check_unshared(&self, ts: &str, at: Span) -> R<()> {
        if self.frame.params.contains(ts) {
            return fail(at, format!("\"{ts}\" is a parameter, which a change in place would change for whoever passed it too: copy it first, const copy = {{ ...{ts} }}"));
        }
        let held = match self.lookup(ts) {
            Some(Local::Bound(n) | Local::Var(Some(n))) => self.frame.shared.contains(&n),
            _ => false,
        };
        if held {
            return fail(at, format!("\"{ts}\" is held elsewhere too (by another name, a record or a write), which a change in place would change behind it: copy it first, const copy = [...{ts}] or {{ ...{ts} }}"));
        }
        Ok(())
    }

    /// Whether `e` is a local named as it is (`b = a`, `{ items }`, `state.k = xs`), which then
    /// holds what another does: it is marked so, and `true` returned.
    fn share(&mut self, e: &js::Expression) -> bool {
        match self.unwrap(e) {
            js::Expression::Identifier(i) => self.share_name(i.name.as_str()),
            _ => false,
        }
    }

    fn share_name(&mut self, ts: &str) -> bool {
        match self.lookup(ts) {
            Some(Local::Bound(n) | Local::Var(Some(n))) => {
                self.frame.shared.insert(n);
                true
            }
            _ => false,
        }
    }

    /// `state.k = v`: what the key holds is `v` where this point is reached and what it held
    /// otherwise, written once, where its value is last settled (§5).
    fn state_write(&mut self, key: String, value: Expr, at: Span, key_span: Span, stmts: &mut Vec<Stmt>, start: usize) -> R<()> {
        self.state_keys.insert(key.clone());
        let twice = || fail(at, format!("state.{key} is written twice, in an if (… !== undefined) and elsewhere: a run writes a key once"));
        if let Some(after) = self.after_value() {
            if self.frame.writes.iter().any(|(k, ..)| *k == key) || self.frame.tied.contains(&key) {
                return twice();
            }
            let value = self.guarded(value);
            self.frame.tied.insert(key.clone());
            stmts.push(Stmt { kind: StmtKind::StateWrite { key: Name { text: key, span: key_span }, value }, span: at, after: Some(after) });
            return Ok(());
        }
        if self.frame.tied.contains(&key) {
            return twice();
        }
        self.update_write(key, value, at, key_span, stmts, start);
        Ok(())
    }

    fn update_write(&mut self, key: String, value: Expr, at: Span, key_span: Span, stmts: &mut Vec<Stmt>, start: usize) {
        let i = self.frame.writes.iter().position(|(k, ..)| *k == key);
        let old = i.map(|i| self.frame.writes[i].1.clone());
        let value = match conj(self.frame.guards[start..].iter().filter_map(|g| if let Guard::Live(c) = g { Some(c.clone()) } else { None })) {
            Some(live) => choose(&Choice::If(live), vec![Some(value)], old, at).expect("a value"),
            None => value,
        };
        match i {
            Some(i) => self.frame.writes[i] = (key.clone(), value.clone(), key_span),
            None => self.frame.writes.push((key.clone(), value.clone(), key_span)),
        }
        if self.frame.depth == 0 {
            stmts.retain(|s| !matches!(&s.kind, StmtKind::StateWrite { key: k, .. } if k.text == key));
            stmts.push(Stmt { kind: StmtKind::StateWrite { key: Name { text: key, span: key_span }, value }, span: at, after: None });
        }
    }

    /// Every branch's condition and every `return` not taken, that the statement being read
    /// runs under.
    fn live(&self) -> Option<Expr> {
        conj(self.frame.guards.iter().filter_map(|g| match g {
            Guard::Branch(c) | Guard::Live(c) => Some(c.clone()),
            Guard::After(_) => None,
        }))
    }

    /// What an `if (x !== undefined)` around the statement waits for.
    fn after_value(&self) -> Option<Expr> {
        self.frame.guards.iter().find_map(|g| if let Guard::After(x) = g { Some(x.clone()) } else { None })
    }

    /// A write's value under the conditions it runs under: absent, so not written, when they
    /// don't hold.
    fn guarded(&self, value: Expr) -> Expr {
        match self.live() {
            Some(c) => when(value, c),
            None => value,
        }
    }

    /// What a call's trigger is tied to: what it waits for, guarded by the conditions it runs
    /// under, so it isn't made when they don't hold.
    fn trigger(&self, at: Span) -> Option<Expr> {
        match (self.after_value(), self.live()) {
            (Some(x), Some(c)) => Some(when(x, c)),
            (Some(x), None) => Some(x),
            (None, Some(c)) => Some(when(Expr { kind: ExprKind::Bool(true), span: at }, c)),
            (None, None) => None,
        }
    }

    /// `if (c) … else …`: each branch read under its condition, then each name and state key
    /// they change holds `if c then a else b` (§5).
    fn if_statement(&mut self, f: &js::IfStatement, block: Block, stmts: &mut Vec<Stmt>, start: usize) -> R<Ret> {
        let at = self.span(f.span);
        if let Some(x) = self.present(&f.test)? {
            return self.after_block(f, x, block, stmts, at);
        }
        let cond = self.expr(&f.test)?;
        let cond = self.hide(cond, "cond", stmts);
        let branches = vec![(Guard::Branch(cond.clone()), Some(statements_of(&f.consequent))), (Guard::Branch(otherwise(&cond)), f.alternate.as_ref().map(statements_of))];
        let mut paths = self.branches(branches, block, stmts)?;
        let b = paths.pop().expect("an else");
        let a = paths.pop().expect("a then");
        let ret = ret_if(&cond, &a.ret, &b.ret);
        self.join(&Choice::If(cond), vec![a], b, stmts, start, at);
        Ok(ret)
    }

    /// `x !== undefined`, as an `if` tests it to wait for `x` (§5): `x`.
    fn present(&mut self, test: &js::Expression) -> R<Option<Expr>> {
        let js::Expression::BinaryExpression(b) = self.unwrap(test) else { return Ok(None) };
        if !matches!(b.operator, js::BinaryOperator::StrictInequality | js::BinaryOperator::Inequality) {
            return Ok(None);
        }
        let undefined = |e: &js::Expression| matches!(self.unwrap(e), js::Expression::Identifier(i) if i.name == "undefined" && self.lookup("undefined").is_none());
        let x = if undefined(&b.right) {
            &b.left
        } else if undefined(&b.left) {
            &b.right
        } else {
            return Ok(None);
        };
        Ok(Some(self.expr(x)?))
    }

    /// `if (x !== undefined) { … }`: the writes and calls inside are tied to `x` (`after x`), so
    /// they are made once it has a value, and not at all when it has none (§5). A `let` given its
    /// first value inside is tied to it too: `let y; if (x !== undefined) y = f()`.
    fn after_block(&mut self, f: &js::IfStatement, x: Expr, block: Block, stmts: &mut Vec<Stmt>, at: Span) -> R<Ret> {
        if let Some(alt) = &f.alternate {
            return fail(self.span(alt.span()), "an if (x !== undefined) has no else, for now: what is done without x is written after it, with ??");
        }
        if self.after_value().is_some() {
            return fail(at, "an if (… !== undefined) inside another comes in a later slice: tie the inner one's writes to the outer value");
        }
        let x = self.hide(x, "cond", stmts);
        let before = self.scopes.clone();
        let from = stmts.len();
        self.frame.guards.push(Guard::After(x.clone()));
        self.scopes.push(HashMap::new());
        let r = self.list(statements_of(&f.consequent), block, stmts);
        self.scopes.pop();
        self.frame.guards.pop();
        r?;
        for (d, scope) in before.iter().enumerate() {
            for (ts, local) in scope {
                let now = self.scopes[d].get(ts).and_then(version);
                if version(local) == now {
                    continue;
                }
                // A `let` with no value before, given one inside: its value is tied to x (the
                // tree's `after`), and absent when x is.
                if let (Local::Var(None), Some(Some(name))) = (local, &now)
                    && let Some(s) = stmts[from..].iter_mut().rev().find(|s| matches!(&s.kind, StmtKind::Bind { name: n, .. } if n.text == *name))
                {
                    s.after = Some(x.clone());
                    continue;
                }
                return fail(at, format!("\"{ts}\" had a value before if (… !== undefined) and is given another inside, which comes in a later slice: give it its value after it, with ??"));
            }
        }
        Ok(Ret::Never)
    }

    /// Each branch read from what holds before them, under its condition: what each leaves.
    fn branches(&mut self, branches: Vec<(Guard, Option<&[js::Statement]>)>, block: Block, stmts: &mut Vec<Stmt>) -> R<Vec<Path>> {
        let scopes = self.scopes.clone();
        let writes = self.frame.writes.clone();
        let written = self.frame.written.clone();
        let mut all_written = written.clone();
        self.frame.depth += 1;
        let mut paths = Vec::new();
        for (guard, body) in branches {
            self.frame.written = written.clone();
            let exits = self.frame.exits.len();
            self.frame.guards.push(guard);
            self.scopes.push(HashMap::new());
            let ret = match body {
                Some(b) => self.list(b, block, stmts),
                None => Ok(Ret::Never),
            };
            self.scopes.pop();
            self.frame.guards.pop();
            let ret = ret?;
            all_written.extend(self.frame.written.iter().cloned());
            let exits = self.frame.exits.len() - exits;
            paths.push(Path { scopes: std::mem::replace(&mut self.scopes, scopes.clone()), writes: std::mem::replace(&mut self.frame.writes, writes.clone()), ret, exits });
        }
        self.frame.depth -= 1;
        self.frame.written = all_written;
        Ok(paths)
    }

    /// After branches (`arms`, then `els`): each name and state key one of them changed holds
    /// what `choice` picks among them. A branch that returned leaves no name behind, but its
    /// writes were made.
    fn join(&mut self, choice: &Choice, arms: Vec<Path>, els: Path, stmts: &mut Vec<Stmt>, start: usize, at: Span) {
        let mut paths = arms;
        paths.push(els);
        let goes_on: Vec<bool> = paths.iter().map(|p| !matches!(p.ret, Ret::Always)).collect();
        if goes_on.iter().any(|g| *g) {
            for d in 0..self.scopes.len() {
                let mut names: Vec<String> = self.scopes[d].keys().cloned().collect();
                names.sort();
                for ts in names {
                    let before = self.scopes[d][&ts].clone();
                    let Some(v0) = version(&before) else { continue };
                    let vs: Vec<Option<String>> = paths.iter().map(|p| p.scopes[d].get(&ts).and_then(version).flatten()).collect();
                    let going: Vec<Option<String>> = vs.iter().zip(&goes_on).filter(|(_, g)| **g).map(|(v, _)| v.clone()).collect();
                    if going.iter().all(|v| *v == v0) {
                        continue;
                    }
                    let now = if going.iter().all(|v| *v == going[0]) {
                        going[0].clone()
                    } else {
                        // A branch that returned picks what the last that goes on has: it is never read.
                        let fallback = going.last().cloned().flatten();
                        let named = |v: &Option<String>| v.as_ref().map(|n| Expr { kind: ExprKind::Name(n.clone()), span: at });
                        let mut picks: Vec<Option<Expr>> = vs.iter().zip(&goes_on).map(|(v, g)| if *g { named(v) } else { named(&fallback) }).collect();
                        let els = picks.pop().expect("an else");
                        match choose(choice, picks, els, at) {
                            Some(value) => {
                                let name = self.fresh(&ts);
                                stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span: at }, value }, span: at, after: None });
                                Some(name)
                            }
                            None => None,
                        }
                    };
                    let local = match (&before, now) {
                        (Local::Bound(_), Some(n)) => Local::Bound(n),
                        (_, now) => Local::Var(now),
                    };
                    self.scopes[d].insert(ts, local);
                }
            }
        }
        let mut keys: Vec<(String, Span)> = Vec::new();
        for p in &paths {
            for (k, _, s) in &p.writes {
                if !keys.iter().any(|(x, _)| x == k) {
                    keys.push((k.clone(), *s));
                }
            }
        }
        for (key, key_span) in keys {
            let old = self.frame.writes.iter().find(|(k, ..)| *k == key).map(|(_, v, _)| v.clone());
            let mut picks: Vec<Option<Expr>> = paths.iter().map(|p| p.writes.iter().find(|(k, ..)| *k == key).map(|(_, v, _)| v.clone())).collect();
            if picks.iter().all(|v| *v == old) {
                continue;
            }
            let els = picks.pop().expect("an else");
            if let Some(value) = choose(choice, picks, els, at) {
                self.update_write(key, value, at, key_span, stmts, start);
            }
        }
    }

    /// `switch (x) { case …: … }` as `match` (§5): each case's names, writes and answer chosen by
    /// `x`. A case ends with `break` or `return`; one that runs on into the next is refused.
    fn switch(&mut self, w: &js::SwitchStatement, block: Block, stmts: &mut Vec<Stmt>, start: usize) -> R<Ret> {
        let at = self.span(w.span);
        let subject = self.expr(&w.discriminant)?;
        let subject = self.hide(subject, "subject", stmts);
        let mut cases: Vec<Case> = Vec::new();
        let (mut patterns, mut default) = (Vec::new(), false);
        for (i, c) in w.cases.iter().enumerate() {
            match &c.test {
                Some(t) => patterns.push(self.pattern(t)?),
                None => default = true,
            }
            // `case "a": case "b": …` is one case of two patterns.
            if c.consequent.is_empty() && i + 1 < w.cases.len() {
                continue;
            }
            let (body, broke) = case_body(&c.consequent);
            cases.push(Case { patterns: std::mem::take(&mut patterns), default: std::mem::take(&mut default), body, broke, span: self.span(c.span) });
        }
        let all: Vec<Expr> = cases.iter().filter(|c| !c.default).flat_map(|c| c.patterns.clone()).collect();
        if all.is_empty() {
            return fail(at, "a switch with a default alone is its statements: write them without it");
        }
        let branches = cases.iter().map(|c| (Guard::Branch(if c.default { otherwise(&matched(&subject, &all, at)) } else { matched(&subject, &c.patterns, at) }), Some(c.body))).collect();
        let exits_before = self.frame.exits.len();
        let paths = self.branches(branches, block, stmts)?;
        for (i, (c, p)) in cases.iter().zip(&paths).enumerate() {
            let returned = matches!(p.ret, Ret::Always);
            if !returned && !c.broke && i + 1 < cases.len() {
                return fail(c.span, "a case runs on into the next: end it with break or return");
            }
            if p.exits != usize::from(returned) {
                return fail(c.span, "a case returns once, last in it, for now: an early return inside a case comes in a later slice");
            }
        }
        let mut answers = self.frame.exits.split_off(exits_before).into_iter();
        let (mut arms, mut default_answer, mut returning, mut breaking) = (Vec::new(), None, Vec::new(), Vec::new());
        let mut default_returns = false;
        for (c, p) in cases.iter().zip(&paths) {
            let answer = if p.exits == 1 {
                match answers.next().map(|e| e.kind) {
                    Some(ExitKind::One(a)) => Some(a),
                    _ => unreachable!("a case's return is one"),
                }
            } else {
                None
            };
            if c.default {
                default_returns = answer.is_some();
                default_answer = answer;
            } else {
                if answer.is_some() { returning.extend(c.patterns.clone()) } else { breaking.extend(c.patterns.clone()) }
                arms.push((c.patterns.clone(), answer));
            }
        }
        let ret = if returning.is_empty() && !default_returns {
            Ret::Never
        } else if default_returns {
            if breaking.is_empty() { Ret::Always } else { Ret::When(otherwise(&matched(&subject, &breaking, at))) }
        } else {
            Ret::When(matched(&subject, &returning, at))
        };
        if !matches!(ret, Ret::Never) {
            self.push_exit(ExitKind::Switch { subject: subject.clone(), arms, default: default_answer }, at)?;
        }
        let (mut case_paths, mut case_patterns, mut default_path) = (Vec::new(), Vec::new(), None);
        for (c, p) in cases.into_iter().zip(paths) {
            if c.default {
                default_path = Some(p);
            } else {
                case_patterns.push(c.patterns);
                case_paths.push(p);
            }
        }
        let els = default_path.unwrap_or_else(|| Path { scopes: self.scopes.clone(), writes: self.frame.writes.clone(), ret: Ret::Never, exits: 0 });
        self.join(&Choice::Match(subject, case_patterns), case_paths, els, stmts, start, at);
        Ok(ret)
    }

    /// A case's value: a literal, as a `match` arm's pattern is.
    fn pattern(&mut self, t: &js::Expression) -> R<Expr> {
        let p = self.expr(t)?;
        match p.kind {
            ExprKind::Number { .. } | ExprKind::Text(_) | ExprKind::Bool(_) => Ok(p),
            _ => fail(p.span, "a case is a literal: a number, a text, true or false; to test a condition, use if"),
        }
    }

    // ── returns ──────────────────────────────────────────────────────────────────────────────

    /// `return e`: what it answers, under the branches it is in.
    fn exit(&mut self, e: &js::Expression, block: Block) -> R<()> {
        let at = self.span(e.span());
        let answer = match block {
            Block::Route => Answer::Response(Box::new(self.response(e)?)),
            Block::Action => {
                if self.answers_with(e).is_some() {
                    return fail(at, "an action answers with a value alone: return it, without status or respond");
                }
                Answer::Value { value: self.expr(e)?, name: None, fields: None }
            }
            Block::Function => self.returned(e)?,
            Block::Every | Block::File => return fail(at, format!("a {}'s run answers nobody: return alone ends it", block_word(block))),
        };
        self.push_exit(ExitKind::One(answer), at)
    }

    fn bare_exit(&mut self, block: Block, at: Span) -> R<()> {
        match block {
            Block::Route => fail(at, "a route answers: return status(204) for an answer with no body"),
            Block::Function => fail(at, "a function returns a value"),
            _ => self.push_exit(ExitKind::One(Answer::Nothing), at),
        }
    }

    fn push_exit(&mut self, kind: ExitKind, span: Span) -> R<()> {
        if self.after_value().is_some() {
            return fail(span, "a return inside if (x !== undefined) comes in a later slice: return after it, with ?? for when x has no value");
        }
        let cond = conj(self.frame.guards.iter().filter_map(|g| if let Guard::Branch(c) = g { Some(c.clone()) } else { None }));
        self.frame.exits.push(Exit { cond, kind, span });
        Ok(())
    }

    /// A function's `return e`: its value, with the binding it names or the object's fields,
    /// which name the function's outputs.
    fn returned(&mut self, e: &js::Expression) -> R<Answer> {
        let name = match self.unwrap(e) {
            js::Expression::Identifier(i) => match self.lookup(i.name.as_str()) {
                Some(Local::Bound(n) | Local::Var(Some(n))) => Some((i.name.to_string(), n)),
                _ => None,
            },
            _ => None,
        };
        let fields = match self.unwrap(e) {
            js::Expression::ObjectExpression(o) if !o.properties.is_empty() && o.properties.iter().all(|p| matches!(p, js::ObjectPropertyKind::ObjectProperty(p) if !p.computed)) => {
                let mut out = Vec::new();
                for (key, value, span) in self.fields(o)? {
                    out.push((key, self.expr(value)?, span));
                }
                Some(out)
            }
            _ => None,
        };
        Ok(Answer::Value { value: self.expr(e)?, name, fields })
    }

    /// The returns, last to first, folded into one answer (`None` where none answers). The last
    /// answers whenever the ones before it didn't, if the body ends with a return on every path
    /// (`ended`).
    fn fold(&mut self, exits: Vec<Exit>, ended: bool) -> R<Option<Answer>> {
        let mut acc: Option<Answer> = None;
        let n = exits.len();
        for (i, exit) in exits.into_iter().enumerate().rev() {
            let here = match exit.kind {
                ExitKind::One(a) => Some(a),
                ExitKind::Switch { subject, arms, default } => {
                    // A case that breaks answers what follows the switch, as no case does.
                    let rest = acc.clone();
                    let els = default.or_else(|| rest.clone());
                    let patterns = arms.iter().map(|(p, _)| p.clone()).collect();
                    let answers = arms.into_iter().map(|(_, a)| a.or_else(|| rest.clone())).collect();
                    self.merge(&Choice::Match(subject, patterns), answers, els, exit.span)?
                }
            };
            acc = match exit.cond {
                Some(c) if !(ended && i + 1 == n) => self.merge(&Choice::If(c), vec![here], acc, exit.span)?,
                _ => here,
            };
        }
        Ok(acc)
    }

    /// One answer choosing among `arms` by `choice`, else `els`: `None` where none answers.
    fn merge(&mut self, choice: &Choice, arms: Vec<Option<Answer>>, els: Option<Answer>, span: Span) -> R<Option<Answer>> {
        let mut sides = arms;
        sides.push(els);
        let sides: Vec<Option<Answer>> = sides.into_iter().map(|a| a.filter(|a| !matches!(a, Answer::Nothing))).collect();
        match sides.iter().flatten().next() {
            None => Ok(None),
            Some(Answer::Response(_)) => {
                let responses = sides
                    .into_iter()
                    .map(|a| {
                        a.map(|a| match a {
                            Answer::Response(r) => *r,
                            _ => unreachable!("a route's returns are responses"),
                        })
                    })
                    .collect();
                Ok(self.merge_responses(choice, responses, span)?.map(|r| Answer::Response(Box::new(r))))
            }
            Some(_) => {
                let parts: Vec<Option<(Expr, Option<Fields>)>> = sides
                    .into_iter()
                    .map(|a| {
                        a.map(|a| match a {
                            Answer::Value { value, fields, .. } => (value, fields),
                            _ => unreachable!("a function's and an action's returns are values"),
                        })
                    })
                    .collect();
                // Every side that answers an object of the same keys: each key chosen alone.
                let keys: Option<Vec<String>> = parts.iter().flatten().next().and_then(|(_, f)| f.as_ref()).map(|f| f.iter().map(|(k, ..)| k.clone()).collect());
                let keys = keys.filter(|keys| {
                    parts.iter().flatten().all(|(_, f)| f.as_ref().is_some_and(|f| f.len() == keys.len() && f.iter().all(|(k, ..)| keys.contains(k))))
                });
                let fields = keys.map(|keys| {
                    keys.iter()
                        .map(|k| {
                            let mut picks: Vec<Option<Expr>> = parts.iter().map(|p| p.as_ref().and_then(|(_, f)| f.as_ref()).and_then(|f| f.iter().find(|(x, ..)| x == k)).map(|(_, v, _)| v.clone())).collect();
                            let els = picks.pop().expect("an else");
                            (k.clone(), choose(choice, picks, els, span).expect("a side answers"), span)
                        })
                        .collect()
                });
                let mut values: Vec<Option<Expr>> = parts.into_iter().map(|p| p.map(|(v, _)| v)).collect();
                let els = values.pop().expect("an else");
                Ok(Some(Answer::Value { value: choose(choice, values, els, span).expect("a side answers"), name: None, fields }))
            }
        }
    }

    /// One route answer choosing among several, as one `respond` gives it: its body chosen, its
    /// status chosen when they differ, and one `missing:` for an absent body. A `status(s)` alone
    /// is that absent body where `s` is what an absent body answers anyway; elsewhere it answers
    /// `s` with an empty body. A route that ends without an answer answers 404.
    fn merge_responses(&mut self, choice: &Choice, sides: Vec<Option<Response>>, span: Span) -> R<Option<Response>> {
        if sides.iter().all(Option::is_none) {
            return Ok(None);
        }
        if let Some(r) = sides.iter().flatten().find(|r| r.stream.is_some()) {
            return fail(r.span, "a streamed answer is a route's only one: return it, not one of several");
        }
        let number = |text: &str| Expr { kind: ExprKind::Number { value: text.parse().unwrap_or(0.0), text: text.into() }, span };
        let status_text = |s: &Option<Expr>| match s.as_ref().map(|e| &e.kind) {
            None => Some("200".to_string()),
            Some(ExprKind::Number { text, .. }) => Some(text.clone()),
            _ => None,
        };
        let last = sides.len() - 1;
        if sides.iter().flatten().all(|r| r.body.is_none()) {
            // Statuses alone: the one chosen, 404 where the route ends unanswered.
            let mut statuses: Vec<Option<Expr>> = sides.into_iter().map(|r| Some(r.and_then(|r| r.status).unwrap_or_else(|| number("404")))).collect();
            let els = statuses.pop().expect("an else");
            return Ok(Some(Response { body: None, status: choose(choice, statuses, els, span), missing: None, headers: None, stream: None, span }));
        }
        let mut missing: Option<(String, Span)> = None;
        for r in sides.iter().flatten().filter(|r| r.body.is_some()) {
            let m = match r.missing.as_ref().map(|e| &e.kind) {
                Some(ExprKind::Number { text, .. }) => text.clone(),
                _ => "404".into(),
            };
            if let Some((first, at)) = &missing
                && *first != m
            {
                return fail(r.span, format!("this answer is {m} when its body is absent, and the one on line {} is {first}: say it the same for both", at.start.line));
            }
            missing.get_or_insert((m, r.span));
        }
        let missing = missing.map(|(m, _)| m).expect("a body");
        let any_headers = sides.iter().flatten().any(|r| r.headers.is_some());
        let (mut bodies, mut statuses, mut headers) = (Vec::new(), Vec::new(), Vec::new());
        for (i, side) in sides.into_iter().enumerate() {
            // `if`'s sides and a match's else may be the absent body; a match's arm can't.
            let may_be_absent = matches!(choice, Choice::If(_)) || i == last;
            let (body, status, h) = match side {
                None if missing == "404" => (None, None, None),
                None => (Some(text_expr("", span)), Some(number("404")), None),
                Some(r) if r.body.is_none() && may_be_absent && status_text(&r.status).as_deref() == Some(missing.as_str()) => (None, None, None),
                Some(r) if r.body.is_none() => (Some(text_expr("", r.span)), r.status, None),
                Some(r) => (r.body, Some(r.status.unwrap_or_else(|| number("200"))), r.headers),
            };
            bodies.push(body);
            statuses.push(status);
            headers.push(h);
        }
        let first = statuses.iter().flatten().next().cloned().expect("a body");
        let status = if statuses.iter().flatten().all(|s| same(s, &first)) {
            Some(first).filter(|s| status_text(&Some(s.clone())).as_deref() != Some("200"))
        } else {
            let mut picks: Vec<Option<Expr>> = statuses.into_iter().map(|s| Some(s.unwrap_or_else(|| first.clone()))).collect();
            let els = picks.pop().expect("an else");
            choose(choice, picks, els, span)
        };
        let headers = if any_headers {
            let empty = Expr { kind: ExprKind::Record { spread: None, fields: vec![] }, span };
            let mut picks: Vec<Option<Expr>> = headers.into_iter().map(|h| Some(h.unwrap_or_else(|| empty.clone()))).collect();
            let els = picks.pop().expect("an else");
            choose(choice, picks, els, span)
        } else {
            None
        };
        let els = bodies.pop().expect("an else");
        let body = choose(choice, bodies, els, span);
        Ok(Some(Response { body, status, missing: (missing != "404").then(|| number(&missing)), headers, stream: None, span }))
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

    /// A route's `return e` as its response: `return v` answers 200 with it, `status(code, body)`
    /// that code, `status(code)` the code alone, `respond(body, { headers, stream, status })` the
    /// rest, `… ?? status(404)` what it answers when the body is absent.
    fn response(&mut self, e: &js::Expression) -> R<Response> {
        let span = self.span(e.span());
        let mut r = Response { body: None, status: None, missing: None, headers: None, stream: None, span };
        // A long answer broken in parentheses, as fmt breaks one, `return (… ?? status(400))`: the
        // parentheses change nothing.
        let mut e = e;
        while let js::Expression::ParenthesizedExpression(p) = e {
            e = &p.expression;
        }
        match self.unwrap(e) {
            js::Expression::LogicalExpression(l) if l.operator == js::LogicalOperator::Coalesce && self.answers_with(&l.right).is_some() => {
                let Some((Vocab::Status, c)) = self.answers_with(&l.right) else {
                    return fail(self.span(l.right.span()), "after ?? comes the status for an absent answer: ?? status(404)");
                };
                let [code] = c.arguments.as_slice() else {
                    return fail(self.span(c.span), "after ?? comes a status alone: ?? status(404)");
                };
                let code = self.expr(code.as_expression().ok_or(()).or_else(|_| fail(span, "a status is a number"))?)?;
                if !matches!(code.kind, ExprKind::Number { .. }) {
                    return fail(code.span, "what an absent answer is, is a status written out: ?? status(404)");
                }
                let mut left = self.response(&l.left)?;
                if left.missing.is_some() {
                    return fail(span, "one ?? status(…) says what an absent answer is");
                }
                left.missing = Some(code);
                left.span = span;
                Ok(left)
            }
            js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Status) => {
                let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
                match args.as_slice() {
                    [code] => r.status = Some(self.expr(code)?),
                    [code, body] => {
                        r.status = Some(self.expr(code)?);
                        r.body = Some(self.expr(body)?);
                    }
                    _ => return fail(span, "status takes the code, then the body: status(201, body)"),
                }
                Ok(r)
            }
            js::Expression::CallExpression(c) if self.vocab_of(&c.callee) == Some(Vocab::Respond) => {
                let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
                let (body, options) = match args.as_slice() {
                    [body] => (body, None),
                    [body, options] => (body, Some(*options)),
                    _ => return fail(span, "respond takes the body, then its options: respond(body, { headers, stream })"),
                };
                r.body = Some(self.expr(body)?);
                if let Some(o) = options {
                    let js::Expression::ObjectExpression(o) = self.unwrap(o) else {
                        return fail(self.span(o.span()), "respond's options are an object: { headers: { … } }");
                    };
                    for (key, value, at) in self.fields(o)? {
                        let value = self.expr(value)?;
                        match key.as_str() {
                            "headers" => r.headers = Some(value),
                            "stream" => r.stream = Some(value),
                            "status" => r.status = Some(value),
                            _ => return fail(at, format!("respond's options are headers, stream and status, not \"{key}\"")),
                        }
                    }
                }
                Ok(r)
            }
            _ => {
                r.body = Some(self.expr(e)?);
                Ok(r)
            }
        }
    }

    // ── functions ────────────────────────────────────────────────────────────────────────────

    /// `function name(params) { …; return slug }`: a graph holon, its output named after what it
    /// returns (a name, or each key of a returned object), else `out`.
    fn function(&mut self, name: &str, name_span: oxc_span::Span, f: &js::Function) -> R<HolonDef> {
        let at = self.span(f.span);
        if f.generator || f.r#async {
            return fail(at, "a function computes: no async, no generator");
        }
        if f.type_parameters.is_some() {
            return fail(at, "a type parameter is refused for now: a port has one type");
        }
        let Some(body) = &f.body else { return fail(at, "a function needs a body") };
        let inputs = self.ports(&f.params, false)?;
        let returns = f.return_type.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
        self.holon_def(name, name_span, inputs, returns, at, |me, stmts| me.function_statements(&body.statements, stmts, at))
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
        let inputs = self.ports(&a.params, false)?;
        let returns = a.return_type.as_ref().map(|t| self.type_expr(&t.type_annotation)).transpose()?;
        self.holon_def(name, name_span, inputs, returns, at, |me, stmts| match &a.body {
            js::ArrowFunctionBody::FunctionBody(b) => me.function_statements(&b.statements, stmts, at),
            body => {
                let e = body.as_expression().expect("an expression body");
                me.exit(e, Block::Function)?;
                let outputs = me.outputs(Ret::Always, stmts, at)?;
                me.settle(stmts, &outputs);
                Ok(outputs)
            }
        })
    }

    fn holon_def(
        &mut self,
        name: &str,
        name_span: oxc_span::Span,
        inputs: Vec<(String, PortDecl)>,
        returns: Option<TypeExpr>,
        at: Span,
        body: impl FnOnce(&mut Self, &mut Vec<Stmt>) -> R<Vec<String>>,
    ) -> R<HolonDef> {
        let saved = std::mem::take(&mut self.frame);
        self.frame.used.extend(inputs.iter().map(|(_, port)| port.name.text.clone()));
        self.frame.params.extend(inputs.iter().map(|(param, _)| param.clone()));
        // Each parameter reads, in the body, as the port it is.
        self.scopes.push(inputs.iter().map(|(param, port)| (param.clone(), Local::Bound(port.name.text.clone()))).collect());
        let inputs: Vec<PortDecl> = inputs.into_iter().map(|(_, port)| port).collect();
        let mut stmts = Vec::new();
        let outputs = body(self, &mut stmts);
        self.scopes.pop();
        self.frame = saved;
        let outputs = outputs?;
        if returns.is_some() && outputs.len() != 1 {
            return fail(at, "a return type names one output's type: a function returning an object has one output per key");
        }
        let outputs = outputs.into_iter().map(|o| PortDecl { name: Name { text: o, span: at }, ty: returns.clone(), unit: None }).collect();
        Ok(HolonDef { name: self.name(name, name_span), inputs, outputs, body: stmts, span: at })
    }

    /// A function body: its statements, then its outputs, from what it returns.
    fn function_statements(&mut self, body: &[js::Statement], stmts: &mut Vec<Stmt>, at: Span) -> R<Vec<String>> {
        let ret = self.top(body, Block::Function, stmts)?;
        let outputs = self.outputs(ret, stmts, at)?;
        self.settle(stmts, &outputs);
        Ok(outputs)
    }

    /// A function's returns as its outputs. One return names them as it says: a name returned
    /// names the output after it, an object literal one output per key, anything else one output,
    /// `out`, as a function written in place has (the note's fork 7). Several returns, each an
    /// object of the same keys, give one output per key, else one, `out`; each output is chosen
    /// among the returns as `if` chooses. A body's own `const out` is `out_`, so it never clashes.
    fn outputs(&mut self, ret: Ret, stmts: &mut Vec<Stmt>, at: Span) -> R<Vec<String>> {
        let exits = std::mem::take(&mut self.frame.exits);
        if exits.is_empty() {
            return fail(at, "a function returns its value: return slug");
        }
        let ended = matches!(ret, Ret::Always);
        if exits.len() == 1
            && ended
            && let Some(Exit { kind: ExitKind::One(Answer::Value { value, name, fields }), span, .. }) = exits.first().cloned()
        {
            if let Some((ts, n)) = name
                && self.is_binding(&n, stmts)
            {
                // `let fee = 0; …; return fee`: the output is `fee`, whichever name holds it last.
                let plain = self.local_name(&ts);
                if n != plain {
                    self.free(&plain, stmts);
                    rename(stmts, &n, &plain);
                }
                return Ok(vec![plain]);
            }
            return match fields {
                Some(fields) => Ok(self.output_fields(fields, stmts)),
                None => {
                    self.output("out", value, span, stmts);
                    Ok(vec!["out".into()])
                }
            };
        }
        match self.fold(exits, ended)? {
            Some(Answer::Value { fields: Some(fields), .. }) => Ok(self.output_fields(fields, stmts)),
            Some(Answer::Value { value, .. }) => {
                self.output("out", value, at, stmts);
                Ok(vec!["out".into()])
            }
            _ => fail(at, "a function returns a value"),
        }
    }

    fn output_fields(&mut self, fields: Fields, stmts: &mut Vec<Stmt>) -> Vec<String> {
        let mut outputs = Vec::new();
        for (key, value, span) in fields {
            let bound_here = matches!(&value.kind, ExprKind::Name(n) if *n == key) && self.is_binding(&key, stmts);
            if !bound_here {
                self.output(&key, value, span, stmts);
            }
            outputs.push(key);
        }
        outputs
    }

    /// Output `name` bound to `value`; a binding the body made of that name before is renamed.
    fn output(&mut self, name: &str, mut value: Expr, span: Span, stmts: &mut Vec<Stmt>) {
        if self.is_binding(name, stmts) {
            let other = self.hidden_name(name);
            rename(stmts, name, &other);
            rename_expr(&mut value, name, &other);
        }
        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.into(), span }, value }, span, after: None });
    }

    /// `name` made free for an output: a binding of it the body made is renamed.
    fn free(&mut self, name: &str, stmts: &mut [Stmt]) {
        if self.is_binding(name, stmts) {
            let other = self.hidden_name(name);
            rename(stmts, name, &other);
        }
    }

    /// Whether `name` is bound by one of `stmts` (not a parameter passed straight through).
    fn is_binding(&self, name: &str, stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, StmtKind::Bind { name: n, .. } if n.text == name))
    }

    /// A function's parameters as ports, each with the name the body reads it by: typed when
    /// annotated, and named as written, but for a graph function's keyword (see [`Self::port_name`]).
    /// A js() function's keep their names whatever they are, as its body is JavaScript that reads
    /// them; a keyword there is the lowering's mistake to name.
    fn ports(&mut self, params: &js::FormalParameters, js: bool) -> R<Vec<(String, PortDecl)>> {
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
            let param = id.name.to_string();
            let port = if js { param.clone() } else { self.port_name(&param) };
            ports.push((param, PortDecl { name: self.name(&port, id.span), ty, unit: None }));
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
        let inputs: Vec<PortDecl> = self.ports(&f.params, true)?.into_iter().map(|(_, port)| port).collect();
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
        let text = blank_types(self.src, f, inner).map_err(|(s, why)| LowerError { span: self.span(s), message: why })?;
        Ok(JsDef { name: self.name(name, name_span), inputs, outputs, fuel, memory, body: text, body_span: self.span(body.span), span: at })
    }

    /// A js() budget: a whole number (`1_000_000_000`), or one in quotes with its scale (`"50M"`,
    /// `"16MB"`).
    fn budget(&self, value: &js::Expression, which: &str) -> R<Budget> {
        let span = self.span(value.span());
        let (value, text) = match self.unwrap(value) {
            js::Expression::NumericLiteral(n) => (Some(n.value), number_text(n)),
            js::Expression::StringLiteral(s) => (scaled(s.value.as_str()), s.value.to_string()),
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
                if let js::Expression::CallExpression(c) = self.unwrap(&a.argument)
                    && self.vocab_of(&c.callee) == Some(Vocab::Event)
                {
                    return self.event(c, span);
                }
                // An action awaited is called, its answer read; one not awaited is started (`call`).
                if let js::Expression::CallExpression(c) = self.unwrap(&a.argument)
                    && let js::Expression::Identifier(i) = self.unwrap(&c.callee)
                    && let Some((flow, action)) = self.action_named(i.name.as_str())
                {
                    let callee = self.name(i.name.as_str(), i.span);
                    return self.called(c, flow, action, callee, span);
                }
                // A request is read as today's graph does, between rounds; its order and what
                // `await` adds come with slice 7.
                self.expr(&a.argument)
            }
            js::Expression::ObjectExpression(o) => self.record(o, span),
            js::Expression::ArrayExpression(a) => {
                // `[...xs, y]` is `append(xs, y)`, `[...xs, ...ys]` `extend(xs, ys)`, `[...xs]` `xs`.
                let mut list: Option<Expr> = None;
                let mut items = Vec::new();
                for item in &a.elements {
                    match item {
                        js::ArrayExpressionElement::SpreadElement(s) => {
                            let spread = self.expr(&s.argument)?;
                            let before = match list.take() {
                                None if items.is_empty() => None,
                                None => Some(Expr { kind: ExprKind::List(std::mem::take(&mut items)), span }),
                                l => l,
                            };
                            list = Some(match before {
                                None => spread,
                                Some(l) => call("extend", vec![arg(l), arg(spread)], span),
                            });
                        }
                        js::ArrayExpressionElement::Elision(s) => return fail(self.span(s.span), "a list has no holes"),
                        _ => {
                            let e = item.as_expression().expect("an item");
                            let value = self.expr(e)?;
                            self.share(e);
                            match list.take() {
                                Some(l) => list = Some(call("append", vec![arg(l), arg(value)], span)),
                                None => items.push(value),
                            }
                        }
                    }
                }
                Ok(list.unwrap_or(Expr { kind: ExprKind::List(items), span }))
            }
            js::Expression::UnaryExpression(u) => match u.operator {
                js::UnaryOperator::LogicalNot => ok(ExprKind::Not(Box::new(self.expr(&u.argument)?))),
                js::UnaryOperator::UnaryNegation => match self.unwrap(&u.argument) {
                    js::Expression::NumericLiteral(n) => ok(ExprKind::Number { value: -n.value, text: format!("-{}", number_text(n)) }),
                    // `x * -1`, exact at `-0` where `0 - x` isn't; in parentheses, one node.
                    _ => {
                        let x = self.expr(&u.argument)?;
                        ok(ExprKind::Paren(Box::new(binary_at(BinOp::Mul, x, minus_one(span), span))))
                    }
                },
                js::UnaryOperator::UnaryPlus => match self.unwrap(&u.argument) {
                    js::Expression::NumericLiteral(n) => ok(ExprKind::Number { value: n.value, text: number_text(n) }),
                    _ => {
                        let x = self.expr(&u.argument)?;
                        Ok(leaf("to_number", &[], vec![x], span))
                    }
                },
                js::UnaryOperator::BitwiseNot => {
                    let x = self.expr(&u.argument)?;
                    Ok(leaf("bitwise", &[("op", "xor")], vec![x, minus_one(span)], span))
                }
                js::UnaryOperator::Delete => fail(span, "delete is a statement: delete notes[k]"),
                op => fail(span, format!("{} isn't an operator Holon has", op.as_str())),
            },
            js::Expression::BinaryExpression(b) => {
                use js::BinaryOperator as B;
                if let Some((name, params)) = number_operator(b.operator) {
                    let (lhs, rhs) = (self.expr(&b.left)?, self.expr(&b.right)?);
                    return Ok(leaf(name, params, vec![lhs, rhs], span));
                }
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
                let literal = matches!(self.unwrap(&l.right), js::Expression::StringLiteral(_) | js::Expression::NumericLiteral(_) | js::Expression::TemplateLiteral(_) | js::Expression::ObjectExpression(_) | js::Expression::ArrayExpression(_));
                if l.operator != js::LogicalOperator::Coalesce && literal {
                    return fail(span, format!("{} takes booleans: a value for where one is absent is ??, x ?? \"none\"", l.operator.as_str()));
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
            js::Expression::UpdateExpression(_) => fail(span, "++ and -- are a statement of their own: n++"),
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
                    self.share(&p.value);
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
                Local::Bound(n) | Local::Var(Some(n)) => ok(ExprKind::Name(n)),
                Local::Var(None) => fail(span, format!("\"{name}\" has no value here: give it one first, let {name} = …")),
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
            None if name == "Math" || name == "Number" => fail(span, format!("{name} is a namespace: call its functions, {}", if name == "Math" { "Math.round(x)" } else { "Number.isInteger(x)" })),
            None if name == "NaN" || name == "Infinity" => fail(span, format!("{name} isn't a value a flow has: a leaf gives nothing where JavaScript gives {name}, so test for presence, x !== undefined, or give a fallback, x ?? 0")),
            None => fail(span, format!("unknown name \"{name}\"{}", if GLOBALS.contains(&name) { ": a flow has no globals; import what it uses" } else { "" })),
        }
    }

    fn static_member(&mut self, m: &js::StaticMemberExpression) -> R<Expr> {
        let span = self.span(m.span);
        let prop = m.property.name.as_str();
        if self.is_state(&m.object) {
            return self.state_read(prop, span);
        }
        if let Some(ns) = self.namespace(&m.object) {
            let constants = if ns == "Math" { MATH_CONSTANTS } else { NUMBER_CONSTANTS };
            if let Some((_, text)) = constants.iter().find(|(k, _)| *k == prop) {
                return Ok(Expr { kind: ExprKind::Number { value: text.parse().expect("a double's literal"), text: text.to_string() }, span });
            }
            if ns == "Number" && ["NaN", "POSITIVE_INFINITY", "NEGATIVE_INFINITY"].contains(&prop) {
                return fail(span, "a flow has no NaN or Infinity: a leaf gives nothing where JavaScript gives one");
            }
            let names = constants.iter().map(|(k, _)| format!("{ns}.{k}")).collect::<Vec<_>>().join(", ");
            return fail(span, format!("{ns}.{prop} isn't a value: {ns}'s constants are {names}, and its functions are called, {ns}.{prop}(…)"));
        }
        if prop == "length" {
            return fail(span, "a list's length is length(xs), a text's text_length(s), imported from \"holon\": .length comes in a later slice");
        }
        let base = self.expr(&m.object)?;
        Ok(Expr { kind: ExprKind::Field { base: Box::new(base), name: self.name(prop, m.property.span) }, span })
    }

    fn computed_member(&mut self, m: &js::ComputedMemberExpression) -> R<Expr> {
        let span = self.span(m.span);
        if let Some(store) = self.store_name(&m.object) {
            self.check_unwritten(&store, span)?;
        }
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
                if let Some((flow, action)) = self.action_named(name) {
                    return self.start(c, flow, action, callee, span);
                }
                match self.imports.get(name).cloned() {
                    Some(Noun::Vocab(Vocab::Leaf(leaf))) => {
                        let args = self.args(&c.arguments, &Takes::of_leaf(&leaf))?;
                        ok(ExprKind::Call { callee: Name { text: leaf, span: callee.span }, args })
                    }
                    Some(Noun::Vocab(Vocab::Status | Vocab::Respond)) => fail(span, "status(…) and respond(…) are a route's answer: return them"),
                    Some(Noun::Vocab(Vocab::Event)) => fail(span, "an event is waited for: await event(\"paid\")"),
                    Some(Noun::Vocab(v)) => fail(span, format!("{} is a statement of the top level", vocab_word(&v))),
                    Some(_) => fail(callee.span, format!("\"{name}\" isn't something to call")),
                    None if self.consts.contains_key(name) => fail(callee.span, format!("\"{name}\" is a value, not something to call")),
                    None if NUMBER_GLOBALS.contains(&name) => self.number_global(c, name, span),
                    None => fail(callee.span, format!("unknown function \"{name}\": define it at the top level, or import the leaf from \"holon\"")),
                }
            }
            js::Expression::StaticMemberExpression(m) => self.method_call(c, m),
            _ => fail(span, "a call names what it calls: a function, a leaf, or a method of something imported"),
        }
    }

    /// The action `name` names, if it names one: another flow's, imported (its flow and action),
    /// or the file's own (no flow). A binding or a function of that name is not one.
    fn action_named(&self, name: &str) -> Option<(Option<String>, String)> {
        if self.lookup(name).is_some() || self.defs.contains_key(name) {
            return None;
        }
        match self.imports.get(name) {
            Some(Noun::Action { flow, action }) => Some((Some(flow.clone()), action.clone())),
            Some(_) => None,
            None => self.actions.contains(name).then(|| (None, name.to_string())),
        }
    }

    /// `payment(x)` without `await`: the action started and not waited for. It is answered once
    /// its run first waits, `{ run, waiting }` with the token for each point it waits at, or once
    /// it ends, as a JavaScript async function runs to its first `await` before it hands back its
    /// promise (the note's §9).
    fn start(&mut self, c: &js::CallExpression, flow: Option<String>, action: String, callee: Name, span: Span) -> R<Expr> {
        let at = callee.span;
        let target = match flow {
            Some(flow) => {
                let base = Expr { kind: ExprKind::Name(super::lower::call_noun(&flow)), span: at };
                Expr { kind: ExprKind::Field { base: Box::new(base), name: Name { text: action, span: at } }, span: at }
            }
            None => Expr { kind: ExprKind::Name(action), span: at },
        };
        let mut args = vec![Arg { name: None, value: target }];
        args.extend(self.sent(c, &callee.text, span)?);
        Ok(Expr { kind: ExprKind::Call { callee: Name { text: "start".into(), span: at }, args }, span })
    }

    /// `await charge(x)`: another flow's action called and its answer read, `{ answer, run }`, or
    /// `{ error, message }` when it didn't run or waits. Waiting for the end of one that waits, the
    /// flow's own among them, comes with calls that park their caller (the note's §9).
    fn called(&mut self, c: &js::CallExpression, flow: Option<String>, action: String, callee: Name, span: Span) -> R<Expr> {
        let Some(flow) = flow else {
            let name = &callee.text;
            return fail(span, format!("await {name}(…) would wait for this flow's own action to end, which a flow can't yet: start it without await, const started = {name}(x), and answer with where it waits, started.waiting"));
        };
        let args = self.sent(c, &callee.text, span)?;
        let receiver = Name { text: super::lower::call_noun(&flow), span: callee.span };
        Ok(Expr { kind: ExprKind::Method { receiver, method: Name { text: action, span: callee.span }, args }, span })
    }

    /// What an action is sent: one value, or nothing.
    fn sent(&mut self, c: &js::CallExpression, name: &str, span: Span) -> R<Vec<Arg>> {
        let args = self.positional(&c.arguments)?;
        if args.len() > 1 {
            return fail(span, format!("an action is sent one value: {name}({{ … }}), a record for more"));
        }
        Ok(args)
    }

    /// `await event("paid", { expires: "1d" })`: the run waits for the event, as `await paid,
    /// expires: 1d` does.
    fn event(&mut self, c: &js::CallExpression, span: Span) -> R<Expr> {
        let shape = "await event(\"paid\", { expires: \"1d\" })";
        if self.frame.route {
            return fail(span, ROUTE_WAITS);
        }
        let args: Vec<&js::Expression> = c.arguments.iter().filter_map(|a| a.as_expression()).collect();
        if args.len() != c.arguments.len() || args.is_empty() || args.len() > 2 {
            return fail(span, format!("event takes its name, then its options: {shape}"));
        }
        let js::Expression::StringLiteral(name) = self.unwrap(args[0]) else {
            return fail(self.span(args[0].span()), format!("an event's name is written in quotes: {shape}"));
        };
        let mut expires = None;
        if let Some(o) = args.get(1) {
            let js::Expression::ObjectExpression(o) = self.unwrap(o) else {
                return fail(self.span(o.span()), format!("event's options are an object: {shape}"));
            };
            for (key, value, at) in self.fields(o)? {
                if key != "expires" {
                    return fail(at, format!("event's option is expires, not \"{key}\""));
                }
                let js::Expression::StringLiteral(s) = self.unwrap(value) else {
                    return fail(self.span(value.span()), format!("expires is a duration in quotes: {shape}"));
                };
                let Some(seconds) = duration(s.value.as_str()) else {
                    return fail(self.span(s.span), format!("{:?} isn't a duration: \"60s\", \"5m\", \"2h\", \"1d\"", s.value.as_str()));
                };
                expires = Some(Box::new(Expr { kind: ExprKind::Duration { seconds, text: s.value.to_string() }, span: self.span(s.span) }));
            }
        }
        Ok(Expr { kind: ExprKind::Await { name: self.name(name.value.as_str(), name.span), expires }, span })
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
                    if matches!((kind, method), ("collection", "list") | ("folder", "list" | "lines" | "slice")) {
                        self.check_unwritten(&name, span)?;
                    }
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
                (None, "Math") => return self.math_call(c, method, method_name.span, span),
                (None, "Number") => return self.number_call(c, method, method_name.span, span),
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
            // A number's: only a number has these, so the subject is taken as one.
            ("toFixed" | "toPrecision" | "toExponential", 0 | 1) => {
                let style = match method {
                    "toFixed" => "fixed",
                    "toPrecision" => "precision",
                    _ => "exponential",
                };
                let mut args = vec![subject];
                args.extend(rest.into_iter().map(|a| a.value));
                Ok(leaf("number_format", &[("style", style)], args, span))
            }
            // With a radix, only a number's; without one, a text's and a list's too, so the
            // lowering takes it on a value it knows is a number.
            ("toString", 1) => {
                let radix = rest.into_iter().map(|a| a.value).collect::<Vec<_>>();
                Ok(leaf("number_format", &[("style", "string")], [vec![subject], radix].concat(), span))
            }
            ("toString", 0) => {
                let mut e = leaf("number_format", &[("style", "string")], vec![subject], span);
                if let ExprKind::Leaf { number, .. } = &mut e.kind {
                    *number = true;
                }
                Ok(e)
            }
            ("toFixed" | "toPrecision" | "toExponential" | "toString", _) => fail(span, format!(".{method}(…) takes one number at most")),
            ("toLocaleString", _) => fail(method_name.span, "a flow has no locale: toFixed(d) writes a number with d decimals"),
            _ => fail(method_name.span, format!("no leaf does .{method}(…): do it in a js() function")),
        }
    }

    /// `Math` or `Number`, when `e` names one and nothing of the file's has the name.
    fn namespace(&self, e: &js::Expression) -> Option<&'static str> {
        let js::Expression::Identifier(i) = self.unwrap(e) else { return None };
        let name = i.name.as_str();
        let own = self.lookup(name).is_some() || self.defs.contains_key(name) || self.consts.contains_key(name) || self.imports.contains_key(name);
        match name {
            "Math" if !own => Some("Math"),
            "Number" if !own => Some("Number"),
            _ => None,
        }
    }

    /// A call's arguments, `n` of them (`min` to `max`), each a value: none spread.
    fn counted(&mut self, c: &js::CallExpression, (min, max): (usize, usize), shape: &str, span: Span) -> R<Vec<Expr>> {
        if c.arguments.len() < min || c.arguments.len() > max {
            return fail(span, shape.to_string());
        }
        Ok(self.positional(&c.arguments)?.into_iter().map(|a| a.value).collect())
    }

    /// `Math.<method>(…)`: each its leaf (numbers.md, "Reader changes").
    fn math_call(&mut self, c: &js::CallExpression, method: &str, at: Span, span: Span) -> R<Expr> {
        match method {
            "max" | "min" | "hypot" => {
                // A list spread is the list's: `aggregate`.
                if let [js::Argument::SpreadElement(s)] = c.arguments.as_slice() {
                    let xs = self.expr(&s.argument)?;
                    return Ok(leaf("aggregate", &[("fn", method)], vec![xs], span));
                }
                let shape = format!("Math.{method} takes two numbers or more, Math.{method}(a, b), or one list spread, Math.{method}(...xs)");
                let args = self.counted(c, (2, usize::MAX), &shape, span)?;
                if method == "hypot" {
                    let list = Expr { kind: ExprKind::List(args), span };
                    return Ok(leaf("aggregate", &[("fn", "hypot")], vec![list], span));
                }
                Ok(leaf(method, &[], args, span))
            }
            "sumPrecise" => {
                let xs = self.counted(c, (1, 1), "Math.sumPrecise takes a list of numbers, Math.sumPrecise(xs)", span)?;
                Ok(leaf("aggregate", &[("fn", "sum_precise")], xs, span))
            }
            "atan2" | "pow" | "imul" => {
                let (name, params, shape): (&str, &[(&str, &str)], &str) = match method {
                    "atan2" => ("atan2", &[], "Math.atan2 takes two numbers, Math.atan2(y, x)"),
                    "pow" => ("power", &[], "Math.pow takes two numbers, Math.pow(a, b), or write a ** b"),
                    _ => ("bitwise", &[("op", "imul")], "Math.imul takes two numbers, Math.imul(a, b)"),
                };
                let args = self.counted(c, (2, 2), shape, span)?;
                Ok(leaf(name, params, args, span))
            }
            "random" => fail(span, "a run has one random number, recorded: import { random } from \"holon:random\""),
            m if is_math_fn(m) => {
                let x = self.counted(c, (1, 1), &format!("Math.{m} takes one number, Math.{m}(x)"), span)?;
                Ok(leaf("math", &[("fn", m)], x, span))
            }
            other => fail(at, format!("Math has no \"{other}\" a flow can call: do it in a js() function")),
        }
    }

    /// `Number.<method>(…)`: each its leaf; `Number.isNaN(x)` is `false`, as a flow has no NaN.
    fn number_call(&mut self, c: &js::CallExpression, method: &str, at: Span, span: Span) -> R<Expr> {
        let (name, params, most): (&str, &[(&str, &str)], usize) = match method {
            "isFinite" => ("number_test", &[("test", "finite"), ("coerce", "none")], 1),
            "isInteger" => ("number_test", &[("test", "integer"), ("coerce", "none")], 1),
            "isSafeInteger" => ("number_test", &[("test", "safe_integer"), ("coerce", "none")], 1),
            "parseFloat" => ("to_number", &[("grammar", "float")], 1),
            "parseInt" => ("to_number", &[("grammar", "int")], 2),
            "isNaN" => {
                self.counted(c, (1, 1), "Number.isNaN takes one value", span)?;
                return Ok(Expr { kind: ExprKind::Bool(false), span });
            }
            other => return fail(at, format!("Number has no \"{other}\" a flow can call: Number(x), Number.isFinite, isInteger, isSafeInteger, isNaN, parseFloat and parseInt are there")),
        };
        let shape = if most == 2 { "Number.parseInt takes a text and a radix, Number.parseInt(s, 16)".to_string() } else { format!("Number.{method} takes one value") };
        let args = self.counted(c, (1, most), &shape, span)?;
        Ok(leaf(name, params, args, span))
    }

    /// `Number(x)`, `parseInt(s, r)`, `parseFloat(s)`, `isNaN(x)`, `isFinite(x)`: each its leaf.
    fn number_global(&mut self, c: &js::CallExpression, name: &str, span: Span) -> R<Expr> {
        let (leaf_name, params, most): (&str, &[(&str, &str)], usize) = match name {
            "Number" => ("to_number", &[], 1),
            "parseInt" => ("to_number", &[("grammar", "int")], 2),
            "parseFloat" => ("to_number", &[("grammar", "float")], 1),
            "isNaN" => ("number_test", &[("test", "nan"), ("coerce", "number")], 1),
            _ => ("number_test", &[("test", "finite"), ("coerce", "number")], 1),
        };
        let shape = if most == 2 { "parseInt takes a text and a radix, parseInt(s, 16)".to_string() } else { format!("{name} takes one value, {name}(x)") };
        let args = self.counted(c, (1, most), &shape, span)?;
        Ok(leaf(leaf_name, params, args, span))
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
                    params.push(self.name(&self.port_name(id.name.as_str()), id.span));
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
    /// named inputs and params: `notes.list({ limit: 50 })`, `api.post("/p", { body })`,
    /// `logs.slice(name, { offset: 0, length: 256 })`. It is read so when each of its keys is one
    /// the call takes by name, and with the arguments before it it gives every input that can't
    /// be left out; otherwise it is a record, passed in order.
    fn args(&mut self, args: &[js::Argument], takes: &Takes) -> R<Vec<Arg>> {
        let options = match args.split_last() {
            Some((js::Argument::ObjectExpression(o), before)) if !o.properties.is_empty() => {
                let named = self.fields(o).ok().filter(|fields| fields.iter().all(|(k, _, _)| takes.named.iter().any(|n| n == k)));
                let given = |fields: &Vec<(String, &js::Expression, Span)>| before.len() + fields.iter().filter(|(k, _, _)| takes.needed.contains(k)).count();
                named.filter(|n| given(n) >= takes.required).map(|n| (before, n))
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

    /// The name the lowering knows a TypeScript binding by: its own, unless the lowering keeps
    /// that word for itself (a leaf, a keyword, a noun), when an underscore follows it.
    fn local_name(&self, name: &str) -> String {
        let taken = RESERVED.contains(&name) || interface_of(alias(name)).is_some() || templates::is_template(name);
        if taken { format!("{name}_") } else { name.to_string() }
    }

    /// The name a parameter's port takes: its own, a leaf's included, as a port may be called
    /// after a leaf (`lower.rs` `check_port_name`); a keyword other than `in` and `out` has an
    /// underscore after it.
    fn port_name(&self, name: &str) -> String {
        if RESERVED.contains(&name) && name != "in" && name != "out" { format!("{name}_") } else { name.to_string() }
    }

    /// A new name for the text's `ts`: its own the first time (see [`Self::local_name`]), then
    /// `ts_2`, `ts_3`, … for each new value (§5), never one the body or the top level has.
    fn fresh(&mut self, ts: &str) -> String {
        let base = self.local_name(ts);
        let mut name = base.clone();
        let mut n = 1;
        while self.frame.used.contains(&name) || self.top_level(&name) || (n > 1 && self.taken.contains(&name)) {
            n += 1;
            name = format!("{base}_{n}");
        }
        self.frame.used.insert(name.clone());
        name
    }

    /// A name the text doesn't write, for a binding the reader makes itself: `cond_1`, `parts_2`.
    fn hidden_name(&mut self, base: &str) -> String {
        let mut n = 1;
        loop {
            let name = format!("{base}_{n}");
            if !self.frame.used.contains(&name) && !self.top_level(&name) && !self.taken.contains(&name) {
                self.frame.used.insert(name.clone());
                return name;
            }
            n += 1;
        }
    }

    /// Whether the top level has `name`: a function, a const, or a collection, folder, database
    /// or channel.
    fn top_level(&self, name: &str) -> bool {
        self.defs.contains_key(name) || self.consts.contains_key(name) || self.imports.values().any(|n| matches!(n, Noun::Declared(_, d) if d == name))
    }

    /// `value` bound to a hidden name, for an `if`'s condition or a switch's subject that each
    /// branch reads: written back where it is read if it is read once ([`Self::settle`]).
    fn hide(&mut self, value: Expr, base: &str, stmts: &mut Vec<Stmt>) -> Expr {
        if matches!(value.kind, ExprKind::Name(_)) {
            return value;
        }
        let span = value.span;
        let name = self.hidden_name(base);
        self.note_call(&name, &value, span);
        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span }, value }, span, after: None });
        self.frame.hidden.push(name.clone());
        Expr { kind: ExprKind::Name(name), span }
    }

    /// `value` as a name, bound once when it isn't one, so that each part read reads the one
    /// value.
    fn once(&mut self, value: Expr, at: Span, stmts: &mut Vec<Stmt>) -> Expr {
        if matches!(value.kind, ExprKind::Name(_)) {
            return value;
        }
        let name = self.hidden_name("parts");
        self.note_call(&name, &value, at);
        stmts.push(Stmt { kind: StmtKind::Bind { name: Name { text: name.clone(), span: at }, value }, span: at, after: None });
        Expr { kind: ExprKind::Name(name), span: at }
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

/// What a call takes by name, and how many inputs can't be left out: given in order, or by name
/// (`needed`).
struct Takes {
    required: usize,
    named: Vec<String>,
    needed: Vec<String>,
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
        let needed: Vec<String> = inputs.iter().filter(|n| !is_optional(leaf, n)).cloned().collect();
        let required = needed.len();
        // A variadic leaf's inputs are more of the same: only its params are named.
        // A `map`'s or `fold`'s function is passed in order, not by name.
        let mut named = params.into_iter().filter(|p| p != "fn" || !PASSES_EXTRA.contains(&leaf)).collect::<Vec<_>>();
        if !VARIADIC.contains(&leaf) {
            named.extend(inputs);
        }
        Takes { required, named, needed }
    }

    /// An outbound request's, after its host: the path, then headers, body, a file or stream.
    fn request() -> Takes {
        Takes { required: 1, named: ["headers", "body", "file", "stream"].map(String::from).to_vec(), needed: vec![] }
    }

    /// A method of a declared collection, folder, database or channel.
    fn of_method(kind: &str, method: &str) -> Takes {
        let leaf = match (kind, method) {
            ("collection", "list") => "collection_list",
            ("folder", "list") => "file_list",
            ("folder", "lines") => "file_lines",
            ("folder", "slice") => "file_slice",
            ("database", "exec") => return Takes { required: 1, named: vec!["expect".into()], needed: vec![] },
            ("database", "find") => return Takes { required: 2, named: vec!["sort".into(), "limit".into()], needed: vec![] },
            ("database", "find_one") => return Takes { required: 2, named: vec!["sort".into()], needed: vec![] },
            ("database", "update") => return Takes { required: 3, named: vec!["expect".into()], needed: vec![] },
            ("database", "delete") => return Takes { required: 2, named: vec!["expect".into()], needed: vec![] },
            ("channel", "subscribe") => return Takes { required: 0, named: vec!["from".into()], needed: vec![] },
            _ => return Takes { required: usize::MAX, named: vec![], needed: vec![] },
        };
        let mut t = Takes::of_leaf(leaf);
        t.named.retain(|n| n != "collection" && n != "folder");
        t
    }
}

/// There is no `start`: an action called is started, and one awaited is called.
const START_IS_A_CALL: &str =
    "\"holon\" has no start: an action called without await is started, const started = payment(req.json), and one awaited is called, await charge(x)";


/// A route's request's parts, each an output of its node: a name for one is the node itself.
const REQUEST_PARTS: &[&str] = &["request", "params", "query", "headers", "body", "json"];

/// JavaScript's globals a flow doesn't have, named in the mistake.
const GLOBALS: &[&str] = &["fetch", "Date", "setTimeout", "setInterval", "process", "globalThis", "window", "console", "Promise", "require"];

/// `Math`'s constants, each the literal of its double (numbers.md, "Race output").
pub(crate) const MATH_CONSTANTS: &[(&str, &str)] = &[
    ("E", "2.718281828459045"),
    ("LN10", "2.302585092994046"),
    ("LN2", "0.6931471805599453"),
    ("LOG10E", "0.4342944819032518"),
    ("LOG2E", "1.4426950408889634"),
    ("PI", "3.141592653589793"),
    ("SQRT1_2", "0.7071067811865476"),
    ("SQRT2", "1.4142135623730951"),
];

/// `Number`'s constants. `NaN` and the infinities aren't values a flow has.
pub(crate) const NUMBER_CONSTANTS: &[(&str, &str)] = &[
    ("EPSILON", "2.220446049250313e-16"),
    ("MAX_SAFE_INTEGER", "9007199254740991"),
    ("MIN_SAFE_INTEGER", "-9007199254740991"),
    ("MAX_VALUE", "1.7976931348623157e308"),
    ("MIN_VALUE", "5e-324"),
];

/// JavaScript's number functions called by name, each a leaf: `parseInt(s, 16)`, `isNaN(x)`.
const NUMBER_GLOBALS: &[&str] = &["Number", "parseInt", "parseFloat", "isNaN", "isFinite"];

/// Whether `Math.<name>` is a one-number function, `math` with that `fn`.
fn is_math_fn(name: &str) -> bool {
    family_values("math", "fn").is_some_and(|values| values.contains(&name))
}

/// A number operator the language writes as a leaf, `%`, `**` and the bitwise ones: the leaf and
/// its params.
fn number_operator(op: js::BinaryOperator) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    use js::BinaryOperator as B;
    Some(match op {
        B::Remainder => ("remainder", &[]),
        B::Exponential => ("power", &[]),
        B::BitwiseAnd => ("bitwise", &[("op", "and")]),
        B::BitwiseOR => ("bitwise", &[("op", "or")]),
        B::BitwiseXOR => ("bitwise", &[("op", "xor")]),
        B::ShiftLeft => ("bitwise", &[("op", "shl")]),
        B::ShiftRight => ("bitwise", &[("op", "shr")]),
        B::ShiftRightZeroFill => ("bitwise", &[("op", "ushr")]),
        _ => return None,
    })
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !s.starts_with(|c: char| c.is_ascii_digit())
}

fn declares_method(m: &str) -> bool {
    holon_core::METHODS.contains(&m)
}

fn is_text(e: &js::Expression) -> bool {
    matches!(e, js::Expression::StringLiteral(_) | js::Expression::TemplateLiteral(_))
}

/// A number as the lowering reads it: its digits as written, without `_` separators.
fn number_text(n: &js::NumericLiteral) -> String {
    match &n.raw {
        Some(raw) if n.base == js::NumberBase::Decimal || n.base == js::NumberBase::Float => raw.as_str().replace('_', ""),
        _ => {
            let v = n.value;
            if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
        }
    }
}

/// A number as written, `2.5` or `1e3`: digits, a decimal part, an exponent.
fn decimal(text: &str) -> Option<f64> {
    let ok = !text.is_empty() && text.starts_with(|c: char| c.is_ascii_digit()) && text.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    ok.then(|| text.parse().ok()).flatten()
}

/// `"10s"`, `"5m"`, `"2h"`, `"1d"` in seconds.
pub(crate) fn duration(text: &str) -> Option<f64> {
    let (n, unit) = text.split_at(text.len().checked_sub(1)?);
    let per = match unit {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => return None,
    };
    decimal(n).map(|n| n * per)
}

/// A budget in quotes: a number, or one with its scale, `"50M"` (millions), `"2K"`, `"1G"`, or
/// bytes, `"16MB"` (MiB), `"512KB"`, `"1GB"`.
fn scaled(text: &str) -> Option<f64> {
    let (n, bytes) = text.strip_suffix('B').map_or((text, false), |n| (n, true));
    let (n, per) = match n.chars().last() {
        Some('K') => (&n[..n.len() - 1], if bytes { 1024.0 } else { 1e3 }),
        Some('M') => (&n[..n.len() - 1], if bytes { 1024.0 * 1024.0 } else { 1e6 }),
        Some('G') => (&n[..n.len() - 1], if bytes { 1024.0 * 1024.0 * 1024.0 } else { 1e9 }),
        _ if bytes => return None,
        _ => (n, 1.0),
    };
    decimal(n).map(|n| n * per)
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

/// A list's methods that change it in place: `push` is a new list with the items after it; the
/// rest come in a later slice.
const MUTATORS: &[&str] = &["push", "pop", "shift", "unshift", "splice", "sort", "reverse", "fill", "copyWithin"];

fn call(leaf: &str, args: Vec<Arg>, span: Span) -> Expr {
    Expr { kind: ExprKind::Call { callee: Name { text: leaf.into(), span }, args }, span }
}

fn arg(value: Expr) -> Arg {
    Arg { name: None, value }
}

fn named(name: &str, value: Expr) -> Arg {
    Arg { name: Some(Name { text: name.into(), span: value.span }), value }
}

fn text_expr(text: &str, span: Span) -> Expr {
    Expr { kind: ExprKind::Text(text.into()), span }
}

/// A value that is never there: `if false then false`.
fn absent(span: Span) -> Expr {
    let no = Expr { kind: ExprKind::Bool(false), span };
    Expr { kind: ExprKind::If { cond: Box::new(no.clone()), then: Box::new(no), els: None }, span }
}

/// A leaf placed for one of JavaScript's forms, with its params (`ExprKind::Leaf`).
fn leaf(leaf: &str, params: &[(&str, &str)], args: Vec<Expr>, span: Span) -> Expr {
    let params = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Expr { kind: ExprKind::Leaf { leaf: Name { text: leaf.into(), span }, params, args: args.into_iter().map(arg).collect(), number: false }, span }
}

/// `-1`, as `-x` and `~x` use it.
fn minus_one(span: Span) -> Expr {
    Expr { kind: ExprKind::Number { value: -1.0, text: "-1".into() }, span }
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    binary_at(op, lhs, rhs, span)
}

fn binary_at(op: BinOp, lhs: Expr, rhs: Expr, span: Span) -> Expr {
    Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span }
}

/// `value when c`.
fn when(value: Expr, c: Expr) -> Expr {
    binary(BinOp::When, value, c)
}

/// `(c ?? false)`: `c` as an `if` reads it, false where it is absent.
fn truthy(c: &Expr) -> Expr {
    let no = Expr { kind: ExprKind::Bool(false), span: c.span };
    Expr { kind: ExprKind::Paren(Box::new(binary_at(BinOp::Coalesce, c.clone(), no, c.span))), span: c.span }
}

/// `!(c ?? false)`: what holds where an `if (c)` takes its else.
fn otherwise(c: &Expr) -> Expr {
    Expr { kind: ExprKind::Not(Box::new(truthy(c))), span: c.span }
}

/// The conditions, all of them: `a && b && …`.
fn conj(cs: impl IntoIterator<Item = Expr>) -> Option<Expr> {
    cs.into_iter().reduce(|a, b| binary(BinOp::And, a, b))
}

/// Whether `subject` is one of `patterns`.
fn matched(subject: &Expr, patterns: &[Expr], at: Span) -> Expr {
    patterns.iter().map(|p| binary_at(BinOp::Eq, subject.clone(), p.clone(), at)).reduce(|a, b| binary_at(BinOp::Or, a, b, at)).expect("a case has a pattern")
}

/// Whether `if (c) a else b` returns, from whether each branch does.
fn ret_if(c: &Expr, a: &Ret, b: &Ret) -> Ret {
    match (a, b) {
        (Ret::Never, Ret::Never) => Ret::Never,
        (Ret::Always, Ret::Always) => Ret::Always,
        (Ret::Always, Ret::Never) => Ret::When(c.clone()),
        (Ret::Never, Ret::Always) => Ret::When(otherwise(c)),
        (Ret::When(x), Ret::Never) => Ret::When(binary(BinOp::And, c.clone(), x.clone())),
        (Ret::Never, Ret::When(y)) => Ret::When(binary(BinOp::And, otherwise(c), y.clone())),
        (Ret::Always, Ret::When(y)) => Ret::When(binary(BinOp::Or, truthy(c), y.clone())),
        (Ret::When(x), Ret::Always) => Ret::When(binary(BinOp::Or, otherwise(c), x.clone())),
        (Ret::When(x), Ret::When(y)) => Ret::When(Expr { kind: ExprKind::If { cond: Box::new(c.clone()), then: Box::new(x.clone()), els: Some(Box::new(y.clone())) }, span: c.span }),
    }
}

/// One value choosing among `arms` by `choice`, else `els`; `None` where no side has one. Where
/// one side alone has a value, it is that value `when` its side is taken (§5).
fn choose(choice: &Choice, arms: Vec<Option<Expr>>, els: Option<Expr>, at: Span) -> Option<Expr> {
    let first = arms.iter().chain(std::iter::once(&els)).flatten().next()?.clone();
    if arms.iter().chain(std::iter::once(&els)).all(|s| s.as_ref().is_some_and(|s| same(s, &first))) {
        return Some(first);
    }
    match choice {
        Choice::If(c) => match (arms.into_iter().next().flatten(), els) {
            (Some(a), Some(b)) => Some(Expr { kind: ExprKind::If { cond: Box::new(c.clone()), then: Box::new(a), els: Some(Box::new(b)) }, span: at }),
            (Some(a), None) => Some(when(a, c.clone())),
            (None, Some(b)) => Some(when(b, otherwise(c))),
            (None, None) => None,
        },
        Choice::Match(subject, patterns) => {
            let mut out = Vec::new();
            for (p, a) in patterns.iter().zip(arms) {
                match (a, &els) {
                    (Some(a), Some(e)) if same(&a, e) => {}
                    (Some(a), _) => out.push(Arm { patterns: p.clone(), value: a }),
                    // A case without a value, and an else with one: the case's is absent.
                    (None, Some(_)) => out.push(Arm { patterns: p.clone(), value: absent(at) }),
                    (None, None) => {}
                }
            }
            if out.is_empty() {
                return els;
            }
            Some(Expr { kind: ExprKind::Match { subject: Box::new(subject.clone()), arms: out, els: els.map(Box::new), open: at }, span: at })
        }
    }
}

/// Whether two values are one: the same literal or name, wherever written.
fn same(a: &Expr, b: &Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Number { text: x, .. }, ExprKind::Number { text: y, .. }) => x == y,
        (ExprKind::Text(x), ExprKind::Text(y)) | (ExprKind::Name(x), ExprKind::Name(y)) => x == y,
        (ExprKind::Bool(x), ExprKind::Bool(y)) => x == y,
        _ => a == b,
    }
}

/// The lowering's name a local holds now (`None` for a `let` not given a value yet), if it is
/// a binding.
fn version(local: &Local) -> Option<Option<String>> {
    match local {
        Local::Bound(n) => Some(Some(n.clone())),
        Local::Var(v) => Some(v.clone()),
        Local::Alias(_) => None,
    }
}

fn read_seg(base: Expr, seg: &Seg, at: Span) -> Expr {
    match seg {
        Seg::Field(name, span) => Expr { kind: ExprKind::Field { base: Box::new(base), name: Name { text: name.clone(), span: *span } }, span: at },
        Seg::Key(k) => Expr { kind: ExprKind::Index { base: Box::new(base), key: Box::new(k.clone()) }, span: at },
    }
}

/// `base` with the value at `path` changed by `edit`: `{ ...base, k: v }` for a field,
/// `insert(base, k, v)` for a key, `without_field` or `remove` to delete one (§5).
fn rebuilt(base: Expr, path: &[Seg], edit: Edit, at: Span) -> Expr {
    match path.split_first() {
        None => match edit {
            Edit::Set(v) => v,
            Edit::Remove(Seg::Field(name, span)) => call("without_field", vec![arg(base), named("name", text_expr(&name, span))], at),
            Edit::Remove(Seg::Key(k)) => call("remove", vec![arg(base), arg(k)], at),
        },
        Some((seg, rest)) => {
            let inner = rebuilt(read_seg(base.clone(), seg, at), rest, edit, at);
            match seg {
                Seg::Field(name, span) => Expr { kind: ExprKind::Record { spread: Some(Box::new(base)), fields: vec![(Key { text: name.clone(), quoted: false, span: *span }, inner)] }, span: at },
                Seg::Key(k) => call("insert", vec![arg(base), arg(k.clone()), arg(inner)], at),
            }
        }
    }
}

/// A route's answer as `respond`'s arguments.
fn respond_args(r: Response) -> Vec<Arg> {
    let mut args = Vec::new();
    match r.body {
        // `respond 204`, as a status alone is written; a computed one is named.
        None => {
            if let Some(s) = r.status {
                let bare = matches!(s.kind, ExprKind::Number { .. }) && r.missing.is_none() && r.headers.is_none();
                args.push(if bare { arg(s) } else { named("status", s) });
            }
        }
        Some(body) => {
            args.push(if matches!(body.kind, ExprKind::Number { .. }) { named("body", body) } else { arg(body) });
            if let Some(s) = r.status {
                args.push(named("status", s));
            }
        }
    }
    for (name, value) in [("missing", r.missing), ("headers", r.headers), ("stream", r.stream)] {
        if let Some(v) = value {
            args.push(named(name, v));
        }
    }
    args
}

/// A branch's statements: a block's, or the one statement it is.
fn statements_of<'a, 'b>(s: &'a js::Statement<'b>) -> &'a [js::Statement<'b>] {
    match s {
        js::Statement::BlockStatement(b) => &b.body[..],
        s => std::slice::from_ref(s),
    }
}

/// A case's statements without the `break` that ends it, and whether one did.
fn case_body<'a, 'b>(body: &'a [js::Statement<'b>]) -> (&'a [js::Statement<'b>], bool) {
    let plain_break = |s: &js::Statement| matches!(s, js::Statement::BreakStatement(b) if b.label.is_none());
    match body {
        [js::Statement::BlockStatement(b)] => match b.body.split_last() {
            Some((last, rest)) if plain_break(last) => (rest, true),
            _ => (body, false),
        },
        [rest @ .., last] if plain_break(last) => (rest, true),
        _ => (body, false),
    }
}

/// Each `var`'s names in `body`, at any depth but a function's.
fn hoisted(body: &[js::Statement], vars: &mut Vec<String>) {
    for s in body {
        match s {
            js::Statement::VariableDeclaration(v) if v.kind == js::VariableDeclarationKind::Var => {
                for d in &v.declarations {
                    pattern_names(&d.id, vars);
                }
            }
            js::Statement::BlockStatement(b) => hoisted(&b.body, vars),
            js::Statement::IfStatement(f) => {
                hoisted(std::slice::from_ref(&f.consequent), vars);
                if let Some(a) = &f.alternate {
                    hoisted(std::slice::from_ref(a), vars);
                }
            }
            js::Statement::SwitchStatement(w) => {
                for c in &w.cases {
                    hoisted(&c.consequent, vars);
                }
            }
            _ => {}
        }
    }
}

/// The names a pattern binds.
fn pattern_names(p: &js::BindingPattern, names: &mut Vec<String>) {
    match p {
        js::BindingPattern::BindingIdentifier(id) => names.push(id.name.to_string()),
        js::BindingPattern::AssignmentPattern(a) => pattern_names(&a.left, names),
        js::BindingPattern::ObjectPattern(o) => {
            for prop in &o.properties {
                pattern_names(&prop.value, names);
            }
            if let Some(r) = &o.rest {
                pattern_names(&r.argument, names);
            }
        }
        js::BindingPattern::ArrayPattern(a) => {
            for el in a.elements.iter().flatten() {
                pattern_names(el, names);
            }
            if let Some(r) = &a.rest {
                pattern_names(&r.argument, names);
            }
        }
    }
}

/// `f` on every expression of `stmts`, as [`walk`] goes.
fn each_expr(stmts: &mut [Stmt], f: &mut dyn FnMut(&mut Expr) -> bool) {
    for s in stmts {
        match &mut s.kind {
            StmtKind::Bind { value, .. } | StmtKind::StateWrite { value, .. } | StmtKind::Expr(value) => walk(value, f),
            StmtKind::CollectionWrite { key, value, .. } => {
                walk(key, f);
                walk(value, f);
            }
            StmtKind::Delete { key, .. } => walk(key, f),
            StmtKind::Respond(args) => {
                for a in args {
                    walk(&mut a.value, f);
                }
            }
        }
        if let Some(a) = &mut s.after {
            walk(a, f);
        }
    }
}

/// `f` on `e`, then on each expression in it while `f` says to go on.
fn walk(e: &mut Expr, f: &mut dyn FnMut(&mut Expr) -> bool) {
    if !f(e) {
        return;
    }
    match &mut e.kind {
        ExprKind::Interpolated(parts) => {
            for p in parts {
                if let Part::Expr(x) = p {
                    walk(x, f);
                }
            }
        }
        ExprKind::Call { args, .. } | ExprKind::Method { args, .. } | ExprKind::Leaf { args, .. } => {
            for a in args {
                walk(&mut a.value, f);
            }
        }
        ExprKind::Field { base, .. } => walk(base, f),
        ExprKind::Index { base, key } => {
            walk(base, f);
            walk(key, f);
        }
        ExprKind::Record { spread, fields } => {
            if let Some(s) = spread {
                walk(s, f);
            }
            for (_, v) in fields {
                walk(v, f);
            }
        }
        ExprKind::List(items) => {
            for i in items {
                walk(i, f);
            }
        }
        ExprKind::Not(x) | ExprKind::Paren(x) | ExprKind::Lambda { body: x, .. } => walk(x, f),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk(lhs, f);
            walk(rhs, f);
        }
        ExprKind::If { cond, then, els } => {
            walk(cond, f);
            walk(then, f);
            if let Some(e) = els {
                walk(e, f);
            }
        }
        ExprKind::Match { subject, arms, els, .. } => {
            walk(subject, f);
            for a in arms {
                for p in &mut a.patterns {
                    walk(p, f);
                }
                walk(&mut a.value, f);
            }
            if let Some(e) = els {
                walk(e, f);
            }
        }
        ExprKind::Await { expires: Some(x), .. } => walk(x, f),
        _ => {}
    }
}

/// How many times `stmts` read `name`.
fn uses(stmts: &mut [Stmt], name: &str) -> usize {
    let mut n = 0;
    each_expr(stmts, &mut |e| {
        if matches!(&e.kind, ExprKind::Name(x) if x == name) {
            n += 1;
        }
        true
    });
    n
}

/// The bindings something reads: a statement that isn't a binding, `outputs`, or a binding read
/// so, through any number of others.
fn live_bindings(stmts: &mut [Stmt], outputs: &[String]) -> BTreeSet<String> {
    let mut reads = Vec::new();
    for s in stmts.iter_mut() {
        let bound = match &s.kind {
            StmtKind::Bind { name, .. } => Some(name.text.clone()),
            _ => None,
        };
        let mut names = BTreeSet::new();
        each_expr(std::slice::from_mut(s), &mut |e| {
            if let ExprKind::Name(n) = &e.kind {
                names.insert(n.clone());
            }
            true
        });
        reads.push((bound, names));
    }
    let mut live: BTreeSet<String> = outputs.iter().cloned().collect();
    for (bound, names) in &reads {
        if bound.is_none() {
            live.extend(names.iter().cloned());
        }
    }
    loop {
        let before = live.len();
        for (bound, names) in &reads {
            if bound.as_ref().is_some_and(|b| live.contains(b)) {
                live.extend(names.iter().cloned());
            }
        }
        if live.len() == before {
            return live;
        }
    }
}

/// `from` renamed `to`, where it is bound and where it is read.
fn rename(stmts: &mut [Stmt], from: &str, to: &str) {
    for s in stmts.iter_mut() {
        if let StmtKind::Bind { name, .. } = &mut s.kind
            && name.text == from
        {
            name.text = to.into();
        }
    }
    each_expr(stmts, &mut |e| rename_one(e, from, to));
}

fn rename_expr(e: &mut Expr, from: &str, to: &str) {
    walk(e, &mut |x| rename_one(x, from, to));
}

/// `e` renamed if it reads `from`; a function written in place whose parameter is `from` reads
/// its own.
fn rename_one(e: &mut Expr, from: &str, to: &str) -> bool {
    match &mut e.kind {
        ExprKind::Name(n) if n == from => {
            *n = to.into();
            true
        }
        ExprKind::Lambda { params, .. } => !params.iter().any(|p| p.text == from),
        _ => true,
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

    /// `ts` and `plain` deploy as the same flow, and fmt lays `ts` out as that flow.
    fn same_unit(what: &str, ts: &str, plain: &str) {
        assert_eq!(deployed(what, compile(ts)), deployed(&format!("{what}, plainly"), compile(plain)), "{what}");
        // And fmt lays it out, as the same flow (which it checks), once for good.
        let laid = super::super::format::format(ts).unwrap_or_else(|e| panic!("{what}: fmt: {e}\n{ts}"));
        assert_eq!(super::super::format::format(&laid).unwrap_or_else(|e| panic!("{what}: fmt again: {e}\n{laid}")), laid, "{what}: fmt again changed it");
    }

    /// Each construct, beside the same flow written plainly: a `const` a node, a ternary for an
    /// `if`, a leaf by its name. (The plain forms were `.holon`'s, written in TypeScript by the
    /// converter that went with it.)
    #[test]
    fn each_construct_lowers_as_its_plain_form() {
        let pairs = [
            (
                "guarded write",
                "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nflow(\"w\", { port: 8000 })\nroute.post(\"/n\", (req) => {\n  if (req.json.keep) notes[req.json.id] = req.json.text\n  return \"ok\"\n})\n",
                r#"import { flow, route } from "holon"
import notes from "collection:notes"

flow("w", { port: 8000 })

route.post("/n", (req) => {
  notes[req.json.id] = req.json.keep ? req.json.text : undefined
  return "ok"
})
"#,
            ),
            (
                "function",
                "import { flow, route } from \"holon\"\nflow(\"s\", { port: 8000 })\nfunction slugify(text) {\n  const slug = text.toLowerCase().split(\" \").join(\"-\")\n  return slug\n}\nroute.post(\"/s\", (req) => ({ slug: slugify(req.json.title) }))\n",
                r#"import { flow, route } from "holon"

flow("s", { port: 8000 })

function slugify(text) {
  const slug = text.toLowerCase().split(" ").join("-")
  return slug
}

route.post("/s", (req) => ({ slug: slugify(req.json.title) }))
"#,
            ),
            (
                "function of several outputs",
                "import { flow, route } from \"holon\"\nflow(\"m\", { port: 8000 })\nconst split_name = (full) => {\n  const first = full.first\n  return { first, last: full.last }\n}\nroute.post(\"/m\", (req) => {\n  const { first, last } = split_name(req.json)\n  return { first, last }\n})\n",
                r#"import { flow, route } from "holon"

flow("m", { port: 8000 })

function split_name(full) {
  const first = full.first
  const last = full.last
  return { first, last }
}

route.post("/m", (req) => {
  const parts_1 = split_name(req.json)
  const first = parts_1.first
  const last = parts_1.last
  return { first, last }
})
"#,
            ),
            (
                "settings",
                "import { flow, every } from \"holon\"\nimport state from \"state\"\nflow(\"k\", { keep: { runs: 5, days: 1 }, limits: { run_budget_ms: \"1m\", max_waiting_runs: 10 } })\nevery(\"1m\", () => {\n  state.beats = (state.beats ?? 0) + 1\n})\n",
                r#"import { every, flow } from "holon"
import state from "state"

flow("k", { keep: { runs: 5, days: 1 }, limits: { run_budget_ms: "1m", max_waiting_runs: 10 } })

every("1m", () => {
  state.beats = (state.beats ?? 0) + 1
})
"#,
            ),
            (
                "defaults and constants",
                "import { flow, route } from \"holon\"\nflow(\"d\", { port: 8000 })\nconst limit = 50\nroute.get(\"/d\", ({ query }) => {\n  const { after = \"\", by } = query\n  return { after, by, limit }\n})\n",
                r#"import { flow, route } from "holon"

flow("d", { port: 8000 })

const limit = 50

route.get("/d", (req) => {
  const after = req.query.after ?? ""
  const by = req.query.by
  return { after, by, limit }
})
"#,
            ),
            (
                "a function returning an expression",
                "import { flow, route } from \"holon\"\nflow(\"o\", { port: 8000 })\nfunction slugify(text) {\n  return text.toLowerCase().split(\" \").join(\"-\")\n}\nroute.post(\"/s\", (req) => slugify(req.json.title))\n",
                r#"import { flow, route } from "holon"

flow("o", { port: 8000 })

function slugify(text) {
  const out = text.toLowerCase().split(" ").join("-")
  return out
}

route.post("/s", (req) => slugify(req.json.title))
"#,
            ),
            (
                "ports named after a leaf and a keyword",
                "import { flow, route } from \"holon\"\nflow(\"p\", { port: 8000 })\nconst total = (sum, json) => sum + json.extra\nroute.post(\"/t\", (req) => ({ total: total(req.json.sum, req.json), more: req.json.xs.map((sum) => sum + 1) }))\n",
                r#"import { flow, route } from "holon"

flow("p", { port: 8000 })

function total(sum, json) {
  const out = sum + json.extra
  return out
}

route.post("/t", (req) => ({
  total: total(req.json.sum, req.json),
  more: req.json.xs.map((sum) => sum + 1),
}))
"#,
            ),
            (
                "a flow's action called",
                "import { flow, route } from \"holon\"\nimport { charge } from \"flow:billing\"\nflow(\"c\", { port: 8000 })\nroute.post(\"/c\", async (req) => await charge(req.json))\n",
                r#"import { flow, route } from "holon"
import { charge } from "flow:billing"

flow("c", { port: 8000 })

route.post("/c", async (req) => await charge(req.json))
"#,
            ),
        ];
        for (what, ts, plain) in pairs {
            same_unit(what, ts, plain);
        }
    }

    /// Slice 2's statements, each beside the same flow written plainly: a `let` given a new value
    /// is a new name, branches join with ternaries, returns fold into one answer.
    #[test]
    fn each_statement_lowers_as_its_plain_form() {
        let pairs = [
            (
                "let, if and else if",
                r#"import { flow, route } from "holon"
flow("f", { port: 8000 })
route.post("/fee", (req) => {
  let fee = 0
  if (req.json.express) {
    fee = 5
  } else if (req.json.amount > 100) {
    fee = 2
  }
  return { fee }
})
"#,
                r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.post("/fee", (req) => {
  const fee = 0
  const fee_2 = 5
  const fee_3 = 2
  const fee_4 = req.json.amount > 100 ? fee_3 : fee
  const fee_5 = req.json.express ? fee_2 : fee_4
  return { fee: fee_5 }
})
"#,
            ),
            (
                "an early return",
                r#"import { flow, route, status } from "holon"
import state from "state"
flow("p", { port: 8000 })
route.post("/pay", (req) => {
  const amount = req.json.amount
  if (amount > 1000) return status(403, "too much")
  state.paid = amount
  return { paid: amount }
})
"#,
                r#"import { flow, route, status } from "holon"
import state from "state"

flow("p", { port: 8000 })

route.post("/pay", (req) => {
  const amount = req.json.amount
  const cond_1 = amount > 1000
  state.paid = !(cond_1 ?? false) ? amount : undefined
  return status(cond_1 ? 403 : 200, cond_1 ? "too much" : { paid: amount })
})
"#,
            ),
            (
                "the rest of destructuring, and changes in place",
                r#"import { flow, route } from "holon"
flow("o", { port: 8000 })
route.post("/order", (req) => {
  const { id, ...rest } = req.json
  const [first, ...others] = rest.items
  const tags = [...others, "new"]
  const order = { id, first }
  order.count = 2
  delete order.first
  tags.push("x")
  return { order, tags, rest }
})
"#,
                r#"import { append, flow, head, route, tail, without_field } from "holon"

flow("o", { port: 8000 })

route.post("/order", (req) => {
  const parts_1 = req.json
  const id = parts_1.id
  const rest = without_field(parts_1, { name: "id" })
  const parts_2 = rest.items
  const first = head(parts_2)
  const others = tail(parts_2)
  const tags = append(others, "new")
  const order = { id, first }
  const order_2 = { ...order, count: 2 }
  const order_3 = without_field(order_2, { name: "first" })
  const tags_2 = append(tags, "x")
  return { order: order_3, tags: tags_2, rest }
})
"#,
            ),
            (
                "updates",
                r#"import { flow, every } from "holon"
import state from "state"
flow("u", { port: 8000 })
every("1m", () => {
  let n = state.beats ?? 0
  n++
  n += 2
  state.beats = n
})
"#,
                r#"import { every, flow } from "holon"
import state from "state"

flow("u", { port: 8000 })

every("1m", () => {
  const n = state.beats ?? 0
  const n_2 = n + 1
  const n_3 = n_2 + 2
  state.beats = n_3
})
"#,
            ),
            (
                "a swap, and var",
                r#"import { flow, route } from "holon"
flow("s", { port: 8000 })
route.post("/s", (req) => {
  var a = req.json.a;
  var b = req.json.b;
  [a, b] = [b, a];
  return { a, b }
})
"#,
                r#"import { flow, route } from "holon"

flow("s", { port: 8000 })

route.post("/s", (req) => {
  const a = req.json.a
  const b = req.json.b
  const a_2 = b
  const b_2 = a
  return { a: a_2, b: b_2 }
})
"#,
            ),
            (
                "an action started, which awaits an event",
                r#"import { flow, route, action, event } from "holon"
import state from "state"
flow("e", { port: 8000 })
route.post("/orders", (req) => {
  const started = order(req.json)
  return `/paid/${started.waiting.paid}`
})
export const order = action(async (input) => {
  const paid = await event("paid", { expires: "1d" })
  state.last_order = { ...input, paid_via: paid.method }
})
route.get("/paid/:token", () => "thanks", { resumes: "paid" })
"#,
                r#"import { action, event, flow, route } from "holon"
import state from "state"

flow("e", { port: 8000 })

route.post("/orders", (req) => {
  const started = order(req.json)
  return `/paid/${started.waiting.paid}`
})

export const order = action(async (input) => {
  const paid = await event("paid", { expires: "1d" })
  state.last_order = { ...input, paid_via: paid.method }
})

route.get("/paid/:token", () => "thanks", { resumes: "paid" })
"#,
            ),
            (
                "another flow's action started",
                r#"import { flow, route } from "holon"
import { charge } from "flow:billing"
flow("s", { port: 8000 })
route.post("/s", (req) => {
  const started = charge(req.json)
  return started.run
})
"#,
                r#"import { flow, route } from "holon"
import { charge } from "flow:billing"

flow("s", { port: 8000 })

route.post("/s", (req) => {
  const started = charge(req.json)
  return started.run
})
"#,
            ),
            (
                "a call under an if, read by nothing",
                r#"import { flow, route } from "holon"
import api from "host:api.example.com"
flow("g", { port: 8000 })
route.post("/ping", async (req) => {
  if (req.json.loud) {
    const sent = await api.post("/ping", { body: req.json })
  }
  if (req.json.twice) await api.post("/again", { body: req.json })
  return "ok"
})
"#,
                r#"import { flow, route } from "holon"
import example from "host:api.example.com"

flow("g", { port: 8000 })

route.post("/ping", async (req) => {
  let sent
  if ((req.json.loud ? true : undefined) !== undefined) {
    sent = await example.post("/ping", { body: req.json })
  }
  if ((req.json.twice ? true : undefined) !== undefined) {
    await example.post("/again", { body: req.json })
  }
  return "ok"
})
"#,
            ),
            (
                "a function's let, returned",
                r#"import { flow, route } from "holon"
flow("t", { port: 8000 })
function price(amount) {
  let fee = 1
  if (amount > 100) fee = 2
  return fee
}
route.post("/t", (req) => price(req.json.amount))
"#,
                r#"import { flow, route } from "holon"

flow("t", { port: 8000 })

function price(amount) {
  const fee_1 = 1
  const fee_2 = 2
  const fee = amount > 100 ? fee_2 : fee_1
  return fee
}

route.post("/t", (req) => price(req.json.amount))
"#,
            ),
        ];
        for (what, ts, plain) in pairs {
            same_unit(what, ts, plain);
        }
    }

    /// What slice 2 refuses (§12), each saying what to write instead.
    #[test]
    fn a_refusal_says_what_to_write() {
        let refused = [
            ("route.post(\"/\", (req) => {\n  switch (req.json.op) {\n    case \"a\":\n      notes[req.json.id] = 1\n    case \"b\":\n      notes[req.json.id] = 2\n  }\n  return 1\n})\n", "7:5: a case runs on into the next: end it with break or return"),
            ("route.post(\"/\", (req) => {\n  if (req.json.a) {\n    break\n  }\n  return 1\n})\n", "7:5: break ends a case, last in it"),
            ("route.post(\"/\", (req) => {\n  const xs = [1]\n  const ys = xs\n  ys.push(2)\n  return ys\n})\n", "8:3: \"ys\" is held elsewhere too (by another name, a record or a write), which a change in place would change behind it: copy it first, const copy = [...ys] or { ...ys }"),
            ("route.post(\"/\", (req) => {\n  let a = 1;\n  [notes[req.json.id]] = [a]\n  return a\n})\n", "7:4: a pattern that writes is refused: bind the name, then write it"),
            ("route.get(\"/\", (req) => req.query.name || \"anon\")\n", "5:25: || takes booleans: a value for where one is absent is ??, x ?? \"none\""),
            ("route.post(\"/\", (req) => {\n  notes[req.json.id] = req.json\n  return notes[req.json.id]\n})\n", "7:10: notes is read after this run writes to it, and a write is made when the run ends: use the value written"),
            ("route.post(\"/\", (req) => {\n  return 1\n  notes[req.json.id] = 2\n})\n", "7:3: nothing after a return runs: remove it"),
            ("route.post(\"/\", (req) => {\n  const n = 1\n  n = 2\n  return n\n})\n", "7:3: \"n\" is a const: declare it with let to give it a new value"),
            ("route.post(\"/\", (req) => {\n  req = 2\n  return 1\n})\n", "6:3: \"req\" is a parameter, which stays what it was passed: give the new value a name of its own"),
            ("route.post(\"/\", (req) => {\n  if (req.json.a !== undefined) {\n    return 1\n  }\n  return 2\n})\n", "7:12: a return inside if (x !== undefined) comes in a later slice: return after it, with ?? for when x has no value"),
            ("route.post(\"/\", (req) => {\n  const xs = [1]\n  xs.sort()\n  return xs\n})\n", "7:3: sort changes the list in place, which comes in a later slice: write the new list, xs = …"),
            ("route.post(\"/\", (req) => {\n  switch (req.json.op) {\n    case req.json.other:\n      return 1\n  }\n  return 2\n})\n", "7:10: a case is a literal: a number, a text, true or false; to test a condition, use if"),
            ("route.post(\"/\", (req) => {\n  if (req.json.a) return status(200, 1)\n  return 2 ?? status(400)\n})\n", "7:10: this answer is 400 when its body is absent, and the one on line 6 is 404: say it the same for both"),
            ("route.post(\"/\", (req) => {\n  const n = req.json.n++\n  return n\n})\n", "6:13: ++ and -- are a statement of their own: n++"),
            ("route.post(\"/\", (req) => {\n  state[req.json.k] = 1\n  return 1\n})\n", "6:3: a state key is a name: state.hits = …; a key computed is a collection's, notes[k] = …"),
        ];
        let head = "import { flow, route, status } from \"holon\"\nimport notes from \"collection:notes\"\nimport state from \"state\"\nflow(\"f\", { port: 8000 })\n";
        let mut wrong = Vec::new();
        for (body, expected) in refused {
            let got = parse(&format!("{head}{body}")).map(|_| ()).map_err(|e| e.to_string());
            if got.as_ref().err().map(String::as_str) != Some(expected) {
                wrong.push(format!("{body:?}\n  => {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// An action called without `await` is started, and one awaited is called, as a JavaScript
    /// async function runs to its first `await` before it hands back its promise (the note's §9).
    /// What can't be is refused, saying what to write.
    #[test]
    fn an_action_not_awaited_is_started() {
        let head = "import { flow, route, action, event } from \"holon\"\nimport { charge } from \"flow:billing\"\nflow(\"f\", { port: 8000 })\nexport const pay = action(async (order) => {\n  const paid = await event(\"paid\", { expires: \"1h\" })\n  return paid.method\n})\n";
        let refused = [
            ("route.post(\"/\", async (req) => await pay(req.json))\n", "await pay(…) would wait for this flow's own action to end, which a flow can't yet: start it without await, const started = pay(x)"),
            ("route.post(\"/\", async (req) => {\n  const paid = await event(\"paid\", { expires: \"1h\" })\n  return paid.method\n})\n", ROUTE_WAITS),
            ("route.post(\"/\", (req) => charge(req.json, 2))\n", "an action is sent one value: charge({ … }), a record for more"),
        ];
        for (body, expected) in refused {
            let e = parse(&format!("{head}{body}")).map(|_| ()).unwrap_err().to_string();
            assert!(e.contains(expected), "{body}\n  => {e}");
        }
        let e = parse("import { flow, start } from \"holon\"\nflow(\"f\", { port: 8000 })\n").map(|_| ()).unwrap_err().to_string();
        assert!(e.contains(START_IS_A_CALL), "{e}");
        // Started, the flow's own as another's; awaited, another's is called.
        let started = parse(&format!("{head}route.post(\"/\", (req) => pay(req.json).waiting.paid)\n")).unwrap_or_else(|e| panic!("{e}"));
        let called = parse(&format!("{head}route.post(\"/\", async (req) => (await charge(req.json)).answer)\n")).unwrap_or_else(|e| panic!("{e}"));
        let leaves = |file: &File| format!("{file:?}");
        assert!(leaves(&started).contains("\"start\""), "{started:?}");
        assert!(leaves(&called).contains("Method"), "{called:?}");
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
        assert_eq!(mistake("route.get(\"/\", () => {\n  let n\n  return n\n})\n"), "6:10: \"n\" has no value here: give it one first, let n = …");
        assert_eq!(mistake("route.get(\"/\", (req) => \"a\" + req.query.b)\n"), "4:25: there is no + on texts: use a template literal, `${a}${b}`");
        assert_eq!(mistake("route.get(\"/\", (req) => fetch(req.query.u))\n"), "4:25: unknown function \"fetch\": define it at the top level, or import the leaf from \"holon\"");
        assert_eq!(mistake("route.get(\"/\", (req) => req.query.xs.length)\n"), "4:25: a list's length is length(xs), a text's text_length(s), imported from \"holon\": .length comes in a later slice");
        assert_eq!(mistake("route.fetch(\"/\", () => 1)\n"), "4:7: route has no \"fetch\": get, post, put, patch or delete");
        assert_eq!(mistake("route.get(\"/\", () => 1, { log: \"loud\" })\n"), "4:32: log is \"strict\", \"lazy\" or \"unrecorded\"");
        assert_eq!(mistake("route.get(\"/\", () => {\n  notes = 1\n  return 1\n})\n"), "5:3: \"notes\" is imported, and stays what it is: give a value of its own a name of its own");
        let unknown = parse("import x from \"npm:left-pad\"\n").unwrap_err();
        assert!(unknown.to_string().starts_with("1:15: \"npm:left-pad\" isn't something a flow imports"), "{unknown}");
        let syntax = parse("import { flow } from \"holon\"\nflow(\"f\", {\n").unwrap_err();
        assert_eq!(syntax.span.start.line, 3, "{syntax}");
        assert!(parse("import { route } from \"holon\"\nroute.get(\"/\", () => 1)\n").unwrap_err().message.starts_with("a flow says what it is first"));
    }
}
