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

use std::collections::BTreeMap;

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

        // A value as it reads in a message: a text is itself (NOT quoted, unlike `to_text`, which
        // writes values as literals so a nested "5" and 5 stay apart), a record or list is its
        // JSON, and anything else is as `to_text` writes it. The one conversion that is the
        // identity on text, which is what lets `"you have {n} likes"` take any value: `concat`
        // refuses a number, `to_text` would quote a text, and no composite can tell the two
        // apart (there is no type test). Total, like `to_text`: JSON has no form for a non-finite
        // number or an effect, so those fall back to `to_text`'s rendering.
        "as_text" => {
            let [v] = inputs else { return None };
            Some(Value::Text(match v {
                Value::Text(s) => s.clone(),
                Value::Record(_) | Value::List(_) => crate::json::stringify(v).unwrap_or_else(|| to_text_str(v)),
                _ => to_text_str(v),
            }))
        }

        // The first of its inputs that arrived: `a` when present, else `b`. The one leaf that
        // fires with a wired input absent, which is what makes it inexpressible as a composite:
        // every other node produces nothing when a wired input is missing, so no wiring of them
        // can say "this, or else that". `default` is the same idea with a frozen literal for
        // `b`; this takes `b` from a wire. The evaluator gathers present inputs in port order
        // (eval.rs hands this leaf exactly that, deliberately), so the first present one IS the
        // first that arrived; with nothing present it produces nothing, like everything else.
        // Two inputs, not variadic: a variadic leaf fails closed on an absent input, the
        // opposite of this one, and `a ?? b ?? c` is two nodes.
        "coalesce" => inputs.first().map(|v| (*v).clone()),

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
                Value::Record(fields) => !fields.is_empty(),
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
        // The pieces of `text` between each occurrence of `sep`, in order: `join`'s inverse, so
        // `join(split(t, s), s) == t`. Text without `sep` is one piece (the empty text too);
        // an empty `sep` has no pieces to cut, and is absent.
        "split" => {
            let [Value::Text(text), Value::Text(sep)] = inputs else { return None };
            if sep.is_empty() {
                return None;
            }
            Some(Value::List(text.split(sep.as_str()).map(|s| Value::Text(s.to_string())).collect()))
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
        // Both lists' items, in order: the first list's, then the second's. Each input must be a
        // list; a single item is `append`'s job, never wrapped here.
        "extend" => {
            let [l, items] = inputs else { return None };
            let mut xs = as_list(l)?.to_vec();
            xs.extend_from_slice(as_list(items)?);
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
        // ── record kit (M1): build a record field by field, read one field, list the keys. A
        // field name is a frozen `name` param, so it is part of the unit's identity, like a
        // state key. A missing field reads as absent, not as a null value. ──
        "record" => {
            let [] = inputs else { return None };
            Some(Value::Record(BTreeMap::new()))
        }
        "field" => {
            let [Value::Record(fields)] = inputs else { return None };
            fields.get(param(params, "name")?).cloned()
        }
        "with_field" => {
            let [Value::Record(fields), v] = inputs else { return None };
            let mut fields = fields.clone();
            fields.insert(param(params, "name")?.to_string(), (*v).clone());
            Some(Value::Record(fields))
        }
        "without_field" => {
            let [Value::Record(fields)] = inputs else { return None };
            let mut fields = fields.clone();
            fields.remove(param(params, "name")?);
            Some(Value::Record(fields))
        }
        "keys" => {
            let [Value::Record(fields)] = inputs else { return None };
            Some(Value::List(fields.keys().map(|k| Value::Text(k.clone())).collect()))
        }
        // The keyed kit: the same reads and writes, with the key a Text input, so a record can be
        // a collection keyed by something a request brings (`/todos/:title`). Any other key is
        // refused, never coerced to text first.
        "lookup" => {
            let [Value::Record(fields), Value::Text(key)] = inputs else { return None };
            fields.get(key).cloned()
        }
        "insert" => {
            let [Value::Record(fields), Value::Text(key), v] = inputs else { return None };
            let mut fields = fields.clone();
            fields.insert(key.clone(), (*v).clone());
            Some(Value::Record(fields))
        }
        "remove" => {
            let [Value::Record(fields), Value::Text(key)] = inputs else { return None };
            let mut fields = fields.clone();
            fields.remove(key);
            Some(Value::Record(fields))
        }
        // Search a list of records: the first whose `name` field equals `value` (structural
        // equality, as `equals`: Text "1" is not Number 1). Items that aren't records, or lack
        // the field, never match; no match is absent.
        "find" => {
            let [Value::List(items), v] = inputs else { return None };
            let name = param(params, "name")?;
            items.iter().find(|item| matches!(item, Value::Record(fields) if fields.get(name) == Some(*v))).cloned()
        }

        // ── JSON (M1): request and response bodies. See json.rs for the mapping; malformed input,
        // or a value with no JSON form, is absent. ──
        "json_parse" => {
            let [Value::Text(s)] = inputs else { return None };
            crate::json::parse(s)
        }
        "json_stringify" => {
            let [v] = inputs else { return None };
            crate::json::stringify(v).map(Value::Text)
        }

        // ── signatures and encodings (crypto.rs): over a text's UTF-8 bytes. An unknown
        // encoding, mode or alphabet is absent, and so is a decode that isn't valid input or
        // doesn't come back to UTF-8. ──
        "sha256" => {
            let [Value::Text(s)] = inputs else { return None };
            crate::crypto::sha256(s, param(params, "encoding")?).map(Value::Text)
        }
        "hmac_sha256" => {
            let [Value::Text(key), Value::Text(message)] = inputs else { return None };
            crate::crypto::hmac_sha256(key, message, param(params, "encoding")?).map(Value::Text)
        }
        "hex" => {
            let [Value::Text(s)] = inputs else { return None };
            match param(params, "mode")? {
                "encode" => Some(Value::Text(crate::crypto::hex_encode(s.as_bytes()))),
                "decode" => String::from_utf8(crate::crypto::hex_decode(s)?).ok().map(Value::Text),
                _ => None,
            }
        }
        // `standard` encodes padded, `url` unpadded; either decodes with or without padding.
        "base64" => {
            let [Value::Text(s)] = inputs else { return None };
            let name = param(params, "alphabet")?;
            let alphabet = crate::crypto::alphabet(name)?;
            match param(params, "mode")? {
                "encode" => Some(Value::Text(crate::crypto::base64_encode(s.as_bytes(), alphabet, name == "standard"))),
                "decode" => String::from_utf8(crate::crypto::base64_decode(s, alphabet)?).ok().map(Value::Text),
                _ => None,
            }
        }
        // Text equality in constant time: what a signature is checked with, so how long the
        // comparison takes says nothing about how much of a guess was right.
        "secure_equals" => {
            let [Value::Text(a), Value::Text(b)] = inputs else { return None };
            Some(Value::Boolean(crate::crypto::secure_equals(a, b)))
        }

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

        // A collection's writes: `{key, value}` to put, the key to delete, each an effect the
        // host commits when the run finishes. The collection rides in the sink name, as a state
        // write's key does. A value holding an effect is not data, and constructs nothing.
        "collection_put" => {
            let name = crate::kind::ident(params, "collection")?;
            let [Value::Text(key), v] = inputs else { return None };
            if contains_effect(v) {
                return None;
            }
            let item = Value::Record(BTreeMap::from([("key".to_string(), Value::Text(key.clone())), ("value".to_string(), (*v).clone())]));
            Some(Value::Effect { sink: format!("collection_put:{name}"), payload: Box::new(item) })
        }
        "collection_delete" => {
            let name = crate::kind::ident(params, "collection")?;
            let [Value::Text(key)] = inputs else { return None };
            Some(Value::Effect { sink: format!("collection_delete:{name}"), payload: Box::new(Value::Text(key.clone())) })
        }

        _ => None, // unknown name, or a driver/state boundary unit handled by the host
    }
}

// ── sink request constructor (Fork B / representation B) ─────────────────────
/// Construct a sink's settled request: `Effect { sink-name, payload }`. The payload is the
/// first (and only) input it was given; no input ⇒ nothing to construct. The capability is
/// derived from the name downstream, never stored here.
///
/// A payload holding an Effect anywhere (`egress(egress(x))`, or one inside a list or record)
/// constructs nothing: a sink's payload is data, and an effect riding inside another one would
/// be performed, or not, by whatever reads that data, outside the host's view. Validation
/// refuses the wiring when the Effect is declared ([`is_sink`]); this catches one that arrives
/// through an `Any`-typed port.
fn sink_effect(sink: &str, inputs: &[&Value]) -> Option<Value> {
    let p = inputs.first()?;
    if contains_effect(p) {
        return None;
    }
    Some(Value::Effect { sink: sink.into(), payload: Box::new((*p).clone()) })
}

/// The primitives that construct an Effect from a payload input (see [`sink_effect`]).
pub(crate) fn is_sink(name: &str) -> bool {
    matches!(name, "egress" | "store_write" | "http_request" | "collection_put")
}

/// The most items one `collection_list` returns.
pub const MAX_LIST_LIMIT: u64 = 1000;

/// What a `collection_get` asks the host: `{collection: <name>, get: <key>}`.
pub(crate) fn collection_get_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let name = crate::kind::ident(params, "collection")?;
    let [Some(Value::Text(key))] = inputs else { return None };
    Some(Value::Record(BTreeMap::from([
        ("collection".to_string(), Value::Text(name)),
        ("get".to_string(), Value::Text(key.clone())),
    ])))
}

/// What a `collection_list` asks the host: `{collection: <name>, list: {prefix, after?, limit}}`,
/// for up to `limit` items (1 to [`MAX_LIST_LIMIT`]) in key order whose keys start with `prefix`
/// (none: every key) and come after `after` (none: from the first). Both inputs are optional.
pub(crate) fn collection_list_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let name = crate::kind::ident(params, "collection")?;
    let limit: u64 = param(params, "limit")?.parse().ok().filter(|n| (1..=MAX_LIST_LIMIT).contains(n))?;
    let [prefix, after] = inputs else { return None };
    let text = |v: &Option<Value>| match v {
        None => Some(None),
        Some(Value::Text(t)) => Some(Some(t.clone())),
        Some(_) => None,
    };
    let (prefix, after) = (text(prefix)?, text(after)?);
    let mut list = BTreeMap::from([
        ("prefix".to_string(), Value::Text(prefix.unwrap_or_default())),
        ("limit".to_string(), Value::Number(limit as f64)),
    ]);
    if let Some(after) = after {
        list.insert("after".to_string(), Value::Text(after));
    }
    Some(Value::Record(BTreeMap::from([
        ("collection".to_string(), Value::Text(name)),
        ("list".to_string(), Value::Record(list)),
    ])))
}

/// The longest a run may wait at one point: 90 days, in seconds.
pub const MAX_AWAIT_SECONDS: u64 = 90 * 24 * 3600;

/// What an `await` node asks the host: `{await: <name>, expires: <seconds>}`. `None` for a bad
/// name or an `expires` that isn't a whole number of seconds from 1 to [`MAX_AWAIT_SECONDS`].
pub(crate) fn await_request(params: &[Param]) -> Option<Value> {
    let name = crate::kind::ident(params, "name")?;
    let expires: u64 = param(params, "expires")?.parse().ok().filter(|s| (1..=MAX_AWAIT_SECONDS).contains(s))?;
    Some(Value::Record(BTreeMap::from([
        ("await".to_string(), Value::Text(name)),
        ("expires".to_string(), Value::Number(expires as f64)),
    ])))
}

/// What a `resume_token` node asks the host: `{resume_token: <await name>}`.
pub(crate) fn resume_token_request(params: &[Param]) -> Option<Value> {
    let name = crate::kind::ident(params, "await")?;
    Some(Value::Record(BTreeMap::from([("resume_token".to_string(), Value::Text(name))])))
}

/// Whether leaf `name` has optional input ports, read as "none" when left unwired: only
/// `http_request` (headers, body). Inside a composite, such a port that *is* wired holds the
/// node back while its value is absent (eval.rs), like any other leaf's input.
pub(crate) fn has_optional_ports(name: &str) -> bool {
    matches!(name, "http_request" | "collection_list")
}

/// The methods `http_request` sends.
const HTTP_METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// The request an `http_request` node makes, from its inputs (`path`, `headers`, `body`, each
/// possibly absent) and its frozen `method` and `host`: `{method, url, headers, body}`, the URL
/// always `https://<host><path>`. `None` when there is nothing valid to send: no path, a path
/// not starting with `/`, a bad method or host, headers that aren't a record of texts, a body
/// with no text or JSON form, or an Effect anywhere in it. Absent headers are `{}`; an absent
/// body is empty; a text body goes as is, anything else as JSON.
pub(crate) fn http_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let [Some(Value::Text(path)), headers, body] = inputs else { return None };
    let method = param(params, "method")?;
    let host = crate::kind::http_host(params)?;
    if !path.starts_with('/') || path.chars().any(|c| c.is_whitespace() || c.is_control()) || !HTTP_METHODS.contains(&method) {
        return None;
    }
    let headers = match headers {
        None => BTreeMap::new(),
        Some(Value::Record(h)) if h.values().all(|v| matches!(v, Value::Text(_))) => h.clone(),
        Some(_) => return None,
    };
    let body = match body {
        None => String::new(),
        Some(v) if contains_effect(v) => return None,
        Some(Value::Text(t)) => t.clone(),
        Some(v) => crate::json::stringify(v)?,
    };
    Some(Value::Record(BTreeMap::from([
        ("method".to_string(), Value::Text(method.to_string())),
        ("url".to_string(), Value::Text(format!("https://{host}{path}"))),
        ("headers".to_string(), Value::Record(headers)),
        ("body".to_string(), Value::Text(body)),
    ])))
}

/// Whether an Effect occurs anywhere in `v`.
fn contains_effect(v: &Value) -> bool {
    match v {
        Value::Effect { .. } => true,
        Value::List(items) => items.iter().any(contains_effect),
        Value::Record(fields) => fields.values().any(contains_effect),
        Value::Sum { payload, .. } => contains_effect(payload),
        Value::Number(_) | Value::Boolean(_) | Value::Text(_) => false,
    }
}

/// The one recursive, total stringifier for an arbitrary `Value` — `to_text`'s own
/// implementation, factored out so it can call itself on a `List`/`Effect`/`Sum`'s inner
/// value(s). Kept in lockstep with the editor's own `formatValue` (`editor/src/diagnostics.ts`)
/// by design, not by accident — same shape, same reasoning, one convention instead of two that
/// could quietly drift apart. `f64::to_string()` is Rust's own shortest-round-trip Display
/// impl: deterministic and portable (I2), not something to hand-roll.
fn to_text_str(v: &Value) -> String {
    match v {
        // `-0` prints as `0`: the two are equal, and the canonical value encoding folds them.
        Value::Number(n) if *n == 0.0 => "0".to_string(),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Text(s) => format!("\"{s}\""),
        Value::List(xs) => format!("[{}]", xs.iter().map(to_text_str).collect::<Vec<_>>().join(", ")),
        Value::Effect { sink, payload } => format!("effect({sink}, {})", to_text_str(payload)),
        Value::Sum { arm, payload } => format!("sum#{arm}({})", to_text_str(payload)),
        Value::Record(fields) => format!(
            "{{{}}}",
            fields.iter().map(|(k, v)| format!("{k}: {}", to_text_str(v))).collect::<Vec<_>>().join(", ")
        ),
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
        // Any structured literal: `[]` for an empty list, `{"done": false}`, … (M1: a state
        // read that falls back to an empty list, via `default`).
        "Json" => crate::json::parse(value),
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
    fn json_literals_for_const_and_default() {
        let j = |v: &str| [p("type", "Json"), p("value", v)];
        assert_eq!(compute("const", &[], &j("[]")), Some(Value::List(vec![])));
        assert_eq!(compute("default", &[], &j("[1]")), Some(Value::List(vec![num(1.0)])));
        assert_eq!(compute("default", &[&txt("x")], &j("[1]")), Some(txt("x")), "a present input wins");
        assert_eq!(compute("const", &[], &j("[1,")), None);
    }

    #[test]
    fn record_kit_builds_reads_and_lists_fields() {
        let empty = compute("record", &[], &[]).unwrap();
        assert_eq!(empty, Value::Record(BTreeMap::new()));
        // Fields added out of order still come out in key order.
        let r = compute("with_field", &[&empty, &txt("buy milk")], &[p("name", "title")]).unwrap();
        let r = compute("with_field", &[&r, &num(7.0)], &[p("name", "id")]).unwrap();
        assert_eq!(compute("field", &[&r], &[p("name", "id")]), Some(num(7.0)));
        assert_eq!(compute("field", &[&r], &[p("name", "missing")]), None, "a missing field is absent");
        assert_eq!(compute("keys", &[&r], &[]), Some(Value::List(vec![txt("id"), txt("title")])));
        assert_eq!(compute("to_text", &[&r], &[]), Some(txt("{id: 7, title: \"buy milk\"}")));
        // Overwrite, remove, and refuse a non-record input.
        let r2 = compute("with_field", &[&r, &num(8.0)], &[p("name", "id")]).unwrap();
        assert_eq!(compute("field", &[&r2], &[p("name", "id")]), Some(num(8.0)));
        let r3 = compute("without_field", &[&r2], &[p("name", "id")]).unwrap();
        assert_eq!(compute("keys", &[&r3], &[]), Some(Value::List(vec![txt("title")])));
        assert_eq!(compute("field", &[&num(1.0)], &[p("name", "id")]), None);
        assert_eq!(compute("is_truthy", &[&empty], &[]), Some(boo(false)));
        assert_eq!(compute("equals", &[&r, &r2], &[]), Some(boo(false)));
    }

    #[test]
    fn keyed_record_kit_takes_its_key_from_an_input() {
        let empty = compute("record", &[], &[]).unwrap();
        let r = compute("insert", &[&empty, &txt("buy milk"), &num(1.0)], &[]).unwrap();
        let r = compute("insert", &[&r, &txt("a b/c"), &num(2.0)], &[]).unwrap();
        assert_eq!(compute("lookup", &[&r, &txt("buy milk")], &[]), Some(num(1.0)));
        assert_eq!(compute("lookup", &[&r, &txt("a b/c")], &[]), Some(num(2.0)), "any text is a key");
        assert_eq!(compute("lookup", &[&r, &txt("nope")], &[]), None, "a missing key is absent");
        // Insert overwrites; remove drops the key and leaves a missing one alone.
        let r = compute("insert", &[&r, &txt("buy milk"), &num(3.0)], &[]).unwrap();
        assert_eq!(compute("lookup", &[&r, &txt("buy milk")], &[]), Some(num(3.0)));
        let r = compute("remove", &[&r, &txt("buy milk")], &[]).unwrap();
        assert_eq!(compute("keys", &[&r], &[]), Some(Value::List(vec![txt("a b/c")])));
        assert_eq!(compute("remove", &[&r, &txt("nope")], &[]), Some(r.clone()));
        // A key that isn't text, or a collection that isn't a record, is refused.
        assert_eq!(compute("lookup", &[&r, &num(1.0)], &[]), None);
        assert_eq!(compute("insert", &[&r, &num(1.0), &num(1.0)], &[]), None);
        assert_eq!(compute("remove", &[&num(1.0), &txt("x")], &[]), None);
        assert_eq!(compute("lookup", &[&r], &[]), None);
    }

    #[test]
    fn collection_reads_ask_the_host_and_writes_construct_effects() {
        let items = [p("collection", "notes")];
        // Reads are requests for the host, by key, or a page in order.
        let get = collection_get_request(&[Some(txt("n1"))], &items).unwrap();
        assert_eq!(get, Value::Record(BTreeMap::from([("collection".into(), txt("notes")), ("get".into(), txt("n1"))])));
        assert_eq!(collection_get_request(&[None], &items), None);
        assert_eq!(collection_get_request(&[Some(num(1.0))], &items), None, "keys are text");
        let page = [p("collection", "notes"), p("limit", "20")];
        let list = collection_list_request(&[None, Some(txt("n1"))], &page).unwrap();
        let Value::Record(r) = &list else { panic!() };
        let Value::Record(l) = &r["list"] else { panic!() };
        assert_eq!((l["prefix"].clone(), l["after"].clone(), l["limit"].clone()), (txt(""), txt("n1"), num(20.0)));
        assert!(collection_list_request(&[None, None], &[p("collection", "notes"), p("limit", "0")]).is_none());
        assert!(collection_list_request(&[None, None], &[p("collection", "notes"), p("limit", "1001")]).is_none());
        assert!(collection_list_request(&[None, None], &[p("collection", "a b"), p("limit", "5")]).is_none(), "a name is an identifier");

        // Writes are effects, the collection in the sink name.
        let put = compute("collection_put", &[&txt("n1"), &txt("buy milk")], &items).unwrap();
        let item = Value::Record(BTreeMap::from([("key".into(), txt("n1")), ("value".into(), txt("buy milk"))]));
        assert_eq!(put, Value::Effect { sink: "collection_put:notes".into(), payload: Box::new(item) });
        assert_eq!(
            compute("collection_delete", &[&txt("n1")], &items),
            Some(Value::Effect { sink: "collection_delete:notes".into(), payload: Box::new(txt("n1")) })
        );
        let effect = Value::Effect { sink: "egress".into(), payload: Box::new(txt("x")) };
        assert_eq!(compute("collection_put", &[&txt("k"), &effect], &items), None, "an effect is not data");
        assert_eq!(compute("collection_put", &[&num(1.0), &txt("v")], &items), None);
    }

    #[test]
    fn split_is_joins_inverse() {
        let texts = |xs: &[&str]| Some(Value::List(xs.iter().map(|x| txt(x)).collect()));
        assert_eq!(compute("split", &[&txt("t=1,v1=ab"), &txt(",")], &[]), texts(&["t=1", "v1=ab"]));
        assert_eq!(compute("split", &[&txt("a,,b,"), &txt(",")], &[]), texts(&["a", "", "b", ""]), "empty pieces kept");
        assert_eq!(compute("split", &[&txt("a::b"), &txt("::")], &[]), texts(&["a", "b"]));
        assert_eq!(compute("split", &[&txt("abc"), &txt(",")], &[]), texts(&["abc"]));
        assert_eq!(compute("split", &[&txt(""), &txt(",")], &[]), texts(&[""]));
        assert_eq!(compute("split", &[&txt("abc"), &txt("")], &[]), None, "an empty separator");
        assert_eq!(compute("split", &[&txt("abc"), &num(1.0)], &[]), None);
        let pieces = compute("split", &[&txt("x, y,,z"), &txt(", ")], &[]).unwrap();
        assert_eq!(compute("join", &[&pieces, &txt(", ")], &[]), Some(txt("x, y,,z")));
    }

    #[test]
    fn signature_primitives_compute_through_their_params() {
        // A webhook check end to end: GitHub's example signature, recomputed and compared.
        let sig = compute("hmac_sha256", &[&txt("It's a Secret to Everybody"), &txt("Hello, World!")], &[p("encoding", "hex")]);
        let expected = txt("757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17");
        assert_eq!(sig.as_ref(), Some(&expected));
        assert_eq!(compute("secure_equals", &[sig.as_ref().unwrap(), &expected], &[]), Some(boo(true)));
        assert_eq!(compute("secure_equals", &[&txt("a"), &txt("b")], &[]), Some(boo(false)));
        assert_eq!(compute("secure_equals", &[&txt("a"), &num(1.0)], &[]), None, "text only");
        assert_eq!(
            compute("sha256", &[&txt("abc")], &[p("encoding", "base64")]),
            Some(txt("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="))
        );
        assert_eq!(compute("sha256", &[&txt("abc")], &[p("encoding", "binary")]), None);
        assert_eq!(compute("sha256", &[&num(1.0)], &[p("encoding", "hex")]), None);

        let b64 = |mode: &str, alphabet: &str| vec![p("mode", mode), p("alphabet", alphabet)];
        assert_eq!(compute("base64", &[&txt("foob")], &b64("encode", "standard")), Some(txt("Zm9vYg==")));
        assert_eq!(compute("base64", &[&txt("foob")], &b64("encode", "url")), Some(txt("Zm9vYg")));
        assert_eq!(compute("base64", &[&txt("w7xuw68=")], &b64("decode", "standard")), Some(txt("ünï")));
        assert_eq!(compute("base64", &[&txt("_w")], &b64("decode", "url")), None, "0xff alone isn't UTF-8");
        assert_eq!(compute("base64", &[&txt("Zm9v")], &b64("decode", "other")), None);
        assert_eq!(compute("base64", &[&txt("Zm9v")], &b64("both", "url")), None);

        assert_eq!(compute("hex", &[&txt("foobar")], &[p("mode", "encode")]), Some(txt("666f6f626172")));
        assert_eq!(compute("hex", &[&txt("666f6f626172")], &[p("mode", "decode")]), Some(txt("foobar")));
        assert_eq!(compute("hex", &[&txt("zz")], &[p("mode", "decode")]), None);
        assert_eq!(compute("hex", &[&txt("ff")], &[p("mode", "decode")]), None, "not UTF-8");
    }

    #[test]
    fn find_returns_the_first_record_whose_field_matches() {
        let rec = |pairs: &[(&str, Value)]| Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let milk = rec(&[("title", txt("buy milk"))]);
        let dog = rec(&[("title", txt("walk the dog")), ("done", boo(false))]);
        let dog2 = rec(&[("title", txt("walk the dog")), ("done", boo(true))]);
        let list = Value::List(vec![num(1.0), rec(&[("name", txt("x"))]), milk.clone(), dog.clone(), dog2]);
        let by_title = [p("name", "title")];
        assert_eq!(compute("find", &[&list, &txt("buy milk")], &by_title), Some(milk));
        assert_eq!(compute("find", &[&list, &txt("walk the dog")], &by_title), Some(dog), "the first match");
        assert_eq!(compute("find", &[&list, &txt("nope")], &by_title), None, "no match is absent");
        assert!(compute("find", &[&list, &boo(false)], &[p("name", "done")]).is_some());
        assert_eq!(compute("find", &[&list, &num(1.0)], &by_title), None, "no coercion, and non-records never match");
        assert_eq!(compute("find", &[&Value::List(vec![]), &txt("x")], &by_title), None);
        assert_eq!(compute("find", &[&txt("not a list"), &txt("x")], &by_title), None);
        assert_eq!(compute("find", &[&list], &by_title), None);
    }

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
        assert_eq!(compute("to_text", &[&num(-0.0)], &[]), Some(txt("0")), "-0 is 0, as the value hash has it");
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
    fn as_text_is_the_identity_on_text_and_renders_everything_else() {
        // The whole point: a text comes back bare, where to_text would quote it.
        assert_eq!(compute("as_text", &[&txt("hi")], &[]), Some(txt("hi")));
        assert_eq!(compute("as_text", &[&txt("")], &[]), Some(txt("")));
        // Numbers and booleans as to_text writes them.
        assert_eq!(compute("as_text", &[&num(3.0)], &[]), Some(txt("3")));
        assert_eq!(compute("as_text", &[&num(-0.0)], &[]), Some(txt("0")));
        assert_eq!(compute("as_text", &[&num(-2.5)], &[]), Some(txt("-2.5")));
        assert_eq!(compute("as_text", &[&boo(false)], &[]), Some(txt("false")));
        // Records and lists as JSON: what a message or a log line wants, texts inside quoted.
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("id".to_string(), num(7.0));
        fields.insert("title".to_string(), txt("buy milk"));
        assert_eq!(compute("as_text", &[&Value::Record(fields)], &[]), Some(txt(r#"{"id":7,"title":"buy milk"}"#)));
        assert_eq!(compute("as_text", &[&list(vec![num(1.0), txt("a")])], &[]), Some(txt(r#"[1,"a"]"#)));
        // Total: what JSON can't write falls back to to_text's rendering, never to nothing.
        assert_eq!(compute("as_text", &[&list(vec![num(f64::INFINITY)])], &[]), Some(txt("[inf]")));
        let effect = Value::Effect { sink: "store_write:count".into(), payload: Box::new(num(3.0)) };
        assert_eq!(compute("as_text", &[&effect], &[]), Some(txt("effect(store_write:count, 3)")));
        assert_eq!(compute("as_text", &[&num(f64::NAN)], &[]), Some(txt("NaN")));
        // Arity: exactly one input, like to_text.
        assert_eq!(compute("as_text", &[], &[]), None);
        assert_eq!(compute("as_text", &[&num(1.0), &num(2.0)], &[]), None);
    }
    #[test]
    fn coalesce_is_the_first_input_that_arrived() {
        // Both present: the first. Only one present (the evaluator drops the absent one, keeping
        // order): that one, whichever side it was on. None present: nothing.
        assert_eq!(compute("coalesce", &[&num(1.0), &num(2.0)], &[]), Some(num(1.0)));
        assert_eq!(compute("coalesce", &[&txt("b")], &[]), Some(txt("b")));
        assert_eq!(compute("coalesce", &[], &[]), None);
        // Any kind passes through untouched, an Effect included (as select passes one).
        let effect = Value::Effect { sink: "store_write:k".into(), payload: Box::new(num(3.0)) };
        assert_eq!(compute("coalesce", &[&effect, &num(0.0)], &[]), Some(effect));
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
    fn extend_adds_every_item_of_the_second_list() {
        let a = list(vec![num(1.0), num(2.0)]);
        let b = list(vec![txt("x"), list(vec![num(3.0)])]);
        assert_eq!(
            compute("extend", &[&a, &b], &[]),
            Some(list(vec![num(1.0), num(2.0), txt("x"), list(vec![num(3.0)])])),
            "one level: an inner list stays one item"
        );
        assert_eq!(compute("extend", &[&list(vec![]), &a], &[]), Some(a.clone()));
        assert_eq!(compute("extend", &[&a, &list(vec![])], &[]), Some(a.clone()));
        assert_eq!(compute("extend", &[&a, &num(3.0)], &[]), None, "an item is not a list");
        assert_eq!(compute("extend", &[&txt("ab"), &a], &[]), None);
        assert_eq!(compute("extend", &[&a], &[]), None);
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
    fn http_request_builds_its_request_from_params_and_inputs() {
        let params = [p("method", "POST"), p("host", "Hooks.Slack.com")];
        let headers = Value::Record([("content-type".to_string(), txt("application/json"))].into());
        let body = Value::Record([("text".to_string(), txt("hi"))].into());
        let req = http_request(&[Some(txt("/services/x?y=1")), Some(headers.clone()), Some(body)], &params).unwrap();
        let Value::Record(r) = req else { panic!() };
        assert_eq!(r["url"], txt("https://hooks.slack.com/services/x?y=1"), "always https, host lowercased");
        assert_eq!(r["method"], txt("POST"));
        assert_eq!(r["headers"], headers);
        assert_eq!(r["body"], txt("{\"text\":\"hi\"}"), "a non-text body goes as JSON");
        // Defaults: no headers is {}, no body is empty; a text body is sent as is.
        let get = [p("method", "GET"), p("host", "api.example.com:8443")];
        let Value::Record(r) = http_request(&[Some(txt("/")), None, Some(txt("raw"))], &get).unwrap() else { panic!() };
        assert_eq!((r["url"].clone(), r["headers"].clone(), r["body"].clone()), (txt("https://api.example.com:8443/"), Value::Record(BTreeMap::new()), txt("raw")));
        // Nothing valid to send: no request at all.
        for (inputs, params) in [
            (vec![None, None, None], get.to_vec()),
            (vec![Some(txt("no-slash")), None, None], get.to_vec()),
            (vec![Some(txt("/a b")), None, None], get.to_vec()),
            (vec![Some(txt("/")), Some(Value::Record([("n".to_string(), num(1.0))].into())), None], get.to_vec()),
            (vec![Some(txt("/")), None, Some(Value::List(vec![compute("egress", &[&txt("x")], &[]).unwrap()]))], get.to_vec()),
            (vec![Some(txt("/")), None, None], vec![p("method", "TRACE"), p("host", "a.com")]),
            (vec![Some(txt("/")), None, None], vec![p("method", "GET"), p("host", "https://a.com")]),
            (vec![Some(txt("/")), None, None], vec![p("method", "GET"), p("host", "a.com/x")]),
            (vec![Some(txt("/")), None, None], vec![p("method", "GET"), p("host", "user@a.com")]),
            (vec![Some(txt("/")), None, None], vec![p("method", "GET"), p("host", "a.com:0")]),
            (vec![Some(txt("/")), None, None], vec![p("method", "GET"), p("host", "")]),
        ] {
            assert_eq!(http_request(&inputs, &params), None, "{inputs:?} {params:?}");
        }
        // As a plain primitive (no host to answer), it computes nothing.
        assert_eq!(compute("http_request", &[&txt("/")], &get), None);
    }
    #[test]
    fn a_sink_refuses_a_payload_holding_an_effect() {
        let inner = compute("egress", &[&txt("hi")], &[]).unwrap();
        let in_list = list(vec![txt("a"), inner.clone()]);
        let in_record = Value::Record([("send".to_string(), inner.clone())].into());
        let in_sum = Value::Sum { arm: 0, payload: Box::new(inner.clone()) };
        for payload in [&inner, &in_list, &in_record, &in_sum] {
            assert_eq!(compute("egress", &[payload], &[]), None, "{payload:?}");
            assert_eq!(compute("store_write", &[payload], &[p("key", "k")]), None, "{payload:?}");
        }
        // Plain data, however nested, still constructs.
        let data = list(vec![txt("a"), Value::Record([("n".to_string(), num(1.0))].into())]);
        assert!(compute("egress", &[&data], &[]).is_some());
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