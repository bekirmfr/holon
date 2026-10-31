//! Lowering: the syntax tree of ast.rs onto the flow builder (docs/LANGUAGE.md §5). Every
//! expression becomes nodes from the live catalog and wires between them; a binding is a named
//! wire; a holon is a composite frozen with its declared boundary; a route is a `request` node
//! and everything wired to it. The nouns and statements for the outside world are sugar over one
//! leaf each (§5.4), and the wiring form's `node` and wire statements lower beside them, so one
//! file may hold both.
//!
//! What comes out is a flow document, the same one the canvas saves, so `flow_unit` and
//! `derive_server` apply unchanged and a text's hash is its graph's. Every error carries the span
//! of the expression at fault.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use holon_core::{interface_of, PASSES_EXTRA, TRIGGER, VARIADIC};
use serde_json::json;

use super::ast::*;
use super::parse::{parse, ParseError};
use crate::builder::{FlowBuilder, FlowDocument, Holon, HolonPort, Requirements, Settings};
use crate::canvas::next_port_name;
use crate::templates;
use crate::wire::{WParam, WPort, WType};

#[derive(Clone, Debug, PartialEq)]
pub struct LowerError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.span.start.line, self.span.start.col, self.message)
    }
}

impl From<ParseError> for LowerError {
    fn from(e: ParseError) -> LowerError {
        LowerError { span: Span { start: e.pos, end: e.pos }, message: e.message }
    }
}

type R<T> = Result<T, LowerError>;

fn err<T>(span: Span, message: impl Into<String>) -> R<T> {
    Err(LowerError { span, message: message.into() })
}

/// A text lowered: the flow's name, its document, each top-level node's name in the text by its
/// id (bindings and `node` names), and each node's span by its id.
#[derive(Clone, Debug)]
pub struct Lowered {
    pub name: Option<String>,
    pub doc: FlowDocument,
    pub names: BTreeMap<String, String>,
    pub spans: BTreeMap<String, Span>,
}

/// The source a `js` node's engine compiles: `(inputs) => { body }`, the declared names and the
/// body verbatim, so the body's line 1 is the statement's header line in the file.
pub fn js_source(def: &JsDef) -> String {
    let params: Vec<&str> = def.inputs.iter().map(|p| p.name.text.as_str()).collect();
    format!("({}) => {{{}}}", params.join(", "), def.body)
}

/// Parse and lower a text.
pub fn compile(src: &str) -> R<Lowered> {
    lower(&parse(src)?)
}

/// Lower a parsed text.
pub fn lower(file: &File) -> R<Lowered> {
    Lowerer::new(file).run(file)
}

/// Words a binding may not take: the keywords and nouns of the language, and the wiring form's.
const RESERVED: &[&str] = &[
    "flow", "port", "every", "browsers", "optional", "collection", "folder", "database", "channel", "holon", "js", "route", "resumes", "respond", "delete", "await", "if", "then", "else",
    "when", "match", "fn", "true", "false", "node", "at", "in", "out", "interval", "secret", "state", "now", "random", "ingress", "http", "stripe", "json",
    "parse_json",
];

/// The name a flow's actions are called by (`calls billing-v2`, `billing_v2.charge(x)`): its slug
/// with `-` as `_`.
pub(crate) fn call_noun(flow: &str) -> String {
    flow.replace('-', "_")
}

/// Plain names for two leaves, read as English.
fn alias(name: &str) -> &str {
    match name {
        "json" => "json_stringify",
        "parse_json" => "json_parse",
        n => n,
    }
}

/// An `on file` line as the log names its handler: `on file inbox deleted`.
fn file_label(f: &FileOn) -> String {
    holon_core::FileTrigger { folder: f.folder.clone(), deleted: f.deleted, matching: f.matching.clone().unwrap_or_default() }.label()
}

/// Whether an `http_stream` node pipes its answer to a channel (`updates.pipe(…)`), rather than
/// forwarding it to the route's caller.
fn piped(d: &crate::canvas::NodeData) -> bool {
    crate::wire::frozen_value(&d.params, "pipe").is_some_and(|c| !c.is_empty())
}

/// Inputs a node works without, left unwired (routes.rs `is_optional_input`, core's
/// `has_optional_ports`).
fn is_optional(leaf: &str, input: &str) -> bool {
    input == TRIGGER
        || matches!((leaf, input), ("http_request" | "http_stream", "headers" | "body") | ("http_upload", "headers") | ("flow_call", "input") | ("collection_list" | "file_list", "prefix" | "after") | ("file_lines", "from"))
}

/// A declared collection's or folder's leaves, and the names of what they take: `c[k]` reads with
/// `get`, `c[k] = v` writes with `put`, `delete c[k]` with `delete`; `c.list(…)` with `list`.
struct Store {
    /// What it is, as an error says it: "collection", "folder".
    kind: &'static str,
    /// The frozen param that names it.
    param: &'static str,
    get: &'static str,
    put: &'static str,
    delete: &'static str,
    list: &'static str,
    /// Its leaves' inputs: what names an item, and what is written.
    key: &'static str,
    value: &'static str,
}

const COLLECTION: Store = Store {
    kind: "collection",
    param: "collection",
    get: "collection_get",
    put: "collection_put",
    delete: "collection_delete",
    list: "collection_list",
    key: "key",
    value: "value",
};

/// A folder an admin binds on the host (`docs/L4s/named-folders-L4.md`): files by name, as text.
const FOLDER: Store = Store {
    kind: "folder",
    param: "folder",
    get: "file_read",
    put: "file_write",
    delete: "file_delete",
    list: "file_list",
    key: "name",
    value: "text",
};

/// A number as a param's text: whole numbers without a fraction.
fn number_text(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() }
}

/// A value on a wire: a node's output, or, inside a holon or a function, one of its inputs.
#[derive(Clone, Debug)]
enum Val {
    /// A node's output `port`; `explicit` when `.name` picked it out of several, so a further
    /// `.name` is a field and a binding of it names no node.
    Out { id: String, outputs: Vec<WPort>, port: usize, explicit: bool },
    In(String),
}

impl Val {
    /// Whether this is, or carries, an effect the host performs: an Effect output, or a
    /// multi-output node with one.
    fn effectful(&self) -> bool {
        match self {
            Val::Out { outputs, port, .. } => outputs.get(*port).is_some_and(|p| p.ty == WType::Effect) || (outputs.len() > 1 && outputs.iter().any(|p| p.ty == WType::Effect)),
            Val::In(_) => false,
        }
    }
}

enum FrameKind {
    /// The flow's own frame.
    Flow { has_routes: bool },
    /// A route's block: its request node, and where it responded.
    Route { request: String, responded: Option<Span> },
    /// An `every` block: its tick node, whose `now` is the time of the tick.
    Every { tick: String },
    /// An `on file` block: the name of the file's handle, which `ingress` would be in the old form.
    OnFile { file: String },
    /// An action's block: its `action` node, the name of what was sent, and where it responded.
    Action { node: String, input: String, responded: Option<Span> },
    /// A holon's block: its declared outputs, in order, and what each is bound to.
    Holon { declared: Vec<String>, bound: HashMap<String, (Val, Span)>, effects: Vec<(Val, Span)> },
    /// A function's body: the names it uses from around it, in first use, each with its value
    /// there. Each becomes an input of the function after its parameters, which `map` or `fold`
    /// is wired to pass (a capture lifted to an input: the function stays a sealed unit).
    Lambda { captures: Vec<(String, Val)> },
}

struct Frame {
    kind: FrameKind,
    env: HashMap<String, Val>,
    /// A holon's boundary wiring, as `end_holon_exact` takes it: `(port, node, input)`.
    wiring_in: Vec<(String, String, String)>,
    /// `(node, output, port)`.
    wiring_out: Vec<(String, String, String)>,
    /// State keys written in this block, by key: one write per key.
    written: HashMap<String, Span>,
    /// Boundary inputs used more than once, by port: the pass-through node their uses share.
    shared: HashMap<String, Val>,
    /// The nodes placed in this block, in order.
    created: Vec<String>,
    /// Whether any wiring-form statement appeared (a holon with none of its own boundary wires
    /// then freezes with the proposed boundary, as the wiring form always did).
    wiring_form: bool,
}

impl Frame {
    fn new(kind: FrameKind) -> Frame {
        Frame { kind, env: HashMap::new(), wiring_in: vec![], wiring_out: vec![], written: HashMap::new(), shared: HashMap::new(), created: vec![], wiring_form: false }
    }
}

/// A handler block's trigger node, the nodes it placed, its header's span, and how a stray's
/// error names the trigger and what to use of it (`the tick`, `now`).
type Block = (String, Vec<String>, Span, (&'static str, String));

struct Lowerer<'a> {
    b: FlowBuilder,
    defs: HashMap<String, &'a HolonDef>,
    /// `js` nodes by name: lowered like holons (`holon`), placed like holons (`place`).
    js_defs: HashMap<String, &'a JsDef>,
    holons: HashMap<String, Holon>,
    in_progress: HashSet<String>,
    collections: HashSet<String>,
    /// The folders the header declares (`folder inbox`): read and written like collections.
    folders: HashSet<String>,
    /// The databases the header declares (`database shop`): `shop.query(…)`, `shop.one(…)`,
    /// `shop.exec(…)`.
    databases: HashSet<String>,
    /// The channels the header declares (`channel updates`): sent on, and subscribed to as a
    /// route's answer.
    channels: HashSet<String>,
    /// The flows the header names (`calls billing`), by the noun that calls them (`billing`, or
    /// `billing_v2` for `billing-v2`): their slugs.
    calls: HashMap<String, String>,
    /// The streamed call (`http_stream`) the current route's `respond …, stream: true` forwards.
    streamed: Option<String>,
    /// The channel the http call being lowered pipes to (`updates.pipe(http.post(…))`).
    piping: Option<String>,
    /// The `on file` lines: `ingress` is the handle of the file that started the run.
    on_files: Vec<FileOn>,
    /// The `ingress` chains of a flow with several `on file` lines (a `coalesce` of their
    /// `file_arrival`s): a handle, as each `file_arrival` is.
    arrivals: HashSet<String>,
    /// Top-level bindings of literals in a flow with routes: lowered afresh in each route.
    consts: HashMap<String, &'a Expr>,
    frames: Vec<Frame>,
    names: BTreeMap<String, String>,
    spans: BTreeMap<String, Span>,
    lambdas: usize,
    /// Each route's request node, the nodes its block placed, and its header's span: a route is
    /// everything wired to its request, so a node left unwired to it would silently fall out.
    routes: Vec<(String, Vec<String>, Span)>,
    /// Each `every` or `on file` block, likewise.
    ticks: Vec<Block>,
}

impl<'a> Lowerer<'a> {
    fn new(file: &'a File) -> Lowerer<'a> {
        let defs = file.items.iter().filter_map(|i| if let Item::Holon(h) = i { Some((h.name.text.clone(), h)) } else { None }).collect();
        let js_defs = file.items.iter().filter_map(|i| if let Item::Js(j) = i { Some((j.name.text.clone(), j)) } else { None }).collect();
        let collections = file.headers.iter().filter_map(|h| if let HeaderKind::Collection(c) = &h.kind { Some(c.clone()) } else { None }).collect();
        let on_files = file.headers.iter().filter_map(|h| if let HeaderKind::OnFile(f) = &h.kind { Some(f.clone()) } else { None }).collect();
        let databases = file.headers.iter().filter_map(|h| if let HeaderKind::Database(d) = &h.kind { Some(d.clone()) } else { None }).collect();
        let channels = file.headers.iter().filter_map(|h| if let HeaderKind::Channel(c) = &h.kind { Some(c.clone()) } else { None }).collect();
        let calls = file.headers.iter().filter_map(|h| if let HeaderKind::Calls(f) = &h.kind { Some((call_noun(f), f.clone())) } else { None }).collect();
        let folders = file
            .headers
            .iter()
            .filter_map(|h| match &h.kind {
                HeaderKind::Folder(f) => Some(f.clone()),
                HeaderKind::OnFile(f) => Some(f.folder.clone()),
                _ => None,
            })
            .chain(file.items.iter().filter_map(|i| if let Item::OnFile(o) = i { Some(o.on.folder.clone()) } else { None }))
            .collect();
        Lowerer {
            b: FlowBuilder::new(),
            defs,
            js_defs,
            holons: HashMap::new(),
            in_progress: HashSet::new(),
            collections,
            folders,
            databases,
            channels,
            calls,
            streamed: None,
            piping: None,
            on_files,
            arrivals: HashSet::new(),
            consts: HashMap::new(),
            frames: vec![],
            names: BTreeMap::new(),
            spans: BTreeMap::new(),
            lambdas: 0,
            routes: vec![],
            ticks: vec![],
        }
    }

    /// Whether `n` names a holon or a `js` node: either is placed by name, and neither may be
    /// shadowed.
    fn is_def(&self, n: &str) -> bool {
        self.defs.contains_key(n) || self.js_defs.contains_key(n)
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame")
    }

    fn cur(&self) -> &Frame {
        self.frames.last().expect("a frame")
    }

    fn run(mut self, file: &'a File) -> R<Lowered> {
        // Holons and js nodes twice defined (or one of each under a name).
        let mut seen = HashSet::new();
        for i in &file.items {
            let name = match i {
                Item::Holon(h) => &h.name,
                Item::Js(j) => &j.name,
                _ => continue,
            };
            if !seen.insert(name.text.as_str()) {
                return err(name.span, format!("there is a holon \"{}\" already", name.text));
            }
        }
        // A flow of handlers (routes, `every`, `on file` and `action` blocks) keeps its statements in them.
        let has_routes = file.items.iter().any(|i| matches!(i, Item::Route(_) | Item::Every(_) | Item::OnFile(_) | Item::Action(_)));
        let actions: Vec<&ActionDef> = file.items.iter().filter_map(|i| if let Item::Action(a) = i { Some(a) } else { None }).collect();
        for (i, a) in actions.iter().enumerate() {
            if actions[..i].iter().any(|o| o.name.text == a.name.text) {
                return err(a.head, format!("action {} is written twice: one block per name", a.name.text));
            }
        }
        let blocks: Vec<&EveryDef> = file.items.iter().filter_map(|i| if let Item::Every(e) = i { Some(e) } else { None }).collect();
        let file_blocks: Vec<&OnFileDef> = file.items.iter().filter_map(|i| if let Item::OnFile(o) = i { Some(o) } else { None }).collect();
        if has_routes
            && let Some(h) = file.headers.iter().find(|h| matches!(h.kind, HeaderKind::Every { .. } | HeaderKind::Interval(_)))
        {
            // The header is the one-handler form's clock: beside handlers it would start nothing.
            if !(blocks.is_empty() && file_blocks.is_empty()) {
                return err(h.span, "a flow with `every` or `on file` blocks has its clocks in them: put the clock's statements in an `every … { … }` block");
            }
            let beside = if file.items.iter().any(|i| matches!(i, Item::Route(_))) { "routes" } else { "actions" };
            return err(h.span, format!("this `every` line is the clock of a flow without handlers: beside {beside}, put the clock's statements in an `every … {{ … }}` block"));
        }
        for (i, b) in blocks.iter().enumerate() {
            if blocks[..i].iter().any(|o| o.seconds == b.seconds) {
                return err(b.head, format!("{} is written twice: one block per period", b.text));
            }
        }
        for (i, b) in file_blocks.iter().enumerate() {
            if let Some(g) = &b.on.matching
                && !holon_core::is_glob(g)
            {
                return err(b.head, format!("{g:?} isn't a pattern of names: `*` and `?` within a folder, `**/` for any folders, `/` between them"));
            }
            if file_blocks[..i].iter().any(|o| o.on == b.on) {
                return err(b.head, format!("{} is written twice: one block per line", file_label(&b.on)));
            }
        }
        // The header is the one-handler form's file line: beside handlers, the file's run is a block.
        if let Some(h) = file.headers.iter().find(|h| matches!(h.kind, HeaderKind::OnFile(_)))
            && has_routes
        {
            let HeaderKind::OnFile(f) = &h.kind else { unreachable!() };
            return err(h.span, format!("a flow with handlers has its triggers in them: put the file's statements in an `{} (f) {{ … }}` block, reading the file as f", file_label(f)));
        }
        let on_file: Vec<&Header> = file.headers.iter().filter(|h| matches!(h.kind, HeaderKind::OnFile(_))).collect();
        for (i, h) in on_file.iter().enumerate() {
            let HeaderKind::OnFile(f) = &h.kind else { continue };
            if let Some(g) = &f.matching
                && !holon_core::is_glob(g)
            {
                return err(h.span, format!("{g:?} isn't a pattern of names: `*` and `?` within a folder, `**/` for any folders, `/` between them"));
            }
            if on_file[..i].iter().any(|o| o.kind == h.kind) {
                return err(h.span, "this `on file` line is written twice");
            }
        }
        self.frames.push(Frame::new(FrameKind::Flow { has_routes }));
        // Top-level statements first (the wiring form's nodes, a flow's body, literal constants),
        // then the routes.
        for i in &file.items {
            if let Item::Stmt(s) = i {
                if has_routes {
                    match &s.kind {
                        StmtKind::Bind { name, value } if literal_param(value).is_some() => {
                            self.check_bindable(name)?;
                            self.consts.insert(name.text.clone(), value);
                        }
                        StmtKind::Bind { .. } => return err(s.span, "routes are separate units; put shared logic in a holon (only a literal may be bound at the top level)"),
                        StmtKind::Node { .. } | StmtKind::Wire { .. } => self.stmt(s)?,
                        _ => return err(s.span, "with routes, a statement goes inside a route"),
                    }
                } else {
                    self.stmt(s)?;
                }
            }
        }
        for i in &file.items {
            match i {
                Item::Route(r) => self.route(r)?,
                Item::Every(e) => self.every(e)?,
                Item::OnFile(o) => self.on_file(o)?,
                Item::Action(a) => self.action(a)?,
                _ => {}
            }
        }
        // Holons and js nodes nobody placed are still defined (and checked).
        let mut names: Vec<(String, Span)> = self.defs.iter().map(|(n, d)| (n.clone(), d.name.span)).collect();
        names.extend(self.js_defs.iter().map(|(n, d)| (n.clone(), d.name.span)));
        names.sort_by_key(|a| (a.1.start.line, a.1.start.col));
        for (name, span) in names {
            if !self.holons.contains_key(&name) {
                self.holon(&name, span)?;
            }
        }
        if let Some(h) = on_file.first()
            && !self.spans.keys().any(|id| self.b.data(id).is_some_and(|d| d.name == "file_arrival"))
        {
            return err(h.span, "`on file` starts a run for each new file, and the run reads it as `ingress`: use `ingress` (its .name, say)");
        }
        let settings = self.settings(file)?;
        let name = file.headers.iter().find_map(|h| if let HeaderKind::Flow(n) = &h.kind { Some(n.clone()) } else { None });
        let mut doc = self.b.finish(settings);
        super::layout::lay_out(&mut doc);
        for node in &mut doc.nodes {
            if let Some(s) = self.spans.get(&node.id) {
                node.extra.insert("source".into(), json!({ "line": s.start.line, "col": s.start.col, "endLine": s.end.line, "endCol": s.end.col }));
            }
        }
        // A route is the connected group holding its request: a statement in its block that
        // doesn't reach the request isn't part of the route, and would be left out of the server.
        let ids: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
        let groups = crate::routes::components(&ids, &doc.edges);
        for (request, created, head) in &self.routes {
            let Some(group) = groups.iter().find(|g| g.contains(request)) else { continue };
            if let Some(stray) = created.iter().find(|id| !group.contains(id)) {
                let span = self.spans.get(stray).copied().unwrap_or(*head);
                return err(span, "this isn't wired to the route's request, so it isn't part of the route (a route is everything wired to its request): use the request in it, or move it to a flow of its own");
            }
        }
        for (tick, created, head, (what, use_)) in &self.ticks {
            let Some(group) = groups.iter().find(|g| g.contains(tick)) else { continue };
            if let Some(stray) = created.iter().find(|id| !group.contains(id)) {
                let span = self.spans.get(stray).copied().unwrap_or(*head);
                return err(span, format!("this isn't wired to {what}, so it isn't part of the block (a block is everything wired to its trigger): write something with it, or use {use_} in it"));
            }
        }
        Ok(Lowered { name, doc, names: self.names, spans: self.spans })
    }

    fn settings(&self, file: &File) -> R<Settings> {
        let mut s = Settings::default();
        let mut optional = Vec::new();
        let mut limits = serde_json::Map::new();
        let mut kept = false;
        for h in &file.headers {
            match &h.kind {
                // The limits the deploy settings keep them as (permissions.rs).
                HeaderKind::Keep { runs, days } => {
                    if std::mem::replace(&mut kept, true) {
                        return err(h.span, "one keep line says how long the log keeps runs: this is a second");
                    }
                    limits.extend(runs.map(|n| ("keep_runs".to_string(), n.into())));
                    limits.extend(days.map(|d| ("keep_run_days".to_string(), d.into())));
                }
                HeaderKind::Port(p) => s.port = p.clone(),
                HeaderKind::Every { seconds, .. } => s.interval = number_text(*seconds),
                HeaderKind::Interval(i) => s.interval = i.clone(),
                HeaderKind::Browsers(o) => s.browsers = Some(o.join(" ")).filter(|b| !b.is_empty()),
                HeaderKind::Optional(c) => optional.push(match c {
                    Consent::Secret(n) => format!("SecretRead({n:?})"),
                    Consent::Named { name, arg: Some(a) } => format!("{name}({a:?})"),
                    Consent::Named { name, arg: None } => name.clone(),
                }),
                HeaderKind::Flow(_) | HeaderKind::Collection(_) | HeaderKind::Folder(_) | HeaderKind::Database(_) | HeaderKind::Channel(_) | HeaderKind::OnFile(_) => {}
                HeaderKind::Calls(flow) => self.check_calls(flow, h.span, file)?,
            }
        }
        let mut requirements = Requirements { optional: Some(optional).filter(|o| !o.is_empty()), ..Default::default() };
        if !limits.is_empty() {
            requirements.extra.insert("limits".into(), limits.into());
        }
        if requirements != Requirements::default() {
            s.requirements = Some(requirements);
        }
        Ok(s)
    }

    /// Whether `calls flow` can be a name: a flow's name (letters, digits, `-` and `_`, as a
    /// deployment's slug is), whose noun is a name of its own, which nothing else in the file has.
    fn check_calls(&self, flow: &str, span: Span, file: &File) -> R<()> {
        let noun = call_noun(flow);
        let name = flow.len() <= 64 && flow.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') && noun.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
        if !name {
            return err(span, format!("\"{flow}\" isn't a flow's name: letters, digits, - and _, starting with a letter"));
        }
        let taken = RESERVED.contains(&noun.as_str())
            || interface_of(alias(&noun)).is_some()
            || templates::is_template(&noun)
            || self.is_def(&noun)
            || self.store(&noun).is_some()
            || self.databases.contains(&noun)
            || self.channels.contains(&noun);
        if taken {
            return err(span, format!("\"{noun}\" is already a name here (a keyword, a leaf, a holon or something declared): a flow called by it can't be named so"));
        }
        let twice = file.headers.iter().filter(|h| matches!(&h.kind, HeaderKind::Calls(f) if call_noun(f) == noun)).count();
        if twice > 1 {
            return err(span, format!("`calls` names \"{noun}\" twice"));
        }
        Ok(())
    }

    // ── nodes and wires ──────────────────────────────────────────────────────────────────────

    /// A node from the catalog or the templates, with params, at `span`.
    fn node(&mut self, leaf: &str, params: &[(String, String)], span: Span) -> R<Val> {
        let id = self.b.node(leaf, params, None).or_else(|e| err(span, e))?;
        self.spans.insert(id.clone(), span);
        self.frame().created.push(id.clone());
        Ok(self.out(&id))
    }

    fn out(&self, id: &str) -> Val {
        let outputs = self.b.data(id).map(|d| d.outputs.clone()).unwrap_or_default();
        Val::Out { id: id.into(), outputs, port: 0, explicit: false }
    }

    /// Wire `v` into `to`'s `input`; a holon's or function's input is recorded for its boundary.
    ///
    /// A boundary input feeds exactly one inner port (core promotes one edge per input), so an
    /// input used twice passes through one node the uses share: `guard(true, input)`, the
    /// identity on presence. It is placed on the second use, and the first use is moved onto it.
    fn connect(&mut self, v: &Val, to: &str, input: &str, span: Span) -> R<()> {
        match v {
            Val::Out { id, outputs, port, .. } => {
                let output = outputs.get(*port).map(|p| p.name.clone()).unwrap_or_else(|| "out".into());
                self.b.wire(id, &output, to, input).or_else(|e| err(span, e))
            }
            Val::In(port) => {
                if let Some(shared) = self.cur().shared.get(port).cloned() {
                    return self.connect(&shared, to, input, span);
                }
                let first = self.cur().wiring_in.iter().position(|(p, _, _)| p == port);
                let Some(k) = first else {
                    self.frame().wiring_in.push((port.clone(), to.into(), input.into()));
                    return Ok(());
                };
                let always = self.node("const", &[("value".into(), "true".into()), ("type".into(), "Boolean".into())], span)?;
                let pass = self.node_with("guard", &[], &[("cond", always)], span)?;
                let Val::Out { id: pass_id, .. } = &pass else { unreachable!() };
                let pass_id = pass_id.clone();
                let (_, first_node, first_input) = std::mem::replace(&mut self.frame().wiring_in[k], (port.clone(), pass_id.clone(), "value".into()));
                self.b.wire(&pass_id, "out", &first_node, &first_input).or_else(|e| err(span, e))?;
                self.b.wire(&pass_id, "out", to, input).or_else(|e| err(span, e))?;
                self.frame().shared.insert(port.clone(), pass);
                Ok(())
            }
        }
    }

    /// A node with its inputs wired from values, in port order, growing a variadic leaf as needed.
    fn node_with(&mut self, leaf: &str, params: &[(String, String)], inputs: &[(&str, Val)], span: Span) -> R<Val> {
        let v = self.node(leaf, params, span)?;
        let Val::Out { id, .. } = &v else { unreachable!() };
        let id = id.clone();
        if inputs.len() > 2 && VARIADIC.contains(&leaf) {
            self.b.arity(&id, inputs.len()).or_else(|e| err(span, e))?;
        }
        for (port, val) in inputs {
            self.connect(val, &id, port, span)?;
        }
        Ok(self.out(&id))
    }

    /// The input names of a variadic leaf grown to `n`.
    fn variadic_ports(first: &[WPort], n: usize) -> Vec<String> {
        let mut names: Vec<String> = first.iter().map(|p| p.name.clone()).collect();
        while names.len() < n {
            let existing: Vec<&str> = names.iter().map(String::as_str).collect();
            names.push(next_port_name(&existing));
        }
        names
    }

    // ── statements ───────────────────────────────────────────────────────────────────────────

    fn check_bindable(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if interface_of(n).is_some() || templates::is_template(n) {
            return err(name.span, format!("\"{n}\" is a leaf; a binding may not shadow it"));
        }
        if RESERVED.contains(&n) {
            return err(name.span, format!("\"{n}\" is a keyword; a binding may not shadow it"));
        }
        if let Some(s) = self.store(n) {
            return err(name.span, format!("\"{n}\" is a {}; a binding may not shadow it", s.kind));
        }
        if self.databases.contains(n) {
            return err(name.span, format!("\"{n}\" is a database; a binding may not shadow it"));
        }
        if self.channels.contains(n) {
            return err(name.span, format!("\"{n}\" is a channel; a binding may not shadow it"));
        }
        if self.calls.contains_key(n) {
            return err(name.span, format!("\"{n}\" is a flow; a binding may not shadow it"));
        }
        if self.is_def(n) {
            return err(name.span, format!("\"{n}\" is a holon; a binding may not shadow it"));
        }
        if self.cur().env.contains_key(n) {
            return err(name.span, format!("there is a \"{n}\" already"));
        }
        Ok(())
    }

    /// A statement, its sinks tied to what `after` names when it has one; the rest are tied to the
    /// block's trigger when the block ends (`route`, `holon`).
    fn stmt(&mut self, s: &'a Stmt) -> R<()> {
        let Some(after) = &s.after else { return self.stmt_kind(s) };
        let before = self.cur().created.len();
        self.stmt_kind(s)?;
        let placed: Vec<String> = self.cur().created[before..].to_vec();
        let sinks: Vec<String> = placed.into_iter().filter(|id| self.untied(id)).collect();
        if sinks.is_empty() {
            return err(after.span, "`after` ties a write or a call to what it names: this statement makes none");
        }
        let v = self.expr(after)?;
        for id in &sinks {
            self.connect(&v, id, TRIGGER, after.span)?;
        }
        Ok(())
    }

    /// Whether node `id` takes a trigger (a sink, or a holon with one) that nothing feeds yet.
    fn untied(&self, id: &str) -> bool {
        self.b.data(id).is_some_and(|d| d.inputs.iter().any(|p| p.name == TRIGGER))
            && !self.b.is_wired(id, TRIGGER)
            && !self.cur().wiring_in.iter().any(|(_, node, input)| node == id && input == TRIGGER)
    }

    /// Every sink placed in this block and not tied by `after`, tied to `v`: the block's trigger.
    fn tie_block(&mut self, v: &Val, span: Span) -> R<bool> {
        let untied: Vec<String> = self.cur().created.iter().filter(|id| self.untied(id)).cloned().collect();
        for id in &untied {
            self.connect(v, id, TRIGGER, span)?;
        }
        Ok(!untied.is_empty())
    }

    fn stmt_kind(&mut self, s: &'a Stmt) -> R<()> {
        match &s.kind {
            StmtKind::Bind { name, value } => {
                // In a holon, a declared output is bound here.
                let output = match &self.cur().kind {
                    FrameKind::Holon { declared, bound, .. } if declared.contains(&name.text) => Some(bound.contains_key(&name.text)),
                    _ => None,
                };
                if let Some(already) = output {
                    if already {
                        return err(name.span, format!("the output \"{}\" is bound twice", name.text));
                    }
                    let v = self.expr(value)?;
                    if let Val::In(_) = v {
                        return err(value.span, "an output can't be an input passed straight through: put a node between");
                    }
                    if let FrameKind::Holon { bound, .. } = &mut self.frame().kind {
                        bound.insert(name.text.clone(), (v, s.span));
                    }
                    return Ok(());
                }
                self.check_bindable(name)?;
                let v = self.expr(value)?;
                if let Val::Out { id, port: 0, explicit: false, .. } = &v {
                    self.names.entry(id.clone()).or_insert_with(|| name.text.clone());
                }
                self.frame().env.insert(name.text.clone(), v);
                Ok(())
            }
            StmtKind::StateWrite { key, value } => {
                if let Some(first) = self.frame().written.get(&key.text) {
                    return err(key.span, format!("state.{} is written twice (first on line {}); a run writes a key once", key.text, first.start.line));
                }
                self.frame().written.insert(key.text.clone(), s.span);
                let v = self.expr(value)?;
                let w = self.node_with("store_write", &[("key".into(), key.text.clone())], &[("value", v)], s.span)?;
                self.effect(w, s.span)
            }
            StmtKind::CollectionWrite { collection, key, value } => {
                let store = self.check_collection(collection)?;
                let k = self.expr(key)?;
                let v = self.expr(value)?;
                let named = [(store.param.into(), collection.text.clone())];
                // A file's handle written to a folder is a copy: its bytes go host to host.
                let handle = self.is_handle(&v);
                let w = if store.kind == "folder" && handle {
                    self.node_with("file_copy", &named, &[("name", k), ("from", v)], s.span)?
                } else {
                    self.node_with(store.put, &named, &[(store.key, k), (store.value, v)], s.span)?
                };
                self.effect(w, s.span)
            }
            StmtKind::Delete { collection, key } => {
                let store = self.check_collection(collection)?;
                let k = self.expr(key)?;
                let w = self.node_with(store.delete, &[(store.param.into(), collection.text.clone())], &[(store.key, k)], s.span)?;
                self.effect(w, s.span)
            }
            StmtKind::Respond(args) => self.respond(args, s.span),
            StmtKind::Expr(e) => {
                let v = self.expr(e)?;
                let top_without_routes = matches!(self.cur().kind, FrameKind::Flow { has_routes: false });
                // A call to another flow's action is made for what it does, whatever it answers.
                let a_call = matches!(&v, Val::Out { id, .. } if self.b.data(id).is_some_and(|d| d.name == "flow_call"));
                if v.effectful() || a_call || top_without_routes {
                    return self.effect(v, s.span);
                }
                err(s.span, "a value nobody uses: bind it, or remove it")
            }
            StmtKind::Node { name, of, arity, params, at: _ } => {
                self.frame().wiring_form = true;
                if name.text == "in" || name.text == "out" {
                    return err(name.span, format!("\"{}\" names a holon's boundary: call the node something else", name.text));
                }
                if self.cur().env.contains_key(&name.text) {
                    return err(name.span, format!("there is a node \"{}\" already", name.text));
                }
                let id = if self.is_def(&of.text) {
                    if !params.is_empty() {
                        return err(of.span, format!("the holon \"{}\" takes no params", of.text));
                    }
                    let h = self.holon(&of.text, of.span)?;
                    self.b.place(&h, None)
                } else {
                    let set: Vec<(String, String)> = params.iter().map(|(k, v)| (k.text.clone(), v.clone())).collect();
                    self.b.node(&of.text, &set, None).or_else(|e| err(of.span, e))?
                };
                if let Some(n) = arity {
                    self.b.arity(&id, *n).or_else(|e| err(name.span, e))?;
                }
                self.spans.insert(id.clone(), s.span);
                self.names.insert(id.clone(), name.text.clone());
                let v = self.out(&id);
                self.frame().env.insert(name.text.clone(), v);
                Ok(())
            }
            StmtKind::Wire { from, to } => {
                self.frame().wiring_form = true;
                let in_holon = matches!(self.cur().kind, FrameKind::Holon { .. });
                if from.0.text == "in" && in_holon {
                    let dst = self.node_id(&to.0)?;
                    self.frame().wiring_in.push((from.1.text.clone(), dst, to.1.text.clone()));
                    return Ok(());
                }
                if to.0.text == "out" && in_holon {
                    let src = self.node_id(&from.0)?;
                    self.frame().wiring_out.push((src, from.1.text.clone(), to.1.text.clone()));
                    return Ok(());
                }
                let src = self.node_id(&from.0)?;
                let dst = self.node_id(&to.0)?;
                self.b.wire(&src, &from.1.text, &dst, &to.1.text).or_else(|e| err(s.span, e))
            }
        }
    }

    /// A wiring-form statement's node, by its name in the text.
    fn node_id(&self, name: &Name) -> R<String> {
        match self.cur().env.get(&name.text) {
            Some(Val::Out { id, .. }) => Ok(id.clone()),
            _ => err(name.span, format!("no node \"{}\" (nodes are declared before they're wired)", name.text)),
        }
    }

    /// The collection or folder `name` is, or why it is neither.
    fn check_collection(&self, name: &Name) -> R<&'static Store> {
        match self.store(&name.text) {
            Some(s) => Ok(s),
            None if self.channels.contains(&name.text) => {
                err(name.span, format!("\"{0}\" is a channel: send on it with {0}.send(…); a route answers with respond {0}.subscribe()", name.text))
            }
            None => err(name.span, format!("\"{}\" is not a declared collection (add `collection {}` to the header)", name.text, name.text)),
        }
    }

    /// Whether `n` is a declared collection or folder, and which.
    fn store(&self, n: &str) -> Option<&'static Store> {
        if self.collections.contains(n) {
            Some(&COLLECTION)
        } else if self.folders.contains(n) {
            Some(&FOLDER)
        } else {
            None
        }
    }

    /// An effect statement's value: inside a holon it leaves through the boundary as an output,
    /// elsewhere an unconsumed Effect output is exposed and performed by the IR's own rule.
    fn effect(&mut self, v: Val, span: Span) -> R<()> {
        if let FrameKind::Holon { effects, .. } = &mut self.frame().kind {
            effects.push((v, span));
        }
        Ok(())
    }

    fn respond(&mut self, args: &'a [Arg], span: Span) -> R<()> {
        if let FrameKind::Action { node, responded, .. } = &self.cur().kind {
            if let Some(first) = responded {
                return err(span, format!("more than one respond in the action (the first is on line {})", first.start.line));
            }
            let node = node.clone();
            return self.answer(&node, args, span);
        }
        let request = match &self.cur().kind {
            FrameKind::Route { request, responded } => {
                if let Some(first) = responded {
                    return err(span, format!("more than one respond in the route (the first is on line {})", first.start.line));
                }
                request.clone()
            }
            _ => return err(span, "respond belongs in a route"),
        };
        let (mut status, mut missing, mut body, mut headers): (Option<String>, Option<String>, Option<&Expr>, Option<&Expr>) = (None, None, None, None);
        // A status that isn't a number written out: computed, it feeds the respond's status input.
        let mut status_from: Option<&Expr> = None;
        let mut stream = false;
        let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        for a in args.iter().filter(|a| a.name.is_some()) {
            let n = a.name.as_ref().unwrap();
            match n.text.as_str() {
                "status" => match &a.value.kind {
                    ExprKind::Number { .. } => status = Some(self.status_text(&a.value)?),
                    _ => status_from = Some(&a.value),
                },
                "missing" => missing = Some(self.status_text(&a.value)?),
                "body" => body = Some(&a.value),
                "headers" => headers = Some(&a.value),
                "stream" => match &a.value.kind {
                    ExprKind::Bool(b) => stream = *b,
                    _ => return err(a.value.span, "stream: is true or false"),
                },
                other => return err(n.span, format!("respond has no \"{other}\": a body, then status:, missing:, headers: and stream:")),
            }
        }
        match positional.as_slice() {
            [] => {}
            [one] => {
                let bare_status = args.len() == 1 && matches!(&one.value.kind, ExprKind::Number { value, .. } if (100.0..600.0).contains(value) && value.fract() == 0.0);
                if bare_status {
                    status = Some(self.status_text(&one.value)?);
                } else if body.is_some() {
                    return err(one.value.span, "the body is given twice");
                } else {
                    body = Some(&one.value);
                }
            }
            [_, second, ..] => return err(second.value.span, "respond takes one body; status: and missing: are named"),
        }
        // A streamed reply is the upstream's answer as it arrives: its status and content type are
        // the call's, sent before the run is over, so nothing the run computes can set them. A call
        // that fails (not 2xx) forwards nothing, and the reply is its answer, whole, with its status:
        // the respond's status is wired from the call's. `missing:` is still the answer of a run
        // whose call was never made (an input absent).
        let mut streamed_status = None;
        if stream {
            if let Some(a) = args.iter().find(|a| a.name.as_ref().is_some_and(|n| ["status", "headers"].contains(&n.text.as_str()))) {
                return err(a.value.span, "a streamed reply answers with its call's status and content type: it takes no status: or headers:");
            }
            let Some(id) = body.and_then(|b| self.streamed_call(b)) else {
                return err(body.map_or(span, |b| b.span), "only a streamed call's body can be streamed: respond sent.body, stream: true, with sent = http.post(…, stream: true)");
            };
            streamed_status = Some(id.clone());
            self.streamed = Some(id);
        }
        // A subscription is the whole answer: the stream of the channel's packs, always a 200.
        let subscribing = body.and_then(|b| match &b.kind {
            ExprKind::Method { receiver, method, args } if method.text == "subscribe" && self.channels.contains(&receiver.text) => Some((receiver, args, b.span)),
            _ => None,
        });
        if subscribing.is_some()
            && let Some(a) = args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.text == "status" || n.text == "missing"))
        {
            return err(a.value.span, "a subscription answers 200 with the stream of packs: it takes no status: or missing:");
        }
        let status = status.unwrap_or_else(|| "200".into());
        let missing = missing.unwrap_or_else(|| "404".into());
        if status == "204"
            && let Some(b) = body
        {
            return err(b.span, "a 204 answers with no body");
        }
        // The body's and headers' nodes first, then the respond they feed.
        let body_val = match (subscribing, body) {
            (Some((channel, sub_args, at)), _) => Some((self.subscribe(channel, sub_args, at)?, at)),
            (None, Some(b)) => Some((self.expr(b)?, b.span)),
            (None, None) => None,
        };
        let headers_val = match headers {
            Some(h) => Some((self.expr(h)?, h.span)),
            None => None,
        };
        let status_val = match (status_from, streamed_status) {
            (Some(s), _) => Some((self.expr(s)?, s.span)),
            (None, Some(call)) => {
                let record = self.out(&call);
                Some((self.node_with("field", &[("name".into(), "status".into())], &[("record", record)], span)?, span))
            }
            (None, None) => None,
        };
        let mut params = vec![("status".to_string(), status), ("missing".to_string(), missing)];
        if headers_val.is_some() {
            params.push(("headers".to_string(), "true".to_string()));
        }
        if status_val.is_some() {
            params.push(("status_input".to_string(), "true".to_string()));
        }
        let node = self.node("respond", &params, span)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.b.wire(&request, "request", &id, "request").or_else(|e| err(span, e))?;
        if let Some((v, at)) = body_val {
            self.connect(&v, &id, "body", at)?;
        }
        if let Some((v, at)) = headers_val {
            self.connect(&v, &id, "headers", at)?;
        }
        if let Some((v, at)) = status_val {
            self.connect(&v, &id, "status", at)?;
        }
        if let FrameKind::Route { responded, .. } = &mut self.frame().kind {
            *responded = Some(span);
        }
        Ok(())
    }

    /// `respond v` in an action: its answer, the value alone. An action is run through Holon, not
    /// HTTP, so it has no status, headers or stream: the respond's envelope is a 200 whose body is
    /// the answer.
    fn answer(&mut self, action: &str, args: &'a [Arg], span: Span) -> R<()> {
        if let Some(n) = args.iter().find_map(|a| a.name.as_ref()) {
            return err(n.span, "an action answers with a value alone: status:, missing:, headers: and stream: are a route's");
        }
        let value = match args {
            [one] => &one.value,
            [] => return err(span, "respond gives the action's answer: respond with a value"),
            [_, second, ..] => return err(second.value.span, "respond takes one value"),
        };
        let v = self.expr(value)?;
        let node = self.node("respond", &[("status".to_string(), "200".to_string()), ("missing".to_string(), "404".to_string())], span)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.b.wire(action, "input", &id, "request").or_else(|e| err(span, e))?;
        self.connect(&v, &id, "body", value.span)?;
        if let FrameKind::Action { responded, .. } = &mut self.frame().kind {
            *responded = Some(span);
        }
        Ok(())
    }

    /// The `http_stream` node `e` is the body of (`sent.body`, `sent` bound to one), if it is.
    fn streamed_call(&self, e: &Expr) -> Option<String> {
        let ExprKind::Field { base, name } = &e.kind else { return None };
        let ExprKind::Name(n) = &base.kind else { return None };
        let Some(Val::Out { id, .. }) = self.cur().env.get(n) else { return None };
        (name.text == "body" && self.b.data(id).is_some_and(|d| d.name == "http_stream" && !piped(d))).then(|| id.clone())
    }

    /// `channel.subscribe(from: "kept")`, a route's answer: the packs sent on the channel from
    /// now on, or (`from: "kept"`) the ones it keeps first.
    fn subscribe(&mut self, channel: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        for a in args {
            match a.name.as_ref().map(|n| n.text.as_str()) {
                Some("from") => {
                    let from = self.literal_text(&a.value)?;
                    if from != "new" && from != "kept" {
                        return err(a.value.span, "from: is \"new\" (packs sent from now on) or \"kept\" (the ones the channel keeps, first)");
                    }
                }
                Some(other) => return err(a.name.as_ref().unwrap().span, format!("subscribe has no \"{other}\": only from: \"new\" or \"kept\"")),
                None => return err(a.value.span, "subscribe takes no value: only from: \"new\" or \"kept\""),
            }
        }
        let u = interface_of("channel_subscribe").expect("a genesis leaf");
        let params: Vec<WParam> = u.params.iter().map(WParam::from_core).collect();
        self.unit_call("channel_subscribe", &[], &params, &[("channel".to_string(), channel.text.clone())], args, span)
    }

    fn status_text(&self, e: &Expr) -> R<String> {
        match &e.kind {
            ExprKind::Number { value, text } if (200.0..600.0).contains(value) && value.fract() == 0.0 => Ok(text.clone()),
            _ => err(e.span, "a status is a number from 200 to 599"),
        }
    }

    // ── routes and holons ────────────────────────────────────────────────────────────────────

    fn route(&mut self, r: &'a RouteDef) -> R<()> {
        let mut params = vec![("method".to_string(), r.method.text.clone()), ("path".to_string(), r.path.text.clone())];
        if let Some(res) = &r.resumes {
            params.push(("resume".into(), res.text.clone()));
        }
        let request = self.node("request", &params, r.method.span.to(r.path.span))?;
        let Val::Out { id, .. } = &request else { unreachable!() };
        let id = id.clone();
        self.names.insert(id.clone(), r.request.text.clone());
        let mut frame = Frame::new(FrameKind::Route { request: id.clone(), responded: None });
        frame.created.push(id);
        self.check_name_free(&r.request)?;
        frame.env.insert(r.request.text.clone(), request);
        self.frames.push(frame);
        self.streamed = None;
        for s in &r.body {
            self.stmt(s)?;
        }
        // Every sink of the route is tied to its request (docs/L4s/triggers-L4.md §3).
        let request = self.cur().env.get(&r.request.text).cloned().filter(|v| matches!(v, Val::Out { .. }));
        let request = match request {
            Some(Val::Out { id, outputs, .. }) => Val::Out { port: outputs.iter().position(|p| p.name == "request").unwrap_or(0), id, outputs, explicit: true },
            _ => self.out(&match &self.cur().kind {
                FrameKind::Route { request, .. } => request.clone(),
                _ => unreachable!("a route's frame"),
            }),
        };
        self.tie_block(&request, r.method.span.to(r.path.span))?;
        let frame = self.frames.pop().expect("the route's frame");
        // A streamed call is forwarded to the caller: the route says so where it answers.
        let streams: Vec<&String> = frame.created.iter().filter(|id| self.b.data(id).is_some_and(|d| d.name == "http_stream" && !piped(d))).collect();
        if streams.len() > 1 {
            return err(r.method.span.to(r.path.span), "a route forwards one streamed call: this one makes several");
        }
        if let Some(id) = streams.first()
            && self.streamed.as_ref() != Some(*id)
        {
            let at = self.spans.get(*id).copied().unwrap_or(r.method.span);
            return err(at, "a streamed call is forwarded to the caller: answer with it, respond sent.body, stream: true");
        }
        // No respond and no effect: route_spec would refuse it too; say so here, at the route.
        let FrameKind::Route { responded, request } = frame.kind else { unreachable!() };
        self.routes.push((request, frame.created, r.method.span.to(r.path.span)));
        let has_effect = r.body.iter().any(|s| !matches!(s.kind, StmtKind::Bind { .. } | StmtKind::Respond(_) | StmtKind::Node { .. } | StmtKind::Wire { .. }));
        if responded.is_none() && !has_effect && !frame.wiring_form {
            return err(r.method.span.to(r.path.span), "the route answers nothing: add a respond (or an effect, answered 204)");
        }
        Ok(())
    }

    /// `every 2s { … }`: a clock handler. Its tick node is its trigger, `now` in it the tick's time,
    /// and its sinks are tied to it (docs/L4s/triggers-L4.md §2, §3).
    fn every(&mut self, e: &'a EveryDef) -> R<()> {
        let seconds = if e.seconds.fract() == 0.0 && e.seconds >= 1.0 { number_text(e.seconds) } else { String::new() };
        crate::templates::period(&seconds).or_else(|_| err(e.head, format!("every {}: a clock runs every whole number of seconds, from 1s to 366d", e.text)))?;
        let tick = self.node("every", &[("seconds".to_string(), seconds)], e.head)?;
        let Val::Out { id, .. } = &tick else { unreachable!() };
        let id = id.clone();
        let mut frame = Frame::new(FrameKind::Every { tick: id.clone() });
        frame.created.push(id.clone());
        self.frames.push(frame);
        for s in &e.body {
            if let StmtKind::Respond(_) = s.kind {
                return err(s.span, "respond belongs in a route: a clock answers nobody");
            }
            self.stmt(s)?;
        }
        let tied = self.tie_block(&tick, e.head)?;
        let frame = self.frames.pop().expect("the block's frame");
        if !tied {
            return err(e.head, "the block does nothing: write something in it (state, a collection, a channel)");
        }
        self.ticks.push((id, frame.created, e.head, ("the tick", "now".into())));
        Ok(())
    }

    /// `on file inbox (f) { … }`: a file handler. Its `on_file` node is its trigger, `f` in it the
    /// file's handle, and its sinks are tied to it (docs/L4s/triggers-L4.md §2, §3).
    fn on_file(&mut self, o: &'a OnFileDef) -> R<()> {
        let mut params = vec![("folder".to_string(), o.on.folder.clone())];
        if o.on.deleted {
            params.push(("on".into(), "deleted".into()));
        }
        if let Some(m) = &o.on.matching {
            params.push(("matching".into(), m.clone()));
        }
        let file = self.node("on_file", &params, o.head)?;
        let Val::Out { id, .. } = &file else { unreachable!() };
        let id = id.clone();
        // A handle: written to a folder, it is copied; sent, it goes by handle.
        self.arrivals.insert(id.clone());
        self.names.insert(id.clone(), o.file.text.clone());
        self.check_name_free(&o.file)?;
        let mut frame = Frame::new(FrameKind::OnFile { file: o.file.text.clone() });
        frame.created.push(id.clone());
        frame.env.insert(o.file.text.clone(), file.clone());
        self.frames.push(frame);
        for s in &o.body {
            if let StmtKind::Respond(_) = s.kind {
                return err(s.span, "respond belongs in a route: a file's run answers nobody");
            }
            self.stmt(s)?;
        }
        let tied = self.tie_block(&file, o.head)?;
        let frame = self.frames.pop().expect("the block's frame");
        if !tied {
            return err(o.head, "the block does nothing: write something in it (a folder, state, a channel)");
        }
        self.ticks.push((id, frame.created, o.head, ("the file", o.file.text.clone())));
        Ok(())
    }

    /// `action reset (input) { … }`: an action, run by hand by its name. Its `action` node is its
    /// trigger, `input` in it what was sent, and its sinks are tied to it (docs/L4s/triggers-L4.md
    /// §2, §3); `respond v` answers.
    fn action(&mut self, a: &'a ActionDef) -> R<()> {
        crate::templates::action_name(&[WParam::frozen("name", &a.name.text)]).or_else(|e| err(a.name.span, e))?;
        let node = self.node("action", &[("name".to_string(), a.name.text.clone())], a.head)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.names.insert(id.clone(), a.input.text.clone());
        self.check_name_free(&a.input)?;
        let mut frame = Frame::new(FrameKind::Action { node: id.clone(), input: a.input.text.clone(), responded: None });
        frame.created.push(id.clone());
        frame.env.insert(a.input.text.clone(), node.clone());
        self.frames.push(frame);
        for s in &a.body {
            self.stmt(s)?;
        }
        let tied = self.tie_block(&node, a.head)?;
        let frame = self.frames.pop().expect("the block's frame");
        let FrameKind::Action { responded, .. } = frame.kind else { unreachable!() };
        if responded.is_none() && !tied {
            return err(a.head, "the action does nothing: respond with something, or write something (state, a collection, a channel)");
        }
        self.ticks.push((id, frame.created, a.head, ("what was sent", a.input.text.clone())));
        Ok(())
    }

    fn check_name_free(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if interface_of(n).is_some() || templates::is_template(n) || RESERVED.contains(&n) || self.store(n).is_some() || self.databases.contains(n) || self.channels.contains(n) || self.is_def(n) {
            return err(name.span, format!("\"{n}\" is taken; pick another name"));
        }
        Ok(())
    }

    /// A holon's or function's port may be called after a leaf (`sum`, `out`: the wiring form
    /// names ports that way, and a name in scope wins over a leaf), but not after a keyword, a
    /// collection or a holon.
    fn check_port_name(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if (RESERVED.contains(&n) && n != "in" && n != "out") || self.store(n).is_some() || self.databases.contains(n) || self.channels.contains(n) || self.is_def(n) {
            return err(name.span, format!("\"{n}\" is taken; pick another name for the port"));
        }
        Ok(())
    }

    /// The holon `name`, lowered once, on first use.
    fn holon(&mut self, name: &str, at: Span) -> R<Holon> {
        if let Some(h) = self.holons.get(name) {
            return Ok(h.clone());
        }
        let Some(def) = self.defs.get(name).copied() else {
            if let Some(js) = self.js_defs.get(name).copied() {
                return self.js_node(js);
            }
            return err(at, format!("no holon \"{name}\""));
        };
        if !self.in_progress.insert(name.to_string()) {
            return err(at, format!("the holon \"{name}\" places itself: a holon can't contain itself"));
        }
        self.b.begin_holon(name);
        let declared: Vec<String> = def.outputs.iter().map(|p| p.name.text.clone()).collect();
        let mut frame = Frame::new(FrameKind::Holon { declared: declared.clone(), bound: HashMap::new(), effects: vec![] });
        for p in &def.inputs {
            self.check_port_name(&p.name)?;
            if frame.env.insert(p.name.text.clone(), Val::In(p.name.text.clone())).is_some() {
                return err(p.name.span, format!("the input \"{}\" is declared twice", p.name.text));
            }
        }
        for p in &def.outputs {
            self.check_port_name(&p.name)?;
            if def.inputs.iter().any(|i| i.name.text == p.name.text) || declared.iter().filter(|d| **d == p.name.text).count() > 1 {
                return err(p.name.span, format!("the port \"{}\" is declared twice", p.name.text));
            }
        }
        self.frames.push(frame);
        let mut result = self.holon_body(def);
        // Its sinks are tied to its trigger: the input named so when it declares one, else one it
        // gets last, which a placement ties to the block it is placed in. (The wiring form is the
        // graph as written: a tie there is a wire, `in.trigger -> w.trigger`, so nothing is added.)
        let mut implicit_trigger = false;
        if result.is_ok() && !self.cur().wiring_form {
            match self.tie_block(&Val::In(TRIGGER.into()), def.name.span) {
                Ok(tied) => implicit_trigger = tied && !def.inputs.iter().any(|p| p.name.text == TRIGGER),
                Err(e) => result = Err(e),
            }
        }
        let frame = self.frames.pop().expect("the holon's frame");
        self.in_progress.remove(name);
        result?;
        let FrameKind::Holon { bound, effects, .. } = frame.kind else { unreachable!() };
        let mut wiring_in = frame.wiring_in;
        let mut wiring_out = frame.wiring_out;
        let holon = if frame.wiring_form && wiring_in.is_empty() && wiring_out.is_empty() && bound.is_empty() && effects.is_empty() {
            // The wiring form without its own boundary wires: the proposed boundary, renamed.
            let names = |ps: &[PortDecl]| ps.iter().map(|p| p.name.text.clone()).collect::<Vec<_>>();
            self.b.end_holon(name, &names(&def.inputs), &names(&def.outputs)).or_else(|e| err(def.name.span, e))?
        } else {
            // Every input used, every declared output bound.
            for p in &def.inputs {
                if !wiring_in.iter().any(|(port, _, _)| *port == p.name.text) {
                    return err(p.name.span, format!("the input \"{}\" is never used", p.name.text));
                }
            }
            let mut outputs: Vec<HolonPort> = Vec::new();
            for p in &def.outputs {
                let port = HolonPort { name: p.name.text.clone(), unit: p.unit.clone() };
                if let Some((v, span)) = bound.get(&p.name.text) {
                    let Val::Out { id, outputs: outs, port: k, .. } = v else { unreachable!("checked when bound") };
                    let output = outs.get(*k).map(|o| o.name.clone()).unwrap_or_else(|| "out".into());
                    let _ = span;
                    wiring_out.push((id.clone(), output, p.name.text.clone()));
                } else if !wiring_out.iter().any(|(_, _, port)| *port == p.name.text) {
                    return err(p.name.span, format!("the output \"{}\" is never bound", p.name.text));
                }
                outputs.push(port);
            }
            // Effects written as statements leave as outputs too, named after their leaves.
            let mut counts: HashMap<String, usize> = HashMap::new();
            for (v, span) in &effects {
                let Val::Out { id, outputs: outs, port, .. } = v else { continue };
                let leaf = self.b.data(id).map(|d| d.name.clone()).unwrap_or_default();
                let base = match leaf.as_str() {
                    "collection_put" => "put",
                    "collection_delete" => "delete",
                    "file_write" => "written",
                    "file_copy" => "copied",
                    "file_create" | "file_create_copy" => "created",
                    "file_delete" => "deleted",
                    "store_write" => "write",
                    "egress" | "channel_send" => "sent",
                    _ => "effect",
                };
                let n = counts.entry(base.to_string()).or_insert(0);
                *n += 1;
                let mut pname = if *n == 1 { base.to_string() } else { format!("{base}_{n}") };
                while outputs.iter().any(|o| o.name == pname) {
                    pname.push('_');
                }
                let output = outs.get(*port).map(|o| o.name.clone()).unwrap_or_else(|| "out".into());
                let _ = span;
                wiring_out.push((id.clone(), output, pname.clone()));
                outputs.push(HolonPort { name: pname, unit: None });
            }
            let mut inputs: Vec<HolonPort> = def.inputs.iter().map(|p| HolonPort { name: p.name.text.clone(), unit: p.unit.clone() }).collect();
            if implicit_trigger {
                inputs.push(HolonPort { name: TRIGGER.into(), unit: None });
            }
            // Inputs in declared order, as the boundary lists them.
            wiring_in.sort_by_key(|(port, _, _)| inputs.iter().position(|p| p.name == *port));
            self.b.end_holon_exact(name, &inputs, &outputs, &wiring_in, &wiring_out).or_else(|e| err(def.name.span, e))?
        };
        self.holons.insert(name.to_string(), holon.clone());
        Ok(holon)
    }

    /// A `js` node (docs/L4s/js-node-L4.md §3): a composite the printer can read back, over the
    /// engine module placed by hash. The source the engine compiles is `(inputs) => { body }`,
    /// assembled here from the declared names, so ports and source can't disagree; the arguments
    /// travel as one list; the engine's `{ value }` is read by `field`, once more per output when
    /// there are several. A budget instantiates the engine under it (its params), so the same
    /// body under two budgets is two units.
    fn js_node(&mut self, def: &'a JsDef) -> R<Holon> {
        let name = def.name.text.as_str();
        for p in def.inputs.iter().chain(&def.outputs) {
            self.check_port_name(&p.name)?;
        }
        let mut seen = HashSet::new();
        for p in def.inputs.iter().chain(&def.outputs) {
            if !seen.insert(p.name.text.as_str()) {
                return err(p.name.span, format!("the port \"{}\" is declared twice", p.name.text));
            }
        }
        if def.body.trim().is_empty() {
            return err(def.body_span, "the js body is empty: return something");
        }
        self.b.begin_holon(name);
        let src = self.b.node("const", &[("value".into(), js_source(def)), ("type".into(), "Text".into())], None).or_else(|e| err(def.span, e))?;
        // The arguments as one list: `list` for two or more, `cons` onto an empty list for one,
        // the empty list itself for none (`list` fails closed on a missing input).
        let empty = || [("value".to_string(), "[]".to_string()), ("type".to_string(), "Json".to_string())];
        let (args, arg_ports): (String, Vec<String>) = match def.inputs.len() {
            0 => (self.b.node("const", &empty(), None).or_else(|e| err(def.span, e))?, vec![]),
            1 => {
                let none = self.b.node("const", &empty(), None).or_else(|e| err(def.span, e))?;
                let cons = self.b.node("cons", &[], None).or_else(|e| err(def.span, e))?;
                self.b.wire(&none, "out", &cons, "list").or_else(|e| err(def.span, e))?;
                (cons, vec!["item".into()])
            }
            n => {
                let list = self.b.node("list", &[], None).or_else(|e| err(def.span, e))?;
                self.b.arity(&list, n).or_else(|e| err(def.span, e))?;
                let ports = self.b.data(&list).map(|d| d.inputs.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
                (list, ports)
            }
        };
        let (fuel, memory) = (def.fuel.as_ref().map(|b| b.value), def.memory.as_ref().map(|b| b.value));
        let engine = self.b.place_js_engine(fuel, memory).or_else(|e| err(def.span, e))?;
        self.b.wire(&src, "out", &engine, "source").or_else(|e| err(def.span, e))?;
        self.b.wire(&args, "out", &engine, "args").or_else(|e| err(def.span, e))?;
        let value = self.b.node("field", &[("name".into(), "value".into())], None).or_else(|e| err(def.span, e))?;
        self.b.wire(&engine, "result", &value, "record").or_else(|e| err(def.span, e))?;
        let mut wiring_out: Vec<(String, String, String)> = Vec::new();
        if def.outputs.len() == 1 {
            wiring_out.push((value.clone(), "out".into(), def.outputs[0].name.text.clone()));
        } else {
            for p in &def.outputs {
                let f = self.b.node("field", &[("name".into(), p.name.text.clone())], None).or_else(|e| err(def.span, e))?;
                self.b.wire(&value, "out", &f, "record").or_else(|e| err(def.span, e))?;
                wiring_out.push((f, "out".into(), p.name.text.clone()));
            }
        }
        let wiring_in: Vec<(String, String, String)> = def.inputs.iter().zip(&arg_ports).map(|(p, port)| (p.name.text.clone(), args.clone(), port.clone())).collect();
        let inputs: Vec<HolonPort> = def.inputs.iter().map(|p| HolonPort { name: p.name.text.clone(), unit: None }).collect();
        let outputs: Vec<HolonPort> = def.outputs.iter().map(|p| HolonPort { name: p.name.text.clone(), unit: None }).collect();
        for id in [&src, &args, &engine, &value] {
            self.spans.insert(id.clone(), def.span);
        }
        let holon = self.b.end_holon_exact(name, &inputs, &outputs, &wiring_in, &wiring_out).or_else(|e| err(def.name.span, e))?;
        self.holons.insert(name.to_string(), holon.clone());
        Ok(holon)
    }

    fn holon_body(&mut self, def: &'a HolonDef) -> R<()> {
        for s in &def.body {
            self.stmt(s)?;
        }
        Ok(())
    }

    /// Place the holon `name` as a node, its inputs wired from `args`.
    fn place(&mut self, name: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        let h = self.holon(&name.text, name.span)?;
        let mut given: Vec<Option<&'a Expr>> = vec![None; h.inputs.len()];
        // Positional arguments fill the inputs in order, the trigger aside: the block ties it.
        let fillable: Vec<usize> = (0..h.inputs.len()).filter(|&k| h.inputs[k].name != TRIGGER).collect();
        let mut positional = 0;
        for a in args {
            match &a.name {
                None => {
                    let Some(&k) = fillable.get(positional) else {
                        return err(a.value.span, format!("\"{}\" takes {} inputs", name.text, fillable.len()));
                    };
                    given[k] = Some(&a.value);
                    positional += 1;
                }
                Some(n) => match h.inputs.iter().position(|p| p.name == n.text) {
                    Some(k) if given[k].is_none() => given[k] = Some(&a.value),
                    Some(_) => return err(n.span, format!("\"{}\" is given twice", n.text)),
                    None => return err(n.span, format!("\"{}\" has no input \"{}\" (a holon takes no params)", name.text, n.text)),
                },
            }
        }
        let vals: Vec<(String, Val)> = h
            .inputs
            .iter()
            .zip(&given)
            .filter(|(p, g)| g.is_some() || p.name != TRIGGER)
            .map(|(p, g)| match g {
                Some(e) => Ok((p.name.clone(), self.expr(e)?)),
                None => err(span, format!("\"{}\": the input \"{}\" can't be left out", name.text, p.name)),
            })
            .collect::<R<_>>()?;
        let id = self.b.place(&h, None);
        self.spans.insert(id.clone(), span);
        self.frame().created.push(id.clone());
        for (port, v) in &vals {
            self.connect(v, &id, port, span)?;
        }
        Ok(self.out(&id))
    }

    /// A function: an anonymous holon with the parameters as inputs, then each name it uses from
    /// around it, and the body as its one output. Its hash, as a `fn` param; how many inputs it
    /// takes; and the values of the names it captured, for the call to pass.
    fn lambda(&mut self, params: &'a [Name], body: &'a Expr, span: Span) -> R<(String, usize, Vec<Val>)> {
        self.lambdas += 1;
        let name = format!("fn_{}", self.lambdas);
        self.b.begin_holon(&name);
        let mut frame = Frame::new(FrameKind::Lambda { captures: vec![] });
        for p in params {
            self.check_port_name(p)?;
            if frame.env.insert(p.text.clone(), Val::In(p.text.clone())).is_some() {
                return err(p.span, format!("the parameter \"{}\" is declared twice", p.text));
            }
        }
        self.frames.push(frame);
        let result = self.expr(body);
        let frame = self.frames.pop().expect("the function's frame");
        let v = result?;
        let Val::Out { id, outputs, port, .. } = &v else {
            return err(body.span, "a function's body can't be a parameter passed straight through: put a node between");
        };
        let output = outputs.get(*port).map(|o| o.name.clone()).unwrap_or_else(|| "out".into());
        let FrameKind::Lambda { captures } = &frame.kind else { unreachable!() };
        // By name, not by first use, so the function's inputs are the same however its body is
        // written, and `fmt` can print the captures back as the names they are.
        let mut captures = captures.clone();
        captures.sort_by(|a, b| a.0.cmp(&b.0));
        let inputs: Vec<HolonPort> =
            params.iter().map(|p| p.text.clone()).chain(captures.iter().map(|(n, _)| n.clone())).map(|name| HolonPort { name, unit: None }).collect();
        for p in params {
            if !frame.wiring_in.iter().any(|(port, _, _)| *port == p.text) {
                return err(p.span, format!("the parameter \"{}\" is never used", p.text));
            }
        }
        let mut wiring_in = frame.wiring_in;
        wiring_in.sort_by_key(|(port, _, _)| inputs.iter().position(|p| p.name == *port));
        let h = self.b.end_holon_exact(&name, &inputs, &[HolonPort { name: "out".into(), unit: None }], &wiring_in, &[(id.clone(), output, "out".into())]).or_else(|e| err(span, e))?;
        Ok((h.hash, inputs.len(), captures.into_iter().map(|(_, v)| v).collect()))
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    fn expr(&mut self, e: &'a Expr) -> R<Val> {
        match &e.kind {
            ExprKind::Number { text, .. } => self.node("const", &[("value".into(), text.clone()), ("type".into(), "Number".into())], e.span),
            ExprKind::Duration { seconds, .. } => self.node("const", &[("value".into(), number_text(*seconds)), ("type".into(), "Number".into())], e.span),
            ExprKind::Text(s) => self.node("const", &[("value".into(), s.clone()), ("type".into(), "Text".into())], e.span),
            ExprKind::Bool(b) => self.node("const", &[("value".into(), b.to_string()), ("type".into(), "Boolean".into())], e.span),
            ExprKind::Interpolated(parts) => {
                let mut vals = Vec::new();
                for p in parts {
                    vals.push(match p {
                        Part::Text(s) => self.node("const", &[("value".into(), s.clone()), ("type".into(), "Text".into())], e.span)?,
                        Part::Expr(x) => {
                            let v = self.expr(x)?;
                            self.node_with("as_text", &[], &[("value", v)], x.span)?
                        }
                    });
                }
                if vals.len() == 1 {
                    return Ok(vals.pop().unwrap());
                }
                let ports = Self::variadic_ports(&[WPort::new("a", WType::Text), WPort::new("b", WType::Text)], vals.len());
                let inputs: Vec<(&str, Val)> = ports.iter().map(String::as_str).zip(vals).collect();
                self.node_with("concat", &[], &inputs, e.span)
            }
            ExprKind::Name(n) => self.name(n, e.span),
            ExprKind::Secret(n) => self.node("secret", &[("name".into(), n.clone())], e.span),
            ExprKind::State(k) => self.node("store_read", &[("key".into(), k.clone())], e.span),
            ExprKind::Noun(Noun::Ingress) => self.ingress(e.span),
            // In an `every` block, `now` is the time of its tick.
            ExprKind::Noun(Noun::Now) if let FrameKind::Every { tick } = &self.cur().kind => Ok(self.out(&tick.clone())),
            ExprKind::Noun(n) => self.node(if *n == Noun::Now { "clock" } else { "random" }, &[], e.span),
            ExprKind::Call { callee, args } => self.call(callee, args, e.span),
            ExprKind::Method { receiver, method, args } => self.method(receiver, method, args, e.span),
            ExprKind::Field { base, name } => {
                let v = self.expr(base)?;
                // One output of a multi-output node, selected by name; from then on the value
                // is that one output, so a further `.name` is a field.
                if let Val::Out { id, outputs, explicit: false, .. } = &v
                    && outputs.len() > 1
                    && let Some(k) = outputs.iter().position(|p| p.name == name.text)
                {
                    return Ok(Val::Out { id: id.clone(), outputs: outputs.clone(), port: k, explicit: true });
                }
                self.node_with("field", &[("name".into(), name.text.clone())], &[("record", v)], e.span)
            }
            ExprKind::Index { base, key } => {
                if let ExprKind::Name(c) = &base.kind
                    && let Some(store) = self.store(c)
                {
                    let k = self.expr(key)?;
                    return self.node_with(store.get, &[(store.param.into(), c.clone())], &[(store.key, k)], e.span);
                }
                let r = self.expr(base)?;
                let k = self.expr(key)?;
                self.node_with("lookup", &[], &[("record", r), ("key", k)], e.span)
            }
            ExprKind::Record { spread, fields } => {
                let mut acc = match spread {
                    Some(s) => self.expr(s)?,
                    None => self.node("record", &[], e.span)?,
                };
                for (k, v) in fields {
                    let val = self.expr(v)?;
                    acc = self.node_with("with_field", &[("name".into(), k.text.clone())], &[("record", acc), ("value", val)], k.span.to(v.span))?;
                }
                Ok(acc)
            }
            ExprKind::List(items) => {
                let items: Vec<&Expr> = items.iter().collect();
                self.list_of(&items, e.span)
            }
            ExprKind::Not(x) => {
                let v = self.expr(x)?;
                self.node_with("not", &[], &[("v", v)], e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span),
            ExprKind::If { cond, then, els } => {
                let c = self.expr(cond)?;
                let t = self.expr(then)?;
                let guarded = self.node_with("guard", &[], &[("cond", c), ("value", t)], e.span)?;
                match els {
                    None => Ok(guarded),
                    Some(b) => self.fallback(guarded, b, e.span),
                }
            }
            ExprKind::Match { subject, arms, els, .. } => self.match_expr(subject, arms, els.as_deref(), e.span),
            ExprKind::Await { name, expires } => {
                let mut params = vec![("name".to_string(), name.text.clone())];
                if let Some(x) = expires {
                    params.push(("expires".into(), self.literal_text(x)?));
                }
                self.node("await", &params, e.span)
            }
            ExprKind::Lambda { .. } => err(e.span, "a function goes in a fn: argument of map or fold"),
            ExprKind::Paren(inner) => self.expr(inner),
        }
    }

    /// A bare name: a binding, a top-level constant, or a mistake named precisely.
    fn name(&mut self, n: &str, span: Span) -> R<Val> {
        if let Some(v) = self.cur().env.get(n) {
            return Ok(v.clone());
        }
        if let Some(c) = self.consts.get(n).copied() {
            return self.expr(c);
        }
        // A name a function uses from around it: looked up there, and taken in as an input.
        if matches!(self.cur().kind, FrameKind::Lambda { .. }) {
            let inner = self.frames.pop().expect("the function's frame");
            let outside = self.name(n, span);
            self.frames.push(inner);
            let v = outside?;
            let FrameKind::Lambda { captures } = &mut self.frame().kind else { unreachable!() };
            captures.push((n.to_string(), v));
            self.frame().env.insert(n.to_string(), Val::In(n.to_string()));
            return Ok(Val::In(n.to_string()));
        }
        if self.is_def(n) {
            return err(span, format!("\"{n}\" is a holon: call it, {n}(…)"));
        }
        // `ingress.name`: a keyword before a dot reads as a name (a wiring form's node may be called
        // so); with no node of that name, it is what arrived.
        if n == "ingress" {
            return self.ingress(span);
        }
        if let Some(s) = self.store(n) {
            return err(span, format!("\"{n}\" is a {}: read it as {n}[{}] or {n}.list(…)", s.kind, s.key));
        }
        if self.databases.contains(n) {
            return err(span, format!("\"{n}\" is a database: read it as {n}.query(\"select …\") or {n}.one(…)"));
        }
        if self.channels.contains(n) {
            return err(span, format!("\"{n}\" is a channel: send on it with {n}.send(…); a route answers with respond {n}.subscribe()"));
        }
        if self.calls.contains_key(n) {
            return err(span, format!("\"{n}\" is a flow: run one of its actions, {n}.action(input)"));
        }
        if interface_of(alias(n)).is_some() || templates::is_template(n) {
            return err(span, format!("\"{n}\" is a leaf: call it, {n}(…)"));
        }
        err(span, format!("unknown name \"{n}\""))
    }

    /// What arrived at the deployment: a request's or a trigger's payload; in a flow that runs on
    /// files, the file's handle: its `on file` line's `file_arrival`, or with several, whichever
    /// started the run (a `coalesce` of them, in the order written: only that one has a value).
    fn ingress(&mut self, span: Span) -> R<Val> {
        // In an `on file` block the file has its own name, in an action what was sent.
        let block = self.frames.iter().rev().take_while(|f| !matches!(f.kind, FrameKind::Holon { .. })).find_map(|f| match &f.kind {
            FrameKind::OnFile { file } => Some(format!("in an `on file` block the file is {file}: use {file} in place of ingress")),
            FrameKind::Every { .. } => Some("a clock's run receives nothing: `now` in it is the time of the tick (ingress is a flow's run by hand)".to_string()),
            FrameKind::Action { input, .. } => Some(format!("in an action what was sent is {input}: use {input} in place of ingress")),
            _ => None,
        });
        if let Some(message) = block {
            return err(span, message);
        }
        let mut chain: Option<Val> = None;
        for f in self.on_files.clone() {
            let mut params = vec![("folder".to_string(), f.folder)];
            if f.deleted {
                params.push(("on".into(), "deleted".into()));
            }
            if let Some(m) = f.matching {
                params.push(("matching".into(), m));
            }
            let arrival = self.node("file_arrival", &params, span)?;
            chain = Some(match chain {
                None => arrival,
                Some(c) => self.node_with("coalesce", &[], &[("a", c), ("b", arrival)], span)?,
            });
        }
        let Some(chain) = chain else { return self.node("ingress", &[], span) };
        if let Val::Out { id, .. } = &chain {
            self.arrivals.insert(id.clone());
        }
        Ok(chain)
    }

    /// Whether `v` is a file's handle: one read here (`file_handle`), or the file that started the
    /// run (`ingress` with `on file`).
    fn is_handle(&self, v: &Val) -> bool {
        matches!(v, Val::Out { id, .. } if self.arrivals.contains(id) || self.b.data(id).is_some_and(|d| d.name == "file_handle" || d.name == "file_arrival"))
    }

    fn binary(&mut self, op: BinOp, lhs: &'a Expr, rhs: &'a Expr, span: Span) -> R<Val> {
        match op {
            BinOp::When => {
                let v = self.expr(lhs)?;
                let c = self.expr(rhs)?;
                self.node_with("guard", &[], &[("cond", c), ("value", v)], span)
            }
            BinOp::Coalesce => {
                let a = self.expr(lhs)?;
                self.fallback(a, rhs, span)
            }
            BinOp::Eq => {
                let a = self.expr(lhs)?;
                let b = self.expr(rhs)?;
                self.node_with("equals", &[], &[("a", a), ("b", b)], span)
            }
            BinOp::Ne => {
                let a = self.expr(lhs)?;
                let b = self.expr(rhs)?;
                let eq = self.node_with("equals", &[], &[("a", a), ("b", b)], span)?;
                self.node_with("not", &[], &[("v", eq)], span)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let a = self.expr(lhs)?;
                let b = self.expr(rhs)?;
                self.node_with("compare", &[("op".into(), op.text().into())], &[("a", a), ("b", b)], span)
            }
            BinOp::Sub | BinOp::Div => {
                let a = self.expr(lhs)?;
                let b = self.expr(rhs)?;
                self.node_with(if op == BinOp::Sub { "subtract" } else { "divide" }, &[], &[("a", a), ("b", b)], span)
            }
            BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or => {
                // A chain of the same operator is one variadic node; parentheses break the chain.
                let mut operands: Vec<&'a Expr> = Vec::new();
                fn gather<'b>(e: &'b Expr, op: BinOp, out: &mut Vec<&'b Expr>) {
                    match &e.kind {
                        ExprKind::Binary { op: o, lhs, rhs } if *o == op => {
                            gather(lhs, op, out);
                            gather(rhs, op, out);
                        }
                        _ => out.push(e),
                    }
                }
                gather(lhs, op, &mut operands);
                gather(rhs, op, &mut operands);
                let leaf = match op {
                    BinOp::Add => "add",
                    BinOp::Mul => "multiply",
                    BinOp::And => "and",
                    _ => "or",
                };
                let mut vals = Vec::new();
                for o in &operands {
                    vals.push(self.expr(o)?);
                }
                let first_ty = if leaf == "and" || leaf == "or" { WType::Boolean } else { WType::Number };
                let ports = Self::variadic_ports(&[WPort::new("a", first_ty.clone()), WPort::new("b", first_ty)], vals.len());
                let inputs: Vec<(&str, Val)> = ports.iter().map(String::as_str).zip(vals).collect();
                self.node_with(leaf, &[], &inputs, span)
            }
        }
    }

    /// `match` (docs/L4s/match-L4.md §4): each arm's value guarded by its patterns matching the
    /// subject, the arms chained by `coalesce`, and the `else` guarded by none of them matching,
    /// so a chosen arm that is absent makes the match absent instead of falling through.
    fn match_expr(&mut self, subject: &'a Expr, arms: &'a [Arm], els: Option<&'a Expr>, span: Span) -> R<Val> {
        let mut seen: Vec<Pattern> = Vec::new();
        for p in arms.iter().flat_map(|a| &a.patterns) {
            let Some(key) = pattern(p) else {
                return err(p.span, "a pattern is a literal (a number, a text, true or false); to test a condition, use `if`");
            };
            if seen.contains(&key) {
                return err(p.span, format!("{} is matched twice; the second arm could never be chosen", key.text()));
            }
            seen.push(key);
        }
        let s = self.expr(subject)?;
        let mut matched = Vec::new();
        let mut chain: Option<Val> = None;
        for arm in arms {
            let mut eqs = Vec::new();
            for p in &arm.patterns {
                let c = self.expr(p)?;
                eqs.push(self.node_with("equals", &[], &[("a", s.clone()), ("b", c)], p.span)?);
            }
            let m = self.any_of(eqs, span)?;
            let v = self.expr(&arm.value)?;
            let at = arm.patterns[0].span.to(arm.value.span);
            let guarded = self.node_with("guard", &[], &[("cond", m.clone()), ("value", v)], at)?;
            matched.push(m);
            chain = Some(match chain {
                None => guarded,
                Some(prev) => self.node_with("coalesce", &[], &[("a", prev), ("b", guarded)], span)?,
            });
        }
        let mut chain = chain.expect("the parser refuses a match without an arm");
        if let Some(e) = els {
            let any = self.any_of(matched, span)?;
            let none = self.node_with("not", &[], &[("v", any)], span)?;
            let v = self.expr(e)?;
            let guarded = self.node_with("guard", &[], &[("cond", none), ("value", v)], e.span)?;
            chain = self.node_with("coalesce", &[], &[("a", chain), ("b", guarded)], span)?;
        }
        Ok(chain)
    }

    /// One condition true when any of `conds` is: the condition itself, or their `or`.
    fn any_of(&mut self, mut conds: Vec<Val>, span: Span) -> R<Val> {
        if conds.len() == 1 {
            return Ok(conds.pop().expect("one"));
        }
        let ports = Self::variadic_ports(&[WPort::new("a", WType::Boolean), WPort::new("b", WType::Boolean)], conds.len());
        let inputs: Vec<(&str, Val)> = ports.iter().map(String::as_str).zip(conds).collect();
        self.node_with("or", &[], &inputs, span)
    }

    /// `a ?? b`: `default` with a frozen literal, `coalesce` with a value from a wire.
    fn fallback(&mut self, a: Val, b: &'a Expr, span: Span) -> R<Val> {
        if let Some((ty, value)) = literal_param(b) {
            return self.node_with("default", &[("value".into(), value), ("type".into(), ty.into())], &[("value", a)], span);
        }
        let v = self.expr(b)?;
        self.node_with("coalesce", &[], &[("a", a), ("b", v)], span)
    }

    /// A param's frozen text from an expression: a literal, a duration (in seconds), or a word.
    fn literal_text(&self, e: &Expr) -> R<String> {
        match &e.kind {
            ExprKind::Number { text, .. } => Ok(text.clone()),
            ExprKind::Duration { seconds, .. } => Ok(number_text(*seconds)),
            ExprKind::Text(s) => Ok(s.clone()),
            ExprKind::Bool(b) => Ok(b.to_string()),
            ExprKind::Name(n) => Ok(n.clone()),
            _ => err(e.span, "a param is a fixed value: a number, a duration, text in quotes, true or false, or a word"),
        }
    }

    fn call(&mut self, callee: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        let name = alias(&callee.text);
        if self.is_def(&callee.text) {
            return self.place(callee, args, span);
        }
        if self.cur().env.contains_key(&callee.text) {
            return err(callee.span, format!("\"{}\" is a value, not something to call", callee.text));
        }
        if templates::is_template(name) {
            return match name {
                "request" => err(callee.span, "a route declares its request: route GET /path (req) { … }"),
                "respond" => err(callee.span, "respond is a statement: respond body, missing: 404"),
                _ => {
                    let params = templates::defaults(name).expect("a template");
                    let iface = templates::interface(name, &params).expect("a template");
                    self.unit_call(name, &iface.inputs, &params, &[], args, span)
                }
            };
        }
        let Some(unit) = interface_of(name) else {
            return err(callee.span, format!("unknown name \"{}\"", callee.text));
        };
        let inputs: Vec<WPort> = unit.inputs.iter().map(WPort::from_core).collect();
        let params: Vec<WParam> = unit.params.iter().map(WParam::from_core).collect();
        self.unit_call(name, &inputs, &params, &[], args, span)
    }

    fn method(&mut self, receiver: &Name, method: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        let leaf = |n: &str| {
            let u = interface_of(n).expect("a genesis leaf");
            (u.inputs.iter().map(WPort::from_core).collect::<Vec<_>>(), u.params.iter().map(WParam::from_core).collect::<Vec<_>>())
        };
        match receiver.text.as_str() {
            "http" => {
                // Inside `updates.pipe(…)`: its answer goes to the channel as it arrives.
                let pipe = self.piping.take();
                let m = method.text.to_ascii_uppercase();
                if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].contains(&m.as_str()) {
                    return err(method.span, format!("http has no \"{}\": get, post, put, patch, delete or head", method.text));
                }
                let Some(first) = args.first().filter(|a| a.name.is_none()) else {
                    return err(span, "http.… takes the host first, in quotes: it is the permission the deployment asks for");
                };
                let ExprKind::Text(host) = &first.value.kind else {
                    return err(first.value.span, "the host is written as text in quotes: it is the permission the deployment asks for, so it can't come from a wire");
                };
                // A file as the body (a handle): the host streams its bytes.
                let named = |k: &str| args.iter().find_map(|a| a.name.as_ref().filter(|n| n.text == k));
                // `stream: true`, last: the answer is forwarded to the route's caller as it arrives.
                let mut args = &args[1..];
                let mut stream = false;
                if let (Some(at), Some(c)) = (named("stream").or(named("file")), &pipe) {
                    return err(at.span, format!("a piped call sends a body and goes to its channel as it arrives: {c}.pipe(http.{}(host, path, body: …)), without {}:", method.text, at.text));
                }
                if let Some(at) = named("stream") {
                    let last = args.last().expect("named is one of them");
                    if last.name.as_ref().map(|n| &n.text) != Some(&at.text) {
                        return err(at.span, "stream: comes last in the call");
                    }
                    let ExprKind::Bool(b) = last.value.kind else { return err(last.value.span, "stream: is true or false") };
                    if b && !matches!(self.cur().kind, FrameKind::Route { .. }) {
                        return err(at.span, "a streamed call is a route's: it forwards its answer to the route's caller, so call it in the route that responds with it");
                    }
                    stream = b;
                    args = &args[..args.len() - 1];
                }
                let sent = if let Some(file) = named("file") {
                    if named("body").is_some() {
                        return err(file.span, "a call sends a body or a file, not both");
                    }
                    if stream {
                        return err(file.span, "a streamed call sends a body, not a file");
                    }
                    "http_upload"
                } else if stream {
                    "http_stream"
                } else {
                    "http_request"
                };
                let sent = if pipe.is_some() { "http_stream" } else { sent };
                let (inputs, params) = leaf(sent);
                let mut preset = vec![("method".to_string(), m), ("host".to_string(), host.clone())];
                preset.extend(pipe.map(|c| ("pipe".to_string(), c)));
                self.unit_call(sent, &inputs, &params, &preset, args, span)
            }
            "stripe" => {
                if method.text != "verify" {
                    return err(method.span, format!("stripe has no \"{}\": stripe.verify(request, secret, now)", method.text));
                }
                let params = templates::defaults("stripe_verify").expect("a template");
                let iface = templates::interface("stripe_verify", &params).expect("a template");
                self.unit_call("stripe_verify", &iface.inputs, &params, &[], args, span)
            }
            "state" => {
                if method.text != "set" {
                    return err(method.span, format!("state has no \"{}\": state.key to read, state.key = … or state.set(key, …) to write", method.text));
                }
                let Some(first) = args.first().filter(|a| a.name.is_none()) else {
                    return err(span, "state.set takes the key first, then the value");
                };
                let key = self.literal_text(&first.value)?;
                let (inputs, params) = leaf("store_write");
                self.unit_call("store_write", &inputs, &params, &[("key".to_string(), key)], &args[1..], span)
            }
            c if self.databases.contains(c) => self.db_call(c, method, args, span),
            c if let Some(store) = self.store(c) => {
                let folder = store.kind == "folder";
                let leaf_name = match method.text.as_str() {
                    "get" => store.get,
                    "put" => store.put,
                    "delete" => store.delete,
                    "list" => store.list,
                    // A folder's files without their bytes, and parts of them.
                    "file" if folder => "file_handle",
                    "lines" if folder => "file_lines",
                    "slice" if folder => "file_slice",
                    "copy" if folder => "file_copy",
                    // Writes that never replace a file.
                    "create" if folder => "file_create",
                    "create_copy" if folder => "file_create_copy",
                    other if folder => {
                        return err(method.span, format!("a folder has no \"{other}\": get, put, delete, list, file, lines, slice, copy, create or create_copy"));
                    }
                    other => return err(method.span, format!("a {} has no \"{other}\": get, put, delete or list", store.kind)),
                };
                let (inputs, params) = leaf(leaf_name);
                self.unit_call(leaf_name, &inputs, &params, &[(store.param.to_string(), c.to_string())], args, span)
            }
            c if self.channels.contains(c) => match method.text.as_str() {
                "send" => {
                    let (inputs, params) = leaf("channel_send");
                    self.unit_call("channel_send", &inputs, &params, &[("channel".to_string(), c.to_string())], args, span)
                }
                "subscribe" => err(span, format!("a subscription is a route's answer: respond {c}.subscribe()")),
                // `updates.pipe(http.post(…))`: the call's answer, to the channel as it arrives.
                "pipe" => {
                    let call = match args {
                        [only] if only.name.is_none() => &only.value,
                        _ => return err(span, format!("{c}.pipe takes one call, whose answer it sends as it arrives: {c}.pipe(http.post(…))")),
                    };
                    if !matches!(&call.kind, ExprKind::Method { receiver, .. } if receiver.text == "http") {
                        return err(call.span, format!("only a call's answer can be piped: {c}.pipe(http.post(…))"));
                    }
                    self.piping = Some(c.to_string());
                    let piped = self.expr(call);
                    self.piping = None;
                    piped
                }
                other => err(method.span, format!("a channel has no \"{other}\": send(…), pipe(http.post(…)), or respond {c}.subscribe() in a route")),
            },
            // `billing.charge(x)`: run the action `charge` of the flow `billing`, sending x.
            c if self.calls.contains_key(c) => {
                if method.text.len() > 64 {
                    return err(method.span, "an action's name is at most 64 characters");
                }
                if args.len() > 1 {
                    return err(args[1].value.span, format!("an action takes one input: {c}.{}(x), with x a record to send more", method.text));
                }
                let flow = self.calls[c].clone();
                let (inputs, params) = leaf("flow_call");
                self.unit_call("flow_call", &inputs, &params, &[("flow".to_string(), flow), ("action".to_string(), method.text.clone())], args, span)
            }
            c if ["get", "put", "delete", "list"].contains(&method.text.as_str()) => {
                err(receiver.span, format!("\"{c}\" is not a declared collection (add `collection {c}` to the header)"))
            }
            other => err(receiver.span, format!("\"{other}\" has no methods; write {other}.{} as a field, or call a leaf", method.text)),
        }
    }

    /// A list of values: `[]` a const, one item appended to it, more a `list`.
    fn list_of(&mut self, items: &[&'a Expr], span: Span) -> R<Val> {
        match items.len() {
            0 => self.node("const", &[("value".into(), "[]".into()), ("type".into(), "Json".into())], span),
            1 => {
                let empty = self.node("const", &[("value".into(), "[]".into()), ("type".into(), "Json".into())], span)?;
                let item = self.expr(items[0])?;
                self.node_with("append", &[], &[("list", empty), ("item", item)], span)
            }
            n => {
                let mut vals = Vec::new();
                for i in items {
                    vals.push(self.expr(i)?);
                }
                let ports = Self::variadic_ports(&[WPort::new("a", WType::Any), WPort::new("b", WType::Any)], n);
                let inputs: Vec<(&str, Val)> = ports.iter().map(String::as_str).zip(vals).collect();
                self.node_with("list", &[], &inputs, span)
            }
        }
    }

    /// `shop.query(sql, a, b)`, `shop.one(…)`, `shop.exec(…, expect: 1)`: the statement in
    /// quotes first (its tables are the permission), then the values of its `$1`, `$2`, …, one
    /// each, wired as a list.
    fn db_call(&mut self, database: &str, method: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        let (leaf, write) = match method.text.as_str() {
            "query" => ("db_query", false),
            "one" => ("db_one", false),
            "exec" => ("db_exec", true),
            "find" | "find_one" | "insert" | "update" | "delete" => return self.document_call(database, method, args, span),
            other => return err(method.span, format!("a database has no \"{other}\": query, one or exec (SQL), or find, find_one, insert, update or delete (a document database)")),
        };
        let Some(first) = args.first().filter(|a| a.name.is_none()) else {
            return err(span, format!("{database}.{} takes the SQL first, in quotes: its tables are the permission the deployment asks for", method.text));
        };
        let ExprKind::Text(sql) = &first.value.kind else {
            return err(first.value.span, "the SQL is written in quotes, as it is: its tables are the permission, and values go after it as $1, $2, …, never into it");
        };
        let tables = holon_core::sql::tables(sql, write).or_else(|why| err(first.value.span, format!("this SQL can't be sent: {why}")))?;
        if tables.reads.is_empty() && tables.writes.is_empty() {
            return err(first.value.span, "this SQL reads no table: what it selects, the flow can compute itself");
        }
        let mut params = vec![("database".to_string(), database.to_string()), ("sql".to_string(), sql.clone())];
        let mut values: Vec<&'a Expr> = Vec::new();
        for a in &args[1..] {
            match &a.name {
                None => values.push(&a.value),
                Some(n) if n.text == "expect" && write => match &a.value.kind {
                    ExprKind::Number { value, text } if value.fract() == 0.0 && *value >= 0.0 && *value < 1e9 => params.push(("expect".to_string(), text.clone())),
                    _ => return err(a.value.span, "expect: is how many rows it must change, a whole number"),
                },
                Some(n) => {
                    let takes = if write { "values, then expect:" } else { "values" };
                    return err(n.span, format!("{database}.{} has no \"{}\": it takes the SQL, then its {takes}", method.text, n.text));
                }
            }
        }
        if values.len() != tables.params {
            let wants = match tables.params {
                0 => "no values".to_string(),
                1 => "one value, for $1".to_string(),
                n => format!("{n} values, for $1 to ${n}"),
            };
            return err(span, format!("the SQL takes {wants}; {} given", values.len()));
        }
        let list = self.list_of(&values, span)?;
        let v = self.node(leaf, &params, span)?;
        let Val::Out { id, .. } = &v else { unreachable!() };
        let id = id.clone();
        self.connect(&list, &id, "params", span)?;
        Ok(self.out(&id))
    }

    /// `notes.find("notes", { owner: who }, sort: { at: -1 }, limit: 20)`, `find_one`, `insert`
    /// (a document), `update` (a filter, then `{ "$set": … }`), `delete` (a filter): the collection
    /// in quotes first (the permission), then record literals written in the call. Their keys and
    /// operators, and every constant in them, are frozen into the leaf's templates (documents.rs);
    /// each other value is a hole, `$1`, `$2`, … in the order written, its value wired in a list.
    fn document_call(&mut self, database: &str, method: &Name, args: &'a [Arg], span: Span) -> R<Val> {
        use holon_core::documents::{check, holes_in_order, Part};
        let m = method.text.as_str();
        let (leaf, parts): (&str, &[(&str, Part)]) = match m {
            "find" => ("db_find", &[("filter", Part::Filter)]),
            "find_one" => ("db_find_one", &[("filter", Part::Filter)]),
            "insert" => ("db_insert", &[("document", Part::Document)]),
            "update" => ("db_update", &[("filter", Part::Filter), ("update", Part::Update)]),
            _ => ("db_delete", &[("filter", Part::Filter)]),
        };
        let positional: Vec<&'a Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        let Some(first) = positional.first() else {
            return err(span, format!("{database}.{m} takes the collection first, in quotes: it is the permission the deployment asks for"));
        };
        let ExprKind::Text(collection) = &first.value.kind else {
            return err(first.value.span, "the collection is written in quotes, as it is: it is the permission");
        };
        if !holon_core::documents::is_collection_name(collection) {
            return err(first.value.span, "a collection is letters, digits, _, - and ., and never system.… (the server's) or holon_… (the host's)");
        }
        let given = &positional[1..];
        // A find with no filter finds every document; every other call says what it selects.
        let needed = if m == "find" || m == "find_one" { 0 } else { parts.len() };
        if given.len() < needed || given.len() > parts.len() {
            let wants: Vec<&str> = parts.iter().map(|(n, _)| *n).collect();
            return err(span, format!("{database}.{m} takes the collection, then its {}", wants.join(", then its ")));
        }
        let mut params = vec![("database".to_string(), database.to_string()), ("collection".to_string(), collection.clone())];
        let mut values: Vec<&'a Expr> = Vec::new();
        let mut holes = std::collections::BTreeSet::new();
        for (i, (name, part)) in parts.iter().enumerate() {
            let text = match given.get(i) {
                Some(a) => {
                    let t = self.template(&a.value, &mut values, false)?;
                    let (_, h) = check(*part, &t.text()).or_else(|why| err(a.value.span, format!("this {name} can't be sent: {why}")))?;
                    holes.extend(h);
                    t.text()
                }
                None => "{}".to_string(),
            };
            params.push((name.to_string(), text));
        }
        let write = matches!(m, "update" | "delete");
        for a in args.iter().filter(|a| a.name.is_some()) {
            let n = a.name.as_ref().expect("named");
            match n.text.as_str() {
                "sort" if m == "find" || m == "find_one" => {
                    let t = self.template(&a.value, &mut values, true)?;
                    check(Part::Sort, &t.text()).or_else(|why| err(a.value.span, format!("this sort can't be sent: {why}")))?;
                    params.push(("sort".to_string(), t.text()));
                }
                "limit" if m == "find" => match &a.value.kind {
                    ExprKind::Number { value, text } if value.fract() == 0.0 && *value >= 1.0 && *value <= 100_000.0 => params.push(("limit".to_string(), text.clone())),
                    _ => return err(a.value.span, "limit: is how many documents at most, a whole number from 1 to 100000"),
                },
                "expect" if write => match &a.value.kind {
                    ExprKind::Number { value, text } if value.fract() == 0.0 && *value >= 0.0 && *value < 1e9 => params.push(("expect".to_string(), text.clone())),
                    _ => return err(a.value.span, format!("expect: is how many documents it must {}, a whole number", if m == "update" { "match" } else { "delete" })),
                },
                other => {
                    let takes = match m {
                        "find" => ", then sort: and limit:",
                        "find_one" => ", then sort:",
                        "update" | "delete" => ", then expect:",
                        _ => "",
                    };
                    return err(n.span, format!("{database}.{m} has no \"{other}\": it takes the collection, its records{takes}"));
                }
            }
        }
        holes_in_order(&holes).or_else(|why| err(span, why))?;
        let list = self.list_of(&values, span)?;
        let v = self.node(leaf, &params, span)?;
        let Val::Out { id, .. } = &v else { unreachable!() };
        let id = id.clone();
        self.connect(&list, &id, "params", span)?;
        Ok(self.out(&id))
    }

    /// A record literal written in a document call, as a template: its keys, and its constants
    /// (strings, numbers, true and false, and the lists and records of them), frozen; each other
    /// value a hole numbered after those in `values`, which it joins. `constant`: a sort, which
    /// has no holes.
    fn template(&self, e: &'a Expr, values: &mut Vec<&'a Expr>, constant: bool) -> R<holon_core::documents::Tpl> {
        use holon_core::documents::Tpl;
        let ExprKind::Record { spread, .. } = &e.kind else {
            return err(e.span, "a document call's filter, update, document and sort are records written in the call, { field: value, … }: their fields are the call's, never the data's");
        };
        if let Some(s) = spread {
            return err(s.span, "a document call's record is written whole: a spread would let the data choose its fields");
        }
        fn walk<'a>(e: &'a Expr, values: &mut Vec<&'a Expr>, constant: bool) -> R<Tpl> {
            Ok(match &e.kind {
                ExprKind::Text(t) => Tpl::Text(t.clone()),
                ExprKind::Number { text, .. } => Tpl::Number(text.clone()),
                ExprKind::Bool(b) => Tpl::Bool(*b),
                ExprKind::Paren(inner) => walk(inner, values, constant)?,
                ExprKind::List(items) => Tpl::List(items.iter().map(|i| walk(i, values, constant)).collect::<R<_>>()?),
                ExprKind::Record { spread: Some(s), .. } => return err(s.span, "a document call's record is written whole: a spread would let the data choose its fields"),
                ExprKind::Record { spread: None, fields } => {
                    let mut out: Vec<(String, Tpl)> = Vec::new();
                    for (k, v) in fields {
                        if out.iter().any(|(o, _)| *o == k.text) {
                            return err(k.span, format!("\"{}\" is given twice", k.text));
                        }
                        out.push((k.text.clone(), walk(v, values, constant)?));
                    }
                    Tpl::Object(out)
                }
                _ if constant => return err(e.span, "a sort is the flow's: each field is 1 or -1, as written"),
                _ => {
                    values.push(e);
                    if values.len() > 100 {
                        return err(e.span, "a document call takes at most 100 values");
                    }
                    Tpl::Hole(values.len())
                }
            })
        }
        walk(e, values, constant)
    }

    /// A leaf or template placed with its arguments: positional ones are inputs in port order
    /// (a variadic leaf grows), named ones are inputs when the unit has that port, else params.
    /// `preset` params come from the sugar (`http.post`'s method and host).
    fn unit_call(&mut self, leaf: &str, inputs: &[WPort], defaults: &[WParam], preset: &[(String, String)], args: &'a [Arg], span: Span) -> R<Val> {
        let variadic = VARIADIC.contains(&leaf);
        let mut params: Vec<(String, String)> = preset.to_vec();
        let mut given: Vec<Option<&'a Expr>> = vec![None; inputs.len()];
        let mut extra: Vec<&'a Expr> = Vec::new();
        // A `fn:` argument: how many inputs it takes, and the values it captured, which the call
        // passes after its own extra inputs.
        let mut function: Option<(usize, Vec<Val>, Span)> = None;
        let mut positional = 0;
        let zero_input = inputs.is_empty();
        // A sink's trigger, last, is never an argument in order: the block or `after` ties it.
        let fillable = if inputs.last().is_some_and(|p| p.name == TRIGGER) { inputs.len() - 1 } else { inputs.len() };
        for a in args {
            match &a.name {
                None if zero_input => {
                    // A leaf with no inputs takes its params in order: resume_token(paid).
                    let k = positional;
                    positional += 1;
                    let Some(p) = defaults.get(k) else { return err(a.value.span, format!("\"{leaf}\" takes no inputs, and has only {} params", defaults.len())) };
                    params.push((p.name().to_string(), self.literal_text(&a.value)?));
                }
                None => {
                    if positional < fillable {
                        given[positional] = Some(&a.value);
                    } else if variadic {
                        extra.push(&a.value);
                    } else {
                        return err(a.value.span, format!("\"{leaf}\" takes {fillable} inputs, {} given", positional + 1));
                    }
                    positional += 1;
                }
                Some(n) => {
                    let as_input = inputs.iter().position(|p| p.name == n.text).filter(|_| leaf != "default");
                    if let Some(k) = as_input {
                        if given[k].is_some() {
                            return err(n.span, format!("\"{}\" is given twice", n.text));
                        }
                        given[k] = Some(&a.value);
                    } else if defaults.iter().any(|p| p.name() == n.text) {
                        if params.iter().any(|(k, _)| *k == n.text) {
                            return err(n.span, format!("\"{}\" is given twice", n.text));
                        }
                        let value = if n.text == "fn" {
                            let (hash, takes, captured) = self.fn_param(&a.value)?;
                            function = Some((takes, captured, a.value.span));
                            hash
                        } else {
                            self.literal_text(&a.value)?
                        };
                        params.push((n.text.clone(), value));
                    } else {
                        return err(n.span, format!("\"{leaf}\" has no input or param \"{}\"", n.text));
                    }
                }
            }
        }
        for (k, p) in inputs.iter().enumerate() {
            if given[k].is_none() && !is_optional(leaf, &p.name) {
                return err(span, format!("\"{leaf}\": the input \"{}\" can't be left out", p.name));
            }
        }
        // Values first, so an argument's nodes are placed before the call's own.
        let mut vals: Vec<(String, Val)> = Vec::new();
        for (k, g) in given.iter().enumerate() {
            if let Some(e) = g {
                vals.push((inputs[k].name.clone(), self.expr(e)?));
            }
        }
        let captured = function.as_ref().map(|(_, c, _)| c.clone()).unwrap_or_default();
        if let Some((takes, _, fn_span)) = &function
            && PASSES_EXTRA.contains(&leaf)
        {
            let gives = inputs.len() + extra.len() + captured.len();
            if *takes != gives {
                let item = if leaf == "fold" { "the result so far and the item" } else { "the item" };
                let more = extra.len() + captured.len();
                let after = if more == 0 { String::new() } else { format!(", then {more} more") };
                return err(*fn_span, format!("\"{leaf}\" gives its function {gives} inputs ({item}{after}); this one takes {takes}"));
            }
        }
        let grown = inputs.len() + extra.len() + captured.len();
        let names = Self::variadic_ports(inputs, grown);
        for (i, e) in extra.iter().enumerate() {
            vals.push((names[inputs.len() + i].clone(), self.expr(e)?));
        }
        for (i, c) in captured.into_iter().enumerate() {
            vals.push((names[inputs.len() + extra.len() + i].clone(), c));
        }
        let v = self.node(leaf, &params, span)?;
        let Val::Out { id, .. } = &v else { unreachable!() };
        let id = id.clone();
        if grown > inputs.len() {
            self.b.arity(&id, grown).or_else(|e| err(span, e))?;
        }
        for (port, val) in &vals {
            self.connect(val, &id, port, span)?;
        }
        Ok(self.out(&id))
    }

    /// A `fn:` argument: a holon by name, or a function; its hash, how many inputs it takes, and
    /// the values a function captured from around it.
    fn fn_param(&mut self, e: &'a Expr) -> R<(String, usize, Vec<Val>)> {
        match &e.kind {
            ExprKind::Name(n) if self.is_def(n) => {
                let h = self.holon(n, e.span)?;
                Ok((h.hash, h.inputs.len(), vec![]))
            }
            ExprKind::Name(n) => err(e.span, format!("\"{n}\" is not a holon: fn: takes a holon's name or fn(…) -> …")),
            ExprKind::Lambda { params, body } => self.lambda(params, body, e.span),
            _ => err(e.span, "fn: takes a holon's name or a function, fn(a, b) -> …"),
        }
    }
}

/// A `match` pattern's value, for telling two patterns apart: `1` and `1.0` are one number, and
/// a number is never a text (`equals` doesn't coerce).
enum Pattern {
    Number(f64, String),
    Text(String),
    Bool(bool),
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Pattern) -> bool {
        match (self, other) {
            (Pattern::Number(a, _), Pattern::Number(b, _)) => a == b,
            (Pattern::Text(a), Pattern::Text(b)) => a == b,
            (Pattern::Bool(a), Pattern::Bool(b)) => a == b,
            _ => false,
        }
    }
}

impl Pattern {
    fn text(&self) -> String {
        match self {
            Pattern::Number(_, t) => t.clone(),
            Pattern::Text(s) => format!("{s:?}"),
            Pattern::Bool(b) => b.to_string(),
        }
    }
}

fn pattern(e: &Expr) -> Option<Pattern> {
    Some(match &e.kind {
        ExprKind::Number { value, text } => Pattern::Number(*value, text.clone()),
        ExprKind::Text(s) => Pattern::Text(s.clone()),
        ExprKind::Bool(b) => Pattern::Bool(*b),
        _ => return None,
    })
}

/// A literal as `default` freezes it: its type and its text. Records and lists of literals are
/// `Json`.
fn literal_param(e: &Expr) -> Option<(&'static str, String)> {
    Some(match &e.kind {
        ExprKind::Number { text, .. } => ("Number", text.clone()),
        ExprKind::Duration { seconds, .. } => ("Number", number_text(*seconds)),
        ExprKind::Text(s) => ("Text", s.clone()),
        ExprKind::Bool(b) => ("Boolean", b.to_string()),
        ExprKind::Record { .. } | ExprKind::List(_) => ("Json", literal_json(e)?.to_string()),
        ExprKind::Paren(inner) => return literal_param(inner),
        _ => return None,
    })
}

/// A whole number as JSON writes one (`1`, not `1.0`).
fn json_number(n: f64) -> serde_json::Value {
    if n.fract() == 0.0 && n.abs() < 1e15 { json!(n as i64) } else { json!(n) }
}

fn literal_json(e: &Expr) -> Option<serde_json::Value> {
    Some(match &e.kind {
        ExprKind::Number { value, .. } => json_number(*value),
        ExprKind::Duration { seconds, .. } => json_number(*seconds),
        ExprKind::Text(s) => json!(s),
        ExprKind::Bool(b) => json!(b),
        ExprKind::Record { spread: None, fields } => {
            let mut m = serde_json::Map::new();
            for (k, v) in fields {
                m.insert(k.text.clone(), literal_json(v)?);
            }
            serde_json::Value::Object(m)
        }
        ExprKind::List(items) => serde_json::Value::Array(items.iter().map(literal_json).collect::<Option<_>>()?),
        ExprKind::Paren(inner) => return literal_json(inner),
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::composites::Names;
    use crate::flow::{derive_server, flow_unit};
    use crate::{commands, samples, text, to_hex};
    use holon_core::{MapResolver, Resolver};

    /// The unit a document deploys as, through the real core.
    fn hash_of(doc: &FlowDocument) -> String {
        let mut store = MapResolver::new();
        text::restore(doc, &mut store, &Names::new());
        let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
        to_hex(&flow_unit(&doc.nodes, &doc.edges, &mut store, port).unwrap_or_else(|e| panic!("{e}")))
    }

    fn builders_hash(name: &str) -> String {
        hash_of(&samples::build(samples::sample(name).unwrap()).unwrap())
    }

    /// A flow without routes is one composite. An input a leaf works without (an http_request's
    /// headers and body, a listing's prefix and after) stays unbound in it, as in a route:
    /// promoted to the boundary it would be wired but absent, and core would hold the leaf back
    /// on every run (a clock flow that never called out, or never listed its collection).
    #[test]
    fn a_routeless_flow_leaves_optional_inputs_unbound() {
        let got = compiled(
            "flow poll\nevery 10s\ncollection items\n\npage = http.get(\"h.example\", \"/\")\nrecent = items.list(limit: 5)\nstate.last = { status: page.status, n: length(keys(recent)) }\n",
        );
        let mut store = MapResolver::new();
        text::restore(&got.doc, &mut store, &Names::new());
        let hash = flow_unit(&got.doc.nodes, &got.doc.edges, &mut store, "").unwrap_or_else(|e| panic!("{e}"));
        let unit = store.resolve(&hash).expect("the unit is stored");
        let inputs: Vec<&str> = unit.inputs.iter().map(|p| p.name.as_str()).collect();
        assert!(inputs.is_empty(), "headers, body, prefix and after stay unbound, not {inputs:?}");
    }

    /// Every sample's text (samples/*.holon): the natural form, as fmt writes it.
    pub(crate) fn sample_texts() -> Vec<(&'static str, &'static str)> {
        samples::samples().iter().map(|s| (s.name, s.text)).collect()
    }

    fn compiled(src: &str) -> Lowered {
        compile(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    fn error(src: &str) -> (u32, u32, String) {
        let e = compile(src).expect_err(src);
        (e.span.start.line, e.span.start.col, e.message)
    }

    /// The leaves placed, by name, in order.
    fn leaves(doc: &FlowDocument) -> Vec<String> {
        doc.nodes.iter().map(|n| n.data.name.clone()).collect()
    }

    // ── the golden tests: a text lowers to the builder's unit ──

    #[test]
    fn every_sample_in_the_wiring_form_lowers_to_the_same_unit_as_its_text() {
        for s in samples::samples() {
            let doc = samples::build(s).unwrap();
            let src = text::print(Some(s.name), &doc, &mut MapResolver::new(), &Names::new()).unwrap();
            let got = compile(&src).unwrap_or_else(|e| panic!("{}: {e}\n{src}", s.name));
            assert_eq!(hash_of(&got.doc), builders_hash(s.name), "{}", s.name);
            assert_eq!(got.name.as_deref(), Some(s.name));
        }
        // And the starter, in the expression form.
        assert!(compile(commands::STARTER).is_ok());
    }

    // ── each construct, by the nodes it places ──

    #[test]
    fn literals_and_interpolation() {
        let d = compiled("x = 1\ny = 5m\nz = \"t\"\nw = true\ns = \"Hello, {z}!\"\n").doc;
        assert_eq!(leaves(&d), ["const", "const", "const", "const", "const", "as_text", "const", "concat"]);
        assert_eq!(d.nodes[1].param("value"), "300");
        assert_eq!(d.nodes[3].param("type"), "Boolean");
        assert_eq!(d.edges.len(), 4, "z into as_text, and the three pieces into concat");
        // One part alone is just as_text; text alone is a const.
        assert_eq!(leaves(&compiled("x = 1\ns = \"{x}\"").doc), ["const", "as_text"]);
        assert_eq!(leaves(&compiled("s = \"{{plain}}\"").doc), ["const"]);
    }

    #[test]
    fn records_lists_and_fields() {
        let d = compiled("r = { a: 1, \"b c\": 2 }\nq = { ..r, d: 3 }\nf = q.d\nl = [1, 2, 3]\ne = []\no = [1]\n").doc;
        assert_eq!(
            leaves(&d),
            ["record", "const", "with_field", "const", "with_field", "const", "with_field", "field", "const", "const", "const", "list", "const", "const", "const", "append"]
        );
        assert_eq!(d.nodes[4].param("name"), "b c");
        assert_eq!(d.nodes[11].data.inputs.len(), 3);
        assert_eq!(d.nodes[12].param("type"), "Json");
    }

    #[test]
    fn operators_lower_to_their_leaves_and_chains_flatten_unless_parenthesised() {
        let d = compiled("a = 1\nb = 2\nc = 3\ns = a + b + c\nt = (a + b) + c\nu = a - b\nv = a < b\nw = a == b\nx = a != b\ny = !w\nz = a && b || c\n").doc;
        let names = leaves(&d);
        assert_eq!(&names[3..], ["add", "add", "add", "subtract", "compare", "equals", "equals", "not", "not", "and", "or"]);
        assert_eq!(d.nodes[3].data.inputs.len(), 3, "a + b + c is one add with three inputs");
        assert_eq!(d.nodes[4].data.inputs.len(), 2, "(a + b) + c is two");
        assert_eq!(d.nodes[7].param("op"), "<");
    }

    #[test]
    fn fallbacks_guards_and_if() {
        let d = compiled("a = 1\nb = 2\nc = true\nx = a ?? 0\ny = a ?? b\nz = a when c\nw = if c then a\nv = if c then a else b\nu = if c then a else \"none\"\nj = a ?? { k: [1, \"x\"] }\n").doc;
        let names = leaves(&d);
        assert_eq!(&names[3..], ["default", "coalesce", "guard", "guard", "guard", "coalesce", "guard", "default", "default"]);
        assert_eq!((d.nodes[3].param("type"), d.nodes[3].param("value")), ("Number", "0"));
        assert_eq!((d.nodes[10].param("type"), d.nodes[10].param("value")), ("Text", "none"));
        assert_eq!((d.nodes[11].param("type"), d.nodes[11].param("value")), ("Json", "{\"k\":[1,\"x\"]}"));
    }

    #[test]
    fn calls_with_named_inputs_params_and_variadics() {
        let d = compiled("a = \"x\"\nb = \"y\"\nc = concat(a, b, a)\nh = hmac_sha256(a, b, encoding: base64)\n").doc;
        assert_eq!(leaves(&d), ["const", "const", "concat", "hmac_sha256"]);
        assert_eq!(d.nodes[2].data.inputs.len(), 3);
        assert_eq!(d.nodes[3].param("encoding"), "base64");
        // Named inputs, optional ones left out, a zero-input leaf's positional params.
        let d = compiled("collection notes\nn = notes.list(after: \"a\", limit: 5)\nt = resume_token(paid)\nq = http.get(\"h.example\", \"/p\")\n").doc;
        assert_eq!(leaves(&d), ["const", "collection_list", "resume_token", "const", "http_request"]);
        assert_eq!(d.nodes[1].param("limit"), "5");
        assert_eq!(d.nodes[2].param("await"), "paid");
        assert_eq!((d.nodes[4].param("method"), d.nodes[4].param("host")), ("GET", "h.example"));
        assert_eq!(d.edges.len(), 2);
    }

    #[test]
    fn nouns_and_writes_are_one_leaf_each() {
        let d = compiled("collection notes\nk = \"key\"\nx = secret.tok\ny = state.beats\nz = now\nr = random\ni = ingress\ng = notes[k]\nstate.saved = y\nnotes[k] = x\ndelete notes[k]\np = notes.put(k, x)\nw = state.set(other, x)\n").doc;
        assert_eq!(
            leaves(&d),
            ["const", "secret", "store_read", "clock", "random", "ingress", "collection_get", "store_write", "collection_put", "collection_delete", "collection_put", "store_write"]
        );
        assert_eq!(d.nodes[1].param("name"), "tok");
        assert_eq!(d.nodes[7].param("key"), "saved");
        assert_eq!(d.nodes[11].param("key"), "other");
    }

    #[test]
    fn a_multi_output_value_selects_an_output_by_name_else_a_field() {
        let d = compiled("route GET /x (req) {\n  a = req.params\n  b = req.params.id\n  c = req.path\n  d = req.request.body\n  respond d\n}\n").doc;
        assert_eq!(leaves(&d), ["request", "field", "field", "field", "respond"]);
        let fields: Vec<&str> = d.nodes[1..4].iter().map(|n| n.param("name")).collect();
        assert_eq!(fields, ["id", "path", "body"]);
        let from_params = d.edges.iter().filter(|w| w.output() == "params").count();
        assert_eq!(from_params, 1);
    }

    #[test]
    fn holons_lambdas_and_their_errors() {
        let d = compiled("holon twice(n) -> o {\n  o = n + n\n}\nholon total(xs) -> amount {\n  amount = fold(0, xs, fn: fn(acc, p) -> acc + p)\n}\nx = twice(1)\ny = map([1, 2], fn: twice)\n").doc;
        assert_eq!(leaves(&d), ["const", "twice", "const", "const", "list", "map"]);
        // The document's units are the closure: the composites and the leaves inside them.
        let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let composites = d
            .units
            .as_ref()
            .unwrap()
            .iter()
            .filter(|h| matches!(holon_core::decode_preimage(&unhex(h)).unwrap().body, holon_core::UnitBody::Composite { .. }))
            .count();
        assert_eq!(composites, 3, "twice, the fold's function, total");
        // `n + n` uses the input twice: one add with its two inputs fed by one shared pass-through.
        let twice = &d.nodes[1];
        assert_eq!(twice.data.inputs.len(), 1);
        assert_eq!(error("holon h(a) -> o {\n  o = a + 1\n}\nx = h(1)\nout = 2"), (5, 1, "\"out\" is a keyword; a binding may not shadow it".into()));
        // A port may be named after a leaf or `out` (the wiring form does), not after a keyword.
        assert!(compile("holon h(text) -> (out, sum) {\n  out = uppercase(text)\n  sum = text_length(text)\n}\nx = h(\"a\")").is_ok());
        assert_eq!(error("holon h(route) -> o {\n  o = route + 1\n}"), (1, 9, "\"route\" is taken; pick another name for the port".into()));
        assert_eq!(d.nodes[5].param("fn").len(), 64);
        assert_eq!(error("holon h(a) -> o {\n  o = a\n}"), (2, 7, "an output can't be an input passed straight through: put a node between".into()));
        assert_eq!(error("holon h(a, b) -> o {\n  o = a + 1\n}"), (1, 12, "the input \"b\" is never used".into()));
        assert_eq!(error("holon h(a) -> (o, p) {\n  o = a + 1\n}"), (1, 19, "the output \"p\" is never bound".into()));
        // A function's body sees what its holon sees, and nothing beyond it.
        assert_eq!(error("route GET /a (req) {\n  respond h([1])\n}\nholon h(a) -> o {\n  o = map(a, fn: fn(p) -> p + req)\n}"), (5, 31, "unknown name \"req\"".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = h(a)\n}"), (2, 7, "the holon \"h\" places itself: a holon can't contain itself".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = a + 1\n}\nx = h(1, 2)"), (4, 10, "\"h\" takes 1 inputs".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = a + 1\n}\nx = h(k: 1)"), (4, 7, "\"h\" has no input \"k\" (a holon takes no params)".into()));
    }

    /// A name a function uses from around it is an input of the function after its parameters,
    /// and `map` (or `fold`) passes it after the item, wired from where the name is bound.
    #[test]
    fn a_function_takes_what_it_uses_from_around_it_as_inputs() {
        let d = compiled("route POST /v (req) {\n  r = req.json\n  respond map(keys(r), fn: fn(k) -> r[k])\n}\n").doc;
        let map = d.nodes.iter().find(|n| n.data.name == "map").unwrap();
        assert_eq!(map.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["list", "in2"]);
        assert_eq!(map.data.inputs[1].ty, WType::Any, "an extra input is whatever the function takes");
        let into = |node: &str, port: &str| d.edges.iter().find(|w| w.target == node && w.target_handle.as_deref() == Some(port)).map(|w| (w.source.clone(), w.source_handle.clone()));
        let keys = d.nodes.iter().find(|n| n.data.name == "keys").unwrap();
        assert!(into(&map.id, "in2").is_some());
        assert_eq!(into(&map.id, "in2"), into(&keys.id, "record"), "r, from where it is bound");
        // The function: (k, r) -> r[k], a unit of its own, the same however often it is written.
        let same = compiled("route POST /v (req) {\n  r = req.json\n  a = map(keys(r), fn: fn(k) -> r[k])\n  b = map(keys(r), fn: fn(k) -> r[k])\n  respond [a, b]\n}\n").doc;
        let fns: Vec<&str> = same.nodes.iter().filter(|n| n.data.name == "map").map(|n| n.param("fn")).collect();
        assert_eq!(fns[0], fns[1]);
        assert_eq!(fns[0], map.param("fn"));
        // Written out: the value passed after the list, to a function of two inputs.
        let explicit = compiled("route POST /v (req) {\n  r = req.json\n  respond map(keys(r), r, fn: fn(k, r) -> r[k])\n}\n").doc;
        assert_eq!(explicit.nodes.iter().find(|n| n.data.name == "map").unwrap().param("fn"), map.param("fn"), "one unit, captured or passed");
        // fold passes them after the result so far and the item; a holon's input is captured too,
        // and a top-level constant is lowered inside.
        assert!(compile("rate = 3\nholon h(xs, k) -> o {\n  o = fold(0, xs, fn: fn(acc, p) -> acc + p * k * rate)\n}\nroute GET /a (req) {\n  respond h([1, 2], 2)\n}\n").is_ok());
        // Nested: the inner function's capture passes through the outer one.
        assert!(compile("route POST /v (req) {\n  n = req.json.n\n  respond map(req.json.rows, fn: fn(row) -> map(row, fn: fn(x) -> x * n))\n}\n").is_ok());
    }

    /// A function given the wrong number of inputs is said at the function, counting what the
    /// call gives it.
    #[test]
    fn a_function_of_the_wrong_arity_is_said() {
        assert_eq!(error("x = map([1], fn: fn(a, b) -> a + b)"), (1, 18, "\"map\" gives its function 1 inputs (the item); this one takes 2".into()));
        assert_eq!(error("x = fold(0, [1], 5, fn: fn(acc, p) -> acc + p)"), (1, 25, "\"fold\" gives its function 3 inputs (the result so far and the item, then 1 more); this one takes 2".into()));
        assert_eq!(error("holon h(a, b, c) -> o {\n  o = a + b + c\n}\nx = map([1], fn: h)"), (4, 18, "\"map\" gives its function 1 inputs (the item); this one takes 3".into()));
    }

    #[test]
    fn a_holons_effect_statements_leave_as_outputs() {
        let d = compiled("collection posts\nholon keep(k, v) -> receipt {\n  posts[k] = v\n  state.last = k\n  receipt = \"ok\"\n}\nx = keep(\"a\", 1)\n").doc;
        let keep = &d.nodes[2];
        let outs: Vec<&str> = keep.data.outputs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(outs, ["receipt", "put", "write"]);
    }

    /// A route's sinks are tied to its request; `after x` ties a statement's to x instead; a
    /// holon's to its own trigger, an input it gets last, which the block placing it ties.
    #[test]
    fn sinks_are_tied_to_their_block_or_to_what_after_names() {
        let src = "collection c\n\nholon keep(v) -> saved {\n  saved = state.set(last, v)\n}\n\nroute POST /v/:id (req) {\n  id = req.params.id\n  put = c.put(id, 1)\n  state.last = id after put\n  keep(id)\n  respond id\n}";
        let d = compiled(src).doc;
        let node = |id: &str| d.nodes.iter().find(|n| n.id == id).unwrap();
        let tie = |leaf: &str| {
            let n = d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
            let w = d.edges.iter().find(|w| w.target == n.id && w.input() == TRIGGER).unwrap_or_else(|| panic!("{leaf} is tied"));
            (node(&w.source).data.name.clone(), w.output().to_string())
        };
        assert_eq!(tie("collection_put"), ("request".into(), "request".into()));
        assert_eq!(tie("store_write"), ("collection_put".into(), "out".into()), "after put");
        assert_eq!(tie("keep"), ("request".into(), "request".into()), "the placed holon");
        let keep = d.nodes.iter().find(|n| n.data.name == "keep").unwrap();
        assert_eq!(keep.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["v", TRIGGER]);
        // One that declares its trigger uses it; a holon with no sink takes none.
        let d = compiled("holon keep(v, trigger) -> saved {\n  saved = state.set(last, v)\n}\nholon twice(n) -> m {\n  m = n * 2\n}\nkeep(twice(1), trigger: 2)").doc;
        let ins = |name: &str| d.nodes.iter().find(|n| n.data.name == name).unwrap().data.inputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(ins("keep"), ["v", TRIGGER]);
        assert_eq!(ins("twice"), ["n"]);
        // `after` on a statement with nothing to tie is said.
        assert!(error("route GET /a (req) {\n  n = 1 after req\n  respond n\n}").2.contains("this statement makes none"));
    }

    /// `calls billing` names a flow whose actions this one runs: `billing.charge(x)` is a
    /// `flow_call{flow, action}` tied to the block's trigger as any sink is, which asks for
    /// `RunsAction("billing/charge")` (docs/L4s/triggers-L4.md §9, slice 6). A slug's `-` is `_`
    /// in the call.
    #[test]
    fn calls_run_another_flows_action_tied_to_the_block() {
        let d = compiled("calls billing-v2\nroute POST /p (req) {\n  r = billing_v2.charge({ amount: req.json.total })\n  respond r.answer\n}\n").doc;
        let call = d.nodes.iter().find(|n| n.data.name == "flow_call").expect("the call");
        assert_eq!((call.param("flow"), call.param("action")), ("billing-v2", "charge"));
        let request = d.nodes.iter().find(|n| n.is_template("request")).unwrap();
        let tie = d.edges.iter().find(|w| w.target == call.id && w.input() == TRIGGER).map(|w| w.source.clone());
        assert_eq!(tie, Some(request.id.clone()), "tied to the request, though it reads none of it");
        // Nothing sent is `{}`; `after` ties a call to a value, and a lone call is a statement.
        let d = compiled("calls billing\ncollection c\nroute POST /p (req) {\n  found = c[req.body]\n  billing.reset() after found\n  respond 204\n}\n").doc;
        let call = d.nodes.iter().find(|n| n.data.name == "flow_call").unwrap();
        assert!(!d.edges.iter().any(|w| w.target == call.id && w.input() == "input"), "no input wired");
        let found = d.nodes.iter().find(|n| n.data.name == "collection_get").unwrap();
        assert!(d.edges.iter().any(|w| w.target == call.id && w.input() == TRIGGER && w.source == found.id), "after the lookup");
        // A name that is already a name, a name twice, a bare noun, a binding on it.
        assert_eq!(error("calls route\nroute GET /a (req) {\n  respond 1\n}").2, "\"route\" is already a name here (a keyword, a leaf, a holon or something declared): a flow called by it can't be named so");
        assert_eq!(error("calls a\ncalls a\nroute GET /a (req) {\n  respond 1\n}").2, "`calls` names \"a\" twice");
        assert_eq!(error("calls a\nroute GET /a (req) {\n  respond a\n}").2, "\"a\" is a flow: run one of its actions, a.action(input)");
        assert_eq!(error("calls a\nroute GET /a (req) {\n  a = 1\n  respond a\n}").2, "\"a\" is a flow; a binding may not shadow it");
        assert_eq!(error("calls a\nroute GET /a (req) {\n  respond a.go(1, 2)\n}").2, "an action takes one input: a.go(x), with x a record to send more");
        // `calls` is a name anywhere else.
        compiled("route GET /a (req) {\n  calls = 1\n  respond calls + req.query.n\n}");
    }

    /// A clock, a file handler or an action whose only work is a call is a handler: the call is
    /// made for what it does, whatever it answers.
    #[test]
    fn a_block_of_calls_alone_is_a_handler() {
        let d = compiled("calls billing\nevery 1h {\n  billing.reset()\n}\n").doc;
        let mut store = MapResolver::new();
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
        assert_eq!(server.errors, Vec::<String>::new());
        assert_eq!(server.clocks.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["every 1h"]);
        let d = compiled("calls billing\naction run (input) {\n  r = billing.charge(input)\n  respond r\n}\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
        assert_eq!(server.errors, Vec::<String>::new());
        assert_eq!(server.actions.len(), 1);
    }

    /// `every 2s { … }` is a clock handler: its tick node, everything wired to it, its sinks tied
    /// to the tick, and `now` in it the tick's time (docs/L4s/triggers-L4.md §2, §3).
    #[test]
    fn every_blocks_are_clock_handlers_tied_to_their_tick() {
        let d = compiled("channel ticks\nevery 2s {\n  state.n = (state.n ?? 0) + 1\n  ticks.send(now)\n}\nroute GET /n (req) {\n  respond state.n ?? 0\n}\n").doc;
        let every = d.nodes.iter().find(|n| n.is_template("every")).expect("the tick");
        assert_eq!(every.param("seconds"), "2");
        assert!(!d.nodes.iter().any(|n| n.data.name == "clock"), "now in a block is the tick's");
        let from = |leaf: &str, input: &str| {
            let n = d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
            d.edges.iter().find(|w| w.target == n.id && w.input() == input).map(|w| (w.source.clone(), w.output().to_string()))
        };
        let tick = Some((every.id.clone(), "now".to_string()));
        assert_eq!(from("store_write", TRIGGER), tick, "the write is tied to the tick");
        assert_eq!(from("channel_send", TRIGGER), tick);
        assert_eq!(from("channel_send", "pack"), tick, "now");

        let mut store = MapResolver::new();
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
        assert_eq!(server.errors, Vec::<String>::new());
        assert_eq!((server.endpoints.len(), server.clocks.iter().map(|c| c.label.as_str()).collect::<Vec<_>>()), (1, vec!["every 2s"]));
        // A flow of clocks alone is a server with no port.
        let d = compiled("every 1h {\n  state.n = (state.n ?? 0) + 1\n}\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        let read = holon_core::read_server(unit, &store).unwrap();
        assert_eq!((read.port, read.routes.len(), read.clocks[0].seconds), (None, 0, 3600));

        assert_eq!(error("every 2s {\n  respond 1\n}"), (2, 3, "respond belongs in a route: a clock answers nobody".into()));
        assert_eq!(error("every 2s {\n  n = now\n}").2, "the block does nothing: write something in it (state, a collection, a channel)");
        assert_eq!(error("every 1.5s {\n  state.n = 1\n}").2, "every 1.5s: a clock runs every whole number of seconds, from 1s to 366d");
        assert_eq!(error("every 2s {\n  state.a = now\n}\nevery 2s {\n  state.b = now\n}").2, "2s is written twice: one block per period");
        assert!(error("every 10s\nevery 2s {\n  state.n = now\n}").2.starts_with("a flow with `every` or `on file` blocks has its clocks in them"));
        assert_eq!(error("every 2s {\n  state.n = ingress\n}").2, "a clock's run receives nothing: `now` in it is the time of the tick (ingress is a flow's run by hand)");
        // The header form's clock beside handlers would start nothing: refused, naming the block.
        assert_eq!(
            error("every 10s\nport 8080\n\nroute GET / (req) {\n  respond \"hi\"\n}").2,
            "this `every` line is the clock of a flow without handlers: beside routes, put the clock's statements in an `every … { … }` block"
        );
        assert_eq!(
            error("every 10s\n\naction reset (input) {\n  state.n = 0\n}").2,
            "this `every` line is the clock of a flow without handlers: beside actions, put the clock's statements in an `every … { … }` block"
        );
        // A write that reads nothing of the tick is still the block's: tied to it.
        let d = compiled("every 2s {\n  state.n = 1\n}\n").doc;
        assert!(d.edges.iter().any(|w| w.source.starts_with("every") && w.input() == TRIGGER));
    }

    /// `action reset (input) { … }` is an action: its `action` node, everything wired to it, its
    /// sinks tied to what was sent, `respond v` its answer (docs/L4s/triggers-L4.md §2, §3).
    #[test]
    fn action_blocks_are_actions_tied_to_what_was_sent() {
        let src = "action reset (input) {\n  state.ticks = input.to ?? 0\n  respond { was: state.ticks ?? 0 }\n}\naction note (n) {\n  state.note = n\n}\nroute GET /n (req) {\n  respond state.ticks ?? 0\n}\n";
        let d = compiled(src).doc;
        let actions: Vec<&crate::canvas::Node> = d.nodes.iter().filter(|n| n.is_template("action")).collect();
        assert_eq!(actions.iter().map(|n| n.param("name")).collect::<Vec<_>>(), ["reset", "note"]);
        let into = |id: &str, input: &str| d.edges.iter().find(|w| w.target == id && w.input() == input).map(|w| (w.source.clone(), w.output().to_string()));
        let sent = Some((actions[0].id.clone(), "input".to_string()));
        let writes: Vec<&crate::canvas::Node> = d.nodes.iter().filter(|n| n.data.name == "store_write").collect();
        assert_eq!(into(&writes[0].id, TRIGGER), sent, "the write is tied to what was sent");
        let respond = d.nodes.iter().find(|n| n.is_template("respond") && into(&n.id, "request") == sent).expect("the action's respond");
        assert_eq!((respond.param("status"), respond.param("missing")), ("200", "404"));

        let mut store = MapResolver::new();
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
        assert_eq!(server.errors, Vec::<String>::new());
        assert_eq!(server.actions.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["action reset", "action note"]);
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        let read = holon_core::read_server(unit, &store).unwrap();
        assert_eq!((read.port, read.routes.len(), read.actions.len()), (Some(8080), 1, 2));
        // A flow of actions alone listens on no port.
        let d = compiled("action reset (input) {\n  state.n = 0\n}\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        assert_eq!(holon_core::read_server(unit, &store).unwrap().port, None);

        assert_eq!(error("action a (input) {\n  respond 1, status: 201\n}").2, "an action answers with a value alone: status:, missing:, headers: and stream: are a route's");
        assert_eq!(error("action a (input) {\n  n = input\n}").2, "the action does nothing: respond with something, or write something (state, a collection, a channel)");
        assert_eq!(error("action a (input) {\n  state.n = ingress\n}").2, "in an action what was sent is input: use input in place of ingress");
        // An `on file` line beside actions: they are blocks, and the file's run goes in one too.
        assert!(error("on file inbox\n\naction a (input) {\n  state.n = 0\n}").2.starts_with("a flow with handlers has its triggers in them: put the file's statements in an `on file inbox (f) { … }` block"));
        assert!(error("on file inbox\nport 8080\n\nroute GET / (req) {\n  respond \"hi\"\n}").2.starts_with("a flow with handlers has its triggers in them: put the file's statements in an `on file inbox (f) { … }` block"));
        assert_eq!(error("action a (x) {\n  state.a = x\n}\naction a (y) {\n  state.b = y\n}").2, "action a is written twice: one block per name");
        assert_eq!(error("action a (input) {\n  respond 1\n  respond 2\n}").2, "more than one respond in the action (the first is on line 2)");
        // An answer that reads nothing of what was sent is still the action's.
        assert!(compile("action ping (input) {\n  respond \"pong\"\n}\n").is_ok());
    }

    /// `on file drop (f) { … }` is a file handler: its `on_file` node, everything wired to it,
    /// its sinks tied to the file, and `f` the file's handle (docs/L4s/triggers-L4.md §2, §3).
    #[test]
    fn on_file_blocks_are_file_handlers_tied_to_their_file() {
        let src = "folder done\nchannel drops\non file drop (f) {\n  drops.send(f.name)\n  done[f.name] = f\n}\non file drop deleted matching \"*.csv\" (gone) {\n  state.last = gone.name\n}\nroute GET /n (req) {\n  respond state.last\n}\n";
        let d = compiled(src).doc;
        let on: Vec<&crate::canvas::Node> = d.nodes.iter().filter(|n| n.is_template("on_file")).collect();
        assert_eq!(on.iter().map(|n| (n.param("folder"), n.param("on"), n.param("matching"))).collect::<Vec<_>>(), [("drop", "new", ""), ("drop", "deleted", "*.csv")]);
        assert!(!d.nodes.iter().any(|n| n.data.name == "file_arrival"), "f in a block is its on_file node's");
        let from = |leaf: &str, input: &str| {
            let n = d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
            d.edges.iter().find(|w| w.target == n.id && w.input() == input).map(|w| (w.source.clone(), w.output().to_string()))
        };
        let file = Some((on[0].id.clone(), "file".to_string()));
        assert_eq!(from("channel_send", TRIGGER), file, "the send is tied to the file");
        assert_eq!(from("file_copy", "from"), file, "f is a handle: written to a folder, it is copied");
        assert_eq!(from("store_write", TRIGGER), Some((on[1].id.clone(), "file".to_string())));

        let mut store = MapResolver::new();
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
        assert_eq!(server.errors, Vec::<String>::new());
        let labels: Vec<&str> = server.files.iter().map(|f| f.label.as_str()).collect();
        assert_eq!((server.endpoints.len(), labels), (1, vec!["on file drop", "on file drop deleted matching \"*.csv\""]));
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        let read = holon_core::read_server(unit, &store).unwrap();
        assert_eq!((read.port, read.files.len()), (Some(8080), 2));
        assert!(holon_core::capabilities(unit, &store).contains(holon_core::Capability::FileArrival("drop".into())));
        // A flow of file handlers alone listens on no port.
        let d = compiled("on file drop (f) {\n  state.n = f.size\n}\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        assert_eq!(holon_core::read_server(unit, &store).unwrap().port, None);

        assert_eq!(error("on file a (f) {\n  respond 1\n}"), (2, 3, "respond belongs in a route: a file's run answers nobody".into()));
        assert_eq!(error("on file a (f) {\n  n = f.name\n}").2, "the block does nothing: write something in it (a folder, state, a channel)");
        assert_eq!(error("on file a (f) {\n  state.n = ingress\n}").2, "in an `on file` block the file is f: use f in place of ingress");
        assert_eq!(error("on file a (f) {\n  state.a = f\n}\non file a (g) {\n  state.b = g\n}").2, "on file a is written twice: one block per line");
        assert!(error("on file a matching \"a**\" (f) {\n  state.a = f\n}").2.contains("isn't a pattern of names"));
        assert!(error("on file b\non file a (f) {\n  state.a = f\n}").2.starts_with("a flow with handlers has its triggers in them: put the file's statements in an `on file b (f) { … }` block"));
        assert!(error("every 10s\non file a (f) {\n  state.a = f\n}").2.starts_with("a flow with `every` or `on file` blocks has its clocks in them"));
        // The header form is unchanged: `ingress` is the file.
        assert!(leaves(&compiled("on file inbox\nstate.n = ingress.name\n").doc).contains(&"file_arrival".to_string()));
    }

    #[test]
    fn routes_respond_and_their_errors() {
        let d = compiled("route GET /a (req) {\n  respond 204\n}\nroute POST /b (r) resumes paid {\n  respond r.json, status: 201, missing: 400\n}\n").doc;
        assert_eq!(leaves(&d), ["request", "respond", "request", "respond"]);
        assert_eq!((d.nodes[1].param("status"), d.nodes[1].data.inputs.len()), ("204", 1));
        assert_eq!((d.nodes[3].param("status"), d.nodes[3].param("missing"), d.nodes[2].param("resume")), ("201", "400", "paid"));
        assert_eq!(d.edges.iter().filter(|w| w.input() == "request").count(), 2, "respond is wired to its request");
        assert_eq!(error("respond 1"), (1, 1, "respond belongs in a route".into()));
        assert_eq!(error("route GET /a (req) {\n  respond 1\n  respond 2\n}"), (3, 3, "more than one respond in the route (the first is on line 2)".into()));
        assert_eq!(error("route GET /a (req) {\n  x = 1\n}"), (1, 7, "the route answers nothing: add a respond (or an effect, answered 204)".into()));
        assert_eq!(error("route GET /a (req) {\n  respond 1, 2\n}"), (2, 14, "respond takes one body; status: and missing: are named".into()));
        assert_eq!(error("route GET /a (req) {\n  respond 1, status: 700\n}"), (2, 22, "a status is a number from 200 to 599".into()));
        assert_eq!(error("route GET /a (req) {\n  respond 1, status: 101\n}"), (2, 22, "a status is a number from 200 to 599".into()), "not an interim one");
        assert_eq!(error("route GET /a (req) {\n  respond \"x\", status: 204\n}"), (2, 11, "a 204 answers with no body".into()));
        assert_eq!(error("route GET /a (req) {\n  1 + 1\n  respond 2\n}"), (2, 3, "a value nobody uses: bind it, or remove it".into()));
        // A write that reads nothing of the request is tied to it by its trigger: a route counts
        // its hits (docs/L4s/triggers-L4.md §3).
        let d = compiled("route GET /a (req) {\n  state.hits = (state.hits ?? 0) + 1\n  respond 2\n}").doc;
        let tie = d.edges.iter().find(|w| w.target.starts_with("store_write") && w.input() == "trigger").expect("the write is tied");
        assert!(tie.source.starts_with("request") && tie.output() == "request", "{tie:?}");
        // A value that never touches the request isn't in the route's group: said, not dropped.
        assert_eq!(
            error("route GET /a (req) {\n  n = length([1, 2])\n  respond 2\n}"),
            (2, 15, "this isn't wired to the route's request, so it isn't part of the route (a route is everything wired to its request): use the request in it, or move it to a flow of its own".into())
        );
        assert_eq!(error("x = 1 + 1\nroute GET /a (req) {\n  respond x\n}"), (1, 1, "routes are separate units; put shared logic in a holon (only a literal may be bound at the top level)".into()));
        // A literal at the top level is placed afresh in each route that uses it.
        let d = compiled("greeting = \"hi\"\nroute GET /a (req) {\n  respond greeting\n}\nroute GET /b (req) {\n  respond greeting\n}\n").doc;
        assert_eq!(leaves(&d), ["request", "const", "respond", "request", "const", "respond"]);
    }

    #[test]
    fn folders_read_write_delete_and_list_files_by_name() {
        let d = compiled("folder inbox\nfolder out\nt = inbox[\"a.csv\"]\nl = inbox.list(prefix: \"2026/\")\nout[\"b.md\"] = t\ndelete out[\"old.md\"]\n").doc;
        let mut got = leaves(&d);
        got.retain(|l| l != "const");
        assert_eq!(got, ["file_read", "file_list", "file_write", "file_delete"]);
        let folder = |leaf: &str| d.nodes.iter().find(|n| n.data.name == leaf).unwrap().param("folder").to_string();
        assert_eq!([folder("file_read"), folder("file_list"), folder("file_write"), folder("file_delete")], ["inbox", "inbox", "out", "out"]);
        assert_eq!(error("folder inbox\nx = inbox"), (2, 5, "\"inbox\" is a folder: read it as inbox[name] or inbox.list(…)".into()));
        assert_eq!(error("folder inbox\ninbox = 1"), (2, 1, "\"inbox\" is a folder; a binding may not shadow it".into()));
        assert_eq!(error("folder inbox\nx = inbox.move(\"a\")"), (2, 11, "a folder has no \"move\": get, put, delete, list, file, lines, slice, copy, create or create_copy".into()));
        assert_eq!(error("collection notes\nx = notes.file(\"a\")"), (2, 11, "a collection has no \"file\": get, put, delete or list".into()));
        assert_eq!(error("folder = 1"), (1, 8, "expected the folder's name, found \"=\"".into()));
    }

    #[test]
    fn databases_take_their_sql_in_quotes_and_its_values_after_it() {
        let d = compiled("database shop
rows = shop.query(\"select id from orders where customer = $1 and total > $2\", \"ada\", 10)
first = shop.one(\"select id from orders\")
shop.exec(\"update stock set n = n - 1 where sku = $1\", rows, expect: 1)
").doc;
        let node = |leaf: &str| d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
        assert_eq!((node("db_query").param("database"), node("db_exec").param("expect")), ("shop", "1"));
        assert_eq!(node("db_one").param("sql"), "select id from orders");
        let into = |id: &str| d.edges.iter().find(|w| w.target == id && w.target_handle.as_deref() == Some("params")).map(|w| d.nodes.iter().find(|n| n.id == w.source).unwrap().data.name.clone());
        assert_eq!(
            [into(&node("db_query").id), into(&node("db_one").id), into(&node("db_exec").id)],
            [Some("list".into()), Some("const".into()), Some("append".into())],
            "its values wired as a list"
        );
        let error = |src: &str| error(&format!("database shop
{src}")).2;
        assert_eq!(error("x = shop.query(\"select id from t where a = $1 and b = $2\", 1)"), "the SQL takes 2 values, for $1 to $2; 1 given");
        assert_eq!(error("x = shop.query(q)"), "the SQL is written in quotes, as it is: its tables are the permission, and values go after it as $1, $2, …, never into it");
        assert!(error("x = shop.query(\"drop table t\")").starts_with("this SQL can't be sent: a read is a select"));
        assert!(error("shop.exec(\"delete from t; delete from u\")").contains("more than one statement"));
        assert_eq!(error("x = shop.query(\"select 1\")"), "this SQL reads no table: what it selects, the flow can compute itself");
        assert_eq!(error("x = shop.query(\"select a from t\", expect: 1)"), "shop.query has no \"expect\": it takes the SQL, then its values");
        assert_eq!(error("shop.exec(\"delete from t\", expect: -1)"), "expect: is how many rows it must change, a whole number");
        assert_eq!(error("x = shop.pick(\"a\")"), "a database has no \"pick\": query, one or exec (SQL), or find, find_one, insert, update or delete (a document database)");
        assert_eq!(error("x = shop"), "\"shop\" is a database: read it as shop.query(\"select …\") or shop.one(…)");
        assert_eq!(error("shop = 1"), "\"shop\" is a database; a binding may not shadow it");
    }

    #[test]
    fn document_calls_freeze_their_records_and_wire_each_computed_value_as_a_hole() {
        let d = compiled("database notes
route POST /n/:who (req) {
  mine = notes.find(\"notes\", { owner: req.params.who, state: { \"$in\": [\"open\", req.json.also] } }, sort: { at: -1 }, limit: 20)
  one = notes.find_one(\"notes\")
  notes.insert(\"notes\", { owner: req.params.who, text: req.json.text, tags: [\"new\"], pinned: false })
  notes.update(\"notes\", { _id: { \"$oid\": req.json.id } }, { \"$set\": { text: req.json.text }, \"$inc\": { edits: 1 } }, expect: 1)
  notes.delete(\"notes\", { owner: req.params.who, done: true })
  respond { mine: mine, one: one }
}
").doc;
        let node = |leaf: &str| d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
        let find = node("db_find");
        assert_eq!(
            (find.param("collection"), find.param("filter"), find.param("sort"), find.param("limit")),
            ("notes", r#"{"owner":$1,"state":{"$in":["open",$2]}}"#, r#"{"at":-1}"#, "20")
        );
        assert_eq!(node("db_find_one").param("filter"), "{}", "a find with no filter finds everything");
        assert_eq!(node("db_insert").param("document"), r#"{"owner":$1,"text":$2,"tags":["new"],"pinned":false}"#);
        let update = node("db_update");
        assert_eq!((update.param("filter"), update.param("update"), update.param("expect")), (r#"{"_id":{"$oid":$1}}"#, r#"{"$set":{"text":$2},"$inc":{"edits":1}}"#, "1"));
        assert_eq!(node("db_delete").param("filter"), r#"{"owner":$1,"done":true}"#);
        let into = |id: &str| d.edges.iter().find(|w| w.target == id && w.target_handle.as_deref() == Some("params")).map(|w| d.nodes.iter().find(|n| n.id == w.source).unwrap().data.name.clone());
        assert_eq!([into(&find.id), into(&node("db_find_one").id), into(&node("db_delete").id)], [Some("list".into()), Some("const".into()), Some("append".into())]);
        let error = |src: &str| error(&format!("database notes
{src}")).2;
        assert_eq!(error("x = notes.find(c)"), "the collection is written in quotes, as it is: it is the permission");
        assert!(error("x = notes.find(\"holon_claims\")").starts_with("a collection is letters"));
        assert!(error("x = notes.find(\"notes\", f)").starts_with("a document call's filter, update, document and sort are records written in the call"));
        assert!(error("x = notes.find(\"notes\", { ..f, a: 1 })").contains("a spread would let the data choose its fields"));
        assert!(error("x = notes.find(\"notes\", { \"$where\": \"1\" })").contains("runs code on the database's server"));
        assert!(error("notes.update(\"notes\", { a: 1 }, { a: 2 })").contains("isn't an update operator"));
        assert!(error("notes.insert(\"notes\", { \"$set\": 1 })").contains("is an operator"));
        assert_eq!(error("x = notes.find(\"notes\", {}, sort: { at: 2 })"), "this sort can't be sent: a sort's fields are each 1 or -1, written as they are: \"at\" isn't");
        assert_eq!(error("x = notes.find(\"notes\", {}, sort: { at: k })"), "a sort is the flow's: each field is 1 or -1, as written");
        assert_eq!(error("x = notes.find(\"notes\", {}, limit: 0)"), "limit: is how many documents at most, a whole number from 1 to 100000");
        assert_eq!(error("notes.delete(\"notes\")"), "notes.delete takes the collection, then its filter");
        assert_eq!(error("notes.update(\"notes\", {})"), "notes.update takes the collection, then its filter, then its update");
        assert_eq!(error("x = notes.find_one(\"notes\", {}, limit: 1)"), "notes.find_one has no \"limit\": it takes the collection, its records, then sort:");
        assert_eq!(error("notes.insert(\"notes\", {}, expect: 1)"), "notes.insert has no \"expect\": it takes the collection, its records");
    }

    #[test]
    fn folders_hand_out_handles_lines_and_slices_and_copy_by_handle() {
        let d = compiled(
            "folder logs\nfolder archive\nh = logs.file(\"app.log\")\nlast = logs.lines(\"app.log\", from: -10, limit: 10)\nstart = logs.slice(\"app.log\", offset: 0, length: 64)\narchive[\"app.log\"] = h\narchive.copy(\"b.log\", h)\narchive[\"c.log\"] = start\n",
        )
        .doc;
        let mut got = leaves(&d);
        got.retain(|l| l != "const" && l != "neg" && l != "sub");
        assert_eq!(got, ["file_handle", "file_lines", "file_slice", "file_copy", "file_copy", "file_write"], "a handle written is a copy; a text, a write");
        let lines = d.nodes.iter().find(|n| n.data.name == "file_lines").unwrap();
        assert_eq!((lines.param("folder"), lines.param("limit")), ("logs", "10"));
        let first = compiled("folder logs\nl = logs.lines(\"a\")\n").doc;
        assert!(first.nodes.iter().any(|n| n.data.name == "file_lines"), "from the first line: from is optional");
    }

    #[test]
    fn a_flow_on_new_files_reads_the_file_as_ingress_and_sends_one_by_handle() {
        let d = compiled("on file inbox\nh = inbox.file(ingress.name)\nr = http.put(\"files.example.com\", \"/up\", file: h)\n").doc;
        let mut got = leaves(&d);
        got.retain(|l| l != "const" && l != "field");
        assert_eq!(got, ["file_arrival", "file_handle", "http_upload"]);
        let arrival = d.nodes.iter().find(|n| n.data.name == "file_arrival").unwrap();
        assert_eq!(arrival.param("folder"), "inbox", "on file declares the folder, and names it");
        let upload = d.nodes.iter().find(|n| n.data.name == "http_upload").unwrap();
        assert_eq!((upload.param("method"), upload.param("host")), ("PUT", "files.example.com"));
        assert!(leaves(&compiled("x = ingress\n").doc).contains(&"ingress".to_string()), "without on file, ingress is a request's");
        assert_eq!(error("on file inbox\nx = 1\n").2, "`on file` starts a run for each new file, and the run reads it as `ingress`: use `ingress` (its .name, say)");
        assert!(error("on file a\non file a\nx = ingress\n").2.contains("written twice"));
        assert!(error("on file a matching \"a**\"\nx = ingress\n").2.contains("isn't a pattern of names"));
        assert!(error("port 8000\non file a\nroute GET /x (req) {\n  respond 1\n}\n").2.contains("put the file's statements in an `on file a (f) { … }` block"));
        assert_eq!(error("folder a\nh = a.file(\"x\")\nr = http.put(\"h.example.com\", \"/\", body: \"t\", file: h)\n").2, "a call sends a body or a file, not both");
        assert!(leaves(&compiled("on = 1\nx = on + 1\n").doc).contains(&"add".to_string()), "on is still a name elsewhere");
    }

    #[test]
    fn a_flow_on_several_folders_deletions_and_some_names_reads_whichever_file_started_it() {
        let src = "on file drop matching \"*.csv\"\non file drop deleted\non file inbox\nfolder done\n\ndone[ingress.name] = ingress\n";
        let d = compiled(src).doc;
        let arrivals: Vec<(&str, &str, &str)> =
            d.nodes.iter().filter(|n| n.data.name == "file_arrival").map(|n| (n.param("folder"), n.param("on"), n.param("matching"))).collect();
        assert_eq!(arrivals.len(), 6, "a chain of the three lines for each use of ingress");
        assert_eq!(&arrivals[..3], [("drop", "new", "*.csv"), ("drop", "deleted", ""), ("inbox", "new", "")]);
        assert_eq!(d.nodes.iter().filter(|n| n.data.name == "coalesce").count(), 4, "two for each chain of three");
        assert!(leaves(&d).contains(&"file_copy".to_string()), "ingress is a handle: written to a folder, it is copied");
        // Writes that never replace a file.
        let d = compiled("folder out\nh = out.file(\"a\")\nout.create(\"b.txt\", \"text\")\nout.create_copy(\"c\", h)\n").doc;
        let got = leaves(&d);
        assert!(got.contains(&"file_create".to_string()) && got.contains(&"file_create_copy".to_string()), "{got:?}");
    }

    #[test]
    fn names_collections_and_state_errors() {
        assert_eq!(error("x = y"), (1, 5, "unknown name \"y\"".into()));
        assert_eq!(error("clock = 1"), (1, 1, "\"clock\" is a leaf; a binding may not shadow it".into()));
        assert_eq!(error("now = 1"), (1, 1, "\"now\" is a keyword; a binding may not shadow it".into()));
        assert_eq!(error("x = 1\nx = 2"), (2, 1, "there is a \"x\" already".into()));
        assert_eq!(error("x = clock"), (1, 5, "\"clock\" is a leaf: call it, clock(…)".into()));
        assert_eq!(error("x = notes[1]"), (1, 5, "unknown name \"notes\"".into()));
        assert_eq!(error("notes[1] = 2"), (1, 1, "\"notes\" is not a declared collection (add `collection notes` to the header)".into()));
        assert_eq!(error("x = notes.list()"), (1, 5, "\"notes\" is not a declared collection (add `collection notes` to the header)".into()));
        assert_eq!(error("collection notes\nx = notes"), (2, 5, "\"notes\" is a collection: read it as notes[key] or notes.list(…)".into()));
        assert_eq!(error("state.a = 1\nstate.a = 2"), (2, 1, "state.a is written twice (first on line 1); a run writes a key once".into()));
        assert_eq!(error("x = http.post(h, \"/p\")"), (1, 15, "the host is written as text in quotes: it is the permission the deployment asks for, so it can't come from a wire".into()));
        assert_eq!(error("x = http.fetch(\"h\", \"/p\")"), (1, 10, "http has no \"fetch\": get, post, put, patch, delete or head".into()));
        assert_eq!(error("x = concat(1, k: 2)"), (1, 15, "\"concat\" has no input or param \"k\"".into()));
        assert_eq!(error("x = subtract(1)"), (1, 5, "\"subtract\": the input \"b\" can't be left out".into()));
        assert_eq!(error("x = subtract(1, 2, 3)"), (1, 20, "\"subtract\" takes 2 inputs, 3 given".into()));
        assert_eq!(error("x = hmac_sha256(\"k\", \"m\", encoding: 1 + 1)"), (1, 37, "a param is a fixed value: a number, a duration, text in quotes, true or false, or a word".into()));
        assert_eq!(error("x = request()"), (1, 5, "a route declares its request: route GET /path (req) { … }".into()));
        assert_eq!(error("x = fn(a) -> a"), (1, 5, "a function goes in a fn: argument of map or fold".into()));
    }

    #[test]
    fn settings_and_spans_come_through() {
        let got = compiled("flow f\nport 8200\nevery 10s\nbrowsers https://a.example, *\noptional secret.k\noptional HttpRequest(\"h\")\ncollection c\n\nx = 1\n");
        let s = got.doc.server.unwrap();
        assert_eq!((s.port.as_str(), s.interval.as_str(), s.browsers.as_deref()), ("8200", "10", Some("https://a.example *")));
        assert_eq!(s.requirements.unwrap().optional.unwrap(), ["SecretRead(\"k\")", "HttpRequest(\"h\")"]);
        assert_eq!(got.name.as_deref(), Some("f"));
        let node = &got.doc.nodes[0];
        assert_eq!(node.extra["source"], json!({ "line": 9, "col": 5, "endLine": 9, "endCol": 6 }));
        assert_eq!(got.names[&node.id], "x");
    }

    #[test]
    fn the_natural_samples_derive_the_same_routes_as_the_builders() {
        // The tour's own spellings (interpolation, `now` twice): same routes and manifest shape,
        // not the same hash.
        let hello = compiled("flow hello-api\nport 8101\nroute GET /hello/:name (req) {\n  respond \"Hello, {req.params.name}\"\n}\n");
        let mut store = MapResolver::new();
        let d = derive_server(&hello.doc.nodes, &hello.doc.edges, &mut store, "8101");
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(d.endpoints.iter().map(|e| (e.method.as_str(), e.path.as_str())).collect::<Vec<_>>(), [("GET", "/hello/:name")]);
        assert_eq!(leaves(&hello.doc), ["request", "const", "field", "as_text", "concat", "respond"]);
    }

    /// The holon `pick` of a text, as core evaluates it, given `op` and `v`.
    fn picked(src: &str, op: Option<holon_core::Value>, v: Option<holon_core::Value>) -> Option<holon_core::Value> {
        let doc = compiled(src).doc;
        let mut store = MapResolver::new();
        text::restore(&doc, &mut store, &Names::new());
        let hash = doc.nodes.iter().find_map(|n| match &n.data.body {
            crate::canvas::PlacedBody::Composite { hash } => Some(hash.clone()),
            _ => None,
        });
        let (_, unit) = crate::composites::resolve(&store, &hash.expect("pick is placed")).unwrap();
        let inputs: Vec<Option<holon_core::Value>> = unit.inputs.iter().map(|p| if p.name == "op" { op.clone() } else { v.clone() }).collect();
        holon_core::eval(unit, &inputs, &store).into_vec().remove(0)
    }

    const PICK: &str = "flow f\n\nholon pick(op, v) -> r {\n  r = match op {\n    \"inv\" -> 1 / v\n    \"neg\", \"minus\" -> 0 - v\n    else -> 99\n  }\n}\n\nstate.x = pick(\"inv\", 1)\n";

    /// `match` is sugar (match-L4.md §4): the guards written out by hand are the same unit.
    #[test]
    fn a_match_is_the_same_unit_as_its_guards_written_by_hand() {
        let by_hand = "flow f\n\nholon pick(op, v) -> r {\n  m1 = op == \"inv\"\n  m2 = op == \"neg\" || op == \"minus\"\n  r = (1 / v when m1) ?? (0 - v when m2) ?? (99 when !(m1 || m2))\n}\n\nstate.x = pick(\"inv\", 1)\n";
        assert_eq!(hash_of(&compiled(PICK).doc), hash_of(&compiled(by_hand).doc));
        // Without an else, the chain alone.
        let no_else = "flow f\n\nholon pick(op, v) -> r {\n  r = match op {\n    \"inv\" -> 1 / v\n    \"neg\" -> 0 - v\n  }\n}\n\nstate.x = pick(\"inv\", 1)\n";
        let by_hand = "flow f\n\nholon pick(op, v) -> r {\n  r = (1 / v when op == \"inv\") ?? (0 - v when op == \"neg\")\n}\n\nstate.x = pick(\"inv\", 1)\n";
        assert_eq!(hash_of(&compiled(no_else).doc), hash_of(&compiled(by_hand).doc));
        // One arm and an else: the else guarded by the arm's own comparison, negated.
        let d = compiled("x = 1\ny = match x { 1 -> \"one\", else -> \"other\" }\n").doc;
        assert_eq!(&leaves(&d)[1..], ["const", "equals", "const", "guard", "not", "const", "guard", "coalesce"]);
    }

    /// Every row of match-L4.md §3, through the lowered unit.
    #[test]
    fn a_match_chooses_one_arm_and_its_absence_is_the_matchs() {
        use holon_core::Value::{Boolean, Number, Text};
        let t = |s: &str| Some(Text(s.into()));
        let n = |x: f64| Some(Number(x));
        assert_eq!(picked(PICK, None, n(2.0)), None, "the subject absent: absent, the else too");
        assert_eq!(picked(PICK, t("inv"), n(2.0)), n(0.5), "the arm chosen, present");
        assert_eq!(picked(PICK, t("inv"), n(0.0)), None, "the arm chosen, absent: absent, not the else");
        assert_eq!(picked(PICK, t("neg"), n(0.0)), n(0.0), "another arm absent: no effect");
        assert_eq!(picked(PICK, t("minus"), n(3.0)), n(-3.0), "a second pattern of an arm");
        assert_eq!(picked(PICK, t("zzz"), None), n(99.0), "no pattern matched: the else, whatever the arms");
        let no_else = PICK.replace("    else -> 99\n", "");
        assert_eq!(picked(&no_else, t("zzz"), n(1.0)), None, "no pattern matched, no else: absent");
        let absent_else = PICK.replace("else -> 99", "else -> 2 / v");
        assert_eq!(picked(&absent_else, t("zzz"), n(0.0)), None, "the else chosen, absent: absent");
        // `equals` doesn't coerce: a number is never a text.
        let kinds = "flow f\n\nholon pick(op, v) -> r {\n  r = match op {\n    1 -> \"one\"\n    \"1\" -> \"text one\"\n    true -> \"yes\"\n    else -> v\n  }\n}\n\nstate.x = pick(1, 1)\n";
        assert_eq!(picked(kinds, n(1.0), None), t("one"));
        assert_eq!(picked(kinds, t("1"), None), t("text one"));
        assert_eq!(picked(kinds, Some(Boolean(true)), None), t("yes"));
        assert_eq!(picked(kinds, Some(Boolean(false)), None), None);
    }

    #[test]
    fn a_match_pattern_is_a_literal_matched_once() {
        let literal = "a pattern is a literal (a number, a text, true or false); to test a condition, use `if`";
        assert_eq!(error("x = 1\ny = match x { x -> 1 }\n"), (2, 15, literal.into()));
        assert_eq!(error("x = 1\ny = match x { 1 -> 1, x > 2 -> 2 }\n"), (2, 23, literal.into()));
        assert_eq!(error("x = 1\ny = match x { \"{x}\" -> 1 }\n"), (2, 15, literal.into()));
        assert_eq!(error("x = 1\ny = match x { [1] -> 1 }\n"), (2, 15, literal.into()));
        assert_eq!(error("x = 1\ny = match x { \"add\" -> 1, \"add\" -> 2 }\n"), (2, 27, "\"add\" is matched twice; the second arm could never be chosen".into()));
        assert_eq!(error("x = 1\ny = match x { 1, 2 -> 1, 1.0 -> 2 }\n"), (2, 26, "1.0 is matched twice; the second arm could never be chosen".into()));
        assert_eq!(error("match = 1\n").2, "expected an expression, found \"=\"");
    }

    /// A streamed reply answers with its call's status: the respond's status input, wired from
    /// the call's `status`. A computed `status:` is the same input, and a number written out the
    /// param, as before.
    #[test]
    fn a_respond_takes_a_computed_status_on_its_status_input() {
        let into = |d: &FlowDocument, node: &str, port: &str| {
            d.edges.iter().find(|w| w.target == node && w.target_handle.as_deref() == Some(port)).map(|w| w.source.clone())
        };
        let streamed = compiled("port 8200\nroute POST /ask (req) {\n  sent = http.post(\"a.com\", \"/x\", body: req.json, stream: true)\n  respond sent.body, stream: true\n}\n").doc;
        let respond = streamed.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("status")), ("true", "200"));
        let field = streamed.nodes.iter().find(|n| Some(&n.id) == into(&streamed, &respond.id, "status").as_ref()).unwrap();
        let call = streamed.nodes.iter().find(|n| n.data.name == "http_stream").unwrap();
        assert_eq!((field.data.name.as_str(), field.param("name")), ("field", "status"));
        assert_eq!(into(&streamed, &field.id, "record"), Some(call.id.clone()));
        let computed = compiled("port 8200\nroute POST /a (req) {\n  respond { ok: false }, status: req.json.code, missing: 400\n}\n").doc;
        let respond = computed.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("missing")), ("true", "400"));
        assert!(into(&computed, &respond.id, "status").is_some());
        let written = compiled("port 8200\nroute POST /a (req) {\n  respond { ok: true }, status: 201\n}\n").doc;
        let respond = written.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("status")), ("false", "201"));
    }
}
