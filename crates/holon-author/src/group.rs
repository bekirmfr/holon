//! Grouping: a selection of a flow's nodes made one placed composite, as the editor's "group"
//! does, and a saved flow placed in another as one node (a holon). Core proposes the boundary
//! (`default_spec`) and freezes it; this only marshals the nodes in and the ports' names out.

use std::collections::{HashMap, HashSet};

use holon_core::{default_spec, derive_and_freeze, EncapError, Hash, MapResolver, Unit, UnitBody, ValidationError, BOUNDARY};
use serde::Serialize;
use serde_json::json;

use crate::canvas::{Node, NodeData, PlacedBody, Position, Wire};
use crate::flow::unit_for;
use crate::to_hex;
use crate::wire::WPort;

/// Why grouping (or freezing a composite) failed, and the nodes at fault, to point at.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct GroupError {
    pub message: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

impl GroupError {
    pub fn new(message: impl Into<String>) -> GroupError {
        GroupError { message: message.into(), node_ids: vec![] }
    }
}

impl From<String> for GroupError {
    fn from(message: String) -> GroupError {
        GroupError::new(message)
    }
}

/// A unit stored, as the editor stores one it was handed.
pub(crate) fn put_unit(store: &mut MapResolver, unit: Unit) -> Result<Hash, String> {
    store.insert(unit).map_err(|e| format!("{e}"))
}

/// Which of the grouped children (by index) a ValidationError implicates. `TypeMismatch` carries
/// only an edge, resolved through the edges grouped; a cycle or a boundary port out of range
/// carries none.
fn validation_children(err: &ValidationError, edges: &[holon_core::Edge]) -> Vec<u32> {
    match err {
        ValidationError::ChildOutOfRange { child, .. }
        | ValidationError::UnresolvedChild { child }
        | ValidationError::OutputPortOutOfRange { child, .. }
        | ValidationError::InputPortOutOfRange { child, .. } => vec![*child],
        ValidationError::DuplicateInputEdge { to_child, .. } => vec![*to_child],
        ValidationError::TypeMismatch { edge, .. } => {
            edges.get(*edge).map(|e| [e.from_child, e.to_child].into_iter().filter(|&c| c != BOUNDARY).collect()).unwrap_or_default()
        }
        ValidationError::BoundaryInputOutOfRange { .. } | ValidationError::BoundaryOutputOutOfRange { .. } | ValidationError::Cycle | ValidationError::Module(_) | ValidationError::FamilyParam { .. } => vec![],
    }
}

/// An EncapError said, with the nodes it implicates.
pub(crate) fn encap_error(err: &EncapError, edges: &[holon_core::Edge], nodes: &[&Node]) -> GroupError {
    let children = match err {
        EncapError::UnresolvedChild(c) | EncapError::PromoteChildOutOfRange(c) | EncapError::ExposeChildOutOfRange(c) => vec![*c],
        EncapError::PromotePortOutOfRange { child, .. }
        | EncapError::PromoteAlreadyBound { child, .. }
        | EncapError::DuplicatePromotion { child, .. }
        | EncapError::ExposePortInvalid { child, .. } => vec![*child],
        EncapError::Invalid(errs) => errs.iter().flat_map(|e| validation_children(e, edges)).collect(),
    };
    GroupError {
        message: err.to_string(),
        node_ids: children.into_iter().filter_map(|c| nodes.get(c as usize).map(|n| n.id.clone())).collect(),
    }
}

/// A composite's ports named by where they came from (`add · b`, not a bare `b`), numbered when
/// two would read the same.
pub fn boundary_labels(ports: &[WPort], source: &[Option<String>]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let label = match source.get(i).and_then(|s| s.as_deref()) {
                Some(s) if !s.is_empty() => format!("{s} · {}", p.name),
                _ => p.name.clone(),
            };
            let n = seen.entry(label.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { label } else { format!("{label} ({n})") }
        })
        .collect()
}

/// A selection of nodes grouped into one placed composite node, and the flow's wires: those
/// inside the selection gone (they are the composite's own), those crossing it re-pointed at the
/// composite's ports, the rest as they were. Its ports are named by provenance (renamed after
/// hashing, so names never touch identity); it sits at the selection's centre, `composite-<id>-…`.
pub fn group_selection(selected: &[Node], wires: &[Wire], store: &mut MapResolver, id: u64) -> Result<(Node, Vec<Wire>), GroupError> {
    let ids: HashSet<&str> = selected.iter().map(|n| n.id.as_str()).collect();
    let index: HashMap<&str, usize> = selected.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let refs: Vec<&Node> = selected.iter().collect();
    let children = selected.iter().map(|n| unit_for(n, store)).collect::<Result<Vec<_>, _>>()?;
    let mut edges = Vec::new();
    for w in wires.iter().filter(|w| ids.contains(w.source.as_str()) && ids.contains(w.target.as_str())) {
        let (from, to) = (index[w.source.as_str()], index[w.target.as_str()]);
        let port = |ports: &[WPort], h: &str| ports.iter().position(|p| p.name == h).map(|i| i as u32);
        let (Some(from_port), Some(to_port)) = (port(&selected[from].data.outputs, w.output()), port(&selected[to].data.inputs, w.input())) else {
            return Err(GroupError::new(format!("a wire references a port {} or {} doesn't have", selected[from].data.name, selected[to].data.name)));
        };
        edges.push(holon_core::Edge { from_child: from as u32, from_port, to_child: to as u32, to_port });
    }
    let spec = default_spec(&children, &edges, store).map_err(|e| encap_error(&e, &edges, &refs))?;
    let composite = derive_and_freeze(children, edges.clone(), &spec, store).map_err(|e| encap_error(&e, &edges, &refs))?;
    let (raw_inputs, raw_outputs) = (composite.inputs.clone(), composite.outputs.clone());
    let boundary = match &composite.body {
        UnitBody::Composite { edges, .. } => edges.clone(),
        UnitBody::Primitive { .. } | UnitBody::Module { .. } => vec![],
    };
    let hash = to_hex(&put_unit(store, composite)?);

    // Which (node, port) each promoted input and exposed output came from, off core's boundary.
    let promoted: Vec<Option<(u32, u32)>> = (0..raw_inputs.len() as u32)
        .map(|i| boundary.iter().find(|e| e.from_child == BOUNDARY && e.from_port == i).map(|e| (e.to_child, e.to_port)))
        .collect();
    let exposed: Vec<Option<(u32, u32)>> = (0..raw_outputs.len() as u32)
        .map(|i| boundary.iter().find(|e| e.to_child == BOUNDARY && e.to_port == i).map(|e| (e.from_child, e.from_port)))
        .collect();
    let named = |ports: &[holon_core::Port], from: &[Option<(u32, u32)>]| -> Vec<WPort> {
        let ports: Vec<WPort> = ports.iter().map(WPort::from_core).collect();
        let source: Vec<Option<String>> = from.iter().map(|x| x.and_then(|(c, _)| selected.get(c as usize)).map(|n| n.data.name.clone())).collect();
        let labels = boundary_labels(&ports, &source);
        ports.into_iter().zip(labels).map(|(p, name)| WPort { name, ..p }).collect()
    };
    let inputs = named(&raw_inputs, &promoted);
    let outputs = named(&raw_outputs, &exposed);

    let group_id = format!("composite-{id}-{}", &hash[..8]);
    let count = selected.len() as f64;
    let position = Position {
        x: selected.iter().fold(0.0, |s, n| s + n.position().x) / count,
        y: selected.iter().fold(0.0, |s, n| s + n.position().y) / count,
    };
    let data = NodeData::new(&hash[..8], PlacedBody::Composite { hash: hash.clone() }, inputs.clone(), outputs.clone(), vec![]);
    let node = Node::placed(&group_id, position, data);

    let port_of = |ports: &[WPort], h: &str| ports.iter().position(|p| p.name == h).map(|i| i as u32);
    let rewired = wires
        .iter()
        .filter_map(|w| {
            let (s_in, t_in) = (ids.contains(w.source.as_str()), ids.contains(w.target.as_str()));
            if s_in && t_in {
                return None;
            }
            if s_in {
                let ci = index[w.source.as_str()];
                let cp = port_of(&selected[ci].data.outputs, w.output());
                let oi = exposed.iter().position(|x| x.is_some() && *x == cp.map(|p| (ci as u32, p)))?;
                return Some(Wire { source: group_id.clone(), source_handle: Some(outputs[oi].name.clone()), ..w.clone() });
            }
            if t_in {
                let ci = index[w.target.as_str()];
                let cp = port_of(&selected[ci].data.inputs, w.input());
                let ii = promoted.iter().position(|x| x.is_some() && *x == cp.map(|p| (ci as u32, p)))?;
                return Some(Wire { target: group_id.clone(), target_handle: Some(inputs[ii].name.clone()), ..w.clone() });
            }
            Some(w.clone())
        })
        .collect();
    Ok((node, rewired))
}

/// A saved flow placed in another as one node: a holon. Its whole canvas is grouped into one
/// composite exactly as "group" does, and the node takes the flow's name (and, given `saved_at`,
/// remembers which flow and when, so an update can be offered later). `store` must hold the
/// flow's own composites. A flow with routes is a server, answered by the host, not a part of
/// another flow: it is refused, as is an empty one. So is a flow with any other handler (a clock,
/// a file's, a mail's, a feed's or a queue's run, an action): what starts its runs is the host's to
/// do for a deployment, and grouped into a node its period, folder or name would be lost unsaid.
pub fn flow_as_node(name: &str, nodes: &[Node], wires: &[Wire], store: &mut MapResolver, id: &str, position: Position, saved_at: Option<f64>) -> Result<Node, String> {
    if nodes.is_empty() {
        return Err(format!("\"{name}\" is empty: nothing to place"));
    }
    if nodes.iter().any(|n| n.is_template("request") || n.is_template("respond")) {
        return Err(format!("\"{name}\" has routes: it is a server, deployed on its own, not a node of another flow"));
    }
    if let Some(label) = nodes.iter().find_map(crate::routes::trigger_label) {
        return Err(format!("\"{name}\" has a handler of its own ({label}): it is deployed on its own, not a node of another flow"));
    }
    let (mut node, _) = group_selection(nodes, wires, store, 0).map_err(|e| format!("\"{name}\" can't be placed as a node: {}", e.message))?;
    node.id = id.into();
    node.position = Some(position);
    node.data.name = name.into();
    if let Some(saved_at) = saved_at {
        node.data.extra.insert("source".into(), json!({ "flow": name, "savedAt": saved_at }));
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FlowBuilder;
    use crate::templates::TEMPLATES;

    /// **Guards:** a flow that holds a handler of any kind is refused as a node of another flow,
    /// by a message that says which handler; a flow of none is placed.
    ///
    /// **Why:** what starts a handler's runs (a route's path, a clock's period, a folder, a mailbox,
    /// a feed, a queue, an action's name) is read by the host from a deployment, and grouping a
    /// canvas into a composite keeps none of it. The refusal named routes alone, so each other
    /// kind was a case of its own: an action's flow was refused as having routes, and a clock's or
    /// a file's was placed, as a node that writes on every run of the flow it sits in.
    ///
    /// **If it fails:** a person drops a saved flow into another and gets a node that looks whole
    /// and has lost its schedule or its folder, or a refusal that names something the flow doesn't
    /// have. A template added without saying which it is fails here too.
    #[test]
    fn a_flow_with_a_handler_of_any_kind_is_refused_as_a_node_by_name() {
        // The templates that start no run, each said so here: a new template is one or the other.
        const NO_TRIGGER: &[&str] = &["respond", "stripe_verify"];
        for name in TEMPLATES {
            let mut b = FlowBuilder::new();
            let id = b.node(name, &[], None).unwrap_or_else(|e| panic!("{name}: {e}"));
            let doc = b.finish(Default::default());
            let node = doc.nodes.iter().find(|n| n.id == id).expect("placed");
            let label = crate::routes::trigger_label(node);
            assert_eq!(label.is_none(), NO_TRIGGER.contains(name), "{name}: a template starts runs or it doesn't; say which");
            let Some(label) = label else { continue };
            let refused = flow_as_node("saved", &doc.nodes, &doc.edges, &mut MapResolver::new(), "n", Position::default(), None).expect_err(name);
            assert!(refused.starts_with("\"saved\" has "), "{name}: {refused}");
            let routed = matches!(*name, "request");
            assert_eq!(refused.contains("has routes"), routed, "{name}: {refused}");
            assert!(routed || refused.contains(&label), "{name}: the refusal names its handler, {label}: {refused}");
        }
        // A flow of no handler is placed, its free ports the node's.
        let mut b = FlowBuilder::new();
        b.node("add", &[], None).unwrap();
        let mut doc = b.finish(Default::default());
        let mut store = MapResolver::new();
        for u in holon_core::genesis_units() {
            store.insert(u).expect("a genesis unit stores");
        }
        crate::text::restore(&doc, &mut store, &Default::default());
        let placed = flow_as_node("sum", &std::mem::take(&mut doc.nodes), &doc.edges, &mut store, "n", Position::default(), None).expect("placed");
        assert_eq!((placed.data.name.as_str(), placed.data.inputs.len(), placed.data.outputs.len()), ("sum", 2, 1));
    }
}
