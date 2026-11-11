//! Building a flow document node by node, as the samples and a flow's text (`.holon.ts`) do: every
//! node from the live catalog (or a template), every name checked (an unknown leaf, param or port
//! is refused), holons grouped exactly as the editor groups them. What it builds is what opening
//! the flow on a canvas loads.

use std::collections::{BTreeMap, HashMap};

use holon_core::{encode_preimage, interface_of, parse_hash, validate, Hash, MapResolver, Resolver, Unit, UnitBody, BOUNDARY, CALLS, PASSES_EXTRA, TRIGGER, VARIADIC};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::canvas::{next_port_name, Node, NodeData, PlacedBody, Position, Wire};
use crate::composites::{freeze_inner, BoundaryPort, BOUNDARY_IN, BOUNDARY_OUT};
use crate::group::group_selection;
use crate::templates;
use crate::wire::{WParam, WPort, WType};

/// A flow of its own, grouped into one composite unit: placed as a node in another flow, it is a
/// holon, a whole that is also a part. Its hash is the same wherever it's placed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Holon {
    pub name: String,
    pub hash: String,
    pub inputs: Vec<WPort>,
    pub outputs: Vec<WPort>,
}

/// A port of a holon whose boundary is given: its name on the node, and the unit's own name for
/// it when that differs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HolonPort {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Deploy settings, as a flow document holds them (`server`). What isn't read here passes through.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Settings {
    #[serde(default)]
    pub port: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements: Option<Requirements>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browsers: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Requirements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

/// Where the canvas looks so that all of `nodes` shows at once, clear of the canvas's panels.
pub fn fit(nodes: &[Node]) -> Viewport {
    let right = nodes.iter().map(|n| n.position().x).fold(f64::NEG_INFINITY, f64::max) + 320.0;
    let bottom = nodes.iter().map(|n| n.position().y).fold(f64::NEG_INFINITY, f64::max) + 220.0;
    let zoom = 0.8f64.min(820.0 / right).min(640.0 / bottom);
    Viewport { x: 10.0, y: 50.0, zoom: (zoom * 100.0).round() / 100.0 }
}

/// A flow document: its nodes and wires, where the canvas looks, the units of its composites
/// (hex preimages, their whole closure, children first) and their names, its deploy settings.
/// What isn't read here passes through.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct FlowDocument {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Wire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<String>>,
    /// The module units its composites place, by engine and params rather than by preimage
    /// (modules.rs): rebuilt from the engine bytes the restoring build has, checked by hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<Vec<crate::modules::ModuleEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<Settings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Every hash reachable from `roots` (children, plus a `map`/`fold` `fn` param), each once,
/// children before the units that reference them: the units a flow document carries.
pub fn closure_of(roots: &[Hash], store: &MapResolver) -> Vec<Hash> {
    fn visit(h: Hash, store: &MapResolver, seen: &mut Vec<Hash>, order: &mut Vec<Hash>) {
        if seen.contains(&h) {
            return;
        }
        seen.push(h);
        if let Some(unit) = store.resolve(&h) {
            if let UnitBody::Composite { children, .. } = &unit.body {
                for c in children.clone() {
                    visit(c, store, seen, order);
                }
            }
            let f = unit.params.iter().find_map(|p| match p {
                holon_core::Param::Frozen { name, value } if name == "fn" => parse_hash(value),
                _ => None,
            });
            if let Some(f) = f {
                visit(f, store, seen, order);
            }
        }
        order.push(h);
    }
    let (mut seen, mut order) = (Vec::new(), Vec::new());
    for &r in roots {
        visit(r, store, &mut seen, &mut order);
    }
    order
}

/// The outputs of `unit` that answer a call made when its trigger arrives: a request or another
/// flow's action tied to it, or a function placed in it whose own output is one. A call is made
/// whatever reads its answer, so a handler's group exposes it (core's `default_spec`) and a
/// function's boundary names it (`call_port`): the function placed is entered for it even where
/// nothing reads what it returns. The printer leaves them out of what the function returns.
pub fn call_outputs(unit: &Unit, store: &impl Resolver) -> Vec<String> {
    let UnitBody::Composite { children, edges } = &unit.body else { return vec![] };
    let mut placed: HashMap<u32, Vec<String>> = HashMap::new();
    let mut out = Vec::new();
    for e in edges.iter().filter(|e| e.to_child == BOUNDARY && e.from_child != BOUNDARY) {
        let Some(child) = children.get(e.from_child as usize).and_then(|h| store.resolve(h)) else { continue };
        let answers = match &child.body {
            UnitBody::Primitive { name } => {
                CALLS.contains(&name.as_str()) && edges.iter().any(|t| t.to_child == e.from_child && child.inputs.get(t.to_port as usize).is_some_and(|p| p.name == TRIGGER))
            }
            UnitBody::Composite { .. } => {
                let theirs = placed.entry(e.from_child).or_insert_with(|| call_outputs(child, store));
                child.outputs.get(e.from_port as usize).is_some_and(|p| theirs.contains(&p.name))
            }
            UnitBody::Module { .. } => false,
        };
        if answers && let Some(p) = unit.outputs.get(e.to_port as usize) {
            out.push(p.name.clone());
        }
    }
    out
}

/// The name a function's boundary gives the next call's answer it names (`call_outputs`):
/// `called`, then `called_2`, …, past the names `taken`.
pub fn call_port(taken: impl Fn(&str) -> bool) -> String {
    let mut name = "called".to_string();
    let mut n = 1;
    while taken(&name) {
        n += 1;
        name = format!("called_{n}");
    }
    name
}

/// A literal's wire type, as the editor sets it when a const's or default's type changes.
fn literal_type(ty: &str) -> Result<WType, String> {
    if ty == "Json" {
        return Ok(WType::Any);
    }
    serde_json::from_value(Value::String(ty.into())).map_err(|_| format!("\"{ty}\" is not a type"))
}

struct Frame {
    nodes: Vec<Node>,
    wires: Vec<Wire>,
    prefix: String,
}

/// Whether wire `w` takes in what its source may hold, an effect, as core's `default_spec` reads
/// it: into an input that isn't a trigger, of a node with an output that can hold it and gives it
/// on (`gives_on`).
fn takes_in(f: &Frame, w: &Wire, gives: &mut HashMap<(String, String), bool>) -> bool {
    let plain = |t: &WType| matches!(t, WType::Number | WType::Boolean | WType::Text | WType::Bytes);
    let Some(reader) = f.nodes.iter().find(|n| n.id == w.target) else { return false };
    w.input() != holon_core::TRIGGER && reader.data.outputs.iter().any(|p| !plain(&p.ty) && gives_on(f, &reader.id, &p.name, gives))
}

/// Whether `node`'s `output`, holding an effect, gives it on to where it is performed: it is an
/// effect itself, nothing reads it, or a reader takes it in (`takes_in`).
fn gives_on(f: &Frame, node: &str, output: &str, gives: &mut HashMap<(String, String), bool>) -> bool {
    let key = (node.to_string(), output.to_string());
    if let Some(&known) = gives.get(&key) {
        return known;
    }
    gives.insert(key.clone(), false);
    let effect = f.nodes.iter().find(|n| n.id == node).and_then(|n| n.data.outputs.iter().find(|p| p.name == output)).is_some_and(|p| p.ty == WType::Effect);
    let readers: Vec<&Wire> = f.wires.iter().filter(|w| w.source == node && w.output() == output).collect();
    let on = effect || readers.is_empty() || readers.into_iter().any(|w| takes_in(f, w, gives));
    gives.insert(key, on);
    on
}

/// Places nodes and wires them, in a flow and in the holons it groups (`begin_holon`, then
/// `end_holon` or `end_holon_exact`, build the holon's own flow in between). Its units are kept in
/// its own store until `finish` hands them over in the document.
pub struct FlowBuilder {
    pub store: MapResolver,
    counter: u64,
    holons: Vec<Hash>,
    names: BTreeMap<String, String>,
    frames: Vec<Frame>,
}

impl Default for FlowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowBuilder {
    pub fn new() -> FlowBuilder {
        FlowBuilder { store: MapResolver::new(), counter: 0, holons: vec![], names: BTreeMap::new(), frames: vec![Frame { nodes: vec![], wires: vec![], prefix: String::new() }] }
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the flow's own frame is never popped")
    }

    /// Where the next node goes when no position is given: five to a row.
    fn next_spot(&self) -> Position {
        Position { x: 40.0 + 260.0 * (self.counter % 5) as f64, y: 40.0 + 160.0 * (self.counter / 5) as f64 }
    }

    fn next_id(&mut self, name: &str) -> String {
        let n = self.counter;
        let id = format!("{}{name}-{n}", self.frame().prefix);
        self.counter += 1;
        id
    }

    /// A leaf from the catalog or a template, with params set by name, at a canvas position (the
    /// next free spot when none). Answers its id.
    pub fn node(&mut self, name: &str, set: &[(String, String)], at: Option<Position>) -> Result<String, String> {
        let position = at.unwrap_or_else(|| self.next_spot());
        let unknown = |names: &[WParam]| set.iter().find(|(k, _)| !names.iter().any(|p| p.name() == k)).map(|(k, _)| format!("{name} has no param \"{k}\""));
        let given = |p: &WParam| set.iter().rev().find(|(k, _)| k == p.name()).map(|(k, v)| WParam::frozen(k, v)).unwrap_or_else(|| p.clone());
        let data = if let Some(defaults) = templates::defaults(name) {
            if let Some(e) = unknown(&defaults) {
                return Err(e);
            }
            let params: Vec<WParam> = defaults.iter().map(given).collect();
            let iface = templates::interface(name, &params).expect("a template has an interface");
            let mut data = NodeData::new(name, PlacedBody::Template, iface.inputs, iface.outputs, params);
            data.hidden_inputs = Some(iface.hidden_inputs);
            data
        } else {
            let unit = interface_of(name).ok_or_else(|| format!("no leaf \"{name}\" in the catalog"))?;
            let defaults: Vec<WParam> = unit.params.iter().map(WParam::from_core).collect();
            if let Some(e) = unknown(&defaults) {
                return Err(e);
            }
            let params: Vec<WParam> = defaults.iter().map(given).collect();
            // A family parameter outside its values is refused here, where it is written, in
            // core's words.
            let core_params = params.iter().cloned().map(WParam::into_core).collect();
            if let Err(errors) = validate(&Unit { params: core_params, ..unit.clone() }, &MapResolver::new()) {
                return Err(errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "));
            }
            let mut inputs: Vec<WPort> = unit.inputs.iter().map(WPort::from_core).collect();
            let mut outputs: Vec<WPort> = unit.outputs.iter().map(WPort::from_core).collect();
            if name == "const" || name == "default" {
                let ty = literal_type(crate::wire::frozen_value(&params, "type").unwrap_or("Number"))?;
                for p in inputs.iter_mut().chain(outputs.iter_mut()) {
                    p.ty = ty.clone();
                }
            }
            let mut data = NodeData::new(name, PlacedBody::Primitive, inputs, outputs, params);
            data.variadic = VARIADIC.contains(&name);
            data.min_inputs = data.inputs.len();
            data
        };
        let id = self.next_id(name);
        self.frame().nodes.push(Node::placed(&id, position, data));
        Ok(id)
    }

    fn find(&mut self, id: &str) -> Result<&mut Node, String> {
        self.frame().nodes.iter_mut().find(|n| n.id == id).ok_or_else(|| format!("no node {id}"))
    }

    /// A placed node of the flow being built (the holon's, while one is begun), by id: its ports
    /// and params, as the language's lowering reads them back.
    pub fn data(&self, id: &str) -> Option<&NodeData> {
        self.frames.last().and_then(|f| f.nodes.iter().find(|n| n.id == id)).map(|n| &n.data)
    }

    /// The outputs of the function `hash` placed that answer a call (`call_outputs`).
    pub fn call_outputs_of(&self, hash: &str) -> Vec<String> {
        parse_hash(hash).and_then(|h| self.store.resolve(&h)).map(|u| call_outputs(u, &self.store)).unwrap_or_default()
    }

    /// A wire from `from`'s output to `to`'s input. A variadic node gains the next input when it
    /// is wired, as its "+" adds one.
    pub fn wire(&mut self, from: &str, output: &str, to: &str, input: &str) -> Result<(), String> {
        let src = self.find(from)?;
        if !src.data.outputs.iter().any(|p| p.name == output) {
            return Err(format!("{} has no output \"{output}\"", src.data.name));
        }
        let dst = self.find(to)?;
        let has = |d: &NodeData| d.inputs.iter().any(|p| p.name == input);
        if !has(&dst.data) && dst.data.variadic && input == next_port_name(&dst.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()) {
            let ty = dst.data.inputs.first().map(|p| p.ty.clone()).unwrap_or(WType::Any);
            dst.data.inputs.push(WPort { name: input.into(), ty });
        }
        if !has(&dst.data) {
            return Err(format!("{} has no input \"{input}\"", dst.data.name));
        }
        let mut wire = Wire { id: Some(format!("xy-edge__{from}{output}-{to}{input}")), ..Wire::new(from, output, to, input) };
        wire.extra.insert("style".into(), json!({ "stroke": "#8295b0", "strokeWidth": 1.6 }));
        self.frame().wires.push(wire);
        Ok(())
    }

    /// What is wired to `to`'s `input` (in the flow being built, or the holon begun): the node and
    /// its output.
    pub fn source(&self, to: &str, input: &str) -> Option<(&str, &str)> {
        self.frames.last()?.wires.iter().find(|w| w.target == to && w.input() == input).map(|w| (w.source.as_str(), w.output()))
    }

    /// Whether something is wired to `to`'s `input` (in the flow being built, or the holon begun).
    pub fn is_wired(&self, to: &str, input: &str) -> bool {
        self.frames.last().is_some_and(|f| f.wires.iter().any(|w| w.target == to && w.input() == input))
    }

    /// The Effect outputs nothing takes in (in the flow being built, or the holon begun), as
    /// `(node, output)` in the order placed: a handler's group exposes them by the IR's own rule
    /// (core's `default_spec`), and a holon's boundary must name them or they are never performed.
    /// As there, a trigger that waits for one doesn't take it in, nor does a test that it arrived,
    /// nor a node that gives it on only to such tests (`takes_in`).
    pub fn unread_effects(&self) -> Vec<(String, String)> {
        let Some(f) = self.frames.last() else { return vec![] };
        let mut gives = HashMap::new();
        let mut unread = Vec::new();
        for n in &f.nodes {
            for p in n.data.outputs.iter().filter(|p| p.ty == WType::Effect) {
                if !f.wires.iter().any(|w| w.source == n.id && w.output() == p.name && takes_in(f, w, &mut gives)) {
                    unread.push((n.id.clone(), p.name.clone()));
                }
            }
        }
        unread
    }

    /// A variadic node given `inputs` inputs, as its "+" adds them.
    pub fn arity(&mut self, node: &str, inputs: usize) -> Result<(), String> {
        let n = self.find(node)?;
        if !n.data.variadic {
            return Err(format!("{} doesn't take more inputs", n.data.name));
        }
        if inputs < n.data.min_inputs {
            return Err(format!("{} takes at least {} inputs", n.data.name, n.data.min_inputs));
        }
        while n.data.inputs.len() < inputs {
            let name = next_port_name(&n.data.inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
            // map's and fold's extra inputs are whatever their function takes after the item.
            let ty = if PASSES_EXTRA.contains(&n.data.name.as_str()) { WType::Any } else { n.data.inputs.first().map(|p| p.ty.clone()).unwrap_or(WType::Any) };
            n.data.inputs.push(WPort { name, ty });
        }
        Ok(())
    }

    /// Start building the holon `name`'s own flow: nodes and wires go to it until it ends.
    pub fn begin_holon(&mut self, name: &str) {
        self.frames.push(Frame { nodes: vec![], wires: vec![], prefix: format!("{name}.") });
    }

    fn end_frame(&mut self) -> Frame {
        assert!(self.frames.len() > 1, "a holon ends only after it begins");
        self.frames.pop().expect("checked")
    }

    fn keep(&mut self, name: &str, hash: &str) {
        self.holons.push(parse_hash(hash).expect("a hash made here"));
        self.names.insert(hash.into(), name.into());
    }

    /// End the holon begun last, grouped into one composite as the editor's "group" does: its free
    /// inputs and outputs become its ports, named `inputs` and `outputs` in boundary order.
    pub fn end_holon(&mut self, name: &str, inputs: &[String], outputs: &[String]) -> Result<Holon, String> {
        let inner = self.end_frame();
        let id = self.counter;
        let (node, _) = group_selection(&inner.nodes, &inner.wires, &mut self.store, id).map_err(|e| format!("holon \"{name}\" doesn't group: {}", e.message))?;
        self.counter += 1;
        let named = |got: &[WPort], want: &[String], side: &str| -> Result<Vec<WPort>, String> {
            if got.len() != want.len() {
                let got: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
                return Err(format!("holon \"{name}\" has {side} {}, not {}", got.join(", "), want.join(", ")));
            }
            Ok(got.iter().zip(want).map(|(p, w)| WPort { name: w.clone(), ty: p.ty.clone() }).collect())
        };
        let inputs = named(&node.data.inputs, inputs, "inputs")?;
        let outputs = named(&node.data.outputs, outputs, "outputs")?;
        let PlacedBody::Composite { hash } = node.data.body else { return Err(format!("holon \"{name}\" isn't a composite")) };
        self.keep(name, &hash);
        Ok(Holon { name: name.into(), hash, inputs, outputs })
    }

    /// End the holon begun last, its boundary given, not proposed: each input feeds the one inner
    /// port `wiring_in` says (`(port, node, input)`), each output is fed as `wiring_out` says
    /// (`(node, output, port)`), in this order: frozen as the expand view freezes, so a holon
    /// edited there round-trips exactly.
    pub fn end_holon_exact(&mut self, name: &str, inputs: &[HolonPort], outputs: &[HolonPort], wiring_in: &[(String, String, String)], wiring_out: &[(String, String, String)]) -> Result<Holon, String> {
        let mut inner = self.end_frame();
        let boundary = |list: &[HolonPort]| -> Vec<BoundaryPort> {
            list.iter().map(|p| BoundaryPort { name: p.name.clone(), ty: WType::Any, outside: vec![], unit: Some(p.unit.clone().unwrap_or_else(|| p.name.clone())), was: None }).collect()
        };
        for (port, node, input) in wiring_in {
            inner.wires.push(Wire { id: Some(format!("in-{port}")), ..Wire::new(BOUNDARY_IN, port, node, input) });
        }
        for (node, output, port) in wiring_out {
            inner.wires.push(Wire { id: Some(format!("out-{port}")), ..Wire::new(node, output, BOUNDARY_OUT, port) });
        }
        let (hash, inputs, outputs) =
            freeze_inner(&boundary(inputs), &boundary(outputs), &inner.nodes, &inner.wires, &mut self.store).map_err(|e| format!("holon \"{name}\": {}", e.message))?;
        self.keep(name, &hash);
        Ok(Holon { name: name.into(), hash, inputs, outputs })
    }

    /// Place the JavaScript engine module (docs/L4s/js-node-L4.md §4) under `fuel` and `memory`
    /// (the defaults when not given): a module unit stored here, placed as a node with its
    /// interface, `engine(source: Text, args: List<Any>) -> (result: Record)`. Answers its id.
    pub fn place_js_engine(&mut self, fuel: Option<u64>, memory: Option<u64>) -> Result<String, String> {
        let unit = crate::modules::js_engine_unit(fuel, memory);
        let (inputs, outputs, params) = (
            unit.inputs.iter().map(WPort::from_core).collect(),
            unit.outputs.iter().map(WPort::from_core).collect(),
            unit.params.iter().map(WParam::from_core).collect(),
        );
        let hash = self.store.insert(unit).map_err(|e| format!("the js engine doesn't store: {e:?}"))?;
        let hex = crate::to_hex(&hash);
        let data = NodeData::new("js", PlacedBody::Module { hash: hex }, inputs, outputs, params);
        let id = self.next_id("js");
        let position = self.next_spot();
        self.frame().nodes.push(Node::placed(&id, position, data));
        Ok(id)
    }

    /// Place a holon as a node, wired by its port names like any other.
    pub fn place(&mut self, holon: &Holon, at: Option<Position>) -> String {
        let position = at.unwrap_or_else(|| self.next_spot());
        let id = self.next_id(&holon.name);
        let data = NodeData::new(&holon.name, PlacedBody::Composite { hash: holon.hash.clone() }, holon.inputs.clone(), holon.outputs.clone(), vec![]);
        self.frame().nodes.push(Node::placed(&id, position, data));
        id
    }

    /// The flow document: its nodes and wires, zoomed so the whole flow shows at once (clear of
    /// the canvas's panels), its holons' units (the whole closure) and names, its settings.
    pub fn finish(mut self, settings: Settings) -> FlowDocument {
        while self.frames.len() > 1 {
            self.frames.pop();
        }
        let top = self.frames.pop().expect("the flow's own frame");
        let viewport = Some(fit(&top.nodes));
        // The closure as the document carries it: a module of a known engine by engine and
        // params (modules.rs), everything else as its preimage.
        let (mut units, mut modules) = (Vec::new(), Vec::new());
        for h in closure_of(&self.holons, &self.store) {
            let Some(unit) = self.store.resolve(&h) else { continue };
            if let Some(entry) = crate::modules::entry_of(unit, &crate::to_hex(&h)) {
                modules.push(entry);
            } else if let Ok(bytes) = encode_preimage(unit) {
                units.push(holon_core::hex(bytes));
            }
        }
        FlowDocument {
            nodes: top.nodes,
            edges: top.wires,
            viewport,
            units: Some(units),
            modules: (!modules.is_empty()).then_some(modules),
            names: Some(self.names),
            server: Some(settings),
            extra: Map::new(),
        }
    }
}

