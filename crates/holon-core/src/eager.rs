//! The evaluator as it was before lazy choices (docs/L4s/lazy-choices-L4.md): every child
//! evaluated, in one topological pass, with no one to answer a request, a read or a waiting
//! point. Kept only as the oracle the lazy evaluator is checked against: for every unit and
//! input, [`crate::eval`] must give what [`eval_eager`] gives. Not for running flows.

use crate::eval::topo_order;
use crate::hash::parse_hash;
use crate::{primitives, Edge, Hash, Param, Resolver, Unit, UnitBody, Value, BOUNDARY, VARIADIC};
use std::collections::BTreeMap;

/// Every output of `unit` on `inputs`, evaluating every node.
pub fn eval_eager(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> Vec<Option<Value>> {
    let inputs = if crate::takes_trigger(unit) && inputs.len() == unit.inputs.len() { &inputs[..inputs.len() - 1] } else { inputs };
    match &unit.body {
        UnitBody::Primitive { name } => vec![primitive(name, unit, inputs, resolver)],
        UnitBody::Composite { children, edges } => composite(children, edges, unit.outputs.len(), inputs, resolver),
        UnitBody::Module { bytes } => crate::eval::run_module_outputs(bytes, unit, inputs, resolver).0,
    }
}

fn primitive(name: &str, unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> Option<Value> {
    match name {
        "map" => map(inputs, &unit.params, resolver),
        "fold" => fold(inputs, &unit.params, resolver),
        "any" | "all" | "filter" | "find" | "find_index" | "find_last" | "find_last_index" => test(name, inputs, &unit.params, resolver),
        "sort_by" => sort_by(inputs, &unit.params, resolver),
        "group_by" => group_by(inputs, &unit.params, resolver),
        "json_parse" | "json_stringify" if unit.params.iter().any(|p| p.name() == "fn" && p.value().is_some_and(|f| !f.is_empty())) => json(name, inputs, &unit.params, resolver),
        // The leaves that ask the host: nobody answers here.
        "http_request" | "http_upload" | "http_stream" | "flow_call" | "flow_start" | "await" | "collection_get" | "collection_list" | "file_read" | "file_list" | "file_handle" | "file_lines" | "file_slice"
        | "db_query" | "db_one" | "db_find" | "db_find_one" | "resume_token" => None,
        "coalesce" => inputs.iter().flatten().next().cloned(),
        _ => {
            if VARIADIC.contains(&name) && inputs.iter().any(Option::is_none) {
                return None;
            }
            let present: Vec<&Value> = inputs.iter().flatten().collect();
            primitives::compute(name, &present, &unit.params)
        }
    }
}

fn composite(children: &[Hash], edges: &[Edge], n_outputs: usize, inputs: &[Option<Value>], resolver: &impl Resolver) -> Vec<Option<Value>> {
    let n = children.len();
    let mut resolved: Vec<&Unit> = Vec::with_capacity(n);
    for h in children {
        match resolver.resolve(h) {
            Some(u) => resolved.push(u),
            None => return vec![None; n_outputs],
        }
    }
    let mut incoming: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in edges {
        incoming.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
    }
    let Some(order) = topo_order(n, edges) else { return vec![None; n_outputs] };
    let mut scratch: Vec<Vec<Option<Value>>> = vec![Vec::new(); n];
    let output_of = |scratch: &[Vec<Option<Value>>], fc: u32, fp: u32| scratch.get(fc as usize).and_then(|o| o.get(fp as usize)).cloned().flatten();
    for ci in order {
        let child = resolved[ci];
        let k = child.inputs.len();
        let mut gathered: Vec<Option<Value>> = (0..k)
            .map(|p| match incoming.get(&(ci as u32, p as u32)) {
                Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(),
                Some(&(fc, fp)) => output_of(&scratch, fc, fp),
                None => None,
            })
            .collect();
        crate::eval::open_triggers(child, &mut gathered, crate::open_trigger, |p| incoming.contains_key(&(ci as u32, p as u32)));
        let wired_but_absent = (0..k).any(|p| gathered[p].is_none() && incoming.contains_key(&(ci as u32, p as u32)));
        let held = wired_but_absent
            && matches!(&child.body, UnitBody::Primitive { name } if primitives::has_optional_ports(name) || crate::TRIGGERED.contains(&name.as_str()));
        scratch[ci] = if held { vec![None; child.outputs.len()] } else { eval_eager(child, &gathered, resolver) };
    }
    (0..n_outputs as u32)
        .map(|op| match incoming.get(&(BOUNDARY, op)) {
            Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(),
            Some(&(fc, fp)) => output_of(&scratch, fc, fp),
            None => None,
        })
        .collect()
}

/// The function a `map`/`fold` applies, applied: its one output.
fn apply(unit: &Unit, args: &[Option<Value>], resolver: &impl Resolver) -> Option<Value> {
    let mut outs = eval_eager(unit, args, resolver);
    if outs.len() == 1 { outs.pop().unwrap() } else { None }
}

fn function<'r>(params: &[Param], resolver: &'r impl Resolver) -> Option<&'r Unit> {
    let hex = params.iter().find(|p| p.name() == "fn").and_then(|p| p.value())?;
    resolver.resolve(&parse_hash(hex)?)
}

fn map(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(Value::List(xs))) = inputs.first() else { return None };
    let extra = &inputs[1..];
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let unit = function(params, resolver)?;
    let out: Vec<Option<Value>> = xs.iter().map(|x| apply(unit, &[&[Some(x.clone())][..], extra].concat(), resolver)).collect();
    out.into_iter().collect::<Option<Vec<Value>>>().map(Value::List)
}

fn fold(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(init)) = inputs.first() else { return None };
    let Some(Some(Value::List(xs))) = inputs.get(1) else { return None };
    let extra = inputs.get(2..).unwrap_or_default();
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let unit = function(params, resolver)?;
    let mut acc = init.clone();
    for x in xs {
        acc = apply(unit, &[&[Some(acc), Some(x.clone())][..], extra].concat(), resolver)?;
    }
    Some(acc)
}

/// The function's answer for one item, read as JavaScript's ToBoolean; none when it gives none.
fn passes(unit: &Unit, x: &Value, extra: &[Option<Value>], resolver: &impl Resolver) -> Option<bool> {
    apply(unit, &[&[Some(x.clone())][..], extra].concat(), resolver).map(|v| crate::lists::truthy(&v))
}

/// `any`, `all`, `filter` and the `find` family, each stopping where it decides.
fn test(name: &str, inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(Value::List(xs))) = inputs.first() else { return None };
    let extra = &inputs[1..];
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let unit = function(params, resolver)?;
    let pass = |x: &Value| passes(unit, x, extra, resolver);
    Some(match name {
        "filter" => {
            let answers: Vec<bool> = xs.iter().map(pass).collect::<Option<_>>()?;
            Value::List(xs.iter().zip(answers).filter(|(_, keep)| *keep).map(|(x, _)| x.clone()).collect())
        }
        "any" | "all" => {
            let decides = name == "any";
            for x in xs {
                if pass(x)? == decides {
                    return Some(Value::Boolean(decides));
                }
            }
            Value::Boolean(!decides)
        }
        _ => {
            let backward = matches!(name, "find_last" | "find_last_index");
            let positions: Vec<usize> = if backward { (0..xs.len()).rev().collect() } else { (0..xs.len()).collect() };
            for k in positions {
                if pass(&xs[k])? {
                    return Some(if matches!(name, "find" | "find_last") { xs[k].clone() } else { Value::Number(k as f64) });
                }
            }
            if matches!(name, "find" | "find_last") {
                return None;
            }
            Value::Number(-1.0)
        }
    })
}

/// `sort_by`, through the same pinned merge sort.
fn sort_by(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(Value::List(xs))) = inputs.first() else { return None };
    let extra = &inputs[1..];
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let unit = function(params, resolver)?;
    let order = crate::lists::merge_sort(xs.len(), |a, b| match apply(unit, &[&[Some(xs[a].clone()), Some(xs[b].clone())][..], extra].concat(), resolver) {
        Some(Value::Number(n)) => Ok(n > 0.0),
        _ => Err(()),
    })
    .ok()?;
    Some(Value::List(order.into_iter().map(|k| xs[k].clone()).collect()))
}

/// `group_by`: every item's key, then the groups.
fn group_by(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(Value::List(xs))) = inputs.first() else { return None };
    let extra = &inputs[1..];
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let unit = function(params, resolver)?;
    let keys: Vec<Value> = xs.iter().map(|x| apply(unit, &[&[Some(x.clone())][..], extra].concat(), resolver)).collect::<Option<_>>()?;
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (x, key) in xs.iter().zip(keys) {
        let Value::Text(key) = key else { return None };
        groups.entry(key).or_default().push(x.clone());
    }
    Some(Value::Record(groups.into_iter().map(|(k, xs)| (k, Value::List(xs))).collect()))
}

/// `json_parse` with a reviver, `json_stringify` with a replacer: the same walks, each application
/// the function of the key and the value.
fn json(name: &str, inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let Some(Some(first)) = inputs.first() else { return None };
    let extra = &inputs[1..];
    if extra.iter().any(Option::is_none) {
        return None;
    }
    let param = |k: &str| params.iter().find(|p| p.name() == k).and_then(|p| p.value()).unwrap_or("");
    let unit = function(params, resolver)?;
    let mut answer = |key: &str, value: &Value| Ok::<_, ()>(apply(unit, &[&[Some(Value::Text(key.to_string())), Some(value.clone())][..], extra].concat(), resolver));
    if name == "json_parse" {
        let Value::Text(text) = first else { return None };
        return crate::json::revive(crate::json::parse(text)?, &mut answer).ok()?;
    }
    if !param("keys").is_empty() {
        return None;
    }
    let replaced = crate::json::replace(first, &mut answer).ok()??;
    crate::json::stringify_laid_out(&replaced, &crate::json::Layout { gap: crate::json::gap(param("indent")), keys: None }).map(Value::Text)
}
