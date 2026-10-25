//! Canonical content hashing for holon units.
//!
//! # What goes into a hash (the structural preimage)
//! - the unit's **kind discriminant** (primitive identity, or "composite");
//! - its **params** (frozen value or open hole), in a deterministic order;
//! - its **derived interface**: input ports (name + type) in declaration order,
//!   and output ports (name + type) in declaration order — names are part of the
//!   frozen public contract (I4), so they are *in*, on both sides now (P4.7:
//!   outputs became `Vec<Port>`, symmetric with inputs — composite-only, no
//!   primitive ever declares more than one);
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

use crate::BOUNDARY;

/// Bumped whenever the canonical encoding changes in any way. Mixed into every
/// preimage so a unit hashed under one version can never collide with the same
/// unit hashed under another. v2: `List<T>` carries an element type. v3: `Product`
/// joins the recursive type encoding. v4: `Sum` (tagged choice) joins it too (P1.3.2).
pub const ENCODING_VERSION: u8 = 5;

/// A 32-byte content hash. This is a unit's identity; the version label in the
/// registry is merely a friendly alias that resolves *to* one of these.
pub type Hash = [u8; 32];

// ── Domain-separation tags ──────────────────────────────────────────────────
// Each distinct structural element gets a unique 1-byte tag so that two
// different shapes can never produce the same byte stream by coincidence.
const TAG_PRIMITIVE: u8 = 0x01;
const TAG_COMPOSITE: u8 = 0x02;
/// A module body (2026-10-25). A new body tag is a compatible extension: nothing already
/// encoded changes, and an older decoder meeting it fails closed with `BadTag`. A change to how
/// an existing tag encodes is what bumps [`ENCODING_VERSION`].
const TAG_MODULE: u8 = 0x03;
const TAG_PARAM: u8 = 0x10;
const TAG_PORT: u8 = 0x11;
const TAG_EDGE: u8 = 0x12;
const TAG_CHILD: u8 = 0x13;
const PARAM_FROZEN: u8 = 0x00; // Param variant tags (domain-separate the two states)
const PARAM_OPEN: u8 = 0x01;

// ── Value types (§5: structural base + `any` wildcard, recursive over `List<T>`) ─

/// The wire types. `Any` is the escape hatch that lets polymorphic units (`select`,
/// `get`, `store`, `map`) exist without a unification type checker. `List` carries
/// its element type, so types compose: `List<Number>`, `List<List<Text>>`, … The
/// element type makes `Type` recursive, so it is `Clone` (not `Copy`) — a `List`
/// owns a boxed element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Number,
    Boolean,
    Text,
    /// A homogeneous list of `T`. The bare/legacy list is `List<Any>` ([`Type::list_any`]).
    List(Box<Type>),
    /// A positional, n-ary product (tuple). `Pair<A,B>` = `Product([A, B])`; `Unit` = `Product([])`.
    /// Compared by **arity + position** — no field names, so no width/depth subtyping. A product
    /// *value* rides the list value (eval is untyped); only the declared type distinguishes it from
    /// a `List`. `Dictionary<K,V>` is sugar for `List<Product([K, V])>`.
    Product(Vec<Type>),
    /// A tagged choice among n-ary alternatives. `Option<T>` = `Sum([T, Unit])`. Compared the same
    /// way as `Product` — arity + position — but a `Sum` *value* ([`crate::Value::Sum`]) carries only
    /// its taken arm; the untaken alternatives are erased at runtime (unlike `Product`, whose value
    /// carries every field). That asymmetry is what makes `Sum` the one generator needing a genuinely
    /// new value representation, not a reuse of `List`.
    Sum(Vec<Type>),
    /// A settled effect *request* (representation B) — the type of a sink's output. It
    /// matches only itself (or `Any`), so an effect can flow to a boundary output or into
    /// `map`, but **never into a sink's payload slot**: a request cannot be re-consumed as
    /// data. Effects flow down and out, never back in.
    Effect,
    Any,
    /// A record (named fields, see [`crate::Value::Record`]). Open for now: any fields, any
    /// field types. Field-typed records can be added later under a new tag.
    Record,
}

impl Type {
    /// The bare/legacy list: a list whose element type is unconstrained. Compatible
    /// with any `List<T>` (the `Any` wildcard, one level down — see `compatible`).
    pub fn list_any() -> Type {
        Type::List(Box::new(Type::Any))
    }

    /// Stable on-wire discriminant (the *head* byte of a type). These bytes are part
    /// of the hash preimage, so they must never be reordered or reused once shipped.
    /// `List`'s element type follows this byte, written by the recursive encoder.
    fn tag(&self) -> u8 {
        match self {
            Type::Number => 1,
            Type::Boolean => 2,
            Type::Text => 3,
            Type::List(_) => 4,
            Type::Any => 5,
            Type::Effect => 6, // appended last — tags 1-5 unchanged, so no existing hash moves
            Type::Product(_) => 7, // n-ary, recursive — element types follow the tag, like List
            Type::Sum(_) => 8, // n-ary, recursive — same shape as Product, distinct tag
            Type::Record => 9, // appended — no existing tag or hash moves
        }
    }

    /// Inverse of [`tag`](Self::tag) for the **nullary** types only. `None` for a byte
    /// that names no known nullary type — including `4` (`List`), `7` (`Product`), and `8`
    /// (`Sum`), which carry element types and are decoded by [`Decoder::ty`], not here. Must
    /// mirror `tag` for the nullary cases.
    fn from_tag(tag: u8) -> Option<Type> {
        match tag {
            1 => Some(Type::Number),
            2 => Some(Type::Boolean),
            3 => Some(Type::Text),
            5 => Some(Type::Any),
            6 => Some(Type::Effect),
            9 => Some(Type::Record),
            _ => None, // 4 (List) / 7 (Product) / 8 (Sum) carry element types — the decoder reads them
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
    /// A WebAssembly module with **no imports**, as is (docs/L4s/wasm-module-L4.md): the
    /// vocabulary's escape hatch. Its bytes are its identity; it is pure because it can reach
    /// nothing (`module::validate_module` reads that from the import section), and total under
    /// the `fuel` and `memory` budgets in its params. An engine outside core runs it
    /// (`Resolver::run_module`).
    Module { bytes: Vec<u8> },
}

/// A complete unit: its body plus its frozen, derived interface. `outputs` is
/// composite-only in practice (P4.7) — every genesis primitive declares exactly
/// one, named `"out"` by convention (matching the editor's existing single-output
/// handle id) — but the type itself doesn't special-case that; a composite can
/// declare zero, one, or several, exactly as `inputs` already could.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub body: UnitBody,
    pub params: Vec<Param>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

/// Errors that can occur while encoding a unit's canonical preimage.
#[derive(Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// A field exceeded the 32-bit length budget of the canonical encoding.
    FieldTooLong,
    /// Two children of a composite are the same unit and neither one's value reaches the
    /// composite's outputs, so nothing tells them apart (a value computed twice and never used).
    /// Rejected — fail-closed — rather than hashed in an order that depends on how the composite
    /// was written. See [`canonical_child_order`].
    UnusedTwins,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::FieldTooLong => write!(f, "a name or value is too long to encode (over 4 GiB)"),
            EncodeError::UnusedTwins => {
                write!(f, "the same value is computed twice and never used, so nothing tells the two apart: remove one, or use it")
            }
        }
    }
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
    /// A type nested deeper than [`MAX_TYPE_DEPTH`] — a hostile or corrupt blob trying to
    /// exhaust the stack during recursive type decoding. Rejected, never recursed into.
    TypeTooDeep,
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

    /// Write a type: its head tag, then — for the recursive constructors — its element type(s).
    /// `List<T>` writes one inner type; `Product`/`Sum` (n-ary) write a `u32` count then each
    /// element, identical shape, distinguished only by their tag. In-memory types come from
    /// `decode` (depth-bounded) or trusted core construction (shallow), so the encoder is not
    /// depth-limited; the untrusted entry point is `decode`. Fallible only to refuse a Product/Sum
    /// wider than the `u32` budget (it never silently truncates).
    fn ty(&mut self, t: &Type) -> Result<(), EncodeError> {
        self.buf.push(t.tag());
        match t {
            Type::List(inner) => self.ty(inner)?,
            Type::Product(elems) | Type::Sum(elems) => {
                self.u32(elems.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
                for el in elems {
                    self.ty(el)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// ── Child-order canonicalization ────────────────────────────────────────────
// A composite's *child order* is presentation, not structure: `eval` topo-orders by edges and uses
// a child's index only to resolve which unit it is, so reordering children (with edges remapped)
// changes no value. The encoder therefore normalizes child order — exactly as it already normalizes
// edge order — so that presentation order is not part of identity. See child-order-canonical-L4.

/// Canonicalize a composite's child order: see [`canonical_child_order`]. Edges are remapped to the
/// new indices (`BOUNDARY` fixed) and sorted. Identity changes; behaviour never does.
fn canonical_composite(
    children: &[Hash],
    edges: &[Edge],
) -> Result<(Vec<Hash>, Vec<Edge>), EncodeError> {
    let order = canonical_child_order(children, edges)?;
    Ok(apply_child_order(children, edges, &order))
}

/// The reordering [`encode_preimage`] applies to a composite's children: `order[k]` is the index,
/// as given, of the child that is child `k` in the canonical (hashed, stored) form. A unit that
/// went through a store comes back in canonical order, so this is how a child index seen there
/// (in a timing, say) is traced back to the child as it was authored.
///
/// Children are ordered by hash, and children with the same hash by their *path label*
/// ([`path_labels`]): where each one's value goes, down to the composite's outputs. Every input
/// port takes one wire, so two different children whose values reach an output have different
/// paths, and the order is the wiring's alone, never the author's. Two children left tied are
/// the same unit and their values go nowhere: [`EncodeError::UnusedTwins`].
pub fn canonical_child_order(children: &[Hash], edges: &[Edge]) -> Result<Vec<usize>, EncodeError> {
    let k = children.len();
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|&i, &j| children[i].cmp(&children[j]));

    // Fast path: all hashes distinct ⇒ the hash order is the unique canonical order.
    if order.windows(2).all(|w| children[w[0]] != children[w[1]]) {
        return Ok(order);
    }

    let labels = path_labels(children, edges);
    order.sort_by(|&i, &j| (children[i], labels[i]).cmp(&(children[j], labels[j])));
    if order.windows(2).any(|w| (children[w[0]], labels[w[0]]) == (children[w[1]], labels[w[1]])) {
        return Err(EncodeError::UnusedTwins);
    }
    Ok(order)
}

/// Each child's path label: a hash of the child's own hash and of every edge leaving it, each
/// edge named by its ports and by where it goes, the composite's output port or the label of the
/// child it feeds. Labels are computed from the outputs back, so a label covers every path from
/// the child to the outputs. A child whose value goes nowhere, or that sits on a cycle (no unit
/// that evaluates has one; a malformed blob might), is labelled by its hash alone.
fn path_labels(children: &[Hash], edges: &[Edge]) -> Vec<Hash> {
    let k = children.len();
    let inside = |c: u32| (c as usize) < k;
    // The edges leaving each child, and how many children each one feeds yet to be labelled.
    let mut leaving: Vec<Vec<&Edge>> = vec![Vec::new(); k];
    for e in edges.iter().filter(|e| inside(e.from_child)) {
        leaving[e.from_child as usize].push(e);
    }
    let mut waiting: Vec<usize> = leaving.iter().map(|es| es.iter().filter(|e| inside(e.to_child)).count()).collect();
    let mut labels: Vec<Option<Hash>> = vec![None; k];
    let mut ready: Vec<usize> = (0..k).filter(|&i| waiting[i] == 0).collect();
    while let Some(i) = ready.pop() {
        let mut items: Vec<Vec<u8>> = leaving[i]
            .iter()
            .map(|e| {
                let mut item = Vec::with_capacity(41);
                if inside(e.to_child) {
                    item.push(1);
                    item.extend_from_slice(&labels[e.to_child as usize].expect("a fed child is labelled first"));
                } else {
                    item.push(0);
                }
                item.extend_from_slice(&e.from_port.to_le_bytes());
                item.extend_from_slice(&e.to_port.to_le_bytes());
                item
            })
            .collect();
        items.sort_unstable();
        let mut h = Hasher::new();
        h.update(b"holon path label");
        h.update(&children[i]);
        for item in &items {
            h.update(item);
        }
        labels[i] = Some(*h.finalize().as_bytes());
        // Each child feeding this one is one step nearer to being labelled.
        for e in edges.iter().filter(|e| inside(e.from_child) && e.to_child as usize == i) {
            let from = e.from_child as usize;
            waiting[from] -= 1;
            if waiting[from] == 0 {
                ready.push(from);
            }
        }
    }
    labels.into_iter().enumerate().map(|(i, l)| l.unwrap_or(children[i])).collect()
}

/// Apply a child ordering (`order[new_index] = old_index`): reorder children, remap edge endpoints to
/// the new indices (`BOUNDARY`, and any out-of-range index in a malformed unit, left untouched — no
/// panic), then sort edges.
fn apply_child_order(children: &[Hash], edges: &[Edge], order: &[usize]) -> (Vec<Hash>, Vec<Edge>) {
    let k = children.len();
    let mut inv = vec![0u32; k]; // old_index -> new_index
    for (new_index, &old) in order.iter().enumerate() {
        inv[old] = new_index as u32;
    }
    let kids: Vec<Hash> = order.iter().map(|&old| children[old]).collect();
    let remap = |c: u32| if c == BOUNDARY || (c as usize) >= k { c } else { inv[c as usize] };
    let mut es: Vec<Edge> = edges
        .iter()
        .map(|e| Edge {
            from_child: remap(e.from_child),
            from_port: e.from_port,
            to_child: remap(e.to_child),
            to_port: e.to_port,
        })
        .collect();
    es.sort_unstable_by_key(|x| (x.from_child, x.from_port, x.to_child, x.to_port));
    (kids, es)
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

            // Canonicalize child order (presentation is not identity, §child-order-canonical-L4):
            // children ordered by hash, ties by where their values go; edges remapped to the new
            // indices and sorted.
            // After this, the authored child *and* edge order never leak into the hash.
            let (children, edges) = canonical_composite(children, edges)?;

            e.u32(children.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
            for child in &children {
                e.tag(TAG_CHILD);
                e.buf.extend_from_slice(child);
            }

            e.u32(edges.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
            for edge in &edges {
                e.tag(TAG_EDGE);
                e.u32(edge.from_child);
                e.u32(edge.from_port);
                e.u32(edge.to_child);
                e.u32(edge.to_port);
            }
        }
        UnitBody::Module { bytes } => {
            // The bytes are the identity: length-prefixed, as is. No canonicalization applies
            // (a module's binary is already one byte string), and nothing else of it is read
            // here; what it must be to be a unit at all is `validate_module`'s to say.
            e.tag(TAG_MODULE);
            e.bytes(bytes)?;
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

    // 3. Interface — inputs in declaration order, then outputs in declaration order (P4.7:
    //    the same shape as inputs now, not a lone presence byte). Both are the frozen contract.
    e.u32(unit.inputs.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
    for port in &unit.inputs {
        e.tag(TAG_PORT);
        e.str(&port.name)?;
        e.ty(&port.ty)?;
    }
    e.u32(unit.outputs.len().try_into().map_err(|_| EncodeError::FieldTooLong)?);
    for port in &unit.outputs {
        e.tag(TAG_PORT);
        e.str(&port.name)?;
        e.ty(&port.ty)?;
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

/// Maximum type nesting depth the decoder will accept (`List<List<…>>`). Far above any
/// legitimate type (real types are depth 1-3) and far below stack exhaustion. A blob
/// nesting deeper is rejected with [`DecodeError::TypeTooDeep`] — depth is the one new
/// attack surface a recursive type decoder opens over untrusted bytes.
///
/// `pub`: the wasm bridge's `WType::into_core` (P1.3.2 editor inch) faces the same
/// attack surface for the same reason — `compatible`/`build_preimage` are public
/// `#[wasm_bindgen]` entry points, reachable by any JS on the page, and `WType` is now
/// recursive too. It reuses this exact bound rather than defining a parallel one that
/// could silently drift out of sync.
pub const MAX_TYPE_DEPTH: u32 = 32;

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
        self.ty_at(0)
    }

    /// Read a type, recursing into `List<T>`'s element type. `depth` guards against a
    /// hostile blob nesting types deep enough to exhaust the stack: past
    /// [`MAX_TYPE_DEPTH`] it returns [`DecodeError::TypeTooDeep`] instead of recursing.
    fn ty_at(&mut self, depth: u32) -> Result<Type, DecodeError> {
        if depth > MAX_TYPE_DEPTH {
            return Err(DecodeError::TypeTooDeep);
        }
        let t = self.u8()?;
        match t {
            4 => {
                // List: tag 4 (must match Type::tag) is followed by the element type.
                Ok(Type::List(Box::new(self.ty_at(depth + 1)?)))
            }
            7 => {
                // Product: tag 7 is followed by a u32 element count, then that many types. The
                // count is untrusted, so elements are pushed as read (never pre-allocated) — a
                // lying count hits UnexpectedEof, and each element is depth-bounded.
                let n = self.u32()?;
                let mut elems = Vec::new();
                for _ in 0..n {
                    elems.push(self.ty_at(depth + 1)?);
                }
                Ok(Type::Product(elems))
            }
            8 => {
                // Sum: identical wire shape to Product (tag 7) — a u32 count then that many
                // types — distinguished only by the tag byte. Same untrusted-count discipline.
                let n = self.u32()?;
                let mut elems = Vec::new();
                for _ in 0..n {
                    elems.push(self.ty_at(depth + 1)?);
                }
                Ok(Type::Sum(elems))
            }
            _ => Type::from_tag(t).ok_or(DecodeError::BadType(t)),
        }
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
        TAG_MODULE => {
            let len = d.u32()? as usize;
            UnitBody::Module { bytes: d.take(len)?.to_vec() }
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
    let n_outputs = d.u32()?;
    let mut outputs = Vec::new();
    for _ in 0..n_outputs {
        match d.u8()? {
            TAG_PORT => {}
            other => return Err(DecodeError::BadTag(other)),
        }
        let name = d.str()?;
        let ty = d.ty()?;
        outputs.push(Port { name, ty });
    }

    if d.pos != d.buf.len() {
        return Err(DecodeError::TrailingBytes);
    }

    Ok(Unit { body, params, inputs, outputs })
}

/// Decode a 64-char hex string into a [`Hash`]. `None` if it is not exactly 64
/// hex digits — the canonical home for the hex⇄`Hash` mapping, shared by the
/// evaluator (reading a `map` `fn` reference) and the kind derivation (following
/// hash-valued params).
pub fn parse_hash(hex: &str) -> Option<Hash> {
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
/// [`parse_hash`]). Used when constructing a hash-valued param (e.g. `map`'s `fn`),
/// and when re-pinning one during a fork.
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
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }

    /// Identical children whose values go nowhere can't be told apart, however many: refused at
    /// once, never searched (the old minimization once tried 66! orderings here).
    #[test]
    fn identical_children_that_feed_nothing_are_refused() {
        let child = hash_unit(&prim("add")).unwrap();
        let composite = |k: usize| Unit {
            body: UnitBody::Composite { children: vec![child; k], edges: vec![] },
            params: vec![],
            inputs: vec![],
            outputs: vec![],
        };
        assert!(hash_unit(&composite(1)).is_ok());
        for k in [2, 9, 66, 200] {
            assert!(matches!(hash_unit(&composite(k)), Err(EncodeError::UnusedTwins)), "{k} children");
        }
    }

    /// A fan-in of many identical children, each fed from its own input (what `"{a}/{b}/…"`
    /// lowers to: an `as_text` per part, into one `concat`), hashes at once and the same however
    /// the children are listed. Past eight twins the old minimization refused it.
    #[test]
    fn many_identical_children_on_distinct_paths_hash_in_any_presentation() {
        let twin = hash_unit(&prim("add")).unwrap();
        let sink = hash_unit(&prim("multiply")).unwrap();
        let fan_in = |k: usize, rotate: usize| {
            // The sink is child 0; twin `t` (input t, sink port t) sits at index 1 + (t + rotate) % k.
            let at = |t: usize| 1 + ((t + rotate) % k) as u32;
            let mut edges = vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }];
            for t in 0..k {
                edges.push(Edge { from_child: BOUNDARY, from_port: t as u32, to_child: at(t), to_port: 0 });
                edges.push(Edge { from_child: at(t), from_port: 0, to_child: 0, to_port: t as u32 });
            }
            let mut children = vec![sink];
            children.extend(std::iter::repeat_n(twin, k));
            Unit {
                body: UnitBody::Composite { children, edges },
                params: vec![],
                inputs: (0..k).map(|t| Port { name: format!("in{t}"), ty: Type::Number }).collect(),
                outputs: vec![Port { name: "out".into(), ty: Type::Number }],
            }
        };
        for k in [2, 9, 40] {
            let h = hash_unit(&fan_in(k, 0)).unwrap();
            assert_eq!(hash_unit(&fan_in(k, 1)).unwrap(), h, "{k} twins, listed another way");
            assert_eq!(hash_unit(&fan_in(k, k - 1)).unwrap(), h, "{k} twins, listed another way");
        }
    }

    /// Twins whose values go nowhere are refused even when they read different inputs: nothing
    /// downstream says which is which. One of them used is enough to tell them apart.
    #[test]
    fn twins_are_told_apart_by_where_their_values_go() {
        let add = hash_unit(&prim("add")).unwrap();
        let unit = |edges: Vec<Edge>| Unit {
            body: UnitBody::Composite { children: vec![add, add], edges },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Number }, Port { name: "y".into(), ty: Type::Number }],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let reads = vec![
            Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
            Edge { from_child: BOUNDARY, from_port: 1, to_child: 1, to_port: 0 },
        ];
        assert!(matches!(hash_unit(&unit(reads.clone())), Err(EncodeError::UnusedTwins)));
        let mut used = reads;
        used.push(Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        assert!(hash_unit(&unit(used)).is_ok());
    }

    /// A malformed blob with a cycle neither hangs nor panics: its children are labelled by their
    /// hashes alone.
    #[test]
    fn a_cycle_is_labelled_without_hanging() {
        let add = hash_unit(&prim("add")).unwrap();
        let mul = hash_unit(&prim("multiply")).unwrap();
        let unit = Unit {
            body: UnitBody::Composite {
                children: vec![add, mul, add],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: 1, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 2, from_port: 0, to_child: 0, to_port: 1 },
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: 7, from_port: 0, to_child: 9, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        assert!(matches!(hash_unit(&unit), Err(EncodeError::UnusedTwins)), "both adds sit on or feed the cycle");
    }

    /// The reordering `canonical_child_order` reports is the one encoding applies: decoding the
    /// preimage gives the children in exactly that order, even with interchangeable twins.
    #[test]
    fn canonical_child_order_is_the_order_a_stored_unit_comes_back_in() {
        let a = hash_unit(&prim("add")).unwrap();
        let m = hash_unit(&prim("multiply")).unwrap();
        // add twice (twins) and multiply, wired so the twins are told apart only by their edges.
        let children = vec![m, a, a];
        let edges = vec![
            Edge { from_child: BOUNDARY, from_port: 0, to_child: 2, to_port: 0 },
            Edge { from_child: 2, from_port: 0, to_child: 1, to_port: 0 },
            Edge { from_child: 1, from_port: 0, to_child: 0, to_port: 0 },
            Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
        ];
        let unit = Unit {
            body: UnitBody::Composite { children: children.clone(), edges: edges.clone() },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Number }],
            outputs: vec![Port { name: "y".into(), ty: Type::Number }],
        };
        let order = canonical_child_order(&children, &edges).unwrap();
        let stored = decode_preimage(&encode_preimage(&unit).unwrap()).unwrap();
        let UnitBody::Composite { children: stored_children, edges: stored_edges } = stored.body else { unreachable!() };
        assert_eq!(stored_children, order.iter().map(|&i| children[i]).collect::<Vec<_>>());
        // Each stored edge, mapped back through the order, is one of the authored edges.
        let back = |c: u32| if c == BOUNDARY { c } else { order[c as usize] as u32 };
        for e in stored_edges {
            let authored = Edge { from_child: back(e.from_child), from_port: e.from_port, to_child: back(e.to_child), to_port: e.to_port };
            assert!(edges.contains(&authored), "{authored:?}");
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
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
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
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
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
            outputs: p.outputs.clone(),
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
        // Author children already in canonical (hash-ascending) order, so the structural — not just
        // hash — equality holds after the round-trip (decode∘encode is identity only on canonical
        // input now that the encoder normalizes child order).
        let mut children = vec![c0, c1];
        children.sort();
        let u = Unit {
            body: UnitBody::Composite {
                children,
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 1 },
                ],
            },
            params: vec![Param::frozen("x", "1"), Param::open("y", "k")],
            inputs: vec![Port { name: "a".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        assert_eq!(decode_preimage(&encode_preimage(&u).unwrap()).unwrap(), u);
    }

    #[test]
    fn child_order_is_cosmetic() {
        // Same children, same wiring (add.out → mul.in0), listed in opposite order with the edge
        // remapped to match ⇒ identical hash. Authoring order is not identity.
        let add = hash_unit(&prim("add")).unwrap();
        let mul = hash_unit(&prim("multiply")).unwrap();
        let presentation = |children: Vec<Hash>, edge: Edge| Unit {
            body: UnitBody::Composite { children, edges: vec![edge] },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        // A: [add@0, mul@1], add@0 → mul@1.   B: [mul@0, add@1], add@1 → mul@0.  Same wiring.
        let a = presentation(vec![add, mul], Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 });
        let b = presentation(vec![mul, add], Edge { from_child: 1, from_port: 0, to_child: 0, to_port: 0 });
        assert_eq!(hash_unit(&a).unwrap(), hash_unit(&b).unwrap());
    }

    #[test]
    fn duplicate_children_canonicalize() {
        // Two identical-hash children (both `add`) in DIFFERENT roles: one feeds the boundary output,
        // the other is fed by the boundary input. The two presentations (which index plays which
        // role) must collapse to ONE hash — the duplicate is told apart by where its value goes.
        let add = hash_unit(&prim("add")).unwrap();
        let roles = |out_child: u32, in_child: u32| Unit {
            body: UnitBody::Composite {
                children: vec![add, add],
                edges: vec![
                    Edge { from_child: out_child, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: in_child, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Number }],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let a = roles(0, 1); // child0 → out ; in → child1
        let b = roles(1, 0); // child1 → out ; in → child0  (same composite, swapped presentation)
        assert_eq!(hash_unit(&a).unwrap(), hash_unit(&b).unwrap());
        assert_eq!(hash_unit(&a).unwrap(), hash_unit(&a).unwrap()); // and deterministic
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

    // ── Recursive types (P1.3.2) ────────────────────────────────────────────

    #[test]
    fn list_type_round_trips() {
        // A composed type — List<List<Number>> on a port, List<Any> as output — survives
        // encode∘decode byte-for-byte and structurally.
        let mut u = prim("add");
        u.inputs[0].ty = Type::List(Box::new(Type::List(Box::new(Type::Number))));
        u.outputs[0].ty = Type::list_any();
        let bytes = encode_preimage(&u).unwrap();
        assert_eq!(decode_preimage(&bytes).unwrap(), u);
        assert_eq!(encode_preimage(&decode_preimage(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn product_type_round_trips() {
        // A Product on a port (Product([Number, List<Any>])) and the empty product (Unit) as output
        // survive encode∘decode byte-for-byte and structurally.
        let mut u = prim("add");
        u.inputs[0].ty = Type::Product(vec![Type::Number, Type::list_any()]);
        u.outputs[0].ty = Type::Product(vec![]); // Unit = the 0-ary product
        let bytes = encode_preimage(&u).unwrap();
        assert_eq!(decode_preimage(&bytes).unwrap(), u);
        assert_eq!(encode_preimage(&decode_preimage(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn sum_type_round_trips() {
        // Option<Number> = Sum([Number, Unit]) on the output port survives intact, including the
        // nested Product (Unit) inside the Sum — proving the two n-ary constructors compose.
        let mut u = prim("add");
        u.outputs[0].ty = Type::Sum(vec![Type::Number, Type::Product(vec![])]);
        let bytes = encode_preimage(&u).unwrap();
        assert_eq!(decode_preimage(&bytes).unwrap(), u);
        assert_eq!(encode_preimage(&decode_preimage(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn bare_list_encodes_as_list_any() {
        // The legacy bare list is List<Any>: tag 4 followed by tag 5 (Any).
        let mut u = prim("add");
        u.inputs[0].ty = Type::list_any();
        let round = decode_preimage(&encode_preimage(&u).unwrap()).unwrap();
        assert_eq!(round.inputs[0].ty, Type::List(Box::new(Type::Any)));
    }

    #[test]
    fn deep_type_is_rejected() {
        // A maliciously over-nested type is refused at decode, never recursed into — the
        // depth bound, the one new attack surface a recursive type decoder opens.
        fn nest(depth: u32) -> Type {
            let mut t = Type::Number;
            for _ in 0..depth {
                t = Type::List(Box::new(t));
            }
            t
        }
        let mut u = prim("add");
        u.inputs[0].ty = nest(MAX_TYPE_DEPTH + 5);
        let bytes = encode_preimage(&u).unwrap(); // encoder allows it (trusted, in-memory)
        assert_eq!(decode_preimage(&bytes), Err(DecodeError::TypeTooDeep));
    }
}