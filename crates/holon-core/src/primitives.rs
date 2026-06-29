//! Native primitive compute (P1.4.2).
//!
//! The pure, first-order primitive vocabulary: `name → native compute`. Small and
//! orthogonal (KISS) — depth comes from composition, not a large library. Boundary
//! units (drivers / state / sinks) are deliberately **absent**; they are a different
//! kind (P2), not pure vocabulary.
//!
//! `compute(name, inputs, params) -> Option<Value>`. `None` means "not computable":
//! an unknown name, wrong arity, or a runtime type mismatch — the last reachable only
//! through an `any` port, since wire-time checking (P1.3.1) already rules out the rest.
//! The higher-order `Map` (and the eval `ctx` it needs) arrive with P1.5.1.

use crate::{Param, Value};

/// Dispatch a primitive by name to its native computation.
pub fn compute(name: &str, inputs: &[Value], params: &[Param]) -> Option<Value> {
    match name {
        // ── constants: value comes from a param, no inputs ──
        "number" => param(params, "value")
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| crate::determinism::is_finite_number(*n))
            .map(Value::Number),
        "boolean" => param(params, "value").and_then(parse_bool).map(Value::Boolean),
        "text" => param(params, "value").map(|s| Value::Text(s.to_string())),

        // ── arithmetic ──
        "add" => fold_numbers(inputs, 0.0, |a, b| a + b),
        "multiply" => fold_numbers(inputs, 1.0, |a, b| a * b),
        "subtract" => bin_numbers(inputs).map(|(a, b)| Value::Number(a - b)),
        "divide" => bin_numbers(inputs)
            .and_then(|(a, b)| if b == 0.0 { None } else { Some(Value::Number(a / b)) }),

        // ── comparison (op param) ──
        "compare" => {
            let (a, b) = bin_numbers(inputs)?;
            compare(a, b, param(params, "op")?).map(Value::Boolean)
        }

        // ── logic ──
        "and" => fold_bools(inputs, true, |a, b| a && b),
        "or" => fold_bools(inputs, false, |a, b| a || b),
        "not" => {
            let [v] = inputs else { return None };
            as_bool(v).map(|x| Value::Boolean(!x))
        }

        // ── control-as-data ──
        "select" => {
            let [c, a, b] = inputs else { return None };
            Some(if as_bool(c)? { a.clone() } else { b.clone() })
        }

        // ── text ──
        "concat" => {
            let mut s = String::new();
            for v in inputs {
                s.push_str(as_text(v)?);
            }
            Some(Value::Text(s))
        }
        "uppercase" => {
            let [v] = inputs else { return None };
            Some(Value::Text(as_text(v)?.to_uppercase()))
        }
        // ── text irreducibles (§13): `Text` stays opaque; these touch the encoding, so they
        // are native. Char-based throughout (Unicode scalar values), so length/slice/index
        // agree. Casing uses Unicode **default** (locale-independent) mapping — Rust's
        // to_uppercase/to_lowercase — so it can never vary by locale and poison a hash (I2). ──
        "lowercase" => {
            let [v] = inputs else { return None };
            Some(Value::Text(as_text(v)?.to_lowercase()))
        }
        "text_length" => {
            let [v] = inputs else { return None };
            Some(Value::Number(as_text(v)?.chars().count() as f64))
        }
        "slice" => {
            let [s, start, len] = inputs else { return None };
            let (s, start, len) = (as_text(s)?, as_index(start)?, as_index(len)?);
            // start past the end ⇒ empty; len past the end ⇒ truncated. Total for valid types.
            Some(Value::Text(s.chars().skip(start).take(len).collect()))
        }
        "index_of" => {
            let [h, n] = inputs else { return None };
            let (h, n) = (as_text(h)?, as_text(n)?);
            // Char index of the first match, or -1 (not-found is a normal answer, not a
            // None-propagating failure). `find` is byte-based, so re-count chars before it.
            let idx = match h.find(n) {
                Some(b) => h[..b].chars().count() as f64,
                None => -1.0,
            };
            Some(Value::Number(idx))
        }

        // ── list ──
        "list" => Some(Value::List(inputs.to_vec())),
        "get" => {
            let [l, i] = inputs else { return None };
            as_list(l)?.get(as_index(i)?).cloned()
        }
        "append" => {
            let [l, item] = inputs else { return None };
            let mut xs = as_list(l)?.to_vec();
            xs.push(item.clone());
            Some(Value::List(xs))
        }
        "length" => {
            let [l] = inputs else { return None };
            Some(Value::Number(as_list(l)?.len() as f64))
        }
        "sum" => {
            let [l] = inputs else { return None };
            let mut acc = 0.0;
            for v in as_list(l)? {
                acc += as_number(v)?;
            }
            Some(Value::Number(acc))
        }
        "join" => {
            let [l, sep] = inputs else { return None };
            let sep = as_text(sep)?;
            let mut parts = Vec::new();
            for v in as_list(l)? {
                parts.push(as_text(v)?.to_string());
            }
            Some(Value::Text(parts.join(sep)))
        }

        // ── list structure (§13 irreducibles): O(1) build / inspect. `fold`-composites
        // (sum, map, filter, …) are built from these. `head`/`tail` of `[]` ⇒ None — the
        // existing `get`-out-of-bounds convention (partial-via-None; it halts, just yields
        // no value, so totality is intact). `is_empty` is total. ──
        "cons" => {
            let [x, l] = inputs else { return None };
            let tail = as_list(l)?;
            let mut xs = Vec::with_capacity(tail.len() + 1);
            xs.push(x.clone());
            xs.extend_from_slice(tail);
            Some(Value::List(xs))
        }
        "head" => {
            let [l] = inputs else { return None };
            as_list(l)?.first().cloned()
        }
        "tail" => {
            let [l] = inputs else { return None };
            as_list(l)?.split_first().map(|(_, rest)| Value::List(rest.to_vec()))
        }
        "is_empty" => {
            let [l] = inputs else { return None };
            Some(Value::Boolean(as_list(l)?.is_empty()))
        }

        // ── boundary sink (Fork B): a sink purely *constructs its request* — representation
        // B: a self-describing `Effect { sink-name, payload }`. The capability is *derived*
        // from the name by the host (`capabilities_of`), never stored in the value. Because
        // each effect self-describes, an output can carry *many* (a `List` of them via `map`),
        // and heterogeneous effects ride out together. Drivers and state reads stay outside
        // the pure core (host-sourced / host-threaded).
        "email" => sink_effect("email", inputs),
        // A state write is a Fork-B sink too: it constructs a `store_write` request; the host
        // performs it by *storing* (capability `StateWrite`). `store_read` has no pure compute
        // — it is sourced (host-supplied), like a driver.
        "store_write" => sink_effect("store_write", inputs),
        // A reply is an egress sink too (P2.5.1): it constructs a `respond` request whose payload
        // carries the correlation token alongside the reply. `request` (ingress) has no compute —
        // it is sourced, like a driver.
        "respond" => sink_effect("respond", inputs),

        _ => None, // unknown name, or a driver/state boundary unit handled by the host
    }
}

// ── sink request constructor (Fork B / representation B) ─────────────────────
/// Construct a sink's settled request: `Effect { sink-name, payload }`. The payload is the
/// first (and only) input it was given; no input ⇒ nothing to construct. The capability is
/// derived from the name downstream, never stored here.
fn sink_effect(sink: &str, inputs: &[Value]) -> Option<Value> {
    inputs.first().map(|p| Value::Effect { sink: sink.into(), payload: Box::new(p.clone()) })
}

// ── extractors (runtime type guards) ────────────────────────────────────────
fn as_number(v: &Value) -> Option<f64> {
    if let Value::Number(n) = v { Some(*n) } else { None }
}
fn as_bool(v: &Value) -> Option<bool> {
    if let Value::Boolean(b) = v { Some(*b) } else { None }
}
fn as_text(v: &Value) -> Option<&str> {
    if let Value::Text(s) = v { Some(s) } else { None }
}
fn as_list(v: &Value) -> Option<&[Value]> {
    if let Value::List(xs) = v { Some(xs) } else { None }
}
/// A list index: a non-negative whole number.
fn as_index(v: &Value) -> Option<usize> {
    let n = as_number(v)?;
    if n >= 0.0 && n.fract() == 0.0 { Some(n as usize) } else { None }
}

// ── helpers ─────────────────────────────────────────────────────────────────
fn param<'a>(params: &'a [Param], name: &str) -> Option<&'a str> {
    params.iter().find(|p| p.name() == name).and_then(|p| p.value())
}
fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
fn fold_numbers(inputs: &[Value], init: f64, f: fn(f64, f64) -> f64) -> Option<Value> {
    let mut acc = init;
    for v in inputs {
        acc = f(acc, as_number(v)?);
    }
    Some(Value::Number(acc))
}
fn fold_bools(inputs: &[Value], init: bool, f: fn(bool, bool) -> bool) -> Option<Value> {
    let mut acc = init;
    for v in inputs {
        acc = f(acc, as_bool(v)?);
    }
    Some(Value::Boolean(acc))
}
fn bin_numbers(inputs: &[Value]) -> Option<(f64, f64)> {
    let [a, b] = inputs else { return None };
    Some((as_number(a)?, as_number(b)?))
}
fn compare(a: f64, b: f64, op: &str) -> Option<bool> {
    Some(match op {
        ">" => a > b,
        "<" => a < b,
        ">=" => a >= b,
        "<=" => a <= b,
        "=" => a == b,
        "!=" => a != b,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(n: f64) -> Value { Value::Number(n) }
    fn txt(s: &str) -> Value { Value::Text(s.into()) }
    fn boo(x: bool) -> Value { Value::Boolean(x) }
    fn list(xs: Vec<Value>) -> Value { Value::List(xs) }
    fn p(name: &str, value: &str) -> Param { Param::frozen(name, value) }

    #[test]
    fn add_sums() {
        assert_eq!(compute("add", &[num(3.0), num(7.0), num(5.0)], &[]), Some(num(15.0)));
    }
    #[test]
    fn multiply_products() {
        assert_eq!(compute("multiply", &[num(2.0), num(3.0), num(4.0)], &[]), Some(num(24.0)));
    }
    #[test]
    fn subtract_is_binary() {
        assert_eq!(compute("subtract", &[num(10.0), num(3.0)], &[]), Some(num(7.0)));
        assert_eq!(compute("subtract", &[num(1.0)], &[]), None);
    }
    #[test]
    fn divide_guards_zero() {
        assert_eq!(compute("divide", &[num(10.0), num(2.0)], &[]), Some(num(5.0)));
        assert_eq!(compute("divide", &[num(1.0), num(0.0)], &[]), None);
    }
    #[test]
    fn compare_uses_op_param() {
        assert_eq!(compute("compare", &[num(72.0), num(70.0)], &[p("op", ">")]), Some(boo(true)));
        assert_eq!(compute("compare", &[num(1.0), num(2.0)], &[p("op", ">=")]), Some(boo(false)));
        assert_eq!(compute("compare", &[num(5.0), num(5.0)], &[p("op", "=")]), Some(boo(true)));
    }
    #[test]
    fn logic_ops() {
        assert_eq!(compute("and", &[boo(true), boo(true)], &[]), Some(boo(true)));
        assert_eq!(compute("and", &[boo(true), boo(false)], &[]), Some(boo(false)));
        assert_eq!(compute("or", &[boo(false), boo(true)], &[]), Some(boo(true)));
        assert_eq!(compute("not", &[boo(true)], &[]), Some(boo(false)));
    }
    #[test]
    fn select_chooses() {
        assert_eq!(compute("select", &[boo(true), txt("yes"), txt("no")], &[]), Some(txt("yes")));
        assert_eq!(compute("select", &[boo(false), txt("yes"), txt("no")], &[]), Some(txt("no")));
    }
    #[test]
    fn text_ops() {
        assert_eq!(compute("concat", &[txt("foo"), txt("bar")], &[]), Some(txt("foobar")));
        assert_eq!(compute("uppercase", &[txt("abc")], &[]), Some(txt("ABC")));
    }
    #[test]
    fn text_irreducibles() {
        assert_eq!(compute("lowercase", &[txt("AbC")], &[]), Some(txt("abc")));
        // char-based (Unicode scalar values), not byte-based: "ab\u{00e7}" is 3 chars / 4 bytes.
        assert_eq!(compute("text_length", &[txt("ab\u{00e7}")], &[]), Some(num(3.0)));
        assert_eq!(compute("slice", &[txt("hello"), num(1.0), num(3.0)], &[]), Some(txt("ell")));
        // start past the end ⇒ empty (total, never None for valid types).
        assert_eq!(compute("slice", &[txt("hi"), num(5.0), num(2.0)], &[]), Some(txt("")));
        assert_eq!(compute("index_of", &[txt("hello"), txt("ll")], &[]), Some(num(2.0)));
        assert_eq!(compute("index_of", &[txt("hello"), txt("z")], &[]), Some(num(-1.0)));
    }
    #[test]
    fn list_construct_and_query() {
        let l = compute("list", &[num(3.0), num(7.0), num(5.0)], &[]).unwrap();
        assert_eq!(l, list(vec![num(3.0), num(7.0), num(5.0)]));
        assert_eq!(compute("length", &[l.clone()], &[]), Some(num(3.0)));
        assert_eq!(compute("get", &[l.clone(), num(1.0)], &[]), Some(num(7.0)));
        assert_eq!(compute("get", &[l.clone(), num(9.0)], &[]), None);
        assert_eq!(compute("sum", &[l.clone()], &[]), Some(num(15.0)));
        assert_eq!(
            compute("append", &[l, num(9.0)], &[]),
            Some(list(vec![num(3.0), num(7.0), num(5.0), num(9.0)]))
        );
    }
    #[test]
    fn join_strings() {
        let l = list(vec![txt("a"), txt("b"), txt("c")]);
        assert_eq!(compute("join", &[l, txt("-")], &[]), Some(txt("a-b-c")));
    }
    #[test]
    fn list_structure_primitives() {
        let l = list(vec![num(1.0), num(2.0), num(3.0)]);
        assert_eq!(
            compute("cons", &[num(0.0), l.clone()], &[]),
            Some(list(vec![num(0.0), num(1.0), num(2.0), num(3.0)]))
        );
        assert_eq!(compute("head", &[l.clone()], &[]), Some(num(1.0)));
        assert_eq!(compute("tail", &[l.clone()], &[]), Some(list(vec![num(2.0), num(3.0)])));
        assert_eq!(compute("is_empty", &[l], &[]), Some(boo(false)));
        // Empty: head/tail are partial (None — the get-out-of-bounds convention); is_empty is total.
        let empty = list(vec![]);
        assert_eq!(compute("head", &[empty.clone()], &[]), None);
        assert_eq!(compute("tail", &[empty.clone()], &[]), None);
        assert_eq!(compute("is_empty", &[empty], &[]), Some(boo(true)));
    }
    #[test]
    fn constants_from_params() {
        assert_eq!(compute("number", &[], &[p("value", "42")]), Some(num(42.0)));
        assert_eq!(compute("boolean", &[], &[p("value", "true")]), Some(boo(true)));
        assert_eq!(compute("text", &[], &[p("value", "hi")]), Some(txt("hi")));
    }
    #[test]
    fn sink_email_constructs_an_effect() {
        // Representation B: a sink constructs a self-describing request, not an identity.
        // *Which* effect it is lives in the name; the host derives the capability from it.
        assert_eq!(
            compute("email", &[txt("hello")], &[]),
            Some(Value::Effect { sink: "email".into(), payload: Box::new(txt("hello")) })
        );
        assert_eq!(compute("email", &[], &[]), None); // no payload ⇒ nothing to construct
    }
    #[test]
    fn store_write_constructs_an_effect() {
        // A state write is a Fork-B sink too: it constructs a `store_write` request.
        assert_eq!(
            compute("store_write", &[num(7.0)], &[]),
            Some(Value::Effect { sink: "store_write".into(), payload: Box::new(num(7.0)) })
        );
        assert_eq!(compute("store_read", &[], &[]), None); // read is sourced, not computed
    }
    #[test]
    fn respond_constructs_an_effect_carrying_its_token() {
        // P2.5.1: a reply is an egress sink; its payload is a [token, reply] pair, plain data.
        let payload = list(vec![num(1.0), txt("ok")]);
        assert_eq!(
            compute("respond", &[payload.clone()], &[]),
            Some(Value::Effect { sink: "respond".into(), payload: Box::new(payload) })
        );
        assert_eq!(compute("request", &[], &[]), None); // ingress is sourced, not computed
    }
    #[test]
    fn unknown_is_none() {
        assert_eq!(compute("frobnicate", &[], &[]), None);
    }
    #[test]
    fn runtime_type_mismatch_is_none() {
        // An `any` port could feed a wrong runtime type; compute guards it.
        assert_eq!(compute("add", &[num(1.0), txt("x")], &[]), None);
        assert_eq!(compute("not", &[num(1.0)], &[]), None);
        assert_eq!(compute("sum", &[list(vec![num(1.0), txt("x")])], &[]), None);
    }
    #[test]
    fn deterministic() {
        let inputs = [num(3.0), num(7.0), num(5.0)];
        assert_eq!(compute("add", &inputs, &[]), compute("add", &inputs, &[]));
    }
}