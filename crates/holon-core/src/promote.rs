//! Param promotion — open / instantiate (P1.5.2 b/c).
//!
//! A `Param::Open { name, bind }` is a hole: `name` is the role `compute` looks up once
//! filled, `bind` is the open-parameter key it is filled by. A unit with any open param in
//! its closure is a **template** — not evaluable until instantiated.
//!
//! - [`open_param`] turns a frozen param into a hole (creates/extends a template).
//! - [`open_params`] is the **derived** open interface: the set of `bind` keys across the
//!   closure (union over every reference: children and a `map`/`fold`'s `fn`) — never stored (I8).
//! - [`instantiate`] specializes a template: substitute bindings throughout the closure,
//!   re-hash bottom-up, insert the new concrete units, return the top hash. The closure is what
//!   [`crate::child_refs`] says a unit references, so a hole inside a `fn` is filled too, and the
//!   `fn` re-pinned to the instance (as a fork re-pins it). Binding is by
//!   `bind` key; partial binding leaves the rest open. The result is a first-class,
//!   content-addressed instance (I4/I5) — eval and the cache treat it like any other unit.

use crate::hash::{parse_hash, to_hex};
use crate::{child_refs, EncodeError, Hash, MapResolver, Param, Resolver, Unit, UnitBody};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq)]
pub enum ParamError {
    /// No frozen param with this role name to open.
    ParamNotFound(String),
    /// A unit referenced during instantiate couldn't be resolved.
    Unresolved(Hash),
    /// Re-encoding a rebuilt unit failed.
    Encode(EncodeError),
}

/// Turn a frozen param (by role `name`) into a hole keyed by `bind`. Pure.
pub fn open_param(unit: &Unit, name: &str, bind: &str) -> Result<Unit, ParamError> {
    let idx = unit
        .params
        .iter()
        .position(|p| p.name() == name && !p.is_open())
        .ok_or_else(|| ParamError::ParamNotFound(name.to_string()))?;
    let mut params = unit.params.clone();
    params[idx] = Param::open(name, bind);
    Ok(Unit {
        body: unit.body.clone(),
        params,
        inputs: unit.inputs.clone(),
        outputs: unit.outputs.clone(),
    })
}

/// The derived open-param interface: the `bind` keys of every hole in the closure.
pub fn open_params(unit: &Unit, resolver: &impl Resolver) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    collect_open(unit, resolver, &mut set);
    set
}

fn collect_open(unit: &Unit, resolver: &impl Resolver, set: &mut BTreeSet<String>) {
    for p in &unit.params {
        if let Some(bind) = p.bind() {
            set.insert(bind.to_string());
        }
    }
    for r in child_refs(unit) {
        if let Some(child) = resolver.resolve(&r) {
            collect_open(child, resolver, set);
        }
    }
}

/// Specialize a template: fill the given `bind→value` bindings throughout the closure,
/// re-hash bottom-up, insert the new concrete units into `store`, and return the top hash.
/// Unprovided bindings stay open (partial instantiation). Content-addressing dedups
/// identical results, so an instance whose bindings change nothing maps back to itself.
pub fn instantiate(
    template: &Hash,
    bindings: &BTreeMap<String, String>,
    store: &mut MapResolver,
) -> Result<Hash, ParamError> {
    let unit = store
        .resolve(template)
        .ok_or(ParamError::Unresolved(*template))?
        .clone();

    let mut params: Vec<Param> = unit.params.iter().map(|p| substitute(p, bindings)).collect();
    // A `fn` param references a unit (a `map`'s function): specialized like a child, and
    // re-pinned to the instance. Bound just now or already frozen, either way.
    for p in &mut params {
        if p.name() == "fn"
            && let Some(target) = p.value().and_then(parse_hash)
        {
            *p = Param::frozen("fn", to_hex(&instantiate(&target, bindings, store)?));
        }
    }

    let body = match &unit.body {
        UnitBody::Primitive { name } => UnitBody::Primitive { name: name.clone() },
        UnitBody::Composite { children, edges } => {
            let mut new_children = Vec::with_capacity(children.len());
            for ch in children {
                new_children.push(instantiate(ch, bindings, store)?);
            }
            UnitBody::Composite { children: new_children, edges: edges.clone() }
        }
    };

    let new_unit = Unit { body, params, inputs: unit.inputs.clone(), outputs: unit.outputs };
    store.insert(new_unit).map_err(ParamError::Encode)
}

fn substitute(p: &Param, bindings: &BTreeMap<String, String>) -> Param {
    match p {
        Param::Open { name, bind } if bindings.contains_key(bind) => {
            Param::frozen(name.clone(), bindings[bind].clone())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eval, Edge, Port, Type, Value, BOUNDARY};

    fn number(v: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", v)],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn prim(name: &str, n_in: usize) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: (0..n_in).map(|i| Port { name: format!("i{i}"), ty: Type::Any }).collect(),
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    fn binding(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    fn num(n: f64) -> Value { Value::Number(n) }

    #[test]
    fn open_then_instantiate_is_identity() {
        // open number(5)'s value as "K", then bind K=5 ⇒ back to number(5) (equal hash).
        let mut store = MapResolver::new();
        let h5 = store.insert(number("5")).unwrap();
        let template = open_param(&number("5"), "value", "K").unwrap();
        let ht = store.insert(template).unwrap();
        let hinst = instantiate(&ht, &binding(&[("K", "5")]), &mut store).unwrap();
        assert_eq!(hinst, h5);
    }

    #[test]
    fn family_by_hash() {
        let mut store = MapResolver::new();
        let ht = store.insert(open_param(&number("0"), "value", "K").unwrap()).unwrap();
        let h2 = instantiate(&ht, &binding(&[("K", "2")]), &mut store).unwrap();
        let h3 = instantiate(&ht, &binding(&[("K", "3")]), &mut store).unwrap();
        assert_ne!(h2, h3);
        let u2 = store.resolve(&h2).unwrap().clone();
        let u3 = store.resolve(&h3).unwrap().clone();
        assert_eq!(eval(&u2, &[], &store).into_vec(), vec![Some(num(2.0))]);
        assert_eq!(eval(&u3, &[], &store).into_vec(), vec![Some(num(3.0))]);
    }

    // MultiplyByK template: x → mul.0 ; number(Open "K") → mul.1 ; mul → out.
    fn multiply_by_k(store: &mut MapResolver) -> (Unit, Hash) {
        let hnk = store.insert(open_param(&number("1"), "value", "K").unwrap()).unwrap();
        let hmul = store.insert(prim("multiply", 2)).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![hmul, hnk],
                edges: vec![e(BOUNDARY, 0, 0, 0), e(1, 0, 0, 1), e(0, 0, BOUNDARY, 0)],
            },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Number }],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let h = store.insert(comp.clone()).unwrap();
        (comp, h)
    }

    #[test]
    fn open_params_is_derived() {
        let mut store = MapResolver::new();
        let (comp, _) = multiply_by_k(&mut store);
        let got = open_params(&comp, &store);
        assert_eq!(got, BTreeSet::from(["K".to_string()]));
    }

    #[test]
    fn primitive_template_is_not_evaluable() {
        // A number with an open hole has no value ⇒ eval is None until instantiated.
        let mut store = MapResolver::new();
        let nk = open_param(&number("1"), "value", "K").unwrap();
        assert_eq!(eval(&nk, &[], &store).into_vec(), vec![None]);

        let hnk = store.insert(nk).unwrap();
        let h = instantiate(&hnk, &binding(&[("K", "9")]), &mut store).unwrap();
        let n9 = store.resolve(&h).unwrap().clone();
        assert_eq!(eval(&n9, &[], &store).into_vec(), vec![Some(num(9.0))]);
    }

    #[test]
    fn instantiate_composite_specializes() {
        // MultiplyByK ⇒ instantiate K=3 ⇒ MultiplyBy3 ⇒ 7 * 3 = 21.
        // (Note: the un-instantiated template does NOT eval to None here — multiply is
        // variadic, so a missing operand is just absent. Templates are "evaluate only
        // after instantiation" by contract, not by a hard eval-time guard.)
        let mut store = MapResolver::new();
        let (_, hcomp) = multiply_by_k(&mut store);
        let h3 = instantiate(&hcomp, &binding(&[("K", "3")]), &mut store).unwrap();
        let mul3 = store.resolve(&h3).unwrap().clone();
        assert_eq!(eval(&mul3, &[Some(num(7.0))], &store).into_vec(), vec![Some(num(21.0))]);
    }

    fn map_over(fn_param: Param) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![fn_param],
            inputs: vec![Port { name: "list".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        }
    }

    #[test]
    fn a_hole_inside_a_fn_is_found_and_filled() {
        // map(MultiplyByK) over [1, 2]: K is part of map's open interface, and binding it
        // specializes the function map calls, re-pinned to the instance.
        let mut store = MapResolver::new();
        let (_, hk) = multiply_by_k(&mut store);
        let template = map_over(Param::frozen("fn", to_hex(&hk)));
        assert_eq!(open_params(&template, &store), BTreeSet::from(["K".to_string()]));
        let ht = store.insert(template).unwrap();

        let h3 = instantiate(&ht, &binding(&[("K", "3")]), &mut store).unwrap();
        let map3 = store.resolve(&h3).unwrap().clone();
        assert!(open_params(&map3, &store).is_empty());
        assert_ne!(child_refs(&map3), vec![hk], "re-pinned off the template");
        let list = Value::List(vec![num(1.0), num(2.0)]);
        assert_eq!(eval(&map3, &[Some(list.clone())], &store).into_vec(), vec![Some(Value::List(vec![num(3.0), num(6.0)]))]);

        // The function itself bound through an open `fn`: followed once bound, in the same pass.
        let hopen = store.insert(map_over(Param::open("fn", "F"))).unwrap();
        let h2 = instantiate(&hopen, &binding(&[("F", &to_hex(&hk)), ("K", "2")]), &mut store).unwrap();
        let map2 = store.resolve(&h2).unwrap().clone();
        assert_eq!(eval(&map2, &[Some(list)], &store).into_vec(), vec![Some(Value::List(vec![num(2.0), num(4.0)]))]);

        // A fn that doesn't resolve is an error, as an unresolved child is.
        let dangling = store.insert(map_over(Param::frozen("fn", to_hex(&[7; 32])))).unwrap();
        assert_eq!(instantiate(&dangling, &binding(&[]), &mut store), Err(ParamError::Unresolved([7; 32])));
    }

    #[test]
    fn partial_instantiation_leaves_rest_open() {
        // add(A, B) with two open holes; bind only A ⇒ B stays open.
        let mut store = MapResolver::new();
        let hna = store.insert(open_param(&number("0"), "value", "A").unwrap()).unwrap();
        let hnb = store.insert(open_param(&number("0"), "value", "B").unwrap()).unwrap();
        let hadd = store.insert(prim("add", 2)).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![hadd, hna, hnb],
                edges: vec![e(1, 0, 0, 0), e(2, 0, 0, 1), e(0, 0, BOUNDARY, 0)],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let hcomp = store.insert(comp).unwrap();
        let hpart = instantiate(&hcomp, &binding(&[("A", "5")]), &mut store).unwrap();
        let part = store.resolve(&hpart).unwrap().clone();
        assert_eq!(open_params(&part, &store), BTreeSet::from(["B".to_string()]));
    }
}