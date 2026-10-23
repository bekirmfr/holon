//! A flow as a flow document holds it: placed nodes and the wires between their named ports.
//! The editor's canvas nodes carry more (a position, what's selected, a run's trace); only what
//! decides the flow's units is read here, the rest is ignored.

use serde::{Deserialize, Serialize};

use crate::wire::{frozen_value, WParam, WPort};

/// A placed node.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub data: NodeData,
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
    #[serde(default)]
    pub params: Vec<WParam>,
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
    /// The frozen param `name`, or "" when it isn't set.
    pub fn param(&self, name: &str) -> &str {
        frozen_value(&self.data.params, name).unwrap_or("")
    }
    pub fn is_template(&self, name: &str) -> bool {
        self.data.body == PlacedBody::Template && self.data.name == name
    }
}

/// A wire from one node's output to another's input, by port name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Wire {
    pub source: String,
    pub target: String,
    #[serde(rename = "sourceHandle", default)]
    pub source_handle: Option<String>,
    #[serde(rename = "targetHandle", default)]
    pub target_handle: Option<String>,
}
