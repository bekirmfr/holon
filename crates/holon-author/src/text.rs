//! A flow as text (`.holon`): written and read in any editor, diffed, reviewed, kept in git, and
//! compiled through the same builder as the samples (builder.rs), against the live catalog, so a
//! flow written here and the same flow drawn on the canvas are the same unit. One statement a line:
//!
//! ```text
//!   flow hello-api                      the flow's name
//!   port 8101                           deploy settings: port, interval, browsers, optional
//!   node greet = concat {} at 40 120    a node: a catalog leaf, a template, or a holon defined
//!                                       here; params in { }, an optional canvas position;
//!                                       `concat[3]` for a variadic node with three inputs
//!   hi.out -> greet.a                   a wire, from an output to an input
//!   holon slugify(text) -> (slug) {     a holon: a flow of its own, placed as one node. Its
//!     node lower = lowercase            boundary is proposed as "group" does, or given:
//!     in.text -> lower.s                `in.<input> -> node.port`, `node.port -> out.<output>`;
//!     ...                               a port's unit name, when it differs, follows in quotes:
//!   }                                   `(text "lowercase · s")`
//!   # a comment
//! ```
//!
//! Names with spaces or dots are quoted ("lowercase · s"); values too, as JSON strings, when they
//! aren't a plain word. Errors say the line.

use std::collections::{BTreeMap, HashMap, HashSet};

use holon_core::{decode_preimage, interface_of, MapResolver, UnitBody, BOUNDARY};
use serde::Serialize;

use crate::builder::{FlowBuilder, FlowDocument, Holon, HolonPort, Requirements, Settings};
use crate::canvas::{Node, NodeData, PlacedBody, Position, Wire};
use crate::composites::{inner_canvas, resolve, Names};
use crate::templates;
use crate::wire::{WParam, WPort};

/// Why a text can't be read or built, and on which line (1-based).
#[derive(Clone, Debug, PartialEq)]
pub struct TextError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, TextError> {
    Err(TextError { line, message: message.into() })
}

// ── reading ──────────────────────────────────────────────────────────────────────────────────

/// A holon's port as its header gives it: its name on the node, and the unit's own name when that
/// differs.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    pub unit: Option<String>,
}

/// A wire's end: a node, and one of its ports.
pub type End = (String, String);

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Node { line: usize, name: String, of: String, arity: Option<usize>, params: Vec<(String, String)>, at: Option<Position> },
    Wire { line: usize, from: End, to: End },
    Holon { line: usize, name: String, inputs: Vec<Port>, outputs: Vec<Port>, body: Vec<Stmt> },
}

/// A parsed `.holon` text: the flow's name, its deploy settings, its statements.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HolonFile {
    pub name: Option<String>,
    pub port: Option<String>,
    pub interval: Option<String>,
    pub browsers: Option<String>,
    pub optional: Vec<String>,
    pub body: Vec<Stmt>,
}

/// JavaScript's whitespace (`\s`), which the format was first read with.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

const PUNCT: &str = "{}()[],:=.";

/// One line's tokens: words, quoted strings (JSON, quotes kept), and punctuation.
fn tokens(text: &str, line: usize) -> Result<Vec<String>, TextError> {
    let c: Vec<char> = text.chars().collect();
    let arrow = |i: usize| c.get(i) == Some(&'-') && c.get(i + 1) == Some(&'>');
    let (mut out, mut i) = (Vec::new(), 0);
    while i < c.len() {
        if c[i] == '#' {
            break;
        } else if is_space(c[i]) {
            i += 1;
        } else if c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            if j >= c.len() {
                return err(line, "a quote isn't closed");
            }
            out.push(c[i..=j].iter().collect());
            i = j + 1;
        } else if arrow(i) {
            out.push("->".into());
            i += 2;
        } else if PUNCT.contains(c[i]) {
            out.push(c[i].to_string());
            i += 1;
        } else {
            let mut j = i;
            while j < c.len() && !is_space(c[j]) && !PUNCT.contains(c[j]) && c[j] != '"' && c[j] != '#' && !arrow(j) {
                j += 1;
            }
            out.push(c[i..j].iter().collect());
            i = j;
        }
    }
    Ok(out)
}

fn unquote(t: &str, line: usize) -> Result<String, TextError> {
    if !t.starts_with('"') {
        return Ok(t.into());
    }
    serde_json::from_str::<String>(t).or_else(|_| err(line, format!("{t} isn't a valid quoted string")))
}

/// A number as JavaScript's `Number()` reads one.
fn js_number(s: &str) -> f64 {
    let t = s.trim_matches(is_space);
    if t.is_empty() {
        return 0.0;
    }
    let radix = |p: &str, r: u32| t.strip_prefix(p).or_else(|| t.strip_prefix(&p.to_uppercase())).map(|d| u64::from_str_radix(d, r).map(|v| v as f64).unwrap_or(f64::NAN));
    if let Some(v) = radix("0x", 16).or_else(|| radix("0o", 8)).or_else(|| radix("0b", 2)) {
        return v;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if t.chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// A JavaScript object's keys, in the order it lists them: integer-like keys first, ascending,
/// then the rest as first set. A key set twice keeps its place and its last value.
fn js_record(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in pairs {
        match out.iter_mut().find(|(x, _)| *x == k) {
            Some(slot) => slot.1 = v,
            None => out.push((k, v)),
        }
    }
    let index = |k: &str| (k == "0" || (!k.starts_with('0') && !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))).then(|| k.parse::<u64>().ok()).flatten().filter(|&n| n < u32::MAX as u64);
    let (mut ints, rest): (Vec<_>, Vec<_>) = out.into_iter().partition(|(k, _)| index(k).is_some());
    ints.sort_by_key(|(k, _)| index(k));
    ints.into_iter().chain(rest).collect()
}

struct Cursor<'a> {
    t: &'a [String],
    at: usize,
    line: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<&str> {
        self.t.get(self.at).map(String::as_str)
    }
    fn next(&mut self, what: &str) -> Result<String, TextError> {
        let tok = self.t.get(self.at).cloned().ok_or_else(|| TextError { line: self.line, message: format!("expected {what}") })?;
        self.at += 1;
        Ok(tok)
    }
    fn expect(&mut self, tok: &str) -> Result<(), TextError> {
        let got = self.next(&format!("\"{tok}\""))?;
        if got != tok {
            return err(self.line, format!("expected \"{tok}\", found \"{got}\""));
        }
        Ok(())
    }
    fn word(&mut self, what: &str) -> Result<String, TextError> {
        let w = self.next(what)?;
        if w == "->" || (w.chars().count() == 1 && PUNCT.contains(w.as_str())) {
            return err(self.line, format!("expected {what}, found \"{w}\""));
        }
        unquote(&w, self.line)
    }
    fn end(&self) -> Result<(), TextError> {
        match self.peek() {
            Some(t) => err(self.line, format!("unexpected \"{t}\"")),
            None => Ok(()),
        }
    }
}

/// Parse `.holon` text: its statements, or the first line it can't read.
pub fn parse(text: &str) -> Result<HolonFile, TextError> {
    let mut file = HolonFile::default();
    // The holon being read, when inside one.
    let mut open: Option<Stmt> = None;
    for (index, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let line = index + 1;
        let t = tokens(raw, line)?;
        if t.is_empty() {
            continue;
        }
        let mut c = Cursor { t: &t, at: 1, line };
        let head = t[0].as_str();
        if head == "}" {
            let Some(h) = open.take() else { return err(line, "a } without a holon to close") };
            file.body.push(h);
            c.end()?;
            continue;
        }
        if open.is_none() && ["flow", "port", "interval", "browsers", "optional"].contains(&head) {
            match head {
                "flow" => file.name = Some(c.word("the flow's name")?),
                "optional" => file.optional.push(c.word("a permission, in quotes")?),
                "browsers" => {
                    let mut rest = Vec::new();
                    while c.peek().is_some() {
                        let w = c.next("an origin")?;
                        if w != "," {
                            rest.push(unquote(&w, line)?);
                        }
                    }
                    file.browsers = Some(rest.join(" "));
                }
                "port" => file.port = Some(c.word("the port")?),
                _ => file.interval = Some(c.word("the interval")?),
            }
            c.end()?;
            continue;
        }
        let stmt = if head == "node" {
            let name = c.word("the node's name")?;
            if name == "in" || name == "out" {
                return err(line, format!("\"{name}\" names a holon's boundary: call the node something else"));
            }
            c.expect("=")?;
            let of = c.word("what the node is (a leaf, a template or a holon)")?;
            let mut arity = None;
            if c.peek() == Some("[") {
                c.at += 1;
                let n = js_number(&c.word("how many inputs")?);
                if !(n.is_finite() && n.fract() == 0.0 && n >= 1.0) {
                    return err(line, "inputs: a whole number");
                }
                arity = Some(n as usize);
                c.expect("]")?;
            }
            let mut params = Vec::new();
            if c.peek() == Some("{") {
                c.at += 1;
                while c.peek() != Some("}") {
                    let key = c.word("a param's name")?;
                    c.expect(":")?;
                    let value = c.word(&format!("{key}'s value"))?;
                    params.push((key.clone(), value));
                    match c.peek() {
                        Some(",") => c.at += 1,
                        Some("}") => {}
                        _ => return err(line, format!("expected \",\" or \"}}\" after {key}'s value")),
                    }
                }
                c.at += 1;
            }
            let mut at = None;
            if c.peek() == Some("at") {
                c.at += 1;
                let (x, y) = (js_number(&c.word("x")?), js_number(&c.word("y")?));
                if !x.is_finite() || !y.is_finite() {
                    return err(line, "at: two numbers, x and y");
                }
                at = Some(Position { x, y });
            }
            c.end()?;
            Stmt::Node { line, name, of, arity, params: js_record(params), at }
        } else if head == "holon" {
            if open.is_some() {
                return err(line, "a holon is defined at the top level (and placed anywhere)");
            }
            let name = c.word("the holon's name")?;
            let ports = |c: &mut Cursor| -> Result<Vec<Port>, TextError> {
                c.expect("(")?;
                let mut list = Vec::new();
                while c.peek() != Some(")") {
                    let name = c.word("a port's name")?;
                    let unit = if c.peek().is_some_and(|p| p.starts_with('"')) { Some(unquote(&c.next("its unit name")?, line)?) } else { None };
                    let unit = unit.filter(|u| !u.is_empty() && *u != name);
                    list.push(Port { name: name.clone(), unit });
                    match c.peek() {
                        Some(",") => c.at += 1,
                        Some(")") => {}
                        _ => return err(line, format!("expected \",\" or \")\" after {name}")),
                    }
                }
                c.at += 1;
                Ok(list)
            };
            let inputs = ports(&mut c)?;
            c.expect("->")?;
            let outputs = ports(&mut c)?;
            c.expect("{")?;
            c.end()?;
            open = Some(Stmt::Holon { line, name, inputs, outputs, body: vec![] });
            continue;
        } else {
            // A wire: a.out -> b.in
            c.at = 0;
            let end = |c: &mut Cursor| -> Result<End, TextError> {
                let node = c.word("a node")?;
                c.expect(".")?;
                Ok((node, c.word("a port")?))
            };
            let from = end(&mut c)?;
            c.expect("->")?;
            let to = end(&mut c)?;
            c.end()?;
            Stmt::Wire { line, from, to }
        };
        match &mut open {
            Some(Stmt::Holon { body, .. }) => body.push(stmt),
            _ => file.body.push(stmt),
        }
    }
    if let Some(Stmt::Holon { line, name, .. }) = open {
        return err(line, format!("the holon \"{name}\" isn't closed with }}"));
    }
    Ok(file)
}

// ── compiling ────────────────────────────────────────────────────────────────────────────────

/// A text compiled: the flow's name (when it gives one), its document, and each top-level node's
/// name in the text, by its id in the document.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Compiled {
    pub name: Option<String>,
    pub doc: FlowDocument,
    pub names: BTreeMap<String, String>,
}

/// Build `stmts` into the builder's current flow: each node by its name in the text.
fn replay(b: &mut FlowBuilder, stmts: &[Stmt], holons: &HashMap<String, Holon>) -> Result<HashMap<String, String>, TextError> {
    let mut ids: HashMap<String, String> = HashMap::new();
    for s in stmts {
        match s {
            Stmt::Node { line, name, of, arity, params, at } => {
                let line = *line;
                if ids.contains_key(name) {
                    return err(line, format!("there is a node \"{name}\" already"));
                }
                let id = match holons.get(of) {
                    Some(holon) => {
                        if !params.is_empty() {
                            return err(line, format!("the holon \"{of}\" takes no params"));
                        }
                        b.place(holon, *at)
                    }
                    None => b.node(of, params, *at).or_else(|e| err(line, e))?,
                };
                // A variadic node's extra inputs, as many as it says.
                if let Some(n) = arity {
                    b.arity(&id, *n).or_else(|e| err(line, e))?;
                }
                ids.insert(name.clone(), id);
            }
            Stmt::Wire { line, from, to } => {
                // A holon's boundary: its block reads these.
                if from.0 == "in" || to.0 == "out" {
                    continue;
                }
                let find = |n: &str| ids.get(n).cloned().ok_or_else(|| TextError { line: *line, message: format!("no node \"{n}\" (nodes are declared before they're wired)") });
                let (src, dst) = (find(&from.0)?, find(&to.0)?);
                b.wire(&src, &from.1, &dst, &to.1).or_else(|e| err(*line, e))?;
            }
            Stmt::Holon { line, .. } => return err(*line, "a holon is defined at the top level"),
        }
    }
    Ok(ids)
}

/// `.holon` text as a flow document (and its name), through the samples' builder; errors say the
/// line.
pub fn compile(text: &str) -> Result<Compiled, TextError> {
    let file = parse(text)?;
    let mut b = FlowBuilder::new();
    let mut holons: HashMap<String, Holon> = HashMap::new();
    let mut top = Vec::new();
    for s in &file.body {
        let Stmt::Holon { line, name, inputs, outputs, body } = s else {
            top.push(s.clone());
            continue;
        };
        let line = *line;
        if holons.contains_key(name) {
            return err(line, format!("there is a holon \"{name}\" already"));
        }
        let boundary: Vec<(usize, &End, &End)> = body
            .iter()
            .filter_map(|x| match x {
                Stmt::Wire { line, from, to } if from.0 == "in" || to.0 == "out" => Some((*line, from, to)),
                _ => None,
            })
            .collect();
        b.begin_holon(name);
        let ids = replay(&mut b, body, &holons)?;
        let holon = if boundary.is_empty() {
            let names = |ps: &[Port]| ps.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
            b.end_holon(name, &names(inputs), &names(outputs))
        } else {
            let find = |line: usize, node: &str| ids.get(node).cloned().ok_or_else(|| TextError { line, message: format!("no node \"{node}\" in the holon \"{name}\"") });
            let wiring_in = boundary.iter().filter(|(_, f, _)| f.0 == "in").map(|(l, f, t)| Ok((f.1.clone(), find(*l, &t.0)?, t.1.clone()))).collect::<Result<Vec<_>, TextError>>()?;
            let wiring_out = boundary.iter().filter(|(_, _, t)| t.0 == "out").map(|(l, f, t)| Ok((find(*l, &f.0)?, f.1.clone(), t.1.clone()))).collect::<Result<Vec<_>, TextError>>()?;
            let ports = |ps: &[Port]| ps.iter().map(|p| HolonPort { name: p.name.clone(), unit: p.unit.clone() }).collect::<Vec<_>>();
            b.end_holon_exact(name, &ports(inputs), &ports(outputs), &wiring_in, &wiring_out)
        }
        .or_else(|e| err(line, e))?;
        holons.insert(name.clone(), holon);
    }
    let ids = replay(&mut b, &top, &holons)?;
    let names = ids.into_iter().map(|(name, id)| (id, name)).collect();
    let settings = Settings {
        port: file.port.clone().unwrap_or_default(),
        interval: file.interval.clone().unwrap_or_default(),
        requirements: (!file.optional.is_empty()).then(|| Requirements { optional: Some(file.optional.clone()), ..Default::default() }),
        browsers: file.browsers.clone().filter(|b| !b.is_empty()),
        ..Default::default()
    };
    Ok(Compiled { name: file.name, doc: b.finish(settings), names })
}

// ── writing ──────────────────────────────────────────────────────────────────────────────────

const KEYWORDS: &[&str] = &["node", "holon", "flow", "port", "interval", "browsers", "optional", "at", "in", "out"];

fn plain(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/'))
}
fn json(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}
/// A name or value as text: bare when it is a plain word, else quoted.
fn q(s: &str) -> String {
    if plain(s) && !KEYWORDS.contains(&s) { s.into() } else { json(s) }
}
/// A port's name as text: bare when a plain word (keywords are fine after a dot), else quoted.
fn qp(s: &str) -> String {
    if plain(s) { s.into() } else { json(s) }
}
/// Names made unique, in order: a repeated one gets _2, _3…
fn unique(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .map(|n| {
            let mut m = n.clone();
            let mut k = 2;
            while seen.contains(&m) {
                m = format!("{n}_{k}");
                k += 1;
            }
            seen.insert(m.clone());
            m
        })
        .collect()
}
/// A name as an identifier: anything but letters, digits and _ made _ (per UTF-16 unit, as the
/// format was first written), a leading digit guarded.
fn ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_string() } else { "_".repeat(c.len_utf16()) }).collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() { "node".into() } else { out }
}
/// `Math.round`: halves up.
fn js_round(x: f64) -> i64 {
    let f = x.floor();
    (if x - f >= 0.5 { f + 1.0 } else { f }) as i64
}

struct Printer<'s> {
    store: &'s MapResolver,
    names: Names,
    holon_names: HashMap<String, String>,
    used: HashSet<String>,
    blocks: Vec<String>,
}

impl Printer<'_> {
    /// The holon `hash`, written once (the ones inside before the ones that hold them): its name.
    fn holon_for(&mut self, hash: &str, shown_in: &[String], shown_out: &[String], hint: &str) -> Result<String, String> {
        if let Some(known) = self.holon_names.get(hash) {
            return Ok(known.clone());
        }
        let mut hname = ident(self.names.get(hash).map(String::as_str).unwrap_or(hint));
        while self.used.contains(&hname) || interface_of(&hname).is_some() || templates::is_template(&hname) {
            hname.push('_');
        }
        self.used.insert(hname.clone());
        self.holon_names.insert(hash.into(), hname.clone());
        let (_, unit) = resolve(self.store, hash)?;
        let unit = unit.clone();
        // A js node has no netlist spelling (its engine is a module, not a leaf): it is written
        // as its statement, which any file may hold.
        if crate::lang::print::is_js_node(self.store, &unit) {
            let d_ins = unique(unit.inputs.iter().enumerate().map(|(k, u)| shown_in.get(k).cloned().unwrap_or_else(|| u.name.clone())).collect());
            let d_outs = unique(unit.outputs.iter().enumerate().map(|(k, u)| shown_out.get(k).cloned().unwrap_or_else(|| u.name.clone())).collect());
            if let Some(stmt) = crate::lang::print::js_statement(self.store, &unit, &hname, &d_ins, &d_outs) {
                self.blocks.push(stmt);
                return Ok(hname);
            }
        }
        let ports = |ps: &[holon_core::Port]| ps.iter().map(WPort::from_core).collect::<Vec<_>>();
        // Open it as the expand view does, its boundary named as the unit names it.
        let target = Node::placed(
            "@holon",
            Position::default(),
            NodeData::new(&hname, PlacedBody::Composite { hash: hash.into() }, ports(&unit.inputs), ports(&unit.outputs), vec![]),
        );
        let opened = inner_canvas(&target, std::slice::from_ref(&target), &[], self.store, &self.names).map_err(|e| format!("the holon \"{hname}\" can't be opened: {e}"))?;
        let (lines, names) = self.write_body(&opened.children, &opened.wires, "  ")?;
        // The boundary, from the unit's own edges, by position (its names may repeat): each port
        // named uniquely in the text, the node's name for it where it has one, the unit's in quotes.
        let unit_ins: Vec<String> = unit.inputs.iter().map(|p| p.name.clone()).collect();
        let unit_outs: Vec<String> = unit.outputs.iter().map(|p| p.name.clone()).collect();
        let d_ins = unique(unit_ins.iter().enumerate().map(|(k, u)| shown_in.get(k).cloned().unwrap_or_else(|| u.clone())).collect());
        let d_outs = unique(unit_outs.iter().enumerate().map(|(k, u)| shown_out.get(k).cloned().unwrap_or_else(|| u.clone())).collect());
        let port = |shown: &str, unit_name: &str| if shown != unit_name { format!("{} {}", qp(shown), json(unit_name)) } else { qp(unit_name) };
        let shown = |list: &[String], k: u32| list.get(k as usize).map(|s| qp(s)).unwrap_or_else(|| "undefined".into());
        let mut boundary = Vec::new();
        if let UnitBody::Composite { edges, .. } = &unit.body {
            for e in edges {
                if e.from_child == BOUNDARY {
                    if let Some(c) = opened.children.get(e.to_child as usize) {
                        let input = c.data.inputs.get(e.to_port as usize).map(|p| p.name.as_str()).unwrap_or("");
                        boundary.push(format!("  in.{} -> {}.{}", shown(&d_ins, e.from_port), names[&c.id], qp(input)));
                    }
                } else if e.to_child == BOUNDARY
                    && let Some(c) = opened.children.get(e.from_child as usize)
                {
                    let output = c.data.outputs.get(e.from_port as usize).map(|p| p.name.as_str()).unwrap_or("");
                    boundary.push(format!("  {}.{} -> out.{}", names[&c.id], qp(output), shown(&d_outs, e.to_port)));
                }
            }
        }
        let ins: Vec<String> = unit_ins.iter().enumerate().map(|(k, u)| port(&d_ins[k], u)).collect();
        let outs: Vec<String> = unit_outs.iter().enumerate().map(|(k, u)| port(&d_outs[k], u)).collect();
        let mut block = vec![format!("holon {hname}({}) -> ({}) {{", ins.join(", "), outs.join(", "))];
        block.extend(lines);
        block.extend(boundary);
        block.push("}".into());
        self.blocks.push(block.join("\n"));
        Ok(hname)
    }

    /// Nodes and the wires between them, each node named for the text.
    fn write_body(&mut self, nodes: &[Node], wires: &[Wire], indent: &str) -> Result<(Vec<String>, HashMap<String, String>), String> {
        let mut names = HashMap::new();
        let mut taken: HashSet<String> = ["in", "out"].into_iter().map(String::from).collect();
        let mut lines = Vec::new();
        for n in nodes {
            let d = &n.data;
            let kind = match &d.body {
                PlacedBody::Composite { hash } => {
                    let shown = |ps: &[WPort]| ps.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
                    self.holon_for(hash, &shown(&d.inputs), &shown(&d.outputs), &d.name)?
                }
                _ => d.name.clone(),
            };
            let base = ident(if matches!(d.body, PlacedBody::Composite { .. }) { &kind } else { &d.name });
            let mut nname = base.clone();
            let mut k = 2;
            while taken.contains(&nname) {
                nname = format!("{base}{k}");
                k += 1;
            }
            taken.insert(nname.clone());
            names.insert(n.id.clone(), nname.clone());
            // Params that differ from the leaf's or template's own; a literal's type and value always.
            let catalog = interface_of(&d.name);
            let defaults: Vec<WParam> = match d.body {
                PlacedBody::Template => templates::defaults(&d.name).unwrap_or_default(),
                PlacedBody::Primitive => catalog.as_ref().map(|u| u.params.iter().map(WParam::from_core).collect()).unwrap_or_default(),
                PlacedBody::Composite { .. } | PlacedBody::Module { .. } => vec![],
            };
            let shown: Vec<String> = d
                .params
                .iter()
                .filter_map(|p| match p {
                    WParam::Frozen { name, value } => {
                        let keep = d.name == "const"
                            || d.name == "default"
                            || match defaults.iter().find(|x| x.name() == name) {
                                Some(WParam::Frozen { value: v, .. }) => v != value,
                                _ => true,
                            };
                        keep.then(|| format!("{}: {}", q(name), q(value)))
                    }
                    WParam::Open { .. } => None,
                })
                .collect();
            let declared = catalog.map(|u| u.inputs.len()).unwrap_or(0);
            let arity = if d.body == PlacedBody::Primitive && d.variadic && d.inputs.len() > declared { format!("[{}]", d.inputs.len()) } else { String::new() };
            let pos = n.position.map(|p| format!(" at {} {}", js_round(p.x), js_round(p.y))).unwrap_or_default();
            let params = if shown.is_empty() { String::new() } else { format!(" {{ {} }}", shown.join(", ")) };
            lines.push(format!("{indent}node {nname} = {}{arity}{params}{pos}", q(&kind)));
        }
        for e in wires {
            let (Some(a), Some(b)) = (names.get(&e.source), names.get(&e.target)) else { continue };
            lines.push(format!("{indent}{a}.{} -> {b}.{}", qp(e.output()), qp(e.input())));
        }
        Ok((lines, names))
    }
}

/// The units a document carries put into `store` (one that can't be read is skipped: a preimage
/// is re-hashed on insert, so a tampered entry can only add a unit nobody points at), and the
/// names its composites go by: `known` ones first, then the document's.
pub fn restore(doc: &FlowDocument, store: &mut MapResolver, known: &Names) -> Names {
    for hex in doc.units.iter().flatten() {
        let bytes: Option<Vec<u8>> = (hex.len() % 2 == 0).then(|| (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()).flatten();
        if let Some(unit) = bytes.and_then(|b| decode_preimage(&b).ok()) {
            let _ = store.insert(unit);
        }
    }
    let mut names = known.clone();
    for (hash, name) in doc.names.iter().flatten() {
        if !names.contains_key(hash) && !name.is_empty() && hash.get(..8) != Some(name.as_str()) {
            names.insert(hash.clone(), name.clone());
        }
    }
    names
}

/// A flow document as `.holon` text: its settings, its holons (each with its boundary given, so it
/// compiles back to the same unit), its nodes and wires. Positions are kept (`at`). The document's
/// units are put into `store` first; `known` names composites the document doesn't.
pub fn print(name: Option<&str>, doc: &FlowDocument, store: &mut MapResolver, known: &Names) -> Result<String, String> {
    let names = restore(doc, store, known);
    let mut out = Vec::new();
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        out.push(format!("flow {}", q(name)));
    }
    if let Some(s) = &doc.server {
        if !s.port.is_empty() {
            out.push(format!("port {}", q(&s.port)));
        }
        if !s.interval.is_empty() {
            out.push(format!("interval {}", q(&s.interval)));
        }
        if let Some(b) = s.browsers.as_ref().filter(|b| !b.is_empty()) {
            let origins: Vec<String> = b.split(|c: char| is_space(c) || c == ',').filter(|o| !o.is_empty()).map(q).collect();
            out.push(format!("browsers {}", origins.join(" ")));
        }
        for o in s.requirements.as_ref().and_then(|r| r.optional.as_ref()).into_iter().flatten() {
            out.push(format!("optional {}", json(o)));
        }
    }
    let mut p = Printer { store, names, holon_names: HashMap::new(), used: HashSet::new(), blocks: vec![] };
    let (top, _) = p.write_body(&doc.nodes, &doc.edges, "")?;
    if p.blocks.is_empty() {
        out.push(String::new());
    } else {
        out.push(String::new());
        for h in p.blocks {
            out.push(h);
            out.push(String::new());
        }
    }
    out.extend(top);
    let mut text = out.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    Ok(format!("{}\n", text.trim_end_matches(is_space)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = r#"flow hello-api
port 8101

node req   = request { method: GET, path: "/hello/:name" }
node name  = field { name: name }
node hi    = const { value: "Hello, ", type: Text }
node greet = concat
node res   = respond { status: "200", missing: "404" }

req.params  -> name.record
hi.out      -> greet.a
name.out    -> greet.b
req.request -> res.request
greet.out   -> res.body
"#;

    const JOINED: &str = r#"flow joined
holon shout(words "join · list") -> (said) {
  node j = join
  node up = uppercase
  j.out -> up.s
  in.words -> j.list
  up.out -> out.said
}
node c = concat[3]
node s = shout
"#;

    /// Printed, then compiled again in a fresh store, a text gives the same document and prints
    /// the same.
    fn round_trip(text: &str) {
        let first = compile(text).unwrap();
        let printed = print(first.name.as_deref(), &first.doc, &mut MapResolver::default(), &Names::new()).unwrap();
        let back = compile(&printed).unwrap();
        assert_eq!(back.doc.nodes, first.doc.nodes, "{printed}");
        assert_eq!(back.doc.edges, first.doc.edges, "{printed}");
        let again = print(back.name.as_deref(), &back.doc, &mut MapResolver::default(), &Names::new()).unwrap();
        assert_eq!(again, printed);
    }

    #[test]
    fn compiles_and_prints_back() {
        let hello = compile(HELLO).unwrap();
        assert_eq!(hello.name.as_deref(), Some("hello-api"));
        assert_eq!(hello.doc.nodes.len(), 5);
        assert_eq!(hello.doc.edges.len(), 5);
        assert_eq!(hello.names.values().filter(|n| *n == "greet").count(), 1);
        round_trip(HELLO);
        round_trip(JOINED);
    }

    #[test]
    fn variadic_nodes_and_holon_boundaries_keep_their_ports() {
        let joined = compile(JOINED).unwrap();
        let inputs = |name: &str| joined.doc.nodes.iter().find(|n| n.data.name == name).unwrap().data.inputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(inputs("concat"), ["a", "b", "c"]);
        assert_eq!(inputs("shout"), ["words"]);
        let printed = print(Some("joined"), &joined.doc, &mut MapResolver::default(), &Names::new()).unwrap();
        assert!(printed.contains(r#"holon shout(words "join · list") -> (said"#), "{printed}");
    }

    #[test]
    fn names_the_line_of_each_mistake() {
        let fails = |text: &str| compile(text).unwrap_err().to_string();
        assert_eq!(fails("node a = fieldd"), r#"line 1: no leaf "fieldd" in the catalog"#);
        assert_eq!(fails("node a = field { nam: x }"), r#"line 1: field has no param "nam""#);
        assert_eq!(fails("node a = field\nnode b = concat\na.nope -> b.a"), r#"line 3: field has no output "nope""#);
        assert!(fails("node a = field\nx.out -> a.record").starts_with(r#"line 2: no node "x""#));
        assert_eq!(fails(r#"node a = const { value: "oops }"#), "line 1: a quote isn't closed");
        assert_eq!(fails("holon h() -> () {\nnode a = field"), r#"line 1: the holon "h" isn't closed with }"#);
        assert_eq!(fails("node a = field\nnode a = field"), r#"line 2: there is a node "a" already"#);
        assert!(fails("node in = field").contains("names a holon's boundary"));
    }
}
