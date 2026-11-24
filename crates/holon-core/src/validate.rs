//! Graph validation (P1.3.1) — the gate upstream of evaluation.
//!
//! Three independent checks over a composite's wiring:
//! 1. **well-formedness** — edge endpoints exist (child or [`crate::BOUNDARY`]),
//!    ports are in range, and **each destination port has at most one incoming edge**
//!    (single source per input — the evaluator depends on this);
//! 2. **acyclicity** — the inner child→child graph is a DAG;
//! 3. **type compatibility** — `dst == any ∨ src == any ∨ src == dst` (§5).
//!
//! Errors are **collected** so an editor can surface all problems at once. A primitive has no
//! wiring: what is checked of one is that its ports are its catalog entry's, as the compiler and
//! the editor make them, and that each family parameter it has is one of its values
//! ([`crate::FAMILIES`]).
//!
//! ## The composite boundary
//! The composite's interface↔inner binding rides the ordinary edge mechanism via the
//! reserved [`crate::BOUNDARY`] endpoint: an edge `from_child == BOUNDARY` carries the
//! composite's input `from_port` inward; an edge `to_child == BOUNDARY` exposes an inner
//! output as the composite's output `to_port`. From the inner graph's view the boundary
//! is a *source* for inputs and a *sink* for outputs — so it never forms a cycle, and it
//! needs no new fields and no encoder change.

use crate::{Edge, Resolver, Type, Unit, UnitBody, BOUNDARY, DID};
use std::collections::BTreeSet;

/// A single validation failure.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// An edge endpoint references a child index that doesn't exist.
    ChildOutOfRange { edge: usize, child: u32 },
    /// A child referenced by hash couldn't be resolved.
    UnresolvedChild { child: u32 },
    /// An edge's source port isn't a valid output of its source child (out of range for
    /// however many outputs it declares — zero or more, P4.7; a source with no outputs at
    /// all also lands here).
    OutputPortOutOfRange { edge: usize, child: u32, port: u32 },
    /// An edge's destination port isn't a valid input of its destination child.
    InputPortOutOfRange { edge: usize, child: u32, port: u32 },
    /// A boundary-source edge names a composite-input index that doesn't exist.
    BoundaryInputOutOfRange { edge: usize, port: u32 },
    /// A boundary-sink edge names a composite-output index that doesn't exist (out of range
    /// for however many outputs the composite declares — zero or more, P4.7).
    BoundaryOutputOutOfRange { edge: usize, port: u32 },
    /// Two edges feed the same destination port (ambiguous single source).
    DuplicateInputEdge { to_child: u32, to_port: u32 },
    /// The inner wiring contains a cycle.
    Cycle,
    /// An edge connects incompatible types.
    TypeMismatch { edge: usize, src: Type, dst: Type },
    /// A module body isn't one (`module::validate_module`): imports, a missing export, a bad
    /// budget, or not WebAssembly at all.
    Module(crate::ModuleError),
    /// A family leaf's parameter isn't one of its values ([`crate::FAMILIES`]): what it holds,
    /// none when it has no such parameter.
    FamilyParam { leaf: String, param: String, value: Option<String> },
    /// A primitive names no leaf of the catalog.
    UnknownLeaf { leaf: String },
    /// A leaf's ports aren't its catalog entry's ([`leaf_port_mistakes`]): which, and how.
    LeafPorts { leaf: String, why: String },
    /// A step that does something (a write, a send, a call, a function that does one) has nothing
    /// wired to its [`crate::TRIGGER`]: what reaches it isn't said, and a step nothing reaches
    /// never runs.
    Untied { child: u32, what: String },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ChildOutOfRange { edge, child } => {
                write!(f, "edge {edge} references child {child}, which doesn't exist")
            }
            ValidationError::UnresolvedChild { child } => {
                write!(f, "child {child} couldn't be resolved")
            }
            ValidationError::OutputPortOutOfRange { edge, child, port } => write!(
                f,
                "edge {edge} reads output port {port} of child {child}, which has no such output"
            ),
            ValidationError::InputPortOutOfRange { edge, child, port } => write!(
                f,
                "edge {edge} targets input port {port} of child {child}, which has no such input"
            ),
            ValidationError::BoundaryInputOutOfRange { edge, port } => {
                write!(f, "edge {edge} names composite-input port {port}, which doesn't exist")
            }
            ValidationError::BoundaryOutputOutOfRange { edge, port } => {
                write!(f, "edge {edge} names composite-output port {port}, which doesn't exist")
            }
            ValidationError::DuplicateInputEdge { to_child, to_port } => {
                write!(f, "child {to_child}'s input port {to_port} has more than one source")
            }
            ValidationError::Cycle => write!(f, "the wiring contains a cycle"),
            ValidationError::TypeMismatch { edge, src, dst } => {
                write!(f, "edge {edge} connects incompatible types: {src:?} does not fit {dst:?}")
            }
            ValidationError::Module(e) => write!(f, "{e}"),
            ValidationError::FamilyParam { leaf, param, value } => {
                let values = crate::family_values(leaf, param).unwrap_or_default().join(", ");
                match value {
                    Some(v) => write!(f, "{leaf}'s {param} is one of {values}: \"{v}\" isn't one"),
                    None => write!(f, "{leaf}'s {param} is one of {values}, and it has none"),
                }
            }
            ValidationError::UnknownLeaf { leaf } => write!(f, "no leaf is named {leaf:?}"),
            ValidationError::LeafPorts { leaf, why } => write!(f, "{leaf} {why}"),
            ValidationError::Untied { child, what } => {
                write!(f, "child {child} ({what}) has nothing wired to its trigger, so it would never run: tie it to what reaches it")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Wire-time type compatibility (§5) — structural equality plus the single `Any`
/// escape hatch, made recursive over `List<T>`: `List<A>` fits `List<B>` iff `A` fits
/// `B`, and `Any` matches anything at any level. No inference, no subtyping — the §5
/// rule, lifted to nested types.
pub fn compatible(src: &Type, dst: &Type) -> bool {
    match (src, dst) {
        (_, Type::Any) | (Type::Any, _) => true,
        (Type::List(a), Type::List(b)) => compatible(a, b),
        (Type::Product(a), Type::Product(b)) | (Type::Sum(a), Type::Sum(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| compatible(x, y))
        }
        _ => src == dst, // scalar equality (Number / Boolean / Text / Effect)
    }
}

/// Whether a type is or contains `Effect`.
fn mentions_effect(ty: &Type) -> bool {
    match ty {
        Type::Effect => true,
        Type::List(t) => mentions_effect(t),
        Type::Product(ts) | Type::Sum(ts) => ts.iter().any(mentions_effect),
        Type::Number | Type::Boolean | Type::Text | Type::Any | Type::Record | Type::Bytes => false,
    }
}

/// The type a `const`'s or `default`'s ports take for the literal type `ty` it is frozen with,
/// when it is one a literal is read as (primitives.rs `parse_literal`): Json holds any value.
pub fn literal_type(ty: &str) -> Option<Type> {
    match ty {
        "Number" => Some(Type::Number),
        "Boolean" => Some(Type::Boolean),
        "Text" => Some(Type::Text),
        "Json" => Some(Type::Any),
        _ => None,
    }
}

/// The input a variadic leaf ([`crate::VARIADIC`]) takes after `existing`: the next letter while
/// they are single letters (a, b → c), else `in<n>`. The editor's "+" and the language's wires
/// name them so.
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

/// What a family leaf's frozen parameters hold that isn't one of their values ([`crate::FAMILIES`]).
/// An open one is a hole, checked once its template is instantiated.
fn family_mistakes(name: &str, params: &[crate::Param]) -> Vec<ValidationError> {
    crate::FAMILIES
        .iter()
        .filter(|(leaf, _, _)| *leaf == name)
        .filter_map(|(leaf, param, values)| {
            let mistake = |value: Option<&str>| ValidationError::FamilyParam { leaf: leaf.to_string(), param: param.to_string(), value: value.map(str::to_string) };
            match params.iter().find(|p| p.name() == *param) {
                None => Some(mistake(None)),
                Some(p) => p.value().filter(|v| !values.contains(v)).map(|v| mistake(Some(v))),
            }
        })
        .collect()
}

/// How a leaf's ports differ from its catalog entry's ([`crate::interface_of`]), but for what the
/// compiler and the editor make of one:
/// - a `const`'s or `default`'s port typed as the literal its frozen `type` names
///   ([`literal_type`]);
/// - a variadic leaf's ([`crate::VARIADIC`]) inputs after its entry's, each named as the "+" names
///   the next ([`next_port_name`]) and typed as one of the entry's inputs, or as `Any` for a leaf
///   that hands its extras on ([`crate::PASSES_EXTRA`]).
///
/// The host computes a leaf by its name and reads what it does from its ports, so the two are one.
fn leaf_port_mistakes(name: &str, unit: &Unit) -> Vec<ValidationError> {
    let Some(entry) = crate::interface_of(name) else {
        return vec![ValidationError::UnknownLeaf { leaf: name.to_string() }];
    };
    let mistake = |why: String| ValidationError::LeafPorts { leaf: name.to_string(), why };
    let literal = match name {
        "const" | "default" => unit.params.iter().find(|p| p.name() == "type").and_then(|p| p.value()).and_then(literal_type),
        _ => None,
    };
    let fits = |ty: &Type, catalog: &Type| ty == catalog || literal.as_ref() == Some(ty);
    let mut errors = Vec::new();
    // A higher-order leaf whose function does something takes that function's trigger last, and
    // hands out what it made at a second output (eval.rs `Did`).
    let (mut inputs, mut outputs) = (&unit.inputs[..], &unit.outputs[..]);
    if crate::PASSES_EXTRA.contains(&name) {
        if let Some((last, rest)) = inputs.split_last()
            && last.name == crate::TRIGGER
        {
            if last.ty != Type::Any {
                errors.push(mistake(format!("has its trigger typed {:?}, where it is Any", last.ty)));
            }
            inputs = rest;
        }
        if let [answer, did] = outputs {
            if did.name != DID || did.ty != Type::Effect {
                errors.push(mistake(format!("has output 1 {:?}: {:?}, where one more is {DID:?}: Effect", did.name, did.ty)));
            }
            outputs = std::slice::from_ref(answer);
        }
    }
    for (which, have, want) in [("input", inputs, &entry.inputs[..]), ("output", outputs, &entry.outputs[..])] {
        let grows = which == "input" && crate::VARIADIC.contains(&name);
        if have.len() < want.len() || (have.len() > want.len() && !grows) {
            errors.push(mistake(format!("has {} {which}s, where the catalog gives it {}", have.len(), want.len())));
        }
        for (i, (p, w)) in have.iter().zip(want.iter()).enumerate() {
            if p.name != w.name || !fits(&p.ty, &w.ty) {
                errors.push(mistake(format!("has {which} {i} {:?}: {:?}, where the catalog has {:?}: {:?}", p.name, p.ty, w.name, w.ty)));
            }
        }
        if !grows {
            continue;
        }
        let passes = crate::PASSES_EXTRA.contains(&name);
        for i in want.len()..have.len() {
            let names: Vec<&str> = have[..i].iter().map(|p| p.name.as_str()).collect();
            let (next, p) = (next_port_name(&names), &have[i]);
            if p.name != next || !(want.iter().any(|w| w.ty == p.ty) || (passes && p.ty == Type::Any)) {
                let typed = if passes { "typed as one of its inputs or Any" } else { "typed as one of its inputs" };
                errors.push(mistake(format!("has {which} {i} {:?}: {:?}, where one more is {next:?}, {typed}", p.name, p.ty)));
            }
        }
    }
    errors
}

/// Validate a unit's graph. A primitive passes when its ports are its catalog entry's
/// ([`leaf_port_mistakes`]) and each family parameter it has is one of its values. A composite is
/// checked for well-formedness, acyclicity, and type compatibility — including its boundary edges.
pub fn validate(unit: &Unit, resolver: &impl Resolver) -> Result<(), Vec<ValidationError>> {
    let (children, edges) = match &unit.body {
        UnitBody::Primitive { name } => {
            let mut errors = leaf_port_mistakes(name, unit);
            errors.extend(family_mistakes(name, &unit.params));
            return if errors.is_empty() { Ok(()) } else { Err(errors) };
        }
        UnitBody::Composite { children, edges } => (children, edges),
        // A module is admitted by its own sections: no imports, the ABI's exports, sane budgets.
        UnitBody::Module { bytes } => {
            let mut errors = Vec::new();
            if let Err(e) = crate::module::validate_module(bytes) {
                errors.push(ValidationError::Module(e));
            }
            if let Err(e) = crate::module::module_limits(&unit.params) {
                errors.push(ValidationError::Module(e));
            }
            return if errors.is_empty() { Ok(()) } else { Err(errors) };
        }
    };

    let mut errors = Vec::new();
    let n = children.len();

    // Resolve each child once (interface lookups need it).
    let resolved: Vec<Option<&Unit>> = children.iter().map(|h| resolver.resolve(h)).collect();
    for (i, r) in resolved.iter().enumerate() {
        if r.is_none() {
            errors.push(ValidationError::UnresolvedChild { child: i as u32 });
        }
    }

    // Track destination ports so a second source to the same port is rejected.
    let mut seen_dst: BTreeSet<(u32, u32)> = BTreeSet::new();

    for (ei, e) in edges.iter().enumerate() {
        if !seen_dst.insert((e.to_child, e.to_port)) {
            errors.push(ValidationError::DuplicateInputEdge {
                to_child: e.to_child,
                to_port: e.to_port,
            });
            continue;
        }

        // ---- source type (a real child's output, or a composite input via BOUNDARY) ----
        let src_ty = if e.from_child == BOUNDARY {
            match unit.inputs.get(e.from_port as usize) {
                Some(p) => p.ty.clone(),
                None => {
                    errors.push(ValidationError::BoundaryInputOutOfRange { edge: ei, port: e.from_port });
                    continue;
                }
            }
        } else {
            let fc = e.from_child as usize;
            if fc >= n {
                errors.push(ValidationError::ChildOutOfRange { edge: ei, child: e.from_child });
                continue;
            }
            match resolved[fc] {
                None => continue, // unresolved child already reported
                Some(src) => match src.outputs.get(e.from_port as usize) {
                    Some(p) => p.ty.clone(),
                    None => {
                        errors.push(ValidationError::OutputPortOutOfRange {
                            edge: ei,
                            child: e.from_child,
                            port: e.from_port,
                        });
                        continue;
                    }
                },
            }
        };

        // ---- destination type (a real child's input, or the composite output via BOUNDARY) ----
        let dst_ty = if e.to_child == BOUNDARY {
            match unit.outputs.get(e.to_port as usize) {
                Some(p) => p.ty.clone(),
                None => {
                    errors.push(ValidationError::BoundaryOutputOutOfRange { edge: ei, port: e.to_port });
                    continue;
                }
            }
        } else {
            let tc = e.to_child as usize;
            if tc >= n {
                errors.push(ValidationError::ChildOutOfRange { edge: ei, child: e.to_child });
                continue;
            }
            match resolved[tc] {
                None => continue,
                Some(dst) => match dst.inputs.get(e.to_port as usize) {
                    Some(p) => p.ty.clone(),
                    None => {
                        errors.push(ValidationError::InputPortOutOfRange {
                            edge: ei,
                            child: e.to_child,
                            port: e.to_port,
                        });
                        continue;
                    }
                },
            }
        };

        // A sink's payload is data: an Effect-typed source never fits it, `Any` port or not
        // (primitives.rs's `sink_effect` refuses one that only shows up at run time).
        // A trigger is never read, so it may be an effect: `after saved` ties a write to a put.
        let into_sink = e.to_child != BOUNDARY
            && matches!(resolved[e.to_child as usize], Some(Unit { body: UnitBody::Primitive { name }, inputs, .. })
                if crate::primitives::is_sink(name) && inputs.get(e.to_port as usize).is_none_or(|p| p.name != crate::TRIGGER));
        if !compatible(&src_ty, &dst_ty) || (into_sink && mentions_effect(&src_ty)) {
            errors.push(ValidationError::TypeMismatch { edge: ei, src: src_ty, dst: dst_ty });
        }
    }

    if has_cycle(n, edges) {
        errors.push(ValidationError::Cycle);
    }

    // Not connected means never: a step that takes a trigger runs only where one arrives, so one
    // left unwired is a step that never runs, which no flow means. Refused here, where a unit is
    // checked whoever built it.
    for (ci, child) in resolved.iter().enumerate() {
        let Some(child) = child else { continue };
        for (p, port) in child.inputs.iter().enumerate() {
            if port.name == crate::TRIGGER && !edges.iter().any(|e| e.to_child == ci as u32 && e.to_port == p as u32) {
                let what = match &child.body {
                    UnitBody::Primitive { name } => name.clone(),
                    UnitBody::Composite { .. } => "a function".to_string(),
                    UnitBody::Module { .. } => "a module".to_string(),
                };
                errors.push(ValidationError::Untied { child: ci as u32, what });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Kahn's algorithm over the in-range **child→child** edges. Boundary edges (an
/// endpoint == [`BOUNDARY`], which is not `< n`) are excluded, so the composite
/// boundary can never form a false cycle. The boolean result is order-independent.
fn has_cycle(n: usize, edges: &[Edge]) -> bool {
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        let (f, t) = (e.from_child as usize, e.to_child as usize);
        if f < n && t < n {
            adj[f].push(t);
            indeg[t] += 1;
        }
    }
    let mut stack: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut removed = 0usize;
    while let Some(node) = stack.pop() {
        removed += 1;
        for &m in &adj[node] {
            indeg[m] -= 1;
            if indeg[m] == 0 {
                stack.push(m);
            }
        }
    }
    removed != n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, Hash, MapResolver, Port, Unit, UnitBody};

    fn num_binop(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![
                Port { name: "a".into(), ty: Type::Number },
                Port { name: "b".into(), ty: Type::Number },
            ],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn text_source() -> Unit {
        // A Text-typed wiring stub (validate never evaluates it), so a bare `const` suffices —
        // no `value`/`type` params needed for type-checking.
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Text }],
        }
    }
    fn composite(children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        composite_io(vec![Port { name: "a".into(), ty: Type::Number }], Type::Number, children, edges)
    }
    fn composite_io(inputs: Vec<Port>, out: Type, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs,
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    fn edge(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }

    // ── inner-wiring checks ──────────────────────────────────────────────
    #[test]
    fn compatibility_rule() {
        use Type::*;
        assert!(compatible(&Number, &Number));
        assert!(compatible(&Any, &Number));
        assert!(compatible(&Number, &Any));
        assert!(!compatible(&Number, &Text));
        assert!(!compatible(&Text, &Number));
    }

    #[test]
    fn compatible_is_structural() {
        use Type::*;
        let list = |t: Type| Type::List(Box::new(t));
        // List<Any> bridges any element type (constructed lists stay permissive).
        assert!(compatible(&Type::list_any(), &list(Number)));
        assert!(compatible(&list(Number), &Type::list_any()));
        // Two precisely-declared list types are checked element-wise.
        assert!(compatible(&list(Number), &list(Number)));
        assert!(!compatible(&list(Text), &list(Number)));
        // …and recursively.
        assert!(compatible(&list(list(Number)), &list(list(Any))));
        assert!(!compatible(&list(list(Text)), &list(list(Number))));
        // A list is not a scalar.
        assert!(!compatible(&list(Number), &Number));
        // Product: arity + position, Any wildcard at each slot; a Product is not a List.
        let pair = |a: Type, b: Type| Type::Product(vec![a, b]);
        assert!(compatible(&pair(Number, Text), &pair(Number, Any)));
        assert!(!compatible(&pair(Number, Text), &pair(Number, Number)));
        assert!(!compatible(&pair(Number, Text), &Type::Product(vec![Number]))); // arity
        assert!(!compatible(&pair(Number, Text), &list(Number))); // Product ≠ List

        // Sum: the same arity-+-position rule as Product, but a Sum is not a Product, even at
        // identical shape — a tagged choice and a tuple are different things (P1.3.2 inch 3).
        let option_of = |t: Type| Type::Sum(vec![t, Type::Product(vec![])]); // T or Unit
        assert!(compatible(&option_of(Number), &option_of(Any)));
        assert!(!compatible(&option_of(Number), &option_of(Text)));
        assert!(!compatible(&Type::Sum(vec![Number, Text]), &Type::Sum(vec![Number]))); // arity
        assert!(!compatible(&Type::Product(vec![Number, Text]), &Type::Sum(vec![Number, Text])));
    }

    /// A family leaf's parameter is one of its values, or the unit is refused: `compare`'s `op`
    /// would otherwise give nothing each time it ran. A hole is filled, and checked, later.
    #[test]
    fn a_family_parameter_outside_its_values_is_refused() {
        let r = MapResolver::new();
        let compare = |params: Vec<crate::Param>| Unit { params, ..crate::interface_of("compare").unwrap() };
        for op in ["=", "!=", "<", "<=", ">", ">="] {
            assert_eq!(validate(&compare(vec![crate::Param::frozen("op", op)]), &r), Ok(()), "{op}");
        }
        let errors = validate(&compare(vec![crate::Param::frozen("op", "~")]), &r).unwrap_err();
        assert_eq!(errors, vec![ValidationError::FamilyParam { leaf: "compare".into(), param: "op".into(), value: Some("~".into()) }]);
        assert_eq!(errors[0].to_string(), "compare's op is one of =, !=, <, <=, >, >=: \"~\" isn't one");
        let errors = validate(&compare(vec![]), &r).unwrap_err();
        assert_eq!(errors[0].to_string(), "compare's op is one of =, !=, <, <=, >, >=, and it has none");
        assert_eq!(validate(&compare(vec![crate::Param::open("op", "OP")]), &r), Ok(()));
        // Every family names a genesis leaf, its parameter is one the catalog gives it, and the
        // catalog's default is one of its values, so a dropped node is valid at once.
        for (leaf, param, values) in crate::FAMILIES {
            let unit = crate::interface_of(leaf).unwrap_or_else(|| panic!("{leaf} isn't a leaf"));
            let default = unit.params.iter().find(|p| p.name() == *param).and_then(|p| p.value());
            assert!(default.is_some_and(|d| values.contains(&d)), "{leaf}'s {param} defaults to {default:?}");
            assert_eq!(validate(&unit, &r), Ok(()), "{leaf}");
        }
    }

    /// **Guards:** a leaf's ports are its catalog entry's (`interface_of`): its inputs (how many,
    /// their names and types, a sink's trigger last) and its output, but for what the compiler
    /// and the editor make of one: a `const`'s or `default`'s ports typed as its literal, and a
    /// variadic leaf's extra inputs, named as its "+" names them and typed as one of its own.
    ///
    /// **Why:** a leaf is computed by its name, and the host reads its ports: what an approval
    /// shows, what is refused and how a value is wired are said of them. A unit's bytes are
    /// whoever sent them (`POST /unit`), so ports of its own would have the host check one leaf
    /// and run another.
    ///
    /// **If it fails:** a leaf with ports of its own deploys, and the host's checks read the
    /// ports it was given rather than the ones its computation has.
    ///
    /// Every catalog leaf: each port renamed and retyped, one more and one fewer, each refused; as
    /// the catalog has it, and as the compiler grows or retypes it, taken. The names a variadic
    /// leaf grows are said here again (a, b → c; else in3), apart from the code that names them.
    #[test]
    fn a_leafs_ports_are_its_catalog_entrys() {
        use crate::{interface_of, Param, Port, GENESIS, PASSES_EXTRA, VARIADIC};
        let r = MapResolver::new();
        let mut counts = [0usize; 2];
        let mut check = |unit: &Unit, taken: bool, what: String| {
            let got = validate(unit, &r);
            assert_eq!(got.is_ok(), taken, "{what}: {got:?}\n{unit:?}");
            counts[taken as usize] += 1;
        };
        let port = |name: &str, ty: &Type| Port { name: name.into(), ty: ty.clone() };
        // A type none of `tys` is (never Any).
        let unlike = |tys: &[&Type]| [Type::Bytes, Type::Record, Type::Effect, Type::Text].into_iter().find(|t| !tys.contains(&t)).expect("one of four");
        let next_name = |ports: &[Port]| {
            let letters = !ports.is_empty() && ports.iter().all(|p| p.name.len() == 1 && p.name.as_bytes()[0].is_ascii_lowercase());
            match ports.iter().map(|p| p.name.as_bytes()[0]).max() {
                Some(max) if letters && max < b'z' => ((max + 1) as char).to_string(),
                _ => format!("in{}", ports.len() + 1),
            }
        };
        fn side(unit: &mut Unit, input: bool) -> &mut Vec<Port> {
            if input { &mut unit.inputs } else { &mut unit.outputs }
        }
        let mut seen = 0;
        for name in GENESIS {
            let honest = interface_of(name).expect("a genesis leaf");
            let with = |change: &dyn Fn(&mut Unit)| {
                let mut unit = honest.clone();
                change(&mut unit);
                unit
            };
            check(&honest, true, format!("{name} as the catalog has it"));
            for input in [true, false] {
                let was: Vec<Type> = side(&mut honest.clone(), input).iter().map(|p| p.ty.clone()).collect();
                let which = if input { "input" } else { "output" };
                for (i, ty) in was.iter().enumerate() {
                    let to = unlike(&[ty]);
                    check(&with(&|u| side(u, input)[i].name.push('2')), false, format!("{name}'s {which} {i} renamed"));
                    check(&with(&|u| side(u, input)[i].ty = to.clone()), false, format!("{name}'s {which} {i} retyped {to:?}"));
                }
                if !was.is_empty() {
                    check(&with(&|u| drop(side(u, input).pop())), false, format!("{name} with one {which} fewer"));
                }
            }
            let out = honest.outputs[0].ty.clone();
            check(&with(&|u| u.outputs.push(port("out2", &out))), false, format!("{name} with one output more"));
            // One input more: taken where the leaf is variadic and it is named and typed as the
            // compiler's or the editor's "+" makes it, refused anywhere else.
            let next = next_name(&honest.inputs);
            let own: Vec<&Type> = honest.inputs.iter().map(|p| &p.ty).collect();
            let variadic = VARIADIC.contains(name);
            let first = own.first().copied().cloned().unwrap_or(Type::Any);
            let last = own.last().copied().cloned().unwrap_or(Type::Any);
            for ty in [first, last] {
                check(&with(&|u| u.inputs.push(port(&next, &ty))), variadic, format!("{name} with one more input, {next}: {ty:?}"));
            }
            check(&with(&|u| u.inputs.push(port(&next, &Type::Any))), variadic && (PASSES_EXTRA.contains(name) || own.contains(&&Type::Any)), format!("{name} with one more input, {next}: Any"));
            check(&with(&|u| u.inputs.push(port("zz", &own.first().copied().cloned().unwrap_or(Type::Any)))), false, format!("{name} with one more input, misnamed"));
            check(&with(&|u| u.inputs.push(port(&next, &unlike(&own)))), false, format!("{name} with one more input, of a type it has none of"));
            if variadic {
                seen += 1;
                // Three more, as the "+" names them, each as the first is typed.
                let grown = with(&|u| {
                    for _ in 0..3 {
                        let n = next_name(&u.inputs);
                        let ty = u.inputs[0].ty.clone();
                        u.inputs.push(port(&n, &ty));
                    }
                });
                check(&grown, true, format!("{name} grown by three"));
            }
        }
        // The 4th catalog fact must name real leaves — a typo here would silently make the
        // editor offer to grow a port on a primitive that doesn't exist.
        for &n in VARIADIC {
            assert!(GENESIS.contains(&n), "{n} is in VARIADIC but not GENESIS");
        }
        assert_eq!(seen, VARIADIC.len(), "every variadic leaf is a genesis leaf, once");
        // `add` is the catalog's Any + Any: typed as numbers it isn't its entry, and the
        // refusal says which port and what the catalog has.
        let errors = validate(&num_binop("add"), &r).unwrap_err();
        assert_eq!(errors[0].to_string(), "add has input 0 \"a\": Number, where the catalog has \"a\": Any");
        // A `const` or `default` typed as its literal, its ports with it; not as another.
        for name in ["const", "default"] {
            let honest = interface_of(name).unwrap();
            for (ty, port_ty) in [("Number", Type::Number), ("Boolean", Type::Boolean), ("Text", Type::Text), ("Json", Type::Any)] {
                let literal = |port_ty: &Type| Unit {
                    params: vec![Param::frozen("value", "1"), Param::frozen("type", ty)],
                    inputs: honest.inputs.iter().map(|p| port(&p.name, port_ty)).collect(),
                    outputs: honest.outputs.iter().map(|p| port(&p.name, port_ty)).collect(),
                    ..honest.clone()
                };
                check(&literal(&port_ty), true, format!("{name} typed {ty}"));
                let not = if port_ty == Type::Text { Type::Boolean } else { Type::Text };
                check(&literal(&not), false, format!("{name} typed {ty}, its ports {not:?}"));
            }
        }
        check(&Unit { body: UnitBody::Primitive { name: "nope".into() }, params: vec![], inputs: vec![], outputs: vec![] }, false, "a leaf the catalog hasn't".into());
        let [refused, taken] = counts;
        assert!(refused > 1_500 && taken > 200, "{refused} refused, {taken} taken");
    }

    #[test]
    fn valid_composite_passes() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 1, 0)]);
        assert_eq!(validate(&c, &r), Ok(()));
    }

    #[test]
    fn out_of_range_child_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 5, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::ChildOutOfRange { edge: 0, child: 5 }));
    }

    #[test]
    fn cycle_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 1, 0), edge(1, 0, 0, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::Cycle));
    }

    #[test]
    fn type_mismatch_rejected() {
        let mut r = MapResolver::new();
        let ht = r.insert(text_source()).unwrap();
        let hn = r.insert(num_binop("add")).unwrap();
        let c = composite(vec![ht, hn], vec![edge(0, 0, 1, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::TypeMismatch { edge: 0, src: Type::Text, dst: Type::Number }));
    }

    #[test]
    fn an_effect_never_fits_a_sink_payload() {
        let mut r = MapResolver::new();
        let egress = r.insert(crate::interface_of("egress").unwrap()).unwrap();
        let store_write = r.insert(crate::interface_of("store_write").unwrap()).unwrap();
        let text = r.insert(text_source()).unwrap();
        let mut effects = text_source();
        effects.outputs[0].ty = Type::List(Box::new(Type::Effect));
        let effects = r.insert(effects).unwrap();
        let mismatch = |src| ValidationError::TypeMismatch { edge: 0, src, dst: Type::Any };
        // What it says of the types alone: these sinks are left untied, which is refused apart.
        let validate = |c: &Unit, r: &MapResolver| {
            validate(c, r).or_else(|e| {
                let e: Vec<_> = e.into_iter().filter(|e| !matches!(e, ValidationError::Untied { .. })).collect();
                if e.is_empty() { Ok(()) } else { Err(e) }
            })
        };

        // egress(egress(x)): the outer payload port is Any, and still refuses the Effect.
        let nested = composite(vec![egress, egress], vec![edge(0, 0, 1, 0)]);
        assert_eq!(validate(&nested, &r), Err(vec![mismatch(Type::Effect)]));
        // The same for a state write, and for effects inside a list.
        assert_eq!(validate(&composite(vec![egress, store_write], vec![edge(0, 0, 1, 0)]), &r), Err(vec![mismatch(Type::Effect)]));
        // An outbound request's inputs too: its body is a payload.
        let request = r.insert(crate::interface_of("http_request").unwrap()).unwrap();
        let into_body = composite(vec![egress, request], vec![edge(0, 0, 1, 2)]);
        assert_eq!(validate(&into_body, &r), Err(vec![mismatch(Type::Effect)]));
        let listed = composite(vec![effects, egress], vec![edge(0, 0, 1, 0)]);
        assert_eq!(validate(&listed, &r), Err(vec![mismatch(Type::List(Box::new(Type::Effect)))]));
        // Data into a sink is fine; an Effect out to the composite boundary is how effects leave.
        assert_eq!(validate(&composite(vec![text, egress], vec![edge(0, 0, 1, 0)]), &r), Ok(()));
        let mut out = composite(vec![egress], vec![edge(0, 0, BOUNDARY, 0)]);
        out.outputs = vec![Port { name: "out".into(), ty: Type::Effect }];
        assert_eq!(validate(&out, &r), Ok(()));
    }

    #[test]
    fn output_port_out_of_range_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 1, 1, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, ValidationError::OutputPortOutOfRange { .. })));
    }

    #[test]
    fn unresolved_child_rejected() {
        let r = MapResolver::new();
        let c = composite(vec![[0u8; 32]], vec![]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::UnresolvedChild { child: 0 }));
    }

    #[test]
    fn collects_multiple_errors() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let h2 = r.insert(text_source()).unwrap();
        let c = composite(vec![h0, h1, h2], vec![edge(0, 0, 1, 0), edge(1, 0, 0, 0), edge(2, 0, 0, 1)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::Cycle));
        assert!(errs.iter().any(|e| matches!(e, ValidationError::TypeMismatch { .. })));
        assert!(errs.len() >= 2);
    }

    // ── boundary checks ──────────────────────────────────────────────────
    #[test]
    fn boundary_wiring_valid() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        // input a → add.a ; add.out → output
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Type::Number,
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(validate(&c, &r), Ok(()));
    }

    #[test]
    fn boundary_input_type_mismatch() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Text }], // Text feeding a Number port
            Type::Number,
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0)],
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::TypeMismatch { edge: 0, src: Type::Text, dst: Type::Number }));
    }

    #[test]
    fn boundary_input_out_of_range() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Type::Number,
            vec![hadd],
            vec![edge(BOUNDARY, 5, 0, 0)], // composite has only input 0
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::BoundaryInputOutOfRange { edge: 0, port: 5 }));
    }

    #[test]
    fn boundary_output_out_of_range() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Type::Number,
            vec![hadd],
            vec![edge(0, 0, BOUNDARY, 3)], // only output 0 exists
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::BoundaryOutputOutOfRange { edge: 0, port: 3 }));
    }

    #[test]
    fn duplicate_input_edge_rejected() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let hmul = r.insert(num_binop("multiply")).unwrap();
        // two sources feed child 1, port 0
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Type::Number,
            vec![hadd, hmul],
            vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 0)],
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::DuplicateInputEdge { to_child: 1, to_port: 0 }));
    }

    #[test]
    fn boundary_does_not_create_false_cycle() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let hmul = r.insert(num_binop("multiply")).unwrap();
        // BOUNDARY → add → mul → BOUNDARY : a normal input→output flow, acyclic
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Type::Number,
            vec![hadd, hmul],
            vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, 1, 0), edge(1, 0, BOUNDARY, 0)],
        );
        assert_eq!(validate(&c, &r), Ok(()));
    }
}