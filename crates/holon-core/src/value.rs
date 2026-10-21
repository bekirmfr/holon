//! Runtime values that flow on wires (P1.4.1).
//!
//! A [`Value`] is the concrete datum a unit produces and consumes. It is immutable
//! with value-semantics: cloning deep-copies, equality is structural. Every value
//! has exactly one *concrete* type ([`Value::ty`]) — never the `Any` wildcard, which
//! is a wire-time type, not a runtime value.

use std::collections::BTreeMap;

use crate::Type;

/// A runtime value — the concrete counterpart of the monomorphic base types in
/// [`Type`] (§5). `Number` is a single IEEE-754 `f64` (one numeric type, not an
/// int/float split); `List` is heterogeneous and may nest.
///
/// Derives `PartialEq` but **not** `Eq`/`Hash`: `f64` has no total equality (NaN).
/// Canonical hashing of *values* (needed later by the memoization cache, P3.3.1) is
/// a deliberate canonical encoder, never `derive(Hash)`; NaN / float-canonicalization
/// discipline is deferred to the determinism harness (P1.4.4).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Boolean(bool),
    Text(String),
    List(Vec<Value>),
    /// A settled effect *request* (representation B): the sink's **name** plus the payload it
    /// constructed. The capability is *derived* from the name (`capabilities_of`), never stored
    /// here — so this layer needs no `kind` dependency, and the name joins the convention the
    /// rest of the system already keys on (`compute`, `capabilities_of`, `kind_of`). The pure
    /// core only ever *constructs* and *carries* an effect; the host alone *performs* it.
    Effect { sink: String, payload: Box<Value> },
    /// A tagged choice (P1.3.2 inch 3): `arm` says *which* alternative of a `Sum([T0, T1, …])`
    /// this is, `payload` is that alternative's value. The untaken arms are not carried — a sum
    /// value is type-erasing in a way `List`/`Product` are not (see [`Value::ty`]). `Option<T> =
    /// Sum([T, Unit])`: `none()` is `{arm: 1, payload: List([])}` (`Unit` = the empty product,
    /// which already rides the list value); `some(x)` is `{arm: 0, payload: x}`.
    Sum { arm: usize, payload: Box<Value> },
    /// Named fields (M1): request and response bodies, stored rows, JSON objects. A `BTreeMap`,
    /// so fields are always in key order: equality, the memo encoding and `to_text` never
    /// depend on the order fields were added in.
    Record(BTreeMap<String, Value>),
}

impl Value {
    /// The concrete type of this value. Total: every value maps to exactly one
    /// non-`Any` [`Type`]. (`Any` is a wire-time wildcard; no runtime value is `Any`.)
    ///
    /// `Sum` is the one case where this is a **coarsening of convenience, not a sound
    /// supremum**: a sum value only ever carries its *taken* arm, so its true arity is not
    /// recoverable from the value alone (unlike `List`, whose `Any`-wildcard coarsening is
    /// always sound because `List` is unary-recursive). `Sum(vec![Any, Any])` — arity 2 — is
    /// honest *today* only because the sole constructors, `some`/`none`, both build `Option`
    /// (`Sum([T, Unit])`). If a generic n-ary sum injector is ever added, this arm needs
    /// rethinking then — not engineered around now, ahead of that need.
    pub fn ty(&self) -> Type {
        match self {
            Value::Number(_) => Type::Number,
            Value::Boolean(_) => Type::Boolean,
            Value::Text(_) => Type::Text,
            Value::List(_) => Type::list_any(),
            Value::Effect { .. } => Type::Effect,
            Value::Sum { .. } => Type::Sum(vec![Type::Any, Type::Any]),
            Value::Record(_) => Type::Record,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ty_agrees_with_type() {
        assert_eq!(Value::Number(1.0).ty(), Type::Number);
        assert_eq!(Value::Boolean(true).ty(), Type::Boolean);
        assert_eq!(Value::Text("x".into()).ty(), Type::Text);
        assert_eq!(Value::List(vec![]).ty(), Type::list_any());
        assert_eq!(
            Value::Effect { sink: "egress".into(), payload: Box::new(Value::Text("hi".into())) }.ty(),
            Type::Effect
        );
        assert_eq!(
            Value::Sum { arm: 0, payload: Box::new(Value::Number(1.0)) }.ty(),
            Type::Sum(vec![Type::Any, Type::Any])
        );
    }

    #[test]
    fn no_value_is_any() {
        // Every value has a concrete type; `Any` is wire-time only.
        for v in [
            Value::Number(0.0),
            Value::Boolean(false),
            Value::Text(String::new()),
            Value::List(vec![Value::Number(1.0)]),
            Value::Effect { sink: "egress".into(), payload: Box::new(Value::Number(1.0)) },
            Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) },
        ] {
            assert_ne!(v.ty(), Type::Any);
        }
    }

    #[test]
    fn value_semantics_clone_is_independent() {
        let original = Value::List(vec![
            Value::Number(3.0),
            Value::Text("a".into()),
            Value::List(vec![Value::Boolean(true)]),
        ]);
        let clone = original.clone();
        assert_eq!(original, clone);

        // Mutating the clone's contents must not touch the original.
        if let Value::List(mut inner) = clone {
            inner.push(Value::Number(9.0));
            assert_ne!(Value::List(inner), original);
        } else {
            unreachable!("clone of a List is a List");
        }
    }

    #[test]
    fn equality_is_structural() {
        assert_eq!(Value::Number(15.0), Value::Number(15.0));
        assert_ne!(Value::Number(15.0), Value::Number(16.0));
        assert_ne!(Value::Number(1.0), Value::Boolean(true)); // cross-type ≠
        assert_eq!(
            Value::List(vec![Value::Number(1.0), Value::Number(2.0)]),
            Value::List(vec![Value::Number(1.0), Value::Number(2.0)]),
        );
        assert_ne!(
            Value::List(vec![Value::Number(1.0)]),
            Value::List(vec![Value::Number(1.0), Value::Number(2.0)]),
        );
    }

    #[test]
    fn sum_arm_is_part_of_identity() {
        // The whole point of a tag: the SAME payload at a DIFFERENT arm is a different value —
        // `some(None)`-shaped confusion (mistaking which alternative was taken) must be
        // impossible. This is what `arm` buys over just storing the payload.
        let a = Value::Sum { arm: 0, payload: Box::new(Value::Number(5.0)) };
        let b = Value::Sum { arm: 1, payload: Box::new(Value::Number(5.0)) };
        assert_ne!(a, b);
        assert_eq!(a, Value::Sum { arm: 0, payload: Box::new(Value::Number(5.0)) });
    }
}