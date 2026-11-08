//! The record leaves: JavaScript's `Object` functions, `in`, spread, `typeof` and `Object.is` on
//! Holon's records and values (docs/engineering/themes/js-library-races/records.md, "Leaf specs").
//! Each was a native candidate in the races (`crates/holon-author/benches/races_records.rs`).
//!
//! A record keeps its keys sorted (records.md, "Key order"), so `values` and a merge come in key
//! order where JavaScript's come in the order the keys were made. Each takes a record where it says
//! so and gives nothing for anything else, where JavaScript would throw or convert. None computes a
//! float, so each gives the same natively and in wasm.
//!
//! `group_by` applies its function in the evaluator, where `map` applies `map`'s; what it does with
//! the answers is here.

use std::collections::BTreeMap;

use crate::Value;

/// `Object.values(r)`: the record's values, in its key order.
pub(crate) fn values(fields: &BTreeMap<String, Value>) -> Value {
    Value::List(fields.values().cloned().collect())
}

/// Whether `entry` is one `Object.fromEntries` reads: a list whose item 0 is a text (the key) and
/// which has an item 1 (the value); items past 1 are ignored.
pub(crate) fn is_entry(entry: &Value) -> bool {
    matches!(entry, Value::List(e) if matches!(e.first(), Some(Value::Text(_))) && e.len() >= 2)
}

/// `Object.fromEntries(es)`: each entry's key and value, a later key replacing an earlier; nothing
/// when an entry isn't one ([`is_entry`]): JavaScript stores `undefined` for a missing value, which
/// a record can't hold, and converts a key that isn't a text.
pub(crate) fn from_entries(entries: &[Value]) -> Option<Value> {
    let mut fields = BTreeMap::new();
    for e in entries {
        let Value::List(e) = e else { return None };
        let (Some(Value::Text(k)), Some(x)) = (e.first(), e.get(1)) else { return None };
        fields.insert(k.clone(), x.clone());
    }
    Some(Value::Record(fields))
}

/// [`from_entries`] on entries it may take apart, each already checked to be one.
pub(crate) fn from_owned_entries(entries: Vec<Value>) -> Value {
    let mut fields = BTreeMap::new();
    for e in entries {
        let Value::List(e) = e else { unreachable!("checked to be an entry") };
        let mut e = e.into_iter();
        let (Some(Value::Text(k)), Some(x)) = (e.next(), e.next()) else { unreachable!("checked to be an entry") };
        fields.insert(k, x);
    }
    Value::Record(fields)
}

/// `{ ...a, ...b }`, `Object.assign({}, a, b)`: every field of every record, a later record's
/// where two have one; shallow (a nested record is replaced, not merged). No record is `{}`.
pub(crate) fn merge(records: &[&Value]) -> Option<Value> {
    let mut fields = BTreeMap::new();
    for r in records {
        let Value::Record(more) = r else { return None };
        fields.extend(more.iter().map(|(k, x)| (k.clone(), x.clone())));
    }
    Some(Value::Record(fields))
}

/// `Object.hasOwn(r, k)`, `k in r`, `r.hasOwnProperty(k)`: whether the record has the key, with
/// any value, JSON's `null` included. A record inherits nothing, so `"toString"` is no key of it.
pub(crate) fn has(fields: &BTreeMap<String, Value>, key: &str) -> Value {
    Value::Boolean(fields.contains_key(key))
}

/// `Object.groupBy(xs, f)` once `f` has answered for each item: the items in lists by their key,
/// each list in the items' order, the groups in key order (JavaScript's come first-seen).
pub(crate) fn grouped(groups: BTreeMap<String, Vec<Value>>) -> Value {
    Value::Record(groups.into_iter().map(|(k, xs)| (k, Value::List(xs))).collect())
}

/// `typeof x`: what JavaScript says of the value Holon's is. JSON's `null` is `none()`, which
/// `typeof` calls `"object"`; `some(x)` is written as `x` in JSON, so it is `typeof x`. An effect
/// has no JavaScript value, and gives nothing. Never `"function"`, `"symbol"` or `"bigint"`: Holon
/// has none of those values.
pub(crate) fn type_of(x: &Value) -> Option<&'static str> {
    Some(match x {
        Value::Number(_) => "number",
        Value::Boolean(_) => "boolean",
        Value::Text(_) => "string",
        Value::List(_) | Value::Record(_) | Value::Bytes(_) | Value::Sum { arm: 1, .. } => "object",
        Value::Sum { payload, .. } => return type_of(payload),
        Value::Effect { .. } => return None,
    })
}

/// `Object.is(a, b)`: ECMA-262's SameValue. Numbers by their bits, so `-0` isn't `0` (and any NaN
/// is NaN); lists and records by what they hold, numbers in them by bits, where JavaScript compares
/// identity, which Holon's values haven't.
pub(crate) fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Value::List(xs), Value::List(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same_value(x, y)),
        (Value::Record(xs), Value::Record(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|((k, x), (l, y))| k == l && same_value(x, y)),
        (Value::Sum { arm: a, payload: x }, Value::Sum { arm: b, payload: y }) => a == b && same_value(x, y),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;
    use crate::primitives::compute;

    fn v(json: &str) -> Value {
        parse(json).unwrap_or_else(|| panic!("{json}"))
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn num(x: f64) -> Value {
        Value::Number(x)
    }
    fn none() -> Value {
        Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
    }
    fn leaf(name: &str, inputs: &[Value]) -> Option<Value> {
        compute(name, &inputs.iter().collect::<Vec<_>>(), &[])
    }

    /// The bench's edge records: empty, one key, nested, keys whose insertion order isn't their
    /// sorted order, a `-0`, an empty list and JSON's `null` as values.
    fn edge_records() -> Vec<Value> {
        vec![
            v("{}"),
            v(r#"{"a": 1}"#),
            v(r#"{"b": {"c": [1, {"d": true}]}, "a": "x"}"#),
            v(r#"{"b": 1, "a": 2, "10": 3, "2": 4, "é": 5, "😀": 6, "": 7}"#),
            Value::Record(BTreeMap::from([("z".into(), num(-0.0)), ("y".into(), Value::List(vec![])), ("n".into(), none())])),
        ]
    }

    #[test]
    fn values_and_entries_come_in_key_order() {
        assert_eq!(leaf("values", &[v(r#"{"b": 1, "a": 2, "10": 3, "2": 4}"#)]), Some(v("[3, 4, 2, 1]")));
        assert_eq!(leaf("values", &[v("{}")]), Some(v("[]")));
        for r in edge_records() {
            let Value::Record(fields) = &r else { unreachable!() };
            assert_eq!(leaf("values", std::slice::from_ref(&r)), Some(Value::List(fields.values().cloned().collect())), "{r:?}");
            assert_eq!(leaf("keys", std::slice::from_ref(&r)), Some(Value::List(fields.keys().map(|k| txt(k)).collect())));
        }
        assert_eq!(leaf("values", &[v("[1]")]), None, "a list's values are no record's");
        assert_eq!(leaf("values", &[txt("ab")]), None);
    }

    #[test]
    fn from_entries_takes_each_entrys_key_and_value_the_last_winning() {
        let cases: [(&str, Option<&str>); 7] = [
            ("[]", Some("{}")),
            (r#"[["a", 1]]"#, Some(r#"{"a": 1}"#)),
            (r#"[["b", 1], ["a", {"c": [2]}], ["10", 3], ["2", 4]]"#, Some(r#"{"10": 3, "2": 4, "a": {"c": [2]}, "b": 1}"#)),
            (r#"[["a", 1], ["a", 2]]"#, Some(r#"{"a": 2}"#)),
            (r#"[["a", 1, 9]]"#, Some(r#"{"a": 1}"#)),
            (r#"[["a"]]"#, None),
            (r#"[[1, 2]]"#, None),
        ];
        for (es, want) in cases {
            assert_eq!(leaf("from_entries", &[v(es)]), want.map(v), "{es}");
        }
        let minus_zero = Value::List(vec![Value::List(vec![txt("z"), num(-0.0)])]);
        assert_eq!(leaf("from_entries", &[minus_zero]), Some(Value::Record(BTreeMap::from([("z".into(), num(-0.0))]))));
        assert_eq!(leaf("from_entries", &[v(r#"["ab"]"#)]), None, "an entry that isn't a list");
        // Its entries round trip.
        for r in edge_records() {
            assert_eq!(leaf("entries", std::slice::from_ref(&r)).and_then(|es| leaf("from_entries", &[es])), Some(r));
        }
    }

    #[test]
    fn merge_takes_every_field_a_later_records_winning() {
        let cases = [
            ("{}", "{}", "{}"),
            (r#"{"a": 1}"#, "{}", r#"{"a": 1}"#),
            ("{}", r#"{"a": 1}"#, r#"{"a": 1}"#),
            (r#"{"a": 1, "b": 2}"#, r#"{"b": 3, "c": 4}"#, r#"{"a": 1, "b": 3, "c": 4}"#),
            (r#"{"a": {"x": 1}}"#, r#"{"a": {"y": 2}}"#, r#"{"a": {"y": 2}}"#),
        ];
        for (a, b, want) in cases {
            assert_eq!(leaf("merge", &[v(a), v(b)]), Some(v(want)), "{a} {b}");
        }
        let other = v(r#"{"a": 0, "2": 0}"#);
        for r in edge_records() {
            let Value::Record(fields) = &r else { unreachable!() };
            let mut over = fields.clone();
            over.extend([("a".to_string(), num(0.0)), ("2".to_string(), num(0.0))]);
            assert_eq!(leaf("merge", &[r.clone(), other.clone()]), Some(Value::Record(over)));
            let mut under = BTreeMap::from([("a".to_string(), num(0.0)), ("2".to_string(), num(0.0))]);
            under.extend(fields.clone());
            assert_eq!(leaf("merge", &[other.clone(), r.clone()]), Some(Value::Record(under)));
        }
        // Variadic, as `add`: three, one and none.
        assert_eq!(leaf("merge", &[v(r#"{"a": 1}"#), v(r#"{"b": 2}"#), v(r#"{"a": 3}"#)]), Some(v(r#"{"a": 3, "b": 2}"#)));
        assert_eq!(leaf("merge", &[v(r#"{"a": 1}"#)]), Some(v(r#"{"a": 1}"#)));
        assert_eq!(leaf("merge", &[]), Some(v("{}")));
        assert_eq!(leaf("merge", &[v("{}"), v("[1]")]), None, "a list isn't spread as a record");
    }

    #[test]
    fn has_finds_a_key_with_any_value_and_nothing_inherited() {
        let keys = ["a", "b", "c", "10", "2", "😀", "", "n", "z"];
        for r in edge_records() {
            let Value::Record(fields) = &r else { unreachable!() };
            for k in keys {
                assert_eq!(leaf("has", &[r.clone(), txt(k)]), Some(Value::Boolean(fields.contains_key(k))), "{r:?} {k}");
            }
        }
        assert_eq!(leaf("has", &[v(r#"{"": 0}"#), txt("")]), Some(Value::Boolean(true)));
        assert_eq!(leaf("has", &[v(r#"{"k": null}"#), txt("k")]), Some(Value::Boolean(true)), "\"k\" in {{ k: null }}");
        assert_eq!(leaf("has", &[v("{}"), txt("toString")]), Some(Value::Boolean(false)));
        assert_eq!(leaf("has", &[v(r#"{"1": 0}"#), num(1.0)]), None, "a number key isn't converted");
        assert_eq!(leaf("has", &[v("[0]"), txt("0")]), None, "a list has no keys");
    }

    #[test]
    fn type_of_says_what_javascripts_typeof_does() {
        let cases = [
            (num(0.0), "number"),
            (num(-0.0), "number"),
            (num(-1.5), "number"),
            (num(1e300), "number"),
            (txt(""), "string"),
            (txt("x"), "string"),
            (txt("\"q"), "string"),
            (txt("true"), "string"),
            (txt("[1]"), "string"),
            (Value::Boolean(true), "boolean"),
            (Value::Boolean(false), "boolean"),
            (v("[]"), "object"),
            (v("[1, [2]]"), "object"),
            (v("{}"), "object"),
            (v(r#"{"a": {"b": 1}}"#), "object"),
            (none(), "object"),
            (Value::Sum { arm: 0, payload: Box::new(txt("x")) }, "string"),
        ];
        for (x, want) in cases {
            assert_eq!(leaf("type_of", std::slice::from_ref(&x)), Some(txt(want)), "{x:?}");
        }
        let effect = Value::Effect { sink: "egress".into(), payload: Box::new(num(1.0)) };
        assert_eq!(leaf("type_of", &[effect]), None);
    }

    #[test]
    fn same_value_tells_minus_zero_from_zero_and_compares_by_value() {
        let cases = [
            (num(0.0), num(-0.0), false),
            (num(-0.0), num(-0.0), true),
            (num(1.0), num(1.0), true),
            (num(f64::NAN), num(f64::NAN), true),
            (v(r#"{"a": [0]}"#), v(r#"{"a": [0]}"#), true),
            (Value::List(vec![num(0.0)]), Value::List(vec![num(-0.0)]), false),
            (txt("1"), num(1.0), false),
            (none(), none(), true),
            (none(), Value::Sum { arm: 0, payload: Box::new(Value::List(vec![])) }, false),
        ];
        for (a, b, is) in cases {
            assert_eq!(leaf("same_value", &[a.clone(), b.clone()]), Some(Value::Boolean(is)), "Object.is({a:?}, {b:?})");
            assert_eq!(leaf("same_value", &[b.clone(), a.clone()]), Some(Value::Boolean(is)), "Object.is({b:?}, {a:?})");
        }
    }

    #[test]
    fn grouped_items_keep_their_order_in_key_order() {
        let groups = BTreeMap::from([("y".to_string(), vec![num(2.0)]), ("x".to_string(), vec![num(1.0), num(3.0)])]);
        assert_eq!(grouped(groups), v(r#"{"x": [1, 3], "y": [2]}"#));
        assert_eq!(grouped(BTreeMap::new()), v("{}"));
    }
}
