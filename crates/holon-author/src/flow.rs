//! A flow's units: each node's own unit, the server its routes derive into, and the one unit the
//! flow deploys as. Each reads what core computes (the proposed boundary, the frozen composite)
//! rather than re-deciding it.

use std::collections::{BTreeMap, HashMap};

use holon_core::{
    canonical_child_order, default_spec, derive_and_freeze, parse_hash, Edge, Hash, MapResolver, Unit, UnitBody,
};
use serde::Serialize;

use crate::canvas::{Node, PlacedBody, Wire};
use crate::routes::{endpoint_unit, find_routes, route_spec, server_unit};
use crate::wire::WPort;
use crate::{templates, to_hex};

fn put(store: &mut MapResolver, u: Unit) -> Result<Hash, String> {
    store.insert(u).map_err(|e| format!("{e:?}"))
}

/// A node's own unit, stored, by hash. A primitive is rebuilt from its name, params and ports (the
/// same primitive always hashes the same, so rebuilding is referencing); a template from its
/// params; a grouped composite was frozen when it was grouped, so it is its hash (which `store`
/// must already hold to be used).
pub fn unit_for(node: &Node, store: &mut MapResolver) -> Result<Hash, String> {
    match &node.data.body {
        PlacedBody::Composite { hash } => parse_hash(hash).ok_or_else(|| format!("{}: bad hash {hash}", node.data.name)),
        PlacedBody::Template => {
            let unit = templates::build(&node.data.name, &node.data.params, store).map_err(|e| format!("{}: {e}", node.data.name))?;
            put(store, unit)
        }
        PlacedBody::Primitive => {
            let ports = |ps: &[WPort]| ps.iter().cloned().map(WPort::into_core).collect::<Result<Vec<_>, _>>();
            let unit = Unit {
                body: UnitBody::Primitive { name: node.data.name.clone() },
                params: node.data.params.iter().cloned().map(|p| p.into_core()).collect(),
                inputs: ports(&node.data.inputs)?,
                outputs: ports(&node.data.outputs)?,
            };
            put(store, unit)
        }
    }
}

/// A group of nodes as core children (each node's hash) plus the wires among them, by index. A
/// wire with an end outside the group is left out: from the group's side it is an open input or
/// output.
pub fn group_of(group: &[&Node], wires: &[Wire], store: &mut MapResolver) -> Result<(Vec<Hash>, Vec<Edge>), String> {
    let index: HashMap<&str, usize> = group.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let children = group.iter().map(|n| unit_for(n, store)).collect::<Result<Vec<_>, _>>()?;
    let mut edges = Vec::new();
    for w in wires {
        let (Some(&from), Some(&to)) = (index.get(w.source.as_str()), index.get(w.target.as_str())) else { continue };
        let port = |ports: &[WPort], handle: &Option<String>, side: &str, node: &Node| {
            let h = handle.as_deref().unwrap_or("");
            ports.iter().position(|p| p.name == h).map(|i| i as u32).ok_or_else(|| format!("{} has no {side} \"{h}\"", node.data.name))
        };
        edges.push(Edge {
            from_child: from as u32,
            from_port: port(&group[from].data.outputs, &w.source_handle, "output", group[from])?,
            to_child: to as u32,
            to_port: port(&group[to].data.inputs, &w.target_handle, "input", group[to])?,
        });
    }
    Ok((children, edges))
}

/// Which node each child of a frozen composite is, in the order the unit is stored (and run, and
/// timed) in: core sorts a composite's children canonically when it hashes it.
pub fn trace_of(unit: &Unit, group: &[&Node]) -> Vec<String> {
    match &unit.body {
        UnitBody::Composite { children, edges } => canonical_child_order(children, edges)
            .map(|order| order.into_iter().filter_map(|i| group.get(i).map(|n| n.id.clone())).collect())
            .unwrap_or_default(),
        UnitBody::Primitive { .. } => vec![],
    }
}

/// One endpoint of the derived server: its route, and the endpoint unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EndpointDraft {
    pub method: String,
    pub path: String,
    pub hash: String,
}

/// The server a flow declares: its endpoints, why any route can't be one, the server unit's hash
/// once every route is sound and the port is a port, and, for each route, which node each of its
/// handler's stored children is.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct DerivedServer {
    pub endpoints: Vec<EndpointDraft>,
    pub errors: Vec<String>,
    #[serde(rename = "serverHash")]
    pub server_hash: Option<String>,
    pub trace: BTreeMap<String, Vec<String>>,
}

/// A port as a flow's settings hold it: an integer 1–65535.
fn port_of(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|&p| p >= 1)
}

/// Derive the server from the flow: each connected group holding a request node is one route
/// (routes.rs). Each group is grouped through core like a selection, narrowed to the request input
/// and the response and effect outputs, and wrapped as an endpoint. Every unit made is stored.
pub fn derive_server(nodes: &[Node], wires: &[Wire], store: &mut MapResolver, port: &str) -> DerivedServer {
    let (routes, mut errors) = find_routes(nodes, wires);
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut endpoints = Vec::new();
    let mut trace = BTreeMap::new();
    for r in routes {
        let label = format!("{} {}", r.method, r.path);
        let group: Vec<&Node> = r.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let endpoint = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match route_spec(proposed, &group, &label) {
                Ok(spec) => spec,
                // Already says which route.
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(label.clone(), trace_of(&handler, &group));
            let (inputs, outputs) = (handler.inputs.clone(), handler.outputs.clone());
            let handler = put(store, handler)?;
            let unit = endpoint_unit(handler, &inputs, &outputs, &r.method, &r.path, &r.resume)?;
            Ok(Ok(put(store, unit)?))
        })();
        match endpoint {
            Ok(Ok(hash)) => endpoints.push(EndpointDraft { method: r.method, path: r.path, hash: to_hex(&hash) }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{label}: {e}")),
        }
    }
    let server_hash = match port_of(port) {
        Some(port) if !endpoints.is_empty() && errors.is_empty() => {
            let hashes = endpoints.iter().map(|e| parse_hash(&e.hash).expect("hashes made here")).collect();
            put(store, server_unit(hashes, port)).ok().map(|h| to_hex(&h))
        }
        _ => None,
    };
    DerivedServer { endpoints, errors, server_hash, trace }
}

/// The unit a flow deploys as, stored: its server when it has routes, else the whole flow grouped
/// as one composite. Why it can't be, otherwise.
pub fn flow_unit(nodes: &[Node], wires: &[Wire], store: &mut MapResolver, port: &str) -> Result<Hash, String> {
    let derived = derive_server(nodes, wires, store, port);
    if !derived.errors.is_empty() {
        return Err(derived.errors.join("; "));
    }
    if !derived.endpoints.is_empty() {
        let hash = derived.server_hash.ok_or_else(|| format!("\"{port}\" is not a port (set it under ⚙)"))?;
        return Ok(parse_hash(&hash).expect("a hash made here"));
    }
    if nodes.is_empty() {
        return Err("the canvas is empty".into());
    }
    let all: Vec<&Node> = nodes.iter().collect();
    let (children, edges) = group_of(&all, wires, store)?;
    let spec = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
    let whole = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
    put(store, whole)
}
