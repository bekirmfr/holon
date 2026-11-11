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

use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::value_hash::{nests_within, MAX_VALUE_DEPTH};
use crate::{dates, lists, records, texts};
use crate::{Param, Value};

/// Dispatch a primitive by name to its native computation. Evaluate primitive `name` on its present inputs: none for a name that isn't one, or a driver
/// or state boundary unit the host handles. [`compute_op`] with the name's [`Op`].
pub fn compute(name: &str, inputs: &[&Value], params: &[Param]) -> Option<Value> {
    compute_op(Op::of(name)?, name, inputs, params)
}

/// The primitives `compute` evaluates, each by its name and its [`Op`]: one list, which makes both
/// the enum and [`Op::of`], so a name can't be in one and not the other
/// (docs/L4s/resilience-audit-L4.md, D1). `compute_op`'s match on `Op` is exhaustive, so a variant
/// can't be listed without an arm either.
macro_rules! ops {
    ($($name:literal => $variant:ident),* $(,)?) => {
        /// A primitive `compute` evaluates, by its name: found once for a node (eval.rs's
        /// `NodeKind`) instead of matched by name at each evaluation
        /// (docs/L4s/interpreter-path-L4.md, step 5).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Op {
            $($variant),*
        }

        impl Op {
            /// Every primitive `compute` evaluates, with its name.
            #[cfg(test)]
            pub(crate) const ALL: &'static [(&'static str, Op)] = &[$(($name, Op::$variant)),*];

            pub(crate) fn of(name: &str) -> Option<Op> {
                Some(match name {
                    $($name => Op::$variant,)*
                    _ => return None,
                })
            }
        }
    };
}

ops! {
    "const" => Const,
    "default" => Default,
    "equals" => Equals,
    "add" => Add,
    "multiply" => Multiply,
    "subtract" => Subtract,
    "divide" => Divide,
    "compare" => Compare,
    "math" => Math,
    "max" => Max,
    "min" => Min,
    "aggregate" => Aggregate,
    "atan2" => Atan2,
    "power" => Power,
    "remainder" => Remainder,
    "bitwise" => Bitwise,
    "number_test" => NumberTest,
    "to_number" => ToNumber,
    "number_format" => NumberFormat,
    "and" => And,
    "or" => Or,
    "not" => Not,
    "select" => Select,
    "concat" => Concat,
    "uppercase" => Uppercase,
    "lowercase" => Lowercase,
    "text_length" => TextLength,
    "to_text" => ToText,
    "as_text" => AsText,
    "coalesce" => Coalesce,
    "fail" => Fail,
    "is_truthy" => IsTruthy,
    "guard" => Guard,
    "slice" => Slice,
    "split" => Split,
    "index_of" => IndexOf,
    "text_at" => TextAt,
    "code_at" => CodeAt,
    "from_codes" => FromCodes,
    "text_contains" => TextContains,
    "trim" => Trim,
    "pad" => Pad,
    "repeat" => Repeat,
    "replace" => Replace,
    "normalize" => Normalize,
    "uri_encode" => UriEncode,
    "uri_decode" => UriDecode,
    "is_well_formed" => IsWellFormed,
    "list" => List,
    "get" => Get,
    "append" => Append,
    "extend" => Extend,
    "length" => Length,
    "range" => Range,
    "sum" => Sum,
    "join" => Join,
    "cons" => Cons,
    "head" => Head,
    "tail" => Tail,
    "is_empty" => IsEmpty,
    "at" => At,
    "indices" => Indices,
    "entries" => Entries,
    "includes" => Includes,
    "last_index_of" => LastIndexOf,
    "reverse" => Reverse,
    "to_spliced" => ToSpliced,
    "with_item" => WithItem,
    "fill" => Fill,
    "copy_within" => CopyWithin,
    "sort" => Sort,
    "flat" => Flat,
    "is_list" => IsList,
    "unique" => Unique,
    "record" => Record,
    "field" => Field,
    "with_field" => WithField,
    "without_field" => WithoutField,
    "keys" => Keys,
    "lookup" => Lookup,
    "insert" => Insert,
    "remove" => Remove,
    "values" => Values,
    "from_entries" => FromEntries,
    "merge" => Merge,
    "has" => Has,
    "same_value" => SameValue,
    "type_of" => TypeOf,
    "date_part" => DatePart,
    "date_utc" => DateUtc,
    "date_set" => DateSet,
    "date_text" => DateText,
    "date_parse" => DateParse,
    "date_add" => DateAdd,
    "business_day_add" => BusinessDayAdd,
    "business_days_between" => BusinessDaysBetween,
    "is_business_day" => IsBusinessDay,
    "json_parse" => JsonParse,
    "json_stringify" => JsonStringify,
    "sha256" => Sha256,
    "hmac_sha256" => HmacSha256,
    "hex" => Hex,
    "base64" => Base64,
    "secure_equals" => SecureEquals,
    "text_to_bytes" => TextToBytes,
    "bytes_to_text" => BytesToText,
    "bytes_length" => BytesLength,
    "bytes_concat" => BytesConcat,
    "bytes_to_list" => BytesToList,
    "list_to_bytes" => ListToBytes,
    "decimal_add" => DecimalAdd,
    "decimal_sub" => DecimalSub,
    "decimal_mul" => DecimalMul,
    "decimal_div" => DecimalDiv,
    "decimal_round" => DecimalRound,
    "decimal_sum" => DecimalSum,
    "decimal_compare" => DecimalCompare,
    "csv_parse" => CsvParse,
    "csv_format" => CsvFormat,
    "xml_parse" => XmlParse,
    "xml_format" => XmlFormat,
    "xml_find" => XmlFind,
    "xml_text" => XmlText,
    "xml_attribute" => XmlAttribute,
    "gzip" => Gzip,
    "gunzip" => Gunzip,
    "zip" => Zip,
    "unzip" => Unzip,
    "jwt_verify" => JwtVerify,
    "jwt_decode" => JwtDecode,
    "pgp_encrypt" => PgpEncrypt,
    "pgp_verify" => PgpVerify,
    "pgp_key" => PgpKey,
    "pair" => Pair,
    "some" => Some,
    "none" => None,
    "egress" => Egress,
    "store_write" => StoreWrite,
    "collection_put" => CollectionPut,
    "collection_delete" => CollectionDelete,
    "file_write" => FileWrite,
    "file_delete" => FileDelete,
    "file_copy" => FileCopy,
    "file_create" => FileCreate,
    "file_create_copy" => FileCreateCopy,
    "db_exec" => DbExec,
    "db_insert" => DbInsert,
    "db_update" => DbUpdate,
    "db_delete" => DbDelete,
    "channel_send" => ChannelSend,
    "mail_send" => MailSend,
    "queue_send" => QueueSend,
    "log" => Log,
    "channel_subscribe" => ChannelSubscribe,
    "file_serve" => FileServe,
}

/// Where leaf `op` puts what it is handed a level further down than it was ([`fits`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Down {
    /// Nowhere: what it gives nests no deeper than what it was handed.
    No,
    /// Every input (`list`'s, `pair`'s).
    All,
    /// The present input at this place (`append`'s item, `some`'s value, every part of what
    /// `entries` is handed).
    Input(usize),
}

/// An effect's payload goes a level down too, checked as the effect is built ([`effect`],
/// [`sink_effect`]).
fn down(op: Op) -> Down {
    match op {
        Op::List | Op::Pair => Down::All,
        Op::Cons | Op::Some | Op::Entries => Down::Input(0),
        Op::Append | Op::WithField | Op::Fill => Down::Input(1),
        Op::Insert | Op::WithItem => Down::Input(2),
        _ => Down::No,
    }
}

/// Whether what leaf `op` builds of `inputs` (present, in port order) still nests within
/// [`MAX_VALUE_DEPTH`]: each input it puts a level down must nest one less. Every value a run
/// holds nests within it (a value read back is bounded by its decoding, a request's by `json`'s),
/// so only these leaves (and `map`'s results, `group_by`'s list and a reviver's answers, eval.rs)
/// could take one past it, and each refuses to: one more level a step, a fold wrapping its
/// accumulator, would otherwise nest without end. Only the input put down is looked at, so
/// `append` to a long list costs what its item does.
#[inline]
fn fits<'a>(op: Op, mut inputs: impl Iterator<Item = &'a Value>) -> bool {
    match down(op) {
        Down::No => true,
        Down::All => inputs.all(|v| nests_within(v, MAX_VALUE_DEPTH - 1)),
        Down::Input(i) => inputs.nth(i).is_none_or(|v| nests_within(v, MAX_VALUE_DEPTH - 1)),
    }
}

/// A file's content type, by its name's extension: the web's types, text ones in UTF-8, and
/// `application/octet-stream` for any other. What a route answering with a file sends unless its
/// flow sets one.
pub fn content_type(name: &str) -> &'static str {
    let last = name.rsplit('/').next().unwrap_or(name);
    let ext = last.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// The effect for `sink` of `payload`: nothing when the payload, a level down in it, nests past
/// [`MAX_VALUE_DEPTH`].
fn effect(sink: String, payload: Value) -> Option<Value> {
    nests_within(&payload, MAX_VALUE_DEPTH - 1).then(|| Value::Effect { sink, payload: Box::new(payload) })
}

/// [`compute`], the primitive's [`Op`] already found (`name` is its name, which a few read):
/// nothing where what it would build nests past [`MAX_VALUE_DEPTH`] ([`fits`]).
pub(crate) fn compute_op(op: Op, name: &str, inputs: &[&Value], params: &[Param]) -> Option<Value> {
    if !fits(op, inputs.iter().copied()) {
        return None;
    }
    match op {
        // ── const (§13): the one literal constructor. `value` is the literal as text; `type`
        // (Number/Boolean/Text) declares how to read it — no inference, so the unit is
        // self-describing and two authors who meant different types never collide (I5). A value
        // that doesn't parse for its type ⇒ None; List/Effect/Any have no literal form (an empty
        // list is the niladic `list`), ⇒ None. ──
        Op::Const => parse_literal(param(params, "type")?, param(params, "value")?),

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
        Op::Default => match inputs {
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
        Op::Equals => {
            let [a, b] = inputs else { return None };
            Some(Value::Boolean(a == b))
        }

        // ── arithmetic ── A sum starts from `-0`, the one number adding which changes nothing
        // (`-0 + -0` is `-0`; from `+0` it was `+0`). A result past the largest number is no
        // number Holon has (`const` refuses one, as `json_parse` does `1e999`): nothing. `add` is
        // JavaScript's `+`: with a text among its operands, it joins them (texts.rs).
        Op::Add => match fold_numbers(inputs, -0.0, |a, b| a + b) {
            Some(sum) => finite(sum),
            None => texts::plus(inputs),
        },
        Op::Multiply => fold_numbers(inputs, 1.0, |a, b| a * b).and_then(finite),
        Op::Subtract => bin_numbers(inputs).and_then(|(a, b)| finite(a - b)),
        Op::Divide => bin_numbers(inputs).and_then(|(a, b)| if b == 0.0 { None } else { finite(a / b) }),

        // ── comparison (op param): two numbers, or two texts in code-unit order ──
        Op::Compare => match inputs {
            [Value::Text(a), Value::Text(b)] => {
                let o = texts::order(a, b);
                Some(Value::Boolean(match param(params, "op")? {
                    "<" => o.is_lt(),
                    ">" => o.is_gt(),
                    "<=" => o.is_le(),
                    ">=" => o.is_ge(),
                    "=" => o.is_eq(),
                    "!=" => o.is_ne(),
                    _ => return None,
                }))
            }
            _ => {
                let (a, b) = bin_numbers(inputs)?;
                compare(a, b, param(params, "op")?).map(Value::Boolean)
            }
        },

        // ── numbers (numbers.rs): JavaScript's Math, Number and numeric operators, each family's
        // function a frozen param (catalog `FAMILIES`) ──
        Op::Math => {
            let [x] = inputs else { return None };
            crate::numbers::math(param(params, "fn")?, as_number(x)?)
        }
        Op::Max => crate::numbers::extreme(inputs.iter().copied(), true),
        Op::Min => crate::numbers::extreme(inputs.iter().copied(), false),
        Op::Aggregate => {
            let [xs] = inputs else { return None };
            crate::numbers::aggregate(param(params, "fn")?, as_list(xs)?)
        }
        // libm's, pinned: `atan2(±0, -0)` is ±π; `pow` gives nothing for `0 ** -1` and
        // `(-8) ** (1/3)`; fmod is exact, with a's sign, and nothing for b zero.
        Op::Atan2 => bin_numbers(inputs).and_then(|(y, x)| crate::numbers::number(libm::atan2(y, x))),
        Op::Power => bin_numbers(inputs).and_then(|(a, b)| crate::numbers::number(libm::pow(a, b))),
        Op::Remainder => bin_numbers(inputs).and_then(|(a, b)| if b == 0.0 { None } else { crate::numbers::number(libm::fmod(a, b)) }),
        Op::Bitwise => {
            let (a, b) = bin_numbers(inputs)?;
            crate::numbers::bitwise(param(params, "op")?, a, b)
        }
        Op::NumberTest => {
            let [x] = inputs else { return None };
            crate::numbers::number_test(param(params, "test")?, param(params, "coerce")?, x)
        }
        // A radix, and a number_format's digits, are optional inputs: absent, unwired.
        Op::ToNumber => match inputs {
            [x] => crate::numbers::to_number(param(params, "grammar")?, x, None),
            [x, radix] => crate::numbers::to_number(param(params, "grammar")?, x, Some(radix)),
            _ => None,
        },
        Op::NumberFormat => match inputs {
            [x] => crate::numbers::number_format(param(params, "style")?, as_number(x)?, None),
            [x, digits] => crate::numbers::number_format(param(params, "style")?, as_number(x)?, Some(digits)),
            _ => None,
        },

        // ── logic ──
        Op::And => fold_bools(inputs, true, |a, b| a && b),
        Op::Or => fold_bools(inputs, false, |a, b| a || b),
        Op::Not => {
            let [v] = inputs else { return None };
            as_bool(v).map(|x| Value::Boolean(!x))
        }

        // ── control-as-data ──
        Op::Select => {
            let [c, a, b] = inputs else { return None };
            Some(if as_bool(c)? { (*a).clone() } else { (*b).clone() })
        }

        // ── text ──
        Op::Concat => {
            // Measured first, so the text is made once at its length rather than grown.
            let mut len = 0;
            for v in inputs {
                len += as_text(v)?.len();
            }
            let mut s = String::with_capacity(len);
            for v in inputs {
                s.push_str(as_text(v)?);
            }
            Some(Value::Text(s))
        }
        Op::Uppercase => {
            let [v] = inputs else { return None };
            Some(Value::Text(as_text(v)?.to_uppercase()))
        }
        // ── text irreducibles (§13): `Text` stays opaque; these touch the encoding, so they
        // are native. Positions and lengths count UTF-16 code units throughout, as JavaScript
        // does (texts.rs). Casing uses Unicode **default** (locale-independent) mapping — Rust's
        // to_uppercase/to_lowercase — so it can never vary by locale and poison a hash (I2). ──
        Op::Lowercase => {
            let [v] = inputs else { return None };
            Some(Value::Text(as_text(v)?.to_lowercase()))
        }
        Op::TextLength => {
            let [v] = inputs else { return None };
            Some(Value::Number(texts::units(as_text(v)?) as f64))
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
        Op::ToText => {
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
        Op::AsText => {
            let [v] = inputs else { return None };
            Some(Value::Text(match v {
                Value::Text(s) => s.clone(),
                Value::Record(_) | Value::List(_) => crate::json::stringify(v).unwrap_or_else(|| to_text_str(v)),
                // As JavaScript writes a `Uint8Array`: its numbers joined with `,`.
                Value::Bytes(_) => crate::numbers::js_string(v),
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
        Op::Coalesce => inputs.first().map(|v| (*v).clone()),

        // Nothing, whatever its message: the language's `throw`. The evaluator hands the message
        // to the observer for the run's log (eval.rs's `Body::Fail`); computed here, as the eager
        // oracle does, it is only nothing.
        Op::Fail => None,

        // The other universal any-typed primitive alongside `to_text`, same reasoning: one
        // uniform check that works before you know a value's runtime type — reading
        // `store_read(key)` (declared `Any`), say, where you often genuinely don't. It is
        // JavaScript's ToBoolean, `Boolean(x)` ([`lists::truthy`]), as the language reads a
        // condition that isn't a Boolean (`if (s)`, `!s`): `false`, `0`, `NaN` and `""` are
        // false, and a `Sum`'s `none`, its absence, as `undefined` is; anything else is true,
        // an empty list or record included, and an Effect, which can't be empty.
        Op::IsTruthy => {
            let [v] = inputs else { return None };
            Some(Value::Boolean(lists::truthy(v)))
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
        Op::Guard => {
            let [c, v] = inputs else { return None };
            match c {
                Value::Boolean(true) => Some((*v).clone()),
                Value::Boolean(false) => None,
                _ => None, // wrong type on cond ⇒ fail closed, same as everywhere else
            }
        }

        // Part of a text or a list, by `mode`: `"length"` reads `end` as how many (both whole and
        // not negative, cut short at the end), `"end"` as a relative position, the length when
        // unwired (`xs.slice(s, e)`), and `"substring"` (a text's: `s.substring(a, b)`) as a
        // position clamped to the text, the two swapped when `end` is before `start`.
        Op::Slice => match inputs {
            [Value::List(xs), rest @ ..] => Some(Value::List(xs[list_slice(params, xs.len(), rest)?].to_vec())),
            // A `Uint8Array`'s `slice` and `subarray`, as a list's.
            [Value::Bytes(b), rest @ ..] => Some(Value::Bytes(b[list_slice(params, b.len(), rest)?].to_vec())),
            [Value::Text(s), start, end @ ..] if end.len() < 2 => texts::slice(param(params, "mode")?, s, start, end.first().copied()),
            _ => None,
        },
        // The pieces of `text` between each occurrence of `sep`, in order: `join`'s inverse, so
        // `join(split(t, s), s) == t`. Text without `sep` is one piece (the empty text too); an
        // empty `sep` cuts every code unit. `limit`, when wired, keeps that many pieces.
        Op::Split => match inputs {
            [Value::Text(text), Value::Text(sep), limit @ ..] if limit.len() < 2 => texts::split(text, sep, optional_number(limit.first().copied())?),
            _ => None,
        },
        // On a list, the first position at or after `from` whose item `equals` the needle; on a
        // text, the code-unit index of the first match at or after it. Either way -1 when there
        // is none (not-found is a normal answer, not a None-propagating failure).
        Op::IndexOf => match inputs {
            [Value::List(xs), x, from @ ..] if from.len() < 2 => Some(lists::index_of(xs, x, optional_number(from.first().copied())?)),
            [Value::Text(h), Value::Text(n), from @ ..] if from.len() < 2 => Some(texts::index_of(h, n, optional_number(from.first().copied())?)),
            _ => None,
        },

        // ── JavaScript's string methods (texts.rs), each family's choice a frozen param ──
        Op::TextAt => match inputs {
            [Value::Text(s), i] => texts::char_at(s, as_number(i)?),
            _ => None,
        },
        Op::CodeAt => match inputs {
            [Value::Text(s), i] => texts::code_at(param(params, "mode")?, s, as_number(i)?),
            _ => None,
        },
        Op::FromCodes => texts::from_codes(param(params, "mode")?, inputs),
        Op::TextContains => match inputs {
            [Value::Text(s), Value::Text(t), position @ ..] if position.len() < 2 => texts::contains(param(params, "at")?, s, t, optional_number(position.first().copied())?),
            _ => None,
        },
        Op::Trim => match inputs {
            [Value::Text(s)] => texts::trim(param(params, "side")?, s).map(|t| Value::Text(t.to_string())),
            _ => None,
        },
        // The fill is a space when unwired.
        Op::Pad => match inputs {
            [Value::Text(s), length] => texts::pad(param(params, "side")?, s, as_number(length)?, " "),
            [Value::Text(s), length, Value::Text(fill)] => texts::pad(param(params, "side")?, s, as_number(length)?, fill),
            _ => None,
        },
        Op::Repeat => match inputs {
            [Value::Text(s), count] => texts::repeat(s, as_number(count)?),
            _ => None,
        },
        Op::Replace => match inputs {
            [Value::Text(s), Value::Text(a), Value::Text(b)] => texts::replace(param(params, "count")?, s, a, b),
            _ => None,
        },
        Op::Normalize => match inputs {
            [Value::Text(s)] => texts::normalize(param(params, "form")?, s),
            _ => None,
        },
        Op::UriEncode => match inputs {
            [Value::Text(s)] => texts::uri_encode(param(params, "set")?, s),
            _ => None,
        },
        Op::UriDecode => match inputs {
            [Value::Text(s)] => texts::uri_decode(param(params, "set")?, s),
            _ => None,
        },
        // A Holon text can't hold a lone surrogate, so every text is well formed.
        Op::IsWellFormed => match inputs {
            [Value::Text(_)] => Some(Value::Boolean(true)),
            _ => None,
        },

        // ── list ──
        Op::List => Some(Value::List(inputs.iter().map(|v| (*v).clone()).collect())),
        Op::Get => {
            let [l, i] = inputs else { return None };
            as_list(l)?.get(as_index(i)?).cloned()
        }
        Op::Append => {
            let [l, item] = inputs else { return None };
            let l = as_list(l)?;
            let mut xs = Vec::with_capacity(l.len() + 1);
            xs.extend_from_slice(l);
            xs.push((*item).clone());
            Some(Value::List(xs))
        }
        // Both lists' items, in order: the first list's, then the second's. Each input must be a
        // list; a single item is `append`'s job, never wrapped here.
        Op::Extend => {
            let [l, items] = inputs else { return None };
            let (l, items) = (as_list(l)?, as_list(items)?);
            let mut xs = Vec::with_capacity(l.len() + items.len());
            xs.extend_from_slice(l);
            xs.extend_from_slice(items);
            Some(Value::List(xs))
        }
        Op::Length => {
            let [l] = inputs else { return None };
            Some(Value::Number(as_list(l)?.len() as f64))
        }
        // The one leaf that makes a list from a number, so it is where counting is bounded: a
        // `map` over it ends because it has at most MAX_RANGE items.
        Op::Range => {
            let [n] = inputs else { return None };
            let n = as_number(n)?;
            if n.fract() != 0.0 || !(0.0..=MAX_RANGE as f64).contains(&n) {
                return None;
            }
            Some(Value::List((0..n as u64).map(|k| Value::Number(k as f64)).collect()))
        }
        // As `add` sums: from `-0`, and nothing past the largest number.
        Op::Sum => {
            let [l] = inputs else { return None };
            let mut acc = -0.0;
            for v in as_list(l)? {
                acc += as_number(v)?;
            }
            finite(acc)
        }
        Op::Join => {
            let [l, sep] = inputs else { return None };
            let (items, sep) = (as_list(l)?, as_text(sep)?);
            // Measured first, then written once into one text, not each item copied to a piece
            // and the pieces copied again.
            // Saturating, as a capacity is only a hint: a separator long enough to overflow it on
            // a 32-bit target is no reason to panic before the items are found not to be texts.
            let mut len = sep.len().saturating_mul(items.len().saturating_sub(1));
            for v in items {
                len = len.saturating_add(as_text(v)?.len());
            }
            let mut s = String::with_capacity(len);
            for (k, v) in items.iter().enumerate() {
                if k > 0 {
                    s.push_str(sep);
                }
                s.push_str(as_text(v)?);
            }
            Some(Value::Text(s))
        }

        // ── list structure (§13 irreducibles): O(1) build / inspect. `fold`-composites
        // (sum, map, filter, …) are built from these. `head`/`tail` of `[]` ⇒ None — the
        // existing `get`-out-of-bounds convention (partial-via-None; it halts, just yields
        // no value, so totality is intact). `is_empty` is total. ──
        Op::Cons => {
            let [x, l] = inputs else { return None };
            let tail = as_list(l)?;
            let mut xs = Vec::with_capacity(tail.len() + 1);
            xs.push((*x).clone());
            xs.extend_from_slice(tail);
            Some(Value::List(xs))
        }
        Op::Head => {
            let [l] = inputs else { return None };
            as_list(l)?.first().cloned()
        }
        Op::Tail => {
            let [l] = inputs else { return None };
            as_list(l)?.split_first().map(|(_, rest)| Value::List(rest.to_vec()))
        }
        Op::IsEmpty => {
            let [l] = inputs else { return None };
            Some(Value::Boolean(as_list(l)?.is_empty()))
        }

        // ── product (P1.3.2 inch 2): pair is the binary Product constructor. A product VALUE rides
        // the list value (eval is untyped) — pair(a,b) builds the same Value::List a 2-list does;
        // only the declared TYPE differs (Product([A,B]) vs List<T>), and the recursive checker keeps
        // them apart. So Dictionary<K,V> is List<Pair<K,V>>, expressible with no new value kind. ──
        // ── lists (lists.rs): JavaScript's array methods. `at`, `includes` and `last_index_of`
        // take a text too (texts.rs), and the searches a position to search from. ──
        Op::At => match inputs {
            [Value::List(xs), i] => xs.get(lists::position(as_number(i)?, xs.len())?).cloned(),
            [Value::Text(s), i] => texts::at(s, as_number(i)?),
            _ => None,
        },
        Op::Indices => {
            let [Value::List(xs)] = inputs else { return None };
            Some(lists::indices(xs.len()))
        }
        Op::Entries => {
            let [x] = inputs else { return None };
            lists::entries(x)
        }
        Op::Includes => match inputs {
            [Value::List(xs), x, from @ ..] if from.len() < 2 => Some(lists::includes(xs, x, optional_number(from.first().copied())?)),
            [Value::Text(s), Value::Text(t), from @ ..] if from.len() < 2 => Some(texts::includes(s, t, optional_number(from.first().copied())?)),
            _ => None,
        },
        Op::LastIndexOf => match inputs {
            [Value::List(xs), x, from @ ..] if from.len() < 2 => Some(lists::last_index_of(xs, x, optional_number(from.first().copied())?)),
            [Value::Text(s), Value::Text(t), from @ ..] if from.len() < 2 => Some(texts::last_index_of(s, t, optional_number(from.first().copied())?)),
            _ => None,
        },
        Op::Reverse => {
            let [Value::List(xs)] = inputs else { return None };
            Some(Value::List(xs.iter().rev().cloned().collect()))
        }
        // `count` unwired is the rest of the list, `items` unwired none.
        Op::ToSpliced => match inputs {
            [Value::List(xs), start, rest @ ..] if rest.len() < 3 => {
                let count = optional_number(rest.first().copied())?;
                let items = rest.get(1).map_or(Some(&[][..]), |items| as_list(items))?;
                Some(Value::List(lists::to_spliced(xs.clone(), as_number(start)?, count, items.to_vec())))
            }
            _ => None,
        },
        Op::WithItem => {
            let [Value::List(xs), i, v] = inputs else { return None };
            let i = as_number(i)?;
            // In range before anything is copied.
            lists::position(i, xs.len())?;
            lists::with_item(xs.clone(), i, (*v).clone()).map(Value::List)
        }
        // `start` unwired is 0, `end` the length.
        Op::Fill => match inputs {
            [Value::List(xs), v, rest @ ..] if rest.len() < 3 => {
                let (start, end) = optional_range(rest)?;
                Some(Value::List(lists::fill(xs.clone(), v, start, end)))
            }
            _ => None,
        },
        Op::CopyWithin => match inputs {
            [Value::List(xs), target, rest @ ..] if rest.len() < 3 => {
                let (start, end) = optional_range(rest)?;
                Some(Value::List(lists::copy_within(xs.clone(), as_number(target)?, start, end)))
            }
            _ => None,
        },
        Op::Sort => {
            let [Value::List(xs)] = inputs else { return None };
            Some(Value::List(lists::sort(xs.clone())))
        }
        Op::Flat => {
            let [Value::List(xs)] = inputs else { return None };
            Some(Value::List(lists::flat(xs.clone(), lists::flat_depth(param(params, "depth")?)?)))
        }
        Op::IsList => {
            let [x] = inputs else { return None };
            Some(Value::Boolean(matches!(x, Value::List(_))))
        }
        Op::Unique => {
            let [Value::List(xs)] = inputs else { return None };
            Some(Value::List(lists::unique(xs.clone())))
        }

        // ── record kit (M1): build a record field by field, read one field, list the keys. A
        // field name is a frozen `name` param, so it is part of the unit's identity, like a
        // state key. A missing field reads as absent, not as a null value. ──
        Op::Record => {
            let [] = inputs else { return None };
            Some(Value::Record(BTreeMap::new()))
        }
        Op::Field => {
            let [Value::Record(fields)] = inputs else { return None };
            fields.get(param(params, "name")?).cloned()
        }
        Op::WithField => {
            let [Value::Record(fields), v] = inputs else { return None };
            let mut fields = fields.clone();
            fields.insert(param(params, "name")?.to_string(), (*v).clone());
            Some(Value::Record(fields))
        }
        Op::WithoutField => {
            let [Value::Record(fields)] = inputs else { return None };
            let mut fields = fields.clone();
            fields.remove(param(params, "name")?);
            Some(Value::Record(fields))
        }
        Op::Keys => {
            let [Value::Record(fields)] = inputs else { return None };
            Some(Value::List(fields.keys().map(|k| Value::Text(k.clone())).collect()))
        }
        // The keyed kit: the same reads and writes, with the key a Text input, so a record can be
        // a collection keyed by something a request brings (`/todos/:title`). Any other key is
        // refused, never coerced to text first.
        Op::Lookup => {
            let [Value::Record(fields), Value::Text(key)] = inputs else { return None };
            fields.get(key).cloned()
        }
        Op::Insert => {
            let [Value::Record(fields), Value::Text(key), v] = inputs else { return None };
            let mut fields = fields.clone();
            fields.insert(key.clone(), (*v).clone());
            Some(Value::Record(fields))
        }
        Op::Remove => {
            let [Value::Record(fields), Value::Text(key)] = inputs else { return None };
            let mut fields = fields.clone();
            fields.remove(key);
            Some(Value::Record(fields))
        }

        // ── records (records.rs): JavaScript's Object functions, `in`, spread, `typeof` and
        // `Object.is`. `group_by` applies its function in the evaluator. ──
        Op::Values => {
            let [Value::Record(fields)] = inputs else { return None };
            Some(records::values(fields))
        }
        Op::FromEntries => {
            let [Value::List(entries)] = inputs else { return None };
            records::from_entries(entries)
        }
        Op::Merge => records::merge(inputs),
        Op::Has => {
            let [Value::Record(fields), Value::Text(key)] = inputs else { return None };
            Some(records::has(fields, key))
        }
        Op::SameValue => {
            let [a, b] = inputs else { return None };
            Some(Value::Boolean(records::same_value(a, b)))
        }
        Op::TypeOf => {
            let [x] = inputs else { return None };
            records::type_of(x).map(|t| Value::Text(t.to_string()))
        }

        // ── dates (dates.rs): JavaScript's Date in UTC, a date its time value; an offset, when
        // given, the calendar a date is read at. ──
        Op::DatePart => {
            let (x, offset) = last_optional(inputs, 1)?;
            dates::part(x[0], offset, param(params, "part")?).map(Value::Number)
        }
        Op::DateUtc => dates::utc(inputs).map(Value::Number),
        Op::DateSet => {
            let [t, given @ ..] = inputs else { return None };
            dates::set(t, given, param(params, "part")?).map(Value::Number)
        }
        Op::DateText => {
            let (t, offset) = last_optional(inputs, 1)?;
            dates::text(t[0], offset, param(params, "format")?).map(Value::Text)
        }
        Op::DateParse => {
            let [Value::Text(s)] = inputs else { return None };
            dates::parse(s).map(Value::Number)
        }
        Op::DateAdd => {
            let (x, offset) = last_optional(inputs, 2)?;
            dates::add(x[0], x[1], offset, param(params, "unit")?).map(Value::Number)
        }
        // ── calendars (dates.rs, host-capabilities L4 §5.1.3): business days over a weekend and
        // the holidays given, a list of calendar dates. ──
        Op::BusinessDayAdd => {
            let (x, holidays) = last_optional(inputs, 2)?;
            dates::business_day_add(x[0], x[1], holidays, param(params, "weekend")?).map(Value::Text)
        }
        Op::BusinessDaysBetween => {
            let (x, holidays) = last_optional(inputs, 2)?;
            dates::business_days_between(x[0], x[1], holidays, param(params, "weekend")?).map(Value::Number)
        }
        Op::IsBusinessDay => {
            let (x, holidays) = last_optional(inputs, 1)?;
            dates::is_business_day(x[0], holidays, param(params, "weekend")?).map(Value::Boolean)
        }

        // ── JSON (M1): request and response bodies. See json.rs for the mapping; malformed input,
        // or a value with no JSON form, is absent. ──
        // With a function (`fn`), the evaluator applies it (eval.rs, `eval_json`); here there is
        // none to apply.
        Op::JsonParse => {
            let [Value::Text(s)] = inputs else { return None };
            if param(params, "fn").is_some_and(|f| !f.is_empty()) {
                return None;
            }
            crate::json::parse(s)
        }
        Op::JsonStringify => {
            let [v] = inputs else { return None };
            if param(params, "fn").is_some_and(|f| !f.is_empty()) {
                return None;
            }
            let keys = match param(params, "keys").unwrap_or("") {
                "" => None,
                list => match crate::json::parse(list)? {
                    Value::List(items) => Some(crate::json::key_list(&items)),
                    _ => return None,
                },
            };
            let layout = crate::json::Layout { gap: crate::json::gap(param(params, "indent").unwrap_or("")), keys: keys.as_deref() };
            crate::json::stringify_laid_out(v, &layout).map(Value::Text)
        }

        // ── signatures and encodings (crypto.rs): over a text's UTF-8 bytes, or bytes. An unknown
        // encoding, mode or alphabet is absent, and so is a decode that isn't valid input or
        // doesn't come back to UTF-8. ──
        Op::Sha256 => {
            let [v] = inputs else { return None };
            crate::crypto::sha256(octets(v)?, param(params, "encoding")?).map(Value::Text)
        }
        Op::HmacSha256 => {
            let [key, message] = inputs else { return None };
            crate::crypto::hmac_sha256(octets(key)?, octets(message)?, param(params, "encoding")?).map(Value::Text)
        }
        Op::Hex => {
            let [Value::Text(s)] = inputs else { return None };
            match param(params, "mode")? {
                "encode" => Some(Value::Text(crate::crypto::hex_encode(s.as_bytes()))),
                "decode" => String::from_utf8(crate::crypto::hex_decode(s)?).ok().map(Value::Text),
                _ => None,
            }
        }
        // `standard` encodes padded, `url` unpadded; either decodes with or without padding. The
        // bytes are the text's UTF-8, or (`latin1`, `btoa` and `atob`'s, the standard alphabet
        // only) each character one byte.
        Op::Base64 => {
            let [Value::Text(s)] = inputs else { return None };
            let name = param(params, "alphabet")?;
            let alphabet = crate::crypto::alphabet(name)?;
            match (param(params, "bytes")?, param(params, "mode")?) {
                ("utf8", "encode") => Some(Value::Text(crate::crypto::base64_encode(s.as_bytes(), alphabet, name == "standard"))),
                ("utf8", "decode") => String::from_utf8(crate::crypto::base64_decode(s, alphabet)?).ok().map(Value::Text),
                ("latin1", "encode") if name == "standard" => texts::latin1_base64(s).map(Value::Text),
                ("latin1", "decode") if name == "standard" => texts::base64_latin1(s).map(Value::Text),
                _ => None,
            }
        }
        // Text equality in constant time: what a signature is checked with, so how long the
        // comparison takes says nothing about how much of a guess was right.
        Op::SecureEquals => {
            let [a, b] = inputs else { return None };
            Some(Value::Boolean(crate::crypto::secure_equals(octets(a)?, octets(b)?)))
        }

        // ── bytes (host-capabilities L4 §5.10.1): a text's encodings, both ways (nothing for hex
        // or base64 that isn't, or bytes that aren't UTF-8), a length, a join, and the numbers 0
        // to 255 they are (nothing for an item that isn't one, where JavaScript wraps it). ──
        Op::TextToBytes => {
            let [Value::Text(s)] = inputs else { return None };
            crate::crypto::text_bytes(s, param(params, "encoding")?).map(Value::Bytes)
        }
        Op::BytesToText => {
            let [Value::Bytes(b)] = inputs else { return None };
            crate::crypto::bytes_text(b, param(params, "encoding")?).map(Value::Text)
        }
        Op::BytesLength => {
            let [Value::Bytes(b)] = inputs else { return None };
            Some(Value::Number(b.len() as f64))
        }
        Op::BytesConcat => {
            let mut out = Vec::new();
            for v in inputs {
                let Value::Bytes(b) = v else { return None };
                out.extend_from_slice(b);
            }
            Some(Value::Bytes(out))
        }
        Op::BytesToList => {
            let [Value::Bytes(b)] = inputs else { return None };
            Some(Value::List(b.iter().map(|x| Value::Number(f64::from(*x))).collect()))
        }
        Op::ListToBytes => {
            let [Value::List(xs)] = inputs else { return None };
            let byte = |x: &Value| match x {
                Value::Number(n) if n.fract() == 0.0 && (0.0..=255.0).contains(n) => Some(*n as u8),
                _ => None,
            };
            xs.iter().map(byte).collect::<Option<Vec<u8>>>().map(Value::Bytes)
        }

        // ── decimals (decimals.rs, host-capabilities L4 §5.1.1): exact, over amounts written as
        // text; nothing for a text that isn't a decimal, a division by zero or a scale that isn't
        // a whole number from 0 to 1,000. ──
        Op::DecimalAdd => crate::decimals::add(inputs).map(Value::Text),
        Op::DecimalSub => {
            let [a, b] = inputs else { return None };
            crate::decimals::sub(a, b).map(Value::Text)
        }
        Op::DecimalMul => crate::decimals::mul(inputs).map(Value::Text),
        Op::DecimalDiv => {
            let [a, b, scale] = inputs else { return None };
            crate::decimals::div(a, b, scale, param(params, "rounding")?).map(Value::Text)
        }
        Op::DecimalRound => {
            let [a, scale] = inputs else { return None };
            crate::decimals::round(a, scale, param(params, "rounding")?).map(Value::Text)
        }
        Op::DecimalSum => {
            let [Value::List(xs)] = inputs else { return None };
            crate::decimals::sum(xs).map(Value::Text)
        }
        Op::DecimalCompare => {
            let [a, b] = inputs else { return None };
            crate::decimals::compare(a, b).map(Value::Number)
        }

        // ── CSV (csv.rs): RFC 4180 both ways; nothing for a malformed file, or a field that
        // isn't a text, a number or a Boolean. ──
        Op::CsvParse => {
            let [Value::Text(s)] = inputs else { return None };
            crate::csv::parse(s, param(params, "header")?, param(params, "separator")?)
        }
        Op::CsvFormat => {
            let (rows, columns) = match inputs {
                [Value::List(rows)] => (rows, None),
                [Value::List(rows), columns] => (rows, Some(*columns)),
                _ => return None,
            };
            crate::csv::format(rows, columns, param(params, "header")?, param(params, "separator")?).map(Value::Text)
        }

        // ── XML (xml.rs): an element as JsonML's list; nothing for text that isn't well-formed or
        // has a DOCTYPE, or for what XML can't hold. ──
        Op::XmlParse => {
            let [Value::Text(s)] = inputs else { return None };
            crate::xml::parse(s)
        }
        Op::XmlFormat => {
            let [element] = inputs else { return None };
            crate::xml::format(element, param(params, "declaration")?, param(params, "layout")?).map(Value::Text)
        }
        Op::XmlFind => {
            let [element, Value::Text(path)] = inputs else { return None };
            crate::xml::find(element, path)
        }
        Op::XmlText => match inputs {
            [element] => crate::xml::text(element, None),
            [element, Value::Text(path)] => crate::xml::text(element, Some(path)),
            _ => None,
        },
        Op::XmlAttribute => {
            let [element, Value::Text(name)] = inputs else { return None };
            crate::xml::attribute(element, name)
        }

        // ── compression (compress.rs, host-capabilities L4 §5.1.7): gzip and zip both ways, the
        // same bytes everywhere; a text is its UTF-8. Nothing for what isn't one, or past 64 MiB
        // inflated. ──
        Op::Gzip => {
            let [data] = inputs else { return None };
            Some(Value::Bytes(crate::compress::gzip(octets(data)?)))
        }
        Op::Gunzip => {
            let [Value::Bytes(b)] = inputs else { return None };
            crate::compress::gunzip(b).map(Value::Bytes)
        }
        Op::Zip => {
            let [Value::Record(files)] = inputs else { return None };
            let files = files.iter().map(|(name, data)| Some((name.as_str(), octets(data)?))).collect::<Option<Vec<_>>>()?;
            crate::compress::zip(files).map(Value::Bytes)
        }
        Op::Unzip => {
            let [Value::Bytes(b)] = inputs else { return None };
            let files = crate::compress::unzip(b)?;
            Some(Value::Record(files.into_iter().map(|(name, data)| (name, Value::Bytes(data))).collect()))
        }

        // ── JWT (jwt.rs, host-capabilities L4 §5.1.9): the key decides the algorithm, and the
        // claims hold at `now`. Nothing for a token that doesn't verify. ──
        Op::JwtVerify => {
            let [Value::Text(token), key, Value::Number(now)] = inputs else { return None };
            let leeway = param(params, "leeway")?.parse::<f64>().ok()?;
            crate::jwt::verify(token, key, *now, param(params, "issuer")?, param(params, "audience")?, leeway)
        }
        Op::JwtDecode => {
            let [Value::Text(token)] = inputs else { return None };
            crate::jwt::decode(token)
        }

        // ── OpenPGP (pgp.rs, host-capabilities L4 §5.1.11): a key's own signatures decide what it
        // may do; nothing for a key or signature that doesn't bear out. ──
        Op::PgpEncrypt => {
            let [data, keys, Value::Bytes(random), Value::Number(now)] = inputs else { return None };
            crate::pgp::encrypt(data, keys, random, *now, param(params, "armor")? == "ascii").map(Value::Bytes)
        }
        Op::PgpVerify => {
            let [data, signature, keys, Value::Number(now)] = inputs else { return None };
            crate::pgp::verify(data, signature, keys, *now)
        }
        Op::PgpKey => {
            let [key] = inputs else { return None };
            crate::pgp::key_info(key)
        }

        Op::Pair => {
            let [a, b] = inputs else { return None };
            Some(Value::List(vec![(*a).clone(), (*b).clone()]))
        }

        // ── sum (P1.3.2 inch 3): Option<T> = Sum([T, Unit]), arm 0 = some, arm 1 = none. Unlike
        // pair, this DOES need a new Value shape (Value::Sum) — a sum value carries only its taken
        // arm, so it can't ride List the way a product can (a product's value always carries every
        // field). none's payload is Unit's value, which already rides the empty list. ──
        Op::Some => {
            let [v] = inputs else { return None };
            Some(Value::Sum { arm: 0, payload: Box::new((*v).clone()) })
        }
        Op::None => {
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
        Op::Egress => sink_effect("egress", inputs),
        // A state write is a Fork-B sink too: it constructs a `store_write` request; the host
        // performs it by *storing* (capability `StateWrite`). `store_read` has no pure compute
        // — it is sourced (host-supplied), like a driver. Drivers are sourced too (no compute).
        //
        // Keyed now (see the keyed-state-capability L4): the settled `Value::Effect` this
        // constructs is the ONLY thing that survives evaluation — the leaf's own `params` are
        // gone by the time the host walks the output looking for effects to perform. So the key
        // has to travel *inside* the sink string itself, or it's lost entirely. Encoded as
        // `"store_write:<key>"`; `holon-host`'s `perform_effects` undoes this same encoding.
        // Fails closed exactly like `const` two arms up (`?` on a missing param): a `store_write`
        // with no key never even constructs an effect, rather than silently constructing an
        // ownerless one that would vanish at the perform step with no visible error.
        Op::StoreWrite => {
            let key = param(params, "key")?;
            sink_effect(&format!("store_write:{key}"), inputs)
        }

        // A collection's writes: `{key, value}` to put, the key to delete, each an effect the
        // host commits when the run finishes. The collection rides in the sink name, as a state
        // write's key does. A value holding an effect is not data, and constructs nothing.
        Op::CollectionPut => {
            let name = crate::kind::ident(params, "collection")?;
            let [Value::Text(key), v] = inputs else { return None };
            if contains_effect(v) {
                return None;
            }
            let item = Value::Record(BTreeMap::from([("key".to_string(), Value::Text(key.clone())), ("value".to_string(), (*v).clone())]));
            effect(format!("collection_put:{name}"), item)
        }
        Op::CollectionDelete => {
            let name = crate::kind::ident(params, "collection")?;
            let [Value::Text(key)] = inputs else { return None };
            effect(format!("collection_delete:{name}"), Value::Text(key.clone()))
        }

        // A named folder's writes: `{name, text}` to write the file (a text, or bytes written as
        // they are), the name to delete it, each an effect the host commits when the run finishes.
        // The folder rides in the sink name, as a collection's does. Which names are allowed is the
        // host's to check (it knows the folder).
        Op::FileWrite => {
            let folder = crate::kind::ident(params, "folder")?;
            let [Value::Text(name), text @ (Value::Text(_) | Value::Bytes(_))] = inputs else { return None };
            let file = Value::Record(BTreeMap::from([("name".to_string(), Value::Text(name.clone())), ("text".to_string(), (*text).clone())]));
            effect(format!("file_write:{folder}"), file)
        }
        Op::FileDelete => {
            let folder = crate::kind::ident(params, "folder")?;
            let [Value::Text(name)] = inputs else { return None };
            effect(format!("file_delete:{folder}"), Value::Text(name.clone()))
        }
        // A copy by handle: `{name, from: <handle>}`, the bytes copied host to host when the run
        // finishes. A handle is what `file_handle` answered; the host checks the file it names is
        // in a folder this deployment reads, and still has the handle's hash.
        Op::FileCopy => {
            let folder = crate::kind::ident(params, "folder")?;
            let [Value::Text(name), from @ Value::Record(handle)] = inputs else { return None };
            if contains_effect(from) || !["folder", "name", "hash"].iter().all(|k| matches!(handle.get(*k), Some(Value::Text(_)))) {
                return None;
            }
            let copy = Value::Record(BTreeMap::from([("name".to_string(), Value::Text(name.clone())), ("from".to_string(), (*from).clone())]));
            effect(format!("file_copy:{folder}"), copy)
        }
        // Create-only: a write's or a copy's payload with `create: true`. The host lands it only
        // where no file has the name when the run commits, never replacing one.
        Op::FileCreate | Op::FileCreateCopy => {
            let folder = crate::kind::ident(params, "folder")?;
            let like = if name == "file_create" { "file_write" } else { "file_copy" };
            let Some(Value::Effect { payload, .. }) = compute(like, inputs, params) else { return None };
            let Value::Record(mut file) = *payload else { return None };
            file.insert("create".to_string(), Value::Boolean(true));
            effect(format!("{name}:{folder}"), Value::Record(file))
        }

        // A statement to run when the run finishes: `{sql, params, expect}`, `expect` the rows it
        // must change (absent: any number). The host sends it in the run's commit transaction.
        Op::DbExec => {
            let database = crate::kind::ident(params, "database")?;
            let [params_in @ Value::List(_)] = inputs else { return None };
            if contains_effect(params_in) {
                return None;
            }
            let sql = params.iter().find(|p| p.name() == "sql")?.value()?;
            let mut exec = BTreeMap::from([("sql".to_string(), Value::Text(sql.to_string())), ("params".to_string(), (*params_in).clone())]);
            if let Some(n) = crate::kind::expect_of(params)? {
                exec.insert("expect".to_string(), Value::Number(n as f64));
            }
            effect(format!("db_exec:{database}"), Value::Record(exec))
        }
        // A document database's writes when the run finishes: `{insert: <collection>, document,
        // params}`, `{update: …, filter, change, params, expect}`, `{delete: …, filter, params,
        // expect}`, the templates as their text. The host fills their holes and sends them in the
        // run's commit transaction there.
        Op::DbInsert | Op::DbUpdate | Op::DbDelete => {
            let database = crate::kind::ident(params, "database")?;
            let [params_in @ Value::List(_)] = inputs else { return None };
            if contains_effect(params_in) {
                return None;
            }
            let text = |name: &str| params.iter().find(|p| p.name() == name).and_then(|p| p.value()).map(|v| Value::Text(v.to_string()));
            let verb = name.trim_start_matches("db_");
            let mut write = BTreeMap::from([(verb.to_string(), text("collection")?), ("params".to_string(), (*params_in).clone())]);
            match verb {
                "insert" => {
                    write.insert("document".to_string(), text("document")?);
                }
                _ => {
                    write.insert("filter".to_string(), text("filter")?);
                    if verb == "update" {
                        write.insert("change".to_string(), text("update")?);
                    }
                    if let Some(n) = crate::kind::expect_of(params)? {
                        write.insert("expect".to_string(), Value::Number(n as f64));
                    }
                }
            }
            effect(format!("{name}:{database}"), Value::Record(write))
        }
        // A channel's pack: any value but an effect, sent to its subscribers once the run is over.
        // The channel rides in the sink name, as a collection's does.
        Op::ChannelSend => {
            let channel = crate::kind::ident(params, "channel")?;
            sink_effect(&format!("channel_send:{channel}"), inputs)
        }
        // A mail: its message made regular and held to the recipients the text names (mail.rs).
        // The mail rides in the sink name; its recipients in the payload, for the host to hold to
        // the grant and the binding as it sends.
        Op::MailSend => {
            let mail = crate::kind::ident(params, "mail")?;
            let message = crate::mail::message(inputs.first()?, param(params, "to"))?;
            Some(Value::Effect { sink: format!("mail_send:{mail}"), payload: Box::new(message) })
        }
        // A queue's message: `{data, group}`, its data any value but an effect, its group text
        // when given. The queue rides in the sink name.
        // A line for the deployment's log: any value but an effect, at the leaf's level. The host
        // keeps it apart from the run's record (logs.rs), so it is no part of what verify checks.
        Op::Log => {
            let level = crate::kind::ident(params, "level")?;
            if !matches!(level.as_str(), "log" | "info" | "debug" | "warn" | "error") {
                return None;
            }
            sink_effect(&format!("log:{level}"), inputs)
        }
        Op::QueueSend => {
            let queue = crate::kind::ident(params, "queue")?;
            let Value::Record(message) = inputs.first()? else { return None };
            let data = message.get("data").filter(|d| !matches!(d, Value::Effect { .. }))?;
            let mut sent = BTreeMap::from([("data".to_string(), data.clone())]);
            match message.get("group") {
                None => {}
                Some(Value::Text(g)) if !g.is_empty() && g.len() <= 128 => {
                    sent.insert("group".to_string(), Value::Text(g.clone()));
                }
                Some(_) => return None,
            }
            Some(Value::Effect { sink: format!("queue_send:{queue}"), payload: Box::new(Value::Record(sent)) })
        }
        // A subscription: what a route answers with, read by the host from the response (never
        // performed as a sink). `from` is "new" (packs from now on) or "kept" (the kept ones first).
        Op::ChannelSubscribe => {
            let channel = crate::kind::ident(params, "channel")?;
            let from = param(params, "from").unwrap_or("new");
            if !["new", "kept"].contains(&from) {
                return None;
            }
            let ask = Value::Record(BTreeMap::from([("channel".to_string(), Value::Text(channel.clone())), ("from".to_string(), Value::Text(from.to_string()))]));
            effect(format!("channel_subscribe:{channel}"), ask)
        }
        // A file as a route's answer: a handle of the node's folder, each part of its type, and
        // the type its name says. The host reads the file again as it sends it (never performed
        // as a sink).
        Op::FileServe => {
            let folder = crate::kind::ident(params, "folder")?;
            let [Value::Record(handle)] = inputs else { return None };
            let text = |k: &str| match handle.get(k) {
                Some(Value::Text(t)) => Some(t.clone()),
                _ => None,
            };
            let count = |k: &str| matches!(handle.get(k), Some(Value::Number(n)) if *n >= 0.0 && n.trunc() == *n && *n <= 9007199254740991.0);
            let (name, hash) = (text("name")?, text("hash")?);
            let blake3 = hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if text("folder")? != folder || handle.len() != 5 || name.is_empty() || !blake3 || !count("size") || !count("modified") {
                return None;
            }
            let mut served = handle.clone();
            served.insert("type".to_string(), Value::Text(content_type(&name).to_string()));
            effect(format!("file_serve:{folder}"), Value::Record(served))
        }

        // An unknown name has no Op: `compute` gives none for it, a driver or state boundary unit
        // being the host's.
    }
}

/// The present inputs of a leaf as the evaluator hands them over when it can give some: each lent
/// (`Cow::Borrowed`), or given (`Cow::Owned`), moved out of where it was kept because nothing will
/// read it there again (eval.rs, `Prepared::fire_giving`).
pub(crate) type Given<'v> = smallvec::SmallVec<[Cow<'v, Value>; 4]>;

/// Whether leaf `op` builds its output from its input at `port` when that input is given, instead
/// of copying it: what makes giving it worth anything. `append` grows its list in place, `field`
/// moves the field out of its record, and so on; a leaf that only reads an input (`length`,
/// `compare`) gains nothing from owning it.
pub(crate) fn keeps(op: Op, port: usize) -> bool {
    match op {
        Op::Append | Op::Extend | Op::Cons | Op::WithField | Op::Pair => port < 2,
        Op::Insert => port < 3,
        Op::Select => port == 1 || port == 2,
        Op::Tail | Op::WithoutField | Op::Remove | Op::Concat | Op::Head | Op::Get | Op::Field | Op::Lookup | Op::Some | Op::Default => port == 0,
        Op::List | Op::Merge => true,
        Op::Values | Op::FromEntries => port == 0,
        Op::At | Op::Reverse | Op::Sort | Op::Unique | Op::Flat | Op::Fill | Op::CopyWithin | Op::Slice => port == 0,
        Op::WithItem => port == 0 || port == 2,
        Op::ToSpliced => port == 0 || port == 3,
        _ => false,
    }
}

/// [`compute_op`] on inputs some of which may be given (`Cow::Owned`): a leaf that builds its
/// output from a given input builds it in place of that input, so `append` to a given list pushes
/// onto it instead of copying it first, and `insert` adds to a given record. The output is always
/// `compute_op`'s on the same values; only what is copied to make it differs. When there is no
/// output, the inputs come back unchanged (`Err`), so a caller that gave one can put it back where
/// it was: every arm checks everything it needs before it takes anything. They come back as they
/// went in, not boxed, which would allocate for every leaf that produces nothing.
#[allow(clippy::result_large_err)]
pub(crate) fn compute_given<'v>(op: Op, name: &str, mut inputs: Given<'v>, params: &[Param]) -> Result<Value, Given<'v>> {
    /// A given input's value, or a copy of a lent one.
    fn own(v: Cow<'_, Value>) -> Value {
        v.into_owned()
    }
    /// The items of a list input, already checked to be one.
    fn items(v: Cow<'_, Value>) -> Vec<Value> {
        match v {
            Cow::Owned(Value::List(xs)) => xs,
            Cow::Borrowed(Value::List(xs)) => xs.clone(),
            _ => unreachable!("checked to be a list"),
        }
    }
    /// The fields of a record input, already checked to be one.
    fn fields(v: Cow<'_, Value>) -> BTreeMap<String, Value> {
        match v {
            Cow::Owned(Value::Record(fs)) => fs,
            Cow::Borrowed(Value::Record(fs)) => fs.clone(),
            _ => unreachable!("checked to be a record"),
        }
    }
    /// The text of a text input, already checked to be one.
    fn string(v: Cow<'_, Value>) -> String {
        match v {
            Cow::Owned(Value::Text(s)) => s,
            Cow::Borrowed(Value::Text(s)) => s.clone(),
            _ => unreachable!("checked to be a text"),
        }
    }
    // What it would build nests within the bound, before anything is taken ([`fits`]).
    if !fits(op, inputs.iter().map(|v| v.as_ref())) {
        return Err(inputs);
    }
    let is = |i: usize, f: fn(&Value) -> bool| inputs.get(i).is_some_and(|v| f(v));
    let list = |v: &Value| matches!(v, Value::List(_));
    let record = |v: &Value| matches!(v, Value::Record(_));
    let text = |v: &Value| matches!(v, Value::Text(_));
    let n = inputs.len();
    match op {
        Op::Append if n == 2 && is(0, list) => {
            let item = own(inputs.pop().unwrap());
            let mut xs = items(inputs.pop().unwrap());
            xs.push(item);
            Ok(Value::List(xs))
        }
        Op::Extend if n == 2 && is(0, list) && is(1, list) => {
            let more = inputs.pop().unwrap();
            let first = inputs.pop().unwrap();
            let shorter = |first: &Value, ys: &[Value]| as_list(first).is_some_and(|xs| xs.len() < ys.len());
            match more {
                // The second list given and the longer (`[x, ...acc]`): the first's items go in
                // front of it, where it is, rather than it behind a copy of them.
                Cow::Owned(Value::List(mut ys)) if shorter(&first, &ys) => {
                    ys.splice(0..0, items(first));
                    Ok(Value::List(ys))
                }
                more => {
                    let mut xs = items(first);
                    match more {
                        Cow::Owned(Value::List(mut ys)) => xs.append(&mut ys),
                        more => xs.extend_from_slice(as_list(&more).expect("checked to be a list")),
                    }
                    Ok(Value::List(xs))
                }
            }
        }
        Op::Cons if n == 2 && is(1, list) => {
            let mut xs = items(inputs.pop().unwrap());
            xs.insert(0, own(inputs.pop().unwrap()));
            Ok(Value::List(xs))
        }
        Op::Tail if n == 1 && matches!(&*inputs[0], Value::List(xs) if !xs.is_empty()) => {
            let mut xs = items(inputs.pop().unwrap());
            xs.remove(0);
            Ok(Value::List(xs))
        }
        Op::Insert if n == 3 && is(0, record) && is(1, text) => {
            let v = own(inputs.pop().unwrap());
            let key = string(inputs.pop().unwrap());
            let mut fs = fields(inputs.pop().unwrap());
            fs.insert(key, v);
            Ok(Value::Record(fs))
        }
        Op::WithField if n == 2 && is(0, record) && param(params, "name").is_some() => {
            let v = own(inputs.pop().unwrap());
            let mut fs = fields(inputs.pop().unwrap());
            fs.insert(param(params, "name").expect("checked").to_string(), v);
            Ok(Value::Record(fs))
        }
        Op::WithoutField if n == 1 && is(0, record) && param(params, "name").is_some() => {
            let mut fs = fields(inputs.pop().unwrap());
            fs.remove(param(params, "name").expect("checked"));
            Ok(Value::Record(fs))
        }
        Op::Remove if n == 2 && is(0, record) && is(1, text) => {
            let key = inputs.pop().unwrap();
            let mut fs = fields(inputs.pop().unwrap());
            fs.remove(as_text(&key).expect("checked to be a text"));
            Ok(Value::Record(fs))
        }
        // The first text grows by the others: one allocation, sized once.
        Op::Concat if inputs.iter().all(|v| text(v)) => {
            let more: usize = inputs.iter().skip(1).map(|v| as_text(v).map_or(0, str::len)).sum();
            let mut parts = inputs.into_iter();
            let mut s = parts.next().map_or_else(String::new, string);
            s.reserve(more);
            for v in parts {
                s.push_str(as_text(&v).expect("checked to be a text"));
            }
            Ok(Value::Text(s))
        }
        Op::Select if n == 3 && matches!(*inputs[0], Value::Boolean(_)) => {
            let b = inputs.pop().unwrap();
            let a = inputs.pop().unwrap();
            Ok(own(if matches!(*inputs[0], Value::Boolean(true)) { a } else { b }))
        }
        Op::Head if n == 1 && matches!(&*inputs[0], Value::List(xs) if !xs.is_empty()) => Ok(items_first(inputs.pop().unwrap())),
        Op::Get if n == 2 && matches!((&*inputs[0], as_index(&inputs[1])), (Value::List(xs), Some(i)) if i < xs.len()) => {
            let i = as_index(&inputs.pop().unwrap()).expect("checked");
            Ok(match inputs.pop().unwrap() {
                Cow::Owned(Value::List(mut xs)) => xs.swap_remove(i),
                l => as_list(&l).expect("checked")[i].clone(),
            })
        }
        Op::Field if n == 1 && matches!((&*inputs[0], param(params, "name")), (Value::Record(fs), Some(k)) if fs.contains_key(k)) => {
            let k = param(params, "name").expect("checked");
            Ok(match inputs.pop().unwrap() {
                Cow::Owned(Value::Record(mut fs)) => fs.remove(k).expect("checked"),
                r => field_of(&r, k),
            })
        }
        Op::Lookup if n == 2 && matches!((&*inputs[0], &*inputs[1]), (Value::Record(fs), Value::Text(k)) if fs.contains_key(k)) => {
            let key = inputs.pop().unwrap();
            let k = as_text(&key).expect("checked");
            Ok(match inputs.pop().unwrap() {
                Cow::Owned(Value::Record(mut fs)) => fs.remove(k).expect("checked"),
                r => field_of(&r, k),
            })
        }
        Op::List => Ok(Value::List(inputs.into_iter().map(own).collect())),
        // The record leaves build on a given record: its values moved out, its fields grown by
        // the next record's (moved too, when given), an entry's key and value moved out.
        Op::Values if n == 1 && is(0, record) => Ok(Value::List(fields(inputs.pop().unwrap()).into_values().collect())),
        Op::FromEntries if n == 1 && matches!(&*inputs[0], Value::List(es) if es.iter().all(records::is_entry)) => Ok(match inputs.pop().unwrap() {
            Cow::Owned(Value::List(es)) => records::from_owned_entries(es),
            es => records::from_entries(as_list(&es).expect("checked to be a list")).expect("checked to be entries"),
        }),
        Op::Merge if inputs.iter().all(|v| record(v)) => {
            let mut parts = inputs.into_iter();
            let mut fs = parts.next().map_or_else(BTreeMap::new, fields);
            for more in parts {
                fs.extend(fields(more));
            }
            Ok(Value::Record(fs))
        }
        Op::Pair if n == 2 => {
            let b = own(inputs.pop().unwrap());
            Ok(Value::List(vec![own(inputs.pop().unwrap()), b]))
        }
        Op::Some if n == 1 => Ok(Value::Sum { arm: 0, payload: Box::new(own(inputs.pop().unwrap())) }),
        Op::Default if n == 1 => Ok(own(inputs.pop().unwrap())),
        // The list leaves build on a given list: reversed, sorted or filled where it is, an item
        // moved out of it.
        Op::At if n == 2 && matches!((&*inputs[0], &*inputs[1]), (Value::List(xs), Value::Number(i)) if lists::position(*i, xs.len()).is_some()) => {
            let i = as_number(&inputs.pop().unwrap()).expect("checked");
            Ok(match inputs.pop().unwrap() {
                Cow::Owned(Value::List(mut xs)) => {
                    let k = lists::position(i, xs.len()).expect("checked");
                    xs.swap_remove(k)
                }
                l => {
                    let xs = as_list(&l).expect("checked");
                    xs[lists::position(i, xs.len()).expect("checked")].clone()
                }
            })
        }
        Op::Reverse if n == 1 && is(0, list) => {
            let mut xs = items(inputs.pop().unwrap());
            xs.reverse();
            Ok(Value::List(xs))
        }
        Op::Sort if n == 1 && is(0, list) => Ok(Value::List(lists::sort(items(inputs.pop().unwrap())))),
        Op::Unique if n == 1 && is(0, list) => Ok(Value::List(lists::unique(items(inputs.pop().unwrap())))),
        Op::Flat if n == 1 && is(0, list) && param(params, "depth").and_then(lists::flat_depth).is_some() => {
            let depth = param(params, "depth").and_then(lists::flat_depth).expect("checked");
            Ok(Value::List(lists::flat(items(inputs.pop().unwrap()), depth)))
        }
        Op::WithItem if n == 3 && matches!((&*inputs[0], &*inputs[1]), (Value::List(xs), Value::Number(i)) if lists::position(*i, xs.len()).is_some()) => {
            let v = own(inputs.pop().unwrap());
            let i = as_number(&inputs.pop().unwrap()).expect("checked");
            Ok(Value::List(lists::with_item(items(inputs.pop().unwrap()), i, v).expect("checked")))
        }
        Op::ToSpliced if (2..=4).contains(&n) && is(0, list) && inputs[1..3.min(n)].iter().all(|v| matches!(**v, Value::Number(_))) && (n < 4 || is(3, list)) => {
            let more = if n == 4 { items(inputs.pop().unwrap()) } else { vec![] };
            let count = (n >= 3).then(|| as_number(&inputs.pop().unwrap()).expect("checked"));
            let start = as_number(&inputs.pop().unwrap()).expect("checked");
            Ok(Value::List(lists::to_spliced(items(inputs.pop().unwrap()), start, count, more)))
        }
        Op::Fill | Op::CopyWithin if (2..=4).contains(&n) && is(0, list) && inputs[if op == Op::Fill { 2 } else { 1 }..].iter().all(|v| matches!(**v, Value::Number(_))) => {
            let (start, end) = optional_range(&inputs[2..].iter().map(|v| v.as_ref()).collect::<smallvec::SmallVec<[&Value; 2]>>()).expect("checked");
            let mut inputs = inputs.into_iter();
            let xs = items(inputs.next().unwrap());
            let second = inputs.next().unwrap();
            Ok(Value::List(match op {
                Op::Fill => lists::fill(xs, &second, start, end),
                _ => lists::copy_within(xs, as_number(&second).expect("checked"), start, end),
            }))
        }
        Op::Slice if n >= 1 && is(0, list) => {
            let range = {
                let rest: smallvec::SmallVec<[&Value; 2]> = inputs[1..].iter().map(|v| v.as_ref()).collect();
                list_slice(params, as_list(&inputs[0]).expect("checked").len(), &rest)
            };
            match range {
                Some(range) => {
                    let mut xs = items(inputs.into_iter().next().unwrap());
                    xs.truncate(range.end);
                    xs.drain(..range.start);
                    Ok(Value::List(xs))
                }
                None => Err(inputs),
            }
        }
        // Anything else reads its inputs: lent, as it always is.
        _ => {
            let out = {
                let lent: smallvec::SmallVec<[&Value; 4]> = inputs.iter().map(|v| v.as_ref()).collect();
                compute_op(op, name, &lent, params)
            };
            out.ok_or(inputs)
        }
    }
}

/// The first item of a non-empty list input, moved out of it when it is given.
fn items_first(v: Cow<'_, Value>) -> Value {
    match v {
        Cow::Owned(Value::List(xs)) => xs.into_iter().next().expect("checked to be non-empty"),
        l => as_list(&l).and_then(|xs| xs.first()).expect("checked to be non-empty").clone(),
    }
}

/// The field `k` of a record input, already checked to hold it, copied.
fn field_of(r: &Value, k: &str) -> Value {
    match r {
        Value::Record(fs) => fs[k].clone(),
        _ => unreachable!("checked to be a record"),
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
    // Nor one nested past the bound, a level down in the effect.
    if contains_effect(p) || !nests_within(p, MAX_VALUE_DEPTH - 1) {
        return None;
    }
    Some(Value::Effect { sink: sink.into(), payload: Box::new((*p).clone()) })
}

/// The primitives that construct an Effect from a payload input (see [`sink_effect`]).
pub(crate) fn is_sink(name: &str) -> bool {
    matches!(name, "egress" | "store_write" | "http_request" | "http_stream" | "flow_call" | "flow_start" | "collection_put" | "channel_send" | "mail_send" | "queue_send" | "log")
}

/// The most items one `collection_list` returns.
pub const MAX_LIST_LIMIT: u64 = 1000;

/// The largest `n` a `range` counts to.
pub const MAX_RANGE: u64 = 10_000;

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

/// What a `file_read` asks the host: `{folder: <name>, read: <file name>}`.
pub(crate) fn file_read_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let folder = crate::kind::ident(params, "folder")?;
    let [Some(Value::Text(name))] = inputs else { return None };
    Some(Value::Record(BTreeMap::from([
        ("folder".to_string(), Value::Text(folder)),
        ("read".to_string(), Value::Text(name.clone())),
    ])))
}

/// What a `file_read_bytes` asks the host: a `file_read`'s request, with `bytes: true`, answered
/// with the file's bytes whatever they are.
pub(crate) fn file_read_bytes_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let Value::Record(mut asked) = file_read_request(inputs, params)? else { return None };
    asked.insert("bytes".to_string(), Value::Boolean(true));
    Some(Value::Record(asked))
}

/// What a `file_list` asks the host: `{folder: <name>, list: {prefix, after?, limit}}`, as
/// [`collection_list_request`] asks for items, here for files by name.
pub(crate) fn file_list_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let folder = crate::kind::ident(params, "folder")?;
    let renamed: Vec<Param> = params.iter().map(|p| if p.name() == "folder" { Param::frozen("collection", &folder) } else { p.clone() }).collect();
    let Value::Record(mut asked) = collection_list_request(inputs, &renamed)? else { return None };
    asked.remove("collection");
    asked.insert("folder".to_string(), Value::Text(folder));
    Some(Value::Record(asked))
}

/// What a `db_query` or `db_one` asks the host: `{database, query: <sql>, params, one}`, `one`
/// when it answers the first row alone.
pub(crate) fn db_query_request(inputs: &[Option<Value>], params: &[Param], one: bool) -> Option<Value> {
    let database = crate::kind::ident(params, "database")?;
    let sql = params.iter().find(|p| p.name() == "sql")?.value()?;
    let [Some(list @ Value::List(_))] = inputs else { return None };
    if contains_effect(list) {
        return None;
    }
    Some(Value::Record(BTreeMap::from([
        ("database".to_string(), Value::Text(database)),
        ("query".to_string(), Value::Text(sql.to_string())),
        ("params".to_string(), list.clone()),
        ("one".to_string(), Value::Boolean(one)),
    ])))
}

/// What a `db_find` or `db_find_one` asks the host: `{database, find: <collection>, filter,
/// params, one}`, and `sort` and `limit` when it has them, the templates as their text; `one` when
/// it answers the first document alone.
pub(crate) fn db_find_request(inputs: &[Option<Value>], params: &[Param], one: bool) -> Option<Value> {
    let database = crate::kind::ident(params, "database")?;
    let text = |name: &str| params.iter().find(|p| p.name() == name).and_then(|p| p.value());
    let [Some(list @ Value::List(_))] = inputs else { return None };
    if contains_effect(list) {
        return None;
    }
    let mut asked = BTreeMap::from([
        ("database".to_string(), Value::Text(database)),
        ("find".to_string(), Value::Text(text("collection")?.to_string())),
        ("filter".to_string(), Value::Text(text("filter")?.to_string())),
        ("params".to_string(), list.clone()),
        ("one".to_string(), Value::Boolean(one)),
    ]);
    if let Some(sort) = text("sort").filter(|s| !s.is_empty()) {
        asked.insert("sort".to_string(), Value::Text(sort.to_string()));
    }
    if !one && let Some(n) = crate::kind::limit_of(params)? {
        asked.insert("limit".to_string(), Value::Number(n as f64));
    }
    Some(Value::Record(asked))
}

/// What a `file_handle` asks the host: `{folder: <name>, handle: <file name>}`.
pub(crate) fn file_handle_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let folder = crate::kind::ident(params, "folder")?;
    let [Some(Value::Text(name))] = inputs else { return None };
    Some(Value::Record(BTreeMap::from([
        ("folder".to_string(), Value::Text(folder)),
        ("handle".to_string(), Value::Text(name.clone())),
    ])))
}

/// The most lines one `file_lines` returns.
pub const MAX_LINES_LIMIT: u64 = 10_000;

/// A whole number, as a range's position or length is.
fn whole(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 9e15 => Some(*n),
        _ => None,
    }
}

/// What a `file_lines` asks the host: `{folder, lines: {name, from, limit}}`, for up to `limit`
/// lines (1 to [`MAX_LINES_LIMIT`]) from line `from` (0, the first, when unwired; negative counts
/// from the end).
pub(crate) fn file_lines_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let folder = crate::kind::ident(params, "folder")?;
    let limit: u64 = param(params, "limit")?.parse().ok().filter(|n| (1..=MAX_LINES_LIMIT).contains(n))?;
    let [Some(Value::Text(name)), from] = inputs else { return None };
    let from = match from {
        None => 0.0,
        Some(v) => whole(v)?,
    };
    let lines = BTreeMap::from([
        ("name".to_string(), Value::Text(name.clone())),
        ("from".to_string(), Value::Number(from)),
        ("limit".to_string(), Value::Number(limit as f64)),
    ]);
    Some(Value::Record(BTreeMap::from([("folder".to_string(), Value::Text(folder)), ("lines".to_string(), Value::Record(lines))])))
}

/// What a `file_slice` asks the host: `{folder, slice: {name, offset, length}}`: `length` bytes
/// (not negative) from byte `offset` (negative counts from the end).
pub(crate) fn file_slice_request(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let folder = crate::kind::ident(params, "folder")?;
    let [Some(Value::Text(name)), Some(offset), Some(length)] = inputs else { return None };
    let (offset, length) = (whole(offset)?, whole(length).filter(|n| *n >= 0.0)?);
    let slice = BTreeMap::from([
        ("name".to_string(), Value::Text(name.clone())),
        ("offset".to_string(), Value::Number(offset)),
        ("length".to_string(), Value::Number(length)),
    ]);
    Some(Value::Record(BTreeMap::from([("folder".to_string(), Value::Text(folder)), ("slice".to_string(), Value::Record(slice))])))
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

/// The most people an approval waits for.
pub const MAX_APPROVAL_PEOPLE: u64 = 5;

/// What an `approval` node asks the host: `{await: <name>, expires: <seconds>, people: <n>}`,
/// an `await`'s request with how many different people must approve. `None` for a bad name, an
/// `expires` as an `await`'s, or `people` not from 1 to [`MAX_APPROVAL_PEOPLE`].
pub(crate) fn approval_request(params: &[Param]) -> Option<Value> {
    let Some(Value::Record(mut r)) = await_request(params) else { return None };
    let people: u64 = param(params, "people")?.parse().ok().filter(|n| (1..=MAX_APPROVAL_PEOPLE).contains(n))?;
    r.insert("people".to_string(), Value::Number(people as f64));
    Some(Value::Record(r))
}

/// What a `resume_token` node asks the host: `{resume_token: <await name>}`.
pub(crate) fn resume_token_request(params: &[Param]) -> Option<Value> {
    let name = crate::kind::ident(params, "await")?;
    Some(Value::Record(BTreeMap::from([("resume_token".to_string(), Value::Text(name))])))
}

/// Whether leaf `name` has optional input ports, read as "none" when left unwired: a request's
/// headers and body, a listing's prefix and after, a `to_number`'s radix, a `number_format`'s
/// digits, a `slice`'s end, `to_spliced`'s count and items, `fill`'s and `copy_within`'s start and
/// end, the position a search starts from, a `split`'s limit, a `pad`'s fill, a `csv_format`'s
/// columns, a date leaf's offset, a business-day leaf's holidays and an `xml_text`'s path. Inside a composite, such a port that *is* wired holds the node back while its value is
/// absent (eval.rs), like any other leaf's input.
pub(crate) fn has_optional_ports(name: &str) -> bool {
    matches!(
        name,
        "http_request"
            | "http_upload"
            | "http_stream"
            | "flow_call"
            | "flow_start"
            | "collection_list"
            | "file_list"
            | "file_lines"
            | "to_number"
            | "number_format"
            | "slice"
            | "to_spliced"
            | "fill"
            | "copy_within"
            | "index_of"
            | "includes"
            | "last_index_of"
            | "text_contains"
            | "split"
            | "pad"
            | "csv_format"
            | "date_part"
            | "date_text"
            | "date_add"
            | "business_day_add"
            | "business_days_between"
            | "is_business_day"
            | "xml_text"
    )
}

/// What a `flow_call` asks the host: `{flow, action, input}`, to run the action `action` of the
/// deployed flow `flow` with `input` (`{}` when none is wired); a `flow_start`'s says `start: true`
/// too, answered once the run finishes or first waits. `None` when there is nothing valid to send:
/// a name that isn't a name, or an Effect anywhere in what is sent.
pub(crate) fn flow_call_request(inputs: &[Option<Value>], params: &[Param], start: bool) -> Option<Value> {
    let flow = crate::kind::ident(params, "flow")?;
    let action = crate::kind::ident(params, "action")?;
    let input = match inputs.first() {
        Some(Some(v)) if contains_effect(v) => return None,
        Some(Some(v)) => v.clone(),
        _ => Value::Record(BTreeMap::new()),
    };
    let mut request = BTreeMap::from([("flow".to_string(), Value::Text(flow)), ("action".to_string(), Value::Text(action)), ("input".to_string(), input)]);
    if start {
        request.insert("start".to_string(), Value::Boolean(true));
    }
    Some(Value::Record(request))
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

/// The request an `http_stream` node makes: an `http_request`'s, marked `stream: true`, which
/// tells the host to forward its answer to the route's caller as it reads it, or, piped,
/// `pipe: "updates"`, to that channel's subscribers.
pub(crate) fn http_stream(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let Value::Record(mut request) = http_request(inputs, params)? else { return None };
    match params.iter().find(|p| p.name() == "pipe").and_then(|p| p.value()).filter(|v| !v.is_empty()) {
        Some(channel) => request.insert("pipe".to_string(), Value::Text(channel.to_string())),
        None => request.insert("stream".to_string(), Value::Boolean(true)),
    };
    Some(Value::Record(request))
}

/// The request an `http_upload` node makes: an `http_request`'s, with `file` in place of `body`:
/// the handle's `{folder, name, size, hash}`, the bytes the host sends. `None` when the request
/// isn't valid as an `http_request`'s, or the handle isn't one (a folder and a name as text, a
/// size, a BLAKE3 hash in hex).
pub(crate) fn http_upload(inputs: &[Option<Value>], params: &[Param]) -> Option<Value> {
    let [path, headers, Some(Value::Record(handle))] = inputs else { return None };
    let Value::Record(mut request) = http_request(&[path.clone(), headers.clone(), None], params)? else { return None };
    let text = |k: &str| match handle.get(k) {
        Some(Value::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    };
    let (folder, name, hash) = (text("folder")?, text("name")?, text("hash")?);
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let size = whole(handle.get("size")?).filter(|n| *n >= 0.0)?;
    request.remove("body");
    request.insert(
        "file".to_string(),
        Value::Record(BTreeMap::from([
            ("folder".to_string(), Value::Text(folder)),
            ("name".to_string(), Value::Text(name)),
            ("size".to_string(), Value::Number(size)),
            ("hash".to_string(), Value::Text(hash)),
        ])),
    );
    Some(Value::Record(request))
}

/// Whether an Effect occurs anywhere in `v`.
pub(crate) fn contains_effect(v: &Value) -> bool {
    match v {
        Value::Effect { .. } => true,
        Value::List(items) => items.iter().any(contains_effect),
        Value::Record(fields) => fields.values().any(contains_effect),
        Value::Sum { payload, .. } => contains_effect(payload),
        Value::Number(_) | Value::Boolean(_) | Value::Text(_) | Value::Bytes(_) => false,
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
        Value::Number(n) if n.is_finite() => number_text(*n),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Text(s) => format!("\"{s}\""),
        Value::List(xs) => format!("[{}]", xs.iter().map(to_text_str).collect::<Vec<_>>().join(", ")),
        Value::Effect { sink, payload } => format!("effect({sink}, {})", to_text_str(payload)),
        Value::Sum { arm, payload } => format!("sum#{arm}({})", to_text_str(payload)),
        Value::Bytes(b) => format!("bytes({})", crate::crypto::hex_encode(b)),
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
/// The bytes a hash or a comparison takes: a text's UTF-8, or bytes as they are.
fn octets(v: &Value) -> Option<&[u8]> {
    match v {
        Value::Text(s) => Some(s.as_bytes()),
        Value::Bytes(b) => Some(b),
        _ => None,
    }
}
/// The items a `slice` of a list of `len` keeps, by its `mode` and its inputs after the list.
fn list_slice(params: &[Param], len: usize, rest: &[&Value]) -> Option<std::ops::Range<usize>> {
    match (param(params, "mode")?, rest) {
        ("length", [start, count]) => Some(lists::length_range(len, as_index(start)?, as_index(count)?)),
        ("end", [start, end @ ..]) if end.len() < 2 => Some(lists::slice_range(len, as_number(start)?, optional_number(end.first().copied())?)),
        _ => None,
    }
}
/// An optional port's number: none when unwired, nothing at all (the outer none) when it isn't a
/// number.
fn optional_number(v: Option<&Value>) -> Option<Option<f64>> {
    match v {
        None => Some(None),
        Some(v) => as_number(v).map(Some),
    }
}
/// Optional `start` and `end` ports, as `fill` and `copy_within` read them: `start` 0 when
/// unwired, `end` none.
fn optional_range(rest: &[&Value]) -> Option<(f64, Option<f64>)> {
    Some((optional_number(rest.first().copied())?.unwrap_or(0.0), optional_number(rest.get(1).copied())?))
}
/// A list index: a non-negative whole number.
fn as_index(v: &Value) -> Option<usize> {
    let n = as_number(v)?;
    if n >= 0.0 && n.fract() == 0.0 { Some(n as usize) } else { None }
}

// ── helpers ─────────────────────────────────────────────────────────────────
/// A leaf's `n` inputs and its optional last one, when it arrived: nothing for another count.
fn last_optional<'a, 'v>(inputs: &'a [&'v Value], n: usize) -> Option<(&'a [&'v Value], Option<&'v Value>)> {
    match inputs.len() {
        k if k == n => Some((inputs, None)),
        k if k == n + 1 => Some((&inputs[..n], Some(inputs[n]))),
        _ => None,
    }
}

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
/// A number as JavaScript writes it (ECMA-262 `Number::toString`): its shortest digits
/// that read back as it, in full from 1e-6 up to 1e21 and in exponent form outside
/// (`1e+21`, `1.5e-7`). `-0` is `0`: the two are equal, and the canonical value encoding folds
/// them. No leaf answers a number that isn't finite, but a module may (a `js` function's
/// `1 / 0`), and one is written as JavaScript writes it too: `NaN`, `Infinity`, `-Infinity`.
pub(crate) fn number_text(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    if !n.is_finite() {
        return if n.is_nan() { "NaN" } else if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `{:e}` writes the same shortest digits, as `d.ddde<exp>`.
    let e = format!("{:e}", n.abs());
    let (mantissa, exp) = e.split_once('e').expect("{:e} writes an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // Where the decimal point falls after the first `point` digits.
    let point = exp.parse::<i32>().expect("an exponent is a number") + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat(-point as usize))
    } else {
        let sign = if point >= 1 { '+' } else { '-' };
        let rest = if k == 1 { String::new() } else { format!(".{}", &digits[1..]) };
        format!("{}{rest}e{sign}{}", &digits[..1], (point - 1).abs())
    };
    if n < 0.0 { format!("-{body}") } else { body }
}

/// `n` as a number when it is finite: what arithmetic gives, else nothing. Tested on the `f64`
/// itself, in place: an `Option` filter here was a call a node (performance.md).
#[inline]
fn finite(n: f64) -> Option<Value> {
    n.is_finite().then_some(Value::Number(n))
}

fn fold_numbers(inputs: &[&Value], init: f64, f: fn(f64, f64) -> f64) -> Option<f64> {
    let mut acc = init;
    for v in inputs {
        acc = f(acc, as_number(v)?);
    }
    Some(acc)
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
        assert_eq!(compute("is_truthy", &[&empty], &[]), Some(boo(true)), "an empty record is true, as in JavaScript");
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
    fn a_queues_message_is_its_data_and_its_group_naming_the_queue() {
        let payouts = [Param::frozen("queue", "payouts")];
        let rec = |kv: &[(&str, Value)]| Value::Record(kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let data = rec(&[("id", txt("po_1")), ("amount", txt("12.50"))]);
        let sent = |m: &Value| compute("queue_send", &[m], &payouts);
        assert_eq!(sent(&rec(&[("data", data.clone())])), Some(Value::Effect { sink: "queue_send:payouts".into(), payload: Box::new(rec(&[("data", data.clone())])) }));
        let grouped = rec(&[("data", data.clone()), ("group", txt("acct_9"))]);
        assert_eq!(sent(&grouped), Some(Value::Effect { sink: "queue_send:payouts".into(), payload: Box::new(grouped.clone()) }));
        assert_eq!(sent(&rec(&[("data", txt("a"))])).map(|e| matches!(e, Value::Effect { .. })), Some(true), "any value is data");
        assert_eq!(sent(&rec(&[("group", txt("g"))])), None, "no data");
        assert_eq!(sent(&rec(&[("data", data.clone()), ("group", num(1.0))])), None, "a group is text");
        assert_eq!(sent(&rec(&[("data", data.clone()), ("group", txt(""))])), None, "an empty group");
        let effect = sent(&grouped).unwrap();
        assert_eq!(sent(&rec(&[("data", effect)])), None, "an effect is not data");
        assert_eq!(compute("queue_send", &[&grouped], &[]), None, "a queue names itself");
    }

    #[test]
    fn a_pack_and_a_subscription_are_effects_naming_their_channel() {
        let updates = [Param::frozen("channel", "updates"), Param::frozen("from", "new")];
        let rec = |kv: &[(&str, Value)]| Value::Record(kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let pack = rec(&[("new", txt("a ticket"))]);
        assert_eq!(compute("channel_send", &[&pack], &updates), Some(Value::Effect { sink: "channel_send:updates".into(), payload: Box::new(pack.clone()) }));
        let effect = compute("channel_send", &[&pack], &updates).unwrap();
        assert_eq!(compute("channel_send", &[&effect], &updates), None, "an effect is not a pack");
        let ask = rec(&[("channel", txt("updates")), ("from", txt("new"))]);
        assert_eq!(compute("channel_subscribe", &[], &updates), Some(Value::Effect { sink: "channel_subscribe:updates".into(), payload: Box::new(ask) }));
        let kept = [Param::frozen("channel", "updates"), Param::frozen("from", "kept")];
        assert!(compute("channel_subscribe", &[], &kept).is_some());
        let odd = [Param::frozen("channel", "updates"), Param::frozen("from", "yesterday")];
        assert_eq!(compute("channel_subscribe", &[], &odd), None, "from is new or kept");
    }

    #[test]
    fn a_served_file_is_an_effect_from_a_handle_of_its_folder() {
        let site = [Param::frozen("folder", "site")];
        let rec = |kv: &[(&str, Value)]| Value::Record(kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let hash = "7c1e2a9b".repeat(8);
        let handle = |folder: &str, name: &str| {
            rec(&[("folder", txt(folder)), ("name", txt(name)), ("size", Value::Number(148.0)), ("hash", txt(&hash)), ("modified", Value::Number(1_700_000_000_000.0))])
        };
        let served = compute("file_serve", &[&handle("site", "assets/app.js")], &site).unwrap();
        let Value::Effect { sink, payload } = served else { panic!("an effect") };
        assert_eq!(sink, "file_serve:site");
        let mut want = handle("site", "assets/app.js");
        if let Value::Record(m) = &mut want {
            m.insert("type".into(), txt("text/javascript; charset=utf-8"));
        }
        assert_eq!(*payload, want);
        assert_eq!(compute("file_serve", &[&handle("logs", "a.js")], &site), None, "another folder's handle");
        assert_eq!(compute("file_serve", &[&txt("assets/app.js")], &site), None, "a name is not a handle");
        let Value::Record(mut odd) = handle("site", "a.js") else { unreachable!() };
        odd.insert("hash".into(), txt("ABC"));
        assert_eq!(compute("file_serve", &[&Value::Record(odd.clone())], &site), None, "a hash is blake3's hex");
        odd.insert("hash".into(), txt(&hash));
        odd.insert("size".into(), Value::Number(-1.0));
        assert_eq!(compute("file_serve", &[&Value::Record(odd.clone())], &site), None, "a size counts bytes");
        odd.insert("size".into(), Value::Number(1.0));
        odd.insert("extra".into(), txt("x"));
        assert_eq!(compute("file_serve", &[&Value::Record(odd)], &site), None, "a handle and no more");
        assert_eq!(compute("file_serve", &[&handle("site", "a.js")], &[]), None, "a folder names itself");
    }

    #[test]
    fn a_files_type_is_its_extensions() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("assets/app.4f2a.JS"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("logo.png"), "image/png");
        assert_eq!(content_type("app.js.map"), "application/json");
        assert_eq!(content_type("font.woff2"), "font/woff2");
        assert_eq!(content_type("README"), "application/octet-stream");
        assert_eq!(content_type("a.b/README"), "application/octet-stream", "a folder's dot is not an extension");
        assert_eq!(content_type("archive.tar.gz"), "application/octet-stream");
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
    fn file_reads_ask_the_host_and_writes_construct_effects() {
        let inbox = [p("folder", "inbox")];
        let read = file_read_request(&[Some(txt("2026/a.csv"))], &inbox).unwrap();
        assert_eq!(read, Value::Record(BTreeMap::from([("folder".into(), txt("inbox")), ("read".into(), txt("2026/a.csv"))])));
        assert_eq!(file_read_request(&[Some(num(1.0))], &inbox), None, "names are text");
        let list = file_list_request(&[Some(txt("2026/")), None], &[p("folder", "inbox"), p("limit", "20")]).unwrap();
        let Value::Record(r) = &list else { panic!() };
        assert_eq!(r.keys().collect::<Vec<_>>(), ["folder", "list"]);
        let Value::Record(l) = &r["list"] else { panic!() };
        assert_eq!((l["prefix"].clone(), l.get("after"), l["limit"].clone()), (txt("2026/"), None, num(20.0)));
        assert!(file_list_request(&[None, None], &[p("folder", "inbox"), p("limit", "0")]).is_none());
        assert!(file_list_request(&[None, None], &[p("folder", "../x"), p("limit", "5")]).is_none(), "a folder is a name");

        let write = compute("file_write", &[&txt("a.md"), &txt("# total")], &inbox).unwrap();
        let file = Value::Record(BTreeMap::from([("name".into(), txt("a.md")), ("text".into(), txt("# total"))]));
        assert_eq!(write, Value::Effect { sink: "file_write:inbox".into(), payload: Box::new(file) });
        assert_eq!(compute("file_delete", &[&txt("a.md")], &inbox), Some(Value::Effect { sink: "file_delete:inbox".into(), payload: Box::new(txt("a.md")) }));
        assert_eq!(compute("file_write", &[&txt("a.md"), &num(1.0)], &inbox), None, "a file's text is text");

        let handle = file_handle_request(&[Some(txt("big.log"))], &inbox).unwrap();
        assert_eq!(handle, Value::Record(BTreeMap::from([("folder".into(), txt("inbox")), ("handle".into(), txt("big.log"))])));
        let lines = file_lines_request(&[Some(txt("big.log")), Some(num(-100.0))], &[p("folder", "inbox"), p("limit", "100")]).unwrap();
        let Value::Record(r) = &lines else { panic!() };
        assert_eq!(r["lines"], Value::Record(BTreeMap::from([("name".into(), txt("big.log")), ("from".into(), num(-100.0)), ("limit".into(), num(100.0))])));
        let first = file_lines_request(&[Some(txt("big.log")), None], &[p("folder", "inbox"), p("limit", "5")]).unwrap();
        let Value::Record(r) = &first else { panic!() };
        assert_eq!(r["lines"], Value::Record(BTreeMap::from([("name".into(), txt("big.log")), ("from".into(), num(0.0)), ("limit".into(), num(5.0))])), "from the first line");
        assert!(file_lines_request(&[Some(txt("a")), Some(num(1.5))], &[p("folder", "inbox"), p("limit", "5")]).is_none(), "a line is whole");
        assert!(file_lines_request(&[Some(txt("a")), None], &[p("folder", "inbox"), p("limit", "0")]).is_none());
        assert!(file_slice_request(&[Some(txt("a")), Some(num(-10.0)), Some(num(10.0))], &inbox).is_some(), "the last 10 bytes");
        assert!(file_slice_request(&[Some(txt("a")), Some(num(0.0)), Some(num(-1.0))], &inbox).is_none(), "a length isn't negative");

        let h = Value::Record(BTreeMap::from([("folder".into(), txt("inbox")), ("name".into(), txt("big.log")), ("hash".into(), txt("ab")), ("size".into(), num(9.0))]));
        let copy = compute("file_copy", &[&txt("b.log"), &h], &inbox).unwrap();
        let payload = Value::Record(BTreeMap::from([("name".into(), txt("b.log")), ("from".into(), h.clone())]));
        assert_eq!(copy, Value::Effect { sink: "file_copy:inbox".into(), payload: Box::new(payload) });
        let not_a_handle = Value::Record(BTreeMap::from([("name".into(), txt("big.log"))]));
        assert_eq!(compute("file_copy", &[&txt("b.log"), &not_a_handle], &inbox), None, "a copy needs a handle");

        // Create-only: the same payloads, saying so.
        let create = compute("file_create", &[&txt("a.md"), &txt("# total")], &inbox).unwrap();
        let file = Value::Record(BTreeMap::from([("name".into(), txt("a.md")), ("text".into(), txt("# total")), ("create".into(), Value::Boolean(true))]));
        assert_eq!(create, Value::Effect { sink: "file_create:inbox".into(), payload: Box::new(file) });
        let copy = compute("file_create_copy", &[&txt("b.log"), &h], &inbox).unwrap();
        let payload = Value::Record(BTreeMap::from([("name".into(), txt("b.log")), ("from".into(), h), ("create".into(), Value::Boolean(true))]));
        assert_eq!(copy, Value::Effect { sink: "file_create_copy:inbox".into(), payload: Box::new(payload) });
        assert_eq!(compute("file_create_copy", &[&txt("b.log"), &not_a_handle], &inbox), None, "a copy needs a handle");
    }

    /// `range(n)` counts from 0 to n-1; a fraction, a negative, a non-number or more than
    /// MAX_RANGE is nothing.
    #[test]
    fn range_counts_to_n_and_no_further_than_its_cap() {
        let nums = |xs: &[f64]| Some(Value::List(xs.iter().map(|&x| num(x)).collect()));
        assert_eq!(compute("range", &[&num(3.0)], &[]), nums(&[0.0, 1.0, 2.0]));
        assert_eq!(compute("range", &[&num(0.0)], &[]), nums(&[]));
        let Some(Value::List(most)) = compute("range", &[&num(MAX_RANGE as f64)], &[]) else { panic!() };
        assert_eq!((most.len(), most.last()), (MAX_RANGE as usize, Some(&num(MAX_RANGE as f64 - 1.0))));
        for n in [num(MAX_RANGE as f64 + 1.0), num(-1.0), num(2.5), num(f64::INFINITY), txt("3")] {
            assert_eq!(compute("range", &[&n], &[]), None, "{n:?}");
        }
    }

    #[test]
    fn split_is_joins_inverse() {
        let texts = |xs: &[&str]| Some(Value::List(xs.iter().map(|x| txt(x)).collect()));
        assert_eq!(compute("split", &[&txt("t=1,v1=ab"), &txt(",")], &[]), texts(&["t=1", "v1=ab"]));
        assert_eq!(compute("split", &[&txt("a,,b,"), &txt(",")], &[]), texts(&["a", "", "b", ""]), "empty pieces kept");
        assert_eq!(compute("split", &[&txt("a::b"), &txt("::")], &[]), texts(&["a", "b"]));
        assert_eq!(compute("split", &[&txt("abc"), &txt(",")], &[]), texts(&["abc"]));
        assert_eq!(compute("split", &[&txt(""), &txt(",")], &[]), texts(&[""]));
        assert_eq!(compute("split", &[&txt("abc"), &txt("")], &[]), texts(&["a", "b", "c"]), "an empty separator cuts every unit");
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
        assert_eq!(compute("secure_equals", &[&txt("a"), &num(1.0)], &[]), None, "a text or bytes");
        assert_eq!(
            compute("sha256", &[&txt("abc")], &[p("encoding", "base64")]),
            Some(txt("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="))
        );
        assert_eq!(compute("sha256", &[&txt("abc")], &[p("encoding", "binary")]), None);
        assert_eq!(compute("sha256", &[&num(1.0)], &[p("encoding", "hex")]), None);

        let b64 = |mode: &str, alphabet: &str| vec![p("mode", mode), p("alphabet", alphabet), p("bytes", "utf8")];
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

    /// Decimals and CSV (host-capabilities L4 §5.1.1) through the catalog, with its defaults: a
    /// control sum exact where a number's is off, rounded half to even, and a payment file read
    /// and written back.
    #[test]
    fn decimal_and_csv_leaves_compute_with_the_catalogs_defaults() {
        let defaults = |name: &str| crate::interface_of(name).unwrap().params;
        let n = Value::Number;
        let amounts = Value::List(vec![txt("0.10"), txt("0.20"), n(0.3)]);
        assert_eq!(compute("decimal_sum", &[&amounts], &defaults("decimal_sum")), Some(txt("0.60")));
        assert_eq!(compute("decimal_add", &[&txt("1.10"), &txt("2"), &txt("-0.05")], &defaults("decimal_add")), Some(txt("3.05")));
        assert_eq!(compute("decimal_round", &[&txt("2.665"), &n(2.0)], &defaults("decimal_round")), Some(txt("2.66")), "half to even");
        assert_eq!(compute("decimal_round", &[&txt("2.665"), &n(2.0)], &[p("rounding", "half_up")]), Some(txt("2.67")));
        assert_eq!(compute("decimal_div", &[&txt("100"), &txt("3"), &n(2.0)], &defaults("decimal_div")), Some(txt("33.33")));
        assert_eq!(compute("decimal_compare", &[&txt("10.00"), &txt("9.99")], &[]), Some(n(1.0)));
        assert_eq!(compute("decimal_mul", &[&txt("19.99"), &n(3.0)], &[]), Some(txt("59.97")));
        assert_eq!(compute("decimal_sub", &[&txt("1"), &txt("one")], &[]), None);

        let file = txt("id;currency;amount\r\n1;EUR;10.50\r\n2;USD;0.20\r\n");
        let rows = compute("csv_parse", &[&file], &[p("header", "first_row"), p("separator", ";")]).unwrap();
        let Value::List(items) = &rows else { panic!("{rows:?}") };
        assert_eq!(items[1], Value::Record([("amount".into(), txt("0.20")), ("currency".into(), txt("USD")), ("id".into(), txt("2"))].into()));
        assert_eq!(compute("csv_parse", &[&txt("a,\"b\n")], &defaults("csv_parse")), None, "a quote that never closes");
        let columns = Value::List(vec![txt("id"), txt("currency"), txt("amount")]);
        let params = [p("header", "first_row"), p("separator", ";")];
        assert_eq!(compute("csv_format", &[&rows, &columns], &params), Some(file), "read and written back");
        let lists = Value::List(vec![Value::List(vec![txt("a"), n(1.0)])]);
        assert_eq!(compute("csv_format", &[&lists], &defaults("csv_format")), Some(txt("a,1\r\n")), "no columns: rows are lists");
    }

    /// Bytes (host-capabilities L4 §5.10.1): a text's encodings both ways, a length, a join, a
    /// slice, the numbers they are, and the hashing leaves over them as over a text's UTF-8.
    #[test]
    fn bytes_leaves_compute_as_javascript_does() {
        let bytes = |b: &[u8]| Value::Bytes(b.to_vec());
        let enc = |e: &str| vec![p("encoding", e)];
        assert_eq!(compute("text_to_bytes", &[&txt("hé")], &enc("utf8")), Some(bytes(&[0x68, 0xc3, 0xa9])));
        assert_eq!(compute("text_to_bytes", &[&txt("00ff10")], &enc("hex")), Some(bytes(&[0, 0xff, 0x10])));
        assert_eq!(compute("text_to_bytes", &[&txt("+/8=")], &enc("base64")), Some(bytes(&[0xfb, 0xff])));
        assert_eq!(compute("text_to_bytes", &[&txt("+/8")], &enc("base64")), Some(bytes(&[0xfb, 0xff])), "padding optional");
        assert_eq!(compute("text_to_bytes", &[&txt("-_8")], &enc("base64url")), Some(bytes(&[0xfb, 0xff])));
        for (bad, e) in [("0g", "hex"), ("abc", "hex"), ("-_8", "base64"), ("Zm9v", "latin1")] {
            assert_eq!(compute("text_to_bytes", &[&txt(bad)], &enc(e)), None, "{bad} as {e}");
        }
        let b = bytes(&[0xfb, 0xff, 0x00]);
        assert_eq!(compute("bytes_to_text", &[&b], &enc("hex")), Some(txt("fbff00")));
        assert_eq!(compute("bytes_to_text", &[&b], &enc("base64")), Some(txt("+/8A")));
        assert_eq!(compute("bytes_to_text", &[&bytes(&[0xfb, 0xff])], &enc("base64")), Some(txt("+/8=")), "base64 pads");
        assert_eq!(compute("bytes_to_text", &[&bytes(&[0xfb, 0xff])], &enc("base64url")), Some(txt("-_8")), "base64url doesn't");
        assert_eq!(compute("bytes_to_text", &[&bytes("hé".as_bytes())], &enc("utf8")), Some(txt("hé")));
        assert_eq!(compute("bytes_to_text", &[&b], &enc("utf8")), None, "a fatal decoder: 0xfb isn't UTF-8");
        assert_eq!(compute("bytes_to_text", &[&txt("x")], &enc("utf8")), None, "takes bytes");

        assert_eq!(compute("bytes_length", &[&b], &[]), Some(num(3.0)));
        assert_eq!(compute("bytes_length", &[&txt("abc")], &[]), None);
        assert_eq!(compute("bytes_concat", &[&bytes(b"ab"), &bytes(b""), &bytes(b"c")], &[]), Some(bytes(b"abc")));
        assert_eq!(compute("bytes_concat", &[&bytes(b"ab"), &txt("c")], &[]), None);
        assert_eq!(compute("bytes_to_list", &[&bytes(&[0, 255])], &[]), Some(list(vec![num(0.0), num(255.0)])));
        assert_eq!(compute("list_to_bytes", &[&list(vec![num(0.0), num(255.0)])], &[]), Some(bytes(&[0, 255])));
        for bad in [num(256.0), num(-1.0), num(1.5), txt("1")] {
            assert_eq!(compute("list_to_bytes", &[&list(vec![bad.clone()])], &[]), None, "{bad:?}: JavaScript wraps it, Holon gives nothing");
        }
        let slice = |start: f64, end: Option<f64>| {
            let mut ins = vec![bytes(b"hello"), num(start)];
            ins.extend(end.map(num));
            let refs: Vec<&Value> = ins.iter().collect();
            compute("slice", &refs, &[p("mode", "end")])
        };
        assert_eq!(slice(1.0, Some(3.0)), Some(bytes(b"el")));
        assert_eq!(slice(-2.0, None), Some(bytes(b"lo")), "negative from the end");

        // Hashing bytes is hashing a text's UTF-8 that has them; a key may be bytes too.
        assert_eq!(compute("sha256", &[&bytes(b"abc")], &enc("hex")), compute("sha256", &[&txt("abc")], &enc("hex")));
        assert_eq!(
            compute("hmac_sha256", &[&bytes(&[0x0b; 20]), &txt("Hi There")], &enc("hex")),
            Some(txt("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"))
        );
        assert_eq!(compute("secure_equals", &[&bytes(b"a"), &txt("a")], &[]), Some(boo(true)), "the same bytes");

        // Equal by content; true as a condition, empty too; String(b) and to_text.
        assert_eq!(compute("equals", &[&bytes(b"a"), &bytes(b"a")], &[]), Some(boo(true)));
        assert_eq!(compute("is_truthy", &[&bytes(b"")], &[]), Some(boo(true)));
        assert_eq!(compute("type_of", &[&b], &[]), Some(txt("object")));
        assert_eq!(compute("as_text", &[&bytes(&[1, 22])], &[]), Some(txt("1,22")));
        assert_eq!(compute("to_text", &[&bytes(b"hi")], &[]), Some(txt("bytes(6869)")));
        assert_eq!(compute("json_stringify", &[&b], &[]), None, "JSON has no bytes");
    }

    /// A leaf's inputs from a JSON array of them, as the races write their cases.
    fn args(json: &str) -> Vec<Value> {
        match crate::json::parse(json) {
            Some(Value::List(xs)) => xs,
            _ => panic!("not a JSON array: {json}"),
        }
    }

    /// Each case: the inputs (a JSON array) and what `name` gives on them (JSON), or nothing.
    fn gives(name: &str, params: &[Param], cases: &[(&str, Option<&str>)]) {
        for (inputs, want) in cases {
            let inputs = args(inputs);
            let refs: Vec<&Value> = inputs.iter().collect();
            let want = want.map(|w| crate::json::parse(w).unwrap_or_else(|| panic!("not JSON: {w}")));
            assert_eq!(compute(name, &refs, params), want, "{name} on {inputs:?}");
        }
    }

    // The list leaves on the races' check cases (crates/holon-author/benches/races_lists.rs),
    // whose native candidates were checked against Boa, with the edges their specs name.

    #[test]
    fn at_reads_from_the_end_when_negative() {
        gives(
            "at",
            &[],
            &[
                ("[[], 0]", None),
                ("[[], -1]", None),
                ("[[1], 0]", Some("1")),
                ("[[1, 2, 3], -1]", Some("3")),
                ("[[1, 2, 3], -3]", Some("1")),
                ("[[1, 2, 3], -4]", None),
                ("[[1, 2, 3], 3]", None),
                ("[[1, 2, 3], 2]", Some("3")),
                ("[[[1], [2]], -1]", Some("[2]")),
                ("[[1, 2, 3], 1.5]", Some("2")),
                ("[[1, 2, 3], -1.5]", Some("3")),
                ("[[1, 2, 3], 4294967296]", None),
                // A text's code unit (texts.rs has the rest).
                ("[\"abc\", 0]", Some("\"a\"")),
                ("[{\"a\": 1}, 0]", None),
            ],
        );
        assert_eq!(compute("at", &[&list(vec![num(1.0), num(2.0)]), &num(-0.0)], &[]), Some(num(1.0)), "-0 is 0");
    }

    #[test]
    fn indices_and_entries_number_a_lists_items_and_entries_pair_a_records() {
        gives("indices", &[], &[("[[]]", Some("[]")), ("[[7]]", Some("[0]")), ("[[1, [2], 3]]", Some("[0, 1, 2]")), ("[\"ab\"]", None)]);
        gives(
            "entries",
            &[],
            &[
                ("[[]]", Some("[]")),
                ("[[7]]", Some("[[0, 7]]")),
                ("[[1, [2], {\"a\": 3}]]", Some("[[0, 1], [1, [2]], [2, {\"a\": 3}]]")),
                ("[{\"b\": 2, \"a\": 1}]", Some("[[\"a\", 1], [\"b\", 2]]")),
                ("[{}]", Some("[]")),
                ("[\"ab\"]", None),
                ("[3]", None),
            ],
        );
    }

    #[test]
    fn includes_index_of_and_last_index_of_compare_by_value() {
        // (inputs, includes, indexOf, lastIndexOf)
        let cases = [
            ("[[], 1]", "false", "-1", "-1"),
            ("[[1], 1]", "true", "0", "0"),
            ("[[1], 2]", "false", "-1", "-1"),
            ("[[1, 2, 3], 1]", "true", "0", "0"),
            ("[[1, 2, 3], 3]", "true", "2", "2"),
            ("[[1, 2, 3], 4]", "false", "-1", "-1"),
            ("[[1, 2, 1, 2], 2]", "true", "1", "3"),
            ("[[[1], [2]], [2]]", "true", "1", "1"),
            ("[[{\"a\": 1}], {\"a\": 1}]", "true", "0", "0"),
            ("[[\"1\", 1], 1]", "true", "1", "1"),
            ("[[true, 1], 1]", "true", "1", "1"),
        ];
        for (inputs, has, first, last) in cases {
            gives("includes", &[], &[(inputs, Some(has))]);
            gives("index_of", &[], &[(inputs, Some(first))]);
            gives("last_index_of", &[], &[(inputs, Some(last))]);
        }
        // -0 equals 0, either way round: SameValueZero.
        let zeros = [(list(vec![num(1.0), num(-0.0)]), num(0.0), 1.0), (list(vec![num(0.0)]), num(-0.0), 0.0)];
        for (xs, x, at) in zeros {
            assert_eq!(compute("includes", &[&xs, &x], &[]), Some(boo(true)));
            assert_eq!(compute("index_of", &[&xs, &x], &[]), Some(num(at)));
            assert_eq!(compute("last_index_of", &[&xs, &x], &[]), Some(num(at)));
        }
        // From a position: relative, as JavaScript's array methods read it.
        // (inputs, includes, indexOf, lastIndexOf)
        let from = [
            ("[[1, 2, 1, 2], 1, 1]", "true", "2", "0"),
            ("[[1, 2, 1, 2], 1, -2]", "true", "2", "2"),
            ("[[1, 2, 1, 2], 2, -1]", "true", "3", "3"),
            ("[[1, 2, 1, 2], 1, 9]", "false", "-1", "2"),
            ("[[1, 2, 1, 2], 1, -9]", "true", "0", "-1"),
            ("[[1, 2, 1, 2], 2, 0]", "true", "1", "-1"),
            ("[[], 1, 0]", "false", "-1", "-1"),
            ("[[1, 2], 2, 1.5]", "true", "1", "1"),
        ];
        for (inputs, has, first, last) in from {
            gives("includes", &[], &[(inputs, Some(has))]);
            gives("index_of", &[], &[(inputs, Some(first))]);
            gives("last_index_of", &[], &[(inputs, Some(last))]);
        }
        gives("index_of", &[], &[("[[1], 1, \"0\"]", None)]);
        // A text: by code units (texts.rs has the rest).
        gives("index_of", &[], &[("[\"abc\", \"c\"]", Some("2")), ("[\"abc\", 1]", None)]);
        gives("includes", &[], &[("[\"abc\", \"b\"]", Some("true"))]);
        gives("last_index_of", &[], &[("[\"abc\", \"b\"]", Some("1"))]);
    }

    #[test]
    fn reverse_and_slice_copy_part_of_a_list() {
        gives("reverse", &[], &[("[[]]", Some("[]")), ("[[1]]", Some("[1]")), ("[[1, 2, 3]]", Some("[3, 2, 1]")), ("[[[1], {\"a\": 2}, 2, 2]]", Some("[2, 2, {\"a\": 2}, [1]]")), ("[\"ab\"]", None)]);
        let end = [p("mode", "end")];
        gives(
            "slice",
            &end,
            &[
                ("[[], 0, 0]", Some("[]")),
                ("[[1], 0, 1]", Some("[1]")),
                ("[[1, 2, 3], 1, 3]", Some("[2, 3]")),
                ("[[1, 2, 3], -2, 3]", Some("[2, 3]")),
                ("[[1, 2, 3], 0, -1]", Some("[1, 2]")),
                ("[[1, 2, 3], 2, 1]", Some("[]")),
                ("[[1, 2, 3], -9, 9]", Some("[1, 2, 3]")),
                ("[[1, 2, 3], 3, 3]", Some("[]")),
                ("[[[1], [2], 3], 0, 2]", Some("[[1], [2]]")),
                ("[[1, 2, 3], 0.5, 2.9]", Some("[1, 2]")),
                // `end` unwired: to the end.
                ("[[1, 2, 3], 1]", Some("[2, 3]")),
                ("[[1, 2, 3], -1]", Some("[3]")),
                ("[[1, 2, 3], \"1\"]", None),
                // A text, by code units (texts.rs has the rest).
                ("[\"abc\", 0, 1]", Some("\"a\"")),
            ],
        );
        let length = [p("mode", "length")];
        gives(
            "slice",
            &length,
            &[
                ("[[1, 2, 3], 1, 1]", Some("[2]")),
                ("[[1, 2, 3], 1, 9]", Some("[2, 3]")),
                ("[[1, 2, 3], 5, 1]", Some("[]")),
                ("[[1, 2, 3], -1, 1]", None),
                ("[[1, 2, 3], 1]", None),
                ("[\"hello\", 1, 3]", Some("\"ell\"")),
            ],
        );
        gives("slice", &[], &[("[[1, 2, 3], 1, 1]", None)]);
        gives("slice", &[p("mode", "substring")], &[("[[1, 2, 3], 1, 1]", None)]);
    }

    #[test]
    fn to_spliced_with_item_fill_and_copy_within_answer_the_changed_list() {
        gives(
            "to_spliced",
            &[],
            &[
                ("[[], 0, 0, []]", Some("[]")),
                ("[[], 0, 0, [1]]", Some("[1]")),
                ("[[1, 2, 3], 1, 1, [9, 9]]", Some("[1, 9, 9, 3]")),
                ("[[1, 2, 3], -1, 5, []]", Some("[1, 2]")),
                ("[[1, 2, 3], 0, -2, [0]]", Some("[0, 1, 2, 3]")),
                ("[[1, 2, 3], 3, 0, [4]]", Some("[1, 2, 3, 4]")),
                ("[[1, 2, 3], 9, 1, [[4]]]", Some("[1, 2, 3, [4]]")),
                ("[[1, 2, 3], -9, 1, []]", Some("[2, 3]")),
                // `count` unwired: the rest; `items` unwired: none.
                ("[[1, 2, 3], 1]", Some("[1]")),
                ("[[1, 2, 3], 1, 1]", Some("[1, 3]")),
                ("[[1, 2, 3], 1, 1, 5]", None),
            ],
        );
        gives(
            "with_item",
            &[],
            &[
                ("[[], 0, 1]", None),
                ("[[1], 0, 9]", Some("[9]")),
                ("[[1, 2, 3], -1, 9]", Some("[1, 2, 9]")),
                ("[[1, 2, 3], 3, 9]", None),
                ("[[1, 2, 3], -4, 9]", None),
                ("[[1, 2, 3], 1, [9]]", Some("[1, [9], 3]")),
            ],
        );
        gives(
            "fill",
            &[],
            &[
                ("[[], 0, 0, 0]", Some("[]")),
                ("[[1, 2, 3], 0, 0, 3]", Some("[0, 0, 0]")),
                ("[[1, 2, 3], 9, 1, 2]", Some("[1, 9, 3]")),
                ("[[1, 2, 3], 9, -2, 9]", Some("[1, 9, 9]")),
                ("[[1, 2, 3], [9], 2, 1]", Some("[1, 2, 3]")),
                // `start` unwired: 0; `end` unwired: the length.
                ("[[1, 2, 3], 7]", Some("[7, 7, 7]")),
                ("[[1, 2, 3], 7, 1]", Some("[1, 7, 7]")),
            ],
        );
        gives(
            "copy_within",
            &[],
            &[
                ("[[], 0, 0, 0]", Some("[]")),
                ("[[1, 2, 3, 4, 5], 0, 3, 5]", Some("[4, 5, 3, 4, 5]")),
                ("[[1, 2, 3, 4, 5], 1, 0, 5]", Some("[1, 1, 2, 3, 4]")),
                ("[[1, 2, 3, 4, 5], 0, 1, 5]", Some("[2, 3, 4, 5, 5]")),
                ("[[1, 2, 3, 4, 5], -2, 0, 9]", Some("[1, 2, 3, 1, 2]")),
                ("[[1, 2, 3, 4, 5], 0, 4, 2]", Some("[1, 2, 3, 4, 5]")),
                ("[[1, 2, 3, 4, 5], 0, 3]", Some("[4, 5, 3, 4, 5]")),
                ("[[1, 2, 3, 4, 5], 2]", Some("[1, 2, 1, 2, 3]")),
            ],
        );
    }

    #[test]
    fn sort_orders_by_javascripts_string_in_utf16_code_units() {
        gives(
            "sort",
            &[],
            &[
                ("[[]]", Some("[]")),
                ("[[10, 9, 1]]", Some("[1, 10, 9]")),
                ("[[\"b\", \"a\", \"B\"]]", Some("[\"B\", \"a\", \"b\"]")),
                ("[[true, false, \"f\"]]", Some("[\"f\", false, true]")),
                ("[[[1, 2], [1], 1]]", Some("[[1], 1, [1, 2]]")),
                // Records all read "[object Object]": stable, they keep their order.
                ("[[{\"a\": 2}, {\"a\": 1}, 0]]", Some("[0, {\"a\": 2}, {\"a\": 1}]")),
                ("[[1e21, 2]]", Some("[1e21, 2]")),
                ("[\"ba\"]", None),
            ],
        );
        // An astral character (U+1F600, D83D DE00 in UTF-16) sorts before U+FF61, as JavaScript
        // compares code units; by code point it would sort after.
        let (astral, high) = (txt("\u{1f600}"), txt("\u{ff61}"));
        assert_eq!(compute("sort", &[&list(vec![high.clone(), astral.clone()])], &[]), Some(list(vec![astral, high])));
        // -1 writes "-1", before "0" (which -0 writes too).
        let signed = list(vec![num(-0.0), num(-1.0), num(0.0)]);
        let Some(Value::List(sorted)) = compute("sort", &[&signed], &[]) else { panic!() };
        assert_eq!(sorted.iter().map(|v| if let Value::Number(n) = v { n.to_bits() } else { 0 }).collect::<Vec<_>>(), [(-1.0f64).to_bits(), (-0.0f64).to_bits(), 0.0f64.to_bits()]);
    }

    #[test]
    fn flat_spreads_lists_depth_levels_deep() {
        let depth = |d: &str| [p("depth", d)];
        gives("flat", &depth("1"), &[("[[]]", Some("[]")), ("[[1]]", Some("[1]")), ("[[[1]]]", Some("[1]")), ("[[1, [2, 3], [[4]], []]]", Some("[1, 2, 3, [4]]")), ("[[{\"a\": [1]}, \"ab\", true]]", Some("[{\"a\": [1]}, \"ab\", true]")), ("[\"ab\"]", None)]);
        let nested = "[[1, [2, [3, [4]]]]]";
        for (d, want) in [("0", "[1, [2, [3, [4]]]]"), ("1", "[1, 2, [3, [4]]]"), ("2", "[1, 2, 3, [4]]"), ("9", "[1, 2, 3, 4]"), ("all", "[1, 2, 3, 4]"), ("2.7", "[1, 2, 3, [4]]"), ("-1", "[1, [2, [3, [4]]]]")] {
            gives("flat", &depth(d), &[(nested, Some(want))]);
        }
        gives("flat", &depth("deep"), &[(nested, None)]);
        gives("flat", &[], &[(nested, None)]);
    }

    #[test]
    fn is_list_and_unique() {
        for (x, want) in [("[[]]", "true"), ("[[1, 2]]", "true"), ("[1]", "false"), ("[\"[1]\"]", "false"), ("[true]", "false"), ("[{}]", "false"), ("[{\"length\": 2}]", "false")] {
            gives("is_list", &[], &[(x, Some(want))]);
        }
        gives(
            "unique",
            &[],
            &[
                ("[[]]", Some("[]")),
                ("[[1]]", Some("[1]")),
                ("[[1, 1]]", Some("[1]")),
                ("[[1, 2, 1, 3, 2]]", Some("[1, 2, 3]")),
                ("[[[1], [1], {\"a\": 1}, {\"a\": 1}, \"1\", 1]]", Some("[[1], {\"a\": 1}, \"1\", 1]")),
                ("[[0, true, \"0\"]]", Some("[0, true, \"0\"]")),
                ("[\"aa\"]", None),
            ],
        );
        let zeros = list(vec![num(0.0), num(-0.0), num(-0.0)]);
        assert_eq!(compute("unique", &[&zeros], &[]), Some(list(vec![num(0.0)])), "-0 is 0, hashed alike");
        // An option is hashed by what it holds, or distinct `some(x)` share one bucket and every
        // pair of them is compared.
        let some = |v: Value| Value::Sum { arm: 0, payload: Box::new(v) };
        let options = list(vec![some(num(1.0)), some(num(2.0)), some(num(1.0)), some(num(2.0))]);
        assert_eq!(compute("unique", &[&options], &[]), Some(list(vec![some(num(1.0)), some(num(2.0))])));
        let hash = |v: &Value| {
            let mut h = std::hash::DefaultHasher::new();
            lists::hash_item(v, &mut h);
            std::hash::Hasher::finish(&h)
        };
        assert_ne!(hash(&some(num(1.0))), hash(&some(num(2.0))));
    }

    #[test]
    fn javascripts_to_boolean_counts_an_empty_list_true() {
        let falsy = [boo(false), num(0.0), num(-0.0), num(f64::NAN), txt("")];
        let truthy = [boo(true), num(1.0), num(-0.5), txt("0"), txt("false"), list(vec![]), Value::Record(BTreeMap::new())];
        assert!(falsy.iter().all(|v| !lists::truthy(v)));
        assert!(truthy.iter().all(lists::truthy));
    }

    #[test]
    fn the_merge_sort_is_stable_total_and_stops_at_an_error() {
        let keys = [3, 1, 2, 1, 3, 0];
        let order = lists::merge_sort::<()>(keys.len(), |a, b| Ok(keys[a] > keys[b])).unwrap();
        assert_eq!(order, [5, 1, 3, 2, 0, 4], "equal keys keep their order");
        for n in 0..40 {
            // A comparator that isn't an order still gives each item once, the same each time.
            let odd = |a: usize, b: usize| Ok::<_, ()>((a * 7 + b * 3) % 5 > 1);
            let first = lists::merge_sort(n, odd).unwrap();
            let mut seen = first.clone();
            seen.sort();
            assert_eq!(seen, (0..n).collect::<Vec<_>>());
            assert_eq!(lists::merge_sort(n, odd).unwrap(), first);
        }
        let mut compared = 0;
        let stopped = lists::merge_sort(8, |_, _| {
            compared += 1;
            if compared == 3 { Err("stop") } else { Ok(false) }
        });
        assert_eq!((stopped, compared), (Err("stop"), 3), "nothing compared after the error");
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
        // UTF-16 code units, not bytes: "ab\u{00e7}" is 3 units / 4 bytes, "a\u{1f600}" 3 units.
        assert_eq!(compute("text_length", &[&txt("ab\u{00e7}")], &[]), Some(num(3.0)));
        assert_eq!(compute("text_length", &[&txt("a\u{1f600}")], &[]), Some(num(3.0)));
        let length = [p("mode", "length")];
        assert_eq!(compute("slice", &[&txt("hello"), &num(1.0), &num(3.0)], &length), Some(txt("ell")));
        // start past the end ⇒ empty (total, never None for valid types).
        assert_eq!(compute("slice", &[&txt("hi"), &num(5.0), &num(2.0)], &length), Some(txt("")));
        assert_eq!(compute("index_of", &[&txt("hello"), &txt("ll")], &[]), Some(num(2.0)));
        assert_eq!(compute("index_of", &[&txt("hello"), &txt("z")], &[]), Some(num(-1.0)));
        // `slice` finds where its units begin and copies them at once: the code units `start..start
        // + len` exactly, as taking them one by one gives, in texts of one-, two-, three- and
        // four-byte chars, wherever the bounds fall; nothing where a bound cuts a pair.
        for s in ["", "hello", "ab\u{00e7}d", "\u{00e7}\u{20ac}x\u{1f600}y", "\u{1f600}\u{1f600}"] {
            let units: Vec<u16> = s.encode_utf16().collect();
            for start in 0..7 {
                for len in 0..7 {
                    let (from, to) = (start.min(units.len()), (start + len).min(units.len()));
                    let one_by_one = String::from_utf16(&units[from..to]).ok().map(|t| txt(&t));
                    assert_eq!(compute("slice", &[&txt(s), &num(start as f64), &num(len as f64)], &length), one_by_one, "{s:?} {start} {len}");
                }
            }
        }
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
    /// What the JavaScript-library races found (docs/engineering/themes/javascript-library.md §9):
    /// `-0 + -0` is `-0`, and arithmetic past the largest number gives nothing, not an infinity.
    #[test]
    fn a_sum_of_negative_zeros_is_negative_zero_and_an_overflow_is_nothing() {
        let bits = |v: Option<Value>| match v {
            Some(Value::Number(n)) => Some(n.to_bits()),
            _ => None,
        };
        assert_eq!(bits(compute("add", &[&num(-0.0), &num(-0.0)], &[])), Some((-0.0f64).to_bits()));
        assert_eq!(bits(compute("add", &[&num(-0.0), &num(-0.0), &num(-0.0)], &[])), Some((-0.0f64).to_bits()));
        assert_eq!(bits(compute("add", &[&num(-0.0), &num(0.0)], &[])), Some(0.0f64.to_bits()));
        assert_eq!(compute("add", &[&num(1.5), &num(2.0)], &[]), Some(num(3.5)));
        let max = f64::MAX;
        assert_eq!(compute("add", &[&num(max), &num(max)], &[]), None);
        assert_eq!(compute("subtract", &[&num(-max), &num(max)], &[]), None);
        assert_eq!(compute("multiply", &[&num(max), &num(2.0)], &[]), None);
        assert_eq!(compute("divide", &[&num(max), &num(0.5)], &[]), None);
        assert_eq!(compute("multiply", &[&num(1e200), &num(1e100)], &[]), Some(num(1e300)));
    }

    /// A number as text is JavaScript's `String(n)`: in full from 1e-6 up to 1e21, in exponent
    /// form outside, its shortest digits that read back as it.
    #[test]
    fn a_number_as_text_is_as_javascript_writes_it() {
        for (n, js) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (123.456, "123.456"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (-1e21, "-1e+21"),
            (1.5e300, "1.5e+300"),
            (9007199254740992.0, "9007199254740992"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.23e-18, "1.23e-18"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (123456789012345680000.0, "123456789012345680000"),
        ] {
            assert_eq!(number_text(n), js, "{n:e}");
            assert_eq!(compute("as_text", &[&num(n)], &[]), Some(txt(js)), "{n:e}");
            assert_eq!(crate::json::stringify(&list(vec![num(n)])), Some(format!("[{js}]")), "{n:e}");
        }
    }

    /// A number that isn't finite comes only from a module (a `js` function's `1 / 0`), and is
    /// written as JavaScript writes it, beside a text too: once a panic, on the run's thread.
    #[test]
    fn a_number_that_isnt_finite_is_written_as_javascript_writes_it() {
        for (n, js) in [(f64::NAN, "NaN"), (f64::INFINITY, "Infinity"), (f64::NEG_INFINITY, "-Infinity")] {
            assert_eq!(number_text(n), js);
            assert_eq!(crate::texts::plus(&[&txt("ratio: "), &num(n)]), Some(txt(&format!("ratio: {js}"))));
            // JSON has no form for one, and a sum that isn't finite is no number.
            assert_eq!(crate::json::stringify(&num(n)), None);
            assert_eq!(crate::texts::plus(&[&num(n), &num(1.0)]), None);
        }
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
        // An empty list is true, as JavaScript's `Boolean([])` is.
        assert_eq!(compute("is_truthy", &[&list(vec![])], &[]), Some(boo(true)));
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
    fn is_truthy_treats_nan_as_javascript_does() {
        assert_eq!(compute("is_truthy", &[&num(f64::NAN)], &[]), Some(boo(false)));
        assert_eq!(compute("is_truthy", &[&num(-0.0)], &[]), Some(boo(false)));
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
    fn flow_call_asks_for_the_action_with_what_it_sends() {
        let params = [p("flow", "billing"), p("action", "charge")];
        let sent = Value::Record([("amount".to_string(), Value::Number(5.0))].into());
        let Some(Value::Record(r)) = flow_call_request(&[Some(sent.clone())], &params, false) else { panic!() };
        assert_eq!((r["flow"].clone(), r["action"].clone(), r["input"].clone()), (txt("billing"), txt("charge"), sent));
        // Nothing wired sends `{}`; an Effect, or a name that isn't one, sends nothing.
        let Some(Value::Record(r)) = flow_call_request(&[None], &params, false) else { panic!() };
        assert_eq!(r["input"], Value::Record(BTreeMap::new()));
        let effect = Value::Effect { sink: "egress".into(), payload: Box::new(Value::Number(1.0)) };
        assert_eq!(flow_call_request(&[Some(effect)], &params, false), None);
        assert_eq!(flow_call_request(&[None], &[p("flow", "a b"), p("action", "x")], false), None);
        // A start says so; a call doesn't.
        let Some(Value::Record(r)) = flow_call_request(&[None], &params, true) else { panic!() };
        assert_eq!(r.get("start"), Some(&Value::Boolean(true)));
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
        assert_eq!(compute("add", &[&num(1.0), &boo(true)], &[]), None);
        assert_eq!(compute("not", &[&num(1.0)], &[]), None);
        assert_eq!(compute("sum", &[&list(vec![num(1.0), txt("x")])], &[]), None);
    }
    #[test]
    fn deterministic() {
        let owned = [num(3.0), num(7.0), num(5.0)];
        let inputs: Vec<&Value> = owned.iter().collect();
        assert_eq!(compute("add", &inputs, &[]), compute("add", &inputs, &[]));
    }

    /// Every leaf that builds on a given input computes what it computes lent, on every mix of
    /// given and lent inputs over values of every kind, the wrong ones included; and when it
    /// computes nothing, it hands back what it was given, unchanged and in order.
    #[test]
    fn given_inputs_compute_what_lent_ones_do() {
        let record = |fs: &[(&str, Value)]| Value::Record(fs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let pool = [
            num(0.0),
            num(1.0),
            num(-2.5),
            boo(true),
            boo(false),
            txt(""),
            txt("a"),
            txt("bé"),
            list(vec![]),
            list(vec![num(1.0), txt("a")]),
            list(vec![list(vec![num(1.0)]), num(2.0), num(3.0)]),
            record(&[]),
            record(&[("a", num(1.0)), ("b", txt("x"))]),
            Value::Sum { arm: 0, payload: Box::new(num(1.0)) },
            // As deep as a value may nest: a leaf that would put it down refuses it, given or lent.
            (0..MAX_VALUE_DEPTH).fold(num(1.0), |v, _| list(vec![v])),
        ];
        let params = [p("name", "a"), p("type", "Number"), p("value", "5"), p("mode", "end"), p("depth", "1")];
        let mut checked = 0;
        for &(name, op) in Op::ALL {
            if !(0..3).any(|port| keeps(op, port)) {
                continue;
            }
            for arity in 0..=3usize {
                let combos = pool.len().pow(arity as u32);
                for combo in 0..combos {
                    let inputs: Vec<&Value> = (0..arity).map(|i| &pool[combo / pool.len().pow(i as u32) % pool.len()]).collect();
                    let lent = compute_op(op, name, &inputs, &params);
                    for mask in 0..(1usize << arity) {
                        let given: Given = inputs.iter().enumerate().map(|(i, v)| if mask >> i & 1 == 1 { Cow::Owned((*v).clone()) } else { Cow::Borrowed(*v) }).collect();
                        match compute_given(op, name, given, &params) {
                            Ok(v) => assert_eq!(Some(&v), lent.as_ref(), "{name} on {inputs:?}, given {mask:b}"),
                            Err(back) => {
                                assert_eq!(lent, None, "{name} on {inputs:?}, given {mask:b}: computed nothing given");
                                let back: Vec<&Value> = back.iter().map(|v| v.as_ref()).collect();
                                assert_eq!(back, inputs, "{name}, given {mask:b}: what came back");
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100_000, "{checked}");
    }

    /// Each leaf that puts a value a level down builds nothing of one already as deep as a value
    /// may nest, lent or given (handing it back as it was), and builds one a level less as it
    /// always did, to exactly the bound: a fold that wraps its accumulator stops there.
    #[test]
    fn a_leaf_puts_nothing_past_the_depth_a_value_may_nest() {
        let nested = |depth: usize| (0..depth).fold(num(1.0), |v, _| list(vec![v]));
        let (at, below) = (nested(MAX_VALUE_DEPTH), nested(MAX_VALUE_DEPTH - 1));
        let record = Value::Record(BTreeMap::new());
        let one = || list(vec![num(0.0)]);
        // Each leaf, its inputs, its params, and the port of the value it puts down (an effect's
        // payload included).
        let cases: Vec<(&str, Vec<Value>, Vec<Param>, usize)> = vec![
            ("list", vec![num(0.0), num(0.0)], vec![], 1),
            ("pair", vec![num(0.0), num(0.0)], vec![], 0),
            ("some", vec![num(0.0)], vec![], 0),
            ("append", vec![one(), num(0.0)], vec![], 1),
            ("cons", vec![num(0.0), one()], vec![], 0),
            ("insert", vec![record.clone(), txt("k"), num(0.0)], vec![], 2),
            ("with_field", vec![record.clone(), num(0.0)], vec![p("name", "k")], 1),
            ("with_item", vec![one(), num(0.0), num(0.0)], vec![], 2),
            ("fill", vec![one(), num(0.0)], vec![], 1),
            ("store_write", vec![num(0.0)], vec![p("key", "k")], 0),
            ("channel_send", vec![num(0.0)], vec![p("channel", "c")], 0),
        ];
        for (name, inputs, params, port) in cases {
            let op = Op::of(name).unwrap();
            for (v, builds) in [(&at, false), (&below, true)] {
                let mut inputs = inputs.clone();
                inputs[port] = v.clone();
                let refs: Vec<&Value> = inputs.iter().collect();
                let lent = compute_op(op, name, &refs, &params);
                assert_eq!(lent.is_some(), builds, "{name}, {} levels put down", if builds { "255" } else { "256" });
                if let Some(out) = &lent {
                    assert!(nests_within(out, MAX_VALUE_DEPTH) && !nests_within(out, MAX_VALUE_DEPTH - 1), "{name} builds to the bound");
                }
                let given: Given = inputs.iter().map(|v| Cow::Owned(v.clone())).collect();
                match compute_given(op, name, given, &params) {
                    Ok(out) => assert_eq!(Some(out), lent, "{name} given"),
                    Err(back) => {
                        assert!(!builds, "{name} given builds nothing");
                        assert_eq!(back.iter().map(|v| v.as_ref()).collect::<Vec<_>>(), refs, "{name} hands back what it was given");
                    }
                }
            }
        }
        // `entries` puts every part of what it is handed a level down (`[["a", v]]`).
        let holding = |v: Value| Value::Record(BTreeMap::from([("a".to_string(), v)]));
        assert_eq!(compute("entries", &[&holding(below)], &[]), None);
        assert!(compute("entries", &[&holding(nested(MAX_VALUE_DEPTH - 2))], &[]).is_some_and(|out| nests_within(&out, MAX_VALUE_DEPTH)));
    }

    /// Every leaf, handed a value at the bound, or one level short of it, in one input and plain
    /// ones in the others (as many as the catalog gives it, its params as given there, an unset one
    /// set), builds nothing nested past the bound: a leaf that puts what it is handed a level down
    /// without `down` saying so, or two levels down, fails here.
    #[test]
    fn no_leaf_builds_a_value_nested_past_the_bound() {
        let nested = |depth: usize| (0..depth).fold(num(1.0), |v, _| list(vec![v]));
        let deep = [nested(MAX_VALUE_DEPTH), nested(MAX_VALUE_DEPTH - 1)];
        let plains = [txt("k"), num(0.0), num(1.0), list(vec![num(1.0)]), Value::Record(BTreeMap::from([("k".to_string(), num(1.0))]))];
        let mut built = std::collections::BTreeSet::new();
        for &(name, op) in Op::ALL {
            let Some(interface) = crate::interface_of(name) else { continue };
            let params: Vec<Param> = interface
                .params
                .iter()
                .map(|q| match q {
                    Param::Frozen { value, .. } if !value.is_empty() => q.clone(),
                    _ => p(q.name(), "k"),
                })
                .collect();
            let ports = interface.inputs.len();
            for (at, v) in (0..ports).flat_map(|at| deep.iter().map(move |v| (at, v))) {
                // Each of the other inputs each plain value in turn.
                for mut pick in 0..plains.len().pow(ports as u32 - 1) {
                    let inputs: Vec<&Value> = (0..ports)
                        .map(|i| {
                            if i == at {
                                return v;
                            }
                            let plain = &plains[pick % plains.len()];
                            pick /= plains.len();
                            plain
                        })
                        .collect();
                    if let Some(out) = compute_op(op, name, &inputs, &params) {
                        built.insert(name);
                        assert!(nests_within(&out, MAX_VALUE_DEPTH), "{name} builds past the bound from its input {at}");
                    }
                }
            }
        }
        // The leaves that put a value down are among those it reached.
        let down = ["list", "pair", "some", "cons", "append", "insert", "with_field", "with_item", "fill", "entries", "store_write", "channel_send", "egress"];
        assert!(down.iter().all(|n| built.contains(n)), "{:?} built nothing", down.iter().filter(|n| !built.contains(*n)).collect::<Vec<_>>());
    }
}