//! The JSON forms of core's types: how a unit, a port, a param and a type cross to the editor
//! (through holon-wasm) and sit in a flow document. Core itself has no serde (it stays
//! blake3-only); these mirror it exactly, and convert through the one core encoder.

use holon_core::{parse_hash, Edge, Param, Port, Type, Unit, UnitBody, MAX_TYPE_DEPTH};
use serde::{Deserialize, Serialize};

use crate::to_hex;

/// A wire type. Mirrors `core::Type`, including its recursive shape. Serde's default external
/// tagging gives the editor's shape: a nullary variant is a bare string (`"Number"`), one carrying
/// data a single-key object (`{"List": "Number"}`, `{"Product": ["Number","Text"]}`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum WType {
    Number,
    Boolean,
    Text,
    /// A homogeneous list of `T`. `{"List": "Number"}`.
    List(Box<WType>),
    /// A positional n-ary tuple. `Pair<A,B>` = `{"Product": [A, B]}`; `Unit` = `{"Product": []}`.
    Product(Vec<WType>),
    /// A tagged choice. `Option<T>` = `{"Sum": [T, {"Product": []}]}`.
    Sum(Vec<WType>),
    Effect,
    Any,
    Record,
}

impl WType {
    pub fn from_core(t: &Type) -> WType {
        match t {
            Type::Number => WType::Number,
            Type::Boolean => WType::Boolean,
            Type::Text => WType::Text,
            Type::List(inner) => WType::List(Box::new(WType::from_core(inner))),
            Type::Product(elems) => WType::Product(elems.iter().map(WType::from_core).collect()),
            Type::Sum(elems) => WType::Sum(elems.iter().map(WType::from_core).collect()),
            Type::Effect => WType::Effect,
            Type::Any => WType::Any,
            Type::Record => WType::Record,
        }
    }

    pub fn into_core(self) -> Result<Type, String> {
        self.into_core_at(0)
    }

    /// Depth-bounded, mirroring core's own `Decoder::ty_at` and for the identical reason: this is
    /// an untrusted-input boundary (any JS on the page, any document opened), and a type can nest.
    /// Reuses `MAX_TYPE_DEPTH` rather than a parallel bound that could drift from core's.
    fn into_core_at(self, depth: u32) -> Result<Type, String> {
        if depth > MAX_TYPE_DEPTH {
            return Err("type nested too deep".into());
        }
        let all = |elems: Vec<WType>| elems.into_iter().map(|e| e.into_core_at(depth + 1)).collect::<Result<Vec<_>, _>>();
        Ok(match self {
            WType::Number => Type::Number,
            WType::Boolean => Type::Boolean,
            WType::Text => Type::Text,
            WType::List(inner) => Type::List(Box::new(inner.into_core_at(depth + 1)?)),
            WType::Product(elems) => Type::Product(all(elems)?),
            WType::Sum(elems) => Type::Sum(all(elems)?),
            WType::Effect => Type::Effect,
            WType::Any => Type::Any,
            WType::Record => Type::Record,
        })
    }
}

/// A param: `{ kind: "Frozen", name, value }` or `{ kind: "Open", name, bind }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum WParam {
    Frozen { name: String, value: String },
    Open { name: String, bind: String },
}

impl WParam {
    pub fn name(&self) -> &str {
        match self {
            WParam::Frozen { name, .. } | WParam::Open { name, .. } => name,
        }
    }
    pub fn frozen(name: &str, value: &str) -> WParam {
        WParam::Frozen { name: name.into(), value: value.into() }
    }
    pub fn from_core(p: &Param) -> WParam {
        match p {
            Param::Frozen { name, value } => WParam::Frozen { name: name.clone(), value: value.clone() },
            Param::Open { name, bind } => WParam::Open { name: name.clone(), bind: bind.clone() },
        }
    }
    pub fn into_core(self) -> Param {
        match self {
            WParam::Frozen { name, value } => Param::Frozen { name, value },
            WParam::Open { name, bind } => Param::Open { name, bind },
        }
    }
}

/// The value of the frozen param `name`, if there is one.
pub fn frozen_value<'a>(params: &'a [WParam], name: &str) -> Option<&'a str> {
    params.iter().find_map(|p| match p {
        WParam::Frozen { name: n, value } if n == name => Some(value.as_str()),
        _ => None,
    })
}

/// A port: `{ name, ty: "Number" }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WPort {
    pub name: String,
    pub ty: WType,
}

impl WPort {
    pub fn new(name: &str, ty: WType) -> WPort {
        WPort { name: name.into(), ty }
    }
    pub fn from_core(p: &Port) -> WPort {
        WPort { name: p.name.clone(), ty: WType::from_core(&p.ty) }
    }
    pub fn into_core(self) -> Result<Port, String> {
        Ok(Port { name: self.name, ty: self.ty.into_core()? })
    }
}

/// A composite's internal wire: child *indices* and port indices. `to_child`/`from_child` ==
/// 4294967295 (u32::MAX, core's `BOUNDARY`) is the composite's own boundary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WEdge {
    pub from_child: u32,
    pub from_port: u32,
    pub to_child: u32,
    pub to_port: u32,
}

impl WEdge {
    pub fn from_core(e: &Edge) -> WEdge {
        WEdge { from_child: e.from_child, from_port: e.from_port, to_child: e.to_child, to_port: e.to_port }
    }
    pub fn into_core(self) -> Edge {
        Edge { from_child: self.from_child, from_port: self.from_port, to_child: self.to_child, to_port: self.to_port }
    }
}

/// A unit's body: `{ kind: "Primitive", name }`, `{ kind: "Composite", children: [hex…], edges }`
/// or `{ kind: "Module", bytes: hex }` (a WebAssembly module's binary, as hex).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum WBody {
    Primitive { name: String },
    Composite { children: Vec<String>, edges: Vec<WEdge> },
    Module { bytes: String },
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("a module's bytes must be hex".into());
    }
    Ok((0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)).collect())
}

/// A unit. `params` / `inputs` / `outputs` may be omitted (default empty).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WUnit {
    pub body: WBody,
    #[serde(default)]
    pub params: Vec<WParam>,
    #[serde(default)]
    pub inputs: Vec<WPort>,
    #[serde(default)]
    pub outputs: Vec<WPort>,
}

impl WUnit {
    pub fn from_core(u: &Unit) -> WUnit {
        let body = match &u.body {
            UnitBody::Primitive { name } => WBody::Primitive { name: name.clone() },
            UnitBody::Composite { children, edges } => WBody::Composite {
                children: children.iter().map(to_hex).collect(),
                edges: edges.iter().map(WEdge::from_core).collect(),
            },
            UnitBody::Module { bytes } => WBody::Module { bytes: bytes_to_hex(bytes) },
        };
        WUnit {
            body,
            params: u.params.iter().map(WParam::from_core).collect(),
            inputs: u.inputs.iter().map(WPort::from_core).collect(),
            outputs: u.outputs.iter().map(WPort::from_core).collect(),
        }
    }

    pub fn into_core(self) -> Result<Unit, String> {
        let body = match self.body {
            WBody::Primitive { name } => UnitBody::Primitive { name },
            WBody::Composite { children, edges } => UnitBody::Composite {
                children: children
                    .iter()
                    .map(|c| parse_hash(c).ok_or_else(|| format!("bad child hash: {c}")))
                    .collect::<Result<_, _>>()?,
                edges: edges.into_iter().map(WEdge::into_core).collect(),
            },
            WBody::Module { bytes } => UnitBody::Module { bytes: hex_to_bytes(&bytes)? },
        };
        Ok(Unit {
            body,
            params: self.params.into_iter().map(WParam::into_core).collect(),
            inputs: self.inputs.into_iter().map(WPort::into_core).collect::<Result<_, _>>()?,
            outputs: self.outputs.into_iter().map(WPort::into_core).collect::<Result<_, _>>()?,
        })
    }
}
