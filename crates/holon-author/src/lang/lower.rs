//! Lowering: the syntax tree of ast.rs onto the flow builder (docs/LANGUAGE.md §5). Every
//! expression becomes nodes from the live catalog and wires between them; a binding is a named
//! wire; a function is a composite frozen with its declared boundary; a route is a `request` node
//! and everything wired to it. The imported names and statements for the outside world are sugar
//! over one leaf each (§5.4).
//!
//! What comes out is a flow document, the same one the canvas saves, so `flow_unit` and
//! `derive_server` apply unchanged and a text's hash is its graph's. Every error carries the span
//! of the expression at fault.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use holon_core::{interface_of, CALLS, PASSES_EXTRA, TRIGGER, VARIADIC};
use serde_json::json;

use super::ast::*;
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

/// Lower a parsed text.
pub fn lower(file: &File) -> R<Lowered> {
    Lowerer::new(file).run(file)
}

/// Words the lowering keeps for itself: a TypeScript binding of one is renamed, `name_`.
pub(crate) const RESERVED: &[&str] = &[
    "flow", "port", "every", "browsers", "optional", "collection", "folder", "database", "channel", "holon", "js", "route", "resumes", "respond", "delete", "await", "if", "then", "else",
    "when", "match", "fn", "true", "false", "secret", "state", "now", "random", "http", "stripe", "json", "parse_json",
];

/// The noun the tree calls another flow's actions by (`import { charge } from "flow:billing-v2"`
/// is `billing_v2.charge(x)` there): its slug with `-` as `_`.
pub(crate) fn call_noun(flow: &str) -> String {
    flow.replace('-', "_")
}

/// Plain names for two leaves, read as English.
pub(crate) fn alias(name: &str) -> &str {
    match name {
        "json" => "json_stringify",
        "parse_json" => "json_parse",
        n => n,
    }
}

/// A file handler as the log names it: `on file inbox deleted`.
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
pub(crate) fn is_optional(leaf: &str, input: &str) -> bool {
    input == TRIGGER
        || matches!(
            (leaf, input),
            ("http_request" | "http_stream", "headers" | "body")
                | ("http_upload", "headers")
                | ("flow_call" | "flow_start", "input")
                | ("collection_list" | "file_list", "prefix" | "after")
                | ("file_lines", "from")
                | ("to_number", "radix")
                | ("number_format", "digits")
                | ("slice", "end")
                | ("to_spliced", "count" | "items")
                | ("fill" | "copy_within", "start" | "end")
        )
}

/// Why a route can't wait: it answers and is done (docs/L4s/typescript-syntax-L4.md §9).
pub(crate) const ROUTE_WAITS: &str = "a route answers and is done, so it doesn't wait: start an action that waits, const started = payment(req.json), and answer with its link, started.waiting.paid";

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
    /// The flow's own frame: its constants.
    Flow,
    /// A route's block: its request node, and where it responded.
    Route { request: String, responded: Option<Span> },
    /// An `every` block: its tick node, whose `now` is the time of the tick.
    Every { tick: String },
    /// A file handler.
    OnFile,
    /// An action's block: its `action` node, and where it responded.
    Action { node: String, responded: Option<Span> },
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
    /// How deep in a choice's branches the expression being lowered is (`branch`).
    branches: usize,
    /// The calls placed in a branch: made only when the branch needs them, so left untied.
    in_branches: HashSet<String>,
}

impl Frame {
    fn new(kind: FrameKind) -> Frame {
        Frame { kind, env: HashMap::new(), wiring_in: vec![], wiring_out: vec![], written: HashMap::new(), shared: HashMap::new(), created: vec![], branches: 0, in_branches: HashSet::new() }
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
    /// The channels the flow imports (`import updates from "channel:updates"`): sent on, and
    /// subscribed to as a route's answer.
    channels: HashSet<String>,
    /// The flows whose actions it imports (`import { charge } from "flow:billing"`), by the noun
    /// that calls them (`billing`, or `billing_v2` for `billing-v2`): their slugs.
    calls: HashMap<String, String>,
    /// The flow's own name (`flow("approval")`), and its actions: what `payment(x)` starts.
    flow: Option<String>,
    actions: HashSet<String>,
    /// The secrets the flow imports (`import key from "secret:signing_key"`), optional or not.
    secrets: HashSet<String>,
    /// The flow's state keys (`state.beats`): the only ones a read or write may name.
    state: HashSet<String>,
    /// The streamed call (`http_stream`) the current route's `respond(…, { stream: true })` forwards.
    streamed: Option<String>,
    /// The channel the http call being lowered pipes to (`updates.pipe(api.post(…))`).
    piping: Option<String>,
    /// The file handlers' `on_file` nodes: each a handle, as a `file_handle` is.
    arrivals: HashSet<String>,
    /// Top-level `const`s of literals: lowered afresh in each handler.
    consts: HashMap<String, &'a Expr>,
    frames: Vec<Frame>,
    names: BTreeMap<String, String>,
    spans: BTreeMap<String, Span>,
    lambdas: usize,
    /// Each route's request node, the nodes its handler placed, and its head's span: a route is
    /// everything wired to its request, so a node left unwired to it would silently fall out.
    routes: Vec<(String, Vec<String>, Span)>,
    /// Each clock, file and action handler, likewise.
    ticks: Vec<Block>,
}

impl<'a> Lowerer<'a> {
    fn new(file: &'a File) -> Lowerer<'a> {
        let defs = file.items.iter().filter_map(|i| if let Item::Holon(h) = i { Some((h.name.text.clone(), h)) } else { None }).collect();
        let js_defs = file.items.iter().filter_map(|i| if let Item::Js(j) = i { Some((j.name.text.clone(), j)) } else { None }).collect();
        let collections = file.headers.iter().filter_map(|h| if let HeaderKind::Collection(c) = &h.kind { Some(c.clone()) } else { None }).collect();
        let databases = file.headers.iter().filter_map(|h| if let HeaderKind::Database(d) = &h.kind { Some(d.clone()) } else { None }).collect();
        let channels = file.headers.iter().filter_map(|h| if let HeaderKind::Channel(c) = &h.kind { Some(c.clone()) } else { None }).collect();
        let calls = file.headers.iter().filter_map(|h| if let HeaderKind::Calls(f) = &h.kind { Some((call_noun(f), f.clone())) } else { None }).collect();
        let secrets = file
            .headers
            .iter()
            .filter_map(|h| match &h.kind {
                HeaderKind::Secret(n) | HeaderKind::Optional(Consent::Secret(n)) => Some(n.clone()),
                _ => None,
            })
            .collect();
        let state = file.headers.iter().filter_map(|h| if let HeaderKind::State(k) = &h.kind { Some(k.clone()) } else { None }).collect();
        let flow = file.headers.iter().find_map(|h| if let HeaderKind::Flow(n) = &h.kind { Some(n.clone()) } else { None });
        let actions = file.items.iter().filter_map(|i| if let Item::Action(a) = i { Some(a.name.text.clone()) } else { None }).collect();
        let folders = file
            .headers
            .iter()
            .filter_map(|h| match &h.kind {
                HeaderKind::Folder(f) => Some(f.clone()),
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
            flow,
            actions,
            secrets,
            state,
            streamed: None,
            piping: None,
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
                return err(name.span, format!("there is a function \"{}\" already", name.text));
            }
        }
        let actions: Vec<&ActionDef> = file.items.iter().filter_map(|i| if let Item::Action(a) = i { Some(a) } else { None }).collect();
        for (i, a) in actions.iter().enumerate() {
            if actions[..i].iter().any(|o| o.name.text == a.name.text) {
                return err(a.head, format!("the action {} is written twice: one handler per name", a.name.text));
            }
        }
        let blocks: Vec<&EveryDef> = file.items.iter().filter_map(|i| if let Item::Every(e) = i { Some(e) } else { None }).collect();
        let file_blocks: Vec<&OnFileDef> = file.items.iter().filter_map(|i| if let Item::OnFile(o) = i { Some(o) } else { None }).collect();
        for (i, b) in blocks.iter().enumerate() {
            if blocks[..i].iter().any(|o| o.seconds == b.seconds) {
                return err(b.head, format!("{} is written twice: one handler per period", b.text));
            }
        }
        for (i, b) in file_blocks.iter().enumerate() {
            if let Some(g) = &b.on.matching
                && !holon_core::is_glob(g)
            {
                return err(b.head, format!("{g:?} isn't a pattern of names: `*` and `?` within a folder, `**/` for any folders, `/` between them"));
            }
            if file_blocks[..i].iter().any(|o| o.on == b.on) {
                return err(b.head, format!("{} is written twice: one handler per folder, event and pattern", file_label(&b.on)));
            }
        }
        self.frames.push(Frame::new(FrameKind::Flow));
        // The top level's constants first, each lowered afresh where it is used, then the handlers.
        for i in &file.items {
            if let Item::Stmt(s) = i {
                match &s.kind {
                    StmtKind::Bind { name, value } if literal_param(value).is_some() => {
                        self.check_bindable(name)?;
                        self.consts.insert(name.text.clone(), value);
                    }
                    _ => return err(s.span, "a statement goes in a handler; the top level holds functions and consts of literals"),
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
                return err(span, format!("this isn't wired to {what}, so it isn't part of the handler (a handler is everything wired to its trigger): write something with it, or use {use_} in it"));
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
                HeaderKind::Limit { name, value } => {
                    if limits.insert(name.clone(), (*value).into()).is_some() {
                        return err(h.span, format!("one limit line says {name}: this is a second"));
                    }
                }
                HeaderKind::Port(p) => s.port = p.clone(),
                HeaderKind::Browsers(o) => s.browsers = Some(o.join(" ")).filter(|b| !b.is_empty()),
                HeaderKind::Optional(Consent::Secret(n)) => optional.push(format!("SecretRead({n:?})")),
                HeaderKind::Flow(_) | HeaderKind::Secret(_) | HeaderKind::State(_) | HeaderKind::Collection(_) | HeaderKind::Folder(_) | HeaderKind::Database(_) | HeaderKind::Channel(_) => {}
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

    /// Whether a flow whose actions are imported can be a name: a flow's name (letters, digits, `-` and `_`, as a
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
            return err(span, format!("\"{noun}\" is already a name here (a keyword, a leaf, a function or something imported): a flow called by it can't be named so"));
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
        if self.cur().branches > 0 && CALLS.contains(&leaf) {
            self.frame().in_branches.insert(id.clone());
        }
        Ok(self.out(&id))
    }

    /// `e` where a choice runs it only when it needs it: a branch of `if`, `when` or `match`, or
    /// what `??` falls back to. A call made there is made only then, so it isn't tied to its
    /// block; one made anywhere else in a statement is made when the run reaches the statement.
    fn branch(&mut self, e: &'a Expr) -> R<Val> {
        self.frame().branches += 1;
        let v = self.expr(e);
        self.frame().branches -= 1;
        v
    }

    fn out(&self, id: &str) -> Val {
        let outputs = self.b.data(id).map(|d| d.outputs.clone()).unwrap_or_default();
        Val::Out { id: id.into(), outputs, port: 0, explicit: false }
    }

    /// `v` read whole. A function placed with several value outputs (one returning `{ a, b }`,
    /// which lowers to one output per key) is the record of them, as JavaScript reads what such a
    /// function gives; its effects stay outputs of their own. One output, one picked by `.name`,
    /// or a template's first (a route's request) is itself.
    fn whole(&mut self, v: Val, span: Span) -> R<Val> {
        let Val::Out { id, outputs, explicit: false, .. } = &v else { return Ok(v) };
        let placed = self.b.data(id).is_some_and(|d| matches!(d.body, crate::canvas::PlacedBody::Composite { .. }));
        let values: Vec<usize> = (0..outputs.len()).filter(|&k| outputs[k].ty != WType::Effect).collect();
        if !placed || values.len() < 2 {
            return Ok(v);
        }
        let (id, outputs) = (id.clone(), outputs.clone());
        let mut acc = self.node("record", &[], span)?;
        for k in values {
            let value = Val::Out { id: id.clone(), outputs: outputs.clone(), port: k, explicit: true };
            acc = self.node_with("with_field", &[("name".into(), outputs[k].name.clone())], &[("record", acc), ("value", value)], span)?;
        }
        Ok(acc)
    }

    /// Wire `v` into `to`'s `input`; a holon's or function's input is recorded for its boundary.
    ///
    /// A boundary input feeds exactly one inner port (core promotes one edge per input), so an
    /// input used twice passes through one node the uses share: `guard(true, input)`, the
    /// identity on presence. It is placed on the second use, and the first use is moved onto it.
    fn connect(&mut self, v: &Val, to: &str, input: &str, span: Span) -> R<()> {
        if let Val::Out { explicit: false, outputs, .. } = v
            && outputs.len() > 1
        {
            let whole = self.whole(v.clone(), span)?;
            if !matches!((&whole, v), (Val::Out { id: a, .. }, Val::Out { id: b, .. }) if a == b) {
                return self.connect(&whole, to, input, span);
            }
        }
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
            return err(name.span, format!("\"{n}\" is a function; a binding may not shadow it"));
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

    /// Whether node `id` takes a trigger (a sink, or a holon with one) that nothing feeds yet,
    /// and is no call made in a branch (`branch`).
    fn untied(&self, id: &str) -> bool {
        self.b.data(id).is_some_and(|d| d.inputs.iter().any(|p| p.name == TRIGGER))
            && !self.b.is_wired(id, TRIGGER)
            && !self.cur().wiring_in.iter().any(|(_, node, input)| node == id && input == TRIGGER)
            && !self.cur().in_branches.contains(id)
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
                    let v = self.whole(v, value.span)?;
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
                self.check_state(&key.text, key.span)?;
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
                // A call (a request, another flow's action) is made for what it does, whatever it
                // answers.
                let a_call = matches!(&v, Val::Out { id, .. } if self.b.data(id).is_some_and(|d| CALLS.contains(&d.name.as_str())));
                if v.effectful() || a_call {
                    return self.effect(v, s.span);
                }
                err(s.span, "a value nobody uses: bind it, or remove it")
            }
        }
    }

    /// The collection or folder `name` is, or why it is neither.
    fn check_collection(&self, name: &Name) -> R<&'static Store> {
        match self.store(&name.text) {
            Some(s) => Ok(s),
            None if self.channels.contains(&name.text) => {
                err(name.span, format!("\"{0}\" is a channel: send on it with {0}.send(…); a route answers with it, return {0}.subscribe()", name.text))
            }
            None => err(name.span, format!("\"{0}\" isn't a collection this flow imports: import {0} from \"collection:{0}\"", name.text)),
        }
    }

    /// Whether `key` is one of the flow's state keys, or the mistake: a key is a name.
    fn check_state(&self, key: &str, span: Span) -> R<()> {
        let name = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !name {
            return err(span, format!("{key:?} isn't a state key: a key is a name (letters, digits and _), as in state.set(last_post, …)"));
        }
        if !self.state.contains(key) {
            return err(span, format!("\"{key}\" isn't one of this flow's state keys"));
        }
        Ok(())
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
                return err(body.map_or(span, |b| b.span), "only a streamed call's body can be streamed: return respond(sent.body, { stream: true }), with const sent = api.post(…, { stream: true })");
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

    /// An action's answer: the value alone. An action is run through Holon, not
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

    /// Whether this block's only effects are calls: it makes one, and writes nothing.
    fn calls_alone(&self) -> bool {
        let f = self.cur();
        let made = |is: &dyn Fn(&crate::canvas::NodeData) -> bool| f.created.iter().any(|id| self.b.data(id).is_some_and(is));
        made(&|d| CALLS.contains(&d.name.as_str())) && !made(&|d| d.outputs.iter().any(|p| p.ty == WType::Effect))
    }

    /// The answer of a route or action whose only effects are calls: 204 with no body, as one
    /// that writes is answered. A call's answer is the far side's, not the block's to give, so
    /// `route_spec` never takes it for the block's answer.
    fn answer_nothing(&mut self, from: &str, port: &str, span: Span) -> R<()> {
        let node = self.node("respond", &[("status".to_string(), "204".to_string()), ("missing".to_string(), "404".to_string())], span)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.b.wire(from, port, &id, "request").or_else(|e| err(span, e))?;
        match &mut self.frame().kind {
            FrameKind::Route { responded, .. } | FrameKind::Action { responded, .. } => *responded = Some(span),
            _ => {}
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
        if let Some(log) = &r.log {
            params.push(("log".into(), log.text.clone()));
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
        if let FrameKind::Route { request, responded: None } = &self.cur().kind
            && self.calls_alone()
        {
            let request = request.clone();
            self.answer_nothing(&request, "request", r.method.span.to(r.path.span))?;
        }
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
            return err(at, "a streamed call is forwarded to the caller: answer with it, return respond(sent.body, { stream: true })");
        }
        // No respond and no effect: route_spec would refuse it too; say so here, at the route.
        let FrameKind::Route { responded, request } = frame.kind else { unreachable!() };
        self.routes.push((request, frame.created, r.method.span.to(r.path.span)));
        let has_effect = r.body.iter().any(|s| !matches!(s.kind, StmtKind::Bind { .. } | StmtKind::Respond(_)));
        if responded.is_none() && !has_effect {
            return err(r.method.span.to(r.path.span), "the route answers nothing: return something (or write something, answered 204)");
        }
        Ok(())
    }

    /// `every("2s", () => …)`: a clock handler. Its tick node is its trigger, `now` in it the tick's time,
    /// and its sinks are tied to it (docs/L4s/triggers-L4.md §2, §3).
    fn every(&mut self, e: &'a EveryDef) -> R<()> {
        let seconds = if e.seconds.fract() == 0.0 && e.seconds >= 1.0 { number_text(e.seconds) } else { String::new() };
        crate::templates::period(&seconds).or_else(|_| err(e.head, format!("every {}: a clock runs every whole number of seconds, from 1s to 366d", e.text)))?;
        let mut params = vec![("seconds".to_string(), seconds)];
        params.extend(e.log.iter().map(|w| ("log".to_string(), w.text.clone())));
        let tick = self.node("every", &params, e.head)?;
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
            return err(e.head, "the handler does nothing: write something in it (state, a collection, a channel)");
        }
        self.ticks.push((id, frame.created, e.head, ("the tick", "now".into())));
        Ok(())
    }

    /// `on.file.created(inbox, (f) => …)`: a file handler. Its `on_file` node is its trigger, `f` in it the
    /// file's handle, and its sinks are tied to it (docs/L4s/triggers-L4.md §2, §3).
    fn on_file(&mut self, o: &'a OnFileDef) -> R<()> {
        let mut params = vec![("folder".to_string(), o.on.folder.clone())];
        if o.on.deleted {
            params.push(("on".into(), "deleted".into()));
        }
        if let Some(m) = &o.on.matching {
            params.push(("matching".into(), m.clone()));
        }
        params.extend(o.log.iter().map(|w| ("log".to_string(), w.text.clone())));
        let file = self.node("on_file", &params, o.head)?;
        let Val::Out { id, .. } = &file else { unreachable!() };
        let id = id.clone();
        // A handle: written to a folder, it is copied; sent, it goes by handle.
        self.arrivals.insert(id.clone());
        self.names.insert(id.clone(), o.file.text.clone());
        self.check_name_free(&o.file)?;
        let mut frame = Frame::new(FrameKind::OnFile);
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
            return err(o.head, "the handler does nothing: write something in it (a folder, state, a channel)");
        }
        self.ticks.push((id, frame.created, o.head, ("the file", o.file.text.clone())));
        Ok(())
    }

    /// `export const reset = action((input) => …)`: an action, run by hand by its name. Its `action`
    /// node is its trigger, `input` in it what was sent, and its sinks are tied to it
    /// (docs/L4s/triggers-L4.md §2, §3); what it returns answers.
    fn action(&mut self, a: &'a ActionDef) -> R<()> {
        crate::templates::action_name(&[WParam::frozen("name", &a.name.text)]).or_else(|e| err(a.name.span, e))?;
        let mut params = vec![("name".to_string(), a.name.text.clone())];
        params.extend(a.log.iter().map(|w| ("log".to_string(), w.text.clone())));
        let node = self.node("action", &params, a.head)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.names.insert(id.clone(), a.input.text.clone());
        self.check_name_free(&a.input)?;
        let mut frame = Frame::new(FrameKind::Action { node: id.clone(), responded: None });
        frame.created.push(id.clone());
        frame.env.insert(a.input.text.clone(), node.clone());
        self.frames.push(frame);
        for s in &a.body {
            self.stmt(s)?;
        }
        let tied = self.tie_block(&node, a.head)?;
        if let FrameKind::Action { responded: None, .. } = &self.cur().kind
            && self.calls_alone()
        {
            self.answer_nothing(&id, "input", a.head)?;
        }
        let frame = self.frames.pop().expect("the block's frame");
        let FrameKind::Action { responded, .. } = frame.kind else { unreachable!() };
        if responded.is_none() && !tied {
            return err(a.head, "the action does nothing: return something, or write something (state, a collection, a channel)");
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

    /// A function's port may be called after a leaf (`sum`: a name in scope wins over a leaf), but
    /// not after a keyword, a collection or a function.
    fn check_port_name(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if RESERVED.contains(&n) || self.store(n).is_some() || self.databases.contains(n) || self.channels.contains(n) || self.is_def(n) {
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
            return err(at, format!("no function \"{name}\""));
        };
        if !self.in_progress.insert(name.to_string()) {
            return err(at, format!("the function \"{name}\" calls itself: a function can't contain itself"));
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
        // gets last, which a placement ties to the block it is placed in.
        let mut implicit_trigger = false;
        if result.is_ok() {
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
        let holon = {
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
                    None => return err(n.span, format!("\"{}\" has no input \"{}\" (a function takes no params)", name.text, n.text)),
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
        let v = match result {
            Ok(v) => self.whole(v, body.span),
            Err(e) => Err(e),
        };
        let frame = self.frames.pop().expect("the function's frame");
        let v = v?;
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
            ExprKind::Secret(n) if !self.secrets.contains(n) => err(e.span, format!("\"{n}\" isn't a secret this flow imports: import key from \"secret:{n}\"")),
            ExprKind::Secret(n) => self.node("secret", &[("name".into(), n.clone())], e.span),
            ExprKind::State(k) => {
                self.check_state(k, e.span)?;
                self.node("store_read", &[("key".into(), k.clone())], e.span)
            }
            // In an `every` block, `now` is the time of its tick.
            ExprKind::Noun(Noun::Now) if let FrameKind::Every { tick } = &self.cur().kind => Ok(self.out(&tick.clone())),
            ExprKind::Noun(n) => self.node(if *n == Noun::Now { "clock" } else { "random" }, &[], e.span),
            ExprKind::Call { callee, args } => self.call(callee, args, e.span),
            ExprKind::Leaf { leaf, params, args, number } => self.leaf_form(leaf, params, args, *number, e.span),
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
                let t = self.branch(then)?;
                let guarded = self.node_with("guard", &[], &[("cond", c), ("value", t)], e.span)?;
                match els {
                    None => Ok(guarded),
                    Some(b) => self.fallback(guarded, b, e.span),
                }
            }
            ExprKind::Match { subject, arms, els, .. } => self.match_expr(subject, arms, els.as_deref(), e.span),
            ExprKind::Await { name, expires } => {
                if matches!(self.cur().kind, FrameKind::Route { .. }) {
                    return err(e.span, ROUTE_WAITS);
                }
                let mut params = vec![("name".to_string(), name.text.clone())];
                if let Some(x) = expires {
                    params.push(("expires".into(), self.literal_text(x)?));
                }
                self.node("await", &params, e.span)
            }
            ExprKind::Lambda { .. } => err(e.span, "an arrow function goes in map or reduce: xs.map((x) => …)"),
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
            return err(span, format!("\"{n}\" is a function: call it, {n}(…)"));
        }
        if let Some(s) = self.store(n) {
            return err(span, format!("\"{n}\" is a {}: read it as {n}[{}] or {n}.list(…)", s.kind, s.key));
        }
        if self.databases.contains(n) {
            return err(span, format!("\"{n}\" is a database: read it as {n}.query(\"select …\") or {n}.one(…)"));
        }
        if self.channels.contains(n) {
            return err(span, format!("\"{n}\" is a channel: send on it with {n}.send(…); a route answers with it, return {n}.subscribe()"));
        }
        if self.calls.contains_key(n) {
            return err(span, format!("\"{n}\" is a flow: import the action you call, import {{ charge }} from \"flow:{n}\""));
        }
        if interface_of(alias(n)).is_some() || templates::is_template(n) {
            return err(span, format!("\"{n}\" is a leaf: call it, {n}(…)"));
        }
        err(span, format!("unknown name \"{n}\""))
    }

    /// Whether `v` is a file's handle: one read here (`file_handle`), or a file handler's file (its
    /// `on_file` node).
    fn is_handle(&self, v: &Val) -> bool {
        matches!(v, Val::Out { id, .. } if self.arrivals.contains(id) || self.b.data(id).is_some_and(|d| d.name == "file_handle" || d.name == "file_arrival"))
    }

    fn binary(&mut self, op: BinOp, lhs: &'a Expr, rhs: &'a Expr, span: Span) -> R<Val> {
        match op {
            BinOp::When => {
                let v = self.branch(lhs)?;
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
                return err(p.span, format!("{} is a case twice; the second could never be chosen", key.text()));
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
            let v = self.branch(&arm.value)?;
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
            let v = self.branch(e)?;
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
        let v = self.branch(b)?;
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
        if name == "start" {
            return self.start(args, span);
        }
        // A token resumes the run that hands it out where it waits, and a route never waits.
        if name == "resume_token" && matches!(self.cur().kind, FrameKind::Route { .. }) {
            return err(callee.span, ROUTE_WAITS);
        }
        if templates::is_template(name) {
            return match name {
                "request" => err(callee.span, "a route's handler takes its request: route.get(\"/path\", (req) => …)"),
                "respond" => err(callee.span, "respond is a route's answer: return respond(body, { headers })"),
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

    /// A leaf the reader placed for a JavaScript form, with its params. `toString()` is a text's
    /// and a list's method as well as a number's, so it is `number_format` only on a value known
    /// to be a number: one from a node whose output is a `Number`.
    fn leaf_form(&mut self, leaf: &Name, params: &[(String, String)], args: &'a [Arg], number: bool, span: Span) -> R<Val> {
        let unit = interface_of(&leaf.text).expect("a genesis leaf");
        let inputs: Vec<WPort> = unit.inputs.iter().map(WPort::from_core).collect();
        if number && let [subject] = args {
            let v = self.expr(&subject.value)?;
            let known = matches!(&v, Val::Out { outputs, port, .. } if outputs.get(*port).is_some_and(|p| p.ty == WType::Number));
            if !known {
                let given = params.iter().map(|(k, v)| format!("{k}: \"{v}\"")).collect::<Vec<_>>().join(", ");
                return err(span, format!(".toString() is {0}(x, {{ {given} }}) on a number, and this value isn't known to be one: import {0} from \"holon\" and call it so, or write `${{x}}` for any value", leaf.text));
            }
            return self.node_with(&leaf.text, params, &[(inputs[0].name.as_str(), v)], span);
        }
        let defaults: Vec<WParam> = unit.params.iter().map(WParam::from_core).collect();
        self.unit_call(&leaf.text, &inputs, &defaults, params, args, span)
    }

    /// `payment(x)` without `await`, the tree's `start(payment, x)`: start this flow's action
    /// `payment` with `x`, or another flow's (`start(billing.charge, x)`), and don't wait for it to
    /// end (docs/L4s/typescript-syntax-L4.md §9). It answers once the run finishes or first waits:
    /// `{answer, run}`, or `{run, waiting}` with each point it waits at and the token that resumes
    /// it there.
    fn start(&mut self, args: &'a [Arg], span: Span) -> R<Val> {
        let shape = "an action is started with what to send it: const started = payment(req.json), or charge(x) imported from another flow";
        let (Some(first), rest) = (args.first(), args.get(1..).unwrap_or_default()) else { return err(span, shape) };
        if let Some(named) = args.iter().find_map(|a| a.name.as_ref()) {
            return err(named.span, shape);
        }
        if rest.len() > 1 {
            return err(rest[1].value.span, format!("an action takes one input: {shape}"));
        }
        let (flow, action) = match &first.value.kind {
            ExprKind::Name(a) if self.actions.contains(a) => match &self.flow {
                Some(flow) => (flow.clone(), a.clone()),
                None => return err(first.value.span, "a flow that starts its own action says its name first: flow(\"approval\")"),
            },
            ExprKind::Field { base, name } => match &base.kind {
                ExprKind::Name(c) if self.calls.contains_key(c) => (self.calls[c].clone(), name.text.clone()),
                _ => return err(first.value.span, shape),
            },
            ExprKind::Name(a) => return err(first.value.span, format!("\"{a}\" isn't an action of this flow: export const {a} = action((input) => …), or import {{ {a} }} from \"flow:…\" for another flow's")),
            _ => return err(first.value.span, shape),
        };
        if action.len() > 64 {
            return err(first.value.span, "an action's name is at most 64 characters");
        }
        let u = interface_of("flow_start").expect("a genesis leaf");
        let (inputs, params) = (u.inputs.iter().map(WPort::from_core).collect::<Vec<_>>(), u.params.iter().map(WParam::from_core).collect::<Vec<_>>());
        self.unit_call("flow_start", &inputs, &params, &[("flow".to_string(), flow), ("action".to_string(), action)], rest, span)
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
                    return err(at.span, format!("a piped call sends a body and goes to its channel as it arrives: {c}.pipe(api.{}(path, {{ body }})), without {}", method.text, at.text));
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
                self.check_state(&key, first.value.span)?;
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
                "subscribe" => err(span, format!("a subscription is a route's answer: return {c}.subscribe()")),
                // `updates.pipe(api.post(…))`: the call's answer, to the channel as it arrives.
                "pipe" => {
                    let call = match args {
                        [only] if only.name.is_none() => &only.value,
                        _ => return err(span, format!("{c}.pipe takes one call, whose answer it sends as it arrives: {c}.pipe(api.post(…))")),
                    };
                    if !matches!(&call.kind, ExprKind::Method { receiver, .. } if receiver.text == "http") {
                        return err(call.span, format!("only a call's answer can be piped: {c}.pipe(api.post(…))"));
                    }
                    self.piping = Some(c.to_string());
                    let piped = self.expr(call);
                    self.piping = None;
                    piped
                }
                other => err(method.span, format!("a channel has no \"{other}\": send(…), pipe(api.post(…)), or return {c}.subscribe() in a route")),
            },
            // `billing.charge(x)`: run the action `charge` of the flow `billing`, sending x.
            c if self.calls.contains_key(c) => {
                if method.text.len() > 64 {
                    return err(method.span, "an action's name is at most 64 characters");
                }
                if args.len() > 1 {
                    return err(args[1].value.span, format!("an action takes one input: {}(x), with x a record to send more", method.text));
                }
                let flow = self.calls[c].clone();
                let (inputs, params) = leaf("flow_call");
                self.unit_call("flow_call", &inputs, &params, &[("flow".to_string(), flow), ("action".to_string(), method.text.clone())], args, span)
            }
            c if ["get", "put", "delete", "list"].contains(&method.text.as_str()) => {
                err(receiver.span, format!("\"{c}\" isn't a collection this flow imports: import {c} from \"collection:{c}\""))
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
                        // A higher-order leaf's `fn` is a function; a family's (`math`) is a name.
                        let value = if n.text == "fn" && PASSES_EXTRA.contains(&leaf) {
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
                let item = match leaf {
                    "fold" => "the result so far and the item",
                    "sort_by" => "the two items it compares",
                    _ => "the item",
                };
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
                if h.outputs.iter().filter(|p| p.ty != WType::Effect).count() < 2 {
                    return Ok((h.hash, h.inputs.len(), vec![]));
                }
                // `map` and `fold` read a function's one answer: one returning `{ a, b }` is
                // placed in a function of the same inputs that answers the record of them.
                self.lambdas += 1;
                let name = format!("fn_{}", self.lambdas);
                self.b.begin_holon(&name);
                self.frames.push(Frame::new(FrameKind::Lambda { captures: vec![] }));
                let id = self.b.place(&h, None);
                let placed = (|| {
                    for p in &h.inputs {
                        self.connect(&Val::In(p.name.clone()), &id, &p.name, e.span)?;
                    }
                    let out = self.out(&id);
                    self.whole(out, e.span)
                })();
                let frame = self.frames.pop().expect("the function's frame");
                let Val::Out { id: rec, outputs, port, .. } = placed? else { unreachable!("a record") };
                let output = outputs.get(port).map(|o| o.name.clone()).unwrap_or_else(|| "out".into());
                let inputs: Vec<HolonPort> = h.inputs.iter().map(|p| HolonPort { name: p.name.clone(), unit: None }).collect();
                let wrapped = self.b.end_holon_exact(&name, &inputs, &[HolonPort { name: "out".into(), unit: None }], &frame.wiring_in, &[(rec, output, "out".into())]).or_else(|err_| err(e.span, err_))?;
                Ok((wrapped.hash, inputs.len(), vec![]))
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
    use crate::lang::ts;
    use crate::{samples, text, to_hex};
    use holon_core::{parse_hash, MapResolver, Resolver, UnitBody};

    /// The unit a document deploys as, through the real core.
    fn hash_of(doc: &FlowDocument) -> String {
        let mut store = MapResolver::new();
        text::restore(doc, &mut store, &Names::new());
        let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
        to_hex(&flow_unit(&doc.nodes, &doc.edges, &mut store, port).unwrap_or_else(|e| panic!("{e}")))
    }

    /// Every sample's text (samples/*.holon.ts): the natural form, as fmt writes it.
    pub(crate) fn sample_texts() -> Vec<(&'static str, &'static str)> {
        samples::samples().iter().map(|s| (s.name, s.text)).collect()
    }

    /// A flow in TypeScript's syntax, read and lowered.
    fn compiled(src: &str) -> Lowered {
        ts::compile(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    /// Where a text's mistake is said, and what it says.
    fn error(src: &str) -> (u32, u32, String) {
        let e = ts::compile(src).expect_err(src);
        (e.span.start.line, e.span.start.col, e.message)
    }

    /// The leaves placed, by name, in order.
    fn leaves(doc: &FlowDocument) -> Vec<String> {
        doc.nodes.iter().map(|n| n.data.name.clone()).collect()
    }

    /// A handler is one composite. An input a leaf works without (an http_request's headers and
    /// body, a listing's prefix and after) stays unbound in it, as in a route: promoted to the
    /// boundary it would be wired but absent, and core would hold the leaf back on every run (a
    /// clock that never called out, or never listed its collection).
    #[test]
    fn a_clock_leaves_optional_inputs_unbound() {
        let got = compiled(
            r#"import { every, flow, length } from "holon"
import state from "state"
import items from "collection:items"
import h from "host:h.example"

flow("poll")

every("10s", async () => {
  const page = await h.get("/")
  const recent = items.list({ limit: 5 })
  state.last = { status: page.status, n: length(Object.keys(recent)) }
})
"#,
        );
        let mut store = MapResolver::new();
        let server = derive_server(&got.doc.nodes, &got.doc.edges, &mut store, "");
        assert_eq!(server.errors, Vec::<String>::new());
        let unit = store.resolve(&parse_hash(&server.clocks[0].hash).unwrap()).expect("the clock's unit is stored");
        let inputs: Vec<&str> = unit.inputs.iter().map(|p| p.name.as_str()).collect();
        assert!(inputs.is_empty(), "headers, body, prefix and after stay unbound, not {inputs:?}");
    }

    /// A family leaf's parameter outside its values is refused where it is written, in core's
    /// words; one of its values is placed.
    #[test]
    fn a_family_parameter_outside_its_values_is_refused_where_it_is_written() {
        let src = |op: &str| format!("import {{ action, compare, flow }} from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((a) => {{\n  return compare(a, 1, {{ op: \"{op}\" }})\n}})\n");
        assert_eq!(error(&src("~")), (6, 10, "compare's op is one of =, !=, <, <=, >, >=: \"~\" isn't one".into()));
        let d = compiled(&src("<=")).doc;
        let compare = d.nodes.iter().find(|n| n.data.name == "compare").expect("a compare");
        assert_eq!(compare.param("op"), "<=");
    }

    /// An action returning `body`, its input `x` and a number `n` bound first.
    fn number_flow(body: &str) -> String {
        format!("import {{ action, flow }} from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((x) => {{\n  let n = parseFloat(x.n)\n{body}\n}})\n")
    }

    /// JavaScript's number forms read as their leaves, each with its params: the operators, the
    /// unary ones, `Math`, `Number`, the globals and a number's methods.
    #[test]
    fn javascript_s_number_forms_read_as_their_leaves() {
        let placed = |expr: &str, leaf: &str| {
            let d = compiled(&number_flow(&format!("  return {expr}"))).doc;
            let node = d.nodes.iter().rev().find(|n| n.data.name == leaf).unwrap_or_else(|| panic!("{expr}: no {leaf} in {:?}", leaves(&d)));
            ["fn", "op", "test", "coerce", "grammar", "style"].iter().filter(|k| interface_of(leaf).is_some_and(|u| u.params.iter().any(|p| p.name() == **k))).map(|k| format!("{k}={}", node.param(k))).collect::<Vec<_>>().join(",")
        };
        let cases: &[(&str, &str, &str)] = &[
            ("n % 3", "remainder", ""),
            ("n ** 2", "power", ""),
            ("Math.pow(n, 2)", "power", ""),
            ("n & 3", "bitwise", "op=and"),
            ("n | 3", "bitwise", "op=or"),
            ("n ^ 3", "bitwise", "op=xor"),
            ("n << 3", "bitwise", "op=shl"),
            ("n >> 3", "bitwise", "op=shr"),
            ("n >>> 3", "bitwise", "op=ushr"),
            ("~n", "bitwise", "op=xor"),
            ("Math.imul(n, 3)", "bitwise", "op=imul"),
            ("Math.round(n)", "math", "fn=round"),
            ("Math.f16round(n)", "math", "fn=f16round"),
            ("Math.max(n, 1, 2)", "max", ""),
            ("Math.min(n, 1)", "min", ""),
            ("Math.max(...x.xs)", "aggregate", "fn=max"),
            ("Math.hypot(n, 1)", "aggregate", "fn=hypot"),
            ("Math.sumPrecise(x.xs)", "aggregate", "fn=sum_precise"),
            ("Math.atan2(n, 1)", "atan2", ""),
            ("Number.isFinite(n)", "number_test", "test=finite,coerce=none"),
            ("Number.isInteger(n)", "number_test", "test=integer,coerce=none"),
            ("Number.isSafeInteger(n)", "number_test", "test=safe_integer,coerce=none"),
            ("isNaN(x.s)", "number_test", "test=nan,coerce=number"),
            ("isFinite(x.s)", "number_test", "test=finite,coerce=number"),
            ("+x.s", "to_number", "grammar=number"),
            ("Number(x.s)", "to_number", "grammar=number"),
            ("parseFloat(x.s)", "to_number", "grammar=float"),
            ("Number.parseInt(x.s, 16)", "to_number", "grammar=int"),
            ("n.toFixed(2)", "number_format", "style=fixed"),
            ("n.toPrecision(3)", "number_format", "style=precision"),
            ("n.toExponential()", "number_format", "style=exponential"),
            ("n.toString(16)", "number_format", "style=string"),
            ("n.toString()", "number_format", "style=string"),
            ("x.s.toString(2)", "number_format", "style=string"),
        ];
        for (expr, leaf, params) in cases {
            assert_eq!(placed(expr, leaf), *params, "{expr}");
        }
        // `-x` is `x * -1`; the constants are their literals; `Number.isNaN` is false.
        for (expr, value) in [("Math.PI", "3.141592653589793"), ("Number.MAX_SAFE_INTEGER", "9007199254740991"), ("Number.MIN_VALUE", "5e-324"), ("Number.isNaN(n)", "false")] {
            let d = compiled(&number_flow(&format!("  return {{ v: {expr}, n }}"))).doc;
            assert!(d.nodes.iter().any(|c| c.data.name == "const" && c.param("value") == value), "{expr}: {:?}", leaves(&d));
        }
    }

    /// Each form lowers to the unit its spelled-out form does: `-x` to `x * -1`, `~x` to
    /// `x ^ -1`, a compound assignment to its operator.
    #[test]
    fn a_number_form_is_the_unit_of_what_it_stands_for() {
        let same = [
            ("return -n", "return n * -1"),
            ("return ~n", "return n ^ -1"),
            ("return +x.s", "return Number(x.s)"),
            ("return Math.pow(n, 2)", "return n ** 2"),
            ("return Number.parseFloat(x.s)", "return parseFloat(x.s)"),
            ("return -(n * 2) * 3", "return ((n * 2) * -1) * 3"),
            ("n %= 3\n  return n", "n = n % 3\n  return n"),
            ("n **= 2\n  return n", "n = n ** 2\n  return n"),
            ("n &= 3\n  n |= 4\n  n ^= 5\n  return n", "n = n & 3\n  n = n | 4\n  n = n ^ 5\n  return n"),
            ("n <<= 1\n  n >>= 2\n  n >>>= 3\n  return n", "n = n << 1\n  n = n >> 2\n  n = n >>> 3\n  return n"),
        ];
        for (form, spelled) in same {
            let (a, b) = (compiled(&number_flow(&format!("  {form}"))), compiled(&number_flow(&format!("  {spelled}"))));
            assert_eq!(hash_of(&a.doc), hash_of(&b.doc), "{form} is {spelled}");
        }
        // `-(n * 2) * 3` keeps its negation one node, as written.
        let d = compiled(&number_flow("  return -(n * 2) * 3")).doc;
        assert_eq!(leaves(&d).iter().filter(|l| *l == "multiply").count(), 3);
    }

    /// The forms a flow can't have are refused, each naming what it can write.
    #[test]
    fn the_number_forms_a_flow_lacks_are_refused() {
        let refused = |body: &str| error(&number_flow(&format!("  return {body}"))).2;
        assert!(refused("x.s.toString()").starts_with(".toString() is number_format(x, { style: \"string\" }) on a number"), "{}", refused("x.s.toString()"));
        assert!(refused("Math.random()").contains("import { random } from \"holon:random\""));
        assert!(refused("Math.max(n)").starts_with("Math.max takes two numbers or more"));
        assert!(refused("Math.sqrt(n, 2)").starts_with("Math.sqrt takes one number"));
        assert!(refused("Math.clamp(n)").starts_with("Math has no \"clamp\""));
        assert!(refused("Math.TAU").starts_with("Math.TAU isn't a value: Math's constants are Math.E"));
        assert!(refused("Number.NaN").starts_with("a flow has no NaN or Infinity"));
        assert!(refused("NaN").starts_with("NaN isn't a value a flow has"));
        assert!(refused("Infinity").starts_with("Infinity isn't a value a flow has"));
        assert!(refused("Math").starts_with("Math is a namespace"));
        assert!(refused("parseInt(x.s, 16, 2)").starts_with("parseInt takes a text and a radix"));
        assert!(refused("n.toLocaleString()").starts_with("a flow has no locale"));
    }

    /// A flow's own function keeps its name's calls, as JavaScript's would shadow the global;
    /// `Math.max` is the leaf whatever the flow defines.
    #[test]
    fn a_flow_s_function_shadows_a_global_and_never_math_s() {
        let src = "import { action, flow } from \"holon\"\n\nfunction max(a) {\n  return a * 2\n}\n\nfunction parseFloat(s) {\n  return s ?? 1\n}\n\nflow(\"f\")\n\nexport const t = action((x) => {\n  return { m: Math.max(x.a, 1), own: max(x.a), p: parseFloat(x.s) }\n})\n";
        let d = compiled(src).doc;
        let placed = |name: &str, composite: bool| d.nodes.iter().filter(|n| n.data.name == name && matches!(n.data.body, crate::canvas::PlacedBody::Composite { .. }) == composite).count();
        assert_eq!((placed("max", false), placed("max", true)), (1, 1), "Math.max is the leaf, max(…) the flow's function: {:?}", leaves(&d));
        assert_eq!((placed("to_number", false), placed("parseFloat", true)), (0, 1), "parseFloat is the flow's own: {:?}", leaves(&d));
    }

    /// `Math`'s and `Number`'s constants are the doubles JavaScript has.
    #[test]
    fn the_constants_are_javascript_s_doubles() {
        use std::f64::consts;
        let math = [consts::E, consts::LN_10, consts::LN_2, consts::LOG10_E, consts::LOG2_E, consts::PI, consts::FRAC_1_SQRT_2, consts::SQRT_2];
        for ((name, text), want) in ts::MATH_CONSTANTS.iter().zip(math) {
            assert_eq!(text.parse::<f64>().unwrap().to_bits(), want.to_bits(), "Math.{name}");
        }
        let number = [f64::EPSILON, 9007199254740991.0, -9007199254740991.0, f64::MAX, f64::from_bits(1)];
        for ((name, text), want) in ts::NUMBER_CONSTANTS.iter().zip(number) {
            assert_eq!(text.parse::<f64>().unwrap().to_bits(), want.to_bits(), "Number.{name}");
        }
    }

    // ── each construct, by the nodes it places ──

    #[test]
    fn literals_and_interpolation() {
        let d = compiled(
            r#"import { action, flow } from "holon"

flow("t")

export const t = action(() => {
  const x = 1
  const z = "t"
  const w = true
  const s = `Hello, ${z}!`
  return [x, z, w, s]
})
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["action", "const", "const", "const", "const", "as_text", "const", "concat", "list", "respond"]);
        assert_eq!(d.nodes[3].param("type"), "Boolean");
        let from = |i: usize| -> Vec<usize> { d.edges.iter().filter(|w| w.target == d.nodes[i].id).map(|w| d.nodes.iter().position(|n| n.id == w.source).unwrap()).collect() };
        assert_eq!((from(5), from(7)), (vec![2], vec![4, 5, 6]), "z into as_text, and the three pieces into concat");
        // One part alone is just as_text; text alone is a const.
        let one = compiled("import { action, flow } from \"holon\"\n\nflow(\"t\")\n\nexport const t = action(() => {\n  const x = 1\n  return `${x}`\n})\n").doc;
        assert_eq!(leaves(&one), ["action", "const", "as_text", "respond"]);
        let plain = compiled("import { action, flow } from \"holon\"\n\nflow(\"t\")\n\nexport const t = action(() => `plain`)\n").doc;
        assert_eq!(leaves(&plain), ["action", "const", "respond"]);
        // A duration, where one is asked for, is its seconds.
        let waits = compiled(
            r#"import { action, event, flow } from "holon"

flow("t")

export const t = action(async () => {
  const paid = await event("paid", { expires: "5m" })
  return paid
})
"#,
        )
        .doc;
        assert_eq!((waits.nodes[1].data.name.as_str(), waits.nodes[1].param("expires")), ("await", "300"));
    }

    #[test]
    fn records_lists_and_fields() {
        let d = compiled(
            r#"import { action, flow } from "holon"

flow("t")

export const t = action(() => {
  const r = { a: 1, "b c": 2 }
  const q = { ...r, d: 3 }
  const f = q.d
  const l = [1, 2, 3]
  const e = []
  const o = [1]
  return [f, l, e, o]
})
"#,
        )
        .doc;
        assert_eq!(
            leaves(&d),
            ["action", "record", "const", "with_field", "const", "with_field", "const", "with_field", "field", "const", "const", "const", "list", "const", "const", "const", "append", "list", "respond"]
        );
        assert_eq!(d.nodes[5].param("name"), "b c");
        assert_eq!(d.nodes[12].data.inputs.len(), 3);
        assert_eq!(d.nodes[13].param("type"), "Json");
    }

    #[test]
    fn operators_lower_to_their_leaves_and_chains_flatten_unless_parenthesised() {
        let d = compiled(
            r#"import { action, flow } from "holon"

flow("t")

export const ops = action(() => {
  const a = 1
  const b = 2
  const c = 3
  const s = a + b + c
  const t = (a + b) + c
  const u = a - b
  const v = a < b
  const w = a == b
  const x = a != b
  const y = !w
  const z = a && b || c
  return [s, t, u, v, x, y, z]
})
"#,
        )
        .doc;
        let names = leaves(&d);
        assert_eq!(&names[4..15], ["add", "add", "add", "subtract", "compare", "equals", "equals", "not", "not", "and", "or"]);
        assert_eq!(d.nodes[4].data.inputs.len(), 3, "a + b + c is one add with three inputs");
        assert_eq!(d.nodes[6].data.inputs.len(), 2, "(a + b) + c is two");
        assert_eq!(d.nodes[8].param("op"), "<");
    }

    #[test]
    fn fallbacks_guards_and_if() {
        let d = compiled(
            r#"import { action, flow } from "holon"

flow("t")

export const t = action(() => {
  const a = 1
  const b = 2
  const c = true
  const x = a ?? 0
  const y = a ?? b
  const z = c ? a : undefined
  const v = c ? a : b
  const u = c ? a : "none"
  const j = a ?? { k: [1, "x"] }
  return [x, y, z, v, u, j]
})
"#,
        )
        .doc;
        let names = leaves(&d);
        assert_eq!(&names[4..12], ["default", "coalesce", "guard", "guard", "coalesce", "guard", "default", "default"]);
        assert_eq!((d.nodes[4].param("type"), d.nodes[4].param("value")), ("Number", "0"));
        assert_eq!((d.nodes[10].param("type"), d.nodes[10].param("value")), ("Text", "none"));
        assert_eq!((d.nodes[11].param("type"), d.nodes[11].param("value")), ("Json", "{\"k\":[1,\"x\"]}"));
    }

    #[test]
    fn calls_with_named_inputs_params_and_variadics() {
        let d = compiled(
            r#"import { action, concat, flow, hmac_sha256 } from "holon"

flow("t")

export const t = action(() => {
  const a = "x"
  const b = "y"
  const c = concat(a, b, a)
  const h = hmac_sha256(a, b, { encoding: "base64" })
  return [c, h]
})
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["action", "const", "const", "concat", "hmac_sha256", "list", "respond"]);
        assert_eq!(d.nodes[3].data.inputs.len(), 3);
        assert_eq!(d.nodes[4].param("encoding"), "base64");
        // Named inputs, optional ones left out, a zero-input leaf's positional params.
        let d = compiled(
            r#"import { action, flow, resume_token } from "holon"
import notes from "collection:notes"
import h from "host:h.example"

flow("t")

export const calls = action(async () => {
  const n = notes.list({ after: "a", limit: 5 })
  const t = resume_token("paid")
  const q = await h.get("/p")
  return [n, t, q]
})
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["action", "const", "collection_list", "resume_token", "const", "http_request", "list", "respond"]);
        assert_eq!(d.nodes[2].param("limit"), "5");
        assert_eq!(d.nodes[3].param("await"), "paid");
        assert_eq!((d.nodes[5].param("method"), d.nodes[5].param("host")), ("GET", "h.example"));
        let wired = |i: usize| -> Vec<&str> { d.edges.iter().filter(|w| w.target == d.nodes[i].id).map(|w| w.input()).collect() };
        assert_eq!((wired(2), wired(5)), (vec!["after"], vec!["path", TRIGGER]), "the optional inputs are left out");
    }

    #[test]
    fn nouns_and_writes_are_one_leaf_each() {
        let d = compiled(
            r#"import { action, flow } from "holon"
import tok from "secret:tok"
import state from "state"
import notes from "collection:notes"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("t")

export const t = action(() => {
  const k = "key"
  const x = tok
  const y = state.beats
  const z = now
  const r = random
  const g = notes[k]
  state.saved = y
  notes[k] = x
  delete notes[k]
  const p = notes.put(k, x)
  state.other = x
  return [z, r, g]
})
"#,
        )
        .doc;
        assert_eq!(
            leaves(&d),
            ["action", "const", "secret", "store_read", "clock", "random", "collection_get", "store_write", "collection_put", "collection_delete", "collection_put", "store_write", "list", "respond"]
        );
        assert_eq!(d.nodes[2].param("name"), "tok");
        assert_eq!(d.nodes[7].param("key"), "saved");
        assert_eq!(d.nodes[11].param("key"), "other");
    }

    #[test]
    fn a_multi_output_value_selects_an_output_by_name_else_a_field() {
        let d = compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.get("/x", (req) => {
  const a = req.params
  const b = req.params.id
  const c = req.path
  const d = req.request.body
  return d
})
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["request", "field", "field", "field", "respond"]);
        let fields: Vec<&str> = d.nodes[1..4].iter().map(|n| n.param("name")).collect();
        assert_eq!(fields, ["id", "path", "body"]);
        let from_params = d.edges.iter().filter(|w| w.output() == "params").count();
        assert_eq!(from_params, 1);
    }

    /// Each route's answer, its body as text, for a GET of its path: the flow compiled from its
    /// text and its endpoints run through core's `eval`.
    pub(crate) fn bodies(src: &str) -> Vec<(String, String)> {
        use holon_core::{eval, Value};
        use std::collections::BTreeMap;
        let lowered = ts::compile(src).unwrap_or_else(|e| panic!("{e}"));
        let mut store = MapResolver::new();
        text::restore(&lowered.doc, &mut store, &Names::new());
        let port = lowered.doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
        let server = derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
        let text = |s: &str| Value::Text(s.into());
        server
            .endpoints
            .iter()
            .map(|e| {
                let unit = store.resolve(&parse_hash(&e.hash).unwrap()).unwrap();
                let request = Value::Record(BTreeMap::from([
                    ("method".to_string(), text(&e.method)),
                    ("path".to_string(), text(&e.path)),
                    ("params".to_string(), Value::Record(BTreeMap::new())),
                    ("query".to_string(), Value::Record(BTreeMap::new())),
                    ("headers".to_string(), Value::Record(BTreeMap::new())),
                    ("body".to_string(), text("")),
                ]));
                let out = eval(unit, &[Some(request)], &store).into_vec();
                // The answer's body, as the server writes it: a text as itself, a value as JSON.
                let body = match out.first() {
                    Some(Some(Value::Record(r))) if !r.contains_key("error") => match r.get("body") {
                        Some(Value::Text(t)) => t.clone(),
                        Some(v) => holon_core::stringify_json(v).unwrap_or_default(),
                        None => String::new(),
                    },
                    other => format!("{other:?}"),
                };
                (e.path.clone(), body)
            })
            .collect()
    }

    /// A function returning `{ a, b }` lowers to one output per key; read whole, it is the
    /// record, as JavaScript reads it: answered, passed on, mapped and folded. Picked by key, it
    /// is that output.
    #[test]
    fn a_function_returning_a_record_is_the_record_when_read_whole() {
        let src = r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

function pair(p) {
  return { a: `<${p}>`, b: 1 }
}

function step(acc, x) {
  return { n: acc.n + 1, sum: acc.sum + x }
}

route.get("/whole", (req) => pair("w"))
route.get("/passed", (req) => JSON.stringify(pair("p")))
route.get("/mapped", (req) => ["x", "y"].map(pair))
route.get("/folded", (req) => [1, 2, 3].reduce(step, { n: 0, sum: 0 }))
route.get("/picked", (req) => `${pair("k").a}!`)
"#;
        let got: BTreeMap<String, String> = bodies(src).into_iter().collect();
        assert_eq!(got["/whole"], r#"{"a":"<w>","b":1}"#);
        assert_eq!(got["/passed"], r#"{"a":"<p>","b":1}"#);
        assert_eq!(got["/mapped"], r#"[{"a":"<x>","b":1},{"a":"<y>","b":1}]"#);
        assert_eq!(got["/folded"], r#"{"n":3,"sum":6}"#);
        assert_eq!(got["/picked"], "<k>!");
    }

    #[test]
    fn holons_lambdas_and_their_errors() {
        let d = compiled(
            r#"import { action, flow } from "holon"

flow("h")

function twice(n) {
  const o = n + n
  return o
}

function total(xs) {
  const amount = xs.reduce((acc, p) => acc + p, 0)
  return amount
}

export const t = action(() => {
  const x = twice(1)
  const y = [1, 2].map(twice)
  return [x, y]
})
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["action", "const", "twice", "const", "const", "list", "map", "list", "respond"]);
        // The document's units are the closure: the composites and the leaves inside them.
        let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let composites = d
            .units
            .as_ref()
            .unwrap()
            .iter()
            .filter(|h| matches!(holon_core::decode_preimage(&unhex(h)).unwrap().body, holon_core::UnitBody::Composite { .. }))
            .count();
        assert_eq!(composites, 3, "twice, the reduce's function, total");
        // `n + n` uses the input twice: one add with its two inputs fed by one shared pass-through.
        let twice = &d.nodes[2];
        assert_eq!(twice.data.inputs.len(), 1);
        assert_eq!(d.nodes[6].param("fn").len(), 64);
        // A port may be named after a leaf or `out`.
        let named = compiled(
            r#"import { action, flow, text_length } from "holon"

flow("h")

function h(text) {
  return { out: text.toUpperCase(), sum: text_length(text) }
}

export const t = action(() => h("a"))
"#,
        )
        .doc;
        let h = named.nodes.iter().find(|n| n.data.name == "h").unwrap();
        let ports = |ps: &[crate::wire::WPort]| ps.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!((ports(&h.data.inputs), ports(&h.data.outputs)), (vec!["text".to_string()], vec!["out".to_string(), "sum".to_string()]));
        assert_eq!(
            error(
                r#"import { action, flow } from "holon"

flow("h")

function h(a) {
  return a
}

export const t = action(() => h(1))
"#
            ),
            (6, 10, "an output can't be an input passed straight through: put a node between".into())
        );
        assert_eq!(
            error(
                r#"import { action, flow } from "holon"

flow("h")

function h(a, b) {
  return a + 1
}

export const t = action(() => h(1, 2))
"#
            ),
            (5, 15, "the input \"b\" is never used".into())
        );
        // A function's body sees what its function sees, and nothing beyond it.
        assert_eq!(
            error(
                r#"import { flow, route } from "holon"

flow("h", { port: 8000 })

route.get("/a", (req) => h([1]))

function h(a) {
  return a.map((p) => p + req)
}
"#
            ),
            (8, 27, "unknown name \"req\"".into())
        );
        assert_eq!(
            error(
                r#"import { action, flow } from "holon"

flow("h")

function h(a) {
  return h(a)
}

export const t = action(() => h(1))
"#
            ),
            (6, 10, "the function \"h\" calls itself: a function can't contain itself".into())
        );
        assert_eq!(
            error(
                r#"import { action, flow } from "holon"

flow("h")

function h(a) {
  return a + 1
}

export const t = action(() => h(1, 2))
"#
            ),
            (9, 36, "\"h\" takes 1 inputs".into())
        );
    }

    /// A name a function uses from around it is an input of the function after its parameters,
    /// and `map` (or `reduce`) passes it after the item, wired from where the name is bound.
    #[test]
    fn a_function_takes_what_it_uses_from_around_it_as_inputs() {
        let d = compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.post("/v", (req) => {
  const r = req.json
  return Object.keys(r).map((k) => r[k])
})
"#,
        )
        .doc;
        let map = d.nodes.iter().find(|n| n.data.name == "map").unwrap();
        assert_eq!(map.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["list", "in2"]);
        assert_eq!(map.data.inputs[1].ty, WType::Any, "an extra input is whatever the function takes");
        let into = |node: &str, port: &str| d.edges.iter().find(|w| w.target == node && w.target_handle.as_deref() == Some(port)).map(|w| (w.source.clone(), w.source_handle.clone()));
        let keys = d.nodes.iter().find(|n| n.data.name == "keys").unwrap();
        assert!(into(&map.id, "in2").is_some());
        assert_eq!(into(&map.id, "in2"), into(&keys.id, "record"), "r, from where it is bound");
        // The function: (k, r) -> r[k], a unit of its own, the same however often it is written.
        let same = compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.post("/v", (req) => {
  const r = req.json
  const a = Object.keys(r).map((k) => r[k])
  const b = Object.keys(r).map((k) => r[k])
  return [a, b]
})
"#,
        )
        .doc;
        let fns: Vec<&str> = same.nodes.iter().filter(|n| n.data.name == "map").map(|n| n.param("fn")).collect();
        assert_eq!(fns[0], fns[1]);
        assert_eq!(fns[0], map.param("fn"));
        // reduce passes them after the result so far and the item; a function's input is
        // captured too, and a top-level constant is lowered inside.
        compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

const rate = 3

function h(xs, k) {
  return xs.reduce((acc, p) => acc + p * k * rate, 0)
}

route.get("/a", () => h([1, 2], 2))
"#,
        );
        // Nested: the inner function's capture passes through the outer one.
        compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.post("/v", (req) => {
  const n = req.json.n
  return req.json.rows.map((row) => row.map((x) => x * n))
})
"#,
        );
    }

    /// A function given the wrong number of inputs is said at the function, counting what the
    /// call gives it.
    #[test]
    fn a_function_of_the_wrong_arity_is_said() {
        // One written in place: the reader says it, as map passes no index.
        assert_eq!(
            error("import { action, flow } from \"holon\"\n\nflow(\"h\")\n\nexport const t = action(() => [1].map((a, b) => a + b))\n"),
            (5, 39, "map's function takes the item: (x) => …; an index isn't passed".into())
        );
        let by_name = |call: &str| {
            error(&format!("import {{ action, flow }} from \"holon\"\n\nflow(\"h\")\n\nfunction h(a, b, c) {{\n  return a + b + c\n}}\n\nexport const t = action(() => {call})\n"))
        };
        assert_eq!(by_name("[1].map(h)"), (9, 39, "\"map\" gives its function 1 inputs (the item); this one takes 3".into()));
        assert_eq!(by_name("[1].reduce(h, 0)"), (9, 42, "\"fold\" gives its function 2 inputs (the result so far and the item); this one takes 3".into()));
    }

    #[test]
    fn a_holons_effect_statements_leave_as_outputs() {
        let d = compiled(
            r#"import { action, flow } from "holon"
import state from "state"
import posts from "collection:posts"

flow("h")

function keep(k, v) {
  posts[k] = v
  state.last = k
  const receipt = "ok"
  return receipt
}

export const t = action(() => keep("a", 1))
"#,
        )
        .doc;
        let keep = d.nodes.iter().find(|n| n.data.name == "keep").unwrap();
        let outs: Vec<&str> = keep.data.outputs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(outs, ["receipt", "put", "write"]);
    }

    /// A route's sinks are tied to its request; `if (x !== undefined)` ties a statement's to x
    /// instead; a function's to its own trigger, an input it gets last, which the block placing
    /// it ties.
    #[test]
    fn sinks_are_tied_to_their_block_or_to_what_after_names() {
        let src = r#"import { flow, route } from "holon"
import state from "state"
import c from "collection:c"

flow("f", { port: 8000 })

function keep(v) {
  state.seen = v
  return "ok"
}

route.post("/v/:id", (req) => {
  const id = req.params.id
  const put = c.put(id, 1)
  if (put !== undefined) state.last = id
  keep(id)
  return id
})
"#;
        let d = compiled(src).doc;
        let node = |id: &str| d.nodes.iter().find(|n| n.id == id).unwrap();
        let tie = |leaf: &str| {
            let n = d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
            let w = d.edges.iter().find(|w| w.target == n.id && w.input() == TRIGGER).unwrap_or_else(|| panic!("{leaf} is tied"));
            (node(&w.source).data.name.clone(), w.output().to_string())
        };
        assert_eq!(tie("collection_put"), ("request".into(), "request".into()));
        assert_eq!(tie("store_write"), ("collection_put".into(), "out".into()), "after put");
        assert_eq!(tie("keep"), ("request".into(), "request".into()), "the placed function");
        let keep = d.nodes.iter().find(|n| n.data.name == "keep").unwrap();
        assert_eq!(keep.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["v", TRIGGER]);
        // One that declares its trigger uses it; a function with no sink takes none.
        let d = compiled(
            r#"import { action, flow } from "holon"
import state from "state"

flow("f")

function keep(v, trigger) {
  state.last = v
  return "ok"
}

function twice(n) {
  return n * 2
}

export const t = action(() => {
  keep(twice(1))
})
"#,
        )
        .doc;
        let ins = |name: &str| d.nodes.iter().find(|n| n.data.name == name).unwrap().data.inputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(ins("keep"), ["v", TRIGGER]);
        assert_eq!(ins("twice"), ["n"]);
        // A tie on a statement with nothing to tie is said.
        let e = error(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.get("/a", (req) => {
  let n
  if (req !== undefined) n = 1
  return n
})
"#,
        );
        assert_eq!((e.0, e.1), (7, 7));
        assert!(e.2.contains("this statement makes none"), "{}", e.2);
    }

    /// `import { charge } from "flow:billing"` names a flow whose actions this one runs:
    /// `await charge(x)` is a `flow_call{flow, action}` tied to the block's trigger as any sink
    /// is, which asks for `RunsAction("billing/charge")` (docs/L4s/triggers-L4.md §9, slice 6).
    #[test]
    fn calls_run_another_flows_action_tied_to_the_block() {
        let d = compiled(
            r#"import { flow, route } from "holon"
import { charge } from "flow:billing-v2"

flow("f", { port: 8000 })

route.post("/p", async (req) => {
  const r = await charge({ amount: req.json.total })
  return r.answer
})
"#,
        )
        .doc;
        let call = d.nodes.iter().find(|n| n.data.name == "flow_call").expect("the call");
        assert_eq!((call.param("flow"), call.param("action")), ("billing-v2", "charge"));
        let request = d.nodes.iter().find(|n| n.is_template("request")).unwrap();
        let tie = d.edges.iter().find(|w| w.target == call.id && w.input() == TRIGGER).map(|w| w.source.clone());
        assert_eq!(tie, Some(request.id.clone()), "tied to the request, though it reads none of it");
        // Nothing sent is `{}`; `if (x !== undefined)` ties a call to a value, and a lone call is
        // a statement.
        let d = compiled(
            r#"import { flow, route, status } from "holon"
import c from "collection:c"
import { reset } from "flow:billing"

flow("f", { port: 8000 })

route.post("/p", async (req) => {
  const found = c[req.body]
  if (found !== undefined) await reset()
  return status(204)
})
"#,
        )
        .doc;
        let call = d.nodes.iter().find(|n| n.data.name == "flow_call").unwrap();
        assert!(!d.edges.iter().any(|w| w.target == call.id && w.input() == "input"), "no input wired");
        let found = d.nodes.iter().find(|n| n.data.name == "collection_get").unwrap();
        assert!(d.edges.iter().any(|w| w.target == call.id && w.input() == TRIGGER && w.source == found.id), "after the lookup");
        // A flow whose slug is already a name, an action used bare, sent two values.
        assert_eq!(
            error("import { flow, route } from \"holon\"\nimport { charge } from \"flow:route\"\n\nflow(\"f\", { port: 8000 })\n\nroute.post(\"/a\", async (req) => await charge(req.json))\n"),
            (2, 1, "\"route\" is already a name here (a keyword, a leaf, a function or something imported): a flow called by it can't be named so".into())
        );
        let called = |answer: &str| error(&format!("import {{ flow, route }} from \"holon\"\nimport {{ go }} from \"flow:a\"\n\nflow(\"f\", {{ port: 8000 }})\n\nroute.get(\"/a\", async () => {answer})\n"));
        assert_eq!(called("go"), (6, 29, "an action is called: go(input) runs go".into()));
        assert_eq!(called("await go(1, 2)"), (6, 29, "an action is sent one value: go({ … }), a record for more".into()));
        // `calls` is a name like any.
        compiled("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8000 })\n\nroute.get(\"/a\", (req) => {\n  const calls = 1\n  return calls + req.query.n\n})\n");
    }

    /// A block whose only work is calls (a request, another flow's action) is a handler: a call is
    /// made for what it does, whatever it answers, read or not, bound or a statement. A route or
    /// action of calls alone answers 204, as one that writes does: never with a call's answer.
    #[test]
    fn a_block_of_calls_alone_is_a_handler() {
        let mut store = MapResolver::new();
        let mut server = |src: &str| {
            let d = compiled(src).doc;
            let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
            assert_eq!(server.errors, Vec::<String>::new(), "{src}");
            let answers: Vec<String> = d.nodes.iter().filter(|n| n.is_template("respond")).map(|n| n.param("status").to_string()).collect();
            (server, answers)
        };
        let (s, _) = server(
            r#"import { every, flow } from "holon"
import { reset } from "flow:billing"

flow("f")

every("1h", async () => {
  await reset()
})
"#,
        );
        assert_eq!(s.clocks.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["every 1h"]);
        let (s, answers) = server(
            r#"import { action, flow } from "holon"
import { charge } from "flow:billing"

flow("f")

export const run = action(async (input) => {
  const r = await charge(input)
  return r
})
"#,
        );
        assert_eq!((s.actions.len(), answers), (1, vec!["200".to_string()]));
        // A heartbeat, a call as a statement or bound and read by nothing.
        let (s, _) = server(
            r#"import { every, flow } from "holon"
import hooks from "host:hooks.example.com"

flow("f")

every("1h", async () => {
  await hooks.post("/beat", { body: "alive" })
})
"#,
        );
        assert_eq!(s.clocks.len(), 1);
        let (s, _) = server(
            r#"import { every, flow } from "holon"
import hooks from "host:hooks.example.com"

flow("f")

every("1h", async () => {
  const sent = await hooks.post("/beat", { body: "alive" })
})
"#,
        );
        assert_eq!(s.clocks.len(), 1);
        // A route or an action of calls alone answers 204; one that also writes answers by it.
        let (s, answers) = server(
            r#"import { flow, route } from "holon"
import hooks from "host:hooks.example.com"

flow("f", { port: 8000 })

route.post("/hook", async (req) => {
  await hooks.post("/a", { body: req.body })
})
"#,
        );
        assert_eq!((s.endpoints.len(), answers), (1, vec!["204".to_string()]));
        let (s, answers) = server(
            r#"import { action, flow } from "holon"
import hooks from "host:hooks.example.com"
import { reset } from "flow:billing"

flow("f")

export const notify = action(async (input) => {
  const sent = await hooks.post("/a", { body: input })
  await reset()
})
"#,
        );
        assert_eq!((s.actions.len(), answers), (1, vec!["204".to_string()]));
        let (s, answers) = server(
            r#"import { flow, route } from "holon"
import c from "collection:c"
import hooks from "host:hooks.example.com"

flow("f", { port: 8000 })

route.post("/hook", async (req) => {
  await hooks.post("/a", { body: req.body })
  c["last"] = req.body
})
"#,
        );
        assert_eq!((s.endpoints.len(), answers), (1, vec![]));
        // The same request twice is two requests, each kept apart by where it goes.
        let (s, _) = server(
            r#"import { flow, route } from "holon"
import hooks from "host:hooks.example.com"

flow("f", { port: 8000 })

route.post("/hook", async (req) => {
  const a = await hooks.post("/a", { body: req.body })
  const b = await hooks.post("/a", { body: req.body })
  return "ok"
})
"#,
        );
        assert_eq!(s.endpoints.len(), 1);
    }

    /// A route answers and is done: what waits is an action it starts, `pay(x)` without `await`,
    /// which answers once the run finishes or first waits (docs/L4s/typescript-syntax-L4.md §9).
    /// A route that waits, or hands out a token of its own, is refused, saying what to write
    /// instead.
    #[test]
    fn a_route_starts_what_waits() {
        let refused = |body: &str| {
            let src = r#"import { event, flow, resume_token, route } from "holon"

flow("f", { port: 8080 })

route.post("/a", async (req) => {
BODY
})

route.get("/paid/:t", () => "ok", { resumes: "paid" })
"#;
            ts::compile(&src.replace("BODY", body)).unwrap_err().message
        };
        for body in ["  const paid = await event(\"paid\", { expires: \"1h\" })\n  return paid.method", "  return `/paid/${resume_token(\"paid\")}`"] {
            assert!(refused(body).starts_with("a route answers and is done, so it doesn't wait"), "{body}");
        }
        // In an action it waits, and a route starts it: this flow's own, or another flow's.
        let own = compiled(
            r#"import { action, event, flow, route } from "holon"

flow("f", { port: 8080 })

route.post("/a", (req) => {
  const started = pay(req.json)
  return started.waiting.paid
})

export const pay = action(async (order) => {
  const paid = await event("paid", { expires: "1h" })
  return paid.method
})
"#,
        )
        .doc;
        let start = own.nodes.iter().find(|n| n.data.name == "flow_start").expect("a start");
        assert_eq!((start.param("flow"), start.param("action")), ("f", "pay"));
        let other = compiled(
            r#"import { flow, route, status } from "holon"
import { charge } from "flow:billing"

flow("f", { port: 8080 })

route.post("/a", (req) => {
  charge()
  return status(204)
})
"#,
        )
        .doc;
        let start = other.nodes.iter().find(|n| n.data.name == "flow_start").expect("a start");
        assert_eq!((start.param("flow"), start.param("action")), ("billing", "charge"));
        // What isn't an action, or a start given more than its one value, is refused.
        let error = |src: &str| ts::compile(src).unwrap_err().message;
        assert!(error("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nroute.post(\"/a\", (req) => nope(1))\n").starts_with("unknown function \"nope\""));
        assert!(
            error("import { action, flow, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nroute.post(\"/a\", (req) => pay(1, { now: true }))\n\nexport const pay = action((x) => x)\n")
                .starts_with("an action is sent one value")
        );
    }

    /// A call is made when the run reaches its statement, under its tie, whatever reads its
    /// answer: tied to its block, the handler keeps it as an output, so a branch not taken doesn't
    /// keep it from being made. One written in a branch (of `?:`, `switch`, or what `??` falls
    /// back to) is made only when the branch needs it: it is left untied, and not kept.
    #[test]
    fn a_call_is_made_where_its_statement_is() {
        let kept = |handler: &str| {
            let src = format!("import {{ flow, route }} from \"holon\"\nimport a from \"host:a.example\"\n\nflow(\"f\", {{ port: 8080 }})\n\n{handler}");
            let d = compiled(&src).doc;
            let mut store = MapResolver::new();
            let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "8080");
            assert_eq!(server.errors, Vec::<String>::new(), "{src}");
            let endpoint = store.resolve(&parse_hash(&server.endpoints[0].hash).unwrap()).unwrap().clone();
            let UnitBody::Composite { children, .. } = &endpoint.body else { panic!("an endpoint wraps its handler") };
            let handler = store.resolve(&children[0]).unwrap();
            let call = d.nodes.iter().find(|n| n.data.name == "http_request").unwrap();
            let tied = d.edges.iter().any(|w| w.target == call.id && w.input() == TRIGGER);
            (tied, handler.outputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
        };
        // The respond's answer first, then the call's.
        let answer_and_call = vec!["response".to_string(), "out".to_string()];
        let reached = kept("route.get(\"/a\", async (req) => {\n  const page = await a.get(\"/x\")\n  return req.json.go ? page.body : undefined\n})\n");
        assert_eq!(reached, (true, answer_and_call.clone()));
        let guarded = kept("route.get(\"/a\", async (req) => {\n  let page\n  if ((req.json.go ? true : undefined) !== undefined) page = await a.get(\"/x\")\n  return page.body ?? \"no\"\n})\n");
        assert_eq!(guarded, (true, answer_and_call));
        for in_branch in [
            "route.get(\"/a\", async (req) => (req.json.go ? (await a.get(\"/x\")).body : undefined))\n",
            "route.get(\"/a\", async (req) => (req.json.go ? (await a.get(\"/x\")).body : \"no\"))\n",
            "route.get(\"/a\", async (req) => req.json.cached ?? (await a.get(\"/x\")).body)\n",
            "route.get(\"/a\", async (req) => {\n  switch (req.json.op) {\n    case \"go\":\n      return (await a.get(\"/x\")).body\n    default:\n      return \"no\"\n  }\n})\n",
        ] {
            assert_eq!(kept(in_branch), (false, vec!["response".to_string()]), "{in_branch}");
        }
    }

    /// `every("2s", () => { … })` is a clock handler: its tick node, everything wired to it, its
    /// sinks tied to the tick, and `now` in it the tick's time (docs/L4s/triggers-L4.md §2, §3).
    #[test]
    fn every_blocks_are_clock_handlers_tied_to_their_tick() {
        let d = compiled(
            r#"import { every, flow, route } from "holon"
import state from "state"
import ticks from "channel:ticks"
import { now } from "holon:clock"

flow("f", { port: 8080 })

every("2s", () => {
  state.n = (state.n ?? 0) + 1
  ticks.send(now)
})

route.get("/n", () => state.n ?? 0)
"#,
        )
        .doc;
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
        let d = compiled("import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nevery(\"1h\", () => {\n  state.n = (state.n ?? 0) + 1\n})\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        let read = holon_core::read_server(unit, &store).unwrap();
        assert_eq!((read.port, read.routes.len(), read.clocks[0].seconds), (None, 0, 3600));

        // A clock answers nobody: the reader says so at its return.
        assert_eq!(error("import { every, flow } from \"holon\"\n\nflow(\"f\")\n\nevery(\"2s\", () => {\n  return 1\n})\n"), (6, 10, "a clock's run answers nobody: return alone ends it".into()));
        assert_eq!(
            error("import { every, flow } from \"holon\"\n\nflow(\"f\")\n\nevery(\"2s\", (now) => {\n  const n = now\n})\n"),
            (5, 1, "the handler does nothing: write something in it (state, a collection, a channel)".into())
        );
        assert_eq!(
            error("import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nevery(\"1.5s\", () => {\n  state.n = 1\n})\n").2,
            "every 1.5s: a clock runs every whole number of seconds, from 1s to 366d"
        );
        assert_eq!(
            error("import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nevery(\"2s\", (now) => {\n  state.a = now\n})\n\nevery(\"2s\", (now) => {\n  state.b = now\n})\n"),
            (10, 1, "2s is written twice: one handler per period".into())
        );
        // A write that reads nothing of the tick is still the block's: tied to it.
        let d = compiled("import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nevery(\"2s\", () => {\n  state.n = 1\n})\n").doc;
        assert!(d.edges.iter().any(|w| w.source.starts_with("every") && w.input() == TRIGGER));
    }

    /// `export const reset = action((input) => { … })` is an action: its `action` node, everything
    /// wired to it, its sinks tied to what was sent, what it returns its answer
    /// (docs/L4s/triggers-L4.md §2, §3).
    #[test]
    fn action_blocks_are_actions_tied_to_what_was_sent() {
        let src = r#"import { action, flow, route } from "holon"
import state from "state"

flow("f", { port: 8080 })

export const reset = action((input) => {
  state.ticks = input.to ?? 0
  return { was: state.ticks ?? 0 }
})

export const note = action((n) => {
  state.note = n
})

route.get("/n", () => state.ticks ?? 0)
"#;
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
        let d = compiled("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nexport const reset = action(() => {\n  state.n = 0\n})\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        assert_eq!(holon_core::read_server(unit, &store).unwrap().port, None);

        // An action answers with a value alone: the reader refuses a status.
        assert_eq!(
            error("import { action, flow, status } from \"holon\"\n\nflow(\"f\")\n\nexport const a = action(() => {\n  return status(201, 1)\n})\n").2,
            "an action answers with a value alone: return it, without status or respond"
        );
        assert_eq!(
            error("import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const a = action((input) => {\n  const n = input\n})\n").2,
            "the action does nothing: return something, or write something (state, a collection, a channel)"
        );
        assert_eq!(
            error("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nexport const a = action((x) => {\n  state.a = x\n})\n\nexport const a = action((y) => {\n  state.b = y\n})\n").2,
            "the action a is written twice: one handler per name"
        );
        // Answering twice: the reader refuses what follows the first return.
        assert_eq!(
            error("import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const a = action(() => {\n  return 1\n  return 2\n})\n"),
            (7, 3, "nothing after a return runs: remove it".into())
        );
        // An answer that reads nothing of what was sent is still the action's.
        compiled("import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const ping = action(() => \"pong\")\n");
    }

    /// `on.file.created(drop, (f) => { … })` is a file handler: its `on_file` node, everything
    /// wired to it, its sinks tied to the file, and `f` the file's handle (docs/L4s/triggers-L4.md
    /// §2, §3).
    #[test]
    fn on_file_blocks_are_file_handlers_tied_to_their_file() {
        let src = r#"import { flow, on, route } from "holon"
import state from "state"
import done from "folder:done"
import drops from "channel:drops"
import drop from "folder:drop"

flow("f", { port: 8080 })

on.file.created(drop, (f) => {
  drops.send(f.name)
  done[f.name] = f
})

on.file.deleted(drop, (gone) => {
  state.last = gone.name
}, { matching: "*.csv" })

route.get("/n", () => state.last)
"#;
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
        let d = compiled("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport drop from \"folder:drop\"\n\nflow(\"f\")\n\non.file.created(drop, (f) => {\n  state.n = f.size\n})\n").doc;
        let server = crate::flow::derive_server(&d.nodes, &d.edges, &mut store, "");
        let unit = server.server_hash.as_deref().and_then(holon_core::parse_hash).and_then(|h| store.resolve(&h)).expect("a server");
        assert_eq!(holon_core::read_server(unit, &store).unwrap().port, None);

        // A file's run answers nobody: the reader says so at its return.
        assert_eq!(
            error("import { flow, on } from \"holon\"\nimport a from \"folder:a\"\n\nflow(\"f\")\n\non.file.created(a, (f) => {\n  return 1\n})\n"),
            (7, 10, "a file handler's run answers nobody: return alone ends it".into())
        );
        assert_eq!(
            error("import { flow, on } from \"holon\"\nimport a from \"folder:a\"\n\nflow(\"f\")\n\non.file.created(a, (f) => {\n  const n = f.name\n})\n").2,
            "the handler does nothing: write something in it (a folder, state, a channel)"
        );
        assert_eq!(
            error("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport a from \"folder:a\"\n\nflow(\"f\")\n\non.file.created(a, (f) => {\n  state.a = f\n})\n\non.file.created(a, (g) => {\n  state.b = g\n})\n").2,
            "on file a is written twice: one handler per folder, event and pattern"
        );
        assert!(error("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport a from \"folder:a\"\n\nflow(\"f\")\n\non.file.created(a, (f) => {\n  state.a = f\n}, { matching: \"a**\" })\n").2.contains("isn't a pattern of names"));
    }

    #[test]
    fn routes_respond_and_their_errors() {
        let d = compiled(
            r#"import { flow, route, status } from "holon"

flow("f", { port: 8080 })

route.get("/a", () => status(204))

route.post("/b", (r) => status(201, r.json) ?? status(400), { resumes: "paid" })
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["request", "respond", "request", "respond"]);
        assert_eq!((d.nodes[1].param("status"), d.nodes[1].data.inputs.len()), ("204", 1));
        assert_eq!((d.nodes[3].param("status"), d.nodes[3].param("missing"), d.nodes[2].param("resume")), ("201", "400", "paid"));
        assert_eq!(d.edges.iter().filter(|w| w.input() == "request").count(), 2, "respond is wired to its request");
        // An answer outside a route, or a second one: the reader says so.
        assert_eq!(
            error("import { flow, respond } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nrespond(1)\n"),
            (5, 1, "the top level holds imports, the flow(…) line, handlers, functions and consts of literals: move this into a handler".into())
        );
        let route = |handler: &str| error(&format!("import {{ flow, respond, route, status }} from \"holon\"\n\nflow(\"f\", {{ port: 8080 }})\n\n{handler}\n"));
        assert_eq!(route("route.get(\"/a\", (req) => {\n  return 1\n  return 2\n})"), (7, 3, "nothing after a return runs: remove it".into()));
        assert_eq!(route("route.get(\"/a\", (req) => {\n  const x = 1\n})"), (5, 7, "the route answers nothing: return something (or write something, answered 204)".into()));
        assert_eq!(route("route.get(\"/a\", (req) => respond(1, 2))"), (5, 37, "respond's options are an object: { headers: { … } }".into()));
        assert_eq!(route("route.get(\"/a\", (req) => status(700, 1))"), (5, 33, "a status is a number from 200 to 599".into()));
        assert_eq!(route("route.get(\"/a\", (req) => status(101, 1))"), (5, 33, "a status is a number from 200 to 599".into()), "not an interim one");
        assert_eq!(route("route.get(\"/a\", (req) => status(204, \"x\"))"), (5, 38, "a 204 answers with no body".into()));
        assert_eq!(route("route.get(\"/a\", (req) => {\n  1 + 1\n  return 2\n})"), (6, 3, "a value nobody uses: bind it, or remove it".into()));
        // A write that reads nothing of the request is tied to it by its trigger: a route counts
        // its hits (docs/L4s/triggers-L4.md §3).
        let d = compiled("import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\", { port: 8080 })\n\nroute.get(\"/a\", (req) => {\n  state.hits = (state.hits ?? 0) + 1\n  return 2\n})\n").doc;
        let tie = d.edges.iter().find(|w| w.target.starts_with("store_write") && w.input() == "trigger").expect("the write is tied");
        assert!(tie.source.starts_with("request") && tie.output() == "request", "{tie:?}");
        // A value that never touches the request isn't in the route's group: said, not dropped.
        assert_eq!(
            error("import { flow, length, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nroute.get(\"/a\", (req) => {\n  const n = length([1, 2])\n  return 2\n})\n"),
            (6, 21, "this isn't wired to the route's request, so it isn't part of the route (a route is everything wired to its request): use the request in it, or move it to a flow of its own".into())
        );
        // Shared logic at the top level: the reader keeps it to literals.
        assert_eq!(
            error("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nconst x = 1 + 1\n\nroute.get(\"/a\", () => x)\n"),
            (5, 11, "the top level binds only literals (const limit = 50): put the rest in a handler, or in a function".into())
        );
        // A literal at the top level is placed afresh in each route that uses it.
        let d = compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8080 })

const greeting = "hi"

route.get("/a", () => greeting)

route.get("/b", () => greeting)
"#,
        )
        .doc;
        assert_eq!(leaves(&d), ["request", "const", "respond", "request", "const", "respond"]);
    }

    #[test]
    fn folders_read_write_delete_and_list_files_by_name() {
        let d = compiled(
            r#"import { action, flow } from "holon"
import inbox from "folder:inbox"
import out from "folder:out"

flow("f")

export const t = action(() => {
  const t = inbox["a.csv"]
  const l = inbox.list({ prefix: "2026/" })
  out["b.md"] = t
  delete out["old.md"]
  return l
})
"#,
        )
        .doc;
        let mut got = leaves(&d);
        got.retain(|l| l.starts_with("file_"));
        assert_eq!(got, ["file_read", "file_list", "file_write", "file_delete"]);
        let folder = |leaf: &str| d.nodes.iter().find(|n| n.data.name == leaf).unwrap().param("folder").to_string();
        assert_eq!([folder("file_read"), folder("file_list"), folder("file_write"), folder("file_delete")], ["inbox", "inbox", "out", "out"]);
        let route = |handler: &str| error(&format!("import {{ flow, route }} from \"holon\"\nimport inbox from \"folder:inbox\"\nimport notes from \"collection:notes\"\n\nflow(\"f\", {{ port: 8080 }})\n\n{handler}\n"));
        assert_eq!(route("route.get(\"/a\", () => inbox)"), (7, 23, "\"inbox\" is a folder: read it as inbox[name] or inbox.list(…)".into()));
        assert_eq!(route("route.get(\"/a\", () => {\n  inbox = 1\n  return 1\n})"), (8, 3, "\"inbox\" is imported, and stays what it is: give a value of its own a name of its own".into()));
        assert_eq!(route("route.get(\"/a\", () => inbox.move(\"a\"))"), (7, 29, "a folder has no \"move\": get, put, delete, list, file, lines, slice, copy, create or create_copy".into()));
        assert_eq!(route("route.get(\"/a\", () => notes.file(\"a\"))"), (7, 29, "a collection has no \"file\": get, put, delete or list".into()));
        // A folder without its name.
        let e = error("import { flow, route } from \"holon\"\nimport inbox from \"folder:\"\n\nflow(\"f\", { port: 8080 })\n\nroute.get(\"/a\", () => 1)\n");
        assert_eq!((e.0, e.1), (2, 19));
        assert!(e.2.starts_with("\"folder:\" isn't something a flow imports"), "{}", e.2);
    }

    #[test]
    fn databases_take_their_sql_in_quotes_and_its_values_after_it() {
        let d = compiled(
            r#"import { action, flow } from "holon"
import shop from "database:shop"

flow("f")

export const t = action(() => {
  const rows = shop.query("select id from orders where customer = $1 and total > $2", "ada", 10)
  const first = shop.one("select id from orders")
  shop.exec("update stock set n = n - 1 where sku = $1", rows, { expect: 1 })
  return first
})
"#,
        )
        .doc;
        let node = |leaf: &str| d.nodes.iter().find(|n| n.data.name == leaf).unwrap();
        assert_eq!((node("db_query").param("database"), node("db_exec").param("expect")), ("shop", "1"));
        assert_eq!(node("db_one").param("sql"), "select id from orders");
        let into = |id: &str| d.edges.iter().find(|w| w.target == id && w.target_handle.as_deref() == Some("params")).map(|w| d.nodes.iter().find(|n| n.id == w.source).unwrap().data.name.clone());
        assert_eq!(
            [into(&node("db_query").id), into(&node("db_one").id), into(&node("db_exec").id)],
            [Some("list".into()), Some("const".into()), Some("append".into())],
            "its values wired as a list"
        );
        let error = |body: &str| {
            let src = "import { action, flow } from \"holon\"\nimport shop from \"database:shop\"\n\nflow(\"f\")\n\nexport const t = action(() => {\nBODY\n})\n";
            error(&src.replace("BODY", body)).2
        };
        assert_eq!(error("  return shop.query(\"select id from t where a = $1 and b = $2\", 1)"), "the SQL takes 2 values, for $1 to $2; 1 given");
        assert_eq!(
            error("  const q = \"select a from t\"\n  return shop.query(q)"),
            "the SQL is written in quotes, as it is: its tables are the permission, and values go after it as $1, $2, …, never into it"
        );
        assert!(error("  return shop.query(\"drop table t\")").starts_with("this SQL can't be sent: a read is a select"));
        assert!(error("  shop.exec(\"delete from t; delete from u\")").contains("more than one statement"));
        assert_eq!(error("  return shop.query(\"select 1\")"), "this SQL reads no table: what it selects, the flow can compute itself");
        // A query has no options: what follows its SQL are its values.
        assert_eq!(error("  return shop.query(\"select a from t\", { expect: 1 })"), "the SQL takes no values; 1 given");
        assert_eq!(error("  shop.exec(\"delete from t\", { expect: -1 })"), "expect: is how many rows it must change, a whole number");
        assert_eq!(error("  return shop.pick(\"a\")"), "a database has no \"pick\": query, one or exec (SQL), or find, find_one, insert, update or delete (a document database)");
        assert_eq!(error("  return shop"), "\"shop\" is a database: read it as shop.query(\"select …\") or shop.one(…)");
        assert_eq!(error("  shop = 1"), "\"shop\" is imported, and stays what it is: give a value of its own a name of its own");
    }

    #[test]
    fn document_calls_freeze_their_records_and_wire_each_computed_value_as_a_hole() {
        let d = compiled(
            r#"import { flow, route } from "holon"
import notes from "database:notes"

flow("f", { port: 8080 })

route.post("/n/:who", (req) => {
  const mine = notes.find(
    "notes",
    { owner: req.params.who, state: { "$in": ["open", req.json.also] } },
    { sort: { at: -1 }, limit: 20 },
  )
  const one = notes.find_one("notes")
  notes.insert("notes", { owner: req.params.who, text: req.json.text, tags: ["new"], pinned: false })
  notes.update(
    "notes",
    { _id: { "$oid": req.json.id } },
    { "$set": { text: req.json.text }, "$inc": { edits: 1 } },
    { expect: 1 },
  )
  notes.delete("notes", { owner: req.params.who, done: true })
  return { mine, one }
})
"#,
        )
        .doc;
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
        let error = |body: &str| {
            let src = "import { action, flow } from \"holon\"\nimport notes from \"database:notes\"\n\nflow(\"f\")\n\nexport const t = action(() => {\nBODY\n})\n";
            error(&src.replace("BODY", body)).2
        };
        assert_eq!(error("  const c = \"notes\"\n  return notes.find(c)"), "the collection is written in quotes, as it is: it is the permission");
        assert!(error("  return notes.find(\"holon_claims\")").starts_with("a collection is letters"));
        assert!(error("  const f = { a: 1 }\n  return notes.find(\"notes\", f)").starts_with("a document call's filter, update, document and sort are records written in the call"));
        assert!(error("  const f = { a: 1 }\n  return notes.find(\"notes\", { ...f, a: 1 })").contains("a spread would let the data choose its fields"));
        assert!(error("  return notes.find(\"notes\", { \"$where\": \"1\" })").contains("runs code on the database's server"));
        assert!(error("  notes.update(\"notes\", { a: 1 }, { a: 2 })").contains("isn't an update operator"));
        assert!(error("  notes.insert(\"notes\", { \"$set\": 1 })").contains("is an operator"));
        assert_eq!(error("  return notes.find(\"notes\", {}, { sort: { at: 2 } })"), "this sort can't be sent: a sort's fields are each 1 or -1, written as they are: \"at\" isn't");
        assert_eq!(error("  const k = 1\n  return notes.find(\"notes\", {}, { sort: { at: k } })"), "a sort is the flow's: each field is 1 or -1, as written");
        assert_eq!(error("  return notes.find(\"notes\", {}, { limit: 0 })"), "limit: is how many documents at most, a whole number from 1 to 100000");
        assert_eq!(error("  notes.delete(\"notes\")"), "notes.delete takes the collection, then its filter");
        assert_eq!(error("  notes.update(\"notes\", {})"), "notes.update takes the collection, then its filter, then its update");
        // Options a call doesn't take are read as one more record, which it doesn't take either.
        assert_eq!(error("  return notes.find_one(\"notes\", {}, { limit: 1 })"), "notes.find_one takes the collection, then its filter");
        assert_eq!(error("  notes.insert(\"notes\", {}, { expect: 1 })"), "notes.insert takes the collection, then its document");
    }

    #[test]
    fn folders_hand_out_handles_lines_and_slices_and_copy_by_handle() {
        let d = compiled(
            r#"import { action, flow } from "holon"
import archive from "folder:archive"
import logs from "folder:logs"

flow("f")

export const t = action(() => {
  const h = logs.file("app.log")
  const last = logs.lines("app.log", { from: -10, limit: 10 })
  const start = logs.slice("app.log", { offset: 0, length: 64 })
  archive["app.log"] = h
  archive.copy("b.log", h)
  archive["c.log"] = start
  return last
})
"#,
        )
        .doc;
        let mut got = leaves(&d);
        got.retain(|l| l.starts_with("file_"));
        assert_eq!(got, ["file_handle", "file_lines", "file_slice", "file_copy", "file_copy", "file_write"], "a handle written is a copy; a text, a write");
        let lines = d.nodes.iter().find(|n| n.data.name == "file_lines").unwrap();
        assert_eq!((lines.param("folder"), lines.param("limit")), ("logs", "10"));
        let first = compiled("import { action, flow } from \"holon\"\nimport logs from \"folder:logs\"\n\nflow(\"f\")\n\nexport const t = action(() => logs.lines(\"a\"))\n").doc;
        assert!(first.nodes.iter().any(|n| n.data.name == "file_lines"), "from the first line: from is optional");
        // Writes that never replace a file.
        let d = compiled(
            r#"import { action, flow } from "holon"
import out from "folder:out"

flow("f")

export const t = action(() => {
  const h = out.file("a")
  out.create("b.txt", "text")
  out.create_copy("c", h)
})
"#,
        )
        .doc;
        let got = leaves(&d);
        assert!(got.contains(&"file_create".to_string()) && got.contains(&"file_create_copy".to_string()), "{got:?}");
    }

    /// A file handler reads the file that started its run by its handle, and sends a file on by
    /// handle: the server streams the bytes, which never pass through the flow.
    #[test]
    fn a_file_handler_sends_a_file_by_handle() {
        let d = compiled(
            r#"import { flow, on } from "holon"
import inbox from "folder:inbox"
import files from "host:files.example.com"

flow("f")

on.file.created(inbox, async (f) => {
  const h = inbox.file(f.name)
  const r = await files.put("/up", { file: h })
})
"#,
        )
        .doc;
        let mut got = leaves(&d);
        got.retain(|l| l != "const" && l != "field");
        assert_eq!(got, ["on_file", "file_handle", "http_upload"]);
        let on = d.nodes.iter().find(|n| n.data.name == "on_file").unwrap();
        assert_eq!(on.param("folder"), "inbox", "the handler names the folder it watches");
        let upload = d.nodes.iter().find(|n| n.data.name == "http_upload").unwrap();
        assert_eq!((upload.param("method"), upload.param("host")), ("PUT", "files.example.com"));
        assert_eq!(
            error(
                r#"import { flow, on } from "holon"
import a from "folder:a"
import h from "host:h.example.com"

flow("f")

on.file.created(a, async (f) => {
  const x = a.file("x")
  const r = await h.put("/", { body: "t", file: x })
})
"#
            )
            .2,
            "a call sends a body or a file, not both"
        );
    }

    /// A secret is read only by its import (docs/L4s/declared-names-L4.md §4):
    /// `import tok from "secret:tok"`, with `{ optional: "true" }` for one the flow can do without.
    #[test]
    fn a_secret_is_read_by_its_import() {
        let signs = |import: &str| {
            compiled(&format!("import {{ flow, hmac_sha256, route }} from \"holon\"\n{import}\n\nflow(\"f\", {{ port: 8200 }})\n\nroute.post(\"/a\", (req) => hmac_sha256(tok, req.body))\n"))
        };
        let d = signs("import tok from \"secret:tok\"").doc;
        assert_eq!(d.nodes.iter().find(|n| n.data.name == "secret").unwrap().param("name"), "tok");
        // The optional one is the same deploy setting.
        let s = signs("import tok from \"secret:tok\" with { optional: \"true\" }").doc.server.unwrap();
        assert_eq!(s.requirements.unwrap().optional.unwrap(), ["SecretRead(\"tok\")"]);
        // A secret not imported is a name like any other unknown one.
        assert_eq!(error("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", (req) => tok)\n"), (5, 26, "unknown name \"tok\"".into()));
        // A function reads with the flow's imports.
        compiled(
            r#"import { action, flow, hmac_sha256 } from "holon"
import tok from "secret:tok"

flow("f")

function signed(t) {
  return hmac_sha256(tok, t)
}

export const t = action(() => signed("a"))
"#,
        );
        // An import nothing reads compiles.
        compiled("import { action, flow } from \"holon\"\nimport unused from \"secret:unused\"\n\nflow(\"f\")\n\nexport const t = action(() => 1)\n");
    }

    #[test]
    fn names_collections_and_state_errors() {
        let route = |head: &str, handler: &str| error(&format!("import {{ flow, route{head} }} from \"holon\"\n\nflow(\"f\", {{ port: 8000 }})\n\n{handler}\n"));
        assert_eq!(route("", "route.get(\"/a\", () => y)"), (5, 23, "unknown name \"y\"".into()));
        assert_eq!(route(", clock", "route.get(\"/a\", () => clock)"), (5, 23, "\"clock\" is a leaf: call it, clock(…)".into()));
        // A collection not imported: unknown to read, refused to write.
        assert_eq!(route("", "route.get(\"/a\", () => notes[1])"), (5, 23, "unknown name \"notes\"".into()));
        assert_eq!(route("", "route.post(\"/a\", (req) => {\n  notes[1] = 2\n})"), (6, 3, "\"notes\" isn't a local of this handler: only state, a collection, a folder or a local is written".into()));
        assert_eq!(route("", "route.get(\"/a\", () => notes.list())"), (5, 23, "unknown name \"notes\"".into()));
        assert_eq!(
            error("import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"f\", { port: 8000 })\n\nroute.get(\"/a\", () => notes)\n"),
            (6, 23, "\"notes\" is a collection: read it as notes[key] or notes.list(…)".into())
        );
        assert_eq!(
            error("import { flow, route } from \"holon\"\nimport api from \"host:h\"\n\nflow(\"f\", { port: 8000 })\n\nroute.get(\"/a\", async () => await api.fetch(\"/p\"))\n"),
            (6, 39, "http has no \"fetch\": get, post, put, patch, delete or head".into())
        );
        assert_eq!(route(", subtract", "route.get(\"/a\", () => subtract(1))"), (5, 23, "\"subtract\": the input \"b\" can't be left out".into()));
        assert_eq!(route(", subtract", "route.get(\"/a\", () => subtract(1, 2, 3))"), (5, 38, "\"subtract\" takes 2 inputs, 3 given".into()));
        assert_eq!(
            route(", hmac_sha256", "route.get(\"/a\", () => hmac_sha256(\"k\", \"m\", { encoding: 1 + 1 }))"),
            (5, 57, "a param is a fixed value: a number, a duration, text in quotes, true or false, or a word".into())
        );
        assert_eq!(route(", request", "route.get(\"/a\", () => request())"), (5, 23, "a route's handler takes its request: route.get(\"/path\", (req) => …)".into()));
        // A function written in place outside map and reduce: the reader says where it goes.
        assert_eq!(
            route("", "route.get(\"/a\", () => {\n  const x = (a) => a\n  return x\n})"),
            (6, 13, "a function written in place goes to map or reduce: xs.map((x) => …); define the others at the top level".into())
        );
    }

    #[test]
    fn settings_and_spans_come_through() {
        let got = compiled(
            r#"import { flow, route } from "holon"
import k from "secret:k" with { optional: "true" }
import c from "collection:c"

flow("f", { port: 8200, browsers: ["https://a.example", "*"] })

route.get("/x", () => {
  const x = 1
  return x
})
"#,
        );
        let s = got.doc.server.unwrap();
        assert_eq!((s.port.as_str(), s.browsers.as_deref()), ("8200", Some("https://a.example *")));
        assert_eq!(s.requirements.unwrap().optional.unwrap(), ["SecretRead(\"k\")"]);
        assert_eq!(got.name.as_deref(), Some("f"));
        let node = got.doc.nodes.iter().find(|n| n.data.name == "const").unwrap();
        assert_eq!(node.extra["source"], json!({ "line": 8, "col": 13, "endLine": 8, "endCol": 14 }));
        assert_eq!(got.names[&node.id], "x");
    }

    #[test]
    fn the_natural_samples_derive_the_same_routes_as_the_builders() {
        // The tour's own spellings (interpolation): same routes and manifest shape.
        let hello = compiled("import { flow, route } from \"holon\"\n\nflow(\"hello-api\", { port: 8101 })\n\nroute.get(\"/hello/:name\", (req) => `Hello, ${req.params.name}`)\n");
        let mut store = MapResolver::new();
        let d = derive_server(&hello.doc.nodes, &hello.doc.edges, &mut store, "8101");
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(d.endpoints.iter().map(|e| (e.method.as_str(), e.path.as_str())).collect::<Vec<_>>(), [("GET", "/hello/:name")]);
        assert_eq!(leaves(&hello.doc), ["request", "const", "field", "as_text", "concat", "respond"]);
    }

    /// The function `pick` of a text, as core evaluates it, given `op` and `v`.
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

    /// A flow whose function `pick` is `body`.
    fn with_pick(body: &str) -> String {
        format!("import {{ action, flow }} from \"holon\"\nimport state from \"state\"\n\nflow(\"f\")\n\nfunction pick(op, v) {{\n{body}\n}}\n\nexport const t = action(() => {{\n  state.x = pick(\"inv\", 1)\n}})\n")
    }

    const PICK: &str = r#"  switch (op) {
    case "inv":
      return 1 / v
    case "neg":
    case "minus":
      return 0 - v
    default:
      return 99
  }"#;

    /// A `switch` that returns is a `match` (match-L4.md §4): the guards written out by hand are
    /// the same unit.
    #[test]
    fn a_switch_is_the_same_unit_as_its_guards_written_by_hand() {
        let by_hand = "  const m1 = op == \"inv\"\n  const m2 = op == \"neg\" || op == \"minus\"\n  return (m1 ? 1 / v : undefined) ?? (m2 ? 0 - v : undefined) ?? (!(m1 || m2) ? 99 : undefined)";
        assert_eq!(hash_of(&compiled(&with_pick(PICK)).doc), hash_of(&compiled(&with_pick(by_hand)).doc));
        // Without a default, the chain alone.
        let no_default = PICK.replace("    case \"minus\":\n", "").replace("    default:\n      return 99\n", "");
        let by_hand = "  return (op == \"inv\" ? 1 / v : undefined) ?? (op == \"neg\" ? 0 - v : undefined)";
        assert_eq!(hash_of(&compiled(&with_pick(&no_default)).doc), hash_of(&compiled(&with_pick(by_hand)).doc));
        // One case and a default: the default guarded by the case's own comparison, negated.
        let d = compiled(
            r#"import { flow, route } from "holon"

flow("f", { port: 8000 })

route.post("/t", (req) => {
  switch (req.json.x) {
    case 1:
      return "one"
    default:
      return "other"
  }
})
"#,
        )
        .doc;
        assert_eq!(&leaves(&d)[2..10], ["const", "equals", "const", "guard", "not", "const", "guard", "coalesce"]);
    }

    /// Every row of match-L4.md §3, through the lowered unit.
    #[test]
    fn a_switch_chooses_one_case_and_its_absence_is_the_switchs() {
        use holon_core::Value::{Boolean, Number, Text};
        let t = |s: &str| Some(Text(s.into()));
        let n = |x: f64| Some(Number(x));
        let pick = with_pick(PICK);
        assert_eq!(picked(&pick, None, n(2.0)), None, "the subject absent: absent, the default too");
        assert_eq!(picked(&pick, t("inv"), n(2.0)), n(0.5), "the case chosen, present");
        assert_eq!(picked(&pick, t("inv"), n(0.0)), None, "the case chosen, absent: absent, not the default");
        assert_eq!(picked(&pick, t("neg"), n(0.0)), n(0.0), "another case absent: no effect");
        assert_eq!(picked(&pick, t("minus"), n(3.0)), n(-3.0), "a second label of a case");
        assert_eq!(picked(&pick, t("zzz"), None), n(99.0), "no case matched: the default, whatever the cases");
        let no_default = with_pick(&PICK.replace("    default:\n      return 99\n", ""));
        assert_eq!(picked(&no_default, t("zzz"), n(1.0)), None, "no case matched, no default: absent");
        let absent_default = with_pick(&PICK.replace("return 99", "return 2 / v"));
        assert_eq!(picked(&absent_default, t("zzz"), n(0.0)), None, "the default chosen, absent: absent");
        // `equals` doesn't coerce: a number is never a text.
        let kinds = with_pick("  switch (op) {\n    case 1:\n      return \"one\"\n    case \"1\":\n      return \"text one\"\n    case true:\n      return \"yes\"\n    default:\n      return v\n  }");
        assert_eq!(picked(&kinds, n(1.0), None), t("one"));
        assert_eq!(picked(&kinds, t("1"), None), t("text one"));
        assert_eq!(picked(&kinds, Some(Boolean(true)), None), t("yes"));
        assert_eq!(picked(&kinds, Some(Boolean(false)), None), None);
    }

    #[test]
    fn a_case_is_a_literal_matched_once() {
        let switch = |cases: &str| {
            let src = "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8000 })\n\nroute.post(\"/t\", (req) => {\n  const x = req.json.x\n  switch (x) {\nCASES\n  }\n})\n";
            error(&src.replace("CASES", cases))
        };
        // What isn't a literal, the reader refuses.
        let literal = "a case is a literal: a number, a text, true or false; to test a condition, use if";
        assert_eq!(switch("    case x:\n      return 1"), (8, 10, literal.into()));
        assert_eq!(switch("    case 1:\n      return 1\n    case x > 2:\n      return 2"), (10, 10, literal.into()));
        assert_eq!(switch("    case `${x}`:\n      return 1"), (8, 10, literal.into()));
        assert_eq!(switch("    case [1]:\n      return 1"), (8, 10, literal.into()));
        assert_eq!(switch("    case \"add\":\n      return 1\n    case \"add\":\n      return 2"), (10, 10, "\"add\" is a case twice; the second could never be chosen".into()));
        assert_eq!(switch("    case 1:\n    case 2:\n      return 1\n    case 1.0:\n      return 2"), (11, 10, "1.0 is a case twice; the second could never be chosen".into()));
    }

    /// A streamed reply answers with its call's status: the respond's status input, wired from
    /// the call's `status`. A computed status is the same input, and a number written out the
    /// param, as before.
    #[test]
    fn a_respond_takes_a_computed_status_on_its_status_input() {
        let into = |d: &FlowDocument, node: &str, port: &str| {
            d.edges.iter().find(|w| w.target == node && w.target_handle.as_deref() == Some(port)).map(|w| w.source.clone())
        };
        let streamed = compiled(
            r#"import { flow, respond, route } from "holon"
import a from "host:a.com"

flow("f", { port: 8200 })

route.post("/ask", async (req) => {
  const sent = await a.post("/x", { body: req.json, stream: true })
  return respond(sent.body, { stream: true })
})
"#,
        )
        .doc;
        let respond = streamed.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("status")), ("true", "200"));
        let field = streamed.nodes.iter().find(|n| Some(&n.id) == into(&streamed, &respond.id, "status").as_ref()).unwrap();
        let call = streamed.nodes.iter().find(|n| n.data.name == "http_stream").unwrap();
        assert_eq!((field.data.name.as_str(), field.param("name")), ("field", "status"));
        assert_eq!(into(&streamed, &field.id, "record"), Some(call.id.clone()));
        let computed = compiled("import { flow, route, status } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.post(\"/a\", (req) => status(req.json.code, { ok: false }) ?? status(400))\n").doc;
        let respond = computed.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("missing")), ("true", "400"));
        assert!(into(&computed, &respond.id, "status").is_some());
        let written = compiled("import { flow, route, status } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.post(\"/a\", () => status(201, { ok: true }))\n").doc;
        let respond = written.nodes.iter().find(|n| n.data.name == "respond").unwrap();
        assert_eq!((respond.param("status_input"), respond.param("status")), ("false", "201"));
    }
}
