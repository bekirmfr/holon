//! A `.holon` text rewritten in TypeScript's syntax (docs/L4s/typescript-syntax-L4.md §14), its
//! comments carried over, laid out as `holon fmt` writes it. Each construct is written the way the
//! TypeScript reader ([`super::ts`]) reads it back, so the two texts lower to the same unit, as the
//! samples did when they were converted with it; a flow without handlers is the exception, as it
//! becomes one (§4). What TypeScript has no form for yet is refused, naming it: a `state.set`
//! whose answer is bound, a `match` inside an expression, the wiring form.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use holon_core::interface_of;

use super::ast::*;
use super::lex::lex;
use super::lower::{alias, LowerError, RESERVED};
use crate::templates;

type R<T> = Result<T, LowerError>;

fn fail<T>(span: Span, message: impl Into<String>) -> R<T> {
    Err(LowerError { span, message: message.into(), declare: None })
}

/// `src`, a `.holon` text, in TypeScript's syntax, laid out.
pub fn convert(src: &str) -> R<String> {
    let src = src.replace("\r\n", "\n");
    let file = super::parse::parse(&src)?;
    let text = Writer::new(&src, &file)?.file()?;
    super::format::format(&text).map_err(|e| LowerError { message: format!("{}\n{text}", e.message), ..e })
}

/// A `#` comment: its line, and its text after the `#`.
struct Comment {
    line: u32,
    text: String,
}

/// The text's comments, in order: each `#` outside a token, to the end of its line.
fn comments(src: &str) -> R<Vec<Comment>> {
    let tokens = lex(src).map_err(|e| LowerError { span: Span { start: e.pos, end: e.pos }, message: e.message, declare: None })?;
    let at = |p: Pos| (p.line, p.col);
    let mut out = Vec::new();
    let mut t = 0;
    for (i, line) in src.lines().enumerate() {
        let n = i as u32 + 1;
        for (col, (b, c)) in line.char_indices().enumerate() {
            let here = (n, col as u32 + 1);
            while t < tokens.len() && at(tokens[t].span.end) <= here {
                t += 1;
            }
            if t < tokens.len() && at(tokens[t].span.start) <= here {
                continue;
            }
            if c == '#' {
                out.push(Comment { line: n, text: line[b + 1..].trim_end().to_string() });
                break;
            }
        }
    }
    Ok(out)
}

// Binding powers, lowest first, as TypeScript's grammar has them.
const COND: u8 = 1;
const COALESCE: u8 = 2;
const OR: u8 = 3;
const AND: u8 = 4;
const EQ: u8 = 5;
const REL: u8 = 6;
const ADD: u8 = 7;
const MUL: u8 = 8;
const UNARY: u8 = 9;
const MEMBER: u8 = 10;
const ATOM: u8 = 11;

/// A handler's kind, for what its parameter is called when the text names none.
#[derive(Clone, Copy, PartialEq)]
enum Block {
    Route,
    Action,
    Every,
    File,
    Function,
}

/// A statement as written, in a block: what it is tied to, its lines with its comments, whether
/// it is the block's answer, and the binding it gives a value inside its `if`, declared before it.
struct Chunk {
    after: Option<String>,
    lines: Vec<String>,
    answers: bool,
    declared: Option<String>,
}

struct Writer<'a> {
    file: &'a File,
    lines: Vec<&'a str>,
    comments: Vec<Comment>,
    /// The first comment not yet written.
    next: usize,
    // What the text uses, imported at its head.
    vocab: BTreeSet<String>,
    state: bool,
    now: bool,
    random: bool,
    /// Each host requested, and the name it is imported by.
    hosts: Vec<(String, String)>,
    /// Each flow called, by its slug, and the actions of it the text calls.
    flows: BTreeMap<String, BTreeSet<String>>,
    // What the top level names.
    holons: HashSet<String>,
    actions: HashSet<String>,
    consts: HashSet<String>,
    /// A called flow's slug, by the name its calls are made through (`billing_v2`).
    calls: HashMap<String, String>,
    stores: HashSet<String>,
    // The handler or function being written.
    /// What `ingress` reads as: the file handler's parameter.
    ingress: Option<String>,
    /// Whether a request or a call is awaited in it, which makes it `async`.
    awaits: bool,
    /// Whether its requests are awaited: a function's aren't, as it computes (slice 7 changes it).
    awaiting: bool,
    /// Inside a channel's `pipe(…)`, whose request streams and isn't awaited.
    piping: bool,
    /// Its parameter, bindings and in-place functions' parameters: a word that is none of these,
    /// nor a top-level name, is a bare word, written in quotes (`encoding: hex`).
    locals: HashSet<String>,
}

impl<'a> Writer<'a> {
    fn new(src: &'a str, file: &'a File) -> R<Writer<'a>> {
        let mut w = Writer {
            file,
            lines: src.lines().collect(),
            comments: comments(src)?,
            next: 0,
            vocab: BTreeSet::new(),
            state: false,
            now: false,
            random: false,
            hosts: Vec::new(),
            flows: BTreeMap::new(),
            holons: HashSet::new(),
            actions: HashSet::new(),
            consts: HashSet::new(),
            calls: HashMap::new(),
            stores: HashSet::new(),
            ingress: None,
            awaits: false,
            awaiting: true,
            piping: false,
            locals: HashSet::new(),
        };
        for h in &file.headers {
            match &h.kind {
                HeaderKind::Collection(n) | HeaderKind::Folder(n) | HeaderKind::Database(n) | HeaderKind::Channel(n) => {
                    w.stores.insert(n.clone());
                }
                HeaderKind::OnFile(f) => {
                    w.stores.insert(f.folder.clone());
                }
                HeaderKind::Calls(slug) => {
                    w.calls.insert(super::lower::call_noun(slug), slug.clone());
                    w.flows.entry(slug.clone()).or_default();
                }
                _ => {}
            }
        }
        for item in &file.items {
            match item {
                Item::Holon(h) => {
                    w.holons.insert(h.name.text.clone());
                }
                Item::Js(j) => {
                    w.holons.insert(j.name.text.clone());
                }
                Item::Action(a) => {
                    w.actions.insert(a.name.text.clone());
                }
                Item::OnFile(o) => {
                    w.stores.insert(o.on.folder.clone());
                }
                Item::Stmt(Stmt { kind: StmtKind::Bind { name, .. }, .. }) => {
                    w.consts.insert(name.text.clone());
                }
                _ => {}
            }
        }
        Ok(w)
    }

    /// The whole text: the comments that head it, the imports, `flow(…)`, then each item.
    fn file(&mut self) -> R<String> {
        let f = self.file;
        let first = f.headers.first().map(|h| h.span.start.line).unwrap_or(u32::MAX);
        let head = self.comments_before(first);
        let settings = self.settings()?;
        // A flow without handlers: its statements are the run of each trigger its header names.
        let handlers = f.items.iter().any(|i| matches!(i, Item::Route(_) | Item::Every(_) | Item::OnFile(_) | Item::Action(_)));
        let triggers: Vec<&HeaderKind> = f.headers.iter().map(|h| &h.kind).filter(|k| matches!(k, HeaderKind::Every { .. } | HeaderKind::OnFile(_))).collect();
        let run: Vec<Stmt> = if handlers { Vec::new() } else { f.items.iter().filter_map(|i| if let Item::Stmt(s) = i { Some(s.clone()) } else { None }).collect() };
        if !run.is_empty() && triggers.is_empty() {
            return fail(run[0].span, "a flow without a handler or a trigger has no TypeScript form: put its statements in a route, every(…) or on.file.created(…)");
        }
        let mut items = Vec::new();
        let mut run_written = false;
        for item in &f.items {
            let text = match item {
                Item::Holon(h) => self.holon(h)?,
                Item::Js(j) => self.js(j)?,
                Item::Route(r) => self.route(r)?,
                Item::Every(e) => self.every(e)?,
                Item::OnFile(o) => self.on_file(o)?,
                Item::Action(a) => self.action(a)?,
                Item::Stmt(s) if handlers => self.top_const(s)?,
                Item::Stmt(_) if run_written => continue,
                Item::Stmt(s) => {
                    run_written = true;
                    let mut lines = self.comments_before(s.span.start.line);
                    lines.push(self.triggered(&triggers, &run)?);
                    lines.join("\n")
                }
            };
            items.push(text);
        }
        let tail = self.comments_before(u32::MAX);
        let mut out = head;
        out.extend(self.imports());
        out.push(String::new());
        out.push(settings);
        for item in items {
            out.push(String::new());
            out.push(item);
        }
        if !tail.is_empty() {
            out.push(String::new());
            out.extend(tail);
        }
        Ok(out.join("\n") + "\n")
    }

    /// The comments before `line` not yet written, each as a line of its own.
    fn comments_before(&mut self, line: u32) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(c) = self.comments.get(self.next).filter(|c| c.line < line) {
            out.push(format!("//{}", c.text));
            self.next += 1;
        }
        out
    }

    /// Whether the line before `line` is blank in the text.
    fn blank_before(&self, line: u32) -> bool {
        line >= 2 && self.lines.get(line as usize - 2).is_some_and(|l| l.trim().is_empty())
    }

    /// `flow("name", { port, browsers, keep, limits })`, from the headers.
    fn settings(&mut self) -> R<String> {
        self.vocab.insert("flow".into());
        let mut name = None;
        let mut fields = Vec::new();
        let mut limits = Vec::new();
        for h in &self.file.headers {
            match &h.kind {
                HeaderKind::Flow(n) => name = Some(n.clone()),
                HeaderKind::Port(p) => fields.push(format!("port: {p}")),
                HeaderKind::Browsers(b) => fields.push(format!("browsers: {}", if b.len() == 1 { quote(&b[0]) } else { format!("[{}]", b.iter().map(|o| quote(o)).collect::<Vec<_>>().join(", ")) })),
                HeaderKind::Keep { runs, days } => {
                    let keep: Vec<String> = [runs.map(|n| format!("runs: {n}")), days.map(|n| format!("days: {n}"))].into_iter().flatten().collect();
                    fields.push(format!("keep: {{ {} }}", keep.join(", ")));
                }
                HeaderKind::Limit { name, value } => limits.push(format!("{name}: {}", limit_value(name, *value))),
                HeaderKind::Interval(_) => return fail(h.span, "the wiring form's interval has no TypeScript form: a clock is every(\"10s\", () => …)"),
                HeaderKind::Optional(Consent::Named { name, .. }) => return fail(h.span, format!("optional {name} has no TypeScript form yet")),
                _ => {}
            }
        }
        if !limits.is_empty() {
            fields.push(format!("limits: {{ {} }}", limits.join(", ")));
        }
        let Some(name) = name else { return fail(Span::default(), "a flow without its flow line has no TypeScript form") };
        Ok(if fields.is_empty() { format!("flow({})", quote(&name)) } else { format!("flow({}, {{ {} }})", quote(&name), fields.join(", ")) })
    }

    /// The imports of what the text uses and declares.
    fn imports(&self) -> Vec<String> {
        let mut out = vec![format!("import {{ {} }} from \"holon\"", self.vocab.iter().cloned().collect::<Vec<_>>().join(", "))];
        if self.state {
            out.push("import state from \"state\"".into());
        }
        for h in &self.file.headers {
            match &h.kind {
                HeaderKind::Secret(s) => out.push(format!("import {s} from \"secret:{s}\"")),
                HeaderKind::Optional(Consent::Secret(s)) => out.push(format!("import {s} from \"secret:{s}\" with {{ optional: \"true\" }}")),
                HeaderKind::Collection(n) => out.push(format!("import {n} from \"collection:{n}\"")),
                HeaderKind::Folder(n) => out.push(format!("import {n} from \"folder:{n}\"")),
                HeaderKind::Database(n) => out.push(format!("import {n} from \"database:{n}\"")),
                HeaderKind::Channel(n) => out.push(format!("import {n} from \"channel:{n}\"")),
                _ => {}
            }
        }
        // A folder an `on file` names is declared by it.
        let declared: HashSet<&String> = self.file.headers.iter().filter_map(|h| if let HeaderKind::Folder(n) = &h.kind { Some(n) } else { None }).collect();
        let mut folders: Vec<&String> = Vec::new();
        for f in self.file.headers.iter().filter_map(|h| if let HeaderKind::OnFile(f) = &h.kind { Some(&f.folder) } else { None }).chain(self.file.items.iter().filter_map(|i| if let Item::OnFile(o) = i { Some(&o.on.folder) } else { None })) {
            if !declared.contains(f) && !folders.contains(&f) {
                folders.push(f);
            }
        }
        for f in folders {
            out.push(format!("import {f} from \"folder:{f}\""));
        }
        for (host, local) in &self.hosts {
            out.push(format!("import {local} from \"host:{host}\""));
        }
        if self.now {
            out.push("import { now } from \"holon:clock\"".into());
        }
        if self.random {
            out.push("import { random } from \"holon:random\"".into());
        }
        for (slug, actions) in &self.flows {
            out.push(format!("import {{ {} }} from \"flow:{slug}\"", actions.iter().cloned().collect::<Vec<_>>().join(", ")));
        }
        out
    }

    // ── the top level ────────────────────────────────────────────────────────────────────────

    /// A binding of the top level: a literal, `const limit = 50`.
    fn top_const(&mut self, s: &Stmt) -> R<String> {
        let StmtKind::Bind { name, value } = &s.kind else {
            return fail(s.span, "the top level of a flow with handlers binds literals: put this in a handler");
        };
        let mut lines = self.comments_before(s.span.start.line);
        self.locals.clear();
        lines.push(format!("const {} = {}", ts_name(&name.text), self.expr(value, COND)?));
        Ok(lines.join("\n"))
    }

    /// `function name(a, b) { … return out }`: a graph holon, its outputs what it returns.
    fn holon(&mut self, h: &HolonDef) -> R<String> {
        let mut lines = self.comments_before(h.span.start.line);
        let params = self.ports(&h.inputs)?;
        self.enter(h.inputs.iter().map(|p| p.name.text.clone()), false);
        let body = self.statements(&h.body, h.span.end.line, Block::Function)?;
        let ret = match h.outputs.as_slice() {
            [one] => format!("return {}", ts_name(&one.name.text)),
            outs => format!("return {{ {} }}", outs.iter().map(|o| ts_name(&o.name.text)).collect::<Vec<_>>().join(", ")),
        };
        for o in &h.outputs {
            if o.unit.is_some() {
                return fail(o.name.span, "an output's name for the unit has no TypeScript form");
            }
            if !h.body.iter().any(|s| matches!(&s.kind, StmtKind::Bind { name, .. } if name.text == o.name.text)) {
                return fail(o.name.span, format!("the output {} isn't bound in the body, which TypeScript can't say", o.name.text));
            }
        }
        let returns = match h.outputs.as_slice() {
            [one] => one.ty.as_ref().map(|t| ts_type(t).map(|t| format!(": {t}"))).transpose()?.unwrap_or_default(),
            _ => String::new(),
        };
        lines.push(format!("function {}({params}){returns} {{\n{body}{}{ret}\n}}", h.name.text, if body.is_empty() { "" } else { "\n" }));
        Ok(lines.join("\n"))
    }

    /// `const name = js(function out(a, b) { … }, { fuel, memory, outputs })`: its body as written.
    fn js(&mut self, j: &JsDef) -> R<String> {
        let mut lines = self.comments_before(j.span.start.line);
        // The body's own comments are part of it.
        while self.comments.get(self.next).is_some_and(|c| c.line <= j.body_span.end.line) {
            self.next += 1;
        }
        self.vocab.insert("js".into());
        let params = self.ports(&j.inputs)?;
        let mut options = Vec::new();
        let budget = |b: &Budget| if b.text.chars().all(|c| c.is_ascii_digit() || c == '_') { b.text.clone() } else { quote(&b.text) };
        if let Some(f) = &j.fuel {
            options.push(format!("fuel: {}", budget(f)));
        }
        if let Some(m) = &j.memory {
            options.push(format!("memory: {}", budget(m)));
        }
        let (fn_name, returns) = match j.outputs.as_slice() {
            [one] => (one.name.text.clone(), one.ty.as_ref().map(|t| ts_type(t).map(|t| format!(": {t}"))).transpose()?.unwrap_or_default()),
            outs => {
                options.push(format!("outputs: [{}]", outs.iter().map(|o| quote(&o.name.text)).collect::<Vec<_>>().join(", ")));
                (j.name.text.clone(), String::new())
            }
        };
        let options = if options.is_empty() { String::new() } else { format!(", {{ {} }}", options.join(", ")) };
        lines.push(format!("const {} = js(function {fn_name}({params}){returns} {{{}}}{options})", j.name.text, j.body));
        Ok(lines.join("\n"))
    }

    /// A holon's parameters, each with its type.
    fn ports(&self, ports: &[PortDecl]) -> R<String> {
        let mut out = Vec::new();
        for p in ports {
            if p.unit.is_some() {
                return fail(p.name.span, "a port's name for the unit has no TypeScript form");
            }
            let ty = p.ty.as_ref().map(|t| ts_type(t).map(|t| format!(": {t}"))).transpose()?.unwrap_or_default();
            out.push(format!("{}{ty}", ts_name(&p.name.text)));
        }
        Ok(out.join(", "))
    }

    /// `route.post("/path", (req) => { … }, { resumes, log })`.
    fn route(&mut self, r: &RouteDef) -> R<String> {
        let mut lines = self.comments_before(r.span.start.line);
        self.vocab.insert("route".into());
        let handler = self.handler(&r.request.text, "req", &r.body, r.span, Block::Route)?;
        let options = options(&[("resumes", r.resumes.as_ref()), ("log", r.log.as_ref())]);
        lines.push(format!("route.{}({}, {handler}{options})", r.method.text.to_ascii_lowercase(), quote(&r.path.text)));
        Ok(lines.join("\n"))
    }

    /// `every("2s", () => { … }, { log })`.
    fn every(&mut self, e: &EveryDef) -> R<String> {
        let mut lines = self.comments_before(e.span.start.line);
        self.vocab.insert("every".into());
        let handler = self.handler("", "", &e.body, e.span, Block::Every)?;
        lines.push(format!("every({}, {handler}{})", quote(&e.text), options(&[("log", e.log.as_ref())])));
        Ok(lines.join("\n"))
    }

    /// `on.file.created(inbox, (f) => { … }, { matching, log })`.
    fn on_file(&mut self, o: &OnFileDef) -> R<String> {
        let mut lines = self.comments_before(o.span.start.line);
        self.vocab.insert("on".into());
        let handler = self.handler(&o.file.text, "f", &o.body, o.span, Block::File)?;
        let matching = o.on.matching.as_ref().map(|m| Name { text: m.clone(), span: o.head });
        let options = options(&[("matching", matching.as_ref()), ("log", o.log.as_ref())]);
        lines.push(format!("on.file.{}({}, {handler}{options})", if o.on.deleted { "deleted" } else { "created" }, o.on.folder));
        Ok(lines.join("\n"))
    }

    /// `export const name = action((input) => { … }, { log })`.
    fn action(&mut self, a: &ActionDef) -> R<String> {
        let mut lines = self.comments_before(a.span.start.line);
        self.vocab.insert("action".into());
        let handler = self.handler(&a.input.text, "input", &a.body, a.span, Block::Action)?;
        lines.push(format!("export const {} = action({handler}{})", a.name.text, options(&[("log", a.log.as_ref())])));
        Ok(lines.join("\n"))
    }

    /// A flow without handlers: its statements as the run of each trigger its header names, a
    /// clock's or a folder's, `ingress` the file handler's parameter.
    fn triggered(&mut self, triggers: &[&HeaderKind], run: &[Stmt]) -> R<String> {
        let span = run[0].span.to(run[run.len() - 1].span);
        // `f = ingress` names the file: that name is the parameter.
        let named = run.iter().find_map(|s| match &s.kind {
            StmtKind::Bind { name, value: Expr { kind: ExprKind::Noun(Noun::Ingress), .. } } if s.after.is_none() => Some(name.text.clone()),
            _ => None,
        });
        let body: Vec<Stmt> = run.iter().filter(|s| !matches!(&s.kind, StmtKind::Bind { value: Expr { kind: ExprKind::Noun(Noun::Ingress), .. }, .. }) || named.is_none()).cloned().collect();
        let param = named.unwrap_or_else(|| if run.iter().any(|s| matches!(&s.kind, StmtKind::Bind { name, .. } if name.text == "f")) { "file".into() } else { "f".into() });
        let start = self.next;
        let mut out = Vec::new();
        for t in triggers {
            // Each handler holds the run; its comments are written with the first.
            let first = self.next;
            self.next = start.max(self.next.min(first));
            let text = match t {
                HeaderKind::Every { text, .. } => {
                    self.vocab.insert("every".into());
                    format!("every({}, {})", quote(text), self.handler("", "", &body, span, Block::Every)?)
                }
                HeaderKind::OnFile(on) => {
                    self.vocab.insert("on".into());
                    self.ingress = Some(param.clone());
                    let handler = self.handler(&param, &param, &body, span, Block::File);
                    self.ingress = None;
                    let matching = on.matching.as_ref().map(|m| Name { text: m.clone(), span });
                    format!("on.file.{}({}, {}{})", if on.deleted { "deleted" } else { "created" }, on.folder, handler?, options(&[("matching", matching.as_ref())]))
                }
                _ => unreachable!("a trigger"),
            };
            out.push(text);
        }
        Ok(out.join("\n\n"))
    }

    // ── handlers ─────────────────────────────────────────────────────────────────────────────

    /// What the next handler or function is written with: its parameters, its bindings.
    fn enter(&mut self, params: impl IntoIterator<Item = String>, awaiting: bool) {
        self.locals = params.into_iter().collect();
        self.awaits = false;
        self.awaiting = awaiting;
    }

    /// A handler's function: `(req) => { … }`, `() => value` when it only answers, `async` when
    /// it awaits. Its parameter is left out when nothing reads it and it has the name the reader
    /// gives one left out (`fallback`).
    fn handler(&mut self, param: &str, fallback: &str, body: &[Stmt], span: Span, block: Block) -> R<String> {
        let ingress = self.ingress.clone();
        self.enter([param.to_string()], true);
        self.ingress = ingress;
        let ingress = |e: &Expr| matches!(&e.kind, ExprKind::Noun(Noun::Ingress)) || matches!(&e.kind, ExprKind::Name(n) if n == "ingress");
        let read = !param.is_empty() && (reads(body, param) || (self.ingress.is_some() && body.iter().any(|s| stmt_exprs(s).iter().any(|e| has(e, &ingress)))));
        let params = if block == Block::Every || (!read && param == fallback) { "()".to_string() } else { format!("({})", ts_name(param)) };
        let inside = self.comments.get(self.next).is_some_and(|c| c.line < span.end.line);
        // A handler that only answers is its answer; an object alone, in parentheses.
        if let [Stmt { kind: StmtKind::Respond(args), after: None, .. }] = body
            && !inside
            && !args.iter().any(|a| matches!(a.value.kind, ExprKind::Match { .. }))
        {
            let object = matches!(args.as_slice(), [Arg { name: None, value: Expr { kind: ExprKind::Record { .. }, .. } }]);
            let value = self.answer(args, body[0].span)?;
            if object || !value.starts_with('{') {
                let value = if object { format!("({value})") } else { value };
                let asy = if self.awaits { "async " } else { "" };
                return Ok(format!("{asy}{params} => {value}"));
            }
        }
        let text = self.statements(body, span.end.line, block)?;
        let asy = if self.awaits { "async " } else { "" };
        Ok(format!("{asy}{params} => {{\n{text}\n}}"))
    }

    /// A block's statements, its answer last, each with its comments, then the comments before
    /// its end. Statements tied to the same value (`after x`) share one `if (x !== undefined)`.
    fn statements(&mut self, body: &[Stmt], end: u32, block: Block) -> R<String> {
        for s in body {
            if let StmtKind::Bind { name, .. } = &s.kind {
                self.locals.insert(name.text.clone());
            }
        }
        let mut chunks: Vec<Chunk> = Vec::new();
        for (i, s) in reads_first(body).into_iter().enumerate() {
            let mut lines = Vec::new();
            let first = self.comments.get(self.next).filter(|c| c.line < s.span.start.line).map(|c| c.line).unwrap_or(s.span.start.line);
            if i > 0 && self.blank_before(first) {
                lines.push(String::new());
            }
            lines.extend(self.comments_before(s.span.start.line));
            let after = s.after.as_ref().map(|x| self.expr(x, EQ)).transpose()?;
            let answers = matches!(s.kind, StmtKind::Respond(_));
            if answers && block == Block::Function {
                return fail(s.span, "a function returns its outputs: respond is a handler's");
            }
            if answers && after.is_some() {
                return fail(s.span, "a respond tied to a value with after has no TypeScript form: answer with a ?? b");
            }
            let declared = match &s.kind {
                StmtKind::Bind { name, .. } if s.after.is_some() => Some(ts_name(&name.text)),
                _ => None,
            };
            lines.extend(self.statement(s)?);
            chunks.push(Chunk { after, lines, answers, declared });
        }
        if chunks.iter().filter(|c| c.answers).count() > 1 {
            return fail(body[0].span, "a handler answers once: one respond");
        }
        // The answer is the handler's return, so it comes last.
        if let Some(i) = chunks.iter().position(|c| c.answers) {
            let mut answer = chunks.remove(i);
            if answer.lines.first().is_some_and(|l| l.is_empty()) && chunks.is_empty() {
                answer.lines.remove(0);
            }
            chunks.push(answer);
        }
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        while i < chunks.len() {
            let Some(after) = chunks[i].after.clone() else {
                out.extend(chunks[i].lines.iter().cloned());
                i += 1;
                continue;
            };
            let mut group = vec![];
            let mut lets = vec![];
            while i < chunks.len() && chunks[i].after.as_ref() == Some(&after) {
                group.push(chunks[i].lines.clone());
                lets.extend(chunks[i].declared.clone());
                i += 1;
            }
            let blank = group[0].first().is_some_and(|l| l.is_empty());
            if blank {
                out.push(String::new());
            }
            // The first statement's comments say what the block does, so they come before it.
            let mut lines: Vec<String> = group.into_iter().flatten().skip(usize::from(blank)).collect();
            let said = lines.iter().take_while(|l| l.starts_with("//")).count();
            out.extend(lines.drain(..said));
            // One short statement goes on the `if`'s line: `if (saved !== undefined) state.last = id`.
            if let [line] = lines.as_slice()
                && lets.is_empty()
                && !line.starts_with("//")
                && line.len() + after.len() < 80
            {
                out.push(format!("if ({after} !== undefined) {line}"));
                continue;
            }
            // A binding given its value inside is declared before it: `let y`, then `y = …`.
            out.extend(lets.into_iter().map(|n| format!("let {n}")));
            out.push(format!("if ({after} !== undefined) {{"));
            out.extend(lines);
            out.push("}".into());
        }
        out.extend(self.comments_before(end));
        Ok(out.join("\n"))
    }

    /// One statement's lines.
    fn statement(&mut self, s: &Stmt) -> R<Vec<String>> {
        Ok(match &s.kind {
            // Tied to a value, it is declared before the `if` that ties it ([`Self::statements`]).
            StmtKind::Bind { name, value } if s.after.is_some() => vec![format!("{} = {}", ts_name(&name.text), self.expr(value, COND)?)],
            StmtKind::Bind { name, value } => {
                match &value.kind {
                    ExprKind::Match { .. } => return fail(value.span, "a match outside a respond has no TypeScript form here yet: write it as a switch"),
                    ExprKind::Method { receiver, .. } if receiver.text == "state" => {
                        return fail(value.span, format!("{} = state.set(…): TypeScript writes the state with state.key = value, whose answer isn't bound", name.text));
                    }
                    _ => vec![format!("const {} = {}", ts_name(&name.text), self.expr(value, COND)?)],
                }
            }
            StmtKind::StateWrite { key, value } => {
                self.state = true;
                vec![format!("state.{} = {}", key.text, self.expr(value, COND)?)]
            }
            StmtKind::CollectionWrite { collection, key, value } => vec![format!("{}[{}] = {}", collection.text, self.expr(key, COND)?, self.expr(value, COND)?)],
            StmtKind::Delete { collection, key } => vec![format!("delete {}[{}]", collection.text, self.expr(key, COND)?)],
            StmtKind::Respond(args) => match args.as_slice() {
                [Arg { name: None, value: Expr { kind: ExprKind::Match { subject, arms, els, .. }, .. } }, rest @ ..] => self.switch(subject, arms, els.as_deref(), rest, s.span)?,
                _ => vec![format!("return {}", self.answer(args, s.span)?)],
            },
            StmtKind::Expr(e) => vec![self.expr(e, COND)?],
            StmtKind::Node { .. } | StmtKind::Wire { .. } => return fail(s.span, "the wiring form (node, a.out -> b.in) has no TypeScript form"),
        })
    }

    /// `respond match s { p -> v … }`: a switch, each case returning its arm's answer.
    fn switch(&mut self, subject: &Expr, arms: &[Arm], els: Option<&Expr>, rest: &[Arg], span: Span) -> R<Vec<String>> {
        let mut out = vec![format!("switch ({}) {{", self.expr(subject, COND)?)];
        let answer = |w: &mut Self, value: &Expr| -> R<String> {
            let mut args = vec![Arg { name: None, value: value.clone() }];
            args.extend(rest.iter().cloned());
            w.answer(&args, span)
        };
        for arm in arms {
            for p in &arm.patterns {
                out.push(format!("case {}:", self.expr(p, COND)?));
            }
            out.push(format!("return {}", answer(self, &arm.value)?));
        }
        if let Some(e) = els {
            out.push("default:".into());
            out.push(format!("return {}", answer(self, e)?));
        }
        out.push("}".into());
        if els.is_none()
            && let Some(m) = rest.iter().find(|a| a.name.as_ref().is_some_and(|n| n.text == "missing"))
        {
            self.vocab.insert("status".into());
            out.push(format!("return status({})", self.expr(&m.value, COND)?));
        }
        Ok(out)
    }

    /// What a respond answers, as a return's value: the body, `status(code, body)`,
    /// `respond(body, { headers, stream, status })`, then `?? status(code)` for an absent one.
    fn answer(&mut self, args: &[Arg], span: Span) -> R<String> {
        let mut body = None;
        let mut named: Vec<(&str, &Expr)> = Vec::new();
        for a in args {
            match &a.name {
                None if body.is_none() => body = Some(&a.value),
                None => return fail(a.value.span, "respond answers one value"),
                Some(n) => named.push((n.text.as_str(), &a.value)),
            }
        }
        let get = |k: &str| named.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
        if let Some((n, v)) = named.iter().find(|(n, _)| !matches!(*n, "status" | "missing" | "headers" | "stream")) {
            return fail(v.span, format!("respond has no {n}"));
        }
        let (mut status, missing, headers, stream) = (get("status"), get("missing"), get("headers"), get("stream"));
        // `respond 204`: a number alone is the status.
        if status.is_none()
            && let Some(b @ Expr { kind: ExprKind::Number { .. }, .. }) = body
        {
            status = Some(b);
            body = None;
        }
        let value = if headers.is_some() || stream.is_some() {
            self.vocab.insert("respond".into());
            let Some(body) = body else { return fail(span, "respond(…) answers a body") };
            let mut options = Vec::new();
            for (k, v) in [("headers", headers), ("stream", stream), ("status", status)] {
                if let Some(v) = v {
                    options.push(format!("{k}: {}", self.expr(v, COND)?));
                }
            }
            (format!("respond({}, {{ {} }})", self.expr(body, COND)?, options.join(", ")), MEMBER)
        } else if let Some(code) = status {
            self.vocab.insert("status".into());
            match body {
                Some(b) => (format!("status({}, {})", self.expr(code, COND)?, self.expr(b, COND)?), MEMBER),
                None => (format!("status({})", self.expr(code, COND)?), MEMBER),
            }
        } else {
            match body {
                Some(b) => self.expr_at(b)?,
                None => return fail(span, "a respond with nothing to answer has no TypeScript form: return a value"),
            }
        };
        Ok(match missing {
            Some(code) => {
                self.vocab.insert("status".into());
                let left = if value.1 == COALESCE || value.1 >= EQ { value.0 } else { format!("({})", value.0) };
                format!("{left} ?? status({})", self.expr(code, COND)?)
            }
            None => value.0,
        })
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    /// `e`, in parentheses when it binds less tightly than `min`.
    fn expr(&mut self, e: &Expr, min: u8) -> R<String> {
        let (text, power) = self.expr_at(e)?;
        Ok(if power < min { format!("({text})") } else { text })
    }

    /// `e` and how tightly it binds.
    fn expr_at(&mut self, e: &Expr) -> R<(String, u8)> {
        Ok(match &e.kind {
            ExprKind::Number { text, .. } => (text.clone(), if text.starts_with('-') { UNARY } else { ATOM }),
            ExprKind::Duration { text, .. } => return fail(e.span, format!("a duration in an expression has no TypeScript form: {text} is a number of seconds there")),
            ExprKind::Text(s) => (quote(s), ATOM),
            ExprKind::Interpolated(parts) => {
                let mut out = String::from("`");
                for p in parts {
                    match p {
                        Part::Text(t) => out.push_str(&template_text(t)),
                        Part::Expr(x) => out.push_str(&format!("${{{}}}", self.expr(x, 0)?)),
                    }
                }
                out.push('`');
                (out, ATOM)
            }
            ExprKind::Bool(b) => (b.to_string(), ATOM),
            // `ingress.name` reads the word as a name.
            ExprKind::Name(n) if n == "ingress" && self.ingress.is_some() && !self.locals.contains(n) => (ts_name(self.ingress.as_deref().expect("checked")), ATOM),
            ExprKind::Name(n) => (ts_name(n), ATOM),
            ExprKind::Secret(s) => (s.clone(), ATOM),
            ExprKind::State(k) => {
                self.state = true;
                (format!("state.{k}"), MEMBER)
            }
            ExprKind::Noun(Noun::Now) => {
                self.now = true;
                ("now".into(), ATOM)
            }
            ExprKind::Noun(Noun::Random) => {
                self.random = true;
                ("random".into(), ATOM)
            }
            ExprKind::Noun(Noun::Ingress) => match &self.ingress {
                Some(n) => (ts_name(n), ATOM),
                None => return fail(e.span, "ingress outside a file handler has no TypeScript form: it is the handler's parameter, (f) => …"),
            },
            ExprKind::Call { callee, args } => self.call(callee, args, e.span)?,
            ExprKind::Method { receiver, method, args } => self.method(receiver, method, args, e.span)?,
            ExprKind::Field { base, name } => (format!("{}.{}", self.expr(base, MEMBER)?, name.text), MEMBER),
            ExprKind::Index { base, key } => (format!("{}[{}]", self.expr(base, MEMBER)?, self.expr(key, 0)?), MEMBER),
            ExprKind::Record { spread, fields } => {
                let mut out = Vec::new();
                if let Some(s) = spread {
                    out.push(format!("...{}", self.expr(s, COND)?));
                }
                for (key, value) in fields {
                    let k = if key.quoted || !is_identifier(&key.text) { quote(&key.text) } else { key.text.clone() };
                    let v = self.expr(value, COND)?;
                    out.push(if !key.quoted && v == key.text { v } else { format!("{k}: {v}") });
                }
                (if out.is_empty() { "{}".into() } else { format!("{{ {} }}", out.join(", ")) }, ATOM)
            }
            ExprKind::List(items) => {
                let items = items.iter().map(|i| self.expr(i, COND)).collect::<R<Vec<_>>>()?;
                (format!("[{}]", items.join(", ")), ATOM)
            }
            ExprKind::Not(x) => (format!("!{}", self.expr(x, UNARY)?), UNARY),
            ExprKind::Binary { op: BinOp::When, lhs, rhs } => (format!("{} ? {} : undefined", self.expr(rhs, COALESCE)?, self.expr(lhs, COND)?), COND),
            ExprKind::Binary { op, lhs, rhs } => {
                let (power, text) = match op {
                    BinOp::Coalesce => (COALESCE, "??"),
                    BinOp::Or => (OR, "||"),
                    BinOp::And => (AND, "&&"),
                    BinOp::Eq => (EQ, "==="),
                    BinOp::Ne => (EQ, "!=="),
                    BinOp::Lt => (REL, "<"),
                    BinOp::Le => (REL, "<="),
                    BinOp::Gt => (REL, ">"),
                    BinOp::Ge => (REL, ">="),
                    BinOp::Add => (ADD, "+"),
                    BinOp::Sub => (ADD, "-"),
                    BinOp::Mul => (MUL, "*"),
                    BinOp::Div => (MUL, "/"),
                    BinOp::When => unreachable!("written above"),
                };
                // `??` takes no `||` or `&&` beside it unparenthesized.
                let left = match (op, &lhs.kind) {
                    (BinOp::Coalesce, ExprKind::Binary { op: BinOp::Coalesce, .. }) => self.expr(lhs, COALESCE)?,
                    (BinOp::Coalesce, _) => self.expr(lhs, EQ)?,
                    _ => self.expr(lhs, power)?,
                };
                let right = self.expr(rhs, if *op == BinOp::Coalesce { EQ } else { power + 1 })?;
                (format!("{left} {text} {right}"), power)
            }
            ExprKind::If { cond, then, els } => {
                let els = match els {
                    Some(x) => self.expr(x, COND)?,
                    None => "undefined".into(),
                };
                (format!("{} ? {} : {els}", self.expr(cond, COALESCE)?, self.expr(then, COND)?), COND)
            }
            ExprKind::Match { .. } => return fail(e.span, "a match inside an expression has no TypeScript form yet: bind it, or answer it, as a switch"),
            ExprKind::Await { name, expires } => {
                self.vocab.insert("event".into());
                self.awaits = true;
                let options = match expires.as_deref() {
                    None => String::new(),
                    Some(Expr { kind: ExprKind::Duration { text, .. }, .. }) => format!(", {{ expires: {} }}", quote(text)),
                    Some(x) => return fail(x.span, "an await's expires is a duration written out: expires: 1d"),
                };
                (format!("await event({}{options})", quote(&name.text)), UNARY)
            }
            ExprKind::Lambda { .. } => return fail(e.span, "a function written in place goes to map or fold"),
            ExprKind::Paren(x) => (format!("({})", self.expr(x, 0)?), ATOM),
        })
    }

    /// A call: a holon's, an action started, or a leaf's, the library's methods where TypeScript
    /// has them (`JSON.parse`, `xs.map(…)`, `s.toUpperCase()`).
    fn call(&mut self, callee: &Name, args: &[Arg], span: Span) -> R<(String, u8)> {
        let name = callee.text.as_str();
        let positional: Vec<&Expr> = args.iter().filter(|a| a.name.is_none()).map(|a| &a.value).collect();
        let named: Vec<(&str, &Expr)> = args.iter().filter_map(|a| a.name.as_ref().map(|n| (n.text.as_str(), &a.value))).collect();
        if name == "start" {
            let [target, rest @ ..] = positional.as_slice() else { return fail(span, "start names the action it starts") };
            let action = match &target.kind {
                ExprKind::Name(a) if self.actions.contains(a) => a.clone(),
                ExprKind::Field { base, name: action } if matches!(&base.kind, ExprKind::Name(n) if self.calls.contains_key(n)) => {
                    let ExprKind::Name(n) = &base.kind else { unreachable!("matched") };
                    self.flows.entry(self.calls[n].clone()).or_default().insert(action.text.clone());
                    action.text.clone()
                }
                _ => return fail(target.span, "start names an action: its own, or a called flow's"),
            };
            return Ok((format!("{action}({})", self.list(rest)?), MEMBER));
        }
        if self.holons.contains(name) {
            if let Some((n, v)) = named.first() {
                return fail(v.span, format!("a holon's {n}: passed by name has no TypeScript form; pass it in order"));
            }
            return Ok((format!("{name}({})", self.list(&positional)?), MEMBER));
        }
        let function = named.iter().find(|(n, _)| *n == "fn").map(|(_, v)| *v);
        let only_fn = named.iter().all(|(n, _)| *n == "fn");
        match (alias(name), positional.as_slice(), function) {
            ("json_parse", [a], None) if named.is_empty() => return Ok((format!("JSON.parse({})", self.expr(a, COND)?), MEMBER)),
            ("json_stringify", [a], None) if named.is_empty() => return Ok((format!("JSON.stringify({})", self.expr(a, COND)?), MEMBER)),
            ("keys", [a], None) if named.is_empty() => return Ok((format!("Object.keys({})", self.expr(a, COND)?), MEMBER)),
            ("uppercase", [a], None) if named.is_empty() => return Ok((format!("{}.toUpperCase()", self.expr(a, MEMBER)?), MEMBER)),
            ("lowercase", [a], None) if named.is_empty() => return Ok((format!("{}.toLowerCase()", self.expr(a, MEMBER)?), MEMBER)),
            (m @ ("split" | "join" | "index_of"), [a, b], None) if named.is_empty() => {
                let method = if m == "index_of" { "indexOf" } else { m };
                return Ok((format!("{}.{method}({})", self.expr(a, MEMBER)?, self.expr(b, COND)?), MEMBER));
            }
            ("map", [xs], Some(f)) if only_fn => return Ok((format!("{}.map({})", self.expr(xs, MEMBER)?, self.function(f, 1)?), MEMBER)),
            ("fold", [init, xs], Some(f)) if only_fn => {
                return Ok((format!("{}.reduce({}, {})", self.expr(xs, MEMBER)?, self.function(f, 2)?, self.expr(init, COND)?), MEMBER));
            }
            ("map" | "fold", ..) if function.is_some() => return fail(span, format!("{name} with more than its function has no TypeScript form yet")),
            _ => {}
        }
        self.vocab.insert(name.to_string());
        Ok((format!("{name}({})", self.args(&positional, &named)?), MEMBER))
    }

    /// The function map or fold takes: a holon by name, or `(x) => …` written in place.
    fn function(&mut self, f: &Expr, takes: usize) -> R<String> {
        match &f.kind {
            ExprKind::Name(n) if self.holons.contains(n) => Ok(n.clone()),
            ExprKind::Lambda { params, body } => {
                if params.len() != takes {
                    return fail(f.span, "map's function takes the item, fold's the result so far and the item");
                }
                let added: Vec<String> = params.iter().map(|p| p.text.clone()).filter(|p| self.locals.insert(p.clone())).collect();
                let body = self.expr(body, COND);
                for p in added {
                    self.locals.remove(&p);
                }
                let body = body?;
                let body = if body.starts_with('{') { format!("({body})") } else { body };
                Ok(format!("({}) => {body}", params.iter().map(|p| ts_name(&p.text)).collect::<Vec<_>>().join(", ")))
            }
            _ => fail(f.span, "map and fold take a function: fn(x) -> …, or a holon's name"),
        }
    }

    /// `receiver.method(args)`: a request, a called flow's action, a store's method, a channel's.
    fn method(&mut self, receiver: &Name, method: &Name, args: &[Arg], span: Span) -> R<(String, u8)> {
        let positional: Vec<&Expr> = args.iter().filter(|a| a.name.is_none()).map(|a| &a.value).collect();
        let named: Vec<(&str, &Expr)> = args.iter().filter_map(|a| a.name.as_ref().map(|n| (n.text.as_str(), &a.value))).collect();
        let r = receiver.text.as_str();
        match r {
            "http" => {
                let [Expr { kind: ExprKind::Text(host), .. }, rest @ ..] = positional.as_slice() else {
                    return fail(span, "a request's host is written out: http.get(\"api.example.com\", \"/path\")");
                };
                let local = self.host(host);
                let call = format!("{local}.{}({})", method.text, self.args(rest, &named)?);
                if self.piping || !self.awaiting {
                    Ok((call, MEMBER))
                } else {
                    self.awaits = true;
                    Ok((format!("await {call}"), UNARY))
                }
            }
            "stripe" => {
                self.vocab.insert("stripe".into());
                Ok((format!("stripe.{}({})", method.text, self.args(&positional, &named)?), MEMBER))
            }
            "state" => fail(span, "state.set(…) has no TypeScript form: write state.key = value"),
            r if self.calls.contains_key(r) => {
                if !named.is_empty() {
                    return fail(span, "an action is sent one value");
                }
                self.flows.entry(self.calls[r].clone()).or_default().insert(method.text.clone());
                let call = format!("{}({})", method.text, self.list(&positional)?);
                if self.awaiting {
                    self.awaits = true;
                    Ok((format!("await {call}"), UNARY))
                } else {
                    Ok((call, MEMBER))
                }
            }
            r if self.stores.contains(r) => {
                let piping = std::mem::replace(&mut self.piping, method.text == "pipe");
                let args = self.args(&positional, &named);
                self.piping = piping;
                Ok((format!("{r}.{}({})", method.text, args?), MEMBER))
            }
            _ => fail(receiver.span, format!("{r}.{}(…) has no TypeScript form here", method.text)),
        }
    }

    /// Values in order, comma-separated.
    fn list(&mut self, values: &[&Expr]) -> R<String> {
        Ok(values.iter().map(|v| self.expr(v, COND)).collect::<R<Vec<_>>>()?.join(", "))
    }

    /// Arguments in order, then the named ones as an object last: `notes.list({ limit: 50 })`.
    /// A bare word given by name is a text (`encoding: hex`).
    fn args(&mut self, positional: &[&Expr], named: &[(&str, &Expr)]) -> R<String> {
        let mut out = vec![self.list(positional)?].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
        if !named.is_empty() {
            let mut fields = Vec::new();
            for (k, v) in named {
                let key = if is_identifier(k) { k.to_string() } else { quote(k) };
                let value = match &v.kind {
                    ExprKind::Name(n) if !self.known(n) => quote(n),
                    _ => self.expr(v, COND)?,
                };
                fields.push(if value == key { value } else { format!("{key}: {value}") });
            }
            out.push(format!("{{ {} }}", fields.join(", ")));
        }
        Ok(out.join(", "))
    }

    /// Whether `name` names a value here: a local, a holon, a const or a store.
    fn known(&self, name: &str) -> bool {
        self.locals.contains(name) || self.holons.contains(name) || self.consts.contains(name) || self.stores.contains(name)
    }

    /// The name a host is imported by: its first label that says something (`api.anthropic.com`
    /// is `anthropic`), its last label added when two hosts would share it.
    fn host(&mut self, host: &str) -> String {
        if let Some((_, local)) = self.hosts.iter().find(|(h, _)| h == host) {
            return local.clone();
        }
        let labels: Vec<&str> = host.split('.').collect();
        let meaningful: Vec<&str> = labels.iter().take(labels.len().saturating_sub(1).max(1)).copied().filter(|l| !matches!(*l, "api" | "www")).collect();
        let base = identifier(meaningful.first().copied().unwrap_or(host));
        let mut local = base.clone();
        if self.hosts.iter().any(|(_, l)| *l == local) || self.known(&local) || taken(&local) {
            local = format!("{base}_{}", identifier(labels.last().copied().unwrap_or("host")));
        }
        self.hosts.push((host.to_string(), local.clone()));
        local
    }
}

/// A handler's options as its call's last argument: `, { resumes: "paid", log: "lazy" }`.
fn options(options: &[(&str, Option<&Name>)]) -> String {
    let set: Vec<String> = options.iter().filter_map(|(k, v)| v.map(|v| format!("{k}: {}", quote(&v.text)))).collect();
    if set.is_empty() { String::new() } else { format!(", {{ {} }}", set.join(", ")) }
}

/// A limit's value as `flow(…)` takes it: a duration in quotes for one counted in milliseconds or
/// seconds, when it is a whole number of seconds, else the number.
fn limit_value(name: &str, value: u64) -> String {
    let seconds = if name.ends_with("_ms") && value.is_multiple_of(1000) && value > 0 {
        value / 1000
    } else if name.ends_with("_seconds") && value > 0 {
        value
    } else {
        return value.to_string();
    };
    let text = [(86400, 'd'), (3600, 'h'), (60, 'm')].iter().find(|(unit, _)| seconds.is_multiple_of(*unit)).map(|(unit, c)| format!("{}{c}", seconds / unit)).unwrap_or_else(|| format!("{seconds}s"));
    quote(&text)
}

/// A port's type as TypeScript writes it.
fn ts_type(t: &TypeExpr) -> R<String> {
    Ok(match (t.name.as_str(), t.args.as_slice()) {
        ("Number", []) => "number".into(),
        ("Text", []) => "string".into(),
        ("Boolean", []) => "boolean".into(),
        ("Any", []) => "unknown".into(),
        ("Record", []) => "Record<string, unknown>".into(),
        ("List", [item]) => {
            let item = ts_type(item)?;
            if item.chars().all(|c| c.is_ascii_alphanumeric()) { format!("{item}[]") } else { format!("Array<{item}>") }
        }
        ("Product", items) => format!("[{}]", items.iter().map(ts_type).collect::<R<Vec<_>>>()?.join(", ")),
        _ => return fail(t.span, format!("the type {} has no TypeScript form", t.name)),
    })
}

/// Whether the `.holon` form keeps `name` for itself, a leaf's or a keyword, so a TypeScript name
/// for it takes an underscore after it there.
fn taken(name: &str) -> bool {
    RESERVED.contains(&name) || interface_of(alias(name)).is_some() || templates::is_template(name)
}

/// A `.holon` name as TypeScript writes it: `json_`, the `.holon` form of a word it keeps, is
/// `json`.
fn ts_name(name: &str) -> String {
    match name.strip_suffix('_') {
        Some(word) if taken(word) => word.to_string(),
        _ => name.to_string(),
    }
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(|c: char| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `s` made an identifier: `postman-echo` is `postman_echo`.
fn identifier(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) { format!("_{out}") } else { out }
}

/// A text in double quotes.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A text part of a template literal.
fn template_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() && c != '\n' => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The statements in the text's order, but a write to a collection moved after the later ones
/// that read it: a write is made when the run ends, so `.holon` reads what was there before it
/// wherever the read is written, and TypeScript, which reads in order, refuses a read after it.
fn reads_first(body: &[Stmt]) -> Vec<&Stmt> {
    let written = |s: &Stmt| match &s.kind {
        StmtKind::CollectionWrite { collection, .. } | StmtKind::Delete { collection, .. } => Some(collection.text.clone()),
        _ => None,
    };
    let reads = |s: &Stmt, store: &str| {
        written(s).as_deref() != Some(store)
            && stmt_exprs(s).iter().any(|e| {
                has(e, &|e| match &e.kind {
                    ExprKind::Index { base, .. } => matches!(&base.kind, ExprKind::Name(n) if n == store),
                    ExprKind::Method { receiver, method, .. } => receiver.text == store && matches!(method.text.as_str(), "get" | "list" | "lines" | "slice"),
                    _ => false,
                })
            })
    };
    let mut out: Vec<&Stmt> = body.iter().collect();
    let mut i = 0;
    while i < out.len() {
        if let Some(store) = written(out[i])
            && let Some(last) = (i + 1..out.len()).rev().find(|&j| reads(out[j], &store))
        {
            let w = out.remove(i);
            out.insert(last, w);
            continue;
        }
        i += 1;
    }
    out
}

/// The expressions a statement holds, its `after` among them.
fn stmt_exprs(s: &Stmt) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = s.after.iter().collect();
    match &s.kind {
        StmtKind::Bind { value, .. } | StmtKind::StateWrite { value, .. } | StmtKind::Expr(value) => out.push(value),
        StmtKind::CollectionWrite { key, value, .. } => out.extend([key, value]),
        StmtKind::Delete { key, .. } => out.push(key),
        StmtKind::Respond(args) => out.extend(args.iter().map(|a| &a.value)),
        StmtKind::Node { .. } | StmtKind::Wire { .. } => {}
    }
    out
}

/// Whether a statement of `body` reads `name`.
fn reads(body: &[Stmt], name: &str) -> bool {
    body.iter().any(|s| stmt_exprs(s).iter().any(|e| has(e, &|e| matches!(&e.kind, ExprKind::Name(n) if n == name))))
}

/// Whether `e` or an expression inside it is one `f` picks.
fn has(e: &Expr, f: &dyn Fn(&Expr) -> bool) -> bool {
    if f(e) {
        return true;
    }
    let args = |args: &[Arg]| args.iter().any(|a| has(&a.value, f));
    match &e.kind {
        ExprKind::Interpolated(parts) => parts.iter().any(|p| matches!(p, Part::Expr(x) if has(x, f))),
        ExprKind::Call { args: a, .. } | ExprKind::Method { args: a, .. } => args(a),
        ExprKind::Field { base, .. } => has(base, f),
        ExprKind::Index { base, key } => has(base, f) || has(key, f),
        ExprKind::Record { spread, fields } => spread.as_deref().is_some_and(|s| has(s, f)) || fields.iter().any(|(_, v)| has(v, f)),
        ExprKind::List(items) => items.iter().any(|i| has(i, f)),
        ExprKind::Not(x) | ExprKind::Paren(x) => has(x, f),
        ExprKind::Binary { lhs, rhs, .. } => has(lhs, f) || has(rhs, f),
        ExprKind::If { cond, then, els } => has(cond, f) || has(then, f) || els.as_deref().is_some_and(|x| has(x, f)),
        ExprKind::Match { subject, arms, els, .. } => has(subject, f) || arms.iter().any(|a| a.patterns.iter().any(|p| has(p, f)) || has(&a.value, f)) || els.as_deref().is_some_and(|x| has(x, f)),
        ExprKind::Await { expires, .. } => expires.as_deref().is_some_and(|x| has(x, f)),
        ExprKind::Lambda { body, .. } => has(body, f),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Settings;
    use crate::composites::Names;
    use holon_core::MapResolver;

    /// A text's flow name, its server unit's hash and its settings, as a deploy derives them.
    fn deployed(what: &str, lowered: R<super::super::Lowered>) -> (Option<String>, Option<String>, Option<Settings>) {
        let lowered = lowered.unwrap_or_else(|e| panic!("{what}: {e}"));
        let mut store = MapResolver::new();
        crate::text::restore(&lowered.doc, &mut store, &Names::new());
        let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
        let server = crate::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
        assert!(server.errors.is_empty(), "{what}: {:?}", server.errors);
        (lowered.name, server.server_hash, lowered.doc.server)
    }

    #[test]
    fn a_text_converts_as_typescript_writes_it() {
        let holon = "# Visits.\nflow visits\nport 8080\nstate last\ncollection seen\n\n# One visit, kept.\nroute POST /v/:id (req) {\n  id = req.params.id\n  saved = seen.put(id, now)\n  state.last = id after saved\n\n  # Its items, priced.\n  respond map(req.json.items, fn: fn(x) -> { name: x.name }) when id != \"\", missing: 400\n}\n\naction count (input) lazy {\n  respond fold(\"\", keys(seen.list()), fn: fn(acc, k) -> \"{acc}{k}\")\n}\n";
        let ts = "// Visits.\nimport { action, flow, route, status } from \"holon\"\nimport state from \"state\"\nimport seen from \"collection:seen\"\nimport { now } from \"holon:clock\"\n\nflow(\"visits\", { port: 8080 })\n\n// One visit, kept.\nroute.post(\"/v/:id\", (req) => {\n  const id = req.params.id\n  const saved = seen.put(id, now)\n  if (saved !== undefined) state.last = id\n\n  // Its items, priced.\n  return (id !== \"\" ? req.json.items.map((x) => ({ name: x.name })) : undefined) ?? status(400)\n})\n\nexport const count = action(\n  () => Object.keys(seen.list()).reduce((acc, k) => `${acc}${k}`, \"\"),\n  { log: \"lazy\" },\n)\n";
        assert_eq!(convert(holon).unwrap(), ts);
        assert_eq!(deployed("visits.holon.ts", super::super::ts::compile(ts)), deployed("visits.holon", super::super::lower::compile(holon)), "the same unit");
    }

    #[test]
    fn what_typescript_has_no_form_for_is_refused() {
        let refused = |src: &str| convert(src).unwrap_err().message;
        assert!(refused("flow f\nport 8080\nholon h(x) -> (a, b) {\n  a = state.set(k, x)\n  b = x\n}\nroute GET /a (req) {\n  respond h(1).b\n}\n").contains("state.set"));
        assert!(refused("flow f\nport 8080\nroute GET /a (req) {\n  n = match req.query.op {\n    \"a\" -> 1\n  }\n  respond n\n}\n").contains("a match outside a respond"));
    }

    #[test]
    fn a_text_s_form_is_told_by_how_it_starts() {
        use super::super::is_typescript;
        assert!(is_typescript("import { flow } from \"holon\"\n"));
        assert!(is_typescript("// Shop.\nimport { flow } from \"holon\"\n"));
        assert!(is_typescript("\u{feff}flow(\"shop\")\n"));
        assert!(!is_typescript("# Shop.\nflow shop\n"));
        assert!(!is_typescript("flow shop\nport 8080\n"));
        assert!(!is_typescript("route GET /a (req) {\n  respond 1\n}\n"));
        assert!(!is_typescript(""));
    }
}
