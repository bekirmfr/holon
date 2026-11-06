//! Lists: each gap of JavaScript's `Array.prototype`, `Array.*`, list access and spread, a
//! composite written in TypeScript against a native candidate
//! (docs/engineering/themes/js-library-races/lists.md, javascript-library.md §11 "Lists").
//!
//! A higher-order method (`filter`, `some`, `find`, …) races on a fixed predicate, `x > 10`, which
//! the candidate computes natively and the composite as a function of its own: the leaf would take
//! its function as `map` and `fold` do, by a `fn` param, and pay an application per item that the
//! candidate here doesn't. So each of those rows also races the composite against `map` of the
//! same function (`xs.map((x) => x > 10)`), the least a leaf that applies a function to every item
//! pays.
//!
//! `range(length(xs))` stands in for `indices(xs)`, the leaf proposed for a list's positions: the
//! same list, built the same way, without `range`'s cap of MAX_RANGE items (asserted below).
//!
//! A list a composite (or a function written in place) uses twice passes through a shared
//! `guard(true, xs)`, which copies it on every run (eval.rs, `run_planned`): the first race shows
//! that copy, and the composites here use each list once where they can.

mod race;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use holon_core::Value;
use race::{check, function, leaf, mapped, num, numbers, race, v, Composite, Side};

// ── values ───────────────────────────────────────────────────────────────────────────────────

fn list(v: &Value) -> Option<&[Value]> {
    if let Value::List(xs) = v { Some(xs) } else { None }
}

fn number(v: &Value) -> Option<f64> {
    if let Value::Number(x) = v { Some(*x) } else { None }
}

/// The predicate the higher-order rows race on.
fn gt10(x: &Value) -> bool {
    matches!(x, Value::Number(n) if *n > 10.0)
}

/// A call's arguments, from a JSON array of them: `args("[[1, 2], 0]")` is a list and a number.
fn args(json: &str) -> Vec<Value> {
    match v(json) {
        Value::List(xs) => xs,
        _ => panic!("{json}: the arguments are a JSON array"),
    }
}

fn cases(jsons: &[&str]) -> Vec<Vec<Value>> {
    jsons.iter().map(|j| args(j)).collect()
}

fn nums(xs: impl IntoIterator<Item = f64>) -> Value {
    Value::List(xs.into_iter().map(Value::Number).collect())
}

/// `n` items none of which is above 10: `k % 10`.
fn none_match(n: usize) -> Value {
    nums((0..n).map(|k| (k % 10) as f64))
}

/// As `none_match`, the first item above 10.
fn first_match(n: usize) -> Value {
    nums((0..n).map(|k| if k == 0 { 100.0 } else { (k % 10) as f64 }))
}

/// As `none_match`, the last item above 10.
fn last_match(n: usize) -> Value {
    nums((0..n).map(|k| if k + 1 == n { 100.0 } else { (k % 10) as f64 }))
}

/// `n` items all above 10.
fn all_match(n: usize) -> Value {
    nums((0..n).map(|k| (k + 11) as f64))
}

/// `n` items, numbers and lists in turn: `[0, [1, 1], 2, [3, 3], …]`.
fn mixed(n: usize) -> Value {
    Value::List((0..n).map(|k| if k % 2 == 0 { num(k as f64) } else { nums([k as f64, k as f64]) }).collect())
}

/// `n` items nested two deep: `[[[0], 0], [[1], 1], …]`.
fn deep(n: usize) -> Value {
    Value::List((0..n).map(|k| Value::List(vec![nums([k as f64]), num(k as f64)])).collect())
}

/// 64 lists of `n` numbers each: a scalar list operation mapped over them.
fn lists(n: usize) -> Value {
    Value::List(vec![numbers(n); 64])
}

/// `n` values of each kind in turn: a number, a list, a text, a record.
fn kinds(n: usize) -> Value {
    Value::List(
        (0..n)
            .map(|k| match k % 4 {
                0 => num(k as f64),
                1 => nums([k as f64]),
                2 => Value::Text(k.to_string()),
                _ => v("{\"length\": 1}"),
            })
            .collect(),
    )
}

/// JavaScript's relative index (`slice`, `fill`, `copyWithin`, `toSpliced`): truncated, from the
/// end when negative, clamped to `0..=len`.
fn relative(x: f64, len: usize) -> usize {
    let (x, len) = (x.trunc(), len as f64);
    (if x < 0.0 { (len + x).max(0.0) } else { x.min(len) }) as usize
}

// ── native candidates: each JavaScript's meaning on Holon's values ─────────────────────────────

/// `xs[i]`: an item at a whole, non-negative index in range (`-0` reads as `"0"`); else nothing.
fn js_index(a: &[&Value]) -> Option<Value> {
    let [l, i] = a else { return None };
    let (xs, i) = (list(l)?, number(i)?);
    (i >= 0.0 && i.fract() == 0.0 && i < xs.len() as f64).then(|| xs[i as usize].clone())
}

fn js_length(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(num(list(l)?.len() as f64))
}

/// `xs.at(i)`: `i` truncated, from the end when negative; out of range, nothing.
fn js_at(a: &[&Value]) -> Option<Value> {
    let [l, i] = a else { return None };
    let (xs, i) = (list(l)?, number(i)?.trunc());
    let k = if i < 0.0 { xs.len() as f64 + i } else { i };
    (k >= 0.0 && k < xs.len() as f64).then(|| xs[k as usize].clone())
}

fn js_at_minus_2(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    js_at(&[l, &num(-2.0)])
}

/// `xs.at(-1)`.
fn js_last(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    list(l)?.last().cloned()
}

/// `[...a, ...b]`, `a.concat(b)` with a list `b`.
fn js_concat(a: &[&Value]) -> Option<Value> {
    let [l, r] = a else { return None };
    let mut xs = list(l)?.to_vec();
    xs.extend_from_slice(list(r)?);
    Some(Value::List(xs))
}

/// `xs.copyWithin(target, start, end)`'s list: the items of `start..end` copied over those from
/// `target`, as from the list before the copy.
fn js_copy_within(a: &[&Value]) -> Option<Value> {
    let [l, t, s, e] = a else { return None };
    let xs = list(l)?;
    let n = xs.len();
    let (to, from, stop) = (relative(number(t)?, n), relative(number(s)?, n), relative(number(e)?, n));
    let count = stop.saturating_sub(from).min(n - to);
    let mut out = xs.to_vec();
    out[to..to + count].clone_from_slice(&xs[from..from + count]);
    Some(Value::List(out))
}

/// `[...xs.entries()]`: `[index, item]` pairs.
fn js_entries(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(Value::List(list(l)?.iter().enumerate().map(|(k, x)| Value::List(vec![num(k as f64), x.clone()])).collect()))
}

/// `[...xs.keys()]`.
fn js_keys(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(nums((0..list(l)?.len()).map(|k| k as f64)))
}

fn js_every(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(Value::Boolean(list(l)?.iter().all(gt10)))
}

fn js_some(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(Value::Boolean(list(l)?.iter().any(gt10)))
}

fn js_filter(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(Value::List(list(l)?.iter().filter(|x| gt10(x)).cloned().collect()))
}

fn js_find(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    list(l)?.iter().find(|x| gt10(x)).cloned()
}

fn js_find_index(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(num(list(l)?.iter().position(gt10).map_or(-1.0, |k| k as f64)))
}

fn js_find_last(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    list(l)?.iter().rev().find(|x| gt10(x)).cloned()
}

fn js_find_last_index(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(num(list(l)?.iter().rposition(gt10).map_or(-1.0, |k| k as f64)))
}

/// `xs.fill(v, start, end)`'s list.
fn js_fill(a: &[&Value]) -> Option<Value> {
    let [l, x, s, e] = a else { return None };
    let xs = list(l)?;
    let (from, stop) = (relative(number(s)?, xs.len()), relative(number(e)?, xs.len()));
    let mut out = xs.to_vec();
    for item in out.iter_mut().take(stop).skip(from) {
        *item = (*x).clone();
    }
    Some(Value::List(out))
}

/// FlattenIntoArray: a list item at a positive depth is spread, one level less deep; any other
/// item is kept.
fn flatten(xs: &[Value], depth: f64, out: &mut Vec<Value>) {
    for x in xs {
        match x {
            Value::List(ys) if depth > 0.0 => flatten(ys, depth - 1.0, out),
            _ => out.push(x.clone()),
        }
    }
}

fn js_flat(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    let mut out = Vec::new();
    flatten(list(l)?, 1.0, &mut out);
    Some(Value::List(out))
}

fn js_flat_depth(a: &[&Value]) -> Option<Value> {
    let [l, d] = a else { return None };
    let mut out = Vec::new();
    flatten(list(l)?, number(d)?.trunc().max(0.0), &mut out);
    Some(Value::List(out))
}

/// `xs.flatMap((x) => (x > 10 ? [x, x * 2] : x))`.
fn js_flat_map(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    let mut out = Vec::new();
    for x in list(l)? {
        match x {
            Value::Number(n) if *n > 10.0 => out.extend([num(*n), num(n * 2.0)]),
            _ => out.push(x.clone()),
        }
    }
    Some(Value::List(out))
}

/// `xs.includes(x)`: SameValueZero on numbers, texts and booleans (Holon has no `NaN`, and `-0`
/// equals `0`); records and lists by value, as `equals` compares them (JavaScript: by identity).
fn js_includes(a: &[&Value]) -> Option<Value> {
    let [l, x] = a else { return None };
    Some(Value::Boolean(list(l)?.iter().any(|y| y == *x)))
}

/// `xs.indexOf(x)`: strict equality, which on Holon's values is SameValueZero.
fn js_index_of(a: &[&Value]) -> Option<Value> {
    let [l, x] = a else { return None };
    Some(num(list(l)?.iter().position(|y| y == *x).map_or(-1.0, |k| k as f64)))
}

fn js_last_index_of(a: &[&Value]) -> Option<Value> {
    let [l, x] = a else { return None };
    Some(num(list(l)?.iter().rposition(|y| y == *x).map_or(-1.0, |k| k as f64)))
}

/// `xs.join(",")` over texts, booleans and whole numbers below 1e21 (where Rust writes a number
/// as JavaScript's `String(x)` does).
fn js_join_any(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    let mut out = String::new();
    for (k, x) in list(l)?.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        match x {
            Value::Text(s) => out.push_str(s),
            Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e21 => out.push_str(&format!("{}", if *n == 0.0 { 0.0 } else { *n })),
            _ => return None,
        }
    }
    Some(Value::Text(out))
}

/// `xs.join(sep)` over texts.
fn js_join(a: &[&Value]) -> Option<Value> {
    let [l, Value::Text(sep)] = a else { return None };
    let parts: Option<Vec<&str>> = list(l)?.iter().map(|x| if let Value::Text(s) = x { Some(s.as_str()) } else { None }).collect();
    Some(Value::Text(parts?.join(sep)))
}

/// `xs.map((x, i) => x + i)`.
fn js_map_indexed(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    list(l)?.iter().enumerate().map(|(k, x)| Some(num(number(x)? + k as f64))).collect::<Option<Vec<_>>>().map(Value::List)
}

/// `xs.reduce((a, b) => a + b)`: the first item is the start; an empty list throws in JavaScript,
/// nothing here.
fn js_reduce_no_init(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    let (first, rest) = list(l)?.split_first()?;
    let mut acc = number(first)?;
    for x in rest {
        acc += number(x)?;
    }
    Some(num(acc))
}

/// `xs.reduceRight((acc, x) => acc * 0.5 + x, init)`.
fn js_reduce_right(a: &[&Value]) -> Option<Value> {
    let [l, init] = a else { return None };
    let mut acc = number(init)?;
    for x in list(l)?.iter().rev() {
        acc = acc * 0.5 + number(x)?;
    }
    Some(num(acc))
}

fn js_to_reversed(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    Some(Value::List(list(l)?.iter().rev().cloned().collect()))
}

/// `xs.slice(start, end)`.
fn js_slice(a: &[&Value]) -> Option<Value> {
    let [l, s, e] = a else { return None };
    let xs = list(l)?;
    let (from, stop) = (relative(number(s)?, xs.len()), relative(number(e)?, xs.len()));
    Some(Value::List(if from < stop { xs[from..stop].to_vec() } else { vec![] }))
}

/// `xs.toSpliced(start, count, ...items)`, the items given as a list.
fn js_to_spliced(a: &[&Value]) -> Option<Value> {
    let [l, s, c, items] = a else { return None };
    let (xs, items) = (list(l)?, list(items)?);
    let from = relative(number(s)?, xs.len());
    let count = number(c)?.trunc().max(0.0).min((xs.len() - from) as f64) as usize;
    let mut out = Vec::with_capacity(xs.len() - count + items.len());
    out.extend_from_slice(&xs[..from]);
    out.extend_from_slice(items);
    out.extend_from_slice(&xs[from + count..]);
    Some(Value::List(out))
}

/// `xs.with(i, v)`: out of range throws a RangeError in JavaScript, nothing here.
fn js_with(a: &[&Value]) -> Option<Value> {
    let [l, i, x] = a else { return None };
    let (xs, i) = (list(l)?, number(i)?.trunc());
    let k = if i < 0.0 { xs.len() as f64 + i } else { i };
    if k < 0.0 || k >= xs.len() as f64 {
        return None;
    }
    let mut out = xs.to_vec();
    out[k as usize] = (*x).clone();
    Some(Value::List(out))
}

fn js_is_array(a: &[&Value]) -> Option<Value> {
    let [x] = a else { return None };
    Some(Value::Boolean(matches!(x, Value::List(_))))
}

/// A hash of a value that agrees with `==`: `-0` hashes as `0`.
fn hash_value(x: &Value, h: &mut impl Hasher) {
    match x {
        Value::Number(n) => (0u8, if *n == 0.0 { 0 } else { n.to_bits() }).hash(h),
        Value::Text(s) => (1u8, s).hash(h),
        Value::Boolean(b) => (2u8, b).hash(h),
        Value::List(xs) => {
            (3u8, xs.len()).hash(h);
            xs.iter().for_each(|x| hash_value(x, h));
        }
        Value::Record(fields) => {
            (4u8, fields.len()).hash(h);
            for (k, x) in fields {
                k.hash(h);
                hash_value(x, h);
            }
        }
        _ => 5u8.hash(h),
    }
}

/// `[...new Set(xs)]`: each item's first occurrence, in order. A hash of each item, compared by
/// `==` within a bucket, so O(n).
fn js_unique(a: &[&Value]) -> Option<Value> {
    let [l] = a else { return None };
    let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut out: Vec<Value> = Vec::new();
    for x in list(l)? {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        hash_value(x, &mut h);
        let bucket = seen.entry(h.finish()).or_default();
        if !bucket.iter().any(|&k| out[k] == *x) {
            bucket.push(out.len());
            out.push(x.clone());
        }
    }
    Some(Value::List(out))
}

// ── composites, in TypeScript ─────────────────────────────────────────────────────────────────

const USED_ONCE: &str = "import { length } from \"holon\"
function used_once(xs) {
  return length(xs) + 1
}";

const USED_TWICE: &str = "import { length, get } from \"holon\"
function used_twice(xs) {
  return length(xs) + get(xs, 0)
}";

/// JavaScript's relative index for whole numbers: from the end when negative, clamped to `0..=n`.
const CLAMP: &str = "function clamp(x, n) {
  return x < 0 ? (n + x < 0 ? 0 : n + x) : x > n ? n : x
}";

/// Whether a value is a list: `length` gives nothing for anything else, and `??` sees that.
const IS_LIST: &str = "function is_list(y) {
  return (length(y) ?? -1) >= 0
}";

const AT: &str = "import { length, get } from \"holon\"
function js_at(xs, i) {
  return i < 0 ? get(xs, length(xs) + i) : get(xs, i)
}
function at_minus_2(xs) {
  return js_at(xs, -2)
}";

const LAST: &str = "import { length, get } from \"holon\"
function js_last(xs) {
  return get(xs, length(xs) - 1)
}";

const KEYS: &str = "import { length, range } from \"holon\"
function js_keys(xs) {
  return range(length(xs))
}";

const ENTRIES: &str = "import { length, get, range } from \"holon\"
function js_entries(xs) {
  return range(length(xs)).map((i) => [i, get(xs, i)])
}";

const SOME: &str = "function js_some(xs) {
  return xs.reduce((acc, x) => acc || x > 10, false)
}";

const EVERY: &str = "function js_every(xs) {
  return xs.reduce((acc, x) => acc && x > 10, true)
}";

const FIND: &str = "import { head, is_empty } from \"holon\"
function js_find(xs) {
  return head(xs.reduce((acc, x) => (is_empty(acc) && x > 10 ? [x] : acc), []))
}";

/// The accumulator is the index once found, else `-(items seen) - 1`.
const FIND_INDEX: &str = "function js_find_index(xs) {
  const r = xs.reduce((a, x) => (a >= 0 ? a : x > 10 ? 0 - a - 1 : a - 1), -1)
  return r >= 0 ? r : -1
}";

const FIND_LAST: &str = "import { head } from \"holon\"
function js_find_last(xs) {
  return head(xs.reduce((acc, x) => (x > 10 ? [x] : acc), []))
}";

const FIND_LAST_INDEX: &str = "import { length, get, range } from \"holon\"
function js_find_last_index(xs) {
  return range(length(xs)).reduce((r, k) => (get(xs, k) > 10 ? k : r), -1)
}";

/// The list is used once, so only `extend`'s copy is paid, not the share's as well.
const FILTER: &str = "function js_filter(xs) {
  return xs.reduce((acc, x) => [...acc, ...(x > 10 ? [x] : [])], [])
}";

fn flat_map() -> String {
    format!(
        "import {{ length }} from \"holon\"\n{IS_LIST}\nfunction flat1(xs) {{\n  return xs.reduce((acc, y) => [...acc, ...(is_list(y) ? y : [y])], [])\n}}\nfunction js_flat_map(xs) {{\n  return flat1(xs.map((x) => (x > 10 ? [x, x * 2] : x)))\n}}"
    )
}

const MAP_INDEXED: &str = "import { length, get, range } from \"holon\"
function js_map_indexed(xs) {
  return range(length(xs)).map((i) => get(xs, i) + i)
}";

const REDUCE_NO_INIT: &str = "import { head, tail } from \"holon\"
function js_reduce_no_init(xs) {
  return tail(xs).reduce((a, b) => a + b, head(xs))
}";

const REDUCE_RIGHT: &str = "import { length, get, range } from \"holon\"
function js_reduce_right(xs, init) {
  const n = length(xs)
  return range(n).reduce((acc, k) => acc * 0.5 + get(xs, n - 1 - k), init)
}";

const INCLUDES: &str = "function js_includes(xs, x) {
  return xs.reduce((acc, y) => acc || y === x, false)
}";

const INDEX_OF: &str = "function js_index_of(xs, x) {
  const r = xs.reduce((a, y) => (a >= 0 ? a : y === x ? 0 - a - 1 : a - 1), -1)
  return r >= 0 ? r : -1
}";

const LAST_INDEX_OF: &str = "import { length, get, range } from \"holon\"
function js_last_index_of(xs, x) {
  return range(length(xs)).reduce((r, k) => (get(xs, k) === x ? k : r), -1)
}";

const TO_REVERSED: &str = "import { length, get, range } from \"holon\"
function js_to_reversed(xs) {
  const n = length(xs)
  return range(n).map((k) => get(xs, n - 1 - k))
}";

fn with_clamp(body: &str) -> String {
    format!("{CLAMP}\n{body}")
}

fn slice() -> String {
    with_clamp(
        "import { length, get, range } from \"holon\"
function js_slice(xs, start, end) {
  const n = length(xs)
  const a = clamp(start, n)
  const b = clamp(end, n)
  return range(b > a ? b - a : 0).map((k) => get(xs, a + k))
}",
    )
}

fn to_spliced() -> String {
    with_clamp(
        "import { length, get, range } from \"holon\"
function js_to_spliced(xs, start, count, items) {
  const n = length(xs)
  const s = clamp(start, n)
  const c = count < 0 ? 0 : count > n - s ? n - s : count
  const m = length(items)
  return range(n - c + m).map((k) => (k >= s && k < s + m ? get(items, k - s) : get(xs, k < s ? k : k - m + c)))
}",
    )
}

const WITH: &str = "import { length, get, range } from \"holon\"
function js_with(xs, i, x) {
  const n = length(xs)
  const j = i < 0 ? n + i : i
  return j >= 0 && j < n ? range(n).map((k) => (k === j ? x : get(xs, k))) : undefined
}";

fn fill() -> String {
    with_clamp(
        "import { length, get, range } from \"holon\"
function js_fill(xs, x, start, end) {
  const n = length(xs)
  const a = clamp(start, n)
  const b = clamp(end, n)
  return range(n).map((k) => (k >= a && k < b ? x : get(xs, k)))
}",
    )
}

fn copy_within() -> String {
    with_clamp(
        "import { length, get, range } from \"holon\"
function js_copy_within(xs, target, start, end) {
  const n = length(xs)
  const dst = clamp(target, n)
  const src = clamp(start, n)
  const stop = clamp(end, n)
  const count = stop - src < n - dst ? stop - src : n - dst
  return range(n).map((k) => get(xs, k >= dst && k < dst + count ? src + k - dst : k))
}",
    )
}

fn flat() -> String {
    format!(
        "import {{ length }} from \"holon\"\n{IS_LIST}\nfunction js_flat(xs) {{\n  return xs.reduce((acc, y) => [...acc, ...(is_list(y) ? y : [y])], [])\n}}"
    )
}

/// The step's index must be used (an unused parameter is refused), so it guards the step.
fn flat_depth() -> String {
    format!(
        "import {{ length, range }} from \"holon\"\n{IS_LIST}\nfunction flat1(xs) {{\n  return xs.reduce((acc, y) => [...acc, ...(is_list(y) ? y : [y])], [])\n}}\nfunction js_flat_depth(xs, d) {{\n  return range(d > 0 ? d : 0).reduce((acc, k) => (k >= 0 ? flat1(acc) : acc), xs)\n}}"
    )
}

const JOIN_ANY: &str = "import { as_text } from \"holon\"
function js_join_any(xs) {
  return xs.map((x) => as_text(x)).join(\",\")
}";

const IS_ARRAY: &str = "import { length } from \"holon\"
function js_is_array(x) {
  return (length(x) ?? -1) >= 0
}";

const UNIQUE: &str = "function js_unique(xs) {
  return xs.reduce((acc, x) => (acc.reduce((seen, y) => seen || y === x, false) ? acc : [...acc, x]), [])
}";

/// A function compiled under a label that says what it is.
fn labelled(text: &str, name: &str, arity: usize, label: &str) -> Composite {
    let mut c = function(text, name, arity);
    c.name = label.to_string();
    c
}

fn main() {
    let c = |text: &str, name: &str, arity: usize| function(text, name, arity);
    let linear = [8, 64, 512, 4096];
    let quadratic = [8, 64, 512];
    let half = |n: usize| num((n / 2) as f64);
    let past_range = numbers(10_001);

    // ── the share's copy: a list used twice is copied once per run ──
    let once = c(USED_ONCE, "used_once", 1);
    let twice = c(USED_TWICE, "used_twice", 1);
    race("a list used twice, against once", &Side::Composite(&twice), &Side::Composite(&once), &linear, |n| vec![numbers(n)]);

    // ── access ──
    // xs[i]: `get` is JavaScript's index exactly: a negative or fractional index is no array index
    // (undefined), `-0` reads as "0".
    let get = leaf("get", &[]);
    let mut index_cases = cases(&["[[], 0]", "[[1, 2, 3], 0]", "[[1, 2, 3], 2]", "[[1, 2, 3], 3]", "[[1, 2, 3], -1]", "[[1, 2, 3], 1.5]", "[[[1], {\"a\": 1}], 1]", "[[1, 2, 3], 4294967296]"]);
    index_cases.push(vec![v("[1, 2, 3]"), num(-0.0)]);
    check("xs[i]", &Side::Leaf(&get), &Side::Native(js_index), &index_cases);
    let length = leaf("length", &[]);
    check("xs.length", &Side::Leaf(&length), &Side::Native(js_length), &cases(&["[[]]", "[[1]]", "[[1, [2, 3], {}]]"]));

    // xs.at(i), whole indices (a fractional one needs Math.trunc): timed where it is hot, mapped
    // over 64 lists of n items.
    let at = c(AT, "js_at", 2);
    let mut at_cases = cases(&["[[], 0]", "[[], -1]", "[[1], 0]", "[[1, 2, 3], -1]", "[[1, 2, 3], -3]", "[[1, 2, 3], -4]", "[[1, 2, 3], 3]", "[[1, 2, 3], 2]", "[[[1], [2]], -1]"]);
    at_cases.push(vec![v("[1, 2, 3]"), num(-0.0)]);
    check("at", &Side::Composite(&at), &Side::Native(js_at), &at_cases);
    let at_m = mapped(AT, "at_minus_2");
    race("at(-2), mapped over 64 lists of n", &Side::Composite(&at_m), &Side::NativeMapped(js_at_minus_2), &linear, |n| vec![lists(n)]);
    let last = c(LAST, "js_last", 1);
    check("at(-1)", &Side::Composite(&last), &Side::Native(js_last), &cases(&["[[]]", "[[1]]", "[[1, 2, 3]]", "[[[1], [2]]]"]));
    let last_m = mapped(LAST, "js_last");
    race("at(-1), mapped over 64 lists of n", &Side::Composite(&last_m), &Side::NativeMapped(js_last), &linear, |n| vec![lists(n)]);

    // [...a, ...b] and a.concat(b) of lists: `extend`.
    let extend = leaf("extend", &[]);
    check("[...a, ...b]", &Side::Leaf(&extend), &Side::Native(js_concat), &cases(&["[[], []]", "[[1], []]", "[[], [1]]", "[[1, [2]], [[3], 4]]", "[[1, 1], [1]]"]));

    // ── positions ──
    let keys = c(KEYS, "js_keys", 1);
    check("keys", &Side::Composite(&keys), &Side::Native(js_keys), &cases(&["[[]]", "[[7]]", "[[1, [2], 3]]"]));
    assert!(keys.call(std::slice::from_ref(&past_range)).is_none() && js_keys(&[&past_range]).is_some(), "range(length(xs)) stops at MAX_RANGE");
    race("keys()", &Side::Composite(&keys), &Side::Native(js_keys), &linear, |n| vec![numbers(n)]);
    let entries = c(ENTRIES, "js_entries", 1);
    check("entries", &Side::Composite(&entries), &Side::Native(js_entries), &cases(&["[[]]", "[[7]]", "[[1, [2], {\"a\": 3}]]"]));
    race("entries()", &Side::Composite(&entries), &Side::Native(js_entries), &linear, |n| vec![numbers(n)]);

    // ── higher-order, on x > 10 ──
    let pred_floor = labelled("function map_pred(xs) {\n  return xs.map((x) => x > 10)\n}", "map_pred", 1, "xs.map((x) => x > 10)");
    let mut pred_cases = cases(&["[[]]", "[[5]]", "[[11]]", "[[11, 1, 2]]", "[[1, 2, 11]]", "[[1, 2, 3]]", "[[11, 12, 11]]", "[[10, 10.5, -11]]"]);
    pred_cases.push(vec![nums([-0.0, 0.0, 20.0])]);

    let some = c(SOME, "js_some", 1);
    check("some", &Side::Composite(&some), &Side::Native(js_some), &pred_cases);
    race("some, match first", &Side::Composite(&some), &Side::Native(js_some), &linear, |n| vec![first_match(n)]);
    race("some, no match", &Side::Composite(&some), &Side::Native(js_some), &linear, |n| vec![none_match(n)]);
    race("some, against map", &Side::Composite(&some), &Side::Composite(&pred_floor), &linear, |n| vec![none_match(n)]);

    let every = c(EVERY, "js_every", 1);
    check("every", &Side::Composite(&every), &Side::Native(js_every), &pred_cases);
    race("every, first fails", &Side::Composite(&every), &Side::Native(js_every), &linear, |n| vec![none_match(n)]);
    race("every, all pass", &Side::Composite(&every), &Side::Native(js_every), &linear, |n| vec![all_match(n)]);
    race("every, against map", &Side::Composite(&every), &Side::Composite(&pred_floor), &linear, |n| vec![all_match(n)]);

    let find = c(FIND, "js_find", 1);
    check("find", &Side::Composite(&find), &Side::Native(js_find), &pred_cases);
    race("find, match first", &Side::Composite(&find), &Side::Native(js_find), &linear, |n| vec![first_match(n)]);
    race("find, no match", &Side::Composite(&find), &Side::Native(js_find), &linear, |n| vec![none_match(n)]);
    race("find, against map", &Side::Composite(&find), &Side::Composite(&pred_floor), &linear, |n| vec![none_match(n)]);

    let find_index = c(FIND_INDEX, "js_find_index", 1);
    check("findIndex", &Side::Composite(&find_index), &Side::Native(js_find_index), &pred_cases);
    race("findIndex, match first", &Side::Composite(&find_index), &Side::Native(js_find_index), &linear, |n| vec![first_match(n)]);
    race("findIndex, no match", &Side::Composite(&find_index), &Side::Native(js_find_index), &linear, |n| vec![none_match(n)]);
    race("findIndex, against map", &Side::Composite(&find_index), &Side::Composite(&pred_floor), &linear, |n| vec![none_match(n)]);

    let find_last = c(FIND_LAST, "js_find_last", 1);
    check("findLast", &Side::Composite(&find_last), &Side::Native(js_find_last), &pred_cases);
    race("findLast, match last", &Side::Composite(&find_last), &Side::Native(js_find_last), &linear, |n| vec![last_match(n)]);
    race("findLast, no match", &Side::Composite(&find_last), &Side::Native(js_find_last), &linear, |n| vec![none_match(n)]);
    race("findLast, against map", &Side::Composite(&find_last), &Side::Composite(&pred_floor), &linear, |n| vec![none_match(n)]);

    let find_last_index = c(FIND_LAST_INDEX, "js_find_last_index", 1);
    check("findLastIndex", &Side::Composite(&find_last_index), &Side::Native(js_find_last_index), &pred_cases);
    race("findLastIndex, match last", &Side::Composite(&find_last_index), &Side::Native(js_find_last_index), &linear, |n| vec![last_match(n)]);
    race("findLastIndex, no match", &Side::Composite(&find_last_index), &Side::Native(js_find_last_index), &linear, |n| vec![none_match(n)]);
    race("findLastIndex, against map", &Side::Composite(&find_last_index), &Side::Composite(&pred_floor), &linear, |n| vec![none_match(n)]);

    let filter = c(FILTER, "js_filter", 1);
    check("filter", &Side::Composite(&filter), &Side::Native(js_filter), &pred_cases);
    race("filter, all kept", &Side::Composite(&filter), &Side::Native(js_filter), &quadratic, |n| vec![all_match(n)]);
    race("filter, against map", &Side::Composite(&filter), &Side::Composite(&pred_floor), &quadratic, |n| vec![all_match(n)]);

    let flat_map = c(&flat_map(), "js_flat_map", 1);
    let flat_map_floor = labelled("function map_f(xs) {\n  return xs.map((x) => (x > 10 ? [x, x * 2] : x))\n}", "map_f", 1, "xs.map((x) => (x > 10 ? [x, x * 2] : x))");
    check("flatMap", &Side::Composite(&flat_map), &Side::Native(js_flat_map), &pred_cases);
    race("flatMap", &Side::Composite(&flat_map), &Side::Native(js_flat_map), &quadratic, |n| vec![all_match(n)]);
    race("flatMap, against map", &Side::Composite(&flat_map), &Side::Composite(&flat_map_floor), &quadratic, |n| vec![all_match(n)]);

    let map_indexed = c(MAP_INDEXED, "js_map_indexed", 1);
    let map_floor = labelled("function map_plus(xs) {\n  return xs.map((x) => x + 1)\n}", "map_plus", 1, "xs.map((x) => x + 1)");
    check("map((x, i) => x + i)", &Side::Composite(&map_indexed), &Side::Native(js_map_indexed), &cases(&["[[]]", "[[5]]", "[[1, 2, 3]]", "[[-1, 0.5, 1e300]]"]));
    race("map((x, i) => x + i)", &Side::Composite(&map_indexed), &Side::Native(js_map_indexed), &linear, |n| vec![numbers(n)]);
    race("map((x, i) => x + i), against map", &Side::Composite(&map_indexed), &Side::Composite(&map_floor), &linear, |n| vec![numbers(n)]);

    let reduce_no_init = c(REDUCE_NO_INIT, "js_reduce_no_init", 1);
    let fold_floor = labelled("function fold_add(xs) {\n  return xs.reduce((a, b) => a + b, 0)\n}", "fold_add", 1, "xs.reduce((a, b) => a + b, 0)");
    let mut reduce_cases = cases(&["[[]]", "[[5]]", "[[1, 2, 3]]", "[[0.1, 0.2, 0.3]]"]);
    reduce_cases.push(vec![nums([-0.0])]);
    check("reduce, no start", &Side::Composite(&reduce_no_init), &Side::Native(js_reduce_no_init), &reduce_cases);
    race("reduce, no start", &Side::Composite(&reduce_no_init), &Side::Native(js_reduce_no_init), &linear, |n| vec![numbers(n)]);
    race("reduce, no start, against reduce with one", &Side::Composite(&reduce_no_init), &Side::Composite(&fold_floor), &linear, |n| vec![numbers(n)]);

    let reduce_right = c(REDUCE_RIGHT, "js_reduce_right", 2);
    let right_floor = labelled("function fold_half(xs, init) {\n  return xs.reduce((acc, x) => acc * 0.5 + x, init)\n}", "fold_half", 2, "xs.reduce((acc, x) => acc * 0.5 + x, init)");
    check("reduceRight", &Side::Composite(&reduce_right), &Side::Native(js_reduce_right), &cases(&["[[], 3]", "[[5], 3]", "[[1, 2, 3], 0]", "[[0.1, 7, -2.5], 1]"]));
    race("reduceRight", &Side::Composite(&reduce_right), &Side::Native(js_reduce_right), &linear, |n| vec![numbers(n), num(1.0)]);
    race("reduceRight, against reduce", &Side::Composite(&reduce_right), &Side::Composite(&right_floor), &linear, |n| vec![numbers(n), num(1.0)]);

    // ── searching by equality ──
    let mut search_cases = cases(&["[[], 1]", "[[1], 1]", "[[1], 2]", "[[1, 2, 3], 1]", "[[1, 2, 3], 3]", "[[1, 2, 3], 4]", "[[1, 2, 1, 2], 2]", "[[[1], [2]], [2]]", "[[{\"a\": 1}], {\"a\": 1}]", "[[\"1\", 1], 1]", "[[true, 1], 1]"]);
    search_cases.push(vec![nums([1.0, -0.0]), num(0.0)]);
    search_cases.push(vec![nums([0.0]), num(-0.0)]);

    let includes = c(INCLUDES, "js_includes", 2);
    check("includes", &Side::Composite(&includes), &Side::Native(js_includes), &search_cases);
    race("includes, match first", &Side::Composite(&includes), &Side::Native(js_includes), &linear, |n| vec![numbers(n), num(0.0)]);
    race("includes, no match", &Side::Composite(&includes), &Side::Native(js_includes), &linear, |n| vec![numbers(n), num(-1.0)]);

    let index_of = c(INDEX_OF, "js_index_of", 2);
    check("indexOf", &Side::Composite(&index_of), &Side::Native(js_index_of), &search_cases);
    race("indexOf, match first", &Side::Composite(&index_of), &Side::Native(js_index_of), &linear, |n| vec![numbers(n), num(0.0)]);
    race("indexOf, no match", &Side::Composite(&index_of), &Side::Native(js_index_of), &linear, |n| vec![numbers(n), num(-1.0)]);

    let last_index_of = c(LAST_INDEX_OF, "js_last_index_of", 2);
    check("lastIndexOf", &Side::Composite(&last_index_of), &Side::Native(js_last_index_of), &search_cases);
    race("lastIndexOf, match last", &Side::Composite(&last_index_of), &Side::Native(js_last_index_of), &linear, |n| vec![numbers(n), num((n - 1) as f64)]);
    race("lastIndexOf, no match", &Side::Composite(&last_index_of), &Side::Native(js_last_index_of), &linear, |n| vec![numbers(n), num(-1.0)]);

    // ── copies ──
    let to_reversed = c(TO_REVERSED, "js_to_reversed", 1);
    check("toReversed", &Side::Composite(&to_reversed), &Side::Native(js_to_reversed), &cases(&["[[]]", "[[1]]", "[[1, 2, 3]]", "[[[1], {\"a\": 2}, 2, 2]]"]));
    race("toReversed", &Side::Composite(&to_reversed), &Side::Native(js_to_reversed), &linear, |n| vec![numbers(n)]);

    let slice = c(&slice(), "js_slice", 3);
    check(
        "slice",
        &Side::Composite(&slice),
        &Side::Native(js_slice),
        &cases(&["[[], 0, 0]", "[[1], 0, 1]", "[[1, 2, 3], 1, 3]", "[[1, 2, 3], -2, 3]", "[[1, 2, 3], 0, -1]", "[[1, 2, 3], 2, 1]", "[[1, 2, 3], -9, 9]", "[[1, 2, 3], 3, 3]", "[[[1], [2], 3], 0, 2]"]),
    );
    race("slice(1, -1)", &Side::Composite(&slice), &Side::Native(js_slice), &linear, |n| vec![numbers(n), num(1.0), num(-1.0)]);

    let to_spliced = c(&to_spliced(), "js_to_spliced", 4);
    check(
        "toSpliced",
        &Side::Composite(&to_spliced),
        &Side::Native(js_to_spliced),
        &cases(&["[[], 0, 0, []]", "[[], 0, 0, [1]]", "[[1, 2, 3], 1, 1, [9, 9]]", "[[1, 2, 3], -1, 5, []]", "[[1, 2, 3], 0, -2, [0]]", "[[1, 2, 3], 3, 0, [4]]", "[[1, 2, 3], 9, 1, [[4]]]", "[[1, 2, 3], -9, 1, []]"]),
    );
    race("toSpliced(n/2, 2, 3 items)", &Side::Composite(&to_spliced), &Side::Native(js_to_spliced), &linear, |n| vec![numbers(n), half(n), num(2.0), v("[100, 101, 102]")]);

    let with = c(WITH, "js_with", 3);
    check("with", &Side::Composite(&with), &Side::Native(js_with), &cases(&["[[], 0, 1]", "[[1], 0, 9]", "[[1, 2, 3], -1, 9]", "[[1, 2, 3], 3, 9]", "[[1, 2, 3], -4, 9]", "[[1, 2, 3], 1, [9]]"]));
    race("with(n/2, 7)", &Side::Composite(&with), &Side::Native(js_with), &linear, |n| vec![numbers(n), half(n), num(7.0)]);

    let fill = c(&fill(), "js_fill", 4);
    check("fill", &Side::Composite(&fill), &Side::Native(js_fill), &cases(&["[[], 0, 0, 0]", "[[1, 2, 3], 0, 0, 3]", "[[1, 2, 3], 9, 1, 2]", "[[1, 2, 3], 9, -2, 9]", "[[1, 2, 3], [9], 2, 1]"]));
    race("fill(7, n/4, 3n/4)", &Side::Composite(&fill), &Side::Native(js_fill), &linear, |n| vec![numbers(n), num(7.0), num((n / 4) as f64), num((3 * n / 4) as f64)]);

    let copy_within = c(&copy_within(), "js_copy_within", 4);
    check(
        "copyWithin",
        &Side::Composite(&copy_within),
        &Side::Native(js_copy_within),
        &cases(&["[[], 0, 0, 0]", "[[1, 2, 3, 4, 5], 0, 3, 5]", "[[1, 2, 3, 4, 5], 1, 0, 5]", "[[1, 2, 3, 4, 5], 0, 1, 5]", "[[1, 2, 3, 4, 5], -2, 0, 9]", "[[1, 2, 3, 4, 5], 0, 4, 2]"]),
    );
    race("copyWithin(0, n/2, n)", &Side::Composite(&copy_within), &Side::Native(js_copy_within), &linear, |n| vec![numbers(n), num(0.0), half(n), num(n as f64)]);

    // ── flattening ──
    let flat = c(&flat(), "js_flat", 1);
    check("flat", &Side::Composite(&flat), &Side::Native(js_flat), &cases(&["[[]]", "[[1]]", "[[[1]]]", "[[1, [2, 3], [[4]], []]]", "[[{\"a\": [1]}, \"ab\", true]]"]));
    race("flat()", &Side::Composite(&flat), &Side::Native(js_flat), &quadratic, |n| vec![mixed(n)]);

    let flat_depth = c(&flat_depth(), "js_flat_depth", 2);
    check("flat(depth)", &Side::Composite(&flat_depth), &Side::Native(js_flat_depth), &cases(&["[[], 2]", "[[1, [2, [3, [4]]]], 0]", "[[1, [2, [3, [4]]]], 1]", "[[1, [2, [3, [4]]]], 2]", "[[1, [2, [3, [4]]]], 9]", "[[1, [2]], -1]"]));
    race("flat(2)", &Side::Composite(&flat_depth), &Side::Native(js_flat_depth), &quadratic, |n| vec![deep(n), num(2.0)]);

    // ── texts ──
    let join = leaf("join", &[]);
    check("join(sep)", &Side::Leaf(&join), &Side::Native(js_join), &cases(&["[[], \",\"]", "[[\"a\"], \",\"]", "[[\"a\", \"\", \"b\"], \"--\"]", "[[\"a\", \"b\"], \"\"]"]));
    let join_any = c(JOIN_ANY, "js_join_any", 1);
    check("join() of numbers", &Side::Composite(&join_any), &Side::Native(js_join_any), &cases(&["[[]]", "[[1]]", "[[1, -2, 300000]]", "[[\"a\", true, 0]]"]));
    race("join() of numbers", &Side::Composite(&join_any), &Side::Native(js_join_any), &linear, |n| vec![numbers(n)]);

    // ── kinds and uniqueness ──
    let is_array = c(IS_ARRAY, "js_is_array", 1);
    check("Array.isArray", &Side::Composite(&is_array), &Side::Native(js_is_array), &cases(&["[[]]", "[[1, 2]]", "[1]", "[\"[1]\"]", "[true]", "[{}]", "[{\"length\": 2}]"]));
    let is_array_m = mapped(IS_ARRAY, "js_is_array");
    race("Array.isArray, mapped", &Side::Composite(&is_array_m), &Side::NativeMapped(js_is_array), &[8, 64, 4096], |n| vec![kinds(n)]);

    let unique = c(UNIQUE, "js_unique", 1);
    check("new Set", &Side::Composite(&unique), &Side::Native(js_unique), &cases(&["[[]]", "[[1]]", "[[1, 1]]", "[[1, 2, 1, 3, 2]]", "[[[1], [1], {\"a\": 1}, {\"a\": 1}, \"1\", 1]]", "[[0, true, \"0\"]]"]));
    race("[...new Set(xs)]", &Side::Composite(&unique), &Side::Native(js_unique), &quadratic, |n| vec![numbers(n)]);

    // The composites over `range(length(xs))` give nothing past MAX_RANGE items, where a list can
    // be longer: `indices(xs)` is what makes them exact.
    for (name, composite, native) in [
        ("entries", &entries, js_entries as fn(&[&Value]) -> Option<Value>),
        ("map((x, i) => x + i)", &map_indexed, js_map_indexed),
        ("findLastIndex", &find_last_index, js_find_last_index),
    ] {
        assert!(composite.call(std::slice::from_ref(&past_range)).is_none() && native(&[&past_range]).is_some(), "{name} past MAX_RANGE");
    }
}
