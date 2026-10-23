//! A flow as a flow document holds it: placed nodes and the wires between their named ports. The
//! editor's canvas nodes and wires carry more (what's selected, a run's trace, a wire's colour):
//! what isn't read here is kept as it was (`extra`), so a node or wire passed through comes back
//! whole.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::wire::{frozen_value, WParam, WPort};

/// Where a node sits on the canvas.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// A placed node.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// The canvas's node type: "primitive" for every placed node.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    pub data: NodeData,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NodeData {
    /// A primitive's or template's name; a grouped composite's label.
    pub name: String,
    pub body: PlacedBody,
    #[serde(default)]
    pub inputs: Vec<WPort>,
    #[serde(default)]
    pub outputs: Vec<WPort>,
    /// A template's inputs that its endpoint feeds, not drawn as handles (templates.rs).
    #[serde(rename = "hiddenInputs", default, skip_serializing_if = "Option::is_none")]
    pub hidden_inputs: Option<bool>,
    #[serde(default)]
    pub params: Vec<WParam>,
    /// A primitive that takes more inputs than it declares (core's `VARIADIC`).
    #[serde(default)]
    pub variadic: bool,
    /// The fewest inputs it takes: its declared ones.
    #[serde(rename = "minInputs", default)]
    pub min_inputs: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What a node stands for: a genesis primitive (rebuilt from its name, params and ports), a
/// frozen composite (by hash), or a template (rebuilt from its params, templates.rs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum PlacedBody {
    Primitive,
    Composite { hash: String },
    Template,
}

impl Node {
    /// A node placed as the canvas places one: of type "primitive", at `position`.
    pub fn placed(id: &str, position: Position, data: NodeData) -> Node {
        Node { id: id.into(), kind: Some("primitive".into()), position: Some(position), data, extra: Map::new() }
    }
    /// The frozen param `name`, or "" when it isn't set.
    pub fn param(&self, name: &str) -> &str {
        frozen_value(&self.data.params, name).unwrap_or("")
    }
    pub fn is_template(&self, name: &str) -> bool {
        self.data.body == PlacedBody::Template && self.data.name == name
    }
    pub fn position(&self) -> Position {
        self.position.unwrap_or_default()
    }
}

impl NodeData {
    pub fn new(name: &str, body: PlacedBody, inputs: Vec<WPort>, outputs: Vec<WPort>, params: Vec<WParam>) -> NodeData {
        NodeData { name: name.into(), body, inputs, outputs, hidden_inputs: None, params, variadic: false, min_inputs: 0, extra: Map::new() }
    }
}

/// A wire from one node's output to another's input, by port name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Wire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub source: String,
    #[serde(rename = "sourceHandle", default, skip_serializing_if = "Option::is_none")]
    pub source_handle: Option<String>,
    pub target: String,
    #[serde(rename = "targetHandle", default, skip_serializing_if = "Option::is_none")]
    pub target_handle: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Wire {
    pub fn new(source: &str, output: &str, target: &str, input: &str) -> Wire {
        Wire {
            id: None,
            source: source.into(),
            source_handle: Some(output.into()),
            target: target.into(),
            target_handle: Some(input.into()),
            extra: Map::new(),
        }
    }
    pub fn output(&self) -> &str {
        self.source_handle.as_deref().unwrap_or("")
    }
    pub fn input(&self) -> &str {
        self.target_handle.as_deref().unwrap_or("")
    }
}

/// The input a variadic node gains next (its "+", the text format's wires): the next letter
/// while they are single letters (a, b → c), else `in<n>`.
pub fn next_port_name(existing: &[&str]) -> String {
    let single = !existing.is_empty() && existing.iter().all(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_lowercase());
    if single {
        let max = existing.iter().map(|n| n.as_bytes()[0]).max().unwrap_or(b'a');
        if max < b'z' {
            return ((max + 1) as char).to_string();
        }
    }
    format!("in{}", existing.len() + 1)
}
