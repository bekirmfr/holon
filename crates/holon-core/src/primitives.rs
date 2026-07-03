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
pub fn compute(name: &str, inputs: &[&Value], params: &[Param]) -> Option<Value> {
    match name {
        // ── const (§13): the one literal constructor. `value` is the literal as text; `type`
        // (Number/Boolean/Text) declares how to read it — no inference, so the unit is
        // self-describing and two authors who meant different types never collide (I5). A value
        // that doesn't parse for its type ⇒ None; List/Effect/Any have no literal form (an empty
        // list is the niladic `list`), ⇒ None. ──
        "const" => parse_literal(param(params, "type")?, param(params, "value")?),

        // A value that passes through when present, or falls back to a frozen literal when
        // absent — the piece that was missing for "read this, or use a default if it was
        // never written" (the same bootstrap gap the very first counter and this greeting
        // flow's state-read both hit, worked around both times with a five-node
        // list/is_empty/select/head chain because a fixed-arity primitive can't tell which of
        // two WIRED inputs survived once one goes absent). A frozen param never has that
        // problem — it's baked in at construction time, never subject to eval-time absence at
        // all — so this is the one place that asymmetry is actually usable. Reuses `const`'s
        // own literal parsing via `parse_literal`, not a second copy of it: `default` with no
        // input present and `type`/`value` set IS a `const`, and this keeps
        // that fact true by construction rather than by two implementations happening to agree.
        "default" => match inputs {
            [v] => Some((*v).clone()),
            [] => parse_literal(param(params, "type")?, param(params, "value")?),
            _ => None,
        },

        // Structural equality over ANY two values, of any kind — including two of DIFFERENT
        // kinds, which are always false, never coerced toward each other first. This is not
        // new comparison logic: `Value` already derives Rust's own `PartialEq`, and this is
        // exactly that, exposed as a primitive. That derived impl is what makes this safe in
        // the way `compare` (Number-only) never had to worry about: Rust's own generated
        // equality for an enum matches BOTH sides to the SAME variant before ever looking at
        // content — Text("true") and Boolean(true) fall into the enum's own "different variant
        // ⇒ not equal" case before either value's content is inspected at all, not because of
        // any check written here. No `to_text` anywhere in this path — a text-conversion
        // approach was considered and rejected specifically because it WOULD conflate them
        // (both stringify to "true"), and that's a real bug this design doesn't have, not
        // an edge case it happens to handle correctly. `Number` equality is plain `f64`
        // equality (IEEE-754's own rules, including `NaN != NaN`) — the same considered,
        // undisguised choice `is_truthy` already made for the identical reason: it's the
        // correct, standard behavior, not something to invent an exception for.
        "equals" => {
            let [a, b] = inputs else { return None };
            Some(Value::Boolean(a == b))
        }

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
            Some(if as_bool(c)? { (*a).clone() } else { (*b).clone() })
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

        // ── universal (any → text): the one primitive that accepts every value kind, not a
        // fixed shape — a flow's own way to turn arbitrary data into text (an egress message
        // describing a value, a log line), the same job the editor's own `formatValue` does
        // for on-screen display. Deliberately kept in lockstep with it, not a second,
        // independently-drifting convention: quoted text (so a nested Text and a nested Number
        // that happen to look the same, e.g. "5" vs 5, stay distinguishable), an Effect's own
        // payload always shown (not just its sink name — the exact bug just fixed on the
        // editor side, worth not re-introducing here), a Sum's arm shown by index, never
        // guessed as "Some"/"None" (core carries no name for which arm means which — showing
        // the index is honest, not a guess). Total over every `Value` shape, including a
        // non-finite Number: `const` already refuses to construct one directly, but arithmetic
        // can still produce one as an intermediate value, and a to-text conversion that
        // sometimes just fails is a worse primitive than one that always produces something,
        // even for an edge case — Rust's own `f64::Display` already prints "NaN"/"inf"/"-inf"
        // as fixed, deterministic literals, not a platform-dependent guess (I2).
        "to_text" => {
            let [v] = inputs else { return None };
            Some(Value::Text(to_text_str(v)))
        }

        // The other universal any-typed primitive alongside `to_text`, same reasoning: one
        // uniform check that works before you know a value's runtime type — reading
        // `store_read(key)` (declared `Any`), say, where you often genuinely don't. Total over
        // every `Value` shape:
        //   Number   → nonzero is true (n != 0.0; NaN, being != everything including itself
        //              under IEEE 754, is therefore true here — a real, deliberate choice, not
        //              an oversight: "not equal to zero" is exactly what this rule says, and
        //              different languages disagree on NaN's truthiness anyway, so there is no
        //              single "obviously correct" convention to defer to instead)
        //   Boolean  → itself (the identity case)
        //   Text     → non-empty is true
        //   List     → non-empty is true (covers Product too — a product's value already rides
        //              List, nothing special-cased for it)
        //   Effect   → always true. An Effect can structurally never be "empty" — `payload:
        //              Box<Value>` is never null in safe Rust, and `sink` is always a real,
        //              non-empty name built from a primitive's own identity. There is no
        //              falsy Effect to detect, so it isn't invented; this just falls into
        //              "true by default", the same as any value with no established falsy form.
        //   Sum      → arm 0 (`some`) is true, arm 1 (`none`) is false. This system's Option —
        //              "is a real value present" is exactly what a truthiness check on an
        //              Option should answer, and it's the same "empty container is falsy"
        //              theme List/Text already carry, not a new idea.
        "is_truthy" => {
            let [v] = inputs else { return None };
            let truthy = match v {
                Value::Number(n) => *n != 0.0,
                Value::Boolean(b) => *b,
                Value::Text(s) => !s.is_empty(),
                Value::List(xs) => !xs.is_empty(),
                Value::Effect { .. } => true,
                Value::Sum { arm, .. } => *arm == 0,
            };
            Some(Value::Boolean(truthy))
        }

        // A conditional sink-gate, not a branch: `value` passes through only if `cond` is
        // true; otherwise this constructs NOTHING — genuine absence, not a null/false value.
        // This is what lets "if X then perform an effect, else do nothing" exist at all: a
        // sink like `store_write` only ever constructs its Effect from a real `value` input
        // (`sink_effect`'s own `inputs.first().map(...)` — no input, no effect), and
        // `perform_effects` only ever performs what's in the FINAL settled output — so a
        // `guard`-suppressed value never reaches either. No new mechanism: absence already
        // propagates through wiring and through sink construction everywhere else in this
        // system; `guard` is just the first primitive whose whole job is to CREATE that
        // absence on purpose, from a condition, rather than only ever inheriting it from an
        // unsupplied capability.
        "guard" => {
            let [c, v] = inputs else { return None };
            match c {
                Value::Boolean(true) => Some((*v).clone()),
                Value::Boolean(false) => None,
                _ => None, // wrong type on cond ⇒ fail closed, same as everywhere else
            }
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
        "list" => Some(Value::List(inputs.iter().map(|v| (*v).clone()).collect())),
        "get" => {
            let [l, i] = inputs else { return None };
            as_list(l)?.get(as_index(i)?).cloned()
        }
        "append" => {
            let [l, item] = inputs else { return None };
            let mut xs = as_list(l)?.to_vec();
            xs.push((*item).clone());
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
            xs.push((*x).clone());
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

        // ── product (P1.3.2 inch 2): pair is the binary Product constructor. A product VALUE rides
        // the list value (eval is untyped) — pair(a,b) builds the same Value::List a 2-list does;
        // only the declared TYPE differs (Product([A,B]) vs List<T>), and the recursive checker keeps
        // them apart. So Dictionary<K,V> is List<Pair<K,V>>, expressible with no new value kind. ──
        "pair" => {
            let [a, b] = inputs else { return None };
            Some(Value::List(vec![(*a).clone(), (*b).clone()]))
        }

        // ── sum (P1.3.2 inch 3): Option<T> = Sum([T, Unit]), arm 0 = some, arm 1 = none. Unlike
        // pair, this DOES need a new Value shape (Value::Sum) — a sum value carries only its taken
        // arm, so it can't ride List the way a product can (a product's value always carries every
        // field). none's payload is Unit's value, which already rides the empty list. ──
        "some" => {
            let [v] = inputs else { return None };
            Some(Value::Sum { arm: 0, payload: Box::new((*v).clone()) })
        }
        "none" => {
            let [] = inputs else { return None };
            Some(Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) })
        }

        // ── boundary sink (Fork B): a sink purely *constructs its request* — representation
        // B: a self-describing `Effect { sink-name, payload }`. The capability is *derived*
        // from the name by the host (`capabilities_of`), never stored in the value. Because
        // each effect self-describes, an output can carry *many* (a `List` of them via `map`),
        // and heterogeneous effects ride out together. Drivers and state reads stay outside
        // the pure core (host-sourced / host-threaded).
        // The one egress sink (§13): `email`/`respond` merged into a single NetworkEgress request
        // constructor — the destination (mailbox, connection token) rides in the payload, not in a
        // distinct primitive. A reply (P2.5.1) is just an egress whose payload is a [token, reply]
        // pair, the token plain data alongside the body.
        "egress" => sink_effect("egress", inputs),
        // A state write is a Fork-B sink too: it constructs a `store_write` request; the host
        // performs it by *storing* (capability `StateWrite`). `store_read` has no pure compute
        // — it is sourced (host-supplied), like a driver. `ingress` is sourced too (no compute).
        //
        // Keyed now (see the keyed-state-capability L4): the settled `Value::Effect` this
        // constructs is the ONLY thing that survives evaluation — the leaf's own `params` are
        // gone by the time the host walks the output looking for effects to perform. So the key
        // has to travel *inside* the sink string itself, or it's lost entirely. Encoded as
        // `"store_write:<key>"`; `holon-host`'s `perform_effects` undoes this same encoding.
        // Fails closed exactly like `const` two arms up (`?` on a missing param): a `store_write`
        // with no key never even constructs an effect, rather than silently constructing an
        // ownerless one that would vanish at the perform step with no visible error.
        "store_write" => {
            let key = param(params, "key")?;
            sink_effect(&format!("store_write:{key}"), inputs)
        }

        _ => None, // unknown name, or a driver/state boundary unit handled by the host
    }
}

// ── sink request constructor (Fork B / representation B) ─────────────────────
/// Construct a sink's settled request: `Effect { sink-name, payload }`. The payload is the
/// first (and only) input it was given; no input ⇒ nothing to construct. The capability is
/// derived from the name downstream, never stored here.
fn sink_effect(sink: &str, inputs: &[&Value]) -> Option<Value> {
    inputs.first().map(|p| Value::Effect { sink: sink.into(), payload: Box::new((*p).clone()) })
}

/// The one recursive, total stringifier for an arbitrary `Value` — `to_text`'s own
/// implementation, factored out so it can call itself on a `List`/`Effect`/`Sum`'s inner
/// value(s). Kept in lockstep with the editor's own `formatValue` (`editor/src/diagnostics.ts`)
/// by design, not by accident — same shape, same reasoning, one convention instead of two that
/// could quietly drift apart. `f64::to_string()` is Rust's own shortest-round-trip Display
/// impl: deterministic and portable (I2), not something to hand-roll.
fn to_text_str(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Text(s) => format!("\"{s}\""),
        Value::List(xs) => format!("[{}]", xs.iter().map(to_text_str).collect::<Vec<_>>().join(", ")),
        Value::Effect { sink, payload } => format!("effect({sink}, {})", to_text_str(payload)),
        Value::Sum { arm, payload } => format!("sum#{arm}({})", to_text_str(payload)),
    }
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
/// The one literal parser `const` and `default` both call — `type`+`value` (as `const` has
/// always declared its own params) into a `Value`. Factored out here rather than left as
/// `const`'s own private logic: `default`'s frozen-fallback case needs to do exactly this,
/// and a second copy could silently drift from the first the way a couple of other things
/// already have this session (`WCapability`, `formatValue`) — one function, called twice,
/// closes that off structurally instead of by discipline.
fn parse_literal(ty: &str, value: &str) -> Option<Value> {
    match ty {
        "Number" => value
            .parse::<f64>()
            .ok()
            .filter(|n| crate::determinism::is_finite_number(*n))
            .map(Value::Number),
        "Boolean" => parse_bool(value).map(Value::Boolean),
        "Text" => Some(Value::Text(value.to_string())),
        _ => None,
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
fn fold_numbers(inputs: &[&Value], init: f64, f: fn(f64, f64) -> f64) -> Option<Value> {
    let mut acc = init;
    for v in inputs {
        acc = f(acc, as_number(v)?);
    }
    Some(Value::Number(acc))
}
fn fold_bools(inputs: &[&Value], init: bool, f: fn(bool, bool) -> bool) -> Option<Value> {
    let mut acc = init;
    for v in inputs {
        acc = f(acc, as_bool(v)?);
    }
    Some(Value::Boolean(acc))
}
fn bin_numbers(inputs: &[&Value]) -> Option<(f64, f64)> {
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
        assert_eq!(compute("add", &[&num(3.0), &num(7.0), &num(5.0)], &[]), Some(num(15.0)));
    }
    #[test]
    fn multiply_products() {
        assert_eq!(compute("multiply", &[&num(2.0), &num(3.0), &num(4.0)], &[]), Some(num(24.0)));
    }
    #[test]
    fn subtract_is_binary() {
        assert_eq!(compute("subtract", &[&num(10.0), &num(3.0)], &[]), Some(num(7.0)));
        assert_eq!(compute("subtract", &[&num(1.0)], &[]), None);
    }
    #[test]
    fn divide_guards_zero() {
        assert_eq!(compute("divide", &[&num(10.0), &num(2.0)], &[]), Some(num(5.0)));
        assert_eq!(compute("divide", &[&num(1.0), &num(0.0)], &[]), None);
    }
    #[test]
    fn compare_uses_op_param() {
        assert_eq!(compute("compare", &[&num(72.0), &num(70.0)], &[p("op", ">")]), Some(boo(true)));
        assert_eq!(compute("compare", &[&num(1.0), &num(2.0)], &[p("op", ">=")]), Some(boo(false)));
        assert_eq!(compute("compare", &[&num(5.0), &num(5.0)], &[p("op", "=")]), Some(boo(true)));
    }
    #[test]
    fn logic_ops() {
        assert_eq!(compute("and", &[&boo(true), &boo(true)], &[]), Some(boo(true)));
        assert_eq!(compute("and", &[&boo(true), &boo(false)], &[]), Some(boo(false)));
        assert_eq!(compute("or", &[&boo(false), &boo(true)], &[]), Some(boo(true)));
        assert_eq!(compute("not", &[&boo(true)], &[]), Some(boo(false)));
    }
    #[test]
    fn select_chooses() {
        assert_eq!(compute("select", &[&boo(true), &txt("yes"), &txt("no")], &[]), Some(txt("yes")));
        assert_eq!(compute("select", &[&boo(false), &txt("yes"), &txt("no")], &[]), Some(txt("no")));
    }
    #[test]
    fn text_ops() {
        assert_eq!(compute("concat", &[&txt("foo"), &txt("bar")], &[]), Some(txt("foobar")));
        assert_eq!(compute("uppercase", &[&txt("abc")], &[]), Some(txt("ABC")));
    }
    #[test]
    fn text_irreducibles() {
        assert_eq!(compute("lowercase", &[&txt("AbC")], &[]), Some(txt("abc")));
        // char-based (Unicode scalar values), not byte-based: "ab\u{00e7}" is 3 chars / 4 bytes.
        assert_eq!(compute("text_length", &[&txt("ab\u{00e7}")], &[]), Some(num(3.0)));
        assert_eq!(compute("slice", &[&txt("hello"), &num(1.0), &num(3.0)], &[]), Some(txt("ell")));
        // start past the end ⇒ empty (total, never None for valid types).
        assert_eq!(compute("slice", &[&txt("hi"), &num(5.0), &num(2.0)], &[]), Some(txt("")));
        assert_eq!(compute("index_of", &[&txt("hello"), &txt("ll")], &[]), Some(num(2.0)));
        assert_eq!(compute("index_of", &[&txt("hello"), &txt("z")], &[]), Some(num(-1.0)));
    }
    #[test]
    fn to_text_covers_every_value_shape() {
        // Number: Rust's own shortest-round-trip Display, not a hand-rolled formatter.
        assert_eq!(compute("to_text", &[&num(3.0)], &[]), Some(txt("3")));
        assert_eq!(compute("to_text", &[&num(-2.5)], &[]), Some(txt("-2.5")));
        // Boolean.
        assert_eq!(compute("to_text", &[&boo(true)], &[]), Some(txt("true")));
        // Text: QUOTED, not passed through bare — so a nested Text and a nested Number that
        // happen to look the same ("5" vs 5) stay distinguishable once nested in a List/Sum.
        assert_eq!(compute("to_text", &[&txt("hi")], &[]), Some(txt("\"hi\"")));
        // List — including the product case (a product's value already rides List; nothing
        // special-cased for it here, it's just a List like any other).
        let l = list(vec![num(1.0), txt("a")]);
        assert_eq!(compute("to_text", &[&l], &[]), Some(txt("[1, \"a\"]")));
        // Effect — the payload is shown, not just the sink name (the exact bug just fixed on
        // the editor's own formatValue; this locks in the same discipline here).
        let effect = Value::Effect { sink: "store_write:count".into(), payload: Box::new(num(3.0)) };
        assert_eq!(compute("to_text", &[&effect], &[]), Some(txt("effect(store_write:count, 3)")));
        // Sum — shown by arm index, never a guessed "Some"/"None" label.
        let some5 = Value::Sum { arm: 0, payload: Box::new(num(5.0)) };
        assert_eq!(compute("to_text", &[&some5], &[]), Some(txt("sum#0(5)")));
    }
    #[test]
    fn to_text_is_total_even_for_a_non_finite_number() {
        // const already refuses to construct one directly, but arithmetic can still produce a
        // non-finite intermediate value — to_text stays total rather than fail on it. Rust's
        // f64::Display already prints these as fixed, deterministic literals.
        assert_eq!(compute("to_text", &[&num(f64::NAN)], &[]), Some(txt("NaN")));
        assert_eq!(compute("to_text", &[&num(f64::INFINITY)], &[]), Some(txt("inf")));
    }
    #[test]
    fn is_truthy_covers_every_value_shape() {
        assert_eq!(compute("is_truthy", &[&num(0.0)], &[]), Some(boo(false)));
        assert_eq!(compute("is_truthy", &[&num(1.0)], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&num(-1.0)], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&boo(true)], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&boo(false)], &[]), Some(boo(false)));
        assert_eq!(compute("is_truthy", &[&txt("")], &[]), Some(boo(false)));
        assert_eq!(compute("is_truthy", &[&txt("hi")], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&list(vec![])], &[]), Some(boo(false)));
        assert_eq!(compute("is_truthy", &[&list(vec![num(1.0)])], &[]), Some(boo(true)));
        // An Effect can never structurally be empty — always true, not a special case.
        let effect = Value::Effect { sink: "egress".into(), payload: Box::new(txt("")) };
        assert_eq!(compute("is_truthy", &[&effect], &[]), Some(boo(true)));
        // Sum: arm 0 (some) is truthy, arm 1 (none) is falsy — this system's Option.
        let some = Value::Sum { arm: 0, payload: Box::new(num(0.0)) }; // even some(0) is truthy —
        let none = Value::Sum { arm: 1, payload: Box::new(list(vec![])) }; // presence, not payload, is what's checked
        assert_eq!(compute("is_truthy", &[&some], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&none], &[]), Some(boo(false)));
    }
    #[test]
    fn is_truthy_treats_nan_as_truthy_deliberately() {
        // NaN != 0.0 under IEEE 754 (NaN is != everything, including itself), so this rule's
        // own logic makes it true — a real, considered choice, not an overlooked edge case.
        assert_eq!(compute("is_truthy", &[&num(f64::NAN)], &[]), Some(boo(true)));
    }
    #[test]
    fn guard_passes_the_value_through_only_when_true() {
        assert_eq!(compute("guard", &[&boo(true), &txt("hi")], &[]), Some(txt("hi")));
        // false ⇒ constructs NOTHING, not a null/false value — the whole point.
        assert_eq!(compute("guard", &[&boo(false), &txt("hi")], &[]), None);
    }
    #[test]
    fn guard_fails_closed_on_a_non_boolean_cond() {
        assert_eq!(compute("guard", &[&num(1.0), &txt("hi")], &[]), None);
    }
    #[test]
    fn list_construct_and_query() {
        let l = compute("list", &[&num(3.0), &num(7.0), &num(5.0)], &[]).unwrap();
        assert_eq!(l, list(vec![num(3.0), num(7.0), num(5.0)]));
        assert_eq!(compute("length", &[&l], &[]), Some(num(3.0)));
        assert_eq!(compute("get", &[&l, &num(1.0)], &[]), Some(num(7.0)));
        assert_eq!(compute("get", &[&l, &num(9.0)], &[]), None);
        assert_eq!(compute("sum", &[&l], &[]), Some(num(15.0)));
        assert_eq!(
            compute("append", &[&l, &num(9.0)], &[]),
            Some(list(vec![num(3.0), num(7.0), num(5.0), num(9.0)]))
        );
    }
    #[test]
    fn join_strings() {
        let l = list(vec![txt("a"), txt("b"), txt("c")]);
        assert_eq!(compute("join", &[&l, &txt("-")], &[]), Some(txt("a-b-c")));
    }
    #[test]
    fn list_structure_primitives() {
        let l = list(vec![num(1.0), num(2.0), num(3.0)]);
        assert_eq!(
            compute("cons", &[&num(0.0), &l], &[]),
            Some(list(vec![num(0.0), num(1.0), num(2.0), num(3.0)]))
        );
        assert_eq!(compute("head", &[&l], &[]), Some(num(1.0)));
        assert_eq!(compute("tail", &[&l], &[]), Some(list(vec![num(2.0), num(3.0)])));
        assert_eq!(compute("is_empty", &[&l], &[]), Some(boo(false)));
        // Empty: head/tail are partial (None — the get-out-of-bounds convention); is_empty is total.
        let empty = list(vec![]);
        assert_eq!(compute("head", &[&empty], &[]), None);
        assert_eq!(compute("tail", &[&empty], &[]), None);
        assert_eq!(compute("is_empty", &[&empty], &[]), Some(boo(true)));
    }
    #[test]
    fn constants_from_params() {
        assert_eq!(compute("const", &[], &[p("type", "Number"), p("value", "42")]), Some(num(42.0)));
        assert_eq!(compute("const", &[], &[p("type", "Boolean"), p("value", "true")]), Some(boo(true)));
        assert_eq!(compute("const", &[], &[p("type", "Text"), p("value", "hi")]), Some(txt("hi")));
        // No inference: a value that doesn't parse for its declared type ⇒ None.
        assert_eq!(compute("const", &[], &[p("type", "Number"), p("value", "hi")]), None);
        // A type with no literal form ⇒ None.
        assert_eq!(compute("const", &[], &[p("type", "List"), p("value", "x")]), None);
    }
    #[test]
    fn default_passes_a_present_value_through_unchanged() {
        // The value's own type doesn't have to match the fallback's declared type at all —
        // this branch never looks at type/value; a present input just
        // passes straight through.
        let fallback = [p("type", "Text"), p("value", "stranger")];
        assert_eq!(compute("default", &[&txt("Bekir")], &fallback), Some(txt("Bekir")));
        assert_eq!(compute("default", &[&num(5.0)], &fallback), Some(num(5.0)));
    }
    #[test]
    fn default_falls_back_to_the_frozen_literal_when_absent() {
        // Absent input ⇒ exactly the same parse const itself would do with the SAME
        // params — proving default with nothing wired in front of it IS a const, not just
        // similar to one.
        let fallback = [p("type", "Text"), p("value", "stranger")];
        assert_eq!(compute("default", &[], &fallback), Some(txt("stranger")));
        assert_eq!(compute("default", &[], &fallback), compute("const", &[], &fallback));
    }
    #[test]
    fn default_fails_closed_on_more_than_one_input() {
        // default declares exactly one input port; this is defensive completeness for the
        // match, not a reachable case through normal wiring (validate() already enforces
        // arity against the catalog's own declared interface before eval ever runs).
        let fallback = [p("type", "Text"), p("value", "x")];
        assert_eq!(compute("default", &[&txt("a"), &txt("b")], &fallback), None);
    }
    #[test]
    fn equals_compares_same_kind_structurally() {
        assert_eq!(compute("equals", &[&txt("hi"), &txt("hi")], &[]), Some(boo(true)));
        assert_eq!(compute("equals", &[&txt("hi"), &txt("bye")], &[]), Some(boo(false)));
        assert_eq!(compute("equals", &[&num(5.0), &num(5.0)], &[]), Some(boo(true)));
        assert_eq!(compute("equals", &[&boo(true), &boo(true)], &[]), Some(boo(true)));
        assert_eq!(
            compute("equals", &[&list(vec![num(1.0)]), &list(vec![num(1.0)])], &[]),
            Some(boo(true)),
        );
    }
    #[test]
    fn equals_never_coerces_across_kinds() {
        // The actual regression this exists to prevent: a naive to-text-then-compare
        // implementation would see Text("true") and Boolean(true) both stringify to "true"
        // and wrongly call them equal. Rust's own derived equality matches both sides to the
        // SAME variant first — different variants are never equal, regardless of content.
        assert_eq!(compute("equals", &[&txt("true"), &boo(true)], &[]), Some(boo(false)));
        assert_eq!(compute("equals", &[&txt("5"), &num(5.0)], &[]), Some(boo(false)));
        assert_eq!(compute("equals", &[&txt("0"), &num(0.0)], &[]), Some(boo(false)));
    }
    #[test]
    fn equals_sum_arm_is_part_of_identity_here_too() {
        // some(5) and none carrying the same payload must not be equal — the same property
        // value.rs's own sum_arm_is_part_of_identity test already locks in at the Value level;
        // this confirms it holds through the primitive too, not just the type.
        let some5 = Value::Sum { arm: 0, payload: Box::new(num(5.0)) };
        let none_with_5 = Value::Sum { arm: 1, payload: Box::new(num(5.0)) };
        assert_eq!(compute("equals", &[&some5, &none_with_5], &[]), Some(boo(false)));
        assert_eq!(compute("equals", &[&some5, &some5.clone()], &[]), Some(boo(true)));
    }
    #[test]
    fn equals_treats_nan_as_unequal_to_itself_deliberately() {
        // f64's own IEEE-754 equality, undisguised — the same considered choice is_truthy
        // already made (NaN != 0.0 there; NaN != NaN here), not an overlooked edge case.
        assert_eq!(compute("equals", &[&num(f64::NAN), &num(f64::NAN)], &[]), Some(boo(false)));
    }
    #[test]
    fn egress_constructs_an_effect() {
        // Representation B: a sink constructs a self-describing request, not an identity.
        // *Which* effect it is lives in the name; the host derives the capability from it.
        assert_eq!(
            compute("egress", &[&txt("hello")], &[]),
            Some(Value::Effect { sink: "egress".into(), payload: Box::new(txt("hello")) })
        );
        assert_eq!(compute("egress", &[], &[]), None); // no payload ⇒ nothing to construct
    }
    #[test]
    fn store_write_constructs_an_effect() {
        // A state write is a Fork-B sink too: it constructs a `store_write` request — now with
        // its key encoded into the sink string, since that's the only thing that survives into
        // the settled value for `holon-host`'s `perform_effects` to read back.
        assert_eq!(
            compute("store_write", &[&num(7.0)], &[p("key", "var1")]),
            Some(Value::Effect { sink: "store_write:var1".into(), payload: Box::new(num(7.0)) })
        );
        assert_eq!(compute("store_read", &[], &[]), None); // read is sourced, not computed
    }

    #[test]
    fn store_write_fails_closed_without_a_key() {
        // Real, deliberate: no key means no effect is constructed at all, rather than silently
        // building one that would vanish, unowned, at the perform step. Same discipline `const`
        // already applies two arms up for a missing/unparseable literal.
        assert_eq!(compute("store_write", &[&num(7.0)], &[]), None);
    }
    #[test]
    fn egress_carries_a_correlation_token() {
        // P2.5.1: a reply is just egress; its payload is a [token, reply] pair, the token plain
        // data alongside the body — the merged `email`/`respond` sink handles both shapes.
        let payload = list(vec![num(1.0), txt("ok")]);
        assert_eq!(
            compute("egress", &[&payload], &[]),
            Some(Value::Effect { sink: "egress".into(), payload: Box::new(payload) })
        );
        assert_eq!(compute("ingress", &[], &[]), None); // ingress is sourced, not computed
    }
    #[test]
    fn pair_builds_a_pair() {
        // A product value rides the list value: pair(a,b) is a 2-list. Wrong arity ⇒ None.
        assert_eq!(compute("pair", &[&num(1.0), &txt("x")], &[]), Some(list(vec![num(1.0), txt("x")])));
        assert_eq!(compute("pair", &[&num(1.0)], &[]), None);
    }

    #[test]
    fn some_and_none_tag_their_arm() {
        let some_5 = compute("some", &[&num(5.0)], &[]).unwrap();
        let none = compute("none", &[], &[]).unwrap();
        assert_eq!(some_5, Value::Sum { arm: 0, payload: Box::new(num(5.0)) });
        assert_eq!(none, Value::Sum { arm: 1, payload: Box::new(list(vec![])) });
        assert_ne!(some_5, none); // distinct arms, never confusable
        // Wrong arity ⇒ None, same discipline as every other leaf.
        assert_eq!(compute("some", &[], &[]), None);
        assert_eq!(compute("none", &[&num(1.0)], &[]), None);
    }

    #[test]
    fn unknown_is_none() {
        assert_eq!(compute("frobnicate", &[], &[]), None);
    }
    #[test]
    fn runtime_type_mismatch_is_none() {
        // An `any` port could feed a wrong runtime type; compute guards it.
        assert_eq!(compute("add", &[&num(1.0), &txt("x")], &[]), None);
        assert_eq!(compute("not", &[&num(1.0)], &[]), None);
        assert_eq!(compute("sum", &[&list(vec![num(1.0), txt("x")])], &[]), None);
    }
    #[test]
    fn deterministic() {
        let owned = [num(3.0), num(7.0), num(5.0)];
        let inputs: Vec<&Value> = owned.iter().collect();
        assert_eq!(compute("add", &inputs, &[]), compute("add", &inputs, &[]));
    }
}