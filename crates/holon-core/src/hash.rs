//! Canonical content hashing for holon units.
//!
//! # What goes into a hash (the structural preimage)
//! - the unit's **kind discriminant** (primitive identity, or "composite");
//! - its **frozen params** (name + value), in a deterministic order;
//! - its **derived interface**: input ports (name + type) in declaration order,
//!   and the output type — names are part of the frozen public contract (I4),
//!   so they are *in*;
//! - for a composite: its **internal wiring** (edges) in a deterministic order,
//!   and each **child referenced by its 32-byte hash** (never inlined).
//!
//! # What is excluded (cosmetics, stripped by normalization)
//! node positions, human labels/descriptions, and the author's incidental
//! wiring order. The test for in-vs-out: *if it changes the computed result it
//! is in; if it is only how the unit looks on screen it is out.*
//!
//! # Encoding rules
//! Length-prefixed, field-tagged **binary** (never JSON, whose number / unicode
//! / key-order ambiguities make byte-identical output across targets fragile).
//! Everything is domain-separated and prefixed with [`ENCODING_VERSION`] so the
//! scheme can evolve without old and new hashes ever colliding.

use blake3::Hasher;

/// Bumped whenever the canonical encoding changes in any way. Mixed into every
/// preimage so a unit hashed under v1 can never collide with the same unit
/// hashed under a future v2 encoding.
pub const ENCODING_VERSION: u8 = 1;

/// A 32-byte content hash. This is a unit's identity; the version label in the
/// registry is merely a friendly alias that resolves *to* one of these.
pub type Hash = [u8; 32];

// ── Domain-separation tags ──────────────────────────────────────────────────
// Each distinct structural element gets a unique 1-byte tag so that two
// different shapes can never produce the same byte stream by coincidence.
const TAG_PRIMITIVE: u8 = 0x01;
const TAG_COMPOSITE: u8 = 0x02;
const TAG_PARAM: u8 = 0x10;
const TAG_PORT: u8 = 0x11;
const TAG_EDGE: u8 = 0x12;
const TAG_CHILD: u8 = 0x13;

// ── Value types (§5: monomorphic base + `any` wildcard) ─────────────────────

/// The wire types. `Any` is the single escape hatch that lets polymorphic units
/// (`Select`, `Get`, `Store`, `Map`) exist without a unification type checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Number,
    Boolean,
    Text,
    List,
    Any,
}

impl Type {
    /// Stable on-wire discriminant. These bytes are part of the hash preimage,
    /// so they must never be reordered or reused once shipped.
    fn tag(self) -> u8 {
        match self {
            Type::Number => 1,
            Type::Boolean => 2,
            Type::Text => 3,
            Type::List => 4,
            Type::Any => 5,
        }
    }
}

/// A frozen parameter: configuration captured at encapsulation time. Param
/// *values* are encoded by a canonical textual form for now; richer typed param
/// values can be added later behind a new [`ENCODING_VERSION`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub value: String,
}

/// An interface port: a name plus a type. Names are part of the frozen public
/// contract (I4), so they participate in the hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub ty: Type,
}

/// An internal wire of a composite, addressed by child *index* within this
/// unit's child list. Indices are resolved against the canonical (sorted) child
/// order, so the same structure always yields the same edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from_child: u32,
    pub from_port: u32,
    pub to_child: u32,
    pub to_port: u32,
}

/// What a unit *is*: either a named primitive, or a composite built from child
/// units (referenced by hash) wired together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitBody {
    /// A primitive's identity is its stable name (e.g. "add", "select").
    Primitive { name: String },
    /// A composite references its children **by hash only** and wires them.
    Composite { children: Vec<Hash>, edges: Vec<Edge> },
}

/// A complete unit: its body plus its frozen, derived interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub body: UnitBody,
    pub params: Vec<Param>,
    pub inputs: Vec<Port>,
    pub output: Option<Type>,
}

/// Errors that can occur while encoding a unit's canonical preimage.
#[derive(Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// A field exceeded the 32-bit length budget of the canonical encoding.
    FieldTooLong,
}

// ── Canonical encoder ───────────────────────────────────────────────────────
//
// Every variable-length field is written length-prefixed (u32-LE byte count
// then the bytes), so the byte stream is unambiguous and self-delimiting. There
// is exactly one way to encode any given unit.

struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        // Domain-separate the whole stream by the encoding version up front.
        Encoder { buf: vec![ENCODING_VERSION] }
    }

    fn tag(&mut self, tag: u8) {
        self.buf.push(tag);
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Length-prefixed bytes. Refuses anything that doesn't fit the u32 budget,
    /// rather than silently truncating.
    fn bytes(&mut self, b: &[u8]) -> Result<(), EncodeError> {
        let len: u32 = b.len().try_into().map_err(|_| EncodeError::FieldTooLong)?;
        self.u32(len);
        self.buf.extend_from_slice(b);
        Ok(())
    }

    fn str(&mut self, s: &str) -> Result<(), EncodeError> {
        self.bytes(s.as_bytes())
    }

    fn ty(&mut self, t: Type) {
        self.buf.push(t.tag());
    }
}

/// Produce the canonical preimage bytes for a unit. The output is a pure
/// function of the unit's *structure* — identical structures (regardless of
/// cosmetics) yield identical bytes; any computational change yields different
/// bytes.
pub fn encode_preimage(unit: &Unit) -> Result<Vec<u8>, EncodeError> {
    let mut e = Encoder::new();

    // 1. Body discriminant + body contents.
    match &unit.body {
        UnitBody::Primitive { name } => {
            e.tag(TAG_PRIMITIVE);
            e.str(name)?;
        }
        UnitBody::Composite { children, edges } => {
            e.tag(TAG_COMPOSITE);

            // Children, each as a 32-byte hash. Child order is preserved as
            // given (it is the canonical index space the edges refer to); the
            // *caller* is responsible for having frozen a deterministic child
            // order at encapsulation time. We still tag + count for safety.
            e.u32(children.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
            for child in children {
                e.tag(TAG_CHILD);
                e.buf.extend_from_slice(child);
            }

            // Edges, sorted into a deterministic order so authored wiring order
            // never leaks into the hash.
            let mut sorted = edges.clone();
            sorted.sort_unstable_by_key(|x| {
                (x.from_child, x.from_port, x.to_child, x.to_port)
            });
            e.u32(sorted.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
            for edge in &sorted {
                e.tag(TAG_EDGE);
                e.u32(edge.from_child);
                e.u32(edge.from_port);
                e.u32(edge.to_child);
                e.u32(edge.to_port);
            }
        }
    }

    // 2. Frozen params, sorted by name for determinism.
    let mut params = unit.params.clone();
    params.sort_by(|a, b| a.name.cmp(&b.name));
    e.u32(params.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
    for p in &params {
        e.tag(TAG_PARAM);
        e.str(&p.name)?;
        e.str(&p.value)?;
    }

    // 3. Interface — inputs in declaration order (the frozen contract), output.
    e.u32(unit.inputs.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
    for port in &unit.inputs {
        e.tag(TAG_PORT);
        e.str(&port.name)?;
        e.ty(port.ty);
    }
    match unit.output {
        Some(t) => {
            e.buf.push(1);
            e.ty(t);
        }
        None => e.buf.push(0),
    }

    Ok(e.buf)
}

/// Hash a single unit from its canonical preimage. Children must already be
/// referenced by their hashes inside `unit` (this is the bottom-up step).
pub fn hash_unit(unit: &Unit) -> Result<Hash, EncodeError> {
    let preimage = encode_preimage(unit)?;
    let mut h = Hasher::new();
    h.update(&preimage);
    Ok(*h.finalize().as_bytes())
}

// ── Tests: the governing property ───────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.to_string() },
            params: vec![],
            inputs: vec![
                Port { name: "a".into(), ty: Type::Number },
                Port { name: "b".into(), ty: Type::Number },
            ],
            output: Some(Type::Number),
        }
    }

    #[test]
    fn hashing_is_deterministic() {
        assert_eq!(hash_unit(&prim("add")).unwrap(), hash_unit(&prim("add")).unwrap());
    }

    #[test]
    fn primitive_identity_changes_hash() {
        assert_ne!(hash_unit(&prim("add")).unwrap(), hash_unit(&prim("multiply")).unwrap());
    }

    #[test]
    fn param_order_is_cosmetic() {
        // Same params, authored in different order ⇒ identical hash.
        let mut u1 = prim("add");
        u1.params = vec![
            Param { name: "x".into(), value: "1".into() },
            Param { name: "y".into(), value: "2".into() },
        ];
        let mut u2 = prim("add");
        u2.params = vec![
            Param { name: "y".into(), value: "2".into() },
            Param { name: "x".into(), value: "1".into() },
        ];
        assert_eq!(hash_unit(&u1).unwrap(), hash_unit(&u2).unwrap());
    }

    #[test]
    fn param_value_changes_hash() {
        let mut u1 = prim("add");
        u1.params = vec![Param { name: "x".into(), value: "1".into() }];
        let mut u2 = prim("add");
        u2.params = vec![Param { name: "x".into(), value: "2".into() }];
        assert_ne!(hash_unit(&u1).unwrap(), hash_unit(&u2).unwrap());
    }

    #[test]
    fn port_name_is_part_of_contract() {
        // Renaming a public port is an interface change, not a cosmetic one (I4).
        let mut u = prim("add");
        u.inputs[0].name = "left".into();
        assert_ne!(hash_unit(&u).unwrap(), hash_unit(&prim("add")).unwrap());
    }

    #[test]
    fn port_type_changes_hash() {
        let mut u = prim("add");
        u.inputs[0].ty = Type::Text;
        assert_ne!(hash_unit(&u).unwrap(), hash_unit(&prim("add")).unwrap());
    }

    #[test]
    fn edge_authoring_order_is_cosmetic() {
        let c0 = hash_unit(&prim("add")).unwrap();
        let c1 = hash_unit(&prim("multiply")).unwrap();
        let mk = |edges: Vec<Edge>| Unit {
            body: UnitBody::Composite { children: vec![c0, c1], edges },
            params: vec![],
            inputs: vec![Port { name: "a".into(), ty: Type::Number }],
            output: Some(Type::Number),
        };
        let e1 = Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 };
        let e2 = Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 1 };
        // Same edge set, opposite authoring order ⇒ identical hash.
        assert_eq!(
            hash_unit(&mk(vec![e1, e2])).unwrap(),
            hash_unit(&mk(vec![e2, e1])).unwrap()
        );
    }

    #[test]
    fn child_hash_changes_parent_hash() {
        // Transitive pinning (I6): swap a child's hash ⇒ parent hash changes.
        let edges = vec![Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }];
        let parent = |children: Vec<Hash>| Unit {
            body: UnitBody::Composite { children, edges: edges.clone() },
            params: vec![],
            inputs: vec![Port { name: "a".into(), ty: Type::Number }],
            output: Some(Type::Number),
        };
        let add = hash_unit(&prim("add")).unwrap();
        let mul = hash_unit(&prim("multiply")).unwrap();
        let other = hash_unit(&prim("subtract")).unwrap();
        assert_ne!(
            hash_unit(&parent(vec![add, mul])).unwrap(),
            hash_unit(&parent(vec![add, other])).unwrap()
        );
    }

    #[test]
    fn primitive_and_composite_do_not_collide() {
        // Domain-separation tags keep a primitive and a composite distinct even
        // if their other fields coincide.
        let p = prim("add");
        let c = Unit {
            body: UnitBody::Composite { children: vec![], edges: vec![] },
            params: p.params.clone(),
            inputs: p.inputs.clone(),
            output: p.output,
        };
        assert_ne!(hash_unit(&p).unwrap(), hash_unit(&c).unwrap());
    }
}
