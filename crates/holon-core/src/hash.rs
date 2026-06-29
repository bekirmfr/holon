//! Canonical content hashing for holon units.
//!
//! # What goes into a hash (the structural preimage)
//! - the unit's **kind discriminant** (primitive identity, or "composite");
//! - its **params** (frozen value or open hole), in a deterministic order;
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
const PARAM_FROZEN: u8 = 0x00; // Param variant tags (domain-separate the two states)
const PARAM_OPEN: u8 = 0x01;

// ── Value types (§5: monomorphic base + `any` wildcard) ─────────────────────

/// The wire types. `Any` is the single escape hatch that lets polymorphic units
/// (`Select`, `Get`, `Store`, `Map`) exist without a unification type checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Number,
    Boolean,
    Text,
    List,
    /// A settled effect *request* (representation B) — the type of a sink's output. It
    /// matches only itself (or `Any`), so an effect can flow to a boundary output or into
    /// `map`, but **never into a sink's payload slot**: a request cannot be re-consumed as
    /// data. Effects flow down and out, never back in.
    Effect,
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
            Type::Effect => 6, // appended last — tags 1-5 unchanged, so no existing hash moves
        }
    }

    /// Inverse of [`tag`](Self::tag): map a wire discriminant back to its `Type`.
    /// `None` for a byte that names no known type (a corrupt or future-version
    /// blob). Must mirror `tag` exactly — they are one append-only table.
    fn from_tag(tag: u8) -> Option<Type> {
        match tag {
            1 => Some(Type::Number),
            2 => Some(Type::Boolean),
            3 => Some(Type::Text),
            4 => Some(Type::List),
            5 => Some(Type::Any),
            6 => Some(Type::Effect),
            _ => None,
        }
    }
}

/// A parameter: configuration that is part of identity. Either a `Frozen` value or an
/// `Open` hole (filled at instantiate time, P1.5.2). A unit with any open param in its
/// closure is a *template* — not directly evaluable until instantiated. Param values are
/// a canonical textual form for now; richer typed values can follow behind a new
/// [`ENCODING_VERSION`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Param {
    Frozen { name: String, value: String },
    /// A hole. `name` is the param's role (what `compute` looks up once filled); `bind` is
    /// the open-parameter key it is filled by at instantiate time.
    Open { name: String, bind: String },
}

impl Param {
    pub fn frozen(name: impl Into<String>, value: impl Into<String>) -> Self {
        Param::Frozen { name: name.into(), value: value.into() }
    }
    pub fn open(name: impl Into<String>, bind: impl Into<String>) -> Self {
        Param::Open { name: name.into(), bind: bind.into() }
    }
    /// The param's name (present for both variants).
    pub fn name(&self) -> &str {
        match self {
            Param::Frozen { name, .. } | Param::Open { name, .. } => name,
        }
    }
    /// The frozen value, or `None` for an open hole.
    pub fn value(&self) -> Option<&str> {
        match self {
            Param::Frozen { value, .. } => Some(value),
            Param::Open { .. } => None,
        }
    }
    /// The open-parameter key, for a hole; `None` when frozen.
    pub fn bind(&self) -> Option<&str> {
        match self {
            Param::Open { bind, .. } => Some(bind),
            Param::Frozen { .. } => None,
        }
    }
    /// Whether this param is an unfilled hole.
    pub fn is_open(&self) -> bool {
        matches!(self, Param::Open { .. })
    }
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

/// Errors from decoding a unit out of its canonical preimage bytes (the inverse
/// of [`encode_preimage`]). Every variant means the bytes are not a well-formed
/// v-[`ENCODING_VERSION`] preimage — which, in the store, is exactly how a
/// corrupt or truncated blob is rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The stream ended before a field was fully read.
    UnexpectedEof,
    /// The leading version byte is not the one this build understands.
    WrongVersion { found: u8, expected: u8 },
    /// A structural tag byte was not one the grammar allows at that position.
    BadTag(u8),
    /// A type discriminant byte did not name a known [`Type`].
    BadType(u8),
    /// A length-prefixed string field was not valid UTF-8.
    BadUtf8,
    /// Bytes remained after a complete unit was decoded.
    TrailingBytes,
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

    // 2. Params, sorted by name for determinism. The variant (Frozen/Open) is tagged so
    //    a frozen and an open param of the same name hash differently.
    let mut params = unit.params.clone();
    params.sort_by(|a, b| a.name().cmp(b.name()));
    e.u32(params.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
    for p in &params {
        e.tag(TAG_PARAM);
        match p {
            Param::Frozen { name, value } => {
                e.tag(PARAM_FROZEN);
                e.str(name)?;
                e.str(value)?;
            }
            Param::Open { name, bind } => {
                e.tag(PARAM_OPEN);
                e.str(name)?;
                e.str(bind)?;
            }
        }
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

// ── Canonical decoder ───────────────────────────────────────────────────────
//
// The exact inverse of `encode_preimage`. Every read is bounds-checked; counts
// from the (untrusted) stream are never pre-allocated, so a blob that lies about
// a length hits `UnexpectedEof` quickly instead of trying to allocate.

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Decoder { buf, pos: 0 }
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self.buf.get(self.pos).ok_or(DecodeError::UnexpectedEof)?;
        self.pos += 1;
        Ok(b)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.take(4)?;
        let arr: [u8; 4] = bytes.try_into().expect("take(4) yields exactly 4 bytes");
        Ok(u32::from_le_bytes(arr))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    fn str(&mut self) -> Result<String, DecodeError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|_| DecodeError::BadUtf8)
    }

    fn hash(&mut self) -> Result<Hash, DecodeError> {
        let bytes = self.take(32)?;
        let mut h = [0u8; 32];
        h.copy_from_slice(bytes);
        Ok(h)
    }

    fn ty(&mut self) -> Result<Type, DecodeError> {
        let t = self.u8()?;
        Type::from_tag(t).ok_or(DecodeError::BadType(t))
    }
}

/// Decode a unit from its canonical preimage bytes — the exact inverse of
/// [`encode_preimage`]. For any unit `u`, `encode_preimage(&decode_preimage(b))`
/// reproduces `b` byte-for-byte (params and edges come back in canonical, not
/// authored, order — which is what re-encoding produces anyway, so the hash is
/// unchanged). This is what lets a content-addressed store hand back a real
/// `Unit` after a blob has been verified against its key.
pub fn decode_preimage(bytes: &[u8]) -> Result<Unit, DecodeError> {
    let mut d = Decoder::new(bytes);

    // 0. Version (Encoder::new writes this byte first).
    let v = d.u8()?;
    if v != ENCODING_VERSION {
        return Err(DecodeError::WrongVersion { found: v, expected: ENCODING_VERSION });
    }

    // 1. Body.
    let body = match d.u8()? {
        TAG_PRIMITIVE => UnitBody::Primitive { name: d.str()? },
        TAG_COMPOSITE => {
            let n_children = d.u32()?;
            let mut children = Vec::new();
            for _ in 0..n_children {
                match d.u8()? {
                    TAG_CHILD => children.push(d.hash()?),
                    other => return Err(DecodeError::BadTag(other)),
                }
            }
            let n_edges = d.u32()?;
            let mut edges = Vec::new();
            for _ in 0..n_edges {
                match d.u8()? {
                    TAG_EDGE => edges.push(Edge {
                        from_child: d.u32()?,
                        from_port: d.u32()?,
                        to_child: d.u32()?,
                        to_port: d.u32()?,
                    }),
                    other => return Err(DecodeError::BadTag(other)),
                }
            }
            UnitBody::Composite { children, edges }
        }
        other => return Err(DecodeError::BadTag(other)),
    };

    // 2. Params.
    let n_params = d.u32()?;
    let mut params = Vec::new();
    for _ in 0..n_params {
        match d.u8()? {
            TAG_PARAM => {}
            other => return Err(DecodeError::BadTag(other)),
        }
        let p = match d.u8()? {
            PARAM_FROZEN => Param::Frozen { name: d.str()?, value: d.str()? },
            PARAM_OPEN => Param::Open { name: d.str()?, bind: d.str()? },
            other => return Err(DecodeError::BadTag(other)),
        };
        params.push(p);
    }

    // 3. Interface.
    let n_inputs = d.u32()?;
    let mut inputs = Vec::new();
    for _ in 0..n_inputs {
        match d.u8()? {
            TAG_PORT => {}
            other => return Err(DecodeError::BadTag(other)),
        }
        let name = d.str()?;
        let ty = d.ty()?;
        inputs.push(Port { name, ty });
    }
    let output = match d.u8()? {
        0 => None,
        1 => Some(d.ty()?),
        other => return Err(DecodeError::BadTag(other)),
    };

    if d.pos != d.buf.len() {
        return Err(DecodeError::TrailingBytes);
    }

    Ok(Unit { body, params, inputs, output })
}

/// Decode a 64-char hex string into a [`Hash`]. `None` if it is not exactly 64
/// hex digits — the canonical home for the hex⇄`Hash` mapping, shared by the
/// evaluator (reading a `map` `fn` reference) and the kind derivation (following
/// hash-valued params).
pub(crate) fn parse_hash(hex: &str) -> Option<Hash> {
    if hex.len() != 64 {
        return None;
    }
    let bytes = hex.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = (bytes[2 * i] as char).to_digit(16)?;
        let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// Encode a [`Hash`] as a 64-char lowercase hex string (the inverse of
/// [`parse_hash`]). Used when constructing a hash-valued param (e.g. `map`'s `fn`).
#[cfg(test)]
pub(crate) fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
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
        u1.params = vec![Param::frozen("x", "1"), Param::frozen("y", "2")];
        let mut u2 = prim("add");
        u2.params = vec![Param::frozen("y", "2"), Param::frozen("x", "1")];
        assert_eq!(hash_unit(&u1).unwrap(), hash_unit(&u2).unwrap());
    }

    #[test]
    fn param_value_changes_hash() {
        let mut u1 = prim("add");
        u1.params = vec![Param::frozen("x", "1")];
        let mut u2 = prim("add");
        u2.params = vec![Param::frozen("x", "2")];
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

    // ── Decoder: the inverse property ───────────────────────────────────────

    #[test]
    fn decode_round_trips_a_primitive() {
        let u = prim("add");
        assert_eq!(decode_preimage(&encode_preimage(&u).unwrap()).unwrap(), u);
    }

    #[test]
    fn decode_round_trips_a_composite_with_params() {
        let c0 = hash_unit(&prim("add")).unwrap();
        let c1 = hash_unit(&prim("multiply")).unwrap();
        let u = Unit {
            body: UnitBody::Composite {
                children: vec![c0, c1],
                // authored already in canonical (sorted) order, so structural
                // equality — not just hash equality — holds after the round-trip.
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 1 },
                ],
            },
            params: vec![Param::frozen("x", "1"), Param::open("y", "k")],
            inputs: vec![Port { name: "a".into(), ty: Type::List }],
            output: Some(Type::Effect),
        };
        assert_eq!(decode_preimage(&encode_preimage(&u).unwrap()).unwrap(), u);
    }

    #[test]
    fn encode_decode_is_byte_exact() {
        // encode∘decode is the identity on bytes even when params are authored
        // out of canonical order (decode yields canonical order, which is what a
        // re-encode produces anyway).
        let mut u = prim("add");
        u.params = vec![Param::frozen("y", "2"), Param::frozen("x", "1")];
        let bytes = encode_preimage(&u).unwrap();
        let round = encode_preimage(&decode_preimage(&bytes).unwrap()).unwrap();
        assert_eq!(round, bytes);
    }

    #[test]
    fn decode_preserves_the_hash() {
        let u = prim("subtract");
        let via_decode = hash_unit(&decode_preimage(&encode_preimage(&u).unwrap()).unwrap()).unwrap();
        assert_eq!(hash_unit(&u).unwrap(), via_decode);
    }

    #[test]
    fn decode_rejects_malformed_bytes() {
        assert_eq!(decode_preimage(&[]), Err(DecodeError::UnexpectedEof));
        assert_eq!(
            decode_preimage(&[99]),
            Err(DecodeError::WrongVersion { found: 99, expected: ENCODING_VERSION })
        );
        let good = encode_preimage(&prim("add")).unwrap();
        assert!(decode_preimage(&good[..good.len() - 1]).is_err()); // truncated
        let mut extra = good.clone();
        extra.push(0);
        assert_eq!(decode_preimage(&extra), Err(DecodeError::TrailingBytes)); // trailing
    }
}