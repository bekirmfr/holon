//! The list leaves: JavaScript's array methods on Holon's lists
//! (docs/engineering/themes/js-library-races/lists.md, "Leaf specs"). Each was a native candidate
//! in the races (`crates/holon-author/benches/races_lists.rs`), checked against Boa there.
//!
//! An index is truncated toward zero (ToIntegerOrInfinity; `-0` is `0`), and a relative one counts
//! from the end when negative. Items are compared by `equals`, by value: SameValueZero on numbers,
//! texts and booleans (Holon has no `NaN`, and `-0` equals `0`), and records and lists by what they
//! hold, where JavaScript compares objects by identity. None computes a float beyond comparing and
//! counting, so each gives the same bits natively and in wasm.
//!
//! The higher-order leaves (`any`, `all`, `filter`, the `find` family and `sort_by`) apply their
//! function in the evaluator, where `map` and `fold` apply theirs; what they share with the
//! first-order leaves is here: JavaScript's ToBoolean, and the merge sort.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::numbers::js_string;
use crate::Value;

/// A relative index (`slice`, `fill`, `copyWithin`, `toSpliced`): truncated, from the end when
/// negative, clamped to `0..=len`.
pub(crate) fn relative(x: f64, len: usize) -> usize {
    let (x, len) = (x.trunc(), len as f64);
    (if x < 0.0 { (len + x).max(0.0) } else { x.min(len) }) as usize
}

/// The position `at` and `with_item` read: `i` truncated, from the end when negative; none out of
/// range.
pub(crate) fn position(i: f64, len: usize) -> Option<usize> {
    let i = i.trunc();
    let k = if i < 0.0 { len as f64 + i } else { i };
    (k >= 0.0 && k < len as f64).then_some(k as usize)
}

/// `[...xs.keys()]`: `[0, 1, …, len - 1]`.
pub(crate) fn indices(len: usize) -> Value {
    Value::List((0..len).map(|k| Value::Number(k as f64)).collect())
}

/// `[...x.entries()]`: a list's `[index, item]` pairs, or a record's `[key, value]` pairs in key
/// order (`Object.entries`).
pub(crate) fn entries(x: &Value) -> Option<Value> {
    let pair = |a: Value, b: &Value| Value::List(vec![a, b.clone()]);
    Some(Value::List(match x {
        Value::List(xs) => xs.iter().enumerate().map(|(k, x)| pair(Value::Number(k as f64), x)).collect(),
        Value::Record(fields) => fields.iter().map(|(k, v)| pair(Value::Text(k.clone()), v)).collect(),
        _ => return None,
    }))
}

/// `xs.indexOf(x, from)`: the first position at or after relative `from` (0 when unwired) whose
/// item equals `x`, or `-1`.
pub(crate) fn index_of(xs: &[Value], x: &Value, from: Option<f64>) -> Value {
    let k = from.map_or(0, |f| relative(f, xs.len()));
    Value::Number(xs[k..].iter().position(|y| y == x).map_or(-1.0, |i| (k + i) as f64))
}

/// `xs.includes(x, from)`: whether an item at or after relative `from` equals `x`.
pub(crate) fn includes(xs: &[Value], x: &Value, from: Option<f64>) -> Value {
    Value::Boolean(xs[from.map_or(0, |f| relative(f, xs.len()))..].contains(x))
}

/// `xs.lastIndexOf(x, from)`: the last position at or before `from` (from the end when negative;
/// the last item when unwired) whose item equals `x`, scanning back, or `-1`.
pub(crate) fn last_index_of(xs: &[Value], x: &Value, from: Option<f64>) -> Value {
    let len = xs.len();
    let end = match from.map(f64::trunc) {
        None => len,
        Some(n) if n >= len as f64 => len,
        Some(n) if n >= 0.0 => n as usize + 1,
        Some(n) => (len as f64 + n + 1.0).max(0.0) as usize,
    };
    Value::Number(xs[..end].iter().rposition(|y| y == x).map_or(-1.0, |k| k as f64))
}

/// The range `slice {mode: "end"}` keeps: from relative `start` up to relative `end` (the length
/// when unwired), empty when `end` is at or before `start`.
pub(crate) fn slice_range(len: usize, start: f64, end: Option<f64>) -> std::ops::Range<usize> {
    let from = relative(start, len);
    let stop = end.map_or(len, |e| relative(e, len));
    from..stop.max(from)
}

/// The range `slice {mode: "length"}` keeps: `count` items from `start`, both whole and not
/// negative, cut short at the end.
pub(crate) fn length_range(len: usize, start: usize, count: usize) -> std::ops::Range<usize> {
    let from = start.min(len);
    from..from + count.min(len - from)
}

/// `xs.toSpliced(start, count, ...items)`: `count` items (truncated, clamped to what is left; the
/// rest when unwired) from relative `start` replaced by `items`.
pub(crate) fn to_spliced(mut xs: Vec<Value>, start: f64, count: Option<f64>, items: Vec<Value>) -> Vec<Value> {
    let from = relative(start, xs.len());
    let left = xs.len() - from;
    let count = count.map_or(left, |c| c.trunc().clamp(0.0, left as f64) as usize);
    xs.splice(from..from + count, items);
    xs
}

/// `xs.with(i, v)`: the item at position `i` replaced; none out of range (a RangeError in
/// JavaScript).
pub(crate) fn with_item(mut xs: Vec<Value>, i: f64, v: Value) -> Option<Vec<Value>> {
    let k = position(i, xs.len())?;
    xs[k] = v;
    Some(xs)
}

/// `xs.fill(v, start, end)`: the positions from relative `start` to relative `end` (the length when
/// unwired) set to `v`.
pub(crate) fn fill(mut xs: Vec<Value>, v: &Value, start: f64, end: Option<f64>) -> Vec<Value> {
    let range = slice_range(xs.len(), start, end);
    xs[range].fill(v.clone());
    xs
}

/// `xs.copyWithin(target, start, end)`: the items of relative `start..end` (`end` the length when
/// unwired) copied over those from relative `target`, as they were before the copy.
pub(crate) fn copy_within(mut xs: Vec<Value>, target: f64, start: f64, end: Option<f64>) -> Vec<Value> {
    let n = xs.len();
    let (to, from) = (relative(target, n), relative(start, n));
    let stop = end.map_or(n, |e| relative(e, n));
    let count = stop.saturating_sub(from).min(n - to);
    let copied = xs[from..from + count].to_vec();
    xs[to..to + count].clone_from_slice(&copied);
    xs
}

/// `xs.toSorted()`: stable, by JavaScript's default order, each item's `String(x)` compared by
/// UTF-16 code units (so an astral character sorts before U+E000–U+FFFF, where Rust's `str` order
/// is by code point). The keys are a total order, so the standard library's stable sort is
/// deterministic here, unlike with a comparator a flow supplies (`sort_by`, [`merge_sort`]).
pub(crate) fn sort(xs: Vec<Value>) -> Vec<Value> {
    let mut keyed: Vec<(Vec<u16>, Value)> = xs.into_iter().map(|x| (js_string(&x).encode_utf16().collect(), x)).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.into_iter().map(|(_, x)| x).collect()
}

/// `flat`'s `depth` param: a whole number (truncated, at least 0), or `all` for
/// `flat(Infinity)`. Anything else is none.
pub(crate) fn flat_depth(depth: &str) -> Option<f64> {
    if depth == "all" {
        return Some(f64::INFINITY);
    }
    let d: f64 = depth.parse().ok()?;
    d.is_finite().then(|| d.trunc().max(0.0))
}

/// FlattenIntoArray: an item that is a list, at a depth above 0, is spread one level less deep;
/// any other item is kept.
pub(crate) fn flat(xs: Vec<Value>, depth: f64) -> Vec<Value> {
    fn spread(xs: Vec<Value>, depth: f64, out: &mut Vec<Value>) {
        for x in xs {
            match x {
                Value::List(ys) if depth > 0.0 => spread(ys, depth - 1.0, out),
                x => out.push(x),
            }
        }
    }
    let mut out = Vec::with_capacity(xs.len());
    spread(xs, depth, &mut out);
    out
}

/// `[...new Set(xs)]`: each item's first occurrence, in order. Each item is hashed
/// ([`hash_item`]) and compared by `==` only with the kept items in its bucket, so it is O(n).
pub(crate) fn unique(xs: Vec<Value>) -> Vec<Value> {
    let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut out: Vec<Value> = Vec::with_capacity(xs.len());
    for x in xs {
        let mut h = DefaultHasher::new();
        hash_item(&x, &mut h);
        let bucket = seen.entry(h.finish()).or_default();
        if !bucket.iter().any(|&k| out[k] == x) {
            bucket.push(out.len());
            out.push(x);
        }
    }
    out
}

/// A hash of an item that agrees with `==`: `-0` hashes as `0`. Only which items share a bucket
/// depends on it, never the answer, so the hasher may differ between builds.
pub(crate) fn hash_item(x: &Value, h: &mut impl Hasher) {
    match x {
        Value::Number(n) => (0u8, if *n == 0.0 { 0 } else { n.to_bits() }).hash(h),
        Value::Text(s) => (1u8, s).hash(h),
        Value::Boolean(b) => (2u8, b).hash(h),
        Value::List(xs) => {
            (3u8, xs.len()).hash(h);
            xs.iter().for_each(|x| hash_item(x, h));
        }
        Value::Record(fields) => {
            (4u8, fields.len()).hash(h);
            for (k, x) in fields {
                k.hash(h);
                hash_item(x, h);
            }
        }
        // The payload too, or a list of distinct `some(x)` shares one bucket and is compared item
        // by item, every pair.
        Value::Sum { arm, payload } => {
            (5u8, arm).hash(h);
            hash_item(payload, h);
        }
        Value::Effect { sink, payload } => {
            (6u8, sink).hash(h);
            hash_item(payload, h);
        }
    }
}

/// JavaScript's ToBoolean, how a higher-order leaf reads its function's answer: `false`, `0`, `-0`
/// and `""` are false, anything else is true, an empty list or record included (unlike
/// `is_truthy`).
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Boolean(b) => *b,
        Value::Number(n) => *n != 0.0,
        Value::Text(s) => !s.is_empty(),
        _ => true,
    }
}

/// The order of `0..n` that `sort_by` gives: a stable merge sort, bottom up, pinned here rather
/// than the standard library's, whose algorithm can change between versions and which may panic
/// on a comparator that isn't a total order. `after(a, b)` says whether item `b` goes before item
/// `a` (the comparator answered above 0); a tie keeps the earlier item first. The comparisons are
/// made in one fixed sequence, so any comparator, consistent or not, gives one deterministic
/// order. The first `Err` stops the sort and is its answer.
pub(crate) fn merge_sort<E>(n: usize, mut after: impl FnMut(usize, usize) -> Result<bool, E>) -> Result<Vec<usize>, E> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut merged: Vec<usize> = Vec::with_capacity(n);
    let mut width = 1;
    while width < n {
        merged.clear();
        let mut lo = 0;
        while lo < n {
            let mid = lo.saturating_add(width).min(n);
            let hi = mid.saturating_add(width).min(n);
            let (mut i, mut j) = (lo, mid);
            while i < mid && j < hi {
                if after(order[i], order[j])? {
                    merged.push(order[j]);
                    j += 1;
                } else {
                    merged.push(order[i]);
                    i += 1;
                }
            }
            merged.extend_from_slice(&order[i..mid]);
            merged.extend_from_slice(&order[j..hi]);
            lo = hi;
        }
        std::mem::swap(&mut order, &mut merged);
        width = width.saturating_mul(2);
    }
    Ok(order)
}
