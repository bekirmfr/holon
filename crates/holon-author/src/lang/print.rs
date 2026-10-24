//! The printer: a flow document as canonical expression-form text (docs/LANGUAGE.md §11). The
//! other direction of lower.rs: what `holon fmt` writes, and how a canvas is exported as text.
//!
//! Headers first, then every holon the flow places (inner before outer, each once), then the
//! routes in the order they derive, or a flow without routes as its statements. Within a block,
//! nodes come in topological order; a node whose one output has exactly one consumer prints
//! inline, as a nested expression, at its consumer; a node with several consumers, or several
//! outputs, gets a binding; an effect nobody consumes prints as a statement, in the assignment
//! form where there is one. Each leaf prints as its sugar (§5.4) when it has one.
//!
//! What the expression form can't spell (a group with a request node that isn't a route, nodes
//! beside the routes) prints in the wiring form, which the parser still reads. The printer never
//! changes the graph: `compile(print(doc))` carries the same unit hashes as `doc`, and printing
//! is idempotent through compiling.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use holon_core::{interface_of, MapResolver, Resolver, UnitBody, BOUNDARY};

use crate::builder::FlowDocument;
use crate::canvas::{Node, PlacedBody};
use crate::composites::{inner_canvas, resolve, Names};
use crate::routes::{components, find_routes};
use crate::templates;
use crate::text::restore;
use crate::wire::{WParam, WPort, WType};

/// The words a binding may not take, as lower.rs holds them.
const RESERVED: &[&str] = &[
    "flow", "port", "every", "browsers", "optional", "collection", "holon", "route", "resumes", "respond", "delete", "await", "if", "then", "else",
    "when", "fn", "true", "false", "node", "at", "in", "out", "interval", "secret", "state", "now", "random", "ingress", "http", "stripe", "json",
    "parse_json",
];

/// Precedence of a printed expression, for parentheses: higher binds tighter.
const ATOM: u8 = 100;
const PREC_IF: u8 = 0;
const PREC_WHEN: u8 = 1;
const PREC_COALESCE: u8 = 2;
const PREC_OR: u8 = 3;
const PREC_AND: u8 = 4;
const PREC_EQ: u8 = 5;
const PREC_CMP: u8 = 6;
const PREC_ADD: u8 = 7;
const PREC_MUL: u8 = 8;
const PREC_NOT: u8 = 9;

/// A plain word: what prints bare as a name, a param value or a record key.
fn plain(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// A text as a string literal, braces doubled since a string interpolates.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A param's value as written: bare when a plain word or a number, else quoted.
fn param_value(v: &str) -> String {
    if (plain(v) && !RESERVED.contains(&v)) || (v.parse::<f64>().is_ok() && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')) {
        v.to_string()
    } else {
        quote(v)
    }
}

/// A number of seconds as a duration when it divides evenly, else as seconds.
fn duration(text: &str) -> String {
    let Ok(n) = text.parse::<f64>() else { return param_value(text) };
    if n <= 0.0 || n.fract() != 0.0 {
        return param_value(text);
    }
    for (per, unit) in [(86400.0, 'd'), (3600.0, 'h'), (60.0, 'm')] {
        if n % per == 0.0 {
            return format!("{}{unit}", (n / per) as i64);
        }
    }
    format!("{}s", n as i64)
}

/// A JSON text as a literal of the language, when writing it back compact gives the same text
/// (so the hash survives); `None` otherwise.
fn json_literal(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if serde_json::to_string(&v).ok()? != text {
        return None;
    }
    fn render(v: &serde_json::Value) -> Option<String> {
        Some(match v {
            serde_json::Value::Null => return None,
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => quote(s),
            serde_json::Value::Array(items) => format!("[{}]", items.iter().map(render).collect::<Option<Vec<_>>>()?.join(", ")),
            serde_json::Value::Object(fields) => {
                if fields.is_empty() {
                    return Some("{}".into());
                }
                let parts = fields.iter().map(|(k, v)| Some(format!("{}: {}", if plain(k) && !RESERVED.contains(&k.as_str()) { k.clone() } else { quote(k) }, render(v)?))).collect::<Option<Vec<_>>>()?;
                format!("{{ {} }}", parts.join(", "))
            }
        })
    }
    render(&v)
}

/// A `const` or `default` literal from its frozen type and value, or `None` when it has no
/// literal spelling.
fn literal(ty: &str, value: &str) -> Option<String> {
    match ty {
        "Number" => value.parse::<f64>().ok().filter(|n| n.is_finite()).map(|_| value.to_string()),
        "Text" => Some(quote(value)),
        "Boolean" => (value == "true" || value == "false").then(|| value.to_string()),
        "Json" => json_literal(value),
        _ => None,
    }
}

/// A value's source within a block.
#[derive(Clone, Debug, PartialEq)]
enum Src {
    Node { id: String, output: String },
    /// A holon's or function's input, by its declared name.
    In(String),
}

/// A connected set of nodes printed as one block: a route's, a holon's, or the whole flow's.
struct Group<'g> {
    nodes: Vec<&'g Node>,
    incoming: HashMap<(String, String), Src>,
    consumers: HashMap<(String, String), usize>,
    /// Boundary outputs: `(node, output, port)`.
    outs: Vec<(String, String, String)>,
    order: Vec<String>,
}

impl<'g> Group<'g> {
    fn new(nodes: Vec<&'g Node>, wires: Vec<(Src, String, String)>, outs: Vec<(String, String, String)>) -> Group<'g> {
        let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let mut incoming = HashMap::new();
        let mut consumers: HashMap<(String, String), usize> = HashMap::new();
        let mut edges: Vec<(String, String)> = Vec::new();
        for (src, to, input) in wires {
            if !ids.contains(to.as_str()) {
                continue;
            }
            if let Src::Node { id, output } = &src {
                if !ids.contains(id.as_str()) {
                    continue;
                }
                *consumers.entry((id.clone(), output.clone())).or_default() += 1;
                edges.push((id.clone(), to.clone()));
            }
            incoming.insert((to, input), src);
        }
        for (id, output, _) in &outs {
            *consumers.entry((id.clone(), output.clone())).or_default() += 1;
        }
        // Kahn's order, ties by the nodes' own order.
        let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let mut indeg = vec![0usize; nodes.len()];
        let mut succ: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
        for (from, to) in &edges {
            let (f, t) = (index[from.as_str()], index[to.as_str()]);
            indeg[t] += 1;
            succ[f].push(t);
        }
        let mut ready: Vec<usize> = (0..nodes.len()).filter(|&i| indeg[i] == 0).collect();
        let mut order = Vec::new();
        while !ready.is_empty() {
            ready.sort_unstable_by(|a, b| b.cmp(a));
            let i = ready.pop().unwrap();
            order.push(nodes[i].id.clone());
            for &s in &succ[i] {
                indeg[s] -= 1;
                if indeg[s] == 0 {
                    ready.push(s);
                }
            }
        }
        Group { nodes, incoming, consumers, outs, order }
    }

    fn node(&self, id: &str) -> &'g Node {
        self.nodes.iter().find(|n| n.id == id).expect("a node of the group")
    }

    fn consumed(&self, id: &str, output: &str) -> usize {
        self.consumers.get(&(id.to_string(), output.to_string())).copied().unwrap_or(0)
    }

    fn consumed_total(&self, node: &Node) -> usize {
        node.data.outputs.iter().map(|o| self.consumed(&node.id, &o.name)).sum()
    }
}

pub struct Printer<'s> {
    store: &'s MapResolver,
    names: Names,
    bindings: BTreeMap<String, String>,
    holon_names: HashMap<String, String>,
    taken: HashSet<String>,
    blocks: Vec<Vec<Line>>,
    collections: BTreeSet<String>,
    holon_defs: HashSet<String>,
}

/// A line of the text, and the top-level nodes it stands for (a statement's nodes, a route
/// header's request, a `node` line's node).
type Line = (String, Vec<String>);

/// A flow document as canonical text. `known` names composites the document doesn't;
/// `bindings` names top-level nodes (a lowering's binding names, so a text keeps its names
/// through `fmt`). The document's units are put into `store` first.
pub fn print(name: Option<&str>, doc: &FlowDocument, store: &mut MapResolver, known: &Names, bindings: &BTreeMap<String, String>) -> Result<String, String> {
    print_lines(name, doc, store, known, bindings).map(|(text, _)| text)
}

/// [`print`], and the line (1-based) each top-level node is written on: an inline node's is its
/// consumer's statement, a route's request node's is the route header. What view mode shows
/// beside a selected node.
pub fn print_lines(name: Option<&str>, doc: &FlowDocument, store: &mut MapResolver, known: &Names, bindings: &BTreeMap<String, String>) -> Result<(String, BTreeMap<String, u32>), String> {
    let names = restore(doc, store, known);
    let mut p = Printer {
        store,
        names,
        bindings: bindings.clone(),
        holon_names: HashMap::new(),
        taken: HashSet::new(),
        blocks: vec![],
        collections: BTreeSet::new(),
        holon_defs: HashSet::new(),
    };
    p.collect_collections(&doc.nodes)?;
    let mut out: Vec<Line> = Vec::new();
    let header = |s: String| (s, vec![]);
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        out.push(header(format!("flow {}", if plain(name) || name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { name.to_string() } else { quote(name) })));
    }
    if let Some(s) = &doc.server {
        if !s.port.is_empty() {
            out.push(header(format!("port {}", param_value(&s.port))));
        }
        if !s.interval.is_empty() {
            out.push(header(format!("every {}", duration(&s.interval))));
        }
        if let Some(b) = s.browsers.as_ref().filter(|b| !b.is_empty()) {
            let origins: Vec<&str> = b.split(|c: char| c.is_whitespace() || c == ',').filter(|o| !o.is_empty()).collect();
            out.push(header(format!("browsers {}", origins.join(", "))));
        }
        for o in s.requirements.as_ref().and_then(|r| r.optional.as_ref()).into_iter().flatten() {
            out.push(header(format!("optional {}", consent(o))));
        }
    }
    for c in &p.collections {
        out.push(header(format!("collection {c}")));
    }
    let body = p.flow_body(doc)?;
    let blocks = std::mem::take(&mut p.blocks);
    for block in blocks {
        if !out.is_empty() {
            out.push(header(String::new()));
        }
        out.extend(block);
    }
    if !body.is_empty() {
        if !out.is_empty() {
            out.push(header(String::new()));
        }
        out.extend(body);
    }
    let mut lines = BTreeMap::new();
    for (k, (_, ids)) in out.iter().enumerate() {
        for id in ids {
            lines.entry(id.clone()).or_insert(k as u32 + 1);
        }
    }
    let text: Vec<&str> = out.iter().map(|(l, _)| l.as_str()).collect();
    Ok((format!("{}\n", text.join("\n").trim_end()), lines))
}

/// Whether the expression form can spell `nodes` as wired: every input a node needs is wired
/// (the lowering refuses one left out), and nothing but a route's request has a hidden input.
fn expressible(nodes: &[&Node], incoming: &HashMap<(String, String), Src>, request: Option<&str>) -> bool {
    nodes.iter().all(|n| {
        if request == Some(n.id.as_str()) {
            return true;
        }
        if n.is_template("request") {
            return false;
        }
        n.data.inputs.iter().all(|p| {
            let optional = n.data.body == PlacedBody::Primitive
                && matches!((n.data.name.as_str(), p.name.as_str()), ("http_request", "headers" | "body") | ("collection_list", "prefix" | "after"));
            optional || (n.is_template("respond") && p.name == "body") || incoming.contains_key(&(n.id.clone(), p.name.clone()))
        })
    })
}

/// A consent name as `optional` writes it: a secret by its name, else the server's form.
fn consent(o: &str) -> String {
    if let Some(rest) = o.strip_prefix("SecretRead(\"")
        && let Some(name) = rest.strip_suffix("\")")
        && plain(name)
    {
        return format!("secret.{name}");
    }
    o.to_string()
}

impl<'s> Printer<'s> {
    /// Every collection any leaf names, the holons' leaves included.
    fn collect_collections(&mut self, nodes: &[Node]) -> Result<(), String> {
        fn walk(store: &MapResolver, hash: &holon_core::Hash, seen: &mut HashSet<holon_core::Hash>, out: &mut BTreeSet<String>) {
            if !seen.insert(*hash) {
                return;
            }
            let Some(unit) = store.resolve(hash) else { return };
            match &unit.body {
                UnitBody::Primitive { name } if name.starts_with("collection_") => {
                    if let Some(c) = unit.params.iter().find_map(|p| match p {
                        holon_core::Param::Frozen { name, value } if name == "collection" => Some(value.clone()),
                        _ => None,
                    }) {
                        out.insert(c);
                    }
                }
                UnitBody::Composite { children, .. } => {
                    for c in children {
                        walk(store, c, seen, out);
                    }
                }
                _ => {}
            }
        }
        let mut seen = HashSet::new();
        for n in nodes {
            match &n.data.body {
                PlacedBody::Primitive if n.data.name.starts_with("collection_") => {
                    let c = n.param("collection");
                    if !c.is_empty() {
                        self.collections.insert(c.to_string());
                    }
                }
                PlacedBody::Composite { hash } => {
                    let (h, _) = resolve(self.store, hash)?;
                    walk(self.store, &h, &mut seen, &mut self.collections);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The top of the flow: its routes, a flow without routes as statements, and whatever the
    /// expression form can't spell in the wiring form.
    fn flow_body(&mut self, doc: &FlowDocument) -> Result<Vec<Line>, String> {
        let (routes, errors) = find_routes(&doc.nodes, &doc.edges);
        let has_request = doc.nodes.iter().any(|n| n.is_template("request"));
        let mut lines: Vec<Line> = Vec::new();
        if !has_request {
            // One block, the flow's own.
            let group = self.group_of(doc.nodes.iter().collect(), &doc.edges, vec![]);
            if !expressible(&group.nodes, &group.incoming, None) {
                return self.wiring_form(&group.nodes, doc.edges.iter(), "").map(|(l, _)| l);
            }
            let mut body = Block::new(self, &group, "", None);
            lines.extend(body.statements()?);
            return Ok(lines);
        }
        let ids: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
        let by_id: HashMap<&str, &Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let mut routed: HashSet<String> = HashSet::new();
        let mut stray_routes: Vec<String> = Vec::new();
        for r in &routes {
            let nodes: Vec<&Node> = r.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
            let request = by_id[r.request_id.as_str()];
            let group = self.group_of(nodes, &doc.edges, vec![]);
            if !expressible(&group.nodes, &group.incoming, Some(&request.id)) {
                stray_routes.push(format!("{} {}: an input isn't wired, so the route is written as nodes and wires", r.method, r.path));
                continue;
            }
            let req_name = self.request_name(request);
            let mut head = format!("route {} {} ({req_name})", r.method, r.path);
            if !r.resume.is_empty() {
                head.push_str(&format!(" resumes {}", r.resume));
            }
            head.push_str(" {");
            if !lines.is_empty() {
                lines.push((String::new(), vec![]));
            }
            lines.push((head, vec![request.id.clone()]));
            let mut body = Block::new(self, &group, "  ", Some((request.id.clone(), req_name)));
            lines.extend(body.statements()?);
            lines.push(("}".into(), vec![]));
            routed.extend(r.node_ids.iter().cloned());
        }
        // Groups that aren't routes written above: the wiring form, with why as comments.
        let stray: Vec<Vec<String>> = components(&ids, &doc.edges).into_iter().filter(|g| !g.iter().any(|id| routed.contains(id))).collect();
        if !stray.is_empty() {
            if !lines.is_empty() {
                lines.push((String::new(), vec![]));
            }
            for e in errors.iter().chain(&stray_routes) {
                lines.push((format!("# {e}"), vec![]));
            }
            let nodes: Vec<&Node> = stray.iter().flatten().map(|id| by_id[id.as_str()]).collect();
            let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
            let (wired, _) = self.wiring_form(&nodes, doc.edges.iter().filter(|w| node_ids.contains(w.source.as_str()) && node_ids.contains(w.target.as_str())), "")?;
            lines.extend(wired);
        }
        Ok(lines)
    }

    fn group_of<'g>(&self, nodes: Vec<&'g Node>, wires: &[crate::canvas::Wire], outs: Vec<(String, String, String)>) -> Group<'g> {
        let ws = wires.iter().map(|w| (Src::Node { id: w.source.clone(), output: w.output().to_string() }, w.target.clone(), w.input().to_string())).collect();
        Group::new(nodes, ws, outs)
    }

    fn request_name(&self, request: &Node) -> String {
        let n = self.bindings.get(&request.id).cloned().unwrap_or_else(|| "req".into());
        if plain(&n) && !RESERVED.contains(&n.as_str()) && interface_of(&n).is_none() && !templates::is_template(&n) && !self.collections.contains(&n) { n } else { "req".into() }
    }

    /// A name for a holon, by hash, taken once, from the document's names or made up; the holon's
    /// block is written the first time it is asked for.
    fn holon_for(&mut self, hash: &str, shown_in: &[String], shown_out: &[String], hint: &str) -> Result<String, String> {
        if let Some(known) = self.holon_names.get(hash) {
            return Ok(known.clone());
        }
        let mut hname = self.names.get(hash).cloned().filter(|n| plain(n)).unwrap_or_else(|| {
            let h: String = hint.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
            if h.is_empty() || h.starts_with(|c: char| c.is_ascii_digit()) { format!("holon_{h}") } else { h }
        });
        while self.taken.contains(&hname) || self.reserved(&hname) {
            hname.push('_');
        }
        self.taken.insert(hname.clone());
        self.holon_names.insert(hash.into(), hname.clone());
        self.holon_defs.insert(hname.clone());
        let block = self.holon_block(hash, &hname, shown_in, shown_out)?;
        self.blocks.push(block);
        Ok(hname)
    }

    fn reserved(&self, n: &str) -> bool {
        RESERVED.contains(&n) || interface_of(n).is_some() || templates::is_template(n) || self.collections.contains(n)
    }

    /// `holon name(inputs) -> (outputs) { body }`, the body as the expand view opens it: the
    /// unit's children as nodes, its edges as wires, its boundary as the block's inputs and output
    /// bindings. Holons inside are written first.
    fn holon_block(&mut self, hash: &str, hname: &str, shown_in: &[String], shown_out: &[String]) -> Result<Vec<Line>, String> {
        let (_, unit) = resolve(self.store, hash)?;
        let unit = unit.clone();
        let ports = |ps: &[holon_core::Port]| ps.iter().map(WPort::from_core).collect::<Vec<_>>();
        let target = Node::placed(
            "@holon",
            Default::default(),
            crate::canvas::NodeData::new(hname, PlacedBody::Composite { hash: hash.into() }, ports(&unit.inputs), ports(&unit.outputs), vec![]),
        );
        let opened = inner_canvas(&target, std::slice::from_ref(&target), &[], self.store, &self.names).map_err(|e| format!("the holon \"{hname}\" can't be opened: {e}"))?;
        // Declared names: the shown ones where given, else the unit's, made unique and plain.
        let declare = |unit_names: &[String], shown: &[String], used: &mut HashSet<String>| -> Vec<(String, String)> {
            unit_names
                .iter()
                .enumerate()
                .map(|(k, u)| {
                    let want = shown.get(k).cloned().unwrap_or_else(|| u.clone());
                    let mut base: String = want.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
                    if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
                        base = format!("p{base}");
                    }
                    let mut name = base.clone();
                    let mut n = 2;
                    while used.contains(&name) || self.reserved(&name) {
                        name = format!("{base}_{n}");
                        n += 1;
                    }
                    used.insert(name.clone());
                    (name, u.clone())
                })
                .collect()
        };
        let mut used = HashSet::new();
        let unit_ins: Vec<String> = unit.inputs.iter().map(|p| p.name.clone()).collect();
        let unit_outs: Vec<String> = unit.outputs.iter().map(|p| p.name.clone()).collect();
        let ins = declare(&unit_ins, shown_in, &mut used);
        let outs = declare(&unit_outs, shown_out, &mut used);
        let port = |(name, unit): &(String, String)| if name == unit { name.clone() } else { format!("{name} {}", quote(unit)) };
        // The boundary, from the unit's own edges.
        let mut wires: Vec<(Src, String, String)> = opened.wires.iter().map(|w| (Src::Node { id: w.source.clone(), output: w.output().to_string() }, w.target.clone(), w.input().to_string())).collect();
        let mut bouts: Vec<(String, String, String)> = Vec::new();
        if let UnitBody::Composite { edges, .. } = &unit.body {
            for e in edges {
                if e.from_child == BOUNDARY {
                    if let Some(c) = opened.children.get(e.to_child as usize) {
                        let input = c.data.inputs.get(e.to_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                        let port_name = ins.get(e.from_port as usize).map(|(n, _)| n.clone()).unwrap_or_default();
                        wires.push((Src::In(port_name), c.id.clone(), input));
                    }
                } else if e.to_child == BOUNDARY
                    && let Some(c) = opened.children.get(e.from_child as usize)
                {
                    let output = c.data.outputs.get(e.from_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                    let port_name = outs.get(e.to_port as usize).map(|(n, _)| n.clone()).unwrap_or_default();
                    bouts.push((c.id.clone(), output, port_name));
                }
            }
        }
        let group = Group::new(opened.children.iter().collect(), wires, bouts);
        let outs_text = if outs.len() == 1 { port(&outs[0]) } else { format!("({})", outs.iter().map(port).collect::<Vec<_>>().join(", ")) };
        let mut lines: Vec<Line> = vec![(format!("holon {hname}({}) -> {outs_text} {{", ins.iter().map(port).collect::<Vec<_>>().join(", ")), vec![])];
        if expressible(&group.nodes, &group.incoming, None) {
            let inputs: HashSet<String> = ins.iter().map(|(n, _)| n.clone()).collect();
            let mut block = Block::new(self, &group, "  ", None);
            block.inputs = inputs;
            lines.extend(block.statements()?);
        } else {
            // The wiring form inside: nodes, wires, and the boundary as `in.x -> n.p`, `n.p -> out.y`.
            let (wired, node_names) = self.wiring_form(&group.nodes, opened.wires.iter(), "  ")?;
            lines.extend(wired);
            for ((to, input), src) in &group.incoming {
                if let Src::In(port) = src
                    && let Some(n) = node_names.get(to)
                {
                    lines.push((format!("  in.{port} -> {n}.{}", if plain(input) { input.clone() } else { quote(input) }), vec![]));
                }
            }
            for (node, output, port) in &group.outs {
                if let Some(n) = node_names.get(node) {
                    lines.push((format!("  {n}.{} -> out.{port}", if plain(output) { output.clone() } else { quote(output) }), vec![]));
                }
            }
        }
        lines.push(("}".into(), vec![]));
        Ok(lines)
    }

    /// Nodes and wires as the wiring form writes them (no positions): the fallback. Answers the
    /// lines and each node's name in them.
    fn wiring_form<'w>(&mut self, nodes: &[&Node], wires: impl Iterator<Item = &'w crate::canvas::Wire>, indent: &str) -> Result<(Vec<Line>, HashMap<String, String>), String> {
        let mut names: HashMap<String, String> = HashMap::new();
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
            let base: String = self.bindings.get(&n.id).cloned().unwrap_or_else(|| kind.clone()).chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
            let mut nname = base.clone();
            let mut k = 2;
            while taken.contains(&nname) {
                nname = format!("{base}{k}");
                k += 1;
            }
            taken.insert(nname.clone());
            names.insert(n.id.clone(), nname.clone());
            let shown = self.changed_params(n);
            let declared = interface_of(&d.name).map(|u| u.inputs.len()).unwrap_or(0);
            let arity = if d.body == PlacedBody::Primitive && d.variadic && d.inputs.len() > declared { format!("[{}]", d.inputs.len()) } else { String::new() };
            let params = if shown.is_empty() { String::new() } else { format!(" {{ {} }}", shown.iter().map(|(k, v)| format!("{}: {}", param_value(k), param_value(v))).collect::<Vec<_>>().join(", ")) };
            lines.push((format!("{indent}node {nname} = {}{arity}{params}", if plain(&kind) { kind.clone() } else { quote(&kind) }), vec![n.id.clone()]));
        }
        for w in wires {
            let (Some(a), Some(b)) = (names.get(&w.source), names.get(&w.target)) else { continue };
            let p = |s: &str| if plain(s) { s.to_string() } else { quote(s) };
            lines.push((format!("{indent}{a}.{} -> {b}.{}", p(w.output()), p(w.input())), vec![]));
        }
        Ok((lines, names))
    }

    /// A node's params that differ from its leaf's or template's defaults (a literal's always).
    fn changed_params(&self, n: &Node) -> Vec<(String, String)> {
        let d = &n.data;
        let defaults: Vec<WParam> = match d.body {
            PlacedBody::Template => templates::defaults(&d.name).unwrap_or_default(),
            PlacedBody::Primitive => interface_of(&d.name).map(|u| u.params.iter().map(WParam::from_core).collect()).unwrap_or_default(),
            PlacedBody::Composite { .. } => vec![],
        };
        d.params
            .iter()
            .filter_map(|p| match p {
                WParam::Frozen { name, value } => {
                    let keep = d.name == "const"
                        || d.name == "default"
                        || match defaults.iter().find(|x| x.name() == name) {
                            Some(WParam::Frozen { value: v, .. }) => v != value,
                            _ => true,
                        };
                    keep.then(|| (name.clone(), value.clone()))
                }
                WParam::Open { .. } => None,
            })
            .collect()
    }
}

/// The statements of one block, and the expressions within them.
struct Block<'p, 's, 'g> {
    p: &'p mut Printer<'s>,
    g: &'g Group<'g>,
    indent: &'static str,
    /// The route's request node and its name, in a route.
    request: Option<(String, String)>,
    /// A holon's or function's inputs, by name.
    inputs: HashSet<String>,
    bound: HashMap<String, String>,
    used: HashSet<String>,
    /// The nodes rendered into the statement being written.
    touched: Vec<String>,
}

impl<'p, 's, 'g> Block<'p, 's, 'g> {
    fn new(p: &'p mut Printer<'s>, g: &'g Group<'g>, indent: &'static str, request: Option<(String, String)>) -> Block<'p, 's, 'g> {
        let mut used = HashSet::new();
        if let Some((_, name)) = &request {
            used.insert(name.clone());
        }
        Block { p, g, indent, request, inputs: HashSet::new(), bound: HashMap::new(), used, touched: vec![] }
    }

    fn line(&mut self, text: String) -> Line {
        (text, std::mem::take(&mut self.touched))
    }

    /// A node folded into the expression being written without rendering it on its own (a
    /// string's pieces, a record chain's steps, a guard an `if` absorbs): it is on this line too.
    fn touch(&mut self, n: &Node) {
        self.touched.push(n.id.clone());
    }

    fn is_request(&self, id: &str) -> bool {
        self.request.as_ref().is_some_and(|(r, _)| r == id)
    }

    /// Whether `node`'s one output prints inline at its one consumer. A node the text named keeps
    /// its binding: fmt keeps an author's names.
    fn inline(&self, node: &Node) -> bool {
        if self.is_request(&node.id) || node.is_template("respond") || node.data.name == "await" || self.bound.contains_key(&node.id) || self.p.bindings.contains_key(&node.id) {
            return false;
        }
        node.data.outputs.len() == 1 && self.g.consumed_total(node) == 1
    }

    /// The input a `guard(true, input)` passes through, when `node` is one: the lowering places it
    /// for an input used twice, and it prints as the input itself, so `n + n` comes back as written.
    fn pass_through(&self, node: &Node) -> Option<String> {
        if node.data.name != "guard" {
            return None;
        }
        let always = self.inline_source(node, "cond", "const").is_some_and(|c| c.param("type") == "Boolean" && c.param("value") == "true");
        match self.g.incoming.get(&(node.id.clone(), "value".into())) {
            Some(Src::In(port)) if always => Some(port.clone()),
            _ => None,
        }
    }

    fn statements(&mut self) -> Result<Vec<Line>, String> {
        let mut lines = Vec::new();
        let order = self.g.order.clone();
        for id in &order {
            let node = self.g.node(id);
            if self.is_request(id) || self.pass_through(node).is_some() {
                continue;
            }
            self.touched.clear();
            if node.is_template("respond") {
                let text = self.respond(node)?;
                let l = self.line(format!("{}{text}", self.indent));
                lines.push(l);
                continue;
            }
            let outs: Vec<(String, String)> = self.g.outs.iter().filter(|(n, _, _)| n == id).map(|(_, o, p)| (o.clone(), p.clone())).collect();
            if self.inline(node) {
                // Its one consumer is a boundary output: bind the output here.
                if let Some((output, port)) = outs.first() {
                    let (text, _) = self.render(node, output)?;
                    let l = self.line(format!("{}{port} = {text}", self.indent));
                    lines.push(l);
                }
                continue;
            }
            let total = self.g.consumed_total(node);
            let single = node.data.outputs.len() == 1;
            let effect = single && node.data.outputs[0].ty == WType::Effect;
            if total == 0 && effect {
                let text = self.effect_statement(node)?;
                let l = self.line(format!("{}{text}", self.indent));
                lines.push(l);
                continue;
            }
            if total == 0 && !single && node.data.outputs.iter().any(|o| o.ty == WType::Effect) {
                let (text, _) = self.render(node, "")?;
                let l = self.line(format!("{}{text}", self.indent));
                lines.push(l);
                continue;
            }
            // A binding, then any boundary outputs it feeds. Rendered first: a holon's block is
            // written (and its name taken) on first use, and the binding must not shadow it.
            let (text, _) = self.render(node, if single { &node.data.outputs[0].name } else { "" })?;
            let name = self.bind(node);
            let l = self.line(format!("{}{name} = {text}", self.indent));
            lines.push(l);
            for (output, port) in outs {
                let value = if single { name.clone() } else { format!("{name}.{output}") };
                lines.push((format!("{}{port} = {value}", self.indent), vec![]));
            }
        }
        Ok(lines)
    }

    /// A binding name for `node`: the one it had in a text, else one from what it is.
    fn bind(&mut self, node: &Node) -> String {
        let suggested = self.p.bindings.get(&node.id).cloned().unwrap_or_else(|| {
            let d = &node.data;
            match d.name.as_str() {
                "field" | "secret" => node.param("name").to_string(),
                "store_read" => node.param("key").to_string(),
                "await" => node.param("name").to_string(),
                "collection_get" => "item".into(),
                "collection_list" => "page".into(),
                "http_request" => "response".into(),
                "stripe_verify" => "verified".into(),
                "clock" => "t".into(),
                "const" => "c".into(),
                _ => match &d.body {
                    PlacedBody::Composite { .. } => d.name.clone(),
                    _ => d.name.clone(),
                },
            }
        });
        let mut base: String = suggested.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
            base = format!("v{base}");
        }
        let mut name = base.clone();
        let mut n = 2;
        while self.used.contains(&name) || self.inputs.contains(&name) || self.p.reserved(&name) || self.p.holon_defs.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.used.insert(name.clone());
        self.bound.insert(node.id.clone(), name.clone());
        name
    }

    /// The value on `node`'s `input`, as text with its precedence, or `None` when unwired.
    fn arg(&mut self, node: &Node, input: &str) -> Result<Option<(String, u8)>, String> {
        let Some(src) = self.g.incoming.get(&(node.id.clone(), input.to_string())).cloned() else { return Ok(None) };
        Ok(Some(self.value(&src)?))
    }

    /// A source as text: an input's name, a request's part, a binding, or the node inline.
    fn value(&mut self, src: &Src) -> Result<(String, u8), String> {
        match src {
            Src::In(port) => Ok((port.clone(), ATOM)),
            Src::Node { id, output } => {
                if let Some((req, name)) = &self.request
                    && req == id
                {
                    return Ok((if output == "request" { name.clone() } else { format!("{name}.{output}") }, ATOM));
                }
                if let Some(name) = self.bound.get(id).cloned() {
                    let node = self.g.node(id);
                    return Ok((if node.data.outputs.len() == 1 { name } else { format!("{name}.{output}") }, ATOM));
                }
                let node = self.g.node(id);
                if let Some(port) = self.pass_through(node) {
                    self.touch(node);
                    if let Some(c) = self.inline_source(node, "cond", "const") {
                        self.touch(c);
                    }
                    return Ok((port, ATOM));
                }
                self.render(node, output)
            }
        }
    }

    /// `arg` as an operand of an operator of precedence `prec`: parenthesised when it binds
    /// no tighter (the same operator twice must not flatten).
    fn operand(&mut self, node: &Node, input: &str, prec: u8) -> Result<String, String> {
        match self.arg(node, input)? {
            Some((text, p)) if p <= prec => Ok(format!("({text})")),
            Some((text, _)) => Ok(text),
            None => Err(format!("{}: its {input} isn't wired", node.data.name)),
        }
    }

    fn atom(&mut self, node: &Node, input: &str) -> Result<String, String> {
        self.operand(node, input, ATOM - 1)
    }

    fn required(&mut self, node: &Node, input: &str) -> Result<String, String> {
        match self.arg(node, input)? {
            Some((text, _)) => Ok(text),
            None => Err(format!("{}: its {input} isn't wired", node.data.name)),
        }
    }

    /// Whether `node`'s `input` comes from a node that prints inline, of leaf `leaf`.
    fn inline_source(&self, node: &Node, input: &str, leaf: &str) -> Option<&'g Node> {
        match self.g.incoming.get(&(node.id.clone(), input.to_string()))? {
            Src::Node { id, .. } => {
                let n = self.g.node(id);
                (n.data.name == leaf && !matches!(n.data.body, PlacedBody::Composite { .. }) && self.inline(n)).then_some(n)
            }
            Src::In(_) => None,
        }
    }

    /// `node` as an expression, with its precedence. `output` is which output, for a
    /// multi-output node referred to bare (ignored otherwise).
    fn render(&mut self, node: &Node, output: &str) -> Result<(String, u8), String> {
        self.touched.push(node.id.clone());
        let d = &node.data;
        if let PlacedBody::Composite { hash } = &d.body {
            let shown = |ps: &[WPort]| ps.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
            let hname = self.p.holon_for(hash, &shown(&d.inputs), &shown(&d.outputs), &d.name)?;
            let args = self.positional_then_named(node, &d.inputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), &[])?;
            let call = format!("{hname}({})", args.join(", "));
            if d.outputs.len() > 1 && !output.is_empty() {
                return Ok((format!("{call}.{output}"), ATOM));
            }
            return Ok((call, ATOM));
        }
        if d.body == PlacedBody::Template {
            return match d.name.as_str() {
                "stripe_verify" => {
                    let mut args = self.positional_then_named(node, &["request".into(), "secret".into(), "now".into()], &[])?;
                    let tol = node.param("tolerance");
                    if tol != "300" && !tol.is_empty() {
                        args.push(format!("tolerance: {}", duration(tol)));
                    }
                    let call = format!("stripe.verify({})", args.join(", "));
                    Ok((if output.is_empty() || d.outputs.len() == 1 { call } else { format!("{call}.{output}") }, ATOM))
                }
                other => Err(format!("a {other} node outside a route")),
            };
        }
        let atom = |s: String| Ok((s, ATOM));
        match d.name.as_str() {
            "const" => match literal(node.param("type"), node.param("value")) {
                Some(l) => atom(l),
                None => self.generic(node),
            },
            "clock" => atom("now".into()),
            "random" => atom("random".into()),
            "ingress" => atom("ingress".into()),
            "secret" if plain(node.param("name")) => atom(format!("secret.{}", node.param("name"))),
            "store_read" if plain(node.param("key")) => atom(format!("state.{}", node.param("key"))),
            "field" if plain(node.param("name")) => {
                let name = node.param("name");
                let base = self.atom(node, "record")?;
                // A field of the request record that shares a name with a request output.
                let clash = self.inline_request_record(node, "record") && ["request", "params", "query", "headers", "body", "json"].contains(&name);
                atom(if clash { format!("{base}.request.{name}") } else { format!("{base}.{name}") })
            }
            "lookup" => {
                let base = self.atom(node, "record")?;
                let key = self.required(node, "key")?;
                atom(format!("{base}[{key}]"))
            }
            "collection_get" if plain(node.param("collection")) => {
                let key = self.required(node, "key")?;
                atom(format!("{}[{key}]", node.param("collection")))
            }
            "collection_put" if plain(node.param("collection")) => {
                let key = self.required(node, "key")?;
                let value = self.required(node, "value")?;
                atom(format!("{}.put({key}, {value})", node.param("collection")))
            }
            "collection_delete" if plain(node.param("collection")) => {
                let key = self.required(node, "key")?;
                atom(format!("{}.delete({key})", node.param("collection")))
            }
            "collection_list" if plain(node.param("collection")) => {
                let mut args = Vec::new();
                for input in ["prefix", "after"] {
                    if let Some((t, _)) = self.arg(node, input)? {
                        args.push(format!("{input}: {t}"));
                    }
                }
                if node.param("limit") != "100" {
                    args.push(format!("limit: {}", param_value(node.param("limit"))));
                }
                atom(format!("{}.list({})", node.param("collection"), args.join(", ")))
            }
            "store_write" if plain(node.param("key")) => {
                let value = self.required(node, "value")?;
                atom(format!("state.set({}, {value})", node.param("key")))
            }
            "http_request" => {
                let method = node.param("method").to_ascii_lowercase();
                if !["get", "post", "put", "patch", "delete", "head"].contains(&method.as_str()) {
                    return self.generic(node);
                }
                let mut args = vec![quote(node.param("host"))];
                args.extend(self.positional_then_named(node, &["path".into(), "headers".into(), "body".into()], &[])?);
                atom(format!("http.{method}({})", args.join(", ")))
            }
            "await" => {
                let mut text = format!("await {}", node.param("name"));
                if node.param("expires") != "604800" {
                    text.push_str(&format!(", expires: {}", duration(node.param("expires"))));
                }
                if !plain(node.param("name")) {
                    return self.generic(node);
                }
                atom(text)
            }
            "resume_token" if plain(node.param("await")) => atom(format!("resume_token({})", node.param("await"))),
            "json_stringify" => atom(format!("json({})", self.required(node, "value")?)),
            "json_parse" => atom(format!("parse_json({})", self.required(node, "text")?)),
            "concat" => self.concat(node),
            "record" => atom("{}".into()),
            "with_field" => self.record(node),
            "list" => {
                let mut items = Vec::new();
                for p in &d.inputs {
                    items.push(self.required(node, &p.name)?);
                }
                atom(format!("[{}]", items.join(", ")))
            }
            "append" => {
                let empty = self.inline_source(node, "list", "const").filter(|c| c.param("type") == "Json" && c.param("value") == "[]");
                if let Some(c) = empty {
                    self.touch(c);
                    let item = self.required(node, "item")?;
                    return atom(format!("[{item}]"));
                }
                self.generic(node)
            }
            "default" => match literal(node.param("type"), node.param("value")) {
                Some(l) => {
                    if let Some(g) = self.inline_source(node, "value", "guard") {
                        self.touch(g);
                        let c = self.operand(g, "cond", PREC_IF)?;
                        let t = self.operand(g, "value", PREC_IF)?;
                        return Ok((format!("if {c} then {t} else {l}"), PREC_IF));
                    }
                    let v = self.operand(node, "value", PREC_COALESCE)?;
                    Ok((format!("{v} ?? {l}"), PREC_COALESCE))
                }
                None => self.generic(node),
            },
            "coalesce" => {
                if let Some(g) = self.inline_source(node, "a", "guard") {
                    self.touch(g);
                    let c = self.operand(g, "cond", PREC_IF)?;
                    let t = self.operand(g, "value", PREC_IF)?;
                    let e = self.operand(node, "b", PREC_IF)?;
                    return Ok((format!("if {c} then {t} else {e}"), PREC_IF));
                }
                let a = self.operand(node, "a", PREC_COALESCE)?;
                let b = self.operand(node, "b", PREC_COALESCE)?;
                Ok((format!("{a} ?? {b}"), PREC_COALESCE))
            }
            "guard" => {
                let v = self.operand(node, "value", PREC_WHEN)?;
                let c = self.operand(node, "cond", PREC_WHEN)?;
                Ok((format!("{v} when {c}"), PREC_WHEN))
            }
            "add" | "multiply" | "and" | "or" => {
                let (op, prec) = match d.name.as_str() {
                    "add" => ("+", PREC_ADD),
                    "multiply" => ("*", PREC_MUL),
                    "and" => ("&&", PREC_AND),
                    _ => ("||", PREC_OR),
                };
                let mut parts = Vec::new();
                for p in &d.inputs {
                    parts.push(self.operand(node, &p.name, prec)?);
                }
                if parts.len() < 2 {
                    return self.generic(node);
                }
                Ok((parts.join(&format!(" {op} ")), prec))
            }
            "subtract" | "divide" => {
                let op = if d.name == "subtract" { "-" } else { "/" };
                let prec = if d.name == "subtract" { PREC_ADD } else { PREC_MUL };
                let a = self.operand(node, "a", prec)?;
                let b = self.operand(node, "b", prec)?;
                Ok((format!("{a} {op} {b}"), prec))
            }
            "compare" if ["<", "<=", ">", ">="].contains(&node.param("op")) => {
                let a = self.operand(node, "a", PREC_CMP)?;
                let b = self.operand(node, "b", PREC_CMP)?;
                Ok((format!("{a} {} {b}", node.param("op")), PREC_CMP))
            }
            "equals" => {
                let a = self.operand(node, "a", PREC_EQ)?;
                let b = self.operand(node, "b", PREC_EQ)?;
                Ok((format!("{a} == {b}"), PREC_EQ))
            }
            "not" => {
                if let Some(eq) = self.inline_source(node, "v", "equals") {
                    self.touch(eq);
                    let a = self.operand(eq, "a", PREC_EQ)?;
                    let b = self.operand(eq, "b", PREC_EQ)?;
                    return Ok((format!("{a} != {b}"), PREC_EQ));
                }
                let v = self.operand(node, "v", PREC_NOT)?;
                Ok((format!("!{v}"), PREC_NOT))
            }
            "map" | "fold" => {
                let hex = node.param("fn");
                let hname = if hex.is_empty() { String::new() } else { self.p.holon_for(hex, &[], &[], "fn")? };
                let inputs: Vec<String> = d.inputs.iter().map(|p| p.name.clone()).collect();
                let mut args = self.positional_then_named(node, &inputs, &[])?;
                if !hname.is_empty() {
                    args.push(format!("fn: {hname}"));
                }
                atom(format!("{}({})", d.name, args.join(", ")))
            }
            _ => self.generic(node),
        }
    }

    /// Whether `node`'s `input` is the request record itself (the request node's `request`).
    fn inline_request_record(&self, node: &Node, input: &str) -> bool {
        match (self.g.incoming.get(&(node.id.clone(), input.to_string())), &self.request) {
            (Some(Src::Node { id, output }), Some((req, _))) => id == req && output == "request",
            _ => false,
        }
    }

    /// `name(inputs…, param: value…)`: inputs positional up to the first unwired one, named after.
    fn generic(&mut self, node: &Node) -> Result<(String, u8), String> {
        let inputs: Vec<String> = node.data.inputs.iter().map(|p| p.name.clone()).collect();
        let params = self.p.changed_params(node);
        let mut args = self.positional_then_named(node, &inputs, &[])?;
        for (k, v) in params {
            args.push(format!("{}: {}", param_value(&k), param_value(&v)));
        }
        Ok((format!("{}({})", node.data.name, args.join(", ")), ATOM))
    }

    fn positional_then_named(&mut self, node: &Node, inputs: &[String], _skip: &[&str]) -> Result<Vec<String>, String> {
        let mut args = Vec::new();
        let mut named = false;
        for input in inputs {
            match self.arg(node, input)? {
                Some((t, _)) if !named => args.push(t),
                Some((t, _)) => args.push(format!("{input}: {t}")),
                None => named = true,
            }
        }
        Ok(args)
    }

    /// A `concat` whose parts are consts and `as_text`s, each inline, as an interpolated string
    /// (only with at least one `{…}` part: a concat of consts stays a concat, so the graph keeps
    /// its nodes); else a plain call.
    fn concat(&mut self, node: &Node) -> Result<(String, u8), String> {
        let mut pieces = Vec::new();
        let mut parts = 0;
        for p in &node.data.inputs {
            if let Some(c) = self.inline_source(node, &p.name, "const")
                && c.param("type") == "Text"
            {
                self.touch(c);
                pieces.push(quote(c.param("value")).trim_matches('"').to_string());
            } else if let Some(a) = self.inline_source(node, &p.name, "as_text") {
                self.touch(a);
                let (t, _) = match self.arg(a, "value")? {
                    Some(v) => v,
                    None => return self.generic(node),
                };
                pieces.push(format!("{{{t}}}"));
                parts += 1;
            } else {
                return self.generic(node);
            }
        }
        if parts == 0 {
            return self.generic(node);
        }
        Ok((format!("\"{}\"", pieces.join("")), ATOM))
    }

    /// A chain of `with_field`s as a record literal: `{ k: v, … }` from a `record()`, or
    /// `{ ..base, k: v, … }` from anything else.
    fn record(&mut self, node: &Node) -> Result<(String, u8), String> {
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut cur = node;
        let base = loop {
            self.touch(cur);
            let key = cur.param("name").to_string();
            let value = self.required(cur, "value")?;
            fields.push((key, value));
            match self.inline_source(cur, "record", "with_field") {
                Some(inner) => cur = inner,
                None => {
                    if let Some(empty) = self.inline_source(cur, "record", "record") {
                        self.touch(empty);
                        break None;
                    }
                    break Some(self.required(cur, "record")?);
                }
            }
        };
        fields.reverse();
        let mut items: Vec<String> = base.into_iter().map(|b| format!("..{b}")).collect();
        // A keyword is fine as a key (`at: now`): the parser reads any word there.
        items.extend(fields.iter().map(|(k, v)| format!("{}: {v}", if plain(k) { k.clone() } else { quote(k) })));
        Ok((format!("{{ {} }}", items.join(", ")), ATOM))
    }

    /// `respond …` as a statement.
    fn respond(&mut self, node: &Node) -> Result<String, String> {
        self.touched.push(node.id.clone());
        let (status, missing) = (node.param("status"), node.param("missing"));
        let body = self.arg(node, "body")?.map(|(t, _)| t);
        let mut parts = Vec::new();
        match body {
            Some(b) => parts.push(b),
            None if status == "204" && missing == "404" => return Ok("respond 204".into()),
            None => parts.push(format!("status: {status}")),
        }
        if status != "200" && parts.first().is_some_and(|p| !p.starts_with("status:")) {
            parts.push(format!("status: {status}"));
        }
        if missing != "404" {
            parts.push(format!("missing: {missing}"));
        }
        Ok(format!("respond {}", parts.join(", ")))
    }

    /// An effect nobody consumes, as a statement.
    fn effect_statement(&mut self, node: &Node) -> Result<String, String> {
        let d = &node.data;
        match d.name.as_str() {
            "store_write" if plain(node.param("key")) => Ok(format!("state.{} = {}", node.param("key"), self.required(node, "value")?)),
            "collection_put" if plain(node.param("collection")) => {
                let key = self.required(node, "key")?;
                let value = self.required(node, "value")?;
                Ok(format!("{}[{key}] = {value}", node.param("collection")))
            }
            "collection_delete" if plain(node.param("collection")) => Ok(format!("delete {}[{}]", node.param("collection"), self.required(node, "key")?)),
            _ => Ok(self.render(node, "")?.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FlowBuilder;
    use crate::flow::flow_unit;
    use crate::lang::compile;
    use crate::{samples, to_hex};

    fn hash_of(doc: &FlowDocument) -> String {
        let mut store = MapResolver::new();
        restore(doc, &mut store, &Names::new());
        let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
        to_hex(&flow_unit(&doc.nodes, &doc.edges, &mut store, port).unwrap_or_else(|e| panic!("{e}")))
    }

    fn printed(name: Option<&str>, doc: &FlowDocument, bindings: &BTreeMap<String, String>) -> String {
        print(name, doc, &mut MapResolver::new(), &Names::new(), bindings).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The round trip of §11: printed, compiled again, the same unit; printed again, the same text.
    fn round_trips(name: &str, doc: &FlowDocument, bindings: &BTreeMap<String, String>) -> String {
        let text = printed(Some(name), doc, bindings);
        let back = compile(&text).unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
        assert_eq!(hash_of(&back.doc), hash_of(doc), "{name}: the printed text lowers to another unit:\n{text}");
        let again = printed(Some(name), &back.doc, &back.names);
        assert_eq!(again, text, "{name}: printing isn't idempotent");
        text
    }

    #[test]
    fn every_sample_round_trips_from_its_builder() {
        for s in samples::samples() {
            let doc = samples::build(s).unwrap();
            round_trips(s.name, &doc, &BTreeMap::new());
        }
    }

    #[test]
    fn every_sample_round_trips_from_its_natural_text() {
        for (name, src) in crate::lang::lower::tests::golden() {
            let first = compile(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            let text = round_trips(name, &first.doc, &first.names);
            // Names from the text survive fmt.
            if *name == "guarded-api" {
                assert!(text.contains("  ok = "), "{text}");
                assert!(text.contains("  receipt = "), "{text}");
            }
        }
    }

    #[test]
    fn the_canonical_form_of_two_samples() {
        let hello = samples::build(samples::sample("hello-api").unwrap()).unwrap();
        assert_eq!(
            printed(Some("hello-api"), &hello, &BTreeMap::new()),
            "flow hello-api\nport 8101\n\nroute GET /hello/:name (req) {\n  respond concat(\"Hello, \", req.params.name)\n}\n"
        );
        let heartbeat = samples::build(samples::sample("heartbeat").unwrap()).unwrap();
        assert_eq!(
            printed(Some("heartbeat"), &heartbeat, &BTreeMap::new()),
            "flow heartbeat\nevery 10s\n\nt = now\nstate.last_beat = t\nstate.beats = (state.beats ?? 0) + 1\nstate.first_beat = state.first_beat ?? t\n"
        );
        let likes = compile(crate::lang::lower::tests::golden().iter().find(|(n, _)| *n == "likes").unwrap().1).unwrap();
        let text = printed(Some("likes"), &likes.doc, &likes.names);
        assert!(text.contains("collection likes\n"), "{text}");
        assert!(text.contains("  respond \"{likes[req.params.id] ?? 0} likes\"\n"), "{text}");
        assert!(text.contains("  likes[id] = n\n"), "{text}");
    }

    #[test]
    fn the_natural_spellings_print_as_written() {
        let src = "flow f\nport 8200\ncollection notes\n\nholon twice(n) -> o {\n  o = n + n\n}\n\nroute POST /x (req) {\n  item = notes[req.params.id] ?? { k: [1, \"a\"] }\n  ok = req.headers.authorization == \"Bearer {secret.tok}\" && !(item.done)\n  sent = http.post(\"h.example\", \"/p\", body: json({ ..item, n: twice(item.n) }))\n  paid = await paid, expires: 2h\n  notes[req.params.id] = if ok then item else { }\n  state.last = { ..req.json, paid_via: paid.method }\n  respond { status: sent.status } when ok, status: 201, missing: 401\n}\n";
        let first = compile(src).unwrap_or_else(|e| panic!("{e}"));
        let text = round_trips("f", &first.doc, &first.names);
        for line in [
            "holon twice(n) -> o {\n  o = n + n\n}",
            "  item = notes[req.params.id] ?? { k: [1, \"a\"] }",
            "  ok = req.headers.authorization == \"Bearer {secret.tok}\" && !item.done",
            "  sent = http.post(\"h.example\", \"/p\", body: json({ ..item, n: twice(item.n) }))",
            "  paid = await paid, expires: 2h",
            "  notes[req.params.id] = if ok then item else {}",
            "  state.last = { ..req.json, paid_via: paid.method }",
            "  respond { status: sent.status } when ok, status: 201, missing: 401",
        ] {
            assert!(text.contains(line), "missing {line:?} in\n{text}");
        }
    }

    #[test]
    fn a_hand_grouped_canvas_round_trips_with_its_proposed_names_and_odd_shapes() {
        // A holon grouped by proposal (core's own port names), placed twice; a variadic leaf;
        // a multi-output composite with an effect; a lookup with a dynamic key.
        let mut b = FlowBuilder::new();
        b.begin_holon("shout");
        let up = b.node("uppercase", &[], None).unwrap();
        let bang = b.node("const", &[("value".into(), "!".into()), ("type".into(), "Text".into())], None).unwrap();
        let cat = b.node("concat", &[], None).unwrap();
        b.wire(&up, "out", &cat, "a").unwrap();
        b.wire(&bang, "out", &cat, "b").unwrap();
        let shout = b.end_holon("shout", &["words".into()], &["said".into()]).unwrap();
        b.begin_holon("keep");
        let w = b.node("store_write", &[("key".into(), "last".into())], None).unwrap();
        let len = b.node("text_length", &[], None).unwrap();
        let keep = b.end_holon("keep", &["value".into(), "text".into()], &["saved".into(), "n".into()]).unwrap();
        let _ = (w, len);
        let a = b.node("const", &[("value".into(), "hi".into()), ("type".into(), "Text".into())], None).unwrap();
        let s1 = b.place(&shout, None);
        let s2 = b.place(&shout, None);
        let k = b.place(&keep, None);
        let three = b.node("concat", &[], None).unwrap();
        b.arity(&three, 3).unwrap();
        b.wire(&a, "out", &s1, "words").unwrap();
        b.wire(&a, "out", &s2, "words").unwrap();
        b.wire(&s1, "said", &three, "a").unwrap();
        b.wire(&s2, "said", &three, "b").unwrap();
        b.wire(&a, "out", &three, "c").unwrap();
        b.wire(&three, "out", &k, "value").unwrap();
        b.wire(&a, "out", &k, "text").unwrap();
        let rec = b.node("record", &[], None).unwrap();
        let look = b.node("lookup", &[], None).unwrap();
        b.wire(&rec, "out", &look, "record").unwrap();
        b.wire(&a, "out", &look, "key").unwrap();
        let doc = b.finish(Default::default());
        let text = round_trips("canvas", &doc, &BTreeMap::new());
        assert!(text.contains("holon shout(words \"s\") -> said"), "{text}");
        assert!(text.contains("holon keep("), "{text}");
        assert!(text.contains("concat("), "{text}");
    }

    #[test]
    fn a_group_with_a_request_that_is_not_a_route_prints_in_the_wiring_form() {
        let mut b = FlowBuilder::new();
        let r1 = b.node("request", &[("method".into(), "GET".into()), ("path".into(), "/a".into())], None).unwrap();
        let r2 = b.node("request", &[("method".into(), "GET".into()), ("path".into(), "/b".into())], None).unwrap();
        let c = b.node("concat", &[], None).unwrap();
        b.wire(&r1, "body", &c, "a").unwrap();
        b.wire(&r2, "body", &c, "b").unwrap();
        let doc = b.finish(Default::default());
        let text = printed(Some("two"), &doc, &BTreeMap::new());
        assert!(text.contains("node request = request { path: \"/a\" }"), "{text}");
        assert!(text.contains("node request2 = request { path: \"/b\" }"), "{text}");
        assert!(text.contains("request.body -> concat.a"), "{text}");
        assert!(text.contains("# GET /a and GET /b are wired together"), "{text}");
        let back = compile(&text).unwrap();
        assert_eq!(back.doc.nodes.len(), 3);
    }

    #[test]
    fn each_node_knows_its_line() {
        let src = "flow f\nport 8200\n\nroute GET /x (req) {\n  n = req.params.id\n  respond \"{n} likes\"\n}\n";
        let first = compile(src).unwrap();
        let (text, lines) = print_lines(Some("f"), &first.doc, &mut MapResolver::new(), &Names::new(), &first.names).unwrap();
        assert_eq!(text, src);
        let line_of = |leaf: &str| first.doc.nodes.iter().filter(|n| n.data.name == leaf).map(|n| lines[&n.id]).collect::<Vec<_>>();
        assert_eq!(line_of("request"), [4], "the request is the route header");
        assert_eq!(line_of("field"), [5]);
        assert_eq!(line_of("as_text"), [6], "an inline node is its consumer's statement");
        assert_eq!(line_of("respond"), [6]);
        assert_eq!(line_of("concat"), [6]);
        // A canvas with an unwired input prints as nodes and wires, each node on its own line.
        let mut b = FlowBuilder::new();
        let up = b.node("uppercase", &[], None).unwrap();
        let doc = b.finish(Default::default());
        let (text, lines) = print_lines(Some("solo"), &doc, &mut MapResolver::new(), &Names::new(), &BTreeMap::new()).unwrap();
        assert_eq!(text, "flow solo\n\nnode uppercase = uppercase\n");
        assert_eq!(lines[&up], 3);
    }

    #[test]
    fn literals_quote_and_duration_as_written() {
        assert_eq!(quote("a \"b\" {c}\n"), "\"a \\\"b\\\" {{c}}\\n\"");
        assert_eq!(duration("86400"), "1d");
        assert_eq!(duration("300"), "5m");
        assert_eq!(duration("45"), "45s");
        assert_eq!(duration("2.5"), "2.5");
        // Keys sorted, as the lowering writes a literal: that is the text that writes back the same.
        assert_eq!(json_literal("{\"done\":false,\"k\":[1,\"x\"]}"), Some("{ done: false, k: [1, \"x\"] }".into()));
        assert_eq!(json_literal("{\"k\": 1}"), None, "a text that doesn't write back the same stays a const");
        assert_eq!(json_literal("{\"k\":[1,\"x\"],\"done\":false}"), None, "unsorted keys don't either");
        assert_eq!(consent("SecretRead(\"receipt_key\")"), "secret.receipt_key");
        assert_eq!(consent("HttpRequest(\"h\")"), "HttpRequest(\"h\")");
    }
}
