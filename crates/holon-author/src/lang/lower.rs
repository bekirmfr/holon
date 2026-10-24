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

use holon_core::{interface_of, VARIADIC};
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
    "flow", "port", "every", "browsers", "optional", "collection", "holon", "route", "resumes", "respond", "delete", "await", "if", "then", "else",
    "when", "fn", "true", "false", "node", "at", "in", "out", "interval", "secret", "state", "now", "random", "ingress", "http", "stripe", "json",
    "parse_json",
];

/// Plain names for two leaves, read as English.
fn alias(name: &str) -> &str {
    match name {
        "json" => "json_stringify",
        "parse_json" => "json_parse",
        n => n,
    }
}

/// Inputs a node works without, left unwired (routes.rs `is_optional_input`, core's
/// `has_optional_ports`).
fn is_optional(leaf: &str, input: &str) -> bool {
    matches!((leaf, input), ("http_request", "headers" | "body") | ("collection_list", "prefix" | "after"))
}

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
    /// A holon's block: its declared outputs, in order, and what each is bound to.
    Holon { declared: Vec<String>, bound: HashMap<String, (Val, Span)>, effects: Vec<(Val, Span)> },
    /// A function's body: the names outside it, for the error that it may not use them.
    Lambda { outer: Vec<String> },
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

struct Lowerer<'a> {
    b: FlowBuilder,
    defs: HashMap<String, &'a HolonDef>,
    holons: HashMap<String, Holon>,
    in_progress: HashSet<String>,
    collections: HashSet<String>,
    /// Top-level bindings of literals in a flow with routes: lowered afresh in each route.
    consts: HashMap<String, &'a Expr>,
    frames: Vec<Frame>,
    names: BTreeMap<String, String>,
    spans: BTreeMap<String, Span>,
    lambdas: usize,
    /// Each route's request node, the nodes its block placed, and its header's span: a route is
    /// everything wired to its request, so a node left unwired to it would silently fall out.
    routes: Vec<(String, Vec<String>, Span)>,
}

impl<'a> Lowerer<'a> {
    fn new(file: &'a File) -> Lowerer<'a> {
        let defs = file.items.iter().filter_map(|i| if let Item::Holon(h) = i { Some((h.name.text.clone(), h)) } else { None }).collect();
        let collections = file.headers.iter().filter_map(|h| if let HeaderKind::Collection(c) = &h.kind { Some(c.clone()) } else { None }).collect();
        Lowerer {
            b: FlowBuilder::new(),
            defs,
            holons: HashMap::new(),
            in_progress: HashSet::new(),
            collections,
            consts: HashMap::new(),
            frames: vec![],
            names: BTreeMap::new(),
            spans: BTreeMap::new(),
            lambdas: 0,
            routes: vec![],
        }
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame")
    }

    fn cur(&self) -> &Frame {
        self.frames.last().expect("a frame")
    }

    fn run(mut self, file: &'a File) -> R<Lowered> {
        // Holons twice defined.
        let mut seen = HashSet::new();
        for i in &file.items {
            if let Item::Holon(h) = i
                && !seen.insert(h.name.text.as_str())
            {
                return err(h.name.span, format!("there is a holon \"{}\" already", h.name.text));
            }
        }
        let has_routes = file.items.iter().any(|i| matches!(i, Item::Route(_)));
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
            if let Item::Route(r) = i {
                self.route(r)?;
            }
        }
        // Holons nobody placed are still defined (and checked).
        let names: Vec<String> = self.defs.keys().cloned().collect();
        for name in names {
            if !self.holons.contains_key(&name) {
                let span = self.defs[&name].name.span;
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
        Ok(Lowered { name, doc, names: self.names, spans: self.spans })
    }

    fn settings(&self, file: &File) -> R<Settings> {
        let mut s = Settings::default();
        let mut optional = Vec::new();
        for h in &file.headers {
            match &h.kind {
                HeaderKind::Port(p) => s.port = p.clone(),
                HeaderKind::Every { seconds, .. } => s.interval = number_text(*seconds),
                HeaderKind::Interval(i) => s.interval = i.clone(),
                HeaderKind::Browsers(o) => s.browsers = Some(o.join(" ")).filter(|b| !b.is_empty()),
                HeaderKind::Optional(c) => optional.push(match c {
                    Consent::Secret(n) => format!("SecretRead({n:?})"),
                    Consent::Named { name, arg: Some(a) } => format!("{name}({a:?})"),
                    Consent::Named { name, arg: None } => name.clone(),
                }),
                HeaderKind::Flow(_) | HeaderKind::Collection(_) => {}
            }
        }
        if !optional.is_empty() {
            s.requirements = Some(Requirements { optional: Some(optional), ..Default::default() });
        }
        Ok(s)
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
        if self.collections.contains(n) {
            return err(name.span, format!("\"{n}\" is a collection; a binding may not shadow it"));
        }
        if self.defs.contains_key(n) {
            return err(name.span, format!("\"{n}\" is a holon; a binding may not shadow it"));
        }
        if self.cur().env.contains_key(n) {
            return err(name.span, format!("there is a \"{n}\" already"));
        }
        Ok(())
    }

    fn stmt(&mut self, s: &'a Stmt) -> R<()> {
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
                self.check_collection(collection)?;
                let k = self.expr(key)?;
                let v = self.expr(value)?;
                let w = self.node_with("collection_put", &[("collection".into(), collection.text.clone())], &[("key", k), ("value", v)], s.span)?;
                self.effect(w, s.span)
            }
            StmtKind::Delete { collection, key } => {
                self.check_collection(collection)?;
                let k = self.expr(key)?;
                let w = self.node_with("collection_delete", &[("collection".into(), collection.text.clone())], &[("key", k)], s.span)?;
                self.effect(w, s.span)
            }
            StmtKind::Respond(args) => self.respond(args, s.span),
            StmtKind::Expr(e) => {
                let v = self.expr(e)?;
                let top_without_routes = matches!(self.cur().kind, FrameKind::Flow { has_routes: false });
                if v.effectful() || top_without_routes {
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
                let id = if self.defs.contains_key(&of.text) {
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

    fn check_collection(&self, name: &Name) -> R<()> {
        if self.collections.contains(&name.text) {
            Ok(())
        } else {
            err(name.span, format!("\"{}\" is not a declared collection (add `collection {}` to the header)", name.text, name.text))
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
        let request = match &self.cur().kind {
            FrameKind::Route { request, responded } => {
                if let Some(first) = responded {
                    return err(span, format!("more than one respond in the route (the first is on line {})", first.start.line));
                }
                request.clone()
            }
            _ => return err(span, "respond belongs in a route"),
        };
        let (mut status, mut missing, mut body): (Option<String>, Option<String>, Option<&Expr>) = (None, None, None);
        let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        for a in args.iter().filter(|a| a.name.is_some()) {
            let n = a.name.as_ref().unwrap();
            match n.text.as_str() {
                "status" => status = Some(self.status_text(&a.value)?),
                "missing" => missing = Some(self.status_text(&a.value)?),
                "body" => body = Some(&a.value),
                other => return err(n.span, format!("respond has no \"{other}\": a body, then status: and missing:")),
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
        let status = status.unwrap_or_else(|| "200".into());
        let missing = missing.unwrap_or_else(|| "404".into());
        if status == "204"
            && let Some(b) = body
        {
            return err(b.span, "a 204 answers with no body");
        }
        // The body's nodes first, then the respond they feed.
        let body_val = match body {
            Some(b) => Some((self.expr(b)?, b.span)),
            None => None,
        };
        let params = vec![("status".to_string(), status), ("missing".to_string(), missing)];
        let node = self.node("respond", &params, span)?;
        let Val::Out { id, .. } = &node else { unreachable!() };
        let id = id.clone();
        self.b.wire(&request, "request", &id, "request").or_else(|e| err(span, e))?;
        if let Some((v, at)) = body_val {
            self.connect(&v, &id, "body", at)?;
        }
        if let FrameKind::Route { responded, .. } = &mut self.frame().kind {
            *responded = Some(span);
        }
        Ok(())
    }

    fn status_text(&self, e: &Expr) -> R<String> {
        match &e.kind {
            ExprKind::Number { value, text } if (100.0..600.0).contains(value) && value.fract() == 0.0 => Ok(text.clone()),
            _ => err(e.span, "a status is a number from 100 to 599"),
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
        for s in &r.body {
            self.stmt(s)?;
        }
        let frame = self.frames.pop().expect("the route's frame");
        // No respond and no effect: route_spec would refuse it too; say so here, at the route.
        let FrameKind::Route { responded, request } = frame.kind else { unreachable!() };
        self.routes.push((request, frame.created, r.method.span.to(r.path.span)));
        let has_effect = r.body.iter().any(|s| !matches!(s.kind, StmtKind::Bind { .. } | StmtKind::Respond(_) | StmtKind::Node { .. } | StmtKind::Wire { .. }));
        if responded.is_none() && !has_effect && !frame.wiring_form {
            return err(r.method.span.to(r.path.span), "the route answers nothing: add a respond (or an effect, answered 204)");
        }
        Ok(())
    }

    fn check_name_free(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if interface_of(n).is_some() || templates::is_template(n) || RESERVED.contains(&n) || self.collections.contains(n) || self.defs.contains_key(n) {
            return err(name.span, format!("\"{n}\" is taken; pick another name"));
        }
        Ok(())
    }

    /// A holon's or function's port may be called after a leaf (`sum`, `out`: the wiring form
    /// names ports that way, and a name in scope wins over a leaf), but not after a keyword, a
    /// collection or a holon.
    fn check_port_name(&self, name: &Name) -> R<()> {
        let n = name.text.as_str();
        if (RESERVED.contains(&n) && n != "in" && n != "out") || self.collections.contains(n) || self.defs.contains_key(n) {
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
        let result = self.holon_body(def);
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
                    "store_write" => "write",
                    "egress" => "sent",
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
            let inputs: Vec<HolonPort> = def.inputs.iter().map(|p| HolonPort { name: p.name.text.clone(), unit: p.unit.clone() }).collect();
            // Inputs in declared order, as the boundary lists them.
            wiring_in.sort_by_key(|(port, _, _)| inputs.iter().position(|p| p.name == *port));
            self.b.end_holon_exact(name, &inputs, &outputs, &wiring_in, &wiring_out).or_else(|e| err(def.name.span, e))?
        };
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
        let mut positional = 0;
        for a in args {
            match &a.name {
                None => {
                    if positional >= h.inputs.len() {
                        return err(a.value.span, format!("\"{}\" takes {} inputs", name.text, h.inputs.len()));
                    }
                    given[positional] = Some(&a.value);
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

    /// A closed function: an anonymous holon with the parameters as inputs and the body as its
    /// one output. Its hash, as a `fn` param.
    fn lambda(&mut self, params: &'a [Name], body: &'a Expr, span: Span) -> R<String> {
        self.lambdas += 1;
        let name = format!("fn_{}", self.lambdas);
        let outer: Vec<String> = self.frames.iter().flat_map(|f| f.env.keys().cloned()).collect();
        self.b.begin_holon(&name);
        let mut frame = Frame::new(FrameKind::Lambda { outer });
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
        let inputs: Vec<HolonPort> = params.iter().map(|p| HolonPort { name: p.text.clone(), unit: None }).collect();
        for p in params {
            if !frame.wiring_in.iter().any(|(port, _, _)| *port == p.text) {
                return err(p.span, format!("the parameter \"{}\" is never used", p.text));
            }
        }
        let mut wiring_in = frame.wiring_in;
        wiring_in.sort_by_key(|(port, _, _)| inputs.iter().position(|p| p.name == *port));
        let h = self.b.end_holon_exact(&name, &inputs, &[HolonPort { name: "out".into(), unit: None }], &wiring_in, &[(id.clone(), output, "out".into())]).or_else(|e| err(span, e))?;
        Ok(h.hash)
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
            ExprKind::Noun(n) => self.node(
                match n {
                    Noun::Now => "clock",
                    Noun::Random => "random",
                    Noun::Ingress => "ingress",
                },
                &[],
                e.span,
            ),
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
                    && self.collections.contains(c)
                {
                    let k = self.expr(key)?;
                    return self.node_with("collection_get", &[("collection".into(), c.clone())], &[("key", k)], e.span);
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
            ExprKind::List(items) => match items.len() {
                0 => self.node("const", &[("value".into(), "[]".into()), ("type".into(), "Json".into())], e.span),
                1 => {
                    let empty = self.node("const", &[("value".into(), "[]".into()), ("type".into(), "Json".into())], e.span)?;
                    let item = self.expr(&items[0])?;
                    self.node_with("append", &[], &[("list", empty), ("item", item)], e.span)
                }
                n => {
                    let mut vals = Vec::new();
                    for i in items {
                        vals.push(self.expr(i)?);
                    }
                    let ports = Self::variadic_ports(&[WPort::new("a", WType::Any), WPort::new("b", WType::Any)], n);
                    let inputs: Vec<(&str, Val)> = ports.iter().map(String::as_str).zip(vals).collect();
                    self.node_with("list", &[], &inputs, e.span)
                }
            },
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
        if let Some(c) = self.consts.get(n).copied()
            && !matches!(self.cur().kind, FrameKind::Lambda { .. })
        {
            return self.expr(c);
        }
        if let FrameKind::Lambda { outer } = &self.cur().kind
            && (outer.iter().any(|o| o == n) || self.consts.contains_key(n))
        {
            return err(span, format!("\"{n}\" is outside the function: fn may only use its own arguments"));
        }
        if self.defs.contains_key(n) {
            return err(span, format!("\"{n}\" is a holon: call it, {n}(…)"));
        }
        if self.collections.contains(n) {
            return err(span, format!("\"{n}\" is a collection: read it as {n}[key] or {n}.list(…)"));
        }
        if interface_of(alias(n)).is_some() || templates::is_template(n) {
            return err(span, format!("\"{n}\" is a leaf: call it, {n}(…)"));
        }
        err(span, format!("unknown name \"{n}\""))
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
        if self.defs.contains_key(&callee.text) {
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
                let (inputs, params) = leaf("http_request");
                let preset = vec![("method".to_string(), m), ("host".to_string(), host.clone())];
                self.unit_call("http_request", &inputs, &params, &preset, &args[1..], span)
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
            c if self.collections.contains(c) => {
                let leaf_name = match method.text.as_str() {
                    "get" => "collection_get",
                    "put" => "collection_put",
                    "delete" => "collection_delete",
                    "list" => "collection_list",
                    other => return err(method.span, format!("a collection has no \"{other}\": get, put, delete or list")),
                };
                let (inputs, params) = leaf(leaf_name);
                self.unit_call(leaf_name, &inputs, &params, &[("collection".to_string(), c.to_string())], args, span)
            }
            c if ["get", "put", "delete", "list"].contains(&method.text.as_str()) => {
                err(receiver.span, format!("\"{c}\" is not a declared collection (add `collection {c}` to the header)"))
            }
            other => err(receiver.span, format!("\"{other}\" has no methods; write {other}.{} as a field, or call a leaf", method.text)),
        }
    }

    /// A leaf or template placed with its arguments: positional ones are inputs in port order
    /// (a variadic leaf grows), named ones are inputs when the unit has that port, else params.
    /// `preset` params come from the sugar (`http.post`'s method and host).
    fn unit_call(&mut self, leaf: &str, inputs: &[WPort], defaults: &[WParam], preset: &[(String, String)], args: &'a [Arg], span: Span) -> R<Val> {
        let variadic = VARIADIC.contains(&leaf);
        let mut params: Vec<(String, String)> = preset.to_vec();
        let mut given: Vec<Option<&'a Expr>> = vec![None; inputs.len()];
        let mut extra: Vec<&'a Expr> = Vec::new();
        let mut positional = 0;
        let zero_input = inputs.is_empty();
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
                    if positional < inputs.len() {
                        given[positional] = Some(&a.value);
                    } else if variadic {
                        extra.push(&a.value);
                    } else {
                        return err(a.value.span, format!("\"{leaf}\" takes {} inputs, {} given", inputs.len(), positional + 1));
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
                        let value = if n.text == "fn" { self.fn_param(&a.value)? } else { self.literal_text(&a.value)? };
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
        let names = Self::variadic_ports(inputs, inputs.len() + extra.len());
        for (i, e) in extra.iter().enumerate() {
            vals.push((names[inputs.len() + i].clone(), self.expr(e)?));
        }
        let v = self.node(leaf, &params, span)?;
        let Val::Out { id, .. } = &v else { unreachable!() };
        let id = id.clone();
        if !extra.is_empty() {
            self.b.arity(&id, inputs.len() + extra.len()).or_else(|e| err(span, e))?;
        }
        for (port, val) in &vals {
            self.connect(val, &id, port, span)?;
        }
        Ok(self.out(&id))
    }

    /// A `fn:` argument: a holon by name, or a closed function; its hash.
    fn fn_param(&mut self, e: &'a Expr) -> R<String> {
        match &e.kind {
            ExprKind::Name(n) if self.defs.contains_key(n) => Ok(self.holon(n, e.span)?.hash),
            ExprKind::Name(n) => err(e.span, format!("\"{n}\" is not a holon: fn: takes a holon's name or fn(…) -> …")),
            ExprKind::Lambda { params, body } => self.lambda(params, body, e.span),
            _ => err(e.span, "fn: takes a holon's name or a function, fn(a, b) -> …"),
        }
    }
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
        assert_eq!(error("x = 1\nholon h(a) -> o {\n  o = fold(0, a, fn: fn(acc, p) -> acc + x)\n}"), (3, 42, "\"x\" is outside the function: fn may only use its own arguments".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = h(a)\n}"), (2, 7, "the holon \"h\" places itself: a holon can't contain itself".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = a + 1\n}\nx = h(1, 2)"), (4, 10, "\"h\" takes 1 inputs".into()));
        assert_eq!(error("holon h(a) -> o {\n  o = a + 1\n}\nx = h(k: 1)"), (4, 7, "\"h\" has no input \"k\" (a holon takes no params)".into()));
    }

    #[test]
    fn a_holons_effect_statements_leave_as_outputs() {
        let d = compiled("collection posts\nholon keep(k, v) -> receipt {\n  posts[k] = v\n  state.last = k\n  receipt = \"ok\"\n}\nx = keep(\"a\", 1)\n").doc;
        let keep = &d.nodes[2];
        let outs: Vec<&str> = keep.data.outputs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(outs, ["receipt", "put", "write"]);
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
        assert_eq!(error("route GET /a (req) {\n  respond 1, status: 700\n}"), (2, 22, "a status is a number from 100 to 599".into()));
        assert_eq!(error("route GET /a (req) {\n  respond \"x\", status: 204\n}"), (2, 11, "a 204 answers with no body".into()));
        assert_eq!(error("route GET /a (req) {\n  1 + 1\n  respond 2\n}"), (2, 3, "a value nobody uses: bind it, or remove it".into()));
        // A statement that never touches the request isn't in the route's group: said, not dropped.
        assert_eq!(
            error("route GET /a (req) {\n  state.hits = (state.hits ?? 0) + 1\n  respond 2\n}"),
            (2, 17, "this isn't wired to the route's request, so it isn't part of the route (a route is everything wired to its request): use the request in it, or move it to a flow of its own".into())
        );
        assert_eq!(error("x = 1 + 1\nroute GET /a (req) {\n  respond x\n}"), (1, 1, "routes are separate units; put shared logic in a holon (only a literal may be bound at the top level)".into()));
        // A literal at the top level is placed afresh in each route that uses it.
        let d = compiled("greeting = \"hi\"\nroute GET /a (req) {\n  respond greeting\n}\nroute GET /b (req) {\n  respond greeting\n}\n").doc;
        assert_eq!(leaves(&d), ["request", "const", "respond", "request", "const", "respond"]);
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
}
