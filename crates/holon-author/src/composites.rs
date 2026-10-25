//! A composite's inside, as canvases: its children rebuilt as nodes (ungroup puts them in place of
//! it; expand opens them on a canvas of their own, with its boundary drawn as two nodes), and such
//! a canvas, once edited, frozen back into a composite. The grouping it undoes is group.rs's.

use std::collections::{BTreeMap, HashMap};

use holon_core::{derive_and_freeze, interface_of, parse_hash, EncapSpec, Expose, Hash, MapResolver, Promote, Resolver, Unit, UnitBody, BOUNDARY, VARIADIC};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canvas::{Node, NodeData, PlacedBody, Position, Wire};
use crate::flow::group_of;
use crate::group::{put_unit, GroupError};
use crate::to_hex;
use crate::wire::{WParam, WPort, WType};

/// The canvas ids of a composite's boundary, drawn as two nodes on its inner canvas.
pub const BOUNDARY_IN: &str = "boundary-in";
pub const BOUNDARY_OUT: &str = "boundary-out";

/// What a composite was called on a canvas, by hash: a unit keeps no names, the editor and the
/// documents do.
pub type Names = BTreeMap<String, String>;

/// One port of a composite's boundary as its inner canvas shows it, with what it is wired to
/// outside the composite, in the flow around it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoundaryPort {
    /// As the node shows it (renamable, not identity).
    pub name: String,
    pub ty: WType,
    #[serde(default)]
    pub outside: Vec<String>,
    /// The unit's own name for this port (part of its identity): kept when frozen again, so an
    /// unchanged composite freezes back to the very same unit. Absent for a port added inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Its name on the node when the canvas was opened, so the flow's wires follow a rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoundaryData {
    pub side: String,
    pub ports: Vec<BoundaryPort>,
}

/// A boundary node of an inner canvas: its inputs (`boundary-in`) or its outputs.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BoundaryNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub position: Position,
    pub data: BoundaryData,
}

/// A composite opened as a canvas of its own: its boundary's two nodes, its children as nodes,
/// its inner wires, and which canvas node each child (by index) is.
#[derive(Clone, Debug, PartialEq)]
pub struct InnerCanvas {
    pub inputs: BoundaryNode,
    pub outputs: BoundaryNode,
    pub children: Vec<Node>,
    pub wires: Vec<Wire>,
    pub child_ids: Vec<String>,
}

pub(crate) fn resolve<'a>(store: &'a MapResolver, hash: &str) -> Result<(Hash, &'a Unit), String> {
    let h = parse_hash(hash).ok_or_else(|| format!("bad hash: {hash}"))?;
    let unit = store.resolve(&h).ok_or_else(|| format!("unresolved: {hash}"))?;
    Ok((h, unit))
}

/// A composite's children as canvas nodes, rebuilt from their units (not yet placed, ids from
/// `next_id`). A primitive is as if just dropped from the palette; a nested composite takes the
/// name it had on a canvas (`names`), else the start of its hash.
pub fn child_nodes(children: &[Hash], store: &MapResolver, mut next_id: impl FnMut() -> u64, names: &Names) -> Result<Vec<Node>, String> {
    children
        .iter()
        .map(|h| {
            let hex = to_hex(h);
            let unit = store.resolve(h).ok_or_else(|| format!("unresolved: {hex}"))?;
            let ports = |ps: &[holon_core::Port]| ps.iter().map(WPort::from_core).collect::<Vec<_>>();
            let (id, data) = match &unit.body {
                UnitBody::Primitive { name } => {
                    let mut data = NodeData::new(name, PlacedBody::Primitive, ports(&unit.inputs), ports(&unit.outputs), unit.params.iter().map(WParam::from_core).collect());
                    data.variadic = VARIADIC.contains(&name.as_str());
                    // The catalog's own declared arity, not this (possibly grown) child's: the
                    // same floor a palette drop enforces.
                    data.min_inputs = interface_of(name).map(|u| u.inputs.len()).unwrap_or(unit.inputs.len());
                    (format!("{name}-{}", next_id()), data)
                }
                UnitBody::Composite { .. } => {
                    let name = names.get(&hex).cloned().unwrap_or_else(|| hex[..8].to_string());
                    let data = NodeData::new(&name, PlacedBody::Composite { hash: hex.clone() }, ports(&unit.inputs), ports(&unit.outputs), vec![]);
                    (format!("composite-{}-{}", next_id(), &hex[..8]), data)
                }
                // A module: code by hash, with its ports and its budgets as params, no inside.
                UnitBody::Module { .. } => {
                    let name = names.get(&hex).cloned().unwrap_or_else(|| hex[..8].to_string());
                    let data = NodeData::new(&name, PlacedBody::Module { hash: hex.clone() }, ports(&unit.inputs), ports(&unit.outputs), unit.params.iter().map(WParam::from_core).collect());
                    (format!("module-{}-{}", next_id(), &hex[..8]), data)
                }
            };
            Ok(Node::placed(&id, Position::default(), data))
        })
        .collect()
}

fn composite_parts(unit: &Unit) -> Option<(&[Hash], &[holon_core::Edge])> {
    match &unit.body {
        UnitBody::Composite { children, edges } => Some((children, edges)),
        UnitBody::Primitive { .. } | UnitBody::Module { .. } => None,
    }
}

/// A composite as a canvas of its own, to look inside it without ungrouping: its children as
/// nodes (read only), laid out left to right in the order data flows (a frozen unit keeps no
/// positions), its inner wires, and its boundary as two nodes, its inputs on the left and its
/// outputs on the right, each port labelled with what it is wired to in the flow around it
/// (`nodes`, `wires`).
pub fn inner_canvas(target: &Node, nodes: &[Node], wires: &[Wire], store: &MapResolver, names: &Names) -> Result<InnerCanvas, String> {
    let PlacedBody::Composite { hash } = &target.data.body else { return Err("not a composite".into()) };
    let (_, unit) = resolve(store, hash)?;
    let Some((child_hashes, inner)) = composite_parts(unit) else { return Err("not a composite".into()) };
    let mut n = 0;
    let mut children = child_nodes(child_hashes, store, || { n += 1; n - 1 }, names)?;
    for c in &mut children {
        c.data.extra.insert("readOnly".into(), Value::Bool(true));
    }

    // Columns by how far data has come: a child fed only by the boundary is in column 1.
    let mut depth = vec![1usize; children.len()];
    for _ in 0..children.len() {
        for e in inner.iter().filter(|e| e.from_child != BOUNDARY && e.to_child != BOUNDARY) {
            let from = depth.get(e.from_child as usize).copied().unwrap_or(1);
            if let Some(d) = depth.get_mut(e.to_child as usize) {
                *d = (*d).max(from + 1);
            }
        }
    }
    const COLUMN: f64 = 250.0;
    const ROW: f64 = 150.0;
    let last = depth.iter().copied().max().unwrap_or(1).max(1) + 1;
    let mut rows: HashMap<usize, usize> = HashMap::new();
    for (c, &d) in children.iter_mut().zip(&depth) {
        let row = rows.entry(d).or_insert(0);
        c.position = Some(Position { x: d as f64 * COLUMN, y: *row as f64 * ROW });
        *row += 1;
    }

    // What each boundary port is wired to outside: the node (with its port when it has several).
    let label = |id: &str, handle: Option<&str>, outputs: bool| -> String {
        let Some(node) = nodes.iter().find(|x| x.id == id) else { return id.to_string() };
        let ports = if outputs { &node.data.outputs } else { &node.data.inputs };
        match handle {
            Some(h) if ports.len() > 1 && !h.is_empty() => format!("{}.{h}", node.data.name),
            _ => node.data.name.clone(),
        }
    };
    let ins = target.data.inputs.iter().enumerate().map(|(i, p)| BoundaryPort {
        name: p.name.clone(),
        was: Some(p.name.clone()),
        unit: unit.inputs.get(i).map(|q| q.name.clone()),
        ty: p.ty.clone(),
        outside: wires.iter().filter(|e| e.target == target.id && e.input() == p.name).map(|e| label(&e.source, e.source_handle.as_deref(), true)).collect(),
    });
    let outs = target.data.outputs.iter().enumerate().map(|(i, p)| BoundaryPort {
        name: p.name.clone(),
        was: Some(p.name.clone()),
        unit: unit.outputs.get(i).map(|q| q.name.clone()),
        ty: p.ty.clone(),
        outside: wires.iter().filter(|e| e.source == target.id && e.output() == p.name).map(|e| label(&e.target, e.target_handle.as_deref(), false)).collect(),
    });
    let middle = ((rows.values().copied().max().unwrap_or(1).max(1) - 1) as f64 * ROW) / 2.0;
    let boundary = |id: &str, side: &str, x: f64, ports: Vec<BoundaryPort>| BoundaryNode {
        id: id.into(),
        kind: "boundary".into(),
        position: Position { x, y: middle },
        data: BoundaryData { side: side.into(), ports },
    };

    let wires = inner
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let (from_id, from_port) = if e.from_child == BOUNDARY {
                (BOUNDARY_IN.to_string(), target.data.inputs.get(e.from_port as usize)?)
            } else {
                let c = children.get(e.from_child as usize)?;
                (c.id.clone(), c.data.outputs.get(e.from_port as usize)?)
            };
            let (to_id, to_port) = if e.to_child == BOUNDARY {
                (BOUNDARY_OUT.to_string(), target.data.outputs.get(e.to_port as usize)?)
            } else {
                let c = children.get(e.to_child as usize)?;
                (c.id.clone(), c.data.inputs.get(e.to_port as usize)?)
            };
            Some(Wire { id: Some(format!("inner-{i}")), ..Wire::new(&from_id, &from_port.name, &to_id, &to_port.name) })
        })
        .collect();
    let child_ids = children.iter().map(|c| c.id.clone()).collect();
    Ok(InnerCanvas {
        inputs: boundary(BOUNDARY_IN, "in", -60.0, ins.collect()),
        outputs: boundary(BOUNDARY_OUT, "out", last as f64 * COLUMN, outs.collect()),
        children,
        wires,
        child_ids,
    })
}

/// A composite expanded in place: its children, the flow's wires re-pointed, its inner wires.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Expanded {
    pub nodes: Vec<Node>,
    pub rewired: Vec<Wire>,
    pub added: Vec<Wire>,
}

/// A placed composite expanded back into its children, in its place (the dual of grouping). One
/// level only: a nested composite is revealed still collapsed. The flow's wires to the
/// composite's ports are re-pointed at the child each port leads to (`rewired`, every other wire
/// as it was); the composite's inner wiring becomes wires of the flow (`added`, as a drawn wire
/// is added: by `xy-edge__…` id, once). Each child's id comes from `ids`, in order.
pub fn expand_composite(target: &Node, wires: &[Wire], store: &MapResolver, ids: &[u64], names: &Names) -> Result<Expanded, String> {
    let PlacedBody::Composite { hash } = &target.data.body else { return Err("expand: not a composite".into()) };
    let (_, unit) = resolve(store, hash)?;
    let Some((child_hashes, inner)) = composite_parts(unit) else { return Err("expand: not a composite".into()) };
    let mut next = ids.iter().copied();
    let mut fallback = ids.len() as u64;
    let mut nodes = child_nodes(child_hashes, store, || next.next().unwrap_or_else(|| { fallback += 1; fallback - 1 }), names)?;
    let spacing = 220.0;
    let at = target.position();
    let start = at.x - ((nodes.len() as f64 - 1.0) * spacing) / 2.0;
    for (i, n) in nodes.iter_mut().enumerate() {
        n.position = Some(Position { x: start + i as f64 * spacing, y: at.y });
    }

    let rewired: Vec<Wire> = wires
        .iter()
        .filter_map(|e| {
            if e.target == target.id {
                let i = target.data.inputs.iter().position(|p| Some(&p.name) == e.target_handle.as_ref())? as u32;
                let be = inner.iter().find(|x| x.from_child == BOUNDARY && x.from_port == i)?;
                let child = nodes.get(be.to_child as usize)?;
                let handle = child.data.inputs.get(be.to_port as usize)?.name.clone();
                return Some(Wire { target: child.id.clone(), target_handle: Some(handle), ..e.clone() });
            }
            if e.source == target.id {
                let i = target.data.outputs.iter().position(|p| Some(&p.name) == e.source_handle.as_ref())? as u32;
                let be = inner.iter().find(|x| x.to_child == BOUNDARY && x.to_port == i)?;
                let child = nodes.get(be.from_child as usize)?;
                let handle = child.data.outputs.get(be.from_port as usize)?.name.clone();
                return Some(Wire { source: child.id.clone(), source_handle: Some(handle), ..e.clone() });
            }
            Some(e.clone())
        })
        .collect();

    let mut added: Vec<Wire> = Vec::new();
    for e in inner.iter().filter(|e| e.from_child != BOUNDARY && e.to_child != BOUNDARY) {
        let (Some(src), Some(dst)) = (nodes.get(e.from_child as usize), nodes.get(e.to_child as usize)) else { continue };
        let (Some(out), Some(input)) = (src.data.outputs.get(e.from_port as usize), dst.data.inputs.get(e.to_port as usize)) else { continue };
        let wire = Wire { id: Some(format!("xy-edge__{}{}-{}{}", src.id, out.name, dst.id, input.name)), ..Wire::new(&src.id, &out.name, &dst.id, &input.name) };
        let same = |w: &Wire| w.source == wire.source && w.target == wire.target && w.source_handle == wire.source_handle && w.target_handle == wire.target_handle;
        if !rewired.iter().chain(&added).any(same) {
            added.push(wire);
        }
    }
    Ok(Expanded { nodes, rewired, added })
}

/// An inner canvas (as `inner_canvas` opened it, then edited) frozen back into a composite: its
/// nodes the children, its wires the inner edges, and its boundary exactly as drawn, each input
/// promoting the one inner port it feeds and each output exposing the inner output that feeds it,
/// in their order and under their names (order and names are part of the unit's identity). An
/// input wired to nothing, or to more than one port, and an output fed by nothing, are refused,
/// saying which; so is anything core's freeze refuses. A port keeps the unit's own name (`unit`),
/// so an unchanged canvas freezes back to the very same unit. Answers the unit's hash and the
/// node's ports (the boundary's names).
pub fn freeze_inner(ins: &[BoundaryPort], outs: &[BoundaryPort], inner: &[Node], wires: &[Wire], store: &mut MapResolver) -> Result<(String, Vec<WPort>, Vec<WPort>), GroupError> {
    if inner.is_empty() {
        return Err(GroupError::new("nothing inside: a composite holds at least one node"));
    }
    let index: HashMap<&str, usize> = inner.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let inner_wires: Vec<Wire> = wires.iter().filter(|e| index.contains_key(e.source.as_str()) && index.contains_key(e.target.as_str())).cloned().collect();
    let mut promote = Vec::new();
    for p in ins {
        let fed: Vec<&Wire> = wires.iter().filter(|e| e.source == BOUNDARY_IN && e.source_handle.as_deref() == Some(p.name.as_str())).collect();
        let unwired = || GroupError::new(format!("the input \"{}\" isn't wired to anything inside: wire it, or remove it", p.name));
        match fed.as_slice() {
            [] => return Err(unwired()),
            [one] => {
                let child = *index.get(one.target.as_str()).ok_or_else(unwired)?;
                let port = inner[child].data.inputs.iter().position(|q| Some(&q.name) == one.target_handle.as_ref()).ok_or_else(unwired)?;
                promote.push(Promote { child: child as u32, port: port as u32, name: p.unit.clone().unwrap_or_else(|| p.name.clone()) });
            }
            many => {
                return Err(GroupError {
                    message: format!("the input \"{}\" feeds {} ports: an input feeds one, so pass it through a node (a default, say) to share it", p.name, many.len()),
                    node_ids: many.iter().map(|e| e.target.clone()).collect(),
                })
            }
        }
    }
    let mut expose = Vec::new();
    for p in outs {
        let unfed = || GroupError::new(format!("the output \"{}\" isn't fed by anything inside: wire it, or remove it", p.name));
        let from = wires.iter().find(|e| e.target == BOUNDARY_OUT && e.target_handle.as_deref() == Some(p.name.as_str())).ok_or_else(unfed)?;
        let child = *index.get(from.source.as_str()).ok_or_else(unfed)?;
        let port = inner[child].data.outputs.iter().position(|q| Some(&q.name) == from.source_handle.as_ref()).ok_or_else(unfed)?;
        expose.push(Expose { child: child as u32, port: port as u32, name: p.unit.clone().unwrap_or_else(|| p.name.clone()) });
    }
    let group: Vec<&Node> = inner.iter().collect();
    let (children, edges) = group_of(&group, &inner_wires, store)?;
    let unit = derive_and_freeze(children, edges, &EncapSpec { promote, expose }, store).map_err(|e| GroupError::new(e.to_string()))?;
    // The node keeps showing the boundary's names; the unit keeps its own.
    let named = |ports: &[holon_core::Port], shown: &[BoundaryPort]| -> Vec<WPort> {
        ports.iter().enumerate().map(|(i, q)| WPort { name: shown.get(i).map(|p| p.name.clone()).unwrap_or_else(|| q.name.clone()), ty: WType::from_core(&q.ty) }).collect()
    };
    let (inputs, outputs) = (named(&unit.inputs, ins), named(&unit.outputs, outs));
    let hash = put_unit(store, unit)?;
    Ok((to_hex(&hash), inputs, outputs))
}
